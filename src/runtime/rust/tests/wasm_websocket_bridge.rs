//! Browser-run proof of the M4 `Ipe.WebSocket` client substitute's Sub-tier
//! receive surface (`sub_subscribe_ws_open`/`_message`/`_close`): the
//! Layer-1 wasm gate used to deny `Sub_subscribeWebSocket` because it had
//! no wasm32 runtime symbol; this file is the headless-browser
//! (`wasm-bindgen-test`, real Chromium via `wasm-bindgen-test-runner` +
//! `chromedriver`) proof that the symbol it now has actually works, not
//! just compiles.
//!
//! Needs a live counterparty: `examples/33-websocket-echo`'s native server
//! running on `127.0.0.1:8033` (echoes every text frame prefixed
//! `"echo: "`) — start it before running this file, e.g.:
//!
//! ```sh
//! (cd examples/33-websocket-echo && ./sky-out/app &)
//! CHROMEDRIVER=chromedriver cargo test --target wasm32-unknown-unknown \
//!     --features wasm-client --test wasm_websocket_bridge
//! ```
//!
//! A real browser socket to a real server (not a mock) is the point: it
//! proves `web_sys::WebSocket`'s `onopen`/`onmessage`/`onclose` handler
//! wiring in `ws_client.rs`'s wasm32 arm actually round-trips a frame,
//! which no native-target test can exercise.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use ipe_runtime_rust::tea::IpeSub;
use ipe_runtime_rust::ws_client::{web_socket_close, web_socket_connect, web_socket_send};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const ECHO_URL: &str = "ws://127.0.0.1:8033/ws";

/// Poll `IpeSub::Source`'s teardown-thunk shape by hand (no TEA scheduler in
/// this test — driving the Source directly is the narrowest proof of the
/// exact runtime fn the M1 gate now allows).
fn drive_source<M: 'static>(sub: IpeSub<M>, emit: Rc<dyn Fn(M)>) -> Box<dyn FnOnce()> {
    match sub {
        IpeSub::Source(spawn) => spawn(emit),
        _ => panic!("test bug: expected IpeSub::Source"),
    }
}

/// Busy-poll-with-yield until `pred` is true or `attempts` is exhausted —
/// `wasm_bindgen_test`'s async support has no timer primitive of its own, so
/// this drives the microtask queue forward via `gloo_timers`-free
/// `wasm_bindgen_futures::JsFuture` yields against a zero-length promise.
async fn wait_until(mut attempts: u32, mut pred: impl FnMut() -> bool) -> bool {
    while attempts > 0 {
        if pred() {
            return true;
        }
        yield_to_browser().await;
        attempts -= 1;
    }
    pred()
}

async fn yield_to_browser() {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        // `wasm_bindgen_test_configure!(run_in_browser)` guarantees a real
        // `Window` global for every test in this file — `None` here would be
        // a harness bug, not a runtime condition this test recovers from, so
        // a `match`-panic (not `.expect()`, which the crate's
        // `clippy::expect_used = "deny"` scope covers even in tests) states
        // that plainly.
        match web_sys::window() {
            Some(window) => {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 20);
            }
            None => panic!("wasm-bindgen-test harness bug: no Window in a run_in_browser test"),
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

#[wasm_bindgen_test]
async fn onopen_onmessage_onclose_round_trip_against_a_live_server() {
    // Task-tier connect — the M4 substitute already tagged `WasmClient`
    // before this change; unaffected here except as the socket this test's
    // new Sub-tier coverage subscribes against.
    let socket_id = web_socket_connect::<String>(ECHO_URL.to_owned())
        .await
        .expect_ok("WebSocket.connect against the live echo server must resolve");

    // ── onOpen: the one-shot Sub-tier receive kernel this commit adds ──────
    let opened = Rc::new(RefCell::new(false));
    let opened_w = Rc::clone(&opened);
    let open_emit: Rc<dyn Fn(())> = Rc::new(move |()| *opened_w.borrow_mut() = true);
    let open_teardown =
        drive_source(ipe_runtime_rust::ws_client::sub_subscribe_ws_open(socket_id, ()), open_emit);
    assert!(
        wait_until(100, || *opened.borrow()).await,
        "onOpen never fired against a live socket"
    );

    // ── onMessage: send a frame, expect the echo server's "echo: " prefix ──
    let received: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let received_w = Rc::clone(&received);
    let msg_emit: Rc<dyn Fn(String)> = Rc::new(move |s: String| received_w.borrow_mut().push(s));
    let message_teardown = drive_source(
        ipe_runtime_rust::ws_client::sub_subscribe_ws_message(socket_id, |m| match m {
            ipe_runtime_rust::ws_client::WsClientMessage::Text(t) => t,
            ipe_runtime_rust::ws_client::WsClientMessage::Binary(_) => {
                "<binary — unexpected in this test>".to_owned()
            }
        }),
        msg_emit,
    );

    web_socket_send::<String>(socket_id, "hello-from-wasm-bindgen-test".to_owned())
        .await
        .expect_ok("WebSocket.send on an open socket must resolve");

    assert!(
        wait_until(100, || received
            .borrow()
            .iter()
            .any(|m| m == "echo: hello-from-wasm-bindgen-test"))
        .await,
        "onMessage never delivered the server's echo (got {:?})",
        received.borrow()
    );

    // ── onClose: closing from this side must still surface a close event ──
    let closed = Rc::new(RefCell::new(false));
    let closed_w = Rc::clone(&closed);
    let close_emit: Rc<dyn Fn(ipe_runtime_rust::ws_client::WsCloseCode)> =
        Rc::new(move |_code| *closed_w.borrow_mut() = true);
    let close_teardown = drive_source(
        ipe_runtime_rust::ws_client::sub_subscribe_ws_close(socket_id, |code| code),
        close_emit,
    );

    web_socket_close::<String>(socket_id)
        .await
        .expect_ok("WebSocket.close must resolve");

    assert!(
        wait_until(100, || *closed.borrow()).await,
        "onClose never fired after WebSocket.close"
    );

    open_teardown();
    message_teardown();
    close_teardown();
}

/// Local `Result`-unwrap helper: this crate's `IpeResult` has no
/// `std::result`-shaped `.expect`, and pulling in `unwrap_used = "deny"`
/// clippy scope means a raw `match` reads clearer at each call site than a
/// borrowed trait impl for a two-call-site test helper.
trait ExpectOk<T> {
    fn expect_ok(self, msg: &str) -> T;
}

impl<E: std::fmt::Debug, T> ExpectOk<T> for ipe_runtime_rust::core::IpeResult<E, T> {
    fn expect_ok(self, msg: &str) -> T {
        match self {
            ipe_runtime_rust::core::IpeResult::Ok(v) => v,
            ipe_runtime_rust::core::IpeResult::Err(e) => {
                panic!("{msg}: {e:?}");
            }
        }
    }
}

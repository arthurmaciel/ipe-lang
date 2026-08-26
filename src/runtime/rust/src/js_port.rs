//! Ipe.Js ports — the raw typed Ipê↔JS transport behind `Js.send` /
//! `Js.subscribe`.
//!
//! A port carries a plain, seal-legal value across the boundary between an Ipê
//! program and hand-written browser JavaScript. The two directions are:
//!
//! * **Outbound (`js_send : a -> Cmd msg`).** The payload is canonically
//!   seal-encoded to one wire string ([`crate::seal_codec::seal_encode`]) and
//!   handed to the browser out-channel. It is fire-and-forget, so it reuses the
//!   existing [`IpeCmd::Publish`] primitive (a `FnOnce(&str) -> i64` the dispatch
//!   loop runs); no new `IpeCmd` variant is introduced.
//! * **Inbound (`js_subscribe : Decoder a -> (a -> msg) -> Sub msg`).** Each raw
//!   string arriving from the browser in-channel is decoded fail-closed through
//!   the SAME bounded seal decoder every crossing uses
//!   ([`crate::seal_codec::seal_decode`]); a clean decode emits `to_msg(a)`, a
//!   rejected payload is DROPPED whole — no panic, no partial value. This is the
//!   same discipline as the `Ui.widget` up-event decode.
//!
//! The transport is process-/tab-local and cfg-split, mirroring `ws_client.rs`
//! and the pub/sub broker:
//!
//! * **Native (server).** The transport is keyed PER SESSION, never process-global.
//!   Each authenticated session owns one inbound broadcast channel and one outbound
//!   sink, held in a registry keyed by the session's sid. A `js_subscribe` reads the
//!   owning session's sid from the same task-local scope the pub/sub broker uses
//!   (`pubsub::current_session_sid`, read synchronously while the driver's
//!   `with_session_sid` scope is active) and drains ONLY that session's inbound
//!   channel; a `js_send` delivers ONLY to the origin session's outbound sink (the
//!   origin sid the dispatch loop supplies). The server's inbound route
//!   (`/_ipe/port`) authenticates by session cookie + CSRF, decodes the body
//!   fail-closed through the bounded seal budget, and delivers to that sid's inbound
//!   channel alone.
//!
//!   Cross-session delivery is UNREPRESENTABLE: there is no process-global port
//!   channel and no API to send to a port by name across sessions — a channel handle
//!   is only ever obtained from a sid. A caller with no session sid in scope
//!   (sid == "") subscribes to an inert per-call channel that no route ever feeds,
//!   so an unscoped subscription receives nothing rather than another session's
//!   traffic. Channels are created on session start (`session_open`) and dropped on
//!   session end/eviction (`session_close`), reusing the session store's lifecycle.
//! * **Wasm (`feature = "wasm-client"`).** In-process, no network. Outbound calls
//!   the browser-registered `window.ipeOnReceive` handler with the seal-encoded
//!   string; inbound is fed by the browser's `window.ipe.send(...)` pushing a
//!   string into an in-tab queue the `Source` drains.

use crate::seal_codec::{SealLimits, seal_encode};
use crate::tea::{IpeCmd, IpeSub};

// ─── Native (server) transport ─────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use crate::error::IpeError;
    use crate::json::Decoder;
    use crate::seal_codec::seal_decode;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    use tokio::sync::broadcast;

    /// Broadcast buffer for one direction. A lagging subscriber drops the gap
    /// (never panics); the size bounds the queue an inbound burst can hold.
    const PORT_CAP: usize = 256;

    /// A registered out-sink for one session: the server installs it so each
    /// outbound `js_send` frame for that session is pushed to that session's
    /// browser over its own SSE connection.
    type OutSink = Arc<dyn Fn(&str) + Send + Sync>;

    /// One session's port endpoints. The inbound channel carries browser→server
    /// frames (already through the fail-closed decode gate) to that session's
    /// `js_subscribe`; `out_sink` is that session's browser-push installed by the
    /// live server. Both belong to exactly one sid — there is no shared channel.
    struct SessionPorts {
        inbound: broadcast::Sender<String>,
        out_sink: Option<OutSink>,
    }

    impl SessionPorts {
        fn new() -> Self {
            SessionPorts {
                inbound: broadcast::channel(PORT_CAP).0,
                out_sink: None,
            }
        }
    }

    /// The per-session port registry, keyed by session sid. A channel handle is
    /// ONLY ever obtained by looking up a sid here; there is no process-global
    /// channel, so no `js_send`/`js_subscribe`/inbound-route can reach a session
    /// other than the one whose sid it holds — cross-session delivery is
    /// unrepresentable.
    fn sessions() -> &'static Mutex<HashMap<String, SessionPorts>> {
        static S: OnceLock<Mutex<HashMap<String, SessionPorts>>> = OnceLock::new();
        S.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn lock_sessions() -> std::sync::MutexGuard<'static, HashMap<String, SessionPorts>> {
        // Poison-tolerant: a panic in one session's callback must not wedge the
        // whole registry; the map is still valid data.
        sessions().lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The sid of the session whose subscriptions/commands are being materialised
    /// in the current scope. Under the `web` server this reads the same task-local
    /// the pub/sub broker sets (via `pubsub::with_session_sid`), so a `js_subscribe`
    /// binds to the OWNING session. Outside a live server (e.g. a `tokio`-only test
    /// build with no `web` feature) there is no session scope, so it is empty and
    /// callers fall back to an explicit sid.
    #[cfg(feature = "web")]
    fn scope_sid() -> String {
        crate::web::pubsub::current_session_sid()
    }
    #[cfg(not(feature = "web"))]
    fn scope_sid() -> String {
        String::new()
    }

    /// Create a session's port endpoints on session start (idempotent). Called by
    /// the live server when it creates the session, so the inbound channel exists
    /// before the browser can POST to it.
    pub fn session_open(sid: &str) {
        lock_sessions()
            .entry(sid.to_string())
            .or_insert_with(SessionPorts::new);
    }

    /// Drop a session's port endpoints on session end/eviction. Any live
    /// `js_subscribe` receiver on the dropped inbound channel observes `Closed`
    /// and ends; no dead-session channel lingers.
    pub fn session_close(sid: &str) {
        lock_sessions().remove(sid);
    }

    /// Install `sid`'s browser-push sink. Called when the live server attaches the
    /// session's client transport; every subsequent `js_send` whose origin is
    /// `sid` is delivered to `sink`. Creates the session entry if the sink is
    /// wired before `session_open` (idempotent).
    pub fn register_out_sink_for(sid: &str, sink: OutSink) {
        let mut g = lock_sessions();
        g.entry(sid.to_string())
            .or_insert_with(SessionPorts::new)
            .out_sink = Some(sink);
    }

    /// Feed one raw inbound string to `sid`'s active `js_subscribe`s. Called by
    /// the server's inbound port route AFTER its session-cookie + CSRF +
    /// bounded-seal checks have accepted the body. Delivery reaches ONLY `sid`;
    /// an unknown/closed session is a fire-and-forget no-op (never an error).
    pub fn deliver_inbound_for(sid: &str, raw: String) {
        let sender = lock_sessions().get(sid).map(|p| p.inbound.clone());
        if let Some(tx) = sender {
            let _ = tx.send(raw);
        }
    }

    /// `js_send payload` — seal-encode `payload` and deliver it to the ORIGIN
    /// session's browser out-sink. Fire-and-forget via [`IpeCmd::Publish`]; the
    /// dispatch loop supplies the origin sid (the same sid it injects for
    /// `Cmd.publish`). The thunk returns 0 (a port has no subscriber-count
    /// semantics).
    pub fn js_send<T, M>(payload: T) -> IpeCmd<M>
    where
        T: serde::Serialize + Send + 'static,
    {
        IpeCmd::Publish(Box::new(move |origin| {
            // A seal-legal payload's concrete type serialises to a JSON value by
            // construction (the seal gate — IPE-L0148 — guarantees a plain, closed,
            // non-secret type). Serialisation therefore cannot fail here. Surface
            // the impossible branch honestly instead of silently dropping the frame:
            // a debug build trips an assertion carrying the type name (so a future
            // non-seal-legal caller is caught in test), and a release build drops the
            // one frame rather than emitting a malformed wire string. The seal is not
            // loosened either way.
            match serde_json::to_value(&payload) {
                Ok(value) => {
                    let encoded = seal_encode(&value);
                    let sink = lock_sessions().get(origin).and_then(|p| p.out_sink.clone());
                    if let Some(sink) = sink {
                        sink(&encoded);
                    }
                }
                Err(e) => {
                    debug_assert!(
                        false,
                        "js_send: seal-legal payload of type {} failed serde serialisation: {e}",
                        std::any::type_name::<T>()
                    );
                    eprintln!(
                        "[ipe-runtime BUG] js_send: payload of type {} failed serialisation ({e}); frame dropped — please report",
                        std::any::type_name::<T>()
                    );
                }
            }
            0
        }))
    }

    /// `js_subscribe decoder to_msg` — drain the OWNING session's inbound port
    /// strings, decode each fail-closed through the bounded seal decoder, and emit
    /// `to_msg(a)` on a clean decode. A rejected payload is dropped whole.
    ///
    /// The owning session's sid is read SYNCHRONOUSLY here (while the driver's
    /// `with_session_sid` scope is active), then moved into the spawned task, so
    /// the recv loop drains exactly that session's channel and can never observe
    /// another session's inbound frames. A subscription materialised with no
    /// session sid in scope (sid == "") binds an inert per-call channel that no
    /// route feeds, so it receives nothing.
    ///
    /// `to_msg` is moved into exactly one detached task and never shared behind an
    /// `Arc`, so `Send` (not `Send + Sync`) is the exact contract — the same
    /// bound `sub_subscribe_ws_message` carries.
    pub fn js_subscribe<T, M, F>(decoder: Decoder<IpeError, T>, to_msg: F) -> IpeSub<M>
    where
        M: Send + 'static,
        F: Fn(T) -> M + Send + 'static,
        T: Send + 'static,
    {
        // Read the owning sid while the materialisation scope is live, then bind
        // this session's inbound receiver. An empty sid (no live session) gets a
        // fresh private channel no route publishes to — an inert, fail-closed drain.
        let owner_sid = scope_sid();
        let rx = if owner_sid.is_empty() {
            broadcast::channel(PORT_CAP).1
        } else {
            let mut g = lock_sessions();
            g.entry(owner_sid)
                .or_insert_with(SessionPorts::new)
                .inbound
                .subscribe()
        };
        IpeSub::Source(Box::new(move |emit| {
            let mut rx = rx;
            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(raw) => {
                            // Fail-closed decode: a payload that does not decode to
                            // the declared type is dropped whole, never a partial
                            // value and never a panic.
                            if let Ok(value) = seal_decode(&raw, &decoder, SealLimits::default()) {
                                emit(to_msg(value));
                            }
                        }
                        // A slow consumer that lagged past the buffer skips the
                        // gap and keeps the subscription alive; a closed channel
                        // (the session was evicted) ends it.
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            })
        }))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{
    deliver_inbound_for, js_send, js_subscribe, register_out_sink_for, session_close, session_open,
};

// ─── Wasm (browser) transport ──────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use crate::error::IpeError;
    use crate::json::Decoder;
    use crate::seal_codec::seal_decode;
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::{JsCast, JsValue};

    thread_local! {
        /// The in-tab inbound queue's drains. The browser's `window.ipe.send(raw)`
        /// pushes a string here (via [`push_inbound`]); each active `js_subscribe`
        /// registers a drain closure that fires on every pushed string.
        static INBOUND: RefCell<Vec<Rc<dyn Fn(&str)>>> = const { RefCell::new(Vec::new()) };
    }

    /// The browser-facing entry the JS glue calls to push an inbound port
    /// message: `window.ipe.send(raw)` routes here. Each registered drain runs
    /// synchronously; a drain's own fail-closed decode drops a bad payload.
    pub fn push_inbound(raw: &str) {
        let drains: Vec<Rc<dyn Fn(&str)>> =
            INBOUND.with(|q| q.borrow().iter().map(Rc::clone).collect());
        for drain in drains {
            drain(raw);
        }
    }

    /// Deliver one seal-encoded outbound frame to the browser by calling the
    /// registered `window.ipeOnReceive` handler. A missing or non-callable
    /// handler is a no-op (the page simply has not wired a receiver yet).
    fn deliver_outbound(encoded: &str) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(handler) = js_sys::Reflect::get(&window, &JsValue::from_str("ipeOnReceive")) else {
            return;
        };
        if let Ok(func) = handler.dyn_into::<js_sys::Function>() {
            let _ = func.call1(&JsValue::NULL, &JsValue::from_str(encoded));
        }
    }

    /// `js_send payload` — seal-encode `payload` and hand it to the browser
    /// `window.ipeOnReceive` handler in-process. Fire-and-forget via
    /// [`IpeCmd::Publish`].
    pub fn js_send<T, M>(payload: T) -> IpeCmd<M>
    where
        // `IpeCmd::Publish` carries a `Send` thunk on every target (the enum
        // variant is target-neutral), so the captured payload is `Send` even
        // though the wasm delivery runs single-threaded.
        T: serde::Serialize + Send + 'static,
    {
        IpeCmd::Publish(Box::new(move |_origin| {
            if let Ok(value) = serde_json::to_value(&payload) {
                deliver_outbound(&seal_encode(&value));
            }
            0
        }))
    }

    /// `js_subscribe decoder to_msg` — register a drain that decodes each inbound
    /// string fail-closed and emits `to_msg(a)` on a clean decode. Returns a real
    /// teardown thunk so the scheduler's stop-all-then-respawn cycle can drop the
    /// drain instead of accumulating duplicates across re-renders.
    pub fn js_subscribe<T, M, F>(decoder: Decoder<IpeError, T>, to_msg: F) -> IpeSub<M>
    where
        M: 'static,
        F: Fn(T) -> M + 'static,
        T: 'static,
    {
        IpeSub::Source(Box::new(move |emit: Rc<dyn Fn(M)>| {
            let decoder = decoder.clone();
            let drain: Rc<dyn Fn(&str)> = Rc::new(move |raw: &str| {
                if let Ok(value) = seal_decode(raw, &decoder, SealLimits::default()) {
                    (emit)(to_msg(value));
                }
            });
            let drain_key = Rc::as_ptr(&drain) as *const () as usize;
            INBOUND.with(|q| q.borrow_mut().push(Rc::clone(&drain)));
            Box::new(move || {
                INBOUND.with(|q| {
                    q.borrow_mut()
                        .retain(|d| Rc::as_ptr(d) as *const () as usize != drain_key);
                });
            })
        }))
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::{js_send, js_subscribe, push_inbound};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::json::JsonVal;

    // ── Feature-independent: canonical seal encoding ───────────────────────────
    #[test]
    fn outbound_encodes_record_canonically() {
        // A record payload seal-encodes to canonical sorted-key JSON.
        let value: JsonVal = serde_json::json!({ "b": 2, "a": 1 });
        assert_eq!(seal_encode(&value), r#"{"a":1,"b":2}"#);
    }

    // ── Per-session transport (needs the `web` session-sid scope) ──────────────
    //
    // The native port transport binds each `js_subscribe`/`js_send` to the OWNING
    // session's channel, read from the pub/sub session-sid task-local. These tests
    // drive that real scope, so they compile only under `web` (which provides
    // `pubsub::with_session_sid`).
    #[cfg(feature = "web")]
    mod per_session {
        use super::super::*;
        use crate::IpeResult;
        use crate::error::IpeError;
        use crate::json::{Decoder, JsonVal, json_decode_int};
        use crate::web::pubsub::with_session_sid;
        use std::sync::{Arc, Mutex};

        fn int_decoder() -> Decoder<IpeError, i64> {
            json_decode_int()
        }

        // A decoder that always fails, to pin the drop-whole contract independent
        // of the payload shape.
        fn never_decoder() -> Decoder<IpeError, i64> {
            Decoder::new(
                Box::new(|_v: &JsonVal| IpeResult::Err(IpeError::from("nope".to_string()))),
                Vec::new(),
            )
        }

        // Materialise a `js_subscribe` Source the way the Web driver does —
        // INSIDE `with_session_sid(sid, …)`, so the subscription binds `sid`'s
        // inbound channel — and collect the emitted Msgs.
        fn collect_for(
            sid: &str,
            decoder: Decoder<IpeError, i64>,
        ) -> (tokio::task::JoinHandle<()>, Arc<Mutex<Vec<i64>>>) {
            let got = Arc::new(Mutex::new(Vec::<i64>::new()));
            let got2 = got.clone();
            let emit: Arc<dyn Fn(i64) + Send + Sync> =
                Arc::new(move |m| got2.lock().unwrap_or_else(|e| e.into_inner()).push(m));
            let sub = with_session_sid(sid.to_string(), || {
                js_subscribe::<i64, i64, _>(decoder, |a| a)
            });
            let handle = match sub {
                IpeSub::Source(spawn) => spawn(emit),
                _ => unreachable!("js_subscribe builds a Source"),
            };
            (handle, got)
        }

        #[tokio::test]
        async fn inbound_clean_payload_is_emitted() {
            session_open("s-clean");
            let (h, got) = collect_for("s-clean", int_decoder());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await; // let it subscribe
            deliver_inbound_for("s-clean", "7".to_string());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert_eq!(*got.lock().unwrap_or_else(|e| e.into_inner()), vec![7]);
            h.abort();
            session_close("s-clean");
        }

        #[tokio::test]
        async fn inbound_undecodable_payload_is_dropped_whole() {
            session_open("s-bad");
            let (h, got) = collect_for("s-bad", int_decoder());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            deliver_inbound_for("s-bad", "\"not an int\"".to_string()); // string, not i64
            deliver_inbound_for("s-bad", "{not json".to_string()); // malformed
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert!(got.lock().unwrap_or_else(|e| e.into_inner()).is_empty()); // both dropped, no panic, no partial
            h.abort();
            session_close("s-bad");
        }

        #[tokio::test]
        async fn inbound_failing_decoder_drops_every_payload() {
            session_open("s-never");
            let (h, got) = collect_for("s-never", never_decoder());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            deliver_inbound_for("s-never", "1".to_string());
            deliver_inbound_for("s-never", "2".to_string());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert!(got.lock().unwrap_or_else(|e| e.into_inner()).is_empty());
            h.abort();
            session_close("s-never");
        }

        #[tokio::test]
        async fn outbound_send_encodes_and_delivers_to_origin_sink() {
            let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let seen2 = seen.clone();
            session_open("s-out");
            register_out_sink_for(
                "s-out",
                Arc::new(move |s: &str| {
                    seen2
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(s.to_string());
                }),
            );
            let cmd: IpeCmd<i64> = js_send::<i64, i64>(42);
            match cmd {
                // The dispatch loop injects the origin sid; delivery reaches only
                // that session's sink.
                IpeCmd::Publish(thunk) => assert_eq!(thunk("s-out"), 0),
                _ => unreachable!("js_send builds a Publish cmd"),
            }
            // Canonical seal encoding of the integer 42 is the string "42".
            assert_eq!(
                seen.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .last()
                    .map(String::as_str),
                Some("42")
            );
            session_close("s-out");
        }

        // THE cross-session-isolation proof. Two live sessions, A and B. A's
        // browser inbound reaches ONLY A's `js_subscribe` (B never sees it); A's
        // `js_send` outbound reaches ONLY A's out-sink (B's stays empty).
        // Cross-session delivery is unrepresentable, verified end-to-end.
        #[tokio::test]
        async fn sessions_are_isolated_inbound_and_outbound() {
            session_open("A");
            session_open("B");

            // Inbound: one subscriber per session, bound via the session scope.
            let (ha, got_a) = collect_for("A", int_decoder());
            let (hb, got_b) = collect_for("B", int_decoder());

            // Outbound: one out-sink per session.
            let out_a: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let out_b: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let oa = out_a.clone();
            let ob = out_b.clone();
            register_out_sink_for(
                "A",
                Arc::new(move |s: &str| {
                    oa.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(s.to_string())
                }),
            );
            register_out_sink_for(
                "B",
                Arc::new(move |s: &str| {
                    ob.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(s.to_string())
                }),
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await; // subscribe

            // A's browser sends 11 inbound; only A's subscriber must see it.
            deliver_inbound_for("A", "11".to_string());
            // A's program sends 99 outbound; only A's sink must see it.
            match js_send::<i64, i64>(99) {
                IpeCmd::Publish(thunk) => {
                    thunk("A");
                }
                _ => unreachable!(),
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;

            assert_eq!(*got_a.lock().unwrap_or_else(|e| e.into_inner()), vec![11]); // A received its inbound
            assert!(got_b.lock().unwrap_or_else(|e| e.into_inner()).is_empty()); // B saw NOTHING (no leak)
            assert_eq!(
                out_a.lock().unwrap_or_else(|e| e.into_inner()).as_slice(),
                ["99".to_string()]
            ); // A's sink
            assert!(out_b.lock().unwrap_or_else(|e| e.into_inner()).is_empty()); // B's sink saw NOTHING (no leak)

            ha.abort();
            hb.abort();
            session_close("A");
            session_close("B");
        }

        // A subscription materialised with NO session sid in scope binds an inert
        // channel that no route feeds — it must never receive another session's
        // inbound frames (fail-closed when the owner is unknown).
        #[tokio::test]
        async fn unscoped_subscription_receives_nothing() {
            session_open("scoped");
            // NOTE: materialised OUTSIDE with_session_sid → owner sid is "".
            let got = Arc::new(Mutex::new(Vec::<i64>::new()));
            let got2 = got.clone();
            let emit: Arc<dyn Fn(i64) + Send + Sync> =
                Arc::new(move |m| got2.lock().unwrap_or_else(|e| e.into_inner()).push(m));
            let sub = js_subscribe::<i64, i64, _>(int_decoder(), |a| a);
            let h = match sub {
                IpeSub::Source(spawn) => spawn(emit),
                _ => unreachable!(),
            };
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            deliver_inbound_for("scoped", "5".to_string());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert!(got.lock().unwrap_or_else(|e| e.into_inner()).is_empty()); // inert drain — nothing delivered
            h.abort();
            session_close("scoped");
        }

        // After `session_close`, a delivery to that sid is a fire-and-forget no-op
        // (the channel is gone) and any live subscriber ends — no dead-session
        // channel lingers.
        #[tokio::test]
        async fn closed_session_delivery_is_a_noop() {
            session_open("gone");
            let (h, got) = collect_for("gone", int_decoder());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            session_close("gone");
            deliver_inbound_for("gone", "1".to_string()); // no channel → dropped
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert!(got.lock().unwrap_or_else(|e| e.into_inner()).is_empty());
            h.abort();
        }
    }
}

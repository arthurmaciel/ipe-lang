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
//! * **Native (server).** Inbound port messages arrive as strings on a broadcast
//!   channel that the server's inbound route feeds after applying the same session
//!   + CSRF + bounded-seal checks the `/_ipe/event` route applies; a `js_subscribe`
//!   `Source` drains that broadcast. Outbound strings are delivered to a registered
//!   out-sink the server drains to push to the browser (alongside the SSE patch
//!   stream). Both channels default to a real in-process broadcast so the transport
//!   is total and observable even with no browser attached.
//!
//!   The default channels here are process-scoped. A live multi-session server must
//!   wire per-SESSION channels (one seam per session, not one per process) before
//!   routing browser traffic through them, or one session's inbound payloads would
//!   reach another session's `js_subscribe`. That per-session wiring is a
//!   security-scoped step the server binder owns; these seams exist so it can drive
//!   them, not so it can share one across sessions.
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
    use std::sync::{Arc, Mutex, OnceLock};
    use tokio::sync::broadcast;

    /// Broadcast buffer for one direction. A lagging subscriber drops the gap
    /// (never panics); the size bounds the queue an inbound burst can hold.
    const PORT_CAP: usize = 256;

    /// The process-global inbound port channel: raw strings the browser sent,
    /// already through the server's fail-closed decode gate. Every
    /// `js_subscribe` subscribes to this; the server's inbound route publishes to
    /// it.
    fn inbound() -> &'static broadcast::Sender<String> {
        static IN: OnceLock<broadcast::Sender<String>> = OnceLock::new();
        IN.get_or_init(|| broadcast::channel(PORT_CAP).0)
    }

    /// The process-global outbound port channel: seal-encoded strings a
    /// `js_send` produced, awaiting delivery to the browser. The server drains
    /// this and forwards each frame to the client.
    fn outbound() -> &'static broadcast::Sender<String> {
        static OUT: OnceLock<broadcast::Sender<String>> = OnceLock::new();
        OUT.get_or_init(|| broadcast::channel(PORT_CAP).0)
    }

    /// A registered out-sink the server installs to receive each outbound
    /// seal-encoded frame directly (in addition to the broadcast), so it can push
    /// the frame to the browser on the same connection that carries SSE patches.
    type OutSink = Arc<dyn Fn(&str) + Send + Sync>;

    fn out_sink() -> &'static Mutex<Option<OutSink>> {
        static SINK: OnceLock<Mutex<Option<OutSink>>> = OnceLock::new();
        SINK.get_or_init(|| Mutex::new(None))
    }

    /// Install the server's browser-push sink. Called once when the live server
    /// wires its client transport; every subsequent `js_send` frame is delivered
    /// to `sink` as well as the observable broadcast.
    pub fn register_out_sink(sink: OutSink) {
        *out_sink().lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
    }

    /// Feed one raw inbound string to every active `js_subscribe`. Called by the
    /// server's inbound port route AFTER its session + CSRF + bounded-seal checks
    /// have accepted the request body. Fire-and-forget: a send with no live
    /// subscribers is not an error.
    pub fn deliver_inbound(raw: String) {
        let _ = inbound().send(raw);
    }

    /// Subscribe to the outbound port stream — the server's push loop drains this
    /// to forward frames to the browser.
    pub fn subscribe_outbound() -> broadcast::Receiver<String> {
        outbound().subscribe()
    }

    /// `js_send payload` — seal-encode `payload` and hand it to the browser
    /// out-channel. Fire-and-forget via [`IpeCmd::Publish`]; the thunk returns 0
    /// (a port has no subscriber-count semantics).
    pub fn js_send<T, M>(payload: T) -> IpeCmd<M>
    where
        T: serde::Serialize + Send + 'static,
    {
        IpeCmd::Publish(Box::new(move |_origin| {
            // A seal-legal payload's concrete type serialises to a JSON value by
            // construction (the seal gate guarantees a plain, closed, non-secret
            // type). A serialisation error cannot arise for such a type; on the
            // impossible error the frame is dropped rather than delivering a
            // malformed wire string.
            if let Ok(value) = serde_json::to_value(&payload) {
                let encoded = seal_encode(&value);
                if let Some(sink) = out_sink().lock().unwrap_or_else(|e| e.into_inner()).clone() {
                    sink(&encoded);
                }
                let _ = outbound().send(encoded);
            }
            0
        }))
    }

    /// `js_subscribe decoder to_msg` — drain inbound port strings, decode each
    /// fail-closed through the bounded seal decoder, and emit `to_msg(a)` on a
    /// clean decode. A rejected payload is dropped whole.
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
        IpeSub::Source(Box::new(move |emit| {
            let mut rx = inbound().subscribe();
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
                        // (unreachable while this receiver lives) ends it.
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            })
        }))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{deliver_inbound, js_send, js_subscribe, register_out_sink, subscribe_outbound};

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
    use crate::IpeResult;
    use crate::error::IpeError;
    use crate::json::{Decoder, JsonVal, json_decode_int};
    use std::sync::{Arc, Mutex};

    // The default inbound port channel is one process-scoped broadcast, so a
    // concurrently-running test's `deliver_inbound` would reach this test's
    // subscriber. Serialise the inbound tests behind one async lock (held across
    // `.await`, so a std guard would be `!Send`) so each drives the shared channel
    // alone.
    fn inbound_test_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    fn int_decoder() -> Decoder<IpeError, i64> {
        json_decode_int()
    }

    // Materialise a `js_subscribe` Source the way the Web loop does and collect
    // the emitted Msgs.
    fn collect(
        decoder: Decoder<IpeError, i64>,
    ) -> (tokio::task::JoinHandle<()>, Arc<Mutex<Vec<i64>>>) {
        let got = Arc::new(Mutex::new(Vec::<i64>::new()));
        let got2 = got.clone();
        let emit: Arc<dyn Fn(i64) + Send + Sync> = Arc::new(move |m| got2.lock().unwrap().push(m));
        let sub = js_subscribe::<i64, i64, _>(decoder, |a| a);
        let handle = match sub {
            IpeSub::Source(spawn) => spawn(emit),
            _ => unreachable!("js_subscribe builds a Source"),
        };
        (handle, got)
    }

    #[tokio::test]
    async fn inbound_clean_payload_is_emitted() {
        let _guard = inbound_test_lock().lock().await;
        let (h, got) = collect(int_decoder());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await; // let it subscribe
        deliver_inbound("7".to_string());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(*got.lock().unwrap(), vec![7]);
        h.abort();
    }

    #[tokio::test]
    async fn inbound_undecodable_payload_is_dropped_whole() {
        let _guard = inbound_test_lock().lock().await;
        let (h, got) = collect(int_decoder());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        deliver_inbound("\"not an int\"".to_string()); // decodes to a string, not i64
        deliver_inbound("{not json".to_string()); // malformed
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(got.lock().unwrap().is_empty()); // both dropped, no panic, no partial
        h.abort();
    }

    #[tokio::test]
    async fn outbound_send_encodes_and_delivers_to_sink() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        register_out_sink(Arc::new(move |s: &str| {
            seen2
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(s.to_string());
        }));
        let mut rx = subscribe_outbound();
        let cmd: IpeCmd<i64> = js_send::<i64, i64>(42);
        match cmd {
            IpeCmd::Publish(thunk) => assert_eq!(thunk("sid"), 0),
            _ => unreachable!("js_send builds a Publish cmd"),
        }
        // Canonical seal encoding of the integer 42 is the string "42".
        assert_eq!(seen.lock().unwrap().last().map(String::as_str), Some("42"));
        assert_eq!(rx.recv().await.unwrap(), "42");
    }

    #[test]
    fn outbound_encodes_record_canonically() {
        // A record payload seal-encodes to canonical sorted-key JSON.
        let value: JsonVal = serde_json::json!({ "b": 2, "a": 1 });
        assert_eq!(seal_encode(&value), r#"{"a":1,"b":2}"#);
    }

    // A decoder that always fails, to pin the drop-whole contract independent of
    // the payload shape.
    fn never_decoder() -> Decoder<IpeError, i64> {
        Decoder::new(
            Box::new(|_v: &JsonVal| IpeResult::Err(IpeError::from("nope".to_string()))),
            Vec::new(),
        )
    }

    #[tokio::test]
    async fn inbound_failing_decoder_drops_every_payload() {
        let _guard = inbound_test_lock().lock().await;
        let (h, got) = collect(never_decoder());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        deliver_inbound("1".to_string());
        deliver_inbound("2".to_string());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(got.lock().unwrap().is_empty());
        h.abort();
    }
}

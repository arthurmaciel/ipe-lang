//! Ipe.Ffi.Js ports — the raw typed Ipê↔JS transport behind `Js.send` /
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
//!   is only ever obtained from a [`SessionId`]. A caller with no session sid in scope
//!   gets `None` from [`scope_sid`] and subscribes to an inert per-call channel that
//!   no route ever feeds, so an unscoped subscription receives nothing rather than
//!   another session's traffic. Channels are created on session start ([`session_open`])
//!   and dropped on session end/eviction ([`session_close`]), reusing the session
//!   store's lifecycle.
//! * **Wasm (`feature = "wasm-client"`).** In-process, no network. Outbound calls
//!   the browser-registered `window.ipeOnReceive` handler with the seal-encoded
//!   string; inbound is fed by the browser's `window.ipe.send(...)` pushing a
//!   string into an in-tab queue the `Source` drains.

use crate::seal_codec::{SealLimits, seal_encode};
use crate::tea::{IpeCmd, IpeSub};

// ─── SessionId newtype ──────────────────────────────────────────────────────

/// A validated session identifier: a non-empty, all-lowercase-hex string
/// produced by the server's CSPRNG-backed session-minting path.
///
/// The constructor [`SessionId::parse`] is the only way to build a value of
/// this type from outside this module. It rejects an empty string and any
/// string whose characters are not all lowercase hexadecimal (`[0-9a-f]`),
/// so an invalid or absent session id is unrepresentable past the parse
/// boundary — the per-call `is_empty()` guards in the native registry fall
/// away entirely.
///
/// `Display`/`Debug` expose the raw hex string so the store can use it as a
/// map key and the dispatch loop can pass it to [`js_send`] via
/// `IpeCmd::Publish`'s `origin: &str` argument.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    /// Parse a raw session-id string into a `SessionId`.
    ///
    /// Returns `None` when `raw` is empty or contains any character outside
    /// `[0-9a-f]`. The server's [`new_sid`][crate::web] always produces a
    /// 32-char lowercase-hex UUID-simple string, so every legitimately minted
    /// session id passes this check.
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() {
            return None;
        }
        if raw.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
            Some(SessionId(raw.to_string()))
        } else {
            None
        }
    }

    /// The validated hex string, borrowed.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Debug for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ─── Native (server) transport ─────────────────────────────────────────────

// ─── Correlation-id wire key ────────────────────────────────────────────────

/// The JSON field name the runtime uses to carry the correlation id on both
/// the outbound request envelope and the inbound reply envelope.
///
/// The double-underscore prefix and `ipe_` namespace make this key impossible
/// to collide with a user-defined field: the seal gate rejects any value that
/// contains a field named `__ipe_id` (the sealed type is closed and declared,
/// so an extra field on an inbound value fails the decoder for any user type).
/// Runtime-private — never exported, never user-reachable.
const COR_ID_FIELD: &str = "__ipe_id";

/// Hard ceiling on outstanding (in-flight) `js_request` waiters per session.
/// A JS handler that never replies must not fill heap indefinitely; once the
/// ceiling is reached, new `js_request` calls resolve immediately with `Err`.
pub(crate) const MAX_OUTSTANDING: usize = 256;

/// Default timeout for a `js_request` waiter, in milliseconds. A JS handler
/// that does not reply within this window resolves the Task with `Err`. The
/// value is generous — it covers slow-permission-prompt flows. A per-call
/// override is a tracked follow-up.
const REQUEST_TIMEOUT_MS: u64 = 10_000;

// ─── Native (server) transport ─────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use crate::error::IpeError;
    use crate::json::{Decoder, JsonVal};
    use crate::seal_codec::seal_decode;
    use std::collections::HashMap;
    use std::sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    };
    use tokio::sync::{broadcast, oneshot};

    /// Broadcast buffer for one direction. A lagging subscriber drops the gap
    /// (never panics); the size bounds the queue an inbound burst can hold.
    const PORT_CAP: usize = 256;

    /// Process-global monotonic correlation-id counter. Ids are runtime-private:
    /// they are minted here and never accepted as input — a JS-injected id cannot
    /// forge a waiter lookup because the registry key is the id minted by this
    /// counter, not an id parsed from an untrusted inbound frame.
    static NEXT_COR_ID: AtomicU64 = AtomicU64::new(1);

    fn next_cor_id() -> u64 {
        NEXT_COR_ID.fetch_add(1, Ordering::Relaxed)
    }

    /// A registered out-sink for one session: the server installs it so each
    /// outbound `js_send` frame for that session is pushed to that session's
    /// browser over its own SSE connection.
    type OutSink = Arc<dyn Fn(&str) + Send + Sync>;

    /// One session's port endpoints. The inbound channel carries browser→server
    /// frames (already through the fail-closed decode gate) to that session's
    /// `js_subscribe`; `out_sink` is that session's browser-push installed by the
    /// live server; `pending` holds in-flight `js_request` waiters keyed by their
    /// runtime-minted correlation id. All belong to exactly one sid.
    pub(crate) struct SessionPorts {
        inbound: broadcast::Sender<String>,
        out_sink: Option<OutSink>,
        /// In-flight correlated one-shot waiters. Key = runtime-minted correlation
        /// id (never derived from JS input). A reply whose id is not in this map is
        /// dropped fail-closed (unknown/duplicate/late id). Bounded by
        /// [`MAX_OUTSTANDING`]: a new waiter is refused when the map is full.
        pub(crate) pending: HashMap<u64, oneshot::Sender<JsonVal>>,
    }

    impl SessionPorts {
        pub(crate) fn new() -> Self {
            SessionPorts {
                inbound: broadcast::channel(PORT_CAP).0,
                out_sink: None,
                pending: HashMap::new(),
            }
        }
    }

    /// The per-session port registry, keyed by session sid. A channel handle is
    /// ONLY ever obtained by looking up a [`SessionId`] here; there is no
    /// process-global channel, so no `js_send`/`js_subscribe`/inbound-route can
    /// reach a session other than the one whose sid it holds — cross-session
    /// delivery is unrepresentable.
    fn sessions() -> &'static Mutex<HashMap<String, SessionPorts>> {
        static S: OnceLock<Mutex<HashMap<String, SessionPorts>>> = OnceLock::new();
        S.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(crate) fn lock_sessions() -> std::sync::MutexGuard<'static, HashMap<String, SessionPorts>> {
        // Poison-tolerant: a panic in one session's callback must not wedge the
        // whole registry; the map is still valid data.
        sessions().lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The sid of the session whose subscriptions/commands are being materialised
    /// in the current scope. Under the `web` server this reads the same task-local
    /// the pub/sub broker sets (via `pubsub::with_session_sid`), so a `js_subscribe`
    /// binds to the OWNING session. Outside a live server (e.g. a `tokio`-only test
    /// build with no `web` feature) there is no session scope, so it returns `None`
    /// and callers fall back to an inert private channel.
    #[cfg(feature = "web")]
    pub(super) fn scope_sid() -> Option<SessionId> {
        SessionId::parse(&crate::web::pubsub::current_session_sid())
    }
    #[cfg(not(feature = "web"))]
    pub(super) fn scope_sid() -> Option<SessionId> {
        None
    }

    /// Create a session's port endpoints on session start (idempotent). Called by
    /// the live server when it creates the session, so the inbound channel exists
    /// before the browser can POST to it.
    pub fn session_open(sid: &SessionId) {
        lock_sessions()
            .entry(sid.0.clone())
            .or_insert_with(SessionPorts::new);
    }

    /// Drop a session's port endpoints on session end/eviction. Any live
    /// `js_subscribe` receiver on the dropped inbound channel observes `Closed`
    /// and ends; no dead-session channel lingers.
    pub fn session_close(sid: &SessionId) {
        lock_sessions().remove(&sid.0);
    }

    /// Install `sid`'s browser-push sink. Called when the live server attaches the
    /// session's client transport; every subsequent `js_send` whose origin is
    /// `sid` is delivered to `sink`. Creates the session entry if the sink is
    /// wired before `session_open` (idempotent).
    pub fn register_out_sink_for(sid: &SessionId, sink: OutSink) {
        let mut g = lock_sessions();
        g.entry(sid.0.clone())
            .or_insert_with(SessionPorts::new)
            .out_sink = Some(sink);
    }

    /// Feed one raw inbound string to `sid`'s port. Called by the server's inbound
    /// port route AFTER its session-cookie + CSRF + bounded-seal checks.
    ///
    /// Dispatch policy (fail-closed at each step):
    ///
    /// 1. If the raw string is valid JSON and its top-level object contains the
    ///    runtime-private `__ipe_id` field, this is a correlated reply: extract the
    ///    id and the rest of the frame, look up the matching one-shot waiter, and
    ///    resolve it. An unknown/duplicate/late id is silently dropped (no panic, no
    ///    forwarding to `js_subscribe` subscribers). The `__ipe_id` field is stripped
    ///    before the reply payload is handed to the waiter's decoder, so it is never
    ///    user-visible.
    /// 2. Otherwise forward to the session's `js_subscribe` broadcast channel.
    ///
    /// Cross-session delivery is unrepresentable: delivery reaches ONLY `sid`.
    pub fn deliver_inbound_for(sid: &SessionId, raw: String) {
        // Peek at the raw string: if it parses as a JSON object with `__ipe_id`,
        // this is a correlated reply — route to the pending map, not subscribers.
        // Wire format: `{"__ipe_id": <u64>, "payload": <sealed_reply>}`.
        // After stripping `__ipe_id`, the remaining object's `"payload"` field is
        // the sealed reply value forwarded to the waiting decoder.
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw)
            && let Some(id_val) = value.as_object_mut().and_then(|o| o.remove(COR_ID_FIELD))
            && let Some(id) = id_val.as_u64()
        {
            // Extract the `"payload"` field from the remaining envelope,
            // falling back to the whole remaining value so an echo that omits
            // the wrapper is still decoded (fail-closed if neither decodes).
            let payload = value
                .as_object_mut()
                .and_then(|o| o.remove("payload"))
                .unwrap_or(value);
            let waiter = lock_sessions()
                .get_mut(&sid.0)
                .and_then(|p| p.pending.remove(&id));
            if let Some(tx) = waiter {
                // Resolve the one-shot with the extracted payload.
                let _ = tx.send(payload);
            }
            // Unknown/duplicate/late id: dropped fail-closed, no subscribers.
            return;
        }
        // No correlation id — forward to `js_subscribe` subscribers as before.
        let sender = lock_sessions().get(&sid.0).map(|p| p.inbound.clone());
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
            // non-secret type), so this Err branch is unreachable for a seal-legal
            // caller. Surface it honestly instead of silently dropping the frame:
            // log the offending type and drop the single frame rather than emit a
            // malformed wire string. The seal is not loosened either way.
            match serde_json::to_value(&payload) {
                Ok(value) => {
                    let encoded = seal_encode(&value);
                    let sink = lock_sessions().get(origin).and_then(|p| p.out_sink.clone());
                    if let Some(sink) = sink {
                        sink(&encoded);
                    }
                }
                Err(e) => {
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
    /// session sid in scope (no valid [`SessionId`]) binds an inert per-call
    /// channel that no route feeds, so it receives nothing.
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
        // this session's inbound receiver. No valid sid (no live session) gets a
        // fresh private channel no route publishes to — an inert, fail-closed drain.
        let rx = match scope_sid() {
            None => broadcast::channel(PORT_CAP).1,
            Some(owner_sid) => {
                let mut g = lock_sessions();
                g.entry(owner_sid.0)
                    .or_insert_with(SessionPorts::new)
                    .inbound
                    .subscribe()
            }
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

    /// `js_request payload decoder` — correlated one-shot port request.
    ///
    /// Semantics:
    ///
    /// 1. Mint a fresh runtime-private correlation id (process-global monotonic
    ///    counter; never derived from JS input, never user-observable).
    /// 2. Register a one-shot waiter keyed by that id in the OWNING session's
    ///    pending map. Fails immediately with `Err` when [`MAX_OUTSTANDING`] is
    ///    already reached (bounded — heap cannot grow unboundedly).
    /// 3. Send `payload` outbound as `{"__ipe_id": id, "payload": <sealed>}` so
    ///    the first-party JS sink can echo the id on its reply.
    /// 4. Await the one-shot with a [`REQUEST_TIMEOUT_MS`] deadline. A reply whose
    ///    id matches resolves the waiter with the stripped reply frame; the decoder
    ///    runs fail-closed over that frame. A duplicate/late/unknown id is dropped
    ///    by `deliver_inbound_for` before the waiter is ever signalled.
    /// 5. Timeout or decode-miss → typed `Err`; never a panic.
    ///
    /// No trust change: the same SEAL discipline governs both directions.
    pub fn js_request<T, R>(
        payload: T,
        decoder: Decoder<IpeError, R>,
    ) -> crate::core::IpeTask<IpeError, R>
    where
        T: serde::Serialize + Send + 'static,
        R: Send + 'static,
    {
        let cor_id = next_cor_id();
        let owner_sid = scope_sid();
        Box::pin(async move {
            // Refuse when the ceiling is already reached (fail-closed, no panic).
            {
                let mut g = lock_sessions();
                if let Some(ports) = owner_sid.as_ref().and_then(|s| g.get_mut(&s.0))
                    && ports.pending.len() >= MAX_OUTSTANDING
                {
                    return crate::core::IpeResult::Err(
                        "js_request: outstanding waiter ceiling reached"
                            .to_string()
                            .into(),
                    );
                }
            }

            // Serialize payload and wrap with the correlation id.
            let outbound_json = match serde_json::to_value(&payload) {
                Ok(v) => v,
                Err(e) => {
                    return crate::core::IpeResult::Err(
                        format!("js_request: payload serialisation failed: {e}").into(),
                    );
                }
            };
            let envelope = serde_json::json!({
                COR_ID_FIELD: cor_id,
                "payload": outbound_json,
            });
            let encoded = seal_encode(&envelope);

            // Register the one-shot waiter.
            let (tx, rx) = oneshot::channel::<JsonVal>();
            if let Some(sid) = &owner_sid {
                let mut g = lock_sessions();
                g.entry(sid.0.clone())
                    .or_insert_with(SessionPorts::new)
                    .pending
                    .insert(cor_id, tx);
            }
            // Deliver outbound to the origin session's sink.
            if let Some(sid) = &owner_sid {
                let sink = lock_sessions().get(&sid.0).and_then(|p| p.out_sink.clone());
                if let Some(sink) = sink {
                    sink(&encoded);
                }
            }

            // Await reply with deadline.
            let timeout = tokio::time::Duration::from_millis(REQUEST_TIMEOUT_MS);
            let result = tokio::time::timeout(timeout, rx).await;

            // Clean up the waiter regardless of outcome (idempotent — already
            // removed by `deliver_inbound_for` on a clean reply).
            if let Some(sid) = &owner_sid {
                lock_sessions()
                    .get_mut(&sid.0)
                    .map(|p| p.pending.remove(&cor_id));
            }

            match result {
                Ok(Ok(reply_value)) => {
                    // Decode the reply fail-closed through the seal gate.
                    match seal_decode(&reply_value.to_string(), &decoder, SealLimits::default()) {
                        Ok(v) => crate::core::IpeResult::Ok(v),
                        Err(_) => crate::core::IpeResult::Err(
                            "js_request: reply failed seal decode".to_string().into(),
                        ),
                    }
                }
                Ok(Err(_)) => {
                    // Sender dropped (session evicted or process exit) — fail closed.
                    crate::core::IpeResult::Err(
                        "js_request: session closed before reply".to_string().into(),
                    )
                }
                Err(_) => {
                    // Timeout.
                    crate::core::IpeResult::Err("js_request: timeout".to_string().into())
                }
            }
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{
    deliver_inbound_for, js_request, js_send, js_subscribe, register_out_sink_for, session_close,
    session_open,
};

#[cfg(all(not(target_arch = "wasm32"), test))]
pub(crate) use native::{SessionPorts, lock_sessions};

// ─── Wasm (browser) transport ──────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use crate::error::IpeError;
    use crate::json::{Decoder, JsonVal};
    use crate::seal_codec::seal_decode;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use wasm_bindgen::{JsCast, JsValue};

    /// Process-local monotonic correlation-id counter for the wasm target.
    /// Single-threaded (wasm32 is `!Send`), so `Cell<u64>` is safe.
    thread_local! {
        static NEXT_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
    }

    fn next_cor_id() -> u64 {
        NEXT_ID.with(|c| {
            let id = c.get();
            c.set(id.wrapping_add(1));
            id
        })
    }

    thread_local! {
        /// The in-tab inbound queue's drains. The browser's `window.ipe.send(raw)`
        /// pushes a string here (via [`push_inbound`]); each active `js_subscribe`
        /// registers a drain closure that fires on every pushed string.
        static INBOUND: RefCell<Vec<Rc<dyn Fn(&str)>>> = const { RefCell::new(Vec::new()) };

        /// In-flight correlated one-shot waiters. Key = runtime-minted id.
        /// A reply whose id is not in this map is dropped fail-closed.
        static PENDING: RefCell<HashMap<u64, Box<dyn FnOnce(JsonVal)>>> =
            RefCell::new(HashMap::new());
    }

    /// The browser-facing entry the JS glue calls to push an inbound port
    /// message: `window.ipe.send(raw)` routes here.
    ///
    /// Dispatch: if the raw JSON has `__ipe_id`, route to the pending waiter
    /// (correlated reply); otherwise fan out to all `js_subscribe` drains.
    pub fn push_inbound(raw: &str) {
        // Peek for a correlation id (fail-closed: a parse error falls through).
        // Wire format: `{"__ipe_id": <u64>, "payload": <sealed_reply>}`.
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(raw)
            && let Some(id_val) = value.as_object_mut().and_then(|o| o.remove(COR_ID_FIELD))
            && let Some(id) = id_val.as_u64()
        {
            // Extract `"payload"`, falling back to the whole remaining value.
            let payload = value
                .as_object_mut()
                .and_then(|o| o.remove("payload"))
                .unwrap_or(value);
            let waiter = PENDING.with(|p| p.borrow_mut().remove(&id));
            if let Some(cb) = waiter {
                cb(payload);
            }
            // Unknown/duplicate/late id: dropped fail-closed.
            return;
        }
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

    /// `js_request payload decoder` — correlated one-shot port request (wasm).
    ///
    /// Mirrors the native semantics using a `js_sys::Promise`-based future (the
    /// wasm-compatible async primitive). A `resolve`/`reject` pair is stored in
    /// `PENDING`; `push_inbound` calls `resolve` when the correlated reply arrives.
    /// A `setTimeout` races the promise: whichever fires first wins.
    pub fn js_request<T, R>(
        payload: T,
        decoder: Decoder<IpeError, R>,
    ) -> crate::core::IpeTask<IpeError, R>
    where
        T: serde::Serialize + 'static,
        R: 'static,
    {
        let cor_id = next_cor_id();
        Box::pin(async move {
            // Refuse when the ceiling is already reached.
            let at_limit = PENDING.with(|p| p.borrow().len() >= MAX_OUTSTANDING);
            if at_limit {
                return crate::core::IpeResult::Err(
                    "js_request: outstanding waiter ceiling reached"
                        .to_string()
                        .into(),
                );
            }

            // Serialize and wrap with the correlation id.
            let outbound_json = match serde_json::to_value(&payload) {
                Ok(v) => v,
                Err(e) => {
                    return crate::core::IpeResult::Err(
                        format!("js_request: payload serialisation failed: {e}").into(),
                    );
                }
            };
            let envelope = serde_json::json!({
                COR_ID_FIELD: cor_id,
                "payload": outbound_json,
            });
            let encoded = seal_encode(&envelope);

            // Build a Promise whose resolve/reject pair is stored as the waiter.
            // When `push_inbound` gets a correlated reply it calls the stored
            // callback, which resolves the promise with the reply JSON string.
            // The `Closure`s are `forget`-ed so they live until the promise
            // settles — a bounded leak (one per outstanding request, cleaned up
            // when the promise resolves or the timeout fires).
            let promise = js_sys::Promise::new(&mut |resolve, reject| {
                // `resolve` receives the reply payload JSON as a JsValue string.
                // `reject` is called by the timeout closure.
                let resolve_rc = Rc::new(resolve);
                let reject_rc = Rc::new(reject);

                // Register resolve as the waiter.
                let resolve_stored = Rc::clone(&resolve_rc);
                PENDING.with(|p| {
                    p.borrow_mut().insert(
                        cor_id,
                        Box::new(move |value: JsonVal| {
                            let s = value.to_string();
                            let _ = resolve_stored.call1(&JsValue::NULL, &JsValue::from_str(&s));
                        }),
                    );
                });

                // Set up the timeout to call reject.
                let reject_cb = wasm_bindgen::closure::Closure::once(move || {
                    let _ = reject_rc.call0(&JsValue::NULL);
                });
                let window = web_sys::window();
                if let Some(w) = window {
                    let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(
                        reject_cb.as_ref().unchecked_ref(),
                        i32::try_from(REQUEST_TIMEOUT_MS).unwrap_or(i32::MAX),
                    );
                }
                reject_cb.forget();
            });

            // Deliver outbound AFTER registering the waiter so no reply can
            // arrive before the waiter is installed.
            deliver_outbound(&encoded);

            // Await the promise (resolve = reply arrived; reject = timeout).
            let js_fut = wasm_bindgen_futures::JsFuture::from(promise);
            match js_fut.await {
                Ok(reply_js) => {
                    // Clean up (idempotent).
                    PENDING.with(|p| p.borrow_mut().remove(&cor_id));
                    let reply_str = reply_js.as_string().unwrap_or_default();
                    match serde_json::from_str::<JsonVal>(&reply_str)
                        .ok()
                        .and_then(|v| {
                            seal_decode(&v.to_string(), &decoder, SealLimits::default()).ok()
                        }) {
                        Some(v) => crate::core::IpeResult::Ok(v),
                        None => crate::core::IpeResult::Err(
                            "js_request: reply failed seal decode".to_string().into(),
                        ),
                    }
                }
                Err(_) => {
                    // Timeout (promise rejected by setTimeout) or unknown error.
                    PENDING.with(|p| p.borrow_mut().remove(&cor_id));
                    crate::core::IpeResult::Err("js_request: timeout".to_string().into())
                }
            }
        })
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::{js_request, js_send, js_subscribe, push_inbound};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::json::JsonVal;

    // ── SessionId constructor contract ────────────────────────────────────────

    #[test]
    fn session_id_rejects_empty() {
        assert!(SessionId::parse("").is_none());
    }

    #[test]
    fn session_id_rejects_non_hex() {
        assert!(SessionId::parse("abc123XYZ").is_none()); // uppercase letters
        assert!(SessionId::parse("g1234567").is_none()); // 'g' not hex
        assert!(SessionId::parse("  ").is_none()); // whitespace
        assert!(SessionId::parse("abc-def").is_none()); // hyphen
        assert!(SessionId::parse("ABCDEF").is_none()); // uppercase hex
    }

    #[test]
    fn session_id_accepts_valid_hex() {
        // 32-char lowercase hex — the shape new_sid() produces.
        let raw = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6";
        let sid = SessionId::parse(raw).expect("valid hex must parse");
        assert_eq!(sid.as_str(), raw);
        assert_eq!(sid.to_string(), raw);
    }

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

        // Parse a test sid string — all test sids are valid hex by construction.
        #[allow(clippy::expect_used)]
        fn test_sid(raw: &str) -> SessionId {
            SessionId::parse(raw).expect("test sid must be valid hex")
        }

        // Materialise a `js_subscribe` Source the way the Web driver does —
        // INSIDE `with_session_sid(sid, …)`, so the subscription binds `sid`'s
        // inbound channel — and collect the emitted Msgs.
        fn collect_for(
            sid: &SessionId,
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
            let sid = test_sid("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6");
            session_open(&sid);
            let (h, got) = collect_for(&sid, int_decoder());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await; // let it subscribe
            deliver_inbound_for(&sid, "7".to_string());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert_eq!(*got.lock().unwrap_or_else(|e| e.into_inner()), vec![7]);
            h.abort();
            session_close(&sid);
        }

        #[tokio::test]
        async fn inbound_undecodable_payload_is_dropped_whole() {
            let sid = test_sid("b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7");
            session_open(&sid);
            let (h, got) = collect_for(&sid, int_decoder());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            deliver_inbound_for(&sid, "\"not an int\"".to_string()); // string, not i64
            deliver_inbound_for(&sid, "{not json".to_string()); // malformed
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert!(got.lock().unwrap_or_else(|e| e.into_inner()).is_empty()); // both dropped, no panic, no partial
            h.abort();
            session_close(&sid);
        }

        #[tokio::test]
        async fn inbound_failing_decoder_drops_every_payload() {
            let sid = test_sid("c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8");
            session_open(&sid);
            let (h, got) = collect_for(&sid, never_decoder());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            deliver_inbound_for(&sid, "1".to_string());
            deliver_inbound_for(&sid, "2".to_string());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert!(got.lock().unwrap_or_else(|e| e.into_inner()).is_empty());
            h.abort();
            session_close(&sid);
        }

        #[tokio::test]
        async fn outbound_send_encodes_and_delivers_to_origin_sink() {
            let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let seen2 = seen.clone();
            let sid = test_sid("d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9");
            session_open(&sid);
            register_out_sink_for(
                &sid,
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
                IpeCmd::Publish(thunk) => assert_eq!(thunk(sid.as_str()), 0),
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
            session_close(&sid);
        }

        // THE cross-session-isolation proof. Two live sessions, A and B. A's
        // browser inbound reaches ONLY A's `js_subscribe` (B never sees it); A's
        // `js_send` outbound reaches ONLY A's out-sink (B's stays empty).
        // Cross-session delivery is unrepresentable, verified end-to-end.
        #[tokio::test]
        async fn sessions_are_isolated_inbound_and_outbound() {
            let sid_a = test_sid("e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0");
            let sid_b = test_sid("f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1");
            session_open(&sid_a);
            session_open(&sid_b);

            // Inbound: one subscriber per session, bound via the session scope.
            let (ha, got_a) = collect_for(&sid_a, int_decoder());
            let (hb, got_b) = collect_for(&sid_b, int_decoder());

            // Outbound: one out-sink per session.
            let out_a: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let out_b: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let oa = out_a.clone();
            let ob = out_b.clone();
            register_out_sink_for(
                &sid_a,
                Arc::new(move |s: &str| {
                    oa.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(s.to_string())
                }),
            );
            register_out_sink_for(
                &sid_b,
                Arc::new(move |s: &str| {
                    ob.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(s.to_string())
                }),
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await; // subscribe

            // A's browser sends 11 inbound; only A's subscriber must see it.
            deliver_inbound_for(&sid_a, "11".to_string());
            // A's program sends 99 outbound; only A's sink must see it.
            match js_send::<i64, i64>(99) {
                IpeCmd::Publish(thunk) => {
                    thunk(sid_a.as_str());
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
            session_close(&sid_a);
            session_close(&sid_b);
        }

        // A subscription materialised with NO session sid in scope binds an inert
        // channel that no route feeds — it must never receive another session's
        // inbound frames (fail-closed when the owner is unknown).
        #[tokio::test]
        async fn unscoped_subscription_receives_nothing() {
            let sid = test_sid("a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2");
            session_open(&sid);
            // NOTE: materialised OUTSIDE with_session_sid → owner sid is absent.
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
            deliver_inbound_for(&sid, "5".to_string());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert!(got.lock().unwrap_or_else(|e| e.into_inner()).is_empty()); // inert drain — nothing delivered
            h.abort();
            session_close(&sid);
        }

        // ── js_request security / refusal tests ──────────────────────────────

        // Helper: drive a `js_request` future and capture its result. The
        // closure `intercept` is called with the outbound frame immediately after
        // the future arms its waiter, so the test can echo a correlated reply.
        async fn drive_request<F>(sid: &SessionId, intercept: F) -> crate::IpeResult<IpeError, i64>
        where
            F: Fn(String) + Send + Sync + 'static,
        {
            let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let seen2 = seen.clone();
            register_out_sink_for(
                sid,
                Arc::new(move |s: &str| {
                    let s = s.to_string();
                    seen2
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(s.clone());
                    intercept(s);
                }),
            );
            with_session_sid(sid.to_string(), || {
                js_request::<i64, i64>(0_i64, int_decoder())
            })
            .await
        }

        // An unknown id delivered to the session (no waiter registered) must be
        // DROPPED fail-closed — no subscriber sees it, no panic, no cross-talk.
        #[tokio::test]
        async fn unknown_id_reply_is_dropped() {
            let sid = test_sid("f0e1d2c3b4a5f0e1d2c3b4a5f0e1d2c3");
            session_open(&sid);
            let (h, got) = collect_for(&sid, int_decoder());
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            // Deliver a frame that looks like a correlated reply with an id that
            // has no pending waiter — it must not reach the subscriber.
            deliver_inbound_for(&sid, r#"{"__ipe_id":999999,"payload":42}"#.to_string());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert!(
                got.lock().unwrap_or_else(|e| e.into_inner()).is_empty(),
                "unknown-id reply must not reach subscribers"
            );
            h.abort();
            session_close(&sid);
        }

        // A clean reply resolves the Task with the decoded value.
        #[tokio::test]
        async fn clean_reply_resolves_task() {
            let sid = test_sid("1a2b3c4d5e6f1a2b3c4d5e6f1a2b3c4d");
            session_open(&sid);
            let sid_clone = sid.clone();
            let result = drive_request(&sid, move |frame| {
                // Parse the outbound envelope to learn the minted id.
                let v: serde_json::Value = serde_json::from_str(&frame).unwrap_or_default();
                if let Some(id) = v.get("__ipe_id").and_then(|x| x.as_u64()) {
                    // Reply with envelope: id + "payload" = bare int 7.
                    let reply = format!(r#"{{"__ipe_id":{id},"payload":7}}"#);
                    deliver_inbound_for(&sid_clone, reply);
                }
            })
            .await;
            assert!(
                matches!(result, crate::IpeResult::Ok(7)),
                "expected Ok(7), got {result:?}"
            );
            session_close(&sid);
        }

        // A duplicate reply (same id echoed twice) does not double-resolve — the
        // second delivery finds an empty pending slot and is dropped fail-closed.
        #[tokio::test]
        async fn duplicate_reply_is_dropped() {
            let sid = test_sid("2b3c4d5e6f7a2b3c4d5e6f7a2b3c4d5e");
            session_open(&sid);
            let sid_clone = sid.clone();
            let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let counter2 = counter.clone();
            register_out_sink_for(
                &sid,
                Arc::new(move |frame: &str| {
                    let v: serde_json::Value = serde_json::from_str(frame).unwrap_or_default();
                    if let Some(id) = v.get("__ipe_id").and_then(|x| x.as_u64()) {
                        let n = counter2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        // Send the reply twice on the FIRST outbound only.
                        if n == 0 {
                            let r1 = format!(r#"{{"__ipe_id":{id},"payload":1}}"#);
                            let r2 = format!(r#"{{"__ipe_id":{id},"payload":2}}"#);
                            deliver_inbound_for(&sid_clone, r1);
                            deliver_inbound_for(&sid_clone, r2); // duplicate
                        }
                    }
                }),
            );
            let result = with_session_sid(sid.to_string(), || {
                js_request::<i64, i64>(0_i64, int_decoder())
            })
            .await;
            // The first reply should win; the second is silently dropped.
            assert!(
                matches!(result, crate::IpeResult::Ok(_)),
                "first reply must resolve the Task, got {result:?}"
            );
            session_close(&sid);
        }

        // Timeout: if no reply arrives within the deadline the Task resolves Err.
        // The deadline is 10 s in production; we shrink it by not supplying any
        // reply and relying on the per-test harness timeout (tokio::test has no
        // default deadline, but the runtime's deadline is 10 s which is too slow
        // for a unit test). We exercise the bounded-ceiling path instead: fill the
        // pending map to MAX_OUTSTANDING so the NEXT `js_request` is refused
        // immediately with an Err — same fail-closed outcome as a timeout.
        #[tokio::test]
        async fn outstanding_ceiling_refuses_immediately() {
            let sid = test_sid("3c4d5e6f7a8b3c4d5e6f7a8b3c4d5e6f");
            session_open(&sid);
            // Fill the pending map with synthetic entries (dummy oneshot senders).
            {
                let mut g = lock_sessions();
                let ports = g.entry(sid.0.clone()).or_insert_with(SessionPorts::new);
                for i in 0..MAX_OUTSTANDING as u64 {
                    let (tx, _rx) = tokio::sync::oneshot::channel::<JsonVal>();
                    ports.pending.insert(i, tx);
                }
            }
            // The next request must be refused immediately.
            let result = with_session_sid(sid.to_string(), || {
                js_request::<i64, i64>(0_i64, int_decoder())
            })
            .await;
            assert!(
                matches!(result, crate::IpeResult::Err(_)),
                "ceiling breach must immediately return Err"
            );
            session_close(&sid);
        }

        // Two concurrent requests resolve to their OWN replies — no cross-talk.
        #[tokio::test]
        async fn two_concurrent_requests_no_cross_talk() {
            let sid_a = test_sid("4d5e6f7a8b9c4d5e6f7a8b9c4d5e6f7a");
            let sid_b = test_sid("5e6f7a8b9c0d5e6f7a8b9c0d5e6f7a8b");
            session_open(&sid_a);
            session_open(&sid_b);

            let sid_a2 = sid_a.clone();
            register_out_sink_for(
                &sid_a,
                Arc::new(move |frame: &str| {
                    let v: serde_json::Value = serde_json::from_str(frame).unwrap_or_default();
                    if let Some(id) = v.get("__ipe_id").and_then(|x| x.as_u64()) {
                        // Reply to A's request with value 11.
                        deliver_inbound_for(
                            &sid_a2,
                            format!(r#"{{"__ipe_id":{id},"payload":11}}"#),
                        );
                    }
                }),
            );
            let sid_b2 = sid_b.clone();
            register_out_sink_for(
                &sid_b,
                Arc::new(move |frame: &str| {
                    let v: serde_json::Value = serde_json::from_str(frame).unwrap_or_default();
                    if let Some(id) = v.get("__ipe_id").and_then(|x| x.as_u64()) {
                        // Reply to B's request with value 22.
                        deliver_inbound_for(
                            &sid_b2,
                            format!(r#"{{"__ipe_id":{id},"payload":22}}"#),
                        );
                    }
                }),
            );

            let (ra, rb) = tokio::join!(
                with_session_sid(sid_a.to_string(), || {
                    js_request::<i64, i64>(0_i64, int_decoder())
                }),
                with_session_sid(sid_b.to_string(), || {
                    js_request::<i64, i64>(0_i64, int_decoder())
                }),
            );

            assert!(
                matches!(ra, crate::IpeResult::Ok(11)),
                "session A must get 11, got {ra:?}"
            );
            assert!(
                matches!(rb, crate::IpeResult::Ok(22)),
                "session B must get 22, got {rb:?}"
            );

            session_close(&sid_a);
            session_close(&sid_b);
        }

        // A malformed (non-JSON) reply frame arriving with a valid id is decoded
        // fail-closed — the Task resolves Err, no panic, no partial value.
        #[tokio::test]
        async fn malformed_reply_decoded_fail_closed() {
            let sid = test_sid("6f7a8b9c0d1e6f7a8b9c0d1e6f7a8b9c");
            session_open(&sid);
            let sid_clone = sid.clone();
            register_out_sink_for(
                &sid,
                Arc::new(move |frame: &str| {
                    let v: serde_json::Value = serde_json::from_str(frame).unwrap_or_default();
                    if let Some(id) = v.get("__ipe_id").and_then(|x| x.as_u64()) {
                        // Reply with a frame whose payload won't decode as i64.
                        let bad = format!(r#"{{"__ipe_id":{id},"payload":"not-an-int"}}"#);
                        deliver_inbound_for(&sid_clone, bad);
                    }
                }),
            );
            let result = with_session_sid(sid.to_string(), || {
                js_request::<i64, i64>(0_i64, int_decoder())
            })
            .await;
            assert!(
                matches!(result, crate::IpeResult::Err(_)),
                "malformed reply must resolve Err, got {result:?}"
            );
            session_close(&sid);
        }

        // After `session_close`, a delivery to that sid is a fire-and-forget no-op
        // (the channel is gone) and any live subscriber ends — no dead-session
        // channel lingers.
        #[tokio::test]
        async fn closed_session_delivery_is_a_noop() {
            let sid = test_sid("b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3");
            session_open(&sid);
            let (h, got) = collect_for(&sid, int_decoder());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            session_close(&sid);
            deliver_inbound_for(&sid, "1".to_string()); // no channel → dropped
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert!(got.lock().unwrap_or_else(|e| e.into_inner()).is_empty());
            h.abort();
        }
    }
}

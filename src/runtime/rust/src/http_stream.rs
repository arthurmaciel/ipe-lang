//! Ipe.Http.Stream — incremental HTTP response bodies (client side).
//!
//! Reads an outbound HTTP response body chunk-by-chunk via reqwest's
//! `bytes_stream()` instead of buffering the whole body (`Http.get`).
//!
//! Surface ported on the Rust backend:
//!
//!   * `open : HttpRequest -> Task Error StreamId`  — fire the request, resolve
//!     once the response headers arrive; register the byte stream under a
//!     freshly minted handle.
//!   * `forEachChunk : StreamId -> (String -> Task Error ()) -> Task Error ()`
//!     — synchronous drain (the relay shape — usable inside a plain
//!     Ipe.Http.Server handler, no TEA loop required).
//!   * `close : StreamId -> Task Error ()` — drop the stream / release the conn.
//!
//! The Sub-tier `chunks` (dispatching `ChunkEvent` Msgs into a TEA update loop)
//! is ported via `sub_subscribe_stream` + the bridged `ChunkEvent` enum below —
//! it drives a `Cli.tea` (or any `console_app`-hosted) TEA loop, the same
//! way `ws_client`'s `onMessage` does.
//!
//! A `StreamId` is an unforgeable capability: only `open` mints one (a 128-bit
//! key from the OS CSPRNG), no source form names one, every decode parses the
//! exact 32-char lowercase-hex wire form once, and `Debug` never renders the
//! key. The one registry refuses every handle it does not hold with a typed
//! `InvalidInput`, so no party reaches a stream it did not open.

use super::*;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::num::NonZeroU128;
use std::sync::{Mutex, OnceLock};

/// Opaque handle for an in-flight HTTP streaming response.
///
/// Backs the opaque `StreamId` type of `Ipe.Http.Stream`. The key is private:
/// the only constructors are `open`'s mint and the parsing `Deserialize`.
/// `Copy` so it can be passed by value to Task closures without cloning.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct IpeStreamId {
    key: StreamKey,
}

/// The registry key behind a `StreamId`: nonzero, so no zeroed value names a stream.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct StreamKey(NonZeroU128);

/// Hex digits in the wire form of a key (128 bits, 4 per digit).
const KEY_HEX_LEN: usize = 32;

/// Draws a mint makes before giving up on a zero or colliding key.
const MINT_ATTEMPTS: usize = 4;

/// The fixed refusal of every malformed wire handle; it never echoes the input.
const INVALID_ID: &str = "invalid stream id";
const UNKNOWN_STREAM: &str = "http stream: unknown or ended stream";
const STREAM_BUSY: &str = "http stream: already being consumed";
const TOO_MANY_STREAMS: &str = "http stream: too many open streams";
const MINT_EXHAUSTED: &str = "http stream: key mint exhausted";
const ENTROPY_UNAVAILABLE: &str = "http stream: entropy unavailable";
const SEQUENCE_EXHAUSTED: &str = "http stream: sequence exhausted";

impl StreamKey {
    /// Parses the exact wire form: 32 lowercase hex digits, value nonzero.
    fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != KEY_HEX_LEN {
            return None;
        }
        let mut acc: u128 = 0;
        for &b in bytes {
            let digit = match b {
                b'0'..=b'9' | b'a'..=b'f' => char::from(b).to_digit(16)?,
                _ => return None,
            };
            acc = (acc << 4) | u128::from(digit);
        }
        NonZeroU128::new(acc).map(Self)
    }
}

impl std::fmt::Debug for IpeStreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StreamId(<opaque>)")
    }
}

impl serde::Serialize for IpeStreamId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{:032x}", self.key.0.get()))
    }
}

impl<'de> serde::Deserialize<'de> for IpeStreamId {
    /// Parses the exact key string once; every refusal (an integer, a
    /// malformed string, any other shape) is the fixed `INVALID_ID` text, so
    /// no decoder error echoes the input.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer
            .deserialize_str(StreamIdVisitor)
            .map_err(|_| serde::de::Error::custom(INVALID_ID))
    }
}

/// Accepts only a string in the exact key form; every other shape falls to
/// the visitor's default refusal.
struct StreamIdVisitor;

impl serde::de::Visitor<'_> for StreamIdVisitor {
    type Value = IpeStreamId;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a stream id")
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<IpeStreamId, E> {
        StreamKey::parse(v)
            .map(|key| IpeStreamId { key })
            .ok_or_else(|| E::custom(INVALID_ID))
    }
}

/// Mints a fresh key from `source`, redrawing a zero or a `live` collision.
///
/// At most `MINT_ATTEMPTS` draws; then `Unavailable`. A failing source is
/// `Unavailable` at once.
fn mint_with<S, L>(mut source: S, live: L) -> Result<StreamKey, IpeError>
where
    S: FnMut(&mut [u8; 16]) -> Result<(), getrandom::Error>,
    L: Fn(StreamKey) -> bool,
{
    for _ in 0..MINT_ATTEMPTS {
        let mut buf = [0u8; 16];
        source(&mut buf).map_err(|_| IpeError::unavailable(ENTROPY_UNAVAILABLE.to_owned()))?;
        if let Some(raw) = NonZeroU128::new(u128::from_le_bytes(buf)) {
            let key = StreamKey(raw);
            if !live(key) {
                return Ok(key);
            }
        }
    }
    Err(IpeError::unavailable(MINT_EXHAUSTED.to_owned()))
}

/// The OS CSPRNG, the production key source.
fn os_entropy(buf: &mut [u8; 16]) -> Result<(), getrandom::Error> {
    getrandom::getrandom(buf)
}

/// `Ipe.Http.Stream.ChunkEvent` — one incremental event on a stream.
/// Bridged (via `runtimeOpaqueTypes`) so the runtime can CONSTRUCT it to hand to
/// the user's `toMsg : ChunkEvent -> msg` callback; user code only ever
/// pattern-matches it. Generic over the Ipê error type `E` (always `IpeError`
/// in practice — pinned at the call site) because `Errored` carries an `Error`.
/// Variant names match the Ipê constructors verbatim so codegen's match arms
/// (`ChunkEvent::Chunk(s)` / `::Done` / `::Errored(e)`) resolve through the
/// `pub type` alias the bridge emits.
// Serde derives: a Web `Msg` may carry a `ChunkEvent` payload, and Web
// messages round-trip through the session store (serde boundary). The derive
// bounds require `E: Serialize/Deserialize`, which holds for both inhabitants
// (`String` and `IpeError`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ChunkEvent<E> {
    Chunk(String),
    Done,
    Errored(E),
}

// Hard cap on registry entries (parked, draining, and ended tombstones). A
// full registry evicts the oldest ended tombstone, else the oldest parked
// stream (its response/connection drops at once); a draining stream is never
// evicted, so a registry full of draining streams refuses `open`.
const CLIENT_STREAMS_MAX: usize = 1024;

/// Where a registered stream is in its life.
enum Slot<V> {
    /// Opened, its response parked until a drain or a close.
    Parked(V),
    /// One drain owns the response; a close ends it at the next chunk.
    Draining,
    /// Drained or closed; kept as a tombstone so a re-subscribe stays quiet.
    Ended,
}

/// One registry entry: an insertion order (never an identity) and its slot.
struct Entry<V> {
    seq: u64,
    slot: Slot<V>,
}

/// What a `chunks` subscribe must do.
enum Subscription<V> {
    /// First subscribe of a parked stream: drain this response.
    Drain(V),
    /// The stream is draining or ended: nothing to start.
    Active,
    /// The handle names no stream: emit one `Errored`; a tombstone now dedups it.
    Refused,
    /// The handle names no stream and the registry is full of draining
    /// streams, so no tombstone could dedup the refusal: emit nothing.
    Unrecorded,
}

/// What `close` does to an entry.
enum CloseAct {
    /// A drain owns the response: mark it ended so the drain stops.
    EndInPlace,
    /// A parked response nothing reads: drop it and its entry.
    Remove,
    /// The stream already ended: nothing to release, so the close is refused.
    Refuse,
}

/// The one registry of client streams; every state change is one method under one lock.
struct StreamRegistry<V> {
    map: HashMap<StreamKey, Entry<V>>,
    next_seq: u64,
}

impl<V> StreamRegistry<V> {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            next_seq: 0,
        }
    }

    /// The entry a full registry gives up for one more, or `None` when there is room.
    fn victim(&self) -> Result<Option<StreamKey>, IpeError> {
        if self.map.len() < CLIENT_STREAMS_MAX {
            return Ok(None);
        }
        let oldest = |want_ended: bool| {
            self.map
                .iter()
                .filter(|(_, e)| match e.slot {
                    Slot::Ended => want_ended,
                    Slot::Parked(_) => !want_ended,
                    Slot::Draining => false,
                })
                .min_by_key(|(_, e)| e.seq)
                .map(|(k, _)| *k)
        };
        oldest(true)
            .or_else(|| oldest(false))
            .map(Some)
            .ok_or_else(|| IpeError::unavailable(TOO_MANY_STREAMS.to_owned()))
    }

    fn bump_seq(&mut self) -> Result<u64, IpeError> {
        let seq = self.next_seq;
        self.next_seq = seq
            .checked_add(1)
            .ok_or_else(|| IpeError::unavailable(SEQUENCE_EXHAUSTED.to_owned()))?;
        Ok(seq)
    }

    /// Makes room, then records `slot` under `key`.
    fn insert(&mut self, key: StreamKey, slot: Slot<V>) -> Result<(), IpeError> {
        let victim = self.victim()?;
        let seq = self.bump_seq()?;
        if let Some(victim) = victim {
            self.map.remove(&victim);
        }
        self.map.insert(key, Entry { seq, slot });
        Ok(())
    }

    /// Parks `value` under a freshly minted handle.
    fn open<S>(&mut self, value: V, source: S) -> Result<IpeStreamId, IpeError>
    where
        S: FnMut(&mut [u8; 16]) -> Result<(), getrandom::Error>,
    {
        // Refuse a registry full of draining streams before drawing entropy.
        self.victim()?;
        let key = mint_with(source, |k| self.map.contains_key(&k))?;
        self.insert(key, Slot::Parked(value))?;
        Ok(IpeStreamId { key })
    }

    /// Hands a parked response to one drain; any other state is a typed refusal.
    fn take_for_drain(&mut self, key: StreamKey) -> Result<V, IpeError> {
        let Some(entry) = self.map.get_mut(&key) else {
            return Err(IpeError::invalid_input(UNKNOWN_STREAM.to_owned()));
        };
        match std::mem::replace(&mut entry.slot, Slot::Draining) {
            Slot::Parked(value) => Ok(value),
            Slot::Draining => Err(IpeError::conflict(STREAM_BUSY.to_owned())),
            Slot::Ended => {
                entry.slot = Slot::Ended;
                Err(IpeError::invalid_input(UNKNOWN_STREAM.to_owned()))
            }
        }
    }

    /// Releases a live stream. A handle naming no live stream (never opened,
    /// already closed, ended, or evicted) is a typed refusal that leaves the
    /// registry unchanged.
    fn close(&mut self, key: StreamKey) -> Result<(), IpeError> {
        let Some(entry) = self.map.get_mut(&key) else {
            return Err(IpeError::invalid_input(UNKNOWN_STREAM.to_owned()));
        };
        let act = match entry.slot {
            Slot::Draining => CloseAct::EndInPlace,
            Slot::Parked(_) => CloseAct::Remove,
            Slot::Ended => CloseAct::Refuse,
        };
        match act {
            CloseAct::EndInPlace => entry.slot = Slot::Ended,
            CloseAct::Remove => {
                self.map.remove(&key);
            }
            CloseAct::Refuse => return Err(IpeError::invalid_input(UNKNOWN_STREAM.to_owned())),
        }
        Ok(())
    }

    /// Decides one `chunks` subscribe.
    fn subscribe(&mut self, key: StreamKey) -> Subscription<V> {
        if let Some(entry) = self.map.get_mut(&key) {
            return match std::mem::replace(&mut entry.slot, Slot::Draining) {
                Slot::Parked(value) => Subscription::Drain(value),
                Slot::Draining => Subscription::Active,
                Slot::Ended => {
                    entry.slot = Slot::Ended;
                    Subscription::Active
                }
            };
        }
        match self.insert(key, Slot::Ended) {
            Ok(()) => Subscription::Refused,
            Err(_) => Subscription::Unrecorded,
        }
    }

    fn is_draining(&self, key: StreamKey) -> bool {
        self.map
            .get(&key)
            .is_some_and(|e| matches!(e.slot, Slot::Draining))
    }

    /// Ends a drain: a still-draining entry becomes a tombstone.
    fn finish_drain(&mut self, key: StreamKey) {
        if let Some(entry) = self.map.get_mut(&key)
            && matches!(entry.slot, Slot::Draining)
        {
            entry.slot = Slot::Ended;
        }
    }
}

// Contract: every `open` should be paired with a `forEachChunk`/`chunks` drain
// or a `close` — each releases the parked response + its connection. The 30s
// connect timeout bounds only the header stage; `CLIENT_STREAMS_MAX` bounds the
// registry under abandoned-stream workloads.
fn registry() -> &'static Mutex<StreamRegistry<reqwest::Response>> {
    static R: OnceLock<Mutex<StreamRegistry<reqwest::Response>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(StreamRegistry::new()))
}

/// Runs `f` under the registry lock.
///
/// A poisoned lock is recovered: no method leaves a cross-entry invariant
/// half-written.
fn with_registry<T>(f: impl FnOnce(&mut StreamRegistry<reqwest::Response>) -> T) -> T {
    let mut guard = registry().lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

/// Ends a drain when dropped, so a cancelled drain never pins its slot as draining.
struct DrainLease {
    key: StreamKey,
}

impl DrainLease {
    fn still_owned(&self) -> bool {
        with_registry(|r| r.is_draining(self.key))
    }
}

impl Drop for DrainLease {
    fn drop(&mut self) {
        with_registry(|r| r.finish_drain(self.key));
    }
}

/// `Ipe.Http.Stream.open : HttpRequest -> Task Error StreamId`
///
/// Returns a freshly minted `IpeStreamId` handle for the parked response.
///
/// No whole-request timeout — streams may run for minutes (LLM completions);
/// a 30s connect timeout bounds the header stage only.
pub fn http_stream_open<E: From<String> + From<IpeError> + Send + 'static>(
    req: HttpRequest,
) -> IpeTask<E, IpeStreamId> {
    Box::pin(async move {
        // SSRF guard: resolve + validate + pin, and the per-redirect re-check,
        // through the shared helper, identical to Http.get/post.
        let builder =
            reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(30));
        let builder = match crate::http_client::ssrf_apply(builder, &req.url, req.redirects).await {
            Ok(b) => b,
            Err(refusal) => {
                return IpeResult::Err(E::from(IpeError::invalid_input(format!(
                    "http: {refusal}"
                ))));
            }
        };
        let client = match builder.build() {
            Ok(c) => c,
            Err(e) => {
                return IpeResult::Err(E::from(IpeError::unavailable(format!(
                    "http.stream.open: client: {e}"
                ))));
            }
        };
        // `HttpMethod` is an ADT — every variant maps to a known reqwest
        // constant (no runtime failure possible here).
        let method = crate::http_client::method_to_reqwest(req.method);
        let mut rb = client.request(method, &req.url);
        for (k, v) in &req.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        if !req.body.is_empty() {
            rb = rb.body(req.body.clone());
        }
        let resp = match rb.send().await {
            Ok(r) => r,
            // [B8] The reqwest error `Debug`/`Display` (and `req.url`) can echo the
            // target URL / request headers / bearer / API key. Route through the
            // correlation-id redaction helper: raw detail → server log under a ref
            // id; Ipê sees only a fixed generic message.
            Err(e) => return IpeResult::Err(crate::http_client::redacted_transport_error(e)),
        };
        // HTTP error statuses (4xx/5xx) still surface as a stream — the body may
        // carry the error payload the caller wants to read. Mirrors Http.get
        // returning Ok with a 4xx status.
        match with_registry(|r| r.open(resp, os_entropy)) {
            Ok(sid) => IpeResult::Ok(sid),
            Err(e) => IpeResult::Err(E::from(e)),
        }
    })
}

/// `Ipe.Http.Stream.forEachChunk : StreamId -> (String -> Task Error ()) -> Task Error ()`
///
/// Drains the stream synchronously from the calling task, invoking `body chunk`
/// per chunk. Bridges the client consumer to a server producer
/// (`Server.Stream.emit`) inside one Ipe.Http.Server handler — the relay shape.
///
/// Semantics:
///   * clean EOF                     → Ok ()
///   * upstream read error           → Err e
///   * `body chunk` returns Err      → abort, close, Err e (fail-fast)
///   * `close` during the drain      → stop at the next chunk boundary, Ok ()
///   * a handle no open stream holds → Err `InvalidInput`
///   * a stream another drain owns   → Err `Conflict`
///   * the connection is always released on exit.
///
/// Backpressure: `body` runs synchronously per chunk; if it blocks on a slow
/// downstream (`Server.Stream.emit` to a bounded channel) the upstream read
/// naturally throttles.
pub fn http_stream_for_each_chunk<E, F>(sid: IpeStreamId, body: F) -> IpeTask<E, ()>
where
    E: From<String> + From<IpeError> + Send + 'static,
    F: Fn(String) -> IpeTask<E, ()> + Send + 'static,
{
    let key = sid.key;
    Box::pin(async move {
        let resp = match with_registry(|r| r.take_for_drain(key)) {
            Ok(r) => r,
            Err(e) => return IpeResult::Err(E::from(e)),
        };
        let lease = DrainLease { key };
        let mut stream = resp.bytes_stream();
        loop {
            if !lease.still_owned() {
                break IpeResult::Ok(());
            }
            match stream.next().await {
                Some(Ok(bytes)) => {
                    #[allow(clippy::disallowed_methods)]
                    // a streamed chunk reaches Ipê as `String` text
                    let chunk = String::from_utf8_lossy(&bytes).into_owned();
                    match body(chunk).await {
                        IpeResult::Ok(()) => {}
                        IpeResult::Err(e) => break IpeResult::Err(e),
                    }
                }
                // [B8] redact the foreign reqwest read error (see open above).
                Some(Err(e)) => {
                    break IpeResult::Err(crate::http_client::redacted_transport_error(e));
                }
                None => break IpeResult::Ok(()),
            }
        }
        // `stream` (the response) and `lease` drop here: the connection is
        // released and the slot ends.
    })
}

/// `Ipe.Http.Stream.close : StreamId -> Task Error ()`
///
/// Releases a live stream: a parked response drops at once, a stream being
/// drained ends at its next chunk boundary. A handle naming no live stream
/// (never opened, already closed, ended by its drain, or evicted) is
/// `Err InvalidInput`, so a double close or a forged handle never passes for a
/// release. A caller wanting idempotence opts in with `Task.onError`.
pub fn http_stream_close<E: From<String> + From<IpeError> + Send + 'static>(
    sid: IpeStreamId,
) -> IpeTask<E, ()> {
    let key = sid.key;
    Box::pin(async move {
        match with_registry(|r| r.close(key)) {
            Ok(()) => IpeResult::Ok(()),
            Err(e) => IpeResult::Err(E::from(e)),
        }
    })
}

// ─── Sub-tier: chunks → ChunkEvent Msgs ─────────────────────────────────────

/// Ipe.Http.Stream.chunks → `Sub_subscribeStream`.
///
/// Returns a `IpeSub::Source` that, on first subscribe for a parked stream,
/// spawns a detached task draining the response and dispatching a `ChunkEvent`
/// Msg per chunk: `Chunk s` per UTF-8 byte chunk, `Done` on clean EOF,
/// `Errored e` on a read fault; a `close` during the drain stops it with no
/// further event. `subscriptions` is re-evaluated on every TEA `update`, so a
/// re-subscribe to a draining or ended stream starts nothing — the registry
/// decides that under its one lock. A handle no open stream holds gets exactly
/// one `Errored InvalidInput` (its tombstone dedups every re-subscribe). The
/// drain is DETACHED — `SubRuntime`'s abort-on-respawn only ever hits the dummy
/// handle, never the drain. `E` is pinned to `IpeError` at the call site.
/// `to_msg` is moved exclusively into the ONE detached `tokio::spawn` task
/// below (never behind a shared `Arc`, never read from two threads at once) --
/// the same shape as the sibling `sub_subscribe_topic` (`pubsub.rs`), whose
/// doc comment states the identical rationale. `Send` is therefore the full
/// and correct contract; `Sync` is NOT required. Over-declaring `+ Sync` would
/// be unsatisfiable: the codegen's generic first-class-function-value
/// rendering boxes the closure as `Box<dyn Fn(..) -> .. + Send + 'static>`
/// (deliberately `+Send`-only, since a trait object's auto-trait set is
/// exactly its bound list), so a `+ Sync` bound could never hold regardless of
/// what the boxed closure captured and every `Http.Stream.chunks` subscription
/// would fail `cargo build` with E0277 despite `ipe` accepting the program (a
/// THE-SEAL violation). The bound matches the actual (Send-only) usage rather
/// than re-wrapping the box in a fresh closure at the emit site (the technique
/// used for `html_on_raw_` / `ui_on_submit_` / `Ui.on*`), because THOSE
/// runtime slots are genuinely `Arc<dyn Fn + Send + Sync>` shared across a live
/// session's concurrently-serviced dispatch table -- a structurally different,
/// stronger requirement this kernel never has.
pub fn sub_subscribe_stream<E, M, F>(sid: IpeStreamId, to_msg: F) -> IpeSub<M>
where
    E: From<String> + From<IpeError> + Send + 'static,
    M: Send + 'static,
    F: Fn(ChunkEvent<E>) -> M + Send + 'static,
{
    let key = sid.key;
    IpeSub::Source(Box::new(move |emit| {
        match with_registry(|r| r.subscribe(key)) {
            Subscription::Drain(resp) => {
                // The lease moves into the task, so a task dropped before its
                // first poll still ends the slot.
                let lease = DrainLease { key };
                tokio::spawn(async move {
                    let mut stream = resp.bytes_stream();
                    loop {
                        if !lease.still_owned() {
                            break;
                        }
                        match stream.next().await {
                            Some(Ok(bytes)) => {
                                #[allow(clippy::disallowed_methods)]
                                // a streamed chunk reaches Ipê as `String` text
                                let chunk = String::from_utf8_lossy(&bytes).into_owned();
                                emit(to_msg(ChunkEvent::Chunk(chunk)));
                            }
                            Some(Err(e)) => {
                                // [B8] redact the foreign reqwest read error (see open above).
                                emit(to_msg(ChunkEvent::Errored(
                                    crate::http_client::redacted_transport_error(e),
                                )));
                                break;
                            }
                            None => {
                                emit(to_msg(ChunkEvent::Done));
                                break;
                            }
                        }
                    }
                });
            }
            Subscription::Refused => {
                emit(to_msg(ChunkEvent::Errored(E::from(
                    IpeError::invalid_input(UNKNOWN_STREAM.to_owned()),
                ))));
            }
            Subscription::Active | Subscription::Unrecorded => {}
        }
        tokio::spawn(async {}) // dummy handle for `SubRuntime` to abort harmlessly
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic key source: the n-th draw is the key `n`.
    fn counting_source() -> impl FnMut(&mut [u8; 16]) -> Result<(), getrandom::Error> {
        let mut next: u128 = 0;
        move |buf: &mut [u8; 16]| {
            next += 1;
            *buf = next.to_le_bytes();
            Ok(())
        }
    }

    fn key(n: u128) -> StreamKey {
        StreamKey(NonZeroU128::new(n).unwrap_or(NonZeroU128::MIN))
    }

    fn kind(e: &IpeError) -> IpeErrorKind {
        let IpeError::Error(k, _) = e;
        *k
    }

    fn message(e: &IpeError) -> &str {
        let IpeError::Error(_, info) = e;
        &info.message
    }

    fn slot_of(reg: &StreamRegistry<()>, k: StreamKey) -> Option<&Slot<()>> {
        reg.map.get(&k).map(|e| &e.slot)
    }

    #[test]
    fn for_each_chunk_unknown_key_is_invalid_input() {
        let mut reg = StreamRegistry::<()>::new();
        let refused = reg.take_for_drain(key(7));
        assert!(matches!(&refused, Err(e) if kind(e) == IpeErrorKind::InvalidInput));
        let Err(e) = refused else { return };
        assert_eq!(message(&e), UNKNOWN_STREAM);
        // Happy twin: an opened handle drains.
        let sid = reg.open((), counting_source()).unwrap();
        assert!(reg.take_for_drain(sid.key).is_ok());
    }

    fn assert_unknown(refused: &Result<(), IpeError>) {
        assert!(matches!(refused, Err(e) if kind(e) == IpeErrorKind::InvalidInput));
        assert!(matches!(refused, Err(e) if message(e) == UNKNOWN_STREAM));
    }

    #[test]
    fn close_unknown_key_refused() {
        let mut reg = StreamRegistry::<()>::new();
        let sid = reg.open((), counting_source()).unwrap();
        // A forged handle the registry never held.
        assert_unknown(&reg.close(key(99)));
        assert_eq!(reg.map.len(), 1);
        assert!(slot_of(&reg, key(99)).is_none());
        assert!(matches!(slot_of(&reg, sid.key), Some(Slot::Parked(()))));
        // Happy twin: closing the held handle removes it.
        assert!(reg.close(sid.key).is_ok());
        assert!(reg.map.is_empty());
    }

    #[test]
    fn close_twice_second_refused() {
        let mut reg = StreamRegistry::<()>::new();
        let parked = reg.open((), counting_source()).unwrap();
        assert!(reg.close(parked.key).is_ok());
        assert_unknown(&reg.close(parked.key));
        // A draining stream: the first close ends it, the second is refused
        // and leaves the tombstone in place.
        let mut reg = StreamRegistry::<()>::new();
        let draining = reg.open((), counting_source()).unwrap();
        assert!(reg.take_for_drain(draining.key).is_ok());
        assert!(reg.close(draining.key).is_ok());
        assert_unknown(&reg.close(draining.key));
        assert!(matches!(slot_of(&reg, draining.key), Some(Slot::Ended)));
    }

    #[test]
    fn close_after_drain_end_refused() {
        let mut reg = StreamRegistry::<()>::new();
        let sid = reg.open((), counting_source()).unwrap();
        assert!(reg.take_for_drain(sid.key).is_ok());
        reg.finish_drain(sid.key);
        assert_unknown(&reg.close(sid.key));
        assert!(matches!(slot_of(&reg, sid.key), Some(Slot::Ended)));
    }

    #[test]
    fn close_evicted_key_refused() {
        let mut reg = StreamRegistry::<()>::new();
        let mut source = counting_source();
        fill(&mut reg, &mut source);
        assert!(reg.open((), &mut source).is_ok());
        assert_unknown(&reg.close(key(1)));
        assert!(reg.close(key(2)).is_ok());
    }

    #[test]
    fn for_each_chunk_twice_second_is_conflict() {
        let mut reg = StreamRegistry::<()>::new();
        let sid = reg.open((), counting_source()).unwrap();
        assert!(reg.take_for_drain(sid.key).is_ok());
        let second = reg.take_for_drain(sid.key);
        assert!(matches!(&second, Err(e) if kind(e) == IpeErrorKind::Conflict));
        assert!(reg.is_draining(sid.key));
    }

    #[test]
    fn for_each_chunk_after_end_is_invalid_input() {
        let mut reg = StreamRegistry::<()>::new();
        let sid = reg.open((), counting_source()).unwrap();
        assert!(reg.take_for_drain(sid.key).is_ok());
        reg.finish_drain(sid.key);
        let again = reg.take_for_drain(sid.key);
        assert!(matches!(&again, Err(e) if kind(e) == IpeErrorKind::InvalidInput));
        assert!(matches!(slot_of(&reg, sid.key), Some(Slot::Ended)));
    }

    #[test]
    fn close_during_drain_ends_the_drain() {
        let mut reg = StreamRegistry::<()>::new();
        let sid = reg.open((), counting_source()).unwrap();
        assert!(reg.take_for_drain(sid.key).is_ok());
        assert!(reg.is_draining(sid.key));
        assert!(reg.close(sid.key).is_ok());
        assert!(!reg.is_draining(sid.key));
        reg.finish_drain(sid.key);
        assert!(matches!(slot_of(&reg, sid.key), Some(Slot::Ended)));
    }

    #[test]
    fn chunks_unknown_key_emits_one_errored_then_dedups() {
        let mut reg = StreamRegistry::<()>::new();
        assert!(matches!(reg.subscribe(key(5)), Subscription::Refused));
        assert!(matches!(reg.subscribe(key(5)), Subscription::Active));
        assert!(matches!(reg.subscribe(key(5)), Subscription::Active));
        // Happy twin: a parked stream drains once, then dedups.
        let sid = reg.open((), counting_source()).unwrap();
        assert!(matches!(reg.subscribe(sid.key), Subscription::Drain(())));
        assert!(matches!(reg.subscribe(sid.key), Subscription::Active));
    }

    fn assert_refused(json: &str, input: &str) {
        let decoded = serde_json::from_str::<IpeStreamId>(json);
        assert!(decoded.is_err(), "{json} must not decode");
        let text = decoded.err().map(|e| e.to_string()).unwrap_or_default();
        // serde_json appends only " at line L column C" to the custom text.
        let head = text.split(" at line ").next().unwrap_or_default();
        assert_eq!(head, INVALID_ID, "{text}");
        assert!(!head.contains(input), "{text} echoes {input}");
    }

    #[test]
    fn deserialize_refuses_integer() {
        assert_refused("5", "5");
        assert_refused("-12345", "12345");
        assert_refused("1.5e3", "1.5");
    }

    #[test]
    fn deserialize_refuses_wrong_length() {
        let short = "1".repeat(31);
        let long = "1".repeat(33);
        assert_refused(&format!("\"{short}\""), &short);
        assert_refused(&format!("\"{long}\""), &long);
        assert_refused("\"\"", "\"");
    }

    #[test]
    fn deserialize_refuses_uppercase_hex() {
        let upper = "ABCDEF0123456789ABCDEF0123456789";
        assert_refused(&format!("\"{upper}\""), upper);
    }

    #[test]
    fn deserialize_refuses_non_hex() {
        let bad = "0123456789abcdef0123456789abcdeg";
        assert_refused(&format!("\"{bad}\""), bad);
        let spaced = " 123456789abcdef0123456789abcdef";
        assert_refused(&format!("\"{spaced}\""), spaced);
    }

    #[test]
    fn deserialize_refuses_all_zero() {
        let zero = "0".repeat(32);
        assert_refused(&format!("\"{zero}\""), &zero);
    }

    #[test]
    fn deserialize_refuses_non_string_shapes() {
        assert_refused("null", "null");
        assert_refused("true", "true");
        assert_refused("[1]", "[1]");
        assert_refused("{\"key\":1}", "key");
    }

    #[test]
    fn serde_round_trip_exact_32_lower_hex() {
        let mut reg = StreamRegistry::<()>::new();
        let mut top = u128::MAX.to_le_bytes();
        top[15] = 0xab;
        let sid = reg
            .open((), |buf: &mut [u8; 16]| {
                *buf = top;
                Ok(())
            })
            .unwrap();
        let wire = serde_json::to_string(&sid).unwrap();
        assert_eq!(wire, "\"abffffffffffffffffffffffffffffff\"");
        let back: IpeStreamId = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, sid);
        let one: IpeStreamId =
            serde_json::from_str("\"00000000000000000000000000000001\"").unwrap();
        assert!(one.key == key(1));
    }

    #[test]
    fn debug_does_not_render_key() {
        let mut reg = StreamRegistry::<()>::new();
        let sid = reg
            .open((), |buf: &mut [u8; 16]| {
                *buf = [0xcd; 16];
                Ok(())
            })
            .unwrap();
        let shown = format!("{sid:?}");
        assert_eq!(shown, "StreamId(<opaque>)");
        assert!(!shown.contains("cdcd"));
        assert!(!format!("{:?}", Some(sid)).contains("cdcd"));
    }

    #[test]
    fn mint_redraws_zero_and_live_collision() {
        let draws = [0u128, 3, 9];
        let mut i = 0;
        let source = |buf: &mut [u8; 16]| {
            *buf = draws.get(i).copied().unwrap_or(0).to_le_bytes();
            i += 1;
            Ok(())
        };
        let minted = mint_with(source, |k| k == key(3));
        assert!(matches!(minted, Ok(k) if k == key(9)));
    }

    #[test]
    fn mint_exhausts_after_4_to_unavailable() {
        let mut draws = 0;
        let zeros = |buf: &mut [u8; 16]| {
            draws += 1;
            *buf = [0; 16];
            Ok(())
        };
        let minted = mint_with(zeros, |_| false);
        assert!(matches!(&minted, Err(e) if kind(e) == IpeErrorKind::Unavailable));
        assert_eq!(draws, MINT_ATTEMPTS);
        let Err(e) = minted else { return };
        assert_eq!(message(&e), MINT_EXHAUSTED);
        // A source that only ever collides exhausts the same way.
        let always_live = mint_with(counting_source(), |_| true);
        assert!(matches!(&always_live, Err(e) if kind(e) == IpeErrorKind::Unavailable));
    }

    #[test]
    fn mint_entropy_failure_is_unavailable() {
        let broken = |_: &mut [u8; 16]| Err(getrandom::Error::UNSUPPORTED);
        let minted = mint_with(broken, |_| false);
        assert!(matches!(&minted, Err(e) if kind(e) == IpeErrorKind::Unavailable));
        let Err(e) = minted else { return };
        assert_eq!(message(&e), ENTROPY_UNAVAILABLE);
    }

    /// Fills `reg` to the cap with parked streams keyed `1..=CLIENT_STREAMS_MAX`.
    fn fill(
        reg: &mut StreamRegistry<()>,
        source: &mut impl FnMut(&mut [u8; 16]) -> Result<(), getrandom::Error>,
    ) {
        for _ in 0..CLIENT_STREAMS_MAX {
            assert!(reg.open((), &mut *source).is_ok());
        }
    }

    #[test]
    fn eviction_prefers_ended_tombstone() {
        let mut reg = StreamRegistry::<()>::new();
        let mut source = counting_source();
        fill(&mut reg, &mut source);
        // A later stream ends; the oldest (key 1) stays parked.
        let ended = key(500);
        assert!(reg.take_for_drain(ended).is_ok());
        reg.finish_drain(ended);
        assert!(reg.open((), &mut source).is_ok());
        assert_eq!(reg.map.len(), CLIENT_STREAMS_MAX);
        assert!(slot_of(&reg, ended).is_none());
        assert!(matches!(slot_of(&reg, key(1)), Some(Slot::Parked(()))));
    }

    #[test]
    fn evicted_parked_key_then_refused() {
        let mut reg = StreamRegistry::<()>::new();
        let mut source = counting_source();
        fill(&mut reg, &mut source);
        assert!(reg.open((), &mut source).is_ok());
        assert_eq!(reg.map.len(), CLIENT_STREAMS_MAX);
        let evicted = reg.take_for_drain(key(1));
        assert!(matches!(&evicted, Err(e) if kind(e) == IpeErrorKind::InvalidInput));
        // The next-oldest is still held.
        assert!(reg.take_for_drain(key(2)).is_ok());
    }

    #[test]
    fn all_draining_open_unavailable() {
        let mut reg = StreamRegistry::<()>::new();
        let mut source = counting_source();
        fill(&mut reg, &mut source);
        for n in 1..=CLIENT_STREAMS_MAX {
            let n = u128::try_from(n).unwrap();
            assert!(reg.take_for_drain(key(n)).is_ok());
        }
        let refused = reg.open((), &mut source);
        assert!(matches!(&refused, Err(e) if kind(e) == IpeErrorKind::Unavailable));
        let Err(e) = refused else { return };
        assert_eq!(message(&e), TOO_MANY_STREAMS);
        assert_eq!(reg.map.len(), CLIENT_STREAMS_MAX);
        // An unknown subscribe finds no room for a tombstone and stays silent.
        assert!(matches!(
            reg.subscribe(key(u128::MAX)),
            Subscription::Unrecorded
        ));
    }

    #[cfg(feature = "web-core")]
    #[derive(serde::Deserialize)]
    struct StreamForm {
        sid: IpeStreamId,
    }

    #[cfg(feature = "web-core")]
    fn form(value: &str) -> crate::html::FormData {
        [("sid".to_owned(), value.to_owned())].into_iter().collect()
    }

    #[cfg(feature = "web-core")]
    #[test]
    fn decode_form_refuses_integer_stream_id() {
        let decoded = crate::dom::form::decode_form::<StreamForm>(form("5"));
        assert!(matches!(
            &decoded,
            Err(crate::dom::form::FormDecodeError::Decode(_))
        ));
        let Err(e) = decoded else { return };
        assert!(e.to_string().ends_with(INVALID_ID), "{e}");
        // Happy twin: the exact wire form decodes to the same handle.
        let wire = "000000000000000000000000000000ff";
        let decoded = crate::dom::form::decode_form::<StreamForm>(form(wire));
        assert!(matches!(&decoded, Ok(f) if f.sid.key == key(0xff)));
    }
}

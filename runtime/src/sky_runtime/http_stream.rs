//! Sky.Core.Http.Stream — incremental HTTP response bodies (client side).
//!
//! Mirror of `runtime-go/rt/http_stream.go`. Reads an outbound HTTP response
//! body chunk-by-chunk via reqwest's `bytes_stream()` instead of buffering the
//! whole body (`Http.get`).
//!
//! Surface ported on the Rust backend:
//!
//!   * `open : HttpRequest -> Task Error StreamId`  — fire the request, resolve
//!     once the response headers arrive; register the byte stream under an id.
//!   * `forEachChunk : StreamId -> (String -> Task Error ()) -> Task Error ()`
//!     — synchronous drain (the relay shape — usable inside a plain
//!     Sky.Http.Server handler, no TEA loop required).
//!   * `close : StreamId -> Task Error ()` — drop the stream / release the conn.
//!
//! The Sub-tier `chunks` (dispatching `ChunkEvent` Msgs into a TEA update loop)
//! is ported via `sub_subscribe_stream` + the bridged `ChunkEvent` enum below —
//! it drives a `Cli.program` (or any `cli_program`-hosted) TEA loop, the same
//! way `ws_client`'s `onMessage` does. (The Sky.Live *web* SSE driver remains a
//! separate deferred arc; this is the in-process Sub path.)
//!
//! `StreamId` stays a generated Sky enum (`StreamId Int`); these kernels only
//! ever deal with the raw `i64` (the stdlib wraps/unwraps at the boundary).

use super::*;
use futures_util::StreamExt;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

/// `Sky.Core.Http.Stream.ChunkEvent` — one incremental event on a stream.
/// Bridged (via `runtimeOpaqueTypes`) so the runtime can CONSTRUCT it to hand to
/// the user's `toMsg : ChunkEvent -> msg` callback; user code only ever
/// pattern-matches it. Generic over the Sky error type `E` (always `SkyError`
/// in practice — pinned at the call site) because `Errored` carries an `Error`.
/// Variant names match the Sky constructors verbatim so codegen's match arms
/// (`ChunkEvent::Chunk(s)` / `::Done` / `::Errored(e)`) resolve through the
/// `pub type` alias the bridge emits.
#[derive(Clone, Debug, PartialEq)]
pub enum ChunkEvent<E> {
    Chunk(String),
    Done,
    Errored(E),
}

// The open response is parked here between `open` and `forEachChunk`/`close`.
// Storing the `reqwest::Response` (rather than its byte stream) avoids naming
// `bytes::Bytes` — `forEachChunk` calls `.bytes_stream()` and the chunk type is
// inferred, so no extra `bytes` dependency is needed.
//
// Hard cap on simultaneous open streams. When `http_stream_open` would push
// the registry past this limit it evicts the numerically-lowest (oldest) id
// before inserting — that response/connection is dropped immediately. Keeps
// memory bounded under abandoned-stream workloads; normal well-behaved callers
// (paired open→forEachChunk/close) are unaffected.
const CLIENT_STREAMS_MAX: usize = 1024;

// Contract: every `open` MUST be paired with a `forEachChunk` (which removes the
// entry on exit) or a `close` (idempotent removal) — both release the parked
// response + its connection. Calling `open` repeatedly without draining/closing
// leaks responses; the 30s connect_timeout bounds only the header stage, not an
// abandoned-but-open stream. The CLIENT_STREAMS_MAX cap bounds unbounded growth
// by evicting the oldest entry when the registry is full.
fn client_streams() -> &'static Mutex<HashMap<i64, reqwest::Response>> {
    static R: OnceLock<Mutex<HashMap<i64, reqwest::Response>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

// Monotonic, never-zero stream ids — a zero-valued StreamId (uninitialised
// model field) must never resolve to a real stream.
static NEXT_ID: AtomicI64 = AtomicI64::new(1);
fn next_stream_id() -> i64 {
    loop {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        if id != 0 {
            return id;
        }
    }
}

/// Sky.Core.Http.Stream.open : HttpRequest -> Task Error Int
/// (the Sky wrapper re-wraps the Int in `StreamId`.)
///
/// No whole-request timeout — streams may run for minutes (LLM completions);
/// a 30s connect timeout bounds the header stage only.
pub fn http_stream_open<E: From<String> + Send + 'static>(req: HttpRequest) -> SkyTask<E, i64> {
    Box::pin(async move {
        // SSRF guard (was MISSING here — this surface built its own client and
        // bypassed SKY_HTTP_DENY_PRIVATE entirely). Resolve+validate+pin + the
        // per-redirect re-check via the shared helper, identical to Http.get/post.
        let builder =
            reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(30));
        let builder = match crate::sky_runtime::http_client::ssrf_apply(
            builder,
            &req.url,
            req.followRedirects,
            req.maxRedirects,
        ) {
            Ok(b) => b,
            Err(msg) => return SkyResult::Err(msg.into()),
        };
        let client = match builder.build() {
            Ok(c) => c,
            Err(e) => return SkyResult::Err(format!("http.stream.open: client: {}", e).into()),
        };
        let method = reqwest::Method::from_bytes(req.method.to_uppercase().as_bytes())
            .unwrap_or(reqwest::Method::GET);
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
            // id; Sky sees only a fixed generic message.
            Err(e) => return SkyResult::Err(sky_error_from_foreign(e)),
        };
        // HTTP error statuses (4xx/5xx) still surface as a stream — the body may
        // carry the error payload the caller wants to read. Mirrors Http.get
        // returning Ok with a 4xx status.
        let id = next_stream_id();
        {
            let mut map = client_streams().lock().unwrap_or_else(|e| e.into_inner());
            // Evict the oldest (lowest id) entry when the cap is reached, so the
            // registry stays bounded under abandoned-stream workloads.
            if map.len() >= CLIENT_STREAMS_MAX {
                if let Some(&oldest) = map.keys().min() {
                    map.remove(&oldest);
                }
            }
            map.insert(id, resp);
        }
        SkyResult::Ok(id)
    })
}

/// Sky.Core.Http.Stream.forEachChunk : Int -> (String -> Task Error ()) -> Task Error ()
///
/// Drains the stream synchronously from the calling task, invoking `body chunk`
/// per chunk. Bridges the client consumer to a server producer
/// (`Server.Stream.emit`) inside one Sky.Http.Server handler — the relay shape.
///
/// Semantics (parity with the Go runtime):
///   * clean EOF              → Ok ()
///   * upstream read error     → Err e
///   * `body chunk` returns Err → abort, close, Err e (fail-fast)
///   * the handle is always removed (connection released) on exit.
///
/// Backpressure: `body` runs synchronously per chunk; if it blocks on a slow
/// downstream (`Server.Stream.emit` to a bounded channel) the upstream read
/// naturally throttles.
pub fn http_stream_for_each_chunk<E, F>(id: i64, body: F) -> SkyTask<E, ()>
where
    E: From<String> + Send + 'static,
    F: Fn(String) -> SkyTask<E, ()> + Send + 'static,
{
    Box::pin(async move {
        // Take ownership of the response — forEachChunk consumes it. An unknown /
        // already-drained id is a no-op (matches close's idempotent contract).
        let resp = match client_streams()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id)
        {
            Some(r) => r,
            None => return SkyResult::Ok(()),
        };
        let mut stream = resp.bytes_stream();
        loop {
            match stream.next().await {
                Some(Ok(bytes)) => {
                    let chunk = String::from_utf8_lossy(&bytes).into_owned();
                    match body(chunk).await {
                        SkyResult::Ok(()) => {}
                        SkyResult::Err(e) => break SkyResult::Err(e),
                    }
                }
                // [B8] redact the foreign reqwest read error (see open above).
                Some(Err(e)) => break SkyResult::Err(sky_error_from_foreign(e)),
                None => break SkyResult::Ok(()),
            }
        }
        // `stream` (and the response) drops here → connection released.
    })
}

/// Sky.Core.Http.Stream.close : Int -> Task Error ()
/// Idempotent — closing an unknown / already-closed id is a no-op.
pub fn http_stream_close<E: From<String> + Send + 'static>(id: i64) -> SkyTask<E, ()> {
    Box::pin(async move {
        client_streams()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        chunk_subscribed()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        SkyResult::Ok(())
    })
}

// ─── Sub-tier: chunks → ChunkEvent Msgs ─────────────────────────────────────

// Dedup guard: `subscriptions` is re-evaluated on every TEA `update`, so a naive
// implementation would spawn a fresh drain per update and race over the parked
// response. We spawn the real drain ONCE per id (the first subscribe), as a
// DETACHED task — the SubManager's abort-on-respawn only ever hits the dummy
// handle, never the drain. Same shape as `ws_client`'s `ws_mark_subscribed`.
fn chunk_subscribed() -> &'static Mutex<HashSet<i64>> {
    static R: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Sky.Core.Http.Stream.chunks → `Sub_subscribeStream`.
///
/// Returns a `SkySub::Source` that, on first subscribe for `id`, spawns a
/// detached task draining the parked response and dispatching a `ChunkEvent`
/// Msg per chunk: `Chunk s` per UTF-8 byte chunk, `Done` on clean EOF,
/// `Errored e` on a read fault. Subscribing to an unknown / already-drained id
/// is a no-op (matches the stdlib contract). `E` is pinned to `SkyError` at the
/// call site; `Errored` builds it via `From<String>`.
pub fn sub_subscribe_stream<E, M, F>(id: i64, to_msg: F) -> SkySub<M>
where
    E: From<String> + Send + 'static,
    M: Send + 'static,
    F: Fn(ChunkEvent<E>) -> M + Send + Sync + 'static,
{
    SkySub::Source(Box::new(move |emit| {
        if chunk_subscribed()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id)
        {
            tokio::spawn(async move {
                let resp = match client_streams()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id)
                {
                    Some(r) => r,
                    None => {
                        chunk_subscribed()
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        return;
                    }
                };
                let mut stream = resp.bytes_stream();
                loop {
                    match stream.next().await {
                        Some(Ok(bytes)) => {
                            let chunk = String::from_utf8_lossy(&bytes).into_owned();
                            emit(to_msg(ChunkEvent::Chunk(chunk)));
                        }
                        Some(Err(e)) => {
                            // [B8] redact the foreign reqwest read error (see open above).
                            emit(to_msg(ChunkEvent::Errored(sky_error_from_foreign(e))));
                            break;
                        }
                        None => {
                            emit(to_msg(ChunkEvent::Done));
                            break;
                        }
                    }
                }
                chunk_subscribed()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
            });
        }
        tokio::spawn(async {}) // dummy handle for the SubManager to abort harmlessly
    }))
}

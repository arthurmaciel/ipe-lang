//! Ipe.WebSocket — outbound WebSocket client (tokio-tungstenite).
//!
//! Task-tier: connect/connectWith/send/sendBinary/close/closeWithCode via a
//! per-socket registry (a write-command mpsc + a frames broadcast). Receive:
//! `Sub_subscribeWebSocket` builds a IpeSub::Source that drains the frames
//! broadcast and emits messages into the TEA loop — completing `onMessage`.
//!
//! WebSocketMessage/CloseCode are bridged to runtime enums so the runtime can
//! construct frames/codes for the user's toMsg. The design intent is that all
//! four event kinds (onOpen/onMessage/onClose/onError) route through their own
//! typed kernel below — one per heterogeneous toMsg shape, so no bounded fn is
//! shared and no stdlib override is needed.
//!
//! STATUS: this client-side receive surface is NOT yet wired — no `KernelFn`
//! variant routes codegen to `sub_subscribe_ws_{message,close,error}`, so these
//! three functions are currently unreachable dead code. Their `F` bound is
//! `Send` (not `Send + Sync`) defensively, ahead of that wiring, so the
//! eventual wiring commit cannot ship exit-0-then-cargo-fail (a THE-SEAL
//! violation) on the generic first-class-function-value render path — the same
//! contract the reachable `sub_subscribe_stream` uses.

use super::*;
#[cfg(not(target_arch = "wasm32"))]
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicI64, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Mutex, OnceLock};
#[cfg(not(target_arch = "wasm32"))]
use tokio_tungstenite::tungstenite::Message;

/// Ipe.WebSocket.WebSocketMessage — bridged so the runtime can build frames.
/// Variant names match the Ipê constructors (Text / Binary).
///
/// The backend emits this type AS the Ipê `WebSocketMessage` ADT (the enum decl
/// is bridged, not user-emitted), so it must carry the same derives a real Ipê
/// enum gets — `serde::{Serialize, Deserialize}` in particular, since a Live
/// `Msg` variant like `GotFrame WebSocketMessage` is serialized to/from the
/// session store and the wire.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum WsClientMessage {
    Text(String),
    /// Binary frames carry raw bytes (`Vec<u8>`) — no Latin-1 bridge. Ipê code
    /// that needs to inspect binary payload passes it through `Bytes.*` kernels.
    Binary(Vec<u8>),
}

/// Ipe.WebSocket.CloseCode — bridged so the runtime can build close codes
/// for onClose's toMsg. Variant names match the Ipê constructors. Carries the
/// same serde derives as [`WsClientMessage`] for the same Live-`Msg` reason.
#[allow(non_snake_case)]
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum WsCloseCode {
    Normal,
    GoingAway,
    UnsupportedData,
    InternalError,
    Custom(i64),
}

fn ws_close_code(code: i64) -> WsCloseCode {
    match code {
        1000 => WsCloseCode::Normal,
        1001 => WsCloseCode::GoingAway,
        1003 => WsCloseCode::UnsupportedData,
        1011 => WsCloseCode::InternalError,
        n => WsCloseCode::Custom(n),
    }
}

/// Internal per-socket event broadcast to onMessage/onClose/onError subs.
#[derive(Clone, Debug)]
#[cfg(not(target_arch = "wasm32"))]
enum WsEvent {
    Message(WsClientMessage),
    Closed(i64),
    Error(String),
}

/// Ipe.WebSocket.WebSocketCfg — built in Ipê (defaultCfg + with*).
#[allow(non_snake_case)]
#[derive(Clone, Debug)]
pub struct WsClientCfg {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub timeout: i64,
    pub pingInterval: i64,
}

#[cfg(not(target_arch = "wasm32"))]
enum WsCmd {
    Text(String),
    Binary(Vec<u8>),
    Close,
    CloseWithCode(u16, String),
}

#[cfg(not(target_arch = "wasm32"))]
struct ClientEntry {
    // Bounded (not unbounded) so a remote peer that stalls reads — wedging the
    // writer task on `write.send().await` — can't make this outbound queue grow
    // without limit (memory DoS). A full queue makes send_cmd return false
    // (try_send) rather than buffering forever.
    cmd_tx: tokio::sync::mpsc::Sender<WsCmd>,
    frames_tx: tokio::sync::broadcast::Sender<WsEvent>,
    // Abort handles for the writer + reader tasks. A Close that can't be enqueued
    // (full queue ⇒ the writer is wedged on a stalled peer) is honoured by
    // aborting BOTH halves — dropping just one leaves the split stream open — so a
    // close request can never strand an open socket. See send_cmd.
    writer_abort: tokio::task::AbortHandle,
    reader_abort: tokio::task::AbortHandle,
}

#[cfg(not(target_arch = "wasm32"))]
fn registry() -> &'static Mutex<HashMap<i64, ClientEntry>> {
    static R: OnceLock<Mutex<HashMap<i64, ClientEntry>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remove a socket from the registry and drop its subscribe-once markers so the
/// associated tasks wind down and the maps don't grow across reconnects.
#[cfg(not(target_arch = "wasm32"))]
fn deregister(id: i64) {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    ws_subscribed()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|&(sid, _)| sid != id);
}

#[cfg(not(target_arch = "wasm32"))]
static WS_CLIENT_NEXT_ID: AtomicI64 = AtomicI64::new(1);

/// Redact any `user:pass@` userinfo from a URL before it is echoed in an error
/// message. WebSocket URLs legitimately carry credentials (`ws://user:pass@host`),
/// and connect-error strings flow to `Ipe.Log` / structured logs, so the raw URL
/// would leak the secret (PRINCIPLES #1: no secret leakage into errors/logs).
/// Parse-and-rebuild via the `url` crate when possible; fall back to a manual
/// `scheme://...@` strip so a URL the parser rejects (the bad-url error path)
/// still never echoes credentials. Total — no unwrap/index/panic.
#[cfg(not(target_arch = "wasm32"))]
fn redact_ws_url(url: &str) -> String {
    if let Ok(mut u) = url::Url::parse(url) {
        if !u.username().is_empty() || u.password().is_some() {
            // set_username/set_password return Err only for cannot-be-a-base URLs,
            // which can't reach here (they have no userinfo); ignore either way.
            let _ = u.set_username("");
            let _ = u.set_password(None);
        }
        return u.to_string();
    }
    // Unparseable URL: strip a leading `scheme://userinfo@` manually. Only the
    // authority's userinfo (before the first '/', '?' or '#') is considered, so a
    // later `@` in a path/query is preserved.
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let (authority, tail) = rest.split_at(authority_end);
            match authority.rsplit_once('@') {
                Some((_userinfo, host)) => format!("{scheme}://{host}{tail}"),
                None => url.to_string(),
            }
        }
        None => url.to_string(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn do_connect<E: From<String> + Send + 'static>(
    url: String,
    headers: Vec<(String, String)>,
    timeout_ms: i64,
    ping_interval_ms: i64,
) -> IpeResult<E, i64> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
    // Build the credential-stripped form ONCE; every error message below echoes
    // this, never the raw `url`.
    let safe_url = redact_ws_url(&url);
    // SSRF guard: when IPE_HTTP_DENY_PRIVATE is set, reject a ws/wss URL whose host
    // resolves to a private/loopback/link-local address BEFORE the handshake — the
    // without this check the WebSocket surface would connect with no
    // deny-private guard, letting an attacker-controlled URL reach internal
    // services the Http client blocks.
    if let Err(msg) = super::ssrf::ssrf_validate_url(&url) {
        return IpeResult::Err(msg.into());
    }
    // Build the handshake request so custom headers (e.g. Authorization) from
    // connectWith's cfg.headers are sent.
    let mut req = match url.as_str().into_client_request() {
        Ok(r) => r,
        Err(e) => {
            return IpeResult::Err(
                format!("WebSocket.connect {}: bad url: {}", safe_url, e).into(),
            );
        }
    };
    // Fail CLOSED on an unparseable caller-supplied header: a credential (e.g.
    // an Authorization bearer) that can't be attached must abort the connect,
    // never connect unauthenticated. Echo only the header NAME (k) in the error
    // — never the value (v), which may carry the secret.
    for (k, v) in &headers {
        let name = match k.parse::<HeaderName>() {
            Ok(n) => n,
            Err(_) => {
                return IpeResult::Err(
                    format!(
                        "WebSocket.connect {}: invalid header name {:?}",
                        safe_url, k
                    )
                    .into(),
                );
            }
        };
        let val = match HeaderValue::from_str(v) {
            Ok(val) => val,
            Err(_) => {
                return IpeResult::Err(
                    format!(
                        "WebSocket.connect {}: invalid value for header {:?}",
                        safe_url, k
                    )
                    .into(),
                );
            }
        };
        req.headers_mut().insert(name, val);
    }
    // Cap inbound frame/message size to prevent a remote server from forcing the
    // client to buffer an arbitrarily large payload. Default 1 MiB (matches the
    // server-side cap): the prior 16 MiB × the 64-deep broadcast buffer below was
    // ~1 GiB worst-case retained per socket under a lagging subscriber. Override
    // via IPE_WS_MAX_MESSAGE_BYTES for apps that legitimately need larger frames.
    //
    // tokio-tungstenite 0.24 exposes connect_async_with_config which passes a
    // tungstenite::protocol::WebSocketConfig directly to the handshake.
    let max_msg: usize = crate::system::read_env_var("IPE_WS_MAX_MESSAGE_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1024 * 1024);
    let ws_config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(max_msg),
        max_frame_size: Some(max_msg),
        ..Default::default()
    };
    // SSRF pin (R1): when IPE_HTTP_DENY_PRIVATE is on, resolve the host to a
    // vetted non-private addr and dial THAT ourselves, so tokio-tungstenite can't
    // re-resolve the name to a rebind target at connect time — closing the
    // resolve->connect TOCTOU that the bare ssrf_validate_url check above leaves
    // open (it validates a name that connect_async would resolve again).
    let pinned = match super::ssrf::ssrf_pinned_ws_addr(&url) {
        Ok(p) => p,
        Err(msg) => return IpeResult::Err(msg.into()),
    };
    type WsConnOut = Result<
        (
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            tokio_tungstenite::tungstenite::handshake::client::Response,
        ),
        tokio_tungstenite::tungstenite::Error,
    >;
    let connect_fut: std::pin::Pin<Box<dyn std::future::Future<Output = WsConnOut> + Send>> =
        match pinned {
            Some(addr) => {
                // When SSRF-pinning is active (IPE_HTTP_DENY_PRIVATE), we dial
                // the already-vetted IP directly via a raw TCP socket, bypassing
                // the name-resolution step. A raw TCP socket carries no TLS
                // context, so a `wss://` URL cannot be serviced here — TLS
                // requires the full resolver path (`connect_async_with_config`,
                // the `None` arm below). Refuse rather than dial plaintext to
                // what the caller believes is a secure endpoint.
                if url.starts_with("wss://") {
                    return IpeResult::Err(
                        format!(
                            "WebSocket.connect {}: wss:// with IPE_HTTP_DENY_PRIVATE is \
                         unsupported (SSRF-pinned dial bypasses TLS; disable \
                         IPE_HTTP_DENY_PRIVATE or use ws:// for this endpoint)",
                            safe_url
                        )
                        .into(),
                    );
                }
                // Dial INSIDE the future so the single handshake timeout below
                // also bounds the pinned TCP connect — otherwise an unreachable
                // / silently-stalling pinned addr would hang here, outside the
                // timeout guard, leaking the task + FD.
                Box::pin(async move {
                    let tcp = tokio::net::TcpStream::connect(addr).await.map_err(|e| {
                        tokio_tungstenite::tungstenite::Error::Io(std::io::Error::new(
                            e.kind(),
                            format!("pinned dial {} failed: {}", addr, e),
                        ))
                    })?;
                    tokio_tungstenite::client_async_with_config(
                        req,
                        tokio_tungstenite::MaybeTlsStream::Plain(tcp),
                        Some(ws_config),
                    )
                    .await
                })
            }
            None => Box::pin(tokio_tungstenite::connect_async_with_config(
                req,
                Some(ws_config),
                false,
            )),
        };
    // Floor the handshake timeout: a non-positive cfg.timeout must NOT disable it
    // (an unreachable / silently-stalling host would otherwise hang connect_async
    // forever, leaking the task + FD). Default 30 s.
    let to_ms: u64 = if timeout_ms > 0 {
        timeout_ms as u64
    } else {
        30_000
    };
    let (stream, _resp) =
        match tokio::time::timeout(std::time::Duration::from_millis(to_ms), connect_fut).await {
            Ok(Ok(ok)) => ok,
            Ok(Err(e)) => {
                return IpeResult::Err(format!("WebSocket.connect {}: {}", safe_url, e).into());
            }
            Err(_) => {
                return IpeResult::Err(
                    format!(
                        "WebSocket.connect {}: handshake timed out after {}ms",
                        safe_url, to_ms
                    )
                    .into(),
                );
            }
        };
    let id = WS_CLIENT_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (mut write, mut read) = stream.split();
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<WsCmd>(1024);
    let (frames_tx, _) = tokio::sync::broadcast::channel::<WsEvent>(64);

    // Writer task: drain outbound commands → ws frames. When pingInterval > 0,
    // also send a periodic Ping so idle connections survive proxy/server idle
    // timeouts (tungstenite auto-pongs inbound pings on the read side).
    let writer = tokio::spawn(async move {
        // `interval` ticks immediately on the first poll; skip that first tick so
        // we ping after the interval, not at t=0.
        let mut ping_iv = if ping_interval_ms > 0 {
            let mut iv =
                tokio::time::interval(std::time::Duration::from_millis(ping_interval_ms as u64));
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            Some(iv)
        } else {
            None
        };
        let mut first_tick = true;
        loop {
            let cmd = match &mut ping_iv {
                Some(iv) => tokio::select! {
                    _ = iv.tick() => {
                        if first_tick { first_tick = false; continue; }
                        if write.send(Message::Ping(Vec::new())).await.is_err() { break; }
                        continue;
                    }
                    c = cmd_rx.recv() => c,
                },
                None => cmd_rx.recv().await,
            };
            let cmd = match cmd {
                Some(c) => c,
                None => break,
            };
            let msg = match cmd {
                WsCmd::Text(s) => Message::Text(s),
                WsCmd::Binary(b) => Message::Binary(b),
                WsCmd::Close => {
                    let _ = write.send(Message::Close(None)).await;
                    break;
                }
                WsCmd::CloseWithCode(code, reason) => {
                    let frame = tokio_tungstenite::tungstenite::protocol::CloseFrame {
                        code: code.into(),
                        reason: reason.into(),
                    };
                    let _ = write.send(Message::Close(Some(frame))).await;
                    break;
                }
            };
            if write.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Reader task: ws frames → frames broadcast (subscriptions drain it). On
    // close/error it deregisters the socket so the writer + subscription tasks
    // wind down (dropping the last frames_tx makes their recv() error) — no leak
    // on server-initiated close / reconnect.
    let writer_abort = writer.abort_handle();
    let frames = frames_tx.clone();
    let reader = tokio::spawn(async move {
        while let Some(item) = read.next().await {
            match item {
                Ok(Message::Text(t)) => {
                    let _ = frames.send(WsEvent::Message(WsClientMessage::Text(t)));
                }
                Ok(Message::Binary(b)) => {
                    // `b` is already `Vec<u8>` from tungstenite — no conversion needed.
                    let _ = frames.send(WsEvent::Message(WsClientMessage::Binary(b)));
                }
                Ok(Message::Close(cf)) => {
                    let code = cf.map(|f| u16::from(f.code) as i64).unwrap_or(1000);
                    let _ = frames.send(WsEvent::Closed(code));
                    break;
                }
                Err(e) => {
                    let _ = frames.send(WsEvent::Error(format!("ws read error: {}", e)));
                    break;
                }
                _ => {} // Ping/Pong handled by tungstenite
            }
        }
        deregister(id);
    });

    let reader_abort = reader.abort_handle();
    registry().lock().unwrap_or_else(|e| e.into_inner()).insert(
        id,
        ClientEntry {
            cmd_tx,
            frames_tx,
            writer_abort,
            reader_abort,
        },
    );
    ok_res(id)
}

/// WebSocket.connect : String -> Task Error Int (raw id; Ipê wraps in WebSocket)
#[cfg(not(target_arch = "wasm32"))]
pub fn web_socket_connect<E: From<String> + Send + 'static>(url: String) -> IpeTask<E, i64> {
    Box::pin(do_connect(url, Vec::new(), 30000, 0))
}

/// WebSocket.connectWith : WebSocketCfg -> Task Error Int. Applies the cfg's
/// custom headers, handshake timeout, and pingInterval (when > 0, the client
/// sends a periodic Ping frame to keep the connection alive through idle proxies;
/// tungstenite auto-pongs inbound pings on the read side).
#[cfg(not(target_arch = "wasm32"))]
pub fn web_socket_connect_with<E: From<String> + Send + 'static>(
    cfg: WsClientCfg,
) -> IpeTask<E, i64> {
    Box::pin(do_connect(
        cfg.url,
        cfg.headers,
        cfg.timeout,
        cfg.pingInterval,
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn send_cmd(id: i64, cmd: WsCmd) -> bool {
    let is_close = matches!(cmd, WsCmd::Close | WsCmd::CloseWithCode(..));
    // Clone what we need, then RELEASE the registry lock before try_send / abort /
    // deregister (deregister re-locks the registry — holding it here would deadlock).
    let handles = {
        let reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        reg.get(&id).map(|e| {
            (
                e.cmd_tx.clone(),
                e.frames_tx.clone(),
                e.writer_abort.clone(),
                e.reader_abort.clone(),
            )
        })
    };
    let (tx, frames, writer_abort, reader_abort) = match handles {
        Some(h) => h,
        None => return false,
    };
    match tx.try_send(cmd) {
        Ok(()) => true,
        Err(_) => {
            // Queue full. For a non-close command we drop it (caller sees false).
            // For a Close, the full queue means the writer is wedged on a stalled
            // peer, so a queued Close would never be sent — guarantee teardown by
            // aborting BOTH halves (drops the split stream → the connection
            // closes), notifying subscribers, and deregistering. Close "succeeds".
            if is_close {
                writer_abort.abort();
                reader_abort.abort();
                let _ = frames.send(WsEvent::Closed(1000));
                deregister(id);
                true
            } else {
                false
            }
        }
    }
}

/// WebSocket.send : Int -> String -> Task Error ()
#[cfg(not(target_arch = "wasm32"))]
pub fn web_socket_send<E: From<String> + Send + 'static>(id: i64, msg: String) -> IpeTask<E, ()> {
    Box::pin(async move {
        if send_cmd(id, WsCmd::Text(msg)) {
            ok_res(())
        } else {
            IpeResult::Err(format!("WebSocket.send: no socket {}", id).into())
        }
    })
}

/// WebSocket.sendBinary : Int -> Bytes -> Task Error ()
#[cfg(not(target_arch = "wasm32"))]
pub fn web_socket_send_binary<E: From<String> + Send + 'static>(
    id: i64,
    msg: Vec<u8>,
) -> IpeTask<E, ()> {
    Box::pin(async move {
        if send_cmd(id, WsCmd::Binary(msg)) {
            ok_res(())
        } else {
            IpeResult::Err(format!("WebSocket.sendBinary: no socket {}", id).into())
        }
    })
}

/// WebSocket.close : Int -> Task Error () (idempotent)
#[cfg(not(target_arch = "wasm32"))]
pub fn web_socket_close<E: From<String> + Send + 'static>(id: i64) -> IpeTask<E, ()> {
    Box::pin(async move {
        let _ = send_cmd(id, WsCmd::Close);
        deregister(id);
        ok_res(())
    })
}

/// WebSocket.closeWithCode : Int -> String -> Int -> Task Error ()
#[cfg(not(target_arch = "wasm32"))]
pub fn web_socket_close_with_code<E: From<String> + Send + 'static>(
    code: i64,
    reason: String,
    id: i64,
) -> IpeTask<E, ()> {
    Box::pin(async move {
        // A WebSocket close code is a u16 (RFC 6455 §7.4). A bare `code as u16`
        // SILENTLY TRUNCATES a Ipê `Int` outside 0..=65535 (e.g. 70000 → 4464),
        // which is worse than rejecting it because the wrapped value can land on
        // a *different valid* code. Out-of-range → 1000 (normal closure).
        let ws_code = u16::try_from(code).unwrap_or(1000);
        let _ = send_cmd(id, WsCmd::CloseWithCode(ws_code, reason));
        deregister(id);
        ok_res(())
    })
}

// The four onX wrappers all call subscribeWebSocketRaw with a compile-time
// literal kind; the Builder peephole routes each to its own typed kernel below
// (so the heterogeneous toMsg shapes never share one bounded fn — no stdlib
// override needed). Each subscribes to the per-socket WsEvent broadcast and
// filters the events it cares about.

#[cfg(not(target_arch = "wasm32"))]
fn subscribe_events(socket_id: i64) -> Option<tokio::sync::broadcast::Receiver<WsEvent>> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&socket_id)
        .map(|e| e.frames_tx.subscribe())
}

// WS subscriptions are set up ONCE per (socket, kind): the SubManager aborts +
// respawns every sub on each update, but a broadcast has no replay, so a
// re-spawned receiver would miss frames sent during the gap. So the real
// listener is spawned DETACHED (not the handle the SubManager tracks) the first
// time, and re-subscribes are no-ops — matching Go's "subsequent re-subscriptions
// are no-ops". The emit callback funnels into the loop channel, stable for the
// program's lifetime.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
#[cfg(not(target_arch = "wasm32"))]
enum WsSubKind {
    Message,
    Open,
    Close,
    Error,
}

#[cfg(not(target_arch = "wasm32"))]
fn ws_subscribed() -> &'static Mutex<std::collections::HashSet<(i64, WsSubKind)>> {
    static S: OnceLock<Mutex<std::collections::HashSet<(i64, WsSubKind)>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}
#[cfg(not(target_arch = "wasm32"))]
fn ws_mark_subscribed(socket_id: i64, kind: WsSubKind) -> bool {
    ws_subscribed()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((socket_id, kind))
}
// True iff the socket is currently in the registry. Gate ws_mark_subscribed on
// this (registry-check FIRST, short-circuiting the insert) so subscribing to a
// never-connected / already-closed id doesn't leave a permanent marker behind
// (socket ids are monotonic, so a leaked marker is never reclaimed by
// deregister). The guard drops at return, so the registry + ws_subscribed locks
// are never held simultaneously.
#[cfg(not(target_arch = "wasm32"))]
fn ws_registered(socket_id: i64) -> bool {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&socket_id)
}

/// onMessage : (WebSocketMessage -> msg) -> Sub msg
///
/// `to_msg` is moved exclusively into the ONE detached `tokio::spawn` task
/// below (never behind a shared `Arc`, never read from two threads at once) —
/// the same shape as the sibling `sub_subscribe_stream` (`http_stream.rs`) and
/// `sub_subscribe_topic` (`pubsub.rs`), whose doc comments state the identical
/// rationale. `Send` is therefore the full and correct contract; `Sync` is NOT
/// required. An over-declared `+ Sync` here is exactly the bound the codegen's
/// generic first-class-function-value rendering
/// (`Box<dyn Fn(..) -> .. + Send + 'static>` — deliberately `+Send`-only) could
/// never satisfy, so it would surface as a THE-SEAL `cargo build` E0277 the day
/// codegen first routes a client `onMessage` subscription here. The `Send`-only
/// bound is DEFENSIVE / PRE-WIRING: these three client-side subs
/// (`_message` / `_close` / `_error`) are currently UNREACHABLE — no `KernelFn`
/// variant routes codegen to them yet (the whole `Ipe.WebSocket` client
/// surface is unwired) — so the future wiring commit does not ship
/// exit-0-then-cargo-fail on day one.
#[cfg(not(target_arch = "wasm32"))]
pub fn sub_subscribe_ws_message<M, F>(socket_id: i64, to_msg: F) -> IpeSub<M>
where
    M: Send + 'static,
    F: Fn(WsClientMessage) -> M + Send + 'static,
{
    IpeSub::Source(Box::new(move |emit| {
        if ws_registered(socket_id) && ws_mark_subscribed(socket_id, WsSubKind::Message) {
            tokio::spawn(async move {
                let mut rx = match subscribe_events(socket_id) {
                    Some(rx) => rx,
                    None => return,
                };
                loop {
                    match rx.recv().await {
                        Ok(WsEvent::Message(m)) => emit(to_msg(m)),
                        Ok(_) => {}
                        // A momentarily-slow consumer that lags past the buffer gets
                        // a Lagged error — skip the gap and keep the subscription
                        // alive (do NOT treat it as terminal). Closed channel ends it.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }
        tokio::spawn(async {}) // dummy handle for the SubManager to abort harmlessly
    }))
}

/// onOpen : msg -> Sub msg — dispatch `msg` once when connected.
#[cfg(not(target_arch = "wasm32"))]
pub fn sub_subscribe_ws_open<M>(socket_id: i64, msg: M) -> IpeSub<M>
where
    M: Send + 'static,
{
    IpeSub::Source(Box::new(move |emit| {
        if ws_registered(socket_id) && ws_mark_subscribed(socket_id, WsSubKind::Open) {
            emit(msg);
        }
        tokio::spawn(async {})
    }))
}

/// onClose : (CloseCode -> msg) -> Sub msg
///
/// Same defensive pre-wiring `Send`-only bound as
/// [`sub_subscribe_ws_message`]: `to_msg` is moved into the single detached
/// `tokio::spawn` below and never shared behind an `Arc`, so `Send + 'static`
/// is the exact contract and `+ Sync` was over-strict. Currently unreachable
/// (no `KernelFn` arm routes here yet); relaxing dead code can't regress.
#[cfg(not(target_arch = "wasm32"))]
pub fn sub_subscribe_ws_close<M, F>(socket_id: i64, to_msg: F) -> IpeSub<M>
where
    M: Send + 'static,
    F: Fn(WsCloseCode) -> M + Send + 'static,
{
    IpeSub::Source(Box::new(move |emit| {
        if ws_registered(socket_id) && ws_mark_subscribed(socket_id, WsSubKind::Close) {
            tokio::spawn(async move {
                let mut rx = match subscribe_events(socket_id) {
                    Some(rx) => rx,
                    None => return,
                };
                loop {
                    match rx.recv().await {
                        Ok(WsEvent::Closed(code)) => {
                            emit(to_msg(ws_close_code(code)));
                            break;
                        }
                        Ok(_) => {}
                        // Transient lag: skip the gap, keep waiting for the close event.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }
        tokio::spawn(async {})
    }))
}

/// onError : (Error -> msg) -> Sub msg. E is the project error (From<String>).
///
/// Same defensive pre-wiring `Send`-only bound as
/// [`sub_subscribe_ws_message`]: `to_msg` is moved into the single detached
/// `tokio::spawn` below and never shared behind an `Arc`, so `Send + 'static`
/// is the exact contract and `+ Sync` was over-strict. Currently unreachable
/// (no `KernelFn` arm routes here yet); relaxing dead code can't regress.
#[cfg(not(target_arch = "wasm32"))]
pub fn sub_subscribe_ws_error<E, M, F>(socket_id: i64, to_msg: F) -> IpeSub<M>
where
    E: From<String> + Send + 'static,
    M: Send + 'static,
    F: Fn(E) -> M + Send + 'static,
{
    IpeSub::Source(Box::new(move |emit| {
        if ws_registered(socket_id) && ws_mark_subscribed(socket_id, WsSubKind::Error) {
            tokio::spawn(async move {
                let mut rx = match subscribe_events(socket_id) {
                    Some(rx) => rx,
                    None => return,
                };
                loop {
                    match rx.recv().await {
                        Ok(WsEvent::Error(s)) => {
                            emit(to_msg(s.into()));
                            break;
                        }
                        Ok(_) => {}
                        // Transient lag: skip the gap, keep waiting for the error event.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }
        tokio::spawn(async {})
    }))
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_code_mapping() {
        assert_eq!(ws_close_code(1000), WsCloseCode::Normal);
        assert_eq!(ws_close_code(1001), WsCloseCode::GoingAway);
        assert_eq!(ws_close_code(1003), WsCloseCode::UnsupportedData);
        assert_eq!(ws_close_code(1011), WsCloseCode::InternalError);
        assert_eq!(ws_close_code(4000), WsCloseCode::Custom(4000));
    }

    #[test]
    fn redact_ws_url_strips_userinfo() {
        // Credentials must never survive into an error/log string.
        let r = redact_ws_url("ws://user:s3cret@example.com:9000/feed?token=abc");
        assert!(!r.contains("s3cret"), "password leaked: {r}");
        assert!(!r.contains("user:"), "username leaked: {r}");
        assert!(r.contains("example.com"));
        // Username-only (no password) is also stripped.
        let r2 = redact_ws_url("wss://admin@host/x");
        assert!(!r2.contains("admin@"), "username leaked: {r2}");
        // No userinfo → unchanged host/path.
        let r3 = redact_ws_url("ws://example.com/feed");
        assert!(r3.contains("example.com") && !r3.contains('@'));
        // Unparseable URL still strips a leading scheme://userinfo@ and keeps a
        // later '@' in the path intact.
        let r4 = redact_ws_url("ws://bob:pw@@@host/a@b");
        assert!(!r4.contains("bob:pw"), "creds leaked from bad url: {r4}");
        assert!(r4.contains("a@b"), "path '@' wrongly stripped: {r4}");
    }
}

// ---------------------------------------------------------------------------
// wasm32 browser substitute — `web_sys::WebSocket`
// ---------------------------------------------------------------------------
//
// Task-tier only (connect/connectWith/send/sendBinary/close/closeWithCode) —
// the native onOpen/onMessage/onClose/onError receive surface
// (`sub_subscribe_ws_*`) is unreachable on EVERY target today (no `KernelFn`
// wires codegen to it yet, per this file's module doc); wiring it for wasm is
// left for the same follow-up that wires it natively, not a wasm-specific gap.
//
// No SSRF guard here (unlike the native `do_connect`, which resolves + pins
// the host): a browser tab cannot open a raw socket or bypass the browser's
// own network stack, so `IPE_HTTP_DENY_PRIVATE`'s DNS-pin mechanism has no
// browser analogue — same rationale as the `fetch` substitute in
// `http_client.rs`. A connect failure (refused, CORS-equivalent origin block,
// TLS error, DNS failure) surfaces through the socket's `error`/`close` event,
// which this substitute maps to `Task.fail` on `send`/`close` calls against a
// never-opened id — never a panic/trap.
#[cfg(target_arch = "wasm32")]
mod wasm_client {
    use super::{HashMap, IpeResult, IpeTask, ok_res};
    use std::cell::{Cell, RefCell};

    thread_local! {
        static SOCKETS: RefCell<HashMap<i64, web_sys::WebSocket>> = RefCell::new(HashMap::new());
    }
    thread_local! {
        static NEXT_ID: Cell<i64> = const { Cell::new(1) };
    }

    fn next_id() -> i64 {
        NEXT_ID.with(|c| {
            let id = c.get();
            c.set(id + 1);
            id
        })
    }

    /// `IpeResult` has no `From<Result<A, E>>` impl (the ADT is Ipê-shaped, not
    /// a `std::result` newtype) — this is the total bridge every fn below uses.
    fn to_ipe<E, A>(r: Result<A, E>) -> IpeResult<E, A> {
        match r {
            Ok(a) => IpeResult::Ok(a),
            Err(e) => IpeResult::Err(e),
        }
    }

    fn open_socket<E: From<String> + 'static>(url: &str) -> Result<i64, E> {
        let ws = web_sys::WebSocket::new(url)
            .map_err(|e| E::from(format!("WebSocket.connect {url}: {e:?}")))?;
        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let id = next_id();
        SOCKETS.with(|s| s.borrow_mut().insert(id, ws));
        Ok(id)
    }

    /// `WebSocket.connect : String -> Task Error Int` — `web_sys::WebSocket::new`
    /// starts connecting asynchronously and returns immediately (readyState
    /// `CONNECTING`), matching the native surface's raw-id contract; `send`
    /// before the handshake completes is rejected below rather than trapping.
    pub fn web_socket_connect<E: From<String> + 'static>(url: String) -> IpeTask<E, i64> {
        Box::pin(async move { to_ipe(open_socket(&url)) })
    }

    /// `WebSocket.connectWith : WebSocketCfg -> Task Error Int`. `cfg.headers`
    /// cannot be attached: the browser `WebSocket` constructor has no header
    /// parameter (a real platform limitation, not a dropped feature on our
    /// side) — surfaced as a console warning rather than a silent drop.
    /// `cfg.timeout`/`cfg.pingInterval` have no browser-substitute wiring yet
    /// (the browser auto-manages ping/pong at the protocol level).
    pub fn web_socket_connect_with<E: From<String> + 'static>(
        cfg: super::WsClientCfg,
    ) -> IpeTask<E, i64> {
        Box::pin(async move {
            if !cfg.headers.is_empty() {
                crate::wasm::console_warn(
                    "Ipe.WebSocket.connectWith: custom headers are not settable via the \
                     browser WebSocket API; ignored",
                );
            }
            to_ipe(open_socket(&cfg.url))
        })
    }

    fn with_open_socket<E: From<String> + 'static, R>(
        id: i64,
        op_name: &str,
        f: impl FnOnce(&web_sys::WebSocket) -> Result<R, wasm_bindgen::JsValue>,
    ) -> Result<R, E> {
        SOCKETS.with(|s| {
            let sockets = s.borrow();
            let Some(ws) = sockets.get(&id) else {
                return Err(E::from(format!("{op_name}: no socket {id}")));
            };
            if ws.ready_state() != web_sys::WebSocket::OPEN {
                return Err(E::from(format!("{op_name}: socket {id} is not open")));
            }
            f(ws).map_err(|e| E::from(format!("{op_name}: {e:?}")))
        })
    }

    /// `WebSocket.send : Int -> String -> Task Error ()`.
    pub fn web_socket_send<E: From<String> + 'static>(id: i64, msg: String) -> IpeTask<E, ()> {
        Box::pin(async move {
            to_ipe(with_open_socket(id, "WebSocket.send", |ws| {
                ws.send_with_str(&msg)
            }))
        })
    }

    /// `WebSocket.sendBinary : Int -> Bytes -> Task Error ()`.
    pub fn web_socket_send_binary<E: From<String> + 'static>(
        id: i64,
        msg: Vec<u8>,
    ) -> IpeTask<E, ()> {
        Box::pin(async move {
            to_ipe(with_open_socket(id, "WebSocket.sendBinary", |ws| {
                ws.send_with_u8_array(&msg)
            }))
        })
    }

    fn close_socket(id: i64, code: Option<(u16, &str)>) {
        SOCKETS.with(|s| {
            if let Some(ws) = s.borrow_mut().remove(&id) {
                let _ = match code {
                    Some((c, reason)) => ws.close_with_code_and_reason(c, reason),
                    None => ws.close(),
                };
            }
        });
    }

    /// `WebSocket.close : Int -> Task Error ()` (idempotent, matches native).
    pub fn web_socket_close<E: 'static>(id: i64) -> IpeTask<E, ()> {
        Box::pin(async move {
            close_socket(id, None);
            ok_res(())
        })
    }

    /// `WebSocket.closeWithCode : Int -> String -> Int -> Task Error ()`. Same
    /// truncation guard as the native arm — an out-of-range code falls back to
    /// 1000 (normal closure) rather than silently wrapping.
    pub fn web_socket_close_with_code<E: 'static>(
        code: i64,
        reason: String,
        id: i64,
    ) -> IpeTask<E, ()> {
        Box::pin(async move {
            let ws_code = u16::try_from(code).unwrap_or(1000);
            close_socket(id, Some((ws_code, &reason)));
            ok_res(())
        })
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm_client::*;

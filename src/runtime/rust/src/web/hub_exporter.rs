//! Remote hub OTLP push — HubExporter.
//!
//! When `IPE_CONSOLE_HUB` is set, this background exporter batches logs + spans
//! and pushes them as **OTLP/JSON** to a remote `ipe console-serve` hub
//! (`POST <hub>/v1/logs`, `POST <hub>/v1/traces`, `Content-Type:
//! application/json`), bearer-authenticated via `IPE_CONSOLE_HUB_TOKEN`. A
//! batch that fails to push is kept in a bounded in-memory **spool** and retried
//! on the next tick, so a transient hub outage never drops telemetry on the
//! floor; transient hub outages never drop telemetry.
//!
//! OTLP/JSON over HTTP (the protobuf field names as JSON keys); no protobuf
//! dependency required.
//!
//! Spool backend: in-memory (bounded) — covers transient outages + retry.
//! File-spool restart-durability (`IPE_CONSOLE_SPOOL_MODE=file`,
//! env knobs `IPE_CONSOLE_SPOOL_*`) is not yet implemented; those env
//! knobs are not read.
//!
//! `live`-gated. Best-effort, no panic vectors: bounded offer queue (drop on
//! full), push failures fall back to the spool, the spool itself is bounded
//! (oldest batch evicted when full). No `unwrap`/`expect`/indexing.

use std::collections::VecDeque;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;

/// Hub OTLP collector base URL — presence enables the exporter.
const HUB_ENV: &str = "IPE_CONSOLE_HUB";
/// Bearer token (must be ≥32 bytes; shorter tokens are refused).
const TOKEN_ENV: &str = "IPE_CONSOLE_HUB_TOKEN";
/// Flush cadence (ms).
const INTERVAL_ENV: &str = "IPE_CONSOLE_BATCH_INTERVAL_MS";
/// Service name attached as the OTLP `service.name` resource attribute.
const SERVICE_ENV: &str = "IPE_SERVICE_NAME";

const DEFAULT_QUEUE_CAP: usize = 1024;
const DEFAULT_INTERVAL_MS: u64 = 2000;
const MIN_INTERVAL_MS: u64 = 100;
const MIN_TOKEN_BYTES: usize = 32;
/// Max batches held in the retry spool before the oldest is evicted.
const SPOOL_MAX_BATCHES: usize = 256;
/// Total bytes of spooled batch JSON before the oldest is evicted
/// (`IPE_CONSOLE_SPOOL_MAX_BYTES` default 100 MB). Bounds the spool by size
/// as well as by batch count — a single batch can be large.
const SPOOL_MAX_BYTES: usize = 100 * 1024 * 1024;
/// Max accumulated entries (logs + spans) held between flushes before an early
/// flush is forced. Bounds the in-memory accumulator by count rather than only
/// by the flush interval — a high-rate producer can otherwise grow it without
/// bound (the channel stays near-empty as the batcher drains it eagerly).
const MAX_BATCH_ENTRIES: usize = 4096;

/// One telemetry record queued for the exporter.
pub(crate) enum Entry {
    Log {
        ts_ms: u64,
        level: String,
        message: String,
    },
    Span {
        ts_ms: u64,
        name: String,
        dur_us: u64,
        ok: bool,
    },
    /// Synchronous flush request: batcher drains its buffer then acks via the
    /// oneshot. Used by `flush_now` for a bounded pre-exit drain.
    Flush(tokio::sync::oneshot::Sender<()>),
}

static SENDER: OnceLock<mpsc::Sender<Entry>> = OnceLock::new();

/// Enable the remote-hub OTLP exporter from env. No-op unless `IPE_CONSOLE_HUB`
/// is set. Refuses a too-short token (< 32 bytes). Idempotent; call once at boot.
pub async fn enable_from_env() {
    // Trim ONCE so the URL we validate is the exact one we push to. A
    // leading-whitespace value (" https://hub") otherwise passes the scheme
    // check (which trims) yet leaves the space in `base`, so every push fails
    // with an invalid URL.
    let hub = match crate::system::read_env_var(HUB_ENV) {
        Ok(h) if !h.trim().is_empty() => h.trim().to_string(),
        _ => return,
    };
    if SENDER.get().is_some() {
        return;
    }
    let token = crate::system::read_env_var(TOKEN_ENV).unwrap_or_default();
    if token.len() < MIN_TOKEN_BYTES {
        eprintln!(
            "[ipe.hub] {TOKEN_ENV} must be ≥{MIN_TOKEN_BYTES} bytes to push to {hub}; exporter disabled"
        );
        return;
    }
    // Secrets-in-transit: refuse to push a bearer token over cleartext HTTP.
    // A misconfigured `http://` hub URL would leak the ≥32-byte token on the
    // wire. Allow a non-https scheme only when no token is set (anonymous push).
    // PARSE the URL — a string prefix `http://localhost` ALSO matches
    // `http://localhost.evil.com`, leaking the bearer token over cleartext to an
    // attacker host. Accept https://, or http:// ONLY when the host is exactly a
    // loopback name/address.
    let scheme_ok = match reqwest::Url::parse(&hub) {
        Ok(u) => {
            u.scheme() == "https"
                || (u.scheme() == "http"
                    && matches!(
                        u.host_str(),
                        Some("localhost") | Some("127.0.0.1") | Some("[::1]")
                    ))
        }
        Err(_) => false,
    };
    if !scheme_ok {
        eprintln!(
            "[ipe.hub] refusing to push bearer token over non-https {HUB_ENV}={hub}; \
             use https:// (or a localhost loopback); exporter disabled"
        );
        return;
    }
    let interval_ms = crate::system::read_env_var(INTERVAL_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_INTERVAL_MS)
        .max(MIN_INTERVAL_MS);
    let service = match crate::system::read_env_var(SERVICE_ENV) {
        Ok(s) if !s.is_empty() => s,
        _ => "app".to_string(),
    };
    let base = hub.trim_end_matches('/').to_string();

    let (tx, rx) = mpsc::channel::<Entry>(DEFAULT_QUEUE_CAP);
    if SENDER.set(tx).is_err() {
        return;
    }
    eprintln!("[ipe.hub] OTLP push → {base}/v1/{{logs,traces}} every {interval_ms}ms");
    tokio::spawn(batcher(rx, base, token, service, interval_ms));
}

/// A ready-to-push OTLP/JSON payload for one signal endpoint.
#[derive(Clone)]
struct OtlpBatch {
    path: &'static str, // "/v1/logs" | "/v1/traces"
    json: String,
}

/// Accumulate entries; on each tick encode + push, retrying spooled batches
/// first. Channel close drains a final flush. A `Flush` sentinel drains
/// immediately and acks.
async fn batcher(
    mut rx: mpsc::Receiver<Entry>,
    base: String,
    token: String,
    service: String,
    interval_ms: u64,
) {
    // Cap each push so a hung/black-holed hub (TCP handshake completes, no HTTP
    // response — slowloris) can't wedge the exporter task forever.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let mut logs: Vec<(u64, String, String)> = Vec::new();
    let mut spans: Vec<(u64, String, u64, bool)> = Vec::new();
    let mut spool: VecDeque<OtlpBatch> = VecDeque::new();
    let mut tick = tokio::time::interval(Duration::from_millis(interval_ms));
    tick.tick().await; // skip the immediate first tick
    loop {
        tokio::select! {
            maybe = rx.recv() => match maybe {
                Some(Entry::Flush(ack)) => {
                    flush(&client, &base, &token, &service, &mut logs, &mut spans, &mut spool).await;
                    // Best-effort ack — ignore send errors (caller may have timed out).
                    let _ = ack.send(());
                }
                Some(Entry::Log { ts_ms, level, message }) => {
                    logs.push((ts_ms, level, message));
                    if logs.len() + spans.len() >= MAX_BATCH_ENTRIES {
                        flush(&client, &base, &token, &service, &mut logs, &mut spans, &mut spool).await;
                    }
                }
                Some(Entry::Span { ts_ms, name, dur_us, ok }) => {
                    spans.push((ts_ms, name, dur_us, ok));
                    if logs.len() + spans.len() >= MAX_BATCH_ENTRIES {
                        flush(&client, &base, &token, &service, &mut logs, &mut spans, &mut spool).await;
                    }
                }
                None => {
                    flush(&client, &base, &token, &service, &mut logs, &mut spans, &mut spool).await;
                    break;
                }
            },
            _ = tick.tick() => {
                flush(&client, &base, &token, &service, &mut logs, &mut spans, &mut spool).await;
            }
        }
    }
}

/// Best-effort pre-exit flush: sends a `Flush` sentinel and waits up to
/// `cap_ms` milliseconds for the batcher to drain its buffer and the spool.
/// No-op when the exporter is disabled or the channel is full. Never panics.
pub async fn flush_now(cap_ms: u64) {
    let Some(tx) = SENDER.get() else { return };
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<()>();
    // try_send: non-blocking; best-effort (telemetry only).
    if tx.try_send(Entry::Flush(ack_tx)).is_err() {
        return;
    }
    let _ = tokio::time::timeout(Duration::from_millis(cap_ms), ack_rx).await;
}

/// Encode the accumulated logs/spans into OTLP batches, then push the spool
/// (oldest first) + the new batches. Failed pushes go back to the spool
/// (bounded — oldest evicted on overflow).
#[allow(clippy::too_many_arguments)]
async fn flush(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    service: &str,
    logs: &mut Vec<(u64, String, String)>,
    spans: &mut Vec<(u64, String, u64, bool)>,
    spool: &mut VecDeque<OtlpBatch>,
) {
    if !logs.is_empty() {
        spool_push(
            spool,
            OtlpBatch {
                path: "/v1/logs",
                json: otlp_logs_json(service, logs),
            },
        );
        logs.clear();
    }
    if !spans.is_empty() {
        spool_push(
            spool,
            OtlpBatch {
                path: "/v1/traces",
                json: otlp_spans_json(service, spans),
            },
        );
        spans.clear();
    }
    // Drain the spool in order; re-spool anything that fails this round.
    let mut pending: VecDeque<OtlpBatch> = std::mem::take(spool);
    while let Some(batch) = pending.pop_front() {
        if !push_one(client, base, token, &batch).await {
            spool_push(spool, batch);
        }
    }
}

/// Push one OTLP batch. `true` on a 2xx; `false` (→ re-spool) on any error.
async fn push_one(client: &reqwest::Client, base: &str, token: &str, batch: &OtlpBatch) -> bool {
    let url = format!("{base}{}", batch.path);
    match client
        .post(&url)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(batch.json.clone())
        .send()
        .await
    {
        Ok(r) => r.status().is_success(),
        Err(e) => {
            eprintln!("[ipe.hub] push {url}: {e}");
            false
        }
    }
}

/// Bounded spool insert (evict oldest on overflow — never grows unbounded).
/// Caps BOTH the batch count and the total JSON byte size, since a single batch
/// can be large.
fn spool_push(spool: &mut VecDeque<OtlpBatch>, batch: OtlpBatch) {
    if spool.len() >= SPOOL_MAX_BATCHES {
        spool.pop_front();
    }
    spool.push_back(batch);
    // Byte cap: evict oldest until under the limit, but always keep at least the
    // batch we just pushed (a single oversized batch is preferable to losing it).
    let mut total: usize = spool.iter().map(|b| b.json.len()).sum();
    while total > SPOOL_MAX_BYTES && spool.len() > 1 {
        match spool.pop_front() {
            Some(b) => total = total.saturating_sub(b.json.len()),
            None => break,
        }
    }
}

fn ns(ts_ms: u64) -> String {
    (ts_ms as u128 * 1_000_000).to_string()
}

/// Minimal valid OTLP/JSON ResourceLogs payload.
fn otlp_logs_json(service: &str, logs: &[(u64, String, String)]) -> String {
    let records: Vec<serde_json::Value> = logs
        .iter()
        .map(|(ts, level, msg)| {
            serde_json::json!({
                "timeUnixNano": ns(*ts),
                "severityText": level,
                "body": { "stringValue": msg },
            })
        })
        .collect();
    serde_json::json!({
        "resourceLogs": [{
            "resource": { "attributes": [service_attr(service)] },
            "scopeLogs": [{ "scope": { "name": "ipe" }, "logRecords": records }],
        }]
    })
    .to_string()
}

/// Minimal valid OTLP/JSON ResourceSpans payload. `ok` → status code 1 (OK),
/// else 2 (ERROR) per the OTLP status-code enum.
fn otlp_spans_json(service: &str, spans: &[(u64, String, u64, bool)]) -> String {
    let records: Vec<serde_json::Value> = spans
        .iter()
        .map(|(ts, name, dur_us, ok)| {
            let start = *ts as u128 * 1_000_000;
            let end = start + (*dur_us as u128) * 1_000;
            serde_json::json!({
                "name": name,
                "startTimeUnixNano": start.to_string(),
                "endTimeUnixNano": end.to_string(),
                "status": { "code": if *ok { 1 } else { 2 } },
            })
        })
        .collect();
    serde_json::json!({
        "resourceSpans": [{
            "resource": { "attributes": [service_attr(service)] },
            "scopeSpans": [{ "scope": { "name": "ipe" }, "spans": records }],
        }]
    })
    .to_string()
}

fn service_attr(service: &str) -> serde_json::Value {
    serde_json::json!({ "key": "service.name", "value": { "stringValue": service } })
}

/// Non-blocking offer of a log. No-op when disabled or the queue is full.
pub fn offer_log(ts_ms: u64, level: &str, message: &str) {
    if let Some(tx) = SENDER.get() {
        let _ = tx.try_send(Entry::Log {
            ts_ms,
            level: level.to_string(),
            message: message.to_string(),
        });
    }
}

/// Non-blocking offer of a span.
pub fn offer_span(ts_ms: u64, name: &str, dur_us: u64, ok: bool) {
    if let Some(tx) = SENDER.get() {
        let _ = tx.try_send(Entry::Span {
            ts_ms,
            name: name.to_string(),
            dur_us,
            ok,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_without_enable_is_noop() {
        offer_log(0, "info", "ignored");
        offer_span(0, "noop", 0, true);
    }

    #[test]
    fn otlp_logs_shape_is_valid() {
        let body = otlp_logs_json("svc", &[(1_700_000_000_000, "error".into(), "boom".into())]);
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(
            v["resourceLogs"][0]["resource"]["attributes"][0]["key"],
            "service.name"
        );
        assert_eq!(
            v["resourceLogs"][0]["resource"]["attributes"][0]["value"]["stringValue"],
            "svc"
        );
        let rec = &v["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0];
        assert_eq!(rec["severityText"], "error");
        assert_eq!(rec["body"]["stringValue"], "boom");
        // ts in ns = ms * 1e6
        assert_eq!(rec["timeUnixNano"], "1700000000000000000");
    }

    #[test]
    fn otlp_spans_shape_and_status() {
        let body = otlp_spans_json(
            "svc",
            &[(1_700_000_000_000, "db.query".into(), 5000, false)],
        );
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        let rec = &v["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(rec["name"], "db.query");
        assert_eq!(rec["status"]["code"], 2); // not ok → ERROR
        // end = start + 5000us(5ms) → +5_000_000 ns
        assert_eq!(rec["startTimeUnixNano"], "1700000000000000000");
        assert_eq!(rec["endTimeUnixNano"], "1700000000005000000");
    }

    #[test]
    fn spool_is_bounded() {
        let mut s: VecDeque<OtlpBatch> = VecDeque::new();
        for i in 0..(SPOOL_MAX_BATCHES + 10) {
            spool_push(
                &mut s,
                OtlpBatch {
                    path: "/v1/logs",
                    json: i.to_string(),
                },
            );
        }
        assert_eq!(s.len(), SPOOL_MAX_BATCHES);
        // Oldest evicted → front is batch #10, not #0.
        assert_eq!(s.front().map(|b| b.json.as_str()), Some("10"));
    }

    // Full push path against a stub hub: flush encodes logs + spans to OTLP/JSON
    // and POSTs both to /v1/logs + /v1/traces with the bearer; a 2xx clears the
    // spool.
    #[tokio::test]
    async fn flush_pushes_otlp_with_bearer_and_clears_spool() {
        use axum::extract::State;
        use axum::{Router, routing::any};
        use std::sync::{Arc, Mutex};

        type Rec = Arc<Mutex<Vec<(String, String, String)>>>; // (path, auth, body)
        let rec: Rec = Arc::new(Mutex::new(Vec::new()));

        async fn capture(State(rec): State<Rec>, req: axum::extract::Request) -> &'static str {
            let path = req.uri().path().to_string();
            let auth = req
                .headers()
                .get("authorization")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("")
                .to_string();
            let body = axum::body::to_bytes(req.into_body(), 1 << 20)
                .await
                .unwrap_or_default();
            if let Ok(mut g) = rec.lock() {
                g.push((path, auth, String::from_utf8_lossy(&body).to_string()));
            }
            "OK"
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let app = Router::new().fallback(any(capture)).with_state(rec.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{port}");
        let token = "x".repeat(MIN_TOKEN_BYTES);
        let mut logs = vec![(
            1_700_000_000_000u64,
            "error".to_string(),
            "boom".to_string(),
        )];
        let mut spans = vec![(1_700_000_000_000u64, "db.query".to_string(), 5000u64, true)];
        let mut spool: VecDeque<OtlpBatch> = VecDeque::new();

        flush(
            &client, &base, &token, "svc", &mut logs, &mut spans, &mut spool,
        )
        .await;

        // Both signals delivered; spool empty (success); bearer present; OTLP valid.
        let got = rec.lock().map(|g| g.clone()).unwrap_or_default();
        assert_eq!(got.len(), 2, "expected /v1/logs + /v1/traces, got {got:?}");
        assert!(spool.is_empty(), "spool should be cleared on 2xx");
        let paths: Vec<&str> = got.iter().map(|(p, _, _)| p.as_str()).collect();
        assert!(
            paths.contains(&"/v1/logs") && paths.contains(&"/v1/traces"),
            "{paths:?}"
        );
        for (_, auth, body) in &got {
            assert_eq!(auth, &format!("Bearer {token}"));
            let _: serde_json::Value = serde_json::from_str(body).expect("valid OTLP json");
        }
    }
}

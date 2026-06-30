//! Observability federation — child→parent telemetry push.
//!
//! When a Rust Live app runs as a sub-app under a parent (`SKY_PARENT_URL` set),
//! this background exporter batches its logs + spans and POSTs them every
//! `SKY_OBSERVABILITY_PUSH_INTERVAL_MS` (default 2000) to the parent's
//! `/_sky/observability/ingest` — the symmetric counterpart to the receiver in
//! `live/console.rs`. Mirrors Go's `observability_push.go` (PushExporter).
//!
//! `live`-gated (uses reqwest). Best-effort end to end: a bounded queue drops on
//! overflow, POST failures warn + drop. The observability path must never block
//! or panic the request path. No `unwrap`/`expect`/indexing in any reachable
//! path.
//!
//! Deliberate divergence from `hub_exporter` (which keeps a bounded retry spool):
//! child→parent federation is a same-host loopback hop on a short cadence — a
//! transient parent blip is recovered by the very next tick's fresh batch, so the
//! added complexity + memory of a retry spool buys little here. The remote-hub
//! exporter spools because its push crosses the network to a possibly-distant hub.

use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;

/// Parent base URL — presence of this var means "I'm a sub-app, push upward".
const PARENT_ENV: &str = "SKY_PARENT_URL";
/// Flush cadence (ms).
const INTERVAL_ENV: &str = "SKY_OBSERVABILITY_PUSH_INTERVAL_MS";
/// Bounded queue depth override.
const BUFFER_ENV: &str = "SKY_OBSERVABILITY_BUFFER";
/// Shared secret the parent's ingest gate checks (`X-Sky-Ingest-Token`).
const TOKEN_ENV: &str = "SKY_INGEST_TOKEN";

const DEFAULT_QUEUE_CAP: usize = 1024;
const DEFAULT_INTERVAL_MS: u64 = 2000;
/// Floor on the flush interval so a typo can't spin a hot loop.
const MIN_INTERVAL_MS: u64 = 100;
/// Hard cap on the in-batcher accumulator so a high log/span rate over a long
/// flush interval can't grow `buf` without bound (the mpsc channel is bounded,
/// but the batcher drains it continuously into `buf`). Reaching the cap forces
/// an early flush instead of waiting for the tick.
const MAX_BATCH: usize = 8192;

/// One telemetry record queued for the exporter.
enum Entry {
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

/// Enable the push exporter from env. No-op unless `SKY_PARENT_URL` is set
/// (i.e. this process runs as a sub-app pushing UP to its parent's ingest —
/// federation). Idempotent. Call once at Live boot.
pub async fn enable_from_env() {
    let parent = match std::env::var(PARENT_ENV) {
        Ok(p) if !p.is_empty() => p,
        _ => return,
    };
    let interval_ms = std::env::var(INTERVAL_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_INTERVAL_MS);
    let ingest_url = format!("{}/_sky/observability/ingest", parent.trim_end_matches('/'));
    enable("federation", ingest_url, interval_ms);
}

/// Enable pushing THIS app's telemetry to a LOCAL console-child collector
/// ("push-to-local-collector"): a lean parent (no SQLite) batches its
/// in-RAM telemetry and POSTs it to the console child's
/// `/_sky/observability/ingest`, where the child (which owns sqlx + the store)
/// records → spills → serves it. Called by the console mount after the child is
/// ready, when the parent has no spill of its own.
pub async fn enable_to_console(child_port: u16) {
    let ingest_url = format!("http://127.0.0.1:{child_port}/_sky/observability/ingest");
    enable("console-collector", ingest_url, DEFAULT_INTERVAL_MS);
}

/// Shared activation: bound the interval, claim the SENDER, spawn the batcher.
/// Idempotent (first caller wins the OnceLock — a sub-app pushes to its parent
/// OR a top-level app pushes to its console child, never both).
fn enable(label: &str, ingest_url: String, interval_ms: u64) {
    if SENDER.get().is_some() {
        return;
    }
    let interval_ms = interval_ms.max(MIN_INTERVAL_MS);
    let cap = std::env::var(BUFFER_ENV)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&c| c > 0)
        .unwrap_or(DEFAULT_QUEUE_CAP);
    let token = std::env::var(TOKEN_ENV).ok().filter(|t| !t.is_empty());
    let (tx, rx) = mpsc::channel::<Entry>(cap);
    if SENDER.set(tx).is_err() {
        return; // lost an enable race
    }
    eprintln!("[sky.push] {label} push → {ingest_url} every {interval_ms}ms");
    tokio::spawn(batcher(rx, ingest_url, token, interval_ms));
}

/// Accumulate entries and flush a batch on each tick. Channel close drains a
/// final batch then exits. A `Flush` sentinel drains immediately and acks.
async fn batcher(
    mut rx: mpsc::Receiver<Entry>,
    ingest_url: String,
    token: Option<String>,
    interval_ms: u64,
) {
    // Explicit timeouts so a parent that accepts the TCP connection but never
    // responds (slow/hung/half-dead) can't wedge the batcher task (and, through
    // it, `flush_now`'s pre-exit drain) forever. Total fallback — never panics.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .connect_timeout(Duration::from_secs(2))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let mut buf: Vec<Entry> = Vec::new();
    let mut tick = tokio::time::interval(Duration::from_millis(interval_ms));
    // The first tick fires immediately; skip it so we don't flush an empty batch.
    tick.tick().await;
    loop {
        tokio::select! {
            maybe = rx.recv() => match maybe {
                Some(Entry::Flush(ack)) => {
                    if !buf.is_empty() {
                        flush(&client, &ingest_url, token.as_deref(), &buf).await;
                        buf.clear();
                    }
                    // Best-effort ack — ignore send errors (caller may have timed out).
                    let _ = ack.send(());
                }
                Some(e) => {
                    buf.push(e);
                    // Bound the accumulator: flush early at the cap rather than
                    // letting it grow until the next tick.
                    if buf.len() >= MAX_BATCH {
                        flush(&client, &ingest_url, token.as_deref(), &buf).await;
                        buf.clear();
                    }
                }
                None => {
                    if !buf.is_empty() {
                        flush(&client, &ingest_url, token.as_deref(), &buf).await;
                    }
                    break;
                }
            },
            _ = tick.tick() => {
                if !buf.is_empty() {
                    flush(&client, &ingest_url, token.as_deref(), &buf).await;
                    buf.clear();
                }
            }
        }
    }
}

/// Best-effort pre-exit flush: sends a `Flush` sentinel and waits up to
/// `cap_ms` milliseconds for the batcher to drain its buffer. No-op when the
/// exporter is disabled or the channel is full. Never panics.
pub async fn flush_now(cap_ms: u64) {
    let Some(tx) = SENDER.get() else { return };
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<()>();
    // try_send: non-blocking; if the channel is full the flush is skipped
    // (best-effort — this is telemetry only, never user/persistent data).
    if tx.try_send(Entry::Flush(ack_tx)).is_err() {
        return;
    }
    let _ = tokio::time::timeout(Duration::from_millis(cap_ms), ack_rx).await;
}

/// Build the `{ "logs": [...], "spans": [...] }` payload the receiver accepts.
/// serde_json (a `live` dep) keeps the escaping correct.
fn build_payload(buf: &[Entry]) -> String {
    let mut logs = Vec::new();
    let mut spans = Vec::new();
    for e in buf {
        match e {
            Entry::Log {
                ts_ms,
                level,
                message,
            } => logs.push(serde_json::json!({
                "ts": ts_ms, "level": level, "message": message,
            })),
            Entry::Span {
                ts_ms,
                name,
                dur_us,
                ok,
            } => spans.push(serde_json::json!({
                "ts": ts_ms, "name": name, "durUs": dur_us, "ok": ok,
            })),
            // The batcher clears the buf before any Flush sentinel reaches here;
            // this arm is unreachable in practice but required for exhaustiveness.
            Entry::Flush(_) => {}
        }
    }
    serde_json::json!({ "logs": logs, "spans": spans }).to_string()
}

/// POST one batch to the parent ingest. Failures warn + drop (best-effort).
async fn flush(client: &reqwest::Client, ingest_url: &str, token: Option<&str>, buf: &[Entry]) {
    let body = build_payload(buf);
    let mut req = client
        .post(ingest_url)
        .header("content-type", "application/json")
        .body(body);
    if let Some(t) = token {
        req = req.header("x-sky-ingest-token", t);
    }
    if let Err(e) = req.send().await {
        eprintln!("[sky.push] flush to {ingest_url}: {e}");
    }
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
    fn payload_shape_logs_and_spans() {
        let buf = vec![
            Entry::Log {
                ts_ms: 1700,
                level: "error".into(),
                message: "boom \"x\"".into(),
            },
            Entry::Span {
                ts_ms: 1700,
                name: "db.query".into(),
                dur_us: 5000,
                ok: true,
            },
        ];
        let body = build_payload(&buf);
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(v["logs"][0]["level"], "error");
        assert_eq!(v["logs"][0]["message"], "boom \"x\"");
        assert_eq!(v["spans"][0]["name"], "db.query");
        assert_eq!(v["spans"][0]["ok"], true);
        assert_eq!(v["spans"][0]["durUs"], 5000);
    }

    /// Verify that the `Flush` sentinel causes the batcher to drain its
    /// buffer and send the ack. This is the load-bearing seam for
    /// `flush_now`: the ack proves the buffered batch was processed before
    /// the pre-exit window expires.
    ///
    /// Uses a fake HTTP server (httptest) is not a dep here — instead we
    /// verify the sentinel path purely at the channel level: the batcher
    /// receives the Flush, calls flush() on the buf (which POSTs; we use a
    /// channel cap=0 so the buf is empty → the POST is skipped), and sends
    /// the ack. We send a Log first to ensure non-empty buf, but since
    /// we can't stand up a real HTTP server in a unit test, we rely on the
    /// "flush POST fails with a log warning" path (best-effort) and confirm
    /// the ack still arrives — i.e. a flush failure does NOT prevent the ack.
    #[tokio::test]
    async fn flush_sentinel_acks_even_when_ingest_unreachable() {
        let (tx, rx) = mpsc::channel::<Entry>(16);
        // Pre-load a log entry into the channel (will land in the batcher's buf).
        tx.try_send(Entry::Log {
            ts_ms: 42,
            level: "info".into(),
            message: "pre-exit".into(),
        })
        .ok();
        // Send the Flush sentinel.
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<()>();
        tx.try_send(Entry::Flush(ack_tx)).ok();
        // Drop the sender so the batcher exits after the ack.
        drop(tx);

        // Spawn the batcher against an unreachable URL.
        tokio::spawn(batcher(rx, "http://127.0.0.1:19999".into(), None, 5000));

        // The ack must arrive within 500 ms — same cap as flush_now uses.
        let result = tokio::time::timeout(Duration::from_millis(500), ack_rx).await;
        assert!(result.is_ok(), "flush ack must arrive within 500 ms");
        assert!(result.unwrap().is_ok(), "ack oneshot must not be dropped");
    }

    /// `flush_now` is a no-op when the exporter is disabled (SENDER not set).
    #[tokio::test]
    async fn flush_now_noop_when_disabled() {
        // flush_now on a fresh (not-enabled) state must return quickly.
        let deadline = tokio::time::timeout(Duration::from_millis(200), flush_now(250)).await;
        assert!(
            deadline.is_ok(),
            "flush_now must not block when exporter is off"
        );
    }
}

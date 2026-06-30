use tokio::sync::mpsc;

/// One framed SSE message body (already serialized patch-envelope JSON).
#[derive(Clone, Debug)]
pub struct SsePatch(pub String);

pub type SseTx = mpsc::Sender<SsePatch>;
pub type SseRx = mpsc::Receiver<SsePatch>;

/// Buffer capacity, honouring `SKY_LIVE_SSE_BUFFER` (clamped to `[1, 1024]`,
/// default 16) to match the Go runtime's configurable bound. Parse failures
/// and out-of-range values fall back to the clamp/default.
fn buffer_capacity() -> usize {
    const DEFAULT: usize = 16;
    const MIN: usize = 1;
    const MAX: usize = 1024;
    std::env::var("SKY_LIVE_SSE_BUFFER")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .map(|n| n.clamp(MIN, MAX))
        .unwrap_or(DEFAULT)
}

/// Bounded buffer (Go default 16, configurable via `SKY_LIVE_SSE_BUFFER`). The
/// current caller in mod.rs `.await`s on send, so this channel BLOCKS (applies
/// TCP backpressure) when full rather than dropping — it does not implement the
/// drop-oldest + `sky_live_sse_drops_total` behaviour. hello/heartbeat framing
/// is done in mod.rs when wiring axum.
pub fn channel() -> (SseTx, SseRx) {
    mpsc::channel(buffer_capacity())
}

/// SSE event framing: `event: <name>\ndata: <payload>\n\n`.
///
/// Self-defending against SSE injection: event names are single-line per the
/// spec, so any CR/LF is stripped (a crafted name otherwise injects fields or
/// terminates the event early); `data` is emitted as one `data: ` field per
/// line so a raw newline cannot inject extra fields or end the message —
/// independent of caller-side JSON escaping. For the common single-line JSON
/// payload the output is byte-identical to `event: <name>\ndata: <payload>\n\n`.
pub fn frame(event: &str, data: &str) -> String {
    let event = event.replace(['\r', '\n'], "");
    let mut out = String::with_capacity(event.len() + data.len() + 16);
    out.push_str("event: ");
    out.push_str(&event);
    out.push('\n');
    for line in data.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        out.push_str("data: ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    out
}

// ─── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_formats_correctly() {
        let s = frame("hello", r#"{"seq":0}"#);
        assert_eq!(s, "event: hello\ndata: {\"seq\":0}\n\n");
    }

    #[test]
    fn channel_returns_bounded_pair() {
        let (tx, _rx) = channel();
        // capacity is 16; can send without await from sync context via try_send
        assert!(tx.try_send(SsePatch("test".into())).is_ok());
    }
}

use tokio::sync::mpsc;

/// One framed SSE message body (already serialized patch-envelope JSON).
#[derive(Clone, Debug)]
pub struct SsePatch(pub String);

pub type SseTx = mpsc::Sender<SsePatch>;
pub type SseRx = mpsc::Receiver<SsePatch>;

/// Buffer capacity, honouring `IPE_WEB_SSE_BUFFER` (clamped to `[1, 1024]`,
/// default 16). Parse failures and out-of-range values fall back to the
/// clamp/default.
fn buffer_capacity() -> usize {
    const DEFAULT: usize = 16;
    const MIN: usize = 1;
    const MAX: usize = 1024;
    crate::system::read_env_var("IPE_WEB_SSE_BUFFER")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .map(|n| n.clamp(MIN, MAX))
        .unwrap_or(DEFAULT)
}

/// Bounded buffer (default 16, configurable via `IPE_WEB_SSE_BUFFER`). The
/// current caller in mod.rs `.await`s on send, so this channel BLOCKS (applies
/// TCP backpressure) when full rather than dropping — it does not implement the
/// drop-oldest + `ipe_web_sse_drops_total` behaviour. hello/heartbeat framing
/// is done in mod.rs when wiring axum.
pub fn channel() -> (SseTx, SseRx) {
    mpsc::channel(buffer_capacity())
}

/// SSE event framing: `event: <name>\ndata: <payload>\n\n`.
///
/// Self-defending against SSE injection: event names are single-line per the
/// spec, so any CR/LF is stripped (a crafted name otherwise injects fields or
/// terminates the event early); `data` is emitted as one `data: ` field per
/// line so no line terminator in the payload can inject extra fields or end the
/// message — independent of caller-side JSON escaping. The SSE spec treats CR,
/// LF, and CRLF each as a line terminator, so all three break `data` into
/// fields here; an interior lone CR cannot smuggle a `data:`/`event:` line into
/// a compliant EventSource. For the common single-line JSON payload the output
/// is byte-identical to `event: <name>\ndata: <payload>\n\n`.
pub fn frame(event: &str, data: &str) -> String {
    let event = event.replace(['\r', '\n'], "");
    let mut out = String::with_capacity(event.len() + data.len() + 16);
    out.push_str("event: ");
    out.push_str(&event);
    out.push('\n');
    for line in split_sse_lines(data) {
        out.push_str("data: ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    out
}

/// Split `data` on SSE line terminators — CR, LF, and CRLF each end one line —
/// yielding the terminator-free content of every line (including a trailing
/// empty line after a final terminator).
fn split_sse_lines(data: &str) -> Vec<&str> {
    let bytes = data.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'\n' => {
                lines.push(data.get(start..i).unwrap_or_default());
                i += 1;
                start = i;
            }
            b'\r' => {
                lines.push(data.get(start..i).unwrap_or_default());
                // A CRLF pair is a single terminator, not two.
                i += if bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = i;
            }
            _ => i += 1,
        }
    }
    lines.push(data.get(start..).unwrap_or_default());
    lines
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

    #[test]
    fn interior_lone_cr_cannot_inject_a_field() {
        // A compliant EventSource treats a lone CR as a line terminator, so the
        // `event:` text after the CR must land on its own `data: ` line rather
        // than continuing the first field.
        let s = frame("hello", "a\revent: injected");
        assert_eq!(s, "event: hello\ndata: a\ndata: event: injected\n\n");
    }

    #[test]
    fn crlf_is_one_terminator() {
        let s = frame("hello", "a\r\nb");
        assert_eq!(s, "event: hello\ndata: a\ndata: b\n\n");
    }

    #[test]
    fn lf_split_is_unchanged() {
        let s = frame("hello", "a\nb");
        assert_eq!(s, "event: hello\ndata: a\ndata: b\n\n");
    }
}

//! In-process telemetry sink — the data the Ipê Console renders.
//!
//! Always compiled (so `Ipe.Log.*` can feed it regardless of features); the
//! Ipe.Web `console` module exposes it over HTTP. Bounded ring buffers (logs +
//! errors) plus monotonic request/error counters. Mirrors the in-RAM tier of
//! Go's console (`runtime-go/rt/console*.go`), minus the `SQLite` spill.
//!
//! No panic vectors: a poisoned lock recovers via `into_inner()` (the data is
//! plain records — a panic mid-push can't corrupt invariants); all reads/writes
//! are bounded.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(all(target_arch = "wasm32", feature = "wasm-client")))]
use std::time::{SystemTime, UNIX_EPOCH};

const LOG_CAP: usize = 1000;
const ERR_CAP: usize = 200;
const SPAN_CAP: usize = 500;

/// One captured log line.
#[derive(Clone)]
pub struct LogEntry {
    pub ts_ms: u64,
    pub level: String,
    pub message: String,
}

/// One completed trace span (Ipe.Trace.span).
#[derive(Clone)]
pub struct SpanEntry {
    pub ts_ms: u64,
    pub name: String,
    pub dur_us: u64,
    pub ok: bool,
}

static LOGS: Mutex<VecDeque<LogEntry>> = Mutex::new(VecDeque::new());
static ERRORS: Mutex<VecDeque<LogEntry>> = Mutex::new(VecDeque::new());
static SPANS: Mutex<VecDeque<SpanEntry>> = Mutex::new(VecDeque::new());
static REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static ERRORS_TOTAL: AtomicU64 = AtomicU64::new(0);

// `SystemTime::now()` COMPILES on `wasm32-unknown-unknown` (part of std) but
// TRAPS at runtime — no clock without `wasmbind`. That's harmless for the bare
// pure-kernel floor (nothing there calls `record_log`), but once the
// `wasm-client` browser sink makes `Ipe.Log.*` reachable it would be a
// well-typed-program-reachable trap. Route through `js_sys::Date::now()`
// (`Date.now()`) specifically when `wasm-client` is on; the floor-only wasm32
// build (no `wasm-client`, `js-sys` not even a resolvable dependency there)
// keeps the original std path unchanged.
#[cfg(not(all(target_arch = "wasm32", feature = "wasm-client")))]
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(all(target_arch = "wasm32", feature = "wasm-client"))]
fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

fn push_bounded<T>(ring: &Mutex<VecDeque<T>>, cap: usize, e: T) {
    let mut g = ring
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if g.len() >= cap {
        g.pop_front();
    }
    g.push_back(e);
}

/// Forward a record to the SQLite spill when enabled. A no-op
/// stub keeps this always-compiled sink tokio/sqlx-free when `db` is off.
#[cfg(feature = "db")]
#[inline]
fn spill_log(ts_ms: u64, level: &str, message: &str) {
    crate::telemetry_spill::offer_log(ts_ms, level, message);
}
#[cfg(not(feature = "db"))]
#[inline]
fn spill_log(_ts_ms: u64, _level: &str, _message: &str) {}

#[cfg(feature = "db")]
#[inline]
fn spill_span(ts_ms: u64, name: &str, dur_us: u64, ok: bool) {
    crate::telemetry_spill::offer_span(ts_ms, name, dur_us, ok);
}
#[cfg(not(feature = "db"))]
#[inline]
fn spill_span(_ts_ms: u64, _name: &str, _dur_us: u64, _ok: bool) {}

/// Forward a record to the remote exporters — federation push to the parent
/// ingest and the remote hub OTLP push. `live`-gated; a no-op
/// stub keeps the always-compiled sink reqwest/tokio-free for non-live programs.
/// Each exporter is independently env-gated and a non-blocking drop-on-full
/// offer, so this never blocks or panics the caller.
#[cfg(feature = "web")]
#[inline]
fn export_log(ts_ms: u64, level: &str, message: &str) {
    crate::web::push_exporter::offer_log(ts_ms, level, message);
    crate::web::hub_exporter::offer_log(ts_ms, level, message);
}
#[cfg(not(feature = "web"))]
#[inline]
fn export_log(_ts_ms: u64, _level: &str, _message: &str) {}

#[cfg(feature = "web")]
#[inline]
fn export_span(ts_ms: u64, name: &str, dur_us: u64, ok: bool) {
    crate::web::push_exporter::offer_span(ts_ms, name, dur_us, ok);
    crate::web::hub_exporter::offer_span(ts_ms, name, dur_us, ok);
}
#[cfg(not(feature = "web"))]
#[inline]
fn export_span(_ts_ms: u64, _name: &str, _dur_us: u64, _ok: bool) {}

/// Record a completed trace span (called from `Ipe.Trace.span`).
pub fn record_span(name: &str, dur_us: u64, ok: bool) {
    let ts = now_ms();
    push_bounded(
        &SPANS,
        SPAN_CAP,
        SpanEntry {
            ts_ms: ts,
            name: name.to_string(),
            dur_us,
            ok,
        },
    );
    spill_span(ts, name, dur_us, ok);
    export_span(ts, name, dur_us, ok);
}

/// Most-recent `limit` spans as a JSON array.
pub fn spans_json(limit: usize) -> String {
    let g = SPANS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let n = g.len();
    let items: Vec<String> = g
        .iter()
        .skip(n.saturating_sub(limit))
        .map(|s| {
            format!(
                r#"{{"ts":{},"name":"{}","durUs":{},"ok":{}}}"#,
                s.ts_ms,
                json_escape(&s.name),
                s.dur_us,
                s.ok
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// Production gate (Go's `productionFromEnv`): `ENV` then `IPE_ENV`; unset OR a
/// dev marker (`dev`/`development`/`local`) → dev (false); anything else → true.
#[must_use]
pub fn production_from_env() -> bool {
    let mut e = crate::system::read_env_var("ENV")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if e.is_empty() {
        e = crate::system::read_env_var("IPE_ENV")
            .unwrap_or_default()
            .to_ascii_lowercase();
    }
    if e.is_empty() {
        return false;
    }
    !matches!(e.as_str(), "dev" | "development" | "local")
}

/// Floating "🔍 Console" link injected into every dev-mode `text/html` response
/// — both the Ipe.Web page path and every buffered Ipe.Http.Server response
/// (Go parity: `devBannerHTML`, `dev_banner.go`). Lives here (the always-compiled
/// telemetry module) rather than under `live` so the server path (`server.rs`,
/// where the `live` module is DCE'd out of server-only builds) can reach it too.
///
/// Suppressed for a sub-app (`base` non-empty — e.g. the bundled console child
/// itself; a console link inside the console is recursive), in production
/// (`ENV`/`IPE_ENV` non-dev), when the banner is turned off (`IPE_DEV_BANNER=off|0`,
/// Go parity), and when the console surface is disabled (`IPE_CONSOLE_EMBED=off`
/// / `IPE_CONSOLE_AUTH=off`). The union of Go's and the live path's gates —
/// suppression only ever makes bodies match MORE often across odd configs, and
/// the sweep's env (nothing set) hits the injecting path either way.
///
/// Rendered as a sibling of `#ipe-root` on the Web path (so a body patch never
/// blows it away); `position:fixed` pins it bottom-right and `pointer-events`
/// stays default so the link is clickable.
#[must_use]
pub fn dev_console_banner(base: &str) -> String {
    if !base.is_empty() || production_from_env() {
        return String::new();
    }
    if matches!(
        crate::system::read_env_var("IPE_DEV_BANNER").as_deref(),
        Ok("off" | "0")
    ) {
        return String::new();
    }
    if matches!(
        crate::system::read_env_var("IPE_CONSOLE_EMBED").as_deref(),
        Ok("off" | "0" | "false")
    ) || crate::system::read_env_var("IPE_CONSOLE_AUTH").is_ok_and(|v| v == "off")
    {
        return String::new();
    }
    // Byte-match Go's `devBannerHTML` (`dev_banner.go`): same id, target/rel/title,
    // monospace blue styling, and the `&#128269;` entity (NOT a literal emoji) so
    // both backends emit identical bytes. href honours `IPE_CONSOLE_URL` (default
    // `/_ipe/console`), attribute-escaped against a hostile env value.
    let url = crate::system::read_env_var("IPE_CONSOLE_URL")
        .map(|v| v.trim().to_string())
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/_ipe/console".to_string());
    let esc = url
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;");
    format!(
        "<a id=\"__ipe-dev-console\" href=\"{esc}\" target=\"_blank\" rel=\"noopener\" \
         title=\"Ipe Console (dev only)\" \
         style=\"position:fixed;right:12px;bottom:12px;z-index:2147483646;\
         font:12px/1.4 ui-monospace,Menlo,monospace;\
         background:#1c2027;color:#7eb6ff;\
         border:1px solid #353b46;border-radius:6px;\
         padding:6px 10px;text-decoration:none;\
         box-shadow:0 2px 8px rgba(0,0,0,0.4);\">\
         &#128269; Console</a>"
    )
}

/// Insert `banner` just before the LAST case-insensitive `</body>` tag (Go
/// parity: `injectDevBanner`, `dev_banner.go`). Falls back to appending when no
/// `</body>` is present (body-only fragments). An empty banner is a no-op.
#[must_use]
pub fn inject_dev_banner(body: &str, banner: &str) -> String {
    if banner.is_empty() {
        return body.to_string();
    }
    let low = body.to_ascii_lowercase();
    // `idx` is the byte offset of the ASCII "</body>" in the lowercased copy;
    // `to_ascii_lowercase` is byte-length-preserving on ASCII and never
    // touches multi-byte UTF-8 lead/continuation bytes, so `idx` is a valid
    // char boundary in `body` too — the `body[..idx]` / `body[idx..]` slices
    // cannot split a codepoint (no panic).
    if let Some(idx) = low.rfind("</body>") {
        let mut out = String::with_capacity(body.len() + banner.len());
        out.push_str(&body[..idx]);
        out.push_str(banner);
        out.push_str(&body[idx..]);
        out
    } else {
        let mut out = String::with_capacity(body.len() + banner.len());
        out.push_str(body);
        out.push_str(banner);
        out
    }
}

/// `Some(value)` when responses run in cross-origin-iframe mode
/// (`IPE_LIVE_FRAME_ANCESTORS` set — the `IpeDeploy` control-plane embeds the
/// console). Snapshotted once into a `OnceLock` so env is read only once
/// (eliminates the TOCTOU window where a dynamic env mutation could split the
/// cookie name / CSP framing decision within a single request).
///
/// Lives here (the always-compiled telemetry module) rather than under `live`
/// so the Ipe.Http.Server path (`server.rs`) can reach it too — the `live`
/// module is DCE'd out of server-only builds.
pub fn frame_ancestors() -> Option<&'static str> {
    use std::sync::OnceLock;
    static FA: OnceLock<String> = OnceLock::new();
    let v = FA.get_or_init(|| {
        // Strip CR / LF / NUL: this value is spliced verbatim into the
        // Content-Security-Policy response header (server.rs / live). A CR or
        // LF would terminate the header line and inject a new response header
        // (HTTP response splitting); NUL is rejected by header encoders. The
        // remaining `frame-ancestors` source-list grammar is the operator's
        // responsibility — we only close the response-splitting vector.
        crate::system::read_env_var("IPE_LIVE_FRAME_ANCESTORS")
            .unwrap_or_default()
            .chars()
            .filter(|c| !matches!(c, '\r' | '\n' | '\0'))
            .collect()
    });
    if v.is_empty() { None } else { Some(v.as_str()) }
}

/// Safe-by-default security response headers (Go parity: `setSecurityHeaders`,
/// live.go:3557 — applied on both the Ipe.Web page path and the Ipe.Http.Server
/// response path, rt.go:7838). Returned as owned `(name, value)` pairs so each
/// caller splices them into its response builder only when the header is unset
/// (an explicit handler override wins).
#[must_use]
pub fn security_headers() -> Vec<(&'static str, String)> {
    let mut h: Vec<(&'static str, String)> = vec![
        // Go parity.
        ("x-content-type-options", "nosniff".to_string()),
        (
            "referrer-policy",
            "strict-origin-when-cross-origin".to_string(),
        ),
        // Beyond Go: deny powerful features by default for a server-rendered app.
        (
            "permissions-policy",
            "geolocation=(), microphone=(), camera=(), payment=()".to_string(),
        ),
    ];
    // Framing: CSP frame-ancestors when an embed origin is configured, else
    // X-Frame-Options: SAMEORIGIN (mutually exclusive, Go parity).
    match frame_ancestors() {
        Some(fa) => h.push(("content-security-policy", format!("frame-ancestors {fa}"))),
        None => h.push(("x-frame-options", "SAMEORIGIN".to_string())),
    }
    h
}

/// Record a structured log line (called from `Ipe.Log.*`). Errors also land in
/// the error ring + bump the error counter.
pub fn record_log(level: &str, message: &str) {
    let ts = now_ms();
    let e = LogEntry {
        ts_ms: ts,
        level: level.to_string(),
        message: message.to_string(),
    };
    if level.eq_ignore_ascii_case("error") {
        ERRORS_TOTAL.fetch_add(1, Ordering::Relaxed);
        push_bounded(&ERRORS, ERR_CAP, e.clone());
    }
    push_bounded(&LOGS, LOG_CAP, e);
    spill_log(ts, level, message);
    export_log(ts, level, message);
}

/// Record one served HTTP request (called from the Web counter middleware).
pub fn record_request(status: u16) {
    REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    if status >= 500 {
        ERRORS_TOTAL.fetch_add(1, Ordering::Relaxed);
        metric_inc("ipe_web_errors_total", &[], 1);
    }
}

pub fn requests_total() -> u64 {
    REQUESTS_TOTAL.load(Ordering::Relaxed)
}
pub fn errors_total() -> u64 {
    ERRORS_TOTAL.load(Ordering::Relaxed)
}

// ===========================================
// Labeled metric registry + Prometheus exposition (Go parity:
// telemetry/store.go + prometheus.go). Labeled counters + gauges + histograms
// keyed by (name, sorted-labels), rendered as canonical 0.0.4 text — giving an
// operator pointing Prometheus/Grafana at a Rust Ipê binary the full
// route/status/SSE breakdown.
// ===========================================

use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MetricKey {
    name: String,
    /// Label pairs, kept sorted so two call sites with the same labels in a
    /// different order map to the same series (and the exposition is stable).
    ///
    /// CARDINALITY CONSTRAINT (read before adding a labeled series): label
    /// VALUES MUST be bounded / low-cardinality — a fixed status class, a
    /// route template, etc. NEVER a session id, raw request path, user id, or
    /// any unbounded value. The registry creates one entry per distinct
    /// `(name, labels)` and NEVER evicts, so an unbounded label is a
    /// memory-DoS (the classic Prometheus cardinality explosion). All current
    /// call sites pass `&[]`.
    labels: Vec<(String, String)>,
}

enum MetricValue {
    Counter(u64),
    Gauge(i64),
    /// Cumulative histogram: `buckets[i]` counts observations `<= boundaries[i]`
    /// (Prometheus cumulative semantics); the `+Inf` bucket is `count`.
    Histogram {
        boundaries: Vec<f64>,
        buckets: Vec<u64>,
        sum: f64,
        count: u64,
    },
}

/// Go's `BucketsLatency` (buckets.go) — hot-path latency seconds, 1ms…5s.
const LATENCY_BUCKETS: [f64; 8] = [0.001, 0.005, 0.010, 0.050, 0.100, 0.500, 1.0, 5.0];

// `Mutex::new` + `BTreeMap::new` are const → a plain static, no OnceLock. BTree
// iteration is sorted by (name, labels), giving deterministic, grouped output.
static REGISTRY: Mutex<BTreeMap<MetricKey, MetricValue>> = Mutex::new(BTreeMap::new());

fn norm_labels(labels: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = labels
        .iter()
        .map(|(k, val)| ((*k).to_string(), (*val).to_string()))
        .collect();
    v.sort();
    v
}

/// Add `by` to a labeled counter (creating it at 0 first). A name already
/// registered as a gauge is left untouched (defensive — a given name is touched
/// by exactly ONE variant; mixing counter/gauge writes on one name silently
/// no-ops the mismatch, so don't). See `MetricKey.labels` for the cardinality
/// constraint on `labels`.
pub fn metric_inc(name: &str, labels: &[(&str, &str)], by: u64) {
    let key = MetricKey {
        name: name.to_string(),
        labels: norm_labels(labels),
    };
    let mut g = REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match g.entry(key).or_insert(MetricValue::Counter(0)) {
        MetricValue::Counter(c) => *c = c.saturating_add(by),
        MetricValue::Gauge(_) | MetricValue::Histogram { .. } => {}
    }
}

/// Adjust a labeled gauge by `delta` (creating it at 0 first). Saturating, and
/// floored at 0 — the gauges here (active sessions / connections) never go
/// negative in correct operation; the floor stops a double-decrement underflow.
pub fn metric_add_gauge(name: &str, labels: &[(&str, &str)], delta: i64) {
    let key = MetricKey {
        name: name.to_string(),
        labels: norm_labels(labels),
    };
    let mut g = REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match g.entry(key).or_insert(MetricValue::Gauge(0)) {
        MetricValue::Gauge(v) => *v = v.saturating_add(delta).max(0),
        MetricValue::Counter(_) | MetricValue::Histogram { .. } => {}
    }
}

/// Record a latency/duration `v` (seconds) into a labeled histogram (creating it
/// with the `BucketsLatency` boundaries first). Cumulative: bumps every bucket
/// whose boundary `>= v` (Go's `Observe`). Labels MUST be low-cardinality (see
/// `MetricKey.labels`) — callers pass `&[]` or a bounded class, NEVER a raw path.
pub fn metric_observe(name: &str, labels: &[(&str, &str)], v: f64) {
    // Contract guard: a non-finite or negative observation would poison `_sum`
    // (e.g. `_sum NaN`) and skip every finite bucket while still bumping `count`.
    // The sole current caller passes a provably-finite, non-negative duration;
    // this guards a future caller from corrupting the exposition.
    if !v.is_finite() || v < 0.0 {
        return;
    }
    let key = MetricKey {
        name: name.to_string(),
        labels: norm_labels(labels),
    };
    let mut g = REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let entry = g.entry(key).or_insert_with(|| MetricValue::Histogram {
        boundaries: LATENCY_BUCKETS.to_vec(),
        buckets: vec![0; LATENCY_BUCKETS.len()],
        sum: 0.0,
        count: 0,
    });
    if let MetricValue::Histogram {
        boundaries,
        buckets,
        sum,
        count,
    } = entry
    {
        for (i, b) in boundaries.iter().enumerate() {
            if v <= *b
                && let Some(c) = buckets.get_mut(i)
            {
                *c = c.saturating_add(1);
            }
        }
        *sum += v;
        *count = count.saturating_add(1);
    }
}

/// Extract the BOUNDED variant name from a `Debug` value, for use as a
/// low-cardinality metric label (e.g. `ipe_web_msg_seconds{name}` — Go parity
/// with `msg_logging.go`). Returns ONLY the leading Rust-identifier characters of
/// the `{:?}` rendering — the enum variant name — and NEVER any payload field.
///
/// CARDINALITY GUARD (load-bearing): a derived-`Debug` enum renders as `Variant`
/// / `Variant(..)` / `Variant { .. }`, so the variant ident is always the leading
/// run of `[A-Za-z_][A-Za-z0-9_]*`; the first `(`, `{`, or space ends it. The
/// distinct label values are therefore bounded by the FINITE variant set, and an
/// attacker-controlled payload field (e.g. a `SetName(String)`'s string) can
/// never reach the label — which would otherwise be the classic Prometheus
/// cardinality memory-DoS (the registry never evicts; see `MetricKey.labels`).
///
/// A capped writer halts the `Debug` render after a small prefix, so a giant
/// payload field can't even force a full-`Debug` allocation on the hot dispatch
/// path. Result capped at 64 bytes; an empty extraction falls back to `"Msg"`.
/// Shared (not Web-specific) so Tui/WebView dispatch can record the same metric.
pub fn variant_name<M: std::fmt::Debug>(m: &M) -> String {
    use std::fmt::Write;
    // Sink accepting at most CAP bytes, then signalling "stop" via Err so
    // `write!` halts rendering — the variant ident is at the very front, so we
    // never materialise a large payload field.
    const CAP: usize = 80;
    struct Prefix {
        buf: String,
    }
    impl Write for Prefix {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for ch in s.chars() {
                if self.buf.len() + ch.len_utf8() > CAP {
                    return Err(std::fmt::Error); // halt the Debug render
                }
                self.buf.push(ch);
            }
            Ok(())
        }
    }
    let mut sink = Prefix { buf: String::new() };
    let _ = write!(sink, "{m:?}"); // ignore the deliberate halt error

    // Take the leading Rust identifier only.
    let mut name = String::new();
    for (idx, ch) in sink.buf.chars().enumerate() {
        let is_ident = if idx == 0 {
            ch.is_ascii_alphabetic() || ch == '_'
        } else {
            ch.is_ascii_alphanumeric() || ch == '_'
        };
        if !is_ident || name.len() >= 64 {
            break;
        }
        name.push(ch);
    }
    if name.is_empty() {
        "Msg".to_string()
    } else {
        name
    }
}

/// Format a float for Prometheus exposition (bucket `le` / `_sum`). Rust's `{}`
/// gives the canonical short form (`0.001`, `0.01`, `1`, `5`).
fn format_float(f: f64) -> String {
    format!("{f}")
}

/// Like `render_labels` but always appends an `le="<bound>"` label (histograms),
/// so the block is never empty.
fn render_labels_with_le(labels: &[(String, String)], le: &str) -> String {
    let mut pairs: Vec<String> = labels
        .iter()
        .map(|(k, v)| format!("{}=\"{}\"", k, escape_label_value(v)))
        .collect();
    pairs.push(format!("le=\"{}\"", escape_label_value(le)));
    format!("{{{}}}", pairs.join(","))
}

/// Prometheus `# TYPE` token from the stored value variant — single source of
/// truth, so the header can't contradict the emitted series body.
fn prom_type_token(v: &MetricValue) -> &'static str {
    match v {
        MetricValue::Counter(_) => "counter",
        MetricValue::Gauge(_) => "gauge",
        MetricValue::Histogram { .. } => "histogram",
    }
}

/// Per-metric HELP line for the exposition header. Unknown names get a generic
/// help line (still well-formed for scrapers). The TYPE header is derived
/// from the stored `MetricValue` variant via `prom_type_token`, so the two
/// can't contradict each other.
fn metric_help(name: &str) -> &'static str {
    match name {
        "ipe_web_requests_total" => "Total HTTP requests served, by method and status.",
        "ipe_web_sse_drops_total" => "SSE patches dropped due to a full per-session buffer.",
        "ipe_web_sse_connections_total" => "Total SSE connections opened.",
        "ipe_web_sessions_active" => "Currently-active Ipe.Web sessions.",
        "ipe_web_errors_total" => "Total responses with a 5xx status.",
        "ipe_web_request_seconds" => "HTTP request latency in seconds.",
        "ipe_web_msg_seconds" => "Msg-handling latency in seconds, by Msg variant name.",
        "ipe_web_msg_total" => "Total Msgs handled, by variant name, outcome, and noop.",
        _ => "Ipe runtime metric.",
    }
}

/// Escape a Prometheus label VALUE (`\`, `"`, newline) — spec 0.0.4.
fn escape_label_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

fn render_labels(labels: &[(String, String)]) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let inner: Vec<String> = labels
        .iter()
        .map(|(k, v)| format!("{}=\"{}\"", k, escape_label_value(v)))
        .collect();
    format!("{{{}}}", inner.join(","))
}

/// Render the registry as Prometheus text exposition (0.0.4). `# HELP`/`# TYPE`
/// are emitted once per metric name (`BTree` groups same-name series adjacently).
pub fn write_prom() -> String {
    use std::fmt::Write as _;
    let g = REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut out = String::new();
    let mut last_name: Option<&str> = None;
    for (key, val) in g.iter() {
        if last_name != Some(key.name.as_str()) {
            let _ = writeln!(out, "# HELP {} {}", key.name, metric_help(&key.name));
            let _ = writeln!(out, "# TYPE {} {}", key.name, prom_type_token(val));
            last_name = Some(key.name.as_str());
        }
        let labels = render_labels(&key.labels);
        match val {
            MetricValue::Counter(c) => {
                let _ = writeln!(out, "{}{} {}", key.name, labels, c);
            }
            MetricValue::Gauge(gv) => {
                let _ = writeln!(out, "{}{} {}", key.name, labels, gv);
            }
            MetricValue::Histogram {
                boundaries,
                buckets,
                sum,
                count,
            } => {
                // Cumulative _bucket lines, then +Inf, _sum, _count (Go's
                // writeHistogram). buckets[i] already holds the cumulative count.
                for (i, b) in boundaries.iter().enumerate() {
                    let c = buckets.get(i).copied().unwrap_or(0);
                    let _ = writeln!(
                        out,
                        "{}_bucket{} {}",
                        key.name,
                        render_labels_with_le(&key.labels, &format_float(*b)),
                        c
                    );
                }
                let _ = writeln!(
                    out,
                    "{}_bucket{} {}",
                    key.name,
                    render_labels_with_le(&key.labels, "+Inf"),
                    count
                );
                let _ = writeln!(out, "{}_sum{} {}", key.name, labels, format_float(*sum));
                let _ = writeln!(out, "{}_count{} {}", key.name, labels, count);
            }
        }
    }
    out
}

/// Most-recent `limit` log entries, oldest→newest.
pub fn recent_logs(limit: usize) -> Vec<LogEntry> {
    let g = LOGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let n = g.len();
    g.iter().skip(n.saturating_sub(limit)).cloned().collect()
}

/// Most-recent `limit` error entries, oldest→newest.
pub fn recent_errors(limit: usize) -> Vec<LogEntry> {
    let g = ERRORS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let n = g.len();
    g.iter().skip(n.saturating_sub(limit)).cloned().collect()
}

/// Minimal JSON string escaping for hand-built console payloads (avoids coupling
/// the always-compiled sink to serde).
#[must_use]
pub fn json_escape(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // cast is safe: we just checked c as u32 < 0x20
            #[allow(clippy::cast_sign_loss)]
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            // U+2028 LINE SEPARATOR / U+2029 PARAGRAPH SEPARATOR are valid JSON
            // but are JS line terminators — unescaped, they break this payload
            // when it is embedded in an inline <script> block (the console
            // bootstrap does exactly that). Escape them defensively.
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// Render a log-entry slice as a JSON array.
#[must_use]
pub fn entries_json(entries: &[LogEntry]) -> String {
    let items: Vec<String> = entries
        .iter()
        .map(|e| {
            format!(
                r#"{{"ts":{},"level":"{}","message":"{}"}}"#,
                e.ts_ms,
                json_escape(&e.level),
                json_escape(&e.message)
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escape_neutralises_js_line_terminators_and_controls() {
        // U+2028 / U+2029 are valid JSON but break an inline <script> JSON
        // payload (JS line terminators) — must be \u-escaped, not passed raw.
        assert_eq!(json_escape("a\u{2028}b"), "a\\u2028b");
        assert_eq!(json_escape("a\u{2029}b"), "a\\u2029b");
        // Quotes, backslashes, and C0 controls stay escaped.
        assert_eq!(json_escape("\"\\\n\t"), "\\\"\\\\\\n\\t");
        assert_eq!(json_escape("\u{0001}"), "\\u0001");
    }

    // Synthetic Debug types for `variant_name_extracts_only_the_bounded_variant_ident`.
    struct LongIdent;
    impl std::fmt::Debug for LongIdent {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}", "A".repeat(200))
        }
    }
    struct NonIdent;
    impl std::fmt::Debug for NonIdent {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "(weird")
        }
    }

    #[test]
    fn variant_name_extracts_only_the_bounded_variant_ident() {
        #[derive(Debug)]
        #[allow(dead_code)]
        enum M {
            Increment,
            Tick(i64),
            SetName(String),
            Login { user: String },
        }
        assert_eq!(variant_name(&M::Increment), "Increment");
        assert_eq!(variant_name(&M::Tick(42)), "Tick");
        // SECURITY (the load-bearing invariant): an attacker-controlled payload
        // field must NEVER reach the label — only the bounded variant ident.
        let evil = "x".repeat(5000) + "\n}{ injected control chars";
        assert_eq!(variant_name(&M::SetName(evil)), "SetName");
        assert_eq!(
            variant_name(&M::Login {
                user: "a".repeat(9000)
            }),
            "Login"
        );

        // A >64-byte leading ident truncates to 64 without leaking (synthetic
        // Debug — real Ipê variant idents are short; this proves the cap).
        let n = variant_name(&LongIdent);
        assert_eq!(n.len(), 64);
        assert!(n.chars().all(|c| c == 'A'));

        // A Debug rendering that doesn't start with an ident char → "Msg".
        assert_eq!(variant_name(&NonIdent), "Msg");
    }

    #[test]
    fn record_and_read_logs() {
        record_log("info", "hello");
        record_log("error", "boom \"x\"");
        let logs = recent_logs(10);
        assert!(logs.iter().any(|e| e.message == "hello"));
        let errs = recent_errors(10);
        assert!(errs.iter().any(|e| e.level == "error"));
        // error escaping is JSON-safe.
        assert!(entries_json(&errs).contains("boom \\\"x\\\""));
    }

    #[test]
    fn request_counters_move() {
        let before = requests_total();
        record_request(200);
        record_request(500);
        assert!(requests_total() >= before + 2);
    }

    #[test]
    fn spans_recorded_as_json() {
        record_span("db.query", 1234, true);
        record_span("http.get", 50, false);
        let j = spans_json(10);
        assert!(j.contains(r#""name":"db.query""#), "{j}");
        assert!(j.contains(r#""durUs":1234"#), "{j}");
        assert!(j.contains(r#""ok":false"#), "{j}");
    }

    #[test]
    fn dev_banner_byte_matches_go_dev_banner_markup() {
        // Go parity (dev_banner.go devBannerHTML): same id, target/rel/title,
        // monospace blue style, `&#128269;` ENTITY (not a literal emoji). Default
        // test env is dev (ENV/IPE_ENV unset) → non-empty banner.
        let b = dev_console_banner("");
        let expected = "<a id=\"__ipe-dev-console\" href=\"/_ipe/console\" target=\"_blank\" \
            rel=\"noopener\" title=\"Ipe Console (dev only)\" \
            style=\"position:fixed;right:12px;bottom:12px;z-index:2147483646;\
            font:12px/1.4 ui-monospace,Menlo,monospace;\
            background:#1c2027;color:#7eb6ff;\
            border:1px solid #353b46;border-radius:6px;\
            padding:6px 10px;text-decoration:none;\
            box-shadow:0 2px 8px rgba(0,0,0,0.4);\">\
            &#128269; Console</a>";
        assert_eq!(b, expected, "dev console banner must byte-match Go");
        assert!(
            !b.contains('🔍'),
            "must use the &#128269; entity, not a literal emoji"
        );
    }

    #[test]
    fn dev_banner_suppressed_for_subapp() {
        // A non-empty base = sub-app (e.g. the console child) → no recursive link.
        // Base-gate needs no env mutation, so this stays race-free.
        assert_eq!(dev_console_banner("/_ipe/console"), "");
    }

    #[test]
    fn inject_dev_banner_before_last_body_close() {
        let body = "<html><body><p>hi</p></body></html>";
        let out = inject_dev_banner(body, "<BANNER>");
        assert_eq!(out, "<html><body><p>hi</p><BANNER></body></html>");
    }

    #[test]
    fn inject_dev_banner_case_insensitive_body_tag() {
        // Go lower-cases the body before LastIndex("</body>").
        let body = "<HTML><BODY>x</BODY></HTML>";
        let out = inject_dev_banner(body, "<B>");
        assert_eq!(out, "<HTML><BODY>x<B></BODY></HTML>");
    }

    #[test]
    fn inject_dev_banner_uses_last_body_close() {
        let body = "</body>first</body>";
        let out = inject_dev_banner(body, "<B>");
        assert_eq!(out, "</body>first<B></body>");
    }

    #[test]
    fn inject_dev_banner_appends_when_no_body_tag() {
        let body = "<p>fragment only</p>";
        let out = inject_dev_banner(body, "<B>");
        assert_eq!(out, "<p>fragment only</p><B>");
    }

    #[test]
    fn inject_dev_banner_empty_is_noop() {
        // Empty banner is the observable effect of the production-suppressed path:
        // dev_console_banner returns "" in production, so injection must no-op.
        let body = "<html><body>x</body></html>";
        assert_eq!(inject_dev_banner(body, ""), body);
    }

    #[test]
    fn inject_dev_banner_utf8_body_char_boundary_safe() {
        // Multi-byte UTF-8 before the </body> must not panic on the slice.
        let body = "<body>café — 日本語</body>";
        let out = inject_dev_banner(body, "<B>");
        assert_eq!(out, "<body>café — 日本語<B></body>");
    }
}

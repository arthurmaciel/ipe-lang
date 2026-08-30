//! Ipe Console — the operator dashboard mounted at `/_ipe/console`, plus the
//! observability federation receiver. Implements `console*.go` in-RAM tier:
//! a plain-HTML shell that polls JSON `/_ipe/console/api/*` endpoints backed by
//! the `telemetry` ring buffers, and a `/_ipe/observability/ingest` POST that
//! folds a sub-app's batched logs into the same rings.
//!
//! Unlike the separate-process console path, this
//! proxies it), the Rust console is served in-process directly off the Web
//! router — no extra process, same data. No panic vectors.

use crate::telemetry;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;

const fn json_ct() -> (header::HeaderName, &'static str) {
    (header::CONTENT_TYPE, "application/json")
}

/// Boot-time decision: should the console be mounted at all?
/// `false` → the caller skips the console entirely (in-process or proxy).
///
/// Conditions that suppress the console mount:
/// - sub-app context: the parent owns its own console; a nested app must not
///   recursively mount one (`IPE_WEB_BASE_PATH` / deprecated `IPE_LIVE_BASE_PATH`);
/// - explicit opt-out via `IPE_CONSOLE_EMBED=off|0|false`;
/// - `IPE_CONSOLE_AUTH=off` (operator declared surface absent);
/// - production without an admin token (fail-closed — no silent open-to-world mount).
///
/// This function is reqwest-free; it lives here so the mount decision is
/// available regardless of whether `http_client` is compiled in.
pub fn gate_allows() -> bool {
    if crate::system::read_env_var_renamed("IPE_WEB_BASE_PATH", "IPE_LIVE_BASE_PATH")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    if matches!(
        crate::system::read_env_var("IPE_CONSOLE_EMBED").as_deref(),
        Ok("off") | Ok("0") | Ok("false")
    ) {
        return false;
    }
    if crate::system::read_env_var("IPE_CONSOLE_AUTH")
        .map(|v| v.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(false)
    {
        return false;
    }
    if super::super::telemetry::production_from_env()
        && crate::system::read_env_var("IPE_ADMIN_TOKEN")
            .map(|v| v.is_empty())
            .unwrap_or(true)
        && crate::system::read_env_var("IPE_CONSOLE_TOKEN")
            .map(|v| v.is_empty())
            .unwrap_or(true)
    {
        return false;
    }
    true
}

/// `GET /_ipe/console` — the plain-HTML dashboard shell (no framework, no CSS
/// deps). Polls the api endpoints below.
pub async fn console_html() -> impl IntoResponse {
    let body = r#"<!doctype html><html><head><meta charset="utf-8">
<title>Ipe Console</title>
<style>
 body{font-family:ui-monospace,monospace;background:#12141c;color:#dfe3ee;margin:0;padding:16px}
 h1{font-size:16px;color:#8ec8a8} .tab{cursor:pointer;padding:4px 10px;margin-right:6px;border:1px solid #2a2f40;border-radius:4px;display:inline-block}
 .tab.on{background:#2a2f40} pre{background:#0c0e14;padding:10px;border-radius:4px;overflow:auto;max-height:70vh}
 .err{color:#e88} .lvl{color:#7a86a8}
</style></head><body>
<h1>Ipe Console</h1>
<div id="ov"></div>
<div><span class="tab on" data-t="logs">Logs</span><span class="tab" data-t="errors">Errors</span></div>
<pre id="out">loading…</pre>
<script>
 let tab="logs";
 async function j(u){try{const r=await fetch(u);return await r.json()}catch(e){return null}}
 async function ov(){const o=await j("/_ipe/console/api/overview");if(o)document.getElementById("ov").textContent=
   "requests="+o.requests+"  errors="+o.errors;}
 function esc(s){return String(s).replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;").replace(/"/g,"&quot;").replace(/'/g,"&#39;");}
 function fmt(es){return (es||[]).map(e=>{const d=new Date(e.ts).toISOString().slice(11,19);
   return "<span class='lvl'>"+esc(d)+" "+esc(e.level)+"</span> "+(e.level=="error"?"<span class='err'>":"")+
   esc(e.message)+(e.level=="error"?"</span>":"");}).join("\n");}
 async function refresh(){const es=await j("/_ipe/console/api/"+tab);
   document.getElementById("out").innerHTML=fmt(es);ov();}
 document.querySelectorAll(".tab").forEach(t=>t.onclick=()=>{
   document.querySelectorAll(".tab").forEach(x=>x.classList.remove("on"));t.classList.add("on");
   tab=t.dataset.t;refresh();});
 refresh();setInterval(refresh,2000);
</script></body></html>"#;
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
}

/// `GET /_ipe/console/api/overview` — request + error counters.
pub async fn api_overview() -> impl IntoResponse {
    let body = format!(
        r#"{{"requests":{},"errors":{}}}"#,
        telemetry::requests_total(),
        telemetry::errors_total()
    );
    (StatusCode::OK, [json_ct()], body)
}

/// `GET /_ipe/console/api/logs` — recent log ring (most recent 200).
pub async fn api_logs() -> impl IntoResponse {
    (
        StatusCode::OK,
        [json_ct()],
        telemetry::entries_json(&telemetry::recent_logs(200)),
    )
}

/// `GET /_ipe/console/api/errors` — recent error ring.
pub async fn api_errors() -> impl IntoResponse {
    (
        StatusCode::OK,
        [json_ct()],
        telemetry::entries_json(&telemetry::recent_errors(200)),
    )
}

/// `GET /_ipe/console/api/traces` — recent completed `Ipe.Trace.span`s.
pub async fn api_traces() -> impl IntoResponse {
    (StatusCode::OK, [json_ct()], telemetry::spans_json(200))
}

/// `GET /_ipe/console/api/metrics-summary` — the parsed counter snapshot the
/// dashboard renders (mirror of  parsed Prometheus summary).
pub async fn api_metrics_summary() -> impl IntoResponse {
    let body = format!(
        r#"{{"ipe_web_requests_total":{},"ipe_web_errors_total":{}}}"#,
        telemetry::requests_total(),
        telemetry::errors_total()
    );
    (StatusCode::OK, [json_ct()], body)
}

/// Production auth gate for the console + metrics surface
/// (`productionFromEnv` + `IPE_CONSOLE_AUTH`). Returns `Some(response)` when the
/// request must be REFUSED. `IPE_CONSOLE_AUTH=off` → 404 (surface declared absent).
/// In production (ENV/IPE_ENV non-dev) a `Bearer` admin token is required
/// (`IPE_ADMIN_TOKEN`, legacy `IPE_CONSOLE_TOKEN`) — 401 otherwise. Dev mode (the
/// default) is open and returns `None`.
/// Does an `Authorization` header value authorize the admin surface? Accepts
/// either `Bearer <tok>` OR `Basic base64(user:tok)` (`hasAdminAuth`
/// honours both, the latter being the Prometheus `basic_auth` scrape path —
/// any username, the password segment is the admin token). Both comparisons are
/// constant-time (subtle::ct_eq). Total: every fallible step is Option/Result.
fn header_authorizes(auth: &str, tok: &str) -> bool {
    use subtle::ConstantTimeEq;
    let bearer = format!("Bearer {tok}");
    if bool::from(auth.as_bytes().ct_eq(bearer.as_bytes())) {
        return true;
    }
    if let Some(b64) = auth.strip_prefix("Basic ") {
        use base64::{Engine, engine::general_purpose::STANDARD as B64};
        if let Ok(raw) = B64.decode(b64.trim())
            && let Ok(creds) = std::str::from_utf8(&raw)
            && let Some((_user, pw)) = creds.split_once(':')
        {
            return bool::from(pw.as_bytes().ct_eq(tok.as_bytes()));
        }
    }
    false
}

/// The parsed console-auth mode — the single authoritative representation of
/// `IPE_CONSOLE_AUTH` (trim + lowercase; unknown → `Off`, no silent widen).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ConsoleAuthMode {
    Off,
    Token,
    App,
    UnsetProd,
    DevOpen,
}
impl ConsoleAuthMode {
    fn label(self) -> &'static str {
        match self {
            ConsoleAuthMode::Off => "off",
            ConsoleAuthMode::Token => "token",
            ConsoleAuthMode::App => "app",
            ConsoleAuthMode::UnsetProd => "unset-prod",
            ConsoleAuthMode::DevOpen => "dev-open",
        }
    }
}
/// The ONE parse of `IPE_CONSOLE_AUTH` (trim + lowercase; unknown → `Off`, no
/// silent widen). Unset → `DevOpen` in dev / `UnsetProd` in production.
fn resolve_console_auth_mode() -> ConsoleAuthMode {
    let raw = crate::system::read_env_var("IPE_CONSOLE_AUTH").unwrap_or_default();
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" => ConsoleAuthMode::Off,
        "token" => ConsoleAuthMode::Token,
        "app" => ConsoleAuthMode::App,
        "" => {
            if telemetry::production_from_env() {
                ConsoleAuthMode::UnsetProd
            } else {
                ConsoleAuthMode::DevOpen
            }
        }
        _ => ConsoleAuthMode::Off,
    }
}

/// The console-auth mode label, mirroring  `describeConsoleAuthMode`
/// (`console_auth_v2.go:149`) over `resolveConsoleAuthMode`'s env/production
/// derivation. Used for the `[ipe.console] inline console mounted … mode=<m>`
/// startup log . Total: every branch is explicit.
///
/// Derivation : `IPE_CONSOLE_AUTH` (case-insensitive, trimmed) selects
/// `off`/`token`/`app`; unset → `dev-open` in dev (the default) or `unset-prod`
/// in production (`ENV`/`IPE_ENV` non-dev); any unknown value → `off` (refuses
/// to silently widen to something more permissive).
pub fn console_auth_mode_label() -> &'static str {
    resolve_console_auth_mode().label()
}

pub fn gate_blocked(headers: &axum::http::HeaderMap) -> Option<axum::response::Response> {
    // Resolve through the SAME normalizer as console_auth_mode_label (trim +
    // lowercase + unknown→off). One resolver, one behaviour — the exhaustive
    // enum match makes the compiler verify every variant is handled.
    match resolve_console_auth_mode() {
        ConsoleAuthMode::Off => {
            return Some((StatusCode::NOT_FOUND, "console disabled").into_response());
        }
        // `IPE_CONSOLE_AUTH=app` (row-poly `consoleAuth` callback) is not yet
        // implemented in the Rust runtime. Fail closed with a clear 501 rather than
        // a misleading 401 that suggests a bad token would fix it.
        ConsoleAuthMode::App => {
            return Some(
                (
                    StatusCode::NOT_IMPLEMENTED,
                    "IPE_CONSOLE_AUTH=app (row-poly consoleAuth callback) is not yet \
                     supported on the Rust runtime; use token/off or IPE_ADMIN_TOKEN",
                )
                    .into_response(),
            );
        }
        // Token / UnsetProd / DevOpen fall through to the prod/token gate below.
        ConsoleAuthMode::Token | ConsoleAuthMode::UnsetProd | ConsoleAuthMode::DevOpen => {}
    }
    if !telemetry::production_from_env() {
        return None;
    }
    // Admin-token source precedence     // aliases IPE_CONSOLE_TOKEN and IPE_METRICS_TOKEN (IPE_METRICS_TOKEN
    // as a back-compat alias — without it a prod operator who only set the legacy
    // var is locked out / forced to a weaker config).
    let want = crate::system::read_env_var("IPE_ADMIN_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
        // In-code `Console.adminToken` (a sealed `Secret`) sits below the env
        // override — env always wins the one precedence, and the token is only
        // revealed here, at the auth check, never logged.
        .or_else(|| {
            crate::app_config::resolve_console_token(crate::app_config::ConsoleTokenKind::Admin)
                .filter(|t| !t.is_empty())
        })
        .or_else(|| {
            crate::system::read_env_var("IPE_CONSOLE_TOKEN")
                .ok()
                .filter(|t| !t.is_empty())
        })
        .or_else(|| {
            crate::system::read_env_var("IPE_METRICS_TOKEN")
                .ok()
                .filter(|t| !t.is_empty())
        })
        // In-code `Console.metricsToken` — the metrics-scrape alias, below its
        // env sibling in the one precedence.
        .or_else(|| {
            crate::app_config::resolve_console_token(crate::app_config::ConsoleTokenKind::Metrics)
                .filter(|t| !t.is_empty())
        });
    let authed = match (want, headers.get(header::AUTHORIZATION)) {
        (Some(tok), Some(h)) => h
            .to_str()
            .map(|h| header_authorizes(h, &tok))
            .unwrap_or(false),
        _ => false,
    };
    if authed {
        None
    } else {
        // Audit the denial (`console.auth.denied` warn into the
        // telemetry ring) so an operator sees brute-force / probing attempts.
        telemetry::record_log(
            "warn",
            "console.auth.denied reason=bad-or-missing-credentials",
        );
        // WWW-Authenticate so a Prometheus `basic_auth` scraper (:
        // HandleMetrics realm "ipe-metrics") gets a proper challenge instead of a
        // bare 401 it can't act on.
        Some(
            (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Basic realm=\"ipe-metrics\"")],
                "console requires a Bearer or Basic admin token in production",
            )
                .into_response(),
        )
    }
}

/// `POST /_ipe/observability/ingest` — federation receiver. Accepts a JSON array
/// of `{ "level": "...", "message": "..." }` (a sub-app's batched logs) and folds
/// them into the local rings. Malformed bodies are accepted as 204 (drop) rather
/// than erroring — telemetry must never break the caller.
///
/// Auth : a shared secret in `X-Ipê-Ingest-Token`, constant-time
/// compared against `IPE_INGEST_TOKEN`. The Rust runtime does not yet spawn
/// sub-apps (no auto-generated token to distribute), so the gate is enforced
/// ONLY when an operator sets `IPE_INGEST_TOKEN` — unset leaves the endpoint open
/// (dev / single-process). When federation lands the parent will generate + pass
/// the token; the check side is already here.
pub async fn ingest(headers: axum::http::HeaderMap, body: String) -> axum::response::Response {
    if let Some(resp) = ingest_token_blocked(&headers) {
        return resp;
    }
    // Two accepted shapes: a bare array of `{level, message}` (legacy), or the
    // federation push object `{ "logs": [...], "spans": [...] }` (from
    // push_exporter::build_payload). Fold both into the local rings.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
        match v {
            serde_json::Value::Array(items) => {
                for it in items {
                    fold_log(&it);
                }
            }
            serde_json::Value::Object(_) => {
                // Iterate the arrays by reference — no `.cloned()` of the whole
                // log/span batch (fold_log + the span reader both take `&Value`).
                if let Some(serde_json::Value::Array(logs)) = v.get("logs") {
                    for it in logs {
                        fold_log(it);
                    }
                }
                if let Some(serde_json::Value::Array(spans)) = v.get("spans") {
                    for it in spans {
                        let name = it.get("name").and_then(|x| x.as_str()).unwrap_or("");
                        let dur_us = it.get("durUs").and_then(|x| x.as_u64()).unwrap_or(0);
                        let ok = it.get("ok").and_then(|x| x.as_bool()).unwrap_or(true);
                        if !name.is_empty() {
                            // Sanitise the untrusted span name (same terminal-escape
                            // injection vector as fold_log's message).
                            telemetry::record_span(&sanitise_ingest(name), dur_us, ok);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Strip control / escape bytes and cap the length of UNTRUSTED ingest text
/// before it enters the operator console rings (which render to a terminal AND
/// re-export over OTLP). A malicious or compromised sub-app could otherwise
/// inject ANSI/CSI/OSC escapes, NUL, or newlines — forged log lines,
/// clear-screen, cursor moves, terminal-title rewrites — into the operator's
/// terminal. Mirrors the discipline `observability::track` already applies to
/// the request path via `sanitise_path`; first-party `Log.*` does NOT route
/// through ingest, so it is unaffected.
fn sanitise_ingest(s: &str) -> String {
    // Reject control bytes AND Unicode bidi-override / zero-width / format chars
    // (U+200B-200F, U+202A-202E, U+2066-2069, U+FEFF) — a right-to-left override
    // or zero-width joiner in an untrusted log message can still spoof / reorder
    // a line in the operator's terminal even though it's not an ASCII control.
    s.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c,
                    '\u{200B}'..='\u{200F}'
                        | '\u{202A}'..='\u{202E}'
                        | '\u{2066}'..='\u{2069}'
                        | '\u{FEFF}')
        })
        .take(2048)
        .collect()
}

/// Fold one ingested log object `{level, message}` into the local rings.
fn fold_log(it: &serde_json::Value) {
    let level = it.get("level").and_then(|v| v.as_str()).unwrap_or("info");
    let message = it.get("message").and_then(|v| v.as_str()).unwrap_or("");
    if !message.is_empty() {
        telemetry::record_log(&sanitise_ingest(level), &sanitise_ingest(message));
    }
}

/// True when `Origin` is present AND does not match `Host` — i.e. this is a
/// cross-origin request. Absent `Origin` (same-origin fetch/XHR, curl,
/// server-to-server pushes from `push_exporter.rs`) is NOT flagged: those
/// callers never send a hostile cross-origin request by construction, and
/// requiring `Origin` would break legitimate non-browser ingest pushes.
/// Mirrors `csrf.rs::origin_mismatch`'s same-origin comparison (via the
/// shared `origin_host_mismatch` helper, also used by
/// `server.rs::ws_cross_origin` — normalizes away each side's scheme-implied
/// default port so the three never drift to different behavior), applied
/// unconditionally here (not opt-in) since it's the ONLY defense available
/// when `IPE_INGEST_TOKEN` is unset.
fn is_cross_origin_ingest(headers: &axum::http::HeaderMap) -> bool {
    let origin = match headers
        .get(axum::http::header::ORIGIN)
        .and_then(|h| h.to_str().ok())
    {
        Some(o) => o,
        None => return false,
    };
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    crate::http_header::origin_host_mismatch(origin, host)
}

/// `Some(401)` when `IPE_INGEST_TOKEN` is set and the `X-Ipê-Ingest-Token` header
/// is absent or wrong (constant-time compare). Unset → open EXCEPT for a
/// cross-origin browser POST (log-injection CSRF shape — see
/// `is_cross_origin_ingest`), which is rejected even in dev.
fn ingest_token_blocked(headers: &axum::http::HeaderMap) -> Option<axum::response::Response> {
    use subtle::ConstantTimeEq;
    let want = match crate::system::read_env_var("IPE_INGEST_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
        // In-code `Console.ingestToken` (a sealed `Secret`) below the env
        // override — env wins; the token is revealed only here, never logged.
        .or_else(|| {
            crate::app_config::resolve_console_token(crate::app_config::ConsoleTokenKind::Ingest)
                .filter(|t| !t.is_empty())
        }) {
        Some(t) => t,
        None => {
            // Unset token: open in dev (single-process / no federation), but in
            // production fail CLOSED — an unauthenticated ingest endpoint folds
            // attacker-supplied telemetry straight into the operator console
            // (log-injection). Matches the console mount's own production gate.
            if telemetry::production_from_env() {
                return Some(
                    (
                        StatusCode::UNAUTHORIZED,
                        "observability ingest requires IPE_INGEST_TOKEN in production",
                    )
                        .into_response(),
                );
            }
            // Dev + no token configured: the ONLY remaining defense is
            // same-origin. A same-origin fetch/XHR, curl, or a same-process
            // push (no Origin header) is allowed; a cross-origin browser POST
            // (the CSRF-log-injection shape — a `POST` with
            // `Content-Type: text/plain` and no custom header is a CORS
            // "simple request", so a malicious cross-origin page can fire it
            // without a preflight) is rejected.
            if is_cross_origin_ingest(headers) {
                return Some(
                    (
                        StatusCode::FORBIDDEN,
                        "observability ingest: cross-origin request rejected (set IPE_INGEST_TOKEN to allow federated pushes)",
                    )
                        .into_response(),
                );
            }
            return None;
        }
    };
    let got = headers
        .get("x-ipe-ingest-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if bool::from(got.as_bytes().ct_eq(want.as_bytes())) {
        None
    } else {
        Some(
            (
                StatusCode::UNAUTHORIZED,
                "invalid or missing X-Ipe-Ingest-Token",
            )
                .into_response(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_skips_in_subapp_context() {
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("IPE_WEB_BASE_PATH", "/billing") };
        assert!(!gate_allows());
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_WEB_BASE_PATH") };
    }

    #[test]
    fn gate_skips_on_explicit_off() {
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("IPE_CONSOLE_EMBED", "off") };
        assert!(!gate_allows());
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_CONSOLE_EMBED") };
    }

    // Pure (no env dependency) — safe as its own test, no race with
    // ingest_token_gate's IPE_INGEST_TOKEN mutation below.
    #[test]
    fn is_cross_origin_ingest_detection() {
        let mk = |origin: Option<&str>, host: &str| {
            let mut h = axum::http::HeaderMap::new();
            if let Some(o) = origin {
                h.insert("origin", o.parse().unwrap());
            }
            h.insert("host", host.parse().unwrap());
            h
        };
        assert!(is_cross_origin_ingest(&mk(
            Some("https://evil.example"),
            "victim.example"
        )));
        assert!(!is_cross_origin_ingest(&mk(
            Some("https://victim.example"),
            "victim.example"
        )));
        // No Origin header at all → not flagged (curl / server-to-server push).
        assert!(!is_cross_origin_ingest(&mk(None, "victim.example")));
        // An implicit-default-port Origin against an explicit-default-port
        // Host is the SAME origin, not a mismatch.
        assert!(!is_cross_origin_ingest(&mk(
            Some("https://victim.example"),
            "victim.example:443"
        )));
    }

    // One test (not split) — IPE_INGEST_TOKEN is process-global env, so a split
    // would race other threads. Sets then clears the var within the test.
    #[test]
    fn ingest_token_gate() {
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_INGEST_TOKEN") };
        // Unset → endpoint open regardless of header, when same-origin (or no
        // Origin at all — curl / non-browser caller).
        let h = axum::http::HeaderMap::new();
        assert!(ingest_token_blocked(&h).is_none(), "open when unset");

        // Unset token + cross-origin browser POST → rejected (the CSRF-log-
        // injection shape: no token configured, so same-origin is the only
        // remaining defense).
        let mut h = axum::http::HeaderMap::new();
        h.insert("origin", "https://evil.example".parse().unwrap());
        h.insert("host", "victim.example".parse().unwrap());
        assert!(
            ingest_token_blocked(&h).is_some(),
            "cross-origin POST with no token configured must be rejected"
        );

        // Unset token + same-origin Origin header → still open.
        let mut h = axum::http::HeaderMap::new();
        h.insert("origin", "https://victim.example".parse().unwrap());
        h.insert("host", "victim.example".parse().unwrap());
        assert!(
            ingest_token_blocked(&h).is_none(),
            "same-origin request still open in dev"
        );

        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("IPE_INGEST_TOKEN", "secret123") };
        // Missing header → blocked.
        let h = axum::http::HeaderMap::new();
        assert!(ingest_token_blocked(&h).is_some(), "missing header blocked");
        // Wrong token → blocked.
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-ipe-ingest-token", "wrong".parse().unwrap());
        assert!(ingest_token_blocked(&h).is_some(), "wrong token blocked");
        // Correct token → allowed, even cross-origin (bearer-token auth makes
        // the same-origin check redundant once a real token is configured).
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-ipe-ingest-token", "secret123".parse().unwrap());
        h.insert("origin", "https://evil.example".parse().unwrap());
        h.insert("host", "victim.example".parse().unwrap());
        assert!(
            ingest_token_blocked(&h).is_none(),
            "correct token allowed even cross-origin"
        );

        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_INGEST_TOKEN") };
    }
}

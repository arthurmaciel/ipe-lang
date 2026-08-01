//! Ipe.Web CSRF protection + security response headers.
//!
//! Mirror of Go's `csrf_middleware.go` (double-submit cookie) + `setSecurityHeaders`
//! (live.go), with a few hardening additions over the Go oracle:
//!   - the `__ipe_csrf` cookie is `SameSite=Strict` + `Secure` (in production /
//!     frame-ancestors mode) — SameSite=Strict is itself a strong CSRF defense,
//!     the double-submit token is belt-and-suspenders;
//!   - an OPT-IN `Origin`/`Host` same-origin check (`IPE_LIVE_CSRF_ORIGIN_CHECK=on`)
//!     for same-origin deployments that want a third layer (off by default so it
//!     can't break reverse-proxied setups where the proxy rewrites `Host`);
//!   - `X-Content-Type-Options: nosniff` + a restrictive `Permissions-Policy`
//!     beyond Go's header set.
//!
//! The Ipe.Web client POSTs JSON to `/_ipe/event` with an `X-Ipe-Csrf` header
//! (never a form body), so the middleware validates header-vs-cookie WITHOUT
//! reading the request body — no buffering, no body-consumption hazard.

use crate::telemetry;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

/// The double-submit cookie name. Hardening BEYOND Go: when cookies are Secure
/// (production / TLS / frame-ancestors), use the `__Host-` prefix —the browser
/// then refuses any `Set-Cookie` carrying a `Domain=` attribute, which blocks
/// the sibling-subdomain cookie-fixation vector (an attacker on
/// `evil.example.com` with a valid cert can otherwise plant `__ipe_csrf` for
/// `example.com`). `__Host-` MANDATES Secure+Path=/+no-Domain, so it can't be
/// used over plain-HTTP dev — there we fall back to the bare name (SameSite=Strict
/// is still the primary guard, and HTTPS-subdomain injection is impossible without
/// TLS anyway). Read AND write must agree, so both route through this.
pub fn csrf_cookie_name() -> &'static str {
    if cookies_secure() {
        "__Host-ipe_csrf"
    } else {
        "__ipe_csrf"
    }
}
/// The header the client echoes the token in (Go parity: `X-Ipê-Csrf`).
pub const CSRF_HEADER: &str = "x-ipe-csrf";

/// CSRF protection is ON by default; `IPE_CSRF=off|0|false` disables it
/// (Go parity: the `IPE_CSRF` env switch / ipe.toml `[security] csrf`).
///
/// Snapshotted once into a `OnceLock` on first call (env is stable at process
/// start; same rationale as `cookies_secure()` — eliminates a per-request
/// `getenv` + global env-lock acquisition on every mutating request).
pub fn csrf_enabled() -> bool {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        !matches!(
            crate::system::read_env_var("IPE_CSRF").ok().as_deref(),
            Some("off") | Some("0") | Some("false")
        )
    })
}

// `frame_ancestors` + `security_headers` were relocated to the always-compiled
// `telemetry` module so the Ipe.Http.Server path can share them (the `live`
// module is DCE'd out of server-only builds). Re-exported here so existing
// `csrf::frame_ancestors` / `csrf::security_headers` call sites keep resolving.
pub use crate::telemetry::{frame_ancestors, security_headers};

/// Whether to mark cookies `Secure`. Production (or frame-ancestors mode, which
/// is always HTTPS) → Secure. Mirrors Go's `r.TLS != nil || X-Forwarded-Proto`.
///
/// Snapshotted once into a `OnceLock` on first call (env is stable at process
/// start; eliminates per-request `getenv` + the TOCTOU race between
/// `cookies_secure()` deciding on `__Host-` and `csrf_set_cookie()` writing it).
pub fn cookies_secure() -> bool {
    use std::sync::OnceLock;
    static SECURE: OnceLock<bool> = OnceLock::new();
    *SECURE.get_or_init(|| telemetry::production_from_env() || frame_ancestors().is_some())
}

/// ~244 random bits (two concatenated UUIDv4s) as 64 lowercase-hex chars —
/// comfortably above the 128-bit CSRF-token floor, same shape as the prior
/// 32-byte scheme. `uuid::Uuid::new_v4` draws from the OS CSPRNG (the approved
/// security-randomness source per `random.rs`, mirroring `server.rs`'s
/// `csrf_gen_token`), so the token needs no `aes-gcm` dependency. Never panics.
pub fn gen_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// A token "looks valid" if it's the expected 64 lowercase-hex shape — used to
/// decide whether to reuse the browser's existing cookie token vs mint a fresh
/// one (a malformed/forged cookie value is replaced, never trusted).
pub fn token_is_well_formed(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Read a named cookie value from the `Cookie:` header (generic; the session
/// cookie has its own base-path-aware reader in `mod.rs`).
///
/// Uses `split_once('=')` and compares the key exactly (after trim) so a cookie
/// named `ipe_csrf` never accidentally matches `__Host-ipe_csrf` or vice-versa
/// (the old `strip_prefix` shape would match any name that is a prefix of the
/// cookie key).
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=')
            && k.trim() == name
        {
            return Some(v.to_string());
        }
    }
    None
}

/// Build the `Set-Cookie` value for the CSRF cookie. `HttpOnly` (the client
/// reads the token from the injected page JS, NOT from the cookie, so HttpOnly
/// is safe and blocks token theft via XSS). `SameSite=Strict` normally;
/// `SameSite=None; Secure` in frame-ancestors mode.
pub fn csrf_set_cookie(token: &str) -> String {
    let name = csrf_cookie_name();
    if frame_ancestors().is_some() {
        // Cross-site iframe: the cookie must cross sites → None+Secure (Secure is
        // mandatory for SameSite=None). `__Host-` is compatible (it only forbids
        // Domain=, not SameSite=None).
        format!("{name}={token}; Path=/; HttpOnly; SameSite=None; Secure")
    } else if cookies_secure() {
        // Production / TLS: `__Host-` name → Secure is mandatory.
        format!("{name}={token}; Path=/; HttpOnly; SameSite=Strict; Secure")
    } else {
        // Plain-HTTP dev: bare name, no Secure (Secure would drop the cookie on http://).
        format!("{name}={token}; Path=/; HttpOnly; SameSite=Strict")
    }
}

/// Paths exempt from CSRF validation (Go parity: `isObservabilityPath` + the
/// console prefix + SSE). GET/HEAD/OPTIONS are exempt by method, separately.
pub fn is_exempt_path(path: &str) -> bool {
    matches!(
        path,
        "/_ipe/healthz"
            | "/_ipe/readyz"
            | "/_ipe/metrics"
            | "/_ipe/buildinfo"
            | "/_ipe/sse"
            | "/_ipe/observability/ingest"
    ) || path == "/_ipe/console"
        || path.starts_with("/_ipe/console/")
}

/// Optional same-origin `Origin`/`Host` check (opt-in via
/// `IPE_LIVE_CSRF_ORIGIN_CHECK=on`; off by default so a reverse proxy that
/// rewrites `Host` can't break legitimate POSTs). Skipped entirely in
/// frame-ancestors mode (cross-origin embedding is intentional there). Returns
/// `true` when the request should be REJECTED.
fn origin_mismatch(headers: &HeaderMap) -> bool {
    // Snapshotted once (env is stable at process start; same rationale as
    // `cookies_secure()` — no per-request global env-lock acquisition).
    fn origin_check_enabled() -> bool {
        use std::sync::OnceLock;
        static CHECK: OnceLock<bool> = OnceLock::new();
        *CHECK.get_or_init(|| {
            crate::system::read_env_var("IPE_LIVE_CSRF_ORIGIN_CHECK").as_deref() == Ok("on")
        })
    }
    if !origin_check_enabled() {
        return false;
    }
    if frame_ancestors().is_some() {
        return false;
    }
    let origin = match headers
        .get(axum::http::header::ORIGIN)
        .and_then(|h| h.to_str().ok())
    {
        Some(o) => o,
        None => return false, // no Origin (e.g. same-origin GET-turned-POST) — don't reject
    };
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    // Compare the Origin's host[:port] to the request Host, normalizing away
    // each side's scheme-implied default port — shared with
    // `console.rs::is_cross_origin_ingest` / `server.rs::ws_cross_origin` so
    // the three never drift to different normalization behavior (see
    // `origin_host_mismatch`'s doc comment).
    crate::http_header::origin_host_mismatch(origin, host)
}

/// The axum middleware. Validates CSRF on mutating, non-exempt requests; passes
/// everything else through. Reads only headers (the Ipe.Web POST body is JSON
/// with the token in `X-Ipê-Csrf`, so no body buffering is needed).
pub async fn csrf_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    use subtle::ConstantTimeEq;

    if !csrf_enabled() {
        return next.run(req).await;
    }
    let method = req.method().clone();
    let mutating = matches!(
        method,
        axum::http::Method::POST
            | axum::http::Method::PUT
            | axum::http::Method::DELETE
            | axum::http::Method::PATCH
    );
    let path = req.uri().path().to_string();
    if !mutating || is_exempt_path(&path) {
        return next.run(req).await;
    }

    let headers = req.headers();
    if origin_mismatch(headers) {
        telemetry::record_log("warn", "csrf.rejected reason=origin-mismatch");
        return (StatusCode::FORBIDDEN, "{\"status\":\"csrf_origin\"}").into_response();
    }

    let cookie_tok = cookie_value(headers, csrf_cookie_name()).unwrap_or_default();
    let header_tok = headers
        .get(CSRF_HEADER)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if cookie_tok.is_empty() || header_tok.is_empty() {
        telemetry::record_log("warn", "csrf.rejected reason=missing");
        return (StatusCode::FORBIDDEN, "{\"status\":\"csrf_missing\"}").into_response();
    }
    // Constant-time equality of the double-submit pair.
    if !bool::from(cookie_tok.as_bytes().ct_eq(header_tok.as_bytes())) {
        telemetry::record_log("warn", "csrf.rejected reason=mismatch");
        return (StatusCode::FORBIDDEN, "{\"status\":\"csrf_invalid\"}").into_response();
    }
    next.run(req).await
}

// `security_headers` now lives in `telemetry` (re-exported at the top of this
// module) so the Ipe.Http.Server path can share it.

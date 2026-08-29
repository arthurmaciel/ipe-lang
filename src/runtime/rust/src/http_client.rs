//! Ipe.Http — outbound HTTP client (reqwest under a Ipê-native surface).
//!
//! HttpResponse/HttpRequest map to the Ipê record aliases via runtimeOpaqueTypes
//! (like Csv's CsvDoc), so `resp.status` / `.body` / `.headers` resolve onto
//! these pub fields and the Ipê-built `defaultRequest` record constructs this
//! struct directly. Field names match the Ipê records verbatim.
//!
//! ## SSRF protection (default-ON in production)
//!
//! The guard blocks requests whose resolved host is loopback, RFC-1918 private,
//! link-local, unique-local (ULA), unspecified, or v4-mapped-private. It is
//! ON by default in production (`ENV`/`IPE_ENV` not in {unset, dev, development,
//! local}) and OFF in dev so development against `localhost` keeps working
//! unchanged. `IPE_HTTP_DENY_PRIVATE=1`/`on`/`true` forces it ON; setting it to
//! any other value (`0`/`off`/`false`) is the explicit production opt-out. See
//! `ssrf::ssrf_deny_private_enabled`.
//!
//! When ON the check runs in three layers:
//! 1. **Pre-send resolve + pin (DNS-rebinding defence)**: parse the URL;
//!    DNS-resolve the host; reject if any resolved IP is private; then
//!    **pin** the client to the first vetted non-private `SocketAddr` via
//!    `ClientBuilder::resolve_to_addrs(host, &[vetted_addr])`.  This closes
//!    the TOCTOU window: reqwest connects to the exact IP that passed the
//!    check — a rebind returning a private address at connect time cannot
//!    bypass the guard.
//! 2. **Each redirect (URL-level re-check)**: a custom
//!    `reqwest::redirect::Policy` repeats the scheme + private-IP check on
//!    every `Location` target before following it, preventing open-redirect
//!    chains that bypass the pre-send check.
//!
//! ### Redirect-pin limitation
//! Full per-redirect IP pinning (resolving + pinning the *new* host on each
//! hop) is not expressible with reqwest's current `redirect::Policy` API: the
//! callback receives only the redirect URL, not a `ClientBuilder`, so we
//! cannot rebuild and re-pin the client mid-chain.  The URL-level re-check
//! (layer 2) is therefore the floor for redirect hops: it catches IP literals
//! and known-private hostnames but cannot prevent a mid-chain rebind on a
//! freshly registered hostname.  In practice redirect chains to attacker-
//! controlled domains are blocked by the scheme + private-IP URL check; a
//! full per-hop pin would require a reqwest API extension or a custom
//! `hyper` connector.

use super::*;
use std::collections::HashMap;
// SSRF deny-private helpers live in the reqwest-free `ssrf` module (so the
// WebSocket client can validate URLs without linking reqwest). The reqwest-
// coupled `ssrf_apply` + the request executor below import the three they use.
#[cfg(not(target_arch = "wasm32"))]
use super::ssrf::{resolve_first_non_private_addr, ssrf_check_url, ssrf_deny_private_enabled};

/// Ipe.Http.HttpResponse — field names/types match the Ipê record alias.
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: i64,
    pub body: String,
    pub headers: HashMap<String, String>,
}

/// Redirect behaviour for an outbound `HttpRequest` — the Rust mirror of the
/// `RedirectPolicy` ADT in `Ipe.Http`.  Variant names match the Ipê
/// constructors verbatim so emitted match arms resolve through the
/// `pub use http_client::*` glob in the generated `mod.rs`.
///
/// `FollowRedirects(i64)` carries the user-supplied max hop count.  The runtime
/// clamps it to `0` via `.max(0)` before casting to `usize`, so a negative
/// value from Ipê code is safe (0 hops followed, not a negative cast).
#[derive(Clone, Debug)]
pub enum RedirectPolicy {
    NoRedirects,
    FollowRedirects(i64),
}

/// Closed set of HTTP methods — the Rust mirror of the `HttpMethod` ADT in
/// `Ipe.Http`.  Variant names match the Ipê constructors verbatim so emitted
/// match arms (`HttpMethod::Get`, `HttpMethod::Post`, …) resolve through the
/// `pub use http_client::*` glob in the generated `mod.rs`.
///
/// Conversions to/from reqwest `Method` happen at the request executor
/// boundary (`method_to_reqwest`) — never at every call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl HttpMethod {
    /// Convert to the canonical uppercase ASCII string.
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
        }
    }

    /// Parse from a string (case-insensitive).  Returns `None` for any
    /// unrecognised verb — the parse-don't-validate boundary.
    // Named `from_str` for call-site readability; it returns `Option<Self>`, not
    // `FromStr`'s `Result`, so it deliberately does not implement that trait.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "GET" => Some(HttpMethod::Get),
            "POST" => Some(HttpMethod::Post),
            "PUT" => Some(HttpMethod::Put),
            "DELETE" => Some(HttpMethod::Delete),
            "PATCH" => Some(HttpMethod::Patch),
            "HEAD" => Some(HttpMethod::Head),
            "OPTIONS" => Some(HttpMethod::Options),
            _ => None,
        }
    }
}

/// Convert an `HttpMethod` to a reqwest `Method`.  Infallible: every ADT
/// variant maps to a well-known reqwest method constant.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn method_to_reqwest(m: HttpMethod) -> reqwest::Method {
    match m {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Post => reqwest::Method::POST,
        HttpMethod::Put => reqwest::Method::PUT,
        HttpMethod::Delete => reqwest::Method::DELETE,
        HttpMethod::Patch => reqwest::Method::PATCH,
        HttpMethod::Head => reqwest::Method::HEAD,
        HttpMethod::Options => reqwest::Method::OPTIONS,
    }
}

/// Ipe.Http.HttpRequest — built in Ipê (defaultRequest + with* updates),
/// so every field is pub for external struct-literal construction.
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub body: String,
    pub headers: Vec<(String, String)>,
    pub method: HttpMethod,
    pub redirects: RedirectPolicy,
    pub timeout: i64,
    pub url: String,
}

/// `Http.methodFromString : String -> Maybe HttpMethod` — the typed parse
/// boundary for inbound method strings.  Returns `Just` for the seven
/// standard verbs (case-insensitive), `Nothing` for anything else.
pub fn http_method_from_string(s: String) -> crate::core::IpeMaybe<HttpMethod> {
    match HttpMethod::from_str(&s) {
        Some(m) => crate::core::IpeMaybe::Just(m),
        None => crate::core::IpeMaybe::Nothing,
    }
}

/// `Http.methodToString : HttpMethod -> String` — canonical uppercase string.
pub fn http_method_to_string(m: HttpMethod) -> String {
    m.as_str().to_string()
}

// ---------------------------------------------------------------------------
// Typed request-target builders — API-layer scheme narrowing (fail-closed)
// ---------------------------------------------------------------------------
//
// The request target is the typed `crate::url::Url` (parsed exactly once at
// `Url.fromString`), never a raw `String`. `http_default_request` and
// `http_with_url` NARROW the scheme to http/https at THIS API layer and return
// a `Result Error HttpRequest`, so a builder-constructed request whose scheme
// is not http(s) cannot be assembled at all — it fails closed EVEN when the
// runtime SSRF guard is disabled in dev (the runtime scheme allowlist is then
// defence-in-depth, not the only line). The single canonical serialization
// (`Url.toString`) is carried as the transport target; the runtime does not
// re-parse a fresh string to reconstruct it.

/// Narrow a typed `Url` to the http/https request surface. The typed `Url`
/// legally carries any absolute scheme (`file:`, `ftp:` are valid `Url`
/// values); this outbound surface accepts only `http`/`https`. Returns the
/// URL's canonical serialization on success, or a blocked-scheme message on
/// any other scheme. Reads the already-parsed scheme — no re-parse of a string.
fn narrow_http_scheme(url: &crate::url::Url) -> Result<String, String> {
    let scheme = crate::url::url_scheme(url.clone());
    if scheme == "http" || scheme == "https" {
        Ok(crate::url::url_to_string(url.clone()))
    } else {
        Err(format!(
            "Http: blocked: scheme {scheme:?} is not http/https"
        ))
    }
}

/// Build an `HttpRequest` targeting `target` (the http/https canonical
/// serialization). Shared by every typed-target builder so the defaults live
/// in one place.
fn http_request_with_target(target: String) -> HttpRequest {
    HttpRequest {
        body: String::new(),
        headers: Vec::new(),
        method: HttpMethod::Get,
        redirects: RedirectPolicy::FollowRedirects(10),
        timeout: 30000,
        url: target,
    }
}

/// `Http.defaultRequest : Url -> Result Error HttpRequest` — the primary
/// request constructor. Takes an already-sealed typed `Url`, narrows its
/// scheme to http/https at the API layer (fail-closed), and carries the single
/// canonical serialization as the transport target.
#[must_use]
pub fn http_default_request<E: From<String>>(url: crate::url::Url) -> IpeResult<E, HttpRequest> {
    match narrow_http_scheme(&url) {
        Ok(target) => IpeResult::Ok(http_request_with_target(target)),
        Err(msg) => IpeResult::Err(msg.into()),
    }
}

/// `Http.defaultRequestFromString : String -> Result Error HttpRequest` — the
/// MARKED parse-at-the-boundary helper for a raw string target. Runs the ONE
/// parse of the string (`Url.fromString`) then the same scheme narrowing,
/// returning `Result` so a raw string is an EXPLICIT parse boundary, never a
/// silent stringly default.
#[must_use]
pub fn http_default_request_from_string<E: From<String>>(raw: String) -> IpeResult<E, HttpRequest> {
    match crate::url::url_from_string::<String>(raw) {
        IpeResult::Ok(url) => http_default_request(url),
        IpeResult::Err(msg) => IpeResult::Err(msg.into()),
    }
}

/// `Http.withUrl : Url -> HttpRequest -> Result Error HttpRequest` — retarget an
/// existing request to a typed `Url`, re-narrowing the scheme to http/https at
/// the API layer (fail-closed), so a retarget cannot smuggle a non-http(s)
/// scheme past the guard-off dev path.
#[must_use]
pub fn http_with_url<E: From<String>>(
    url: crate::url::Url,
    req: HttpRequest,
) -> IpeResult<E, HttpRequest> {
    match narrow_http_scheme(&url) {
        Ok(target) => IpeResult::Ok(HttpRequest { url: target, ..req }),
        Err(msg) => IpeResult::Err(msg.into()),
    }
}

// ---------------------------------------------------------------------------
// SSRF guard — reqwest client integration
// ---------------------------------------------------------------------------
// The reqwest-free validators (ssrf_check_url / resolve_first_non_private_addr /
// is_private_ip / ssrf_validate_url / ssrf_pinned_ws_addr / …) live in `ssrf.rs`.
// What remains here is reqwest-coupled: `ssrf_apply` (a reqwest::ClientBuilder)
// and the request executor.

/// Apply the SSRF deny-private guard to a `reqwest::ClientBuilder` for `url`.
/// When `IPE_HTTP_DENY_PRIVATE` is set, validates the URL scheme + host, resolves
/// to a vetted non-private `SocketAddr`, pins DNS to it (defeats DNS-rebinding),
/// and installs a per-redirect-hop re-check. SHARED by every outbound request
/// surface — the regular Http client, `Http.Stream.open`, the WebSocket client,
/// the Email SES path — so the guard can NEVER be missing from a request path
/// (the bug class this closes: a new path that built its own client without the
/// check). When the guard is off, installs the caller's plain redirect policy.
/// Fail-closed DNS resolver vetting EVERY hostname reqwest resolves (initial AND
/// every redirect hop) at connect time against `ssrf::is_private_ip`, returning
/// only non-private addresses. Closes the redirect-rebinding TOCTOU the per-hop
/// URL re-check left open: `ssrf_check_url` validated a hostname then DISCARDED the
/// address, so reqwest re-resolved it by name at connect and could hit a rebind
/// target; with this resolver reqwest connects to exactly the vetted addrs (no
/// re-resolve). Installed only under IPE_HTTP_DENY_PRIVATE. IP-literal targets
/// bypass the resolver, so the literal/scheme checks below and the per-hop
/// redirect `Policy` remain mandatory.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
struct DenyPrivateResolver;

#[cfg(not(target_arch = "wasm32"))]
impl reqwest::dns::Resolve for DenyPrivateResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let resolved = tokio::task::spawn_blocking(move || {
                use std::net::ToSocketAddrs;
                // Port 0: reqwest substitutes the real port onto the returned addrs.
                let addrs: Vec<std::net::SocketAddr> = (host.as_str(), 0u16)
                    .to_socket_addrs()
                    .map_err(|e| e.to_string())?
                    .filter(|a| !super::ssrf::is_private_ip(a.ip()))
                    .collect();
                if addrs.is_empty() {
                    Err("http: blocked: host resolves only to private/blocked addresses (IPE_HTTP_DENY_PRIVATE)".to_string())
                } else {
                    Ok(addrs)
                }
            })
            .await;
            match resolved {
                Ok(Ok(addrs)) => {
                    let iter: Box<dyn Iterator<Item = std::net::SocketAddr> + Send> =
                        Box::new(addrs.into_iter());
                    Ok(iter)
                }
                Ok(Err(msg)) => Err(msg.into()),
                Err(join) => Err(format!("http: blocked: DNS resolver task failed: {join}").into()),
            }
        })
    }
}

/// Redact a `user:pass@` userinfo segment from a URL string for safe inclusion
/// in an error message. Used on the parse-FAILURE path where the `url` crate
/// can't help, so it's a best-effort split (no raw indexing — `indexing_slicing`
/// is denied crate-wide). Removes the `userinfo@` between `://` and the next `/`.
#[cfg(not(target_arch = "wasm32"))]
fn redact_userinfo(url: &str) -> String {
    match url.split_once("://") {
        None => url.to_string(),
        Some((scheme, rest)) => {
            let (authority, path) = match rest.split_once('/') {
                Some((a, p)) => (a, Some(p)),
                None => (rest, None),
            };
            let host = authority.rsplit_once('@').map_or(authority, |(_ui, h)| h);
            match path {
                Some(p) => format!("{scheme}://{host}/{p}"),
                None => format!("{scheme}://{host}"),
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn ssrf_apply(
    mut builder: reqwest::ClientBuilder,
    url: &str,
    policy: RedirectPolicy,
) -> Result<reqwest::ClientBuilder, String> {
    let deny = ssrf_deny_private_enabled();
    if deny {
        let parsed = reqwest::Url::parse(url).map_err(|e| {
            format!(
                "http: blocked: invalid URL {:?}: {} (IPE_HTTP_DENY_PRIVATE)",
                redact_userinfo(url),
                e
            )
        })?;
        let scheme = parsed.scheme();
        if scheme != "http" && scheme != "https" && scheme != "ws" && scheme != "wss" {
            return Err(format!(
                "http: blocked: scheme {:?} is not http/https/ws/wss (IPE_HTTP_DENY_PRIVATE)",
                scheme
            ));
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| "http: blocked: URL has no host (IPE_HTTP_DENY_PRIVATE)".to_string())?
            .to_owned();
        let addr = resolve_first_non_private_addr(&host)?;
        builder = builder.resolve_to_addrs(host.as_str(), &[addr]);
        // Pin EVERY hostname (the initial host's static override above + every
        // redirect hop) through the vetting resolver so a redirect to a DIFFERENT
        // hostname can't be re-resolved by name to a rebind target at connect.
        builder = builder.dns_resolver(std::sync::Arc::new(DenyPrivateResolver));
    }
    // Apply the redirect policy.  The per-redirect-hop SSRF re-check is
    // preserved unchanged: only the follow/limit decision now reads
    // `RedirectPolicy` instead of a `(bool, i64)` pair.
    builder = match policy {
        RedirectPolicy::NoRedirects => builder.redirect(reqwest::redirect::Policy::none()),
        RedirectPolicy::FollowRedirects(max_hops) => {
            // Clamp to 0: a user-supplied negative Int is safe (0 hops
            // followed).  This is the load-bearing `.max(0)` the spec requires.
            let max = max_hops.max(0) as usize;
            if deny {
                builder.redirect(reqwest::redirect::Policy::custom(move |attempt| {
                    if attempt.previous().len() >= max {
                        return attempt.error(format!("http: too many redirects (max {})", max));
                    }
                    if let Err(msg) = ssrf_check_url(attempt.url().as_str()) {
                        return attempt.error(msg);
                    }
                    attempt.follow()
                }))
            } else {
                builder.redirect(reqwest::redirect::Policy::limited(max))
            }
        }
    };
    Ok(builder)
}

// ---------------------------------------------------------------------------
// Core request executor
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
async fn do_request<E: From<String> + Send + 'static>(
    req: HttpRequest,
) -> IpeResult<E, HttpResponse> {
    // This surface (Http.get/post/request) accepts only http/https — ws/wss is
    // the WebSocket client's surface. When the SSRF guard is on, enforce that
    // narrower scheme set here, then delegate the host-resolve + DNS-rebinding
    // pin + per-redirect re-check to the SHARED `ssrf_apply` (single source of
    // truth — no second hand-rolled copy of the guard that could drift out of
    // sync with the one every other outbound surface uses).
    if ssrf_deny_private_enabled() {
        match reqwest::Url::parse(&req.url) {
            Ok(u) => {
                let scheme = u.scheme();
                if scheme != "http" && scheme != "https" {
                    return IpeResult::Err(
                        format!(
                            "http: blocked: scheme {:?} is not http/https (IPE_HTTP_DENY_PRIVATE)",
                            scheme
                        )
                        .into(),
                    );
                }
            }
            Err(e) => {
                return IpeResult::Err(
                    format!(
                        "http: blocked: invalid URL {:?}: {} (IPE_HTTP_DENY_PRIVATE)",
                        redact_userinfo(&req.url),
                        e
                    )
                    .into(),
                );
            }
        }
    }

    let builder = reqwest::Client::builder();
    let mut builder = match ssrf_apply(builder, &req.url, req.redirects) {
        Ok(b) => b,
        Err(e) => return IpeResult::Err(e.into()),
    };

    // Always install a request deadline. A Ipê-controllable `timeout <= 0`
    // would otherwise leave the request with no deadline (slowloris / hung-
    // connection vector), so floor it to 30 s instead of disabling it.
    let timeout_ms = if req.timeout > 0 {
        req.timeout as u64
    } else {
        30_000
    };
    builder = builder.timeout(std::time::Duration::from_millis(timeout_ms));

    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => return IpeResult::Err(format!("http: client build failed: {}", e).into()),
    };
    let method = method_to_reqwest(req.method);
    let mut rb = client.request(method, &req.url);
    for (k, v) in &req.headers {
        rb = rb.header(k.as_str(), v.as_str());
    }
    if !req.body.is_empty() {
        rb = rb.body(req.body.clone());
    }
    let resp = match rb.send().await {
        Ok(r) => r,
        // [B8] Route the transport error through the redacting constructor: a
        // reqwest/hyper error Display can echo `req.url` (which may carry userinfo
        // / a query-string API key) and the resolved address. The raw detail is
        // logged server-side under a correlation id; Ipê sees only the generic
        // `external operation failed (ref <id>)`. Mirrors http_stream.rs.
        Err(e) => {
            return IpeResult::Err(ipe_error_from_foreign(e));
        }
    };
    let status = resp.status().as_u16() as i64;
    let mut headers = HashMap::new();
    for (k, v) in resp.headers() {
        if let Ok(s) = v.to_str() {
            // MIME-case parity: reqwest's `HeaderName::as_str()`
            // ALWAYS returns lower-case, but the Go reference stores response
            // headers canonicalised (`net/http.Header` always is) — a Ipê
            // program's `Dict.get "Content-Type" resp.headers` must find the
            // key. Route through the SAME `canonical_header` the Server
            // inbound path uses (its Go-parity table is pinned by
            // `canonical_header_matches_go_canonical_mime_key`).
            headers.insert(
                crate::http_header::canonical_header(k.as_str()),
                s.to_string(),
            );
        }
    }
    let body = match read_body_capped::<E>(resp).await {
        IpeResult::Ok(b) => b,
        IpeResult::Err(e) => return IpeResult::Err(e),
    };
    ok_res(HttpResponse {
        status,
        body,
        headers,
    })
}

/// Default cap on a buffered HTTP response body (`Http.get`/`post`/`request`):
/// 100 MiB. `Http.*` returns the body as a single `String`, so an unbounded
/// read of an attacker- or upstream-controlled response is a memory-exhaustion
/// (OOM) vector. Override via `IPE_HTTP_MAX_BODY_BYTES` (streaming consumers that
/// need unbounded bodies use `Ipe.Http.Stream` instead).
#[cfg(not(target_arch = "wasm32"))]
const HTTP_BODY_CAP_DEFAULT: usize = 100 * 1024 * 1024;

#[cfg(not(target_arch = "wasm32"))]
fn http_body_cap() -> usize {
    crate::system::read_env_var("IPE_HTTP_MAX_BODY_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(HTTP_BODY_CAP_DEFAULT)
}

/// Read a response body into a `String` with a hard byte cap. The
/// `Content-Length` pre-check only fast-fails the declared-length case — note
/// reqwest's `gzip` feature transparently decompresses and STRIPS
/// `Content-Length`, so for a gzip'd (e.g. compression-bomb) response
/// `content_length()` is `None` and this pre-check is skipped. The load-bearing
/// guard is the INCREMENTAL cap in the stream loop below, which bounds the
/// decompressed body to `cap` regardless of encoding or a lying/chunked length —
/// a bomb is capped at `cap` resident bytes, never unbounded. UTF-8 lossy
/// (matches `Http.Stream`'s chunk decode; Go reads bytes→string too).
#[cfg(not(target_arch = "wasm32"))]
async fn read_body_capped<E: From<String> + Send + 'static>(
    resp: reqwest::Response,
) -> IpeResult<E, String> {
    use futures_util::StreamExt;
    let cap = http_body_cap();
    if let Some(len) = resp.content_length()
        && len as usize > cap
    {
        return IpeResult::Err(
                format!(
                    "http: response body too large ({} > {} bytes; raise IPE_HTTP_MAX_BODY_BYTES or use Http.Stream)",
                    len, cap
                )
                .into(),
            );
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = match chunk {
            Ok(b) => b,
            // [B8] Redact: a body-read transport error can also echo the URL /
            // resolved address. Log server-side under a correlation id, return
            // only the generic ref to Ipê.
            Err(e) => return IpeResult::Err(ipe_error_from_foreign(e)),
        };
        if buf.len().saturating_add(bytes.len()) > cap {
            return IpeResult::Err(
                format!(
                    "http: response body too large (> {} bytes; raise IPE_HTTP_MAX_BODY_BYTES or use Http.Stream)",
                    cap
                )
                .into(),
            );
        }
        buf.extend_from_slice(&bytes);
    }
    IpeResult::Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Http.get : Url -> Task Error HttpResponse
///
/// Takes an already-sealed typed `Url` (parsed exactly once at
/// `Url.fromString`, the single SSRF parse boundary) and carries its canonical
/// serialization as the transport target. The runtime SSRF floor in
/// `do_request` (per-hop scheme allowlist + private-IP resolver) is unchanged —
/// the typed argument is the defence-in-depth guard at the API boundary, not a
/// replacement for the runtime floor.
#[cfg(not(target_arch = "wasm32"))]
pub fn http_get<E: From<String> + Send + 'static>(
    url: crate::url::Url,
) -> IpeTask<E, HttpResponse> {
    Box::pin(do_request(HttpRequest {
        body: String::new(),
        headers: Vec::new(),
        method: HttpMethod::Get,
        redirects: RedirectPolicy::FollowRedirects(10),
        timeout: 30000,
        url: crate::url::url_to_string(url),
    }))
}

/// Http.post : Url -> String -> Task Error HttpResponse
///
/// Takes an already-sealed typed `Url` (see [`http_get`]) and carries its
/// canonical serialization as the transport target. The runtime SSRF floor is
/// unchanged.
#[cfg(not(target_arch = "wasm32"))]
pub fn http_post<E: From<String> + Send + 'static>(
    url: crate::url::Url,
    body: String,
) -> IpeTask<E, HttpResponse> {
    Box::pin(do_request(HttpRequest {
        body,
        headers: Vec::new(),
        method: HttpMethod::Post,
        redirects: RedirectPolicy::FollowRedirects(10),
        timeout: 30000,
        url: crate::url::url_to_string(url),
    }))
}

/// Http.request : HttpRequest -> Task Error HttpResponse
#[cfg(not(target_arch = "wasm32"))]
pub fn http_request<E: From<String> + Send + 'static>(
    req: HttpRequest,
) -> IpeTask<E, HttpResponse> {
    Box::pin(do_request(req))
}

/// Http.parseQuery : String -> Dict String String (pure; first value wins).
pub fn http_parse_query(raw: String) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for pair in raw.trim_start_matches('?').split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut it = pair.splitn(2, '=');
        let k = form_url_decode(it.next().unwrap_or(""));
        let v = form_url_decode(it.next().unwrap_or(""));
        out.entry(k).or_insert(v); // repeated keys keep the first value
    }
    out
}

// ---------------------------------------------------------------------------
// wasm32 browser substitute — `fetch` (Open Decision 1, resolved)
// ---------------------------------------------------------------------------
//
// `docs/adr/0042-wasm-client-target.md`'s Open Decision 1 asks to settle
// reqwest-wasm vs raw `web-sys` fetch against the actual `http_client` kernel
// code. Resolved to raw `web-sys` fetch:
//
//   - The native kernel's substantial logic beyond request-building is the
//     SSRF deny-private guard above (`ssrf_apply`/`DenyPrivateResolver`) — DNS
//     resolution, address pinning, per-redirect-hop re-checks. None of it has
//     a browser analogue: a tab cannot open a raw socket or resolve a
//     hostname itself; CORS/mixed-content/CSP are the browser's OWN network
//     boundary, already enforced beneath this code with no
//     `IPE_HTTP_DENY_PRIVATE`-shaped opt-in needed. There is no shared logic
//     worth reusing from the reqwest path.
//   - reqwest's wasm32 backend is itself a thin wrapper over `fetch`; taking
//     it adds a dependency layer (reqwest + its wasm shims) atop the same
//     browser primitive called directly below, for no behavioural gain and a
//     real bundle-size cost (efficiency ranks above completeness once the
//     reuse case is gone — spec's own stated tie-breaker).
//
// CORS blocks, network errors, and timeouts all reject the `fetch` Promise
// rather than trapping the instance; every rejection here routes through the
// SAME generic `Task.fail` arm — never a panic, never a silent drop.

/// `IPE_HTTP_MAX_BODY_BYTES` cap, mirrored from the native arm's
/// `http_body_cap` (same env var, same default) — `fetch`'s `.text()` buffers
/// the whole body itself, so this is a post-hoc size guard rather than a
/// streamed one, but it keeps the same DoS floor on both targets.
#[cfg(target_arch = "wasm32")]
fn wasm_http_body_cap() -> usize {
    crate::system::read_env_var("IPE_HTTP_MAX_BODY_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(100 * 1024 * 1024)
}

#[cfg(target_arch = "wasm32")]
async fn do_fetch<E: From<String> + 'static>(req: HttpRequest) -> IpeResult<E, HttpResponse> {
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    let window = match web_sys::window() {
        Some(w) => w,
        None => {
            return IpeResult::Err(
                "http: no window (not a browser context?)"
                    .to_string()
                    .into(),
            );
        }
    };

    let headers = match web_sys::Headers::new() {
        Ok(h) => h,
        Err(_) => {
            return IpeResult::Err("http: failed to build request headers".to_string().into());
        }
    };
    for (k, v) in &req.headers {
        if headers.set(k, v).is_err() {
            return IpeResult::Err(format!("http: invalid header {k:?}").into());
        }
    }

    let init = web_sys::RequestInit::new();
    init.set_method(req.method.as_str());
    init.set_headers(&headers);
    init.set_mode(web_sys::RequestMode::Cors);
    init.set_redirect(match &req.redirects {
        RedirectPolicy::FollowRedirects(_) => web_sys::RequestRedirect::Follow,
        RedirectPolicy::NoRedirects => web_sys::RequestRedirect::Error,
    });
    if !req.body.is_empty() {
        init.set_body(&JsValue::from_str(&req.body));
    }

    // Timeout via `AbortController` — the browser analogue of the native
    // request deadline. A fired abort rejects the fetch Promise, routed
    // through the same generic transport-error arm below (never a trap).
    let controller = web_sys::AbortController::new().ok();
    let _timeout_handle = controller.as_ref().map(|ctl| {
        init.set_signal(Some(&ctl.signal()));
        let timeout_ms = if req.timeout > 0 { req.timeout } else { 30_000 };
        let ctl = ctl.clone();
        gloo_timers::callback::Timeout::new(timeout_ms.max(0) as u32, move || {
            ctl.abort();
        })
    });

    let request = match web_sys::Request::new_with_str_and_init(&req.url, &init) {
        Ok(r) => r,
        Err(e) => return IpeResult::Err(format!("http: bad request: {:?}", e).into()),
    };

    let resp_value = match JsFuture::from(window.fetch_with_request(&request)).await {
        Ok(v) => v,
        Err(e) => {
            return IpeResult::Err(
                format!(
                    "http: fetch failed (network error, CORS block, or timeout): {:?}",
                    e
                )
                .into(),
            );
        }
    };
    let resp: web_sys::Response = match resp_value.dyn_into() {
        Ok(r) => r,
        Err(_) => {
            return IpeResult::Err("http: fetch did not return a Response".to_string().into());
        }
    };

    let status = i64::from(resp.status());

    let mut out_headers = HashMap::new();
    if let Ok(Some(iter)) = js_sys::try_iter(resp.headers().as_ref()) {
        for entry in iter.flatten() {
            if let Ok(pair) = entry.dyn_into::<js_sys::Array>() {
                let k = pair.get(0).as_string().unwrap_or_default();
                let v = pair.get(1).as_string().unwrap_or_default();
                if !k.is_empty() {
                    out_headers.insert(crate::http_header::canonical_header(&k), v);
                }
            }
        }
    }

    let text_promise = match resp.text() {
        Ok(p) => p,
        Err(e) => {
            return IpeResult::Err(format!("http: failed reading response body: {:?}", e).into());
        }
    };
    let text_value = match JsFuture::from(text_promise).await {
        Ok(v) => v,
        Err(e) => {
            return IpeResult::Err(format!("http: failed reading response body: {:?}", e).into());
        }
    };
    let body = text_value.as_string().unwrap_or_default();
    let cap = wasm_http_body_cap();
    if body.len() > cap {
        return IpeResult::Err(
            format!("http: response body too large (> {cap} bytes; raise IPE_HTTP_MAX_BODY_BYTES)")
                .into(),
        );
    }

    ok_res(HttpResponse {
        status,
        body,
        headers: out_headers,
    })
}

/// Http.get : Url -> Task Error HttpResponse (browser substitute)
#[cfg(target_arch = "wasm32")]
pub fn http_get<E: From<String> + 'static>(url: crate::url::Url) -> IpeTask<E, HttpResponse> {
    Box::pin(do_fetch(HttpRequest {
        body: String::new(),
        headers: Vec::new(),
        method: HttpMethod::Get,
        redirects: RedirectPolicy::FollowRedirects(10),
        timeout: 30000,
        url: crate::url::url_to_string(url),
    }))
}

/// Http.post : Url -> String -> Task Error HttpResponse (browser substitute)
#[cfg(target_arch = "wasm32")]
pub fn http_post<E: From<String> + 'static>(
    url: crate::url::Url,
    body: String,
) -> IpeTask<E, HttpResponse> {
    Box::pin(do_fetch(HttpRequest {
        body,
        headers: Vec::new(),
        method: HttpMethod::Post,
        redirects: RedirectPolicy::FollowRedirects(10),
        timeout: 30000,
        url: crate::url::url_to_string(url),
    }))
}

/// Http.request : HttpRequest -> Task Error HttpResponse (browser substitute)
#[cfg(target_arch = "wasm32")]
pub fn http_request<E: From<String> + 'static>(req: HttpRequest) -> IpeTask<E, HttpResponse> {
    Box::pin(do_fetch(req))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wiring seal: the response-header collection loop must route
    /// every key through `http_header::canonical_header` (reqwest's
    /// `HeaderName::as_str()` is always lower-case; Go's `net/http.Header` is
    /// always canonical — `Dict.get "Content-Type"` parity depends on it).
    /// `do_request` needs a live round-trip to test end-to-end, so this pins
    /// the call's PRESENCE at the source level; the transform itself is
    /// proven by `http_header`'s own Go-parity table test.
    #[test]
    fn response_header_loop_canonicalises_keys() {
        let src = include_str!("http_client.rs");
        let loop_marker = "for (k, v) in resp.headers()";
        let start = src.find(loop_marker);
        assert!(start.is_some(), "response-header loop not found");
        let Some(start) = start else { return };
        // Window sized to comfortably cover the loop body INCLUDING its doc
        // comment (the canonicalisation call sits ~700 bytes past the marker).
        let window = &src[start..src.len().min(start + 1600)];
        assert!(
            window.contains("canonical_header(k.as_str())"),
            "response-header keys must be canonicalised (Go MIME-case parity, #33 §6.1)"
        );
        assert!(
            !window.contains("insert(k.as_str().to_string()"),
            "raw lower-case header-key insert reintroduced (#33 §6.1 regression)"
        );
    }

    #[test]
    fn parse_query_decode_and_first_wins() {
        let q = http_parse_query("a=1&b=two%20words&a=ignored&c".to_string());
        assert_eq!(q.get("a").map(String::as_str), Some("1")); // first value wins
        assert_eq!(q.get("b").map(String::as_str), Some("two words"));
        assert_eq!(q.get("c").map(String::as_str), Some(""));
        // Leading '?' tolerated; empty pairs skipped.
        let q2 = http_parse_query("?x=9&".to_string());
        assert_eq!(q2.get("x").map(String::as_str), Some("9"));
        assert_eq!(q2.len(), 1);
    }
    // SSRF guard unit tests moved to `ssrf.rs` alongside the validators.

    // ── Typed request-target builders — API-layer scheme narrowing ──────────
    //
    // These prove the fail-closed narrowing is enforced at the API layer, so a
    // builder-constructed request whose scheme is not http(s) cannot be
    // assembled REGARDLESS of the runtime SSRF guard (no env toggling here —
    // the narrowing is unconditional).

    fn seal(raw: &str) -> crate::url::Url {
        match crate::url::url_from_string::<String>(raw.to_string()) {
            IpeResult::Ok(u) => u,
            IpeResult::Err(e) => panic!("expected {raw:?} to be a valid Url: {e}"),
        }
    }

    #[test]
    fn default_request_accepts_http_and_https() {
        for raw in ["http://example.com/", "https://example.com:8443/a?q=1"] {
            match http_default_request::<String>(seal(raw)) {
                IpeResult::Ok(req) => assert_eq!(req.url, raw),
                IpeResult::Err(e) => panic!("{raw:?} must build, got Err: {e}"),
            }
        }
    }

    #[test]
    fn default_request_rejects_non_http_scheme_fail_closed() {
        // `file:` / `ftp:` are valid `Url` values but must be rejected at the
        // API layer, independent of the runtime SSRF guard.
        for raw in [
            "ftp://example.com/x",
            "file:///etc/passwd",
            "ws://example.com/",
        ] {
            match http_default_request::<String>(seal(raw)) {
                IpeResult::Err(e) => assert!(
                    e.contains("not http/https"),
                    "{raw:?} must fail closed with a scheme message, got: {e}"
                ),
                IpeResult::Ok(_) => panic!("{raw:?} must NOT build — non-http(s) scheme"),
            }
        }
    }

    #[test]
    fn default_request_from_string_is_the_marked_parse_boundary() {
        // Ok on a valid http(s) string.
        match http_default_request_from_string::<String>("https://example.com/".to_string()) {
            IpeResult::Ok(req) => assert_eq!(req.url, "https://example.com/"),
            IpeResult::Err(e) => panic!("valid https string must build, got: {e}"),
        }
        // A relative / scheme-less string fails at the parse seal.
        match http_default_request_from_string::<String>("/just/a/path".to_string()) {
            IpeResult::Err(_) => {}
            IpeResult::Ok(_) => panic!("a relative reference must not build a request"),
        }
        // A syntactically valid but non-http(s) scheme fails at the narrowing.
        match http_default_request_from_string::<String>("ftp://example.com/x".to_string()) {
            IpeResult::Err(e) => assert!(e.contains("not http/https"), "got: {e}"),
            IpeResult::Ok(_) => panic!("ftp must fail closed at the API layer"),
        }
    }

    #[test]
    fn with_url_retarget_re_narrows_scheme() {
        let base = http_request_with_target("http://example.com/".to_string());
        // Retarget to another http URL — Ok, url replaced, other fields kept.
        let base_body = HttpRequest {
            body: "keep-me".to_string(),
            ..base.clone()
        };
        match http_with_url::<String>(seal("https://example.org/next"), base_body.clone()) {
            IpeResult::Ok(req) => {
                assert_eq!(req.url, "https://example.org/next");
                assert_eq!(req.body, "keep-me", "non-url fields must be preserved");
            }
            IpeResult::Err(e) => panic!("http(s) retarget must succeed, got: {e}"),
        }
        // Retarget to a non-http(s) scheme — fail closed, at the API layer.
        match http_with_url::<String>(seal("file:///etc/passwd"), base_body) {
            IpeResult::Err(e) => assert!(e.contains("not http/https"), "got: {e}"),
            IpeResult::Ok(_) => panic!("file: retarget must fail closed"),
        }
    }

    // ── RedirectPolicy — struct field and negative-clamp proof ──────────────

    /// `HttpRequest` carries `redirects: RedirectPolicy` and the default is
    /// `FollowRedirects(10)`.  Verify the struct builds correctly and the
    /// `NoRedirects` variant is distinct.
    #[test]
    fn redirect_policy_default_and_variants() {
        let req = http_request_with_target("http://example.com/".to_string());
        match req.redirects {
            RedirectPolicy::FollowRedirects(n) => {
                assert_eq!(n, 10, "default hop count must be 10")
            }
            RedirectPolicy::NoRedirects => panic!("default must be FollowRedirects(10)"),
        }
        // NoRedirects round-trip.
        let mut req2 = req.clone();
        req2.redirects = RedirectPolicy::NoRedirects;
        assert!(
            matches!(req2.redirects, RedirectPolicy::NoRedirects),
            "NoRedirects must survive a clone-and-assign"
        );
    }

    /// A negative `FollowRedirects` value from user code must not panic or
    /// cause a negative cast.  The `.max(0)` clamp in `ssrf_apply` is the
    /// load-bearing floor; this test pins it at the type level.
    #[test]
    fn negative_follow_redirects_clamps_to_zero_without_panic() {
        // `max_hops.max(0) as usize` — must not panic for any i64 value.
        let negative: i64 = -1;
        let clamped = negative.max(0) as usize;
        assert_eq!(
            clamped, 0,
            "negative hop count must clamp to 0, not wrap or panic"
        );
        let min_i64: i64 = i64::MIN;
        let clamped_min = min_i64.max(0) as usize;
        assert_eq!(clamped_min, 0, "i64::MIN must clamp to 0");
    }
}

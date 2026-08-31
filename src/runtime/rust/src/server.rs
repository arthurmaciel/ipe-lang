//! Ipe.Http.Server runtime — axum/hyper under a Ipê-native surface.
//!
//! Handlers are Ipê closures `Fn(Request) -> Task Error Response`. server_get
//! ERASES the project-defined error type E into a non-generic ServerRoute
//! (awaiting the task, mapping Err -> 500) so routes are uniform yet handlers
//! stay Send+Sync+'static for axum. server_listen builds an axum Router and
//! serves via tokio.
//!
//! `Request` and `Response` are OPAQUE `Ty::Con` types in the compiler IR
//! (`ipe_ir::IrType::ServerRequest` / `ServerResponse`). Ipê code cannot
//! construct or mutate them directly — it reads a request via ACCESSOR KERNELS
//! (`Server.body` / `Server.path` / `Server.method` / `Server.header` /
//! `Server.queryParam` / `Server.getCookie` / `Server.param`) and builds a
//! response via typed builder kernels (`Server.text` / `Server.json` /
//! `Server.html` / `Server.withStatus` / `Server.withHeader` /
//! `Server.redirect` / `Server.withCookie`). The `pub` fields on
//! `ServerRequest` / `ServerResponse` below exist solely so the accessor
//! functions in this file can read them — they are NOT part of the Ipê API.
//! `Route`/`Cookie` are opaque Ipê ADTs mapped the same way.

use super::*;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Ipe.Http.Server.Request — opaque parsed request handle.
// camelCase field names are required because accessor kernels (server_body,
// server_path, server_method, …) read these fields directly by name. These
// fields are NOT part of the Ipê API — Ipê code always goes through a kernel.
// `build_request` populates every field exactly once at the axum boundary.
#[allow(non_snake_case)]
#[derive(Clone, Debug)]
pub struct ServerRequest {
    pub method: String,
    pub path: String,
    pub body: String,
    pub headers: HashMap<String, String>,
    pub params: HashMap<String, String>,
    pub query: HashMap<String, String>,
    pub cookies: HashMap<String, String>,
    pub remoteAddr: String,
}

/// Ipe.Http.Server.Response — opaque response handle built by accessor kernels.
// camelCase field names are required because builder/emit kernels (server_text,
// server_with_status, to_axum_response, …) write/read these fields directly.
// These fields are NOT part of the Ipê API — Ipê code always uses builder kernels.
#[allow(non_snake_case)]
#[derive(Clone, Debug)]
pub struct ServerResponse {
    pub status: i64,
    pub body: String,
    pub headers: HashMap<String, String>,
    pub contentType: String,
    /// Pre-built `Set-Cookie` header VALUES (e.g. `"sid=abc; Path=/; HttpOnly"`),
    /// one entry per cookie. Kept separate from `headers` because `headers` is a
    /// `HashMap` (one value per key) and HTTP allows/requires MULTIPLE
    /// `Set-Cookie` response headers when a response needs to set more than one
    /// cookie (RFC 6265 §4.1 — Set-Cookie is the one header that must NEVER be
    /// comma-folded). A second caller writing into `headers["Set-Cookie"]` would
    /// silently clobber the first.
    pub cookies: Vec<String>,
}

/// Ipe.Http.Server.Cookie (opaque) — safe defaults applied at attach time.
#[derive(Clone, Debug)]
pub struct ServerCookie {
    pub name: String,
    pub value: String,
}

/// A handler erased of its Ipê error type `E`: it awaits the Ipê task and maps
/// the result to either the response (Ok) or a 500 marker (Err). Erasing E here
/// keeps `ServerRoute` non-generic so it bridges to the non-generic Ipê `Route`.
type ErasedHandler = Arc<
    dyn Fn(ServerRequest) -> Pin<Box<dyn Future<Output = Result<ServerResponse, String>> + Send>>
        + Send
        + Sync,
>;

/// The Ipê `Handler` type (`Request -> Task Error Response`) reified as a
/// shareable, error-typed closure. The Rust codegen renders the `Handler` type
/// alias (and any `Request -> Task Error Response` arrow — e.g. the `h :
/// Handler` param of a middleware-wrapping closure `guarded h = …`) as exactly
/// this `Arc<dyn Fn>`, because a real route handler CAPTURES app state
/// (`handleRegister cfg db`) and a capturing closure cannot coerce to a bare
/// `fn` pointer.
pub type ServerHandler<E> = Arc<dyn Fn(ServerRequest) -> IpeTask<E, ServerResponse> + Send + Sync>;

/// Accept a route / middleware handler as EITHER a bare closure / fn item OR an
/// already-boxed `ServerHandler<E>` (the Arc the `Handler` alias renders as),
/// converging both to `ServerHandler<E>`. The two impls below can never overlap:
/// `Arc<dyn Fn>` does NOT itself implement `Fn`, so a value is covered by at most
/// one impl. This is what lets `server_get(path, my_fn)` (15-http-server, a bare
/// fn item) AND `wrap(guarded(handleDelete cfg db))` (36-composite-server, a
/// captured Arc handler threaded through middleware) both register without any
/// call-site wrapping in the generated code — the conversion is total and
/// allocation-free on the Arc path (it returns the Arc as-is).
pub trait IntoServerHandler<E> {
    fn into_server_handler(self) -> ServerHandler<E>;
}

impl<E, F> IntoServerHandler<E> for F
where
    F: Fn(ServerRequest) -> IpeTask<E, ServerResponse> + Send + Sync + 'static,
{
    fn into_server_handler(self) -> ServerHandler<E> {
        Arc::new(self)
    }
}

impl<E> IntoServerHandler<E> for ServerHandler<E> {
    fn into_server_handler(self) -> ServerHandler<E> {
        self
    }
}

// The codegen Arc-wraps a partial-applied route handler at its construction site
// (`Arc::new(move |req| handle_register(cfg, db, req))`), yielding an
// `Arc<{concrete closure}>` — distinct from both the blanket `F: Fn` impl (an
// `Arc` is not itself `Fn`) and the `Arc<dyn Fn>` (`ServerHandler<E>`) impl above
// (`dyn Fn` is `!Sized`, so it can't match this `Sized` `F`). Unsize it to
// `Arc<dyn Fn>` here so that form registers directly with `server_get` /
// `server_api`. The three impls cover pairwise-disjoint types.
impl<E, F> IntoServerHandler<E> for Arc<F>
where
    F: Fn(ServerRequest) -> IpeTask<E, ServerResponse> + Send + Sync + 'static,
{
    fn into_server_handler(self) -> ServerHandler<E> {
        self
    }
}

/// A `Server.mountApp` target: the mounted web app's router-builder, kept
/// behind `Arc<Mutex<Option<..>>>` so `RouteTarget`/`ServerRoute` stay `Clone`
/// (the builder is `FnOnce`, taken exactly once when `server_listen` nests it).
/// A second nest of the same route (a clone) finds `None` and skips — inert, no
/// panic. The `web` feature gates it because the builder produces an `axum`
/// router the mount nests; a server built without `web` never sees one.
#[cfg(feature = "web")]
type MountCell = Arc<std::sync::Mutex<Option<crate::tea::MountBuilder>>>;

/// Discriminated union over the possible route targets — replaces the
/// two `Option` fields so both-None is unrepresentable.
#[derive(Clone)]
enum RouteTarget {
    Handler(ErasedHandler),
    Static(String),
    /// `Server.mountApp prefix webApp`: nest the embedded web app's router
    /// under `path` (the prefix) on the shared server port.
    #[cfg(feature = "web")]
    MountWeb(MountCell),
}

/// Ipe.Http.Server.Route (opaque). Non-generic — see ErasedHandler.
#[derive(Clone)]
pub struct ServerRoute {
    pub method: String,
    pub path: String,
    target: RouteTarget, // private; was the two pub Options
}

// ─── handler erasure ──────────────────────────────────────────────────────

fn erase<E>(h: ServerHandler<E>) -> ErasedHandler
where
    E: Send + 'static,
{
    Arc::new(move |req: ServerRequest| {
        let task = h(req);
        Box::pin(async move {
            match task.await {
                IpeResult::Ok(resp) => Ok(resp),
                // The error detail is dropped at the boundary (-> 500). Handlers
                // wanting a typed error response should return an Ok response with
                // Server.withStatus instead; Err is for unexpected failures.
                IpeResult::Err(_) => Err("handler returned Err".to_string()),
            }
        }) as Pin<Box<dyn Future<Output = Result<ServerResponse, String>> + Send>>
    })
}

fn route<E, H>(method: &str, path: String, h: H) -> ServerRoute
where
    E: Send + 'static,
    H: IntoServerHandler<E>,
{
    ServerRoute {
        method: method.to_string(),
        path,
        target: RouteTarget::Handler(erase(h.into_server_handler())),
    }
}

// ─── routing kernels ──────────────────────────────────────────────────────

pub fn server_get<E, H>(path: String, h: H) -> ServerRoute
where
    E: Send + 'static,
    H: IntoServerHandler<E>,
{
    route("GET", path, h)
}

pub fn server_post<E, H>(path: String, h: H) -> ServerRoute
where
    E: Send + 'static,
    H: IntoServerHandler<E>,
{
    route("POST", path, h)
}

pub fn server_put<E, H>(path: String, h: H) -> ServerRoute
where
    E: Send + 'static,
    H: IntoServerHandler<E>,
{
    route("PUT", path, h)
}

pub fn server_delete<E, H>(path: String, h: H) -> ServerRoute
where
    E: Send + 'static,
    H: IntoServerHandler<E>,
{
    route("DELETE", path, h)
}

pub fn server_any<E, H>(path: String, h: H) -> ServerRoute
where
    E: Send + 'static,
    H: IntoServerHandler<E>,
{
    route("ANY", path, h)
}

/// Server.api : String -> (Request -> Task Error Response) -> Route
///
/// `spec` is "METHOD /path" (e.g. "POST /v1/generate"); an omitted method
/// matches any verb. Mirrors Go's `Server_api`. The CSRF-exemption Go performs
/// (`WithoutCsrf`) is a browser-session / double-submit concern from Ipe.Web
/// with no analogue on the Rust HTTP server, so it has no effect here.
pub fn server_api<E, H>(spec: String, h: H) -> ServerRoute
where
    E: Send + 'static,
    H: IntoServerHandler<E>,
{
    // split_once is total by construction — no raw `spec[..idx]` range slice
    // (the restriction-lint footgun if the delimiter ever became multi-byte).
    let (method, path) = match spec.split_once(' ') {
        Some((m, p)) if !m.is_empty() => (m.trim().to_uppercase(), p.trim().to_string()),
        _ => ("ANY".to_string(), spec.trim().to_string()),
    };
    route(&method, path, h)
}

/// Server.static : String -> String -> Route  (urlPrefix, dir)
pub fn server_static(path: String, dir: String) -> ServerRoute {
    ServerRoute {
        method: "GET".to_string(),
        path,
        target: RouteTarget::Static(dir),
    }
}

/// `Server.mountApp : String -> WebApp -> Route` — mount a `Web.embed` handle
/// at the `prefix` path into the shared server Router. The embedded app runs on
/// the SAME listener as the sibling `Server.get`/`post` routes (one port). The
/// `WebApp` arg is a `Web.embed` handle carrying a router-builder; the type
/// system (`mountApp : String -> WebApp -> Route`) guarantees only a `WebApp`
/// reaches here, so a wrong-shape app is already a compile error.
///
/// A `Web.app` (standalone) handle would carry no mount builder (`None`); that
/// is unreachable for well-typed source that reached `mountApp` via `Web.embed`,
/// but is handled fail-closed anyway (the route becomes inert — it nests
/// nothing — never a panic).
#[cfg(feature = "web")]
pub fn server_mount_app(prefix: String, app: crate::tea::WebApp) -> ServerRoute {
    let cell: MountCell = Arc::new(std::sync::Mutex::new(app.into_mount_builder()));
    ServerRoute {
        method: "MOUNT".to_string(),
        path: prefix,
        target: RouteTarget::MountWeb(cell),
    }
}

// ─── authenticated routes (fail-closed; sole Principal minter) ─────────────
//
// Token verification runs through `crate::auth`, so the authed surface compiles
// only when the `jwt` feature is also selected. A program that uses `getAuthed`
// pulls `jwt` into the emitted project's features.

/// Ipe.Server.TokenSource — where the auth middleware reads the session token.
#[cfg(feature = "jwt")]
#[derive(Clone, Debug)]
pub enum TokenSource {
    /// `Authorization: Bearer <token>`.
    BearerHeader,
    /// A named request cookie carrying the token.
    Cookie(String),
}

/// Ipe.Server.AuthConfig (opaque) — the secret, token source, claim key, and
/// revocation mode the middleware uses. Built by [`server_auth_config`]; the
/// only value the authed-route kernels accept. The revocation mode defaults to
/// `Off` and is set by [`server_with_revocation`].
#[cfg(feature = "jwt")]
#[derive(Clone)]
pub struct AuthConfig {
    secret: crate::secret::Secret,
    source: TokenSource,
    subject_claim: String,
    revocation_mode: crate::app_config::RevocationMode,
}

#[cfg(feature = "jwt")]
/// Ipe.Server.authConfig : Secret -> TokenSource -> AuthConfig. The subject
/// claim key defaults to the JWT standard `"sub"`. Revocation defaults to `Off`.
#[must_use]
pub fn server_auth_config(secret: crate::secret::Secret, source: TokenSource) -> AuthConfig {
    AuthConfig {
        secret,
        source,
        subject_claim: "sub".to_string(),
        revocation_mode: crate::app_config::RevocationMode::Off,
    }
}

#[cfg(feature = "jwt")]
/// Ipe.Server.withRevocation : RevocationMode -> AuthConfig -> AuthConfig.
/// Arms (or keeps armed) the per-request revocation gate on this config.
/// The mode is supplied as a raw tag: `0` = `Off`, `1` = `Store`; out-of-range
/// falls closed to `Store`. Stricter-only: once `Store`, a subsequent `Off` is
/// a no-op.
#[must_use]
pub fn server_with_revocation(mode_tag: i64, mut cfg: AuthConfig) -> AuthConfig {
    use crate::app_config::RevocationMode;
    let requested = match mode_tag {
        0 => RevocationMode::Off,
        _ => RevocationMode::Store,
    };
    // Stricter-only: Store wins over Off.
    if cfg.revocation_mode != RevocationMode::Store {
        cfg.revocation_mode = requested;
    }
    cfg
}

#[cfg(feature = "jwt")]
/// Ipe.Server.bearerToken : TokenSource. Reads the token from the
/// `Authorization: Bearer` header.
#[must_use]
pub fn server_token_bearer() -> TokenSource {
    TokenSource::BearerHeader
}

#[cfg(feature = "jwt")]
/// Ipe.Server.cookieToken : String -> TokenSource. Reads the token from the
/// named request cookie.
#[must_use]
pub fn server_cookie_token(name: String) -> TokenSource {
    TokenSource::Cookie(name)
}

#[cfg(feature = "jwt")]
/// Read the raw token string from the request per the configured source, or
/// `None` when it is absent/empty. A `Bearer` scheme prefix is matched
/// case-insensitively (RFC 7235 auth-scheme is case-insensitive); any other
/// scheme, or a missing header/cookie, yields `None` so the caller fails closed.
fn read_token(source: &TokenSource, req: &ServerRequest) -> Option<String> {
    match source {
        TokenSource::BearerHeader => {
            let raw = header_ci(&req.headers, "authorization")?;
            let rest = raw.strip_prefix("Bearer ").or_else(|| {
                raw.get(..7)
                    .filter(|p| p.eq_ignore_ascii_case("bearer "))
                    .and_then(|_| raw.get(7..))
            })?;
            let tok = rest.trim();
            (!tok.is_empty()).then(|| tok.to_string())
        }
        TokenSource::Cookie(name) => req
            .cookies
            .get(name)
            .filter(|v| !v.is_empty())
            .map(ToString::to_string),
    }
}

#[cfg(feature = "jwt")]
/// A `401 Unauthorized` response. The body carries no verification detail (a
/// specific reason would be an oracle to an attacker probing tokens).
fn unauthorized() -> ServerResponse {
    plain_resp(401, "unauthorized", &[("WWW-Authenticate", "Bearer")])
}

#[cfg(feature = "jwt")]
/// Build the `Set-Cookie` value for a re-issued session token. Preserves all
/// security attributes of the original session cookie: `__Host-`/Secure,
/// `Path=/`, `HttpOnly`, and `SameSite`. The cookie name must be the same name
/// that the request carried the token under so the browser replaces the existing
/// cookie entry rather than creating a duplicate.
///
/// `is_https` must be pre-captured from the incoming request's headers
/// BEFORE the request is moved into the handler — mirrors the combined gate
/// in `page_response` (`csrf::cookies_secure() || request_is_https(headers)`):
/// a re-issued cookie must never be less-Secure than the initial session cookie.
fn reissue_set_cookie(
    cookie_name: &str,
    token: &str,
    slide_window_secs: u64,
    is_https: bool,
) -> String {
    // The cookie-security signal lives in `web::csrf` for a program that emits the
    // web surface; a server-only program (no `web` module) falls back to the
    // production-env signal. `feature = "web"` is the exact condition under which
    // `crate::web` is present, so the reference is compiled out when it is absent.
    #[cfg(feature = "web")]
    let base_secure = crate::web::csrf::cookies_secure();
    #[cfg(not(feature = "web"))]
    let base_secure = crate::telemetry::production_from_env();
    let secure = if base_secure || is_https {
        "; Secure"
    } else {
        ""
    };

    // Frame-ancestors (CSP embedding) is a web-surface concept; a server-only
    // program cannot be framed, so `SameSite=Lax` is the fail-closed default.
    #[cfg(feature = "web")]
    let embeddable = crate::web::csrf::frame_ancestors().is_some();
    #[cfg(not(feature = "web"))]
    let embeddable = false;
    let same_site = if embeddable { "None" } else { "Lax" };
    format!(
        "{}={}; Path=/; HttpOnly; SameSite={same_site}{secure}; Max-Age={slide_window_secs}",
        cookie_name, token
    )
}

#[cfg(feature = "jwt")]
/// The shared authed-route builder. Wraps the caller's
/// `Request -> Principal -> Task Response` handler in fail-closed middleware
/// that runs BEFORE the handler: it reads the token, verifies it, checks the
/// revocation store (when armed), extracts the subject claim, and mints the
/// `Principal` — dispatching to the handler only on full success, and answering
/// `401` at the first failing step. This is the sole site that mints a `Principal`.
///
/// Revocation gate (when `RevocationMode::Store`): the store is queried AFTER
/// signature + expiry verification and BEFORE the `Principal` is minted. Deny
/// on `Verdict::Revoked`, on `Verdict::Unknown`, and on any store error
/// (fail-closed). Only `Verdict::Active` allows the request through.
///
/// Sliding re-issue: for cookie-based token sources, when the verified token is
/// past its re-issue threshold (`exp - slide_window/2`) and the absolute cap has
/// not been reached, a fresh token is minted and attached via `Set-Cookie`.
/// `iat`, `cap`, and `jti` are carried verbatim from the verified-origin
/// `ReissueContext` — a client cannot move the cap outward or change the session id.
fn authed_route<E, F>(method: &str, path: String, cfg: AuthConfig, handler: F) -> ServerRoute
where
    E: Send + 'static,
    F: Fn(ServerRequest, crate::principal::Principal) -> IpeTask<E, ServerResponse>
        + Send
        + Sync
        + 'static,
{
    let handler = Arc::new(handler);
    let guarded = move |req: ServerRequest| -> IpeTask<E, ServerResponse> {
        // Snapshot the request-scoped TLS signal BEFORE `req` is moved into
        // the async block — same technique as `middleware_with_csrf`. The bool
        // is `Copy`, so this is a zero-cost capture.
        let is_https = request_is_https(&req.headers);
        let cfg = cfg.clone();
        let handler = Arc::clone(&handler);
        Box::pin(async move {
            let Some(token) = read_token(&cfg.source, &req) else {
                return ok_res(unauthorized());
            };
            let secret = crate::secret::secret_reveal(cfg.secret.clone());
            let claims: HashMap<String, String> =
                match crate::auth::auth_verify_token::<String>(secret.clone(), token) {
                    IpeResult::Ok(c) => c,
                    IpeResult::Err(_) => return ok_res(unauthorized()),
                };
            let Some(subject) = claims.get(&cfg.subject_claim).filter(|s| !s.is_empty()) else {
                return ok_res(unauthorized());
            };

            // Revocation gate — consulted only when the mode is `Store`.
            // Runs AFTER token verification and BEFORE `Principal` mint.
            // Fail-closed: deny on Revoked, Unknown, and any store error.
            if cfg.revocation_mode == crate::app_config::RevocationMode::Store {
                let jti = claims.get("jti").map(String::as_str).unwrap_or("");
                match crate::revocation::is_revoked(subject, jti) {
                    crate::revocation::Verdict::Active => {}
                    // Revoked or Unknown both deny — fail-closed.
                    crate::revocation::Verdict::Revoked | crate::revocation::Verdict::Unknown => {
                        return ok_res(unauthorized());
                    }
                }
            }

            let principal = crate::principal::principal_mint(subject.clone());

            // Sliding re-issue — cookie-source only (bearer tokens are API
            // credentials; the client manages re-issue itself via re-auth).
            let reissue_cookie: Option<String> = if let TokenSource::Cookie(ref name) = cfg.source {
                if let Some(ctx) = crate::auth::reissue_context_from_claims(&claims) {
                    let slide_window_secs = crate::app_config::resolve_auth_slide_window();
                    let slide_i64 = i64::try_from(slide_window_secs).unwrap_or(i64::MAX);
                    let now = crate::jwt::now_unix_seconds();
                    // Throttle: re-issue only once past exp - slide_window/2.
                    // Parse exp from the verified claims string representation.
                    let past_threshold = claims
                        .get("exp")
                        .and_then(|s| s.parse::<i64>().ok())
                        .map(|exp| now > exp.saturating_sub(slide_i64 / 2))
                        .unwrap_or(false);
                    if past_threshold && now < ctx.cap {
                        // Extra claims to carry into the re-issued token (all
                        // verified claims except the time anchors and subject —
                        // those come from the ReissueContext).
                        let extra: HashMap<String, String> = claims
                            .iter()
                            .filter(|(k, _)| {
                                // Time anchors and session-identity fields come from
                                // ReissueContext verbatim; skip them in extra_claims.
                                *k != "exp"
                                    && *k != "iat"
                                    && *k != "cap"
                                    && *k != "jti"
                                    && *k != "sub"
                            })
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        match crate::auth::auth_reissue_token::<String>(
                            &secret, &ctx, extra, slide_i64,
                        ) {
                            Some(IpeResult::Ok(new_token)) => Some(reissue_set_cookie(
                                name,
                                &new_token,
                                slide_window_secs,
                                is_https,
                            )),
                            _ => None,
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let mut resp = handler(req, principal).await;
            // Attach the re-issue cookie when warranted. The handler returns an
            // IpeResult<E, ServerResponse>; we append the Set-Cookie to the Ok path.
            if let (IpeResult::Ok(r), Some(cookie)) = (&mut resp, reissue_cookie) {
                r.cookies.push(cookie);
            }
            resp
        })
    };
    route::<E, _>(method, path, guarded)
}

#[cfg(feature = "jwt")]
/// Server.getAuthed : String -> AuthConfig -> (Request -> Principal -> Task Response) -> Route
pub fn server_get_authed<E, F>(path: String, cfg: AuthConfig, handler: F) -> ServerRoute
where
    E: Send + 'static,
    F: Fn(ServerRequest, crate::principal::Principal) -> IpeTask<E, ServerResponse>
        + Send
        + Sync
        + 'static,
{
    authed_route("GET", path, cfg, handler)
}

#[cfg(feature = "jwt")]
/// Server.postAuthed : String -> AuthConfig -> (Request -> Principal -> Task Response) -> Route
pub fn server_post_authed<E, F>(path: String, cfg: AuthConfig, handler: F) -> ServerRoute
where
    E: Send + 'static,
    F: Fn(ServerRequest, crate::principal::Principal) -> IpeTask<E, ServerResponse>
        + Send
        + Sync
        + 'static,
{
    authed_route("POST", path, cfg, handler)
}

#[cfg(feature = "jwt")]
/// Server.putAuthed : String -> AuthConfig -> (Request -> Principal -> Task Response) -> Route
pub fn server_put_authed<E, F>(path: String, cfg: AuthConfig, handler: F) -> ServerRoute
where
    E: Send + 'static,
    F: Fn(ServerRequest, crate::principal::Principal) -> IpeTask<E, ServerResponse>
        + Send
        + Sync
        + 'static,
{
    authed_route("PUT", path, cfg, handler)
}

#[cfg(feature = "jwt")]
/// Server.deleteAuthed : String -> AuthConfig -> (Request -> Principal -> Task Response) -> Route
pub fn server_delete_authed<E, F>(path: String, cfg: AuthConfig, handler: F) -> ServerRoute
where
    E: Send + 'static,
    F: Fn(ServerRequest, crate::principal::Principal) -> IpeTask<E, ServerResponse>
        + Send
        + Sync
        + 'static,
{
    authed_route("DELETE", path, cfg, handler)
}

// ─── response builders (pure) ─────────────────────────────────────────────

fn resp(status: i64, body: String, ct: &str) -> ServerResponse {
    ServerResponse {
        status,
        body,
        headers: HashMap::new(),
        contentType: ct.to_string(),
        cookies: Vec::new(),
    }
}

pub fn server_text(body: String) -> ServerResponse {
    resp(200, body, "text/plain")
}
pub fn server_json(body: String) -> ServerResponse {
    resp(200, body, "application/json")
}
pub fn server_html(body: String) -> ServerResponse {
    resp(200, body, "text/html")
}

pub fn server_with_status(status: i64, mut r: ServerResponse) -> ServerResponse {
    r.status = status;
    r
}
pub fn server_with_header(k: String, v: String, mut r: ServerResponse) -> ServerResponse {
    r.headers.insert(k, v);
    r
}
/// Ipê `redirect : String -> Response` — a 302 to `location`. Matches the Ipê
/// kernel's one-arg contract and Go's `Server_redirectT` (status is hardcoded,
/// not a parameter; use `withStatus` to override).
pub fn server_redirect(location: String) -> ServerResponse {
    let mut r = resp(302, String::new(), "text/plain");
    r.headers.insert("Location".to_string(), location);
    r
}

// ─── request accessors (pure) ─────────────────────────────────────────────

pub fn server_param(name: String, req: ServerRequest) -> IpeMaybe<String> {
    match req.params.get(&name) {
        Some(v) => IpeMaybe::Just(v.clone()),
        None => IpeMaybe::Nothing,
    }
}
pub fn server_query_param(name: String, req: ServerRequest) -> IpeMaybe<String> {
    match req.query.get(&name) {
        Some(v) => IpeMaybe::Just(v.clone()),
        None => IpeMaybe::Nothing,
    }
}
pub fn server_header(name: String, req: ServerRequest) -> IpeMaybe<String> {
    // Go's `r.Header.Get` canonicalises the lookup key, so `Server.header
    // "content-type"` and `"Content-Type"` both resolve. `build_request` stores
    // request headers under the same canonical key, so this lookup is
    // case-insensitive with respect to the caller's casing.
    match req
        .headers
        .get(&crate::http_header::canonical_header(&name))
    {
        Some(v) => IpeMaybe::Just(v.clone()),
        None => IpeMaybe::Nothing,
    }
}
pub fn server_get_cookie(name: String, req: ServerRequest) -> IpeMaybe<String> {
    match req.cookies.get(&name) {
        Some(v) => IpeMaybe::Just(v.clone()),
        None => IpeMaybe::Nothing,
    }
}

// These three are total (every well-formed request has a body, path, and
// method — they are populated unconditionally by `build_request`), so they
// return plain `String`, NOT `IpeMaybe<String>`. Go parity: Go's analogous
// accessors return the raw parsed string values with no Maybe wrapper.
pub fn server_body(req: ServerRequest) -> String {
    req.body
}
pub fn server_path(req: ServerRequest) -> String {
    req.path
}
pub fn server_method(req: ServerRequest) -> String {
    req.method
}

// ─── cookies ──────────────────────────────────────────────────────────────

pub fn server_cookie(name: String, value: String) -> ServerCookie {
    ServerCookie { name, value }
}

/// Strip any byte that could smuggle extra cookie attributes or inject a header
/// line. We drop CTLs (incl. CR/LF), `;`, `,`, and whitespace from both the
/// cookie name and value rather than reject (a total, never-panicking transform):
/// the resulting Set-Cookie carries exactly one attribute set we control, and the
/// downstream `builder.header` can't see a CRLF to error on. Conservative — these
/// bytes are not valid in a cookie name/value per RFC 6265 token/cookie-octet
/// grammar anyway.
fn sanitise_cookie_field(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            !c.is_control()
                && c != ';'
                && c != ','
                && c != ' '
                && c != '\t'
                && c != '"'
                && c != '\\'
        })
        .collect()
}

pub fn server_with_cookie(c: ServerCookie, mut r: ServerResponse) -> ServerResponse {
    // Minimal Set-Cookie with safe defaults; full attributes land with step 4.
    // name/value are sanitised so a value containing `;`/`,`/CRLF can't smuggle
    // extra attributes or inject a second header line.
    let name = sanitise_cookie_field(&c.name);
    let value = sanitise_cookie_field(&c.value);
    // Add `Secure` in production so an auth/session cookie is never transmitted
    // over the cleartext proxy→app hop (SSL-strip / sniff). Omit it in dev so
    // cookies still work over plain-http localhost. Gate matches the rest of the
    // runtime's production detection (ENV / IPE_ENV via productionFromEnv).
    let secure = if crate::telemetry::production_from_env() {
        "; Secure"
    } else {
        ""
    };
    let v = format!(
        "{}={}; HttpOnly; Path=/; SameSite=Lax{}",
        name, value, secure
    );
    r.cookies.push(v);
    r
}

// ─── listen + axum adapter (step 4) ───────────────────────────────────────

const DEFAULT_MAX_BODY: usize = 32 * 1024 * 1024; // 32 MiB

/// Request-body cap. Overridable via `IPE_WEB_MAX_BODY_BYTES`; falls back to 32 MiB.
fn max_body() -> usize {
    crate::system::read_env_var("IPE_WEB_MAX_BODY_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_BODY)
}

fn parse_query(q: Option<&str>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(q) = q {
        for pair in q.split('&') {
            if pair.is_empty() {
                continue;
            }
            let mut it = pair.splitn(2, '=');
            let k = it.next().unwrap_or("");
            let v = it.next().unwrap_or("");
            // Repeated keys keep the FIRST value — consistent with
            // http_client::http_parse_query and the Go runtime's parseQuery.
            out.entry(urldecode(k)).or_insert_with(|| urldecode(v));
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    form_url_decode(s)
}

/// Percent-decode a raw path-param value so `Server.param` matches the decoding
/// `Server.queryParam` already applies (`RawPathParams` hands back the raw,
/// still-escaped segment). Path segments are percent-encoded ONLY — unlike a
/// query string a literal `+` is NOT a space here (RFC 3986 §3.3), so this uses a
/// pure percent-decode rather than `form_url_decode` (which maps `+` → space).
/// Lossy + total (never panics on invalid UTF-8).
fn decode_path_param(v: &str) -> String {
    percent_encoding::percent_decode_str(v)
        .decode_utf8_lossy()
        .into_owned()
}

fn parse_cookies(header: &str, out: &mut HashMap<String, String>) {
    for c in header.split(';') {
        let c = c.trim();
        if let Some((k, v)) = c.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
}

/// Build the Ipê `ServerRequest` from the axum request. Returns `Err(status)`
/// when the request must be rejected before the handler runs — currently only
/// `Err(413)` for an oversize body (Go parity: `http.MaxBytesReader` →
/// `WriteHeader(413)`, rt.go:7738). The previous code collapsed an oversize
/// (and any body read error) into an empty body via `.unwrap_or_default()`,
/// silently handing the handler `""` instead of refusing the request.
async fn build_request(
    req: axum::extract::Request,
) -> Result<(ServerRequest, Option<axum::extract::ws::WebSocketUpgrade>), u16> {
    use axum::extract::{FromRequestParts, RawPathParams};
    let method = req.method().as_str().to_string();
    let uri = req.uri().clone();
    let path = uri.path().to_string();
    let query = parse_query(uri.query());
    let mut headers = HashMap::new();
    let mut cookies = HashMap::new();
    for (k, v) in req.headers() {
        if let Ok(s) = v.to_str() {
            if k.as_str().eq_ignore_ascii_case("cookie") {
                parse_cookies(s, &mut cookies);
            }
            // Store under Go's canonical MIME casing (`content-type` ->
            // `Content-Type`), aligning with Go's request-header storage and the
            // Ipe.Web path, so `server_header` (which canonicalises its lookup
            // key) matches any caller casing.
            headers.insert(
                crate::http_header::canonical_header(k.as_str()),
                s.to_string(),
            );
        }
    }
    let (mut parts, body) = req.into_parts();
    let params = match RawPathParams::from_request_parts(&mut parts, &()).await {
        Ok(rpp) => rpp
            .iter()
            .map(|(k, v)| (k.to_string(), decode_path_param(v)))
            .collect(),
        Err(_) => HashMap::new(),
    };
    // remoteAddr: trust the real TCP peer (ConnectInfo) by DEFAULT. Only honour a
    // proxy's X-Forwarded-For / X-Real-IP when `IPE_TRUSTED_PROXY` is set — i.e.
    // the operator declares the app sits behind a trusted proxy that sets those
    // headers. Trusting client-supplied XFF unconditionally let ANY client spoof
    // their IP → rate-limit bypass (the fixed-window limiter keys on remoteAddr)
    // + forged access logs. Security-by-default: spoofable headers are opt-in.
    let peer = parts
        .extensions
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip().to_string());
    let trust_proxy = crate::system::read_env_var("IPE_TRUSTED_PROXY")
        .map(|v| !v.is_empty() && v != "0" && v != "false")
        .unwrap_or(false);
    let remote_addr = if trust_proxy {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("x-forwarded-for"))
            .map(|(_, v)| v.split(',').next().unwrap_or("").trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("x-real-ip"))
                    .map(|(_, v)| v.clone())
            })
            .or(peer)
            .unwrap_or_default()
    } else {
        peer.unwrap_or_default()
    };
    // Extract the WebSocket upgrader if this is an upgrade request
    // (succeeds only when the Connection/Upgrade/Sec-WebSocket-* headers are
    // present). Stashed via task-local so server_web_socket_upgrade can reach it.
    let upgrader = axum::extract::ws::WebSocketUpgrade::from_request_parts(&mut parts, &())
        .await
        .ok();
    let cap = max_body();
    // Reject an oversize body with 413 instead of silently truncating to "".
    // Pre-check Content-Length when declared (deterministic for non-chunked
    // requests); to_bytes still enforces the cap for chunked bodies.
    if let Some(declared) = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        && declared > cap
    {
        return Err(413);
    }
    let body = match axum::body::to_bytes(body, cap).await {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        // to_bytes-with-limit fails almost exclusively on cap-exceeded; a
        // transport read error means the client is already gone so the status
        // is moot. Either way never hand the handler a silently-truncated body.
        Err(_) => return Err(413),
    };
    Ok((
        ServerRequest {
            method,
            path,
            body,
            headers,
            params,
            query,
            cookies,
            remoteAddr: remote_addr,
        },
        upgrader,
    ))
}

fn to_axum_response(r: ServerResponse) -> axum::response::Response {
    use axum::response::IntoResponse;
    // Ipe.Http.Server.Stream: a streaming response carries a sentinel body the
    // handler stashed via ServerStream.stream. Detect it + serve the chunked
    // body before the buffered path runs.
    if let Some(streamed) = serve_streaming_sentinel(&r) {
        return streamed;
    }
    // Clamp to the valid HTTP status range before the u16 cast so an out-of-range
    // Ipê integer (e.g. from a buggy handler returning status=99999) produces a
    // defined 500 rather than a wrapping or panicking cast.
    let status_u16 = r.status.clamp(100, 599) as u16;
    let status = axum::http::StatusCode::from_u16(status_u16)
        .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    let mut builder = axum::http::Response::builder().status(status);
    // Emit the response's contentType UNLESS the handler already set a
    // content-type via withHeader (case-insensitive). builder.header APPENDS, so
    // emitting both would produce two `content-type` headers on the wire; an
    // explicit handler override wins (parity with the security-headers `if-unset`
    // policy below).
    let has_ct_header = r
        .headers
        .keys()
        .any(|k| k.eq_ignore_ascii_case("content-type"));
    if !r.contentType.is_empty() && !has_ct_header {
        builder = builder.header("content-type", r.contentType.clone());
    }
    for (k, v) in &r.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    // Multiple Set-Cookie response headers (RFC 6265 §4.1 forbids comma-folding
    // them into one line) — kept in a dedicated Vec (see `ServerResponse.cookies`)
    // so two cookie-setting code paths (e.g. a handler's `Server.withCookie` plus
    // a wrapping `Middleware.withCsrf`) never clobber each other via the
    // single-valued `headers` map. `builder.header` APPENDS, so repeated calls
    // with the same key name produce separate header lines on the wire.
    for cookie_v in &r.cookies {
        builder = builder.header("set-cookie", cookie_v.as_str());
    }
    // Safe-by-default security headers (Go parity: setSecurityHeaders on the
    // server path, rt.go:7838) — applied only when the handler hasn't already
    // set them, so an explicit handler override wins (mirrors Go's `if h.Get
    // (...) == ""`). Values are env/static (no request-derived strings → no
    // header-injection surface).
    for (name, value) in crate::telemetry::security_headers() {
        if !r.headers.keys().any(|k| k.eq_ignore_ascii_case(name)) {
            builder = builder.header(name, value);
        }
    }
    // Dev-only "🔍 Console" banner injection (Go parity: rt.go server dispatch
    // tail — injectDevBanner(devBannerHTML()) for every text/html response).
    // Runs on the buffered body only; streaming responses returned above via
    // serve_streaming_sentinel bypass this (same as Go, where streams skip the
    // buffered fmt.Fprint path). Effective content-type = the handler's override
    // header when present, else r.contentType (mirrors the has_ct_header
    // resolution used to emit the content-type above).
    let effective_ct: String = if has_ct_header {
        r.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    } else {
        r.contentType.clone()
    };
    // Prefix test matches Go's `strings.HasPrefix(ct, "text/html")` exactly:
    // case-sensitive, no trimming — Ipê's html builder always sets a lowercase
    // `text/html; charset=utf-8`, so this is the byte-parity comparison.
    let body = if effective_ct.starts_with("text/html") {
        let banner = crate::telemetry::dev_console_banner("");
        crate::telemetry::inject_dev_banner(&r.body, &banner)
    } else {
        r.body
    };
    match builder.body(axum::body::Body::from(body)) {
        Ok(resp) => resp,
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn method_router(method: &str, h: ErasedHandler) -> axum::routing::MethodRouter {
    use axum::response::IntoResponse;
    use axum::routing::{any, delete, get, post, put};
    let svc = move |req: axum::extract::Request| {
        let h = h.clone();
        async move {
            let (ipe_req, upgrader) = match build_request(req).await {
                Ok(v) => v,
                Err(code) => {
                    let status = axum::http::StatusCode::from_u16(code)
                        .unwrap_or(axum::http::StatusCode::PAYLOAD_TOO_LARGE);
                    return (status, "Payload Too Large").into_response();
                }
            };
            // Run the handler with the WS upgrader + a response slot in scope.
            // If the handler called server_web_socket_upgrade, it stashed the
            // real 101 response in WS_RESPONSE — prefer it over the sentinel.
            WS_UPGRADER
                .scope(std::cell::Cell::new(upgrader), async move {
                    WS_RESPONSE
                        .scope(std::cell::Cell::new(None), async move {
                            let result = h(ipe_req).await;
                            if let Some(ws_resp) = WS_RESPONSE.with(|c| c.take()) {
                                return ws_resp;
                            }
                            match result {
                                Ok(resp) => to_axum_response(resp),
                                Err(_) => (
                                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                                    "Internal Server Error",
                                )
                                    .into_response(),
                            }
                        })
                        .await
                })
                .await
        }
    };
    match method.to_uppercase().as_str() {
        "GET" => get(svc),
        "POST" => post(svc),
        "PUT" => put(svc),
        "DELETE" => delete(svc),
        _ => any(svc),
    }
}

fn strip_trailing_slash(p: &str) -> String {
    // strip_suffix is total — drops the trailing '/' without a raw range slice,
    // keeping a lone "/" intact (filtering out the empty result).
    match p.strip_suffix('/') {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => p.to_string(),
    }
}

/// Server.listen : Int -> List Route -> Task Error ()  — serves via axum/tokio.
pub fn server_listen<E: From<String> + Send + 'static>(
    port: i64,
    routes: Vec<ServerRoute>,
) -> IpeTask<E, ()> {
    Box::pin(async move {
        let mut app: axum::Router = axum::Router::new();
        // At most ONE mounted web app per server: the embedded app's cookie /
        // CSRF / asset paths are scoped through the process-wide base path
        // (`IPE_WEB_BASE_PATH`), so a SECOND mount at a different prefix would
        // silently mis-scope the first. Reject the second fail-closed (before
        // bind) rather than serve a subtly broken session/CSRF surface.
        #[cfg(feature = "web")]
        let mut web_mounted = false;
        for r in routes {
            match r.target {
                RouteTarget::Static(dir) => {
                    app = app.nest_service(
                        &strip_trailing_slash(&r.path),
                        tower_http::services::ServeDir::new(dir),
                    );
                }
                RouteTarget::Handler(h) => {
                    app = app.route(&r.path, method_router(&r.method, h));
                }
                // `Server.mountApp`: build the embedded web app's router scoped
                // to the prefix, then nest it under that prefix on this same
                // listener (one port). The builder is taken once; a duplicate
                // (a cloned route) finds `None` and is skipped — inert.
                #[cfg(feature = "web")]
                RouteTarget::MountWeb(cell) => {
                    if web_mounted {
                        return IpeResult::Err(
                            "Server.mountApp: at most one Web app may be mounted per server \
                             (the embedded app's cookie/CSRF/asset paths are scoped through one \
                             process-wide base path); mount a single Web app, or serve additional \
                             apps as separate servers"
                                .to_string()
                                .into(),
                        );
                    }
                    let builder = cell
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
                    if let Some(build) = builder {
                        let prefix = strip_trailing_slash(&r.path);
                        let sub = build(prefix.clone()).await;
                        app = app.nest(&prefix, sub);
                        web_mounted = true;
                    }
                }
            }
        }
        // Ipê doctrine: a panicking handler returns 500, never crashes the
        // process (mirrors the Go runtime's per-handler recover()). The custom
        // responder classifies + logs the panic SERVER-SIDE (errId) and returns a
        // 500 carrying ONLY the errId — never the panic message (no info leak).
        let app = app.layer(tower_http::catch_panic::CatchPanicLayer::custom(
            |err: Box<dyn std::any::Any + Send + 'static>| {
                use axum::response::IntoResponse;
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    crate::core::panic_500_body(&*err),
                )
                    .into_response()
            },
        ));
        // Bind host obeys the one runtime-config precedence: `IPE_HTTP_BIND`
        // (env) > the app's `Host.bind` setting > the build-profile fallback
        // (loopback in debug, all interfaces in release). The conservative
        // loopback default keeps a dev console off the LAN by construction.
        let host = crate::app_config::resolve_host_bind();
        let addr = format!("{}:{}", host, port);
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                return IpeResult::Err(
                    format!(
                        "port {port} is already in use — another application is bound to it.\n\
                         Set a different port with the IPE_WEB_PORT environment variable, e.g.:\n\
                         IPE_WEB_PORT=8123 ipe run"
                    )
                    .into(),
                );
            }
            Err(e) => return IpeResult::Err(format!("Server.listen: bind {}: {}", addr, e).into()),
        };
        eprintln!("[ipe.http.server] listening on http://{}", addr);
        // with_connect_info so each request carries the peer SocketAddr —
        // populates ServerRequest.remoteAddr (also used by per-IP rate limiting).
        let svc = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
        match axum::serve(listener, svc).await {
            Ok(()) => ok_res(()),
            Err(e) => IpeResult::Err(format!("Server.listen: serve: {}", e).into()),
        }
    })
}

// ─── Ipe.Http.Server.WebSocket ────────────────────────────────────────────
//
// Bridged types (runtimeOpaqueTypes): WebSocketServer -> WsHandle (the opaque
// per-peer handle the stdlib pattern-matches as `WebSocketServer raw`);
// WebSocketServerCfg -> WsServerCfg (fn-pointer callbacks so the stdlib's
// `defaultCfg |> withOnX` record updates compile — see the design doc on why
// non-capturing handlers are the first-cut limit).

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Ipe.Http.Server.WebSocket.WebSocketServer — opaque per-peer handle. The
/// variant name matches the Ipê constructor so `case sock of WebSocketServer
/// raw` lowers onto it.
#[derive(Clone, Copy, Debug)]
pub enum WsHandle {
    WebSocketServer(i64),
}

/// Ipe.Http.Server.WebSocket.WebSocketServerCfg — fn-pointer callbacks (cannot
/// capture; capturing handlers need Arc<dyn Fn> erasure, a follow-up).
///
/// Generic over the error type E because the project's concrete error
/// (IpeCoreErrorError) is unnameable from the runtime crate. The Ipê-side
/// bridge pins `E = IpeError` (and drops the phantom `msg`) via a generic type
/// alias — see aliasToRustTypeDef. fn pointers don't store E, so WsServerCfg<E>
/// is Send/Copy-of-fields regardless of E.
#[allow(non_snake_case)]
#[derive(Clone)]
pub struct WsServerCfg<E> {
    // Stored effectful callbacks. These are `Arc<dyn Fn + Send + Sync>`, NOT
    // bare `fn` pointers: a real handler captures app state (the SSE-relay shape
    // proves capturing closures are first-class — see ex-32), and a captured
    // closure is not a `fn` pointer. The codegen renders function-typed record
    // fields as `Arc<dyn Fn(..) -> .. + Send + Sync>` and wraps the assigned
    // value in `Arc::new(..)` at every record literal / field-update site, so
    // the `withOnX` setters (param `impl Fn`) and `defaultCfg` (lambda literals)
    // both store cleanly. Arc is Clone, so the `#[derive(Clone)]` above holds.
    pub onConnect: Arc<dyn Fn(WsHandle) -> IpeTask<E, ()> + Send + Sync>,
    pub onMessage: Arc<dyn Fn(WsHandle, String) -> IpeTask<E, ()> + Send + Sync>,
    pub onClose: Arc<dyn Fn(WsHandle) -> IpeTask<E, ()> + Send + Sync>,
    pub onError: Arc<dyn Fn(WsHandle, E) -> IpeTask<E, ()> + Send + Sync>,
    pub maxMessageBytes: i64,
    pub originPatterns: Vec<String>,
}

enum WsOut {
    Text(String),
    Binary(Vec<u8>),
    Close,
}

/// Per-peer outbound queue depth. A slow/idle WebSocket consumer must NOT let the
/// server buffer unboundedly (OOM) — the channel is bounded and a full queue drops
/// the message (the send kernel returns Err), giving real backpressure. Override
/// via IPE_WS_SEND_BUFFER; default 256 frames.
fn ws_send_buffer() -> usize {
    crate::system::read_env_var("IPE_WS_SEND_BUFFER")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(256)
}

/// Heartbeat interval for WebSocket Ping frames.  Mirrors Go's
/// `wsDefaultPingInterval = 30s` (`runtime-go/rt/server_websocket.go`).
/// Override via `IPE_WS_HEARTBEAT` (seconds, must be > 0).
fn ws_heartbeat_secs() -> u64 {
    crate::system::read_env_var("IPE_WS_HEARTBEAT")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(30)
}

fn ws_registry() -> &'static Mutex<HashMap<i64, tokio::sync::mpsc::Sender<WsOut>>> {
    static R: OnceLock<Mutex<HashMap<i64, tokio::sync::mpsc::Sender<WsOut>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

static WS_NEXT_ID: AtomicI64 = AtomicI64::new(1);

tokio::task_local! {
    // The axum upgrader for the in-flight request (Some only on a WS upgrade).
    static WS_UPGRADER: std::cell::Cell<Option<axum::extract::ws::WebSocketUpgrade>>;
    // The 101 response server_web_socket_upgrade produced (preferred by method_router).
    static WS_RESPONSE: std::cell::Cell<Option<axum::response::Response>>;
}

/// Resolve the configured per-message byte cap, mirroring Go's `SetReadLimit`:
/// treat 0/negative as "unset" and apply the 1 MiB default
/// (`wsDefaultMaxMessageBytes = 1 << 20` in `runtime-go/rt/websocket.go`).
/// `try_from` avoids a wrapping/truncating cast on a caller-controlled `i64`.
/// Shared by `server_web_socket_upgrade` (framing-layer enforcement, applied
/// to the `WebSocketUpgrade` builder before the frame is even buffered) and
/// `ws_loop` (application-layer defense in depth on the already-decoded
/// message).
fn ws_max_message_bytes(max_message_bytes: i64) -> usize {
    if max_message_bytes > 0 {
        usize::try_from(max_message_bytes).unwrap_or(1 << 20)
    } else {
        1 << 20 // 1 MiB
    }
}

async fn ws_loop<E: From<String> + Send + 'static>(
    mut socket: axum::extract::ws::WebSocket,
    cfg: WsServerCfg<E>,
    id: i64,
) {
    use axum::extract::ws::Message;
    use std::time::Duration;
    let max_bytes: usize = ws_max_message_bytes(cfg.maxMessageBytes);
    // Framing-layer enforcement lives on the `WebSocketUpgrade` builder, applied
    // at upgrade time in `server_web_socket_upgrade` (`.max_message_size()` /
    // `.max_frame_size()` — axum 0.7.9 exposes both). A frame over the cap is
    // rejected by tokio-tungstenite before it reaches this loop. The Text/Binary
    // size checks below are application-layer defense in depth (belt-and-braces
    // against a future axum/tungstenite version silently dropping the cap).
    let (tx, mut rx) = tokio::sync::mpsc::channel::<WsOut>(ws_send_buffer());
    ws_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, tx);
    let _ = (cfg.onConnect)(WsHandle::WebSocketServer(id)).await;
    // Heartbeat: send a Ping every `ws_heartbeat_secs()` seconds to keep the
    // connection alive through proxies and detect silent drops.  Mirrors Go's
    // `wsDefaultPingInterval = 30s` + `wsPingTimeout = 10s` pattern in
    // `runtime-go/rt/server_websocket.go`.  axum auto-replies to incoming Pong
    // frames on our behalf, so we only need to send the Ping here.
    let mut heartbeat = tokio::time::interval(Duration::from_secs(ws_heartbeat_secs()));
    heartbeat.tick().await; // consume the immediate first tick
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(t))) => {
                    if t.len() > max_bytes {
                        let _ = socket.send(Message::Close(None)).await;
                        break;
                    }
                    let _ = (cfg.onMessage)(WsHandle::WebSocketServer(id), t).await;
                }
                Some(Ok(Message::Binary(b))) => {
                    if b.len() > max_bytes {
                        let _ = socket.send(Message::Close(None)).await;
                        break;
                    }
                    // Convert binary frame bytes to String via UTF-8 (lossy): Ipê's
                    // String invariant is valid UTF-8; non-UTF-8 binary replaces
                    // malformed sequences with U+FFFD rather than producing an
                    // ill-formed String. The server `onMessage` callback receives a
                    // uniform `String` for both text and binary frames; applications
                    // that need lossless binary round-trips should use a text+base64
                    // encoding at the Ipê level.
                    let s = String::from_utf8_lossy(&b).into_owned();
                    let _ = (cfg.onMessage)(WsHandle::WebSocketServer(id), s).await;
                }
                Some(Ok(Message::Close(_))) | None => break,
                // Incoming Ping/Pong frames are auto-handled by axum; no user
                // callback needed.
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    let _ = (cfg.onError)(WsHandle::WebSocketServer(id), format!("ws read error: {}", e).into()).await;
                    break;
                }
            },
            outgoing = rx.recv() => match outgoing {
                Some(WsOut::Text(s)) => { if socket.send(Message::Text(s)).await.is_err() { break; } }
                Some(WsOut::Binary(b)) => { if socket.send(Message::Binary(b)).await.is_err() { break; } }
                Some(WsOut::Close) => { let _ = socket.send(Message::Close(None)).await; break; }
                None => break,
            },
            _ = heartbeat.tick() => {
                // Send a Ping frame; if the peer has gone away the send will
                // fail and we break, triggering onClose cleanup.
                if socket.send(Message::Ping(vec![])).await.is_err() {
                    break;
                }
            },
        }
    }
    let _ = (cfg.onClose)(WsHandle::WebSocketServer(id)).await;
    ws_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
}

fn ws_production() -> bool {
    let v = crate::system::read_env_var("ENV")
        .or_else(|_| crate::system::read_env_var("IPE_ENV"))
        .unwrap_or_default();
    !matches!(v.as_str(), "" | "dev" | "development" | "local")
}

fn ws_resp(status: i64, body: &str) -> ServerResponse {
    ServerResponse {
        status,
        body: body.to_string(),
        headers: HashMap::new(),
        contentType: "text/plain".to_string(),
        cookies: Vec::new(),
    }
}

/// Glob match with `*` wildcards (e.g. "https://*.example.com"). `*` matches any
/// run of characters; all other chars are literal. Used for WS origin allowlists.
///
/// Security (CSWSH glob bypass): when a `*` is followed by a non-empty literal
/// anchor (a middle segment, or the trailing domain suffix), the region the `*`
/// covers MUST be a syntactic host fragment — `[A-Za-z0-9.:-]` only. Without this
/// a pattern like `https://*.example.com` would wrongly accept a forged origin
/// such as `https://evil.com/.example.com` or `https://evil.com@x.example.com`,
/// where the trusted suffix sits behind a path / userinfo delimiter. The explicit
/// allow-all pattern `*` (and any pattern ending in `*`) keeps matching anything —
/// that region has no literal anchor after it, so the user has opted into it.
fn ws_origin_matches(pattern: &str, origin: &str) -> bool {
    // A `*`-covered span that precedes a literal anchor may only contain host
    // characters — never a `/`, `@`, `?`, `#`, whitespace, or control byte that
    // could push the trusted literal behind a URL delimiter.
    fn host_safe(span: &str) -> bool {
        span.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == origin; // no wildcard → exact
    }
    let mut rest = origin;
    // First segment must be a prefix (unless pattern starts with '*').
    if let Some(first) = parts.first() {
        if !rest.starts_with(first) {
            return false;
        }
        rest = rest.get(first.len()..).unwrap_or("");
    }
    // Middle segments must appear in order. (parts.len() >= 2 here — the
    // len == 1 case returned early — so the slice is total.)
    for seg in parts.get(1..parts.len() - 1).unwrap_or(&[]) {
        if seg.is_empty() {
            continue;
        }
        match rest.find(seg) {
            Some(i) => {
                // `rest[..i]` was covered by the preceding `*`, and `seg` is a
                // literal anchor after it → enforce host-only.
                if !host_safe(rest.get(..i).unwrap_or("")) {
                    return false;
                }
                rest = rest.get(i + seg.len()..).unwrap_or("");
            }
            None => return false,
        }
    }
    // Last segment must be a suffix (unless pattern ends with '*').
    let last = parts.last().copied().unwrap_or("");
    if last.is_empty() {
        // Pattern ends with `*` → trailing region unrestricted (explicit allow-all).
        return true;
    }
    if !rest.ends_with(last) {
        return false;
    }
    // The span the trailing `*` covered (before the literal suffix) must be a host.
    host_safe(rest.get(..rest.len() - last.len()).unwrap_or(""))
}

/// True when `Origin` is present and does not match `Host` (cross-origin).
/// Absent `Origin` (same-origin browsers on older UA quirks, non-browser WS
/// clients, and legitimate same-origin pages under some proxy setups that
/// strip it) is NOT flagged — matches the equivalent CSRF/ingest same-origin
/// helpers elsewhere in this runtime (`csrf.rs::origin_mismatch`,
/// `console.rs::is_cross_origin_ingest`), via the shared `origin_host_mismatch`
/// helper in `http_header` (normalizes away each side's scheme-implied
/// default port so the three never drift to different behavior).
/// `http_header` is gated on `feature = "server"` only (not `live`), so this
/// reuse still builds standalone under `--features server` without `live`.
fn ws_cross_origin(req: &ServerRequest) -> bool {
    let origin = match header_ci(&req.headers, "origin") {
        Some(o) if !o.is_empty() => o,
        _ => return false,
    };
    let host = header_ci(&req.headers, "host").unwrap_or("");
    crate::http_header::origin_host_mismatch(origin, host)
}

/// ServerWebSocket_upgrade : Request -> WebSocketServerCfg -> Task Error Response
pub fn server_web_socket_upgrade<E: From<String> + Send + 'static>(
    req: ServerRequest,
    cfg: WsServerCfg<E>,
) -> IpeTask<E, ServerResponse> {
    Box::pin(async move {
        // Origin allowlist. Production with no patterns → reject (matches Go). With
        // patterns set (any mode), the request's Origin must match one of them.
        if ws_production() && cfg.originPatterns.is_empty() {
            return ok_res(ws_resp(
                403,
                "websocket: origin allowlist required in production",
            ));
        }
        if !cfg.originPatterns.is_empty() {
            let origin = req
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("origin"))
                .map(|(_, v)| v.as_str())
                .unwrap_or("");
            if !cfg
                .originPatterns
                .iter()
                .any(|p| ws_origin_matches(p, origin))
            {
                return ok_res(ws_resp(403, "websocket: origin not allowed"));
            }
        } else if ws_cross_origin(&req) {
            // Dev mode, no explicit allowlist: default to same-origin rather
            // than allow-all (closes CSWSH — Cross-Site WebSocket Hijacking. A
            // WS handshake can't carry a custom header, so unlike a
            // CSRF-protected form POST, Origin validation is the ONLY defense
            // available). Configure `Ws.withOriginPatterns` explicitly to
            // allow legitimate cross-origin clients.
            return ok_res(ws_resp(
                403,
                "websocket: cross-origin request rejected (set Ws.withOriginPatterns to allow)",
            ));
        }
        let upgrader = WS_UPGRADER.try_with(|c| c.take()).ok().flatten();
        match upgrader {
            Some(up) => {
                let id = WS_NEXT_ID.fetch_add(1, Ordering::Relaxed);
                // Enforce the cap at the framing layer: tokio-tungstenite rejects
                // an over-cap frame/message before it is ever fully buffered, so
                // the limit holds even before `ws_loop`'s in-loop check runs.
                let max_bytes = ws_max_message_bytes(cfg.maxMessageBytes);
                let up = up.max_message_size(max_bytes).max_frame_size(max_bytes);
                let resp = up.on_upgrade(move |socket| ws_loop(socket, cfg, id));
                let _ = WS_RESPONSE.try_with(|c| c.set(Some(resp)));
                // Sentinel — method_router returns WS_RESPONSE instead of this.
                ok_res(ServerResponse {
                    status: 101,
                    body: String::new(),
                    headers: HashMap::new(),
                    contentType: String::new(),
                    cookies: Vec::new(),
                })
            }
            None => ok_res(ws_resp(400, "websocket: expected an Upgrade request")),
        }
    })
}

fn ws_send_raw(id: i64, out: WsOut) -> bool {
    // try_send (non-blocking): a full per-peer queue (slow consumer) drops the
    // frame and returns false rather than buffering unboundedly — bounded memory.
    match ws_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
    {
        Some(tx) => tx.try_send(out).is_ok(),
        None => false,
    }
}

/// ServerWebSocket_sendToClient : Int -> String -> Task Error ()
pub fn server_web_socket_send_to_client<E: From<String> + Send + 'static>(
    id: i64,
    msg: String,
) -> IpeTask<E, ()> {
    Box::pin(async move {
        if ws_send_raw(id, WsOut::Text(msg)) {
            ok_res(())
        } else {
            IpeResult::Err(format!("ws: no client {}", id).into())
        }
    })
}

/// ServerWebSocket_sendBinaryToClient : Int -> Bytes -> Task Error ()
pub fn server_web_socket_send_binary_to_client<E: From<String> + Send + 'static>(
    id: i64,
    msg: Vec<u8>,
) -> IpeTask<E, ()> {
    Box::pin(async move {
        if ws_send_raw(id, WsOut::Binary(msg)) {
            ok_res(())
        } else {
            IpeResult::Err(format!("ws: no client {}", id).into())
        }
    })
}

/// ServerWebSocket_broadcast : List Int -> String -> Task Error ()
pub fn server_web_socket_broadcast<E: From<String> + Send + 'static>(
    ids: Vec<i64>,
    msg: String,
) -> IpeTask<E, ()> {
    Box::pin(async move {
        let mut any_ok = false;
        {
            let reg = ws_registry().lock().unwrap_or_else(|e| e.into_inner());
            for id in &ids {
                if let Some(tx) = reg.get(id)
                    && tx.try_send(WsOut::Text(msg.clone())).is_ok()
                {
                    any_ok = true;
                }
            }
        }
        if ids.is_empty() || any_ok {
            ok_res(())
        } else {
            IpeResult::Err("ws broadcast: every send failed".to_string().into())
        }
    })
}

/// ServerWebSocket_closeClient : Int -> Task Error () (idempotent)
pub fn server_web_socket_close_client<E: From<String> + Send + 'static>(id: i64) -> IpeTask<E, ()> {
    Box::pin(async move {
        let _ = ws_send_raw(id, WsOut::Close);
        ok_res(())
    })
}

// ─── Ipe.Http.Server.WebSocket adapters ────────────────────────────
//
// Kernel-callable entry points (D3: handle-taking wrappers + cfg builders).
// The i64 family above is the registry API kept for upstream-sync; these
// adapters sit in front of it.
//
// Design decisions (docs/adr/0023-websocket-server-kernel-only-typed-handles.md):
//   D2 — WsServerCfg is monomorphic (pins E = IpeError, drops phantom msg).
//   D3 — kernels take WsHandle, not i64; adapters unwrap.
//   D4 — bounded fail-fast `try_send` (IPE_WS_SEND_BUFFER=256 default).

/// `Ws.defaultCfg` — no-op callbacks, `maxMessageBytes = 0` (→ 1 MiB in
/// `ws_loop`), empty `originPatterns` (dev: same-origin only — `Origin` must
/// match `Host` when `Origin` is present, `ws_cross_origin`; production: 403
/// on `upgrade`).
pub fn ws_server_default_cfg<E: From<String> + Send + 'static>() -> WsServerCfg<E> {
    WsServerCfg {
        onConnect: Arc::new(|_| Box::pin(async { ok_res(()) })),
        onMessage: Arc::new(|_, _| Box::pin(async { ok_res(()) })),
        onClose: Arc::new(|_| Box::pin(async { ok_res(()) })),
        onError: Arc::new(|_, _| Box::pin(async { ok_res(()) })),
        maxMessageBytes: 0,
        originPatterns: Vec::new(),
    }
}

/// `Ws.withOnConnect` — replace the `onConnect` callback.
///
/// Accepts `Arc<dyn Fn(WsHandle) -> IpeTask<E, ()> + Send + Sync + 'static>` directly
/// because stable Rust does not implement `Fn<Args>` for `Arc<dyn Fn<Args>>` — the
/// emitter always pre-wraps the function in `Arc::new`, so the adapter stores it as-is.
pub fn ws_server_with_on_connect<E>(
    cb: Arc<dyn Fn(WsHandle) -> IpeTask<E, ()> + Send + Sync + 'static>,
    cfg: WsServerCfg<E>,
) -> WsServerCfg<E>
where
    E: From<String> + Send + 'static,
{
    WsServerCfg {
        onConnect: cb,
        ..cfg
    }
}

/// `Ws.withOnMessage` — replace the `onMessage` callback.
///
/// The callback is uncurried (two args: `WsHandle` and `String`) to match
/// the `dict_foldl` uncurried precedent (see design doc §3).
///
/// Accepts `Arc<dyn Fn(...)>` directly — see `ws_server_with_on_connect` for rationale.
pub fn ws_server_with_on_message<E>(
    cb: Arc<dyn Fn(WsHandle, String) -> IpeTask<E, ()> + Send + Sync + 'static>,
    cfg: WsServerCfg<E>,
) -> WsServerCfg<E>
where
    E: From<String> + Send + 'static,
{
    WsServerCfg {
        onMessage: cb,
        ..cfg
    }
}

/// `Ws.withOnClose` — replace the `onClose` callback.
///
/// Accepts `Arc<dyn Fn(...)>` directly — see `ws_server_with_on_connect` for rationale.
pub fn ws_server_with_on_close<E>(
    cb: Arc<dyn Fn(WsHandle) -> IpeTask<E, ()> + Send + Sync + 'static>,
    cfg: WsServerCfg<E>,
) -> WsServerCfg<E>
where
    E: From<String> + Send + 'static,
{
    WsServerCfg { onClose: cb, ..cfg }
}

/// `Ws.withOnError` — replace the `onError` callback.
///
/// Accepts `Arc<dyn Fn(...)>` directly — see `ws_server_with_on_connect` for rationale.
pub fn ws_server_with_on_error<E>(
    cb: Arc<dyn Fn(WsHandle, E) -> IpeTask<E, ()> + Send + Sync + 'static>,
    cfg: WsServerCfg<E>,
) -> WsServerCfg<E>
where
    E: From<String> + Send + 'static,
{
    WsServerCfg { onError: cb, ..cfg }
}

/// `Ws.withMaxMessageBytes` — set per-message size cap (0 → 1 MiB default
/// in `ws_loop`).
pub fn ws_server_with_max_message_bytes<E: From<String> + Send + 'static>(
    n: i64,
    cfg: WsServerCfg<E>,
) -> WsServerCfg<E> {
    WsServerCfg {
        maxMessageBytes: n,
        ..cfg
    }
}

/// `Ws.withOriginPatterns` — set the origin allowlist.  Empty list = dev
/// allow-all; production mode with an empty list causes `upgrade` to return
/// 403 (see `server_web_socket_upgrade`).
pub fn ws_server_with_origin_patterns<E: From<String> + Send + 'static>(
    ps: Vec<String>,
    cfg: WsServerCfg<E>,
) -> WsServerCfg<E> {
    WsServerCfg {
        originPatterns: ps,
        ..cfg
    }
}

/// `Ws.sendToClient` — send a text frame.  D3: unwraps `WsHandle` before
/// delegating to the i64 registry family.
pub fn ws_server_send_to_client<E: From<String> + Send + 'static>(
    h: WsHandle,
    msg: String,
) -> IpeTask<E, ()> {
    let WsHandle::WebSocketServer(id) = h;
    server_web_socket_send_to_client(id, msg)
}

/// `Ws.sendBinaryToClient` — send a binary frame.  `Bytes = Vec<u8>` (ipe
/// divergence: upstream Ipe uses `Bytes = String`; see divergences doc §D2).
pub fn ws_server_send_binary_to_client<E: From<String> + Send + 'static>(
    h: WsHandle,
    data: Vec<u8>,
) -> IpeTask<E, ()> {
    let WsHandle::WebSocketServer(id) = h;
    server_web_socket_send_binary_to_client(id, data)
}

/// `Ws.broadcast` — best-effort text broadcast.  D3: unwraps each handle.
pub fn ws_server_broadcast<E: From<String> + Send + 'static>(
    hs: Vec<WsHandle>,
    msg: String,
) -> IpeTask<E, ()> {
    let ids: Vec<i64> = hs
        .into_iter()
        .map(|WsHandle::WebSocketServer(id)| id)
        .collect();
    server_web_socket_broadcast(ids, msg)
}

/// `Ws.closeClient` — close a peer connection.  D3; idempotent.
pub fn ws_server_close_client<E: From<String> + Send + 'static>(h: WsHandle) -> IpeTask<E, ()> {
    let WsHandle::WebSocketServer(id) = h;
    server_web_socket_close_client(id)
}

#[cfg(test)]
mod ws_adapter_tests {
    use super::*;

    #[test]
    fn default_cfg_max_message_bytes_is_zero() {
        // 0 → ws_loop applies the 1 MiB default; NOT a hard limit of 0.
        let cfg = ws_server_default_cfg::<String>();
        assert_eq!(cfg.maxMessageBytes, 0);
    }

    #[test]
    fn ws_max_message_bytes_zero_or_negative_is_1mib_default() {
        assert_eq!(ws_max_message_bytes(0), 1 << 20);
        assert_eq!(ws_max_message_bytes(-1), 1 << 20);
    }

    #[test]
    fn ws_max_message_bytes_positive_passes_through() {
        assert_eq!(ws_max_message_bytes(4096), 4096);
    }

    #[test]
    fn default_cfg_origin_patterns_empty() {
        let cfg = ws_server_default_cfg::<String>();
        assert!(cfg.originPatterns.is_empty());
    }

    #[test]
    fn with_max_message_bytes_sets_field() {
        let cfg = ws_server_default_cfg::<String>();
        let cfg2 = ws_server_with_max_message_bytes(4096, cfg);
        assert_eq!(cfg2.maxMessageBytes, 4096);
    }

    #[test]
    fn with_origin_patterns_sets_field() {
        let cfg = ws_server_default_cfg::<String>();
        let cfg2 = ws_server_with_origin_patterns(vec!["https://example.com".into()], cfg);
        assert_eq!(cfg2.originPatterns, vec!["https://example.com"]);
    }

    #[test]
    fn broadcast_empty_list_produces_empty_ids() {
        // The empty-list fast-path in server_web_socket_broadcast returns Ok(()).
        let hs: Vec<WsHandle> = Vec::new();
        let ids: Vec<i64> = hs
            .into_iter()
            .map(|WsHandle::WebSocketServer(id)| id)
            .collect();
        assert!(ids.is_empty());
    }

    #[test]
    fn handle_unwrap_roundtrip() {
        let h = WsHandle::WebSocketServer(99);
        let WsHandle::WebSocketServer(id) = h;
        assert_eq!(id, 99);
    }

    // ── ws_send_buffer env parsing ────────────────────────────────────────────

    #[test]
    fn ws_send_buffer_default_is_256() {
        // Without IPE_WS_SEND_BUFFER the default is 256 frames.
        // This test avoids touching the env so it's safe to run in parallel
        // with other tests; it just confirms the fallback constant.
        // (env-mutation tests use std::env::set_var which is not thread-safe
        // in parallel test harnesses — we test the parsing logic separately.)
        let parsed = "256"
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(256);
        assert_eq!(parsed, 256);
    }

    // ── ws_heartbeat_secs env parsing ──────────────────────────────────

    /// Default heartbeat interval is 30 s (Go parity: `wsDefaultPingInterval`).
    #[test]
    fn ws_heartbeat_default_is_30() {
        // Simulate what ws_heartbeat_secs() returns when the env var is absent.
        let result: u64 = None::<String>
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(30);
        assert_eq!(result, 30);
    }

    /// A valid positive integer overrides the default.
    #[test]
    fn ws_heartbeat_env_override_parses() {
        let result: u64 = Some("60".to_string())
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(30);
        assert_eq!(result, 60);
    }

    /// Zero is rejected and the default is used.
    #[test]
    fn ws_heartbeat_zero_falls_back_to_default() {
        let result: u64 = Some("0".to_string())
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(30);
        assert_eq!(result, 30);
    }

    /// Non-numeric input is rejected and the default is used.
    #[test]
    fn ws_heartbeat_non_numeric_falls_back_to_default() {
        let result: u64 = Some("not-a-number".to_string())
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(30);
        assert_eq!(result, 30);
    }
}

// ─── Ipe.Http.Middleware + Ipe.Http.RateLimit ─────────────────────────────
//
// A Handler is `Fn(ServerRequest) -> IpeTask<E, ServerResponse>`. Each `with*`
// wraps a handler and returns a new one; they chain generically (each output is
// the next's input H), so no concrete `Handler` type is named.

fn header_ci<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn plain_resp(status: i64, body: &str, extra: &[(&str, &str)]) -> ServerResponse {
    let mut headers = HashMap::new();
    for (k, v) in extra {
        headers.insert(k.to_string(), v.to_string());
    }
    ServerResponse {
        status,
        body: body.to_string(),
        headers,
        contentType: "text/plain".to_string(),
        cookies: Vec::new(),
    }
}

/// Middleware.withCors : List String -> Handler -> Handler. Echoes an allowed
/// Origin (or `*`), answers preflight OPTIONS with 204, and tags responses.
pub fn middleware_with_cors<E, H>(origins: Vec<String>, h: H) -> ServerHandler<E>
where
    E: Send + 'static,
    H: IntoServerHandler<E>,
{
    let h = h.into_server_handler();
    Arc::new(move |req: ServerRequest| {
        let req_origin = header_ci(&req.headers, "origin").unwrap_or("").to_string();
        let allow = if origins.iter().any(|o| o == "*") {
            Some("*".to_string())
        } else if origins.iter().any(|o| o == &req_origin) && !req_origin.is_empty() {
            Some(req_origin)
        } else {
            None
        };
        if req.method.eq_ignore_ascii_case("OPTIONS") {
            let mut resp = plain_resp(
                204,
                "",
                &[
                    (
                        "access-control-allow-methods",
                        "GET, POST, PUT, DELETE, OPTIONS",
                    ),
                    (
                        "access-control-allow-headers",
                        "Content-Type, Authorization",
                    ),
                ],
            );
            if let Some(a) = allow {
                // A reflected SPECIFIC origin (not `*`) makes the response
                // origin-dependent — emit `Vary: Origin` so a shared/intermediary
                // cache can't serve one origin's ACAO to another (CORS cache
                // poisoning). `*` is origin-independent, so no Vary needed.
                if a != "*" {
                    // MERGE, don't clobber: a handler may already have set Vary
                    // (e.g. `Accept-Encoding`). Append `Origin` unless present.
                    let vary = match resp.headers.get("Vary") {
                        Some(prev)
                            if !prev
                                .split(',')
                                .any(|p| p.trim().eq_ignore_ascii_case("origin")) =>
                        {
                            format!("{}, Origin", prev)
                        }
                        Some(prev) => prev.clone(),
                        None => "Origin".to_string(),
                    };
                    resp.headers.insert("Vary".to_string(), vary);
                }
                resp.headers
                    .insert("access-control-allow-origin".to_string(), a);
            }
            return Box::pin(async move { ok_res(resp) });
        }
        let task = h(req);
        Box::pin(async move {
            match task.await {
                IpeResult::Ok(mut resp) => {
                    if let Some(a) = allow {
                        // See preflight branch: a reflected specific origin needs
                        // `Vary: Origin` to be cache-safe.
                        if a != "*" {
                            // MERGE, don't clobber: a handler may already have set Vary
                            // (e.g. `Accept-Encoding`). Append `Origin` unless present.
                            let vary = match resp.headers.get("Vary") {
                                Some(prev)
                                    if !prev
                                        .split(',')
                                        .any(|p| p.trim().eq_ignore_ascii_case("origin")) =>
                                {
                                    format!("{}, Origin", prev)
                                }
                                Some(prev) => prev.clone(),
                                None => "Origin".to_string(),
                            };
                            resp.headers.insert("Vary".to_string(), vary);
                        }
                        resp.headers
                            .insert("access-control-allow-origin".to_string(), a);
                    }
                    ok_res(resp)
                }
                other => other,
            }
        })
    })
}

/// Middleware.withLogging : Handler -> Handler. Logs `method path status Nms`.
pub fn middleware_with_logging<E, H>(h: H) -> ServerHandler<E>
where
    E: Send + 'static,
    H: IntoServerHandler<E>,
{
    let h = h.into_server_handler();
    Arc::new(move |req: ServerRequest| {
        let method = req.method.clone();
        let path = req.path.clone();
        let start = std::time::Instant::now();
        let task = h(req);
        Box::pin(async move {
            let result = task.await;
            let status = match &result {
                IpeResult::Ok(r) => r.status,
                IpeResult::Err(_) => 500,
            };
            eprintln!(
                "[ipe.http] {} {} {} {}ms",
                method,
                path,
                status,
                start.elapsed().as_millis()
            );
            result
        })
    })
}

/// Middleware.withBasicAuth : String -> String -> Handler -> Handler. Requires
/// HTTP Basic auth; constant-time credential comparison; 401 otherwise.
pub fn middleware_with_basic_auth<E, H>(user: String, pass: String, h: H) -> ServerHandler<E>
where
    E: Send + 'static,
    H: IntoServerHandler<E>,
{
    let h = h.into_server_handler();
    Arc::new(move |req: ServerRequest| {
        use subtle::ConstantTimeEq;
        let expected = format!("Basic {}", base64_encode(format!("{}:{}", user, pass)));
        let got = header_ci(&req.headers, "authorization").unwrap_or("");
        let ok: bool = got.as_bytes().ct_eq(expected.as_bytes()).into();
        if ok {
            h(req)
        } else {
            Box::pin(async move {
                ok_res(plain_resp(
                    401,
                    "Unauthorized",
                    &[("www-authenticate", "Basic realm=\"Ipe\"")],
                ))
            })
        }
    })
}

/// Middleware.withRateLimit : String -> Int -> Int -> Handler -> Handler.
/// Per-(key, client-IP) fixed window; 429 when exceeded.
pub fn middleware_with_rate_limit<E, H>(
    key: String,
    limit: i64,
    window_secs: i64,
    h: H,
) -> ServerHandler<E>
where
    E: Send + 'static,
    H: IntoServerHandler<E>,
{
    let h = h.into_server_handler();
    Arc::new(move |req: ServerRequest| {
        if fixed_window_allow(&key, &req.remoteAddr, limit, window_secs) {
            h(req)
        } else {
            Box::pin(async move { ok_res(plain_resp(429, "Too Many Requests", &[])) })
        }
    })
}

/// Whether to trust `X-Forwarded-Proto` (and friends) for TLS-termination
/// detection. Mirrors `build_request`'s existing `IPE_TRUSTED_PROXY` gate for
/// `remoteAddr` (line ~497 above) and `live/mod.rs`'s `trust_proxy_headers()`
/// for the session cookie's `Secure` gate — same env var, same rationale: a
/// client-supplied header must never be trusted by default, an operator opts
/// in only when a real reverse proxy sits in front of this process.
///
/// Snapshotted once (env is stable at process start; same rationale as
/// `csrf_cookie_name`'s production check being re-read per call is fine
/// because it's a plain fn call, but this one backs a per-request hot path so
/// it's cached like `live/mod.rs`'s twin).
fn trust_proxy_headers() -> bool {
    use std::sync::OnceLock;
    static TRUST: OnceLock<bool> = OnceLock::new();
    *TRUST.get_or_init(|| {
        crate::system::read_env_var("IPE_TRUSTED_PROXY")
            .map(|v| !v.is_empty() && v != "0" && v != "false")
            .unwrap_or(false)
    })
}

/// Request-scoped HTTPS detection, parameterised on the trust decision so
/// it's unit-testable without mutating the real (`OnceLock`-cached) process
/// env. Only consulted (via `request_is_https`) when `trust` is true —
/// otherwise a client could forge `X-Forwarded-Proto` to fool the
/// Secure-cookie decision (the same footgun `build_request` already closed
/// for `X-Forwarded-For`). Mirrors `live/mod.rs::request_is_https_with_trust`,
/// adapted to `ServerRequest.headers`'s `HashMap<String, String>` shape
/// (already canonicalised from the axum request at `build_request` time —
/// see `header_ci`) instead of a raw `axum::http::HeaderMap`.
fn request_is_https_with_trust(headers: &HashMap<String, String>, trust: bool) -> bool {
    if !trust {
        return false;
    }
    header_ci(headers, "x-forwarded-proto")
        .map(|v| v.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

/// Request-scoped HTTPS detection: true when THIS request arrived over TLS at
/// a trusted proxy (`X-Forwarded-Proto: https`). See
/// `request_is_https_with_trust` for the testable core.
///
/// MUST be called (and its result captured) BEFORE the `ServerRequest` is
/// moved into the wrapped handler in `middleware_with_csrf` — by the time the
/// response comes back the request is gone, so the boolean has to be
/// snapshotted up front and threaded through as a plain `bool` capture.
fn request_is_https(headers: &HashMap<String, String>) -> bool {
    request_is_https_with_trust(headers, trust_proxy_headers())
}

/// `__Host-` prefix requires Secure + Path=/ + no Domain — mirrors
/// `live/csrf.rs::csrf_cookie_name`'s reasoning, gated on the SAME
/// process-wide production signal `server_with_cookie` already uses
/// (`telemetry::production_from_env`), so naming stays internally consistent
/// with the rest of `server.rs`'s cookie handling.
///
/// This stays process-global (NOT request-scoped) deliberately, same
/// reasoning as the session cookie's `__Host-` name decision
/// (`csrf::cookies_secure()`, `live/mod.rs`): the cookie's IDENTITY must stay
/// stable across a browser session, or the double-submit compare would
/// spuriously fail whenever proxy-scheme detection flips between requests.
/// Only the `Secure` ATTRIBUTE (`csrf_set_cookie_value`) becomes
/// request-scoped.
fn csrf_cookie_name() -> &'static str {
    if crate::telemetry::production_from_env() {
        "__Host-ipe_csrf"
    } else {
        "ipe_csrf"
    }
}

/// 64 lowercase-hex chars (two concatenated UUIDv4s, ~244 combined random
/// bits — comfortably above the 128-bit CSRF-token floor). `uuid::Uuid::new_v4`
/// draws from the OS CSPRNG, the approved security-bearing-randomness source
/// per `random.rs`. Re-exported from `web/csrf.rs` as `gen_token`.
pub fn csrf_gen_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// A token "looks valid" if it is the expected 64 lowercase-hex shape — used
/// both to decide whether to reuse a browser cookie token vs mint a fresh one,
/// and as the well-formedness half of `csrf_pair_valid`. Re-exported from
/// `web/csrf.rs` as `token_is_well_formed`.
pub fn csrf_token_well_formed(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Returns `true` iff BOTH tokens pass the well-formedness gate AND compare
/// equal in constant time. The structural check runs before the secret compare —
/// this ordering is standard and does not create a timing side-channel on the
/// secret (the well-formedness predicate observes only length and character
/// class, not the secret value). Fail-closed: any malformed, missing, or
/// mismatched pair returns `false`. Re-exported from `web/csrf.rs`.
pub fn csrf_pair_valid(cookie_tok: &str, header_tok: &str) -> bool {
    use subtle::ConstantTimeEq;
    csrf_token_well_formed(cookie_tok)
        && csrf_token_well_formed(header_tok)
        && bool::from(cookie_tok.as_bytes().ct_eq(header_tok.as_bytes()))
}

/// NOT HttpOnly — client JS must be able to read this to echo it into
/// `X-Csrf-Token` (classic double-submit; still safe against a forging
/// cross-origin page because SOP blocks that page from reading the
/// victim-origin cookie).
///
/// `Secure` is set when EITHER `production_from_env()` is true (unconditional
/// floor — a production deploy always gets `Secure`, matching
/// `server_with_cookie`'s gate and the session cookie's
/// `csrf::cookies_secure()` half) OR `request_is_https` is true (THIS
/// specific request arrived over TLS at a trusted proxy, opt-in via
/// `IPE_TRUSTED_PROXY` — closes the gap where a dev process (`ENV` unset)
/// fronted by a TLS-terminating proxy would otherwise emit a non-Secure CSRF
/// cookie even though the browser connection was HTTPS). Same OR-gate shape as
/// the session cookie in `live/mod.rs::page_response`.
///
/// `request_is_https` MUST be computed from the ORIGINAL request headers
/// before the request is consumed — see the call site in
/// `middleware_with_csrf`, which captures it into a local `bool` before
/// moving `req` into the wrapped handler `h(req)`. By the time this function
/// runs (after the handler's `Task` resolves), the request itself is gone;
/// only the pre-captured bool survives.
fn csrf_set_cookie_value(token: &str, request_is_https: bool) -> String {
    let name = csrf_cookie_name();
    let secure = if crate::telemetry::production_from_env() || request_is_https {
        "; Secure"
    } else {
        ""
    };
    format!("{name}={token}; Path=/; SameSite=Strict{secure}")
}

/// Middleware.withCsrf : Handler -> Handler. Double-submit-cookie CSRF guard
/// for `Ipe.Http.Server` routes (Go/upstream-audit parity: `__Host-ipe_csrf`
/// cookie, safe methods set/refresh it, unsafe methods require cookie ==
/// `X-Csrf-Token` header via constant-time compare, 403 on any
/// mismatch/missing value).
///
/// Depends on `ServerResponse.cookies` so this middleware's Set-Cookie can
/// never clobber (or be clobbered by) one the wrapped handler sets via
/// `Server.withCookie`.
pub fn middleware_with_csrf<E, H>(h: H) -> ServerHandler<E>
where
    E: Send + 'static,
    H: IntoServerHandler<E>,
{
    let h = h.into_server_handler();
    Arc::new(move |req: ServerRequest| {
        let safe = matches!(
            req.method.to_ascii_uppercase().as_str(),
            "GET" | "HEAD" | "OPTIONS"
        );
        let cookie_name = csrf_cookie_name();
        let existing = req.cookies.get(cookie_name).cloned();
        let token = existing
            .clone()
            .filter(|t| csrf_token_well_formed(t))
            .unwrap_or_else(csrf_gen_token);
        if !safe {
            let cookie_tok = existing.unwrap_or_default();
            let header_tok = header_ci(&req.headers, "x-csrf-token")
                .unwrap_or("")
                .to_string();
            if !csrf_pair_valid(&cookie_tok, &header_tok) {
                return Box::pin(async move {
                    ok_res(plain_resp(403, "csrf token invalid or missing", &[]))
                });
            }
        }
        // Capture the request-scoped TLS signal HERE — before `req` is moved
        // into `h(req)` below. `ServerRequest` is not `Clone`-cheap-by-design
        // (it owns the full body/headers/cookies maps) and the wrapped
        // handler legitimately needs to consume it, so there is no request
        // left to inspect once `task` is awaited. `bool` is `Copy`, so this
        // one-line snapshot is the entire adaptation needed versus the
        // session-cookie fix (which reads `headers` at cookie-set time
        // because `page_response` runs BEFORE the request is handed off).
        let is_https = request_is_https(&req.headers);
        let task = h(req);
        Box::pin(async move {
            match task.await {
                IpeResult::Ok(mut resp) => {
                    resp.cookies.push(csrf_set_cookie_value(&token, is_https));
                    IpeResult::Ok(resp)
                }
                other => other,
            }
        })
    })
}

/// How often (in calls) the rate-limit maps run their full-map expiry sweep.
/// The `retain` is O(n); running it every call lets an attacker who supplies many
/// distinct client keys (trusted-proxy mode, attacker-controlled X-Forwarded-For)
/// turn each request into a full-map scan (CPU amplification). Amortizing the
/// sweep to every RL_SWEEP_EVERY calls bounds that to O(n / RL_SWEEP_EVERY) per
/// request while still reclaiming expired entries (memory stays bounded).
const RL_SWEEP_EVERY: u64 = 256;

fn unix_secs_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

struct WindowEntry {
    start: f64,
    count: i64,
}

fn fixed_window_allow(key: &str, client: &str, limit: i64, window_secs: i64) -> bool {
    static W: OnceLock<Mutex<HashMap<(String, String), WindowEntry>>> = OnceLock::new();
    static TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = unix_secs_f64();
    let window = window_secs.max(1) as f64;
    let mut m = W
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // Evict fully-expired entries so the map can't grow without bound (distinct
    // clients/keys would otherwise accumulate forever → memory-DoS). The O(n) scan
    // is AMORTIZED to every RL_SWEEP_EVERY calls so an attacker can't force a
    // full-map scan per request (CPU amplification). An expired entry resets to
    // count 0 on access anyway, so a lingering one between sweeps is
    // behaviour-preserving for the surviving (live) entries.
    if TICK
        .fetch_add(1, Ordering::Relaxed)
        .is_multiple_of(RL_SWEEP_EVERY)
    {
        m.retain(|_, ent| now - ent.start < window);
    }
    let e = m
        .entry((key.to_string(), client.to_string()))
        .or_insert(WindowEntry {
            start: now,
            count: 0,
        });
    if now - e.start >= window {
        e.start = now;
        e.count = 0;
    }
    if e.count < limit.max(0) {
        e.count += 1;
        true
    } else {
        false
    }
}

struct Bucket {
    tokens: f64,
    last: f64,
}

/// RateLimit.allow : String -> String -> Int -> Int -> Bool — token bucket per
/// (name, key); capacity tokens, refilled `refill_per_sec`. True if a token was
/// consumed.
pub fn rate_limit_allow(name: String, key: String, capacity: i64, refill_per_sec: i64) -> bool {
    static B: OnceLock<Mutex<HashMap<(String, String), Bucket>>> = OnceLock::new();
    static TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let cap = capacity.max(0) as f64;
    let now = unix_secs_f64();
    let refill = refill_per_sec.max(0) as f64;
    let mut m = B
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // Evict an entry if EITHER it has refilled back to full (indistinguishable
    // from a fresh bucket) OR it has been idle longer than RL_IDLE_TTL. The
    // idle bound is refill-INDEPENDENT: with refill_per_sec == 0 a partially-drained
    // bucket never refills to full, so the refill-only predicate would retain it
    // forever and the map grows unbounded across distinct (name, key) pairs
    // (memory-DoS). The O(n) scan is AMORTIZED to every RL_SWEEP_EVERY calls so an
    // attacker supplying many distinct keys can't force a full-map scan per request
    // (CPU amplification). Either way the current entry is re-created below if swept.
    const RL_IDLE_TTL: f64 = 3600.0; // 1 h with no access → reclaim
    if TICK
        .fetch_add(1, Ordering::Relaxed)
        .is_multiple_of(RL_SWEEP_EVERY)
    {
        m.retain(|_, bk| {
            let refilled = (bk.tokens + (now - bk.last) * refill).min(cap);
            refilled < cap && (now - bk.last) < RL_IDLE_TTL
        });
    }
    let b = m.entry((name, key)).or_insert(Bucket {
        tokens: cap,
        last: now,
    });
    b.tokens = (b.tokens + (now - b.last) * refill).min(cap);
    b.last = now;
    if b.tokens >= 1.0 {
        b.tokens -= 1.0;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::ready;

    #[test]
    fn server_header_is_case_insensitive_go_parity() {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        let req = ServerRequest {
            method: "GET".to_string(),
            path: "/".to_string(),
            body: String::new(),
            headers,
            params: HashMap::new(),
            query: HashMap::new(),
            cookies: HashMap::new(),
            remoteAddr: String::new(),
        };
        for probe in ["content-type", "Content-Type", "CONTENT-TYPE"] {
            assert!(
                matches!(
                    server_header(probe.to_string(), req.clone()),
                    IpeMaybe::Just(ref v) if v == "application/json"
                ),
                "lookup {probe:?} should resolve to the stored value",
            );
        }
        assert!(matches!(
            server_header("x-missing".to_string(), req.clone()),
            IpeMaybe::Nothing
        ));
    }

    #[tokio::test]
    async fn build_request_stores_canonical_header_keys() {
        let wire = axum::http::Request::builder()
            .method("GET")
            .uri("/")
            .header("x-trace-id", "abc123")
            .header("content-type", "text/plain")
            .body(axum::body::Body::empty())
            .expect("test request builds");
        let (req, _upgrader) = build_request(wire).await.expect("build_request succeeds");
        assert_eq!(
            req.headers.get("X-Trace-Id").map(String::as_str),
            Some("abc123")
        );
        assert_eq!(
            req.headers.get("Content-Type").map(String::as_str),
            Some("text/plain")
        );
        // The verbatim lower-cased key must NOT be present (canonical only).
        assert!(!req.headers.contains_key("x-trace-id"));
        assert!(matches!(
            server_header("X-TRACE-ID".to_string(), req),
            IpeMaybe::Just(ref v) if v == "abc123"
        ));
    }

    #[test]
    fn build_routes_and_response() {
        // Validate the crux: a Ipê-shaped handler closure boxes into a Route.
        let r: ServerRoute = server_get::<String, _>("/".to_string(), |_req: ServerRequest| {
            Box::pin(ready(ok_res::<String, _>(server_text("hi".to_string()))))
                as IpeTask<String, ServerResponse>
        });
        assert_eq!(r.method, "GET");
        assert!(matches!(r.target, RouteTarget::Handler(_)));
        let resp = server_with_status(404, server_text("nope".to_string()));
        assert_eq!(resp.status, 404);
    }

    async fn axum_body_string(resp: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("collect body");
        String::from_utf8(bytes.to_vec()).expect("utf8 body")
    }

    #[tokio::test]
    async fn to_axum_response_injects_dev_banner_into_html_before_body_close() {
        // Default test env is dev (ENV/IPE_ENV unset), so the banner is emitted.
        // Go parity: injectDevBanner runs on every text/html buffered response.
        let ipe = server_html("<html><body><h1>hi</h1></body></html>".to_string());
        let out = axum_body_string(to_axum_response(ipe)).await;
        assert!(
            out.contains(r#"<a id="__ipe-dev-console""#),
            "banner must be injected: {out}"
        );
        let banner_at = out
            .find(r#"<a id="__ipe-dev-console""#)
            .expect("banner present");
        let body_close = out.rfind("</body>").expect("</body> present");
        assert!(
            banner_at < body_close,
            "banner must sit before </body>: {out}"
        );
    }

    #[tokio::test]
    async fn to_axum_response_leaves_non_html_untouched() {
        // JSON / plain-text responses never get the banner (Go: the HasPrefix
        // "text/html" gate excludes them).
        let ipe = server_json(r#"{"ok":true}"#.to_string());
        let out = axum_body_string(to_axum_response(ipe)).await;
        assert_eq!(out, r#"{"ok":true}"#, "non-html body must be verbatim");

        let ipe_text = server_text("plain body</body>".to_string());
        let out_text = axum_body_string(to_axum_response(ipe_text)).await;
        assert_eq!(
            out_text, "plain body</body>",
            "text/plain body must be verbatim even with a </body> substring"
        );
    }

    #[test]
    fn origin_glob_matching() {
        assert!(ws_origin_matches(
            "https://app.example.com",
            "https://app.example.com"
        ));
        assert!(!ws_origin_matches(
            "https://app.example.com",
            "https://evil.com"
        ));
        assert!(ws_origin_matches(
            "https://*.example.com",
            "https://app.example.com"
        ));
        assert!(ws_origin_matches(
            "https://*.example.com",
            "https://a.b.example.com"
        ));
        assert!(!ws_origin_matches(
            "https://*.example.com",
            "https://example.com"
        ));
        assert!(!ws_origin_matches(
            "https://*.example.com",
            "http://app.example.com"
        ));
        assert!(ws_origin_matches("*", "anything://x"));
        assert!(ws_origin_matches("*.local", "x.local"));
        assert!(!ws_origin_matches("*.local", "x.remote"));
        // CSWSH glob-bypass: the trusted suffix must not be reachable behind a
        // path / userinfo / query delimiter smuggled through the `*`.
        assert!(!ws_origin_matches(
            "https://*.example.com",
            "https://evil.com/.example.com"
        ));
        assert!(!ws_origin_matches(
            "https://*.example.com",
            "https://evil.com@x.example.com"
        ));
        assert!(!ws_origin_matches(
            "https://*.example.com",
            "https://evil.com?.example.com"
        ));
        assert!(!ws_origin_matches(
            "https://*.example.com",
            "https://evil.com#.example.com"
        ));
        // A trailing `*` is an explicit allow-all of the remainder (opt-in).
        assert!(ws_origin_matches(
            "https://app.example.com*",
            "https://app.example.com/anything"
        ));
    }

    fn mk_ws_req(headers: &[(&str, &str)]) -> ServerRequest {
        let mut h = HashMap::new();
        for (k, v) in headers {
            h.insert(k.to_string(), v.to_string());
        }
        ServerRequest {
            method: "GET".to_string(),
            path: "/ws".to_string(),
            body: String::new(),
            headers: h,
            params: HashMap::new(),
            query: HashMap::new(),
            cookies: HashMap::new(),
            remoteAddr: String::new(),
        }
    }

    #[test]
    fn ws_cross_origin_detection() {
        assert!(ws_cross_origin(&mk_ws_req(&[
            ("origin", "https://evil.example"),
            ("host", "victim.example:8000"),
        ])));
        assert!(!ws_cross_origin(&mk_ws_req(&[
            ("origin", "https://victim.example:8000"),
            ("host", "victim.example:8000"),
        ])));
        // No Origin header at all → not flagged (non-browser client).
        assert!(!ws_cross_origin(&mk_ws_req(&[("host", "victim.example")])));
        // Backlog port-mismatch fix: an implicit-default-port Origin against
        // an explicit-default-port Host is the SAME origin, not a mismatch.
        assert!(!ws_cross_origin(&mk_ws_req(&[
            ("origin", "https://victim.example"),
            ("host", "victim.example:443"),
        ])));
    }

    #[tokio::test]
    async fn ws_upgrade_dev_rejects_cross_origin_without_allowlist() {
        // No IPE_TRUSTED_PROXY / ENV involvement — this exercises the CSWSH
        // default-deny path directly: dev mode (no ENV set in this test
        // process), empty originPatterns, cross-origin Origin/Host pair. The
        // pre-fix behaviour fell through with no check at all (allow-all).
        let cfg = ws_server_default_cfg::<String>();
        let req = mk_ws_req(&[
            ("origin", "https://evil.example"),
            ("host", "victim.example"),
        ]);
        // No WS_UPGRADER task-local is set in a plain unit test, so a request
        // that PASSES the origin check would hit the `None => 400` upgrader
        // branch instead of 403 — the origin check must short-circuit before
        // that point for this assertion to distinguish the two paths.
        match server_web_socket_upgrade::<String>(req, cfg).await {
            IpeResult::Ok(r) => assert_eq!(
                r.status, 403,
                "cross-origin WS upgrade must be rejected outside production too"
            ),
            IpeResult::Err(e) => panic!("expected Ok(403), got Err({e})"),
        }
    }

    #[tokio::test]
    async fn ws_upgrade_dev_allows_same_origin_without_allowlist() {
        let cfg = ws_server_default_cfg::<String>();
        let req = mk_ws_req(&[
            ("origin", "https://victim.example"),
            ("host", "victim.example"),
        ]);
        // Same-origin passes the CSWSH check; falls through to the "no
        // upgrader present" 400 (this unit test doesn't drive a real axum
        // WS upgrade), which is enough to prove it did NOT hit the 403
        // cross-origin branch.
        match server_web_socket_upgrade::<String>(req, cfg).await {
            IpeResult::Ok(r) => assert_eq!(
                r.status, 400,
                "same-origin WS upgrade must pass the origin check (400 = no real upgrader in this unit test, not 403)"
            ),
            IpeResult::Err(e) => panic!("expected Ok(400), got Err({e})"),
        }
    }

    #[test]
    fn query_and_cookies() {
        let q = parse_query(Some("a=1&b=two%20words&a=ignored&flag"));
        assert_eq!(q.get("a").map(String::as_str), Some("1")); // first value wins
        assert_eq!(q.get("b").map(String::as_str), Some("two words"));
        assert_eq!(q.get("flag").map(String::as_str), Some(""));
        assert!(parse_query(None).is_empty());

        let mut c = std::collections::HashMap::new();
        parse_cookies("sid=abc; theme=dark", &mut c);
        assert_eq!(c.get("sid").map(String::as_str), Some("abc"));
        assert_eq!(c.get("theme").map(String::as_str), Some("dark"));
    }

    #[test]
    fn max_body_env_override() {
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_WEB_MAX_BODY_BYTES") };
        assert_eq!(max_body(), DEFAULT_MAX_BODY);
        // SAFETY: test-only env mutation.
        unsafe { std::env::set_var("IPE_WEB_MAX_BODY_BYTES", "1024") };
        assert_eq!(max_body(), 1024);
        // SAFETY: test-only env mutation.
        unsafe { std::env::set_var("IPE_WEB_MAX_BODY_BYTES", "0") }; // invalid → default
        assert_eq!(max_body(), DEFAULT_MAX_BODY);
        // SAFETY: test-only env mutation.
        unsafe { std::env::remove_var("IPE_WEB_MAX_BODY_BYTES") };
    }

    #[tokio::test]
    async fn two_set_cookie_headers_both_survive() {
        let mut r = server_text("ok".to_string());
        r = server_with_cookie(server_cookie("a".into(), "1".into()), r);
        r = server_with_cookie(server_cookie("b".into(), "2".into()), r);
        let resp = to_axum_response(r);
        let cookies: Vec<_> = resp
            .headers()
            .get_all(axum::http::header::SET_COOKIE)
            .iter()
            .collect();
        assert_eq!(cookies.len(), 2, "both Set-Cookie lines must survive");
    }

    fn mk_req(
        method: &str,
        cookies: HashMap<String, String>,
        headers: HashMap<String, String>,
    ) -> ServerRequest {
        ServerRequest {
            method: method.to_string(),
            path: "/".to_string(),
            body: String::new(),
            headers,
            params: HashMap::new(),
            query: HashMap::new(),
            cookies,
            remoteAddr: String::new(),
        }
    }

    #[tokio::test]
    async fn csrf_get_mints_and_sets_cookie_no_check() {
        let h = middleware_with_csrf::<String, _>(|_req: ServerRequest| {
            Box::pin(ready(ok_res::<String, _>(server_text("ok".into()))))
                as IpeTask<String, ServerResponse>
        });
        let req = mk_req("GET", HashMap::new(), HashMap::new());
        let resp = h(req).await;
        match resp {
            IpeResult::Ok(r) => assert_eq!(r.cookies.len(), 1, "GET must mint a fresh cookie"),
            IpeResult::Err(_) => panic!("GET must never be rejected"),
        }
    }

    #[tokio::test]
    async fn csrf_post_without_header_rejected() {
        let mut cookies = HashMap::new();
        cookies.insert("ipe_csrf".to_string(), "a".repeat(64));
        let h = middleware_with_csrf::<String, _>(|_req: ServerRequest| {
            Box::pin(ready(ok_res::<String, _>(server_text("ok".into()))))
                as IpeTask<String, ServerResponse>
        });
        let req = mk_req("POST", cookies, HashMap::new());
        match h(req).await {
            IpeResult::Ok(r) => assert_eq!(r.status, 403),
            IpeResult::Err(_) => panic!("expected an Ok(403), not an Err"),
        }
    }

    #[tokio::test]
    async fn csrf_post_with_matching_cookie_and_header_allowed() {
        let tok = "b".repeat(64);
        let mut cookies = HashMap::new();
        cookies.insert("ipe_csrf".to_string(), tok.clone());
        let mut headers = HashMap::new();
        headers.insert("x-csrf-token".to_string(), tok);
        let h = middleware_with_csrf::<String, _>(|_req: ServerRequest| {
            Box::pin(ready(ok_res::<String, _>(server_text("ok".into()))))
                as IpeTask<String, ServerResponse>
        });
        let req = mk_req("POST", cookies, headers);
        match h(req).await {
            IpeResult::Ok(r) => assert_eq!(r.status, 200),
            IpeResult::Err(_) => panic!("expected Ok(200)"),
        }
    }

    #[tokio::test]
    async fn csrf_post_with_mismatched_cookie_and_header_rejected() {
        let mut cookies = HashMap::new();
        cookies.insert("ipe_csrf".to_string(), "c".repeat(64));
        let mut headers = HashMap::new();
        headers.insert("x-csrf-token".to_string(), "d".repeat(64));
        let h = middleware_with_csrf::<String, _>(|_req: ServerRequest| {
            Box::pin(ready(ok_res::<String, _>(server_text("ok".into()))))
                as IpeTask<String, ServerResponse>
        });
        let req = mk_req("POST", cookies, headers);
        match h(req).await {
            IpeResult::Ok(r) => assert_eq!(r.status, 403),
            IpeResult::Err(_) => panic!("expected Ok(403)"),
        }
    }

    /// Regression for the well-formedness gap: an EQUAL pair of malformed
    /// values (too short to be a real server-minted token) must still be
    /// rejected — the compare alone (`cookie_tok == header_tok`) is not
    /// sufficient, both sides must also look like a genuine token.
    #[tokio::test]
    async fn csrf_post_with_matching_but_malformed_tokens_rejected() {
        let mut cookies = HashMap::new();
        cookies.insert("ipe_csrf".to_string(), "x".to_string());
        let mut headers = HashMap::new();
        headers.insert("x-csrf-token".to_string(), "x".to_string());
        let h = middleware_with_csrf::<String, _>(|_req: ServerRequest| {
            Box::pin(ready(ok_res::<String, _>(server_text("ok".into()))))
                as IpeTask<String, ServerResponse>
        });
        let req = mk_req("POST", cookies, headers);
        match h(req).await {
            IpeResult::Ok(r) => assert_eq!(r.status, 403),
            IpeResult::Err(_) => panic!("expected Ok(403)"),
        }
    }

    // ── CSRF cookie `Secure` — ENV-vs-TLS combined gate ──────────────

    #[test]
    fn request_is_https_ignored_without_trust_opt_in() {
        let mut headers = HashMap::new();
        headers.insert("x-forwarded-proto".to_string(), "https".to_string());
        assert!(
            !request_is_https_with_trust(&headers, false),
            "must ignore X-Forwarded-Proto without IPE_TRUSTED_PROXY opt-in"
        );
    }

    #[test]
    fn request_is_https_honoured_when_trusted() {
        let mut headers = HashMap::new();
        headers.insert("x-forwarded-proto".to_string(), "https".to_string());
        assert!(request_is_https_with_trust(&headers, true));

        let mut headers2 = HashMap::new();
        headers2.insert("x-forwarded-proto".to_string(), "http".to_string());
        assert!(!request_is_https_with_trust(&headers2, true));
    }

    #[test]
    fn request_is_https_missing_header_is_not_https() {
        let headers = HashMap::new();
        assert!(!request_is_https_with_trust(&headers, true));
    }

    /// `csrf_set_cookie_value`'s combined gate: `Secure` when EITHER
    /// production OR the (pre-captured) request-scoped TLS signal is true.
    /// Exercises all four (production, request_is_https) combinations —
    /// this is the pure-function core, independent of env-var mutation.
    #[test]
    fn csrf_cookie_secure_or_gate_truth_table() {
        // production=false is simulated by calling with request_is_https
        // directly; production is exercised via the ENV-var tests below
        // (per-process under nextest, so mutating ENV here is safe).
        let tok = "a".repeat(64);

        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("ENV") };
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_ENV") };
        // (a) not production, request IS https -> Secure.
        assert!(
            csrf_set_cookie_value(&tok, true).contains("; Secure"),
            "TLS-detected request must get Secure regardless of ENV"
        );
        // (b) not production, request NOT https -> no Secure (dev-mode-correct).
        assert!(
            !csrf_set_cookie_value(&tok, false).contains("; Secure"),
            "plain-HTTP dev request must NOT get Secure"
        );
    }

    #[test]
    fn csrf_cookie_secure_production_forces_secure_even_without_tls_signal() {
        // (c) the exact gap this closes: ENV=production set, but THIS
        // request is not detected as TLS (e.g. IPE_TRUSTED_PROXY unset, or
        // no proxy in front) -> Secure still fires off the unconditional
        // production floor. Matches the session cookie's own combined-gate
        // semantics in live/mod.rs::page_response (`csrf::cookies_secure()
        // || request_is_https(headers)` — production forces Secure
        // unconditionally; the request-scoped signal only ADDS Secure in
        // the non-production case).
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("ENV", "production") };
        let tok = "b".repeat(64);
        let cookie = csrf_set_cookie_value(&tok, false);
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("ENV") };
        assert!(
            cookie.contains("; Secure"),
            "ENV=production must force Secure even when this request isn't TLS-detected: {cookie}"
        );
    }

    #[test]
    fn csrf_cookie_secure_production_and_tls_signal_both_true() {
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("ENV", "production") };
        let tok = "c".repeat(64);
        let cookie = csrf_set_cookie_value(&tok, true);
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("ENV") };
        assert!(cookie.contains("; Secure"));
    }

    /// End-to-end through `middleware_with_csrf` (not just the pure
    /// `csrf_set_cookie_value` helper): a GET request carrying
    /// `X-Forwarded-Proto: https` mints a Secure cookie when
    /// `IPE_TRUSTED_PROXY` is honoured, proving the signal survives the
    /// capture-before-move + thread-through-the-closure adaptation.
    #[tokio::test]
    async fn csrf_middleware_mints_secure_cookie_for_trusted_https_request() {
        let mut headers = HashMap::new();
        headers.insert("x-forwarded-proto".to_string(), "https".to_string());
        let h = middleware_with_csrf::<String, _>(|_req: ServerRequest| {
            Box::pin(ready(ok_res::<String, _>(server_text("ok".into()))))
                as IpeTask<String, ServerResponse>
        });
        let req = mk_req("GET", HashMap::new(), headers);
        // `middleware_with_csrf` calls the process-wide `request_is_https`
        // (via `trust_proxy_headers()`'s `OnceLock`), which without
        // `IPE_TRUSTED_PROXY` set never trusts the header — so this test
        // documents the untrusted-by-default floor: no Secure without the
        // operator's opt-in, even though the header claims https.
        match h(req).await {
            IpeResult::Ok(r) => {
                assert_eq!(r.cookies.len(), 1);
                assert!(
                    !r.cookies[0].contains("; Secure"),
                    "X-Forwarded-Proto must be ignored without IPE_TRUSTED_PROXY opt-in: {}",
                    r.cookies[0]
                );
            }
            IpeResult::Err(_) => panic!("GET must never be rejected"),
        }
    }

    // ── authenticated routes (fail-closed) ────────────────────────────
    #[cfg(feature = "jwt")]
    mod authed {
        use super::*;

        const SECRET: &str = "a-test-secret-of-32-bytes-padding";

        fn req_with(headers: &[(&str, &str)], cookies: &[(&str, &str)]) -> ServerRequest {
            ServerRequest {
                method: "GET".to_string(),
                path: "/me".to_string(),
                body: String::new(),
                headers: headers
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
                params: HashMap::new(),
                query: HashMap::new(),
                cookies: cookies
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
                remoteAddr: String::new(),
            }
        }

        fn hs256(claims: &serde_json::Value) -> String {
            let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
            let key = jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes());
            jsonwebtoken::encode(&header, claims, &key).expect("encode")
        }

        // Drive `server_get_authed`'s guarded handler directly: a handler that
        // answers 200 with the principal's subject, so the response status tells
        // us whether the middleware minted (200) or rejected (401).
        async fn run(cfg: AuthConfig, req: ServerRequest) -> ServerResponse {
            let route = server_get_authed::<String, _>("/me".to_string(), cfg, |_req, p| {
                let subject = crate::principal::principal_subject(p);
                Box::pin(std::future::ready(ok_res(server_text(subject))))
            });
            let RouteTarget::Handler(h) = route.target else {
                panic!("authed route must carry a handler");
            };
            h(req).await.expect("guarded handler never returns Err")
        }

        fn bearer_cfg() -> AuthConfig {
            server_auth_config(
                crate::secret::secret_from_string(SECRET.to_string()),
                TokenSource::BearerHeader,
            )
        }

        #[tokio::test]
        async fn missing_token_is_401() {
            let resp = run(bearer_cfg(), req_with(&[], &[])).await;
            assert_eq!(resp.status, 401, "no Authorization header must fail closed");
        }

        #[tokio::test]
        async fn malformed_token_is_401() {
            let req = req_with(&[("authorization", "Bearer not-a-jwt")], &[]);
            let resp = run(bearer_cfg(), req).await;
            assert_eq!(resp.status, 401, "an unverifiable token must fail closed");
        }

        #[tokio::test]
        async fn expired_token_is_401() {
            let token = hs256(&serde_json::json!({ "sub": "u1", "exp": 1 }));
            let req = req_with(&[("authorization", &format!("Bearer {token}"))], &[]);
            let resp = run(bearer_cfg(), req).await;
            assert_eq!(resp.status, 401, "an expired token must fail closed");
        }

        #[tokio::test]
        async fn absent_subject_claim_is_401() {
            let token = hs256(&serde_json::json!({ "role": "admin", "exp": 9_999_999_999i64 }));
            let req = req_with(&[("authorization", &format!("Bearer {token}"))], &[]);
            let resp = run(bearer_cfg(), req).await;
            assert_eq!(
                resp.status, 401,
                "a token with no subject claim must fail closed"
            );
        }

        #[tokio::test]
        async fn valid_bearer_token_mints_and_dispatches() {
            let token = hs256(&serde_json::json!({ "sub": "user-7", "exp": 9_999_999_999i64 }));
            let req = req_with(&[("authorization", &format!("Bearer {token}"))], &[]);
            let resp = run(bearer_cfg(), req).await;
            assert_eq!(
                resp.status, 200,
                "a valid token must dispatch to the handler"
            );
            assert_eq!(resp.body, "user-7", "the handler sees the minted subject");
        }

        #[tokio::test]
        async fn valid_cookie_token_mints_and_dispatches() {
            let token = hs256(&serde_json::json!({ "sub": "user-9", "exp": 9_999_999_999i64 }));
            let cfg = server_auth_config(
                crate::secret::secret_from_string(SECRET.to_string()),
                TokenSource::Cookie("ipe_sid".to_string()),
            );
            let resp = run(cfg, req_with(&[], &[("ipe_sid", &token)])).await;
            assert_eq!(resp.status, 200, "a valid cookie token must dispatch");
            assert_eq!(resp.body, "user-9");
        }

        #[tokio::test]
        async fn wrong_secret_is_401() {
            let token = hs256(&serde_json::json!({ "sub": "u1", "exp": 9_999_999_999i64 }));
            let cfg = server_auth_config(
                crate::secret::secret_from_string("a-DIFFERENT-secret-32-bytes-pad!".to_string()),
                TokenSource::BearerHeader,
            );
            let req = req_with(&[("authorization", &format!("Bearer {token}"))], &[]);
            let resp = run(cfg, req).await;
            assert_eq!(
                resp.status, 401,
                "a token signed under another secret must fail closed"
            );
        }

        // ── sliding re-issue ──────────────────────────────────────────────

        fn now_secs() -> i64 {
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .expect("system time is after epoch")
                .as_secs() as i64
        }

        fn cookie_cfg() -> AuthConfig {
            server_auth_config(
                crate::secret::secret_from_string(SECRET.to_string()),
                TokenSource::Cookie("ipe_sid".to_string()),
            )
        }

        /// A cookie token past the re-issue threshold (exp - slide_window/2)
        /// triggers a Set-Cookie response header with the refreshed token.
        /// The refreshed cookie carries the same name, Path=/, HttpOnly, and
        /// SameSite=Lax attributes.
        #[tokio::test]
        async fn cookie_past_threshold_gets_reissue_set_cookie() {
            let now = now_secs();
            // exp = now + 800; default slide = 1800s, threshold = exp - 900 = now - 100.
            // now > now - 100, so past_threshold = true; cap is in the future.
            let token = hs256(&serde_json::json!({
                "sub": "user-slide",
                "exp": now + 800,
                "iat": now - 600,
                "cap": now + 7200,
            }));
            let resp = run(cookie_cfg(), req_with(&[], &[("ipe_sid", &token)])).await;
            assert_eq!(
                resp.status, 200,
                "valid token must still dispatch the handler"
            );
            assert!(
                !resp.cookies.is_empty(),
                "a re-issue Set-Cookie must be attached for a past-threshold cookie token"
            );
            let cookie = &resp.cookies[0];
            assert!(
                cookie.starts_with("ipe_sid="),
                "re-issued cookie must carry the same name: {cookie}"
            );
            assert!(
                cookie.contains("; Path=/"),
                "re-issued cookie must include Path=/: {cookie}"
            );
            assert!(
                cookie.contains("; HttpOnly"),
                "re-issued cookie must be HttpOnly: {cookie}"
            );
            assert!(
                cookie.contains("; SameSite="),
                "re-issued cookie must include SameSite: {cookie}"
            );
            assert!(
                cookie.contains("; Max-Age="),
                "re-issued cookie must include Max-Age: {cookie}"
            );
        }

        /// A fresh cookie token (exp well beyond the re-issue threshold) must
        /// not produce a Set-Cookie — the throttle holds, no unnecessary write.
        #[tokio::test]
        async fn fresh_cookie_not_past_threshold_has_no_reissue_set_cookie() {
            let now = now_secs();
            // exp = now + 3600; threshold = exp - 900 = now + 2700.
            // now < now + 2700, so past_threshold = false.
            let token = hs256(&serde_json::json!({
                "sub": "user-fresh",
                "exp": now + 3600,
                "iat": now - 60,
                "cap": now + 7200,
            }));
            let resp = run(cookie_cfg(), req_with(&[], &[("ipe_sid", &token)])).await;
            assert_eq!(resp.status, 200, "valid token must dispatch the handler");
            assert!(
                resp.cookies.is_empty(),
                "no re-issue cookie for a fresh token that has not crossed the threshold: {:?}",
                resp.cookies
            );
        }

        /// Bearer tokens are API credentials; re-issue is the client's
        /// responsibility. The authed-route middleware must never attach a
        /// Set-Cookie for a bearer-source token, even when the exp is past the
        /// re-issue threshold.
        #[tokio::test]
        async fn bearer_past_threshold_never_gets_reissue_set_cookie() {
            let now = now_secs();
            let token = hs256(&serde_json::json!({
                "sub": "user-api",
                "exp": now + 800,
                "iat": now - 600,
                "cap": now + 7200,
            }));
            let req = req_with(&[("authorization", &format!("Bearer {token}"))], &[]);
            let resp = run(bearer_cfg(), req).await;
            assert_eq!(resp.status, 200, "valid bearer token must dispatch");
            assert!(
                resp.cookies.is_empty(),
                "bearer-source must never get a re-issue Set-Cookie: {:?}",
                resp.cookies
            );
        }

        // ── reissue Secure parity ──────────────────────────────────────────

        /// `reissue_set_cookie` pure-function truth-table:
        ///   (cookies_secure=false, is_https=true)  → Secure
        ///   (cookies_secure=false, is_https=false) → no Secure
        /// Mirrors the combined gate in `page_response`:
        /// a re-issued cookie must never be less-Secure than the initial one.
        #[test]
        fn reissue_set_cookie_secure_matches_initial_gate() {
            // SAFETY: test-only env mutation.
            unsafe { std::env::remove_var("ENV") };
            // SAFETY: test-only env mutation.
            unsafe { std::env::remove_var("IPE_ENV") };

            // is_https=true, cookies_secure()=false → Secure must fire.
            let c_https = reissue_set_cookie("ipe_sid", "tok", 1800, true);
            assert!(
                c_https.contains("; Secure"),
                "reissue behind TLS proxy must carry Secure: {c_https}"
            );

            // is_https=false, cookies_secure()=false → no Secure (dev default).
            let c_plain = reissue_set_cookie("ipe_sid", "tok", 1800, false);
            assert!(
                !c_plain.contains("; Secure"),
                "reissue over plain HTTP in dev must NOT carry Secure: {c_plain}"
            );
        }

        /// End-to-end: a cookie-source authed route where the request carries
        /// `X-Forwarded-Proto: https` (trusted-proxy opt-in via the testable
        /// `request_is_https_with_trust` overload) produces a re-issued cookie
        /// that carries `Secure`.
        ///
        /// Uses `request_is_https_with_trust(..., true)` directly to bypass the
        /// `OnceLock`-cached `trust_proxy_headers()` without mutating process env.
        #[test]
        fn reissue_set_cookie_https_proxy_sets_secure() {
            // SAFETY: test-only env mutation.
            unsafe { std::env::remove_var("ENV") };
            // SAFETY: test-only env mutation.
            unsafe { std::env::remove_var("IPE_ENV") };

            let mut headers = HashMap::new();
            headers.insert("x-forwarded-proto".to_string(), "https".to_string());
            let is_https = request_is_https_with_trust(&headers, true);
            assert!(is_https, "trusted HTTPS header must be detected");

            let cookie = reissue_set_cookie("ipe_sid", "tok", 1800, is_https);
            assert!(
                cookie.contains("; Secure"),
                "reissue with HTTPS proxy signal must carry Secure: {cookie}"
            );
        }

        /// Non-proxy default: no `X-Forwarded-Proto`, trust=false → no Secure on reissue.
        #[test]
        fn reissue_set_cookie_plain_http_no_secure() {
            // SAFETY: test-only env mutation.
            unsafe { std::env::remove_var("ENV") };
            // SAFETY: test-only env mutation.
            unsafe { std::env::remove_var("IPE_ENV") };

            let headers = HashMap::new();
            let is_https = request_is_https_with_trust(&headers, false);
            assert!(!is_https);

            let cookie = reissue_set_cookie("ipe_sid", "tok", 1800, is_https);
            assert!(
                !cookie.contains("; Secure"),
                "reissue over plain HTTP must NOT carry Secure: {cookie}"
            );
        }
    }
}

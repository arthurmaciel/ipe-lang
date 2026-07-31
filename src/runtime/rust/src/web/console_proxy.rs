//! Pre-built console child + reverse-proxy.
//!
//! Replaces the in-process `console.rs` plain-HTML shell with the **real bundled
//! Ipe.Web console**, spawned as a child process and reverse-proxied at
//! `/_ipe/console/*`. The console binary is **pre-built at the user's `ipe build`
//! time** into a shared cache — at runtime this module only `exec`s it,
//! never builds. See `runtime-rust/README.md` §"Rust vs Go — divergent strategies"
//! for why Rust takes the separate-process path Go abandoned (Go's subprocess
//! OOM was a *runtime* `go build`, which a pre-built binary doesn't incur).
//!
//! This module: gate + spawn + lifecycle + the reverse-proxy handler.
//!
//! No panic vectors: a missing binary / spawn failure / disabled gate returns
//! `None` so the caller falls back to the in-process console; no `unwrap`.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

/// Override for the pre-built console binary path. When unset, the cache path
/// (`~/.cache/ipe/rust-console/<ipe-version>/ipe-console`, written at build time) is used.
const CONSOLE_BIN_ENV: &str = "IPE_CONSOLE_BIN";

/// The mount prefix. The parent proxies everything under this path to the child
/// and STRIPS the prefix before forwarding (the strip convention — see the
/// module doc): the child's router stays root-relative, identical to a
/// standalone Web app. The child only learns the prefix via `IPE_LIVE_BASE_PATH`
/// (so its rendered `/_ipe/event` / `/_ipe/sse` URLs come back prefixed and we
/// strip them again on the way in).
const CONSOLE_BASE: &str = "/_ipe/console";

/// Request-body buffer cap for the proxy (16 MiB). Event POST bodies are far
/// smaller (`IPE_LIVE_MAX_BODY_BYTES` defaults to 5 MiB); this is the hard
/// ceiling above which we 502 rather than buffer unboundedly. Responses are
/// STREAMED, never buffered, so SSE is unaffected by this cap.
const MAX_PROXY_BODY: usize = 16 * 1024 * 1024;

/// Readiness-wait ceiling after spawn before we declare the child live (else we
/// fall back to the in-process console). Bounded so a wedged child can't hang
/// boot.
const READY_TIMEOUT: Duration = Duration::from_secs(8);

/// The spawned console child, tracked so the parent can kill it on shutdown
/// (Go's `ShutdownSubApps` equivalent — Go deleted it when it went in-process;
/// the separate-process Rust path needs it back to avoid an orphan child).
static CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// Resolve the pre-built console binary path: `IPE_CONSOLE_BIN`, else the
/// version-keyed cache path the build step populates. `None` when neither
/// exists (→ the caller falls back to the in-process console; first build
/// before the console is pre-built, or a build env where it couldn't be).
pub fn console_bin_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = crate::system::read_env_var(CONSOLE_BIN_ENV)
        && !p.is_empty()
    {
        let pb = std::path::PathBuf::from(p);
        return if pb.is_file() { Some(pb) } else { None };
    }
    // Key on the IPE compiler version (same source as `/_ipe/buildinfo`), NOT
    // the generated crate's CARGO_PKG_VERSION (always "0.1.0"). The ipe build
    // sets IPE_VERSION when compiling this app, and Ipe.Build.Rust.Console
    // caches the console binary under the SAME version — so both agree on the
    // `~/.cache/ipe/rust-console/<ver>/ipe-console` path.
    let ver = option_env!("IPE_VERSION").unwrap_or("dev");
    let home = crate::system::read_env_var("HOME").ok()?;
    let pb = std::path::Path::new(&home)
        .join(".cache/ipe/rust-console")
        .join(ver)
        .join("ipe-console");
    if pb.is_file() { Some(pb) } else { None }
}

/// Boot-time decision: should the console child be spawned + mounted at all?
/// Mirrors Go `MountEmbeddedConsole`'s skip conditions (console.go:257).
/// `false` → the caller skips the proxy (and may mount the in-process console or
/// nothing, per its own gate).
pub fn gate_allows() -> bool {
    // Sub-app context: the parent owns its own console; a nested app must not
    // recursively mount one.
    if crate::system::read_env_var("IPE_LIVE_BASE_PATH")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    // Explicit opt-outs.
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
    // Production without an admin token → no silent open-to-the-world mount.
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

/// Spawn the pre-built console child on `child_port`, pointing it at the data
/// `store`. Returns `Some(())` on a successful spawn (the `Child` is tracked in
/// `CHILD`); `None` when the binary is absent or the spawn fails — the caller
/// falls back to the in-process console.
///
/// `store` is the SQLite file the console renders from (`IPE_CONSOLE_HUB_DB` →
/// hubStore). `child_collects` selects who WRITES it:
///   - `true`  — push-to-local-collector: a lean parent has no spill,
///     so the child is the collector — it also writes `store`
///     (`IPE_CONSOLE_DB_PATH`) from the parent's pushed telemetry.
///   - `false` — the parent writes `store` directly (db parent's own spill); the
///     child reads only, and MUST NOT also write it (double-write).
///
/// No-orphan defence in depth: `kill_on_drop` + `shutdown_console` (signal
/// handler) cover the graceful paths, and on Linux `PR_SET_PDEATHSIG` makes the
/// kernel SIGTERM the child if the parent dies by ANY means — including SIGKILL
/// / OOM / a crash the signal handler can't catch.
pub fn spawn_console(child_port: u16, store: &str, child_collects: bool) -> Option<()> {
    let bin = console_bin_path()?;
    let mut cmd = Command::new(&bin);
    cmd.env("IPE_LIVE_PORT", child_port.to_string())
        .env("IPE_LIVE_BASE_PATH", "/_ipe/console")
        // Belt-and-braces: suppress the child's own console auto-mount + banner.
        .env("IPE_CONSOLE_EMBED", "off")
        .kill_on_drop(true);
    // hubStore read source.
    if store.is_empty() {
        cmd.env_remove("IPE_CONSOLE_HUB_DB");
    } else {
        cmd.env("IPE_CONSOLE_HUB_DB", store);
    }
    // Collector write source: only when the child collects (parent pushes).
    // env_remove otherwise so an inherited IPE_CONSOLE_DB_PATH (the parent's own
    // spill path) doesn't make the child double-write it.
    if child_collects && !store.is_empty() {
        cmd.env("IPE_CONSOLE_DB_PATH", store);
    } else {
        cmd.env_remove("IPE_CONSOLE_DB_PATH");
    }
    // Linux: parent-death signal. If the parent process dies for ANY reason
    // (SIGKILL, OOM, panic-abort) the kernel delivers SIGTERM to this child, so
    // it can never outlive the parent as an orphan. No-op on non-Linux (the
    // signal handler + kill_on_drop cover those).
    #[cfg(target_os = "linux")]
    {
        // SAFETY: the closure runs in the forked child between fork and exec. It
        // only calls prctl (async-signal-safe) — no allocation, no locks, no
        // Rust runtime re-entry. Failure is non-fatal (best-effort hardening).
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong);
                Ok(())
            });
        }
    }
    match cmd.spawn() {
        Ok(child) => {
            if let Ok(mut g) = CHILD.lock() {
                *g = Some(child);
            }
            eprintln!(
                "[ipe.console] spawned console child on :{child_port} (bin {})",
                bin.display()
            );
            Some(())
        }
        Err(e) => {
            eprintln!("[ipe.console] spawn failed ({e}); falling back to in-process console");
            None
        }
    }
}

/// Kill the tracked console child (parent shutdown). Idempotent; never panics.
pub fn shutdown_console() {
    if let Ok(mut g) = CHILD.lock() {
        if let Some(child) = g.as_mut() {
            let _ = child.start_kill();
        }
        *g = None;
    }
}

// NOTE (shutdown ownership): the console child's teardown on parent shutdown is
// owned by the ONE coherent graceful-shutdown path in `web::web_shutdown_signal`
// — it calls `shutdown_console()` then returns so axum drains and the process
// exits 0. A previous `install_shutdown_hook` here installed a SECOND tokio
// signal handler that `std::process::exit(130)`'d; two handlers raced and the
// 130 exit defeated the exit-0-on-clean-shutdown contract. It was removed. The
// `PR_SET_PDEATHSIG` (Linux) + `kill_on_drop` set in `spawn_console` remain the
// defense-in-depth floor for NON-graceful parent death (SIGKILL / OOM / crash).

// ─── Reverse proxy ──────────────────────────────────────────────────────────

/// Shared proxy state, initialised once when the proxy mounts: the upstream
/// origin (`http://127.0.0.1:<child_port>`) and a connection-pooling client.
struct ProxyState {
    client: reqwest::Client,
    upstream: String,
}

static PROXY: OnceLock<ProxyState> = OnceLock::new();

/// RFC 7230 §6.1 hop-by-hop headers (plus `host`, which reqwest derives from the
/// upstream URL, and `content-length`, which we drop because every proxied
/// response is re-encoded as a stream). Never forwarded in either direction.
fn is_hop_by_hop(name: &axum::http::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
            | "content-length"
    )
}

/// Build a small status-only error response without panicking.
fn error_response(status: axum::http::StatusCode, msg: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    (status, msg.to_string()).into_response()
}

/// Forward one request to `upstream`, STRIPPING the `/_ipe/console` prefix from
/// the path (strip convention). Response body is streamed, so SSE
/// (`/_ipe/console/_ipe/sse`) passes through without buffering. No panic
/// vectors: every fallible step degrades to a 502/503, never `unwrap`.
///
/// Factored to take `client` + `upstream` explicitly (rather than reading the
/// `PROXY` static) so it is unit-testable against a throwaway upstream without
/// touching global state.
async fn forward(
    client: &reqwest::Client,
    upstream: &str,
    req: axum::extract::Request,
) -> axum::response::Response {
    // Strip the mount prefix: `/_ipe/console` → `/`, `/_ipe/console/x` → `/x`.
    let path = req.uri().path();
    let rest = path.strip_prefix(CONSOLE_BASE).unwrap_or(path);
    let rest = if rest.is_empty() { "/" } else { rest };
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let url = format!("{upstream}{rest}{query}");

    let method = req.method().clone();
    let headers = req.headers().clone();
    let body_bytes = match axum::body::to_bytes(req.into_body(), MAX_PROXY_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return error_response(
                axum::http::StatusCode::BAD_GATEWAY,
                "console proxy: request body read failed",
            );
        }
    };

    let mut rb = client.request(method, &url);
    for (name, value) in headers.iter() {
        if is_hop_by_hop(name) {
            continue;
        }
        // The child performs NO auth (spawned with IPE_CONSOLE_EMBED=off). The
        // parent admin credential — validated by `console::gate_blocked` in the
        // `Authorization` header BEFORE this handler runs — is pure liability
        // downstream: the child can't act on it and would record it verbatim in
        // its telemetry store (request-header capture). Strip it so the gate
        // secret never crosses into the child. (`proxy-authorization` is already
        // dropped as hop-by-hop above.)
        if name.as_str() == "authorization" {
            continue;
        }
        rb = rb.header(name, value);
    }
    // Pass the buffered `Bytes` straight to reqwest (which impls `From<Bytes>`)
    // rather than cloning into a `Vec<u8>` — avoids a second full-body copy and
    // halves peak per-request memory for large proxied POSTs.
    let upstream_resp = match rb.body(body_bytes).send().await {
        Ok(r) => r,
        Err(_) => {
            return error_response(
                axum::http::StatusCode::BAD_GATEWAY,
                "console proxy: upstream unreachable",
            );
        }
    };

    let status = upstream_resp.status();
    let resp_headers = upstream_resp.headers().clone();
    let stream = upstream_resp.bytes_stream();
    let body = axum::body::Body::from_stream(stream);

    let mut builder = axum::response::Response::builder().status(status);
    for (name, value) in resp_headers.iter() {
        if is_hop_by_hop(name) {
            continue;
        }
        builder = builder.header(name, value);
    }
    match builder.body(body) {
        Ok(r) => r,
        Err(_) => error_response(
            axum::http::StatusCode::BAD_GATEWAY,
            "console proxy: malformed upstream response",
        ),
    }
}

/// axum route handler: forward via the mounted `PROXY` state. 503 if the proxy
/// somehow isn't initialised (can't happen once mounted, but degrade rather
/// than panic).
async fn proxy_entry(req: axum::extract::Request) -> axum::response::Response {
    // Per-request auth, defense-in-depth. The PRIMARY enforcement is the
    // outermost `observability::track` middleware, which routes every
    // `/_ipe/console*` request through `console::gate_blocked` (production →
    // Bearer admin token required; dev open) BEFORE it reaches this handler — so
    // the proxied path is already gated. This second call to the SAME gate keeps
    // the sensitive console surface protected even if a future router change ever
    // mounts the proxy outside that middleware. `gate_allows()` (mount-time) is
    // orthogonal: it decides whether to mount at all, not who may reach it.
    if let Some(blocked) = super::console::gate_blocked(req.headers()) {
        return blocked;
    }
    match PROXY.get() {
        Some(state) => forward(&state.client, &state.upstream, req).await,
        None => error_response(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "console proxy not initialised",
        ),
    }
}

/// Poll the child's TCP port until it accepts a connection or `timeout` elapses.
/// `true` = ready. Bounded so a child that never binds can't wedge boot.
async fn wait_ready(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Grab a free ephemeral loopback port by binding `:0` and reading the assigned
/// port. `None` if the OS won't hand one out. (Small TOCTOU window between drop
/// and the child's bind — same approach Go uses for sub-app ports.)
fn pick_free_port() -> Option<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    drop(listener);
    Some(port)
}

/// Gate → pick port → spawn the pre-built console child → wait for readiness →
/// init the proxy state + shutdown hook. Returns `true` when the reverse proxy
/// is live (the caller mounts `proxy_routes` instead of the in-process console);
/// `false` on gate-closed / binary-absent / spawn-fail / readiness-timeout
/// (caller mounts the in-process console fallback — no side effects left behind).
///
/// Reads the parent's `IPE_CONSOLE_DB_PATH` (the telemetry spill D writes) and
/// wires it to the child's `IPE_CONSOLE_HUB_DB` so the dashboard renders what
/// the parent recorded. Decided BEFORE the router is built so both the proxy and
/// the in-process fallback sit under the same observability middleware.
/// The console child's data store path. The user's `IPE_CONSOLE_DB_PATH` when
/// set (durable history at their chosen location), else an internal per-process
/// temp file so the console works zero-config (a lean app gets a live console
/// without configuring durability).
fn console_store_path() -> String {
    match crate::system::read_env_var("IPE_CONSOLE_DB_PATH") {
        Ok(p) if !p.is_empty() => p,
        // Default to a per-process file in the temp dir, but add an UNGUESSABLE
        // suffix: a bare `ipe-console-<pid>.db` is predictable, so a local
        // attacker on the shared temp dir could pre-create that path (or a
        // symlink) and hijack/redirect the console store (TOCTOU). The nonce is
        // OS-seeded via RandomState (std-only — no new crate in this shared
        // module). Computed once per process and passed to the child via env.
        _ => {
            use std::hash::{BuildHasher, Hasher};
            let nonce = std::collections::hash_map::RandomState::new()
                .build_hasher()
                .finish();
            std::env::temp_dir()
                .join(format!(
                    "ipe-console-{}-{:016x}.db",
                    std::process::id(),
                    nonce
                ))
                .to_string_lossy()
                .into_owned()
        }
    }
}

/// Whether THIS (parent) process writes the telemetry store directly via its own
/// spill (db app with `IPE_CONSOLE_DB_PATH`). When true the child reads only;
/// when false the parent pushes to the child collector. Always false without the
/// `db` feature (a lean live app can't spill).
fn parent_spill_active() -> bool {
    #[cfg(feature = "db")]
    {
        crate::telemetry_spill::is_enabled()
    }
    #[cfg(not(feature = "db"))]
    {
        false
    }
}

pub async fn ensure_console_proxy() -> bool {
    if !gate_allows() {
        return false;
    }
    // Fast path for the common case (binary not pre-built yet): skip the
    // port-pick + spawn entirely and let the in-process console serve.
    if console_bin_path().is_none() {
        return false;
    }
    // Console data store + who writes it (push-to-local-collector):
    //   - db parent (its own spill is active) → parent writes the store
    //     directly; the child only reads it. No push.
    //   - lean/memory parent → the child collects: the parent PUSHES its in-RAM
    //     telemetry to the child, which writes + reads the store.
    let store = console_store_path();
    let parent_writes = parent_spill_active();
    let port = match pick_free_port() {
        Some(p) => p,
        None => return false,
    };
    if spawn_console(port, &store, /* child_collects = */ !parent_writes).is_none() {
        // Binary absent (not pre-built / different ipe version) or spawn error.
        return false;
    }
    if !wait_ready(port, READY_TIMEOUT).await {
        eprintln!(
            "[ipe.console] child not ready within {READY_TIMEOUT:?}; falling back to in-process console"
        );
        shutdown_console();
        return false;
    }
    // Lean parent: start pushing our telemetry to the child collector now that
    // its ingest is up. (db parent already wrote the store directly.)
    if !parent_writes {
        super::push_exporter::enable_to_console(port).await;
    }
    // Bound the upstream hop so a wedged child can't accumulate in-flight
    // requests without limit. `connect_timeout` caps the TCP handshake; a
    // `read_timeout` (per-read inactivity, NOT a total `.timeout`) caps a child
    // that accepts the connection then stalls — set well above the Ipe.Web SSE
    // heartbeat (~15 s) + TTL (~35 s) so long-lived `/_ipe/sse` streams are not
    // severed. `.build()` only fails on a TLS-backend init error (we use none
    // for loopback http); fall back to the default client rather than panic.
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .read_timeout(Duration::from_secs(60))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    if PROXY
        .set(ProxyState {
            client,
            upstream: format!("http://127.0.0.1:{port}"),
        })
        .is_err()
    {
        // Already initialised once (shouldn't happen — one Web server per process).
        eprintln!("[ipe.console] proxy already initialised; keeping the first mount");
    }
    // NOTE: child teardown on shutdown is now owned by the ONE coherent
    // graceful-shutdown path in `web::web_shutdown_signal` (it calls
    // `shutdown_console()` then lets axum drain → exit 0). We deliberately do
    // NOT install the old `install_shutdown_hook` here — two signal handlers
    // would race, and that hook's `std::process::exit(130)` would defeat the
    // exit-0-on-clean-shutdown contract. The PR_SET_PDEATHSIG + kill_on_drop on
    // the child remain the defense-in-depth floor for non-graceful parent death.
    eprintln!("[ipe.console] reverse-proxy ready at {CONSOLE_BASE}/* → 127.0.0.1:{port}");
    true
}

/// Add the reverse-proxy routes (`/_ipe/console` + `/_ipe/console/*rest`) to a
/// router. Generic over the app state `S` because `proxy_entry` is state-free —
/// so this composes into the main `Router<WebState<…>>` before `with_state`,
/// keeping the proxy under the same `track` middleware as every other route.
/// Call only when `ensure_console_proxy().await` returned `true`.
pub fn proxy_routes<S>(router: axum::Router<S>) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    use axum::routing::any;
    router
        .route(CONSOLE_BASE, any(proxy_entry))
        .route(&format!("{CONSOLE_BASE}/*rest"), any(proxy_entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_skips_in_subapp_context() {
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("IPE_LIVE_BASE_PATH", "/billing") };
        assert!(!gate_allows());
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_LIVE_BASE_PATH") };
    }

    #[test]
    fn gate_skips_on_explicit_off() {
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("IPE_CONSOLE_EMBED", "off") };
        assert!(!gate_allows());
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_CONSOLE_EMBED") };
    }

    #[test]
    fn bin_path_none_when_absent() {
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var(CONSOLE_BIN_ENV, "/nonexistent/ipe-console-xyz") };
        assert!(console_bin_path().is_none());
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var(CONSOLE_BIN_ENV) };
    }

    #[test]
    fn spawn_returns_none_without_binary() {
        // No binary at the override path → None (caller falls back), no panic.
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var(CONSOLE_BIN_ENV, "/nonexistent/ipe-console-xyz") };
        assert!(spawn_console(9931, "", false).is_none());
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var(CONSOLE_BIN_ENV) };
    }

    #[test]
    fn shutdown_is_idempotent_noop_when_empty() {
        shutdown_console();
        shutdown_console();
    }

    #[test]
    fn pick_free_port_returns_a_port() {
        let p = pick_free_port();
        assert!(p.is_some());
        assert!(p.unwrap_or(0) > 0);
    }

    #[test]
    fn hop_by_hop_filters_connection_not_content_type() {
        use axum::http::header::{CONNECTION, CONTENT_TYPE};
        assert!(is_hop_by_hop(&CONNECTION));
        assert!(!is_hop_by_hop(&CONTENT_TYPE));
    }

    // Spin a throwaway upstream that echoes "METHOD PATH BODY", forward a
    // parent-shaped request through `forward`, and assert the /_ipe/console
    // prefix is stripped while method + body + query round-trip.
    #[tokio::test]
    async fn forward_strips_prefix_and_round_trips() {
        use axum::{Router, routing::any};

        async fn echo(req: axum::extract::Request) -> String {
            let method = req.method().clone();
            let uri = req.uri().clone();
            let body = axum::body::to_bytes(req.into_body(), 1 << 20)
                .await
                .unwrap_or_default();
            format!(
                "{method} {}{} {}",
                uri.path(),
                uri.query().map(|q| format!("?{q}")).unwrap_or_default(),
                String::from_utf8_lossy(&body)
            )
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let port = listener.local_addr().expect("addr").port();
        let app = Router::new().fallback(any(echo));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        // The parent receives POST /_ipe/console/_ipe/event?x=1 with body "hi".
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/_ipe/console/_ipe/event?x=1")
            .body(axum::body::Body::from("hi"))
            .expect("build req");

        let client = reqwest::Client::new();
        let upstream = format!("http://127.0.0.1:{port}");
        let resp = forward(&client, &upstream, req).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .expect("read resp body");
        let text = String::from_utf8_lossy(&bytes);
        // Prefix stripped → child sees /_ipe/event; method, query, body preserved.
        assert_eq!(text, "POST /_ipe/event?x=1 hi", "got: {text}");
    }

    // The bare mount path `/_ipe/console` (no trailing slash) maps to the
    // child's root `/`.
    #[tokio::test]
    async fn forward_bare_base_maps_to_root() {
        use axum::{Router, routing::any};

        async fn echo_path(req: axum::extract::Request) -> String {
            req.uri().path().to_string()
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let port = listener.local_addr().expect("addr").port();
        let app = Router::new().fallback(any(echo_path));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/_ipe/console")
            .body(axum::body::Body::empty())
            .expect("build req");

        let client = reqwest::Client::new();
        let upstream = format!("http://127.0.0.1:{port}");
        let resp = forward(&client, &upstream, req).await;
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .expect("read resp body");
        assert_eq!(String::from_utf8_lossy(&bytes), "/");
    }
}

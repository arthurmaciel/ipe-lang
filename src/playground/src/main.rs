//! Ipê playground backend — the opt-in "Run for real" tier.
//!
//! # Architecture
//!
//! One HTTP endpoint:
//!
//! - `POST /compile` — accepts `{"source": "<Ipê source text>"}`, writes the
//!   source to a per-request scratch directory, runs `ipe build --target wasm`
//!   inside a hardened sandbox, and returns the compiled bundle or a diagnostic
//!   on error.
//!
//! The bundle response carries the files the WASM browser runtime needs:
//! `www/index.html`, `www/boot.js`, `www/pkg/ipe_app.js`, and
//! `www/pkg/ipe_app_bg.wasm` (base64-encoded), plus the compile diagnostics
//! string on failure.
//!
//! # Opt-in — off by default
//!
//! The build endpoint runs `cargo build` on a crate emitted from UNTRUSTED user
//! input. Compiling Rust executes native code at build time (`build.rs`,
//! proc-macros) regardless of `--target wasm32`, so the build is an RCE surface.
//! Because of that, the endpoint is **off by default**. It is enabled only when
//! `IPE_PLAYGROUND_RUN=1` AND the host can host a sound sandbox
//! ([`sandbox_build::probe_build_jail`]). When it is not enabled the server
//! still serves the static playground UI (which compiles Ipê in the browser and
//! needs no backend), and `/compile` returns a typed "disabled" response — so
//! the default deployment has no build surface at all.
//!
//! # Sandbox — the BUILD is jailed
//!
//! When enabled, every build runs inside the repo's hardened `ipe_sandbox`
//! primitive (the same bubblewrap + prlimit jail the FFI untrusted-crate path
//! uses): a fresh empty network namespace (no egress), a read-only `/`, a
//! single writable per-request scratch dir (+ a shared warm dependency target),
//! `--clearenv`, and prlimit caps on address space / CPU / file descriptors /
//! process count / file size, all under a `timeout` wall clock. A hostile
//! `build.rs` is contained: no network, confined filesystem, resource-killed.
//! See `sandbox_build` and `docs/internals/playground-run.md`.
//!
//! # Offline, fixed deps
//!
//! The emitted crate depends only on the in-repo vendored runtime plus a fixed
//! set of browser-safe crates (the playground gates to the `WasmClient` target,
//! so no FFI foreign crates enter). The jailed build runs offline
//! (`CARGO_NET_OFFLINE=1`) against a warm target that already holds those deps
//! — so no crates.io fetch can pull a hostile transitive `build.rs`.
//!
//! # Static playground UI
//!
//! If `IPE_PLAYGROUND_STATIC_DIR` is set, the server also serves a static
//! directory at `/` (the playground front-end HTML/JS).

#![allow(clippy::module_name_repetitions)] // AppState, CompileRequest etc. are clear
#![allow(clippy::missing_errors_doc)] // internal helpers, not public API

mod sandbox_build;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use ipe_sandbox::Capabilities;
use sandbox_build::{BuildToolchain, JailedBuild, build_limits, run_jailed_build};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Default per-build wall-clock timeout (seconds).
const DEFAULT_COMPILE_TIMEOUT_SECS: u64 = 120;

/// Maximum source file size accepted (1 MiB). Larger payloads are rejected
/// before spawning a subprocess, so a malicious oversized request is cheap.
const MAX_SOURCE_BYTES: usize = 1024 * 1024;

/// The build path, present only when the endpoint is enabled AND the host has a
/// sound sandbox. Its absence is the OFF state: `/compile` refuses.
#[derive(Clone)]
struct RunConfig {
    /// The probed host sandbox tools (bwrap / prlimit / timeout).
    caps: Capabilities,
    /// Toolchain + warm-target paths bound into the jail.
    toolchain: BuildToolchain,
    /// Per-build wall-clock cap (seconds).
    wall_secs: u64,
}

/// Resolved server configuration, populated once at startup.
#[derive(Clone)]
struct AppState {
    /// The sandboxed-build configuration, or `None` when the endpoint is off
    /// (the default). When `None`, `/compile` returns a typed "disabled"
    /// response and never spawns a build.
    run: Option<Arc<RunConfig>>,
}

// ── Request / response shapes ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct CompileRequest {
    source: String,
}

/// The compiled bundle returned on success: each file is base64-encoded so the
/// JSON remains text-only.
#[derive(Serialize)]
struct CompileSuccess {
    /// `www/index.html`
    index_html: String,
    /// `www/boot.js`
    boot_js: String,
    /// `www/pkg/ipe_app.js`  (wasm-bindgen glue)
    pkg_js: String,
    /// `www/pkg/ipe_app_bg.wasm` (binary, base64-encoded)
    pkg_wasm_b64: String,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
enum CompileResponse {
    Ok(CompileSuccess),
    Error { diagnostics: String },
    /// The build endpoint is not enabled on this server. The static playground
    /// (browser-only compile + diagnostics) remains fully usable; only the
    /// server-side "Run for real" build is off.
    Disabled { reason: String },
}

// ── Error handling ────────────────────────────────────────────────────────────

struct AppError(String);

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({ "status": "error", "diagnostics": self.0 })),
        )
            .into_response()
    }
}

// ── /compile handler ──────────────────────────────────────────────────────────

async fn compile(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<CompileRequest>,
) -> Result<axum::Json<CompileResponse>, AppError> {
    // Off by default: no build surface unless explicitly enabled with a sound
    // sandbox. Refuse cheaply, before touching the filesystem.
    let Some(run) = state.run.clone() else {
        return Ok(axum::Json(CompileResponse::Disabled {
            reason: "server-side build is disabled; set IPE_PLAYGROUND_RUN=1 with a working \
                     sandbox (bwrap + prlimit + timeout) to enable it. The browser playground \
                     compiles Ipê client-side with no backend."
                .to_owned(),
        }));
    };

    // Guard: source size limit (before spawning anything).
    if req.source.len() > MAX_SOURCE_BYTES {
        return Ok(axum::Json(CompileResponse::Error {
            diagnostics: format!(
                "source too large ({} bytes, limit {} bytes)",
                req.source.len(),
                MAX_SOURCE_BYTES
            ),
        }));
    }

    // Fresh per-request scratch dir: the ONLY primary writable mount in the
    // jail. The user source + emitted crate live here and are dropped after.
    let tmpdir = tempfile::TempDir::new()?;
    let scratch = tmpdir.path().to_path_buf();
    let src_dir = scratch.join("src");
    std::fs::create_dir_all(&src_dir)?;
    let entry = src_dir.join("Main.ipe");
    {
        let mut f = std::fs::File::create(&entry)?;
        f.write_all(req.source.as_bytes())?;
    }
    let out_dir = scratch.join("out");

    info!(
        "sandboxed build of source ({} bytes) in {:?}",
        req.source.len(),
        scratch
    );

    // The jailed build is blocking (spawn + wait); run it off the async
    // executor so it cannot starve the reactor.
    let source_len = req.source.len();
    let result = tokio::task::spawn_blocking(move || {
        let job = JailedBuild {
            scratch: &scratch,
            entry: &entry,
            out_dir: &out_dir,
            limits: build_limits(run.wall_secs),
        };
        let jailed = run_jailed_build(&run.caps, &run.toolchain, &job)?;
        Ok::<_, sandbox_build::BuildJailError>((jailed, out_dir))
    })
    .await
    .map_err(|e| AppError(format!("build task panicked: {e}")))?;

    let _ = source_len;
    match result {
        Ok((jailed, out_dir)) => {
            let stderr = String::from_utf8_lossy(&jailed.stderr);
            if jailed.status == Some(0) {
                if !stderr.trim().is_empty() {
                    info!("build stderr (success): {stderr}");
                }
                match read_bundle(&out_dir) {
                    Ok(bundle) => Ok(axum::Json(CompileResponse::Ok(bundle))),
                    Err(diag) => Ok(axum::Json(CompileResponse::Error { diagnostics: diag })),
                }
            } else {
                // Non-zero (or signal-killed) exit inside the jail: a compile
                // failure or a resource kill. Surface the captured diagnostics.
                let code = jailed
                    .status
                    .map_or_else(|| "killed (signal / wall clock)".to_owned(), |c| c.to_string());
                warn!("jailed build exited {code}");
                Ok(axum::Json(CompileResponse::Error {
                    diagnostics: format!("build failed (exit {code})\n{stderr}"),
                }))
            }
        }
        Err(e) => {
            error!("sandbox error: {e}");
            Err(AppError(format!("sandbox error: {e}")))
        }
    }
}

/// Read the five bundle files from `out_dir/www/`.
fn read_bundle(out_dir: &Path) -> Result<CompileSuccess, String> {
    let www = out_dir.join("www");

    let index_html = read_text(&www.join("index.html"))?;
    let boot_js = read_text(&www.join("boot.js"))?;
    let pkg_js = read_text(&www.join("pkg").join("ipe_app.js"))?;
    let wasm_bytes = read_bytes(&www.join("pkg").join("ipe_app_bg.wasm"))?;
    let pkg_wasm_b64 = base64_encode(&wasm_bytes);

    Ok(CompileSuccess {
        index_html,
        boot_js,
        pkg_js,
        pkg_wasm_b64,
    })
}

fn read_text(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|e| format!("missing bundle file {}: {e}", path.display()))
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("missing bundle file {}: {e}", path.display()))
}

/// Minimal base64 encoder (RFC 4648, no padding variation).
///
/// Using a hand-rolled encoder rather than pulling in the `base64` crate keeps
/// the dependency tree minimal. The alphabet is the standard RFC 4648 table.
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    // Index the alphabet through a helper so a 6-bit value maps to a char with
    // no possibility of an out-of-bounds panic (the mask already bounds it to
    // 0..64, but the compiler cannot see that through `[]`).
    let sextet = |v: u32| TABLE.get((v & 0x3f) as usize).map_or('=', |&b| b as char);
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk.first().copied().map_or(0, u32::from);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(sextet(n >> 18));
        out.push(sextet(n >> 12));
        if chunk.len() >= 2 {
            out.push(sextet(n >> 6));
        } else {
            out.push('=');
        }
        if chunk.len() == 3 {
            out.push(sextet(n));
        } else {
            out.push('=');
        }
    }
    out
}

// ── Startup ───────────────────────────────────────────────────────────────────

/// Resolve the `ipe` binary path: `IPE_BIN` env var, or search `PATH`.
fn resolve_ipe_bin() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("IPE_BIN") {
        return Ok(PathBuf::from(p));
    }
    // Walk PATH entries.
    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in path_var.split(':') {
        let candidate = PathBuf::from(dir).join("ipe");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("`ipe` binary not found; set IPE_BIN or ensure `ipe` is on PATH".to_owned())
}

/// Resolve the runtime directory: `IPE_RUNTIME_DIR` env var, or the default
/// relative to the workspace root found by walking up from the binary.
fn resolve_runtime_dir() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("IPE_RUNTIME_DIR") {
        let path = PathBuf::from(p);
        if path.is_dir() {
            return Ok(path);
        }
        return Err(format!(
            "IPE_RUNTIME_DIR={} is not a directory",
            path.display()
        ));
    }
    Err("runtime directory not found; set IPE_RUNTIME_DIR to the \
         ipe_runtime source directory (src/runtime/rust/src)"
        .to_owned())
}

/// Resolve the toolchain paths the jailed build binds read-only: the cargo
/// home (`~/.cargo`, holding the cargo/rustc/wasm-bindgen proxies + bin) and
/// the rustup home (`~/.rustup`, holding the actual toolchains + the wasm32
/// sysroot). Honours `CARGO_HOME` / `RUSTUP_HOME`, else the conventional
/// `~/.cargo` / `~/.rustup`.
///
/// Both are re-exposed read-only in the jail *after* the `/home` tmpfs mask, so
/// the build can execute the toolchain but never mutate it.
fn resolve_toolchain(ipe_bin: PathBuf, runtime_dir: PathBuf, warm_target_dir: PathBuf) -> Result<BuildToolchain, String> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".cargo")))
        .ok_or("cannot resolve CARGO_HOME or ~/.cargo; set CARGO_HOME")?;
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".rustup")))
        .ok_or("cannot resolve RUSTUP_HOME or ~/.rustup; set RUSTUP_HOME")?;
    if !cargo_home.is_dir() {
        return Err(format!("CARGO_HOME {} is not a directory", cargo_home.display()));
    }
    if !rustup_home.is_dir() {
        return Err(format!("RUSTUP_HOME {} is not a directory", rustup_home.display()));
    }
    let cargo_bin = cargo_home.join("bin");
    Ok(BuildToolchain {
        ipe_bin,
        runtime_dir,
        toolchain_ro_binds: vec![cargo_home.clone(), rustup_home.clone()],
        path_prepend: vec![cargo_bin],
        rustup_home: Some(rustup_home),
        warm_target_dir,
    })
}

/// Assemble the opt-in build config, or `None` (endpoint off) with a logged
/// reason. The endpoint is enabled ONLY when `IPE_PLAYGROUND_RUN=1` AND the
/// host has a sound sandbox — fail-closed on anything missing.
fn resolve_run_config() -> Option<RunConfig> {
    if std::env::var_os("IPE_PLAYGROUND_RUN").is_none_or(|v| v != "1") {
        info!(
            "server-side build DISABLED (IPE_PLAYGROUND_RUN != 1); serving the browser \
             playground only, with no build surface"
        );
        return None;
    }
    // Probe for a sound sandbox — refuse to enable an unsandboxed build.
    let caps = match sandbox_build::probe_build_jail() {
        Ok(c) => c,
        Err(e) => {
            error!("IPE_PLAYGROUND_RUN=1 but {e}");
            error!("REFUSING to enable the build endpoint unsandboxed — it stays OFF");
            return None;
        }
    };
    let ipe_bin = match resolve_ipe_bin() {
        Ok(p) => p,
        Err(e) => {
            error!("IPE_PLAYGROUND_RUN=1 but {e}; endpoint stays OFF");
            return None;
        }
    };
    let runtime_dir = match resolve_runtime_dir() {
        Ok(p) => p,
        Err(e) => {
            error!("IPE_PLAYGROUND_RUN=1 but {e}; endpoint stays OFF");
            return None;
        }
    };
    let warm_target_dir = std::env::var_os("IPE_PLAYGROUND_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/ipe-playground-target"));
    if let Err(e) = std::fs::create_dir_all(&warm_target_dir) {
        error!("cannot create warm target dir {}: {e}; endpoint stays OFF", warm_target_dir.display());
        return None;
    }
    let toolchain = match resolve_toolchain(ipe_bin, runtime_dir, warm_target_dir) {
        Ok(t) => t,
        Err(e) => {
            error!("IPE_PLAYGROUND_RUN=1 but toolchain unresolved: {e}; endpoint stays OFF");
            return None;
        }
    };
    let wall_secs = std::env::var("IPE_PLAYGROUND_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_COMPILE_TIMEOUT_SECS);
    info!(
        "server-side build ENABLED (sandboxed): ipe={:?} runtime={:?} warm_target={:?} wall={}s",
        toolchain.ipe_bin, toolchain.runtime_dir, toolchain.warm_target_dir, wall_secs
    );
    info!(
        "jail: bwrap={:?} prlimit={:?} timeout={:?} — network denied, / read-only, per-request scratch",
        caps.bwrap, caps.prlimit, caps.timeout
    );
    Some(RunConfig {
        caps,
        toolchain,
        wall_secs,
    })
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("ipe_playground=info".parse().unwrap_or_default()),
        )
        .init();

    // Opt-in build config (off by default). When off, the server still serves
    // the browser playground; only the server-side build is disabled.
    let state = AppState {
        run: resolve_run_config().map(Arc::new),
    };

    let mut app = Router::new()
        .route("/compile", post(compile))
        .with_state(state)
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods([axum::http::Method::POST, axum::http::Method::OPTIONS])
                .allow_headers([axum::http::header::CONTENT_TYPE]),
        );

    // Optionally serve a static playground UI.
    if let Ok(static_dir) = std::env::var("IPE_PLAYGROUND_STATIC_DIR") {
        info!("serving static playground UI from {:?}", static_dir);
        app = app.nest_service("/", tower_http::services::ServeDir::new(static_dir));
    }

    let port = std::env::var("IPE_PLAYGROUND_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(3000);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    info!("listening on {addr}");

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("failed to bind {addr}: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        error!("server error: {e}");
        std::process::exit(1);
    }
}

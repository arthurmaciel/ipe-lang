//! Ipê playground backend — B1 server-compile-then-ship-WASM tier.
//!
//! # Architecture
//!
//! One HTTP endpoint:
//!
//! - `POST /compile` — accepts `{"source": "<Ipê source text>"}`, writes the
//!   source to a temp directory, runs `ipe build --target wasm` as a
//!   subprocess, and returns the compiled bundle or a diagnostic on error.
//!
//! The bundle response carries the five files the WASM browser runtime needs:
//! `www/index.html`, `www/boot.js`, `www/pkg/ipe_app.js`, and
//! `www/pkg/ipe_app_bg.wasm` (base64-encoded), plus the compile diagnostics
//! string on failure.
//!
//! # Isolation
//!
//! Each compile runs as a separate subprocess. A configurable `COMPILE_TIMEOUT`
//! (default 120 s) kills the subprocess and returns a timeout error. CPU/memory
//! limits are supplied by the operating environment (cgroups / container
//! runtime); this process imposes no further kernel-level sandboxing — that is
//! a deployment concern, not a library concern.
//!
//! # Warmth
//!
//! A single `CARGO_TARGET_DIR` is shared across all compile requests (set via
//! `IPE_PLAYGROUND_TARGET_DIR` or defaulting to `/tmp/ipe-playground-target`).
//! The runtime rlib, wasm-bindgen-cli, and all dependency crates are compiled
//! once and reused. `sccache` is respected if `RUSTC_WRAPPER=sccache` is set
//! in the environment.
//!
//! # Static playground UI
//!
//! If `IPE_PLAYGROUND_STATIC_DIR` is set, the server also serves a static
//! directory at `/` (the playground front-end HTML/JS). This is optional —
//! the compile endpoint is fully usable standalone.

#![allow(clippy::module_name_repetitions)] // AppState, CompileRequest etc. are clear
#![allow(clippy::missing_errors_doc)] // internal helpers, not public API

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{error, info, warn};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Default compile subprocess timeout.
const DEFAULT_COMPILE_TIMEOUT_SECS: u64 = 120;

/// Maximum source file size accepted (1 MiB). Larger payloads are rejected
/// before spawning a subprocess, so a malicious oversized request is cheap.
const MAX_SOURCE_BYTES: usize = 1024 * 1024;

/// Resolved server configuration, populated once at startup.
#[derive(Clone)]
struct AppState {
    /// Absolute path to the `ipe` binary.
    ipe_bin: PathBuf,
    /// Absolute path to the compiled runtime directory passed as `--runtime`.
    runtime_dir: PathBuf,
    /// Absolute path to the shared warm `CARGO_TARGET_DIR`.
    cargo_target_dir: PathBuf,
    /// Per-compile subprocess timeout.
    compile_timeout: Duration,
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
    // Guard: source size limit.
    if req.source.len() > MAX_SOURCE_BYTES {
        return Ok(axum::Json(CompileResponse::Error {
            diagnostics: format!(
                "source too large ({} bytes, limit {} bytes)",
                req.source.len(),
                MAX_SOURCE_BYTES
            ),
        }));
    }

    // Write source to a temp directory.
    let tmpdir = tempfile::TempDir::new()?;
    let src_dir = tmpdir.path().join("src");
    std::fs::create_dir_all(&src_dir)?;
    let entry = src_dir.join("Main.ipe");
    {
        let mut f = std::fs::File::create(&entry)?;
        f.write_all(req.source.as_bytes())?;
    }
    let out_dir = tmpdir.path().join("out");

    info!(
        "compiling source ({} bytes) in {:?}",
        req.source.len(),
        tmpdir.path()
    );

    let result = compile_wasm(&state, &entry, &out_dir).await;
    match result {
        Ok(bundle) => Ok(axum::Json(CompileResponse::Ok(bundle))),
        Err(diag) => Ok(axum::Json(CompileResponse::Error { diagnostics: diag })),
    }
}

/// Run `ipe build --target wasm` and collect the bundle files from `out_dir`.
///
/// Returns the bundle on success, or a diagnostics string on compile error /
/// timeout.
async fn compile_wasm(
    state: &AppState,
    entry: &Path,
    out_dir: &Path,
) -> Result<CompileSuccess, String> {
    let mut cmd = Command::new(&state.ipe_bin);
    cmd.arg("build")
        .arg("--target")
        .arg("wasm")
        .arg("--entry")
        .arg(entry)
        .arg("--out")
        .arg(out_dir)
        .arg("--runtime")
        .arg(&state.runtime_dir)
        .env("CARGO_TARGET_DIR", &state.cargo_target_dir)
        // Disable incremental to keep each job's artifact space bounded.
        .env("CARGO_INCREMENTAL", "0")
        // Suppress noisy cargo progress bars in subprocess stdout.
        .env("CARGO_TERM_PROGRESS_WHEN", "never")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let future = async {
        let output = cmd
            .output()
            .await
            .map_err(|e| format!("spawn failed: {e}"))?;
        if output.status.success() {
            Ok(output)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            Err(stderr)
        }
    };

    let output = match timeout(state.compile_timeout, future).await {
        Ok(Ok(out)) => out,
        Ok(Err(diag)) => return Err(diag),
        Err(_elapsed) => {
            warn!("compile timed out after {:?}", state.compile_timeout);
            return Err(format!(
                "compile timed out after {} s",
                state.compile_timeout.as_secs()
            ));
        }
    };

    // Log any stderr output at debug level even on success.
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        info!("compile stderr: {stderr}");
    }

    read_bundle(out_dir)
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

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("ipe_playground=info".parse().unwrap_or_default()),
        )
        .init();

    let ipe_bin = match resolve_ipe_bin() {
        Ok(p) => {
            info!("ipe binary: {:?}", p);
            p
        }
        Err(e) => {
            error!("{e}");
            // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — playground binary `main` startup boundary: no ipe binary means nothing to serve [ledger #boundary]
            std::process::exit(1);
        }
    };

    let runtime_dir = match resolve_runtime_dir() {
        Ok(p) => {
            info!("runtime dir: {:?}", p);
            p
        }
        Err(e) => {
            error!("{e}");
            // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — playground binary `main` startup boundary: no runtime dir means nothing to serve [ledger #boundary]
            std::process::exit(1);
        }
    };

    let cargo_target_dir = std::env::var("IPE_PLAYGROUND_TARGET_DIR").map_or_else(
        |_| PathBuf::from("/tmp/ipe-playground-target"),
        PathBuf::from,
    );

    let compile_timeout = std::env::var("IPE_PLAYGROUND_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map_or(
            Duration::from_secs(DEFAULT_COMPILE_TIMEOUT_SECS),
            Duration::from_secs,
        );

    info!(
        "cargo_target_dir={:?} compile_timeout={:?}",
        cargo_target_dir, compile_timeout
    );

    let state = AppState {
        ipe_bin,
        runtime_dir,
        cargo_target_dir,
        compile_timeout,
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
            // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — playground binary `main` bind boundary: cannot open the listening socket [ledger #boundary]
            std::process::exit(1);
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        error!("server error: {e}");
        // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — playground binary `main` serve boundary: the server loop returned an error [ledger #boundary]
        std::process::exit(1);
    }
}

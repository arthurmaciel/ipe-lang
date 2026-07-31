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

use ipe_playground::run_jailed;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
    /// Per-compile subprocess timeout (the trusted `ipe build` step).
    compile_timeout: Duration,
    /// The pre-warmed dependency cache seeding each jailed offline build
    /// (`Some` once startup pre-warm succeeds; `None` disables `/run`, which then
    /// fails closed with a clear message).
    warm: Option<WarmCache>,
}

/// The pre-warmed offline build inputs: a `CARGO_HOME` registry (crate sources)
/// and a target dir holding the FIXED dependency closure's compiled artifacts.
/// Every per-request jailed build seeds both, so it builds fully offline AND only
/// compiles the user's own crate rather than the whole closure.
#[derive(Clone)]
struct WarmCache {
    cargo_home: PathBuf,
    target: PathBuf,
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

// ── /run handler — sandboxed native build + execute ───────────────────────────

#[derive(Deserialize)]
struct RunRequest {
    source: String,
}

/// The result of a `POST /run`. A single tagged enum so the client can branch on
/// exactly one of the five terminal states (`parse, don't validate` at the wire).
#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
enum RunResponse {
    /// The trusted compiler rejected the source (before any jail).
    CompileError { diagnostics: String },
    /// The emitted crate failed to `cargo build` inside the jail.
    BuildError { diagnostics: String },
    /// The program ran to completion inside the jail.
    Ran {
        /// The program's captured stdout (bounded).
        stdout: String,
        /// The program's captured stderr (bounded).
        stderr: String,
        /// The process exit code (`null` if it was killed).
        exit_code: Option<i32>,
        /// Wall-clock milliseconds for the build+run inside the jail.
        elapsed_ms: u64,
    },
    /// The wall-clock (or a resource cap) killed the build or the run.
    Timeout { phase: String, limit_secs: u64 },
    /// The jail could not be established — the endpoint refuses to run user code
    /// unconfined (fail-closed).
    SandboxRefused { reason: String },
}

async fn run(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<RunRequest>,
) -> Result<axum::Json<RunResponse>, AppError> {
    // Guard: same source-size ceiling as /compile — reject an oversized payload
    // before doing any work.
    if req.source.len() > MAX_SOURCE_BYTES {
        return Ok(axum::Json(RunResponse::CompileError {
            diagnostics: format!(
                "source too large ({} bytes, limit {} bytes)",
                req.source.len(),
                MAX_SOURCE_BYTES
            ),
        }));
    }

    // The whole build+run is CPU-and-fork heavy and calls blocking sandbox code;
    // run it on a blocking thread so the async runtime keeps serving.
    let resp = tokio::task::spawn_blocking(move || run_sandboxed(&state, &req.source))
        .await
        .map_err(|e| AppError(format!("run task panicked: {e}")))?;
    Ok(axum::Json(resp))
}

/// The synchronous build+run pipeline, entirely off the async runtime.
///
/// Steps: emit (trusted compiler) → seed offline deps → jailed build → jailed
/// run. Every step that touches user-derived code is inside the jail; a jail that
/// cannot be established is a fail-closed refusal, never an unsandboxed run.
fn run_sandboxed(state: &AppState, source: &str) -> RunResponse {
    // Fail-closed FIRST: no jail primitives ⇒ refuse before writing anything.
    let caps = match run_jailed::probe_or_refuse() {
        Ok(c) => c,
        Err(refusal) => {
            return RunResponse::SandboxRefused {
                reason: refusal.reason,
            };
        }
    };
    // The pre-warm must have populated the offline build inputs.
    let Some(warm) = state.warm.as_ref() else {
        return RunResponse::SandboxRefused {
            reason: "sandbox refused: the offline dependency cache was not pre-warmed at startup; \
                     /run is disabled (see server logs)"
                .to_owned(),
        };
    };

    // One scratch dir per request: the jail's ONLY writable mount. Removed at the
    // end however this function returns (the guard drops on every path).
    let scratch = match tempfile::TempDir::new() {
        Ok(t) => t,
        Err(e) => {
            return RunResponse::SandboxRefused {
                reason: format!("sandbox refused: could not create a scratch dir: {e}"),
            };
        }
    };
    let scoped_tmp = scratch.path();

    // 1. Emit the native crate with the TRUSTED compiler (no user code runs).
    let crate_dir = scoped_tmp.join("crate");
    if let Err(diag) = emit_native_crate(state, source, &crate_dir) {
        return diag;
    }

    // 2. Seed the offline dependency cache (registry + prebuilt dep artifacts)
    //    into the jail-visible scratch. The build then compiles ONLY the user's
    //    crate, fully offline.
    if let Err(e) = run_jailed::seed_cargo_home(scoped_tmp, &warm.cargo_home) {
        return RunResponse::SandboxRefused {
            reason: format!("sandbox refused: could not seed the offline dependency cache: {e}"),
        };
    }
    if let Err(e) = run_jailed::seed_target_dir(scoped_tmp, &warm.target) {
        return RunResponse::SandboxRefused {
            reason: format!("sandbox refused: could not seed the prebuilt dependency target: {e}"),
        };
    }

    let started = Instant::now();

    // 3. Jailed build (offline, no network, resource-capped).
    let build = match run_jailed::jailed_build(&caps, scoped_tmp) {
        Ok(o) => o,
        Err(defect) => {
            return RunResponse::SandboxRefused {
                reason: format!("sandbox refused: {defect}"),
            };
        }
    };
    if build.killed {
        return RunResponse::Timeout {
            phase: "build".to_owned(),
            limit_secs: run_jailed::RunCaps::build_defaults().wall_secs,
        };
    }
    if build.status != Some(0) {
        // A cargo build failure — distinct from a compile (ipe) error.
        return RunResponse::BuildError {
            diagnostics: truncate_diag(&format!("{}{}", build.stdout, build.stderr)),
        };
    }

    // 4. Jailed run of the freshly-built binary.
    let app = run_jailed::app_binary_path(scoped_tmp);
    if !app.is_file() {
        return RunResponse::BuildError {
            diagnostics: "build reported success but produced no `ipe-app` binary".to_owned(),
        };
    }
    let run = match run_jailed::jailed_run(&caps, scoped_tmp, &app) {
        Ok(o) => o,
        Err(defect) => {
            return RunResponse::SandboxRefused {
                reason: format!("sandbox refused: {defect}"),
            };
        }
    };
    if run.killed {
        return RunResponse::Timeout {
            phase: "run".to_owned(),
            limit_secs: run_jailed::RunCaps::run_defaults().wall_secs,
        };
    }
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    RunResponse::Ran {
        stdout: run.stdout,
        stderr: run.stderr,
        exit_code: run.status,
        elapsed_ms,
    }
}

/// Emit the native crate from `source` with the trusted `ipe` compiler.
///
/// Returns `Ok(())` on a clean emit; a [`RunResponse::CompileError`] (as the
/// `Err` variant) when `ipe build` rejects the program.
fn emit_native_crate(state: &AppState, source: &str, crate_dir: &Path) -> Result<(), RunResponse> {
    let compile_err = |d: String| RunResponse::CompileError { diagnostics: d };

    let src_dir = crate_dir.join("src-ipe");
    let entry = src_dir.join("Main.ipe");
    std::fs::create_dir_all(&src_dir)
        .and_then(|()| std::fs::write(&entry, source))
        .map_err(|e| compile_err(format!("could not stage source: {e}")))?;

    // `ipe build <src>` with NO `--target wasm` ⇒ the native crate. The source is
    // the positional argument. Trusted compiler, deterministic codegen; a plain
    // timeout-bounded subprocess (not the jail — it does not run the user's
    // program, it emits it).
    let output = std::process::Command::new(&state.ipe_bin)
        .arg("build")
        .arg(&entry)
        .arg("--out")
        .arg(crate_dir)
        .arg("--runtime")
        .arg(&state.runtime_dir)
        .env("CARGO_TERM_PROGRESS_WHEN", "never")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| compile_err(format!("could not launch the ipe compiler: {e}")))?;

    if output.status.success() {
        Ok(())
    } else {
        let diag = String::from_utf8_lossy(&output.stderr).into_owned();
        Err(compile_err(truncate_diag(&diag)))
    }
}

/// Bound a diagnostics string so a pathological compiler/cargo error cannot blow
/// the response size.
fn truncate_diag(s: &str) -> String {
    const MAX: usize = 64 * 1024;
    if s.len() <= MAX {
        return s.to_owned();
    }
    // Truncate on a char boundary at or below MAX.
    let mut end = MAX;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (truncated)", &s[..end])
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

/// Pre-warm the FIXED dependency closure so every per-request jailed build runs
/// fully offline AND compiles only the user's own crate.
///
/// The dependency set is identical for every native program (the runtime fixes
/// the manifest), so this runs once at startup. It:
///   1. emits a trivial native crate (trusted compiler),
///   2. `cargo build`s it into a persistent warm `CARGO_HOME` + target dir —
///      compiling the whole dependency closure ONCE, running only our own
///      trusted dependencies' build scripts (never user code; the untrusted
///      per-request build runs offline in the jail).
///
/// Each per-request jail then seeds both the registry and the prebuilt target, so
/// its offline build reuses every dependency `.rlib` and compiles only the user
/// crate. Returns the warm cache, or `None` (disabling `/run`) on any failure.
async fn prewarm_offline_deps(
    ipe_bin: &Path,
    runtime_dir: &Path,
    cargo_target_dir: &Path,
) -> Option<WarmCache> {
    let warm_home = cargo_target_dir.join("playground-cargo-home");
    let warm_target = cargo_target_dir.join("playground-warm-target");
    let scratch = match tempfile::TempDir::new() {
        Ok(t) => t,
        Err(e) => {
            error!("pre-warm: could not create scratch dir: {e}");
            return None;
        }
    };
    let src_dir = scratch.path().join("src-ipe");
    let entry = src_dir.join("Main.ipe");
    let crate_dir = scratch.path().join("crate");
    let hello =
        "module Main exposing (main)\n\nimport Ipe.Io as Io\n\nmain =\n    Io.println \"warm\"\n";
    if let Err(e) = std::fs::create_dir_all(&src_dir).and_then(|()| std::fs::write(&entry, hello)) {
        error!("pre-warm: could not stage source: {e}");
        return None;
    }

    // Emit the native crate (trusted compiler). The source is the positional arg.
    let emit = Command::new(ipe_bin)
        .arg("build")
        .arg(&entry)
        .arg("--out")
        .arg(&crate_dir)
        .arg("--runtime")
        .arg(runtime_dir)
        .env("CARGO_TERM_PROGRESS_WHEN", "never")
        .output()
        .await;
    match emit {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            error!(
                "pre-warm: ipe build failed:\n{}",
                String::from_utf8_lossy(&o.stderr)
            );
            return None;
        }
        Err(e) => {
            error!("pre-warm: could not launch ipe: {e}");
            return None;
        }
    }

    if let Err(e) =
        std::fs::create_dir_all(&warm_home).and_then(|()| std::fs::create_dir_all(&warm_target))
    {
        error!("pre-warm: could not create warm dirs: {e}");
        return None;
    }
    info!("pre-warm: building the fixed dependency closure once (first run is slow)…");
    // A full build compiles + caches every dependency `.rlib` into the warm
    // target, and populates the warm CARGO_HOME registry. Only our own trusted
    // dependencies run build scripts here — never user code.
    let build = Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(crate_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&warm_target)
        .env("CARGO_HOME", &warm_home)
        .env("CARGO_TERM_PROGRESS_WHEN", "never")
        .output()
        .await;
    match build {
        Ok(o) if o.status.success() => Some(WarmCache {
            cargo_home: warm_home,
            target: warm_target,
        }),
        Ok(o) => {
            error!(
                "pre-warm: cargo build of the dependency closure failed:\n{}",
                String::from_utf8_lossy(&o.stderr)
            );
            None
        }
        Err(e) => {
            error!("pre-warm: could not launch cargo: {e}");
            None
        }
    }
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

    // Pre-warm the offline dependency cache so `/run`'s jailed builds never need
    // the network. Failure disables `/run` (it then fails closed) but leaves
    // `/compile` fully usable.
    let warm = prewarm_offline_deps(&ipe_bin, &runtime_dir, &cargo_target_dir).await;
    if let Some(w) = &warm {
        info!(
            "/run enabled — offline deps warmed (cargo_home={:?}, target={:?})",
            w.cargo_home, w.target
        );
    } else {
        warn!("/run DISABLED — offline dependency pre-warm failed (see errors above)");
    }

    let state = AppState {
        ipe_bin,
        runtime_dir,
        cargo_target_dir,
        compile_timeout,
        warm,
    };

    let mut app = Router::new()
        .route("/compile", post(compile))
        .route("/run", post(run))
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

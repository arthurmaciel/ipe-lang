//! Shared helpers for the backend SEAL end-to-end tests.
//!
//! Each SEAL test emits an Ipê program, vendors the runtime source tree beside
//! it, and runs `cargo build` (and optionally the resulting binary) to prove the
//! emitted crate compiles and produces the expected output.
//!
//! This module is NOT a test binary — cargo treats a directory-with-mod.rs as a
//! library module and excludes it from the test-binary discovery pass.

use std::path::{Path, PathBuf};
use std::process::Command;

use e2e_support::child_shared_target_from_env;
use ipe_backend::EmittedProject;
use ipe_diagnostics::{DResult, Diagnostic};

/// Locate the Ipê runtime source tree (`src/runtime/rust/src`), checking
/// `IPE_RUNTIME_DIR` first, then walking ancestor directories.
///
/// Returns `None` when the runtime cannot be found. Callers that require the
/// runtime should skip gracefully on `None` rather than hard-erroring, so a
/// bare dev environment without a runtime checkout does not break the test suite.
pub fn resolve_runtime() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("IPE_RUNTIME_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
    }
    let cwd = std::env::current_dir().ok()?;
    let mut here: Option<&Path> = Some(cwd.as_path());
    while let Some(dir) = here {
        // In-repo runtime (ipe-lang monorepo): the flat `src/` directory whose
        // `.rs` files are vendored into each emitted crate's `src/ipe_runtime/`.
        let candidate = dir.join("src").join("runtime").join("rust").join("src");
        if candidate.is_dir() {
            return Some(candidate);
        }
        here = dir.parent();
    }
    None
}

/// Recursively copy the runtime source tree `src` into `dst`.
///
/// `src` is the flat `src/runtime/rust/src/` directory; `dst` is the emitted
/// crate's `src/ipe_runtime/` target. Subdirectories are copied recursively.
pub fn copy_dir(src: &Path, dst: &Path) -> DResult<()> {
    std::fs::create_dir_all(dst).map_err(|e| io_bug(dst, &e))?;
    for entry in std::fs::read_dir(src).map_err(|e| io_bug(src, &e))? {
        let entry = entry.map_err(|e| io_bug(src, &e))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type().map_err(|e| io_bug(&from, &e))?;
        if file_type.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| io_bug(&from, &e))?;
        }
    }
    Ok(())
}

/// Write `emitted` into a fresh temp directory named `slot`, vendor the runtime
/// beside it, run `cargo <subcmd>`, and return the process exit status.
#[allow(dead_code)]
///
/// Callers pass `"build"` or `"run"` as `subcmd`. The runtime must be available
/// (i.e. `resolve_runtime()` returned `Some`) before calling this; the function
/// errors rather than skipping — skip decisions belong to the test body.
pub fn vendor_and_run(
    emitted: &EmittedProject,
    runtime: &Path,
    slot: &str,
    subcmd: &str,
) -> DResult<std::io::Result<std::process::ExitStatus>> {
    let out = std::env::temp_dir().join(slot);
    let _ = std::fs::remove_dir_all(&out);
    let src = out.join("src");
    std::fs::create_dir_all(&src).map_err(|e| io_bug(&src, &e))?;

    copy_dir(runtime, &src.join("ipe_runtime"))?;

    let cargo_toml = out.join("Cargo.toml");
    std::fs::write(&cargo_toml, &emitted.cargo_toml).map_err(|e| io_bug(&cargo_toml, &e))?;
    for (rel, contents) in &emitted.files {
        let path = out.join(rel.as_str());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_bug(parent, &e))?;
        }
        std::fs::write(&path, contents).map_err(|e| io_bug(&path, &e))?;
    }

    // Forward the warm shared target when CI exports `IPE_ORACLE_SHARED_TARGET`,
    // else fall back to an isolated per-slot target so a bare local run stays
    // hermetic. The shared target lets heavy dep crates (tokio, axum, runtime)
    // reuse already-compiled artifacts rather than cold-building per test.
    let shared = child_shared_target_from_env();
    let target_dir = shared
        .as_deref()
        .map_or_else(|| out.join("target"), PathBuf::from);
    let status = Command::new("cargo")
        .arg(subcmd)
        .current_dir(&out)
        .env("CARGO_TARGET_DIR", &target_dir)
        .status();
    // Only prune an isolated per-slot target; the shared warm target is owned
    // by the harness and must not be removed here.
    if shared.is_none() {
        let _ = std::fs::remove_dir_all(&target_dir);
    }
    Ok(status)
}

/// Resolve the `CARGO_TARGET_DIR` for a hand-rolled emitted-crate build.
///
/// When CI exports `IPE_ORACLE_SHARED_TARGET` (an absolute path), returns that
/// path so the emitted crate's deps link against the warm shared tree rather than
/// cold-building. Falls back to an ambient `CARGO_TARGET_DIR` a local lane set,
/// then to `out_dir.join("target")` for a bare local run (hermetic, isolated).
///
/// Callers that prune the target after the build must check whether the returned
/// path equals `out_dir.join("target")` before removing it — the shared warm
/// target must never be pruned here.
#[must_use]
#[allow(dead_code)]
pub fn emitted_target_dir(out_dir: &Path) -> PathBuf {
    child_shared_target_from_env().map_or_else(|| out_dir.join("target"), PathBuf::from)
}

/// Wrap a filesystem error as a `CompilerBug` diagnostic.
pub fn io_bug(path: &Path, e: &std::io::Error) -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: "seal e2e io",
        detail: format!("{}: {e}", path.display()),
    }
}

#![forbid(unsafe_code)]
//! `skyc` — the Milestone-0 command-line driver.
//!
//! Wires the pipeline end to end: read a `.sky` entry file, run it through
//! [`sky_parse`] → [`sky_canon`] → [`sky_types`] → [`sky_lower`] → the
//! [`sky_backend_rust`] emitter, write the emitted Cargo project, and vendor the
//! Sky runtime module tree into it (a port of the copy step in the Haskell
//! compiler's `Sky.Generate.Rust.Project`).
//!
//! Generated Rust projects do not depend on the runtime as a Cargo path crate;
//! instead `main.rs` declares `mod sky_runtime;` and the runtime sources are
//! copied in beside it. The driver therefore must locate
//! `runtime-rust/src/sky_runtime/` and copy it under `<out>/src/sky_runtime/`.
//!
//! Errors are typed ([`CliError`]); no operation panics or unwraps.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use sky_backend::Backend;
use sky_backend_rust::RustBackend;
use sky_diagnostics::{Diagnostic, render};
use sky_intern::Interner;

/// A driver-level error. Distinct from a compiler [`Diagnostic`]: it also covers
/// filesystem failures and command-line misuse, neither of which is a property
/// of the Sky program being compiled.
#[derive(Debug)]
pub enum CliError {
    /// Command-line misuse; carries a fixed usage hint.
    Usage(&'static str),
    /// A filesystem operation failed at `path`.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The compiler rejected the program. Carries the entry path and full
    /// source text alongside the diagnostic so [`fmt::Display`] can render a
    /// rustc/Elm-style report (caret snippet + help + `skyc explain` pointer)
    /// rather than a debug dump.
    Pipeline {
        file: PathBuf,
        src: String,
        diag: Diagnostic,
    },
    /// The Sky runtime module tree could not be located.
    RuntimeNotFound,
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(hint) => write!(f, "{hint}"),
            Self::Io { path, source } => write!(f, "io error at {}: {source}", path.display()),
            Self::Pipeline { file, src, diag } => {
                f.write_str(&render(diag, &file.to_string_lossy(), src))
            }
            Self::RuntimeNotFound => write!(
                f,
                "could not locate the Sky runtime; set SKY_RUNTIME_DIR or pass --runtime <dir>"
            ),
        }
    }
}

impl std::error::Error for CliError {}

/// Build `entry` into a Rust Cargo project under `out_dir`, vendoring the
/// runtime module tree from `runtime_dir`.
///
/// # Errors
/// Returns [`CliError::Pipeline`] when the compiler rejects the program,
/// [`CliError::Io`] on any filesystem failure.
pub fn build(entry: &Path, out_dir: &Path, runtime_dir: &Path) -> Result<(), CliError> {
    let source = fs::read_to_string(entry).map_err(|e| io_err(entry, e))?;

    // A pipeline diagnostic is rendered against the entry's path + source, so
    // bundle both into every `CliError::Pipeline` produced below.
    let pipeline_err = |diag: Diagnostic| CliError::Pipeline {
        file: entry.to_path_buf(),
        src: source.clone(),
        diag,
    };

    let mut interner = Interner::new();
    let module = sky_parse::parse_module(&source, &mut interner).map_err(&pipeline_err)?;
    let canonical = sky_canon::canonicalise(&module, &mut interner).map_err(&pipeline_err)?;
    let types = sky_types::infer(&canonical, &mut interner).map_err(&pipeline_err)?;
    let program = sky_lower::lower(&canonical, &types, &interner).map_err(&pipeline_err)?;
    let emitted = RustBackend::new(&interner)
        .emit(&program)
        .map_err(&pipeline_err)?;

    let src_dir = out_dir.join("src");
    fs::create_dir_all(&src_dir).map_err(|e| io_err(&src_dir, e))?;

    // Vendor the runtime module tree FIRST, then write the emitted files. The
    // backend emits a trimmed `sky_runtime/mod.rs` + `config.rs`; writing the
    // emitted files last lets them overwrite the fuller copies from the source
    // tree (whose module list reaches for crates outside the M0 manifest).
    copy_dir(runtime_dir, &src_dir.join("sky_runtime"))?;

    let cargo_path = out_dir.join("Cargo.toml");
    fs::write(&cargo_path, &emitted.cargo_toml).map_err(|e| io_err(&cargo_path, e))?;

    // Each `rel` is a `sky_backend::RelPath`: validated at construction to be
    // relative and free of `..` components, so `out_dir.join(rel)` cannot escape
    // `out_dir` (no absolute-write, no path-traversal). The trust boundary is the
    // newtype, not this loop.
    for (rel, contents) in &emitted.files {
        let path = out_dir.join(rel.as_str());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        }
        fs::write(&path, contents).map_err(|e| io_err(&path, e))?;
    }

    Ok(())
}

/// Locate the Sky runtime module tree (`runtime-rust/src/sky_runtime/`).
///
/// Resolution order: `$SKY_RUNTIME_DIR`, then an upward search from the current
/// directory for a sibling `sky/runtime-rust/src/sky_runtime` or
/// `runtime-rust/src/sky_runtime`.
///
/// # Errors
/// Returns [`CliError::RuntimeNotFound`] when no candidate directory exists, or
/// [`CliError::Io`] if the current directory cannot be read.
pub fn resolve_runtime() -> Result<PathBuf, CliError> {
    if let Ok(dir) = std::env::var("SKY_RUNTIME_DIR") {
        let path = PathBuf::from(dir);
        if path.is_dir() {
            return Ok(path);
        }
    }

    let cwd = std::env::current_dir().map_err(|e| io_err(Path::new("."), e))?;
    let mut here: Option<&Path> = Some(cwd.as_path());
    while let Some(dir) = here {
        for candidate in [
            dir.join("sky")
                .join("runtime-rust")
                .join("src")
                .join("sky_runtime"),
            dir.join("runtime-rust").join("src").join("sky_runtime"),
        ] {
            if candidate.is_dir() {
                return Ok(candidate);
            }
        }
        here = dir.parent();
    }
    Err(CliError::RuntimeNotFound)
}

/// Parse `argv` (excluding the program name) and run the requested command.
///
/// # Errors
/// Returns [`CliError`] on misuse, a compile failure, or a filesystem error.
pub fn run_cli(args: &[String]) -> Result<(), CliError> {
    const USAGE: &str = "usage: skyc build <entry.sky> [--out <dir>] [--runtime <dir>]";

    let mut it = args.iter();
    match it.next().map(String::as_str) {
        Some("build") => {}
        _ => return Err(CliError::Usage(USAGE)),
    }

    let entry = it.next().ok_or(CliError::Usage(USAGE))?.clone();
    let mut out: Option<String> = None;
    let mut runtime: Option<String> = None;
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--out" => out = Some(it.next().ok_or(CliError::Usage(USAGE))?.clone()),
            "--runtime" => runtime = Some(it.next().ok_or(CliError::Usage(USAGE))?.clone()),
            _ => return Err(CliError::Usage(USAGE)),
        }
    }

    let out_dir = out.map_or_else(|| PathBuf::from("sky-out").join("rust"), PathBuf::from);
    let runtime_dir = match runtime {
        Some(r) => PathBuf::from(r),
        None => resolve_runtime()?,
    };
    build(Path::new(&entry), &out_dir, &runtime_dir)
}

/// Recursively copy `src` into `dst`. `src` is the trusted, in-repo runtime
/// tree, so its depth is bounded.
fn copy_dir(src: &Path, dst: &Path) -> Result<(), CliError> {
    fs::create_dir_all(dst).map_err(|e| io_err(dst, e))?;
    let entries = fs::read_dir(src).map_err(|e| io_err(src, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_err(src, e))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type().map_err(|e| io_err(&from, e))?;
        if file_type.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| io_err(&from, e))?;
        }
    }
    Ok(())
}

fn io_err(path: &Path, source: std::io::Error) -> CliError {
    CliError::Io {
        path: path.to_path_buf(),
        source,
    }
}

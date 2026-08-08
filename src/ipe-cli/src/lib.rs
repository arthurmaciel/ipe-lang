#![forbid(unsafe_code)]
//! `ipe` — the command-line driver.
//!
//! Wires the pipeline end to end: read a `.ipe` entry file, run it through
//! [`ipe_parse`] → [`ipe_canon`] → [`ipe_types`] → [`ipe_lower`] → the
//! [`ipe_backend_rust`] emitter, write the emitted Cargo project, and vendor the
//! Ipe runtime module tree into it (a port of the copy step in the Haskell
//! compiler's `Ipe.Generate.Rust.Project`).
//!
//! Generated Rust projects do not depend on the runtime as a Cargo path crate;
//! instead `main.rs` declares `mod ipe_runtime;` and the runtime sources are
//! copied in beside it. The driver therefore must locate
//! `src/runtime/rust/src/` (the in-repo copy) and vendor it under
//! `<out>/src/ipe_runtime/`.
//!
//! Errors are typed ([`CliError`]); no operation panics or unwraps.

pub mod api_surface;
pub mod audit;
pub mod audit_native;
pub mod build_plan;
mod cache;
pub mod clean;
pub mod cli_args;
pub mod diff;
pub mod doc;
pub mod ffi;
pub mod fmt;
pub mod health;
pub mod help;
pub mod index;
pub mod init;
pub mod lockfile;
pub mod login;
mod lsp;
pub mod pkg;
pub mod progress;
pub mod project;
pub mod publish;
pub mod resolve;
pub mod run_sandbox;
pub mod runtime_embed;
pub mod style;
pub mod toolchain;
/// The embedded Ipê standard-library source now lives in the dependency-free
/// [`ipe_stdlib`] leaf crate so the WebAssembly frontend can share one copy.
/// Re-exported here so `crate::stdlib::…` call sites resolve unchanged.
pub use ipe_stdlib as stdlib;
pub mod watch;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use ipe_diagnostics::{
    ALL_CODES, Applicability, Diagnostic, HelpLine, Suggestion, explain_page, render, title,
};
use ipe_intern::Interner;

/// The runtime crate an emitted project linked against: its root and declared
/// version.
///
/// Carried into [`CliError::EmittedBuildFailed`] so a `cargo` failure that names
/// a missing runtime feature can point at the exact stale crate.
#[derive(Debug, Clone)]
pub struct RuntimeContext {
    /// The resolved runtime crate root.
    pub root: PathBuf,
    /// The version that crate declares.
    pub version: String,
}

/// A driver-level error. Distinct from a compiler [`Diagnostic`]: it also covers
/// filesystem failures and command-line misuse, neither of which is a property
/// of the Ipê program being compiled.
#[derive(Debug)]
pub enum CliError {
    /// Command-line misuse; carries a fixed usage hint.
    Usage(&'static str),
    /// No command, or an unrecognised one: the top-level help is shown and the
    /// process exits non-zero. Distinct from [`Self::Usage`] because it renders
    /// the full sectioned screen (coloured for a terminal) rather than a hint.
    ///
    /// `attempted` is the token the user typed (empty when no command was
    /// given); a near-miss to a known command is offered as a `maybe` hint.
    UnknownCommand { attempted: String },
    /// Command-line / manifest misuse whose message must echo user-supplied
    /// input (e.g. an unrecognised `ipe.toml` value) — kept distinct from
    /// [`Self::Usage`] so no call site needs to leak a `String` into a
    /// `&'static str` just to report what the user actually wrote.
    UsageOwned(String),
    /// A filesystem operation failed at `path`.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The compiler rejected the program. Carries the entry path and full
    /// source text alongside the diagnostic so [`fmt::Display`] can render a
    /// rustc/Elm-style report (caret snippet + help + `ipe explain` pointer)
    /// rather than a debug dump.
    ///
    /// `diag` is boxed: a bare [`Diagnostic`] is the widest field of the
    /// widest variant, and every one of the ~200 functions returning
    /// `Result<_, CliError>` pays the enum's size in its `Err` slot. Boxing
    /// this one field keeps `CliError` small — the compile-failure path (the
    /// exceptional one) is the only place that pays for the diagnostic — while
    /// leaving the `file`/`src` field names intact so existing pattern matches
    /// on this variant are unaffected.
    Pipeline {
        file: PathBuf,
        src: String,
        diag: Box<Diagnostic>,
    },
    /// The Ipê runtime module tree could not be located.
    RuntimeNotFound,
    /// `$IPE_RUNTIME_DIR` was set but does not name a runtime crate root (a
    /// directory whose `Cargo.toml` declares the `ipe-runtime-rust` package).
    /// The override is a trust decision — an unverified directory is a hard,
    /// typed refusal, never a silent fall-through to a different runtime.
    RuntimeDirInvalid {
        /// The path the override named.
        path: PathBuf,
        /// The named path is the inner runtime module directory
        /// (`…/src/ipe_runtime` or `…/rust/src`) rather than the crate root — a
        /// common misconfiguration worth calling out explicitly.
        points_at_inner: bool,
    },
    /// No directory could be resolved to materialize the embedded runtime into
    /// (no `IPE_HOME`, `XDG_DATA_HOME`, or `HOME`). Without a home there is
    /// nowhere to write the runtime the emitted project links against.
    RuntimeHomeUnknown,
    /// Writing the embedded runtime source to `<IPE_HOME>/runtime/<version>/rust`
    /// failed (disk full, permission denied, or a drifted embed). This is a
    /// fail-closed refusal — the build stops rather than link a wrong or empty
    /// runtime. Carries a specific detail.
    RuntimeMaterializeFailed {
        /// What specifically failed.
        detail: String,
    },
    /// The resolved runtime crate declares a version different from the
    /// compiler's own. The emitted project pins features and shapes against the
    /// compiler's runtime; a crate at a different version lacks those, so linking
    /// it fails deep inside `cargo` with an opaque feature error. This is a hard,
    /// typed refusal at resolution time — a stale `out/`, a walked-up old crate
    /// root, or a mismatched `IPE_RUNTIME_DIR` is caught before emit, never
    /// linked. Carries the resolved root, the version found there, and the
    /// compiler's expected version.
    RuntimeVersionMismatch {
        /// The resolved runtime crate root whose version disagrees.
        path: PathBuf,
        /// The version that crate's `Cargo.toml` declares.
        found: String,
        /// The compiler's own version, which the runtime must equal.
        expected: String,
    },
    /// Building the emitted Rust project failed — `cargo` exited non-zero while
    /// compiling the program this compiler emitted. This is neither a
    /// command-line misuse (so it never shows the command's `--help` page) nor a
    /// fault in the user's Ipê source (the compile already succeeded). Carries
    /// `cargo`'s exit code and its captured stderr so `Display` can surface a
    /// targeted cause (a runtime-feature gap) or the trimmed `cargo` error under
    /// a clean header.
    EmittedBuildFailed {
        /// What the build step compiled (e.g. `the emitted program`).
        what: &'static str,
        /// `cargo`'s exit code.
        code: i32,
        /// `cargo`'s captured stderr, presented after trimming.
        stderr: String,
        /// The runtime crate root the emitted project linked against, when the
        /// caller resolved one — named in a runtime-feature-gap message.
        runtime: Option<RuntimeContext>,
    },
    /// `ipe explain <CODE>` was given a string that is not a taxonomy code.
    /// Carries the (trimmed) input and a deterministic did-you-mean list over
    /// the known codes, ranked by `(Levenshtein, code)`.
    UnknownCode {
        input: String,
        suggestions: Vec<&'static str>,
    },
    /// A static-build request was refused (typed reason — see
    /// [`build_plan::Refusal`]). Refusal means NO artifact: the build asked
    /// to be static is never silently degraded to a dynamic one.
    StaticRefusal(build_plan::Refusal),
    /// A declared capability set did not equal the set inferred from the
    /// program. Carries the capabilities the program uses but did not declare
    /// (`missing`) and the ones declared but never used (`extra`), each a stable
    /// sorted list of wire names. Consumed by SP2/SP4 to reject a drifted
    /// manifest.
    CapabilityMismatch {
        missing: Vec<&'static str>,
        extra: Vec<&'static str>,
    },
    /// Package resolution failed for a non-security reason: an index entry could
    /// not be found or parsed, no published version satisfied the requirement, or
    /// a `git` fetch of the source failed. Carries a message naming the package.
    Resolve(String),
    /// A fetched package's content hash did not equal the hash the index pinned.
    /// This is the verify-before-trust boundary: a mismatch is always a hard,
    /// typed error — never a warning — because the source that was fetched is not
    /// the source the publisher registered. Carries the package name, the
    /// expected hash, and the hash actually computed over the fetched tree.
    HashMismatch {
        package: String,
        expected: String,
        actual: String,
    },
    /// `ipe diff` could not compute the public-API delta — a tree could not be
    /// read, did not typecheck, or exposed an open interface. Carries the typed
    /// [`api_surface::DiffError`] cause.
    Diff(api_surface::DiffError),
    /// `ipe diff --check` found the proposed new version does not clear the
    /// required semver floor. Carries the required floor version and the
    /// human-readable required bump so the message is actionable.
    SemverRejected {
        required: String,
        floor: String,
        proposed: String,
    },
    /// A `ipe package audit` Tier-1 check rejected the package. Carries the
    /// typed [`audit::Rejection`] naming the failing check and its one
    /// diagnostic. This is the package gate's hard reject — a check that would
    /// let an unsafe or dishonest version through is a security hole, so it is
    /// always a typed error, never a warning.
    PackageAudit(audit::Rejection),
    /// `ipe package publish` declined to proceed. Carries the typed
    /// [`publish::Refusal`] naming the precondition that failed (a dirty working
    /// tree, an unpushed HEAD, or an already-published version).
    /// A publish precondition is a hard, typed refusal — never a warning — because
    /// a merged index entry must pin an immutable, reproducible revision.
    Publish(publish::Refusal),
    /// `ipe doc check` found one or more exposed bindings without a doc-comment.
    /// Carries the ready-to-print coverage report. This is a legitimate gate
    /// result — the check ran correctly and the package is under-documented — not
    /// a command misuse, so it exits non-zero with the report alone and never the
    /// command's `--help` page.
    DocCoverage(String),
    /// A known command was misused (bad or missing arguments, an unknown flag).
    /// Carries the specific reason and the command name; [`fmt::Display`] renders
    /// the reason followed by that command's full, indented `--help` page — the
    /// uniform "misuse shows help" output every command shares, printed to stderr
    /// by [`crate::run_cli`]'s caller. The command name is always a known command
    /// (the dispatcher wraps a raw [`Self::Usage`] / [`Self::UsageOwned`] into
    /// this only for a command it recognised).
    CommandUsage {
        /// The command whose help page to show (a known command name).
        command: &'static str,
        /// The specific reason for the misuse (e.g. an unknown flag).
        reason: String,
    },
    /// A stage of `ipe verify` failed. Carries the stage name and the stage's
    /// own already-rendered report. Like [`Self::DocCoverage`], this is a
    /// legitimate gate result — the `verify` invocation was valid and the
    /// underlying check ran correctly — so it exits non-zero with the report
    /// alone and never the `verify` command's `--help` page.
    VerifyFailed {
        /// The failing stage (e.g. `format`).
        stage: &'static str,
        /// The stage's rendered failure report, printed as-is.
        report: String,
    },
    /// The project's test runner exited non-zero — one or more `Ipe.Test` cases
    /// failed. The test binary has already printed the per-case failures and the
    /// `N passed, M failed` summary to stdout, so this carries only the exit
    /// code and renders a short trailing line; it is a legitimate gate result
    /// (`ipe test` / `verify`'s test stage ran correctly), never a command-line
    /// misuse, so it exits non-zero with no `--help` page.
    TestFailed {
        /// The test binary's exit code (1 from `Ipe.Test.runMain` on a failing
        /// case, or another non-zero code from a crash).
        code: i32,
    },
    /// `ipe upgrade` could not find a prebuilt binary for the requested version
    /// and platform. This is a transient operational failure — the release was
    /// tagged but the CI build artifacts are still being generated — NOT a
    /// command-line misuse. Exits non-zero with the friendly message alone and
    /// never the `upgrade` command's `--help` page.
    UpgradeNoPrebuilt {
        /// The release version tag (e.g. `v0.1.24`).
        version: String,
        /// The platform–architecture pair (e.g. `linux-x64`).
        platform: String,
    },
    /// A command needed the Rust toolchain (`cargo`/`rustc`) to build, run, or
    /// test a program, but `cargo` was not found. This is an environment
    /// failure, not a command-line misuse — the invocation was valid; the host
    /// is missing a prerequisite — so it exits non-zero with the friendly,
    /// root-cause message alone and never the command's `--help` page. Carries
    /// the typed [`toolchain::ToolchainMissing`] naming what the command was
    /// doing and whether the toolchain is uninstalled or merely off the `PATH`.
    ToolchainMissing(toolchain::ToolchainMissing),
    /// `ipe health` found a critical prerequisite missing (no `rustc`/`cargo`,
    /// or an unresolvable runtime). This is a legitimate diagnostic verdict —
    /// the command ran correctly and reported the environment fully to stdout —
    /// not a command-line misuse, so it exits non-zero after the report and
    /// never shows the `health` command's `--help` page. Carries nothing: the
    /// report is the message; this variant is only the exit-code signal.
    HealthCritical,
    /// `ipe eject` was asked to eject a program it cannot make self-contained.
    /// Eject vendors ONLY the embedded runtime source; a program that binds a
    /// foreign Rust crate (FFI) would need those external crates pulled from a
    /// registry, which the self-contained, source-only eject contract forbids.
    /// This is a hard, typed refusal — never a partial eject that would emit a
    /// tree `cargo build` could not resolve offline. Carries the reason.
    EjectUnsupported {
        /// The specific reason the program cannot be ejected.
        reason: String,
    },
}

impl From<toolchain::ToolchainMissing> for CliError {
    fn from(missing: toolchain::ToolchainMissing) -> Self {
        Self::ToolchainMissing(missing)
    }
}

impl From<api_surface::DiffError> for CliError {
    fn from(err: api_surface::DiffError) -> Self {
        Self::Diff(err)
    }
}

impl From<build_plan::Refusal> for CliError {
    fn from(refusal: build_plan::Refusal) -> Self {
        Self::StaticRefusal(refusal)
    }
}

/// The one-line stderr verdict for a failed test run, guttered and glyphed so
/// the caller prints it as-is. The per-case failures and the `N passed, M
/// failed` summary already went to stdout from the test binary; this pairs the
/// non-zero exit with a short, human-readable reason.
fn test_failed_message(code: i32) -> String {
    format!(
        "{}{} one or more tests failed (runner exited {code})",
        style::GUTTER,
        style::glyph::FAIL,
    )
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(hint) => write!(f, "{hint}"),
            Self::UsageOwned(hint) => write!(f, "{hint}"),
            Self::UnknownCommand { attempted } => fmt_unknown_command(attempted, f),
            Self::Io { path, source } => write!(f, "io error at {}: {source}", path.display()),
            Self::Pipeline { file, src, diag } => {
                f.write_str(&render(diag, &file.to_string_lossy(), src))
            }
            Self::RuntimeNotFound => write!(
                f,
                "could not locate the Ipe runtime; \
                 set IPE_RUNTIME_DIR to an explicit path or pass --runtime <dir>"
            ),
            Self::RuntimeDirInvalid { .. }
            | Self::RuntimeHomeUnknown
            | Self::RuntimeMaterializeFailed { .. }
            | Self::RuntimeVersionMismatch { .. } => fmt_runtime_install_error(self, f),
            Self::EmittedBuildFailed { .. } => fmt_emitted_build_failed(self, f),
            Self::StaticRefusal(refusal) => write!(f, "static build refused: {refusal}"),
            Self::CapabilityMismatch { missing, extra } => {
                f.write_str("declared capabilities do not match the program's inferred set")?;
                if !missing.is_empty() {
                    write!(f, "\n  used but not declared: {}", missing.join(", "))?;
                }
                if !extra.is_empty() {
                    write!(f, "\n  declared but not used: {}", extra.join(", "))?;
                }
                Ok(())
            }
            Self::Resolve(message) => f.write_str(message),
            Self::HashMismatch {
                package,
                expected,
                actual,
            } => write!(
                f,
                "package `{package}`: content hash mismatch — the fetched source does not \
                 match the hash the index pinned.\n  expected: {expected}\n  actual:   {actual}\n\
                 the source was NOT trusted; nothing was written."
            ),
            Self::UnknownCode { input, suggestions } => {
                write!(f, "unknown error code `{input}`")?;
                match suggestions.split_first() {
                    None => Ok(()),
                    Some((first, rest)) => {
                        write!(f, "\n  did you mean: {first}")?;
                        for s in rest {
                            write!(f, ", {s}")?;
                        }
                        write!(f, "?")
                    }
                }
            }
            Self::Diff(err) => write!(f, "{err}"),
            Self::SemverRejected {
                required,
                floor,
                proposed,
            } => write!(
                f,
                "version {proposed} does not clear the required {required} bump — the new \
                 version must be at least {floor}."
            ),
            Self::DocCoverage(report) => f.write_str(report),
            Self::PackageAudit(rejection) => write!(f, "{rejection}"),
            Self::Publish(refusal) => write!(f, "ipe package publish refused: {refusal}"),
            // The reason, then the command's full `--help` page (indented,
            // coloured for a terminal). Rendered against stderr because misuse
            // output goes there. A known command always has a help page; the
            // `None` fallback (never taken for a known command) degrades to the
            // top-level screen rather than panicking.
            Self::CommandUsage { command, reason } => {
                writeln!(f, "{}", crate::style::gutter(reason))?;
                let page = help::command(command, &std::io::stderr())
                    .unwrap_or_else(|| help::top_level(&std::io::stderr()));
                f.write_str(page.trim_end_matches('\n'))
            }
            Self::VerifyFailed { stage, report } => {
                writeln!(f, "verify: the {stage} stage failed")?;
                f.write_str(report.trim_end_matches('\n'))
            }
            // The test binary already printed its own per-case failures and the
            // `N passed, M failed` summary to stdout; this is only the one-line
            // verdict that pairs with the non-zero exit, self-guttered so the
            // caller prints it as-is.
            Self::TestFailed { code } => f.write_str(&test_failed_message(*code)),
            Self::UpgradeNoPrebuilt { version, platform } => {
                use crate::style::{GUTTER, glyph};
                write!(
                    f,
                    "{GUTTER}{} No prebuilt binary for {version} on {platform}.\n\
                     {GUTTER}    Possibly the binaries for that version are still being generated.\n\
                     {GUTTER}    If you prefer, build from source:\n\
                     {GUTTER}        cargo install --git https://github.com/arthurmaciel/ipe-lang ipe",
                    glyph::FAIL
                )
            }
            // The toolchain-missing message gutters and frames itself; it owns
            // its rendering (see `toolchain::ToolchainMissing`'s `Display`).
            Self::ToolchainMissing(missing) => write!(f, "{missing}"),
            // The full diagnostic report already went to stdout; this stderr
            // line is only the one-line verdict that pairs with the non-zero
            // exit, self-guttered so the caller prints it as-is.
            Self::HealthCritical => write!(
                f,
                "{}health: a required prerequisite is missing (see the report above)",
                style::GUTTER
            ),
            Self::EjectUnsupported { reason } => write!(f, "{}eject: {reason}", style::GUTTER),
        }
    }
}

/// Render [`CliError::UnknownCommand`] for `Display`: an optional "unknown
/// command" line with a near-miss suggestion, then the top-level help screen
/// (coloured for a terminal). Output goes to stderr, where misuse output belongs.
fn fmt_unknown_command(attempted: &str, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    if !attempted.is_empty() {
        writeln!(f, "unknown command `{attempted}`")?;
        if let Some(sugg) = nearest_command(attempted) {
            writeln!(f, "  = help: maybe `{sugg}`?")?;
        }
    }
    f.write_str(help::top_level(&std::io::stderr()).trim_start())
}

/// Render the runtime-install error family (`RuntimeDirInvalid`,
/// `RuntimeHomeUnknown`, `RuntimeMaterializeFailed`) for [`CliError`]'s `Display`.
/// Split out so the main `Display` match stays within one screen.
fn fmt_runtime_install_error(err: &CliError, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match err {
        CliError::RuntimeDirInvalid {
            path,
            points_at_inner,
        } => {
            write!(
                f,
                "IPE_RUNTIME_DIR points at {}, which is not an Ipe runtime crate root \
                 (its Cargo.toml must declare `name = \"ipe-runtime-rust\"`)",
                path.display()
            )?;
            if *points_at_inner {
                write!(
                    f,
                    "\n  = help: this looks like the inner runtime module directory; \
                     point IPE_RUNTIME_DIR at the crate root that holds Cargo.toml \
                     (e.g. `src/runtime/rust`), not the `src/ipe_runtime` inside it"
                )?;
            }
            Ok(())
        }
        CliError::RuntimeHomeUnknown => write!(
            f,
            "could not determine where to install the Ipe runtime: none of IPE_HOME, \
             XDG_DATA_HOME, or HOME is set; set IPE_HOME to a writable directory"
        ),
        CliError::RuntimeMaterializeFailed { detail } => write!(
            f,
            "could not install the Ipe runtime: {detail}\n  \
             the build was stopped rather than link an incomplete runtime"
        ),
        CliError::RuntimeVersionMismatch {
            path,
            found,
            expected,
        } => write!(
            f,
            "the Ipe runtime at {} is version {found}, but this compiler is {expected}; \
             a program emitted by this compiler cannot link a different runtime.\n  \
             = help: this runtime is out of date. Remove the stale copy (the project's \
             `out/` directory, or whatever `IPE_RUNTIME_DIR` points at) and rebuild — the \
             matching runtime re-materializes automatically.",
            path.display()
        ),
        // The caller only dispatches the runtime-install variants here.
        _ => Ok(()),
    }
}

/// Render [`CliError::EmittedBuildFailed`] for `Display`. When `cargo`'s stderr
/// reveals a missing runtime feature, lead with a targeted line naming the
/// out-of-date runtime; otherwise present the trimmed `cargo` error under a clean
/// header. Neither form shows any command's `--help` page.
fn fmt_emitted_build_failed(err: &CliError, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let CliError::EmittedBuildFailed {
        what,
        code,
        stderr,
        runtime,
    } = err
    else {
        // The caller only dispatches the one variant here.
        return Ok(());
    };
    let runtime = runtime.as_ref();
    let trimmed = stderr.trim();
    if let Some(feature) = missing_runtime_feature(trimmed) {
        write!(
            f,
            "building {what} failed: it needs the runtime feature `{feature}`",
        )?;
        if let Some(rt) = runtime {
            write!(
                f,
                ", but the runtime at {} (version {}) does not provide it",
                rt.root.display(),
                rt.version
            )?;
        }
        return write!(
            f,
            ".\n  = help: the runtime is out of date. Remove the stale copy (the project's \
             `out/` directory, or whatever `IPE_RUNTIME_DIR` points at) and rebuild — the \
             matching runtime re-materializes automatically."
        );
    }
    write!(f, "building {what} failed (cargo exited {code})")?;
    if !trimmed.is_empty() {
        write!(f, "\n{trimmed}")?;
    }
    Ok(())
}

/// Extract the runtime feature name from a `cargo` feature-resolution error of
/// the form ``… depends on ipe-runtime-rust with feature `X` but ipe-runtime-rust
/// does not have that feature``. The name is quoted in backticks or single
/// quotes; both are accepted. `None` when the stderr is some other failure.
fn missing_runtime_feature(stderr: &str) -> Option<String> {
    if !stderr.contains("does not have that feature") {
        return None;
    }
    // The name sits between `with feature <q>` and the matching close quote,
    // where the quote is a backtick or a single quote.
    let after = stderr.split_once("with feature ")?.1;
    let mut chars = after.chars();
    let close = match chars.next()? {
        '`' => '`',
        '\'' => '\'',
        _ => return None,
    };
    let rest = chars.as_str();
    let name = rest.split_once(close)?.0;
    if name.is_empty() {
        None
    } else {
        Some(name.to_owned())
    }
}

impl std::error::Error for CliError {}

// `CliError` is the `Err` type of every driver `Result`, so its size is paid
// in the `Err` slot of ~200 functions. Boxing the wide payloads (the `Pipeline`
// diagnostic) keeps it under clippy's `result_large_err` threshold; the bound
// below IS that threshold, so the assertion and the lint enforce one fact. A
// future variant that carries an unboxed wide payload trips both. The bound is
// the lint's ceiling, not the type's current exact size, so it holds on every
// target ABI (`std::io::Error` is wider on Windows than on Linux, for one).
const CLI_ERROR_MAX_BYTES: usize = 128;
// IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — compile-time `const` assertion (not a runtime panic); it fails the build if a future `CliError` variant exceeds the size bound rather than boxing its payload [ledger #boundary]
const _: () = assert!(std::mem::size_of::<CliError>() <= CLI_ERROR_MAX_BYTES);

/// Options modifying a build beyond plain source compilation — some (the
/// static plan) apply post-emit at write time; others (`target`,
/// `wasm_public_env`) feed the compile/emit pipeline itself.
///
/// The static plan is applied post-emit at write time — the compile pipeline
/// and its on-disk caches stay untouched (their keys deliberately exclude
/// the plan; the transform is a deterministic function of the plan applied
/// on cache-hit and cache-miss paths alike).
// The four `bool` fields (`wasm_hydrate_mode`, `production`, `runtime_dep`,
// `tree_shake_vendored`) are genuinely independent, orthogonal build toggles —
// any combination is valid (a production dep-model build, a vendored tree-shaken
// dev build, …). They are not the states of one machine, so collapsing them into
// a two-variant enum or a state enum would obscure their independence rather than
// clarify it; the clippy heuristic's usual remedy does not apply here.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Default)]
pub struct BuildOptions {
    /// `Some` — staticize the emitted project (activate the planned
    /// allocator feature, add the generated `.cargo/config.toml`). `None` —
    /// normal dynamic build; also removes a stale generated static config.
    pub static_plan: Option<ipe_backend_rust::static_build::StaticPlan>,
    /// The compilation target (`Native` default; `WasmClient` under
    /// `ipe build --target wasm`) — threaded into kernel resolution (the
    /// Layer-1 wasm gate), the emitted manifest, and both cache keys.
    pub target: ipe_ir::Target,
    /// The `[wasm] publicEnv` allowlist (`ipe.toml`, already validated
    /// against the secret-name denylist at parse time). Empty when the
    /// project has no `[wasm]` section (or no `ipe.toml` at all — the
    /// sibling-discovery single-file path). Threaded into
    /// [`ipe_backend_rust::RustBackend::with_wasm_public_env`] /
    /// [`ipe_db::BuildConfig::wasm_public_env`].
    pub wasm_public_env: Vec<String>,
    /// `true` when `[wasm] mode = "hydrate"` in the project's `ipe.toml`.
    /// Causes the backend to emit a `#[wasm_bindgen] pub fn hydrate(model_json: &str)`
    /// export in addition to the `#[wasm_bindgen(start)] pub fn ipe_start()` entry.
    /// The emitted `hydrate` function parses the island JSON as the user's declared
    /// `HydrationState` type, converts to `Model` via `fromHydrationState`, and
    /// calls `ipe_runtime::wasm::wasm_adopt_app`. On parse failure it falls back
    /// to clean `ipe_main()` with a console warning (fault-tolerant hydrate — see
    /// spec Q6 §"Fault-tolerant hydrate — parse, don't unwrap").
    pub wasm_hydrate_mode: bool,
    /// `true` for a PRODUCTION build (`ipe build --optimize`). Threaded into
    /// [`ipe_db::BuildConfig::production`] so the emit demand rejects any
    /// development-only `Debug.*` escape hatch (IPE-L0140). Default `false`
    /// (a development build).
    pub production: bool,
    /// `true` (the DEFAULT) selects the dependency-model emit: the emitted
    /// project declares the runtime as a path dependency with a
    /// `runtime_features`-selected feature list and vendors no runtime source.
    /// Applies to BOTH targets — a native project selects the reached native
    /// features; a wasm project selects the `wasm-client` floor plus any
    /// browser-admissible surface it reaches, built for
    /// `wasm32-unknown-unknown`. `false` opts back into the byte-identical
    /// vendored-source emit — the fallback for debugging / a machine without an
    /// installed runtime crate — set via `IPE_RUNTIME_VENDORED=1` (or directly by
    /// a test).
    pub runtime_dep: bool,
    /// `true` tree-shakes the vendored runtime tree to only the modules the
    /// program reaches — the `ipe eject` shape. The emitted `ipe_runtime/mod.rs`
    /// already declares `pub mod X;` for exactly the reached top-level modules,
    /// so [`build_emit_manifest`] vendors only those source files instead of the
    /// whole runtime tree. Ignored unless the emit is the vendored shape (it has
    /// no effect on a dependency-model emit, which carries no vendored source at
    /// all). Default `false`: a plain vendored build copies the whole tree
    /// (rustc drops the undeclared files itself, so the emitted binary is
    /// identical either way — trimming only changes what source lands on disk).
    pub tree_shake_vendored: bool,
}

/// Select the emit model from the environment.
///
/// The dependency model is the DEFAULT; `IPE_RUNTIME_VENDORED=1` opts back into
/// the vendored-source emit (debugging / a machine that cannot resolve the
/// runtime crate). The legacy `IPE_RUNTIME_DEP=1` remains an explicit no-op
/// affirmation of the default. A [`BuildOptions::runtime_dep`] already set by a
/// caller (a test) is what is threaded; this function only computes the
/// env-derived default.
#[must_use]
pub fn runtime_dep_from_env() -> bool {
    !std::env::var("IPE_RUNTIME_VENDORED").is_ok_and(|v| v == "1")
}

impl BuildOptions {
    /// The default build options with the emit model resolved from the
    /// environment (dependency-model by default; vendored under
    /// `IPE_RUNTIME_VENDORED=1`). The zero-configuration entrypoints
    /// ([`build`], [`build_with_sibling_discovery`], [`build_project`]) seed
    /// this so a library caller gets the same default emit model a `ipe build`
    /// invocation does, rather than the raw `Default` (which is vendored — the
    /// fallback shape).
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            runtime_dep: runtime_dep_from_env(),
            ..Self::default()
        }
    }
}

/// Build `entry` into a Rust Cargo project under `out_dir`, vendoring the
/// runtime module tree from `runtime_dir`.
///
/// # Errors
/// Returns [`CliError::Pipeline`] when the compiler rejects the program,
/// [`CliError::Io`] on any filesystem failure.
pub fn build(entry: &Path, out_dir: &Path, runtime_dir: &Path) -> Result<(), CliError> {
    build_with_options(entry, out_dir, runtime_dir, BuildOptions::from_env())
}

/// [`build`] with explicit [`BuildOptions`] (the static-plan-aware variant).
///
/// # Errors
/// As [`build`], plus [`CliError::StaticRefusal`] when the emitted app shape
/// cannot be static (an `Ipe.WebView` app under a static plan).
pub fn build_with_options(
    entry: &Path,
    out_dir: &Path,
    runtime_dir: &Path,
    options: BuildOptions,
) -> Result<(), CliError> {
    let source = fs::read_to_string(entry).map_err(|e| io_err(entry, e))?;

    // Parse ONCE with a throwaway interner to learn the entry's declared module
    // path. Using the declared name as the entry's `module_path` means the shared
    // graph core's N0023 (path mismatch) can never fire for a single-file build
    // (expected == declared by construction), while still routing a single-file
    // program through the SAME injection-aware pipeline as a project — so a
    // single file importing `Ipe.Palette` injects the compiled source instead of
    // 404-ing (design §2.6). For a program with no compiled-source import the
    // core is emit-byte-identical to a plain single-module path (link over one
    // module is the identity — regression-covered by the golden suite).
    let mut name_interner = Interner::new();
    let pipeline_err = |diag: Diagnostic| CliError::Pipeline {
        file: entry.to_path_buf(),
        src: source.clone(),
        diag: Box::new(diag),
    };
    let parsed = ipe_parse::parse_module(&source, &mut name_interner).map_err(&pipeline_err)?;
    let entry_path: Vec<String> = parsed
        .name
        .value
        .iter()
        .map(|s| name_interner.resolve(*s).unwrap_or_default().to_owned())
        .collect();

    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    sources.insert(entry_path.clone(), (entry.to_path_buf(), source.clone()));
    let discovered = vec![project::DiscoveredModule {
        path: entry.to_path_buf(),
        module_path: entry_path.clone(),
    }];

    // No manifest on the single-file path — default to sqlite, matching the
    // documented `ipe.toml` default for a project that has no `[database]`
    // section at all.
    compile_modules(
        sources,
        discovered,
        &entry_path,
        out_dir,
        runtime_dir,
        entry,
        ipe_backend_rust::DbDriver::Sqlite,
        options,
    )
}

/// Build a `.ipe` entry file and all sibling modules discovered in the same
/// source directory.
///
/// When no `ipe.toml` is present, the entry file's parent directory is used
/// as the source root. Every `*.ipe` file found there is loaded and compiled
/// together — fixing IPE-N0020 for multi-file projects built via the
/// file-path shorthand (`ipe build src/Main.ipe`).
///
/// This is the faithful port of Haskell's `Graph.discoverModulesMulti
/// (sourceRoot : ...) entryPath` call in `Ipe.Build.Compile.hs`: it probes
/// the source root recursively and follows imports across sibling files before
/// running the shared `compile_modules` core.
///
/// When the source directory contains only the entry file this function is
/// byte-identical to `build` (single-module pipeline is the identity over
/// `link`).
///
/// # Errors
/// [`CliError::Pipeline`] when the compiler rejects the program.
/// [`CliError::Io`] on any filesystem failure.
pub fn build_with_sibling_discovery(
    entry: &Path,
    out_dir: &Path,
    runtime_dir: &Path,
) -> Result<(), CliError> {
    build_with_sibling_discovery_with_options(entry, out_dir, runtime_dir, BuildOptions::from_env())
}

/// [`build_with_sibling_discovery`] with explicit [`BuildOptions`] (the
/// static-plan-aware variant).
///
/// # Errors
/// As [`build_with_sibling_discovery`], plus [`CliError::StaticRefusal`]
/// when the emitted app shape cannot be static.
pub fn build_with_sibling_discovery_with_options(
    entry: &Path,
    out_dir: &Path,
    runtime_dir: &Path,
    options: BuildOptions,
) -> Result<(), CliError> {
    let collected = collect_entry_and_siblings(entry)?;

    // No ipe.toml on this path either (sibling discovery is the "no manifest
    // found" fallback) — default to sqlite, same rationale as `build`.
    compile_modules(
        collected.sources,
        collected.discovered,
        &collected.entry_module_path,
        out_dir,
        runtime_dir,
        entry,
        ipe_backend_rust::DbDriver::Sqlite,
        options,
    )
}

/// Build `ipe verify`'s test entry against the project's `src/` sources.
///
/// Unlike [`build_with_sibling_discovery`], which roots discovery at the
/// entry's own directory, this roots the code under test at `project_src_root`
/// (the `src/` tree) and additionally discovers the test entry's own directory
/// (the `tests/` tree) — so a `tests/Main.ipe` that imports `Lib.Foo` from
/// `src/Lib/Foo.ipe` resolves. See [`collect_test_sources`] for the source-set
/// model.
///
/// # Errors
/// [`CliError::Pipeline`] when the compiler rejects the program; [`CliError::Io`]
/// on any filesystem failure; [`CliError::StaticRefusal`] when the emitted app
/// shape cannot be static.
fn build_test_with_project_sources(
    project_src_root: &Path,
    test_entry: &Path,
    out_dir: &Path,
    runtime_dir: &Path,
) -> Result<(), CliError> {
    let collected = collect_test_sources(project_src_root, test_entry)?;

    // No ipe.toml driver is threaded here (the test stage mirrors the sibling
    // build's "no manifest" fallback) — default to sqlite, same rationale as
    // `build_with_sibling_discovery`.
    compile_modules(
        collected.sources,
        collected.discovered,
        &collected.entry_module_path,
        out_dir,
        runtime_dir,
        test_entry,
        ipe_backend_rust::DbDriver::Sqlite,
        BuildOptions::from_env(),
    )
}

/// The entry file and every sibling `.ipe` module discovered in its source
/// directory, ready to feed the shared compile core.
struct CollectedSources {
    sources: BTreeMap<Vec<String>, (PathBuf, String)>,
    discovered: Vec<project::DiscoveredModule>,
    entry_module_path: Vec<String>,
}

/// Collect the entry module plus every sibling `.ipe` file in its source
/// directory, reading each source once.
///
/// This is the file-path shorthand's source-collection step, shared by the
/// build path ([`build_with_sibling_discovery_with_options`]) and the
/// single-entry analysis paths ([`lower_entry`], [`emit_ir_text`]) so all
/// three see the SAME module set — a program that imports a compiled-source
/// stdlib module resolves identically whether it is built or merely analysed.
/// It is the equivalent of `Graph.discoverModulesMulti [srcRoot] entryPath` in
/// `Ipe.Build.Compile.hs`; the compiled-source stdlib closure is injected
/// downstream (in [`compile_modules_observed`] / [`lower_entry_via_graph`]),
/// not here, so the injection routine stays single-sourced.
///
/// # Errors
/// [`CliError::Pipeline`] when the entry does not parse; [`CliError::Io`] on
/// any filesystem failure reading a discovered module.
fn collect_entry_and_siblings(entry: &Path) -> Result<CollectedSources, CliError> {
    let source = fs::read_to_string(entry).map_err(|e| io_err(entry, e))?;
    let entry_module_path = parse_entry_module_path(entry, &source)?;

    // Source root: the directory containing the entry file.
    let src_root = entry
        .parent()
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| Path::new("."));

    // Discover ALL .ipe files in the source root (recursively).
    let mut discovered = project::discover_modules(src_root)?;
    ensure_entry_present(&mut discovered, entry, &entry_module_path);

    let sources = read_discovered_sources(&discovered, entry, &entry_module_path, &source)?;

    Ok(CollectedSources {
        sources,
        discovered,
        entry_module_path,
    })
}

/// Collect the sources for `ipe verify`'s test stage: the project's `src/`
/// tree (the code under test) unioned with the `tests/` tree (the test entry
/// and any test-only siblings).
///
/// A test entry lives in a sibling directory from the code it exercises
/// (`tests/Main.ipe` importing `Lib.Db` under `src/Lib/`), so a single-root
/// discovery cannot see both: `src/` and `tests/` must be relativised against
/// their OWN roots for module paths to resolve (`src/Lib/Foo.ipe` → `Lib.Foo`,
/// `tests/Main.ipe` → `Main`). This resolves both rooted discoveries into one
/// well-typed [`CollectedSources`] whose entry is the test module, so a test
/// module can import the code under test AND its test-only siblings.
///
/// When a module path is defined in both trees, the `src/` definition wins for
/// non-entry modules — the code under test is authoritative — while the entry
/// module is always the test entry itself.
///
/// # Errors
/// [`CliError::Pipeline`] when the test entry does not parse; [`CliError::Io`]
/// on any filesystem failure reading a discovered module.
fn collect_test_sources(
    project_src_root: &Path,
    test_entry: &Path,
) -> Result<CollectedSources, CliError> {
    let entry_source = fs::read_to_string(test_entry).map_err(|e| io_err(test_entry, e))?;
    let entry_module_path = parse_entry_module_path(test_entry, &entry_source)?;

    // The `tests/` tree: the directory holding the test entry, rooted at itself
    // so `tests/Main.ipe` → `Main` and `tests/Support/Fixtures.ipe` →
    // `Support.Fixtures`.
    let tests_root = test_entry
        .parent()
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| Path::new("."));

    // The code under test: the `src/` tree, rooted at itself so
    // `src/Lib/Foo.ipe` → `Lib.Foo` — the SAME relativisation the build stage
    // uses. Union the two rooted discoveries; a `src/` module masks a `tests/`
    // module of the same path (code under test wins), and the test entry is
    // always added last so it is never masked.
    let mut discovered = project::discover_modules(project_src_root)?;
    let src_paths: std::collections::BTreeSet<Vec<String>> =
        discovered.iter().map(|m| m.module_path.clone()).collect();
    for m in project::discover_modules(tests_root)? {
        if !src_paths.contains(&m.module_path) {
            discovered.push(m);
        }
    }
    ensure_entry_present(&mut discovered, test_entry, &entry_module_path);

    let sources =
        read_discovered_sources(&discovered, test_entry, &entry_module_path, &entry_source)?;

    Ok(CollectedSources {
        sources,
        discovered,
        entry_module_path,
    })
}

/// Parse a `.ipe` entry file's already-read source to learn its declared
/// module path (e.g. `["Lib", "Db"]`).
///
/// # Errors
/// [`CliError::Pipeline`] when the source does not parse.
fn parse_entry_module_path(entry: &Path, source: &str) -> Result<Vec<String>, CliError> {
    let pipeline_err = |diag: Diagnostic| CliError::Pipeline {
        file: entry.to_path_buf(),
        src: source.to_owned(),
        diag: Box::new(diag),
    };
    let mut name_interner = Interner::new();
    let parsed = ipe_parse::parse_module(source, &mut name_interner).map_err(&pipeline_err)?;
    Ok(parsed
        .name
        .value
        .iter()
        .map(|s| name_interner.resolve(*s).unwrap_or_default().to_owned())
        .collect())
}

/// Ensure the entry itself is in the discovered set, even when its file name
/// does not match the module-segment validation (e.g. a temp path). This
/// prevents the entry from being silently dropped.
fn ensure_entry_present(
    discovered: &mut Vec<project::DiscoveredModule>,
    entry: &Path,
    entry_module_path: &[String],
) {
    if !discovered
        .iter()
        .any(|m| m.module_path == entry_module_path)
    {
        discovered.push(project::DiscoveredModule {
            path: entry.to_path_buf(),
            module_path: entry_module_path.to_vec(),
        });
    }
}

/// Read every discovered module into the module-path-keyed source map. The
/// entry's source is already in memory (`entry_source`), so it is inserted
/// without a second read.
///
/// # Errors
/// [`CliError::Io`] on any filesystem failure reading a discovered module.
fn read_discovered_sources(
    discovered: &[project::DiscoveredModule],
    entry: &Path,
    entry_module_path: &[String],
    entry_source: &str,
) -> Result<BTreeMap<Vec<String>, (PathBuf, String)>, CliError> {
    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    for m in discovered {
        if m.module_path == entry_module_path {
            sources.insert(
                entry_module_path.to_vec(),
                (entry.to_path_buf(), entry_source.to_owned()),
            );
        } else {
            let src = fs::read_to_string(&m.path).map_err(|e| io_err(&m.path, e))?;
            sources.insert(m.module_path.clone(), (m.path.clone(), src));
        }
    }
    Ok(sources)
}

/// Walk up the directory tree from a `.ipe` file's parent, looking for a
/// `ipe.toml` manifest. Returns the manifest path if found, or `None` when
/// the walk reaches the filesystem root.
///
/// Faithful port of the Haskell `ipe build src/Main.ipe` behavior: when
/// given a file entry the Haskell driver locates the project root (where
/// `ipe.toml` lives) before calling `buildProject`, so the full module graph
/// is compiled instead of just the single entry file.
fn find_manifest_for_ipe_file(ipe_file: &Path) -> Option<PathBuf> {
    let mut dir = ipe_file.parent()?;
    loop {
        let candidate = dir.join("ipe.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
}

/// Whether [`compile_modules_observed`] served an on-disk build-cache
/// hit or ran the full compile pipeline. Exists for tests and future CLI
/// verbosity — [`compile_modules`] (used by every stable entry point) does
/// not need it and discards it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CacheOutcome {
    /// A matching, same-epoch [`ipe_backend::EmittedProject`] entry was
    /// found on disk; the whole compile pipeline (parse through emit) was
    /// skipped.
    Hit,
    /// No `EmittedProject`-tier entry existed, but a matching, same-epoch
    /// lowered-[`ipe_ir::Program`] entry was — parse through
    /// lower were skipped; only `RustBackend::emit` ran over the relocated
    /// IR (see `crate::cache`'s lowered-IR module doc section).
    IrHit,
    /// No usable entry existed at either tier (cache disabled, epoch
    /// undeterminable, key miss, or corrupt entry) — the full pipeline ran.
    Miss,
}

/// The shared multi-module compile core: inject the compiled-source stdlib
/// closure, topologically order the graph, canonicalise each module dep-first
/// (with its unforgeable [`ipe_canon::ModuleOrigin`]), link, then infer → lower →
/// emit → write. Both [`build`] and [`build_project`] route through this so the
/// injection seam is identical on the single-file and project paths.
///
/// `blame_path` is the file a cross-file diagnostic with no single owner (an
/// import cycle) is rendered against.
///
/// # Errors
/// [`CliError::Pipeline`] carrying the first compiler diagnostic; [`CliError::Io`]
/// on any filesystem failure.
#[allow(clippy::too_many_arguments)]
fn compile_modules(
    sources: BTreeMap<Vec<String>, (PathBuf, String)>,
    discovered: Vec<project::DiscoveredModule>,
    entry_path: &[String],
    out_dir: &Path,
    runtime_dir: &Path,
    blame_path: &Path,
    db_driver: ipe_backend_rust::DbDriver,
    options: BuildOptions,
) -> Result<(), CliError> {
    let cache_dir = cache::env_cache_dir(out_dir);
    compile_modules_observed(
        sources,
        discovered,
        entry_path,
        out_dir,
        runtime_dir,
        blame_path,
        db_driver,
        cache_dir.as_deref(),
        options,
    )
    .0
}

/// [`compile_modules`]'s full implementation, with the on-disk build
/// cache's root made an EXPLICIT parameter (`None` disables the cache
/// entirely) rather than read from the environment internally — the
/// dependency-injection seam this module's tests use instead of
/// `std::env::set_var` (which is `unsafe` as of the standard library's
/// current signature, and this crate is `#![forbid(unsafe_code)]`; a
/// same-process env mutation would also be a cross-test race under a
/// shared-process runner, though `cargo nextest` avoids that specific
/// hazard by isolating tests into their own processes — the explicit
/// parameter avoids both concerns at once).
///
/// Cache flow (see `crate::cache`'s module doc for the full design): the
/// content-address key and version-epoch are computed
/// BEFORE any salsa database exists (driver-boundary only — INV-1: no
/// `std::fs` on a tracked path). On a hit, the ENTIRE compile pipeline
/// (parse through emit) is skipped; only [`write_emitted_project`] runs,
/// materialising the cached [`ipe_backend::EmittedProject`] verbatim. On a
/// miss, the full pipeline runs, and a successful
/// result is best-effort stored for the next invocation.
// `options` is threaded onward by value into the cache-hit / full-pipeline
// branches below (mirroring every sibling `BuildOptions` consumer in this
// file); a `&BuildOptions` parameter would just push the clone this struct's
// `Vec<String>` field (`wasm_public_env`) now needs onto every call site
// instead of the one place that actually reads it.
#[allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::needless_pass_by_value
)]
fn compile_modules_observed(
    mut sources: BTreeMap<Vec<String>, (PathBuf, String)>,
    mut discovered: Vec<project::DiscoveredModule>,
    entry_path: &[String],
    out_dir: &Path,
    runtime_dir: &Path,
    blame_path: &Path,
    db_driver: ipe_backend_rust::DbDriver,
    cache_dir: Option<&Path>,
    options: BuildOptions,
) -> (Result<(), CliError>, CacheOutcome) {
    // Inject the transitive compiled-source stdlib closure. `injected` is the
    // driver's unforgeable record of which module paths are trusted stdlib
    // source — the ONLY inputs that earn `ModuleOrigin::EmbeddedStdlib` below.
    let injected = project::inject_compiled_std_closure(&mut sources, &mut discovered);

    // The FFI seam: the SAME catalog-load → nominal-unification →
    // interface-inject → emit-assemble sequence `watch` and `lsp` use
    // (`prepare_ffi`) — a divergent copy here once skipped the unification
    // step entirely.
    let ffi_prep = match ffi::prepare_ffi(&mut sources, blame_path) {
        Ok(p) => p,
        Err(e) => return (Err(e), CacheOutcome::Miss),
    };
    let ffi_injected = ffi_prep.injected;
    let ffi_emit = ffi_prep.emit;

    // Resolve the dependency-model runtime crate ONCE, fail-closed: if the
    // opt-in is set but no verified `ipe-runtime-rust` crate root is found, the
    // build refuses loudly here rather than falling back to a vendored — or
    // worse, a wrong — runtime. Native and wasm share the ONE dependency model
    // (the wasm emit selects the crate's `wasm-client` floor + reached surface,
    // built for `wasm32-unknown-unknown`).
    let runtime_dep = if options.runtime_dep {
        match runtime_embed::resolve() {
            Ok(resolved) => Some(ipe_backend_rust::RuntimeDep {
                root: resolved.root().to_path_buf(),
            }),
            Err(e) => return (Err(e), CacheOutcome::Miss),
        }
    } else {
        None
    };

    // The on-disk build caches key only the Ipê sources — the FFI bindings
    // text and opaque map live OUTSIDE that key, so a cache hit could serve a
    // stale emitted project after `ipe add`/`ipe remove`. Disable both cache
    // tiers for FFI-using builds (correctness over warm-start speed).
    // The dependency-model flag also changes emit shape without changing the
    // Ipê sources, so a cache keyed only on sources must not serve a
    // cross-model artifact: disable the caches when the dep model is active.
    let cache_dir = if ffi_emit.is_some() || runtime_dep.is_some() {
        None
    } else {
        cache_dir
    };

    // The on-disk build cache. `epoch` folds in BOTH the running
    // `ipe` binary's own content hash and the active `rustc`'s fingerprint
    // (see `cache::derive_epoch`'s doc for why this makes
    // "refuse, don't guess" structural rather than a runtime check).
    let cache_key = cache::compute_project_key(
        &sources,
        &injected,
        entry_path,
        db_driver,
        options.target,
        &options.wasm_public_env,
        options.production,
    );
    let epoch = cache_dir.and_then(|_| cache::derive_epoch());
    if let (Some(root), Some(epoch)) = (cache_dir, epoch.as_deref())
        && let Some(emitted) = cache::try_load(root, epoch, &cache_key)
    {
        return (
            write_emitted_project(
                &emitted,
                out_dir,
                runtime_dir,
                options.static_plan.as_ref(),
                options.tree_shake_vendored,
            ),
            CacheOutcome::Hit,
        );
    }

    // The lowered-IR cache tier (see `crate::cache`'s module doc
    // section for the full design). A hit here skips parse -> canon -> link
    // -> infer -> lower ENTIRELY — no `IpeDatabase` is constructed at all —
    // running only `RustBackend::emit` over the relocated `Program` before
    // falling through to the SAME disk-write + tier-1-warming path a full
    // pipeline run uses. The `ir_key` deliberately excludes `db_driver`
    // (`compute_ir_key`'s own doc explains why), so this tier can still hit
    // when the `EmittedProject` tier just missed on a `db_driver`-only edit.
    if let (Some(root), Some(epoch)) = (cache_dir, epoch.as_deref()) {
        let ir_key = cache::compute_ir_key(&sources, &injected, entry_path, options.target);
        let fresh_interner: std::sync::Arc<std::sync::Mutex<ipe_intern::Interner>> =
            std::sync::Arc::new(std::sync::Mutex::new(ipe_intern::Interner::new()));
        if let Some(program) = cache::try_load_ir(root, epoch, &ir_key, &fresh_interner) {
            use ipe_backend::Backend as _;
            // Production gate on the IR-cache fast path: this path bypasses
            // `emit_project` (the DB layer where the gate normally runs), so a
            // cached IR that uses a development-only `Debug.*` escape hatch must
            // be rejected here too (IPE-L0140) — otherwise `--optimize` would
            // ship the debug window whenever the IR tier happened to hit.
            if options.production && program.modules.iter().any(|m| m.uses_debug) {
                let diag = Diagnostic::Lower {
                    span: ipe_diagnostics::Span::DUMMY,
                    msg: ipe_diagnostics::LowerError::DevOnlyKernelInProduction {
                        kernel: "Debug.log".into(),
                    },
                };
                let src = std::fs::read_to_string(blame_path).unwrap_or_default();
                return (
                    Err(CliError::Pipeline {
                        file: blame_path.to_path_buf(),
                        src,
                        diag: Box::new(diag),
                    }),
                    CacheOutcome::IrHit,
                );
            }
            let emit_result = {
                let guard = fresh_interner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                ipe_backend_rust::RustBackend::new(&guard)
                    .with_db_driver(db_driver)
                    .with_target(options.target)
                    .with_wasm_public_env(options.wasm_public_env.clone())
                    .with_wasm_hydrate_mode(options.wasm_hydrate_mode)
                    .with_runtime_dep(runtime_dep.clone())
                    .emit(&program)
            };
            if let Ok(emitted) = emit_result {
                // Warm the (cheaper-to-hit) EmittedProject tier for the
                // next build too — advisory, best-effort, same as every
                // other cache-write in this module.
                cache::store(root, epoch, &cache_key, &emitted);
                return (
                    write_emitted_project(
                        &emitted,
                        out_dir,
                        runtime_dir,
                        options.static_plan.as_ref(),
                        options.tree_shake_vendored,
                    ),
                    CacheOutcome::IrHit,
                );
            }
            // A relocated Program that fails to emit is never a build
            // failure from this fast path — fall through to the full
            // pipeline exactly as a tier-2 miss would. This should not
            // happen for a genuinely-cached (not tampered, not epoch-
            // mismatched) entry, but the advisory contract holds
            // regardless of why.
        }
    }

    // Salsa database (see
    // docs/architecture/salsa-incremental-compilation-2026-07-11.md). The
    // driver parses external state ONCE into typed inputs here (`SourceFile`
    // per module + the `SourceRoot` file set); the front-end stages are
    // demanded as memoized queries inside `compile_prepared`. The database is
    // cold and per-invocation, and queries are demanded in the fixed topo
    // order, so the interning sequence — and therefore emitted bytes — is
    // deterministic across runs (golden-suite-enforced).
    let db = ipe_db::IpeDatabase::new();
    let source_root = create_source_root(&db, &sources, &injected, &ffi_injected);
    // The config input (see `ipe_db::BuildConfig`'s doc for why this
    // is narrowed to `db_driver` rather than the full `ipe.toml` shape). A
    // fresh `BuildConfig` per one-shot invocation is fine here — unlike the
    // clean-vs-incremental parity gate's warm sequence, this driver never
    // re-demands `emit_project` against a second config instance.
    let config = ipe_db::BuildConfig::new(
        &db,
        db_driver,
        ffi_emit,
        options.target,
        options.wasm_public_env.clone(),
        options.wasm_hydrate_mode,
        options.production,
        runtime_dep,
    );

    let emitted = match compile_prepared(&db, source_root, &sources, entry_path, blame_path, config)
    {
        Ok(emitted) => emitted,
        Err(e) => return (Err(e), CacheOutcome::Miss),
    };

    if let (Some(root), Some(epoch)) = (cache_dir, epoch.as_deref()) {
        cache::store(root, epoch, &cache_key, &emitted);
        // Also store the lowered `Program` at the IR tier.
        // `ipe_db::lower_program` is a PURE MEMO HIT here — it already ran
        // (transitively, via `compile_prepared`'s `emit_project` demand
        // chain) inside the salsa database above, so this costs nothing
        // beyond the lookup + relocation-pass serialize. Best-effort: an
        // entry-file lookup failure or a serialize failure never turns a
        // successful build into a reported failure (same advisory contract
        // as the `EmittedProject` tier's own store).
        if let Some(entry_file) = source_root.files(&db).get(entry_path).copied()
            && let Ok(program) = ipe_db::lower_program(&db, source_root, entry_file)
        {
            let ir_key = cache::compute_ir_key(&sources, &injected, entry_path, options.target);
            cache::store_ir(
                root,
                epoch,
                &ir_key,
                &program,
                ipe_db::Db::interner(&db).as_arc(),
            );
        }
    }

    (
        write_emitted_project(
            &emitted,
            out_dir,
            runtime_dir,
            options.static_plan.as_ref(),
            options.tree_shake_vendored,
        ),
        CacheOutcome::Miss,
    )
}

/// Create the salsa inputs for one build: a [`ipe_db::SourceFile`] per module
/// plus the [`ipe_db::SourceRoot`] file set.
///
/// The trust tag: `EmbeddedStdlib` IFF the module path is in `injected` (the
/// driver's unforgeable record from [`project::inject_compiled_std_closure`]).
/// A user file squatting on `Ipe.Foo` is NOT in `injected` (injection skipped
/// it on the pre-existing-key guard), so it is `User` and stays
/// IPE-N0025-rejected.
#[must_use]
pub fn create_source_root(
    db: &ipe_db::IpeDatabase,
    sources: &BTreeMap<Vec<String>, (PathBuf, String)>,
    injected: &std::collections::BTreeSet<Vec<String>>,
    ffi_injected: &std::collections::BTreeSet<Vec<String>>,
) -> ipe_db::SourceRoot {
    let file_handles: BTreeMap<Vec<String>, ipe_db::SourceFile> = sources
        .iter()
        .map(|(mod_path, (_, src))| {
            let origin = if injected.contains(mod_path) {
                ipe_canon::ModuleOrigin::EmbeddedStdlib
            } else if ffi_injected.contains(mod_path) {
                ipe_canon::ModuleOrigin::FfiInterface
            } else {
                ipe_canon::ModuleOrigin::User
            };
            (
                mod_path.clone(),
                ipe_db::SourceFile::new(db, mod_path.clone(), src.clone(), origin),
            )
        })
        .collect();
    ipe_db::SourceRoot::new(db, file_handles)
}

/// Intern each module path in `sources` to build the module-home →
/// `(file, src)` blame map every span-attribution step reads. The lookups run
/// against symbols `canonicalize` already interned, so this cannot append a new
/// symbol and cannot perturb interning order (the golden byte-identity SEAL).
fn home_to_source_map(
    interner: &ipe_db::SharedInterner,
    sources: &BTreeMap<Vec<String>, (PathBuf, String)>,
) -> BTreeMap<Vec<ipe_intern::Symbol>, (PathBuf, String)> {
    let mut guard = interner.lock();
    let mut map = BTreeMap::new();
    for (str_path, (file, src)) in sources {
        let sym_path: Result<Vec<_>, _> = str_path.iter().map(|s| guard.intern(s)).collect();
        if let Ok(sym_path) = sym_path {
            map.insert(sym_path, (file.clone(), src.clone()));
        }
    }
    map
}

/// Map a diagnostic span (byte offsets into its *home* module's source) to the
/// `(file, src)` pair that source is rendered against.
///
/// Every span in a def's body is a byte offset into that def's home module —
/// preserved across `link`. Among all defs whose `body_span` contains the
/// target span, prefer the one whose `body_span.lo` is *closest* to `span.lo`
/// (the def starting nearest the failing expression); width is the secondary
/// tiebreaker (narrower body wins on a tie). Union constructor spans live in
/// the union's home byte-namespace, outside any def body, so they are scanned
/// too — without this a `lower_enum` error (IPE-L0102 / IPE-L0114) would fall
/// back to the entry file at a coincidental byte offset.
///
/// The closest-lo criterion is what keeps this from picking a numerically
/// narrower def in a *different* module (a different byte namespace): same-module
/// defs share a byte namespace, so the intended def almost always has the
/// smaller distance from its own `lo`. Falls back to `entry` when no def or
/// constructor encloses the span (e.g. a `CompilerBug` with `Span::DUMMY`).
fn source_for_span_in_linked(
    linked: &ipe_canon::ast::Module,
    home_to_source: &BTreeMap<Vec<ipe_intern::Symbol>, (PathBuf, String)>,
    entry: &(PathBuf, String),
    span: ipe_diagnostics::Span,
) -> (PathBuf, String) {
    if span == ipe_diagnostics::Span::DUMMY {
        return entry.clone();
    }
    // (lo_dist, width, home)
    let mut best: Option<(u32, u32, &[ipe_intern::Symbol])> = None;
    for def in &linked.defs {
        let body_span = match def {
            ipe_canon::ast::Def::Untyped { body, .. } | ipe_canon::ast::Def::Typed { body, .. } => {
                body.span
            }
        };
        if body_span.lo <= span.lo && span.hi <= body_span.hi {
            let lo_dist = span.lo.saturating_sub(body_span.lo);
            let width = body_span.hi.saturating_sub(body_span.lo);
            if best.is_none_or(|(prev_dist, prev_w, _)| {
                lo_dist < prev_dist || (lo_dist == prev_dist && width < prev_w)
            }) {
                best = Some((lo_dist, width, def.home()));
            }
        }
    }
    for union in &linked.unions {
        for ctor in &union.ctors {
            if ctor.span.lo <= span.lo && span.hi <= ctor.span.hi {
                let lo_dist = span.lo.saturating_sub(ctor.span.lo);
                let width = ctor.span.hi.saturating_sub(ctor.span.lo);
                if best.is_none_or(|(prev_dist, prev_w, _)| {
                    lo_dist < prev_dist || (lo_dist == prev_dist && width < prev_w)
                }) {
                    best = Some((lo_dist, width, union.home.as_slice()));
                }
            }
        }
    }
    best.and_then(|(_, _, home)| home_to_source.get(home))
        .cloned()
        .unwrap_or_else(|| entry.clone())
}

/// Attribute a `(diag, home)` query error to the source file that OWNS it.
///
/// A non-empty `home` resolves DIRECTLY via `home_to_source` (O(log N), exact);
/// an empty home (homeless backend/emit error, or a non-solver error) falls
/// back to the byte-offset heuristic over the linked program. This is the
/// single attribution rule every post-link pipeline error shares, so `ipe build`
/// and `ipe type-check` frame the identical diagnostic against the identical source.
fn attribute_post_link_error(
    linked: &ipe_canon::ast::Module,
    home_to_source: &BTreeMap<Vec<ipe_intern::Symbol>, (PathBuf, String)>,
    entry: &(PathBuf, String),
    diag: Diagnostic,
    home: &[ipe_intern::Symbol],
) -> CliError {
    let (file, src) = if home.is_empty() {
        source_for_span_in_linked(linked, home_to_source, entry, diag_span(&diag))
    } else {
        home_to_source.get(home).cloned().unwrap_or_else(|| {
            source_for_span_in_linked(linked, home_to_source, entry, diag_span(&diag))
        })
    };
    CliError::Pipeline {
        file,
        src,
        diag: Box::new(diag),
    }
}

/// Run the canon decoder-pipeline direction gate (IPE-N0040) over the linked
/// program, returning the rejection in the post-link `(diag, home)` shape both
/// the build and the type-check surfaces attribute through.
///
/// The gate rejects the reverse-associated hand-nested spelling of the
/// `required` / `optional` / `requiredAt` / `custom` decoder combinators, which
/// silently swaps two same-typed fields with no type error. It runs on the
/// LINKED module so a decoder split across modules is still seen whole. Both
/// [`compile_prepared`] (the build/lower/emit path) and the `ipe type-check`
/// flow call THIS one helper, so the two surfaces cannot drift on whether the
/// footgun is caught. The returned `home` is empty: the diagnostic's own span
/// carries the offending source location, resolved by the byte-offset heuristic
/// the other homeless post-link errors already use.
fn gate_decoder_pipelines(
    linked: &ipe_canon::ast::Module,
) -> Result<(), (Diagnostic, Vec<ipe_intern::Symbol>)> {
    ipe_canon::decoder_pipeline_gate::check_decoder_pipelines(linked)
        .map_err(|diag| (diag, Vec::new()))
}

/// Demand `canonicalize` for every module in dep-first order, attributing a
/// canon error (e.g. IPE-N0020 module-not-found) to the source file of the
/// module that produced it. A canon error fires *before* `link`, so there is no
/// linked program to run the byte-offset heuristic against; the module whose
/// `canonicalize` fails IS the owner, so blaming that module's `(path, src)` is
/// exact.
///
/// On the build path these demands are the memoized inputs `linked_program`
/// re-uses; running the loop here first (also on the `check`/analysis paths)
/// makes a canon-error diagnostic frame against its own file on every surface.
///
/// # Errors
/// [`CliError::Pipeline`] carrying the first module's canon error;
/// [`CliError::Usage`] if a topo-ordered module is absent from the source map.
fn attribute_canon_errors(
    db: &ipe_db::IpeDatabase,
    source_root: ipe_db::SourceRoot,
    sources: &BTreeMap<Vec<String>, (PathBuf, String)>,
    entry_file: ipe_db::SourceFile,
    blame_path: &Path,
) -> Result<(), CliError> {
    let topo =
        ipe_db::topo_order(db, source_root, entry_file).map_err(|diag| CliError::Pipeline {
            file: blame_path.to_path_buf(),
            src: String::new(),
            diag: Box::new(diag),
        })?;
    for mod_path in topo.iter() {
        let Some((path, src)) = sources.get(mod_path) else {
            return Err(CliError::Usage(
                "internal: module in topo order not in source map",
            ));
        };
        let Some(file_handle) = source_root.files(db).get(mod_path).copied() else {
            return Err(CliError::Usage(
                "internal: module in topo order not in source map",
            ));
        };
        ipe_db::canonicalize(db, source_root, file_handle).map_err(|diag| CliError::Pipeline {
            file: path.clone(),
            src: src.clone(),
            diag: Box::new(diag),
        })?;
    }
    Ok(())
}

/// The in-memory compile core over an already-populated database.
///
/// topo order → per-module canonicalisation (memoized, blame-attributed) →
/// [`ipe_db::linked_program`] (the coarse whole-program spine) → infer → lower →
/// emit. Returns the emitted project without touching the filesystem.
///
/// This is THE production pipeline — [`compile_modules`] wraps it with input
/// creation and disk writes, and the clean-vs-incremental parity gate
/// drives it against both cold and warm databases, so the gate can
/// never test a divergent copy of the pipeline.
///
/// `sources` is consulted for diagnostic blame only (module path → file/src).
///
/// `config` is the [`ipe_db::BuildConfig`] handle — callers that
/// re-demand `compile_prepared` across a warm sequence (the parity
/// gate) MUST hold one stable `BuildConfig` across the sequence rather than
/// constructing a fresh one per call, or `emit_project`'s memo key never
/// matches between calls and the seam's memoization is silently defeated.
///
/// # Errors
/// [`CliError::Pipeline`] carrying the first compiler diagnostic.
#[allow(clippy::too_many_lines)]
pub fn compile_prepared(
    db: &ipe_db::IpeDatabase,
    source_root: ipe_db::SourceRoot,
    sources: &BTreeMap<Vec<String>, (PathBuf, String)>,
    entry_path: &[String],
    blame_path: &Path,
    config: ipe_db::BuildConfig,
) -> Result<ipe_backend::EmittedProject, CliError> {
    // The build-wide interner is owned by the database (Option 3a) so the
    // parse query and the non-salsa passes share one symbol table. NEVER hold
    // a lock guard across a salsa query demand (the mutex is not reentrant).
    let shared_interner = ipe_db::Db::interner(db).clone();

    let Some(entry_file) = source_root.files(db).get(entry_path).copied() else {
        return Err(CliError::Usage("internal: entry module not in source map"));
    };

    // Canonicalise each module in dep-first order, attributing a canon error
    // (e.g. IPE-N0020) to its own module's file — the SAME blame loop the
    // `check`/analysis surfaces reuse (`attribute_canon_errors`), so a
    // canon-error diagnostic frames against its own file on every surface.
    // `linked_program` below re-demands these memos.
    attribute_canon_errors(db, source_root, sources, entry_file, blame_path)?;

    // Link → infer → lower → emit on the merged module. Blame link/lower/emit
    // errors on the entry file; infer errors and warnings are attributed to the
    // dep module that owns the failing span.
    let entry_src_path = sources
        .get(entry_path)
        .map_or_else(|| blame_path.to_path_buf(), |(p, _)| p.clone());
    let entry_src = sources
        .get(entry_path)
        .map(|(_, s)| s.clone())
        .unwrap_or_default();
    let pipeline_err = |diag: ipe_diagnostics::Diagnostic| CliError::Pipeline {
        file: entry_src_path.clone(),
        src: entry_src.clone(),
        diag: Box::new(diag),
    };

    // The coarse whole-program spine: every per-module canonical result
    // assembled + linked inside salsa. All
    // `canonicalize` demands above are memo hits here. The link step gates
    // cross-module type-identity duplicates `(home, name)`, blamed on
    // the entry file like every other post-link diagnostic.
    let linked_program =
        ipe_db::linked_program(db, source_root, entry_file).map_err(&pipeline_err)?;
    let linked = &linked_program.module;

    // The fresh-name collision universe for this build: the identifier words
    // of the CURRENT program — a pure function of the source inputs, so the
    // lowering pools (`eta_*`, `cap_*`, …) mint the SAME names on a warm
    // (reused) database as on a cold one. Interner-membership minting would
    // skip the previous build's pool names and drift the emitted bytes — the
    // exact divergence the clean-vs-incremental parity gate guards against.
    let mut fresh_avoid: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for file in source_root.files(db).values() {
        fresh_avoid.extend(ipe_db::identifier_words(db, *file).iter().cloned());
    }

    // Set the fresh-name avoid-set (must happen before `lower_program` may
    // execute below). Short lock scope: the guard is dropped before any further
    // salsa query is demanded — the interner mutex is not reentrant, and
    // `typecheck`/`lower_program` each take their own lock internally.
    {
        let mut interner = shared_interner.lock();
        interner.set_fresh_avoid(fresh_avoid);
    }

    // The module-home → (file, src) blame map, and the span-attribution helper
    // built on it — the SAME resolution the `check`/analysis surfaces reuse (via
    // `attribute_post_link_error`), so every surface frames a given diagnostic
    // against the identical source.
    let home_to_source = home_to_source_map(&shared_interner, sources);
    let entry = (entry_src_path.clone(), entry_src.clone());
    let source_for_span = |span: ipe_diagnostics::Span| -> (PathBuf, String) {
        source_for_span_in_linked(linked, &home_to_source, &entry, span)
    };

    // Layer-2 wasm security gate (IPE-N0030, M5): the client entry's
    // reachability closure must not transitively reach a server-classified
    // module. Runs BEFORE Layer 1 so a reachability violation gets the
    // friendlier exact-chain message; Layer 1 remains the flat,
    // defense-in-depth backstop for everything this closure does not cover
    // (e.g. a server kernel named directly in the entry's own module).
    // `linked.name` is the client entry's module path for today's
    // single-entry `--target wasm` build (a distinct `[wasm].entry` module
    // takes the same role once the M6 integration wires a separate client
    // entry through).
    if config.target(db) == ipe_ir::Target::WasmClient {
        let gate_result = {
            let interner = shared_interner.lock();
            ipe_canon::module_classify::check_client_reachability(linked, &linked.name, &interner)
        };
        gate_result.map_err(|diag| {
            let span = diag_span(&diag);
            let (file, src) = source_for_span(span);
            CliError::Pipeline {
                file,
                src,
                diag: Box::new(diag),
            }
        })?;
    }

    // Layer-1 wasm security gate (IPE-N0029): under `--target wasm`, every
    // kernel named anywhere in the linked program must be on the WasmClient
    // allowlist. Runs on the LINKED module (everything linked is emitted, so
    // a denied kernel anywhere would otherwise become a cargo failure — THE
    // SEAL — or a secret consumer in a public bundle). Blame via the same
    // span→file heuristic the type errors use.
    if config.target(db) == ipe_ir::Target::WasmClient {
        let gate_result = {
            let interner = shared_interner.lock();
            ipe_canon::target_gate::check_wasm_client(linked, &interner)
        };
        gate_result.map_err(|diag| {
            let span = diag_span(&diag);
            let (file, src) = source_for_span(span);
            CliError::Pipeline {
                file,
                src,
                diag: Box::new(diag),
            }
        })?;
    }

    // Use the attributed variant so cross-module type errors are attributed to
    // the correct source file via the `home` carried on the failing constraint,
    // rather than relying solely on the byte-offset heuristic (`source_for_span`)
    // which can mis-attribute when two merged modules share overlapping numeric
    // span ranges.
    //
    // When `home` is non-empty we look it up in `home_to_source` directly —
    // O(log N) and exact.  When the home is empty (non-solver errors: constraint
    // generation, field-access pass, exhaustiveness) we fall back to the
    // byte-offset heuristic.
    //
    // `ipe_db::typecheck` is the memoized
    // SEAM over `ipe_types::infer_attributed`: same whole-program computation,
    // skippable on a warm no-op rebuild. No interner guard is held across
    // this demand — the query takes its own lock internally.
    let types = ipe_db::typecheck(db, source_root, entry_file).map_err(|(diag, home)| {
        attribute_post_link_error(linked, &home_to_source, &entry, diag, &home)
    })?;
    // Print non-fatal warnings (e.g. IPE-T0011 RedundantCaseBranch) to stderr.
    // These are Severity::Warning: the build continues and exit code stays 0.
    for w in &types.warnings {
        let span = diag_span(w);
        let (w_file, w_src) = source_for_span(span);
        eprintln!("{}", render(w, &w_file.to_string_lossy(), &w_src));
    }
    // Attribute lower / backend diagnostics to the source file that OWNS the
    // failing span, not blindly to the entry file. After link, every module's
    // defs keep their original `home` byte-namespace, so a bare `pipeline_err`
    // (which always blames the entry file) mis-renders a dep-module diagnostic
    // against the entry file at a coincidental byte offset — e.g. a State.ipe
    // IPE-L0115 shown at an unrelated Main.ipe line. `source_for_span` maps the
    // span back to its owning def's file, the same heuristic already used for
    // constraint-gen / exhaustiveness type errors.
    // Lowering (and emit) errors carry the owning def's `home`,
    // exactly like `typecheck` above. When `home` is non-empty we resolve the
    // source file DIRECTLY via `home_to_source` (O(log N), exact) — this is what
    // makes a Server.ipe IPE-L0126 render against Server.ipe, not against a
    // Main.ipe def whose byte range coincidentally overlaps the failing span.
    // An empty `home` (homeless backend diagnostic, or a pre-def lowering
    // error) falls back to the byte-offset heuristic `source_for_span`.
    let span_attributed_err =
        |(diag, home): (ipe_diagnostics::Diagnostic, Vec<ipe_intern::Symbol>)| {
            attribute_post_link_error(linked, &home_to_source, &entry, diag, &home)
        };

    // Decoder-pipeline direction gate (IPE-N0040): reject the hand-nested
    // `required`/`optional`/`requiredAt`/`custom` spelling that silently
    // reverses field→constructor binding. Runs on the linked module, framed
    // against the owning source like every other post-link diagnostic. The
    // IDENTICAL gate runs on the `ipe type-check` path (both call
    // `gate_decoder_pipelines`), so the footgun is caught on every pre-ship
    // surface, not just the build.
    gate_decoder_pipelines(linked).map_err(span_attributed_err)?;

    // `ipe_db::program_metadata` — the whole-program DCE-reachability seam
    // over `lower_program`.
    // Its own dependency on `lower_program` is what forces the lowering pass
    // to execute here; a standalone `lower_program` demand alongside this one
    // would be a redundant duplicate of the SAME memoized query (its error
    // maps through the same `span_attributed_err` closure either way).
    // Demanded here, on the production path, purely as a FORWARD SEAM:
    // nothing downstream consumes its value yet (no pruning pass exists —
    // see the query's own doc for the honestly-recorded scope), matching the
    // `kernel_types` precedent (materialized and proven memoized
    // before it has a real consumer). The demand costs nothing observable in
    // emitted bytes — the point is to put the query on the same path the
    // clean-vs-incremental parity gate drives, so a future divergence in this
    // analysis cannot go undetected.
    ipe_db::program_metadata(db, source_root, entry_file).map_err(span_attributed_err)?;

    // `ipe_db::emit_manifest` (design doc §4.4) — the top-level
    // emit demand, assembled from the per-`RustFileId` query graph:
    // `program_rust_file_ids` + `emit_spine_file` + one `emit_rust_file` per
    // home. For a single-module program it routes straight to `emit_project`
    // (byte-identical Spine-collapse); for a genuine 2+-home program it
    // assembles the split from those per-file memos, so a body edit to an
    // UNRELATED module early-cuts that module's `emit_rust_file` (byte-identical
    // value → salsa backdate → the on-disk write skips, §4.3). The
    // `EmitResult` SHAPE matches a plain `emit_project` demand, so
    // `build_emit_manifest`/`reconcile_emitted_project`/`prune_orphaned_files`
    // need zero changes (§4.4). The no-op-rebuild + `db_driver`-only
    // memoization properties `phase6_build_config.rs` proves hold — the
    // config field flows through unchanged.
    let emitted =
        ipe_db::emit_manifest(db, source_root, entry_file, config).map_err(span_attributed_err)?;
    Ok((*emitted).clone())
}

/// Write an emitted project to `out_dir`, vendoring the runtime module tree
/// from `runtime_dir`.
///
/// The emit→cargo bridge (design doc H7/H8):
/// assembles the COMPLETE intended project (`build_emit_manifest`) — the
/// vendored runtime tree, `Cargo.toml`, and every backend-emitted file — then
/// [`reconcile_emitted_project`] writes only what changed (content-gated,
/// atomic tmp-then-rename) and deletes anything under `out_dir/src` the
/// manifest no longer names (manifest-driven prune). On an unchanged rebuild
/// this writes NOTHING; `cargo` therefore sees no mtime churn and does not
/// invalidate its own build cache. This is a pure driver-boundary filesystem
/// operation — no salsa query touches disk (INV-1).
///
/// Under a static plan (see `docs/architecture/static-compilation.md`) the
/// intended project additionally gets the planned allocator feature spliced
/// into `Cargo.toml` and a generated `.cargo/config.toml` — and an
/// `Ipe.WebView` shape is refused BEFORE any file is written (a webview app
/// links the system webview; a "static" artifact would be a lie). A
/// non-static build removes a stale generated config so `+crt-static` can
/// never leak from an earlier static build into later ones.
///
/// # Errors
/// [`CliError::Io`] on any filesystem failure; [`CliError::StaticRefusal`]
/// for a webview shape under a static plan; [`CliError::Pipeline`] on a
/// backend-invariant breach (manifest anchor drift).
fn write_emitted_project(
    emitted: &ipe_backend::EmittedProject,
    out_dir: &Path,
    runtime_dir: &Path,
    static_plan: Option<&ipe_backend_rust::static_build::StaticPlan>,
    tree_shake_vendored: bool,
) -> Result<(), CliError> {
    use ipe_backend_rust::static_build;

    let mut manifest = build_emit_manifest(emitted, runtime_dir, tree_shake_vendored)?;
    if let Some(plan) = static_plan {
        if static_build::manifest_is_webview(&emitted.cargo_toml).map_err(backend_invariant_err)? {
            return Err(CliError::StaticRefusal(build_plan::Refusal::WebviewStatic));
        }
        let cargo_toml = static_build::staticize_manifest(&emitted.cargo_toml, plan.allocator())
            .map_err(backend_invariant_err)?;
        manifest.insert(PathBuf::from("Cargo.toml"), cargo_toml);
        manifest.insert(
            PathBuf::from(".cargo/config.toml"),
            static_build::cargo_config(plan),
        );
    }
    reconcile_emitted_project(&manifest, out_dir)?;
    if static_plan.is_none() {
        remove_stale_static_config(out_dir)?;
    }
    Ok(())
}

/// Map a backend-invariant [`Diagnostic`] (a `CompilerBug` from manifest
/// surgery — no owning source file) onto the pipeline error channel, blamed
/// on the emitted manifest.
fn backend_invariant_err(diag: Diagnostic) -> CliError {
    CliError::Pipeline {
        file: PathBuf::from("Cargo.toml"),
        src: String::new(),
        diag: Box::new(diag),
    }
}

/// Remove a stale GENERATED `.cargo/config.toml` from the project root — and
/// only a generated one: the file is deleted solely when it starts with
/// [`ipe_backend_rust::static_build::CARGO_CONFIG_MARKER`], so a config a
/// user placed there by hand is never touched. Needed because the
/// reconciler's prune pass is scoped to `out_dir/src` and cannot own
/// root-level files.
fn remove_stale_static_config(out_dir: &Path) -> Result<(), CliError> {
    let path = out_dir.join(".cargo").join("config.toml");
    match fs::read_to_string(&path) {
        Ok(text) if text.starts_with(ipe_backend_rust::static_build::CARGO_CONFIG_MARKER) => {
            match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(io_err(&path, e)),
            }
        }
        _ => Ok(()),
    }
}

/// Assemble the complete intended on-disk project, relative to `out_dir`:
/// every path this build produces, mapped to its exact text.
///
/// Every file this driver ever writes is UTF-8 Rust/TOML source, so `String`
/// (not raw bytes) is the honest content type — it lets this function reuse
/// the existing [`write_atomic`] helper unchanged (see
/// [`reconcile_emitted_project`]) instead of a parallel byte-oriented atomic
/// writer.
///
/// Three sources, in the same precedence `write_emitted_project` has always
/// used ("vendor first, emit second" — the backend's trimmed
/// `ipe_runtime/mod.rs` / `config.rs` must win over the fuller copies from
/// the source tree):
///   1. The vendored runtime module tree (`runtime_dir`, read recursively
///      under `src/ipe_runtime/`) — a driver-boundary filesystem read, the
///      same discipline as reading the entry file (never inside a
///      salsa-tracked query).
///   2. `Cargo.toml` at the project root.
///   3. Every backend-emitted file (`emitted.files`; each key is already a
///      validated [`ipe_backend::RelPath`] — relative and `..`-free — so no
///      entry here can escape `out_dir`).
///
/// # Errors
/// [`CliError::Io`] on any filesystem failure reading `runtime_dir` (including
/// a non-UTF-8 file, surfaced as an I/O error rather than a panic — the
/// runtime tree is trusted in-repo source, so this is not expected to fire in
/// practice).
fn build_emit_manifest(
    emitted: &ipe_backend::EmittedProject,
    runtime_dir: &Path,
    tree_shake_vendored: bool,
) -> Result<BTreeMap<PathBuf, String>, CliError> {
    let mut manifest = BTreeMap::new();
    // Vendoring is skipped for a dependency-model emit — it declares the runtime
    // as a path dependency and carries NO `src/ipe_runtime/` files, so there is
    // no vendored tree to overlay. The emit shape is self-describing: a vendored
    // emit always writes `src/ipe_runtime/mod.rs`; the dep-model emit never does.
    let emitted_mod_rs = emitted
        .files
        .iter()
        .find(|(rel, _)| rel.as_str() == "src/ipe_runtime/mod.rs")
        .map(|(_, contents)| contents.as_str());
    if let Some(mod_rs) = emitted_mod_rs {
        if tree_shake_vendored {
            // Eject shape: vendor only the runtime source the program reaches.
            // The emitted `mod.rs` declares `pub mod X;` for exactly the reached
            // top-level modules; a source file whose module is never declared is
            // one rustc would drop anyway, so omitting it from the tree is
            // behaviour-preserving and shrinks the shippable, auditable artifact.
            collect_reachable_runtime_text(
                runtime_dir,
                Path::new("src/ipe_runtime"),
                mod_rs,
                &mut manifest,
            )?;
        } else {
            collect_dir_text(runtime_dir, Path::new("src/ipe_runtime"), &mut manifest)?;
        }
    }
    manifest.insert(PathBuf::from("Cargo.toml"), emitted.cargo_toml.clone());
    for (rel, contents) in &emitted.files {
        manifest.insert(PathBuf::from(rel.as_str()), contents.clone());
    }
    Ok(manifest)
}

/// Vendor only the runtime source files the emitted `mod.rs` reaches.
///
/// The emitted `ipe_runtime/mod.rs` is a flat, non-`cfg`-gated list of `pub mod
/// X;` for exactly the top-level modules the program reaches. For each declared
/// name this copies either the single file `X.rs` or, when `X` is a directory
/// module, the ENTIRE `X/` subtree — never parsing the subtree's own nested
/// `mod` declarations. Copying a reached directory whole is the fail-closed
/// choice the eject contract requires: it can only ever include a file, never
/// omit one a nested `mod` needs, so the vendored tree always compiles. The
/// modules a directory does not reach are already excluded at the top level
/// (an unreached `web`/`db`/`tui`/… directory is never declared, so its whole
/// subtree is dropped) — where the large size wins come from.
///
/// `mod.rs` itself is always copied (it IS the emitted file, overlaid verbatim
/// by the caller's `emitted.files` pass afterwards); the loop only resolves the
/// modules it names.
///
/// # Errors
/// [`CliError::Io`] on any filesystem failure reading `runtime_dir`.
fn collect_reachable_runtime_text(
    runtime_dir: &Path,
    dst_prefix: &Path,
    emitted_mod_rs: &str,
    manifest: &mut BTreeMap<PathBuf, String>,
) -> Result<(), CliError> {
    for name in declared_modules(emitted_mod_rs) {
        let file = runtime_dir.join(format!("{name}.rs"));
        let dir = runtime_dir.join(&name);
        if dir.is_dir() {
            collect_dir_text(&dir, &dst_prefix.join(&name), manifest)?;
        } else if file.is_file() {
            let text = fs::read_to_string(&file).map_err(|e| io_err(&file, e))?;
            manifest.insert(dst_prefix.join(format!("{name}.rs")), text);
        }
        // A `pub mod X;` with neither `X.rs` nor `X/` on disk is an inline
        // module (a `pub mod web { pub mod route; }` block) — it has no separate
        // source file to vendor, so there is nothing to copy for it here.
    }
    Ok(())
}

/// The module names a runtime `mod.rs` declares with `pub mod X;` / `mod X;`.
///
/// A parse-free line scan sufficient for the emitted `mod.rs`, whose module
/// declarations are one-per-line `pub mod <name>;` with no attributes, braces,
/// or trailing content. A declaration that opens an inline module body (`pub
/// mod web {`) is deliberately excluded — it has no separate source file — by
/// requiring the statement to end in `;`.
fn declared_modules(mod_rs: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in mod_rs.lines() {
        let line = line.trim();
        let rest = line
            .strip_prefix("pub mod ")
            .or_else(|| line.strip_prefix("mod "));
        if let Some(rest) = rest
            && let Some(name) = rest.strip_suffix(';')
            && !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            names.insert(name.to_owned());
        }
    }
    names
}

/// Recursively read every file under `src_dir` as UTF-8 text, inserting
/// `(dst_prefix.join(rel), contents)` into `manifest`.
fn collect_dir_text(
    src_dir: &Path,
    dst_prefix: &Path,
    manifest: &mut BTreeMap<PathBuf, String>,
) -> Result<(), CliError> {
    let entries = fs::read_dir(src_dir).map_err(|e| io_err(src_dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_err(src_dir, e))?;
        let from = entry.path();
        let dst = dst_prefix.join(entry.file_name());
        let file_type = entry.file_type().map_err(|e| io_err(&from, e))?;
        if file_type.is_dir() {
            collect_dir_text(&from, &dst, manifest)?;
        } else {
            let text = fs::read_to_string(&from).map_err(|e| io_err(&from, e))?;
            manifest.insert(dst, text);
        }
    }
    Ok(())
}

/// Reconcile `out_dir` against `manifest`: write only files whose content
/// differs from what is already on disk (content-gated — H8, avoids spurious
/// `cargo` rebuilds from an identical-byte rewrite bumping mtime) via
/// [`write_atomic`]'s existing tmp-then-rename, then DELETE every file under
/// `out_dir/src` that is NOT a manifest key (manifest-driven prune — H7,
/// makes an orphaned/stale `.rs` left over from a deleted module or a
/// runtime-tree removal structurally impossible: `manifest` is authoritative).
///
/// Scope discipline: the prune walk is confined to `out_dir/src` and never
/// touches the project root — `Cargo.lock`, a `target/` build-cache
/// directory, or any other file `cargo` itself manages there must never be
/// touched by this pass.
///
/// # Errors
/// [`CliError::Io`] on any filesystem failure.
fn reconcile_emitted_project(
    manifest: &BTreeMap<PathBuf, String>,
    out_dir: &Path,
) -> Result<(), CliError> {
    for (rel, contents) in manifest {
        let path = out_dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        }
        write_if_changed(&path, contents)?;
    }
    prune_orphaned_files(&out_dir.join("src"), manifest, out_dir)
}

/// Write `contents` to `path` only when the existing content differs (or the
/// file is absent) — the content-gate `write_atomic` alone does not provide
/// (it always writes). Delegating the actual write to [`write_atomic`] reuses
/// its established tmp-then-rename + cleanup-on-failure behaviour rather than
/// a second, parallel atomic-write implementation.
fn write_if_changed(path: &Path, contents: &str) -> Result<(), CliError> {
    if fs::read_to_string(path).is_ok_and(|existing| existing == contents) {
        return Ok(());
    }
    write_atomic(path, contents)
}

/// Delete every FILE under `dir` whose path relative to `out_dir` is not a
/// key of `manifest`. Recurses into subdirectories but never removes a
/// directory itself (leaving empty directories behind is harmless — `cargo`
/// does not care — and staying file-only keeps this pass's blast radius
/// minimal).
fn prune_orphaned_files(
    dir: &Path,
    manifest: &BTreeMap<PathBuf, String>,
    out_dir: &Path,
) -> Result<(), CliError> {
    if !dir.is_dir() {
        return Ok(());
    }
    // A directory that vanishes between the `is_dir()` check above and this
    // read (a concurrent external cleanup — see `write_atomic`'s doc for the
    // shared-scratch-directory scenario this guards) trivially has nothing
    // left to prune; treat `NotFound` as success rather than failing the
    // whole build over a race that already resolved itself.
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_err(dir, e)),
    };
    for entry in entries {
        // Likewise: a file this same walk is iterating over can disappear
        // mid-loop (a sibling process finished its OWN rebuild and deleted
        // its temp state). Skip rather than fail — there is nothing left to
        // prune at a path that no longer exists.
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io_err(dir, e)),
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io_err(&path, e)),
        };
        if file_type.is_dir() {
            prune_orphaned_files(&path, manifest, out_dir)?;
        } else {
            // `path` was built from `dir`, itself built from `out_dir` by
            // construction (the initial call passes `out_dir.join("src")`,
            // and every recursive call passes a child of that) — the
            // `strip_prefix` can only fail if `out_dir` itself is relative
            // and the working directory changed mid-walk; skip rather than
            // fail the whole build over a diagnostic-only path label.
            let Ok(rel) = path.strip_prefix(out_dir) else {
                continue;
            };
            if !manifest.contains_key(rel)
                && let Err(e) = fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                // A concurrent deleter reaching `path` first (see above) is
                // NOT a failure to prune it — the goal ("this orphan is gone")
                // is already satisfied.
                return Err(io_err(&path, e));
            }
        }
    }
    Ok(())
}

/// Build a multi-module Ipe project rooted at `manifest_path` (`ipe.toml`) into
/// a Rust Cargo project under `out_dir`, vendoring the runtime from `runtime_dir`.
///
/// The build pipeline:
/// 1. Parse `ipe.toml` to locate the source root.
/// 2. Discover every `*.ipe` file under `src/`.
/// 3. Scan each file for `import` declarations (token-level lexer scan) to
///    build the import graph.
/// 4. Topological sort — fail closed on a cycle (IPE-N0021).
/// 5. Canonicalise each module in dep-first order (IPE-N0020 / N0022 / N0023 /
///    N0024 / N0025 gate).
/// 6. Link (merge) all canonical modules into one.
/// 7. Infer → lower → emit as a single-module program (byte-identical to the
///    single-file pipeline on the entry module).
///
/// # Errors
/// [`CliError::Io`] on any filesystem failure.
/// [`CliError::Pipeline`] carrying the first compiler diagnostic.
pub fn build_project(
    manifest_path: &Path,
    out_dir: &Path,
    runtime_dir: &Path,
) -> Result<(), CliError> {
    build_project_with_options(
        manifest_path,
        out_dir,
        runtime_dir,
        BuildOptions::from_env(),
    )
}

/// [`build_project`] with explicit [`BuildOptions`] (the static-plan-aware
/// variant).
///
/// # Errors
/// As [`build_project`], plus [`CliError::StaticRefusal`] when the emitted
/// app shape cannot be static.
// `options` is reconstructed (struct-update syntax) with the parsed
// manifest's `[wasm] publicEnv` allowlist before threading onward — a
// genuine consuming use clippy's by-value heuristic doesn't credit; taking
// `&BuildOptions` here would ripple a lifetime through every call site for
// no benefit (every caller already owns a fresh `BuildOptions`).
#[allow(clippy::needless_pass_by_value)]
pub fn build_project_with_options(
    manifest_path: &Path,
    out_dir: &Path,
    runtime_dir: &Path,
    options: BuildOptions,
) -> Result<(), CliError> {
    let manifest = project::parse_manifest(manifest_path)?;
    let discovered = project::discover_modules(&manifest.src_root)?;

    // For each module, read its source and extract imports.
    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    for m in &discovered {
        let src = fs::read_to_string(&m.path).map_err(|e| CliError::Io {
            path: m.path.clone(),
            source: e,
        })?;
        sources.insert(m.module_path.clone(), (m.path.clone(), src));
    }

    let entry_path = vec!["Main".to_owned()];

    // Fold in the `[wasm] publicEnv` allowlist this manifest declares — the
    // caller's `options` carries no manifest-derived data (it is built before
    // the manifest is parsed), so it is completed here, the same way
    // `manifest.driver` bypasses `options` entirely as its own positional arg.
    let options = BuildOptions {
        wasm_public_env: manifest.wasm.public_env.clone(),
        wasm_hydrate_mode: manifest.wasm.mode.as_deref() == Some("hydrate"),
        ..options
    };

    // The manifest is the blame location for an import cycle (no single file
    // owns it); post-link errors are blamed on the entry file inside the core.
    compile_modules(
        sources,
        discovered,
        &entry_path,
        out_dir,
        runtime_dir,
        manifest_path,
        manifest.driver,
        options,
    )
}

/// Locate the Ipê runtime module tree (`src/runtime/rust/src/`).
///
/// Resolution order:
/// 1. `$IPE_RUNTIME_DIR` — explicit override, allows pointing at any tree.
/// 2. Upward walk from the current directory, checking in order:
///    - `src/runtime/rust/src/ipe_runtime` (the in-repo copy — found immediately when
///      running from anywhere inside the ipe-lang workspace)
///    - `ipe/runtime-rust/src/ipe_runtime` (sibling ipe checkout — legacy)
///    - `runtime-rust/src/ipe_runtime` (legacy sibling path)
///
/// # Errors
/// Returns [`CliError::RuntimeNotFound`] when no candidate directory exists, or
/// [`CliError::Io`] if the current directory cannot be read.
pub fn resolve_runtime() -> Result<PathBuf, CliError> {
    if let Ok(dir) = std::env::var("IPE_RUNTIME_DIR") {
        let path = PathBuf::from(dir);
        if path.is_dir() {
            return Ok(path);
        }
    }

    let cwd = std::env::current_dir().map_err(|e| io_err(Path::new("."), e))?;
    let mut here: Option<&Path> = Some(cwd.as_path());
    while let Some(dir) = here {
        for candidate in [
            // In-repo runtime (ipe-lang monorepo): found when CWD is anywhere
            // inside the workspace.
            dir.join("src").join("runtime").join("rust").join("src"),
            // Legacy: sibling `ipe` checkout.
            dir.join("ipe")
                .join("runtime-rust")
                .join("src")
                .join("ipe_runtime"),
            // Legacy: sibling `runtime-rust` directory.
            dir.join("runtime-rust").join("src").join("ipe_runtime"),
        ] {
            if candidate.is_dir() {
                return Ok(candidate);
            }
        }
        here = dir.parent();
    }
    Err(CliError::RuntimeNotFound)
}

/// Resolve the vendored runtime MODULE tree the emit copies into the project,
/// but only when that emit shape actually needs it.
///
/// The dependency-model native emit (the default) carries no vendored
/// `src/ipe_runtime/` tree — it names the runtime as a path dependency, resolved
/// separately by [`runtime_embed::resolve`] — so requiring a vendored tree up
/// front would fail a perfectly valid build run outside a repo checkout. The
/// vendored tree is needed only when the vendored shape is emitted: the wasm
/// target (which still vendors) or a build with the dependency model turned off.
///
/// `cli_override` is the explicit `--runtime <dir>` value, honoured verbatim when
/// present. When the vendored tree is not needed, an empty sentinel path is
/// returned; the dep-model native emit never reads it.
///
/// # Errors
/// [`CliError::RuntimeNotFound`] / [`CliError::Io`] from [`resolve_runtime`] when
/// a vendored tree is required but cannot be located.
fn resolve_vendored_runtime_dir(
    cli_override: Option<String>,
    needs_vendored: bool,
) -> Result<PathBuf, CliError> {
    match cli_override {
        Some(r) => Ok(PathBuf::from(r)),
        None if needs_vendored => resolve_runtime(),
        None => Ok(PathBuf::new()),
    }
}

/// The misuse reason shown when `build` / `run` / `watch` are invoked with no
/// entry and none can be discovered. Just the reason — the command's own
/// `--help` page (appended by [`CliError::CommandUsage`]) carries the synopsis
/// and options, so this never re-lists them.
const NO_ENTRY: &str = "nothing to build here — pass a source file or run inside a project (an ipe.toml, \
     or a src/Main.ipe)";

/// A request for help asks for output, not an error: it prints to stdout and
/// exits successfully. Returned by [`intercept_help`] so [`run_cli`] can honour
/// it before any command runs.
struct HelpRequest;

/// Recognise a help request in `args` and, when found, print the matching page
/// to stdout. Handles the top-level screen (no args, or a leading `--help` /
/// `-h` / `help`) and every per-command page (`<cmd> --help` or `help <cmd>`).
///
/// Returns `Some(HelpRequest)` when help was printed (the caller returns `Ok`),
/// or `None` when `args` is an ordinary command to dispatch.
fn intercept_help(args: &[String]) -> Option<HelpRequest> {
    let is_help_flag = |a: &str| a == "--help" || a == "-h" || a == "help";

    // No arguments, or a leading bare help token: the top-level screen.
    match args.split_first() {
        None => {
            print!("{}", help::top_level(&std::io::stdout()));
            return Some(HelpRequest);
        }
        Some((first, rest)) if is_help_flag(first) => {
            // `help <cmd>` / `--help <cmd>`: that command's page, else the
            // top-level screen.
            let named = rest
                .first()
                .and_then(|c| help::command(c, &std::io::stdout()));
            match named {
                Some(page) => print!("{page}"),
                None => print!("{}", help::top_level(&std::io::stdout())),
            }
            return Some(HelpRequest);
        }
        _ => {}
    }

    // `<cmd> --help`: the command's own page, when the command is known.
    if let Some((cmd, rest)) = args.split_first()
        && help::is_command(cmd)
        && rest.iter().any(|a| is_help_flag(a))
        && let Some(page) = help::command(cmd, &std::io::stdout())
    {
        print!("{page}");
        return Some(HelpRequest);
    }
    None
}

/// Parse `argv` (excluding the program name) and run the requested command.
///
/// # Errors
/// Returns [`CliError`] on misuse, a compile failure, or a filesystem error.
pub fn run_cli(args: &[String]) -> Result<(), CliError> {
    if intercept_help(args).is_some() {
        return Ok(());
    }
    match args.split_first() {
        Some((cmd, rest)) if cmd == "init" => with_help_on_misuse("init", init::run_init(rest)),
        Some((cmd, rest)) if cmd == "build" => with_help_on_misuse("build", run_build(rest)),
        Some((cmd, rest)) if cmd == "eject" => with_help_on_misuse("eject", run_eject(rest)),
        Some((cmd, rest)) if cmd == "type-check" => {
            with_help_on_misuse("type-check", run_type_check(rest))
        }
        Some((cmd, rest)) if cmd == "test" => with_help_on_misuse("test", run_test(rest)),
        Some((cmd, rest)) if cmd == "verify" => with_help_on_misuse("verify", run_verify(rest)),
        Some((cmd, rest)) if cmd == "run" => with_help_on_misuse("run", run_run(rest)),
        Some((cmd, rest)) if cmd == "exec" => with_help_on_misuse("exec", run_exec(rest)),
        Some((cmd, rest)) if cmd == "watch" => with_help_on_misuse("watch", run_watch(rest)),
        Some((cmd, rest)) if cmd == "explain" => with_help_on_misuse("explain", run_explain(rest)),
        Some((cmd, rest)) if cmd == "capabilities" => {
            with_help_on_misuse("capabilities", run_capabilities(rest))
        }
        Some((cmd, rest)) if cmd == "diff" => with_help_on_misuse("diff", diff::run_diff(rest)),
        Some((cmd, rest)) if cmd == "doc" => with_help_on_misuse("doc", doc::run_doc(rest)),
        Some((cmd, rest)) if cmd == "rust" => with_help_on_misuse("rust", ffi::run_rust(rest)),
        Some((cmd, rest)) if cmd == "add" => with_help_on_misuse("add", pkg::run_add(rest)),
        Some((cmd, rest)) if cmd == "remove" => {
            with_help_on_misuse("remove", pkg::run_remove(rest))
        }
        Some((cmd, rest)) if cmd == "package" => with_help_on_misuse("package", run_package(rest)),
        Some((cmd, rest)) if cmd == "login" => with_help_on_misuse("login", login::run_login(rest)),
        Some((cmd, rest)) if cmd == "fix" => with_help_on_misuse("fix", run_fix(rest)),
        Some((cmd, rest)) if cmd == "fmt" => with_help_on_misuse("fmt", fmt::run_fmt(rest)),
        Some((cmd, rest)) if cmd == "clean" => with_help_on_misuse("clean", clean::run_clean(rest)),
        Some((cmd, rest)) if cmd == "lsp" => with_help_on_misuse("lsp", lsp::run_lsp(rest)),
        Some((cmd, rest)) if cmd == "upgrade" => with_help_on_misuse("upgrade", run_upgrade(rest)),
        Some((cmd, rest)) if cmd == "health" => {
            with_help_on_misuse("health", health::run_health(rest))
        }
        Some((cmd, rest)) if cmd == "version" || cmd == "--version" || cmd == "-V" => {
            with_help_on_misuse("version", run_version(rest))
        }
        // An unknown command is misuse: show the top-level help and fail. Unlike
        // an explicit `--help`, this is not a request, so it exits non-zero. The
        // typed token is kept so a near-miss can be suggested; a bare `ipe`
        // (no command) carries an empty token and just shows help.
        Some((cmd, _)) => Err(CliError::UnknownCommand {
            attempted: cmd.clone(),
        }),
        None => Err(CliError::UnknownCommand {
            attempted: String::new(),
        }),
    }
}

/// Map a known command's raw usage error into a [`CliError::CommandUsage`] so the
/// caller prints that command's full, indented `--help` page — the uniform
/// "misuse shows help" output. Any non-usage error (a compile failure, a
/// filesystem error) passes through untouched, since it is not a help-worthy
/// misuse. `command` is always a known command name.
fn with_help_on_misuse(
    command: &'static str,
    result: Result<(), CliError>,
) -> Result<(), CliError> {
    match result {
        Err(CliError::Usage(reason)) => Err(CliError::CommandUsage {
            command,
            reason: reason.to_owned(),
        }),
        Err(CliError::UsageOwned(reason)) => Err(CliError::CommandUsage { command, reason }),
        other => other,
    }
}

/// Project-aware default entry when no positional argument is given to
/// `build`, `run`, or `watch`.
///
/// Resolution order:
/// 1. `./ipe.toml` exists — entry `"."` (project mode; `discover_manifest`
///    routes it to the directory's manifest).
/// 2. `./src/Main.ipe` exists — entry `"src/Main.ipe"` (single-file
///    shorthand without a manifest).
/// 3. Neither — usage error: nothing to build here.
fn default_entry() -> Result<String, CliError> {
    if std::path::Path::new("ipe.toml").exists() {
        return Ok(".".to_owned());
    }
    if std::path::Path::new("src/Main.ipe").exists() {
        return Ok("src/Main.ipe".to_owned());
    }
    Err(CliError::Usage(NO_ENTRY))
}

/// `ipe watch [<path>]` — rebuild and re-run on every source change
/// (`crate::watch`). Never returns
/// `Err` for a build failure (INV-3: a red build is logged, not fatal);
/// only misuse / setup failures propagate.
fn run_watch(rest: &[String]) -> Result<(), CliError> {
    let args = cli_args::parse_watch(rest)?;
    let entry = match args.entry {
        Some(e) => e,
        None => default_entry()?,
    };

    let out_dir = args
        .out
        .map_or_else(|| PathBuf::from("out").join("rust"), PathBuf::from);
    let runtime_dir = match args.runtime {
        Some(r) => PathBuf::from(r),
        None => resolve_runtime()?,
    };

    // Fail closed before the watch loop starts: `ipe watch` rebuilds with cargo
    // on every change, so a missing toolchain is reported once, up front, with
    // its root cause — not as a per-rebuild opaque spawn error.
    let cargo_bin = toolchain::require_cargo(toolchain::ToolIntent::Watch)?;

    let mut opts = watch::WatchOptions::new(PathBuf::from(entry), out_dir, runtime_dir);
    opts.port = args.port;
    opts.cargo_path = cargo_bin.path().to_path_buf();
    watch::run(&opts)
}

/// Route an entry argument to its `ipe.toml`, when one governs it:
/// a directory must contain one, a `.toml` argument IS one, and a `.ipe`
/// entry walks up the tree looking for one (falling back to sibling
/// discovery when none exists).
fn discover_manifest(entry_path: &Path) -> Result<Option<PathBuf>, CliError> {
    if entry_path.is_dir() {
        let candidate = entry_path.join("ipe.toml");
        if candidate.is_file() {
            Ok(Some(candidate))
        } else {
            Err(CliError::Usage(
                "directory supplied but no ipe.toml found inside it",
            ))
        }
    } else if entry_path.extension().and_then(|e| e.to_str()) == Some("toml") {
        Ok(Some(entry_path.to_path_buf()))
    } else {
        Ok(find_manifest_for_ipe_file(entry_path))
    }
}

/// Resolve the static request with full precedence — CLI flags > env
/// (`IPE_STATIC` / `IPE_TARGET` / `IPE_ALLOC`) > `ipe.toml` `[rust]` > AUTO —
/// into a typed plan (or a typed refusal — no artifact), run the toolchain
/// preflight, and surface the mimalloc opt-in notice. Shared by `build` and
/// `run`; resolved ONCE before any compilation starts.
///
/// `IPE_TARGET=wasm` is a wasm-target axis signal (resolved by
/// [`resolve_wasm_target`]) and is NOT a static-link triple; it is stripped
/// here so it never reaches the musl-triple gate in [`build_plan::resolve`].
fn resolve_static_plan(
    cli_layer: build_plan::StaticRequestLayer,
    manifest: Option<&Path>,
) -> Result<Option<ipe_backend_rust::static_build::StaticPlan>, CliError> {
    let toml_layer = match manifest {
        Some(m) => project::parse_manifest(m)?.static_request,
        None => build_plan::StaticRequestLayer::default(),
    };
    let mut env = build_plan::env_layer()?;
    if env.target.as_deref() == Some("wasm") {
        env.target = None;
    }
    let merged = cli_layer.or(env).or(toml_layer);
    let static_plan = build_plan::resolve(&merged)?;
    if let Some(plan) = &static_plan {
        build_plan::preflight(plan)?;
        if plan.allocator() == ipe_backend_rust::static_build::StaticAllocator::Mimalloc {
            // The design's explicit opt-in notice: the C cost is acknowledged,
            // never silent.
            eprintln!(
                "{}",
                style::gutter(
                    "note: mimalloc adds a C toolchain and unsafe FFI, vendors C source, and \
                     freezes it into the artifact for CVE-rebuild purposes; chosen explicitly."
                )
            );
        }
    }
    Ok(static_plan)
}

/// Resolve the wasm-vs-native target with the three-tier precedence chain:
/// CLI flag (`--target wasm`) > `IPE_TARGET=wasm` env > `[wasm].mode` in
/// `ipe.toml` > default native.
///
/// `cli_wasm` carries the parsed `--target wasm` flag from `BuildMode::Emit`.
/// `wasm_config` is `None` when there is no manifest (sibling-discovery build).
///
/// Returns `true` when the resolved target is `WasmClient`.
fn resolve_wasm_target(cli_wasm: bool, wasm_config: Option<&project::WasmConfig>) -> bool {
    cli_wasm
        || std::env::var("IPE_TARGET").ok().as_deref() == Some("wasm")
        || wasm_config.is_some_and(project::WasmConfig::implies_wasm_target)
}

/// `ipe build [<path>]` — compile a program to a native or WebAssembly artifact.
fn run_build(rest: &[String]) -> Result<(), CliError> {
    let args = cli_args::parse_build(rest)?;
    let entry = match args.entry {
        Some(e) => e,
        None => default_entry()?,
    };
    let entry_path = PathBuf::from(&entry);

    // `--fix` carries durable authorization: apply machine-applicable fixes
    // non-interactively before the (re-run) build sees the source.
    if args.fix {
        apply_fixes_cmd(&entry_path, true, &mut std::io::stdout())?;
    }

    // Parse guaranteed `--emit-ir` composes with no emit-affecting flag, so the
    // IR-dump path carries no options to drop.
    let (out, wasm_target, cli_layer) = match args.mode {
        cli_args::BuildMode::EmitIr => {
            // `--emit-ir` reads a single entry file, so route a directory / bare
            // `.` project root to its entry `.ipe` — the same convention the
            // analysis surfaces use — rather than handing the directory straight
            // to the source reader (which would fail with a raw "Is a directory").
            let ir_entry = resolve_analysis_entry(&entry_path)?;
            let tree = emit_ir_text(&ir_entry)?;
            print!("{tree}");
            return Ok(());
        }
        cli_args::BuildMode::Emit {
            out,
            wasm,
            static_layer,
        } => (out, wasm, static_layer),
    };

    let out_dir = out.map_or_else(|| PathBuf::from("out").join("rust"), PathBuf::from);

    // Route the build:
    //   1. Directory → expect ipe.toml inside it.
    //   2. .toml file → build_project directly.
    //   3. .ipe file → walk up looking for ipe.toml (project-mode); fall back
    //      to sibling discovery when no ipe.toml exists (fixes IPE-N0020 for
    //      multi-file projects built via the file-path shorthand). This mirrors
    //      the Haskell driver's `Graph.discoverModulesMulti srcRoot entryPath`
    //      call in `Ipe.Build.Compile.hs`.
    let manifest = discover_manifest(&entry_path)?;

    // Parse the manifest early to read [wasm].mode for target inference.
    // build_project_with_options re-parses it later to fill in publicEnv /
    // hydrate-mode; the double parse is acceptable (manifests are small).
    let manifest_wasm: Option<project::WasmConfig> = manifest
        .as_deref()
        .map(project::parse_manifest)
        .transpose()?
        .map(|m| m.wasm);

    // Precedence: CLI --target wasm > IPE_TARGET=wasm > [wasm].mode != "off".
    let wasm_target = resolve_wasm_target(wasm_target, manifest_wasm.as_ref());

    // The dependency model (native OR wasm) needs no vendored tree — the runtime
    // is a path dependency. Only a dep-model-OFF build vendors the source subtree.
    let runtime_dep = runtime_dep_from_env();
    let runtime_dir = resolve_vendored_runtime_dir(args.runtime, !runtime_dep)?;

    let static_plan = resolve_static_plan(cli_layer, manifest.as_deref())?;

    // Fail closed before emitting: `ipe build` compiles the emitted project so a
    // reported success means the crate actually built. A missing toolchain is a
    // clear root-cause error now, not an opaque OS spawn error after the
    // (wasted) emit. The wasm branch delegates to `bundle_wasm`, which resolves
    // cargo itself, so only the native branch resolves here — the resolved path
    // is reused for its build.
    let native_cargo = if wasm_target {
        None
    } else {
        Some(toolchain::require_cargo(toolchain::ToolIntent::Build)?)
    };

    let options = BuildOptions {
        static_plan,
        target: if wasm_target {
            ipe_ir::Target::WasmClient
        } else {
            ipe_ir::Target::Native
        },
        wasm_public_env: Vec::new(),
        wasm_hydrate_mode: false,
        production: args.production,
        runtime_dep,
        // `ipe build` never tree-shakes the vendored tree — a dep-model build
        // carries no vendored source, and a vendored (`IPE_RUNTIME_VENDORED`)
        // build keeps the full tree so rustc, not the driver, drops the unreached
        // files. Only `ipe eject` sets this.
        tree_shake_vendored: false,
    };

    // Human-friendly progress: the compile+emit below is otherwise silent, so
    // bracket it with a start/done line. Shown only on an interactive terminal so
    // piped / CI output stays clean; status goes to stderr (stdout carries data).
    let show_progress = {
        use std::io::IsTerminal as _;
        std::io::stderr().is_terminal()
    };
    if show_progress {
        eprintln!(
            "{}",
            style::gutter(&format!("{} building {entry}", style::glyph::STEP))
        );
    }

    // No ipe.toml found: compile entry + all sibling .ipe files in the same
    // directory. Byte-identical to `build` when the directory holds only the
    // entry file (regression-covered by the golden suite).
    manifest.as_ref().map_or_else(
        || {
            build_with_sibling_discovery_with_options(
                &entry_path,
                &out_dir,
                &runtime_dir,
                options.clone(),
            )
        },
        |m| build_project_with_options(m, &out_dir, &runtime_dir, options.clone()),
    )?;

    if wasm_target {
        bundle_wasm(&out_dir)?;
    } else {
        compile_and_finalize_native_build(
            &out_dir,
            native_cargo,
            static_plan,
            runtime_dep,
            manifest.as_deref(),
            &entry_path,
        )?;
    }

    if show_progress {
        eprintln!(
            "{}",
            style::gutter(&format!(
                "{} built → {}",
                style::glyph::OK,
                out_dir.display()
            ))
        );
    }
    Ok(())
}

/// Compile the just-emitted native crate and write its runtime-enforcement
/// artifacts. Split out of [`run_build`] so each stays a readable unit.
///
/// The compile is the SEAL: a reported build success MUST mean the crate
/// actually built, so a non-zero cargo exit surfaces as a typed
/// [`CliError::EmittedBuildFailed`] rather than a silent exit-0 that would mask a
/// miscompile. It also produces the `target/debug/ipe-app` binary that
/// `ipe exec` later runs. CWD = the emitted crate dir so the generated
/// `.cargo/config.toml` is discovered; a static plan additionally selects the
/// target triple explicitly.
///
/// A native-bearing artifact then carries its own runtime enforcement — an
/// `ipe.profile` mirror plus the authoritative capability floor embedded in the
/// binary — so the jail travels with a copied-off-host artifact (ADR 0040). A
/// pure Ipê artifact is structurally bounded and needs neither profile nor floor.
///
/// # Errors
/// - [`CliError::EmittedBuildFailed`] when the emitted crate fails to compile.
/// - The toolchain, manifest-parse, and capability-resolution errors of the
///   steps it composes.
fn compile_and_finalize_native_build(
    out_dir: &Path,
    native_cargo: Option<toolchain::CargoBin>,
    static_plan: Option<ipe_backend_rust::static_build::StaticPlan>,
    runtime_dep: bool,
    manifest: Option<&Path>,
    entry_path: &Path,
) -> Result<(), CliError> {
    // `native_cargo` is `Some` on every native path (the caller's wasm branch
    // returns before here); the fallback re-resolves rather than unwrapping so
    // the toolchain error stays typed even if that invariant ever changes.
    let cargo_bin = match native_cargo {
        Some(bin) => bin,
        None => toolchain::require_cargo(toolchain::ToolIntent::Build)?,
    };
    let mut cargo = std::process::Command::new(cargo_bin.path());
    cargo.arg("build").current_dir(out_dir);
    if let Some(plan) = &static_plan {
        cargo.args(["--target", plan.triple.as_str()]);
    }
    let runtime_ctx = if runtime_dep {
        runtime_context_for_message()
    } else {
        None
    };
    build_emitted_project(&mut cargo, "the emitted program", runtime_ctx, out_dir)?;

    let manifest_parsed = match manifest {
        Some(m) => Some(project::parse_manifest(m)?),
        None => None,
    };
    let driver = manifest_parsed
        .as_ref()
        .map_or(ipe_backend_rust::DbDriver::Sqlite, |m| m.driver);
    let resolved = run_sandbox::resolve_for_run(manifest_parsed.as_ref(), manifest, entry_path)?;
    if run_sandbox::is_native_bearing(&resolved.union()) {
        let profile = run_sandbox::build_profile(&resolved, driver)?;
        run_sandbox::write_build_artifacts(out_dir, &profile)?;
    }
    Ok(())
}

/// `ipe eject [<path>] --out <dir>` — emit a self-contained Rust Cargo project a
/// user can `cargo build` with no `ipe` toolchain installed.
///
/// The escape hatch from the dependency-crate model: where `ipe build` emits a
/// project that names the runtime as a path dependency (resolved by the
/// toolchain), eject VENDORS the runtime source into the output — and tree-shakes
/// it to only the modules the program reaches. The emitted `ipe_runtime/mod.rs`
/// already declares `pub mod X;` for exactly the reached top-level modules, so
/// [`build_emit_manifest`] copies only those source files. The result is a
/// small, auditable, offline-buildable crate: pure, reviewable Rust with no
/// external runtime path and no registry fetch.
///
/// Eject is native-only and FFI-free by contract:
///   - A foreign-crate FFI binding would need external crates pulled from a
///     registry, which the source-only, self-contained contract forbids — so an
///     FFI-bearing program is a hard [`CliError::EjectUnsupported`] refusal
///     rather than a tree that would not resolve offline.
///   - `--target wasm` is a distinct compilation axis with its own bundling
///     step; eject targets a plain-`cargo build` native crate.
///
/// # Errors
/// [`CliError::EjectUnsupported`] for an FFI-bearing program; the same
/// pipeline / filesystem / runtime-resolution errors as [`build_project`].
fn run_eject(rest: &[String]) -> Result<(), CliError> {
    let args = cli_args::parse_eject(rest)?;
    let entry = match args.entry {
        Some(e) => e,
        None => default_entry()?,
    };
    let entry_path = PathBuf::from(&entry);
    let out_dir = PathBuf::from(&args.out);

    // Fail closed on an FFI-bearing project BEFORE any emit: eject vendors only
    // the embedded runtime SOURCE, so a program binding a foreign Rust crate
    // cannot be made self-contained (its external crates would be a registry
    // fetch at the ejected project's `cargo build`). Detecting it from the
    // installed FFI catalog is the same trusted signal the build pipeline reads;
    // a non-empty catalog means at least one `Rust.` binding is in scope.
    if !ffi::load_catalog_for(&entry_path)?.is_empty() {
        return Err(CliError::EjectUnsupported {
            reason: "this program binds a foreign Rust crate (FFI). Eject vendors only the \
                     embedded runtime source, so it cannot produce a self-contained project for \
                     a program that pulls external crates — build it with `ipe build` instead"
                .to_owned(),
        });
    }

    let manifest = discover_manifest(&entry_path)?;

    // Eject targets a plain native `cargo build`; the wasm target has its own
    // bundling step and a distinct closed vendoring template. Refuse a wasm
    // request from ANY tier — the `IPE_TARGET=wasm` env OR a project's
    // `[wasm].mode` — rather than silently emit a native tree for a browser app.
    // (`parse_eject` has no `--target` flag, so the CLI tier cannot select wasm
    // here; `false` for the CLI axis is exact.)
    let manifest_wasm: Option<project::WasmConfig> = manifest
        .as_deref()
        .map(project::parse_manifest)
        .transpose()?
        .map(|m| m.wasm);
    if resolve_wasm_target(false, manifest_wasm.as_ref()) {
        return Err(CliError::EjectUnsupported {
            reason: "eject produces a native Cargo project; the wasm target has a separate \
                     bundling step — use `ipe build --target wasm`"
                .to_owned(),
        });
    }

    let runtime_dir = resolve_vendored_runtime_dir(args.runtime, true)?;

    // Force the vendored, tree-shaken emit shape: a self-contained project names
    // no runtime path dependency (`runtime_dep = false`) and carries only the
    // reached runtime source (`tree_shake_vendored = true`). Static/wasm options
    // stay at their defaults — eject is the plain native standalone shape.
    let options = BuildOptions {
        runtime_dep: false,
        tree_shake_vendored: true,
        ..BuildOptions::default()
    };

    let show_progress = {
        use std::io::IsTerminal as _;
        std::io::stderr().is_terminal()
    };
    if show_progress {
        eprintln!(
            "{}",
            style::gutter(&format!("{} ejecting {entry}", style::glyph::STEP))
        );
    }

    manifest.as_ref().map_or_else(
        || {
            build_with_sibling_discovery_with_options(
                &entry_path,
                &out_dir,
                &runtime_dir,
                options.clone(),
            )
        },
        |m| build_project_with_options(m, &out_dir, &runtime_dir, options.clone()),
    )?;

    if show_progress {
        eprintln!(
            "{}",
            style::gutter(&format!(
                "{} ejected → {} (self-contained; `cd {} && cargo build`)",
                style::glyph::OK,
                out_dir.display(),
                out_dir.display()
            ))
        );
    }
    Ok(())
}

/// Run a `cargo build` of an emitted project to completion, *streaming* its
/// stderr to this process's stderr line by line as `cargo` emits it — so the
/// user sees the live compile progress (which crate is building, warnings)
/// rather than a silent wait that only reveals itself once `cargo` has already
/// finished. The same lines are accumulated so that on a non-zero exit the
/// captured text is returned inside a typed [`CliError::EmittedBuildFailed`]:
/// the failure renders as a clean `ipe`-level diagnostic — a targeted
/// runtime-feature line when `cargo` reports a missing feature, otherwise the
/// trimmed `cargo` error under a plain header — and never the command's `--help`
/// page. `what` names what was built; `runtime` is the crate the project linked
/// against, when the caller resolved one.
///
/// `cargo`'s stdout is inherited untouched (a `cargo build` writes only status
/// to stderr; nothing on stdout needs capture), so any tool output stays on
/// stdout while progress stays on stderr.
///
/// # Errors
/// - [`CliError::Io`] if `cargo` cannot be spawned or its stderr pipe cannot be
///   opened.
/// - [`CliError::EmittedBuildFailed`] if `cargo` exits non-zero.
fn build_emitted_project(
    cargo: &mut std::process::Command,
    what: &'static str,
    runtime: Option<RuntimeContext>,
    io_path: &Path,
) -> Result<(), CliError> {
    use std::io::BufReader;
    use std::process::Stdio;

    let io_err = |e: std::io::Error| CliError::Io {
        path: io_path.to_path_buf(),
        source: e,
    };

    // Pipe stderr so we can both forward it live AND capture it for the typed
    // error; leave stdout inherited (a `cargo build` writes only to stderr).
    let mut child = cargo
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io_err)?;

    // The pipe is present because we just set `Stdio::piped()`; the fallback
    // keeps this panic-free rather than unwrapping the `Option`.
    let mut captured = String::new();
    if let Some(stderr) = child.stderr.take() {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        // Read raw bytes per chunk so a carriage-return progress bar (which
        // carries no newline) still surfaces; `read_line` alone would block on
        // cargo's in-place progress line until the next newline.
        loop {
            line.clear();
            let read = read_progress_chunk(&mut reader, &mut line).map_err(io_err)?;
            if read == 0 {
                break;
            }
            // Forward this chunk live so the user sees cargo's progress as it
            // happens; also accumulate it for a failure diagnostic.
            eprint!("{line}");
            let _ = std::io::stderr().flush();
            captured.push_str(&line);
        }
    }

    let status = child.wait().map_err(io_err)?;
    if status.success() {
        return Ok(());
    }
    Err(CliError::EmittedBuildFailed {
        what,
        code: status.code().unwrap_or(1),
        stderr: captured,
        runtime,
    })
}

/// Read the next chunk of `cargo`'s stderr into `out`, stopping at either a
/// newline (a completed message line) or a carriage return (the boundary of
/// cargo's in-place progress bar, which carries no newline). Returns the number
/// of bytes read; `0` marks end of stream. Reading to *either* terminator keeps
/// the live progress bar flowing rather than buffering until the next `\n`.
///
/// Bytes are decoded lossily so a non-UTF-8 byte from a compiler message never
/// aborts the build's progress relay.
///
/// # Errors
/// Propagates the underlying read error from the `cargo` stderr pipe.
fn read_progress_chunk<R: std::io::Read>(
    reader: &mut R,
    out: &mut String,
) -> std::io::Result<usize> {
    let mut bytes: Vec<u8> = Vec::new();
    let mut total = 0usize;
    loop {
        let mut byte = [0u8; 1];
        let n = reader.read(&mut byte)?;
        if n == 0 {
            break;
        }
        total += n;
        bytes.push(byte[0]);
        if byte[0] == b'\n' || byte[0] == b'\r' {
            break;
        }
    }
    out.push_str(&String::from_utf8_lossy(&bytes));
    Ok(total)
}

/// The runtime crate the emit will link against, as a [`RuntimeContext`] for a
/// build-failure message. `None` when no dependency-model runtime is resolved
/// (a wasm or vendored build), in which case a feature-gap message simply omits
/// the crate reference. Resolution failure is swallowed to `None` — this is only
/// for enriching an error message, never a gate.
fn runtime_context_for_message() -> Option<RuntimeContext> {
    runtime_embed::resolve().ok().map(|r| RuntimeContext {
        root: r.root().to_path_buf(),
        version: r.version().to_owned(),
    })
}

/// Run the three post-emit bundle steps for `--target wasm`:
/// 1. `cargo build --target wasm32-unknown-unknown --release` (THE SEAL cross-target)
/// 2. `wasm-bindgen` CLI — emits the JS glue + `www/pkg/ipe_app_bg.wasm`
/// 3. `wasm-opt -Oz` — optional; silently skipped when not on PATH
///
/// Writes the final `www/pkg/` tree into `out_dir/www/pkg/`. On success the
/// directory at `out_dir/www/` is a self-contained static SPA ready to serve.
///
/// # Errors
/// [`CliError::EmittedBuildFailed`] when the wasm `cargo build` fails;
/// [`CliError::UsageOwned`] when `wasm-bindgen` fails.
fn bundle_wasm(out_dir: &Path) -> Result<(), CliError> {
    // Fail closed before the cross-compile: a missing toolchain becomes a clear
    // root-cause message rather than an opaque OS spawn error.
    let cargo_bin = toolchain::require_cargo(toolchain::ToolIntent::BundleWasm)?;

    // Step 1: compile to .wasm
    let mut cargo = std::process::Command::new(cargo_bin.path());
    cargo
        .args(["build", "--target", "wasm32-unknown-unknown", "--release"])
        .current_dir(out_dir);
    // The wasm build uses the SAME dependency-model runtime crate the native path
    // does (selected via the `wasm-client` floor). Attach the resolved runtime
    // context so a `cargo build` failure that names a missing runtime feature can
    // point at the exact crate; resolution failure degrades to `None` (message
    // enrichment only, never a gate — the missing-path-dependency error cargo
    // itself raises is already fail-closed).
    build_emitted_project(
        &mut cargo,
        "the emitted wasm program",
        runtime_context_for_message(),
        out_dir,
    )?;

    // Step 2: wasm-bindgen — locate the .wasm the cargo build just produced
    // (`CARGO_TARGET_DIR` may relocate it; probe the env var first, then the
    // per-project fallback the emitted manifest's `[workspace]` detachment
    // would use).
    let wasm_path = {
        let via_env = std::env::var_os("CARGO_TARGET_DIR").map(|d| {
            std::path::PathBuf::from(d)
                .join("wasm32-unknown-unknown")
                .join("release")
                .join("ipe_app.wasm")
        });
        let via_crate = out_dir
            .join("target")
            .join("wasm32-unknown-unknown")
            .join("release")
            .join("ipe_app.wasm");
        via_env.filter(|p| p.is_file()).unwrap_or(via_crate)
    };

    let pkg_dir = out_dir.join("www").join("pkg");
    fs::create_dir_all(&pkg_dir).map_err(|e| io_err(&pkg_dir, e))?;

    let wb_status = std::process::Command::new("wasm-bindgen")
        .args([
            wasm_path.to_string_lossy().as_ref(),
            "--target",
            "web",
            "--no-typescript",
            "--out-dir",
            pkg_dir.to_string_lossy().as_ref(),
        ])
        .status()
        .map_err(|e| CliError::Io {
            path: wasm_path.clone(),
            source: e,
        })?;
    if !wb_status.success() {
        let code = wb_status.code().unwrap_or(1);
        return Err(CliError::UsageOwned(format!(
            "wasm-bindgen failed (exit {code}); ensure wasm-bindgen-cli {ver} is installed: \
             cargo install wasm-bindgen-cli --version {ver}",
            ver = "0.2.126"
        )));
    }

    // Step 3: wasm-opt -Oz — optional size pass; silently skip when absent
    // (`Command::new` returns `Err` when the tool is missing).
    let bg_wasm = pkg_dir.join("ipe_app_bg.wasm");
    if bg_wasm.is_file()
        && let Ok(status) = std::process::Command::new("wasm-opt")
            .args([
                bg_wasm.to_string_lossy().as_ref(),
                "-Oz",
                "-o",
                bg_wasm.to_string_lossy().as_ref(),
            ])
            .status()
        && !status.success()
    {
        // wasm-opt found but failed — non-fatal; the unoptimised bundle
        // is still correct. Log and continue.
        eprintln!(
            "{}",
            style::gutter(&format!(
                "note: wasm-opt exited {}; bundle is unoptimised but functional",
                status.code().unwrap_or(1)
            ))
        );
    }

    let bundle_kb = bg_wasm.metadata().map_or(0, |m| m.len() / 1024);
    let www = out_dir.join("www");
    eprintln!(
        "{}",
        style::gutter(&format!(
            "wasm bundle ready at {www}/\n\
             bundle size: {bundle_kb} KB ({bg})\n\
             serve with: python3 -m http.server -d {www} 8080",
            www = www.display(),
            bg = bg_wasm.display(),
        ))
    );
    Ok(())
}

/// `ipe run [<path>]` — compile a program and run the resulting binary.
///
/// One-shot build + run: compiles the entry to `out_dir` (same routing as
/// [`run_build`]), then invokes `cargo build` on the emitted project and
/// execs the resulting `ipe-app` binary, forwarding any arguments supplied
/// after `--` and propagating the binary's exit code.
///
/// Build failures (ipe compile step or cargo build step) surface as
/// [`CliError`] and print to stderr via the normal error path. The binary
/// exec step replaces the current process (Unix) or propagates the child's
/// exit code (all platforms) so the caller sees it as `ipe run`'s own exit.
// A linear pipeline (compile → cargo build → resolve capabilities → jail →
// exec); the steps share enough locals that splitting reads worse than the whole.
#[allow(clippy::too_many_lines)]
fn run_run(rest: &[String]) -> Result<(), CliError> {
    let args = cli_args::parse_run(rest)?;
    let bin_args = args.bin_args;
    let cli_layer = args.static_layer;
    let entry = match args.entry {
        Some(e) => e,
        None => default_entry()?,
    };

    let entry_path = PathBuf::from(&entry);
    let out_dir = args
        .out
        .map_or_else(|| PathBuf::from("out").join("rust"), PathBuf::from);

    // --- Step 1: ipe compile → emit the Rust project ---
    let manifest = discover_manifest(&entry_path)?;

    // Parse the manifest early to read [wasm].mode for target inference.
    let manifest_wasm: Option<project::WasmConfig> = manifest
        .as_deref()
        .map(project::parse_manifest)
        .transpose()?
        .map(|m| m.wasm);

    // When the project declares [wasm].mode != "off", or IPE_TARGET=wasm is
    // set, treat `ipe run` as a wasm build-and-bundle (no native binary to
    // exec). A plain `ipe run` in a non-wasm project stays native.
    let wasm_target = resolve_wasm_target(false, manifest_wasm.as_ref());

    // The dependency model (native OR wasm) needs no vendored tree — the runtime
    // is a path dependency. Only a dep-model-OFF build vendors the source subtree.
    let runtime_dep = runtime_dep_from_env();
    let runtime_dir = resolve_vendored_runtime_dir(args.runtime, !runtime_dep)?;

    // Fail closed before emitting: `ipe run` shells out to cargo to build the
    // emitted project, so a missing toolchain is a clear root-cause error now,
    // not an opaque OS spawn error after the (wasted) compile. The wasm branch
    // delegates to `bundle_wasm`, which resolves cargo itself, so only the
    // native branch resolves here — the resolved path is reused for its build.
    let native_cargo = if wasm_target {
        None
    } else {
        Some(toolchain::require_cargo(toolchain::ToolIntent::Run)?)
    };

    let static_plan = resolve_static_plan(cli_layer, manifest.as_deref())?;
    // `ipe run` is a DEVELOPMENT execution, so `Debug.*` is allowed
    // (production = false).
    let options = BuildOptions {
        static_plan,
        target: if wasm_target {
            ipe_ir::Target::WasmClient
        } else {
            ipe_ir::Target::Native
        },
        wasm_public_env: Vec::new(),
        wasm_hydrate_mode: false,
        production: false,
        runtime_dep,
        // `ipe run` builds and executes; it never tree-shakes the vendored tree
        // (only `ipe eject` does).
        tree_shake_vendored: false,
    };

    // Human-friendly progress: the compile+emit below is otherwise silent, so
    // announce the running step. On a terminal only (piped / CI output stays
    // clean); to stderr, so stdout carries only the program's own output. The
    // cargo build that follows streams its own progress; the exec that ends
    // `ipe run` leaves no room for a settled "done" line, so the run just starts
    // producing the program's output.
    let show_progress = {
        use std::io::IsTerminal as _;
        std::io::stderr().is_terminal()
    };
    if show_progress {
        eprintln!(
            "{}",
            style::gutter(&format!("{} building {entry}", style::glyph::STEP))
        );
    }

    manifest.as_ref().map_or_else(
        || {
            build_with_sibling_discovery_with_options(
                &entry_path,
                &out_dir,
                &runtime_dir,
                options.clone(),
            )
        },
        |m| build_project_with_options(m, &out_dir, &runtime_dir, options.clone()),
    )?;

    // A wasm project has no native binary to run; `ipe run` for a wasm
    // project produces the browser bundle (same post-emit step as
    // `ipe build --target wasm`) and returns, skipping the native exec steps.
    if wasm_target {
        return bundle_wasm(&out_dir);
    }

    // --- Step 2: cargo build the emitted project ---
    // CWD = the emitted crate dir, so the generated `.cargo/config.toml`
    // (`+crt-static` under a static plan) is discovered. The static plan
    // additionally selects the target triple explicitly — the config carries
    // only rustflags, never a `[build] target` pin.
    // `native_cargo` is `Some` on every path that reaches here: the wasm branch
    // returned above, and the native branch resolved cargo before emitting. The
    // fallback re-resolves rather than unwrapping so the toolchain error stays
    // typed even if the branch invariant ever changes.
    let cargo_bin = match native_cargo {
        Some(bin) => bin,
        None => toolchain::require_cargo(toolchain::ToolIntent::Run)?,
    };
    let mut cargo = std::process::Command::new(cargo_bin.path());
    cargo.arg("build").current_dir(&out_dir);
    if let Some(plan) = &static_plan {
        cargo.args(["--target", plan.triple.as_str()]);
    }
    let runtime_ctx = if runtime_dep && !wasm_target {
        runtime_context_for_message()
    } else {
        None
    };
    build_emitted_project(&mut cargo, "the emitted program", runtime_ctx, &out_dir)?;

    // --- Step 3: exec the emitted binary, forwarding args and exit code ---
    // The binary name is always `ipe-app` (the default package name used by
    // `write_emitted_project`; see `ipe_backend_rust::EmittedProject`). The
    // target directory is asked of cargo itself (`cargo metadata`) — a
    // `CARGO_TARGET_DIR` env or a user-level `[build] target-dir` pin
    // relocates the artifact, so a hardcoded `<out>/target` would exec a
    // missing or stale binary.
    let mut bin = cargo_target_directory(&out_dir)?;
    if let Some(plan) = &static_plan {
        bin.push(plan.triple.as_str());
    }
    bin.push("debug");
    bin.push("ipe-app");

    // --- Step 3a: resolve the capability set and, for native code, the jail ---
    // The jail confines the emitted app to `inferred ∪ declared`. It is scoped to
    // native-bearing programs (ADR 0040): pure Ipê is structurally bounded to its
    // inferred capabilities and runs directly; only a `Rust.` crossing has
    // effects inference cannot prove, and only that is jailed. For a native
    // program a missing primitive is fail-closed (refuses unless recorded
    // consent).
    let manifest_parsed = match &manifest {
        Some(m) => Some(project::parse_manifest(m)?),
        None => None,
    };
    let driver = manifest_parsed
        .as_ref()
        .map_or(ipe_backend_rust::DbDriver::Sqlite, |m| m.driver);
    let resolved =
        run_sandbox::resolve_for_run(manifest_parsed.as_ref(), manifest.as_deref(), &entry_path)?;
    let union = resolved.union();
    let native = run_sandbox::is_native_bearing(&union);
    let profile = run_sandbox::build_profile(&resolved, driver)?;
    let bin_args_os: Vec<std::ffi::OsString> =
        bin_args.iter().map(std::ffi::OsString::from).collect();

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        if native {
            // The scoped writable tempdir (the sole writable mount when
            // `filesystem` is absent) and the working tree (bound read-write only
            // when granted) — built only for a jailed run.
            let scoped_tmp = run_sandbox::make_scoped_tmp()?;
            let working_tree = std::env::current_dir().map_err(|e| CliError::Io {
                path: PathBuf::from("."),
                source: e,
            })?;
            // The jail is established and `exec_in_run_jail` replaces this process
            // with the jailed app (does not return on success). On a platform with
            // no jail primitive, the fail-closed policy either refuses or (recorded
            // consent) returns to run unconfined below.
            run_sandbox::jail_and_exec(
                &profile,
                &union,
                &scoped_tmp,
                &working_tree,
                &bin,
                &bin_args_os,
            )?;
        }
        // Pure Ipê (structural guarantee, no jail) or a native program that
        // proceeded unconfined after the recorded-consent warning: run directly.
        let mut cmd = std::process::Command::new(&bin);
        cmd.args(&bin_args);
        let err = cmd.exec();
        Err(CliError::Io {
            path: bin,
            source: err,
        })
    }
    #[cfg(not(unix))]
    {
        if native {
            // Off Unix there is no jail (the documented refuse-gap): `jail_and_exec`
            // applies the fail-closed policy — refuse the native program, or
            // (recorded consent) return Ok to run unconfined below.
            let scoped_tmp = run_sandbox::make_scoped_tmp()?;
            let working_tree = std::env::current_dir().map_err(|e| CliError::Io {
                path: PathBuf::from("."),
                source: e,
            })?;
            run_sandbox::jail_and_exec(
                &profile,
                &union,
                &scoped_tmp,
                &working_tree,
                &bin,
                &bin_args_os,
            )?;
        }
        let mut cmd = std::process::Command::new(&bin);
        cmd.args(&bin_args);
        let status = cmd.status().map_err(|e| CliError::Io {
            path: bin,
            source: e,
        })?;
        // Propagate the child's exit code.  `CliError` only models failure, so
        // a non-zero exit is surfaced as a usage-owned message; the caller
        // (main.rs) prints it to stderr and exits 1.
        if !status.success() {
            let code = status.code().unwrap_or(1);
            return Err(CliError::UsageOwned(format!(
                "ipe-app exited with code {code}"
            )));
        }
        Ok(())
    }
}

/// `ipe exec <artifact-dir> [-- args…]` — run a built artifact, jailing it when
/// it is native-bearing.
///
/// The deployable launcher. A **native-bearing** artifact (ADR 0040) carries an
/// `ipe.profile` mirror plus a capability floor embedded in the binary, so an
/// artifact copied off the build host still runs confined: the profile is
/// *strictly parsed* (parse-fail ⇒ refuse) and refused if weaker than the
/// embedded floor — a tampered profile cannot under-isolate. A **pure** Ipê
/// artifact carries no floor (structurally bounded to its inferred capabilities)
/// and runs directly. A bare `./ipe-app` invocation is the documented, deliberate
/// deployer escape (the raw binary opts out of the jail); this path does not.
///
/// # Errors
/// [`CliError::UsageOwned`] on a missing binary, a native artifact whose profile
/// is missing/tampered, a refused floor check, or a fail-closed jail refusal.
fn run_exec(rest: &[String]) -> Result<(), CliError> {
    // Split `<dir> [-- args…]`.
    let (dir_arg, app_args) = rest
        .iter()
        .position(|a| a == "--")
        .map_or((rest, &[][..]), |i| {
            (
                rest.get(..i).unwrap_or(&[]),
                rest.get(i + 1..).unwrap_or(&[]),
            )
        });
    let dir = dir_arg
        .first()
        .map_or_else(|| PathBuf::from("out").join("rust"), PathBuf::from);
    if !dir.is_dir() {
        return Err(CliError::UsageOwned(format!(
            "ipe exec: no artifact directory at {}",
            dir.display()
        )));
    }

    // Locate the emitted binary (cargo metadata honours a relocated target dir).
    let mut bin = cargo_target_directory(&dir)?;
    bin.push("debug");
    bin.push("ipe-app");
    if !bin.is_file() {
        return Err(CliError::UsageOwned(format!(
            "ipe exec: no built binary at {} — run `ipe build` first",
            bin.display()
        )));
    }

    let app_args_os: Vec<std::ffi::OsString> =
        app_args.iter().map(std::ffi::OsString::from).collect();

    // A native-bearing artifact carries an embedded capability floor and is
    // jailed; a pure Ipê artifact carries none and runs directly (ADR 0040).
    if run_sandbox::artifact_is_native(&bin)? {
        let profile_path = dir.join("ipe.profile");
        if !profile_path.is_file() {
            return Err(CliError::UsageOwned(format!(
                "ipe exec: {} embeds a capability floor but carries no ipe.profile — the artifact \
                 is incomplete or tampered; refusing to run native code without its jail profile",
                bin.display()
            )));
        }
        // Strictly parse the profile and verify it against the embedded floor.
        let profile = run_sandbox::load_and_verify_artifact(&profile_path, &bin)?;

        // The union for the consent/refusal policy is reconstructed from the
        // profile's granted axes (the deployed artifact has no source to
        // re-infer); the floor's presence already established it is native-bearing.
        let mut union = run_sandbox::profile_axes(&profile);
        union.insert(ipe_ir::Capability::NativeFfi);
        let scoped_tmp = run_sandbox::make_scoped_tmp()?;
        let working_tree = std::env::current_dir().map_err(|e| CliError::Io {
            path: PathBuf::from("."),
            source: e,
        })?;

        run_sandbox::jail_and_exec(
            &profile,
            &union,
            &scoped_tmp,
            &working_tree,
            &bin,
            &app_args_os,
        )?;
        // Returns only if recorded consent permitted an unconfined run; fall
        // through to the direct exec below.
    }

    // Pure Ipê artifact, or native that proceeded after the recorded-consent
    // warning: run directly.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let mut cmd = std::process::Command::new(&bin);
        cmd.args(app_args);
        let err = cmd.exec();
        Err(CliError::Io {
            path: bin,
            source: err,
        })
    }
    #[cfg(not(unix))]
    {
        let status = std::process::Command::new(&bin)
            .args(app_args)
            .status()
            .map_err(|e| CliError::Io {
                path: bin.clone(),
                source: e,
            })?;
        if !status.success() {
            return Err(CliError::UsageOwned(format!(
                "ipe-app exited with code {}",
                status.code().unwrap_or(1)
            )));
        }
        Ok(())
    }
}

/// The target directory cargo will use for a build with CWD = `crate_dir`,
/// resolved by cargo itself (`cargo metadata`) so every relocation source —
/// `CARGO_TARGET_DIR`, a user-level `[build] target-dir` pin, a config in an
/// ancestor dir — is honoured instead of guessed at.
fn cargo_target_directory(crate_dir: &Path) -> Result<PathBuf, CliError> {
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(crate_dir)
        .output()
        .map_err(|e| CliError::Io {
            path: crate_dir.to_path_buf(),
            source: e,
        })?;
    if !output.status.success() {
        return Err(CliError::UsageOwned(format!(
            "cargo metadata failed in {}: {}",
            crate_dir.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let meta: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        CliError::UsageOwned(format!("cargo metadata emitted unparseable JSON: {e}"))
    })?;
    meta.get("target_directory")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| {
            CliError::UsageOwned("cargo metadata reported no target_directory".to_owned())
        })
}

/// `ipe explain [<CODE>]`. No argument prints the one-line index of every code
/// and its title; an argument prints that code's embedded explain page.
fn run_explain(rest: &[String]) -> Result<(), CliError> {
    // The format flags apply to the LIST (`ipe explain` with no code) — the
    // machine-consumable surface. Explaining a single code prints a human
    // teaching page, which carries no `--plain` / `--json` form.
    let (format, positional) = cli_args::split_format(rest, "explain")?;
    match positional.first() {
        None => {
            print!("{}", render_code_index(format, &std::io::stdout()));
            Ok(())
        }
        Some(arg) => {
            if format != cli_args::OutputFormat::Human {
                return Err(CliError::Usage(
                    "--plain / --json apply to the code list (`ipe explain` with no code), \
                     not to a single code's explanation",
                ));
            }
            let page = explain_lookup(arg)?;
            print!("{}", style::frame(&style::gutter(page)));
            Ok(())
        }
    }
}

/// Render the diagnostic-code list in the requested [`OutputFormat`].
///
/// - Human (default): a guttered `<CODE>  <title>` table, one code per line.
/// - `--plain`: the same `<CODE>\t<title>` rows, flush-left and tab-separated so
///   `cut -f1` yields the codes and `grep`/`awk` slice the table.
/// - `--json`: `{"codes": [{"code": "IPE-…", "title": "…"}, …]}`, a stable array
///   of `{code, title}` objects in taxonomy order.
fn render_code_index(format: cli_args::OutputFormat, stream: &impl std::io::IsTerminal) -> String {
    use std::fmt::Write as _;

    use cli_args::OutputFormat::{Human, Json, Plain};
    match format {
        Plain => {
            let mut out = String::new();
            for &c in ALL_CODES {
                let _ = writeln!(out, "{}\t{}", c.as_str(), title(c));
            }
            out
        }
        Json => {
            let rows: Vec<String> = ALL_CODES
                .iter()
                .map(|&c| format!("{{\"code\":{:?},\"title\":{:?}}}", c.as_str(), title(c)))
                .collect();
            format!("{{\"codes\":[{}]}}\n", rows.join(","))
        }
        Human => {
            let p = style::Palette::for_stream(stream);
            let mut body = String::new();
            let _ = writeln!(
                body,
                "{}Diagnostic codes{} — run {}ipe explain <CODE>{} for the full teaching page:\n",
                p.bold, p.reset, p.yellow, p.reset,
            );
            for &c in ALL_CODES {
                let _ = writeln!(
                    body,
                    "  {}{}{}  {}",
                    p.yellow,
                    c.as_str(),
                    p.reset,
                    title(c),
                );
            }
            style::frame(&style::gutter(&body))
        }
    }
}

/// `ipe fix <path>` — apply machine-applicable fixes to the source file.
/// Default is interactive per-edit confirmation;
/// `--yes` is durable authorization to apply every machine-applicable edit.
fn run_fix(rest: &[String]) -> Result<(), CliError> {
    let args = cli_args::parse_fix(rest)?;
    apply_fixes_cmd(
        &PathBuf::from(&args.entry),
        args.auto,
        &mut std::io::stdout(),
    )?;
    Ok(())
}

// ===========================================================================
// `explain` — code index, lookup, and did-you-mean
// ===========================================================================

/// The one-line-per-code index: `<CODE>  <title>\n`, in taxonomy order.
#[must_use]
pub fn code_index() -> String {
    let mut s = String::new();
    for &c in ALL_CODES {
        s.push_str(c.as_str());
        s.push_str("  ");
        s.push_str(title(c));
        s.push('\n');
    }
    s
}

/// Resolve a (case-insensitive) code string to its embedded explain page.
///
/// The input is trimmed and upper-cased before matching, so `ipe-t0001` and
/// `IPE-T0001` both resolve.
///
/// # Errors
/// Returns [`CliError::UnknownCode`] (carrying a deterministic did-you-mean
/// list) when the string is not a taxonomy code.
pub fn explain_lookup(input: &str) -> Result<&'static str, CliError> {
    let canonical = input.trim().to_ascii_uppercase();
    for &c in ALL_CODES {
        if c.as_str() == canonical {
            // `explain_page` is `Some` for every `ALL_CODES` member; the `None`
            // arm is surfaced as a typed error rather than a panic.
            return explain_page(c).map_or_else(
                || {
                    Err(CliError::UnknownCode {
                        input: input.trim().to_owned(),
                        suggestions: Vec::new(),
                    })
                },
                Ok,
            );
        }
    }
    Err(CliError::UnknownCode {
        input: input.trim().to_owned(),
        suggestions: did_you_mean_codes(&canonical),
    })
}

/// The known command closest to `attempted` by Levenshtein distance, within a
/// small edit threshold — the "maybe ...?" hint for a mistyped command. `None`
/// when nothing is close enough, so a wildly different token gets only the help
/// screen, not a misleading guess.
fn nearest_command(attempted: &str) -> Option<&'static str> {
    help::command_names()
        .into_iter()
        .map(|name| (levenshtein(attempted, name), name))
        .filter(|&(dist, _)| dist <= 3)
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, name)| name)
}

/// The closest known codes to `canonical` (already upper-cased), ranked by
/// `(Levenshtein, code)` and filtered to a small edit distance. Deterministic.
fn did_you_mean_codes(canonical: &str) -> Vec<&'static str> {
    let mut scored: Vec<(usize, &'static str)> = ALL_CODES
        .iter()
        .map(|&c| (levenshtein(canonical, c.as_str()), c.as_str()))
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    scored
        .into_iter()
        .filter(|&(dist, _)| dist <= 3)
        .take(3)
        .map(|(_, name)| name)
        .collect()
}

/// Classic two-row Levenshtein edit distance. Uses no slice indexing (only
/// `get`/`push`/`last`), so it cannot panic.
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur: Vec<usize> = Vec::with_capacity(b.len().saturating_add(1));
        cur.push(i.saturating_add(1));
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let del = prev.get(j.saturating_add(1)).copied().unwrap_or(usize::MAX);
            let ins = cur.get(j).copied().unwrap_or(usize::MAX);
            let sub = prev.get(j).copied().unwrap_or(usize::MAX);
            cur.push(
                del.saturating_add(1)
                    .min(ins.saturating_add(1))
                    .min(sub.saturating_add(cost)),
            );
        }
        prev = cur;
    }
    prev.last().copied().unwrap_or(0)
}

// ===========================================================================
// `--emit-ir` — pretty-print the lowered IR
// ===========================================================================

/// Run parse → canon → types → lower and return the pretty-printed IR tree,
/// stopping before codegen.
///
/// # Errors
/// Returns [`CliError::Pipeline`] when the compiler rejects the program, or
/// [`CliError::Io`] when the entry file cannot be read.
pub fn emit_ir_text(entry: &Path) -> Result<String, CliError> {
    let (db, program) = lower_entry_via_graph(entry)?;
    let interner = ipe_db::Db::interner(&db).lock();
    Ok(ipe_ir::pretty(&program, &interner))
}

// ===========================================================================
// `capabilities` — report / verify a program's inferred capability set
// ===========================================================================

/// Run parse → canon → types → lower over a single `.ipe` entry, returning the
/// lowered program. Shares the exact pipeline [`emit_ir_text`] uses so the two
/// analysis surfaces cannot diverge.
///
/// # Errors
/// [`CliError::Pipeline`] when the compiler rejects the program;
/// [`CliError::Io`] when the entry file cannot be read.
pub(crate) fn lower_entry(entry: &Path) -> Result<ipe_ir::Program, CliError> {
    let (_db, program) = lower_entry_via_graph(entry)?;
    Ok((*program).clone())
}

/// Lower a single `.ipe` entry through the SAME injection-aware source-graph
/// pipeline the build path uses, returning the owning database (its interner
/// backs any downstream `ipe_ir::pretty`) and the lowered program.
///
/// This routes through sibling discovery + compiled-source stdlib injection +
/// the salsa `lower_program` query rather than a bare single-module
/// parse→canon→infer→lower. Without injection an entry importing a
/// compiled-source stdlib module (e.g. `Ipe.Test`) fails name resolution with
/// IPE-N0004 even though a real `ipe build` of the same program succeeds — the
/// analysis surfaces (`ipe capabilities`, `ipe build --emit-ir`) must resolve
/// such a module identically to the build.
///
/// # Errors
/// [`CliError::Pipeline`] carrying the first compiler diagnostic;
/// [`CliError::Io`] when a source file cannot be read.
fn lower_entry_via_graph(
    entry: &Path,
) -> Result<(ipe_db::IpeDatabase, std::sync::Arc<ipe_ir::Program>), CliError> {
    let graph = build_source_graph(entry)?;
    let program = graph.run_attributed(entry, |db, root, file| {
        ipe_db::lower_program(db, root, file)
    })?;
    Ok((graph.db, program))
}

/// The salsa inputs one analysis needs: the owning database, the whole-program
/// source root, and the entry module's [`ipe_db::SourceFile`] handle — the
/// product of sibling discovery + compiled-source stdlib injection shared by
/// every single-entry analysis path.
struct SourceGraph {
    db: ipe_db::IpeDatabase,
    source_root: ipe_db::SourceRoot,
    entry_file: ipe_db::SourceFile,
    /// The whole module set (path → (file, src)) — every module a diagnostic
    /// span may index into, so a rejecting query can be framed against the
    /// source that OWNS the span rather than the entry file (the caret bug).
    sources: BTreeMap<Vec<String>, (PathBuf, String)>,
    /// The entry module's dotted path — its `(file, src)` is the fallback frame
    /// for a homeless / dummy-span diagnostic.
    entry_module_path: Vec<String>,
}

impl SourceGraph {
    /// Run the per-module canonicalisation blame loop, then map a rejecting
    /// query's `(diag, home)` to the source file that OWNS it — the SAME
    /// attribution the build path uses (`attribute_canon_errors` +
    /// `attribute_post_link_error`), so `ipe type-check` and every other analysis
    /// surface frame a given diagnostic against the identical source as
    /// `ipe build`.
    ///
    /// A canon error (e.g. IPE-N0020) surfaces from the blame loop already
    /// framed against its own module; only a post-link error reaches the
    /// `run_query` closure, where its `home` (or the byte-offset heuristic over
    /// the linked program) selects the owning source.
    ///
    /// # Errors
    /// [`CliError::Pipeline`] carrying the first compiler diagnostic; the query
    /// closure's own error otherwise.
    fn run_attributed<T>(
        &self,
        blame_path: &Path,
        run_query: impl FnOnce(
            &ipe_db::IpeDatabase,
            ipe_db::SourceRoot,
            ipe_db::SourceFile,
        ) -> Result<T, (Diagnostic, Vec<ipe_intern::Symbol>)>,
    ) -> Result<T, CliError> {
        attribute_canon_errors(
            &self.db,
            self.source_root,
            &self.sources,
            self.entry_file,
            blame_path,
        )?;
        run_query(&self.db, self.source_root, self.entry_file).map_err(|(diag, home)| {
            // Canon succeeded, so the linked program exists; use it for the
            // byte-offset fallback when `home` is empty. A link failure here
            // (empty home, no linked program) frames against the entry file.
            let entry = self
                .sources
                .get(&self.entry_module_path)
                .cloned()
                .unwrap_or_else(|| (blame_path.to_path_buf(), String::new()));
            let interner = ipe_db::Db::interner(&self.db).clone();
            let home_to_source = home_to_source_map(&interner, &self.sources);
            match ipe_db::linked_program(&self.db, self.source_root, self.entry_file) {
                Ok(linked) => {
                    attribute_post_link_error(&linked.module, &home_to_source, &entry, diag, &home)
                }
                Err(link_diag) => {
                    // A link error has no linked program to scan; frame the
                    // ORIGINAL query diagnostic (not the link error) against the
                    // home module if known, else the entry file.
                    let (file, src) = if home.is_empty() {
                        entry
                    } else {
                        home_to_source.get(&home).cloned().unwrap_or(entry)
                    };
                    // `link_diag` is discarded: the query's own diagnostic is the
                    // one the user asked about; a link error would already have
                    // surfaced from the canon blame loop or a build.
                    let _ = link_diag;
                    CliError::Pipeline {
                        file,
                        src,
                        diag: Box::new(diag),
                    }
                }
            }
        })
    }
}

/// Build the injection-aware whole-program source graph for a single `.ipe`
/// entry: discover its siblings, inject the compiled-source stdlib closure, and
/// create the salsa source root. Shared by [`lower_entry_via_graph`] and
/// [`typecheck_entry_via_graph`] so the build, capabilities, `--emit-ir`, and
/// `check` surfaces all resolve the same module set — a compiled-source stdlib
/// import (e.g. `Ipe.Test`) resolves identically across every one.
///
/// # Errors
/// [`CliError::Pipeline`] when the entry does not parse; [`CliError::Io`] on any
/// filesystem failure; [`CliError::Usage`] if the entry is not in the built map.
fn build_source_graph(entry: &Path) -> Result<SourceGraph, CliError> {
    let mut collected = collect_entry_and_siblings(entry)?;
    let injected =
        project::inject_compiled_std_closure(&mut collected.sources, &mut collected.discovered);
    // The SAME FFI seam the build runs: without it, a project with installed
    // crates (or asserted `Rust.Ffi.call` definitions) has no `Rust.*`
    // interface modules here, so `ipe type-check` / `ipe capabilities` /
    // `--emit-ir` would refuse a program the build accepts.
    let ffi_injected = ffi::prepare_ffi(&mut collected.sources, entry)?.injected;

    let db = ipe_db::IpeDatabase::new();
    let source_root = create_source_root(&db, &collected.sources, &injected, &ffi_injected);
    let Some(entry_file) = source_root
        .files(&db)
        .get(&collected.entry_module_path)
        .copied()
    else {
        return Err(CliError::Usage("internal: entry module not in source map"));
    };

    Ok(SourceGraph {
        db,
        source_root,
        entry_file,
        sources: collected.sources,
        entry_module_path: collected.entry_module_path,
    })
}

/// Type-check a single `.ipe` entry through the SAME injection-aware
/// source-graph pipeline the build path uses, stopping at type-checking: it
/// demands the `typecheck` query (parse → canon → link → HM infer) and never
/// lowers to IR or emits Rust. This is what `ipe type-check` runs.
///
/// # Errors
/// [`CliError::Pipeline`] carrying the first compiler diagnostic;
/// [`CliError::Io`] when a source file cannot be read.
fn typecheck_entry_via_graph(entry: &Path) -> Result<(), CliError> {
    let graph = build_source_graph(entry)?;
    graph.run_attributed(entry, |db, root, file| {
        // Type-check first so an ordinary type error surfaces ahead of the
        // decoder-direction gate; then run the SAME IPE-N0040 gate the build
        // path runs (`gate_decoder_pipelines`) over the linked module, so
        // `ipe type-check` rejects the hand-nested decoder footgun for the
        // earliest possible feedback rather than deferring it to `ipe build`.
        // `linked_program` re-demands the memos `typecheck` just populated.
        ipe_db::typecheck(db, root, file)?;
        let linked = ipe_db::linked_program(db, root, file).map_err(|d| (d, Vec::new()))?;
        gate_decoder_pipelines(&linked.module)
    })
}

/// `ipe capabilities <entry.ipe>` — print the program's inferred security
/// capabilities, one per line in sorted order, or `none` when the program is
/// pure. Read-only analysis: nothing is emitted or written.
/// `ipe package <subcommand>` — package-authoring commands: `audit` (the SP4
/// Tier-1 package gate) and `publish` (run the gate, compute the index entry, and
/// open the index PR).
///
/// # Errors
/// [`CliError::UsageOwned`] on a missing or unknown subcommand; the subcommand's
/// own errors (a build failure, a [`CliError::PackageAudit`] reject, or a
/// [`CliError::Publish`] refusal) otherwise.
fn run_package(rest: &[String]) -> Result<(), CliError> {
    match rest.split_first() {
        Some((sub, tail)) if sub == "audit" => audit::run_audit(tail),
        Some((sub, tail)) if sub == "publish" => publish::run_publish(tail),
        Some((sub, tail)) if sub == "validate-entry" => run_validate_entry(tail),
        Some((sub, _)) => Err(CliError::UsageOwned(format!(
            "ipe package: unknown subcommand `{sub}` (expected `audit`, `publish`, or \
             `validate-entry`)"
        ))),
        None => Err(CliError::Usage(
            "usage: ipe package <audit|publish|validate-entry> [<path>]",
        )),
    }
}

/// `ipe package validate-entry <packages/<name>.toml>` — validate one curated
/// index entry file against the entry schema, fail-closed.
///
/// The index repository's admission CI runs this on a submitted entry as its
/// cheap structural gate before the source-pin and `ipe package audit` steps: it
/// reuses the resolver's own parser ([`index::validate_entry_file`]), so a file
/// that validates here is exactly a file the resolver will later read. On success
/// it prints the package name and every version it parsed; on any malformed field
/// it exits non-zero with the parser's diagnostic.
///
/// # Errors
/// [`CliError::Usage`] when no entry file is given; [`CliError::UsageOwned`] on a
/// bad path or an extra argument; the parser's [`CliError::Resolve`] /
/// [`CliError::Io`] when the entry is malformed or unreadable.
fn run_validate_entry(rest: &[String]) -> Result<(), CliError> {
    let path = match rest {
        [one] => PathBuf::from(one),
        [] => {
            return Err(CliError::Usage(
                "usage: ipe package validate-entry <packages/<name>.toml>",
            ));
        }
        _ => {
            return Err(CliError::UsageOwned(
                "ipe package validate-entry: expected a single entry-file path".to_owned(),
            ));
        }
    };
    let entry = index::validate_entry_file(&path)?;
    let versions: Vec<String> = entry
        .versions
        .iter()
        .map(|v| v.version.to_string())
        .collect();
    let body = format!(
        "entry ok: {} (publisher {}) — {} version(s): {}",
        entry.name,
        entry.publisher,
        versions.len(),
        versions.join(", ")
    );
    print!("{}", style::frame(&style::gutter(&body)));
    Ok(())
}

/// Resolve a `check`/analysis `<path>` argument to the entry `.ipe` file the
/// source-graph pipeline reads. Same argument convention as `ipe build`:
///
/// 1. a directory → its `ipe.toml`'s `src`-root `Main.ipe`;
/// 2. an `ipe.toml` → that manifest's `src`-root `Main.ipe`;
/// 3. a `.ipe` file → itself.
///
/// A project's entry module is always `Main` (`project` module doc), so the
/// entry file is `<src_root>/Main.ipe`.
///
/// # Errors
/// [`CliError::Usage`] for a directory with no `ipe.toml`; the manifest's own
/// parse errors otherwise.
fn resolve_analysis_entry(path: &Path) -> Result<PathBuf, CliError> {
    let manifest = discover_manifest(path)?;
    match manifest {
        Some(m) => {
            let parsed = project::parse_manifest(&m)?;
            Ok(parsed.src_root.join("Main.ipe"))
        }
        None => Ok(path.to_path_buf()),
    }
}

/// `ipe type-check [<path>]` — type-check a program and stop. Runs the same
/// injection-aware source graph `ipe build` uses, but demands only the
/// `typecheck` query: no IR lowering, no Rust emission, nothing written. Exits
/// 0 with a friendly framed success line when the program type-checks, or
/// non-zero carrying the first rendered diagnostic when it does not.
fn run_type_check(rest: &[String]) -> Result<(), CliError> {
    let arg = match cli_args::single_positional(rest, "type-check")? {
        Some(e) => PathBuf::from(e),
        None => PathBuf::from(default_entry()?),
    };
    let entry = resolve_analysis_entry(&arg)?;
    typecheck_entry_via_graph(&entry)?;
    let p = style::Palette::for_stream(&std::io::stdout());
    print!(
        "{}",
        style::frame(&style::gutter(&format!(
            "{}{} No type errors — this program type-checks.{}",
            p.green,
            style::glyph::OK,
            p.reset,
        )))
    );
    Ok(())
}

/// A single `ipe verify` stage: run the underlying check over an optional
/// `<path>` (the current project when `None`), returning its own error on
/// failure.
type VerifyStage = fn(Option<&str>) -> Result<(), CliError>;

/// The ordered stages `ipe verify` runs, each composing the same code path its
/// standalone command uses. The order is the cheapest, most localised check
/// first: a formatting scan reads source only; a type-check parses and infers
/// but emits nothing; a build compiles all the way to an artifact; a test run
/// exercises the project's `tests/Main.ipe` entry (when one exists).
const VERIFY_STAGES: &[(&str, VerifyStage)] = &[
    ("format", verify_fmt),
    ("type-check", verify_check),
    ("build", verify_build),
    ("test", verify_test),
];

/// Stage 1: the formatting scan — `ipe fmt --check` over `<path>` (the current
/// directory when none is given), reporting unformatted files without rewriting.
fn verify_fmt(path: Option<&str>) -> Result<(), CliError> {
    let mut rest: Vec<String> = Vec::new();
    if let Some(p) = path {
        rest.push(p.to_owned());
    }
    rest.push("--check".to_owned());
    fmt::run_fmt(&rest)
}

/// Stage 2: the type-check — the same source-graph pipeline as `ipe type-check`.
fn verify_check(path: Option<&str>) -> Result<(), CliError> {
    run_type_check(&path.map(str::to_owned).into_iter().collect::<Vec<_>>())
}

/// Stage 3: the build — the same compilation as `ipe build`.
fn verify_build(path: Option<&str>) -> Result<(), CliError> {
    run_build(&path.map(str::to_owned).into_iter().collect::<Vec<_>>())
}

/// The outcome of running a project's `tests/Main.ipe` entry.
///
/// A parsed result rather than a bare `Result<(), _>`: "the project defines no
/// test entry" is a distinct, legitimate state from "the tests ran and all
/// passed", and the two render differently (`no tests to run` vs `all passed`).
/// A failing test run is NOT this type — it is a hard [`CliError::TestFailed`],
/// because the test binary has already printed its own summary and the CLI's
/// only job is to fail non-zero. This makes the exit-code contract structural:
/// a `TestOutcome` value can never represent a failure, so no caller can
/// accidentally return success over failing tests.
#[derive(Debug, PartialEq, Eq)]
enum TestOutcome {
    /// The project has no `tests/Main.ipe` — there is nothing to run, which is
    /// not an error.
    NoTestEntry,
    /// The test entry was built, run, and every case passed (the binary exited
    /// zero).
    AllPassed,
}

/// Build and run a project's `tests/Main.ipe`, the single test runner shared by
/// `ipe test` and `ipe verify`'s final stage.
///
/// The test entry is the file at `<project-root>/tests/Main.ipe`. The project
/// root is the directory holding `ipe.toml`; with no manifest it is the parent
/// of the entry's `src/` directory (the conventional layout), or the entry's
/// own directory for a flat single-directory project. The test entry is built
/// against the project's `src/` tree AND its `tests/` siblings, so a test that
/// imports the code under test resolves. When the test entry is absent the
/// runner returns [`TestOutcome::NoTestEntry`] — a project with no test entry is
/// not an error. When it exists, the test runner is compiled to a temporary
/// output directory, the emitted Rust project is built with `cargo build`, and
/// the resulting `ipe-app` binary is executed. The binary itself prints the
/// per-test failures and the `N passed, M failed` summary (from
/// `Ipe.Test.runMain`) to stdout; this function only classifies its exit code.
///
/// # Errors
/// [`CliError::TestFailed`] when the test binary exits non-zero (one or more
/// cases failed) — the binary's own output is the report. Otherwise any build
/// or toolchain error encountered while compiling the runner.
fn run_project_tests(path: Option<&str>) -> Result<TestOutcome, CliError> {
    // Resolve the project root from the supplied path (or cwd defaults).
    let entry_path = match path {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(default_entry()?),
    };

    // Resolve the project root and the source root (the `src/` tree the code
    // under test lives in). With a manifest, both come from it — the manifest's
    // directory and its declared `src_root` (honouring a `srcDir` override).
    // Without a manifest, the entry's own directory is the source root, and the
    // project root is the source root's parent when the entry lives under a
    // conventional `src/` directory (so `src/Main.ipe`'s sibling `tests/` tree
    // is at `<project-root>/tests`, not `src/tests`); otherwise the entry's
    // directory is itself the project root (a flat single-directory project).
    let manifest = discover_manifest(&entry_path)?;
    let (project_root, project_src_root): (PathBuf, PathBuf) = if let Some(m) = manifest.as_ref() {
        let root = m
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        (root, project::parse_manifest(m)?.src_root)
    } else {
        let src_root = entry_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let root = if src_root.file_name().and_then(|n| n.to_str()) == Some("src") {
            src_root
                .parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        } else {
            src_root.clone()
        };
        (root, src_root)
    };

    let test_entry = project_root.join("tests").join("Main.ipe");
    if !test_entry.is_file() {
        // No test entry — there is nothing to run.
        return Ok(TestOutcome::NoTestEntry);
    }

    // Fail closed before emitting: the test stage shells out to cargo to build
    // the test runner, so a missing toolchain is reported with its root cause
    // rather than an opaque OS spawn error.
    let cargo_bin = toolchain::require_cargo(toolchain::ToolIntent::Test)?;

    let runtime_dir = resolve_runtime()?;

    // Emit into a unique temp directory so concurrent verify runs do not
    // collide and the output is never confused with the project's own `out/`.
    let out_dir = std::env::temp_dir().join(format!("ipe_verify_test_{}", std::process::id()));

    // Build the test entry. When the project has a `src/` tree, the test entry
    // is built against BOTH it (the code under test) and the `tests/` tree (its
    // test-only siblings), so a `tests/Main.ipe` importing `Lib.Foo` from
    // `src/Lib/Foo.ipe` resolves. A `tests/`-only project with no `src/` (a
    // standalone test) falls back to sibling discovery rooted at `tests/`. On
    // any compile failure the stage propagates that error directly — the error
    // is already a well-formed `CliError`.
    // Build, then run, the test entry. Everything after the temp output is
    // created runs inside this closure so a single cleanup below removes the
    // temp directory on EVERY exit — a compile failure, a cargo failure, a
    // spawn error, or a normal run — not only the success path.
    let outcome = build_and_run_test_entry(
        &project_src_root,
        &test_entry,
        &out_dir,
        &runtime_dir,
        cargo_bin.path(),
    );

    // Clean up the temp output regardless of the outcome.
    let _ = std::fs::remove_dir_all(&out_dir);

    outcome
}

/// Compile the test entry into `out_dir`, build the emitted Rust project, and
/// run the resulting `ipe-app` binary, classifying its exit code.
///
/// Split from [`run_project_tests`] so the caller's temp-directory cleanup runs
/// on every exit of this fallible sequence, not only the success path.
///
/// # Errors
/// The compile/build error on a compile or cargo failure; [`CliError::Io`] when
/// the test binary cannot be spawned; [`CliError::TestFailed`] when it exits
/// non-zero (a failing case, or a crash/signal with no exit code).
fn build_and_run_test_entry(
    project_src_root: &Path,
    test_entry: &Path,
    out_dir: &Path,
    runtime_dir: &Path,
    cargo_bin: &Path,
) -> Result<TestOutcome, CliError> {
    if project_src_root.is_dir() {
        build_test_with_project_sources(project_src_root, test_entry, out_dir, runtime_dir)?;
    } else {
        build_with_sibling_discovery(test_entry, out_dir, runtime_dir)?;
    }

    // Compile the emitted Rust project.
    let mut cargo = std::process::Command::new(cargo_bin);
    cargo.arg("build").current_dir(out_dir);
    build_emitted_project(
        &mut cargo,
        "the emitted test runner",
        runtime_context_for_message(),
        out_dir,
    )?;

    // Locate the compiled binary via `cargo metadata` so a user-level
    // `CARGO_TARGET_DIR` pin or workspace override is respected.
    let mut bin = cargo_target_directory(out_dir)?;
    bin.push("debug");
    bin.push("ipe-app");

    // Run the test binary. `Ipe.Test.runMain` exits 0 on all-pass, 1 on any
    // failure — propagate that as a stage error.
    let run_status = std::process::Command::new(&bin)
        .status()
        .map_err(|e| CliError::Io {
            path: bin.clone(),
            source: e,
        })?;

    if run_status.success() {
        Ok(TestOutcome::AllPassed)
    } else {
        // A zero exit is the ONLY success signal. Any other exit — a failing
        // case (1 from `Ipe.Test.runMain`) or a crash/signal (no code) — is a
        // failure; classify the absent code as a failure, never a pass.
        let code = run_status.code().unwrap_or(1);
        Err(CliError::TestFailed { code })
    }
}

/// Stage 4 of `ipe verify`: run the project's tests via the shared
/// [`run_project_tests`] runner, discarding the pass/no-entry distinction the
/// stage does not need (both are a passing stage).
///
/// # Errors
/// [`CliError::TestFailed`] when a test case fails; otherwise any build or
/// toolchain error from compiling the runner.
fn verify_test(path: Option<&str>) -> Result<(), CliError> {
    run_project_tests(path).map(|_| ())
}

/// `ipe test [<path>]` — build and run the project's tests, with human-friendly
/// output and a machine-readable exit code.
///
/// Compiles `tests/Main.ipe` against the project's `src/` tree and runs it. The
/// test binary prints the per-case failures and the `N passed, M failed`
/// summary itself (from `Ipe.Test.runMain`); this command wraps that in a
/// single progress stage — a light-yellow running line that settles to a green
/// check (`all tests passed` / `no tests to run`) or, on a failing case, a red
/// cross and a non-zero exit. A project with no `tests/Main.ipe` is not an
/// error: the command reports there is nothing to run and exits zero.
///
/// # Errors
/// [`CliError::UsageOwned`] on an unexpected option or extra argument.
/// [`CliError::TestFailed`] when a test case fails (the non-zero exit contract).
/// Otherwise any build or toolchain error from compiling the runner.
fn run_test(rest: &[String]) -> Result<(), CliError> {
    let path = cli_args::single_positional(rest, "test")?;

    // Wrap the runner in a progress stage so `ipe test` follows the same
    // running → ✓/✗ shape every other multi-step command uses. The stage writes
    // to stdout; the test binary the runner spawns inherits stdout too, so its
    // own summary appears between the running line and the settled outcome.
    let stage = progress::Stage::start(std::io::stdout(), "Running tests");
    match run_project_tests(path) {
        Ok(TestOutcome::AllPassed) => {
            stage.success("all tests passed");
            Ok(())
        }
        Ok(TestOutcome::NoTestEntry) => {
            stage.success("no tests to run (no tests/Main.ipe)");
            Ok(())
        }
        Err(err) => {
            // A failing case (or any build error) settles the stage red before
            // the error propagates to the exit-code contract.
            stage.failure("tests failed");
            Err(err)
        }
    }
}

/// `ipe verify [<path>]` — the one-command project gate.
///
/// Runs the project's checks in order — format, type-check, build, test —
/// stopping at the first failure. Each stage composes the same code path its
/// standalone command uses, so `verify` is a faithful union of them, never a
/// second implementation. `<path>` defaults to the current project.
///
/// The test stage builds and runs `tests/Main.ipe` when that file exists in the
/// project root. A project with no `tests/Main.ipe` passes the test stage
/// immediately — no test entry means no tests to run.
///
/// # Errors
/// [`CliError::UsageOwned`] on an unexpected option or extra argument. Otherwise
/// the first failing stage's own error, which carries its diagnostic and drives
/// the non-zero exit; a clean run exits 0.
fn run_verify(rest: &[String]) -> Result<(), CliError> {
    let path = cli_args::single_positional(rest, "verify")?;
    let total = VERIFY_STAGES.len();

    for (index, (name, stage)) in VERIFY_STAGES.iter().enumerate() {
        let step = index + 1;
        // Each stage is one progress line: a light-yellow running line that
        // settles to a green ✓ or a red ✗ — the shared stage shape every
        // multi-step command uses, not a hand-rolled colour print.
        let line =
            progress::Stage::start(std::io::stdout(), format!("stage {step}/{total}: {name}"));
        if let Err(err) = stage(path) {
            line.failure(format!("stage {step}/{total}: {name} failed"));
            // The stage ran correctly and reported a real failure — a gate
            // result, not a misuse of `verify`. Rewrap it as [`VerifyFailed`] so
            // the stage's own rendered report is shown alone, never the `verify`
            // `--help` page a raw usage error would trigger.
            return Err(CliError::VerifyFailed {
                stage: name,
                report: err.to_string(),
            });
        }
        line.success(format!("stage {step}/{total}: {name} passed"));
    }

    let summary = progress::Stage::start(std::io::stdout(), "gate");
    summary.success(format!("all {total} stages passed"));
    Ok(())
}

fn run_capabilities(rest: &[String]) -> Result<(), CliError> {
    let (format, positional) = cli_args::split_format(rest, "capabilities")?;
    let arg = match positional.first() {
        Some(e) => PathBuf::from(e),
        None => PathBuf::from(default_entry()?),
    };
    // Route a directory / `ipe.toml` / project-root `.` to its entry `.ipe` file,
    // the same argument convention `ipe type-check` uses. Without this a bare
    // `ipe capabilities` in a project dir passes `.` straight to the reader and
    // fails with a raw "Is a directory" io error.
    let entry = resolve_analysis_entry(&arg)?;
    let program = lower_entry(&entry)?;
    let caps = ipe_lower::program_capabilities(&program);
    let names: Vec<&'static str> = caps.iter().map(|c| c.as_str()).collect();
    print!(
        "{}",
        render_capabilities(&names, format, &std::io::stdout())
    );
    Ok(())
}

/// Render a program's inferred capability set in the requested [`OutputFormat`].
///
/// - Human (default): a guttered, labelled report — a heading and one bullet per
///   capability, or a line saying the program is pure.
/// - `--plain`: the bare capability names, one per line, flush-left (or nothing
///   at all for a pure program — the scriptable form pipelines already consume).
/// - `--json`: `{"capabilities": ["network", …]}`, a stable object whose one
///   `capabilities` field is the sorted name array (empty for a pure program).
fn render_capabilities(
    names: &[&str],
    format: cli_args::OutputFormat,
    stream: &impl std::io::IsTerminal,
) -> String {
    use std::fmt::Write as _;

    use cli_args::OutputFormat::{Human, Json, Plain};
    match format {
        Plain => {
            // The historical scriptable form: bare names, one per line. A pure
            // program prints nothing, so `| wc -l` counts the capabilities.
            let mut out = String::new();
            for name in names {
                out.push_str(name);
                out.push('\n');
            }
            out
        }
        Json => {
            let quoted: Vec<String> = names.iter().map(|n| format!("{n:?}")).collect();
            format!("{{\"capabilities\":[{}]}}\n", quoted.join(","))
        }
        Human => {
            let p = style::Palette::for_stream(stream);
            let mut body = String::new();
            if names.is_empty() {
                body.push_str("This program is pure — it exercises no security capabilities.\n");
            } else {
                let noun = if names.len() == 1 {
                    "capability"
                } else {
                    "capabilities"
                };
                let _ = writeln!(
                    body,
                    "This program exercises {} security {noun}:",
                    names.len(),
                );
                for name in names {
                    let _ = writeln!(
                        body,
                        "  {}{}{} {}{name}{}",
                        p.yellow,
                        style::glyph::STEP,
                        p.reset,
                        p.yellow,
                        p.reset,
                    );
                }
            }
            style::frame(&style::gutter(&body))
        }
    }
}

/// `ipe version` — print the ipe version in the requested format.
fn run_version(rest: &[String]) -> Result<(), CliError> {
    let (format, positional) = cli_args::split_format(rest, "version")?;
    if let Some(extra) = positional.first() {
        return Err(CliError::UsageOwned(format!(
            "ipe version: unexpected argument `{extra}`"
        )));
    }
    print!("{}", render_version(format, &std::io::stdout()));
    Ok(())
}

/// The one-liner installer URL — the same script the docs' `curl … | sh` install
/// uses. `ipe upgrade` re-runs it to fetch the latest release binary and install
/// it over the current one.
const INSTALL_SH_URL: &str =
    "https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/scripts/install.sh";

/// `ipe upgrade [--dry-run]` — self-update by re-running the release installer.
///
/// Delegates to `tools/scripts/install.sh` (the documented install path): it detects
/// the platform, downloads the matching latest-release binary, and installs it
/// over the current one — the same function and interface as a fresh install.
/// Requires `sh` and `curl` (a POSIX host); `--dry-run` prints the command
/// without running it.
///
/// The installer exits with code 2 when it finds no prebuilt binary for the
/// requested version and platform (a transient condition — the release was
/// tagged but CI is still building the artifacts). That distinct code lets the
/// wrapper surface a clear, actionable message rather than a generic failure.
///
/// # Errors
/// [`CliError::UsageOwned`] on an unexpected argument or a non-POSIX host.
/// [`CliError::UpgradeNoPrebuilt`] when the installer exits 2 (no binary yet).
/// [`CliError::UsageOwned`] when the installer cannot be launched or exits with
/// any other non-zero code.
pub fn run_upgrade(rest: &[String]) -> Result<(), CliError> {
    let mut dry_run = false;
    for arg in rest {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            other => {
                return Err(CliError::UsageOwned(format!(
                    "upgrade: unexpected argument `{other}` (usage: ipe upgrade [--dry-run])"
                )));
            }
        }
    }

    let command = format!("curl -fsSL {INSTALL_SH_URL} | sh");
    if dry_run {
        print!(
            "{}",
            style::frame(&style::gutter(&format!("would run: {command}")))
        );
        return Ok(());
    }
    if cfg!(not(unix)) {
        return Err(CliError::UsageOwned(format!(
            "upgrade: not supported on this platform — run the installer manually:\n  {command}"
        )));
    }

    // Render the hand-off to the installer as a stage on stderr: a running
    // light-yellow line while we spawn `sh`, settled to a green success (or a
    // red failure) BEFORE the child inherits the terminal, so the installer's
    // own staged output begins on a fresh, uncorrupted line.
    let stage = progress::Stage::start(std::io::stderr(), "Launching the release installer…");
    let child = std::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .spawn();
    let mut child = match child {
        Ok(child) => {
            stage.success("Installer launched — following its progress below.");
            child
        }
        Err(e) => {
            stage.failure(format!(
                "Could not launch the installer (needs `sh` and `curl`): {e}"
            ));
            return Err(CliError::UsageOwned(format!(
                "upgrade: cannot launch the installer (needs `sh` and `curl`): {e}"
            )));
        }
    };
    let status = child.wait().map_err(|e| {
        CliError::UsageOwned(format!(
            "upgrade: the installer could not be waited on: {e}"
        ))
    })?;
    if status.success() {
        return Ok(());
    }
    // Exit code 2: the installer found no prebuilt binary for the requested
    // version and platform. Report it as a typed, operational failure — NOT
    // misuse — so the caller skips the `--help` page.
    if status.code() == Some(2) {
        // The installer already printed the platform/version details; supply
        // the same fields the Display impl needs so the Rust-side message is
        // self-contained regardless of whether the script output was captured.
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let platform = format!(
            "{}-{}",
            match os {
                "linux" => "linux",
                "macos" => "darwin",
                "freebsd" => "freebsd",
                "windows" => "windows",
                other => other,
            },
            match arch {
                "x86_64" => "x64",
                "aarch64" => "arm64",
                other => other,
            }
        );
        // The version is not known here (the installer resolves it); use the
        // running binary's version as the best available proxy.
        let version = format!("v{}", env!("CARGO_PKG_VERSION"));
        return Err(CliError::UpgradeNoPrebuilt { version, platform });
    }
    Err(CliError::UsageOwned(
        "upgrade: the installer exited non-zero — nothing was changed".to_owned(),
    ))
}

/// Render the ipe version in the requested [`OutputFormat`].
///
/// - Human (default): a guttered `ipe <version>` line.
/// - `--plain`: the bare version string, flush-left, nothing else.
/// - `--json`: `{"version": "<x.y.z>"}`, a stable single-field object.
fn render_version(format: cli_args::OutputFormat, _stream: &impl std::io::IsTerminal) -> String {
    use cli_args::OutputFormat::{Human, Json, Plain};
    let version = env!("CARGO_PKG_VERSION");
    match format {
        Plain => format!("{version}\n"),
        Json => format!("{{\"version\":{version:?}}}\n"),
        Human => style::frame(&style::gutter(&format!("ipe {version}\n"))),
    }
}

/// Verify a declared capability set equals the set inferred from `entry`.
///
/// Returns `Ok(())` iff `declared` is exactly the inferred set. Otherwise a
/// [`CliError::CapabilityMismatch`] naming the capabilities used but not
/// declared and those declared but not used. This is the primitive SP2 (manifest
/// generation) and SP4 (sandbox configuration) consume to reject a drifted or
/// under-declared manifest.
///
/// # Errors
/// [`CliError::Pipeline`] / [`CliError::Io`] when `entry` cannot be lowered, or
/// [`CliError::CapabilityMismatch`] on a set mismatch.
pub fn verify_capabilities(
    entry: &Path,
    declared: &std::collections::BTreeSet<ipe_ir::Capability>,
) -> Result<(), CliError> {
    let program = lower_entry(entry)?;
    let inferred = ipe_lower::program_capabilities(&program);
    if *declared == inferred {
        return Ok(());
    }
    let missing: Vec<&'static str> = inferred.difference(declared).map(|c| c.as_str()).collect();
    let extra: Vec<&'static str> = declared.difference(&inferred).map(|c| c.as_str()).collect();
    Err(CliError::CapabilityMismatch { missing, extra })
}

/// The security capabilities a whole PACKAGE exercises — the union over every
/// module the package ships, not just the entry's reachability closure.
///
/// A single-entry program's capability set is its entry's reachable kernels
/// ([`verify_capabilities`]). A publishable package is different: a downstream
/// consumer can `import` ANY exposed module, so a sibling module that makes a
/// network call is a real capability of the package even when the package's own
/// `Main` never reaches it. The declared `[capabilities]` set the index records
/// is the consumer's consent surface, so it must cover the whole shipped surface
/// — the same whole-tree posture the enforced-semver check already takes over the
/// package's public API.
///
/// This lowers each discovered module in turn (with every sibling source present,
/// so cross-module imports resolve) and unions their inferred capabilities. A
/// module that fails to lower on its own — e.g. one that is only meaningful as a
/// dependency of another — is skipped for the union rather than failing the whole
/// inference, so a helper module never masks a sibling's real effect.
///
/// # Errors
/// [`CliError::Pipeline`] / [`CliError::Io`] when the package cannot be read or
/// no module lowers at all.
pub fn infer_package_capabilities(
    manifest_path: &Path,
) -> Result<std::collections::BTreeSet<ipe_ir::Capability>, CliError> {
    let manifest = project::parse_manifest(manifest_path)?;
    let mut discovered = project::discover_modules(&manifest.src_root)?;

    // Read every module's source once; the shared map lets each per-module
    // lowering resolve its sibling imports.
    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    for m in &discovered {
        let src = fs::read_to_string(&m.path).map_err(|e| io_err(&m.path, e))?;
        sources.insert(m.module_path.clone(), (m.path.clone(), src));
    }

    // Inject the compiled-source stdlib closure (e.g. `Ipe.Css`) just like the
    // real build path, so a module that imports a compiled-source stdlib module
    // lowers standalone here instead of failing name resolution (which, since a
    // failing entry surfaces its real diagnostic, would otherwise abort build).
    let injected = project::inject_compiled_std_closure(&mut sources, &mut discovered);
    // Inject the FFI interface modules (installed crates + the asserted-call
    // `Rust.Ffi` module) exactly as the build does, so an FFI-using module
    // lowers here and its `native-ffi`/`ffi-raw` capabilities are inferred
    // rather than the whole module being skipped on a resolve failure.
    let ffi_injected = ffi::prepare_ffi(&mut sources, manifest_path)?.injected;
    let mut inferred: std::collections::BTreeSet<ipe_ir::Capability> =
        std::collections::BTreeSet::new();
    let mut any_lowered = false;
    // When nothing lowers, the entry module's real diagnostic is far more useful
    // than a generic "nothing lowered". Keep the best candidate to surface: the
    // entry module `Main` if it fails, otherwise the first failure seen.
    let mut lowering_error: Option<CliError> = None;

    // Lower each module as its own entry (a fresh database per module keeps the
    // interning deterministic and the borrow of the shared interner scoped). A
    // module that does not lower standalone is skipped, never fatal — its
    // capabilities, if any, surface through whichever sibling does reach it.
    for m in &discovered {
        let db = ipe_db::IpeDatabase::new();
        let source_root = create_source_root(&db, &sources, &injected, &ffi_injected);
        let Some(entry_file) = source_root.files(&db).get(&m.module_path).copied() else {
            continue;
        };
        match ipe_db::lower_program(&db, source_root, entry_file) {
            Ok(program) => {
                inferred.extend(ipe_lower::program_capabilities(&program));
                any_lowered = true;
            }
            Err((diag, _)) => {
                let is_entry = m.module_path.last().map(String::as_str) == Some("Main");
                if lowering_error.is_none() || is_entry {
                    let src = sources
                        .get(&m.module_path)
                        .map(|(_, s)| s.clone())
                        .unwrap_or_default();
                    lowering_error = Some(CliError::Pipeline {
                        file: m.path.clone(),
                        src,
                        diag: Box::new(diag),
                    });
                }
            }
        }
    }

    if any_lowered {
        Ok(inferred)
    } else {
        // Surface the real reason the entry could not be lowered, not a generic
        // "nothing lowered" that hides the actual compiler diagnostic.
        Err(lowering_error.unwrap_or(CliError::Usage(
            "package capability inference: no module in the package could be lowered",
        )))
    }
}

// ===========================================================================
// `fix` / `--fix` — apply machine-applicable suggestions
// ===========================================================================

/// Run the front of the pipeline (parse → canon → types → lower) and return the
/// first diagnostic it raises, or `None` when the program compiles cleanly.
fn pipeline_first_diagnostic(source: &str) -> Option<Diagnostic> {
    let mut interner = Interner::new();
    let module = match ipe_parse::parse_module(source, &mut interner) {
        Ok(m) => m,
        Err(d) => return Some(d),
    };
    let canonical = match ipe_canon::canonicalise(&module, &mut interner) {
        Ok(c) => c,
        Err(d) => return Some(d),
    };
    let types = match ipe_types::infer(&canonical, &mut interner) {
        Ok(t) => t,
        Err(d) => return Some(d),
    };
    // `--fix` diagnostic probe: single source, home is irrelevant — take just
    // the diagnostic.
    ipe_lower::lower(&canonical, &types, &mut interner)
        .err()
        .map(|(diag, _home)| diag)
}

/// Collect every [`Applicability::MachineApplicable`] suggestion a diagnostic
/// carries — the only kind eligible for auto-patch.
fn machine_applicable_suggestions(diag: &Diagnostic) -> Vec<Suggestion> {
    diag.help()
        .into_iter()
        .filter_map(|line| match line {
            HelpLine::Suggest(s) if s.applicability == Applicability::MachineApplicable => Some(s),
            _ => None,
        })
        .collect()
}

/// Validate spans against `src_len` and keep a non-overlapping subset, ordered
/// back-to-front (largest `lo` first) so applying them never shifts a
/// not-yet-applied span.
#[must_use]
pub fn select_non_overlapping(mut suggestions: Vec<Suggestion>, src_len: usize) -> Vec<Suggestion> {
    let limit = u32::try_from(src_len).unwrap_or(u32::MAX);
    suggestions.retain(|s| s.span.lo <= s.span.hi && s.span.hi <= limit);
    suggestions.sort_by(|a, b| {
        b.span
            .lo
            .cmp(&a.span.lo)
            .then_with(|| b.span.hi.cmp(&a.span.hi))
    });
    let mut kept: Vec<Suggestion> = Vec::new();
    // Lowest `lo` retained so far; the next (further-left) span must end at or
    // before it to avoid overlapping a span we already chose.
    let mut floor = u32::MAX;
    for s in suggestions {
        if s.span.hi <= floor {
            floor = s.span.lo;
            kept.push(s);
        }
    }
    kept
}

/// Apply `fixes` to `src`, returning the patched text.
///
/// `fixes` are assumed non-overlapping and ordered back-to-front. Returns `None`
/// if any span is out of bounds or not on a UTF-8 char boundary. Never indexes
/// raw bytes.
#[must_use]
pub fn apply_fixes(src: &str, fixes: &[Suggestion]) -> Option<String> {
    let mut out = src.to_owned();
    for s in fixes {
        let lo = usize::try_from(s.span.lo).ok()?;
        let hi = usize::try_from(s.span.hi).ok()?;
        if lo > hi || hi > out.len() || !out.is_char_boundary(lo) || !out.is_char_boundary(hi) {
            return None;
        }
        let before = out.get(..lo)?;
        let after = out.get(hi..)?;
        let mut next = String::with_capacity(before.len() + s.replacement.len() + after.len());
        next.push_str(before);
        next.push_str(&s.replacement);
        next.push_str(after);
        out = next;
    }
    Some(out)
}

/// 1-based `(line, column)` of a byte `offset` into `src`, counting columns in
/// characters. Clamps gracefully — never panics.
fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, ch) in src.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line = line.saturating_add(1);
            col = 1;
        } else {
            col = col.saturating_add(1);
        }
    }
    (line, col)
}

/// The fix command/flow: read `entry`, run the pipeline, and apply the
/// machine-applicable suggestions of the first diagnostic.
///
/// `auto` (set by `--yes` / `--fix`) is durable authorization to apply every
/// edit without prompting; otherwise each edit is confirmed interactively on
/// stdin. The patch is never silent: every applied or skipped edit is reported
/// on `w`. The patched text is re-parsed before it replaces the file, and a
/// result that no longer parses is rejected (the file is left untouched).
///
/// Writes go through a temp file + atomic rename.
///
/// # Errors
/// Returns [`CliError::Io`] on a filesystem failure.
fn apply_fixes_cmd<W: Write>(entry: &Path, auto: bool, w: &mut W) -> Result<(), CliError> {
    let source = fs::read_to_string(entry).map_err(|e| io_err(entry, e))?;

    let Some(diag) = pipeline_first_diagnostic(&source) else {
        writeln!(
            w,
            "fix: nothing to do — {} compiles cleanly",
            entry.display()
        )
        .map_err(|e| io_err(entry, e))?;
        return Ok(());
    };

    let candidates = machine_applicable_suggestions(&diag);
    let selected = select_non_overlapping(candidates, source.len());
    if selected.is_empty() {
        writeln!(
            w,
            "fix: no machine-applicable suggestions for {} [{}]",
            entry.display(),
            diag.code().as_str()
        )
        .map_err(|e| io_err(entry, e))?;
        return Ok(());
    }

    let mut chosen: Vec<Suggestion> = Vec::new();
    for s in &selected {
        let lo = usize::try_from(s.span.lo).unwrap_or(usize::MAX);
        let hi = usize::try_from(s.span.hi).unwrap_or(usize::MAX);
        let original = source.get(lo..hi).unwrap_or("");
        let (line, col) = line_col(&source, lo);
        if auto {
            writeln!(
                w,
                "fix: replacing `{original}` with `{}` at {}:{line}:{col}",
                s.replacement,
                entry.display()
            )
            .map_err(|e| io_err(entry, e))?;
            chosen.push(s.clone());
        } else {
            write!(
                w,
                "Replace `{original}` with `{}` at {}:{line}:{col}? [y/N] ",
                s.replacement,
                entry.display()
            )
            .map_err(|e| io_err(entry, e))?;
            w.flush().map_err(|e| io_err(entry, e))?;
            if read_yes_no() {
                chosen.push(s.clone());
            }
        }
    }

    if chosen.is_empty() {
        writeln!(w, "fix: no edits applied").map_err(|e| io_err(entry, e))?;
        return Ok(());
    }

    let Some(patched) = apply_fixes(&source, &chosen) else {
        writeln!(
            w,
            "fix: internal span mismatch — file left unchanged (please report)"
        )
        .map_err(|e| io_err(entry, e))?;
        return Ok(());
    };

    // Re-parse guard: refuse to keep a patch whose result no longer parses.
    let mut guard_interner = Interner::new();
    if ipe_parse::parse_module(&patched, &mut guard_interner).is_err() {
        writeln!(
            w,
            "fix: patched source no longer parses — rolled back, file left unchanged"
        )
        .map_err(|e| io_err(entry, e))?;
        return Ok(());
    }

    write_atomic(entry, &patched)?;
    writeln!(
        w,
        "fix: applied {} edit(s) to {}",
        chosen.len(),
        entry.display()
    )
    .map_err(|e| io_err(entry, e))?;
    Ok(())
}

/// Read a line from stdin and interpret it as a yes/no answer. EOF or any read
/// error is treated as "no" (the safe default for a mutating action).
pub(crate) fn read_yes_no() -> bool {
    read_yes_no_default(false)
}

/// Read a line from stdin and interpret it as a yes/no answer, taking `default`
/// when the answer is empty (a bare Enter). An explicit `y`/`yes` or `n`/`no`
/// overrides the default; EOF or any read error takes the default, so the caller
/// controls the fail-safe direction (default `false` for a mutating action).
pub(crate) fn read_yes_no_default(default: bool) -> bool {
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(_) => {
            let a = line.trim();
            if a.is_empty() {
                default
            } else {
                a.eq_ignore_ascii_case("y") || a.eq_ignore_ascii_case("yes")
            }
        }
        Err(_) => default,
    }
}

/// Write `contents` to `target` atomically: write a sibling temp file, then
/// rename it over `target` (atomic on a single filesystem). On a rename
/// failure the temp file is removed so no debris is left behind.
///
/// Retries ONCE, recreating `target`'s parent directory, when the write or
/// rename fails with `NotFound`. This closes a real race surfaced by the
/// emit→cargo bridge (`reconcile_emitted_project`, this function's
/// other caller besides `ipe fix`): several `crates/ipe/tests/
/// golden_*` integration-test files share ONE `CARGO_TARGET_TMPDIR`-rooted
/// output directory across sibling `#[test]` functions, and `cargo-nextest`
/// runs each test as its own process — so one test's `remove_dir_all` +
/// rebuild can delete a directory this function is mid-write into. A single
/// retry recovers from that transient case; a genuinely permanent failure
/// (permissions, a disallowed ancestor) still surfaces as an error after the
/// retry.
fn write_atomic(target: &Path, contents: &str) -> Result<(), CliError> {
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty());
    let name = target.file_name().map_or_else(
        || String::from("source.ipe"),
        |n| n.to_string_lossy().into_owned(),
    );
    let tmp_name = format!(".{name}.ipec-fix.{}.tmp", std::process::id());
    let tmp = match dir {
        Some(d) => d.join(tmp_name),
        None => PathBuf::from(tmp_name),
    };

    match write_and_rename(&tmp, target, contents) {
        Ok(()) => Ok(()),
        Err(CliError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            if let Some(d) = dir {
                fs::create_dir_all(d).map_err(|e| io_err(d, e))?;
            }
            write_and_rename(&tmp, target, contents)
        }
        Err(e) => Err(e),
    }
}

/// Write `contents` to `tmp`, then rename it over `target`. On a rename
/// failure the temp file is removed so no debris is left behind.
fn write_and_rename(tmp: &Path, target: &Path, contents: &str) -> Result<(), CliError> {
    fs::write(tmp, contents).map_err(|e| io_err(tmp, e))?;
    if let Err(e) = fs::rename(tmp, target) {
        let _ = fs::remove_file(tmp);
        return Err(io_err(target, e));
    }
    Ok(())
}

pub(crate) fn io_err(path: &Path, source: std::io::Error) -> CliError {
    CliError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Extract the source span from a diagnostic, returning [`ipe_diagnostics::Span::DUMMY`]
/// for the span-less [`Diagnostic::CompilerBug`] variant.
///
/// Used by the cross-module error-attribution path in [`compile_modules`] to
/// locate the source file that owns a diagnostic.
const fn diag_span(d: &Diagnostic) -> ipe_diagnostics::Span {
    match d {
        Diagnostic::Parse { span, .. }
        | Diagnostic::Name { span, .. }
        | Diagnostic::Type { span, .. }
        | Diagnostic::Lower { span, .. } => *span,
        Diagnostic::CompilerBug { .. } => ipe_diagnostics::Span::DUMMY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipe_diagnostics::{NameError, Span};

    /// `read_progress_chunk` stops at a newline OR a carriage return, so cargo's
    /// in-place progress bar (which uses `\r` with no `\n`) surfaces live rather
    /// than buffering until the next line, and it drains a stream with no final
    /// terminator without dropping bytes.
    #[test]
    fn read_progress_chunk_stops_at_newline_or_carriage_return() {
        use std::io::BufReader;
        // A `\r` progress frame, then a `\n` message line, then a trailing chunk
        // with no terminator at end of stream.
        let input = "  Building [==>   ]\r   Compiling ipe-app\ndone";
        let mut reader = BufReader::new(input.as_bytes());
        let mut out = String::new();

        let n1 = read_progress_chunk(&mut reader, &mut out).expect("read frame");
        assert_eq!(out, "  Building [==>   ]\r");
        assert_eq!(n1, out.len());

        out.clear();
        read_progress_chunk(&mut reader, &mut out).expect("read line");
        assert_eq!(out, "   Compiling ipe-app\n");

        out.clear();
        read_progress_chunk(&mut reader, &mut out).expect("read tail");
        assert_eq!(out, "done");

        // End of stream returns zero and leaves `out` empty.
        out.clear();
        assert_eq!(
            read_progress_chunk(&mut reader, &mut out).expect("read eof"),
            0
        );
        assert!(out.is_empty());
    }

    /// `missing_runtime_feature` pulls the feature name out of `cargo`'s
    /// feature-resolution error, whether the name is backtick- or single-quoted,
    /// and yields `None` for an unrelated failure.
    #[test]
    fn extracts_missing_runtime_feature() {
        let backtick = "package `ipe-app` depends on `ipe-runtime-rust` with feature `regex` \
             but `ipe-runtime-rust` does not have that feature.";
        assert_eq!(missing_runtime_feature(backtick).as_deref(), Some("regex"));
        let single = "package `ipe-app` depends on ipe-runtime-rust with feature 'random' \
             but ipe-runtime-rust does not have that feature";
        assert_eq!(missing_runtime_feature(single).as_deref(), Some("random"));
        assert_eq!(
            missing_runtime_feature("error: linking with `cc` failed: exit status: 1"),
            None
        );
    }

    /// A cargo build failure whose stderr names a missing runtime feature renders
    /// a targeted, actionable diagnostic that names the feature and the stale
    /// runtime — and never the `run` command's `--help` page.
    #[test]
    fn emitted_build_failure_reports_missing_feature() {
        let err = CliError::EmittedBuildFailed {
            what: "the emitted program",
            code: 101,
            stderr: "package `ipe-app` depends on `ipe-runtime-rust` with feature `regex` \
                 but `ipe-runtime-rust` does not have that feature."
                .to_owned(),
            runtime: Some(RuntimeContext {
                root: PathBuf::from("/tmp/rt"),
                version: "0.1.34".to_owned(),
            }),
        };
        let rendered = err.to_string();
        assert!(rendered.contains("runtime feature `regex`"), "{rendered}");
        assert!(rendered.contains("/tmp/rt"), "{rendered}");
        assert!(rendered.contains("out of date"), "{rendered}");
        assert!(
            !rendered.contains("ipe run [<path>]"),
            "the build failure must not print the run help page: {rendered}"
        );
    }

    /// A cargo build failure that is not a feature gap renders the trimmed cargo
    /// error under a plain header, still without any help page.
    #[test]
    fn emitted_build_failure_reports_generic_cargo_error() {
        let err = CliError::EmittedBuildFailed {
            what: "the emitted program",
            code: 101,
            stderr: "error[E0425]: cannot find value `x` in this scope".to_owned(),
            runtime: None,
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("building the emitted program failed"),
            "{rendered}"
        );
        assert!(rendered.contains("cannot find value"), "{rendered}");
        assert!(!rendered.contains("ipe run [<path>]"), "{rendered}");
    }

    /// The golden entry, located relative to this crate's manifest.
    fn golden_entry() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("golden")
            .join("basics")
            .join("Main.ipe")
    }

    /// Drift-closed proof: every entry in `ALL_CODES` resolves via `explain_lookup`.
    /// If any code is in the taxonomy but missing from `ALL_CODES` this test fails.
    #[test]
    fn all_taxonomy_codes_resolve_via_explain_lookup() {
        for &c in ALL_CODES {
            let result = explain_lookup(c.as_str());
            assert!(
                result.is_ok(),
                "{} is in ALL_CODES but explain_lookup returned: {:?}",
                c.as_str(),
                result.err()
            );
        }
    }

    #[test]
    fn explain_resolves_a_known_code() {
        let page = explain_lookup("IPE-T0001");
        assert!(page.is_ok(), "known code must resolve: {:?}", page.err());
        let Ok(page) = page else { return };
        assert!(
            page.starts_with("# IPE-T0001:"),
            "page line 1 must name the code, got:\n{page}"
        );
    }

    #[test]
    fn explain_is_case_insensitive() {
        assert!(explain_lookup("ipe-t0001").is_ok());
        assert!(explain_lookup("  Ipe-T0001  ").is_ok());
    }

    #[test]
    fn explain_resolves_ipe_t0014() {
        // IPE-T0014 resolves via ALL_CODES from ipe_diagnostics rather than
        // a hand-mirror that could omit it.
        let result = explain_lookup("IPE-T0014");
        assert!(
            result.is_ok(),
            "IPE-T0014 must resolve via ALL_CODES: {:?}",
            result.err()
        );
    }

    #[test]
    fn explain_rejects_unknown_code_with_suggestions() {
        // Genuinely unknown code, close to IPE-T0013 — must yield did-you-mean.
        let result = explain_lookup("IPE-T0099");
        assert!(
            matches!(&result, Err(CliError::UnknownCode { .. })),
            "unknown code must error, got: {result:?}"
        );
        let Err(CliError::UnknownCode { suggestions, .. }) = result else {
            return;
        };
        assert!(
            !suggestions.is_empty(),
            "a near-miss must yield did-you-mean suggestions"
        );
    }

    #[test]
    fn explain_unknown_code_display_is_deterministic() {
        let err = CliError::UnknownCode {
            input: "IPE-Z9999".to_owned(),
            suggestions: vec!["IPE-T0001", "IPE-T0002"],
        };
        assert_eq!(
            err.to_string(),
            "unknown error code `IPE-Z9999`\n  did you mean: IPE-T0001, IPE-T0002?"
        );
    }

    #[test]
    fn explain_output_ends_with_trailing_newline() {
        // `ipe explain <CODE>` does `print!("{page}")`, so the page itself must
        // end with a newline to avoid a missing newline at the shell prompt.
        let page = explain_lookup("IPE-T0001").expect("known code must resolve");
        assert!(
            page.ends_with('\n'),
            "explain output must end with a trailing newline; got: {:?}",
            &page[page.len().saturating_sub(20)..]
        );
    }

    #[test]
    fn code_index_lists_every_code() {
        let index = code_index();
        let lines = index.lines().count();
        assert_eq!(lines, ALL_CODES.len(), "one line per code");
        assert!(
            index.contains("IPE-T0001  type mismatch"),
            "index pairs code with title"
        );
    }

    #[test]
    fn emit_ir_prints_a_tree_for_the_golden() {
        let tree = emit_ir_text(&golden_entry());
        assert!(
            tree.is_ok(),
            "emit-ir must succeed: {:?}",
            tree.as_ref().err()
        );
        let Ok(tree) = tree else { return };
        assert!(
            tree.starts_with("program"),
            "tree roots at `program`:\n{tree}"
        );
        assert!(tree.contains("main"), "tree names the `main` func:\n{tree}");
    }

    /// A program importing a compiled-source stdlib module that defines its own
    /// types (`Ipe.Test`) must resolve its qualified members through the CLI
    /// analysis path (`ipe build --emit-ir` / `ipe capabilities`), exactly as it
    /// does through a real `ipe build`. Both share the injection-aware
    /// source-graph pipeline: the analysis path once ran a bare single-module
    /// lower that never injected the closure, so `Test.runMain` / `Test.equal`
    /// failed with IPE-N0004 "unknown module `Test`" here while the build
    /// succeeded. This pins the CLI<->build parity for compiled-source-with-types
    /// modules so the divergence cannot return.
    #[test]
    fn emit_ir_resolves_compiled_source_stdlib_with_own_types() {
        let entry = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("golden")
            .join("test_summary_line_219")
            .join("Main.ipe");
        let tree = emit_ir_text(&entry);
        assert!(
            tree.is_ok(),
            "emit-ir must resolve `Ipe.Test` (no IPE-N0004): {:?}",
            tree.as_ref().err()
        );
        let Ok(tree) = tree else { return };
        // The injected compiled-source module's OWN types + members are present
        // — proof the closure was injected, not merely that the diagnostic was
        // silenced.
        assert!(
            tree.contains("type TestResult"),
            "injected `Ipe.Test` types must appear in the IR:\n{tree}"
        );
        assert!(
            tree.contains("runMain"),
            "`Test.runMain` must resolve to the injected member:\n{tree}"
        );

        // The same source-graph pipeline backs `ipe capabilities` via
        // `lower_entry`; it must resolve identically (a pure test program).
        assert!(
            lower_entry(&entry).is_ok(),
            "lower_entry (capabilities path) must resolve `Ipe.Test` too"
        );
    }

    /// A compiled-source stdlib module that imports a kernel stdlib module inside
    /// its own body must not fire IPE-N0034 on those imports.  `Ipe.Money`
    /// imports `Ipe.String` (a kernel module) and uses `String.*` members
    /// throughout; the Tier-C import gate must see those imports as satisfied
    /// when the embedded source is injected and canonicalised.
    ///
    /// The `money_parse_currency_maybe` golden exercises `Money.currencyCode`
    /// (which calls `String.*` internally), making it the ideal witness.
    #[test]
    fn compiled_source_stdlib_own_imports_resolve_no_n0034() {
        let entry = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("golden")
            .join("money_parse_currency_maybe")
            .join("Main.ipe");
        let tree = emit_ir_text(&entry);
        assert!(
            tree.is_ok(),
            "emit-ir must resolve `Ipe.Money` (no IPE-N0034 inside the embedded module): {:?}",
            tree.as_ref().err()
        );
        let Ok(tree) = tree else { return };
        // The injected module's types must appear — proof the closure was injected,
        // not merely that the diagnostic was silenced at a shallower stage.
        assert!(
            tree.contains("Money") || tree.contains("currency"),
            "injected `Ipe.Money` members must appear in the IR:\n{tree}"
        );
    }

    #[test]
    fn machine_applicable_suggestion_is_collected_and_applied() {
        let src = "main = lenght";
        // `lenght` occupies bytes 7..13.
        let diag = Diagnostic::Name {
            span: Span::new(7, 13),
            msg: NameError::ValueNotFound {
                name: "lenght".into(),
                suggestions: Box::new(["length".into()]),
            },
        };
        let fixes = machine_applicable_suggestions(&diag);
        assert_eq!(fixes.len(), 1, "single candidate is machine-applicable");
        let selected = select_non_overlapping(fixes, src.len());
        let patched = apply_fixes(src, &selected);
        assert_eq!(patched.as_deref(), Some("main = length"));
    }

    #[test]
    fn overlapping_suggestions_are_filtered_back_to_front() {
        let left = Suggestion {
            span: Span::new(0, 5),
            replacement: "x".into(),
            applicability: Applicability::MachineApplicable,
        };
        let right = Suggestion {
            span: Span::new(3, 8),
            replacement: "y".into(),
            applicability: Applicability::MachineApplicable,
        };
        let kept = select_non_overlapping(vec![left, right], 8);
        assert_eq!(kept.len(), 1, "overlapping spans collapse to one");
        // Back-to-front: the right-most (larger lo) span survives.
        assert_eq!(kept.first().map(|s| s.span.lo), Some(3));
    }

    #[test]
    fn apply_fixes_rejects_out_of_bounds_span() {
        let s = Suggestion {
            span: Span::new(0, 999),
            replacement: "z".into(),
            applicability: Applicability::MachineApplicable,
        };
        assert_eq!(apply_fixes("short", &[s]), None);
    }

    #[test]
    fn apply_fixes_rejects_non_char_boundary_span() {
        // "é" is two UTF-8 bytes; a span that splits it is rejected.
        let s = Suggestion {
            span: Span::new(0, 1),
            replacement: "z".into(),
            applicability: Applicability::MachineApplicable,
        };
        assert_eq!(apply_fixes("é", &[s]), None);
    }

    #[test]
    fn levenshtein_is_symmetric_and_zero_on_equal() {
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("abc", "abd"), 1);
        assert_eq!(levenshtein("abc", "abd"), levenshtein("abd", "abc"));
    }

    #[test]
    fn line_col_counts_from_one() {
        let src = "ab\ncd";
        assert_eq!(line_col(src, 0), (1, 1));
        assert_eq!(line_col(src, 1), (1, 2));
        assert_eq!(line_col(src, 3), (2, 1));
        assert_eq!(line_col(src, 4), (2, 2));
    }

    /// Generic records, end to end from SOURCE: parse → canon → infer → lower →
    /// emit → `cargo build` → run, asserting the program prints `42` — the value
    /// the Go reference backend produces for the same program (hand-verified in a
    /// temp dir). Gated on `IPE_E2E=1` so the default `cargo test` stays fast and
    /// offline. Complements the backend crate's hand-built-IR e2e by exercising
    /// the whole frontend (record type annotations + generalisation + lowering).
    #[test]
    fn generic_record_program_builds_and_prints_forty_two() {
        const SRC: &str = "module Main exposing (main)\n\n\
             import Ipe.Io\n\
             import Ipe.String\n\n\
             wrap : a -> { value : a }\n\
             wrap x =\n    { value = x }\n\n\
             unwrap : { value : a } -> a\n\
             unwrap r =\n    r.value\n\n\
             main = Io.println (String.fromInt (unwrap (wrap 42)))\n";

        if std::env::var("IPE_E2E").is_err() {
            return;
        }

        let dir = std::env::temp_dir().join("ipec_generic_record_src_e2e");
        let _ = fs::remove_dir_all(&dir);
        let entry = dir.join("Main.ipe");
        let created = fs::create_dir_all(&dir).and_then(|()| fs::write(&entry, SRC));
        assert!(created.is_ok(), "write source: {created:?}");

        let runtime = resolve_runtime();
        assert!(runtime.is_ok(), "runtime must resolve: {runtime:?}");
        let Ok(runtime) = runtime else { return };

        let out = dir.join("out");
        let built = build(&entry, &out, &runtime);
        assert!(built.is_ok(), "ipe build must succeed: {built:?}");

        let status = std::process::Command::new("cargo")
            .arg("build")
            .current_dir(&out)
            .env("CARGO_TARGET_DIR", out.join("target"))
            .status();
        assert!(
            matches!(&status, Ok(s) if s.success()),
            "emitted generic-record crate must compile: {status:?}"
        );

        let bin = out.join("target").join("debug").join("ipe-app");
        let run = std::process::Command::new(&bin).output();
        let Ok(run) = run else {
            assert!(false_marker(), "run binary: {run:?}");
            return;
        };
        assert_eq!(
            String::from_utf8_lossy(&run.stdout),
            "42\n",
            "generic-record program prints 42 (Go-backend parity)"
        );
        assert!(run.status.success(), "exit 0, matching the Go oracle");
        let _ = std::fs::remove_dir_all(out.join("target"));
    }

    /// A runtime `false` the optimiser cannot fold, so `assert!(false_marker())`
    /// fails the test without tripping `clippy::assertions_on_constants`.
    fn false_marker() -> bool {
        std::hint::black_box(false)
    }

    // -----------------------------------------------------------------------
    // find_manifest_for_ipe_file tests (IPE-N0020 fix)
    // -----------------------------------------------------------------------

    /// Creates a temp directory with a nested `src/Main.ipe` and a `ipe.toml`
    /// at the project root, confirming the upward walk finds the manifest.
    #[test]
    fn find_manifest_walks_up_to_project_root() {
        let tmp = std::env::temp_dir().join("ipec_find_manifest_test");
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(&src).expect("create src/");
        let toml = tmp.join("ipe.toml");
        fs::write(&toml, "name = \"test\"\n").expect("write ipe.toml");
        let main_ipe = src.join("Main.ipe");
        fs::write(&main_ipe, "module Main exposing (main)\nmain = 0\n").expect("write Main.ipe");

        let found = find_manifest_for_ipe_file(&main_ipe);
        assert_eq!(
            found.as_deref(),
            Some(toml.as_path()),
            "upward walk must find ipe.toml at project root"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // Regression: PAnything (wildcard lambda param with unconstrained Ty::Var)
    // -----------------------------------------------------------------------

    /// Regression for `IPE-L0102` (`Feature::Polymorphism`) on wildcard `_`
    /// lambda parameters.
    ///
    /// Calling `ir_type_from_ty` on the `_` param's type is unsound: when the
    /// type is still an unconstrained `Ty::Var` (e.g. the continuation of a
    /// `Task.andThen` after `Task.fail` where the ok-type is never forced),
    /// `ir_type_from_ty` returns `Err(unsupported(…, Feature::Polymorphism))`
    /// and the pipeline aborts.
    ///
    /// So `PAnything` params route through `ir_type_from_ty_json`, which
    /// maps `Ty::Var → IrType::Json` instead of failing.
    ///
    /// Source mirrors the failing pattern from `examples/14-task-demo`.
    #[test]
    fn panything_wildcard_lambda_compiles_without_polymorphism_error() {
        const SRC: &str = "\
module Main exposing (main)
import Ipe.Task as Task
import Ipe.Error as Error exposing (Error)
import Ipe.Io as Io

main =
    Task.fail (Error.unexpected \"intentional\")
        |> Task.andThen (\\_ -> Task.succeed \"unreachable\")
        |> Task.andThen Io.println
        |> Task.onError (\\e -> Io.println (Error.toString e))
";

        let runtime = resolve_runtime();
        if runtime.is_err() {
            // Runtime not present in this environment — skip rather than fail.
            return;
        }
        let Ok(runtime) = runtime else { return };

        let dir = std::env::temp_dir().join("ipec_panything_regression");
        let _ = fs::remove_dir_all(&dir);
        let entry = dir.join("Main.ipe");
        let created = fs::create_dir_all(&dir).and_then(|()| fs::write(&entry, SRC));
        assert!(created.is_ok(), "write source: {created:?}");

        let out = dir.join("out");
        let result = build(&entry, &out, &runtime);
        assert!(
            result.is_ok(),
            "wildcard lambda with unconstrained type must not fire IPE-L0102: {result:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // Regression: Task.run elision — ipe_main must return IpeTask<A>
    // -----------------------------------------------------------------------

    /// `main` returning a `Task` directly must emit `fn ipe_main() -> IpeTask<`
    /// (the shape the `block_on(ipe_main())` epilogue requires), never
    /// `IpeResult<…>`. The internal `TaskRun` kernel is the auto-run mechanism
    /// at the entry boundary; the surface `Task.run` binding is gone.
    #[test]
    fn task_run_main_emits_ipetask_not_iperesult() {
        const SRC: &str = "\
module Main exposing (main)
import Ipe.Io as Io

main =
    Io.println \"hello from main task\"
";

        let runtime = resolve_runtime();
        if runtime.is_err() {
            return;
        }
        let Ok(runtime) = runtime else { return };

        let dir = std::env::temp_dir().join("ipec_taskrun_elision_regression");
        let _ = fs::remove_dir_all(&dir);
        let entry = dir.join("Main.ipe");
        let created = fs::create_dir_all(&dir).and_then(|()| fs::write(&entry, SRC));
        assert!(created.is_ok(), "write source: {created:?}");

        let out = dir.join("out");
        let built = build(&entry, &out, &runtime);
        assert!(built.is_ok(), "task-returning main must compile: {built:?}");

        let main_rs = out.join("src").join("main.rs");
        let emitted = fs::read_to_string(&main_rs).expect("emitted main.rs must exist after build");

        assert!(
            emitted.contains("fn ipe_main() -> IpeTask<"),
            "ipe_main must return IpeTask<…>, got signature region:\n{}",
            emitted
                .lines()
                .filter(|l| l.contains("ipe_main")
                    || l.contains("IpeTask")
                    || l.contains("IpeResult"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert!(
            !emitted.contains("fn ipe_main() -> IpeResult"),
            "ipe_main must NOT return IpeResult"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// When no ipe.toml exists in any parent directory, returns None.
    #[test]
    fn find_manifest_returns_none_when_absent() {
        let tmp = std::env::temp_dir().join("ipec_no_manifest_test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("create dir");
        let ipe = tmp.join("Standalone.ipe");
        fs::write(&ipe, "module Standalone exposing (f)\nf = 0\n").expect("write ipe");
        // Deliberately no ipe.toml anywhere under tmp.
        // The walk terminates at the filesystem root without finding one.
        // We cannot guarantee the walk terminates before reaching /tmp or /
        // on all systems, so we only assert non-panicking behaviour and that
        // the returned path (if Some) is a real file.
        let found = find_manifest_for_ipe_file(&ipe);
        if let Some(ref p) = found {
            assert!(p.is_file(), "if Some, the manifest must exist on disk");
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Two-module program: `Main.ipe` calls a helper in sibling `Lib.ipe`.
    /// `build_with_sibling_discovery` must compile both without IPE-N0020.
    #[test]
    fn sibling_discovery_compiles_two_module_program() {
        let runtime = resolve_runtime();
        if runtime.is_err() {
            // Runtime not found in this environment (CI without IPE_RUNTIME_DIR) —
            // skip rather than fail: the sweep catches this live.
            return;
        }
        let Ok(runtime) = runtime else { return };

        let tmp = std::env::temp_dir().join("ipec_sibling_disc_test");
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(&src).expect("create src/");

        // Helper module: src/Helper.ipe
        fs::write(
            src.join("Helper.ipe"),
            "module Helper exposing (answer)\nanswer = 42\n",
        )
        .expect("write Helper.ipe");

        // Entry module: src/Main.ipe — imports Helper
        fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nimport Helper\nimport Ipe.Io\nimport Ipe.String\nmain = Io.println (String.fromInt Helper.answer)\n",
        )
        .expect("write Main.ipe");

        let out = tmp.join("out");
        let result = build_with_sibling_discovery(&src.join("Main.ipe"), &out, &runtime);
        assert!(
            result.is_ok(),
            "two-module program must compile via sibling discovery: {:?}",
            result.err()
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    /// `ipe verify`'s test-stage build: a `tests/Main.ipe` that imports a module
    /// living under `src/Lib/` must resolve the `src/` code under test, not fail
    /// with IPE-N0020. This is the standard `src/` + `tests/` layout the naive
    /// entry-parent source root cannot see across.
    #[test]
    fn test_stage_build_resolves_src_modules_from_tests_dir() {
        let runtime = resolve_runtime();
        let Ok(runtime) = runtime else { return };

        let tmp = std::env::temp_dir().join("ipec_verify_test_stage_src_disc");
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        let tests = tmp.join("tests");
        fs::create_dir_all(src.join("Lib")).expect("create src/Lib/");
        fs::create_dir_all(&tests).expect("create tests/");

        // Code under test: src/Lib/Foo.ipe (a multi-segment module referenced
        // via an alias, the way real projects import a nested module).
        fs::write(
            src.join("Lib").join("Foo.ipe"),
            "module Lib.Foo exposing (answer)\nanswer = 42\n",
        )
        .expect("write src/Lib/Foo.ipe");

        // A src entry that also uses the library (mirrors a real project).
        fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nimport Lib.Foo as Foo\nimport Ipe.Io as Io\nimport Ipe.String as String\nmain = Io.println (String.fromInt Foo.answer)\n",
        )
        .expect("write src/Main.ipe");

        // Test entry in the sibling tests/ directory imports the src/ module.
        fs::write(
            tests.join("Main.ipe"),
            "module Main exposing (main)\nimport Lib.Foo as Foo\nimport Ipe.Io as Io\nimport Ipe.String as String\nmain = Io.println (String.fromInt Foo.answer)\n",
        )
        .expect("write tests/Main.ipe");

        let out = tmp.join("out");
        let result = build_test_with_project_sources(&src, &tests.join("Main.ipe"), &out, &runtime);
        assert!(
            result.is_ok(),
            "the test stage must resolve src/ modules from tests/ (no IPE-N0020): {:?}",
            result.err()
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    /// The test-stage source collection unions the `src/` and `tests/` trees
    /// with the correct per-root relativisation: `src/Lib/Foo.ipe` → `Lib.Foo`,
    /// `tests/Main.ipe` → `Main`, and the entry is the test module. This is the
    /// resolution the build depends on, asserted without a runtime.
    #[test]
    fn collect_test_sources_unions_src_and_tests_trees() {
        let tmp = std::env::temp_dir().join("ipec_collect_test_sources_union");
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        let tests = tmp.join("tests");
        fs::create_dir_all(src.join("Lib")).expect("create src/Lib/");
        fs::create_dir_all(&tests).expect("create tests/");
        fs::write(
            src.join("Lib").join("Foo.ipe"),
            "module Lib.Foo exposing (answer)\nanswer = 42\n",
        )
        .expect("write src/Lib/Foo.ipe");
        fs::write(
            tests.join("Main.ipe"),
            "module Main exposing (main)\nimport Lib.Foo as Foo\nmain = Foo.answer\n",
        )
        .expect("write tests/Main.ipe");

        let collected = collect_test_sources(&src, &tests.join("Main.ipe"))
            .expect("collect_test_sources must succeed");

        assert_eq!(
            collected.entry_module_path,
            vec!["Main".to_owned()],
            "the entry is the test module"
        );
        assert!(
            collected
                .sources
                .contains_key(&vec!["Lib".to_owned(), "Foo".to_owned()]),
            "src/Lib/Foo.ipe must be present as Lib.Foo, got keys: {:?}",
            collected.sources.keys().collect::<Vec<_>>()
        );
        assert!(
            collected.sources.contains_key(&vec!["Main".to_owned()]),
            "the test entry must be present as Main"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // Cross-module infer errors name the dep module's file
    // -----------------------------------------------------------------------

    /// When a type error originates in a dep module (`Helper.ipe`), the rendered
    /// diagnostic must cite `Helper.ipe` as the file, NOT the entry `Main.ipe`.
    /// A single `pipeline_err` closure capturing only the entry file path would
    /// render dep-module errors with the wrong source snippet and file name.
    ///
    /// Runtime is not reached (infer aborts first), so we pass a dummy path.
    #[test]
    fn infer_error_in_dep_module_names_dep_file() {
        let tmp = std::env::temp_dir().join("ipec_144_dep_err_test");
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(&src).expect("create src/");

        // Helper.ipe: deliberate type error — `1 + "oops"` mixes Int and String.
        let helper_path = src.join("Helper.ipe");
        fs::write(
            &helper_path,
            "module Helper exposing (broken)\nbroken = 1 + \"oops\"\n",
        )
        .expect("write Helper.ipe");

        // Main.ipe: imports Helper and uses `broken` — but the error is in Helper.
        let main_path = src.join("Main.ipe");
        fs::write(
            &main_path,
            "module Main exposing (main)\nimport Helper\nimport Ipe.Io\nimport Ipe.String\nmain = Io.println (String.fromInt Helper.broken)\n",
        )
        .expect("write Main.ipe");

        // Runtime is never accessed: a type error fires at infer, before lower/emit.
        let dummy_runtime = std::env::temp_dir();
        let out = tmp.join("out");
        let result = build_with_sibling_discovery(&main_path, &out, &dummy_runtime);

        // Must fail — the program has a type error in Helper.
        assert!(
            result.is_err(),
            "#144 fixture must fail (type error in dep); got Ok unexpectedly"
        );
        let Err(CliError::Pipeline { file, .. }) = result else {
            let _ = fs::remove_dir_all(&tmp);
            return; // any other error kind is a separate concern
        };

        // The file blamed must be Helper.ipe, not Main.ipe.
        let file_name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert_eq!(
            file_name,
            "Helper.ipe",
            "#144 regression: type error in dep module must blame `Helper.ipe`, \
             not `{file_name}`; full path: {}",
            file.display()
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // Home-module discriminant — cross-module errors use `home` on Constraint
    // -----------------------------------------------------------------------

    /// Regression test for the home-module span discriminant fix.
    ///
    /// Before this fix the constraint solver emitted bare `Span` values (byte
    /// offsets with no module tag).  After `link::link` merges N modules into
    /// one flat def list, a byte offset like 34 can be numerically contained by
    /// a def from *either* module.  The byte-offset heuristic (`source_for_span`)
    /// picks the closest def, but it can pick the wrong one when two modules have
    /// overlapping numeric span ranges — e.g., a wide def in module A that starts
    /// at byte 20 and a narrow def in module B that starts at byte 30, with the
    /// type error at byte 34.  Both body spans contain byte 34, but A has a
    /// closer `lo_dist` to the wrong def, so the heuristic blames the wrong file
    /// whenever the numerically-nearest def belongs to a different module.
    ///
    /// Every `Constraint` carries its source module's `home` path, so
    /// `compile_modules` routes `Err((diag, home))` directly via
    /// `home_to_source.get(&home)`, bypassing the heuristic entirely when a home
    /// is available.
    ///
    /// This test builds a two-module program where the type error is in module B
    /// (`Lib.ipe`) but the heuristic *could* be fooled by a wide def in module A
    /// (`Pad.ipe`).  The assertion checks that the blamed file is `Lib.ipe`.
    ///
    /// To exercise the home-discriminant path rather than the heuristic, `Pad.ipe`
    /// is constructed so that its def body starts at roughly the same byte offset
    /// as the error in `Lib.ipe` — any byte-offset resolver that ignores the home
    /// would be ambiguous.  The discriminant is the only reliable resolver.
    #[test]
    fn home_discriminant_cross_module_type_error_names_correct_file() {
        let tmp = std::env::temp_dir().join("ipec_home_disc_test");
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(&src).expect("create src/");

        // Pad.ipe: a valid module whose single def body starts at roughly the
        // same byte offset as the type error in Lib.ipe.  Constructed so the
        // body span (a long arithmetic chain) numerically overlaps with Lib's
        // error span.  The body itself is well-typed.
        //
        //   "module Pad exposing (pad)\npad = " is 27 bytes.
        //   The body "1 + 2 + 3 + 4 + 5 + 6 + 7 + 8 + 9" starts at byte 27.
        //   The body ends at byte 27+35 = 62.
        //
        // After link, Pad's def body covers bytes [27, 62] in Pad's namespace.
        fs::write(
            src.join("Pad.ipe"),
            "module Pad exposing (pad)\npad = 1 + 2 + 3 + 4 + 5 + 6 + 7 + 8 + 9\n",
        )
        .expect("write Pad.ipe");

        // Lib.ipe: a module with a deliberate type error at a span that falls
        // numerically inside Pad's body range.
        //
        //   "module Lib exposing (bad)\nbad = " is 27 bytes.
        //   The body "1 + 2 + 3 + 4 + \"oops\"" starts at byte 27.
        //   The type error is at "\"oops\"" = byte 27+20 = 47, inside [27,62].
        //
        // Without the home discriminant, `source_for_span(span=47)` would see
        // BOTH Pad's body [27,62] (lo_dist=20) and Lib's body [27,49] (lo_dist=20)
        // as equally-distanced candidates — and would pick the narrower body, which
        // happens to be Lib here.  But in general (different padding choices) it
        // can pick the wrong one.  The fix makes the home the authoritative signal.
        fs::write(
            src.join("Lib.ipe"),
            "module Lib exposing (bad)\nbad = 1 + 2 + 3 + 4 + \"oops\"\n",
        )
        .expect("write Lib.ipe");

        // Main.ipe: imports both; the error is in Lib, not Main or Pad.
        fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nimport Lib\nimport Pad\nimport Ipe.Io\nimport Ipe.String\nmain = Io.println (String.fromInt Lib.bad)\n",
        )
        .expect("write Main.ipe");

        let dummy_runtime = std::env::temp_dir();
        let out = tmp.join("out");
        let result = build_with_sibling_discovery(&src.join("Main.ipe"), &out, &dummy_runtime);

        // Must fail — type error in Lib.
        assert!(
            result.is_err(),
            "home-discriminant fixture must fail (type error in Lib); got Ok unexpectedly"
        );
        let Err(CliError::Pipeline { file, .. }) = result else {
            let _ = fs::remove_dir_all(&tmp);
            return;
        };

        // The blamed file must be Lib.ipe — the module that OWNS the failing
        // constraint, regardless of which module the byte-offset heuristic
        // would pick.
        let file_name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert_eq!(
            file_name,
            "Lib.ipe",
            "home-discriminant regression: type error in Lib must blame `Lib.ipe`, \
             not `{file_name}`; full path: {}",
            file.display()
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------
    // On-disk build cache end-to-end proof
    // -----------------------------------------------------------------

    /// Walk `cache_root/<epoch>/` and return the single `EmittedProject`-tier
    /// entry (`<key>.json`) a fresh build just wrote. The epoch name is
    /// unpredictable from a test's perspective (it folds in the running binary's
    /// own content hash), so this has to search rather than construct the path
    /// directly. The co-resident IR tier writes `<key>.ir.json` under the same
    /// epoch dir — that file's extension is also `json`, so it is excluded by
    /// name to keep this matcher pinned to the `EmittedProject` tier.
    fn find_single_cache_entry(cache_root: &Path) -> Option<PathBuf> {
        for epoch_entry in fs::read_dir(cache_root).ok()?.flatten() {
            let epoch_dir = epoch_entry.path();
            if !epoch_dir.is_dir() {
                continue;
            }
            for file_entry in fs::read_dir(&epoch_dir).ok()?.flatten() {
                let path = file_entry.path();
                let is_json = path.extension().and_then(std::ffi::OsStr::to_str) == Some("json");
                let is_ir_tier = path
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|n| n.ends_with(".ir.json"));
                if is_json && !is_ir_tier {
                    return Some(path);
                }
            }
        }
        None
    }

    /// The end-to-end proof that `compile_modules_observed` actually
    /// CONSULTS and TRUSTS the on-disk cache, not merely that two identical
    /// builds happen to agree (which determinism alone would already give,
    /// without proving the cache was read at all).
    ///
    /// Strategy: compile once (a genuine cache miss, populates the cache),
    /// locate the single entry the build just wrote, and TAMPER with its
    /// `cargo_toml` field with a sentinel no fresh compile of the SAME
    /// source could ever produce. Compile again with the SAME inputs and
    /// the SAME cache dir; if the driver reads and trusts the cache, the
    /// second build's `Cargo.toml` carries the sentinel verbatim. If it
    /// silently recompiled instead, the sentinel is gone.
    #[test]
    fn on_disk_cache_hit_serves_a_tampered_entry_verbatim() {
        const SENTINEL: &str = "# CACHE-HIT-SENTINEL\n";

        let Ok(runtime) = resolve_runtime() else {
            return; // No in-repo runtime tree in this environment — see other tests' pattern.
        };

        let tmp = std::env::temp_dir().join(format!("ipe-cache-e2e-{}", std::process::id()));
        let cache_dir = tmp.join("cache");
        let out_a = tmp.join("out-a");
        let out_b = tmp.join("out-b");
        let _ = fs::remove_dir_all(&tmp);

        let entry_path = vec!["Main".to_owned()];
        let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
        sources.insert(
            entry_path.clone(),
            (
                PathBuf::from("<cache-e2e>/Main.ipe"),
                "module Main exposing (main)\n\nmain = 1\n".to_owned(),
            ),
        );
        let discovered = vec![project::DiscoveredModule {
            path: PathBuf::from("<cache-e2e>/Main.ipe"),
            module_path: entry_path.clone(),
        }];

        let (result_a, outcome_a) = compile_modules_observed(
            sources.clone(),
            discovered.clone(),
            &entry_path,
            &out_a,
            &runtime,
            Path::new("<cache-e2e>"),
            ipe_backend_rust::DbDriver::Sqlite,
            Some(&cache_dir),
            BuildOptions::default(),
        );
        assert!(
            result_a.is_ok(),
            "first (cold) compile must succeed: {:?}",
            result_a.err()
        );
        assert_eq!(
            outcome_a,
            CacheOutcome::Miss,
            "first compile against an empty cache dir must be a miss"
        );

        let entry_json = find_single_cache_entry(&cache_dir)
            .expect("first build must have written exactly one cache entry");
        let stored = fs::read_to_string(&entry_json).expect("cache entry must be readable");
        let mut cached: ipe_backend::EmittedProject =
            serde_json::from_str(&stored).expect("cache entry must deserialize");
        cached.cargo_toml = format!("{SENTINEL}{}", cached.cargo_toml);
        fs::write(
            &entry_json,
            serde_json::to_vec(&cached).expect("re-serialize must succeed"),
        )
        .expect("tamper write must succeed");

        let (result_b, outcome_b) = compile_modules_observed(
            sources,
            discovered,
            &entry_path,
            &out_b,
            &runtime,
            Path::new("<cache-e2e>"),
            ipe_backend_rust::DbDriver::Sqlite,
            Some(&cache_dir),
            BuildOptions::default(),
        );
        assert!(
            result_b.is_ok(),
            "second (cache-hit) compile must succeed: {:?}",
            result_b.err()
        );
        assert_eq!(
            outcome_b,
            CacheOutcome::Hit,
            "second compile with byte-identical inputs must hit the cache"
        );

        let written = fs::read_to_string(out_b.join("Cargo.toml")).expect("Cargo.toml must exist");
        assert!(
            written.starts_with(SENTINEL),
            "materialized output must be the TAMPERED cache entry, not a fresh \
             recompile — proves the driver actually reads and trusts the \
             on-disk cache: {written}"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    /// Walk `cache_root/<epoch>/*.ir.json` and return the single
    /// lowered-IR entry file a build just wrote. Mirrors
    /// [`find_single_cache_entry`], but matches on the `.ir.json` suffix
    /// specifically — `Path::extension()` alone cannot tell `key.json` from
    /// `key.ir.json` apart (both report `json`), so a build that populated
    /// BOTH tiers in the same epoch directory needs the suffix check to
    /// find the right one.
    fn find_single_ir_cache_entry(cache_root: &Path) -> Option<PathBuf> {
        for epoch_entry in fs::read_dir(cache_root).ok()?.flatten() {
            let epoch_dir = epoch_entry.path();
            if !epoch_dir.is_dir() {
                continue;
            }
            for file_entry in fs::read_dir(&epoch_dir).ok()?.flatten() {
                let path = file_entry.path();
                if path
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|n| n.ends_with(".ir.json"))
                {
                    return Some(path);
                }
            }
        }
        None
    }

    /// **End-to-end proof that a `db_driver`-only edit reuses the
    /// lowered-IR tier instead of a full recompile.** The `EmittedProject`
    /// tier's key folds in `db_driver` (a real dependency of the FINAL emit
    /// stage), so it correctly MISSES on a driver flip — but
    /// `linked_program`/`typecheck`/`lower_program` never read `db_driver`
    /// at all, so the SAME lowered `Program` is still exactly reusable. This
    /// is the concrete case the IR tier exists to cover that the
    /// `EmittedProject` tier structurally cannot.
    #[test]
    fn ir_cache_hit_reuses_lowered_program_across_a_db_driver_only_edit() {
        let Ok(runtime) = resolve_runtime() else {
            return;
        };
        let tmp = std::env::temp_dir().join(format!("ipec-ir-cache-driver-{}", std::process::id()));
        let cache_dir = tmp.join("cache");
        let out_a = tmp.join("out-a");
        let out_b = tmp.join("out-b");
        let _ = fs::remove_dir_all(&tmp);

        let entry_path = vec!["Main".to_owned()];
        let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
        sources.insert(
            entry_path.clone(),
            (
                PathBuf::from("<p>/Main.ipe"),
                "module Main exposing (main)\n\nmain = 1\n".to_owned(),
            ),
        );
        let discovered = vec![project::DiscoveredModule {
            path: PathBuf::from("<p>/Main.ipe"),
            module_path: entry_path.clone(),
        }];

        let (result_a, outcome_a) = compile_modules_observed(
            sources.clone(),
            discovered.clone(),
            &entry_path,
            &out_a,
            &runtime,
            Path::new("<p>"),
            ipe_backend_rust::DbDriver::Sqlite,
            Some(&cache_dir),
            BuildOptions::default(),
        );
        assert!(
            result_a.is_ok(),
            "first (cold, Sqlite) compile must succeed: {:?}",
            result_a.err()
        );
        assert_eq!(
            outcome_a,
            CacheOutcome::Miss,
            "first compile against an empty cache dir must be a miss"
        );
        assert!(
            find_single_ir_cache_entry(&cache_dir).is_some(),
            "the cold compile must have populated the IR tier"
        );

        // Same source, DIFFERENT driver, same cache dir: the EmittedProject
        // tier's key changes (driver is part of it) so it misses, but the
        // IR tier's key does not depend on driver — it must hit.
        let (result_b, outcome_b) = compile_modules_observed(
            sources,
            discovered,
            &entry_path,
            &out_b,
            &runtime,
            Path::new("<p>"),
            ipe_backend_rust::DbDriver::Postgres,
            Some(&cache_dir),
            BuildOptions::default(),
        );
        assert!(
            result_b.is_ok(),
            "second (Postgres) compile must succeed: {:?}",
            result_b.err()
        );
        assert_eq!(
            outcome_b,
            CacheOutcome::IrHit,
            "a db_driver-only edit must hit the IR tier, not re-run the full pipeline nor \
             merely miss everything"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    /// **The IR-tier end-to-end tamper proof**, mirroring
    /// [`on_disk_cache_hit_serves_a_tampered_entry_verbatim`] one tier
    /// earlier: compile once (populates BOTH tiers), tamper the ON-DISK
    /// lowered-IR entry's literal body (`main`'s `Expr::Int(1)` ->
    /// `Expr::Int(42)`) with a value no fresh compile of the SAME source
    /// could ever produce, then force an IR-tier hit (a `db_driver` flip,
    /// which misses the `EmittedProject` tier deterministically) and assert
    /// the SENTINEL VALUE reaches the materialised `main.rs` — proof the
    /// driver actually reads, relocates, and RE-EMITS the on-disk IR entry
    /// rather than silently recompiling or ignoring the tamper.
    #[test]
    fn on_disk_ir_cache_hit_serves_a_tampered_entry_verbatim() {
        let Ok(runtime) = resolve_runtime() else {
            return;
        };
        let tmp = std::env::temp_dir().join(format!("ipec-ir-cache-tamper-{}", std::process::id()));
        let cache_dir = tmp.join("cache");
        let out_a = tmp.join("out-a");
        let out_b = tmp.join("out-b");
        let _ = fs::remove_dir_all(&tmp);

        let entry_path = vec!["Main".to_owned()];
        let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
        sources.insert(
            entry_path.clone(),
            (
                PathBuf::from("<p>/Main.ipe"),
                "module Main exposing (main)\n\nmain = 1\n".to_owned(),
            ),
        );
        let discovered = vec![project::DiscoveredModule {
            path: PathBuf::from("<p>/Main.ipe"),
            module_path: entry_path.clone(),
        }];

        let (result_a, outcome_a) = compile_modules_observed(
            sources.clone(),
            discovered.clone(),
            &entry_path,
            &out_a,
            &runtime,
            Path::new("<p>"),
            ipe_backend_rust::DbDriver::Sqlite,
            Some(&cache_dir),
            BuildOptions::default(),
        );
        assert!(
            result_a.is_ok(),
            "first (cold) compile must succeed: {:?}",
            result_a.err()
        );
        assert_eq!(outcome_a, CacheOutcome::Miss);

        let ir_json_path =
            find_single_ir_cache_entry(&cache_dir).expect("cold compile must write an IR entry");
        let stored = fs::read_to_string(&ir_json_path).expect("IR entry must be readable");
        // Verified shape via a one-off print during development:
        // `{"modules":[{"name":["Main"],...,"funcs":[{...,"body":{"Int":1}}],...}]}`.
        assert!(
            stored.contains("\"body\":{\"Int\":1}"),
            "unexpected IR JSON shape, cannot safely tamper: {stored}"
        );
        let tampered = stored.replace("\"body\":{\"Int\":1}", "\"body\":{\"Int\":42}");
        fs::write(&ir_json_path, &tampered).expect("tamper write must succeed");

        // Force the EmittedProject tier to miss (driver flip) so the
        // IR-tier fast path is the one actually exercised.
        let (result_b, outcome_b) = compile_modules_observed(
            sources,
            discovered,
            &entry_path,
            &out_b,
            &runtime,
            Path::new("<p>"),
            ipe_backend_rust::DbDriver::Postgres,
            Some(&cache_dir),
            BuildOptions::default(),
        );
        assert!(
            result_b.is_ok(),
            "second (tampered IR, hit) compile must succeed: {:?}",
            result_b.err()
        );
        assert_eq!(outcome_b, CacheOutcome::IrHit);

        let main_rs = fs::read_to_string(out_b.join("src/main.rs")).expect("main.rs must exist");
        assert!(
            main_rs.contains("42"),
            "materialized output must be re-EMITTED FROM the tampered IR entry \
             (contains the literal 42), proving the driver reads/relocates/re-emits \
             the on-disk lowered-IR cache rather than recompiling or discarding the \
             tamper: {main_rs}"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    /// A cache disabled via `cache_dir: None` never touches disk for
    /// caching purposes and always runs the full pipeline.
    #[test]
    fn cache_dir_none_disables_caching_entirely() {
        let Ok(runtime) = resolve_runtime() else {
            return;
        };
        let tmp = std::env::temp_dir().join(format!("ipe-cache-disabled-{}", std::process::id()));
        let out_dir = tmp.join("out");
        let _ = fs::remove_dir_all(&tmp);

        let entry_path = vec!["Main".to_owned()];
        let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
        sources.insert(
            entry_path.clone(),
            (
                PathBuf::from("<cache-e2e>/Main.ipe"),
                "module Main exposing (main)\n\nmain = 1\n".to_owned(),
            ),
        );
        let discovered = vec![project::DiscoveredModule {
            path: PathBuf::from("<cache-e2e>/Main.ipe"),
            module_path: entry_path.clone(),
        }];

        let (result, outcome) = compile_modules_observed(
            sources,
            discovered,
            &entry_path,
            &out_dir,
            &runtime,
            Path::new("<cache-e2e>"),
            ipe_backend_rust::DbDriver::Sqlite,
            None,
            BuildOptions::default(),
        );
        assert!(result.is_ok(), "compile must succeed: {:?}", result.err());
        assert_eq!(
            outcome,
            CacheOutcome::Miss,
            "a disabled cache is always reported as a miss"
        );
        assert!(
            !tmp.join(".ipe-cache").exists(),
            "no cache directory should be created when caching is disabled"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    // ── [wasm].mode target inference ─────────────────────────────────────────

    fn wasm_config(mode: Option<&str>) -> project::WasmConfig {
        project::WasmConfig {
            mode: mode.map(str::to_owned),
            ..Default::default()
        }
    }

    /// `[wasm] mode = "spa"` with no CLI flag → inferred `WasmClient`.
    #[test]
    fn wasm_mode_spa_infers_wasm_target() {
        let cfg = wasm_config(Some("spa"));
        assert!(
            resolve_wasm_target(false, Some(&cfg)),
            "spa mode must infer wasm target"
        );
    }

    /// `[wasm] mode = "hydrate"` with no CLI flag → inferred `WasmClient`.
    #[test]
    fn wasm_mode_hydrate_infers_wasm_target() {
        let cfg = wasm_config(Some("hydrate"));
        assert!(
            resolve_wasm_target(false, Some(&cfg)),
            "hydrate mode must infer wasm target"
        );
    }

    /// `[wasm] mode = "off"` → native (explicit opt-out).
    #[test]
    fn wasm_mode_off_does_not_infer_wasm_target() {
        let cfg = wasm_config(Some("off"));
        assert!(
            !resolve_wasm_target(false, Some(&cfg)),
            "off mode must not infer wasm target"
        );
    }

    /// No `[wasm]` section (None config) → native default.
    #[test]
    fn no_wasm_config_defaults_to_native_target() {
        assert!(
            !resolve_wasm_target(false, None),
            "absent [wasm] section must default to native"
        );
    }

    /// `mode = None` (section present but no mode key) → native.
    #[test]
    fn wasm_config_absent_mode_key_defaults_to_native_target() {
        let cfg = wasm_config(None);
        assert!(
            !resolve_wasm_target(false, Some(&cfg)),
            "absent mode key must default to native"
        );
    }

    /// CLI `--target wasm` (`cli_wasm` = true) wins even when no manifest.
    #[test]
    fn cli_flag_overrides_absent_manifest_to_wasm() {
        assert!(
            resolve_wasm_target(true, None),
            "cli flag must win over absent manifest"
        );
    }

    /// CLI `--target wasm` wins even if the manifest says off (highest precedence).
    #[test]
    fn cli_flag_wins_over_mode_off() {
        let cfg = wasm_config(Some("off"));
        assert!(
            resolve_wasm_target(true, Some(&cfg)),
            "explicit cli --target wasm must win over mode=off"
        );
    }

    /// `declared_modules` reads exactly the `pub mod X;` / `mod X;` statements a
    /// runtime `mod.rs` declares — the oracle the eject tree-shaker copies from.
    /// A `pub use X::*;` re-export is NOT a module declaration and must not add a
    /// file to the copy set, and a block-opening `pub mod X {` (an inline module
    /// with no separate source file) is excluded by the `;` requirement.
    #[test]
    fn declared_modules_reads_only_semicolon_terminated_mod_statements() {
        let mod_rs = "\
// GENERATED by Ipê — do not edit
pub mod basics;
pub mod core;
mod path_core;
pub use basics::*;
pub use core::*;
pub mod web {
    pub mod route;
}
";
        let names = declared_modules(mod_rs);
        assert!(names.contains("basics"), "a `pub mod` is a declaration");
        assert!(names.contains("core"), "a `pub mod` is a declaration");
        assert!(names.contains("path_core"), "a bare `mod` is a declaration");
        assert!(
            !names.contains("web"),
            "a block-opening `pub mod web {{` has no separate file — excluded"
        );
        // A `pub use X::*;` glob is a re-export, never a module declaration.
        assert!(
            !names.contains("basics::*") && names.iter().all(|n| !n.contains('*')),
            "a glob re-export is not a module declaration"
        );
        // The one `;`-terminated statement inside the block (`pub mod route;`) is
        // collected by name — it is harmless in practice: the copy step resolves
        // it against no top-level `route.rs`/`route/` and vendors nothing for it.
        // The real emitted native `mod.rs` is flat (no inline blocks), so this
        // case never arises there; the copy step, not this scanner, is where the
        // fail-safe lives.
        assert!(names.contains("route"));
    }

    /// The tree-shaker copies a reached module's single `.rs` file, a reached
    /// directory module's ENTIRE subtree (fail-closed — never omit a nested
    /// `mod`'s file), and nothing for a module the emitted `mod.rs` never
    /// declares. This is the whole tree-shaking contract, asserted without a
    /// compile.
    #[test]
    fn reachable_runtime_copy_takes_declared_files_and_whole_reached_dirs() {
        let tmp = std::env::temp_dir().join("ipe_eject_reach_copy");
        let _ = fs::remove_dir_all(&tmp);
        let rt = tmp.join("ipe_runtime");
        fs::create_dir_all(rt.join("web")).expect("create web/");
        fs::create_dir_all(rt.join("db")).expect("create db/");
        fs::write(rt.join("mod.rs"), "pub mod core;\npub mod web;\n").expect("mod.rs");
        fs::write(rt.join("core.rs"), "// core").expect("core.rs");
        fs::write(rt.join("unreached.rs"), "// unreached").expect("unreached.rs");
        fs::write(rt.join("web").join("mod.rs"), "pub mod route;").expect("web/mod.rs");
        fs::write(rt.join("web").join("route.rs"), "// route").expect("web/route.rs");
        // An unreached directory module: its whole subtree must be dropped.
        fs::write(rt.join("db").join("mod.rs"), "// db").expect("db/mod.rs");

        // The emitted mod.rs reaches `core` (file) and `web` (directory), never
        // `unreached` or `db`.
        let emitted_mod_rs = "pub mod core;\npub mod web;\n";
        let mut manifest = BTreeMap::new();
        collect_reachable_runtime_text(
            &rt,
            Path::new("src/ipe_runtime"),
            emitted_mod_rs,
            &mut manifest,
        )
        .expect("copy reachable runtime");

        let has = |p: &str| manifest.contains_key(&PathBuf::from(p));
        assert!(has("src/ipe_runtime/core.rs"), "reached file copied");
        assert!(
            has("src/ipe_runtime/web/mod.rs") && has("src/ipe_runtime/web/route.rs"),
            "reached directory module copied WHOLE (nested mod's file included)"
        );
        assert!(
            !has("src/ipe_runtime/unreached.rs"),
            "an undeclared file is tree-shaken away"
        );
        assert!(
            !manifest.keys().any(|k| k.starts_with("src/ipe_runtime/db")),
            "an undeclared directory module's whole subtree is tree-shaken away"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Eject refuses a wasm-target project from the `[wasm].mode` manifest tier —
    /// not only the `IPE_TARGET` env — so a browser SPA is never silently ejected
    /// as a native tree (a target the emitted crate would not build). The refusal
    /// fires before any file is written.
    #[test]
    fn eject_refuses_a_wasm_mode_project_from_the_manifest_tier() {
        let tmp = std::env::temp_dir().join("ipe_eject_wasm_mode_refuse");
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(&src).expect("create src/");
        // A project whose manifest selects the wasm target via `[wasm].mode`,
        // with no `IPE_TARGET` env set — the tier the env-only check missed.
        fs::write(
            tmp.join("ipe.toml"),
            "name = \"w\"\nentry = \"src/Main.ipe\"\n\n[wasm]\nmode = \"spa\"\n",
        )
        .expect("write ipe.toml");
        fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nmain = 0\n",
        )
        .expect("write Main.ipe");

        let out = tmp.join("out");
        let args = [
            tmp.join("ipe.toml").to_string_lossy().into_owned(),
            "--out".to_owned(),
            out.to_string_lossy().into_owned(),
        ];
        let result = run_eject(&args);
        assert!(
            matches!(result, Err(CliError::EjectUnsupported { .. })),
            "a `[wasm].mode` project must be refused, not ejected native: {result:?}"
        );
        assert!(
            !out.exists(),
            "the refusal must fire before any project tree is written"
        );
        let _ = fs::remove_dir_all(&tmp);
    }
}

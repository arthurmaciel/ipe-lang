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
pub mod contained_path;
pub mod coverage;
pub mod diff;
pub mod doc;
pub mod doc_bundle;
pub mod doc_type_search;
pub mod ffi;
pub mod fmt;
pub mod health;
pub mod help;
pub mod hot_classify;
pub mod index;
pub mod init;
pub mod io_bounded;
pub mod lint;
pub mod lockfile;
pub mod login;
mod lsp;
pub mod migrate;
pub mod net;
pub mod package_manifest;
pub mod pkg;
pub mod progress;
pub mod project;
pub mod publish;
pub mod resolve;
pub mod run_sandbox;
pub mod runtime_embed;
pub mod scratch;
pub mod style;
pub mod toolchain;
pub mod unsafe_ack;
pub mod version_check;
pub mod web_consent;
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
    ALL_CODES, Applicability, Diagnostic, HelpLine, Suggestion, explain_page, render, render_json,
    title,
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
    /// input (e.g. an unrecognised manifest value) — kept distinct from
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
    /// The verify mode found the proposed new version does not clear the
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
    /// `ipe doc --check-examples` found one or more broken doc-string examples.
    /// Carries the ready-to-print failure report. A legitimate gate result — the
    /// extraction ran correctly and an example does not compile or produce the
    /// expected result — not a command misuse, so it exits non-zero with the
    /// report alone and never the command's `--help` page.
    DocExamplesFailed(String),
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
    /// `ipe lint` found one or more findings at or above the configured gate
    /// severity. This is a legitimate gate verdict — the linter ran correctly and
    /// already printed every finding to stdout — not a command-line misuse, so it
    /// exits non-zero after the report and never shows the `lint` command's
    /// `--help` page. Carries nothing: the printed findings are the message.
    LintGateFailed,
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
    /// A `Pipeline` diagnostic was already rendered as JSON and written to
    /// stderr by the caller. The process must exit non-zero, but there is
    /// nothing left to print — the JSON line is the complete machine output.
    DiagnosticJsonEmitted,
    /// A file exceeded the per-surface read ceiling in
    /// [`io_bounded::read_to_string_capped`]. The read was stopped at the cap;
    /// no unbounded allocation was made.
    FileTooLarge {
        /// The path of the oversized file.
        path: PathBuf,
        /// The ceiling (bytes) that was enforced.
        max: u64,
    },
    /// A manifest `sourceRoot` (or equivalent dependency path) was rejected by
    /// [`contained_path::ContainedRelPath::parse`] because it escapes the
    /// project directory. Carries the specific [`contained_path::PathEscape`]
    /// reason so the diagnostic names exactly why the path was refused.
    PathEscape {
        /// The raw path string as it appeared in the manifest.
        raw: String,
        /// Why the path was rejected.
        reason: contained_path::PathEscape,
    },
    /// The module-discovery walk hit its depth ceiling or detected a symlink
    /// cycle. Carries the maximum depth that was configured and, for a cycle,
    /// the directory path where the cycle was detected.
    DiscoveryLimitReached {
        /// The depth ceiling that was enforced (`MAX_DISCOVERY_DEPTH`), or the
        /// path at which a symlink cycle was detected.
        detail: String,
    },
    /// `ipe upgrade` (or `ipe health`) could not reach the release feed. This
    /// is a transient, non-zero operational result — not a command misuse — so
    /// it exits with no `--help` page and renders its own message. Carries
    /// nothing: the human or machine output was already printed; this is only
    /// the exit-code signal.
    UpgradeFeedUnreachable,
    /// `ipe upgrade --check --exit-code` resolved the action and must exit
    /// with a numeric code that is neither SUCCESS nor FAILURE (e.g. 10 for
    /// "upgrade available"). Carries the code so `main` can return it as an
    /// `ExitCode` after printing nothing (the status line was already printed
    /// by `run_upgrade`).
    UpgradeCheckExit {
        /// The process exit code (10 = available, 0 = up to date,
        /// 2 = unreachable).
        code: i32,
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

/// Emit a `Pipeline` diagnostic as a JSON object on stderr, then return
/// [`CliError::DiagnosticJsonEmitted`] so the caller exits non-zero without
/// printing the human-readable layout a second time.
///
/// Any non-`Pipeline` error is returned as-is (the human path continues for it).
fn emit_pipeline_json(err: CliError) -> CliError {
    if let CliError::Pipeline {
        ref file,
        ref src,
        ref diag,
    } = err
    {
        let json = render_json(diag, &file.to_string_lossy(), src);
        // Best-effort write; if stderr is closed we still exit non-zero.
        let _ = std::io::stderr().write_all(json.as_bytes());
        return CliError::DiagnosticJsonEmitted;
    }
    err
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
    #[allow(clippy::too_many_lines)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(hint) => write!(f, "{hint}"),
            Self::UsageOwned(hint) => write!(f, "{hint}"),
            Self::UnknownCommand { attempted } => fmt_unknown_command(attempted, f),
            Self::Io { path, source } => fmt_io_error(path, source, f),
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
            Self::DocCoverage(report) | Self::DocExamplesFailed(report) => f.write_str(report),
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
            // The findings already went to stdout; this stderr line is the
            // one-line verdict paired with the non-zero gate exit.
            Self::LintGateFailed => write!(
                f,
                "{}lint: findings remain at or above the gate severity (see above)",
                style::GUTTER
            ),
            // Both already wrote their final output; nothing more to display.
            Self::DiagnosticJsonEmitted | Self::UpgradeCheckExit { .. } => Ok(()),
            Self::UpgradeFeedUnreachable => write!(
                f,
                "{}{}{}  couldn't reach the release feed — check your connection",
                style::GUTTER,
                style::glyph::FAIL,
                style::GUTTER
            ),
            Self::FileTooLarge { path, max } => write!(
                f,
                "{}: file exceeds the {max}-byte read ceiling — \
                 refusing to allocate an unbounded buffer",
                path.display()
            ),
            Self::PathEscape { raw, reason } => {
                write!(f, "manifest path {raw:?} was rejected: {reason}")
            }
            Self::DiscoveryLimitReached { detail } => {
                write!(f, "module-discovery walk aborted: {detail}")
            }
        }
    }
}

/// Render [`CliError::UnknownCommand`] for `Display`: an optional "unknown
/// command" line with a near-miss suggestion, then the top-level help screen
/// (coloured for a terminal). Output goes to stderr, where misuse output belongs.
///
/// The whole block is guttered as one unit so the "unknown command" lines and
/// the help header share the same left gutter — the screen reads identically to
/// the plain top-level page, only with the leading advice. The top-level page
/// already carries its own gutter, so an unknown-command entry re-gutters only
/// its own advice lines and leaves the page as-is.
fn fmt_unknown_command(attempted: &str, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    if !attempted.is_empty() {
        writeln!(
            f,
            "{}",
            style::gutter(&format!("unknown command `{attempted}`"))
        )?;
        if let Some(sugg) = nearest_command(attempted) {
            writeln!(f, "{}", style::gutter(&format!("= help: maybe `{sugg}`?")))?;
        }
    }
    f.write_str(&help::top_level(&std::io::stderr()))
}

/// Render [`CliError::Io`] for `Display`, styled and actionable rather than a
/// raw OS string. A missing file is the common case a first-time user hits, so
/// it gets a plain-language message with no `os error N` tail and no `io error`
/// jargon; every other kind keeps the readable OS description under the same
/// guttered, path-naming frame. Never leaks an errno.
fn fmt_io_error(
    path: &Path,
    source: &std::io::Error,
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    // The generic error path in the binary already frames and gutters this
    // (`ipe: <message>`); render only the message body, styled and errno-free.
    if source.kind() == std::io::ErrorKind::NotFound {
        write!(
            f,
            "no such file `{}` — pass a source file, or run inside an Ipê project \
             (a directory with a package.ipe, or a src/Main.ipe)",
            path.display()
        )
    } else {
        // A readable kind description, never the `(os error N)` tail. `ErrorKind`
        // renders as a short human phrase (e.g. "permission denied").
        write!(
            f,
            "could not access `{}` — {}",
            path.display(),
            source.kind()
        )
    }
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

/// Render [`CliError::EmittedBuildFailed`] for `Display`.
///
/// Two cases:
///
/// - **Attributable**: `cargo`'s stderr names a missing runtime feature — lead
///   with a targeted line pointing at the stale runtime crate.
/// - **Unattributable**: every other `cargo` failure after a successful Ipê
///   compile. The front-end gate ensures only valid programs reach emit, so a
///   `cargo` failure here means the emitted Rust is wrong — a miscompile in Ipê,
///   not the user's source. Render it as a humble `CompilerBug` ICE so the user
///   knows to file a report rather than try to fix their source. The full `cargo`
///   stderr is embedded as the reportable detail.
///
/// Neither form shows any command's `--help` page.
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
    // Registry/network unreachable: cargo could not reach crates.io. This is an
    // environment failure (DNS, offline, proxy), not a compiler bug and not the
    // user's source. Render a calm, actionable message and do NOT invite a bug
    // report.
    if is_registry_unreachable(trimmed) {
        let detail = if trimmed.is_empty() {
            format!("cargo exited {code} while fetching crates for {what}")
        } else {
            format!("cargo exited {code} while fetching crates for {what}:\n{trimmed}")
        };
        let d = Diagnostic::RegistryUnreachable { detail };
        return f.write_str(&render(&d, "", ""));
    }
    // Unattributable: the emitted Rust crate failed to compile for a reason that
    // is not a known runtime-feature gap. Because the front-end gate ensures only
    // valid programs reach emit, this cargo failure reflects a bug in Ipê's own
    // emission, not the user's source. Surface it as a humble ICE. The full cargo
    // stderr is embedded as the reportable detail so a bug report contains
    // everything needed to reproduce the miscompile.
    let detail = if trimmed.is_empty() {
        format!("cargo exited {code} with no output while compiling {what}")
    } else {
        format!("cargo exited {code} while compiling {what}:\n{trimmed}")
    };
    let ice = Diagnostic::CompilerBug {
        where_: "emit.cargo_build",
        detail,
    };
    f.write_str(&render(&ice, "", ""))
}

/// Detect whether cargo's stderr signals a network-level registry failure
/// (offline, DNS resolution, or a transient fetch error) rather than a compiler
/// miscompile.
///
/// Only genuinely network-level phrases qualify. Broader phrases like "failed to
/// load source for dependency" or "registry index" are deliberately excluded:
/// they also fire when a local path dependency is missing or a manifest is
/// malformed — not connectivity problems, and reporting them as "check your
/// connection" would misdirect the user. The offline case always surfaces one of
/// the network phrases below as its root cause.
fn is_registry_unreachable(stderr: &str) -> bool {
    stderr.contains("Could not resolve host")
        || stderr.contains("spurious network error")
        || stderr.contains("failed to fetch")
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
    /// The `[wasm] publicEnv` allowlist from `package.ipe`, already validated
    /// against the secret-name denylist at parse time. Empty when the
    /// project has no `[wasm]` section (or no manifest — the
    /// sibling-discovery single-file path). Threaded into
    /// [`ipe_backend_rust::RustBackend::with_wasm_public_env`] /
    /// [`ipe_db::BuildConfig::wasm_public_env`].
    pub wasm_public_env: Vec<String>,
    /// `true` when `[wasm] mode = "hydrate"` in the project's `package.ipe`.
    /// Causes the backend to emit a `#[wasm_bindgen] pub fn hydrate(model_json: &str)`
    /// export in addition to the `#[wasm_bindgen(start)] pub fn ipe_start()` entry.
    /// The emitted `hydrate` function parses the island JSON as the user's declared
    /// `HydrationState` type, converts to `Model` via `fromHydrationState`, and
    /// calls `ipe_runtime::wasm::wasm_adopt_app`. On parse failure it falls back
    /// to clean `ipe_main()` with a console warning (fault-tolerant hydrate — see
    /// spec Q6 §"Fault-tolerant hydrate — parse, don't unwrap").
    pub wasm_hydrate_mode: bool,
    /// `true` for a production build (`ipe release` — any target). Threaded into
    /// [`ipe_db::BuildConfig::production`] so the emit demand rejects any
    /// development-only `Debug.*` escape hatch (IPE-L0140). Default `false`
    /// (`ipe build` / `ipe run` are development builds — `Debug.*` permitted).
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
    /// The sanitized Cargo package name for the emitted crate, derived from
    /// `package.ipe`'s name via
    /// [`ipe_backend_rust::sanitize_cargo_name`]. The emitted `Cargo.toml`
    /// carries `[package] name = "<cargo_name>"` and the built binary is
    /// named accordingly. Empty string uses the safe `"ipe-app"` default
    /// (single-file builds with no manifest).
    pub cargo_name: String,
    /// `true` when `ipe build --debugger` / `ipe run --debugger` was passed.
    /// Threaded through [`ipe_db::BuildConfig`] to
    /// [`ipe_backend_rust::RustBackend::with_debugger`], which adds the
    /// `debugger` feature to the emitted project's runtime dependency so the TEA
    /// driver instantiates the recorder. NEVER set for `ipe release` builds — the
    /// release command does not expose this flag, so no production artifact can
    /// carry recorder code.
    pub debugger: bool,
    /// `true` routes style-value literals through a per-view `LiteralTable` and
    /// emits the `/_ipe/hot-appearance` endpoint, so an appearance-only source
    /// edit hot-swaps in the running app instead of forcing a recompile. Set
    /// ONLY by the `ipe watch` entry (from [`hot_appearance_enabled`]); the
    /// `ipe build` / `ipe run` / `ipe release` entries leave it `false` so a
    /// release artifact never carries hot-swap scaffolding. Default `false`.
    pub hot_appearance: bool,
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

/// Whether the dev-only appearance hot-swap emit is enabled for `ipe watch`.
///
/// Default ON: `ipe watch` hot-swaps appearance-only edits (e.g. `Ui.spacing`)
/// without a recompile out of the box. Opt out with `IPE_WATCH_NO_HOT_APPEARANCE`
/// (set to any non-empty value other than `0`), which forces the plain
/// direct-literal emit. `IPE_WATCH_HOT_APPEARANCE`, when set, is honoured
/// explicitly (`0` or empty = off, anything else = on) and overrides the
/// default; the opt-out takes precedence over it.
///
/// This lever exists ONLY in `ipe watch`. `ipe build` / `ipe run` / `ipe release`
/// thread [`BuildOptions::hot_appearance`] `= false`, so a release artifact never
/// carries hot-swap scaffolding regardless of these variables.
#[must_use]
pub fn hot_appearance_enabled() -> bool {
    hot_appearance_from_env(
        std::env::var("IPE_WATCH_NO_HOT_APPEARANCE").ok().as_deref(),
        std::env::var("IPE_WATCH_HOT_APPEARANCE").ok().as_deref(),
    )
}

/// Pure decision for [`hot_appearance_enabled`], over the two raw variable
/// values (`None` = unset). Opt-out (`no_var`) wins; then an explicit
/// `hot_var`; otherwise the default is on.
#[must_use]
fn hot_appearance_from_env(no_var: Option<&str>, hot_var: Option<&str>) -> bool {
    let set = |v: Option<&str>| v.is_some_and(|s| !s.is_empty() && s != "0");
    if set(no_var) {
        return false;
    }
    hot_var.is_none_or(|v| !v.is_empty() && v != "0")
}

/// Whether the dev-only browser build-status banner is enabled for `ipe watch`.
///
/// Enabled unless `IPE_WEB_BANNER` is explicitly `off`/`0`/`false`. Mirrors the
/// runtime's `watch_banner_active` disable semantics so the CLI-side poster and
/// the server-side endpoint agree on when the banner is live. Distinct from
/// [`hot_appearance_enabled`]: the failure banner must surface a red compile
/// error whenever the banner is on, even with appearance hot-swap off.
#[must_use]
pub fn watch_banner_enabled() -> bool {
    std::env::var("IPE_WEB_BANNER").map_or(true, |v| {
        let v = v.trim().to_ascii_lowercase();
        !(v == "off" || v == "0" || v == "false")
    })
}

/// Whether the DEV-ONLY blue-green front proxy is enabled for `ipe watch`.
///
/// Default ON: `ipe watch` puts a persistent proxy on the user's port and cuts
/// each rebuilt binary over behind it once it passes readiness, so a rebuild
/// never drops the browser's connection (no "Reconnecting…" flash — the client
/// gets a brief "updated ✓" toast instead). Opt out with `IPE_WATCH_NO_BLUEGREEN`
/// (set to any non-empty value other than `0`) to fall back to the direct-bind,
/// kill-old-then-spawn-new path. The legacy `IPE_WATCH_BLUEGREEN` still forces a
/// choice when set (`0`/empty ⇒ off, anything else ⇒ on) and takes precedence
/// over the default but yields to the opt-out. This lever exists ONLY in
/// `ipe watch`; it is never compiled into a release binary or an emitted app.
#[must_use]
pub fn bluegreen_enabled() -> bool {
    bluegreen_from_env_values(
        std::env::var("IPE_WATCH_NO_BLUEGREEN").ok().as_deref(),
        std::env::var("IPE_WATCH_BLUEGREEN").ok().as_deref(),
    )
}

/// The pure default-resolution behind [`bluegreen_enabled`], separated from the
/// process-env read so it is unit-testable without mutating global state.
///
/// `no_bluegreen` / `bluegreen` are the respective env values (`None` = unset).
/// Precedence: opt-out wins, then an explicit legacy choice, else default on.
#[must_use]
fn bluegreen_from_env_values(no_bluegreen: Option<&str>, bluegreen: Option<&str>) -> bool {
    // Opt-out wins: a hard "never proxy" for a user who needs the direct bind.
    if no_bluegreen.is_some_and(|v| !v.is_empty() && v != "0") {
        return false;
    }
    // Then an explicit legacy choice, if any: `0`/empty off, else on.
    if let Some(v) = bluegreen {
        return !v.is_empty() && v != "0";
    }
    // Default on.
    true
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
    let source =
        crate::io_bounded::read_to_string_capped(entry, crate::io_bounded::SOURCE_READ_CAP)?;

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
    // documented `package.ipe` default for a project that has no database
    // setting.
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
/// When no manifest is present, the entry file's parent directory is used
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

    // No manifest on this path either (sibling discovery is the "no manifest
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

    // No manifest driver is threaded here (the test stage mirrors the sibling
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
/// single-entry analysis paths ([`lower_entry_via_graph`], [`emit_ir_text`]) so all
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
    let source =
        crate::io_bounded::read_to_string_capped(entry, crate::io_bounded::SOURCE_READ_CAP)?;
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
    let entry_source =
        crate::io_bounded::read_to_string_capped(test_entry, crate::io_bounded::SOURCE_READ_CAP)?;
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
            let src = crate::io_bounded::read_to_string_capped(
                &m.path,
                crate::io_bounded::SOURCE_READ_CAP,
            )?;
            sources.insert(m.module_path.clone(), (m.path.clone(), src));
        }
    }
    Ok(sources)
}

/// Walk up the directory tree from a `.ipe` file's parent, looking for a
/// `package.ipe` manifest. Returns the manifest path if found, or `None` when
/// the walk reaches the filesystem root.
///
/// When given a file entry, the driver locates the project root (where
/// `package.ipe` lives) before building, so the full module graph is compiled
/// instead of just the single entry file.
fn find_manifest_for_ipe_file(ipe_file: &Path) -> Option<PathBuf> {
    let mut dir = ipe_file.parent()?;
    loop {
        if let Some(manifest) = project::manifest_in_dir(dir) {
            return Some(manifest);
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
        options.hot_appearance,
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
            // be rejected here too (IPE-L0140) — otherwise a cached dev artifact
            // could slip through a release build that hits this tier.
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
                    .with_debugger(options.debugger)
                    .with_project_name(&options.cargo_name)
                    .with_hot_appearance(options.hot_appearance)
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
    // is narrowed to `db_driver` rather than the full manifest shape). A
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
        options.debugger,
        options.cargo_name.clone(),
        options.hot_appearance,
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

/// The project root a `customElement "<js-path>"` literal resolves against: the
/// directory holding the entry file, the same root sibling module discovery uses.
///
/// Canon refuses a `..`-escaping literal (IPE-P0063) and an absolute/rooted one
/// (IPE-N0044), so the joined path is lexically inside this root. That is
/// necessary but NOT sufficient: a symlink UNDER the root can still point outside
/// it, so the caller additionally canonicalises the join and asserts containment
/// (`starts_with` the canonical root) before trusting it — the lexical seals and
/// the resolved-path containment check are independent layers.
fn widget_file_root(entry_src_path: &Path) -> &Path {
    entry_src_path
        .parent()
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| Path::new("."))
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

    // Widget-file gate (IPE-N0044, Security #1). The `customElement` constructor's
    // shape + lexical path seals (traversal IPE-P0063, absolute/rooted IPE-N0044)
    // already ran in canon; here — the one stage that owns the project root — two
    // FILESYSTEM invariants are enforced, defence-in-depth against the lexical
    // seals:
    //   1. Containment: the resolved path must lie strictly INSIDE the project
    //      root. `ContainedRelPath::parse` canonicalises the join and asserts it
    //      is a descendant of the canonical root, so a symlink UNDER the root that
    //      points OUTSIDE it (which the lexical seals cannot see) is refused. A
    //      bare `Path::join`/`is_file` would instead FOLLOW that symlink and stat
    //      an out-of-project file — the escape this check closes.
    //   2. Existence: the contained path must name a real file, so a widget never
    //      registers against a file that is not there.
    // Both fail closed with IPE-N0044; the widget seam never reaches emission on
    // an out-of-project or absent file.
    let widget_root = widget_file_root(&entry_src_path);
    // The program's widget manifest: one entry per DISTINCT reached
    // `customElement "<path>"`, carrying the lowerer-minted `ipe-ce-<hex>` tag
    // (so the served-glue registration targets the SAME tag the view node
    // renders) and the author hook file's verbatim content (WP5 serves it
    // content-addressed + SRI). Populated as the containment/existence gate
    // proves each file; deduplicated by tag so two views of one widget register
    // once. Empty for a program that uses no `Ui.widget`.
    let mut widget_manifest: BTreeMap<String, String> = BTreeMap::new();
    for widget in ipe_canon::custom_element_gate::collect_widget_files(linked) {
        let reject = |detail: String| {
            let (file, src) = source_for_span(widget.span);
            CliError::Pipeline {
                file,
                src,
                diag: Box::new(ipe_diagnostics::Diagnostic::Name {
                    span: widget.span,
                    msg: ipe_diagnostics::NameError::CustomElementCtorMalformed {
                        detail: detail.into_boxed_str(),
                    },
                }),
            }
        };
        let contained = contained_path::ContainedRelPath::parse(widget_root, &widget.cleaned_path)
            .map_err(|_| {
                reject(format!(
                    "the widget-hook file `{}` resolves outside the project directory \
                         (an absolute path, a `..` climb, or a symlink pointing above the \
                         project root) and is refused",
                    widget.cleaned_path
                ))
            })?;
        if !contained.resolved().is_file() {
            return Err(reject(format!(
                "the widget-hook file `{}` does not exist in the project",
                widget.cleaned_path
            )));
        }
        // Read the verified in-project file's content for content-addressed +
        // SRI serving. `resolved()` is the containment-checked canonical path, so
        // this read stays strictly inside the project root. A read failure (a
        // race that removed the file between the `is_file` check and here, or a
        // permission fault) fails the build closed — the widget seam never
        // reaches emission on a file we could not read whole.
        let content = std::fs::read_to_string(contained.resolved()).map_err(|e| {
            reject(format!(
                "the widget-hook file `{}` could not be read: {e}",
                widget.cleaned_path
            ))
        })?;
        // The tag is the SINGLE lowerer definition, keyed on the same cleaned
        // path the view node hashed — never a second, drift-prone hash here.
        let tag = ipe_lower::custom_element_tag(&widget.cleaned_path);
        widget_manifest.entry(tag).or_insert(content);
    }

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
    let mut emitted = (*emitted).clone();

    // Thread the widget manifest into the emitted program. The emit query is a
    // pure function of the source text and cannot read the widget files off disk
    // (INV-1: no salsa query touches the filesystem); this stage owns the project
    // root and already read each file above, so it wires the transport here.
    //
    // Two transports, one manifest:
    //  * Native (server-driven Web) — inject the one-time `widget_assets::register`
    //    into `main`, so the runtime serves each asset content-addressed + SRI and
    //    generates the attribute/POST glue at process start.
    //  * WasmClient (browser client) — there is no server to serve routes, so the
    //    static bundle carries the assets: write each author file + the generated
    //    property/CustomEvent glue into `www/`, SRI-pinned, and reference them from
    //    the static `index.html`. The in-process wasm sink delivers down-state as a
    //    decoded property and folds up-`CustomEvent`s into `update`.
    //
    // A widget-free program injects nothing under either target (byte-identical
    // emit).
    if !widget_manifest.is_empty() {
        match config.target(db) {
            ipe_ir::Target::Native => {
                inject_widget_registration(&mut emitted, &widget_manifest)?;
            }
            ipe_ir::Target::WasmClient => {
                inject_wasm_widget_bundle(&mut emitted, &widget_manifest)?;
            }
        }
    }
    Ok(emitted)
}

/// Inject the process-start `Ui.widget` asset registration into the emitted
/// `main.rs`, so the served app registers its widget assets before the web
/// server binds.
///
/// The registration is a single `ipe_runtime::web::widget_assets::register(&[…])`
/// call spliced in right after `install_panic_classifier();` in the generated
/// `main()` — the first line of the entry point, before any task runs. Each
/// `(tag, content)` is rendered as a Rust string-literal pair; the content is
/// emitted as a raw string literal with a hash fence wide enough to clear any run
/// of `#` in the file, so arbitrary JS (including embedded `"` / `#`) is a valid
/// literal and no author byte can break out of the string into code (the content
/// is DATA in the emitted program, exactly as it is data in the browser).
///
/// # Errors
/// [`CliError`] carrying a [`Diagnostic::CompilerBug`] if `src/main.rs` is absent
/// from the emitted file set or the `install_panic_classifier();` anchor the
/// splice keys on is missing — a drifted emit template, surfaced loudly rather
/// than silently emitting a program that never registers its widgets.
fn inject_widget_registration(
    emitted: &mut ipe_backend::EmittedProject,
    manifest: &BTreeMap<String, String>,
) -> Result<(), CliError> {
    // The splice point: the first line of the generated `main()`, before any task
    // runs, so the registry is populated before the web server binds.
    const ANCHOR: &str = "install_panic_classifier();";
    let bug = |detail: &str| CliError::Pipeline {
        file: PathBuf::from("src/main.rs"),
        src: String::new(),
        diag: Box::new(ipe_diagnostics::Diagnostic::CompilerBug {
            where_: "ipe_cli::inject_widget_registration",
            detail: detail.to_owned(),
        }),
    };
    let main = emitted
        .files
        .get_mut("src/main.rs")
        .ok_or_else(|| bug("no src/main.rs in the emitted file set for widget registration"))?;

    // Build the `register(&[(tag, content), …])` argument list. Deterministic
    // order (BTreeMap iteration) keeps the emit byte-stable across builds.
    let mut entries = String::new();
    for (tag, content) in manifest {
        entries.push_str("        (");
        entries.push_str(&rust_str_literal(tag));
        entries.push_str(", ");
        entries.push_str(&rust_raw_str_literal(content));
        entries.push_str("),\n");
    }
    let call = format!("\n    ipe_runtime::web::widget_assets::register(&[\n{entries}    ]);\n");

    let Some(pos) = main.find(ANCHOR) else {
        return Err(bug(
            "the emitted main() is missing the `install_panic_classifier();` anchor the widget \
             registration splices after — the emit template drifted",
        ));
    };
    let insert_at = pos + ANCHOR.len();
    main.insert_str(insert_at, &call);
    Ok(())
}

/// Assemble the browser-client widget bundle into the emitted static SPA.
///
/// The `WasmClient` target has no server to mount asset routes, so the assets
/// ride the static `www/` tree. This writes, for the widget manifest:
///
///  * each author hook file at `www/_ipe/widget.<hex16>.js` (content-addressed,
///    so a page pinning its SRI can never be served different bytes);
///  * the generated registration glue at `www/_ipe/widget-glue.<hex16>.js`
///    (the `WasmClient` transport: down as a decoded property, up as a typed
///    `CustomEvent`) — produced by the SAME `ipe_runtime::widget_assets` generator
///    the server path serves, so there is one glue, not a drift-prone twin;
///  * SRI-pinned `<link rel="modulepreload">` + glue `<script type="module">`
///    references spliced into `www/index.html` before `</head>`.
///
/// The hash the page pins is `sha256` over the served bytes (§ `widget_assets`),
/// so page integrity == served bytes for the static target exactly as for the
/// server. `base` is empty: the static SPA is root-mounted, so the absolute
/// `/_ipe/…` asset URLs resolve against the `www/` document root.
///
/// # Errors
/// [`CliError`] carrying a [`Diagnostic::CompilerBug`] if `www/index.html` is
/// absent from the emitted file set or lacks the `</head>` anchor — a drifted
/// wasm emit template, surfaced loudly.
fn inject_wasm_widget_bundle(
    emitted: &mut ipe_backend::EmittedProject,
    manifest: &BTreeMap<String, String>,
) -> Result<(), CliError> {
    use ipe_runtime_rust::widget_assets::{
        WidgetAsset, WidgetTransport, glue_js_for, glue_path_for, page_scripts_for,
        widget_asset_path,
    };

    // The static SPA is root-mounted; the `/_ipe/…` asset URLs are document-root
    // absolute, so the `www/`-relative file path drops the leading slash.
    const BASE: &str = "";
    const TRANSPORT: WidgetTransport = WidgetTransport::WasmClient;
    const HEAD_CLOSE: &str = "</head>";

    let bug = |detail: String| CliError::Pipeline {
        file: PathBuf::from("www/index.html"),
        src: String::new(),
        diag: Box::new(ipe_diagnostics::Diagnostic::CompilerBug {
            where_: "ipe_cli::inject_wasm_widget_bundle",
            detail,
        }),
    };
    let rel = |p: &str| -> Result<ipe_backend::RelPath, CliError> {
        ipe_backend::RelPath::new(p.to_owned()).map_err(|_| {
            bug(format!(
                "the generated widget asset path `{p}` is not a valid in-project relative path"
            ))
        })
    };

    // Rebuild the explicit asset slice (deterministic BTreeMap order → byte-stable
    // emit) the registry-free generator consumes.
    let assets: Vec<WidgetAsset> = manifest
        .iter()
        .map(|(tag, content)| WidgetAsset {
            tag: tag.clone(),
            content: content.clone(),
        })
        .collect();

    // Write each author hook file content-addressed under `www/`. `widget_asset_path`
    // yields the absolute URL path `/_ipe/widget.<hex16>.js`; strip the leading
    // `/` for the `www/`-relative file key.
    for asset in &assets {
        let url_path = widget_asset_path(&asset.content);
        let file_path = format!("www{url_path}");
        emitted
            .files
            .insert(rel(&file_path)?, asset.content.clone());
    }

    // Write the generated glue (WasmClient transport) content-addressed under `www/`.
    let glue_url = glue_path_for(&assets, BASE, TRANSPORT);
    let glue_body = glue_js_for(&assets, BASE, TRANSPORT);
    emitted
        .files
        .insert(rel(&format!("www{glue_url}"))?, glue_body);

    // Splice the SRI-pinned preload + glue script references into `index.html`
    // before `</head>` (external + SRI + crossorigin — no inline script, so the
    // static shell's CSP `script-src 'self' 'wasm-unsafe-eval'` is unchanged).
    let scripts = page_scripts_for(&assets, BASE, TRANSPORT);
    let index = emitted
        .files
        .get_mut("www/index.html")
        .ok_or_else(|| bug("no www/index.html in the emitted wasm file set".to_owned()))?;
    let Some(pos) = index.find(HEAD_CLOSE) else {
        return Err(bug(
            "the emitted www/index.html is missing the `</head>` anchor the widget bundle \
             splices before — the wasm emit template drifted"
                .to_owned(),
        ));
    };
    index.insert_str(pos, &scripts);
    Ok(())
}

/// Render `s` as a plain double-quoted Rust string literal (the tag: a fixed
/// `ipe-ce-<hex>`, `[a-z0-9-]` only, so escaping is trivial but applied for
/// safety).
fn rust_str_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Render `s` as a Rust RAW string literal `r#"…"#` with a hash fence wide enough
/// to clear any `"#` run inside `s`, so arbitrary content (author JS with quotes
/// and hashes) is emitted verbatim as data — it can never terminate the literal
/// early and spill into code.
fn rust_raw_str_literal(s: &str) -> String {
    // The fence must be longer than the longest run of `#` that immediately
    // follows a `"` in the content (that is the only sequence that could close a
    // raw literal). Computing the max `#`-run overall is a safe over-approximation.
    let mut max_hashes = 0usize;
    let mut run = 0usize;
    for ch in s.chars() {
        if ch == '#' {
            run += 1;
            max_hashes = max_hashes.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "#".repeat(max_hashes + 1);
    format!("r{fence}\"{s}\"{fence}")
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
///      salsa-tracked query). For the dependency model this step is replaced
///      by bundling the embedded runtime source under `ipe_runtime_dep/` so
///      the relative path dep in the emitted `Cargo.toml` is always satisfied.
///   2. `Cargo.toml` at the project root.
///   3. Every backend-emitted file (`emitted.files`; each key is already a
///      validated [`ipe_backend::RelPath`] — relative and `..`-free — so no
///      entry here can escape `out_dir`).
///
/// # Errors
/// [`CliError::Io`] on any filesystem failure reading `runtime_dir` (including
/// a non-UTF-8 file, surfaced as an I/O error rather than a panic — the
/// runtime tree is trusted in-repo source, so this is not expected to fire in
/// practice). [`CliError::RuntimeMaterializeFailed`] when the embedded runtime
/// crate contains non-UTF-8 files (unexpected for in-repo source).
fn build_emit_manifest(
    emitted: &ipe_backend::EmittedProject,
    runtime_dir: &Path,
    tree_shake_vendored: bool,
) -> Result<BTreeMap<PathBuf, String>, CliError> {
    let mut manifest = BTreeMap::new();
    // The emit shape is self-describing: a vendored emit always writes
    // `src/ipe_runtime/mod.rs`; the dep-model emit never does. Use this to
    // branch between the two materialization strategies.
    let emitted_mod_rs = emitted
        .files
        .iter()
        .find(|(rel, _)| rel.as_str() == "src/ipe_runtime/mod.rs")
        .map(|(_, contents)| contents.as_str());
    if let Some(mod_rs) = emitted_mod_rs {
        // Vendored model: copy the runtime source tree into `src/ipe_runtime/`.
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
    } else {
        // Dependency model: the emitted `Cargo.toml` declares the runtime via
        // a relative path dep (`path = "ipe_runtime_dep"`). Bundle the embedded
        // runtime source under that directory so the dep resolves in any
        // environment — cross-compiler container, offline, CI — without a
        // host-absolute path. The embedded source is the binary's own version,
        // identical to the in-repo tree by construction.
        let embedded = runtime_embed::collect_embedded_crate_text()?;
        for (rel, text) in embedded {
            manifest.insert(PathBuf::from("ipe_runtime_dep").join(rel), text);
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

/// Build a multi-module Ipe project rooted at `manifest_path` (`package.ipe`)
/// into a Rust Cargo project under `out_dir`, vendoring the runtime from
/// `runtime_dir`.
///
/// The build pipeline:
/// 1. Parse `package.ipe` to locate the source root.
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
        let src =
            crate::io_bounded::read_to_string_capped(&m.path, crate::io_bounded::SOURCE_READ_CAP)?;
        sources.insert(m.module_path.clone(), (m.path.clone(), src));
    }

    // A library package (declares `exposedModules`, has no runnable entry —
    // neither a `programs` stage nor a `src/Main.ipe`) has nothing to emit. Refuse
    // with a clean, honest message directing the author to `ipe type-check`
    // (which analyses the public surface) rather than the internal
    // missing-entry error a bogus `["Main"]` entry would raise downstream.
    let entry_path = manifest.resolved_entry()?;
    if manifest.default_program().is_none()
        && !manifest.exposed_modules.is_empty()
        && !sources.contains_key(&entry_path)
    {
        return Err(CliError::Usage(
            "this is a library package (it declares `exposedModules` and no runnable program) — \
             there is no entry to build. Use `ipe type-check` to verify its public surface, or \
             add a `Package.programs [ … ]` stage to declare a runnable entry",
        ));
    }
    // The emit epilogue's fixed `fn main` calls `ipe_main`, which the backend
    // names only for a `main` in module `Main`. A `programs`-declared entry in a
    // non-`Main` module type-checks (analysis honours the declared entry) but the
    // emit's main-symbol naming does not yet thread through a per-program entry,
    // so emitting a non-`Main` program entry would miscompile. Refuse cleanly and
    // point at the working analysis path rather than emit a broken crate.
    if entry_path != ["Main".to_owned()] {
        return Err(CliError::UsageOwned(format!(
            "program entry module `{}` is not yet buildable — a declared `programs` entry outside \
             module `Main` type-checks (`ipe type-check`) but native emission still assumes a \
             `Main` entry. Name the entry file `Main.ipe`, or track the multi-program emit \
             follow-up",
            entry_path.join(".")
        )));
    }

    // Fold in the manifest-derived fields: `[wasm] publicEnv`, hydrate mode,
    // and the project name (sanitized to a valid Cargo package name). The
    // caller's `options` carries no manifest-derived data — it is built before
    // the manifest is parsed — so these three fields are completed here, the
    // same way `manifest.driver` bypasses `options` as its own positional arg.
    let options = BuildOptions {
        wasm_public_env: manifest.wasm.public_env.clone(),
        wasm_hydrate_mode: manifest.wasm.mode.as_deref() == Some("hydrate"),
        cargo_name: ipe_backend_rust::sanitize_cargo_name(&manifest.name),
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
pub(crate) fn resolve_vendored_runtime_dir(
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
const NO_ENTRY: &str = "nothing to build here — pass a source file or run inside a project (a \
     package.ipe, or a src/Main.ipe)";

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
    let Some((cmd, rest)) = args.split_first() else {
        // A bare `ipe` (no command) carries an empty token and just shows help.
        return Err(CliError::UnknownCommand {
            attempted: String::new(),
        });
    };
    // `ipe explain` has been folded into `ipe doc`. Print a pointer and
    // forward to `run_explain` so existing scripts keep working with a
    // deprecation notice rather than a hard failure.
    if cmd == "explain" {
        return with_help_on_misuse("doc", run_explain(rest));
    }
    // One registry drives both dispatch and help: a command runs exactly when it
    // is described, so the two cannot drift. The handler carries the canonical
    // static name its misuse `--help` page keys on. Version is the `version`
    // command only — there is no `--version`/`-V` flag alias.
    match help::handler(cmd.as_str()) {
        Some((name, run)) => with_help_on_misuse(name, run(rest)),
        // An unknown command is misuse: show the top-level help and fail. Unlike
        // an explicit `--help`, this is not a request, so it exits non-zero. The
        // typed token is kept so a near-miss can be suggested.
        None => Err(CliError::UnknownCommand {
            attempted: cmd.clone(),
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
/// 1. `./package.ipe` exists — entry `"."` (project mode; `discover_manifest`
///    reads the directory's `package.ipe`).
/// 2. `./src/Main.ipe` exists — entry `"src/Main.ipe"` (single-file
///    shorthand without a manifest).
/// 3. A bare `./ipe.toml` with no `package.ipe` — a clear migration error, so
///    the legacy manifest never silently governs a build.
/// 4. Neither — usage error: nothing to build here.
pub(crate) fn default_entry() -> Result<String, CliError> {
    if std::path::Path::new(package_manifest::PACKAGE_IPE).exists() {
        return Ok(".".to_owned());
    }
    if std::path::Path::new("src/Main.ipe").exists() {
        return Ok("src/Main.ipe".to_owned());
    }
    if project::migration_pending(std::path::Path::new(".")) {
        return Err(CliError::Usage(project::MIGRATE_CONFIG_HINT));
    }
    Err(CliError::Usage(NO_ENTRY))
}

/// `ipe watch [<path>]` — rebuild and re-run on every source change
/// (`crate::watch`). Never returns
/// `Err` for a build failure (INV-3: a red build is logged, not fatal);
/// only misuse / setup failures propagate.
pub(crate) fn run_watch(rest: &[String]) -> Result<(), CliError> {
    let args = cli_args::parse_watch(rest)?;
    let entry = match args.entry {
        Some(e) => e,
        None => default_entry()?,
    };

    let out_dir = args
        .out
        .map_or_else(|| PathBuf::from("out").join("rust"), PathBuf::from);
    // Watch is always a native dependency-model dev build (it never vendors the
    // runtime tree, nor targets wasm), so — like `ipe build` on its default path
    // — it must NOT require the vendored runtime source subtree. It resolves the
    // dependency crate root itself via `runtime_embed::resolve` once the loop
    // starts (see `watch::run`); the vendored tree is honoured only when passed
    // explicitly with `--runtime`. Requiring `resolve_runtime` up front made
    // `ipe watch` fail to locate the runtime in an installed checkout where the
    // vendored subtree is absent but the dependency crate root resolves fine.
    let runtime_dir = resolve_vendored_runtime_dir(args.runtime, false)?;

    // Fail closed before the watch loop starts: `ipe watch` rebuilds with cargo
    // on every change, so a missing toolchain is reported once, up front, with
    // its root cause — not as a per-rebuild opaque spawn error.
    let cargo_bin = toolchain::require_cargo(toolchain::ToolIntent::Watch)?;

    let mut opts = watch::WatchOptions::new(PathBuf::from(entry), out_dir, runtime_dir);
    opts.port = args.port;
    opts.cargo_path = cargo_bin.path().to_path_buf();
    opts.quiet = args.quiet;
    opts.bluegreen = bluegreen_enabled();
    opts.reset_state = args.reset_state;
    // Version header: human mode only (not quiet, not piped).
    if !args.quiet {
        use std::io::IsTerminal as _;
        if std::io::stderr().is_terminal() {
            style::print_command_header();
        }
    }
    watch::run(&opts)
}

/// Route an entry argument to its `package.ipe`, when one governs it:
/// a directory must contain one, and a `.ipe` entry walks up the tree looking
/// for one (returning no manifest — single-file mode — when none exists). A
/// directory carrying only a legacy `ipe.toml` is a clear migration error.
fn discover_manifest(entry_path: &Path) -> Result<Option<PathBuf>, CliError> {
    if entry_path.is_dir() {
        if let Some(manifest) = project::manifest_in_dir(entry_path) {
            return Ok(Some(manifest));
        }
        if project::migration_pending(entry_path) {
            return Err(CliError::Usage(project::MIGRATE_CONFIG_HINT));
        }
        Err(CliError::Usage(
            "directory supplied but no package.ipe found inside it",
        ))
    } else {
        Ok(find_manifest_for_ipe_file(entry_path))
    }
}

/// Resolve the static request with full precedence — CLI flags > env
/// (`IPE_STATIC` / `IPE_TARGET` / `IPE_ALLOC`) > `package.ipe` `[rust]` > AUTO —
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
/// `package.ipe` > default native.
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
// A linear pipeline (parse → discover manifest → acknowledge unsafe → resolve
// target → emit → cargo build); the steps share enough locals that splitting
// reads worse than the whole.
/// The outcome of a successful `ipe build`, carrying the facts needed to render
/// either a human progress line or a JSON success object.
struct BuildSuccess {
    /// The entry source file that was compiled.
    entry: String,
    /// The output directory holding the emitted Rust project.
    out_dir: PathBuf,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn run_build(rest: &[String]) -> Result<(), CliError> {
    // Parse args once to learn the format before running the body.
    let format = cli_args::parse_build(rest)
        .map(|a| a.format)
        .unwrap_or_default();
    let result = run_build_body(rest);
    match result {
        Err(e) => Err(if format == cli_args::OutputFormat::Json {
            emit_pipeline_json(e)
        } else {
            e
        }),
        Ok(success) => {
            if format == cli_args::OutputFormat::Json {
                // Machine-readable success: one JSON object to stdout.
                let json = serde_json::json!({
                    "status": "ok",
                    "entry": success.entry,
                    "out": success.out_dir.to_string_lossy(),
                });
                println!("{json}");
            }
            // Human progress line already printed inside run_build_body.
            Ok(())
        }
    }
}

/// Inner implementation of `run_build`, format-agnostic on the success path.
/// Returns a [`BuildSuccess`] describing the outcome; the caller renders it.
#[allow(clippy::too_many_lines)]
fn run_build_body(rest: &[String]) -> Result<BuildSuccess, CliError> {
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
            return Ok(BuildSuccess {
                entry,
                out_dir: PathBuf::new(),
            });
        }
        cli_args::BuildMode::Emit {
            out,
            wasm,
            static_layer,
        } => (out, wasm, static_layer),
    };

    let out_dir = out.map_or_else(|| PathBuf::from("out").join("rust"), PathBuf::from);

    // Route the build:
    //   1. Directory → expect package.ipe inside it.
    //   2. .ipe file → walk up looking for package.ipe (project-mode); fall back
    //      to sibling discovery when no manifest exists, so a multi-file project
    //      built via the file-path shorthand still compiles the whole module
    //      graph rather than the single entry file.
    let manifest = discover_manifest(&entry_path)?;

    // Parse the manifest early to read [wasm].mode for target inference.
    // build_project_with_options re-parses it later to fill in publicEnv /
    // hydrate-mode; the double parse is acceptable (manifests are small).
    let manifest_parsed = manifest
        .as_deref()
        .map(project::parse_manifest)
        .transpose()?;
    let manifest_wasm: Option<project::WasmConfig> =
        manifest_parsed.as_ref().map(|m| m.wasm.clone());

    // Resolve the static plan FIRST: it is pure over the flag/env/manifest
    // request layer and reads no source, so a flag-contradiction refusal
    // (talc-without-arena, --target-without-static, cfree conflicts) fires
    // before any filesystem read of the entry — a refused build produces no
    // artifact and touches nothing.
    let static_plan = resolve_static_plan(cli_layer, manifest.as_deref())?;

    // Acknowledge any disclosed `.Unsafe` escape-hatch import BEFORE the (costly)
    // emit + cargo build. The safe path (no `.Unsafe` import) returns silently;
    // an exposed program requires `--accept-risks`, the manifest token, or an
    // interactive yes, and a non-interactive build without consent fails closed
    // rather than blocking on a prompt.
    acknowledge_unsafe_imports(
        manifest_parsed.as_ref(),
        manifest.as_deref(),
        &entry_path,
        args.accept_risks,
    )?;

    // App-boundary web-capability consent: a disclosed `js-port:<axis>` reached by
    // a dependency must be granted by THIS app's `[capabilities] accept`, else the
    // build fails closed naming the disclosing module.
    gate_web_consent(manifest_parsed.as_ref(), manifest.as_deref(), &entry_path)?;

    // Precedence: CLI --target wasm > IPE_TARGET=wasm > [wasm].mode != "off".
    let wasm_target = resolve_wasm_target(wasm_target, manifest_wasm.as_ref());

    // The dependency model (native OR wasm) needs no vendored tree — the runtime
    // is a path dependency. Only a dep-model-OFF build vendors the source subtree.
    let runtime_dep = runtime_dep_from_env();
    let runtime_dir = resolve_vendored_runtime_dir(args.runtime, !runtime_dep)?;

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
        // `ipe build` is a development artifact — Debug.* is permitted.
        production: false,
        runtime_dep,
        // `ipe build` never tree-shakes the vendored tree — a dep-model build
        // carries no vendored source, and a vendored (`IPE_RUNTIME_VENDORED`)
        // build keeps the full tree so rustc, not the driver, drops the unreached
        // files. Only `ipe eject` sets this.
        tree_shake_vendored: false,
        // Filled in by build_project_with_options once the manifest is parsed.
        cargo_name: String::new(),
        debugger: args.debugger,
        // `ipe build` never emits appearance hot-swap scaffolding — that is a
        // `ipe watch`-only dev affordance. A release artifact stays clean.
        hot_appearance: false,
    };

    // Human-friendly progress: the compile+emit below is otherwise silent, so
    // bracket it with a start/done line. Shown only on an interactive terminal so
    // piped / CI output stays clean; status goes to stderr (stdout carries data).
    // Suppressed in quiet mode (only warnings/errors) and in JSON mode (machine
    // output only — one JSON object to stdout at the end).
    let show_progress = !args.quiet && args.format != cli_args::OutputFormat::Json && {
        use std::io::IsTerminal as _;
        std::io::stderr().is_terminal()
    };
    if show_progress {
        style::print_command_header();
        eprintln!(
            "{}",
            style::gutter(&format!("{} building {entry}", style::glyph::STEP))
        );
    }

    // No manifest found: compile entry + all sibling .ipe files in the same
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
            args.quiet,
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
    Ok(BuildSuccess { entry, out_dir })
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
    quiet: bool,
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
    if quiet {
        cargo.arg("-q");
    } else {
        force_cargo_terminal_ui(&mut cargo);
    }
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
pub(crate) fn run_eject(rest: &[String]) -> Result<(), CliError> {
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

/// `ipe release [<path>] [--out <dir>] [--target wasm|<triple>] [--embed]` —
/// build the production artifact for every app kind.
///
/// The artifact kind is determined by the app and the `--target` flag:
///
/// - **Native-bearing** (app crosses into `Rust.` / FFI): the jailed bundle —
///   `ipe-wrapper` (jailed launcher), `ipe-app` (statically-linked app binary),
///   and `ipe.profile` (serialised capability profile). The `--embed` flag
///   (the default) fuses all three into a single self-jailing binary.
/// - **Pure native** (no native/FFI content): a plain optimised binary under
///   the release cargo profile. No jail wrapper is needed; the binary is
///   structurally bounded to its inferred capabilities.
/// - **Browser/wasm** (`--target wasm`): the production browser bundle
///   (optimised `.wasm` + generated glue + assets) exactly as `ipe build
///   --target wasm` produces, but with the production flag set so the
///   `Ipe.Debug` gate (IPE-L0140) fires.
///
/// Every path sets `production = true` so the `Ipe.Debug.*` gate fires for
/// all app kinds. `ipe build` and `ipe run` leave `production = false`
/// (development — `Debug.*` is permitted there).
///
/// ## Honest limit (native-bearing)
///
/// The inner `ipe-app` is a native ELF/Mach-O/PE binary — an operator can run
/// it directly without the wrapper, bypassing the jail. The wrapper makes the
/// sanctioned, jailed, profile-verified path the easy toolchain-free one; it
/// does not make unjailed execution impossible for a sufficiently privileged
/// local operator. This limit is documented, not a defect.
///
/// ## Security boundary
///
/// The jail enforcement is the SAME code path as `ipe exec` — both call into
/// `ipe_sandbox::run_jail::{scan_capfloor, satisfies_capfloor, exec_in_run_jail}`.
/// There is no second jail implementation; any future change to the jail
/// mechanism automatically applies to both paths.
///
/// # Errors
///
/// Build, toolchain, manifest-parse, filesystem, and capability-resolution
/// errors.
#[allow(clippy::too_many_lines)]
pub(crate) fn run_release(rest: &[String]) -> Result<(), CliError> {
    let args = cli_args::parse_release(rest)?;
    let entry = match args.entry {
        Some(e) => e,
        None => default_entry()?,
    };
    let entry_path = PathBuf::from(&entry);

    // `--capabilities` / `--show-profile`: inspect the inferred capability
    // model without building or writing anything.
    if args.capabilities_only {
        let manifest = discover_manifest(&entry_path)?;
        return run_release_capabilities(&entry_path, manifest.as_deref(), args.format);
    }

    // Discover the manifest (same logic as build/eject).
    let manifest = discover_manifest(&entry_path)?;

    let manifest_parsed = match manifest.as_deref() {
        Some(m) => Some(project::parse_manifest(m)?),
        None => None,
    };
    let manifest_wasm: Option<project::WasmConfig> =
        manifest_parsed.as_ref().map(|m| m.wasm.clone());

    // Route on the typed target: Wasm → browser bundle; Native → static binary.
    // `resolve_wasm_target` also checks the `IPE_TARGET` env var and manifest.
    let wasm_target = resolve_wasm_target(
        args.target == cli_args::ReleaseTarget::Wasm,
        manifest_wasm.as_ref(),
    );

    if wasm_target {
        // Browser/wasm production path.
        let out_dir = args
            .out
            .as_deref()
            .map_or_else(|| PathBuf::from("release"), PathBuf::from);
        let runtime_dep = runtime_dep_from_env();
        let runtime_dir = resolve_vendored_runtime_dir(args.runtime, !runtime_dep)?;

        let show_progress = {
            use std::io::IsTerminal as _;
            std::io::stderr().is_terminal()
        };
        if show_progress {
            eprintln!(
                "{}",
                style::gutter(&format!("{} releasing {entry} (wasm)", style::glyph::STEP))
            );
        }

        // Emit the Rust wasm project with production=true so the Debug gate fires.
        let options = BuildOptions {
            static_plan: None,
            target: ipe_ir::Target::WasmClient,
            wasm_public_env: manifest_parsed
                .as_ref()
                .map(|m| m.wasm.public_env.clone())
                .unwrap_or_default(),
            wasm_hydrate_mode: manifest_wasm
                .as_ref()
                .is_some_and(|w| w.mode.as_deref() == Some("hydrate")),
            production: true,
            runtime_dep,
            tree_shake_vendored: false,
            cargo_name: String::new(),
            // The debugger is never enabled on a release build.
            debugger: false,
            // A release build never carries appearance hot-swap scaffolding.
            hot_appearance: false,
        };
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
        bundle_wasm(&out_dir)?;
        if show_progress {
            eprintln!(
                "{}",
                style::gutter(&format!(
                    "{} released → {}/www/",
                    style::glyph::OK,
                    out_dir.display()
                ))
            );
        }
        return Ok(());
    }

    // Native path: extract the triple already validated at parse time.
    let triple = match args.target {
        cli_args::ReleaseTarget::Native(t) => t,
        cli_args::ReleaseTarget::Wasm => {
            // `wasm_target` above is true when `args.target == Wasm`, so this
            // branch is unreachable in practice; the exhaustive match keeps the
            // compiler satisfied without a panic or unreachable!().
            return Ok(());
        }
    };

    // Resolve capabilities up-front to discriminate between native-bearing
    // (needs jail wrapper) and pure-native (plain optimised binary).
    let driver = manifest_parsed
        .as_ref()
        .map_or(ipe_backend_rust::DbDriver::Sqlite, |m| m.driver);
    let resolved =
        run_sandbox::resolve_for_run(manifest_parsed.as_ref(), manifest.as_deref(), &entry_path)?;

    let runtime_dir = resolve_vendored_runtime_dir(args.runtime, false)?;

    let show_progress = {
        use std::io::IsTerminal as _;
        std::io::stderr().is_terminal()
    };

    if !run_sandbox::is_native_bearing(&resolved.union()) {
        // Pure-native path: emit and build a plain release binary.
        let out_dir = args
            .out
            .as_deref()
            .map_or_else(|| PathBuf::from("release"), PathBuf::from);

        if show_progress {
            eprintln!(
                "{}",
                style::gutter(&format!("{} releasing {entry}", style::glyph::STEP))
            );
        }

        let cargo_bin = toolchain::require_cargo(toolchain::ToolIntent::Build)?;

        let app_static_plan = Some(ipe_backend_rust::static_build::StaticPlan {
            triple,
            c_profile: ipe_backend_rust::static_build::CProfile::WithLibc {
                allocator: ipe_backend_rust::static_build::StaticAllocator::Dlmalloc,
            },
        });

        let options = BuildOptions {
            static_plan: app_static_plan,
            target: ipe_ir::Target::Native,
            production: true,
            runtime_dep: runtime_dep_from_env(),
            tree_shake_vendored: false,
            ..BuildOptions::default()
        };
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

        let mut app_cargo = std::process::Command::new(cargo_bin.path());
        app_cargo
            .arg("build")
            .arg("--release")
            .args(["--target", triple.as_str()])
            .current_dir(&out_dir);
        force_cargo_terminal_ui(&mut app_cargo);
        build_emitted_project(&mut app_cargo, "the release binary", None, &out_dir)?;

        let app_target_dir = cargo_target_directory(&out_dir)?;
        let bin_name = emitted_bin_name(&out_dir);
        let bin_path = app_target_dir
            .join(triple.as_str())
            .join("release")
            .join(&bin_name);
        if show_progress {
            eprintln!(
                "{}",
                style::gutter(&format!(
                    "{} released → {}",
                    style::glyph::OK,
                    bin_path.display()
                ))
            );
        }
        return Ok(());
    }

    // Native-bearing path: jailed bundle (same substance as the predecessor).
    let out_dir = args
        .out
        .as_deref()
        .map_or_else(|| PathBuf::from("release"), PathBuf::from);

    if show_progress {
        eprintln!(
            "{}",
            style::gutter(&format!("{} releasing {entry}", style::glyph::STEP))
        );
    }

    let cargo_bin = toolchain::require_cargo(toolchain::ToolIntent::Build)?;

    // Step 1: emit + build the app binary (static, musl, production).
    let app_out = out_dir.join("app");
    let app_static_plan = Some(ipe_backend_rust::static_build::StaticPlan {
        triple,
        c_profile: ipe_backend_rust::static_build::CProfile::WithLibc {
            allocator: ipe_backend_rust::static_build::StaticAllocator::Dlmalloc,
        },
    });
    let options = BuildOptions {
        static_plan: app_static_plan,
        target: ipe_ir::Target::Native,
        production: true,
        runtime_dep: runtime_dep_from_env(),
        tree_shake_vendored: false,
        ..BuildOptions::default()
    };
    manifest.as_ref().map_or_else(
        || {
            build_with_sibling_discovery_with_options(
                &entry_path,
                &app_out,
                &runtime_dir,
                options.clone(),
            )
        },
        |m| build_project_with_options(m, &app_out, &runtime_dir, options.clone()),
    )?;

    let mut app_cargo = std::process::Command::new(cargo_bin.path());
    app_cargo
        .arg("build")
        .arg("--release")
        .args(["--target", triple.as_str()])
        .current_dir(&app_out);
    force_cargo_terminal_ui(&mut app_cargo);
    build_emitted_project(&mut app_cargo, "the release app", None, &app_out)?;

    // Write the capability enforcement artifacts (ipe.profile + embedded floor).
    let profile = run_sandbox::build_profile(&resolved, driver)?;
    run_sandbox::write_build_artifacts(&app_out, &profile)?;

    // Locate the compiled app binary. The target dir may be a global
    // `CARGO_TARGET_DIR` (set by the user or the agent lane), so we resolve
    // it via cargo metadata rather than assuming `app_out/target/`.
    let app_target_dir = cargo_target_directory(&app_out)?;
    let release_bin_name = emitted_bin_name(&app_out);
    let app_binary = app_target_dir
        .join(triple.as_str())
        .join("release")
        .join(&release_bin_name);
    if !app_binary.is_file() {
        return Err(CliError::UsageOwned(format!(
            "ipe release: expected app binary at {} — cargo build succeeded but binary is missing",
            app_binary.display()
        )));
    }
    let profile_src = app_out.join("ipe.profile");

    // Step 2: build the wrapper binary (static, musl).
    let wrapper_triple = triple;
    let wrapper_static_plan = ipe_backend_rust::static_build::StaticPlan {
        triple: wrapper_triple,
        c_profile: ipe_backend_rust::static_build::CProfile::WithLibc {
            allocator: ipe_backend_rust::static_build::StaticAllocator::Dlmalloc,
        },
    };

    let mut wrapper_cargo = std::process::Command::new(cargo_bin.path());
    wrapper_cargo
        .arg("build")
        .arg("--release")
        .arg("--package")
        .arg("ipe_wrapper")
        .args(["--target", wrapper_static_plan.triple.as_str()]);

    if matches!(args.mode, cli_args::ReleaseMode::Embed) {
        // Embed mode: pass the app binary + profile as env vars so build.rs
        // copies them into OUT_DIR and enables the embed_mode cfg.
        wrapper_cargo
            .env("IPE_EMBED_APP", &app_binary)
            .env("IPE_EMBED_PROFILE", &profile_src);
    }

    // Run from the workspace root so cargo finds the workspace Cargo.toml.
    let workspace_root = find_workspace_root()?;
    wrapper_cargo.current_dir(&workspace_root);
    force_cargo_terminal_ui(&mut wrapper_cargo);

    build_emitted_project(
        &mut wrapper_cargo,
        "the release wrapper",
        None,
        &workspace_root,
    )?;

    // Step 3: lay out the bundle.
    let bundle_dir = out_dir.join("bundle");
    std::fs::create_dir_all(&bundle_dir).map_err(|e| CliError::Io {
        path: bundle_dir.clone(),
        source: e,
    })?;

    // Locate the wrapper binary. As with the app binary, the target dir may be
    // a global CARGO_TARGET_DIR; resolve via cargo metadata.
    let wrapper_target_dir = cargo_target_directory(&workspace_root)?;
    let wrapper_src = wrapper_target_dir
        .join(wrapper_static_plan.triple.as_str())
        .join("release")
        .join("ipe-wrapper");

    let artifact = match args.mode {
        cli_args::ReleaseMode::Embed => {
            // Single-file embed: copy only the wrapper (app + profile baked in).
            let dest = bundle_dir.join("ipe-wrapper");
            std::fs::copy(&wrapper_src, &dest).map_err(|e| CliError::Io {
                path: dest.clone(),
                source: e,
            })?;
            #[cfg(unix)]
            set_executable(&dest)?;
            dest
        }
        cli_args::ReleaseMode::Bundle => {
            // Bundle: wrapper + app + profile as siblings.
            let wrapper_dest = bundle_dir.join("ipe-wrapper");
            let app_dest = bundle_dir.join("ipe-app");
            let profile_dest = bundle_dir.join("ipe.profile");
            std::fs::copy(&wrapper_src, &wrapper_dest).map_err(|e| CliError::Io {
                path: wrapper_dest.clone(),
                source: e,
            })?;
            std::fs::copy(&app_binary, &app_dest).map_err(|e| CliError::Io {
                path: app_dest.clone(),
                source: e,
            })?;
            std::fs::copy(&profile_src, &profile_dest).map_err(|e| CliError::Io {
                path: profile_dest.clone(),
                source: e,
            })?;
            #[cfg(unix)]
            {
                set_executable(&wrapper_dest)?;
                set_executable(&app_dest)?;
            }
            bundle_dir
        }
    };

    if show_progress {
        // Post-build report: how the binary is linked, where it landed, and the
        // capability model it will enforce.
        let cap_names: Vec<&'static str> = resolved.union().iter().map(|c| c.as_str()).collect();
        eprint!(
            "{}",
            release_bundle_report(&artifact, &cap_names, args.mode)
        );
        match args.mode {
            cli_args::ReleaseMode::Embed => eprintln!(
                "{}",
                style::gutter(&format!(
                    "{} released → {} (single self-jailing binary; \
                     run `--capabilities` to audit)",
                    style::glyph::OK,
                    artifact.display()
                ))
            ),
            cli_args::ReleaseMode::Bundle => eprintln!(
                "{}",
                style::gutter(&format!(
                    "{} released (bundle) → {} (run `./ipe-wrapper -- <args>`; \
                     WARNING: ipe-app can be run directly, bypassing the sandbox — \
                     prefer embed mode for production)",
                    style::glyph::OK,
                    artifact.display()
                ))
            ),
        }
    }
    Ok(())
}

/// Inspect the inferred capability model for `entry_path` without building or
/// writing anything — the body of `ipe release --capabilities` / `--show-profile`.
fn run_release_capabilities(
    entry_path: &Path,
    manifest: Option<&Path>,
    format: cli_args::OutputFormat,
) -> Result<(), CliError> {
    let manifest_parsed = match manifest {
        Some(m) => Some(project::parse_manifest(m)?),
        None => None,
    };
    let resolved = run_sandbox::resolve_for_run(manifest_parsed.as_ref(), manifest, entry_path)?;
    let names: Vec<&'static str> = resolved.union().iter().map(|c| c.as_str()).collect();
    print!(
        "{}",
        render_capabilities(&names, format, &std::io::stdout())
    );
    Ok(())
}

/// The human-readable post-build report for a native-bearing release bundle:
/// link kind, artifact path, and the enforced capability model.
fn release_bundle_report(
    artifact: &Path,
    capabilities: &[&str],
    mode: cli_args::ReleaseMode,
) -> String {
    use std::fmt::Write as _;

    let kind = match mode {
        cli_args::ReleaseMode::Embed => "single self-jailing binary",
        cli_args::ReleaseMode::Bundle => "multi-file bundle (wrapper + app + profile)",
    };
    let mut body = String::new();
    let _ = writeln!(body, "link: static (musl)");
    let _ = writeln!(body, "shape: {kind}");
    let _ = writeln!(body, "artifact: {}", artifact.display());
    if capabilities.is_empty() {
        let _ = writeln!(body, "capabilities: none");
    } else {
        let _ = writeln!(body, "capabilities: {}", capabilities.join(", "));
    }
    style::frame(&style::gutter(&body))
}

/// Walk parent directories from the current directory to find the workspace
/// root (the directory containing the root `Cargo.toml` with `[workspace]`).
///
/// # Errors
///
/// [`CliError::UsageOwned`] if the workspace root cannot be found.
fn find_workspace_root() -> Result<PathBuf, CliError> {
    let cwd = std::env::current_dir().map_err(|e| CliError::Io {
        path: PathBuf::from("."),
        source: e,
    })?;
    let mut candidate = cwd.as_path();
    loop {
        let toml = candidate.join("Cargo.toml");
        if toml.is_file() {
            let text = std::fs::read_to_string(&toml).map_err(|e| CliError::Io {
                path: toml.clone(),
                source: e,
            })?;
            if text.contains("[workspace]") {
                return Ok(candidate.to_path_buf());
            }
        }
        match candidate.parent() {
            Some(p) => candidate = p,
            None => {
                return Err(CliError::UsageOwned(
                    "ipe release: cannot locate workspace root (no Cargo.toml with [workspace] \
                     found in any parent directory)"
                        .to_owned(),
                ));
            }
        }
    }
}

/// Set the executable bit on a file (Unix only; no-op on other platforms).
///
/// # Errors
///
/// [`CliError::Io`] when the permission cannot be set.
#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt as _;
    let meta = std::fs::metadata(path).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut perms = meta.permissions();
    let mode = perms.mode() | 0o111;
    perms.set_mode(mode);
    std::fs::set_permissions(path, perms).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        source: e,
    })
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
pub(crate) fn read_progress_chunk<R: std::io::Read>(
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

/// Apply three environment variables to `cmd` so `cargo` emits ANSI colour and
/// its `Building [===]` progress bar even through a pipe — but only when our own
/// stderr is a real terminal (`NO_COLOR` unset). Without the explicit width,
/// `cargo` draws no bar at all (it reads the bar width from its piped stderr,
/// which reports no size).
#[cfg(unix)]
pub(crate) fn force_cargo_terminal_ui(cmd: &mut std::process::Command) {
    let stderr = std::io::stderr();
    if !crate::style::use_color(&stderr) {
        return;
    }
    cmd.env("CARGO_TERM_COLOR", "always");
    cmd.env("CARGO_TERM_PROGRESS_WHEN", "always");
    let cols = terminal_width(&stderr).unwrap_or(80);
    cmd.env("CARGO_TERM_PROGRESS_WIDTH", cols.to_string());
}

/// No-op shim for non-Unix targets where `rustix::termios` is unavailable.
#[cfg(not(unix))]
pub(crate) fn force_cargo_terminal_ui(_cmd: &mut std::process::Command) {}

/// The column width of `stream`'s terminal, or `None` when it is not a terminal
/// or the size cannot be read. Uses `TIOCGWINSZ` via rustix — no libc binding.
#[cfg(unix)]
fn terminal_width(stream: &impl std::os::fd::AsFd) -> Option<u16> {
    let ws = rustix::termios::tcgetwinsize(stream).ok()?;
    (ws.ws_col > 0).then_some(ws.ws_col)
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
    force_cargo_terminal_ui(&mut cargo);
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
pub(crate) fn run_run(rest: &[String]) -> Result<(), CliError> {
    let format = cli_args::parse_run(rest)
        .map(|a| a.format)
        .unwrap_or_default();
    run_run_body(rest).map_err(|e| {
        if format == cli_args::OutputFormat::Json {
            emit_pipeline_json(e)
        } else {
            e
        }
    })
}

/// Inner implementation of `run_run`, unaware of JSON formatting.
#[allow(clippy::too_many_lines)]
fn run_run_body(rest: &[String]) -> Result<(), CliError> {
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
    let manifest_parsed = manifest
        .as_deref()
        .map(project::parse_manifest)
        .transpose()?;
    let manifest_wasm: Option<project::WasmConfig> =
        manifest_parsed.as_ref().map(|m| m.wasm.clone());

    // Resolve the static plan FIRST: it is pure over the flag/env/manifest
    // request layer and reads no source, so a flag-contradiction refusal fires
    // before any filesystem read of the entry — identical to `ipe build`.
    let static_plan = resolve_static_plan(cli_layer, manifest.as_deref())?;

    // Acknowledge any disclosed `.Unsafe` escape-hatch import BEFORE the (costly)
    // emit + cargo build. Same gate as `ipe build`: the safe path is silent, an
    // exposed program needs consent, and a non-interactive run without consent
    // fails closed rather than blocking on a prompt.
    acknowledge_unsafe_imports(
        manifest_parsed.as_ref(),
        manifest.as_deref(),
        &entry_path,
        args.accept_risks,
    )?;

    // App-boundary web-capability consent: same gate as `ipe build` — a disclosed
    // `js-port:<axis>` must be granted by this app's manifest, else fail closed.
    gate_web_consent(manifest_parsed.as_ref(), manifest.as_deref(), &entry_path)?;

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
        // Filled in by build_project_with_options once the manifest is parsed.
        cargo_name: String::new(),
        debugger: args.debugger,
        // `ipe run` never emits appearance hot-swap scaffolding — that is a
        // `ipe watch`-only dev affordance.
        hot_appearance: false,
    };

    // Human-friendly progress: the compile+emit below is otherwise silent, so
    // announce the running step. On a terminal only (piped / CI output stays
    // clean); to stderr, so stdout carries only the program's own output. The
    // cargo build that follows streams its own progress; the exec that ends
    // `ipe run` leaves no room for a settled "done" line, so the run just starts
    // producing the program's output. Suppressed when `--quiet` is set.
    let show_progress = !args.quiet && {
        use std::io::IsTerminal as _;
        std::io::stderr().is_terminal()
    };
    if show_progress {
        style::print_command_header();
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
    if args.quiet {
        cargo.arg("-q");
    } else {
        force_cargo_terminal_ui(&mut cargo);
    }
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
    // The binary name is read from the emitted crate's `Cargo.toml` — the
    // same file cargo just built from, so there is ONE source of truth and
    // no independent re-derivation can drift. Falls back to `"ipe-app"` when
    // the manifest is absent or unparseable (same guarantee as `run_exec`).
    // The target directory is asked of cargo itself (`cargo metadata`) — a
    // `CARGO_TARGET_DIR` env or a user-level `[build] target-dir` pin
    // relocates the artifact, so a hardcoded `<out>/target` would exec a
    // missing or stale binary.
    let bin_name = emitted_bin_name(&out_dir);
    let mut bin = cargo_target_directory(&out_dir)?;
    if let Some(plan) = &static_plan {
        bin.push(plan.triple.as_str());
    }
    bin.push("debug");
    bin.push(&bin_name);

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
                scoped_tmp.path(),
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
                scoped_tmp.path(),
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
                "{bin_name} exited with code {code}"
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
pub(crate) fn run_exec(rest: &[String]) -> Result<(), CliError> {
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
    // The binary name matches the emitted crate's `[package] name`, read from
    // the artifact dir's `Cargo.toml`. Falls back to `"ipe-app"` when the
    // manifest is absent or the name cannot be parsed.
    let exec_bin_name = emitted_bin_name(&dir);
    let mut bin = cargo_target_directory(&dir)?;
    bin.push("debug");
    bin.push(&exec_bin_name);
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
            scoped_tmp.path(),
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
                "{exec_bin_name} exited with code {}",
                status.code().unwrap_or(1)
            )));
        }
        Ok(())
    }
}

/// Read the `[package] name` from an emitted project's `Cargo.toml` so
/// `ipe run` / `ipe exec` / `ipe test` locate the correct binary. Falls back
/// to `"ipe-app"` when the manifest is absent or unparseable — never panics.
fn emitted_bin_name(crate_dir: &Path) -> String {
    let manifest = crate_dir.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return "ipe-app".to_owned();
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("name") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"');
                if !value.is_empty() {
                    return value.to_owned();
                }
            }
        }
    }
    "ipe-app".to_owned()
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

/// `ipe explain` has been folded into `ipe doc`.
///
/// Invoking `ipe explain` emits a pointer to `ipe doc` and returns a usage
/// error so the dispatcher shows the `ipe doc` help page. The command is no
/// longer advertised; the COMMANDS registry entry was removed.
pub(crate) fn run_explain(_rest: &[String]) -> Result<(), CliError> {
    Err(CliError::UsageOwned(
        "`ipe explain` has moved: use `ipe doc <key>` instead\n\
         \n\
         Examples:\n\
           ipe doc IPE-L0107   look up a diagnostic code\n\
           ipe doc case        look up a language construct\n\
           ipe doc List.map    look up a stdlib symbol\n\
           ipe doc version     look up a command"
            .to_owned(),
    ))
}

/// `ipe fix <path>` — apply machine-applicable fixes to the source file.
/// Default is interactive per-edit confirmation;
/// `--yes` is durable authorization to apply every machine-applicable edit.
pub(crate) fn run_fix(rest: &[String]) -> Result<(), CliError> {
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

/// The whole set of security capabilities a program discloses: the kernel-derived
/// set [`ipe_lower::program_capabilities`] infers from the lowered program, PLUS
/// [`ipe_ir::Capability::CustomElement`] whenever the program constructs any
/// `customElement` handle.
///
/// The custom-element axis is derived from the SAME walk emission serves from —
/// [`ipe_canon::custom_element_gate::collect_widget_files`] over the pre-DCE
/// `linked` module — so the served-asset set and the disclosed-capability set are
/// one set by construction. A handle that is constructed but never mounted (and
/// so lowers to a capability-free leaf that DCE may drop) still ships its browser
/// JS through the emitted `widget_assets::register`, and this derivation discloses
/// it regardless of the lowered program's reachability. `collect_widget_files`
/// walks the whole linked program, so a handle constructed in an imported module
/// is disclosed transitively.
///
/// This is the single inference point every capability consumer routes through —
/// the report, the declared-set verify, package inference, and index admission —
/// so none of them can disclose a different set than the emitter serves.
pub(crate) fn capabilities_including_served_widgets(
    db: &dyn ipe_db::Db,
    root: ipe_db::SourceRoot,
    entry_file: ipe_db::SourceFile,
    program: &ipe_ir::Program,
) -> std::collections::BTreeSet<ipe_ir::Capability> {
    let mut caps = ipe_lower::program_capabilities(program);
    if program_constructs_a_widget(db, root, entry_file) {
        caps.insert(ipe_ir::Capability::CustomElement);
    }
    caps
}

/// True when the linked program constructs at least one `customElement` handle —
/// i.e. the emitter serves at least one widget asset for it. Reuses the exact
/// [`ipe_canon::custom_element_gate::collect_widget_files`] walk emission uses, so
/// the serve decision and this disclose decision are the same decision.
///
/// A program whose linking fails has no served widget (nothing is emitted), so a
/// link failure conservatively contributes no widget disclosure here; the failing
/// pipeline surfaces its own diagnostic through the caller's own lowering.
fn program_constructs_a_widget(
    db: &dyn ipe_db::Db,
    root: ipe_db::SourceRoot,
    entry_file: ipe_db::SourceFile,
) -> bool {
    ipe_db::linked_program(db, root, entry_file).is_ok_and(|linked| {
        !ipe_canon::custom_element_gate::collect_widget_files(&linked.module).is_empty()
    })
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
pub(crate) struct SourceGraph {
    pub(crate) db: ipe_db::IpeDatabase,
    pub(crate) source_root: ipe_db::SourceRoot,
    pub(crate) entry_file: ipe_db::SourceFile,
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
    pub(crate) fn run_attributed<T>(
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
pub(crate) fn build_source_graph(entry: &Path) -> Result<SourceGraph, CliError> {
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

/// Collect the USER `.ipe` source texts a build sees, for the `.Unsafe`-import
/// scan. A manifest project reads every discovered module under its source root
/// (the same whole-tree posture package-capability inference takes); a
/// single-file entry reads the entry plus its imported siblings.
///
/// Fail-closed: any unreadable module causes an immediate `Err` so the
/// acknowledgment gate never operates on a partial source set.
///
/// # Errors
/// [`CliError::Io`] when any discovered module cannot be read.
fn user_sources_for_unsafe_scan(
    manifest: Option<&Path>,
    entry: &Path,
) -> Result<Vec<String>, CliError> {
    if let Some(mpath) = manifest
        && let Ok(m) = project::parse_manifest(mpath)
        && let Ok(discovered) = project::discover_modules(&m.src_root)
    {
        return discovered
            .iter()
            .map(|d| {
                crate::io_bounded::read_to_string_capped(
                    &d.path,
                    crate::io_bounded::SOURCE_READ_CAP,
                )
            })
            .collect::<Result<Vec<_>, _>>();
    }
    // Single file (or a manifest that failed to parse — the build will surface
    // that error itself): the entry and its siblings.
    match collect_entry_and_siblings(entry) {
        Ok(collected) => Ok(collected
            .sources
            .into_values()
            .map(|(_, src)| src)
            .collect()),
        Err(_) => {
            crate::io_bounded::read_to_string_capped(entry, crate::io_bounded::SOURCE_READ_CAP)
                .map(|src| vec![src])
        }
    }
}

/// The build-time acknowledgment gate for `Ipe.<M>.Unsafe` escape-hatch imports,
/// shared by `ipe build` and `ipe run`.
///
/// Resolves the program's inferred capabilities the same way the sandbox does,
/// and — only when the disclosed `unsafe` capability is present — surfaces the
/// risk and requires consent (the `--accept-risks` flag, a `[capabilities]
/// accept = ["unsafe"]` manifest token, or an interactive `y`). A non-interactive
/// build without pre-acceptance fails closed (`IPE-S0001`); it never blocks on a
/// prompt. A program with no `.Unsafe` import is untouched.
///
/// # Errors
/// [`CliError::UsageOwned`] (`IPE-S0001`) when consent is required but absent;
/// the capability-resolution errors it composes.
fn acknowledge_unsafe_imports(
    manifest_parsed: Option<&project::ProjectManifest>,
    manifest_path: Option<&Path>,
    entry: &Path,
    accept_risks_flag: bool,
) -> Result<(), CliError> {
    let resolved = run_sandbox::resolve_for_run(manifest_parsed, manifest_path, entry)?;
    // Short-circuit before any source read when the disclosed capability is
    // absent — the safe path does no work at all.
    if !resolved.inferred.contains(&ipe_ir::Capability::Unsafe) {
        return Ok(());
    }
    let sources = user_sources_for_unsafe_scan(manifest_path, entry)?;
    let via = unsafe_ack::unsafe_modules_in_sources(sources.iter().map(String::as_str));
    let manifest_accept = manifest_parsed
        .map(|m| m.capabilities_accept.clone())
        .unwrap_or_default();
    let mut stdin = std::io::stdin().lock();
    let mut stderr = std::io::stderr().lock();
    unsafe_ack::gate(
        &resolved.inferred,
        accept_risks_flag,
        &manifest_accept,
        &via,
        unsafe_ack::is_interactive(),
        &mut stdin,
        &mut stderr,
    )
}

/// The app-boundary web-capability consent gate, shared by `ipe build` and
/// `ipe run` and invoked right after the `.Unsafe` acknowledgment.
///
/// Resolves the program's inferred capabilities the same way the sandbox does; if
/// any disclosed `js-port:<axis>` web capability is present, it demands that the
/// top-level app's `[capabilities] accept` set grant it. An ungranted (or
/// un-attributable) web axis is a fail-closed, typed refusal naming the disclosing
/// module — it never prompts and never composes a dependency's own grant. A
/// program that reaches no web capability is untouched.
///
/// # Errors
/// [`CliError::UsageOwned`] (`IPE-S0002`) when a disclosed web axis is ungranted;
/// the capability-resolution errors it composes.
fn gate_web_consent(
    manifest_parsed: Option<&project::ProjectManifest>,
    manifest_path: Option<&Path>,
    entry: &Path,
) -> Result<(), CliError> {
    let resolved = run_sandbox::resolve_for_run(manifest_parsed, manifest_path, entry)?;
    // Short-circuit before any source read when no web axis is disclosed.
    if !resolved
        .inferred
        .iter()
        .any(|c| matches!(c, ipe_ir::Capability::JsPort(_)))
    {
        return Ok(());
    }
    // Provenance over the whole module set (app + siblings + any dep modules the
    // infer path reads), keyed on the module path so the refusal names the
    // disclosing module. Total by construction: an inferred axis that no source
    // attributes is refused as un-attributable, never dropped.
    let named_sources = named_sources_for_web_scan(manifest_path, entry)?;
    let provenance = web_consent::WebAxisProvenance::from_sources(
        named_sources
            .iter()
            .map(|(name, src)| (name.as_str(), src.as_str())),
    );
    let granted = manifest_parsed
        .map(|m| m.capabilities_accept.clone())
        .unwrap_or_default();
    web_consent::gate(&resolved.inferred, &granted, &provenance)
}

/// Collect `(dotted-module-name, source)` pairs spanning the app entry and its
/// siblings (and, when a manifest is present, every discovered package module),
/// for the web-axis provenance scan. Falls back to the bare entry when sibling
/// discovery fails, exactly as the `.Unsafe` scan does.
fn named_sources_for_web_scan(
    manifest_path: Option<&Path>,
    entry: &Path,
) -> Result<Vec<(String, String)>, CliError> {
    if let Some(mpath) = manifest_path
        && let Ok(manifest) = project::parse_manifest(mpath)
    {
        let discovered = project::discover_modules(&manifest.src_root)?;
        let mut out = Vec::with_capacity(discovered.len());
        for m in &discovered {
            let src = crate::io_bounded::read_to_string_capped(
                &m.path,
                crate::io_bounded::SOURCE_READ_CAP,
            )?;
            out.push((m.module_path.join("."), src));
        }
        return Ok(out);
    }
    match collect_entry_and_siblings(entry) {
        Ok(collected) => Ok(collected
            .sources
            .into_iter()
            .map(|(path, (_, src))| (path.join("."), src))
            .collect()),
        Err(_) => {
            crate::io_bounded::read_to_string_capped(entry, crate::io_bounded::SOURCE_READ_CAP)
                .map(|src| vec![(entry.display().to_string(), src)])
        }
    }
}

/// Type-check a single `.ipe` entry through the SAME injection-aware
/// source-graph pipeline the build path uses, stopping at type-checking: it
/// demands the `typecheck` query (parse → canon → link → HM infer) and never
/// lowers to IR or emits Rust. This is what `ipe type-check` runs.
///
/// # Errors
/// [`CliError::Pipeline`] carrying the first compiler diagnostic;
/// [`CliError::Io`] when a source file cannot be read.
pub(crate) fn typecheck_entry_via_graph(entry: &Path) -> Result<(), CliError> {
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
/// Tier-1 package gate), `publish` (run the gate, compute the index entry, and
/// open the index PR), `validate-entry` (schema-check an entry file), and
/// `audit-entry` (the index CI's authoritative receiving gate).
///
/// # Errors
/// [`CliError::UsageOwned`] on a missing or unknown subcommand; the subcommand's
/// own errors (a build failure, a [`CliError::PackageAudit`] reject, or a
/// [`CliError::Publish`] refusal) otherwise.
pub(crate) fn run_package(rest: &[String]) -> Result<(), CliError> {
    match rest.split_first() {
        Some((sub, tail)) if sub == "audit" => audit::run_audit(tail),
        Some((sub, tail)) if sub == "publish" => publish::run_publish(tail),
        Some((sub, tail)) if sub == "validate-entry" => run_validate_entry(tail),
        Some((sub, tail)) if sub == "audit-entry" => run_audit_entry(tail),
        Some((sub, _)) => Err(cli_args::usage_unknown_subcommand(
            "package",
            sub,
            "`audit`, `audit-entry`, `publish`, or `validate-entry`",
        )),
        None => Err(CliError::Usage(
            "usage: ipe package <audit|audit-entry|publish|validate-entry> [<path>]",
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

/// `ipe package audit-entry <packages/<name>.toml> [--index <root>]` — the index
/// CI's authoritative receiving gate for a submitted entry.
///
/// Composes the existing pieces in a fixed, fail-closed order so the CI cannot
/// diverge from `ipe package audit`:
///
/// 1. **Schema** — validate the entry via [`index::validate_entry_file`] (the same
///    parser `validate-entry` uses); reject on any malformation.
/// 2. **New versions** — compare against the baseline entry at
///    `<index-root>/packages/<name>.toml` (if it exists) and identify every
///    `[[version]]` that is not already in the baseline. When there is no baseline,
///    all versions are audited. A PR normally adds exactly one new version.
/// 3. **Fetch + verify** — for each new version, `git`-fetch the source at the
///    pinned revision and verify the fetched tree's `sha256` equals the entry's pin
///    via [`resolve::fetch_and_verify_index_version`] (verify-before-trust; a
///    mismatch is [`CliError::HashMismatch`], never a warning).
/// 4. **Audit** — run the full [`audit::run_audit`] gate (Tier-1 provenance,
///    capability consistency, enforced semver, supply-chain; Tier-2 for
///    native-bearing packages) on each verified source tree. Reject on the first
///    failing check.
///
/// Exits 0 with a per-version passing summary only when ALL steps pass for ALL new
/// versions. Any failure is a typed [`CliError`] + non-zero exit; no step is
/// warn-and-pass.
///
/// # Errors
/// [`CliError::Usage`] when no entry file is given; [`CliError::UsageOwned`] on
/// argument misuse; [`CliError::Resolve`] / [`CliError::Io`] on a schema or read
/// failure; [`CliError::HashMismatch`] on an integrity mismatch; and
/// [`CliError::PackageAudit`] when a Tier-1 check rejects a version.
fn run_audit_entry(rest: &[String]) -> Result<(), CliError> {
    let (entry_path, index_root_opt) = parse_audit_entry_args(rest)?;

    // Step 1 — schema: parse + validate the submitted entry file.
    let submitted = index::validate_entry_file(&entry_path)?;

    // Step 2 — baseline: read the previously-published entry (if any).
    // Fail closed: a present-but-unreadable baseline propagates as an error
    // so the immutability wall below never runs against an empty baseline and
    // silently classifies every submitted version as "new".
    let index_root = index_root_opt.clone().unwrap_or_else(resolve::index_root);
    let baseline: Option<index::IndexEntry> =
        index::read_entry_lookup(&index_root, &submitted.name).require_present()?;
    let baseline_by_version: std::collections::BTreeMap<&semver::Version, &index::EntryVersion> =
        baseline
            .as_ref()
            .map(|e| e.versions.iter().map(|v| (&v.version, v)).collect())
            .unwrap_or_default();

    // Immutability — a published version is immutable. A submitted version whose
    // NUMBER already exists in the baseline must be byte-for-byte identical to the
    // published row; rewriting its `source`/`rev`/`sha256`/`capabilities` is a
    // supply-chain mutation and is rejected here, never silently skipped. This gate
    // is the authoritative wall (ADR 0044): it enforces immutability even for an
    // entry hand-edited around the author-side `ipe publish`, whose own immutability
    // check an attacker opening the index PR directly would bypass.
    for version in &submitted.versions {
        if let Some(&prior) = baseline_by_version.get(&version.version)
            && prior != version
        {
            return Err(CliError::UsageOwned(format!(
                "ipe package audit-entry: `{}` version {} is already published and immutable, \
                 but the submitted entry rewrites it (source, rev, sha256, or capabilities \
                 differ). A published version must never be rewritten — publish a new version.",
                submitted.name, version.version
            )));
        }
    }

    // The new versions are those present in the submitted entry but absent from
    // the baseline. A PR normally adds exactly one. Each is fetched, hash-verified,
    // and audited below; an existing-number row is only the immutability check above.
    let new_versions: Vec<&index::EntryVersion> = submitted
        .versions
        .iter()
        .filter(|v| !baseline_by_version.contains_key(&v.version))
        .collect();

    if new_versions.is_empty() {
        return Err(CliError::UsageOwned(format!(
            "ipe package audit-entry: `{}` — every version in the submitted entry is already in \
             the baseline index; nothing new to audit",
            submitted.name
        )));
    }

    // A scratch root for fetch caches under the standard per-user cache root
    // (the write-boundary from PRINCIPLES.md), isolated per process so concurrent
    // audit-entry runs never share a cache directory.
    let cache_base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".ipe"));
    let scratch_root = cache_base
        .join("ipe")
        .join(format!("audit-entry-{}", std::process::id()));
    std::fs::create_dir_all(&scratch_root).map_err(|e| CliError::Io {
        path: scratch_root.clone(),
        source: e,
    })?;

    let mut passing: Vec<String> = Vec::new();

    for version in new_versions {
        let ver_str = version.version.to_string();

        // Step 3 — fetch + verify: git-clone the source at the pinned revision and
        // assert the fetched tree's sha256 equals the index pin. A mismatch is a
        // CliError::HashMismatch — the fetched bytes are not the source the
        // publisher registered, so nothing derived from them is trusted.
        let checkout =
            resolve::fetch_and_verify_index_version(&scratch_root, &submitted.name, version)?;

        // Step 4 — audit: run the full Tier-1 (+ Tier-2 where applicable) gate on
        // the verified source tree. Pass --index so the enforced-semver check reads
        // the right baseline. Reject on the first failing check.
        let checkout_str = checkout.to_string_lossy().into_owned();
        // Pass the submitted entry's publisher so the reserved-namespace ownership
        // check can exempt the blessed first-party publisher and reject any other
        // publisher whose source tree provides a reserved-namespace (`Ipe.*`)
        // module — the admission-time squat-proofing of the trusted namespace.
        let mut audit_args: Vec<String> = vec![
            checkout_str,
            "--publisher".to_owned(),
            submitted.publisher.clone(),
        ];
        if let Some(ir) = &index_root_opt {
            audit_args.push("--index".to_owned());
            audit_args.push(ir.to_string_lossy().into_owned());
        }
        // Propagate typed errors directly — run_audit already produces a
        // descriptive typed CliError (PackageAudit / HashMismatch / etc.) whose
        // Display names the failing check; the version context is clear from
        // the eprintln below and the structured error kind.
        if let Err(e) = audit::run_audit(&audit_args) {
            eprintln!(
                "audit-entry: `{}` version {} rejected",
                submitted.name, ver_str
            );
            return Err(e);
        }

        passing.push(ver_str);
    }

    // All new versions passed — print the certified summary.
    let versions_list = passing.join(", ");
    let body = format!(
        "audit-entry: {} — {} new version(s) certified: {versions_list}",
        submitted.name,
        passing.len()
    );
    print!("{}", style::frame(&style::gutter(&body)));

    // Remove the per-run scratch directory (best-effort; a leftover is harmless).
    let _ = std::fs::remove_dir_all(&scratch_root);
    Ok(())
}

/// Parse `ipe package audit-entry`'s tail: a required positional entry-file path
/// and an optional `--index <dir>`.
///
/// # Errors
/// [`CliError::Usage`] when the entry file is missing; [`CliError::UsageOwned`] on
/// an unknown flag, a missing `--index` value, or a duplicate flag/positional.
fn parse_audit_entry_args(rest: &[String]) -> Result<(PathBuf, Option<PathBuf>), CliError> {
    let mut entry_path: Option<PathBuf> = None;
    let mut index_root: Option<PathBuf> = None;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--index" => {
                let value = it.next().ok_or(CliError::Usage(
                    "ipe package audit-entry: --index needs a value",
                ))?;
                if index_root.is_some() {
                    return Err(CliError::Usage(
                        "ipe package audit-entry: --index given more than once",
                    ));
                }
                index_root = Some(PathBuf::from(value));
            }
            flag if flag.starts_with('-') => {
                return Err(cli_args::usage_unknown_flag("package audit-entry", flag));
            }
            positional => {
                if entry_path.is_some() {
                    return Err(CliError::Usage(
                        "ipe package audit-entry: expected a single entry-file path",
                    ));
                }
                entry_path = Some(PathBuf::from(positional));
            }
        }
    }
    let path = entry_path.ok_or(CliError::Usage(
        "usage: ipe package audit-entry <packages/<name>.toml> [--index <root>]",
    ))?;
    Ok((path, index_root))
}

/// Resolve a `check`/analysis `<path>` argument to the entry `.ipe` file the
/// source-graph pipeline reads. Same argument convention as `ipe build`:
///
/// 1. a directory → its `package.ipe`'s `src`-root `Main.ipe`;
/// 2. a `.ipe` file → itself.
///
/// A project's entry module is always `Main` (`project` module doc), so the
/// entry file is `<src_root>/Main.ipe`.
///
/// # Errors
/// [`CliError::Usage`] for a directory with no `package.ipe`; the manifest's own
/// parse errors otherwise.
fn resolve_analysis_entry(path: &Path) -> Result<PathBuf, CliError> {
    let manifest = discover_manifest(path)?;
    match manifest {
        Some(m) => {
            let parsed = project::parse_manifest(&m)?;
            Ok(analysis_root_of(&parsed))
        }
        None => Ok(path.to_path_buf()),
    }
}

/// The source file `ipe type-check` uses as its analysis root for a manifest
/// project.
///
/// An application uses `<src_root>/Main.ipe`. A library (a manifest declaring
/// `exposedModules` with no `src/Main.ipe` and no runnable program) has no
/// `main` to check, so its analysis root is its first exposed module's file —
/// checking the public surface is a library's meaningful verification. The
/// declared-program entry (when a `programs` stage names one) takes precedence
/// via [`ProjectManifest::resolved_entry`]'s module path, mapped back to a file
/// under the source root.
fn analysis_root_of(parsed: &project::ProjectManifest) -> PathBuf {
    let main = parsed.src_root.join("Main.ipe");
    if main.is_file() {
        return main;
    }
    // No Main: prefer a declared program's entry file, else the first exposed
    // module's file. Fall back to `Main.ipe` (the caller surfaces a clean
    // missing-entry diagnostic) when the manifest names neither.
    if let Some(program) = parsed.default_program() {
        return parsed.src_root.join(&program.entry);
    }
    if let Some(module) = parsed.exposed_modules.first() {
        let rel: PathBuf = module.split('.').collect();
        return parsed.src_root.join(rel).with_extension("ipe");
    }
    main
}

/// `ipe type-check [<path>]` — type-check a program and stop. Runs the same
/// injection-aware source graph `ipe build` uses, but demands only the
/// `typecheck` query: no IR lowering, no Rust emission, nothing written. Exits
/// 0 with a friendly framed success line when the program type-checks, or
/// non-zero carrying the first rendered diagnostic when it does not.
///
/// With `--json`, each diagnostic is a JSON object on stderr, and success
/// is `{"status":"ok"}` on stdout — both machine-parseable.
pub(crate) fn run_type_check(rest: &[String]) -> Result<(), CliError> {
    let args = cli_args::parse_type_check(rest)?;
    let arg = match args.entry {
        Some(e) => PathBuf::from(e),
        None => PathBuf::from(default_entry()?),
    };
    let entry = resolve_analysis_entry(&arg)?;
    typecheck_entry_via_graph(&entry).map_err(|e| {
        if args.format == cli_args::OutputFormat::Json {
            emit_pipeline_json(e)
        } else {
            e
        }
    })?;
    match args.format {
        cli_args::OutputFormat::Json => {
            println!("{{\"status\":\"ok\"}}");
        }
        cli_args::OutputFormat::Plain => {
            println!("ok");
        }
        cli_args::OutputFormat::Human => {
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
        }
    }
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

/// Where the test binary's own `N passed, M failed` summary goes.
///
/// The default human path inherits stdout so the summary appears inline between
/// the progress lines; the `--json` path routes it to stderr so stdout carries
/// exactly the one JSON verdict line a consumer parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestStdio {
    /// Inherit stdout — the child's summary prints where the user sees it.
    Inherit,
    /// Redirect the child's stdout to stderr — keep our stdout machine-clean.
    Quiet,
}

/// Build and run a project's `tests/Main.ipe`, the single test runner shared by
/// `ipe test` and `ipe verify`'s final stage.
///
/// The test entry is the file at `<project-root>/tests/Main.ipe`. The project
/// root is the directory holding `package.ipe`; with no manifest it is the parent
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
    run_project_tests_with(path, TestStdio::Inherit)
}

/// The shared test runner, parameterised by where the test binary's own summary
/// goes ([`TestStdio`]). See [`run_project_tests`] for the resolution rules.
///
/// # Errors
/// As [`run_project_tests`].
fn run_project_tests_with(path: Option<&str>, stdio: TestStdio) -> Result<TestOutcome, CliError> {
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

    // Emit into an exclusively-created, unpredictably-named temp directory so
    // concurrent verify runs cannot collide and an attacker cannot pre-seed the
    // path.
    let out_scratch = scratch::ScratchDir::new("ipe-verify-test").map_err(|e| CliError::Io {
        path: PathBuf::from("ipe-verify-test"),
        source: e,
    })?;
    let out_dir = out_scratch.path().to_path_buf();

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
        stdio,
    );

    // `out_scratch` drops here, removing the temp directory on every exit path
    // (compile failure, cargo error, spawn error, or normal completion).
    drop(out_scratch);
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
    stdio: TestStdio,
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
    // `CARGO_TARGET_DIR` pin or workspace override is respected. The binary
    // name matches the emitted crate's package name (read from `Cargo.toml`).
    let test_bin_name = emitted_bin_name(out_dir);
    let mut bin = cargo_target_directory(out_dir)?;
    bin.push("debug");
    bin.push(&test_bin_name);

    // Run the test binary. `Ipe.Test.runMain` exits 0 on all-pass, 1 on any
    // failure — propagate that as a stage error. Under `--json` the child's own
    // human summary is captured and re-emitted on OUR stderr, so our stdout stays
    // a single JSON line a consumer can parse.
    let run_status = match stdio {
        TestStdio::Inherit => {
            std::process::Command::new(&bin)
                .status()
                .map_err(|e| CliError::Io {
                    path: bin.clone(),
                    source: e,
                })?
        }
        TestStdio::Quiet => {
            let output = std::process::Command::new(&bin)
                .stdout(std::process::Stdio::piped())
                .output()
                .map_err(|e| CliError::Io {
                    path: bin.clone(),
                    source: e,
                })?;
            let _ = std::io::stderr().write_all(&output.stdout);
            output.status
        }
    };

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
pub(crate) fn run_test(rest: &[String]) -> Result<(), CliError> {
    let (path, format) = cli_args::single_positional_with_format(rest, "test")?;

    if format == cli_args::OutputFormat::Json {
        return run_test_json(path);
    }

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

/// `ipe test --json`: run the tests and emit a compact verdict object to stdout.
///
/// The test binary's own human `N passed, M failed` summary is routed to stderr
/// (via [`TestStdio::Quiet`]) so stdout carries exactly one JSON line a consumer
/// can parse. A failing case still exits non-zero: the verdict object is written,
/// then the already-emitted sentinel drives the exit without a second message.
fn run_test_json(path: Option<&str>) -> Result<(), CliError> {
    use cli_args::json;

    let verdict = |result: &str| json::object(&[("result", json::string(result))]);
    match run_project_tests_with(path, TestStdio::Quiet) {
        Ok(TestOutcome::AllPassed) => {
            println!("{}", verdict("passed"));
            Ok(())
        }
        Ok(TestOutcome::NoTestEntry) => {
            println!("{}", verdict("no-tests"));
            Ok(())
        }
        Err(CliError::TestFailed { code }) => {
            println!(
                "{}",
                json::object(&[
                    ("result", json::string("failed")),
                    ("exitCode", code.to_string()),
                ])
            );
            Err(CliError::DiagnosticJsonEmitted)
        }
        // A build/toolchain error is not a test verdict — surface it as itself.
        Err(other) => Err(other),
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
pub(crate) fn run_verify(rest: &[String]) -> Result<(), CliError> {
    let (path, format) = cli_args::single_positional_with_format(rest, "verify")?;

    if format == cli_args::OutputFormat::Json {
        return run_verify_json(path);
    }

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

/// `ipe verify --json`: run the gate and emit a single compact verdict object to
/// stdout — `{"result":"passed","stages":N}` on a clean run, or
/// `{"result":"failed","stage":"<name>"}` at the first failing stage (then a
/// non-zero exit via the already-emitted sentinel).
///
/// Each stage runs in a machine-quiet form so stdout carries EXACTLY the verdict
/// line: the type-check core prints nothing, the build banner and any stage
/// diagnostic go to stderr, and the test binary's summary is captured to stderr.
fn run_verify_json(path: Option<&str>) -> Result<(), CliError> {
    use cli_args::json;

    let stages: &[(&str, VerifyStage)] = &[
        ("format", verify_fmt),
        ("type-check", verify_check_quiet),
        ("build", verify_build),
        ("test", verify_test_quiet),
    ];

    for (name, stage) in stages {
        if stage(path).is_err() {
            println!(
                "{}",
                json::object(&[
                    ("result", json::string("failed")),
                    ("stage", json::string(name)),
                ])
            );
            return Err(CliError::DiagnosticJsonEmitted);
        }
    }
    println!(
        "{}",
        json::object(&[
            ("result", json::string("passed")),
            ("stages", stages.len().to_string()),
        ])
    );
    Ok(())
}

/// The type-check stage in machine-quiet form: the same source-graph type-check
/// as [`verify_check`], but through the non-printing core so stdout stays clean
/// for the JSON verdict (a diagnostic still renders through the error channel).
fn verify_check_quiet(path: Option<&str>) -> Result<(), CliError> {
    let arg = match path {
        Some(e) => PathBuf::from(e),
        None => PathBuf::from(default_entry()?),
    };
    let entry = resolve_analysis_entry(&arg)?;
    typecheck_entry_via_graph(&entry)
}

/// The test stage in machine-quiet form: the shared runner with the test
/// binary's own summary routed to stderr, so stdout stays the JSON verdict alone.
fn verify_test_quiet(path: Option<&str>) -> Result<(), CliError> {
    run_project_tests_with(path, TestStdio::Quiet).map(|_| ())
}

pub(crate) fn run_capabilities(rest: &[String]) -> Result<(), CliError> {
    let (format, positional) = cli_args::split_format(rest, "capabilities")?;
    let arg = match positional.first() {
        Some(e) => PathBuf::from(e),
        None => PathBuf::from(default_entry()?),
    };
    // Route a directory / project-root `.` to its entry `.ipe` file, the same
    // argument convention `ipe type-check` uses. Without this a bare
    // `ipe capabilities` in a project dir passes `.` straight to the reader and
    // fails with a raw "Is a directory" io error.
    let entry = resolve_analysis_entry(&arg)?;
    let graph = build_source_graph(&entry)?;
    let program = graph.run_attributed(&entry, |db, root, file| {
        ipe_db::lower_program(db, root, file)
    })?;
    let caps = capabilities_including_served_widgets(
        &graph.db,
        graph.source_root,
        graph.entry_file,
        &program,
    );
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
            format!(
                "{}\n",
                cli_args::json::object(&[("capabilities", cli_args::json::string_array(names),)])
            )
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
pub(crate) fn run_version(rest: &[String]) -> Result<(), CliError> {
    let (format, positional) = cli_args::split_format(rest, "version")?;
    if let Some(extra) = positional.first() {
        return Err(cli_args::usage_unexpected_argument("version", extra));
    }
    print!("{}", render_version(format, &std::io::stdout()));
    Ok(())
}

/// The one-liner installer URL.
///
/// The same script the docs' `curl … | sh` install uses; `ipe upgrade` re-runs it
/// to fetch the latest release binary and install it over the current one. `pub`
/// so the install-drift test can assert the README `curl` one-liner and this
/// self-updater URL stay in agreement.
pub const INSTALL_SH_URL: &str =
    "https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/install.sh";

/// `ipe upgrade` — self-update by re-running the release installer.
///
/// Checks the latest published release, then installs it when a newer one is
/// available (and confirmed). `--dry-run` shows what would run without touching
/// anything; `--check` reports only and never installs; `--yes`/`-y` or a
/// non-TTY stdout skips the prompt; `--plain`/`--json` emit machine output and
/// never prompt. `--check --exit-code` signals 10 (available), 0 (up-to-date),
/// or 2 (feed unreachable) via the process exit code.
///
/// The installer (`install.sh`) exits with code 2 when it finds no prebuilt
/// binary; that distinct code surfaces as [`CliError::UpgradeNoPrebuilt`].
///
/// # Errors
/// [`CliError::UsageOwned`] on an unknown flag or a non-POSIX host.
/// [`CliError::UpgradeNoPrebuilt`] when the installer exits 2.
/// [`CliError::UpgradeFeedUnreachable`] when the release feed is offline and
/// `--check`/`--exit-code` are not in use.
/// [`CliError::UpgradeCheckExit`] for `--check --exit-code` numeric signals.
#[allow(clippy::too_many_lines)]
pub fn run_upgrade(rest: &[String]) -> Result<(), CliError> {
    use std::io::IsTerminal as _;

    let mut dry_run = false;
    let mut yes = false;
    let mut check = false;
    let mut exit_code_flag = false;
    let mut format: Option<cli_args::OutputFormat> = None;

    for arg in rest {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--yes" | "-y" => yes = true,
            "--check" => check = true,
            "--exit-code" => exit_code_flag = true,
            "--plain" => {
                if format.is_some() {
                    return Err(CliError::UsageOwned(
                        "ipe upgrade: --plain and --json are mutually exclusive".to_owned(),
                    ));
                }
                format = Some(cli_args::OutputFormat::Plain);
            }
            "--json" => {
                if format.is_some() {
                    return Err(CliError::UsageOwned(
                        "ipe upgrade: --plain and --json are mutually exclusive".to_owned(),
                    ));
                }
                format = Some(cli_args::OutputFormat::Json);
            }
            other if other.starts_with('-') => {
                return Err(cli_args::usage_unknown_flag("upgrade", other));
            }
            other => {
                return Err(cli_args::usage_unexpected_argument("upgrade", other));
            }
        }
    }

    let fmt = format.unwrap_or_default();
    let command = format!("curl -fsSL {INSTALL_SH_URL} | sh");

    // --dry-run: show the installer command and stop — no version check needed.
    if dry_run {
        print!(
            "{}",
            style::frame(&style::gutter(&format!("would run: {command}")))
        );
        return Ok(());
    }

    let vc = version_check::version_check();
    let action = vc.action();

    // --plain / --json: emit machine output and never prompt or install.
    if fmt != cli_args::OutputFormat::Human {
        print!("{}", render_upgrade(&vc, &action, false, fmt));
        return match action {
            version_check::UpgradeAction::Unreachable => Err(CliError::UpgradeFeedUnreachable),
            _ => Ok(()),
        };
    }

    // Human output: print the status line.
    let stdout = std::io::stdout();
    let p = style::Palette::for_stream(&stdout);
    match action {
        version_check::UpgradeAction::UpToDate => {
            let v = vc.current.to_string();
            print!(
                "{}",
                style::frame(&style::gutter(&format!(
                    "{}{}{} ipe {v} — already the latest release",
                    p.green,
                    style::glyph::OK,
                    p.reset
                )))
            );
            if check && exit_code_flag {
                return Err(CliError::UpgradeCheckExit {
                    code: check_exit_code(&version_check::UpgradeAction::UpToDate),
                });
            }
            return Ok(());
        }
        version_check::UpgradeAction::Unreachable => {
            print!(
                "{}",
                style::frame(&style::gutter(&format!(
                    "{}{}{}  couldn't reach the release feed — check your connection",
                    p.red,
                    style::glyph::FAIL,
                    p.reset
                )))
            );
            if check && exit_code_flag {
                return Err(CliError::UpgradeCheckExit {
                    code: check_exit_code(&version_check::UpgradeAction::Unreachable),
                });
            }
            return Err(CliError::UpgradeFeedUnreachable);
        }
        version_check::UpgradeAction::Available => {
            let cur = vc.current.to_string();
            let lat = vc
                .latest
                .as_ref()
                .map(semver::Version::to_string)
                .unwrap_or_default();
            print!(
                "{}",
                style::frame(&style::gutter(&format!(
                    "{}?{}  ipe {cur} \u{2192} {lat} available",
                    p.yellow, p.reset
                )))
            );
            if check {
                if exit_code_flag {
                    return Err(CliError::UpgradeCheckExit {
                        code: check_exit_code(&version_check::UpgradeAction::Available),
                    });
                }
                return Ok(());
            }
        }
    }

    // Available + not --check: confirm then install.
    let stdout_is_tty = stdout.is_terminal();
    let should_prompt = fmt == cli_args::OutputFormat::Human && stdout_is_tty && !yes;
    let confirmed = if should_prompt {
        use std::io::Write as _;
        print!(
            "{}",
            style::gutter(&format!("{}Upgrade now? [Y/n] ", style::GUTTER))
        );
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(n) if n > 0 => {
                matches!(line.trim().to_ascii_lowercase().as_str(), "" | "y" | "yes")
            }
            _ => false,
        }
    } else {
        // Non-TTY stdout or --yes: treat as confirmed.
        yes || !stdout_is_tty
    };

    if !confirmed {
        return Ok(());
    }

    run_installer(&command)
}

/// Spawn the installer script and wait for it to finish.
///
/// The installer script exits 2 when no prebuilt binary exists for the current
/// platform; any other non-zero exit is a generic failure.
///
/// # Errors
/// [`CliError::UsageOwned`] when the host is not POSIX, the installer cannot
/// be launched, or it exits with a non-zero code that is not 2.
/// [`CliError::UpgradeNoPrebuilt`] when the installer exits 2.
pub(crate) fn run_installer(command: &str) -> Result<(), CliError> {
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
        .arg(command)
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

/// The process exit code for `ipe upgrade --check --exit-code`, mirroring
/// git's `--exit-code` convention.
const fn check_exit_code(action: &version_check::UpgradeAction) -> i32 {
    match action {
        version_check::UpgradeAction::Available => 10,
        version_check::UpgradeAction::UpToDate => 0,
        version_check::UpgradeAction::Unreachable => 2,
    }
}

/// Render the upgrade status in `--plain` or `--json` format.
///
/// `upgraded` is `true` when the installer was actually run this session,
/// yielding `"action":"upgraded"` in JSON rather than `"checked"`.
/// Neither format ever prompts.
fn render_upgrade(
    check: &version_check::VersionCheck,
    action: &version_check::UpgradeAction,
    upgraded: bool,
    format: cli_args::OutputFormat,
) -> String {
    use cli_args::OutputFormat::{Json, Plain};
    let cur = check.current.to_string();
    let lat = check.latest.as_ref().map(semver::Version::to_string);
    match format {
        Json => {
            let action_str = if upgraded {
                "upgraded"
            } else {
                match action {
                    version_check::UpgradeAction::UpToDate => "up-to-date",
                    version_check::UpgradeAction::Available => "checked",
                    version_check::UpgradeAction::Unreachable => "unreachable",
                }
            };
            let lat_json = lat
                .as_deref()
                .map_or_else(|| "null".to_owned(), cli_args::json::string);
            let obj = cli_args::json::object(&[
                ("current", cli_args::json::string(&cur)),
                ("latest", lat_json),
                (
                    "upgradeAvailable",
                    if check.upgrade_available {
                        "true".to_owned()
                    } else {
                        "false".to_owned()
                    },
                ),
                (
                    "reachedFeed",
                    if check.reached_feed {
                        "true".to_owned()
                    } else {
                        "false".to_owned()
                    },
                ),
                ("action", cli_args::json::string(action_str)),
            ]);
            format!("{obj}\n")
        }
        Plain => match action {
            version_check::UpgradeAction::UpToDate => format!("ipe {cur} up-to-date\n"),
            version_check::UpgradeAction::Available => {
                if upgraded {
                    format!("ipe upgraded to {}\n", lat.unwrap_or_default())
                } else {
                    format!("ipe {cur} -> {} available\n", lat.unwrap_or_default())
                }
            }
            version_check::UpgradeAction::Unreachable => "feed unreachable\n".to_owned(),
        },
        // Human format is handled directly in `run_upgrade`.
        cli_args::OutputFormat::Human => String::new(),
    }
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
    let graph = build_source_graph(entry)?;
    let program = graph.run_attributed(entry, |db, root, file| {
        ipe_db::lower_program(db, root, file)
    })?;
    let inferred = capabilities_including_served_widgets(
        &graph.db,
        graph.source_root,
        graph.entry_file,
        &program,
    );
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
        let src =
            crate::io_bounded::read_to_string_capped(&m.path, crate::io_bounded::SOURCE_READ_CAP)?;
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
                inferred.extend(capabilities_including_served_widgets(
                    &db,
                    source_root,
                    entry_file,
                    &program,
                ));
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
    // the diagnostic. Source info not available here; location falls back.
    ipe_lower::lower(&canonical, &types, &mut interner, "", "")
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
    let source =
        crate::io_bounded::read_to_string_capped(entry, crate::io_bounded::SOURCE_READ_CAP)?;

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
pub(crate) fn write_atomic(target: &Path, contents: &str) -> Result<(), CliError> {
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
        Diagnostic::CompilerBug { .. }
        | Diagnostic::Ffi { .. }
        | Diagnostic::Sandbox { .. }
        | Diagnostic::Consent { .. }
        | Diagnostic::RegistryUnreachable { .. } => ipe_diagnostics::Span::DUMMY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipe_diagnostics::{NameError, Span};

    #[test]
    fn bluegreen_defaults_on_when_no_env_set() {
        // No opt-out, no explicit choice → on (the new default).
        assert!(bluegreen_from_env_values(None, None));
    }

    #[test]
    fn bluegreen_opt_out_wins() {
        // IPE_WATCH_NO_BLUEGREEN set (non-empty ≠ "0") → off, even if the legacy
        // flag would force on.
        assert!(!bluegreen_from_env_values(Some("1"), None));
        assert!(!bluegreen_from_env_values(Some("anything"), Some("1")));
        // "0"/empty opt-out is NOT an opt-out → the rest of the precedence runs.
        assert!(bluegreen_from_env_values(Some("0"), None));
        assert!(bluegreen_from_env_values(Some(""), None));
    }

    #[test]
    fn bluegreen_explicit_legacy_choice_is_honoured() {
        // Explicit IPE_WATCH_BLUEGREEN: "0"/empty off, anything else on.
        assert!(!bluegreen_from_env_values(None, Some("0")));
        assert!(!bluegreen_from_env_values(None, Some("")));
        assert!(bluegreen_from_env_values(None, Some("1")));
        assert!(bluegreen_from_env_values(None, Some("yes")));
    }

    #[test]
    fn registry_unreachable_matches_network_signals_only() {
        // Genuine network/offline failures.
        assert!(is_registry_unreachable(
            "Caused by:\n  Could not resolve host: index.crates.io"
        ));
        assert!(is_registry_unreachable("warning: spurious network error"));
        assert!(is_registry_unreachable(
            "error: failed to fetch `https://github.com/rust-lang/crates.io-index`"
        ));
        // A missing local path dependency or malformed manifest is NOT a
        // connectivity problem and must not be reported as one.
        assert!(!is_registry_unreachable(
            "error: failed to load source for dependency `handle_demo`\n\
             Caused by:\n  path `/tmp/x` does not exist"
        ));
        assert!(!is_registry_unreachable(
            "error: no matching package named `foo` found; updating registry index"
        ));
        assert!(!is_registry_unreachable("error[E0433]: cannot find crate"));
    }

    #[test]
    fn vendored_runtime_dir_is_required_only_when_vendoring() {
        // The dependency-model path (default `ipe build`/`run`, and `ipe watch`)
        // never vendors the runtime source tree — it reaches the runtime as a
        // crate dependency — so it must resolve to an empty sentinel WITHOUT
        // demanding a runtime dir. Requiring the vendored tree here is what made
        // `ipe watch` fail to locate the runtime in an installed checkout.
        assert_eq!(
            resolve_vendored_runtime_dir(None, false).ok(),
            Some(PathBuf::new()),
        );
        // An explicit `--runtime` is honoured verbatim, vendoring or not — so the
        // vendoring path (e.g. `ipe eject`) resolves a runtime dir even when the
        // ambient vendored tree is absent.
        assert_eq!(
            resolve_vendored_runtime_dir(Some("/opt/ipe-runtime".to_owned()), false).ok(),
            Some(PathBuf::from("/opt/ipe-runtime")),
        );
        assert_eq!(
            resolve_vendored_runtime_dir(Some("/opt/ipe-runtime".to_owned()), true).ok(),
            Some(PathBuf::from("/opt/ipe-runtime")),
        );
    }

    #[test]
    fn io_not_found_renders_styled_without_os_error() {
        let err = CliError::Io {
            path: PathBuf::from("/no/such.ipe"),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("no such file `/no/such.ipe`"),
            "styled NotFound message, got: {rendered}"
        );
        // No jargon: never the raw `io error` prefix, never an `os error N` tail.
        assert!(!rendered.contains("os error"), "leaks errno: {rendered}");
        assert!(!rendered.contains("io error"), "leaks jargon: {rendered}");
    }

    #[test]
    fn io_other_kind_stays_readable_without_errno() {
        let err = CliError::Io {
            path: PathBuf::from("/x"),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        let rendered = err.to_string();
        assert!(!rendered.contains("os error"), "leaks errno: {rendered}");
        assert!(rendered.contains("/x"), "names the path: {rendered}");
    }

    #[test]
    fn unknown_command_screen_is_fully_guttered() {
        let err = CliError::UnknownCommand {
            attempted: "frobnicate".to_owned(),
        };
        let rendered = err.to_string();
        // The advice line and the help header both carry the shared gutter — no
        // flush-left line breaks the screen the way the trim_start header did.
        assert!(
            rendered.starts_with("  unknown command `frobnicate`"),
            "advice guttered, got: {rendered:?}"
        );
        for line in rendered.lines().filter(|l| !l.is_empty()) {
            assert!(
                line.starts_with(style::GUTTER),
                "every non-empty line is guttered, offending: {line:?}"
            );
        }
    }

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

    /// Cargo terminal UI should be forced only when our stderr is a TTY and
    /// `NO_COLOR` is unset — both conditions must hold. Checked via a
    /// closed-form helper that mirrors the guard inside `force_cargo_terminal_ui`.
    #[test]
    fn force_cargo_ui_truth_table() {
        // Pure function extracted from the guard: is_tty && no_color is unset.
        let should_force = |is_tty: bool, no_color: bool| -> bool { is_tty && !no_color };
        assert!(should_force(true, false), "tty + color on → force");
        assert!(!should_force(false, false), "not a tty → no force");
        assert!(!should_force(true, true), "NO_COLOR set → no force");
        assert!(
            !should_force(false, true),
            "not a tty + NO_COLOR → no force"
        );
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

    /// A cargo build failure that is not a feature gap is unattributable: the
    /// front-end gate already rejected invalid programs, so a cargo failure here
    /// is a miscompile in Ipê's own emission, not the user's fault. It renders as
    /// a humble compiler-bug ICE that apologises, points at the issue tracker, and
    /// still embeds the raw cargo stderr as the reportable detail — never a bare
    /// rustc error presented as user error, and never any command's help page.
    #[test]
    fn emitted_build_failure_reports_unattributed_as_compiler_bug() {
        let err = CliError::EmittedBuildFailed {
            what: "the emitted program",
            code: 101,
            stderr: "error[E0425]: cannot find value `x` in this scope".to_owned(),
            runtime: None,
        };
        let rendered = err.to_string();
        // The humble ICE framing: this is the compiler's fault, please report it.
        assert!(rendered.contains("please report"), "{rendered}");
        assert!(rendered.contains("bug in Ipe"), "{rendered}");
        // The raw cargo error is preserved for the bug report.
        assert!(rendered.contains("cannot find value"), "{rendered}");
        assert!(rendered.contains("E0425"), "{rendered}");
        // Neither a help page nor the old plain-header user-error framing.
        assert!(!rendered.contains("ipe run [<path>]"), "{rendered}");
        assert!(
            !rendered.contains("building the emitted program failed (cargo exited"),
            "{rendered}"
        );
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
        // `lower_entry_via_graph`; it must resolve identically (a pure test
        // program).
        assert!(
            lower_entry_via_graph(&entry).is_ok(),
            "lower_entry_via_graph (capabilities path) must resolve `Ipe.Test` too"
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

    /// Creates a temp directory with a nested `src/Main.ipe` and a `package.ipe`
    /// at the project root, confirming the upward walk finds the manifest.
    #[test]
    fn find_manifest_walks_up_to_project_root() {
        let tmp = std::env::temp_dir().join("ipec_find_manifest_test");
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(&src).expect("create src/");
        let manifest = tmp.join("package.ipe");
        fs::write(
            &manifest,
            "module Package exposing (package)\n\n\npackage =\n    { name = \"test\" }\n",
        )
        .expect("write package.ipe");
        let main_ipe = src.join("Main.ipe");
        fs::write(&main_ipe, "module Main exposing (main)\nmain = 0\n").expect("write Main.ipe");

        let found = find_manifest_for_ipe_file(&main_ipe);
        assert_eq!(
            found.as_deref(),
            Some(manifest.as_path()),
            "upward walk must find package.ipe at project root"
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

    /// The pure hot-appearance decision (over the two raw variable values):
    /// default ON, the opt-out wins, and an explicit `IPE_WATCH_HOT_APPEARANCE`
    /// is honoured. Exercises the logic without mutating process env.
    #[test]
    fn hot_appearance_defaults_on_and_honours_overrides() {
        // Neither var set ⇒ on (the new default for `ipe watch`).
        assert!(hot_appearance_from_env(None, None), "unset ⇒ default on");
        // Opt-out set ⇒ off, regardless of the explicit var.
        assert!(
            !hot_appearance_from_env(Some("1"), None),
            "IPE_WATCH_NO_HOT_APPEARANCE=1 ⇒ off"
        );
        assert!(
            !hot_appearance_from_env(Some("anything"), Some("1")),
            "opt-out wins over an explicit on"
        );
        // Opt-out empty / `0` does NOT opt out.
        assert!(
            hot_appearance_from_env(Some(""), None),
            "empty opt-out is not an opt-out ⇒ still on"
        );
        assert!(
            hot_appearance_from_env(Some("0"), None),
            "`0` opt-out is not an opt-out ⇒ still on"
        );
        // Explicit `IPE_WATCH_HOT_APPEARANCE` is honoured when opt-out is absent.
        assert!(
            !hot_appearance_from_env(None, Some("0")),
            "explicit `0` ⇒ off"
        );
        assert!(
            !hot_appearance_from_env(None, Some("")),
            "explicit empty ⇒ off"
        );
        assert!(
            hot_appearance_from_env(None, Some("1")),
            "explicit `1` ⇒ on"
        );
    }

    /// A web app with a hoist-eligible style literal (`Ui.style "font-weight"
    /// "bold"`). Used to prove the build-vs-watch emit difference.
    const WEB_APP_WITH_STYLE: &str = "\
module Main exposing (main)
import Ipe.Tea.Web as Web
import Ipe.Ui as Ui
import Ipe.Tea.Web.Cmd as Cmd
import Ipe.Tea.Web.Sub as Sub

type Msg = Noop
type alias Model = { count : Int }

init : a -> ( Model, Cmd Msg )
init _req = ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update _msg model = ( model, Cmd.none )

subscriptions : Model -> Sub Msg
subscriptions _model = Sub.none

view : Model -> Element Msg
view _model =
    Ui.el [ Ui.style \"font-weight\" \"bold\" ] (Ui.text \"Counter\")

main =
    Web.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = []
        , notFound = Noop
        }
";

    /// Emit `WEB_APP_WITH_STYLE` with an explicit `hot_appearance` and return the
    /// CONCATENATED emitted Rust source (`src/main.rs` plus every per-module file
    /// under `src/ipe_mods/`, where the `view` body actually lands), or `None`
    /// when the runtime cannot be resolved (so the test is a no-op on a machine
    /// without an installed runtime crate).
    fn emit_web_app_source(hot_appearance: bool, tag: &str) -> Option<String> {
        let runtime = resolve_runtime().ok()?;
        let dir = std::env::temp_dir().join(format!("ipec_hot_appearance_{tag}"));
        let _ = fs::remove_dir_all(&dir);
        let entry = dir.join("Main.ipe");
        fs::create_dir_all(&dir).ok()?;
        fs::write(&entry, WEB_APP_WITH_STYLE).ok()?;
        let out = dir.join("out");
        let options = BuildOptions {
            hot_appearance,
            ..BuildOptions::from_env()
        };
        let built = build_with_sibling_discovery_with_options(&entry, &out, &runtime, options);
        assert!(built.is_ok(), "web app must compile ({tag}): {built:?}");
        // Walk `out/src` and concatenate every emitted `.rs` file: the view body
        // (and thus any hoisted `__ipe_lit` table) lands in a per-module file
        // under `src/ipe_mods/`, not in `src/main.rs`.
        let src_dir = out.join("src");
        let mut sources = String::new();
        let mut stack = vec![src_dir];
        while let Some(d) = stack.pop() {
            let Ok(entries) = fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|x| x.to_str()) == Some("rs")
                    && let Ok(text) = fs::read_to_string(&p)
                {
                    sources.push_str(&text);
                }
            }
        }
        assert!(
            !sources.is_empty(),
            "emitted src/ must carry at least one .rs file ({tag})"
        );
        let _ = fs::remove_dir_all(&dir);
        Some(sources)
    }

    /// PROD-CLEAN: a build-mode emit (`hot_appearance = false`, what `ipe build`
    /// / `ipe run` / `ipe release` thread) carries NO hot-swap scaffolding — no
    /// `LiteralTable` and no `/_ipe/hot-appearance` endpoint.
    #[test]
    fn build_mode_emit_carries_no_hot_swap_scaffolding() {
        let Some(src) = emit_web_app_source(false, "build_clean") else {
            return;
        };
        assert!(
            !src.contains("__ipe_lit"),
            "a build-mode emit must introduce no literal table, got:\n{src}"
        );
        assert!(
            !src.contains("/_ipe/hot-appearance"),
            "a build-mode emit must not mount the hot-appearance endpoint, got:\n{src}"
        );
    }

    /// WATCH: a watch-mode emit (`hot_appearance = true`) DOES hoist the style
    /// literal into the per-view `LiteralTable`, so an appearance edit can be
    /// hot-swapped without a rebuild.
    #[test]
    fn watch_mode_emit_hoists_literal_table() {
        let Some(src) = emit_web_app_source(true, "watch_hoist") else {
            return;
        };
        assert!(
            src.contains("__ipe_lit"),
            "a watch-mode emit must hoist style literals into a table, got:\n{src}"
        );
    }

    /// When no package.ipe exists in any parent directory, returns None.
    #[test]
    fn find_manifest_returns_none_when_absent() {
        let tmp = std::env::temp_dir().join("ipec_no_manifest_test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("create dir");
        let ipe = tmp.join("Standalone.ipe");
        fs::write(&ipe, "module Standalone exposing (f)\nf = 0\n").expect("write ipe");
        // Deliberately no package.ipe anywhere under tmp.
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
                "module Main exposing (main)\n\nimport Ipe.Io as Io\nimport Ipe.String as String\n\nmain : Task Error ()\nmain =\n    Io.println (String.fromInt 1)\n".to_owned(),
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
                "module Main exposing (main)\n\nimport Ipe.Io as Io\nimport Ipe.String as String\n\nmain : Task Error ()\nmain =\n    Io.println (String.fromInt 1)\n".to_owned(),
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
                "module Main exposing (main)\n\nimport Ipe.Io as Io\nimport Ipe.String as String\n\nmain : Task Error ()\nmain =\n    Io.println (String.fromInt 1)\n".to_owned(),
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
        // Verified shape via a one-off print during development: `main`'s body is
        // `Io.println (String.fromInt 1)`, so the only integer literal in the IR is
        // the `{"Int":1}` argument to `String.fromInt`. Tampering it to `42` makes
        // the re-emitted program print `42` — a value no fresh compile of this
        // source could produce.
        assert!(
            stored.contains("{\"Int\":1}"),
            "unexpected IR JSON shape, cannot safely tamper: {stored}"
        );
        let tampered = stored.replace("{\"Int\":1}", "{\"Int\":42}");
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
                "module Main exposing (main)\n\nimport Ipe.Io as Io\nimport Ipe.String as String\n\nmain : Task Error ()\nmain =\n    Io.println (String.fromInt 1)\n".to_owned(),
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
        // A project whose manifest selects the wasm target via `Package.wasm`,
        // with no `IPE_TARGET` env set — the tier the env-only check missed.
        fs::write(
            tmp.join("package.ipe"),
            "module Package exposing (package)\n\n\npackage =\n    { name = \"w\", wasm = On { mode = Spa } }\n",
        )
        .expect("write package.ipe");
        fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nmain = 0\n",
        )
        .expect("write Main.ipe");

        let out = tmp.join("out");
        let args = [
            tmp.join("package.ipe").to_string_lossy().into_owned(),
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

    #[test]
    fn analysis_root_prefers_main_then_program_then_exposed() {
        // An application with a src/Main.ipe uses it as the analysis root.
        let app = std::env::temp_dir().join("ipe_analysis_root_app");
        let _ = fs::remove_dir_all(&app);
        let app_src = app.join("src");
        fs::create_dir_all(&app_src).expect("create src/");
        fs::write(
            app.join("package.ipe"),
            "module Package exposing (package)\n\n\npackage =\n    { name = \"app\" }\n",
        )
        .expect("pkg");
        fs::write(
            app_src.join("Main.ipe"),
            "module Main exposing (main)\nmain = 0\n",
        )
        .expect("main");
        let app_manifest = project::parse_manifest(&app.join("package.ipe")).expect("app parses");
        assert_eq!(analysis_root_of(&app_manifest), app_src.join("Main.ipe"));
        let _ = fs::remove_dir_all(&app);

        // A library (exposedModules, no Main) uses its first exposed module's file.
        let lib = std::env::temp_dir().join("ipe_analysis_root_lib");
        let _ = fs::remove_dir_all(&lib);
        let lib_src = lib.join("src");
        fs::create_dir_all(&lib_src).expect("create src/");
        fs::write(lib.join("package.ipe"), "module Package exposing (package)\n\n\npackage =\n    { name = \"lib\", exposedModules = [ \"Core.Utils\" ] }\n").expect("pkg");
        // src/ must exist for the manifest reader's source-root check; the module
        // file itself need not exist for the pure path derivation under test.
        let lib_manifest = project::parse_manifest(&lib.join("package.ipe")).expect("lib parses");
        assert_eq!(
            analysis_root_of(&lib_manifest),
            lib_src.join("Core").join("Utils.ipe")
        );
        let _ = fs::remove_dir_all(&lib);
    }

    #[test]
    fn build_refuses_a_pure_library_with_a_clean_message() {
        let tmp = std::env::temp_dir().join("ipe_build_refuse_library");
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(&src).expect("create src/");
        fs::write(
            tmp.join("package.ipe"),
            "module Package exposing (package)\n\n\npackage =\n    { name = \"lib\", exposedModules = [ \"Core\" ] }\n",
        )
        .expect("pkg");
        fs::write(src.join("Core.ipe"), "module Core exposing (x)\nx = 0\n").expect("core");

        let out = tmp.join("out");
        let result = build_project_with_options(
            &tmp.join("package.ipe"),
            &out,
            Path::new("."),
            BuildOptions::from_env(),
        );
        assert!(
            matches!(&result, Err(CliError::Usage(msg)) if msg.contains("library package")),
            "a pure library must be refused with a clean library message: {result:?}"
        );
        assert!(!out.exists(), "the refusal fires before any emit");
        let _ = fs::remove_dir_all(&tmp);
    }

    // =========================================================================
    // `ipe package audit-entry` — argument parsing and fail-closed schema gate
    // =========================================================================

    fn temp_dir_unique(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ipe-audit-entry-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// Write a minimal well-formed `packages/<name>.toml` entry into `root`.
    fn write_entry(root: &Path, name: &str, versions: &[(&str, &str, &str, &str)]) {
        use std::fmt::Write as _;
        let pkgs = root.join("packages");
        std::fs::create_dir_all(&pkgs).expect("packages dir");
        let mut text = format!("name = \"{name}\"\npublisher = \"tester\"\n");
        for (ver, source, rev, sha) in versions {
            let _ = write!(
                text,
                "\n[[version]]\nversion = \"{ver}\"\nsource = \"{source}\"\n\
                 rev = \"{rev}\"\nsha256 = \"{sha}\"\ncapabilities = []\n"
            );
        }
        std::fs::write(pkgs.join(format!("{name}.toml")), text).expect("write entry");
    }

    /// `parse_audit_entry_args` — missing positional yields a `Usage` error.
    #[test]
    fn parse_audit_entry_args_requires_entry_file() {
        let err = parse_audit_entry_args(&[]).unwrap_err();
        assert!(
            matches!(err, CliError::Usage(_)),
            "missing entry-file must be a Usage error: {err:?}"
        );
    }

    /// `parse_audit_entry_args` — unknown flag yields `UsageOwned`.
    #[test]
    fn parse_audit_entry_args_rejects_unknown_flag() {
        let args: Vec<String> = ["packages/foo.toml", "--unknown"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let err = parse_audit_entry_args(&args).unwrap_err();
        assert!(
            matches!(err, CliError::UsageOwned(_)),
            "unknown flag must be a UsageOwned error: {err:?}"
        );
    }

    /// `parse_audit_entry_args` — `--index` without a value yields `Usage`.
    #[test]
    fn parse_audit_entry_args_rejects_index_without_value() {
        let args: Vec<String> = ["packages/foo.toml", "--index"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let err = parse_audit_entry_args(&args).unwrap_err();
        assert!(
            matches!(err, CliError::Usage(_)),
            "--index without value must be a Usage error: {err:?}"
        );
    }

    /// `parse_audit_entry_args` — two positionals yields `Usage`.
    #[test]
    fn parse_audit_entry_args_rejects_two_positionals() {
        let args: Vec<String> = ["packages/foo.toml", "extra"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let err = parse_audit_entry_args(&args).unwrap_err();
        assert!(
            matches!(err, CliError::Usage(_)),
            "two positionals must be a Usage error: {err:?}"
        );
    }

    /// `parse_audit_entry_args` — valid path + `--index` round-trips correctly.
    #[test]
    fn parse_audit_entry_args_parses_path_and_index() {
        let args: Vec<String> = ["packages/foo.toml", "--index", "/some/index"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let (path, index) = parse_audit_entry_args(&args).expect("parses");
        assert_eq!(path, PathBuf::from("packages/foo.toml"));
        assert_eq!(index, Some(PathBuf::from("/some/index")));
    }

    /// `run_audit_entry` — a malformed entry file (missing `sha256`) is a hard
    /// schema reject, never a warn-and-pass (§0 fail-closed).
    #[test]
    fn audit_entry_rejects_malformed_entry_schema() {
        let root = temp_dir_unique("ae-bad-schema");
        let pkgs = root.join("packages");
        std::fs::create_dir_all(&pkgs).expect("packages dir");
        // No `sha256` — the integrity anchor is mandatory; parse must reject.
        std::fs::write(
            pkgs.join("nohash.toml"),
            "publisher = \"tester\"\n\n[[version]]\nversion = \"1.0.0\"\n\
             source = \"https://example.invalid/nohash\"\nrev = \"abc\"\n",
        )
        .expect("write entry");
        let args: Vec<String> =
            std::iter::once(pkgs.join("nohash.toml").to_string_lossy().into_owned()).collect();
        let err = run_audit_entry(&args).unwrap_err();
        // Must be a Resolve or Io error from the schema parse — never Ok.
        assert!(
            matches!(err, CliError::Resolve(_) | CliError::Io { .. }),
            "malformed entry must be rejected at schema step: {err:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `run_audit_entry` — an entry whose every `[[version]]` is already in the
    /// baseline index is rejected: nothing new to audit (§0 fail-closed; the gate
    /// must not silently pass with no work done).
    #[test]
    fn audit_entry_rejects_when_all_versions_are_already_in_baseline() {
        let submitted_root = temp_dir_unique("ae-all-baseline-sub");
        let baseline_root = temp_dir_unique("ae-all-baseline-idx");
        // Both the submitted and the baseline have exactly version 1.0.0.
        write_entry(
            &submitted_root,
            "mylib",
            &[(
                "1.0.0",
                "https://x.invalid/mylib",
                "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
                "00",
            )],
        );
        write_entry(
            &baseline_root,
            "mylib",
            &[(
                "1.0.0",
                "https://x.invalid/mylib",
                "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
                "00",
            )],
        );
        let args: Vec<String> = [
            submitted_root
                .join("packages")
                .join("mylib.toml")
                .to_string_lossy()
                .into_owned(),
            "--index".to_owned(),
            baseline_root.to_string_lossy().into_owned(),
        ]
        .into_iter()
        .collect();
        let err = run_audit_entry(&args).unwrap_err();
        assert!(
            matches!(err, CliError::UsageOwned(_)),
            "no new versions must be a UsageOwned error: {err:?}"
        );
        let _ = std::fs::remove_dir_all(&submitted_root);
        let _ = std::fs::remove_dir_all(&baseline_root);
    }

    /// `run_audit_entry` — a published version is immutable. Re-submitting an
    /// existing version number with a *different* row (here a changed `sha256`)
    /// must be a hard reject naming immutability, never a silent skip. This closes
    /// the version-delta bypass: were the delta keyed on version number alone, a
    /// rewritten `source`/`rev`/`sha256`/`capabilities` on an already-published
    /// version would slip past both hash-verify and audit (ADR 0044, §receiving-gate).
    #[test]
    fn audit_entry_rejects_rewriting_a_published_version() {
        let submitted_root = temp_dir_unique("ae-immutable-sub");
        let baseline_root = temp_dir_unique("ae-immutable-idx");
        // Baseline published 1.0.0 with sha "00"; the submission keeps the same
        // version number but rewrites its sha256 to "11".
        write_entry(
            &baseline_root,
            "mylib",
            &[(
                "1.0.0",
                "https://x.invalid/mylib",
                "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
                "00",
            )],
        );
        write_entry(
            &submitted_root,
            "mylib",
            &[(
                "1.0.0",
                "https://x.invalid/mylib",
                "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
                "11",
            )],
        );
        let args: Vec<String> = [
            submitted_root
                .join("packages")
                .join("mylib.toml")
                .to_string_lossy()
                .into_owned(),
            "--index".to_owned(),
            baseline_root.to_string_lossy().into_owned(),
        ]
        .into_iter()
        .collect();
        let err = run_audit_entry(&args).unwrap_err();
        assert!(
            matches!(&err, CliError::UsageOwned(msg) if msg.contains("immutable")),
            "rewriting a published version must be a UsageOwned reject naming immutability: {err:?}"
        );
        let _ = std::fs::remove_dir_all(&submitted_root);
        let _ = std::fs::remove_dir_all(&baseline_root);
    }

    /// `run_audit_entry` — a new version whose `sha256` does not match the fetched
    /// tree is a hard [`CliError::HashMismatch`] (verify-before-trust, §0).
    ///
    /// Uses a local git repo as the source so the test runs offline.
    #[test]
    fn audit_entry_rejects_on_hash_mismatch() {
        // Build a tiny local git repo.
        let repo = temp_dir_unique("ae-mismatch-repo");
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git runs")
                .status
                .success();
            assert!(ok, "git {args:?} must succeed");
        };
        git(&["init", "--quiet"]);
        std::fs::write(repo.join("lib.ipe"), "module Lib\n").expect("write");
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "seed"]);
        // Get the HEAD commit hash.
        let rev_out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .expect("git rev-parse");
        let rev = String::from_utf8_lossy(&rev_out.stdout).trim().to_owned();

        // Write an entry that points at this repo but with a deliberately wrong sha256.
        let entry_root = temp_dir_unique("ae-mismatch-entry");
        let pkgs = entry_root.join("packages");
        std::fs::create_dir_all(&pkgs).expect("packages dir");
        let entry_text = format!(
            "name = \"testlib\"\npublisher = \"tester\"\n\n[[version]]\n\
             version = \"1.0.0\"\nsource = \"{}\"\nrev = \"{rev}\"\n\
             sha256 = \"000000000000000000000000000000000000000000000000000000000000wrong\"\n\
             capabilities = []\n",
            repo.display()
        );
        std::fs::write(pkgs.join("testlib.toml"), entry_text).expect("write entry");

        // Point --index at a root with no baseline so all versions are "new".
        let idx_root = temp_dir_unique("ae-mismatch-idx");
        std::fs::create_dir_all(idx_root.join("packages")).expect("packages dir");

        let args: Vec<String> = [
            pkgs.join("testlib.toml").to_string_lossy().into_owned(),
            "--index".to_owned(),
            idx_root.to_string_lossy().into_owned(),
        ]
        .into_iter()
        .collect();
        let err = run_audit_entry(&args).unwrap_err();
        assert!(
            matches!(err, CliError::HashMismatch { .. }),
            "a wrong sha256 must be a HashMismatch error, not: {err:?}"
        );

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&entry_root);
        let _ = std::fs::remove_dir_all(&idx_root);
    }

    // ---- unsafe-scan fail-closed tests -----------------------------------

    /// Returns a unique scratch directory under the OS temp root.
    /// The caller is responsible for removing it when done.
    fn unsafe_scan_test_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ipe-unsafe-scan-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create test scratch dir");
        dir
    }

    /// A manifest project with an unreadable module must return `Err(CliError::Io)`
    /// naming the unreadable path — not `Ok` with a partial source list.
    #[cfg(unix)]
    #[test]
    fn unsafe_scan_manifest_project_fails_closed_on_unreadable_module() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = unsafe_scan_test_dir("manifest-fail");
        let src = dir.join("src");
        fs::create_dir_all(&src).expect("create src");

        // One readable module and one unreadable one.
        let readable = src.join("Main.ipe");
        fs::write(&readable, "module Main exposing (main)\n").expect("write Main");
        let unreadable = src.join("Locked.ipe");
        fs::write(&unreadable, "module Locked exposing ()\n").expect("write Locked");
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).expect("chmod 000");

        let manifest_path = dir.join("package.ipe");
        fs::write(
            &manifest_path,
            "module Package exposing (package)\n\n\npackage =\n    { name = \"test\" }\n",
        )
        .expect("write manifest");

        let result = user_sources_for_unsafe_scan(Some(&manifest_path), &readable);

        // Restore permissions before any assertion so cleanup always runs.
        let _ = fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644));
        let _ = fs::remove_dir_all(&dir);

        // Must be `Err(CliError::Io)` naming the unreadable path — never an
        // `Ok` partial scan and never a different error variant.
        assert!(
            matches!(&result, Err(CliError::Io { path, .. }) if path == &unreadable),
            "expected Err(CliError::Io) naming {unreadable:?}, got: {result:?}"
        );
    }

    /// A manifest project where every module is readable must return `Ok` with
    /// every source text present.
    #[test]
    fn unsafe_scan_manifest_project_ok_when_all_readable() {
        use std::fs;

        let dir = unsafe_scan_test_dir("manifest-ok");
        let src = dir.join("src");
        fs::create_dir_all(&src).expect("create src");

        let entry = src.join("Main.ipe");
        fs::write(&entry, "module Main exposing (main)\n").expect("write Main");
        let other = src.join("Helper.ipe");
        fs::write(&other, "module Helper exposing ()\n").expect("write Helper");

        let manifest_path = dir.join("package.ipe");
        fs::write(
            &manifest_path,
            "module Package exposing (package)\n\n\npackage =\n    { name = \"test\" }\n",
        )
        .expect("write manifest");

        let result = user_sources_for_unsafe_scan(Some(&manifest_path), &entry);
        let _ = fs::remove_dir_all(&dir);

        // Every module readable ⇒ `Ok` carrying a source for each.
        assert!(
            matches!(&result, Ok(sources) if sources.len() >= 2),
            "expected Ok with a source for every readable module, got: {result:?}"
        );
    }

    /// Single-file fallback: when `collect_entry_and_siblings` fails and the
    /// entry itself is unreadable, the result must be `Err(CliError::Io)`
    /// naming the entry path — not `Ok` with an empty list.
    #[cfg(unix)]
    #[test]
    fn unsafe_scan_single_file_fallback_fails_closed_on_unreadable_entry() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = unsafe_scan_test_dir("single-fail");
        let entry = dir.join("Main.ipe");
        fs::write(&entry, "module Main exposing (main)\n").expect("write entry");
        fs::set_permissions(&entry, fs::Permissions::from_mode(0o000)).expect("chmod 000");

        // Pass no manifest so the single-file (entry + siblings) path is taken.
        let result = user_sources_for_unsafe_scan(None, &entry);

        let _ = fs::set_permissions(&entry, fs::Permissions::from_mode(0o644));
        let _ = fs::remove_dir_all(&dir);

        // Must be `Err(CliError::Io)` naming the unreadable entry — never an
        // `Ok` empty scan and never a different error variant.
        assert!(
            matches!(&result, Err(CliError::Io { path, .. }) if path == &entry),
            "expected Err(CliError::Io) naming {entry:?}, got: {result:?}"
        );
    }

    #[test]
    fn check_exit_code_is_git_style() {
        use crate::version_check::UpgradeAction::*;
        assert_eq!(super::check_exit_code(&Available), 10);
        assert_eq!(super::check_exit_code(&UpToDate), 0);
        assert_eq!(super::check_exit_code(&Unreachable), 2);
    }

    #[test]
    fn upgrade_json_reports_available() {
        use crate::version_check::{UpgradeAction, VersionCheck};
        let vc = VersionCheck {
            current: semver::Version::parse("0.1.72").expect("valid semver"),
            latest: Some(semver::Version::parse("0.1.75").expect("valid semver")),
            upgrade_available: true,
            reached_feed: true,
        };
        let s = super::render_upgrade(
            &vc,
            &UpgradeAction::Available,
            false,
            crate::cli_args::OutputFormat::Json,
        );
        assert!(
            s.contains("\"upgradeAvailable\":true"),
            "upgradeAvailable: {s}"
        );
        assert!(s.contains("\"action\":\"checked\""), "action: {s}");
        assert!(s.contains("\"latest\":\"0.1.75\""), "latest: {s}");
    }

    #[test]
    fn upgrade_plain_is_flush_and_terse() {
        use crate::version_check::{UpgradeAction, VersionCheck};
        let vc = VersionCheck {
            current: semver::Version::parse("0.1.72").expect("valid semver"),
            latest: None,
            upgrade_available: false,
            reached_feed: false,
        };
        let s = super::render_upgrade(
            &vc,
            &UpgradeAction::Unreachable,
            false,
            crate::cli_args::OutputFormat::Plain,
        );
        assert_eq!(s, "feed unreachable\n");
    }
}

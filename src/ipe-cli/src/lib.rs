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
pub mod cli_args;
pub mod diff;
pub mod doc;
pub mod ffi;
pub mod fmt;
pub mod help;
pub mod index;
pub mod init;
pub mod lockfile;
pub mod login;
mod lsp;
pub mod pkg;
pub mod project;
pub mod publish;
pub mod resolve;
pub mod run_sandbox;
pub mod style;
/// The embedded Ipê standard-library source now lives in the dependency-free
/// [`ipe_stdlib`] leaf crate so the WebAssembly frontend can share one copy.
/// Re-exported here so `crate::stdlib::…` call sites resolve unchanged.
pub use ipe_stdlib as stdlib;
pub mod watch;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use ipe_diagnostics::{
    ALL_CODES, Applicability, Diagnostic, HelpLine, Suggestion, explain_page, render, title,
};
use ipe_intern::Interner;

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

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(hint) => write!(f, "{hint}"),
            Self::UsageOwned(hint) => write!(f, "{hint}"),
            // The top-level help, coloured for a terminal. Rendered against
            // stderr because misuse output goes to stderr.
            Self::UnknownCommand { attempted } => {
                if !attempted.is_empty() {
                    writeln!(f, "unknown command `{attempted}`")?;
                    if let Some(sugg) = nearest_command(attempted) {
                        writeln!(f, "  = help: maybe `{sugg}`?")?;
                    }
                }
                f.write_str(help::top_level(&std::io::stderr()).trim_start())
            }
            Self::Io { path, source } => write!(f, "io error at {}: {source}", path.display()),
            Self::Pipeline { file, src, diag } => {
                f.write_str(&render(diag, &file.to_string_lossy(), src))
            }
            Self::RuntimeNotFound => write!(
                f,
                "could not locate the Ipe runtime; \
                 set IPE_RUNTIME_DIR to an explicit path or pass --runtime <dir>"
            ),
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
        }
    }
}

impl std::error::Error for CliError {}

// `CliError` is the `Err` type of every driver `Result`, so its size is paid
// in the `Err` slot of ~200 functions. Boxing the `Pipeline` diagnostic keeps
// it well under clippy's `result_large_err` 128-byte threshold (80 bytes today,
// bounded by the three-`String` variants such as `HashMismatch`); this
// assertion fails the build if a future variant reintroduces the bloat rather
// than boxing its payload.
const _: () = assert!(std::mem::size_of::<CliError>() <= 96);

/// Options modifying a build beyond plain source compilation — some (the
/// static plan) apply post-emit at write time; others (`target`,
/// `wasm_public_env`) feed the compile/emit pipeline itself.
///
/// The static plan is applied post-emit at write time — the compile pipeline
/// and its on-disk caches stay untouched (their keys deliberately exclude
/// the plan; the transform is a deterministic function of the plan applied
/// on cache-hit and cache-miss paths alike).
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
}

/// Build `entry` into a Rust Cargo project under `out_dir`, vendoring the
/// runtime module tree from `runtime_dir`.
///
/// # Errors
/// Returns [`CliError::Pipeline`] when the compiler rejects the program,
/// [`CliError::Io`] on any filesystem failure.
pub fn build(entry: &Path, out_dir: &Path, runtime_dir: &Path) -> Result<(), CliError> {
    build_with_options(entry, out_dir, runtime_dir, BuildOptions::default())
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
    build_with_sibling_discovery_with_options(entry, out_dir, runtime_dir, BuildOptions::default())
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
    let pipeline_err = |diag: Diagnostic| CliError::Pipeline {
        file: entry.to_path_buf(),
        src: source.clone(),
        diag: Box::new(diag),
    };

    // Parse the entry to learn its declared module path.
    let mut name_interner = Interner::new();
    let parsed = ipe_parse::parse_module(&source, &mut name_interner).map_err(&pipeline_err)?;
    let entry_module_path: Vec<String> = parsed
        .name
        .value
        .iter()
        .map(|s| name_interner.resolve(*s).unwrap_or_default().to_owned())
        .collect();

    // Source root: the directory containing the entry file.
    let src_root = entry
        .parent()
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| Path::new("."));

    // Discover ALL .ipe files in the source root (recursively).
    let mut discovered = project::discover_modules(src_root)?;

    // Ensure the entry itself is always in the discovered set, even when its
    // file name doesn't match the module-segment validation (e.g. a temp
    // path). This prevents the entry from being silently dropped.
    if !discovered
        .iter()
        .any(|m| m.module_path == entry_module_path)
    {
        discovered.push(project::DiscoveredModule {
            path: entry.to_path_buf(),
            module_path: entry_module_path.clone(),
        });
    }

    // Read every discovered module. The entry's source is already in memory.
    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    for m in &discovered {
        if m.module_path == entry_module_path {
            sources.insert(
                entry_module_path.clone(),
                (entry.to_path_buf(), source.clone()),
            );
        } else {
            let src = fs::read_to_string(&m.path).map_err(|e| io_err(&m.path, e))?;
            sources.insert(m.module_path.clone(), (m.path.clone(), src));
        }
    }

    Ok(CollectedSources {
        sources,
        discovered,
        entry_module_path,
    })
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
    // The on-disk build caches key only the Ipê sources — the FFI bindings
    // text and opaque map live OUTSIDE that key, so a cache hit could serve a
    // stale emitted project after `ipe add`/`ipe remove`. Disable both cache
    // tiers for FFI-using builds (correctness over warm-start speed).
    let cache_dir = if ffi_emit.is_some() { None } else { cache_dir };

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
            write_emitted_project(&emitted, out_dir, runtime_dir, options.static_plan.as_ref()),
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
        write_emitted_project(&emitted, out_dir, runtime_dir, options.static_plan.as_ref()),
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
/// and `ipe check` frame the identical diagnostic against the identical source.
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
) -> Result<(), CliError> {
    use ipe_backend_rust::static_build;

    let mut manifest = build_emit_manifest(emitted, runtime_dir)?;
    if let Some(plan) = static_plan {
        if static_build::manifest_is_webview(&emitted.cargo_toml).map_err(backend_invariant_err)? {
            return Err(CliError::StaticRefusal(build_plan::Refusal::WebviewStatic));
        }
        let cargo_toml = static_build::staticize_manifest(&emitted.cargo_toml, plan.allocator)
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
) -> Result<BTreeMap<PathBuf, String>, CliError> {
    let mut manifest = BTreeMap::new();
    collect_dir_text(runtime_dir, Path::new("src/ipe_runtime"), &mut manifest)?;
    manifest.insert(PathBuf::from("Cargo.toml"), emitted.cargo_toml.clone());
    for (rel, contents) in &emitted.files {
        manifest.insert(PathBuf::from(rel.as_str()), contents.clone());
    }
    Ok(manifest)
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
    build_project_with_options(manifest_path, out_dir, runtime_dir, BuildOptions::default())
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
        Some((cmd, rest)) if cmd == "upgrade-agents" => {
            with_help_on_misuse("upgrade-agents", init::run_upgrade_agents(rest))
        }
        Some((cmd, rest)) if cmd == "build" => with_help_on_misuse("build", run_build(rest)),
        Some((cmd, rest)) if cmd == "check" => with_help_on_misuse("check", run_check(rest)),
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
        Some((cmd, rest)) if cmd == "lsp" => with_help_on_misuse("lsp", lsp::run_lsp(rest)),
        Some((cmd, rest)) if cmd == "upgrade" => with_help_on_misuse("upgrade", run_upgrade(rest)),
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

    let mut opts = watch::WatchOptions::new(PathBuf::from(entry), out_dir, runtime_dir);
    opts.port = args.port;
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
        if plan.allocator == ipe_backend_rust::static_build::StaticAllocator::Mimalloc {
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
            let tree = emit_ir_text(&entry_path)?;
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
    let runtime_dir = match args.runtime {
        Some(r) => PathBuf::from(r),
        None => resolve_runtime()?,
    };

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

    let static_plan = resolve_static_plan(cli_layer, manifest.as_deref())?;
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
        // A native-bearing build artifact carries its own runtime enforcement: an
        // `ipe.profile` mirror plus the authoritative capability floor embedded
        // in the binary. `ipe exec <out_dir>` reads these and applies the jail,
        // so the enforcement travels with a copied-off-host artifact. A pure Ipê
        // artifact is structurally bounded and needs no jail (ADR 0040), so it
        // carries no profile or floor and `ipe exec` runs it directly. (A wasm
        // bundle has no native binary to jail.)
        let manifest_parsed = match &manifest {
            Some(m) => Some(project::parse_manifest(m)?),
            None => None,
        };
        let driver = manifest_parsed
            .as_ref()
            .map_or(ipe_backend_rust::DbDriver::Sqlite, |m| m.driver);
        let resolved = run_sandbox::resolve_for_run(
            manifest_parsed.as_ref(),
            manifest.as_deref(),
            &entry_path,
        )?;
        if run_sandbox::is_native_bearing(&resolved.union()) {
            let profile = run_sandbox::build_profile(&resolved, driver)?;
            run_sandbox::write_build_artifacts(&out_dir, &profile)?;
        }
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

/// Run the three post-emit bundle steps for `--target wasm`:
/// 1. `cargo build --target wasm32-unknown-unknown --release` (THE SEAL cross-target)
/// 2. `wasm-bindgen` CLI — emits the JS glue + `www/pkg/ipe_app_bg.wasm`
/// 3. `wasm-opt -Oz` — optional; silently skipped when not on PATH
///
/// Writes the final `www/pkg/` tree into `out_dir/www/pkg/`. On success the
/// directory at `out_dir/www/` is a self-contained static SPA ready to serve.
///
/// # Errors
/// [`CliError::UsageOwned`] when cargo or wasm-bindgen fails.
fn bundle_wasm(out_dir: &Path) -> Result<(), CliError> {
    // Step 1: compile to .wasm
    let cargo_status = std::process::Command::new("cargo")
        .args(["build", "--target", "wasm32-unknown-unknown", "--release"])
        .current_dir(out_dir)
        .status()
        .map_err(|e| CliError::Io {
            path: out_dir.to_path_buf(),
            source: e,
        })?;
    if !cargo_status.success() {
        let code = cargo_status.code().unwrap_or(1);
        return Err(CliError::UsageOwned(format!(
            "cargo build --target wasm32-unknown-unknown failed (exit {code})"
        )));
    }

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
    let runtime_dir = match args.runtime {
        Some(r) => PathBuf::from(r),
        None => resolve_runtime()?,
    };

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
    let mut cargo = std::process::Command::new("cargo");
    cargo.arg("build").current_dir(&out_dir);
    if let Some(plan) = &static_plan {
        cargo.args(["--target", plan.triple.as_str()]);
    }
    let cargo_status = cargo.status().map_err(|e| CliError::Io {
        path: out_dir.clone(),
        source: e,
    })?;
    if !cargo_status.success() {
        let code = cargo_status.code().unwrap_or(1);
        return Err(CliError::UsageOwned(format!(
            "cargo build failed with exit code {code}"
        )));
    }

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
fn render_code_index(format: cli_args::OutputFormat, _stream: &impl std::io::IsTerminal) -> String {
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
        Human => style::gutter(&code_index()),
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
    /// `attribute_post_link_error`), so `ipe check` and every other analysis
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
    let ffi_injected = std::collections::BTreeSet::new();

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
/// lowers to IR or emits Rust. This is what `ipe check` runs.
///
/// # Errors
/// [`CliError::Pipeline`] carrying the first compiler diagnostic;
/// [`CliError::Io`] when a source file cannot be read.
fn typecheck_entry_via_graph(entry: &Path) -> Result<(), CliError> {
    let graph = build_source_graph(entry)?;
    graph
        .run_attributed(entry, |db, root, file| ipe_db::typecheck(db, root, file))
        .map(|_| ())
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
        Some((sub, _)) => Err(CliError::UsageOwned(format!(
            "ipe package: unknown subcommand `{sub}` (expected `audit` or `publish`)"
        ))),
        None => Err(CliError::Usage(
            "usage: ipe package <audit|publish> [<path>]",
        )),
    }
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

/// `ipe check [<path>]` — type-check a program and stop. Runs the same
/// injection-aware source graph `ipe build` uses, but demands only the
/// `typecheck` query: no IR lowering, no Rust emission, nothing written. Exits
/// 0 with a terse `ok` when the program type-checks, or non-zero carrying the
/// first rendered diagnostic when it does not.
fn run_check(rest: &[String]) -> Result<(), CliError> {
    let arg = match cli_args::single_positional(rest, "check")? {
        Some(e) => PathBuf::from(e),
        None => PathBuf::from(default_entry()?),
    };
    let entry = resolve_analysis_entry(&arg)?;
    typecheck_entry_via_graph(&entry)?;
    print!("{}", style::frame(&style::gutter("ok")));
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

/// Stage 2: the type-check — the same source-graph pipeline as `ipe check`.
fn verify_check(path: Option<&str>) -> Result<(), CliError> {
    run_check(&path.map(str::to_owned).into_iter().collect::<Vec<_>>())
}

/// Stage 3: the build — the same compilation as `ipe build`.
fn verify_build(path: Option<&str>) -> Result<(), CliError> {
    run_build(&path.map(str::to_owned).into_iter().collect::<Vec<_>>())
}

/// Stage 4: the test run — build and execute `tests/Main.ipe` if one exists.
///
/// The test entry is the file at `<project-root>/tests/Main.ipe` (where
/// "project root" is the directory holding `ipe.toml`, or the directory
/// containing the supplied `.ipe` file when no manifest exists). When that
/// file is absent the stage passes immediately — a project with no test entry
/// is not an error. When it exists, the test runner is compiled to a temporary
/// output directory, the emitted Rust project is built with `cargo build`, and
/// the resulting `ipe-app` binary is executed. A non-zero exit from the binary
/// (propagated by `Ipe.Test.runMain`) is reported as a stage failure.
fn verify_test(path: Option<&str>) -> Result<(), CliError> {
    // Resolve the project root from the supplied path (or cwd defaults).
    let entry_path = match path {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(default_entry()?),
    };

    // Determine the directory that is the project root: a manifest directory,
    // or the parent of the supplied .ipe file.
    let manifest = discover_manifest(&entry_path)?;
    let project_root: PathBuf = manifest.as_ref().map_or_else(
        || {
            entry_path
                .parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        },
        |m| {
            m.parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        },
    );

    let test_entry = project_root.join("tests").join("Main.ipe");
    if !test_entry.is_file() {
        // No test entry — the stage is vacuously green.
        return Ok(());
    }

    let runtime_dir = resolve_runtime()?;

    // Emit into a unique temp directory so concurrent verify runs do not
    // collide and the output is never confused with the project's own `out/`.
    let out_dir = std::env::temp_dir().join(format!("ipe_verify_test_{}", std::process::id()));

    // Build the test entry. On any compile failure the stage propagates that
    // error directly — the error is already a well-formed `CliError`.
    build_with_sibling_discovery(&test_entry, &out_dir, &runtime_dir)?;

    // Compile the emitted Rust project.
    let cargo_status = std::process::Command::new("cargo")
        .arg("build")
        .current_dir(&out_dir)
        .status()
        .map_err(|e| CliError::Io {
            path: out_dir.clone(),
            source: e,
        })?;
    if !cargo_status.success() {
        let code = cargo_status.code().unwrap_or(1);
        return Err(CliError::UsageOwned(format!(
            "cargo build of the test runner failed with exit code {code}"
        )));
    }

    // Locate the compiled binary via `cargo metadata` so a user-level
    // `CARGO_TARGET_DIR` pin or workspace override is respected.
    let mut bin = cargo_target_directory(&out_dir)?;
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

    // Clean up the temp output regardless of the run outcome.
    let _ = std::fs::remove_dir_all(&out_dir);

    if run_status.success() {
        Ok(())
    } else {
        let code = run_status.code().unwrap_or(1);
        Err(CliError::UsageOwned(format!(
            "test runner exited with code {code}: one or more Ipe.Test cases failed"
        )))
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
    let p = style::Palette::for_stream(&std::io::stdout());

    for (index, (name, stage)) in VERIFY_STAGES.iter().enumerate() {
        let step = index + 1;
        print!(
            "{}",
            style::gutter(&format!(
                "{}{} stage {step}/{}: {name}{}\n",
                p.yellow,
                style::glyph::STEP,
                VERIFY_STAGES.len(),
                p.reset,
            ))
        );
        if let Err(err) = stage(path) {
            print!(
                "{}",
                style::gutter(&format!(
                    "{}{} {name} failed{}\n",
                    p.red,
                    style::glyph::FAIL,
                    p.reset,
                ))
            );
            // The stage ran correctly and reported a real failure — a gate
            // result, not a misuse of `verify`. Rewrap it as [`VerifyFailed`] so
            // the stage's own rendered report is shown alone, never the `verify`
            // `--help` page a raw usage error would trigger.
            return Err(CliError::VerifyFailed {
                stage: name,
                report: err.to_string(),
            });
        }
        print!(
            "{}",
            style::gutter(&format!(
                "{}{} {name} passed{}\n",
                p.green,
                style::glyph::OK,
                p.reset,
            ))
        );
    }

    print!(
        "{}",
        style::frame(&style::gutter(&format!(
            "{}{} all {} stages passed{}",
            p.green,
            style::glyph::OK,
            VERIFY_STAGES.len(),
            p.reset,
        )))
    );
    Ok(())
}

fn run_capabilities(rest: &[String]) -> Result<(), CliError> {
    let (format, positional) = cli_args::split_format(rest, "capabilities")?;
    let entry = match positional.first() {
        Some(e) => PathBuf::from(e),
        None => PathBuf::from(default_entry()?),
    };
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
/// Delegates to `scripts/install.sh` (the documented install path): it detects
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

    eprintln!(
        "{}",
        style::gutter(&format!(
            "{} upgrading ipe via install.sh …",
            style::glyph::STEP
        ))
    );
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .status()
        .map_err(|e| {
            CliError::UsageOwned(format!(
                "upgrade: cannot launch the installer (needs `sh` and `curl`): {e}"
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
    let ffi_injected = std::collections::BTreeSet::new();
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
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(_) => {
            let a = line.trim();
            a.eq_ignore_ascii_case("y") || a.eq_ignore_ascii_case("yes")
        }
        Err(_) => false,
    }
}

/// Write `contents` to `target` atomically: write a sibling temp file, then
/// rename it over `target` (atomic on a single filesystem). On a rename
/// failure the temp file is removed so no debris is left behind.
///
/// Retries ONCE, recreating `target`'s parent directory, when the write or
/// rename fails with `NotFound`. This closes a real race surfaced by the
/// emit→cargo bridge (`reconcile_emitted_project`, this function's
/// other caller besides `ipe doctor --fix`): several `crates/ipe/tests/
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
        assert_eq!(ALL_CODES.len(), 113, "taxonomy is 113 codes");
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
import Ipe.Prelude exposing (..)
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
import Ipe.Prelude exposing (..)
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
}

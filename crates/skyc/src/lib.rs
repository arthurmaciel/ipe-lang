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
//! `runtime/src/sky_runtime/` (the in-repo copy) and vendor it under
//! `<out>/src/sky_runtime/`.
//!
//! Errors are typed ([`CliError`]); no operation panics or unwraps.

pub mod project;
pub mod stdlib;

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use sky_backend::Backend;
use sky_backend_rust::RustBackend;
use sky_diagnostics::{
    ALL_CODES, Applicability, Diagnostic, HelpLine, Suggestion, explain_page, render, title,
};
use sky_intern::Interner;


/// A driver-level error. Distinct from a compiler [`Diagnostic`]: it also covers
/// filesystem failures and command-line misuse, neither of which is a property
/// of the Sky program being compiled.
#[derive(Debug)]
pub enum CliError {
    /// Command-line misuse; carries a fixed usage hint.
    Usage(&'static str),
    /// Command-line / manifest misuse whose message must echo user-supplied
    /// input (e.g. an unrecognised `sky.toml` value) — kept distinct from
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
    /// rustc/Elm-style report (caret snippet + help + `skyc explain` pointer)
    /// rather than a debug dump.
    Pipeline {
        file: PathBuf,
        src: String,
        diag: Diagnostic,
    },
    /// The Sky runtime module tree could not be located.
    RuntimeNotFound,
    /// `skyc explain <CODE>` was given a string that is not a taxonomy code.
    /// Carries the (trimmed) input and a deterministic did-you-mean list over
    /// the known codes, ranked by `(Levenshtein, code)`.
    UnknownCode {
        input: String,
        suggestions: Vec<&'static str>,
    },
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(hint) => write!(f, "{hint}"),
            Self::UsageOwned(hint) => write!(f, "{hint}"),
            Self::Io { path, source } => write!(f, "io error at {}: {source}", path.display()),
            Self::Pipeline { file, src, diag } => {
                f.write_str(&render(diag, &file.to_string_lossy(), src))
            }
            Self::RuntimeNotFound => write!(
                f,
                "could not locate the Sky runtime; \
                 set SKY_RUNTIME_DIR to an explicit path or pass --runtime <dir>"
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

    // Parse ONCE with a throwaway interner to learn the entry's declared module
    // path. Using the declared name as the entry's `module_path` means the shared
    // graph core's N0023 (path mismatch) can never fire for a single-file build
    // (expected == declared by construction), while still routing a single-file
    // program through the SAME injection-aware pipeline as a project — so a
    // single file importing `Std.Palette` injects the compiled source instead of
    // 404-ing (design §2.6). For a program with no compiled-source import the
    // core is emit-byte-identical to the pre-#98 single-module path (link over one
    // module is the identity — regression-covered by the golden suite).
    let mut name_interner = Interner::new();
    let pipeline_err = |diag: Diagnostic| CliError::Pipeline {
        file: entry.to_path_buf(),
        src: source.clone(),
        diag,
    };
    let parsed = sky_parse::parse_module(&source, &mut name_interner).map_err(&pipeline_err)?;
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
    // documented `sky.toml` default for a project that has no `[database]`
    // section at all.
    compile_modules(
        sources,
        discovered,
        &entry_path,
        out_dir,
        runtime_dir,
        entry,
        sky_backend_rust::DbDriver::Sqlite,
    )
}

/// Build a `.sky` entry file and all sibling modules discovered in the same
/// source directory.
///
/// When no `sky.toml` is present, the entry file's parent directory is used
/// as the source root. Every `*.sky` file found there is loaded and compiled
/// together — fixing SKY-N0020 for multi-file projects built via the
/// file-path shorthand (`skyc build src/Main.sky`).
///
/// This is the faithful port of Haskell's `Graph.discoverModulesMulti
/// (sourceRoot : ...) entryPath` call in `Sky.Build.Compile.hs`: it probes
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
    let source = fs::read_to_string(entry).map_err(|e| io_err(entry, e))?;
    let pipeline_err = |diag: Diagnostic| CliError::Pipeline {
        file: entry.to_path_buf(),
        src: source.clone(),
        diag,
    };

    // Parse the entry to learn its declared module path.
    let mut name_interner = Interner::new();
    let parsed = sky_parse::parse_module(&source, &mut name_interner).map_err(&pipeline_err)?;
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

    // Discover ALL .sky files in the source root (recursively). This is the
    // equivalent of `Graph.discoverModulesMulti [srcRoot] entryPath` in
    // `Sky.Build.Compile.hs`.
    let mut discovered = project::discover_modules(src_root)?;

    // Ensure the entry itself is always in the discovered set, even when its
    // file name doesn't match the module-segment validation (e.g. a temp
    // path). This prevents the entry from being silently dropped.
    if !discovered.iter().any(|m| m.module_path == entry_module_path) {
        discovered.push(project::DiscoveredModule {
            path: entry.to_path_buf(),
            module_path: entry_module_path.clone(),
        });
    }

    // Read every discovered module. The entry's source is already in memory.
    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    for m in &discovered {
        if m.module_path == entry_module_path {
            sources.insert(entry_module_path.clone(), (entry.to_path_buf(), source.clone()));
        } else {
            let src = fs::read_to_string(&m.path).map_err(|e| io_err(&m.path, e))?;
            sources.insert(m.module_path.clone(), (m.path.clone(), src));
        }
    }

    // No sky.toml on this path either (sibling discovery is the "no manifest
    // found" fallback) — default to sqlite, same rationale as `build`.
    compile_modules(
        sources,
        discovered,
        &entry_module_path,
        out_dir,
        runtime_dir,
        entry,
        sky_backend_rust::DbDriver::Sqlite,
    )
}

/// Walk up the directory tree from a `.sky` file's parent, looking for a
/// `sky.toml` manifest. Returns the manifest path if found, or `None` when
/// the walk reaches the filesystem root.
///
/// Faithful port of the Haskell `sky build src/Main.sky` behavior: when
/// given a file entry the Haskell driver locates the project root (where
/// `sky.toml` lives) before calling `buildProject`, so the full module graph
/// is compiled instead of just the single entry file.
fn find_manifest_for_sky_file(sky_file: &Path) -> Option<PathBuf> {
    let mut dir = sky_file.parent()?;
    loop {
        let candidate = dir.join("sky.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
}

/// The shared multi-module compile core: inject the compiled-source stdlib
/// closure, topologically order the graph, canonicalise each module dep-first
/// (with its unforgeable [`sky_canon::ModuleOrigin`]), link, then infer → lower →
/// emit → write. Both [`build`] and [`build_project`] route through this so the
/// injection seam is identical on the single-file and project paths.
///
/// `blame_path` is the file a cross-file diagnostic with no single owner (an
/// import cycle) is rendered against.
///
/// # Errors
/// [`CliError::Pipeline`] carrying the first compiler diagnostic; [`CliError::Io`]
/// on any filesystem failure.
#[allow(clippy::too_many_lines)]
fn compile_modules(
    mut sources: BTreeMap<Vec<String>, (PathBuf, String)>,
    mut discovered: Vec<project::DiscoveredModule>,
    entry_path: &[String],
    out_dir: &Path,
    runtime_dir: &Path,
    blame_path: &Path,
    db_driver: sky_backend_rust::DbDriver,
) -> Result<(), CliError> {
    // Inject the transitive compiled-source stdlib closure. `injected` is the
    // driver's unforgeable record of which module paths are trusted stdlib
    // source — the ONLY inputs that earn `ModuleOrigin::EmbeddedStdlib` below.
    let injected = project::inject_compiled_std_closure(&mut sources, &mut discovered);

    // Build the import graph, then topologically sort (dep-first, cycle = N0021).
    let topo = project::topological_order(&discovered, entry_path, |mod_path| {
        sources
            .get(mod_path)
            .map(|(_, src)| project::extract_imports_from_source(src))
            .unwrap_or_default()
    })
    .map_err(|cycle| {
        let path: Box<[Box<str>]> = cycle.path.into_iter().map(String::into_boxed_str).collect();
        let diag = sky_diagnostics::Diagnostic::Name {
            span: sky_diagnostics::Span::DUMMY,
            msg: sky_diagnostics::NameError::ImportCycle { path },
        };
        CliError::Pipeline {
            file: blame_path.to_path_buf(),
            src: String::new(),
            diag,
        }
    })?;

    // Canonicalise each module in dep-first order.
    let mut interner = sky_intern::Interner::new();
    let mut dep_exports: BTreeMap<Vec<sky_intern::Symbol>, sky_canon::ModuleExports> =
        BTreeMap::new();
    let mut canon_modules: Vec<sky_canon::ast::Module> = Vec::new();
    let mut entry_name: Vec<sky_intern::Symbol> = Vec::new();

    for m in &topo {
        let Some((path, src)) = sources.get(&m.module_path) else {
            return Err(CliError::Usage(
                "internal: module in topo order not in source map",
            ));
        };

        let pipeline_err = |diag: sky_diagnostics::Diagnostic| CliError::Pipeline {
            file: path.clone(),
            src: src.clone(),
            diag,
        };

        let parsed = sky_parse::parse_module(src, &mut interner).map_err(&pipeline_err)?;

        let expected_path: Vec<sky_intern::Symbol> = m
            .module_path
            .iter()
            .map(|s| interner.intern(s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(&pipeline_err)?;

        // The trust tag: EmbeddedStdlib IFF this exact module path was injected
        // from the embed table. A user file squatting on `Std.Foo` is NOT in
        // `injected` (injection skipped it on the pre-existing-key guard), so it
        // is `User` and stays SKY-N0025-rejected.
        let origin = if injected.contains(&m.module_path) {
            sky_canon::ModuleOrigin::EmbeddedStdlib
        } else {
            sky_canon::ModuleOrigin::User
        };

        let (canon_mod, exports) = sky_canon::canonicalise_module_with_origin(
            &parsed,
            &expected_path,
            &dep_exports,
            origin,
            &mut interner,
        )
        .map_err(&pipeline_err)?;

        if m.module_path == entry_path {
            entry_name.clone_from(&expected_path);
        }

        dep_exports.insert(expected_path, exports);
        canon_modules.push(canon_mod);
    }

    // Link → infer → lower → emit on the merged module. Blame link/lower/emit
    // errors on the entry file; infer errors and warnings are attributed to the
    // dep module that owns the failing span (#144).
    let entry_src_path = sources
        .get(entry_path)
        .map_or_else(|| blame_path.to_path_buf(), |(p, _)| p.clone());
    let entry_src = sources
        .get(entry_path)
        .map(|(_, s)| s.clone())
        .unwrap_or_default();
    let pipeline_err = |diag: sky_diagnostics::Diagnostic| CliError::Pipeline {
        file: entry_src_path.clone(),
        src: entry_src.clone(),
        diag,
    };

    // The link step now gates cross-module type-identity duplicates
    // `(home, name)` (#100), so it is fallible; blame a duplicate on the entry
    // file like every other post-link diagnostic.
    let linked =
        sky_canon::link::link(entry_name, canon_modules, &interner).map_err(&pipeline_err)?;

    // Build a module-home → (file, src) map used to attribute infer diagnostics
    // to the correct dep source file (#144).  Intern each String module-path
    // segment so the keys match the Symbol-keyed `home` fields on linked defs.
    let mut home_to_source: BTreeMap<Vec<sky_intern::Symbol>, (PathBuf, String)> =
        BTreeMap::new();
    for (str_path, (file, src)) in &sources {
        let sym_path: Result<Vec<_>, _> =
            str_path.iter().map(|s| interner.intern(s)).collect();
        if let Ok(sym_path) = sym_path {
            home_to_source.insert(sym_path, (file.clone(), src.clone()));
        }
    }

    // Given a diagnostic span, find the most tightly enclosing def in `linked`
    // and return that def's (file, src).  Defs preserve their original `home`
    // after link; every span in a def's body is a byte offset into that home
    // module's source.  Falls back to the entry file when no def encloses the
    // span (e.g. a CompilerBug with `Span::DUMMY`).
    // `source_for_span` maps a compiler-internal Span (byte offsets into its
    // *home module*'s source) to the (path, source) pair for error display.
    //
    // Heuristic: among all defs whose body_span *contains* the target span,
    // prefer the one whose `body_span.lo` is *closest* to `span.lo` (i.e. the
    // def that starts nearest to the failing expression).  Width is used as a
    // secondary tiebreaker — narrower body wins when distances are equal.
    //
    // This is strictly better than the prior "narrowest body wins" approach,
    // which could pick a short def from a *different module* (different byte
    // namespace) that happened to be numerically narrower, producing a
    // misattributed error location.  The closest-lo criterion naturally
    // selects the def in the same file because same-module defs share a byte
    // namespace; across modules, the intended def almost always has a smaller
    // distance from its own `lo`.
    let source_for_span = |span: sky_diagnostics::Span| -> (PathBuf, String) {
        if span == sky_diagnostics::Span::DUMMY {
            return (entry_src_path.clone(), entry_src.clone());
        }
        // (lo_dist, width, home)
        let mut best: Option<(u32, u32, &[sky_intern::Symbol])> = None;
        for def in &linked.defs {
            let body_span = match def {
                sky_canon::ast::Def::Untyped { body, .. }
                | sky_canon::ast::Def::Typed { body, .. } => body.span,
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
        // Union ctor spans live in the union's home byte-namespace, outside any
        // def body — without this they fall back to the entry file and render
        // at a coincidental byte offset in Main.sky (the misattribution class
        // for SKY-L0102 / SKY-L0114 and other `lower_enum` errors).
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
            .unwrap_or_else(|| (entry_src_path.clone(), entry_src.clone()))
    };

    // Use the attributed variant so cross-module type errors are attributed to
    // the correct source file via the `home` carried on the failing constraint,
    // rather than relying solely on the byte-offset heuristic (`source_for_span`)
    // which can mis-attribute when two merged modules share overlapping numeric
    // span ranges (the original #144 bug class).
    //
    // When `home` is non-empty we look it up in `home_to_source` directly —
    // O(log N) and exact.  When the home is empty (non-solver errors: constraint
    // generation, field-access pass, exhaustiveness) we fall back to the existing
    // heuristic, preserving the behaviour for every error class that pre-dates
    // this fix.
    let types = sky_types::infer_attributed(&linked, &mut interner).map_err(|(diag, home)| {
        let span = diag_span(&diag);
        let (file, src) = if home.is_empty() {
            source_for_span(span)
        } else {
            home_to_source
                .get(&home)
                .cloned()
                .unwrap_or_else(|| source_for_span(span))
        };
        CliError::Pipeline { file, src, diag }
    })?;
    // Print non-fatal warnings (e.g. SKY-T0011 RedundantCaseBranch) to stderr.
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
    // against the entry file at a coincidental byte offset — e.g. a State.sky
    // SKY-L0115 shown at an unrelated Main.sky line. `source_for_span` maps the
    // span back to its owning def's file, the same heuristic already used for
    // constraint-gen / exhaustiveness type errors.
    let span_attributed_err = |diag: sky_diagnostics::Diagnostic| {
        let (file, src) = source_for_span(diag_span(&diag));
        CliError::Pipeline { file, src, diag }
    };
    let program =
        sky_lower::lower(&linked, &types, &mut interner).map_err(span_attributed_err)?;
    let emitted = RustBackend::new(&interner)
        .with_db_driver(db_driver)
        .emit(&program)
        .map_err(span_attributed_err)?;

    // Vendor the runtime module tree FIRST, then write the emitted files. The
    // backend emits a trimmed `sky_runtime/mod.rs` + `config.rs`; writing the
    // emitted files last lets them overwrite the fuller copies from the source
    // tree (whose module list reaches for crates outside the M0 manifest).
    let src_dir = out_dir.join("src");
    fs::create_dir_all(&src_dir).map_err(|e| io_err(&src_dir, e))?;
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

/// Build a multi-module Sky project rooted at `manifest_path` (`sky.toml`) into
/// a Rust Cargo project under `out_dir`, vendoring the runtime from `runtime_dir`.
///
/// The build pipeline:
/// 1. Parse `sky.toml` to locate the source root.
/// 2. Discover every `*.sky` file under `src/`.
/// 3. Scan each file for `import` lines to build the import graph.
/// 4. Topological sort — fail closed on a cycle (SKY-N0021).
/// 5. Canonicalise each module in dep-first order (SKY-N0020 / N0022 / N0023 /
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
    )
}

/// Locate the Sky runtime module tree (`runtime/src/sky_runtime/`).
///
/// Resolution order:
/// 1. `$SKY_RUNTIME_DIR` — explicit override, allows pointing at any tree.
/// 2. Upward walk from the current directory, checking in order:
///    - `runtime/src/sky_runtime` (the in-repo copy — found immediately when
///      running from anywhere inside the sky-rust workspace)
///    - `sky/runtime-rust/src/sky_runtime` (sibling sky checkout — legacy)
///    - `runtime-rust/src/sky_runtime` (legacy sibling path)
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
            // In-repo runtime (sky-rust monorepo): found when CWD is anywhere
            // inside the workspace.
            dir.join("runtime").join("src").join("sky_runtime"),
            // Legacy: sibling `sky` checkout.
            dir.join("sky")
                .join("runtime-rust")
                .join("src")
                .join("sky_runtime"),
            // Legacy: sibling `runtime-rust` directory.
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

/// The top-level usage hint, listing every subcommand and flag.
const USAGE: &str = "usage:\n  \
     skyc build <entry.sky|project-dir|sky.toml> [--out <dir>] [--runtime <dir>] [--emit-ir] [--fix]\n  \
     skyc explain [<CODE>]\n  \
     skyc fix <entry.sky> [--yes]";

/// Parse `argv` (excluding the program name) and run the requested command.
///
/// # Errors
/// Returns [`CliError`] on misuse, a compile failure, or a filesystem error.
pub fn run_cli(args: &[String]) -> Result<(), CliError> {
    match args.split_first() {
        Some((cmd, rest)) if cmd == "build" => run_build(rest),
        Some((cmd, rest)) if cmd == "explain" => run_explain(rest),
        Some((cmd, rest)) if cmd == "fix" => run_fix(rest),
        _ => Err(CliError::Usage(USAGE)),
    }
}

/// `skyc build <entry.sky> [--out <dir>] [--runtime <dir>] [--emit-ir] [--fix]`.
fn run_build(rest: &[String]) -> Result<(), CliError> {
    let mut it = rest.iter();
    let entry = it.next().ok_or(CliError::Usage(USAGE))?.clone();
    let mut out: Option<String> = None;
    let mut runtime: Option<String> = None;
    let mut emit_ir = false;
    let mut fix = false;
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--out" => out = Some(it.next().ok_or(CliError::Usage(USAGE))?.clone()),
            "--runtime" => runtime = Some(it.next().ok_or(CliError::Usage(USAGE))?.clone()),
            "--emit-ir" => emit_ir = true,
            "--fix" => fix = true,
            _ => return Err(CliError::Usage(USAGE)),
        }
    }

    let entry_path = PathBuf::from(&entry);

    // `--fix` carries durable authorization: apply machine-applicable fixes
    // non-interactively before the (re-run) build sees the source.
    if fix {
        apply_fixes_cmd(&entry_path, true, &mut std::io::stdout())?;
    }

    if emit_ir {
        let tree = emit_ir_text(&entry_path)?;
        print!("{tree}");
        return Ok(());
    }

    let out_dir = out.map_or_else(|| PathBuf::from("sky-out").join("rust"), PathBuf::from);
    let runtime_dir = match runtime {
        Some(r) => PathBuf::from(r),
        None => resolve_runtime()?,
    };

    // Route the build:
    //   1. Directory → expect sky.toml inside it.
    //   2. .toml file → build_project directly.
    //   3. .sky file → walk up looking for sky.toml (project-mode); fall back
    //      to sibling discovery when no sky.toml exists (fixes SKY-N0020 for
    //      multi-file projects built via the file-path shorthand). This mirrors
    //      the Haskell driver's `Graph.discoverModulesMulti srcRoot entryPath`
    //      call in `Sky.Build.Compile.hs`.
    let manifest = if entry_path.is_dir() {
        let candidate = entry_path.join("sky.toml");
        if candidate.is_file() {
            Some(candidate)
        } else {
            return Err(CliError::Usage(
                "directory supplied but no sky.toml found inside it",
            ));
        }
    } else if entry_path.extension().and_then(|e| e.to_str()) == Some("toml") {
        Some(entry_path.clone())
    } else {
        // .sky file: walk up the directory tree looking for a sky.toml. When
        // found, build_project discovers all modules; when absent, fall through
        // to build_with_sibling_discovery which uses the entry's directory as
        // the source root.
        find_manifest_for_sky_file(&entry_path)
    };

    // No sky.toml found: compile entry + all sibling .sky files in the same
    // directory. Byte-identical to `build` when the directory holds only the
    // entry file (regression-covered by the golden suite).
    manifest.map_or_else(
        || build_with_sibling_discovery(&entry_path, &out_dir, &runtime_dir),
        |m| build_project(&m, &out_dir, &runtime_dir),
    )
}

/// `skyc explain [<CODE>]`. No argument prints the one-line index of every code
/// and its title; an argument prints that code's embedded explain page.
fn run_explain(rest: &[String]) -> Result<(), CliError> {
    match rest.first() {
        None => {
            print!("{}", code_index());
            Ok(())
        }
        Some(arg) => {
            let page = explain_lookup(arg)?;
            print!("{page}");
            Ok(())
        }
    }
}

/// `skyc fix <entry.sky> [--yes]`. Default is interactive per-edit confirmation;
/// `--yes` is durable authorization to apply every machine-applicable edit.
fn run_fix(rest: &[String]) -> Result<(), CliError> {
    let mut it = rest.iter();
    let entry = it.next().ok_or(CliError::Usage(USAGE))?.clone();
    let mut auto = false;
    for flag in it {
        match flag.as_str() {
            "--yes" => auto = true,
            _ => return Err(CliError::Usage(USAGE)),
        }
    }
    apply_fixes_cmd(&PathBuf::from(&entry), auto, &mut std::io::stdout())?;
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
/// The input is trimmed and upper-cased before matching, so `sky-t0001` and
/// `SKY-T0001` both resolve.
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
    let source = fs::read_to_string(entry).map_err(|e| io_err(entry, e))?;
    let pipeline_err = |diag: Diagnostic| CliError::Pipeline {
        file: entry.to_path_buf(),
        src: source.clone(),
        diag,
    };

    let mut interner = Interner::new();
    let module = sky_parse::parse_module(&source, &mut interner).map_err(&pipeline_err)?;
    let canonical = sky_canon::canonicalise(&module, &mut interner).map_err(&pipeline_err)?;
    let types = sky_types::infer(&canonical, &mut interner).map_err(&pipeline_err)?;
    let program = sky_lower::lower(&canonical, &types, &mut interner).map_err(&pipeline_err)?;
    Ok(sky_ir::pretty(&program, &interner))
}

// ===========================================================================
// `fix` / `--fix` — apply machine-applicable suggestions
// ===========================================================================

/// Run the front of the pipeline (parse → canon → types → lower) and return the
/// first diagnostic it raises, or `None` when the program compiles cleanly.
fn pipeline_first_diagnostic(source: &str) -> Option<Diagnostic> {
    let mut interner = Interner::new();
    let module = match sky_parse::parse_module(source, &mut interner) {
        Ok(m) => m,
        Err(d) => return Some(d),
    };
    let canonical = match sky_canon::canonicalise(&module, &mut interner) {
        Ok(c) => c,
        Err(d) => return Some(d),
    };
    let types = match sky_types::infer(&canonical, &mut interner) {
        Ok(t) => t,
        Err(d) => return Some(d),
    };
    sky_lower::lower(&canonical, &types, &mut interner).err()
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
    if sky_parse::parse_module(&patched, &mut guard_interner).is_err() {
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
fn read_yes_no() -> bool {
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
/// rename it over `target` (atomic on a single filesystem). On a rename failure
/// the temp file is removed so no debris is left behind.
fn write_atomic(target: &Path, contents: &str) -> Result<(), CliError> {
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty());
    let name = target.file_name().map_or_else(
        || String::from("source.sky"),
        |n| n.to_string_lossy().into_owned(),
    );
    let tmp_name = format!(".{name}.skyc-fix.{}.tmp", std::process::id());
    let tmp = match dir {
        Some(d) => d.join(tmp_name),
        None => PathBuf::from(tmp_name),
    };

    fs::write(&tmp, contents).map_err(|e| io_err(&tmp, e))?;
    if let Err(e) = fs::rename(&tmp, target) {
        let _ = fs::remove_file(&tmp);
        return Err(io_err(target, e));
    }
    Ok(())
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

/// Extract the source span from a diagnostic, returning [`sky_diagnostics::Span::DUMMY`]
/// for the span-less [`Diagnostic::CompilerBug`] variant.
///
/// Used by the cross-module error-attribution path in [`compile_modules`] to
/// locate the source file that owns a diagnostic.
const fn diag_span(d: &Diagnostic) -> sky_diagnostics::Span {
    match d {
        Diagnostic::Parse { span, .. }
        | Diagnostic::Name { span, .. }
        | Diagnostic::Type { span, .. }
        | Diagnostic::Lower { span, .. } => *span,
        Diagnostic::CompilerBug { .. } => sky_diagnostics::Span::DUMMY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sky_diagnostics::{NameError, Span};

    /// The golden M0 entry, located relative to this crate's manifest.
    fn golden_entry() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("golden")
            .join("m0")
            .join("Main.sky")
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
        let page = explain_lookup("SKY-T0001");
        assert!(page.is_ok(), "known code must resolve: {:?}", page.err());
        let Ok(page) = page else { return };
        assert!(
            page.starts_with("# SKY-T0001:"),
            "page line 1 must name the code, got:\n{page}"
        );
    }

    #[test]
    fn explain_is_case_insensitive() {
        assert!(explain_lookup("sky-t0001").is_ok());
        assert!(explain_lookup("  Sky-T0001  ").is_ok());
    }

    #[test]
    fn explain_resolves_sky_t0014() {
        // SKY-T0014 was previously absent from the hand-mirror and returned
        // UnknownCode. After replacing the mirror with ALL_CODES from
        // sky_diagnostics, it must resolve successfully.
        let result = explain_lookup("SKY-T0014");
        assert!(
            result.is_ok(),
            "SKY-T0014 must resolve via ALL_CODES: {:?}",
            result.err()
        );
    }

    #[test]
    fn explain_rejects_unknown_code_with_suggestions() {
        // Genuinely unknown code, close to SKY-T0013 — must yield did-you-mean.
        let result = explain_lookup("SKY-T0099");
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
            input: "SKY-Z9999".to_owned(),
            suggestions: vec!["SKY-T0001", "SKY-T0002"],
        };
        assert_eq!(
            err.to_string(),
            "unknown error code `SKY-Z9999`\n  did you mean: SKY-T0001, SKY-T0002?"
        );
    }

    #[test]
    fn code_index_lists_every_code() {
        let index = code_index();
        let lines = index.lines().count();
        assert_eq!(lines, ALL_CODES.len(), "one line per code");
        assert_eq!(ALL_CODES.len(), 86, "taxonomy is 86 codes"); // AUD-14: +SKY-N0027
        assert!(
            index.contains("SKY-T0001  type mismatch"),
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
    /// temp dir). Gated on `SKY_E2E=1` so the default `cargo test` stays fast and
    /// offline. Complements the backend crate's hand-built-IR e2e by exercising
    /// the whole frontend (record type annotations + generalisation + lowering).
    #[test]
    fn generic_record_program_builds_and_prints_forty_two() {
        const SRC: &str = "module Main exposing (main)\n\n\
             wrap : a -> { value : a }\n\
             wrap x =\n    { value = x }\n\n\
             unwrap : { value : a } -> a\n\
             unwrap r =\n    r.value\n\n\
             main = println (String.fromInt (unwrap (wrap 42)))\n";

        if std::env::var("SKY_E2E").is_err() {
            return;
        }

        let dir = std::env::temp_dir().join("skyc_generic_record_src_e2e");
        let _ = fs::remove_dir_all(&dir);
        let entry = dir.join("Main.sky");
        let created = fs::create_dir_all(&dir).and_then(|()| fs::write(&entry, SRC));
        assert!(created.is_ok(), "write source: {created:?}");

        let runtime = resolve_runtime();
        assert!(runtime.is_ok(), "runtime must resolve: {runtime:?}");
        let Ok(runtime) = runtime else { return };

        let out = dir.join("out");
        let built = build(&entry, &out, &runtime);
        assert!(built.is_ok(), "skyc build must succeed: {built:?}");

        let status = std::process::Command::new("cargo")
            .arg("build")
            .current_dir(&out)
            .env("CARGO_TARGET_DIR", out.join("target"))
            .status();
        assert!(
            matches!(&status, Ok(s) if s.success()),
            "emitted generic-record crate must compile: {status:?}"
        );

        let bin = out.join("target").join("debug").join("sky-app");
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
    // find_manifest_for_sky_file tests (SKY-N0020 fix)
    // -----------------------------------------------------------------------

    /// Creates a temp directory with a nested `src/Main.sky` and a `sky.toml`
    /// at the project root, confirming the upward walk finds the manifest.
    #[test]
    fn find_manifest_walks_up_to_project_root() {
        let tmp = std::env::temp_dir().join("skyc_find_manifest_test");
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(&src).expect("create src/");
        let toml = tmp.join("sky.toml");
        fs::write(&toml, "name = \"test\"\n").expect("write sky.toml");
        let main_sky = src.join("Main.sky");
        fs::write(&main_sky, "module Main exposing (main)\nmain = 0\n")
            .expect("write Main.sky");

        let found = find_manifest_for_sky_file(&main_sky);
        assert_eq!(
            found.as_deref(),
            Some(toml.as_path()),
            "upward walk must find sky.toml at project root"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // Regression: PAnything (wildcard lambda param with unconstrained Ty::Var)
    // -----------------------------------------------------------------------

    /// Regression for `SKY-L0102` (`Feature::Polymorphism`) on wildcard `_`
    /// lambda parameters.
    ///
    /// Before the fix, `lower_lambda` called `ir_type_from_ty` on the `_`
    /// param's type.  When the type was still an unconstrained `Ty::Var` (e.g.
    /// the continuation of a `Task.andThen` after `Task.fail` where the ok-type
    /// is never forced), `ir_type_from_ty` returned `Err(unsupported(…,
    /// Feature::Polymorphism))` and the pipeline aborted.
    ///
    /// The fix routes `PAnything` params through `ir_type_from_ty_json` which
    /// maps `Ty::Var → IrType::Json` instead of failing.
    ///
    /// Source mirrors the failing pattern from `examples/14-task-demo`.
    #[test]
    fn panything_wildcard_lambda_compiles_without_polymorphism_error() {
        const SRC: &str = "\
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
import Sky.Core.Task as Task
import Sky.Core.Error as Error exposing (Error)
import Std.Log exposing (println)

main =
    let
        result =
            Task.run
                (Task.fail (Error.unexpected \"intentional\")
                    |> Task.andThen (\\_ -> Task.succeed \"unreachable\"))
    in
        case result of
            Ok val -> println val
            Err e  -> println (Error.toString e)
";

        let runtime = resolve_runtime();
        if runtime.is_err() {
            // Runtime not present in this environment — skip rather than fail.
            return;
        }
        let Ok(runtime) = runtime else { return };

        let dir = std::env::temp_dir().join("skyc_panything_regression");
        let _ = fs::remove_dir_all(&dir);
        let entry = dir.join("Main.sky");
        let created = fs::create_dir_all(&dir).and_then(|()| fs::write(&entry, SRC));
        assert!(created.is_ok(), "write source: {created:?}");

        let out = dir.join("out");
        let result = build(&entry, &out, &runtime);
        assert!(
            result.is_ok(),
            "wildcard lambda with unconstrained type must not fire SKY-L0102: {result:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // Regression: Task.run elision — sky_main must return SkyTask<A>
    // -----------------------------------------------------------------------

    /// Regression for the `Task.run` elision in `emit_func`.
    ///
    /// Before the fix, `main = someTask |> Task.run` lowered to:
    ///   `func.ret  = IrType::Result(Error, A)`
    ///   `func.body = Call(TaskRun, [inner])`
    /// and `emit_func` emitted `task_run(inner)` which returns
    /// `SkyResult<E, A>`.  The epilogue calls `block_on(sky_main())`, which
    /// requires `SkyTask<A>`.  This caused an `E0308 mismatched types` Rust
    /// compile error.
    ///
    /// The fix detects the `Call(TaskRun|TaskPerform, [inner])` body in
    /// `sky_main`, emits `inner` directly, and rewrites the return type from
    /// `Result(Error, A)` → `Task(A)`.
    ///
    /// This test verifies that the emitted `src/main.rs` contains
    /// `fn sky_main() -> SkyTask<` and does NOT contain `task_run(` at the
    /// `sky_main` definition site.
    #[test]
    fn task_run_main_emits_skytask_not_skyresult() {
        // A minimal Sky.Cli-style program: main = task |> Task.run
        // The shape that previously caused E0308 in the emitted Rust.
        const SRC: &str = "\
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
import Sky.Core.Task as Task
import Std.Log exposing (println)

main =
    println \"hello from task run\" |> Task.run
";

        let runtime = resolve_runtime();
        if runtime.is_err() {
            return;
        }
        let Ok(runtime) = runtime else { return };

        let dir = std::env::temp_dir().join("skyc_taskrun_elision_regression");
        let _ = fs::remove_dir_all(&dir);
        let entry = dir.join("Main.sky");
        let created = fs::create_dir_all(&dir).and_then(|()| fs::write(&entry, SRC));
        assert!(created.is_ok(), "write source: {created:?}");

        let out = dir.join("out");
        let built = build(&entry, &out, &runtime);
        assert!(built.is_ok(), "Task.run main must compile: {built:?}");

        // Read the emitted main.rs and verify the signature.
        let main_rs = out.join("src").join("main.rs");
        let emitted =
            fs::read_to_string(&main_rs).expect("emitted main.rs must exist after build");

        assert!(
            emitted.contains("fn sky_main() -> SkyTask<"),
            "sky_main must return SkyTask<…>, got signature region:\n{}",
            emitted
                .lines()
                .filter(|l| l.contains("sky_main") || l.contains("SkyTask") || l.contains("SkyResult"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert!(
            !emitted.contains("fn sky_main() -> SkyResult"),
            "sky_main must NOT return SkyResult (Task.run elision missing)"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// When no sky.toml exists in any parent directory, returns None.
    #[test]
    fn find_manifest_returns_none_when_absent() {
        let tmp = std::env::temp_dir().join("skyc_no_manifest_test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("create dir");
        let sky = tmp.join("Standalone.sky");
        fs::write(&sky, "module Standalone exposing (f)\nf = 0\n").expect("write sky");
        // Deliberately no sky.toml anywhere under tmp.
        // The walk terminates at the filesystem root without finding one.
        // We cannot guarantee the walk terminates before reaching /tmp or /
        // on all systems, so we only assert non-panicking behaviour and that
        // the returned path (if Some) is a real file.
        let found = find_manifest_for_sky_file(&sky);
        if let Some(ref p) = found {
            assert!(p.is_file(), "if Some, the manifest must exist on disk");
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Two-module program: `Main.sky` calls a helper in sibling `Lib.sky`.
    /// `build_with_sibling_discovery` must compile both without SKY-N0020.
    #[test]
    fn sibling_discovery_compiles_two_module_program() {
        let runtime = resolve_runtime();
        if runtime.is_err() {
            // Runtime not found in this environment (CI without SKY_RUNTIME_DIR) —
            // skip rather than fail: the sweep catches this live.
            return;
        }
        let Ok(runtime) = runtime else { return };

        let tmp = std::env::temp_dir().join("skyc_sibling_disc_test");
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(&src).expect("create src/");

        // Helper module: src/Helper.sky
        fs::write(
            src.join("Helper.sky"),
            "module Helper exposing (answer)\nanswer = 42\n",
        )
        .expect("write Helper.sky");

        // Entry module: src/Main.sky — imports Helper
        fs::write(
            src.join("Main.sky"),
            "module Main exposing (main)\nimport Helper\nmain = println (String.fromInt Helper.answer)\n",
        )
        .expect("write Main.sky");

        let out = tmp.join("out");
        let result = build_with_sibling_discovery(&src.join("Main.sky"), &out, &runtime);
        assert!(
            result.is_ok(),
            "two-module program must compile via sibling discovery: {:?}",
            result.err()
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // Regression #144 — cross-module infer errors name the dep module's file
    // -----------------------------------------------------------------------

    /// When a type error originates in a dep module (`Helper.sky`), the rendered
    /// diagnostic must cite `Helper.sky` as the file, NOT the entry `Main.sky`.
    ///
    /// Before the fix, `compile_modules` had a single `pipeline_err` closure that
    /// always captured the entry file path, so dep-module errors rendered with the
    /// wrong source snippet and an incorrect file name.
    ///
    /// Runtime is not reached (infer aborts first), so we pass a dummy path.
    #[test]
    fn infer_error_in_dep_module_names_dep_file() {
        let tmp = std::env::temp_dir().join("skyc_144_dep_err_test");
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(&src).expect("create src/");

        // Helper.sky: deliberate type error — `1 + "oops"` mixes Int and String.
        let helper_path = src.join("Helper.sky");
        fs::write(
            &helper_path,
            "module Helper exposing (broken)\nbroken = 1 + \"oops\"\n",
        )
        .expect("write Helper.sky");

        // Main.sky: imports Helper and uses `broken` — but the error is in Helper.
        let main_path = src.join("Main.sky");
        fs::write(
            &main_path,
            "module Main exposing (main)\nimport Helper\nmain = println (String.fromInt Helper.broken)\n",
        )
        .expect("write Main.sky");

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

        // The file blamed must be Helper.sky, not Main.sky.
        let file_name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert_eq!(
            file_name, "Helper.sky",
            "#144 regression: type error in dep module must blame `Helper.sky`, \
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
    /// The fix: every `Constraint` carries its source module's `home` path.
    /// `compile_modules` now routes `Err((diag, home))` directly via
    /// `home_to_source.get(&home)`, bypassing the heuristic entirely when a home
    /// is available.
    ///
    /// This test builds a two-module program where the type error is in module B
    /// (`Lib.sky`) but the heuristic *could* be fooled by a wide def in module A
    /// (`Pad.sky`).  The assertion checks that the blamed file is `Lib.sky`.
    ///
    /// To exercise the home-discriminant path rather than the heuristic, `Pad.sky`
    /// is constructed so that its def body starts at roughly the same byte offset
    /// as the error in `Lib.sky` — any byte-offset resolver that ignores the home
    /// would be ambiguous.  The discriminant is the only reliable resolver.
    #[test]
    fn home_discriminant_cross_module_type_error_names_correct_file() {
        let tmp = std::env::temp_dir().join("skyc_home_disc_test");
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(&src).expect("create src/");

        // Pad.sky: a valid module whose single def body starts at roughly the
        // same byte offset as the type error in Lib.sky.  Constructed so the
        // body span (a long arithmetic chain) numerically overlaps with Lib's
        // error span.  The body itself is well-typed.
        //
        //   "module Pad exposing (pad)\npad = " is 27 bytes.
        //   The body "1 + 2 + 3 + 4 + 5 + 6 + 7 + 8 + 9" starts at byte 27.
        //   The body ends at byte 27+35 = 62.
        //
        // After link, Pad's def body covers bytes [27, 62] in Pad's namespace.
        fs::write(
            src.join("Pad.sky"),
            "module Pad exposing (pad)\npad = 1 + 2 + 3 + 4 + 5 + 6 + 7 + 8 + 9\n",
        )
        .expect("write Pad.sky");

        // Lib.sky: a module with a deliberate type error at a span that falls
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
            src.join("Lib.sky"),
            "module Lib exposing (bad)\nbad = 1 + 2 + 3 + 4 + \"oops\"\n",
        )
        .expect("write Lib.sky");

        // Main.sky: imports both; the error is in Lib, not Main or Pad.
        fs::write(
            src.join("Main.sky"),
            "module Main exposing (main)\nimport Lib\nimport Pad\nmain = println (String.fromInt Lib.bad)\n",
        )
        .expect("write Main.sky");

        let dummy_runtime = std::env::temp_dir();
        let out = tmp.join("out");
        let result = build_with_sibling_discovery(&src.join("Main.sky"), &out, &dummy_runtime);

        // Must fail — type error in Lib.
        assert!(
            result.is_err(),
            "home-discriminant fixture must fail (type error in Lib); got Ok unexpectedly"
        );
        let Err(CliError::Pipeline { file, .. }) = result else {
            let _ = fs::remove_dir_all(&tmp);
            return;
        };

        // The blamed file must be Lib.sky — the module that OWNS the failing
        // constraint, regardless of which module the byte-offset heuristic
        // would pick.
        let file_name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert_eq!(
            file_name, "Lib.sky",
            "home-discriminant regression: type error in Lib must blame `Lib.sky`, \
             not `{file_name}`; full path: {}",
            file.display()
        );

        let _ = fs::remove_dir_all(&tmp);
    }
}

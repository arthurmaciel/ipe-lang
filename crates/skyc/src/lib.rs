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
use std::io::Write;
use std::path::{Path, PathBuf};

use sky_backend::Backend;
use sky_backend_rust::RustBackend;
use sky_diagnostics::{
    Applicability, Code, Diagnostic, HelpLine, Suggestion, explain_page, render, title,
};
use sky_intern::Interner;

/// Every shipped taxonomy code, built from the crate's public constants.
///
/// `sky_diagnostics::Code` has a private field — it cannot be forged from a
/// user-supplied string — so `skyc explain` resolves a code by scanning this
/// authoritative list and comparing [`Code::as_str`]. The list mirrors the
/// taxonomy in `sky_diagnostics::code`; the `explain`/`index` tests fail loudly
/// if it drifts (count + page-presence checks).
const ALL_CODES: &[Code] = &[
    sky_diagnostics::SKY_P0001,
    sky_diagnostics::SKY_P0002,
    sky_diagnostics::SKY_P0003,
    sky_diagnostics::SKY_P0010,
    sky_diagnostics::SKY_P0011,
    sky_diagnostics::SKY_P0012,
    sky_diagnostics::SKY_P0013,
    sky_diagnostics::SKY_P0014,
    sky_diagnostics::SKY_P0015,
    sky_diagnostics::SKY_P0020,
    sky_diagnostics::SKY_P0021,
    sky_diagnostics::SKY_P0030,
    sky_diagnostics::SKY_P0031,
    sky_diagnostics::SKY_P0040,
    sky_diagnostics::SKY_P0041,
    sky_diagnostics::SKY_P0050,
    sky_diagnostics::SKY_P0060,
    sky_diagnostics::SKY_P0061,
    sky_diagnostics::SKY_P0062,
    sky_diagnostics::SKY_N0001,
    sky_diagnostics::SKY_N0002,
    sky_diagnostics::SKY_N0003,
    sky_diagnostics::SKY_N0004,
    sky_diagnostics::SKY_N0005,
    sky_diagnostics::SKY_N0010,
    sky_diagnostics::SKY_N0011,
    sky_diagnostics::SKY_N0012,
    sky_diagnostics::SKY_N0013,
    sky_diagnostics::SKY_T0001,
    sky_diagnostics::SKY_T0002,
    sky_diagnostics::SKY_T0003,
    sky_diagnostics::SKY_T0004,
    sky_diagnostics::SKY_T0010,
    sky_diagnostics::SKY_T0011,
    sky_diagnostics::SKY_T0012,
    sky_diagnostics::SKY_T0013,
    sky_diagnostics::SKY_L0100,
    sky_diagnostics::SKY_L0101,
    sky_diagnostics::SKY_L0102,
    sky_diagnostics::SKY_L0103,
    sky_diagnostics::SKY_L0104,
    sky_diagnostics::SKY_L0105,
    sky_diagnostics::SKY_L0106,
    sky_diagnostics::SKY_L0107,
    sky_diagnostics::SKY_L0108,
    sky_diagnostics::SKY_L0110,
    sky_diagnostics::SKY_L0111,
    sky_diagnostics::SKY_L0112,
    sky_diagnostics::SKY_L0113,
    sky_diagnostics::SKY_L0200,
    sky_diagnostics::SKY_I0001,
    sky_diagnostics::SKY_I0010,
    sky_diagnostics::SKY_I0011,
    sky_diagnostics::SKY_I0100,
    sky_diagnostics::SKY_I0101,
    sky_diagnostics::SKY_I0102,
    sky_diagnostics::SKY_I0103,
    sky_diagnostics::SKY_I0200,
    sky_diagnostics::SKY_I0201,
    sky_diagnostics::SKY_I0202,
    sky_diagnostics::SKY_I0203,
];

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
            Self::Io { path, source } => write!(f, "io error at {}: {source}", path.display()),
            Self::Pipeline { file, src, diag } => {
                f.write_str(&render(diag, &file.to_string_lossy(), src))
            }
            Self::RuntimeNotFound => write!(
                f,
                "could not locate the Sky runtime; set SKY_RUNTIME_DIR or pass --runtime <dir>"
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
    let program = sky_lower::lower(&canonical, &types, &mut interner).map_err(&pipeline_err)?;
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

/// The top-level usage hint, listing every subcommand and flag.
const USAGE: &str = "usage:\n  \
     skyc build <entry.sky> [--out <dir>] [--runtime <dir>] [--emit-ir] [--fix]\n  \
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
    build(&entry_path, &out_dir, &runtime_dir)
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
    fn explain_rejects_unknown_code_with_suggestions() {
        // One digit off SKY-T0013 / SKY-T0010..12 — close enough to suggest.
        let result = explain_lookup("SKY-T0014");
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
        assert_eq!(ALL_CODES.len(), 61, "taxonomy is 61 codes");
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
    }

    /// A runtime `false` the optimiser cannot fold, so `assert!(false_marker())`
    /// fails the test without tripping `clippy::assertions_on_constants`.
    fn false_marker() -> bool {
        std::hint::black_box(false)
    }
}

//! `ipe lint [<path>]` (+ `--fix`) — extensible static analysis over `.ipe`
//! source.
//!
//! The command loads the project's own (user) modules through the SAME
//! resolution path `ipe build` uses — never the injected stdlib — reads an
//! optional `lint.ipe`, runs every enabled rule over the parsed source, and
//! renders each finding compiler-style. With `--fix` it applies every
//! machine-applicable (semantics-preserving) rewrite and reports what changed;
//! otherwise a surviving finding at or above the configured gate severity exits
//! non-zero for CI.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ipe_lint::{LintConfig, SourceModule};

use crate::{CliError, watch};

/// The `lint.ipe` file name, resolved next to a project's `package.ipe` (or in
/// the current directory for a single-file lint).
const LINT_IPE: &str = "lint.ipe";

/// Parsed `ipe lint` arguments.
struct LintArgs {
    /// The path to lint: a `.ipe` file or a project directory. Defaults to the
    /// current project.
    entry: Option<String>,
    /// Apply every machine-applicable fix instead of only reporting.
    fix: bool,
}

/// Parse `ipe lint` arguments: an optional positional path and `--fix`.
fn parse_lint(rest: &[String]) -> Result<LintArgs, CliError> {
    let mut entry: Option<String> = None;
    let mut fix = false;
    for arg in rest {
        match arg.as_str() {
            "--fix" => fix = true,
            flag if flag.starts_with('-') => {
                return Err(crate::cli_args::usage_unknown_flag("lint", flag));
            }
            positional => {
                if entry.is_some() {
                    return Err(CliError::Usage("ipe lint takes at most one path"));
                }
                entry = Some(positional.to_owned());
            }
        }
    }
    Ok(LintArgs { entry, fix })
}

/// `ipe lint [<path>]` — run the linter, printing findings (or applying fixes).
///
/// # Errors
/// [`CliError::Usage`] on misuse; [`CliError::Io`] on a filesystem failure;
/// [`CliError::Pipeline`] if an entry file fails to parse; [`CliError::UsageOwned`]
/// for a malformed `lint.ipe`; [`CliError::LintGateFailed`] when a surviving
/// finding is at or above the gate severity (report path only).
pub(crate) fn run_lint(rest: &[String]) -> Result<(), CliError> {
    let args = parse_lint(rest)?;
    let entry = match args.entry {
        Some(e) => PathBuf::from(e),
        None => PathBuf::from(crate::default_entry()?),
    };

    let resolved = watch::resolve_project_sources(&entry, None)?;

    // The config lives next to the resolved manifest, or in the entry's
    // directory for a single-file lint. Absent → defaults.
    let config = load_config(&resolved.blame_path)?;

    // Map every user module to its source and file path (for rendering).
    let mut paths: BTreeMap<Vec<String>, PathBuf> = BTreeMap::new();
    let mut modules: Vec<SourceModule> = Vec::new();
    for (module, (path, source)) in &resolved.sources {
        paths.insert(module.clone(), path.clone());
        modules.push(SourceModule {
            module: module.clone(),
            source: source.clone(),
        });
    }

    if args.fix {
        return apply_and_report(&modules, &config, &paths);
    }
    report_findings(&modules, &config, &paths)
}

/// Read `lint.ipe` from the directory holding `blame_path` (the resolved
/// manifest or entry file), returning defaults when none exists.
fn load_config(blame_path: &Path) -> Result<LintConfig, CliError> {
    let dir = blame_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let lint_ipe = dir.join(LINT_IPE);
    if !lint_ipe.is_file() {
        return Ok(LintConfig::default());
    }
    let text =
        crate::io_bounded::read_to_string_capped(&lint_ipe, crate::io_bounded::MANIFEST_READ_CAP)?;
    ipe_lint::read_lint_config(&text, &lint_ipe.display().to_string())
        .map_err(|e| CliError::UsageOwned(e.to_string()))
}

/// Run the linter and print each finding; fail the gate if any survives at or
/// above the gate severity.
fn report_findings(
    modules: &[SourceModule],
    config: &LintConfig,
    paths: &BTreeMap<Vec<String>, PathBuf>,
) -> Result<(), CliError> {
    let report = ipe_lint::run(modules, config);
    let source_of: BTreeMap<&[String], &str> = modules
        .iter()
        .map(|m| (m.module.as_slice(), m.source.as_str()))
        .collect();

    if report.findings.is_empty() {
        println!("{}", crate::style::gutter("lint: no findings"));
        return Ok(());
    }

    for finding in &report.findings {
        let file = paths
            .get(&finding.module)
            .map_or_else(|| finding.module.join("."), |p| p.display().to_string());
        let source = source_of
            .get(finding.module.as_slice())
            .copied()
            .unwrap_or("");
        let severity = config.severity_of(finding.rule);
        // `render_finding` ends with a newline; `println!` adds the blank line
        // that separates one finding's block from the next.
        println!(
            "{}",
            ipe_lint::render_finding(finding, &file, source, severity)
        );
    }

    let count = report.findings.len();
    println!(
        "{}",
        crate::style::gutter(&format!(
            "lint: {count} finding{}",
            if count == 1 { "" } else { "s" }
        ))
    );

    if report.gate_tripped(config) {
        return Err(CliError::LintGateFailed);
    }
    Ok(())
}

/// Apply every machine-applicable fix, write the changed sources back to disk,
/// and report what changed.
fn apply_and_report(
    modules: &[SourceModule],
    config: &LintConfig,
    paths: &BTreeMap<Vec<String>, PathBuf>,
) -> Result<(), CliError> {
    let outcome = ipe_lint::apply_fixes(modules, config);
    if outcome.applied == 0 {
        println!(
            "{}",
            crate::style::gutter("lint --fix: no machine-applicable fixes")
        );
        return Ok(());
    }

    for (module, rewritten) in &outcome.rewritten {
        let Some(path) = paths.get(module) else {
            continue;
        };
        crate::write_atomic(path, rewritten)?;
        println!(
            "{}",
            crate::style::gutter(&format!("lint --fix: rewrote {}", path.display()))
        );
    }
    println!(
        "{}",
        crate::style::gutter(&format!(
            "lint --fix: applied {} fix{}",
            outcome.applied,
            if outcome.applied == 1 { "" } else { "es" }
        ))
    );
    Ok(())
}

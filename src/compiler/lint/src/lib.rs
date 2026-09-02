#![forbid(unsafe_code)]
//! `ipe_lint` — extensible static analysis over valid Ipê source.
//!
//! The compiler enforces what *must* be true (soundness — it may only reject).
//! The linter enforces what *should* be true by convention on code that already
//! type-checks: idiom, consistency, and the "make invalid states
//! unrepresentable" discipline the language deliberately keeps out of its small,
//! refinement-free core. A rule is a pure function from a parsed module to a
//! list of [`Finding`]s; a finding optionally carries a semantics-preserving
//! [`Fix`] that `ipe lint --fix` applies. The same rules flow three ways —
//! `ipe lint` (CLI), the LSP (as diagnostics), and CI (a non-zero exit on a
//! surviving denied finding).
//!
//! The front-end is reused, never re-implemented: each module is parsed with
//! [`ipe_parse::parse_module`] (the compiler's own parser) and the rules walk
//! that AST. `lint.ipe` is likewise parsed with the front-end and its sole
//! `lint` binding walked — never evaluated (see [`config`]).

mod config;
mod finding;
mod registry;
mod render;
mod rules;

use std::collections::BTreeMap;

use ipe_intern::Interner;

pub use config::{ConfigError, LintConfig, Suppressions, read_lint_config};
pub use finding::{Finding, Fix, Severity};
pub use registry::{RULES, RuleInfo, is_known, lookup};
pub use render::render_finding;

/// One module handed to the linter: its dotted path and its source text.
#[derive(Clone, Debug)]
pub struct SourceModule {
    /// Dotted module-path segments, e.g. `["Main"]`.
    pub module: Vec<String>,
    /// The module's full source text.
    pub source: String,
}

/// The outcome of a lint run: the surviving findings (config- and
/// suppression-filtered, deterministically ordered) and whether any of them is
/// at or above the gate severity — the CI exit signal.
#[derive(Clone, Debug, Default)]
pub struct LintReport {
    /// Findings that survived rule severity and inline suppression, sorted.
    pub findings: Vec<Finding>,
}

impl LintReport {
    /// True when a surviving finding is at or above `config`'s gate severity —
    /// the signal `ipe lint` turns into a non-zero exit for CI.
    #[must_use]
    pub fn gate_tripped(&self, config: &LintConfig) -> bool {
        let gate = config.gate();
        self.findings
            .iter()
            .any(|f| config.severity_of(f.rule) >= gate && gate != Severity::Allow)
    }
}

/// Run every enabled rule over `modules` under `config`, dropping findings whose
/// rule is `Allow` or that an inline `-- ipe-lint: allow <rule>` suppresses.
///
/// The result is deterministically ordered by `(module, span, rule)`, so a
/// report and its golden are stable regardless of module or rule evaluation
/// order.
#[must_use]
pub fn run(modules: &[SourceModule], config: &LintConfig) -> LintReport {
    let mut findings: Vec<Finding> = Vec::new();
    for module in modules {
        // Each module parses with its own interner — the same front-end entry
        // (`ipe_parse::parse_module`) the compiler and the `package.ipe` reader
        // use. A module that does not parse is the compiler's business, not the
        // linter's: the linter reasons only over valid code, so a red parse
        // yields no lint findings (the build surfaces the parse error itself).
        let mut interner = Interner::new();
        let Ok(parsed) = ipe_parse::parse_module(&module.source, &mut interner) else {
            continue;
        };
        let ctx = rules::Ctx {
            module: &module.module,
            source: &module.source,
            interner: &interner,
            ast: &parsed,
        };
        let suppressions = Suppressions::scan(&module.source);
        for raw in rules::run_all(&ctx) {
            let severity = config.severity_of(raw.rule);
            if severity == Severity::Allow {
                continue;
            }
            let line = zero_based_line(&module.source, raw.span.lo);
            if suppressions.suppresses(raw.rule, line) {
                continue;
            }
            findings.push(raw);
        }
    }
    findings.sort();
    LintReport { findings }
}

/// Apply every [`Fix`] a lint run over `modules` produces, returning the rewritten
/// source per module that changed and the count of fixes applied.
///
/// Only fixes from enabled (non-`Allow`), non-suppressed findings are applied.
/// Fixes within one module are applied in reverse source order so an earlier
/// edit never shifts a later edit's byte offsets; overlapping fixes are resolved
/// by keeping the earliest and skipping any that overlaps an already-applied one,
/// so the result is always well-formed. Re-running `run` over the rewritten
/// source reports strictly fewer findings (idempotence), because every fix is
/// semantics-preserving and removes the finding that produced it.
#[must_use]
pub fn apply_fixes(modules: &[SourceModule], config: &LintConfig) -> FixOutcome {
    let report = run(modules, config);
    let mut edits_by_module: BTreeMap<Vec<String>, Vec<Fix>> = BTreeMap::new();
    for finding in &report.findings {
        if let Some(fix) = &finding.fix {
            edits_by_module
                .entry(finding.module.clone())
                .or_default()
                .push(fix.clone());
        }
    }

    let mut rewritten: BTreeMap<Vec<String>, String> = BTreeMap::new();
    let mut applied = 0usize;
    for module in modules {
        let Some(fixes) = edits_by_module.get(&module.module) else {
            continue;
        };
        let (text, n) = apply_module_fixes(&module.source, fixes);
        if n > 0 {
            rewritten.insert(module.module.clone(), text);
            applied += n;
        }
    }
    FixOutcome { rewritten, applied }
}

/// The result of [`apply_fixes`]: the rewritten source for each module that
/// changed, keyed by module path, and the total number of fixes applied.
#[derive(Clone, Debug, Default)]
pub struct FixOutcome {
    /// Module path → its rewritten source, for modules that changed.
    pub rewritten: BTreeMap<Vec<String>, String>,
    /// The total number of fixes applied across all modules.
    pub applied: usize,
}

/// Apply one module's fixes to its source, returning the rewritten text and the
/// count applied. Fixes are sorted by descending start offset and applied in
/// that order so no applied edit shifts a not-yet-applied edit's offsets; a fix
/// overlapping an already-applied one is skipped.
fn apply_module_fixes(source: &str, fixes: &[Fix]) -> (String, usize) {
    let mut ordered: Vec<&Fix> = fixes.iter().collect();
    // Descending by start; a stable tie-break by end keeps the order total.
    ordered.sort_by(|a, b| b.span.lo.cmp(&a.span.lo).then(b.span.hi.cmp(&a.span.hi)));

    let mut text = source.to_owned();
    let mut applied = 0usize;
    // Track the lowest start already edited: since we go high→low, a fix whose
    // end reaches into an already-applied region overlaps and is skipped.
    let mut lowest_edited = u32::MAX;
    for fix in ordered {
        if fix.span.hi > lowest_edited {
            continue;
        }
        let lo = fix.span.lo as usize;
        let hi = fix.span.hi as usize;
        // Guard the byte range against a stale / out-of-bounds span rather than
        // indexing (which would panic): a fix that does not name a valid char
        // boundary range is dropped, never applied blindly.
        if lo > hi || hi > text.len() || !text.is_char_boundary(lo) || !text.is_char_boundary(hi) {
            continue;
        }
        text.replace_range(lo..hi, &fix.replacement);
        lowest_edited = fix.span.lo;
        applied += 1;
    }
    (text, applied)
}

/// The 0-based line number containing byte offset `at`, clamped so an
/// out-of-range offset degrades to the last line rather than panicking.
fn zero_based_line(source: &str, at: u32) -> usize {
    let at = (at as usize).min(source.len());
    source
        .get(..at)
        .unwrap_or("")
        .bytes()
        .filter(|&b| b == b'\n')
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(source: &str) -> SourceModule {
        SourceModule {
            module: vec!["Main".to_owned()],
            source: source.to_owned(),
        }
    }

    #[test]
    fn adjacent_bools_is_found_over_a_fixture() {
        let src = "module Main exposing (render)\n\nrender : Bool -> Bool -> String\nrender a b =\n    \"x\"\n";
        let report = run(&[module(src)], &LintConfig::default());
        assert!(
            report.findings.iter().any(|f| f.rule == "adjacent-bools"),
            "expected an adjacent-bools finding, got {:?}",
            report.findings
        );
    }

    #[test]
    fn allow_severity_silences_a_rule() {
        let src = "module Main exposing (render)\n\nrender : Bool -> Bool -> String\nrender a b =\n    \"x\"\n";
        let config = read_lint_config(
            "module Lint exposing (lint)\n\nlint = Lint.config |> Lint.allow \"adjacent-bools\"\n",
            "lint.ipe",
        )
        .expect("config parses");
        let report = run(&[module(src)], &config);
        assert!(
            !report.findings.iter().any(|f| f.rule == "adjacent-bools"),
            "allow should silence the rule"
        );
    }

    #[test]
    fn inline_suppression_silences_one_site() {
        let src = "module Main exposing (render)\n\n-- ipe-lint: allow adjacent-bools\nrender : Bool -> Bool -> String\nrender a b =\n    \"x\"\n";
        let report = run(&[module(src)], &LintConfig::default());
        assert!(
            !report.findings.iter().any(|f| f.rule == "adjacent-bools"),
            "inline suppression should silence the site"
        );
    }

    #[test]
    fn fixes_are_idempotent() {
        // A nested call the prefer-pipeline rule rewrites; re-running finds none.
        let src =
            "module Main exposing (main)\n\nmain =\n    List.map fmt (List.filter live records)\n";
        let outcome = apply_fixes(&[module(src)], &LintConfig::default());
        assert_eq!(outcome.applied, 1, "one pipeline rewrite expected");
        let fixed = outcome
            .rewritten
            .get(&vec!["Main".to_owned()])
            .expect("Main was rewritten")
            .clone();
        let second = apply_fixes(&[module(&fixed)], &LintConfig::default());
        assert_eq!(
            second.applied, 0,
            "re-running --fix finds nothing to change"
        );
    }
}

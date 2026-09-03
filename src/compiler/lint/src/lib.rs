#![forbid(unsafe_code)]
//! `ipe_lint` — extensible static analysis over valid Ipê source.
//!
//! The compiler enforces what *must* be true (soundness — it may only reject).
//! The linter enforces what *should* be true by convention on code that already
//! type-checks: idiom, consistency, and the "make invalid states
//! unrepresentable" discipline the language deliberately keeps out of its small,
//! refinement-free core. A rule is a pure function from a parsed module to a
//! list of [`Finding`]s; a finding optionally carries a semantics-preserving
//! [`Fix`] that `ipe lint --fix` applies, or a [`SigFix`] that `apply_sig_fixes`
//! resolves against the canonical call graph and applies cross-module. The same
//! rules flow three ways — `ipe lint` (CLI), the LSP (as diagnostics), and CI
//! (a non-zero exit on a surviving denied finding).
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
pub use finding::{Finding, Fix, Severity, SigFix};
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

/// A manual-review span emitted by `apply_sig_fixes` when a call site cannot be
/// automatically rewritten by the change-signature engine.
#[derive(Clone, Debug)]
pub struct ManualReviewReport {
    /// Module path of the file containing the call site.
    pub module: Vec<String>,
    /// The rule that flagged the definition.
    pub rule: &'static str,
    /// The unqualified name of the symbol whose caller needs manual attention.
    pub symbol_name: String,
    /// A human-readable reason the engine declined to rewrite this call site.
    pub reason: String,
}

/// The result of [`apply_sig_fixes`]: per-module rewritten sources, counts, and
/// any call sites the engine could not mechanically transform.
#[derive(Clone, Debug, Default)]
pub struct SigFixOutcome {
    /// Module path → rewritten source, for modules where at least one edit
    /// landed.
    pub rewritten: BTreeMap<Vec<String>, String>,
    /// The total number of call-site edits applied across all modules.
    pub applied: usize,
    /// Call sites the engine could not mechanically rewrite — reported to the
    /// user, never silently skipped.
    pub manual_reviews: Vec<ManualReviewReport>,
}

/// One sig-fix intent extracted from a lint finding, referencing data owned by
/// the [`LintReport`]. Kept at module scope so it can precede any statements in
/// [`apply_sig_fixes`].
struct Intent<'a> {
    rule: &'static str,
    symbol_name: &'a str,
    symbol_module_str: &'a [String],
    sig_fix: &'a crate::finding::SigFix,
}

/// A parse-and-canonicalised module, paired with the interner used to build it.
/// Kept at module scope so it can precede any statements in [`apply_sig_fixes`].
struct CanonModule {
    module_str: Vec<String>,
    canon: ipe_canon::ast::Module,
    interner: Interner,
}

/// Apply every [`SigFix`] a lint run over `modules` produces.
///
/// For each finding that carries a [`SigFix`], this function:
///
/// 1. Parses and canonicalises every module to obtain the resolved
///    call graph.
/// 2. Walks each canonical module's expression tree to collect all
///    `Call` nodes whose callee resolves to the flagged symbol.
/// 3. Calls [`ipe_canon::sig_delta::apply_sig_delta`] on each such
///    call node to compute the source edits.
/// 4. Applies the edits to the source text in descending byte order
///    (the same overlap-safe strategy as [`apply_module_fixes`]).
///
/// Call sites that the engine cannot mechanically rewrite are collected in
/// [`SigFixOutcome::manual_reviews`] and never silently applied (fail-closed).
///
/// Only findings from enabled (non-`Allow`), non-suppressed rules with a
/// `sig_fix` are processed. The outer single-module [`Fix`] path is not
/// re-run here; callers combine [`apply_fixes`] and [`apply_sig_fixes`] as
/// needed.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn apply_sig_fixes(modules: &[SourceModule], config: &LintConfig) -> SigFixOutcome {
    use ipe_canon::ast::Def;

    let report = run(modules, config);

    // Collect sig-fix intents: one entry per (rule, symbol_module, symbol_name, delta).
    // Multiple findings may target the same symbol (e.g. two adjacent-bools runs on
    // different exported functions); collect them all.
    let intents: Vec<Intent<'_>> = report
        .findings
        .iter()
        .filter_map(|f| {
            f.sig_fix.as_ref().map(|sf| Intent {
                rule: f.rule,
                symbol_name: &sf.symbol_name,
                symbol_module_str: &sf.symbol_module,
                sig_fix: sf,
            })
        })
        .collect();

    if intents.is_empty() {
        return SigFixOutcome::default();
    }

    // Build a source map for fast lookup.
    let source_by_module: BTreeMap<&[String], &str> = modules
        .iter()
        .map(|m| (m.module.as_slice(), m.source.as_str()))
        .collect();

    // Parse + canonicalize each module. Modules that fail to parse or
    // canonicalize are skipped (the compiler surfaces those errors; the linter
    // only reasons over valid code).
    let mut canon_modules: Vec<CanonModule> = modules
        .iter()
        .filter_map(|m| {
            let mut interner = Interner::new();
            let parsed = ipe_parse::parse_module(&m.source, &mut interner).ok()?;
            let canon = ipe_canon::canonicalise(&parsed, &mut interner).ok()?;
            Some(CanonModule {
                module_str: m.module.clone(),
                canon,
                interner,
            })
        })
        .collect();

    // For each intent, walk every canon module's expressions and collect
    // Call nodes whose callee is VarTopLevel pointing at the symbol.
    let mut edits_by_module: BTreeMap<Vec<String>, Vec<ipe_canon::rename::Edit>> = BTreeMap::new();
    let mut manual_reviews: Vec<ManualReviewReport> = Vec::new();

    for intent in &intents {
        for cm in &mut canon_modules {
            // Intern the target symbol's module path + name in THIS module's
            // interner so we can compare against VarTopLevel nodes.
            let target_module_syms: Vec<ipe_intern::Symbol> = intent
                .symbol_module_str
                .iter()
                .filter_map(|seg| cm.interner.intern(seg).ok())
                .collect();
            let Some(target_name_sym) = cm.interner.intern(intent.symbol_name).ok() else {
                continue;
            };

            // Skip if the interned path length doesn't match — the symbol
            // cannot resolve in this module.
            if target_module_syms.len() != intent.symbol_module_str.len() {
                continue;
            }

            let source = source_by_module
                .get(cm.module_str.as_slice())
                .copied()
                .unwrap_or("");

            // Build the containing-module path as symbols in the same interner.
            let file_syms: Vec<ipe_intern::Symbol> = cm
                .module_str
                .iter()
                .filter_map(|seg| cm.interner.intern(seg).ok())
                .collect();

            // Walk all defs in this module to find Call nodes targeting the
            // symbol.
            for def in &cm.canon.defs {
                let body = match def {
                    Def::Untyped { body, .. } | Def::Typed { body, .. } => body,
                };
                collect_call_edits(
                    body,
                    source,
                    &file_syms,
                    &target_module_syms,
                    target_name_sym,
                    &intent.sig_fix.delta,
                    intent.rule,
                    intent.symbol_name,
                    &cm.module_str,
                    &mut edits_by_module,
                    &mut manual_reviews,
                );
            }
        }
    }

    // Apply collected edits per module.
    let mut rewritten: BTreeMap<Vec<String>, String> = BTreeMap::new();
    let mut applied = 0usize;
    for (module_path, edits) in &edits_by_module {
        let source = match source_by_module.get(module_path.as_slice()) {
            Some(s) => *s,
            None => continue,
        };
        let lint_fixes: Vec<Fix> = edits
            .iter()
            .map(|e| Fix {
                describe: "call-site rewrite from sig-fix".to_owned(),
                span: e.span,
                replacement: e.replacement.clone(),
            })
            .collect();
        let (text, n) = apply_module_fixes(source, &lint_fixes);
        if n > 0 {
            rewritten.insert(module_path.clone(), text);
            applied += n;
        }
    }

    SigFixOutcome {
        rewritten,
        applied,
        manual_reviews,
    }
}

/// Walk `expr` recursively; for every `Call` node whose callee is
/// `VarTopLevel { module: target_module, name: target_name }`, apply `delta`
/// via [`apply_sig_delta`] and push the resulting edits (or a manual-review
/// report) into the output collections.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn collect_call_edits(
    expr: &ipe_canon::ast::Expr,
    source: &str,
    file_syms: &[ipe_intern::Symbol],
    target_module: &[ipe_intern::Symbol],
    target_name: ipe_intern::Symbol,
    delta: &ipe_canon::sig_delta::ShapeDelta,
    rule: &'static str,
    symbol_name: &str,
    module_str: &[String],
    edits_by_module: &mut BTreeMap<Vec<String>, Vec<ipe_canon::rename::Edit>>,
    manual_reviews: &mut Vec<ManualReviewReport>,
) {
    use ipe_canon::ast::Expr_;
    use ipe_canon::sig_delta::{ApplyOutcome, apply_sig_delta};

    match &expr.value {
        Expr_::Call(callee, args) => {
            // Check whether the callee resolves to our target symbol.
            let is_target = matches!(
                &callee.value,
                Expr_::VarTopLevel { module, name }
                    if module.as_slice() == target_module && *name == target_name
            );
            if is_target {
                match apply_sig_delta(source, file_syms, expr, delta) {
                    ApplyOutcome::Edits(edit_set) => {
                        edits_by_module
                            .entry(module_str.to_vec())
                            .or_default()
                            .extend(edit_set.edits);
                    }
                    ApplyOutcome::ManualReview(mr) => {
                        manual_reviews.push(ManualReviewReport {
                            module: module_str.to_vec(),
                            rule,
                            symbol_name: symbol_name.to_owned(),
                            reason: mr.reason,
                        });
                    }
                }
            }
            // Always recurse into callee and args — a call may appear inside
            // another call's argument position.
            collect_call_edits(
                callee,
                source,
                file_syms,
                target_module,
                target_name,
                delta,
                rule,
                symbol_name,
                module_str,
                edits_by_module,
                manual_reviews,
            );
            for arg in args {
                collect_call_edits(
                    arg,
                    source,
                    file_syms,
                    target_module,
                    target_name,
                    delta,
                    rule,
                    symbol_name,
                    module_str,
                    edits_by_module,
                    manual_reviews,
                );
            }
        }

        Expr_::VarTopLevel { .. }
        | Expr_::VarLocal(_)
        | Expr_::VarKernel { .. }
        | Expr_::VarCtor { .. }
        | Expr_::Int(_)
        | Expr_::Float(_)
        | Expr_::Str(_)
        | Expr_::Char(_)
        | Expr_::PathLit(_)
        | Expr_::CustomElementCtor(_)
        | Expr_::Unit => {}

        Expr_::ForeignCall { args, .. } => {
            for a in args {
                collect_call_edits(
                    a,
                    source,
                    file_syms,
                    target_module,
                    target_name,
                    delta,
                    rule,
                    symbol_name,
                    module_str,
                    edits_by_module,
                    manual_reviews,
                );
            }
        }

        Expr_::Lambda(_, body) => {
            collect_call_edits(
                body,
                source,
                file_syms,
                target_module,
                target_name,
                delta,
                rule,
                symbol_name,
                module_str,
                edits_by_module,
                manual_reviews,
            );
        }

        Expr_::Case(scrutinee, branches) => {
            collect_call_edits(
                scrutinee,
                source,
                file_syms,
                target_module,
                target_name,
                delta,
                rule,
                symbol_name,
                module_str,
                edits_by_module,
                manual_reviews,
            );
            for branch in branches {
                collect_call_edits(
                    &branch.body,
                    source,
                    file_syms,
                    target_module,
                    target_name,
                    delta,
                    rule,
                    symbol_name,
                    module_str,
                    edits_by_module,
                    manual_reviews,
                );
            }
        }

        Expr_::Let(bindings, body) => {
            for binding in bindings {
                collect_call_edits(
                    &binding.body,
                    source,
                    file_syms,
                    target_module,
                    target_name,
                    delta,
                    rule,
                    symbol_name,
                    module_str,
                    edits_by_module,
                    manual_reviews,
                );
            }
            collect_call_edits(
                body,
                source,
                file_syms,
                target_module,
                target_name,
                delta,
                rule,
                symbol_name,
                module_str,
                edits_by_module,
                manual_reviews,
            );
        }

        Expr_::If(branches, else_) => {
            for (cond, then_) in branches {
                collect_call_edits(
                    cond,
                    source,
                    file_syms,
                    target_module,
                    target_name,
                    delta,
                    rule,
                    symbol_name,
                    module_str,
                    edits_by_module,
                    manual_reviews,
                );
                collect_call_edits(
                    then_,
                    source,
                    file_syms,
                    target_module,
                    target_name,
                    delta,
                    rule,
                    symbol_name,
                    module_str,
                    edits_by_module,
                    manual_reviews,
                );
            }
            collect_call_edits(
                else_,
                source,
                file_syms,
                target_module,
                target_name,
                delta,
                rule,
                symbol_name,
                module_str,
                edits_by_module,
                manual_reviews,
            );
        }

        Expr_::Binop { lhs, rhs, .. } => {
            collect_call_edits(
                lhs,
                source,
                file_syms,
                target_module,
                target_name,
                delta,
                rule,
                symbol_name,
                module_str,
                edits_by_module,
                manual_reviews,
            );
            collect_call_edits(
                rhs,
                source,
                file_syms,
                target_module,
                target_name,
                delta,
                rule,
                symbol_name,
                module_str,
                edits_by_module,
                manual_reviews,
            );
        }

        Expr_::Tuple(elems) | Expr_::List(elems) => {
            for e in elems {
                collect_call_edits(
                    e,
                    source,
                    file_syms,
                    target_module,
                    target_name,
                    delta,
                    rule,
                    symbol_name,
                    module_str,
                    edits_by_module,
                    manual_reviews,
                );
            }
        }

        Expr_::Cons(h, t) => {
            collect_call_edits(
                h,
                source,
                file_syms,
                target_module,
                target_name,
                delta,
                rule,
                symbol_name,
                module_str,
                edits_by_module,
                manual_reviews,
            );
            collect_call_edits(
                t,
                source,
                file_syms,
                target_module,
                target_name,
                delta,
                rule,
                symbol_name,
                module_str,
                edits_by_module,
                manual_reviews,
            );
        }

        Expr_::Record(fields) => {
            for (_, v) in fields {
                collect_call_edits(
                    v,
                    source,
                    file_syms,
                    target_module,
                    target_name,
                    delta,
                    rule,
                    symbol_name,
                    module_str,
                    edits_by_module,
                    manual_reviews,
                );
            }
        }

        Expr_::Access(rec, _) => {
            collect_call_edits(
                rec,
                source,
                file_syms,
                target_module,
                target_name,
                delta,
                rule,
                symbol_name,
                module_str,
                edits_by_module,
                manual_reviews,
            );
        }

        Expr_::Update(base, fields) => {
            collect_call_edits(
                base,
                source,
                file_syms,
                target_module,
                target_name,
                delta,
                rule,
                symbol_name,
                module_str,
                edits_by_module,
                manual_reviews,
            );
            for (_, v) in fields {
                collect_call_edits(
                    v,
                    source,
                    file_syms,
                    target_module,
                    target_name,
                    delta,
                    rule,
                    symbol_name,
                    module_str,
                    edits_by_module,
                    manual_reviews,
                );
            }
        }
    }
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

    // ── sig-fix tests ─────────────────────────────────────────────────────────

    /// A `prim-param` finding carries a `SigFix` naming the symbol and a
    /// `WrapPrimitive` delta.
    #[test]
    fn prim_param_finding_carries_sig_fix() {
        // `listen` exposes a bare `Int` param named `port` → triggers prim-param.
        // (Using a dedicated function to avoid `url` hint firing first.)
        let src = "module Main exposing (listen)\n\
                   \n\
                   listen : Int -> String\n\
                   listen port =\n\
                   \x20   \"ok\"\n";
        let report = run(&[module(src)], &LintConfig::default());
        let finding = report
            .findings
            .iter()
            .find(|f| f.rule == "prim-param")
            .expect("expected a prim-param finding");
        assert!(
            finding.sig_fix.is_some(),
            "prim-param finding must carry a sig_fix, got none"
        );
        let sf = finding.sig_fix.as_ref().expect("just checked");
        assert_eq!(sf.symbol_name, "listen");
        assert_eq!(sf.symbol_module, vec!["Main"]);
        // `port` matches the `port` name-hint → WrapPrimitive with `fromInt`.
        assert!(
            matches!(
                &sf.delta,
                ipe_canon::sig_delta::ShapeDelta::WrapPrimitive { ctor_name, .. }
                    if ctor_name == "fromInt"
            ),
            "expected WrapPrimitive with fromInt ctor, got {:?}",
            sf.delta
        );
    }

    /// An `adjacent-bools` finding carries a `SigFix` with a
    /// `GroupAdjacentBoolsIntoRecord` delta.
    #[test]
    fn adjacent_bools_finding_carries_sig_fix() {
        let src = "module Main exposing (render)\n\
                   \n\
                   render : Bool -> Bool -> String\n\
                   render a b =\n\
                   \x20   \"x\"\n";
        let report = run(&[module(src)], &LintConfig::default());
        let finding = report
            .findings
            .iter()
            .find(|f| f.rule == "adjacent-bools")
            .expect("expected adjacent-bools finding");
        assert!(
            finding.sig_fix.is_some(),
            "adjacent-bools must carry a sig_fix"
        );
        let sf = finding.sig_fix.as_ref().expect("just checked");
        assert!(
            matches!(
                &sf.delta,
                ipe_canon::sig_delta::ShapeDelta::GroupAdjacentBoolsIntoRecord { .. }
            ),
            "expected GroupAdjacentBoolsIntoRecord delta"
        );
    }

    /// `apply_sig_fixes` rewrites a call site in the SAME module (single-module
    /// project). The function `render` is exported with two adjacent `Bool`
    /// params; a caller in the same module passes two bool literals; `--fix`
    /// groups them into a record.
    ///
    /// Same-module top-level calls resolve as `VarTopLevel` after
    /// canonicalisation, so the change-signature engine can rewrite them.
    #[test]
    fn apply_sig_fixes_same_module_rewrites_call_site() {
        let src = "module Main exposing (render)\n\
                   \n\
                   render : Bool -> Bool -> String\n\
                   render a b =\n\
                   \x20   \"x\"\n\
                   \n\
                   main =\n\
                   \x20   render True False\n";
        let outcome = apply_sig_fixes(&[module(src)], &LintConfig::default());
        // The same-module top-level call resolves as VarTopLevel — the engine
        // rewrites it.
        assert_eq!(
            outcome.applied, 1,
            "one call-site edit expected; got {}",
            outcome.applied
        );
        let rewritten = outcome
            .rewritten
            .get(&vec!["Main".to_owned()])
            .expect("Main was rewritten");
        assert!(
            rewritten.contains("{ a =") || rewritten.contains("{ b ="),
            "rewritten source must contain the record literal, got:\n{rewritten}"
        );
        assert!(
            outcome.manual_reviews.is_empty(),
            "no manual reviews expected for a literal-bool call"
        );
    }

    /// A `ManualReview` call site (lambda argument) in the SAME module is NOT
    /// applied; it appears in `manual_reviews` instead.
    ///
    /// Within a single module, top-level calls resolve as `VarTopLevel` after
    /// canonicalisation, so the change-signature engine sees the call. A lambda
    /// argument is structurally opaque and triggers the fail-closed path.
    #[test]
    fn apply_sig_fixes_lambda_arg_is_manual_review() {
        // `render` has two adjacent `Bool` params → adjacent-bools sig_fix.
        // The `main` binding calls `render` with a lambda as one arg → opaque.
        let src = "module Main exposing (render)\n\
                   \n\
                   render : Bool -> Bool -> String\n\
                   render a b =\n\
                   \x20   \"x\"\n\
                   \n\
                   main =\n\
                   \x20   render (\\x -> x) False\n";
        let outcome = apply_sig_fixes(&[module(src)], &LintConfig::default());
        // The lambda arg is opaque → ManualReview, not applied.
        assert_eq!(
            outcome.applied, 0,
            "lambda arg must not be applied; expected 0 edits, got {}",
            outcome.applied
        );
        assert!(
            !outcome.manual_reviews.is_empty(),
            "lambda arg must produce a manual-review report"
        );
        let mr = outcome
            .manual_reviews
            .first()
            .expect("just checked non-empty");
        assert_eq!(mr.rule, "adjacent-bools");
        assert!(
            mr.reason.contains("lambda") || mr.reason.contains("complex"),
            "manual-review reason must mention the opaque shape, got: {}",
            mr.reason
        );
    }
}

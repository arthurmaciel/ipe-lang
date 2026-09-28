//! Whole-file source actions: `organize_imports` and `fix_all`.
//!
//! Both compose the SSOT the rest of the crate already produces — the
//! `unused-imports` [`crate::Finding`] and the registered [`crate::Fix`]
//! values from [`crate::apply_fixes`] — and never re-derive usage or
//! fixability themselves. `organize_imports` reads the shape of surviving
//! `import` declarations straight off the parsed AST and renders it back
//! out sorted and merged; `fix_all` is a bounded repeat of the crate's own
//! single-round fix application, re-linting between rounds.

use std::collections::{BTreeMap, HashSet};

use ipe_diagnostics::Span;
use ipe_intern::{Interner, Symbol};
use ipe_syntax::{Exposed, Exposing, Import, Privacy as AstPrivacy};

use crate::rules::unused_imports::{import_clause_end, line_end, line_start};
use crate::{LintConfig, SourceModule, apply_fixes, run};

/// The round ceiling for [`fix_all`]: a fix can only unlock a further fix on
/// the *next* re-lint (e.g. `prefer-pipeline` flattens one nesting level per
/// round), so full convergence on deeply-shaped input can take several
/// rounds. This is the soundness floor against input shaped to defeat
/// convergence (arbitrarily deep nesting), not a tuning knob.
pub const FIX_ALL_MAX_ROUNDS: usize = 8;

/// Sort, merge, and prune the `import` block of one module.
///
/// An import flagged `unused-imports` — matched back to its AST node by the
/// finding's own identity key, the `import_kw` span — is dropped wholesale;
/// this is the only usage signal consulted, and it is never recomputed here.
/// Surviving imports of the same dotted module path and `as` alias are
/// merged into one declaration, their `exposing` lists unioned (privacy
/// only ever widens: `Public` beats `PublicCtors` beats `Private`, so a
/// merge never narrows what a duplicate had already exposed) and sorted by
/// name; the merged declarations are sorted by module path, then alias.
/// A name exposed only by a wholly-unused duplicate vanishes with it — this
/// is how an unused *exposed name* is pruned, without any per-name usage
/// analysis of its own. An empty merged `exposing` list is rendered as a
/// bare `import Foo` (never `exposing ()` — the parser's own reading of an
/// absent clause), which keeps a second run a no-op.
///
/// When the import block holds anything besides import lines and blank
/// lines (most commonly a comment) the rewrite is refused and the source is
/// returned unchanged: absent proof the region is safe to replace
/// wholesale, no edit is offered.
#[must_use]
pub fn organize_imports(module: &SourceModule, config: &LintConfig) -> String {
    let mut interner = Interner::new();
    let Ok(ast) = ipe_parse::parse_module(&module.source, &mut interner) else {
        return module.source.clone();
    };
    let Some(first) = ast.imports.first() else {
        return module.source.clone();
    };
    let Some(last) = ast.imports.last() else {
        return module.source.clone();
    };

    let report = run(std::slice::from_ref(module), config);
    let unused: HashSet<Span> = report
        .findings
        .iter()
        .filter(|f| f.rule == "unused-imports" && f.module == module.module)
        .map(|f| f.span)
        .collect();
    let kept: Vec<&Import> = ast
        .imports
        .iter()
        .filter(|imp| !unused.contains(&imp.import_kw))
        .collect();

    let block_lo = line_start(&module.source, first.import_kw.lo as usize);
    let block_hi = line_end(&module.source, import_clause_end(&module.source, last));

    let mut owned: Vec<(usize, usize)> = ast
        .imports
        .iter()
        .map(|imp| {
            (
                line_start(&module.source, imp.import_kw.lo as usize),
                line_end(&module.source, import_clause_end(&module.source, imp)),
            )
        })
        .collect();
    owned.sort_unstable();

    // Refuse unless the block is exactly import declarations plus blank
    // lines: every gap between (and around) them must be whitespace-only.
    let mut cursor = block_lo;
    for (lo, hi) in &owned {
        let gap = module.source.get(cursor..*lo).unwrap_or("");
        if *lo < cursor || !gap.chars().all(char::is_whitespace) {
            return module.source.clone();
        }
        cursor = *hi;
    }
    if cursor != block_hi {
        return module.source.clone();
    }

    let rendered = render_import_block(&kept, &interner);
    let mut out = String::new();
    out.push_str(module.source.get(..block_lo).unwrap_or(""));
    out.push_str(&rendered);
    out.push_str(module.source.get(block_hi..).unwrap_or(""));
    out
}

/// Apply every machine-applicable fix across `modules`, up to
/// [`FIX_ALL_MAX_ROUNDS`] re-lint rounds, and return `target`'s final text.
#[must_use]
pub fn fix_all(modules: &[SourceModule], target: &[String], config: &LintConfig) -> String {
    fix_all_bounded(modules, target, config, FIX_ALL_MAX_ROUNDS)
}

/// [`fix_all`] with an explicit round ceiling. Production always pins
/// [`FIX_ALL_MAX_ROUNDS`] via [`fix_all`]; a caller proving the bound itself
/// (a test) passes a small one directly, so the refusal is driven by the
/// same code path rather than a hand-verified convergence depth.
///
/// Each round is [`crate::apply_fixes`] itself — non-overlapping fixes from
/// a fresh lint pass — so an unfixable finding is, by construction, never
/// touched: it carries no [`crate::Fix`] for any round to apply.
#[must_use]
pub fn fix_all_bounded(
    modules: &[SourceModule],
    target: &[String],
    config: &LintConfig,
    max_rounds: usize,
) -> String {
    let mut current: Vec<SourceModule> = modules.to_vec();
    for _ in 0..max_rounds {
        let outcome = apply_fixes(&current, config);
        if outcome.applied == 0 {
            break;
        }
        for m in &mut current {
            if let Some(text) = outcome.rewritten.get(&m.module) {
                m.source.clone_from(text);
            }
        }
    }
    current
        .iter()
        .find(|m| m.module == target)
        .map_or_else(String::new, |m| m.source.clone())
}

/// A ctor-name set, or a value/opaque-type marker — the crate-owned
/// [`AstPrivacy`] widened rather than copied, so a merge only ever grows it.
#[derive(Clone, PartialEq, Eq)]
enum MergedPrivacy {
    Public,
    PrivateOpaque,
    Ctors(std::collections::BTreeSet<String>),
}

/// One merged `exposing` entry: a plain value name, or a type name with its
/// merged constructor privacy.
enum ExposedKind {
    Value,
    Type(MergedPrivacy),
}

/// A merged `exposing` clause: `exposing (..)` absorbs everything, else the
/// deduplicated, alphabetically-ordered (by the `BTreeMap` key) item set.
enum MergedExposing {
    All,
    List(BTreeMap<(String, bool), ExposedKind>),
}

fn resolve(interner: &Interner, sym: Symbol) -> String {
    interner.resolve(sym).unwrap_or_default().to_owned()
}

fn render_privacy(privacy: &AstPrivacy, interner: &Interner) -> MergedPrivacy {
    match privacy {
        AstPrivacy::Public => MergedPrivacy::Public,
        AstPrivacy::Private => MergedPrivacy::PrivateOpaque,
        AstPrivacy::PublicCtors(ctors) => {
            MergedPrivacy::Ctors(ctors.iter().map(|s| resolve(interner, *s)).collect())
        }
    }
}

fn widen_privacy(a: MergedPrivacy, b: MergedPrivacy) -> MergedPrivacy {
    match (a, b) {
        (MergedPrivacy::Public, _) | (_, MergedPrivacy::Public) => MergedPrivacy::Public,
        (MergedPrivacy::Ctors(mut x), MergedPrivacy::Ctors(y)) => {
            x.extend(y);
            MergedPrivacy::Ctors(x)
        }
        (MergedPrivacy::Ctors(x), MergedPrivacy::PrivateOpaque)
        | (MergedPrivacy::PrivateOpaque, MergedPrivacy::Ctors(x)) => MergedPrivacy::Ctors(x),
        (MergedPrivacy::PrivateOpaque, MergedPrivacy::PrivateOpaque) => {
            MergedPrivacy::PrivateOpaque
        }
    }
}

fn merge_exposing(entry: &mut MergedExposing, exposing: &Exposing, interner: &Interner) {
    if matches!(entry, MergedExposing::All) {
        return;
    }
    let Exposing::List(items) = exposing else {
        *entry = MergedExposing::All;
        return;
    };
    let MergedExposing::List(map) = entry else {
        return;
    };
    for item in items {
        match &item.value {
            Exposed::Value(sym) => {
                map.entry((resolve(interner, *sym), false))
                    .or_insert(ExposedKind::Value);
            }
            Exposed::Type(sym, privacy) => {
                let rendered = render_privacy(privacy, interner);
                match map.entry((resolve(interner, *sym), true)) {
                    std::collections::btree_map::Entry::Vacant(v) => {
                        v.insert(ExposedKind::Type(rendered));
                    }
                    std::collections::btree_map::Entry::Occupied(mut o) => {
                        if let ExposedKind::Type(existing) = o.get_mut() {
                            let taken = std::mem::replace(existing, MergedPrivacy::PrivateOpaque);
                            *existing = widen_privacy(taken, rendered);
                        }
                    }
                }
            }
        }
    }
}

fn render_exposed(key: &(String, bool), kind: &ExposedKind) -> String {
    let (name, _) = key;
    match kind {
        ExposedKind::Value => name.clone(),
        ExposedKind::Type(MergedPrivacy::Public) => format!("{name}(..)"),
        ExposedKind::Type(MergedPrivacy::PrivateOpaque) => name.clone(),
        ExposedKind::Type(MergedPrivacy::Ctors(ctors)) => {
            let names: Vec<&str> = ctors.iter().map(String::as_str).collect();
            format!("{name}({})", names.join(", "))
        }
    }
}

fn render_import_line(key: &(String, Option<String>), exposing: &MergedExposing) -> String {
    let (path, alias) = key;
    let mut line = format!("import {path}");
    if let Some(a) = alias {
        line.push_str(" as ");
        line.push_str(a);
    }
    match exposing {
        MergedExposing::All => line.push_str(" exposing (..)"),
        MergedExposing::List(map) if map.is_empty() => {}
        MergedExposing::List(map) => {
            let items: Vec<String> = map.iter().map(|(k, v)| render_exposed(k, v)).collect();
            line.push_str(" exposing (");
            line.push_str(&items.join(", "));
            line.push(')');
        }
    }
    line.push('\n');
    line
}

/// Render surviving imports as sorted, merged source text: one line per
/// distinct (module path, alias), grouped in the `BTreeMap`'s own order.
fn render_import_block(imports: &[&Import], interner: &Interner) -> String {
    let mut groups: BTreeMap<(String, Option<String>), MergedExposing> = BTreeMap::new();
    for imp in imports {
        let path = imp
            .name
            .value
            .iter()
            .map(|s| resolve(interner, *s))
            .collect::<Vec<_>>()
            .join(".");
        let alias = imp.alias.map(|s| resolve(interner, s));
        let entry = groups
            .entry((path, alias))
            .or_insert_with(|| MergedExposing::List(BTreeMap::new()));
        merge_exposing(entry, &imp.exposing.value, interner);
    }
    let mut out = String::new();
    for (key, exposing) in &groups {
        out.push_str(&render_import_line(key, exposing));
    }
    out
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

    fn target() -> Vec<String> {
        vec!["Main".to_owned()]
    }

    #[test]
    fn organize_imports_sorts_by_module_path() {
        let src = "module Main exposing (main)\n\nimport Zeta\nimport Alpha\n\nmain =\n    (Zeta.a, Alpha.b)\n";
        let out = organize_imports(&module(src), &LintConfig::default());
        let alpha_at = out.find("import Alpha").expect("Alpha import kept");
        let zeta_at = out.find("import Zeta").expect("Zeta import kept");
        assert!(alpha_at < zeta_at, "sorted output, got:\n{out}");
    }

    #[test]
    fn organize_imports_merges_duplicate_imports_and_sorts_exposing() {
        let src = "module Main exposing (main)\n\nimport Data exposing (b)\nimport Data exposing (a)\n\nmain =\n    (a, b)\n";
        let out = organize_imports(&module(src), &LintConfig::default());
        assert_eq!(
            out.matches("import Data").count(),
            1,
            "duplicate imports merge into one, got:\n{out}"
        );
        assert!(
            out.contains("import Data exposing (a, b)"),
            "exposing lists union and sort, got:\n{out}"
        );
    }

    #[test]
    fn organize_imports_removes_a_wholly_unused_import() {
        let src = "module Main exposing (main)\n\nimport Data exposing (a)\nimport Unused\n\nmain =\n    a\n";
        let out = organize_imports(&module(src), &LintConfig::default());
        assert!(
            !out.contains("Unused"),
            "unused import dropped, got:\n{out}"
        );
        assert!(
            out.contains("import Data exposing (a)"),
            "used import kept, got:\n{out}"
        );
    }

    #[test]
    fn organize_imports_keeps_as_alias() {
        let src = "module Main exposing (main)\n\nimport Data as D exposing (a)\n\nmain =\n    (D.x, a)\n";
        let out = organize_imports(&module(src), &LintConfig::default());
        assert!(
            out.contains("import Data as D exposing (a)"),
            "alias survives the rewrite, got:\n{out}"
        );
    }

    #[test]
    fn organize_imports_is_idempotent() {
        let src = "module Main exposing (main)\n\nimport Zeta\nimport Data exposing (b)\nimport Data exposing (a)\nimport Unused\n\nmain =\n    (Zeta.z, a, b)\n";
        let first = organize_imports(&module(src), &LintConfig::default());
        let second = organize_imports(&module(&first), &LintConfig::default());
        assert_eq!(first, second, "a second pass is a no-op");
    }

    #[test]
    fn organize_imports_keeps_an_import_used_only_in_a_type_annotation() {
        let src = "module Main exposing (main)\n\nimport Types exposing (Config)\n\nmain : Config -> Config\nmain x =\n    x\n";
        let out = organize_imports(&module(src), &LintConfig::default());
        assert!(
            out.contains("import Types exposing (Config)"),
            "a type-annotation-only reference counts as used, got:\n{out}"
        );
    }

    #[test]
    fn fix_all_applies_several_findings_in_one_file() {
        let src = "module Main exposing (main)\n\nimport Unused\n\nmain =\n    List.map fmt (List.filter live records)\n";
        let out = fix_all(&[module(src)], &target(), &LintConfig::default());
        assert!(!out.contains("Unused"), "unused import fixed, got:\n{out}");
        assert!(
            out.contains("|>"),
            "nested call flattened to a pipeline, got:\n{out}"
        );
    }

    #[test]
    fn fix_all_bounded_stops_at_the_round_bound() {
        // Four `List.map` levels deep: round 1 flattens the outer pair into a
        // `|>` chain, which leaves the inner pair nested as that chain's first
        // operand — a second round is needed to flatten it too.
        let src = "module Main exposing (main)\n\nmain =\n    List.map fmt (List.map g (List.map h (List.map i xs)))\n";
        let modules = [module(src)];

        let partial = fix_all_bounded(&modules, &target(), &LintConfig::default(), 1);
        let residual = run(&[module(&partial)], &LintConfig::default());
        assert!(
            residual
                .findings
                .iter()
                .any(|f| f.rule == "prefer-pipeline"),
            "one round only partially flattens a 4-deep nest, got:\n{partial}"
        );

        let full = fix_all_bounded(
            &modules,
            &target(),
            &LintConfig::default(),
            FIX_ALL_MAX_ROUNDS,
        );
        let converged = run(&[module(&full)], &LintConfig::default());
        assert!(
            !converged
                .findings
                .iter()
                .any(|f| f.rule == "prefer-pipeline"),
            "enough rounds fully converge, got:\n{full}"
        );
    }

    #[test]
    fn fix_all_leaves_an_unfixable_finding_unapplied() {
        let src = "module Main exposing (main)\n\nmain =\n    unsafeDoIt\n";
        let out = fix_all(&[module(src)], &target(), &LintConfig::default());
        assert_eq!(
            out, src,
            "unsafe-convention carries no fix, source is untouched"
        );
        let report = run(&[module(&out)], &LintConfig::default());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.rule == "unsafe-convention"),
            "the unfixable finding still stands, got {:?}",
            report.findings
        );
    }
}

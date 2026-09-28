//! Positive and refusal tests for the ported elm-review rules:
//! `simplify-double-not`, `simplify-map-identity`, `simplify-cons-append`,
//! `no-redundant-cons`, `no-redundant-concat`, and `no-missing-type-annotation`.
//!
//! Every fixture is asserted to parse first: the engine skips a module that
//! fails to parse, so an unparsed negative fixture would pass vacuously.

use ipe_intern::Interner;

use crate::{Finding, LintConfig, SourceModule, apply_fixes, read_lint_config, run};

fn module(body: &str) -> SourceModule {
    SourceModule {
        module: vec!["Main".to_owned()],
        source: format!("module Main exposing (main)\n\nmain =\n{body}\n"),
    }
}

fn parses(src: &str) -> bool {
    ipe_parse::parse_module(src, &mut Interner::new()).is_ok()
}

/// The findings of `rule` over `body`, after asserting the fixture parses.
/// Uses the registry default severity, matching the `style` tests —
/// correct for every rule here except `no-missing-type-annotation`, which ships
/// `Allow` by default and gets its own helper below.
fn findings(rule: &str, body: &str) -> Vec<Finding> {
    let m = module(body);
    assert!(parses(&m.source), "fixture must parse:\n{}", m.source);
    run(&[m], &LintConfig::default())
        .findings
        .into_iter()
        .filter(|f| f.rule == rule)
        .collect()
}

/// The fix replacements `rule` offers over `body`, one entry per finding
/// (`None` for a finding reported without a fix).
fn fixes_of(rule: &str, body: &str) -> Vec<Option<String>> {
    findings(rule, body)
        .into_iter()
        .map(|f| f.fix.map(|fix| fix.replacement))
        .collect()
}

/// True when `rule` fires exactly once over `body` with the fix `replacement`.
fn fixes_to(rule: &str, body: &str, replacement: &str) -> bool {
    fixes_of(rule, body) == vec![Some(replacement.to_owned())]
}

/// True when `rule` fires exactly once over `body`, reported without a fix.
fn fires_without_fix(rule: &str, body: &str) -> bool {
    fixes_of(rule, body) == vec![None]
}

/// The findings of `rule` over a whole-module `src`, under the registry
/// default severity.
fn findings_in(rule: &str, src: &str) -> Vec<Finding> {
    findings_with(&LintConfig::default(), rule, src)
}

/// The `Main` source after `ipe lint --fix`, and the number of fixes applied.
fn apply(source: String) -> (String, usize) {
    let m = SourceModule {
        module: vec!["Main".to_owned()],
        source,
    };
    assert!(parses(&m.source), "fixture must parse:\n{}", m.source);
    let outcome = apply_fixes(&[m], &LintConfig::default());
    let fixed = outcome
        .rewritten
        .get(&vec!["Main".to_owned()])
        .cloned()
        .unwrap_or_default();
    (fixed, outcome.applied)
}

/// True when `--fix` rewrites `body` to exactly `expected`, the result parses,
/// and a second `--fix` pass changes nothing.
fn fix_is_exact_and_idempotent(body: &str, expected: &str) -> bool {
    let (fixed, applied) = apply(module(body).source);
    if applied == 0 || fixed != module(expected).source {
        return false;
    }
    let (_, reapplied) = apply(fixed);
    reapplied == 0
}

fn help_of(rule: &str, body: &str) -> String {
    findings(rule, body)
        .iter()
        .map(|f| f.help.join("\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ── simplify-double-not ──────────────────────────────────────────────────────

const DOUBLE_NOT: &str = "simplify-double-not";

#[test]
fn double_not_fires() {
    assert!(help_of(DOUBLE_NOT, "    not (not ok)").contains("write `ok`"));
}

#[test]
fn single_not_is_fine() {
    assert!(findings(DOUBLE_NOT, "    not ok").is_empty());
}

#[test]
fn triple_not_reports_each_nested_pair() {
    // `not (not (not ok))` nests two overlapping `not (not _)` pairs — the
    // outer wrapping `not ok`, the inner wrapping `ok` — and each fires once.
    let hits = findings(DOUBLE_NOT, "    not (not (not ok))");
    assert_eq!(hits.len(), 2);
    let help: Vec<String> = hits.iter().map(|f| f.help.join("\n")).collect();
    assert!(help.iter().any(|h| h.contains("write `(not ok)`")));
    assert!(help.iter().any(|h| h.contains("write `ok`")));
}

#[test]
fn double_not_fix_replaces_with_the_operand() {
    assert!(fixes_to(DOUBLE_NOT, "    not (not ok)", "ok"));
}

#[test]
fn double_not_fix_keeps_an_operator_operand_grouped() {
    assert!(fixes_to(DOUBLE_NOT, "    not (not (a && b))", "(a && b)"));
}

#[test]
fn double_not_fix_applies_exactly() {
    assert!(fix_is_exact_and_idempotent(
        "    f (not (not ok)) y",
        "    f ok y"
    ));
}

#[test]
fn triple_not_fix_collapses_to_one_not() {
    // The two findings overlap; the engine applies the inner one and skips the
    // outer, leaving the single negation the triple denotes.
    assert!(fix_is_exact_and_idempotent(
        "    not (not (not ok))",
        "    not ok"
    ));
}

#[test]
fn double_not_with_a_comment_offers_no_fix() {
    assert!(fires_without_fix(DOUBLE_NOT, "    not ({- why -} not ok)"));
}

#[test]
fn top_level_not_shadow_refuses() {
    let src = "module Main exposing (main)\n\nnot x =\n    x\n\nmain =\n    not (not ok)\n";
    assert!(findings_in(DOUBLE_NOT, src).is_empty());
}

#[test]
fn lambda_param_not_shadow_refuses() {
    assert!(findings(DOUBLE_NOT, "    \\not -> not (not ok)").is_empty());
}

#[test]
fn let_bound_not_shadow_refuses() {
    assert!(
        findings(
            DOUBLE_NOT,
            "    let\n        not = f\n    in\n    not (not ok)"
        )
        .is_empty()
    );
}

#[test]
fn wildcard_import_could_shadow_not_and_refuses() {
    let src =
        "module Main exposing (main)\n\nimport Foo exposing (..)\n\nmain =\n    not (not ok)\n";
    assert!(findings_in(DOUBLE_NOT, src).is_empty());
}

#[test]
fn explicit_import_of_not_refuses() {
    let src =
        "module Main exposing (main)\n\nimport Foo exposing (not)\n\nmain =\n    not (not ok)\n";
    assert!(findings_in(DOUBLE_NOT, src).is_empty());
}

// ── simplify-map-identity ────────────────────────────────────────────────────

const MAP_IDENTITY: &str = "simplify-map-identity";

#[test]
fn map_identity_fires_fully_applied() {
    assert!(help_of(MAP_IDENTITY, "    List.map identity xs").contains("write `xs`"));
}

#[test]
fn map_identity_fires_partially_applied_without_a_fix() {
    // `List.map identity` is `List a -> List a`; `identity` is the wider
    // `a -> a`, so the rewrite is not exact and no fix is offered.
    assert!(fires_without_fix(MAP_IDENTITY, "    List.map identity"));
    assert!(
        help_of(MAP_IDENTITY, "    List.map identity")
            .contains("drop the `List.map identity` step")
    );
}

#[test]
fn map_with_a_real_function_is_fine() {
    assert!(findings(MAP_IDENTITY, "    List.map fmt xs").is_empty());
}

#[test]
fn map_identity_fix_replaces_with_the_list() {
    assert!(fixes_to(MAP_IDENTITY, "    List.map identity xs", "xs"));
}

#[test]
fn map_identity_fix_applies_inside_an_argument() {
    assert!(fix_is_exact_and_idempotent(
        "    f (List.map identity xs) y",
        "    f xs y"
    ));
}

#[test]
fn map_identity_fix_keeps_a_grouped_list_argument() {
    assert!(fixes_to(
        MAP_IDENTITY,
        "    List.map identity (g ys)",
        "(g ys)"
    ));
}

#[test]
fn map_identity_with_qualified_identity_fires() {
    assert!(fixes_to(
        MAP_IDENTITY,
        "    List.map Basics.identity xs",
        "xs"
    ));
}

#[test]
fn map_identity_through_a_stdlib_alias_fires() {
    let src =
        "module Main exposing (main)\n\nimport Ipe.List as L\n\nmain =\n    L.map identity xs\n";
    assert_eq!(findings_in(MAP_IDENTITY, src).len(), 1);
}

#[test]
fn map_identity_on_a_foreign_list_alias_refuses() {
    let src =
        "module Main exposing (main)\n\nimport Foo as List\n\nmain =\n    List.map identity xs\n";
    assert!(findings_in(MAP_IDENTITY, src).is_empty());
}

#[test]
fn map_identity_on_a_project_list_module_refuses() {
    let src =
        "module Main exposing (main)\n\nimport Utils.List\n\nmain =\n    List.map identity xs\n";
    assert!(findings_in(MAP_IDENTITY, src).is_empty());
}

#[test]
fn map_identity_with_shadowed_identity_refuses() {
    let src =
        "module Main exposing (main)\n\nidentity x =\n    x\n\nmain =\n    List.map identity xs\n";
    assert!(findings_in(MAP_IDENTITY, src).is_empty());
}

#[test]
fn map_identity_with_a_comment_offers_no_fix() {
    assert!(fires_without_fix(
        MAP_IDENTITY,
        "    List.map {- why -} identity xs"
    ));
}

// ── simplify-cons-append ─────────────────────────────────────────────────────

const CONS_APPEND: &str = "simplify-cons-append";

#[test]
fn single_element_append_fires() {
    assert!(help_of(CONS_APPEND, "    [ a ] ++ xs").contains("write `a :: xs`"));
}

#[test]
fn multi_element_append_is_fine() {
    assert!(findings(CONS_APPEND, "    [ a, b ] ++ xs").is_empty());
}

#[test]
fn appending_a_non_literal_is_fine() {
    assert!(findings(CONS_APPEND, "    ys ++ xs").is_empty());
}

#[test]
fn cons_append_fix_conses_the_element() {
    assert!(fixes_to(CONS_APPEND, "    [ a ] ++ xs", "a :: xs"));
}

#[test]
fn cons_append_fix_groups_an_operator_element() {
    // `x |> f :: xs` would re-associate as `x |> (f :: xs)`.
    assert!(fixes_to(
        CONS_APPEND,
        "    [ x |> f ] ++ xs",
        "(x |> f) :: xs"
    ));
}

#[test]
fn cons_append_fix_leaves_a_tight_call_element_bare() {
    assert!(fixes_to(CONS_APPEND, "    [ g x ] ++ xs", "g x :: xs"));
}

#[test]
fn cons_append_fix_keeps_the_match_grouping() {
    assert!(fix_is_exact_and_idempotent(
        "    f ([ a ] ++ xs) y",
        "    f (a :: xs) y"
    ));
}

#[test]
fn cons_append_fix_applies_exactly() {
    assert!(fix_is_exact_and_idempotent(
        "    [ a ] ++ xs",
        "    a :: xs"
    ));
}

#[test]
fn cons_append_in_a_longer_chain_is_fine() {
    assert!(findings(CONS_APPEND, "    [ a ] ++ ys ++ xs").is_empty());
}

#[test]
fn cons_append_with_a_comment_offers_no_fix() {
    assert!(fires_without_fix(CONS_APPEND, "    [ a ] {- why -} ++ xs"));
}

// ── no-redundant-cons ─────────────────────────────────────────────────────────

const REDUNDANT_CONS: &str = "no-redundant-cons";

#[test]
fn cons_onto_list_literal_fires() {
    assert!(help_of(REDUNDANT_CONS, "    x :: [ a, b ]").contains("write `[ x, a, b ]`"));
}

#[test]
fn cons_onto_empty_list_fires() {
    assert!(help_of(REDUNDANT_CONS, "    x :: []").contains("write `[ x ]`"));
}

#[test]
fn cons_onto_a_name_is_fine() {
    assert!(findings(REDUNDANT_CONS, "    x :: rest").is_empty());
}

#[test]
fn redundant_cons_fix_writes_the_longer_literal() {
    assert!(fixes_to(REDUNDANT_CONS, "    x :: [ a, b ]", "[ x, a, b ]"));
}

#[test]
fn redundant_cons_fix_onto_empty_list() {
    assert!(fixes_to(REDUNDANT_CONS, "    x :: []", "[ x ]"));
}

#[test]
fn redundant_cons_fix_applies_exactly() {
    assert!(fix_is_exact_and_idempotent(
        "    f (g x :: [ a ]) y",
        "    f [ g x, a ] y"
    ));
}

#[test]
fn redundant_cons_in_a_longer_chain_is_fine() {
    assert!(findings(REDUNDANT_CONS, "    x :: [ a ] ++ ys").is_empty());
}

#[test]
fn redundant_cons_onto_a_grouped_list_offers_no_fix() {
    assert!(fires_without_fix(REDUNDANT_CONS, "    x :: ([ a ])"));
}

#[test]
fn redundant_cons_with_a_comment_offers_no_fix() {
    assert!(fires_without_fix(
        REDUNDANT_CONS,
        "    x {- why -} :: [ a ]"
    ));
}

// ── no-redundant-concat ───────────────────────────────────────────────────────

const REDUNDANT_CONCAT: &str = "no-redundant-concat";

#[test]
fn list_concat_of_one_fires() {
    assert!(help_of(REDUNDANT_CONCAT, "    List.concat [ xs ]").contains("write `xs`"));
}

#[test]
fn string_concat_of_one_fires() {
    assert!(help_of(REDUNDANT_CONCAT, "    String.concat [ s ]").contains("write `s`"));
}

#[test]
fn concat_of_several_is_fine() {
    assert!(findings(REDUNDANT_CONCAT, "    List.concat [ xs, ys ]").is_empty());
}

#[test]
fn concat_fix_replaces_with_the_element() {
    assert!(fixes_to(REDUNDANT_CONCAT, "    List.concat [ xs ]", "xs"));
}

#[test]
fn concat_fix_leaves_a_tight_call_bare_where_a_call_stood() {
    assert!(fixes_to(REDUNDANT_CONCAT, "    List.concat [ f x ]", "f x"));
}

#[test]
fn concat_fix_groups_an_operator_element() {
    assert!(fixes_to(
        REDUNDANT_CONCAT,
        "    List.concat [ a ++ b ]",
        "(a ++ b)"
    ));
}

#[test]
fn concat_fix_regroups_a_call_in_argument_position() {
    assert!(fix_is_exact_and_idempotent(
        "    g (List.concat [ f x ]) y",
        "    g (f x) y"
    ));
}

#[test]
fn concat_on_a_foreign_alias_refuses() {
    let src =
        "module Main exposing (main)\n\nimport Foo as String\n\nmain =\n    String.concat [ s ]\n";
    assert!(findings_in(REDUNDANT_CONCAT, src).is_empty());
}

#[test]
fn concat_on_a_project_list_module_refuses() {
    let src =
        "module Main exposing (main)\n\nimport Utils.List\n\nmain =\n    List.concat [ xs ]\n";
    assert!(findings_in(REDUNDANT_CONCAT, src).is_empty());
}

#[test]
fn concat_with_a_comment_offers_no_fix() {
    assert!(fires_without_fix(
        REDUNDANT_CONCAT,
        "    List.concat [ {- why -} xs ]"
    ));
}

// ── no-missing-type-annotation ────────────────────────────────────────────────
//
// `Allow` by default, so these tests build a `LintConfig` through the real
// `lint.ipe` reader (`Lint.warn "no-missing-type-annotation"`) rather than the
// registry default — exercising the same opt-in path a project would use.

const MISSING_ANNOTATION: &str = "no-missing-type-annotation";

fn warn_on_missing_annotation() -> LintConfig {
    let src = "module Lint exposing (lint)\n\nlint =\n    Lint.config\n        |> Lint.warn \"no-missing-type-annotation\"\n";
    read_lint_config(src, "lint.ipe").expect("valid lint.ipe fixture")
}

fn findings_with(config: &LintConfig, rule: &str, src: &str) -> Vec<Finding> {
    assert!(parses(src), "fixture must parse:\n{src}");
    let m = SourceModule {
        module: vec!["Main".to_owned()],
        source: src.to_owned(),
    };
    run(&[m], config)
        .findings
        .into_iter()
        .filter(|f| f.rule == rule)
        .collect()
}

#[test]
fn unannotated_top_level_value_fires_once_opted_in() {
    let src = "module Main exposing (main)\n\nmain =\n    1\n";
    let hits = findings_with(&warn_on_missing_annotation(), MISSING_ANNOTATION, src);
    assert_eq!(hits.len(), 1);
    assert!(
        hits.first()
            .expect("exactly one finding")
            .help
            .join("\n")
            .contains("add a `main : T` signature")
    );
}

#[test]
fn annotated_top_level_value_is_fine() {
    let src = "module Main exposing (main)\n\nmain : Int\nmain =\n    1\n";
    let hits = findings_with(&warn_on_missing_annotation(), MISSING_ANNOTATION, src);
    assert!(hits.is_empty());
}

#[test]
fn rule_is_allow_by_default() {
    let src = "module Main exposing (main)\n\nmain =\n    1\n";
    let hits = findings_with(&LintConfig::default(), MISSING_ANNOTATION, src);
    assert!(hits.is_empty());
}

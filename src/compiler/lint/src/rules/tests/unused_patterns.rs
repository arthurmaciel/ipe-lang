//! Positive and refusal tests for `no-unused-parameters` and
//! `no-unused-patterns`.
//!
//! Every fixture is asserted to parse first: the engine skips a module that
//! fails to parse, so an unparsed negative fixture would pass vacuously.

use ipe_intern::Interner;

use crate::{Finding, LintConfig, SourceModule, apply_fixes, run};

const PARAMS: &str = "no-unused-parameters";
const PATTERNS: &str = "no-unused-patterns";

fn main_module(source: String) -> SourceModule {
    SourceModule {
        module: vec!["Main".to_owned()],
        source,
    }
}

/// A module whose `main` is `body`.
fn with_main(body: &str) -> String {
    format!("module Main exposing (main)\n\nmain =\n{body}\n")
}

fn parses(src: &str) -> bool {
    ipe_parse::parse_module(src, &mut Interner::new()).is_ok()
}

/// The findings of `rule` over the whole-module `src`.
fn findings_in(rule: &str, src: &str) -> Vec<Finding> {
    assert!(parses(src), "fixture must parse:\n{src}");
    run(&[main_module(src.to_owned())], &LintConfig::default())
        .findings
        .into_iter()
        .filter(|f| f.rule == rule)
        .collect()
}

/// The fix replacements `rule` offers over `src`, one entry per finding
/// (`None` for a finding reported without a fix).
fn fixes_in(rule: &str, src: &str) -> Vec<Option<String>> {
    findings_in(rule, src)
        .into_iter()
        .map(|f| f.fix.map(|fix| fix.replacement))
        .collect()
}

/// True when `rule` fires exactly once over `main`'s `body` with `replacement`.
fn fixes_to(rule: &str, body: &str, replacement: &str) -> bool {
    fixes_in(rule, &with_main(body)) == vec![Some(replacement.to_owned())]
}

/// True when `rule` fires exactly once over `main`'s `body` without a fix.
fn fires_without_fix(rule: &str, body: &str) -> bool {
    fixes_in(rule, &with_main(body)) == vec![None]
}

/// True when neither rule fires over `main`'s `body`.
fn is_clean(body: &str) -> bool {
    let src = with_main(body);
    findings_in(PARAMS, &src).is_empty() && findings_in(PATTERNS, &src).is_empty()
}

/// The source after `ipe lint --fix`, and the number of fixes applied.
fn apply(source: String) -> (String, usize) {
    assert!(parses(&source), "fixture must parse:\n{source}");
    let outcome = apply_fixes(&[main_module(source)], &LintConfig::default());
    let fixed = outcome
        .rewritten
        .get(&vec!["Main".to_owned()])
        .cloned()
        .unwrap_or_default();
    (fixed, outcome.applied)
}

/// True when `--fix` rewrites `src` to exactly `expected`, the result parses,
/// and a second `--fix` pass changes nothing.
fn fix_is_exact_and_idempotent(src: &str, expected: &str) -> bool {
    let (fixed, applied) = apply(src.to_owned());
    if applied == 0 || fixed != expected {
        return false;
    }
    let (_, reapplied) = apply(fixed);
    reapplied == 0
}

// ── no-unused-parameters ─────────────────────────────────────────────────────

#[test]
fn top_level_parameter_is_renamed_with_an_underscore() {
    assert!(fix_is_exact_and_idempotent(
        "module Main exposing (f)\n\nf x y =\n    y\n",
        "module Main exposing (f)\n\nf _x y =\n    y\n",
    ));
}

#[test]
fn lambda_parameter_fires_under_parameters_not_patterns() {
    assert!(fixes_to(PARAMS, "    \\x -> 0", "_x"));
    assert!(findings_in(PATTERNS, &with_main("    \\x -> 0")).is_empty());
}

#[test]
fn a_read_parameter_is_clean() {
    assert!(is_clean("    \\x -> x + 1"));
}

#[test]
fn an_underscore_prefixed_parameter_is_intentional() {
    assert!(is_clean("    \\_x -> 0"));
    assert!(is_clean("    \\_ -> 0"));
}

#[test]
fn a_record_update_base_counts_as_a_read() {
    assert!(is_clean("    \\r -> { r | count = 1 }"));
}

#[test]
fn an_interpolated_name_counts_as_a_read() {
    assert!(is_clean("    \\name -> \"\"\"hi {{name}}\"\"\""));
}

#[test]
fn a_shadowed_read_hides_the_outer_binder_rather_than_inventing_one() {
    assert!(is_clean("    \\x -> \\x -> x"));
}

#[test]
fn a_taken_underscore_name_falls_back_to_a_wildcard() {
    let read = "module Main exposing (f)\n\nf x _x =\n    _x\n";
    assert!(fixes_in(PARAMS, read) == vec![Some("_".to_owned())]);
    let sibling = "module Main exposing (f)\n\nf x _x =\n    0\n";
    assert!(fixes_in(PARAMS, sibling) == vec![Some("_".to_owned())]);
}

#[test]
fn the_rename_keeps_the_name_for_prim_param() {
    let src = "module Main exposing (connect)\n\nconnect : String -> Int -> String\nconnect host port =\n    host\n";
    let (fixed, applied) = apply(src.to_owned());
    assert!(applied >= 1, "the unused `port` must be fixed");
    assert!(fixed.contains("connect host _port ="), "{fixed}");
    assert!(
        !findings_in("prim-param", &fixed).is_empty(),
        "prim-param must still read `_port` as a port:\n{fixed}"
    );
}

#[test]
fn a_suppression_comment_silences_the_site() {
    let body = "    -- ipe-lint: allow no-unused-parameters\n    \\x -> 0";
    assert!(findings_in(PARAMS, &with_main(body)).is_empty());
}

// ── no-unused-patterns ───────────────────────────────────────────────────────

#[test]
fn case_arm_variable_is_renamed() {
    let body =
        "    case m of\n        Just x ->\n            0\n\n        Nothing ->\n            1";
    assert!(fixes_to(PATTERNS, body, "_x"));
    let expected =
        "    case m of\n        Just _x ->\n            0\n\n        Nothing ->\n            1";
    assert!(fix_is_exact_and_idempotent(
        &with_main(body),
        &with_main(expected)
    ));
}

#[test]
fn case_arm_variable_read_in_its_arm_is_clean() {
    assert!(is_clean(
        "    case m of\n        Just x ->\n            x\n\n        Nothing ->\n            1"
    ));
}

#[test]
fn let_destructure_flags_only_the_unread_part() {
    let body = "    let\n        ( a, b ) =\n            p\n    in\n    a";
    assert!(fixes_to(PATTERNS, body, "_b"));
}

#[test]
fn a_plain_let_binder_is_left_to_unused_bindings() {
    assert!(is_clean(
        "    let\n        x =\n            1\n    in\n    0"
    ));
}

#[test]
fn do_bind_fires_under_patterns_not_parameters() {
    let body = "    do\n        x <- Task.succeed 1\n        Task.succeed 2";
    assert!(fixes_to(PATTERNS, body, "_x"));
    assert!(findings_in(PARAMS, &with_main(body)).is_empty());
}

#[test]
fn a_do_bind_read_later_is_clean() {
    assert!(is_clean(
        "    do\n        x <- Task.succeed 1\n        Task.succeed x"
    ));
}

#[test]
fn an_unused_alias_is_dropped() {
    let body = "    case m of\n        Just y as whole ->\n            y\n\n        Nothing ->\n            0";
    let expected =
        "    case m of\n        Just y ->\n            y\n\n        Nothing ->\n            0";
    assert!(fix_is_exact_and_idempotent(
        &with_main(body),
        &with_main(expected)
    ));
}

#[test]
fn a_read_alias_is_clean() {
    assert!(is_clean(
        "    case m of\n        Just y as whole ->\n            ( y, whole )\n\n        Nothing ->\n            ( 0, m )"
    ));
}

#[test]
fn an_or_pattern_variable_fires_once_without_a_fix() {
    let body = "    case m of\n        A x | B x ->\n            0\n\n        C ->\n            1";
    assert!(fires_without_fix(PATTERNS, body));
}

#[test]
fn an_unused_record_field_fires_without_a_fix() {
    let body = "    case r of\n        { a, b } ->\n            a";
    assert!(fires_without_fix(PATTERNS, body));
}

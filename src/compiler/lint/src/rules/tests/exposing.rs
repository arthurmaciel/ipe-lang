//! Positive and refusal tests for `no-exposing-everything` and
//! `no-importing-everything`.
//!
//! Every fixture is asserted to parse first: the engine skips a module that
//! fails to parse, so an unparsed negative fixture would pass vacuously.

use ipe_intern::Interner;

use crate::{Finding, LintConfig, SourceModule, run};

const EXPOSING: &str = "no-exposing-everything";
const IMPORTING: &str = "no-importing-everything";

/// The findings of `rule` over the whole-module `src`, under the registry
/// default severity; `None` when the fixture does not parse.
fn findings(rule: &str, src: &str) -> Option<Vec<Finding>> {
    ipe_parse::parse_module(src, &mut Interner::new()).ok()?;
    let m = SourceModule {
        module: vec!["Main".to_owned()],
        source: src.to_owned(),
    };
    Some(
        run(&[m], &LintConfig::default())
            .findings
            .into_iter()
            .filter(|f| f.rule == rule)
            .collect(),
    )
}

/// The source text each finding of `rule` over `src` points at.
fn flagged(rule: &str, src: &str) -> Option<Vec<String>> {
    let found = findings(rule, src)?;
    found
        .iter()
        .map(|f| {
            src.get(f.span.lo as usize..f.span.hi as usize)
                .map(str::to_owned)
        })
        .collect()
}

/// True when `rule` parses `src` and reports nothing.
fn is_clean(rule: &str, src: &str) -> bool {
    findings(rule, src).is_some_and(|found| found.is_empty())
}

/// True when `rule` parses `src` and reports exactly one finding, with no fix.
fn fires_once_without_fix(rule: &str, src: &str) -> bool {
    findings(rule, src).is_some_and(|found| matches!(found.as_slice(), [f] if f.fix.is_none()))
}

// ── no-exposing-everything ───────────────────────────────────────────────────

#[test]
fn open_module_header_fires_on_the_header() {
    let src = "module Main exposing (..)\n\nmain =\n    1\n";
    assert_eq!(flagged(EXPOSING, src), Some(vec!["module Main".to_owned()]));
}

#[test]
fn open_module_header_offers_no_fix() {
    let src = "module Main exposing (..)\n\nmain =\n    1\n";
    assert!(fires_once_without_fix(EXPOSING, src));
}

#[test]
fn explicit_module_header_is_fine() {
    assert!(is_clean(
        EXPOSING,
        "module Main exposing (main)\n\nmain =\n    1\n"
    ));
}

#[test]
fn open_module_header_can_be_suppressed() {
    let src =
        "-- ipe-lint: allow no-exposing-everything\nmodule Main exposing (..)\n\nmain =\n    1\n";
    assert!(is_clean(EXPOSING, src));
}

// ── no-importing-everything ──────────────────────────────────────────────────

#[test]
fn open_import_fires_on_the_module_name() {
    let src = "module Main exposing (main)\n\nimport Ipe.Html exposing (..)\n\nmain =\n    1\n";
    assert_eq!(flagged(IMPORTING, src), Some(vec!["Ipe.Html".to_owned()]));
}

#[test]
fn each_open_import_fires() {
    let src = "module Main exposing (main)\n\nimport Ipe.Html exposing (..)\nimport Foo exposing (..)\nimport Bar exposing (bar)\n\nmain =\n    1\n";
    assert_eq!(
        flagged(IMPORTING, src),
        Some(vec!["Ipe.Html".to_owned(), "Foo".to_owned()])
    );
}

#[test]
fn open_import_offers_no_fix() {
    let src = "module Main exposing (main)\n\nimport Foo exposing (..)\n\nmain =\n    1\n";
    assert!(fires_once_without_fix(IMPORTING, src));
}

#[test]
fn explicit_and_bare_imports_are_fine() {
    let src = "module Main exposing (main)\n\nimport Foo exposing (foo, Bar(..))\nimport Baz\nimport Qux as Q\n\nmain =\n    1\n";
    assert!(is_clean(IMPORTING, src));
}

#[test]
fn manifest_dsl_import_is_exempt() {
    let src = "module Package exposing (package)\n\nimport Ipe.Package exposing (..)\n\npackage =\n    1\n";
    assert!(is_clean(IMPORTING, src));
}

#[test]
fn exemption_is_the_stdlib_manifest_module_only() {
    // A project `My.Package` or a deeper `Ipe.Package.Extra` is an ordinary
    // import, not the manifest DSL.
    let src = "module Main exposing (main)\n\nimport My.Package exposing (..)\nimport Ipe.Package.Extra exposing (..)\n\nmain =\n    1\n";
    assert_eq!(
        flagged(IMPORTING, src),
        Some(vec![
            "My.Package".to_owned(),
            "Ipe.Package.Extra".to_owned()
        ])
    );
}

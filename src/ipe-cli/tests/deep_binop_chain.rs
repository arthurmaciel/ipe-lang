//! A flat operator chain re-associates into a nesting-deep canonical tree that
//! recursive downstream walkers (and the AST's own `Drop`) traverse, so an
//! unbounded chain is a stack-overflow / denial-of-service surface on accepted
//! input. The parser must count each operator toward its nesting limit and
//! reject an over-long chain with a diagnostic rather than build the tree.

use ipe_intern::Interner;

/// A chain far longer than the parser's nesting limit must be REJECTED at parse
/// time (fail-closed), not accepted into a tree that overflows a later walker.
#[test]
fn over_deep_binop_chain_is_rejected() {
    // Well beyond MAX_DEPTH (256): a chain this long previously parsed into a
    // multi-thousand-deep Binop tree.
    const N: usize = 50_000;

    let mut src = String::with_capacity(N * 10 + 300);
    src.push_str("module Main exposing (main)\n\nmain =\n    (");
    src.push_str(r#""a""#);
    for _ in 0..N {
        src.push_str(r#" ++ "a""#);
    }
    src.push_str(")\n");

    let mut interner = Interner::new();
    let parsed = ipe_parse::parse_module(&src, &mut interner);

    // The over-deep chain must be refused; a bounded tree never reaches canon.
    assert!(
        parsed.is_err(),
        "over-deep operator chain must be rejected at parse"
    );
}

/// A short chain (well within the limit) still parses and canon-compiles — the
/// bound must not regress ordinary expressions.
#[test]
fn short_binop_chain_still_compiles() {
    let mut src = String::from("module Main exposing (main)\n\nmain =\n    (");
    src.push_str(r#""a""#);
    for _ in 0..8 {
        src.push_str(r#" ++ "a""#);
    }
    src.push_str(")\n");

    let mut interner = Interner::new();
    let parsed = ipe_parse::parse_module(&src, &mut interner);
    assert!(parsed.is_ok(), "short operator chain must parse");
    if let Ok(parsed) = parsed {
        assert!(
            ipe_canon::canonicalise(&parsed, &mut interner).is_ok(),
            "short operator chain must canon-compile"
        );
    }
}

//! Refusal and shape tests for parser front-end corrections.
//!
//! Each pins a path that must be rejected (a dotted binder, a dotted lowercase
//! type, an unindented `do` block) or a legal shape that must now parse (field
//! access on a qualified value). A rejection no test drives is one edit from
//! silently vanishing.

use ipe_intern::Interner;
use ipe_parse::parse_module;
use ipe_syntax::{Expr_, Pattern_};

/// Wrap `body` as the single `main` binding of a minimal module.
fn module_with_main(body: &str) -> String {
    format!("module Main exposing (main)\n\nmain =\n    {body}\n")
}

/// A dotted lowercase identifier in binder position (`\a.b -> a.b`) is not a
/// value binder and must be rejected rather than minted as a `PVar` with a
/// dotted name.
#[test]
fn dotted_lowercase_lambda_param_is_rejected() {
    let mut i = Interner::new();
    let src = module_with_main("\\a.b -> a.b");
    assert!(parse_module(&src, &mut i).is_err());
}

/// A dotted lowercase top-level parameter is likewise rejected.
#[test]
fn dotted_lowercase_top_level_param_is_rejected() {
    let mut i = Interner::new();
    let src = "module Main exposing (f)\n\nf a.b = a.b\n";
    assert!(parse_module(src, &mut i).is_err());
}

/// A dotted lowercase-head identifier in type position (`f : a.b -> a.b`) is
/// not a type variable; it is rejected instead of silently generalising.
#[test]
fn dotted_lowercase_type_is_rejected() {
    let mut i = Interner::new();
    let src = "module Main exposing (f)\n\nf : a.b -> a.b\nf x = x\n";
    assert!(parse_module(src, &mut i).is_err());
}

/// An unindented `do` block whose statements sit at or before the enclosing
/// layout threshold must be rejected, not silently swallow the following
/// top-level declaration.
#[test]
fn unindented_do_block_is_rejected() {
    let mut i = Interner::new();
    let src = "module Main exposing (main)\n\nmain = do\nx = 1\n\nother = 2\n";
    assert!(parse_module(src, &mut i).is_err());
}

/// Field access on a qualified value parses as a `VarQual` followed by an
/// `Access` chain, not a `VarQual` with a dotted qualifier.
#[test]
fn field_access_on_qualified_value_is_access_chain() {
    let mut i = Interner::new();
    let src = module_with_main("Http.defaultConfig.timeout");
    let m = parse_module(&src, &mut i).expect("qualified field access must parse");
    let body = &m.values.first().expect("main binding").value.body.value;
    let ok = matches!(body, Expr_::Access(inner, field)
        if i.resolve(field.value) == Some("timeout")
            && matches!(&inner.value, Expr_::VarQual(q, name)
                if i.resolve(*q) == Some("Http") && i.resolve(*name) == Some("defaultConfig")));
    assert!(
        ok,
        "expected Access(VarQual(Http, defaultConfig), timeout), got {body:?}"
    );
}

/// A plain qualified constructor reference (`Maybe.Nothing`, all-uppercase run)
/// still parses as a bare `VarQual` with no accessor tail.
#[test]
fn all_uppercase_qualified_name_stays_var_qual() {
    let mut i = Interner::new();
    let src = module_with_main("Result.Ok");
    let m = parse_module(&src, &mut i).expect("qualified ctor ref must parse");
    let body = &m.values.first().expect("main binding").value.body.value;
    assert!(
        matches!(body, Expr_::VarQual(q, name)
            if i.resolve(*q) == Some("Result") && i.resolve(*name) == Some("Ok")),
        "expected VarQual(Result, Ok), got {body:?}"
    );
}

/// A parenthesised pattern's span covers the whole `( … )` range, not just the
/// opening paren.
#[test]
fn parenthesised_pattern_span_covers_full_range() {
    let mut i = Interner::new();
    let src = "module Main exposing (f)\n\nf (x) = x\n";
    let m = parse_module(src, &mut i).expect("parenthesised param must parse");
    let value = &m.values.first().expect("f binding").value;
    let pat = value.patterns.first().expect("one parameter");
    assert!(
        matches!(pat.value, Pattern_::PVar(_)),
        "grouped pattern unwraps to its inner PVar"
    );
    // The span must be wider than a single byte (the lone `(` bug produced a
    // 1-byte span); `(x)` spans three bytes.
    assert!(
        pat.span.hi - pat.span.lo >= 3,
        "span must cover `(x)`, got {}..{}",
        pat.span.lo,
        pat.span.hi
    );
}

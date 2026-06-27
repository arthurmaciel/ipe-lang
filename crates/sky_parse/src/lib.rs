#![forbid(unsafe_code)]
//! `sky_parse` — the lexer + layout + recursive-descent parser for the
//! Milestone-0 subset of Sky.
//!
//! Entry point: [`parse_module`]. It consumes source text plus a mutable
//! [`Interner`] and produces a [`sky_syntax::Module`], or a typed
//! [`sky_diagnostics::Diagnostic`]. The parser is a hand-written recursive
//! descent port of the Haskell compiler's `Sky.Parse.*`, narrowed to the
//! nodes the M0 golden program exercises. Recursion is bounded by
//! [`parser::MAX_DEPTH`] so adversarial input cannot overflow the stack.

mod layout;
mod lexer;
mod parser;

use sky_diagnostics::DResult;
use sky_intern::Interner;
use sky_syntax::Module;

pub use parser::MAX_DEPTH;

/// Parse a complete module from source text.
///
/// # Errors
/// Returns a [`sky_diagnostics::Diagnostic`] if the source cannot be lexed or
/// does not form a valid M0 module, or [`sky_diagnostics::ParseError::TooDeep`]
/// if the recursion-depth guard trips.
pub fn parse_module(src: &str, interner: &mut Interner) -> DResult<Module> {
    let toks = lexer::lex(src)?;
    let mut p = parser::Parser::new(toks, interner);
    p.parse_module()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sky_syntax::{Exposed, Exposing, Expr_, Pattern_, TypeAnnotation, Value};

    const GOLDEN: &str = include_str!("../../../tests/golden/m0/Main.sky");

    fn find_value<'a>(m: &'a Module, i: &Interner, name: &str) -> Option<&'a Value> {
        m.values
            .iter()
            .map(|v| &v.value)
            .find(|v| i.resolve(v.name.value) == Some(name))
    }

    #[test]
    fn parses_golden_module_structure() {
        let mut i = Interner::new();
        let result = parse_module(GOLDEN, &mut i);
        assert!(result.is_ok(), "golden must parse: {result:?}");
        let Ok(m) = result else { return };

        // Module header: name `Main`, exposing (main).
        assert_eq!(m.name.value.len(), 1);
        assert_eq!(
            m.name.value.first().and_then(|&s| i.resolve(s)),
            Some("Main")
        );
        assert!(matches!(
            &m.exposing.value,
            Exposing::List(items)
                if items.len() == 1
                    && items.first().is_some_and(|e| matches!(e.value, Exposed::Value(_)))
        ));

        // One import: Sky.Core.Prelude exposing (..).
        assert_eq!(m.imports.len(), 1);
        if let Some(imp) = m.imports.first() {
            let segs: Vec<&str> = imp
                .name
                .value
                .iter()
                .filter_map(|&s| i.resolve(s))
                .collect();
            assert_eq!(segs, ["Sky", "Core", "Prelude"]);
            assert!(imp.alias.is_none());
            assert!(matches!(imp.exposing.value, Exposing::All));
        }

        // One union: Msg { Increment, Decrement }.
        assert_eq!(m.unions.len(), 1);
        if let Some(union) = m.unions.first().map(|u| &u.value) {
            assert_eq!(i.resolve(union.name.value), Some("Msg"));
            assert_eq!(union.ctors.len(), 2);
            assert_eq!(
                union.ctors.first().and_then(|c| i.resolve(c.value.name)),
                Some("Increment")
            );
            assert_eq!(
                union.ctors.get(1).and_then(|c| i.resolve(c.value.name)),
                Some("Decrement")
            );
            assert!(union.ctors.first().is_some_and(|c| c.value.args.is_empty()));
        }

        // Two values: `update` (annotated, two patterns, case body) and `main`.
        assert_eq!(m.values.len(), 2);

        let update = find_value(&m, &i, "update");
        assert!(update.is_some(), "update value present");
        if let Some(uval) = update {
            assert_eq!(uval.patterns.len(), 2);
            assert!(
                uval.patterns
                    .first()
                    .is_some_and(|p| matches!(p.value, Pattern_::PVar(_)))
            );
            assert!(
                uval.patterns
                    .get(1)
                    .is_some_and(|p| matches!(p.value, Pattern_::PVar(_)))
            );

            // Annotation: Msg -> Int -> Int (two nested arrows, all TType leaves).
            assert!(uval.type_annotation.as_ref().is_some_and(|ann| matches!(
                &ann.value,
                TypeAnnotation::TLambda(a, rest)
                    if matches!(**a, TypeAnnotation::TType(_, _, _))
                        && matches!(
                            &**rest,
                            TypeAnnotation::TLambda(b, c)
                                if matches!(**b, TypeAnnotation::TType(_, _, _))
                                    && matches!(**c, TypeAnnotation::TType(_, _, _))
                        )
            )));

            // Body is a `case` with two arms, each a Binops body.
            assert!(
                matches!(&uval.body.value, Expr_::Case(s, _) if matches!(s.value, Expr_::VarLocal(_)))
            );
            if let Expr_::Case(_, arms) = &uval.body.value {
                assert_eq!(arms.len(), 2);
                for (pat, body) in arms {
                    assert!(matches!(pat.value, Pattern_::PCtor(_, _, _)));
                    assert!(matches!(body.value, Expr_::Binops(_, _)));
                }
            }
        }

        // `main`: no patterns, no annotation, nested Call body.
        let main = find_value(&m, &i, "main");
        assert!(main.is_some(), "main value present");
        if let Some(mval) = main {
            assert!(mval.patterns.is_empty());
            assert!(mval.type_annotation.is_none());
            assert!(
                matches!(&mval.body.value, Expr_::Call(c, _) if matches!(c.value, Expr_::VarLocal(_)))
            );
            if let Expr_::Call(_, args) = &mval.body.value {
                assert_eq!(args.len(), 1);
                // The single argument is itself a Call (String.fromInt (...)).
                assert!(
                    args.first()
                        .is_some_and(|a| matches!(a.value, Expr_::Call(_, _)))
                );
            }
        }
    }

    #[test]
    fn qualified_name_in_body_is_var_qual() {
        let mut i = Interner::new();
        let result = parse_module(GOLDEN, &mut i);
        assert!(result.is_ok());
        let Ok(m) = result else { return };
        let main = find_value(&m, &i, "main");
        assert!(main.is_some(), "main present");
        // main = println (String.fromInt (update Increment 0))
        let inner = main.and_then(|mv| match &mv.body.value {
            Expr_::Call(_, outer_args) => outer_args.first(),
            _ => None,
        });
        let qual = inner.and_then(|arg0| match &arg0.value {
            Expr_::Call(inner_callee, _) => Some(&inner_callee.value),
            _ => None,
        });
        assert!(
            qual.is_some_and(|q| matches!(
                q,
                Expr_::VarQual(qsym, nsym)
                    if i.resolve(*qsym) == Some("String") && i.resolve(*nsym) == Some("fromInt")
            )),
            "expected String.fromInt VarQual, got {qual:?}"
        );
    }

    /// A tiny deterministic xorshift PRNG — no external dependency.
    fn xorshift(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    /// The error code a bad source produces, or `"OK"` when it (unexpectedly)
    /// parses. Comparing the wire string keeps the assertions readable without
    /// importing every taxonomy constant.
    fn err_code(src: &str) -> String {
        let mut i = Interner::new();
        match parse_module(src, &mut i) {
            Ok(_) => "OK".to_owned(),
            Err(d) => d.code().as_str().to_owned(),
        }
    }

    /// A well-formed module header so a per-construct test isolates the body.
    const HDR: &str = "module Main exposing (main)\n";

    #[test]
    fn lexer_errors_carry_their_codes() {
        // SKY-P0010 unknown character.
        assert_eq!(err_code("module Main exposing (main)\nx = @"), "SKY-P0010");
        // SKY-P0011 stray '.'.
        assert_eq!(err_code("."), "SKY-P0011");
        // SKY-P0012 number joined to a name.
        assert_eq!(err_code("123abc"), "SKY-P0012");
        // SKY-P0013 integer literal out of range.
        assert_eq!(err_code("99999999999999999999999"), "SKY-P0013");
    }

    #[test]
    fn malformed_module_header_is_p0020() {
        // Not `module`.
        assert_eq!(err_code("import X exposing (..)"), "SKY-P0020");
        // Module name not an identifier.
        assert_eq!(err_code("module = x"), "SKY-P0020");
        // Missing `exposing`.
        assert_eq!(err_code("module Main"), "SKY-P0020");
    }

    #[test]
    fn malformed_exposing_list_is_p0021() {
        // Missing `(`.
        assert_eq!(err_code("module Main exposing main)"), "SKY-P0021");
        // Bad separator between items.
        assert_eq!(err_code("module Main exposing (a b)"), "SKY-P0021");
    }

    #[test]
    fn missing_equals_is_p0030() {
        assert_eq!(err_code(&format!("{HDR}foo 5")), "SKY-P0030");
    }

    #[test]
    fn malformed_type_declaration_is_p0031() {
        // Missing type name.
        assert_eq!(err_code(&format!("{HDR}type = Foo")), "SKY-P0031");
        // Missing `=` before constructors.
        assert_eq!(err_code(&format!("{HDR}type Foo Bar")), "SKY-P0031");
        // Constructor not uppercase.
        assert_eq!(err_code(&format!("{HDR}type Foo = bar")), "SKY-P0031");
    }

    #[test]
    fn type_args_on_non_constructor_is_p0040() {
        assert_eq!(err_code(&format!("{HDR}x : a Int\nx = 5")), "SKY-P0040");
    }

    #[test]
    fn expected_type_is_p0041() {
        // A token that cannot begin a type.
        assert_eq!(err_code(&format!("{HDR}x : =\nx = 5")), "SKY-P0041");
        // SHOULD-FIX: a dotted upper-ident (qualified type) is rejected, not
        // collapsed into a non-reference AST.
        assert_eq!(err_code(&format!("{HDR}x : Foo.Bar\nx = 5")), "SKY-P0041");
    }

    #[test]
    fn unclosed_delimiter_is_p0050() {
        assert_eq!(err_code(&format!("{HDR}main = (5")), "SKY-P0050");
    }

    #[test]
    fn malformed_case_is_p0060() {
        // `of` missing after the scrutinee.
        assert_eq!(err_code(&format!("{HDR}main = case 5")), "SKY-P0060");
        // `->` missing in a branch.
        let missing_arrow = format!("{HDR}main =\n    case main of\n        Foo 5\n");
        assert_eq!(err_code(&missing_arrow), "SKY-P0060");
    }

    #[test]
    fn unexpected_token_is_p0001() {
        // A token that cannot begin an expression.
        assert_eq!(err_code(&format!("{HDR}main = )")), "SKY-P0001");
        // SHOULD-FIX: a qualified constructor in pattern position is rejected.
        let qual_ctor = format!("{HDR}main =\n    case main of\n        Foo.Bar -> 1\n");
        assert_eq!(err_code(&qual_ctor), "SKY-P0001");
    }

    #[test]
    fn unexpected_eof_is_p0002() {
        assert_eq!(err_code(&format!("{HDR}main =")), "SKY-P0002");
    }

    #[test]
    fn nesting_too_deep_is_p0003() {
        let deep = format!("{HDR}main = {}", "(".repeat(400));
        assert_eq!(err_code(&deep), "SKY-P0003");
    }

    #[test]
    fn fuzz_random_bytes_never_panics_and_errors() {
        // Feed many 1 KB blobs of random bytes; the parser must reject every
        // one with a typed error and must never panic.
        let mut seed: u64 = 0x5DEE_CE66_D00D_F00D;
        for _ in 0..256 {
            let mut bytes = Vec::with_capacity(1024);
            for _ in 0..1024 {
                let r = xorshift(&mut seed);
                bytes.push(u8::try_from(r & 0xFF).unwrap_or(0));
            }
            let s = String::from_utf8_lossy(&bytes).into_owned();
            let mut i = Interner::new();
            let result = parse_module(&s, &mut i);
            assert!(result.is_err(), "random bytes must not parse as a module");
        }
    }
}

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
            .find(|v| i.resolve(v.name.value) == name)
    }

    #[test]
    fn parses_golden_module_structure() {
        let mut i = Interner::new();
        let result = parse_module(GOLDEN, &mut i);
        assert!(result.is_ok(), "golden must parse: {result:?}");
        let Ok(m) = result else { return };

        // Module header: name `Main`, exposing (main).
        assert_eq!(m.name.value.len(), 1);
        assert_eq!(m.name.value.first().map(|&s| i.resolve(s)), Some("Main"));
        assert!(matches!(
            &m.exposing.value,
            Exposing::List(items)
                if items.len() == 1
                    && items.first().is_some_and(|e| matches!(e.value, Exposed::Value(_)))
        ));

        // One import: Sky.Core.Prelude exposing (..).
        assert_eq!(m.imports.len(), 1);
        if let Some(imp) = m.imports.first() {
            let segs: Vec<&str> = imp.name.value.iter().map(|&s| i.resolve(s)).collect();
            assert_eq!(segs, ["Sky", "Core", "Prelude"]);
            assert!(imp.alias.is_none());
            assert!(matches!(imp.exposing.value, Exposing::All));
        }

        // One union: Msg { Increment, Decrement }.
        assert_eq!(m.unions.len(), 1);
        if let Some(union) = m.unions.first().map(|u| &u.value) {
            assert_eq!(i.resolve(union.name.value), "Msg");
            assert_eq!(union.ctors.len(), 2);
            assert_eq!(
                union.ctors.first().map(|c| i.resolve(c.value.name)),
                Some("Increment")
            );
            assert_eq!(
                union.ctors.get(1).map(|c| i.resolve(c.value.name)),
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
            assert!(uval
                .patterns
                .first()
                .is_some_and(|p| matches!(p.value, Pattern_::PVar(_))));
            assert!(uval
                .patterns
                .get(1)
                .is_some_and(|p| matches!(p.value, Pattern_::PVar(_))));

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
                assert!(args
                    .first()
                    .is_some_and(|a| matches!(a.value, Expr_::Call(_, _))));
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
                    if i.resolve(*qsym) == "String" && i.resolve(*nsym) == "fromInt"
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

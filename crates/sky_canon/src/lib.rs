#![forbid(unsafe_code)]
//! `sky_canon` — name resolution / canonicalisation for the Milestone-0 subset
//! of Sky.
//!
//! Entry point: [`canonicalise`]. It consumes a [`sky_syntax::Module`] (the raw
//! parse tree) plus a mutable [`Interner`] and produces a name-resolved
//! [`ast::Module`], or a typed [`sky_diagnostics::Diagnostic`]. Every variable
//! reference is classified — local binding, top-level binding, stdlib kernel,
//! or data constructor — by porting the M0 subset of the Haskell compiler's
//! `Sky.Canonicalise.{Module,Expression,Pattern,Type,Environment}`.

pub mod ast;
mod env;
mod resolve;

use sky_diagnostics::DResult;
use sky_intern::Interner;

pub use env::{CtorHome, Env, VarHome};

/// Canonicalise a parsed module into its name-resolved canonical AST.
///
/// # Errors
/// Returns a [`sky_diagnostics::Diagnostic`] — specifically
/// [`sky_diagnostics::NameError::Unknown`] — for any name that resolves to
/// neither a constructor, a bound variable, a top-level binding, nor a kernel
/// function.
pub fn canonicalise(m: &sky_syntax::Module, interner: &mut Interner) -> DResult<ast::Module> {
    resolve::canonicalise(m, interner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ast::{Def, Expr, Expr_, Pattern_};
    use sky_diagnostics::{Diagnostic, NameError};
    use sky_intern::Symbol;

    const GOLDEN: &str = include_str!("../../../tests/golden/m0/Main.sky");

    /// Parse + canonicalise the golden M0 module. Returns `None` (failing the
    /// caller's assertions) rather than panicking, per the no-panic gate.
    fn canon_golden(i: &mut Interner) -> Option<ast::Module> {
        let src = sky_parse::parse_module(GOLDEN, i).ok()?;
        canonicalise(&src, i).ok()
    }

    fn find_def<'a>(m: &'a ast::Module, i: &Interner, name: &str) -> Option<&'a Def> {
        m.defs.iter().find(|d| i.resolve(d.name().value) == name)
    }

    /// Drill into a [`Call`] node, returning callee + args.
    fn as_call(e: &Expr) -> Option<(&Expr_, &[Expr])> {
        match &e.value {
            Expr_::Call(callee, args) => Some((&callee.value, args)),
            _ => None,
        }
    }

    #[test]
    fn module_name_and_union_resolve() {
        let mut i = Interner::new();
        let m = canon_golden(&mut i);
        assert!(m.is_some(), "golden must parse + canonicalise");
        let Some(m) = m else { return };

        assert_eq!(m.name.len(), 1);
        assert_eq!(m.name.first().map(|&s| i.resolve(s)), Some("Main"));

        // The `Msg` union with two nullary constructors.
        assert_eq!(m.unions.len(), 1);
        let Some(union) = m.unions.first() else {
            return;
        };
        assert_eq!(i.resolve(union.name), "Msg");
        let names: Vec<(&str, usize)> = union
            .ctors
            .iter()
            .map(|c| (i.resolve(c.name), c.index))
            .collect();
        assert_eq!(names, vec![("Increment", 0), ("Decrement", 1)]);
    }

    #[test]
    fn update_body_resolves_locals_and_ctor_patterns() {
        let mut i = Interner::new();
        let m = canon_golden(&mut i);
        assert!(m.is_some(), "golden");
        let Some(m) = m else { return };

        let def = find_def(&m, &i, "update");
        assert!(
            matches!(def, Some(Def::Typed { .. })),
            "update is a typed def"
        );
        let Some(Def::Typed { patterns, body, .. }) = def else {
            return;
        };
        assert_eq!(patterns.len(), 2);

        // case msg of ...
        assert!(
            matches!(&body.value, Expr_::Case(..)),
            "update body is a case"
        );
        let Expr_::Case(scrut, branches) = &body.value else {
            return;
        };
        assert!(matches!(scrut.value, Expr_::VarLocal(s) if i.resolve(s) == "msg"));
        assert_eq!(branches.len(), 2);

        // First arm: `Increment -> count + 1`.
        let Some(inc) = branches.first() else { return };
        assert!(
            matches!(&inc.pat.value, Pattern_::PCtor { .. }),
            "arm pattern is a ctor"
        );
        let Pattern_::PCtor {
            type_name,
            name,
            index,
            ..
        } = &inc.pat.value
        else {
            return;
        };
        assert_eq!(i.resolve(*type_name), "Msg");
        assert_eq!(i.resolve(*name), "Increment");
        assert_eq!(*index, 0);

        // Body `count + 1` → Binop resolving to Basics.add over a local lhs.
        assert!(
            matches!(&inc.body.value, Expr_::Binop { .. }),
            "arm body is a binop"
        );
        let Expr_::Binop {
            home, func, lhs, ..
        } = &inc.body.value
        else {
            return;
        };
        assert_eq!(i.resolve(*home), "Basics");
        assert_eq!(i.resolve(*func), "add");
        assert!(matches!(lhs.value, Expr_::VarLocal(s) if i.resolve(s) == "count"));

        // Second arm resolves `-` to Basics.sub.
        let Some(dec) = branches.get(1) else { return };
        assert!(
            matches!(&dec.body.value, Expr_::Binop { .. }),
            "arm body is a binop"
        );
        let Expr_::Binop { func, .. } = &dec.body.value else {
            return;
        };
        assert_eq!(i.resolve(*func), "sub");
    }

    #[test]
    fn main_body_resolves_kernel_toplevel_and_ctor() {
        let mut i = Interner::new();
        let m = canon_golden(&mut i);
        assert!(m.is_some(), "golden");
        let Some(m) = m else { return };

        let def = find_def(&m, &i, "main");
        assert!(
            matches!(def, Some(Def::Untyped { .. })),
            "main is an untyped def"
        );
        let Some(Def::Untyped { body, .. }) = def else {
            return;
        };

        // main = println (String.fromInt (update Increment 0))
        let outer = as_call(body);
        assert!(
            matches!(outer, Some((Expr_::VarKernel { .. }, _))),
            "main body is a call to a kernel"
        );
        let Some((Expr_::VarKernel { module, name }, outer_args)) = outer else {
            return;
        };
        assert_eq!(i.resolve(*module), "Log");
        assert_eq!(i.resolve(*name), "println");
        assert_eq!(outer_args.len(), 1);

        // String.fromInt → VarKernel { String, fromInt }.
        let Some(arg0) = outer_args.first() else {
            return;
        };
        let mid = as_call(arg0);
        assert!(
            matches!(mid, Some((Expr_::VarKernel { .. }, _))),
            "arg is a call to a kernel"
        );
        let Some((Expr_::VarKernel { module, name }, mid_args)) = mid else {
            return;
        };
        assert_eq!(i.resolve(*module), "String");
        assert_eq!(i.resolve(*name), "fromInt");

        // update Increment 0 → VarTopLevel update applied to VarCtor + Int.
        let Some(mid0) = mid_args.first() else { return };
        let inner = as_call(mid0);
        assert!(
            matches!(inner, Some((Expr_::VarTopLevel { .. }, _))),
            "arg is a call to a top-level"
        );
        let Some((Expr_::VarTopLevel { module, name }, inner_args)) = inner else {
            return;
        };
        assert_eq!(module.first().map(|&s| i.resolve(s)), Some("Main"));
        assert_eq!(i.resolve(*name), "update");
        assert_eq!(inner_args.len(), 2);

        // `Increment` used as a value → VarCtor of Main.Msg.
        let Some(ctor_arg) = inner_args.first() else {
            return;
        };
        assert!(
            matches!(&ctor_arg.value, Expr_::VarCtor { .. }),
            "Increment is a ctor value"
        );
        let Expr_::VarCtor {
            type_name,
            name,
            index,
            home,
        } = &ctor_arg.value
        else {
            return;
        };
        assert_eq!(i.resolve(*type_name), "Msg");
        assert_eq!(i.resolve(*name), "Increment");
        assert_eq!(*index, 0);
        assert_eq!(home.first().map(|&s| i.resolve(s)), Some("Main"));

        // `0` literal.
        assert!(matches!(
            inner_args.get(1).map(|a| &a.value),
            Some(Expr_::Int(0))
        ));
    }

    #[test]
    fn typed_def_carries_arrow_annotation() {
        let mut i = Interner::new();
        let m = canon_golden(&mut i);
        assert!(m.is_some(), "golden");
        let Some(m) = m else { return };

        let def = find_def(&m, &i, "update");
        assert!(matches!(def, Some(Def::Typed { .. })), "update is typed");
        let Some(Def::Typed { ty, free_vars, .. }) = def else {
            return;
        };
        // No type variables in `Msg -> Int -> Int`.
        assert!(free_vars.is_empty());
        // Outer arrow: Msg -> (Int -> Int).
        assert!(
            matches!(ty, ast::Type::Lambda(_, _)),
            "annotation is an arrow"
        );
        let ast::Type::Lambda(arg, rest) = ty else {
            return;
        };
        assert!(
            matches!(arg.as_ref(), ast::Type::Con { .. }),
            "first arg is a constructor type"
        );
        let ast::Type::Con { name, home, .. } = arg.as_ref() else {
            return;
        };
        assert_eq!(i.resolve(*name), "Msg");
        // `Msg` is a local union → home is this module.
        assert_eq!(home.first().map(|&s| i.resolve(s)), Some("Main"));
        // Tail is Int -> Int.
        assert!(matches!(rest.as_ref(), ast::Type::Lambda(_, _)));
    }

    #[test]
    fn unknown_name_is_a_name_error() {
        let mut i = Interner::new();
        let src_text = "module Main exposing (main)\n\nmain = nope\n";
        let parsed = sky_parse::parse_module(src_text, &mut i);
        assert!(parsed.is_ok(), "source parses");
        let Ok(src) = parsed else { return };
        let result = canonicalise(&src, &mut i);
        assert!(matches!(
            result,
            Err(Diagnostic::Name {
                msg: NameError::Unknown,
                ..
            })
        ));
    }

    #[test]
    fn env_var_homes_compare() {
        // Exercise the VarHome surface for PartialEq coverage.
        assert_eq!(VarHome::Local, VarHome::Local);
        let m: Vec<Symbol> = vec![Symbol::from_raw(1)];
        assert_ne!(VarHome::TopLevel(m.clone()), VarHome::Local);
        assert_eq!(VarHome::TopLevel(m.clone()), VarHome::TopLevel(m));
    }
}

#![forbid(unsafe_code)]
//! `sky_lower` — the sequential integration point of the Milestone-0 pipeline.
//!
//! Entry point: [`lower`]. It consumes a name-resolved [`sky_canon::ast::Module`]
//! together with the [`sky_types::SolvedTypes`] produced by inference and emits
//! a backend-agnostic [`sky_ir::Program`]. This is a faithful but narrowed port
//! of the Haskell compiler's `Sky.Build.Compile` lowering core plus
//! `Sky.Build.LowerCtx`:
//!
//! * union declarations → [`sky_ir::TypeDef::Enum`];
//! * each top-level binding → a [`sky_ir::Func`] (its `case` body lowered to an
//!   exhaustive [`sky_ir::Match`] built through the validating
//!   [`sky_ir::Match::new`], its binops to [`sky_ir::BinOp`]);
//! * `main` → the module's `entry` function;
//! * kernel references (`Log.println`, `String.fromInt`) → [`sky_ir::Callee::Kernel`];
//! * top-level references (`Main.update`) → [`sky_ir::Callee::Func`].
//!
//! Lowering is *type-directed*: every [`sky_ir::IrType`] slot is filled from the
//! region/binding types in [`sky_types::SolvedTypes`]. A slot whose region type
//! is absent is an internal-invariant violation and surfaces as
//! [`sky_diagnostics::Diagnostic::CompilerBug`] — never a panic.

mod lower;

use sky_canon::ast as canon;
use sky_diagnostics::DResult;
use sky_intern::Interner;
use sky_types::SolvedTypes;

/// Lower a canonical module + its solved types into the typed IR.
///
/// # Errors
/// * Returns [`sky_diagnostics::Diagnostic::Lower`] when the input is valid Sky
///   that the M0 subset does not model yet (polymorphism, higher-order values,
///   non-`Task ()` results, extra kernels, non-constructor patterns, …),
///   carrying the offending node's span and its `SKY-L01##` feature.
/// * Returns [`sky_diagnostics::Diagnostic::CompilerBug`] when an internal
///   invariant is violated — a missing region type for an `IrType` slot, an
///   unresolved scrutinee enum, or a match arm set that fails
///   [`sky_ir::Match::new`]'s exhaustiveness proof. These are unreachable for
///   well-typed, well-canonicalised M0 input.
pub fn lower(
    m: &canon::Module,
    types: &SolvedTypes,
    interner: &Interner,
) -> DResult<sky_ir::Program> {
    lower::Lowerer::new(m, types, interner).run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sky_ir::{BinOp, Callee, Expr, IrType, KernelFn, TypeDef};

    const GOLDEN: &str = include_str!("../../../tests/golden/m0/Main.sky");

    /// Parse → canonicalise → infer the golden M0 module, then return the
    /// lowered program alongside the interner. Returns `None` (failing the
    /// caller's assertions) rather than panicking, per the no-panic gate.
    fn lower_golden() -> Option<(sky_ir::Program, Interner)> {
        let mut i = Interner::new();
        let src = sky_parse::parse_module(GOLDEN, &mut i).ok()?;
        let m = sky_canon::canonicalise(&src, &mut i).ok()?;
        let types = sky_types::infer(&m, &mut i).ok()?;
        let program = lower(&m, &types, &i).ok()?;
        Some((program, i))
    }

    fn find_func<'a>(
        module: &'a sky_ir::Module,
        i: &Interner,
        name: &str,
    ) -> Option<&'a sky_ir::Func> {
        module
            .funcs
            .iter()
            .find(|f| i.resolve(f.name) == Some(name))
    }

    #[test]
    fn lowers_one_module_with_main_entry() {
        let opt = lower_golden();
        assert!(opt.is_some(), "golden must lower");
        let Some((program, i)) = opt else { return };

        assert_eq!(program.modules.len(), 1);
        let Some(module) = program.modules.first() else {
            return;
        };
        assert_eq!(
            module
                .name
                .0
                .iter()
                .filter_map(|&s| i.resolve(s))
                .collect::<Vec<_>>(),
            vec!["Main"]
        );

        // entry points at the `main` func.
        let Some(main) = find_func(module, &i, "main") else {
            return;
        };
        assert_eq!(module.entry, Some(main.id));
    }

    #[test]
    fn lowers_msg_enum_in_declaration_order() {
        let opt = lower_golden();
        assert!(opt.is_some(), "golden");
        let Some((program, i)) = opt else { return };
        let Some(module) = program.modules.first() else {
            return;
        };

        assert_eq!(module.types.len(), 1);
        let Some(TypeDef::Enum(en)) = module.types.first() else {
            return;
        };
        assert_eq!(i.resolve(en.name), Some("Msg"));
        let variants: Vec<&str> = en.variants.iter().filter_map(|&s| i.resolve(s)).collect();
        assert_eq!(variants, vec!["Increment", "Decrement"]);
    }

    #[test]
    fn lowers_update_to_typed_func_with_exhaustive_match() {
        let opt = lower_golden();
        assert!(opt.is_some(), "golden");
        let Some((program, i)) = opt else { return };
        let Some(module) = program.modules.first() else {
            return;
        };

        let Some(update) = find_func(module, &i, "update") else {
            return;
        };

        // params: msg : Enum(Msg), count : Int.
        assert_eq!(update.params.len(), 2);
        let Some((p0, t0)) = update.params.first() else {
            return;
        };
        let Some((p1, t1)) = update.params.get(1) else {
            return;
        };
        assert_eq!(i.resolve(*p0), Some("msg"));
        assert!(matches!(t0, IrType::Enum(s) if i.resolve(*s) == Some("Msg")));
        assert_eq!(i.resolve(*p1), Some("count"));
        assert_eq!(*t1, IrType::Int);

        // return type : Int.
        assert_eq!(update.ret, IrType::Int);

        // body: an exhaustive match with two arms.
        assert!(
            matches!(&update.body, Expr::Match(_)),
            "update body must be a Match"
        );
        let Expr::Match(m) = &update.body else { return };
        assert!(matches!(m.scrutinee(), Expr::Var(s) if i.resolve(*s) == Some("msg")));
        assert_eq!(m.arms().len(), 2);

        // first arm: Increment -> (count + 1).
        let Some(arm0) = m.arms().first() else { return };
        let sky_ir::Pat::Ctor { variant, .. } = arm0.pat;
        assert_eq!(i.resolve(variant), Some("Increment"));
        assert!(matches!(&arm0.body, Expr::BinOp { op: BinOp::Add, .. }));

        // second arm: Decrement -> (count - 1).
        let Some(arm1) = m.arms().get(1) else { return };
        assert!(matches!(&arm1.body, Expr::BinOp { op: BinOp::Sub, .. }));
    }

    #[test]
    fn lowers_main_to_kernel_call_chain() {
        let opt = lower_golden();
        assert!(opt.is_some(), "golden");
        let Some((program, i)) = opt else { return };
        let Some(module) = program.modules.first() else {
            return;
        };

        let Some(main) = find_func(module, &i, "main") else {
            return;
        };
        assert!(main.params.is_empty());
        assert_eq!(main.ret, IrType::TaskUnit);

        // main = println (String.fromInt (update Increment 0))
        assert!(
            matches!(&main.body, Expr::Call { .. }),
            "main body is a call"
        );
        let Expr::Call { callee, args } = &main.body else {
            return;
        };
        assert_eq!(*callee, Callee::Kernel(KernelFn::LogPrintln));
        assert_eq!(args.len(), 1);

        let Some(Expr::Call {
            callee: c1,
            args: a1,
        }) = args.first()
        else {
            return;
        };
        assert_eq!(*c1, Callee::Kernel(KernelFn::StringFromInt));

        // inner: update Increment 0 → Callee::Func.
        let Some(Expr::Call {
            callee: c2,
            args: a2,
        }) = a1.first()
        else {
            return;
        };
        assert!(matches!(c2, Callee::Func(_)));
        assert_eq!(a2.len(), 2);
        assert!(matches!(a2.first(), Some(Expr::Ctor { .. })));
        assert!(matches!(a2.get(1), Some(Expr::Int(0))));
    }

    /// Lower a free-standing module and return the body of `which`.
    fn lower_body(source: &str, which: &str) -> Option<(Expr, Interner)> {
        let mut i = Interner::new();
        let src = sky_parse::parse_module(source, &mut i).ok()?;
        let m = sky_canon::canonicalise(&src, &mut i).ok()?;
        let types = sky_types::infer(&m, &mut i).ok()?;
        let program = lower(&m, &types, &i).ok()?;
        let module = program.modules.into_iter().next()?;
        let func = module
            .funcs
            .into_iter()
            .find(|f| i.resolve(f.name) == Some(which))?;
        Some((func.body, i))
    }

    #[test]
    fn lowers_full_arithmetic_with_precedence() {
        // `2 + 3 * 4` ⇒ Add(2, Mul(3, 4)).
        let opt = lower_body(
            "module Main exposing (v)\nv : Int\nv =\n    2 + 3 * 4\n",
            "v",
        );
        assert!(opt.is_some(), "v must lower");
        let Some((body, _)) = opt else { return };
        assert!(
            matches!(&body, Expr::BinOp { op: BinOp::Add, .. }),
            "body is Add(_, _)"
        );
        let Expr::BinOp { lhs, rhs, .. } = &body else {
            return;
        };
        assert!(matches!(lhs.as_ref(), Expr::Int(2)));
        assert!(
            matches!(rhs.as_ref(), Expr::BinOp { op: BinOp::Mul, .. }),
            "rhs is Mul(3, 4)"
        );
    }

    #[test]
    fn lowers_comparison_and_boolean_ops() {
        // `n > 10 && n < 100` ⇒ And(Gt(..), Lt(..)).
        let opt = lower_body(
            "module Main exposing (f)\nf : Int -> Bool\nf n =\n    n > 10 && n < 100\n",
            "f",
        );
        assert!(opt.is_some(), "f must lower");
        let Some((body, _)) = opt else { return };
        assert!(
            matches!(&body, Expr::BinOp { op: BinOp::And, .. }),
            "body is And(_, _)"
        );
        let Expr::BinOp { lhs, rhs, .. } = &body else {
            return;
        };
        assert!(matches!(lhs.as_ref(), Expr::BinOp { op: BinOp::Gt, .. }));
        assert!(matches!(rhs.as_ref(), Expr::BinOp { op: BinOp::Lt, .. }));
    }

    #[test]
    fn lowers_remaining_operators() {
        // Cover Sub, Div, Eq, Neq, Le, Ge, Or paths through `binop`.
        for (src_op, want) in [
            ("a - b", BinOp::Sub),
            ("a / b", BinOp::Div),
            ("a == b", BinOp::Eq),
            ("a /= b", BinOp::Neq),
            ("a <= b", BinOp::Le),
            ("a >= b", BinOp::Ge),
            ("a || b", BinOp::Or),
        ] {
            // Annotate to keep operand/result types concrete for each operator.
            // `/` (fdiv) is Float-typed, matching the Go backend.
            let sig = match want {
                BinOp::Sub => "f : Int -> Int -> Int",
                BinOp::Div => "f : Float -> Float -> Float",
                BinOp::Or => "f : Bool -> Bool -> Bool",
                _ => "f : Int -> Int -> Bool",
            };
            let source = format!("module Main exposing (f)\n{sig}\nf a b =\n    {src_op}\n");
            let opt = lower_body(&source, "f");
            assert!(
                matches!(&opt, Some((Expr::BinOp { .. }, _))),
                "{src_op} must lower to a binop"
            );
            let Some((Expr::BinOp { op, .. }, _)) = opt else {
                continue;
            };
            assert_eq!(op, want, "operator {src_op}");
        }
    }
}

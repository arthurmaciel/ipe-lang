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
    fn lowers_multi_binding_let_to_nested_lets() {
        // `let a = 1; b = a in a + b` ⇒ Let a (Let b (Add a b)).
        let opt = lower_body(
            "module Main exposing (v)\nv : Int\nv =\n    let\n        a = 1\n        b = a\n    in\n    a + b\n",
            "v",
        );
        assert!(opt.is_some(), "v must lower");
        let Some((body, i)) = opt else { return };
        let Expr::Let { name, value, body } = &body else {
            assert!(false_marker(), "outer is a Let, got {body:?}");
            return;
        };
        assert_eq!(i.resolve(*name), Some("a"), "outer binds a");
        assert!(matches!(value.as_ref(), Expr::Int(1)), "a = 1");
        // Inner: Let b = (Var a) in (Add a b).
        let Expr::Let {
            name: n2,
            value: v2,
            body: b2,
        } = body.as_ref()
        else {
            assert!(false_marker(), "inner is a Let");
            return;
        };
        assert_eq!(i.resolve(*n2), Some("b"), "inner binds b");
        assert!(
            matches!(v2.as_ref(), Expr::Var(s) if i.resolve(*s) == Some("a")),
            "b = a"
        );
        assert!(
            matches!(b2.as_ref(), Expr::BinOp { op: BinOp::Add, .. }),
            "in-body is a + b"
        );
    }

    #[test]
    fn lowers_inline_let_in_function_body() {
        // `let d = n + n in d` inside a typed function lowers to a single Let.
        let opt = lower_body(
            "module Main exposing (f)\nf : Int -> Int\nf n =\n    let d = n + n in d\n",
            "f",
        );
        assert!(opt.is_some(), "f must lower");
        let Some((body, i)) = opt else { return };
        assert!(
            matches!(&body, Expr::Let { name, .. } if i.resolve(*name) == Some("d")),
            "body is `let d = …`, got {body:?}"
        );
    }

    #[test]
    fn lowers_multi_way_if_to_nested_ifs() {
        // `if n > 0 then 1 else if n < 0 then 2 else 0` ⇒
        // If (n>0) 1 (If (n<0) 2 0): a right-nested chain of binary `If`s.
        let opt = lower_body(
            "module Main exposing (f)\nf : Int -> Int\nf n =\n    if n > 0 then\n        1\n    else if n < 0 then\n        2\n    else\n        0\n",
            "f",
        );
        assert!(opt.is_some(), "f must lower");
        let Some((body, _i)) = opt else { return };
        let Expr::If { cond, then_, else_ } = &body else {
            assert!(false_marker(), "outer is an If, got {body:?}");
            return;
        };
        assert!(
            matches!(cond.as_ref(), Expr::BinOp { op: BinOp::Gt, .. }),
            "outer cond is n > 0"
        );
        assert!(matches!(then_.as_ref(), Expr::Int(1)), "outer then is 1");
        // The else arm is the nested `if n < 0 then 2 else 0`.
        let Expr::If {
            cond: c2,
            then_: t2,
            else_: e2,
        } = else_.as_ref()
        else {
            assert!(false_marker(), "inner else is an If");
            return;
        };
        assert!(
            matches!(c2.as_ref(), Expr::BinOp { op: BinOp::Lt, .. }),
            "inner cond is n < 0"
        );
        assert!(matches!(t2.as_ref(), Expr::Int(2)), "inner then is 2");
        assert!(matches!(e2.as_ref(), Expr::Int(0)), "final else is 0");
    }

    /// A runtime `false` the optimiser cannot fold, so `assert!(false_marker())`
    /// fails the test without tripping `clippy::assertions_on_constants`.
    fn false_marker() -> bool {
        std::hint::black_box(false)
    }

    #[test]
    fn lowers_lambda_to_typed_closure_and_application_to_apply() {
        // `let inc = \x -> x + 1 in inc 41`: the binding value is a typed
        // `Lambda`, and `inc 41` (a local callee) lowers to `Apply`.
        let opt = lower_body(
            "module Main exposing (v)\nv : Int\nv =\n    let inc = \\x -> x + 1 in inc 41\n",
            "v",
        );
        assert!(opt.is_some(), "v must lower");
        let Some((body, i)) = opt else { return };
        let Expr::Let { value, body, .. } = &body else {
            assert!(false_marker(), "outer is a Let, got {body:?}");
            return;
        };
        // The let value is a one-parameter `Int -> Int` lambda.
        assert!(
            matches!(
                value.as_ref(),
                Expr::Lambda { params, ret, .. }
                    if params.len() == 1
                        && params.first().map(|(_, t)| t) == Some(&IrType::Int)
                        && *ret == IrType::Int
            ),
            "inc is a typed Int->Int lambda, got {value:?}"
        );
        // The `in` body applies the local `inc` via Apply.
        assert!(
            matches!(
                body.as_ref(),
                Expr::Apply { func, args }
                    if matches!(func.as_ref(), Expr::Var(s) if i.resolve(*s) == Some("inc"))
                        && args.len() == 1
            ),
            "inc 41 lowers to Apply, got {body:?}"
        );
    }

    #[test]
    fn lowers_inline_capturing_lambda_application() {
        // `let n = 10 in (\x -> x + n) 5`: the inline lambda is the callee, so
        // the application lowers to `Apply` over a `Lambda` (capturing `n`).
        let opt = lower_body(
            "module Main exposing (v)\nv : Int\nv =\n    let n = 10 in (\\x -> x + n) 5\n",
            "v",
        );
        assert!(opt.is_some(), "v must lower");
        let Some((body, _)) = opt else { return };
        let Expr::Let { body, .. } = &body else {
            assert!(false_marker(), "outer is a Let, got {body:?}");
            return;
        };
        assert!(
            matches!(
                body.as_ref(),
                Expr::Apply { func, args }
                    if matches!(func.as_ref(), Expr::Lambda { .. }) && args.len() == 1
            ),
            "applied inline lambda lowers to Apply over a Lambda, got {body:?}"
        );
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

    #[test]
    fn tuple_value_lowers_to_ir_tuple() {
        // `v = (1, 2)` lowers to the IR tuple constructor over two Int literals.
        let opt = lower_body("module Main exposing (v)\nv =\n    (1, 2)\n", "v");
        assert!(
            matches!(&opt, Some((Expr::Tuple(es), _))
                if es.len() == 2
                    && matches!(es.first(), Some(Expr::Int(1)))
                    && matches!(es.get(1), Some(Expr::Int(2)))),
            "v lowers to `Tuple([Int(1), Int(2)])`, got {:?}",
            opt.as_ref().map(|(b, _)| b)
        );
    }

    #[test]
    fn tuple_return_type_lowers_to_ir_tuple_type() {
        // An untyped no-param binding's inferred tuple type flows to the func's
        // IR return type as `IrType::Tuple`.
        let mut i = Interner::new();
        let pipeline = (|| {
            let src =
                sky_parse::parse_module("module Main exposing (v)\nv =\n    (1, 2)\n", &mut i)
                    .ok()?;
            let m = sky_canon::canonicalise(&src, &mut i).ok()?;
            let types = sky_types::infer(&m, &mut i).ok()?;
            lower(&m, &types, &i).ok()
        })();
        assert!(pipeline.is_some(), "v must lower");
        let Some(program) = pipeline else { return };
        let Some(module) = program.modules.first() else {
            return;
        };
        let Some(v) = find_func(module, &i, "v") else {
            return;
        };
        assert!(
            matches!(&v.ret, IrType::Tuple(es)
                if es.len() == 2
                    && matches!(es.first(), Some(IrType::Int))
                    && matches!(es.get(1), Some(IrType::Int))),
            "v's IR return type is `(Int, Int)`, got {:?}",
            v.ret
        );
    }
}

//! Tail-recursion detection + rewrite units.
//!
//! IR is built by hand (no full lower pipeline) so each shape is isolated. The
//! detection mirrors the reference implementation (`Ipe.Build.TailCallOpt`).

use ipe_diagnostics::DResult;
use ipe_intern::{Interner, Symbol};
use ipe_ir::{BinOp, CallPin, Callee, Expr, FuncId, IrType, OnFormKind};
use ipe_lower::tco_analysis::{TailRecursion, analyze_tail_recursion, rewrite_tail_calls};

const SELF: FuncId = FuncId::from_raw(7);

/// A hand-built function under analysis: its id, arity, params, and body.
type BuiltFn = (FuncId, usize, Vec<(Symbol, IrType)>, Expr);

/// `count n acc = if n == 0 then acc else count (n - 1) (acc + 1)`.
/// Arity 2; the else-branch self-call is a tail call.
fn build_count(i: &mut Interner) -> DResult<BuiltFn> {
    let n = i.intern("n")?;
    let acc = i.intern("acc")?;
    let params = vec![(n, IrType::Int), (acc, IrType::Int)];
    let body = Expr::If {
        cond: Box::new(Expr::BinOp {
            op: BinOp::Eq,
            lhs: Box::new(Expr::Var(n)),
            rhs: Box::new(Expr::Int(0)),
        }),
        then_: Box::new(Expr::Var(acc)),
        else_: Box::new(Expr::Call {
            callee: Callee::Func(SELF),
            args: vec![
                Expr::BinOp {
                    op: BinOp::IntSub,
                    lhs: Box::new(Expr::Var(n)),
                    rhs: Box::new(Expr::Int(1)),
                },
                Expr::BinOp {
                    op: BinOp::IntAdd,
                    lhs: Box::new(Expr::Var(acc)),
                    rhs: Box::new(Expr::Int(1)),
                },
            ],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }),
    };
    Ok((SELF, 2, params, body))
}

fn contains_self_call(id: FuncId, e: &Expr) -> bool {
    match e {
        Expr::Call {
            callee: Callee::Func(c),
            args,
            ..
        } => *c == id || args.iter().any(|a| contains_self_call(id, a)),
        Expr::Call { args, .. } => args.iter().any(|a| contains_self_call(id, a)),
        Expr::If { cond, then_, else_ } => {
            contains_self_call(id, cond)
                || contains_self_call(id, then_)
                || contains_self_call(id, else_)
        }
        Expr::Match(m) => {
            contains_self_call(id, m.scrutinee())
                || m.arms().iter().any(|a| contains_self_call(id, &a.body))
        }
        Expr::Let { value, body, .. } | Expr::Destructure { value, body, .. } => {
            contains_self_call(id, value) || contains_self_call(id, body)
        }
        Expr::BinOp { lhs, rhs, .. } => contains_self_call(id, lhs) || contains_self_call(id, rhs),
        Expr::Lambda { body, .. } | Expr::TailLoop { body, .. } => contains_self_call(id, body),
        Expr::Apply { func, args } => {
            contains_self_call(id, func) || args.iter().any(|a| contains_self_call(id, a))
        }
        Expr::Ctor { args, .. } | Expr::TailRecur { args } => {
            args.iter().any(|a| contains_self_call(id, a))
        }
        Expr::Tuple(xs) | Expr::List { items: xs, .. } => {
            xs.iter().any(|x| contains_self_call(id, x))
        }
        Expr::Cons { head, tail } => contains_self_call(id, head) || contains_self_call(id, tail),
        _ => false,
    }
}

fn contains_tail_recur(e: &Expr) -> bool {
    match e {
        Expr::TailRecur { .. } => true,
        Expr::TailLoop { body, .. } | Expr::Let { body, .. } | Expr::Destructure { body, .. } => {
            contains_tail_recur(body)
        }
        Expr::If { then_, else_, .. } => contains_tail_recur(then_) || contains_tail_recur(else_),
        Expr::Match(m) => m.arms().iter().any(|a| contains_tail_recur(&a.body)),
        _ => false,
    }
}

#[test]
fn count_is_tail_recursive() -> DResult<()> {
    let mut i = Interner::new();
    let (id, arity, _params, body) = build_count(&mut i)?;
    assert_eq!(
        analyze_tail_recursion(id, arity, &body),
        TailRecursion::TailRecursive
    );
    Ok(())
}

#[test]
fn foldr_shape_is_not_tail_recursive() -> DResult<()> {
    // f x = g (f x) — the self-call is an ARG to another Call, so it is non-tail.
    let mut i = Interner::new();
    let x = i.intern("x")?;
    let g = FuncId::from_raw(99);
    let body = Expr::Call {
        callee: Callee::Func(g),
        args: vec![Expr::Call {
            callee: Callee::Func(SELF),
            args: vec![Expr::Var(x)],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }],
        pin: CallPin::None,
        on_form: OnFormKind::NotForm,
    };
    assert_eq!(
        analyze_tail_recursion(SELF, 1, &body),
        TailRecursion::NotTailRecursive
    );
    Ok(())
}

#[test]
fn self_call_in_lambda_is_not_tail() -> DResult<()> {
    // A self-call inside a Lambda body placed in tail position: `in_tail` flips
    // false entering the lambda, so it counts as non-tail → disqualifies.
    let mut i = Interner::new();
    let x = i.intern("x")?;
    let body = Expr::Lambda {
        params: vec![(x, IrType::Int)],
        ret: IrType::Int,
        body: Box::new(Expr::Call {
            callee: Callee::Func(SELF),
            args: vec![Expr::Var(x)],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }),
    };
    assert_eq!(
        analyze_tail_recursion(SELF, 1, &body),
        TailRecursion::NotTailRecursive
    );
    Ok(())
}

#[test]
fn wrong_arity_self_call_disqualifies() -> DResult<()> {
    // A tail-position self-call at arity 1 when the fn's arity is 2 is an escape.
    let mut i = Interner::new();
    let x = i.intern("x")?;
    let body = Expr::Call {
        callee: Callee::Func(SELF),
        args: vec![Expr::Var(x)],
        pin: CallPin::None,
        on_form: OnFormKind::NotForm,
    };
    assert_eq!(
        analyze_tail_recursion(SELF, 2, &body),
        TailRecursion::NotTailRecursive
    );
    Ok(())
}

#[test]
fn no_self_call_is_not_tail_recursive() {
    // Zero self-calls → tail count 0 → NotTailRecursive.
    let body = Expr::Int(42);
    assert_eq!(
        analyze_tail_recursion(SELF, 0, &body),
        TailRecursion::NotTailRecursive
    );
}

#[test]
fn func_value_self_reference_disqualifies() {
    // Passing OUR fn as a first-class value is an escape, not a jump.
    let body = Expr::FuncValue {
        callee: Callee::Func(SELF),
        ty: IrType::Fun(vec![IrType::Int], Box::new(IrType::Int)),
    };
    assert_eq!(
        analyze_tail_recursion(SELF, 1, &body),
        TailRecursion::NotTailRecursive
    );
}

#[test]
fn rewrite_wraps_in_tailloop_and_replaces_jump() -> DResult<()> {
    let mut i = Interner::new();
    let (id, arity, params, body) = build_count(&mut i)?;
    let out = rewrite_tail_calls(id, arity, params, body);
    // Top node is a TailLoop; no self-`Call` survives; a TailRecur was produced.
    // (`contains_*` recurse through the TailLoop wrapper.)
    assert!(
        matches!(out, Expr::TailLoop { .. }),
        "expected TailLoop at the top of the rewritten body"
    );
    assert!(
        !contains_self_call(id, &out),
        "a self-Call survived the rewrite"
    );
    assert!(
        contains_tail_recur(&out),
        "no TailRecur produced by the rewrite"
    );
    Ok(())
}

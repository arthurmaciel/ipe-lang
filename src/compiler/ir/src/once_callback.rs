//! At-most-once callback slots, read once for the lowerer and both emitters.
//!
//! A kernel whose runtime wrapper takes one argument as an owned
//! `Box<dyn FnOnce(..)>` ([`crate::KernelFn::once_callback_arg`]) lets an inline
//! lambda in that slot MOVE its non-`Clone` captures. Capture-clone passes may
//! wrap that lambda in a prelude of `let s = s.clone();` bindings
//! (`Let { s, CloneVar(s), .. }`). [`peel_once_closure`] is the single reading of
//! that shape: the emitters render what it accepts as an unannotated `FnOnce`
//! closure, and the lowerer refuses a moving slot argument it rejects
//! ([`find_map_unpeelable_once_arg`]), so no slot is ever emitted through the
//! `Fn`-annotated boxed-lambda form that a moved capture cannot satisfy.

use ipe_intern::Symbol;

use crate::{Callee, Expr, IrType};

/// An inline closure in an at-most-once slot, under its clone prelude.
///
/// `clones` holds each `let binder = source.clone();` of the prelude, outermost
/// first; `params`, `ret`, and `body` are the closure's own.
#[derive(Debug)]
pub struct OnceClosure<'a> {
    pub clones: Vec<(Symbol, Symbol)>,
    pub params: &'a [(Symbol, IrType)],
    pub ret: &'a IrType,
    pub body: &'a Expr,
}

/// Peel a chain of `Let { _, CloneVar(_), .. }` down to an inline lambda.
///
/// `None` when the chain ends in anything but a `Lambda` / `SharedLambda`, or
/// when a prelude binding's value is not a bare `CloneVar`.
#[must_use]
pub fn peel_once_closure(arg: &Expr) -> Option<OnceClosure<'_>> {
    let mut clones = Vec::new();
    let mut current = arg;
    loop {
        match current {
            Expr::Let { name, value, body } => {
                let Expr::CloneVar(source) = value.as_ref() else {
                    return None;
                };
                clones.push((*name, *source));
                current = body;
            }
            Expr::Lambda { params, ret, body } | Expr::SharedLambda { params, ret, body } => {
                return Some(OnceClosure {
                    clones,
                    params,
                    ret,
                    body,
                });
            }
            _ => return None,
        }
    }
}

/// The at-most-once slot of a call and the argument filling it.
///
/// `None` for a non-kernel callee, a kernel with no such slot, or an argument
/// list too short to fill it.
#[must_use]
pub fn once_callback_split<'a>(callee: &Callee, args: &'a [Expr]) -> Option<(usize, &'a Expr)> {
    let Callee::Kernel(kernel) = callee else {
        return None;
    };
    let slot = kernel.once_callback_arg()?;
    args.get(slot).map(|arg| (slot, arg))
}

/// The first `Some` that `probe` yields for an at-most-once slot argument [`peel_once_closure`] rejects.
///
/// Walks every node of `expr` (iteratively, so nesting depth never grows the
/// call stack) and offers each rejected slot argument to `probe`.
#[must_use]
pub fn find_map_unpeelable_once_arg<T>(
    expr: &Expr,
    mut probe: impl FnMut(&Expr) -> Option<T>,
) -> Option<T> {
    let mut pending: Vec<&Expr> = vec![expr];
    while let Some(node) = pending.pop() {
        if let Expr::Call { callee, args, .. } = node
            && let Some((_, arg)) = once_callback_split(callee, args)
            && peel_once_closure(arg).is_none()
            && let Some(found) = probe(arg)
        {
            return Some(found);
        }
        push_children(node, &mut pending);
    }
    None
}

/// Push every direct sub-expression of `expr` onto `pending`.
fn push_children<'a>(expr: &'a Expr, pending: &mut Vec<&'a Expr>) {
    match expr {
        Expr::Int(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::PathLit(_)
        | Expr::CustomElementRef { .. }
        | Expr::Char(_)
        | Expr::Unit
        | Expr::Var(_)
        | Expr::CloneVar(_)
        | Expr::FuncValue { .. } => {}
        Expr::Ctor { args, .. }
        | Expr::Call { args, .. }
        | Expr::Tuple(args)
        | Expr::TailRecur { args }
        | Expr::List { items: args, .. } => pending.extend(args),
        Expr::BinOp { lhs, rhs, .. } => pending.extend([lhs.as_ref(), rhs.as_ref()]),
        Expr::Let { value, body, .. }
        | Expr::Destructure { value, body, .. }
        | Expr::TaskSeq {
            effect: value,
            rest: body,
        }
        | Expr::Cons {
            head: value,
            tail: body,
        } => pending.extend([value.as_ref(), body.as_ref()]),
        Expr::If { cond, then_, else_ } => {
            pending.extend([cond.as_ref(), then_.as_ref(), else_.as_ref()]);
        }
        Expr::Match(m) => {
            pending.push(m.scrutinee());
            for arm in m.arms() {
                pending.push(&arm.body);
                pending.extend(arm.guard.as_ref());
            }
        }
        Expr::ListIndexClone { list, .. } | Expr::ListLenCheck { list, .. } => pending.push(list),
        Expr::Record { fields, .. } => pending.extend(fields.iter().map(|(_, e)| e)),
        Expr::Access { record, .. } => pending.push(record),
        Expr::Update { record, fields } => {
            pending.push(record);
            pending.extend(fields.iter().map(|(_, e)| e));
        }
        Expr::Lambda { body, .. }
        | Expr::SharedLambda { body, .. }
        | Expr::TailLoop { body, .. } => pending.push(body),
        Expr::Apply { func, args } => {
            pending.push(func);
            pending.extend(args);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CallPin, KernelFn, OnFormKind};

    #[allow(clippy::expect_used)] // a fresh interner always has room for two names
    fn symbols() -> (Symbol, Symbol) {
        let mut interner = ipe_intern::Interner::new();
        let s = interner.intern("s").expect("intern");
        let n = interner.intern("n").expect("intern");
        (s, n)
    }

    fn lambda(param: Symbol, body: Expr) -> Expr {
        Expr::Lambda {
            params: vec![(param, IrType::Int)],
            ret: IrType::Int,
            body: Box::new(body),
        }
    }

    fn clone_let(name: Symbol, body: Expr) -> Expr {
        Expr::Let {
            name,
            value: Box::new(Expr::CloneVar(name)),
            body: Box::new(body),
        }
    }

    fn and_then(cont: Expr, effect: Expr) -> Expr {
        Expr::Call {
            callee: Callee::Kernel(KernelFn::TaskAndThen),
            args: vec![cont, effect],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }
    }

    /// A bare lambda peels with an empty prelude.
    #[test]
    fn bare_lambda_peels_with_no_clones() {
        let (s, n) = symbols();
        let peeled = peel_once_closure(&lambda(n, Expr::Var(s)));
        assert!(peeled.is_some_and(|closure| closure.clones.is_empty()));
    }

    /// A clone-prelude chain peels outermost first down to its lambda.
    #[test]
    fn clone_prelude_chain_peels_in_order() {
        let (s, n) = symbols();
        let arg = clone_let(s, clone_let(n, lambda(n, Expr::Var(s))));
        let peeled = peel_once_closure(&arg);
        assert!(peeled.is_some_and(|closure| closure.clones == vec![(s, s), (n, n)]));
    }

    /// A prelude binding a non-clone value, or ending off a lambda, is rejected.
    #[test]
    fn non_clone_prelude_or_non_lambda_tail_is_rejected() {
        let (s, n) = symbols();
        let non_clone_value = Expr::Let {
            name: s,
            value: Box::new(Expr::Var(n)),
            body: Box::new(lambda(n, Expr::Var(s))),
        };
        assert!(peel_once_closure(&non_clone_value).is_none());
        assert!(peel_once_closure(&clone_let(s, Expr::Var(s))).is_none());
        assert!(peel_once_closure(&Expr::Var(s)).is_none());
    }

    /// The slot split follows `once_callback_arg`, never a hardcoded kernel.
    #[test]
    fn split_follows_the_kernel_slot() {
        let (s, _) = symbols();
        let args = [Expr::Var(s), Expr::Unit];
        let split = once_callback_split(&Callee::Kernel(KernelFn::TaskAndThen), &args);
        assert!(matches!(split, Some((0, Expr::Var(v))) if *v == s));
        assert!(once_callback_split(&Callee::Kernel(KernelFn::TaskMap), &args).is_none());
    }

    /// Only an unpeelable slot argument, however deeply nested, reaches the probe.
    #[test]
    fn walker_offers_only_unpeelable_slot_args() {
        let (s, n) = symbols();
        let peelable = and_then(clone_let(s, lambda(n, Expr::Var(s))), Expr::Unit);
        assert!(find_map_unpeelable_once_arg(&peelable, |_| Some(())).is_none());
        let nested = lambda(n, and_then(Expr::Var(s), Expr::Unit));
        let found = find_map_unpeelable_once_arg(&nested, |arg| match arg {
            Expr::Var(v) => Some(*v),
            _ => None,
        });
        assert_eq!(found, Some(s));
    }
}

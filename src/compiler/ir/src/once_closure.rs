//! The one verdict on whether a closure position calls its closure at most once.
//!
//! A closure that moves a non-`Clone` capture is [`Expr::OnceLambda`]: it is
//! `FnOnce` only. [`admits_once`] is the single table that decides which parent
//! positions may hold one. The lowerer refuses an `OnceLambda` at every other
//! position, and the backend emits one only where this predicate admits it, so
//! the two boundaries cannot disagree.

use ipe_intern::Symbol;

use crate::{Expr, IrType, KernelFn};

/// The first non-`Clone` capture an [`Expr::OnceLambda`] body moves.
///
/// `lo`/`hi` are the raw byte offsets of the capture's use in the source: the
/// IR carries no `Span`, and the lowerer rebuilds one from these offsets for
/// the refusal diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MovedCapture {
    /// The moved local.
    pub name: Symbol,
    /// Start offset of the capture's use.
    pub lo: u32,
    /// End offset of the capture's use.
    pub hi: u32,
}

/// The position a closure value occupies in its parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClosureSite<'a> {
    /// `Apply { func: <closure>, args }`: `arity` is the closure's parameter
    /// count and `args` the number of arguments applied.
    ImmediateApply {
        /// The closure's parameter count.
        arity: usize,
        /// The number of arguments the apply passes.
        args: usize,
    },
    /// Argument `index` of a kernel call.
    KernelArg {
        /// The called kernel.
        kernel: &'a KernelFn,
        /// The argument position, in Ipê order.
        index: usize,
    },
    /// Any other position: let-bound, stored, returned, a constructor field, a
    /// user-function argument, a list element.
    Other,
}

/// Does `site` call the closure it holds at most once?
///
/// * An immediate apply admits exactly when it is saturated with at least one
///   argument: the backend then inlines the closure as `let` bindings, so the
///   closure is never boxed.
/// * A kernel argument admits only `Task.andThen`'s continuation (index 0),
///   which the backend boxes into the runtime's `Box<dyn FnOnce>` slot.
/// * Every other position never admits. A user function's parameter is an
///   `impl Fn` or a boxed `Fn`, and its body may call it many times.
#[must_use]
pub const fn admits_once(site: &ClosureSite<'_>) -> bool {
    match site {
        ClosureSite::ImmediateApply { arity, args } => *arity == *args && *args > 0,
        ClosureSite::KernelArg { kernel, index } => {
            matches!(**kernel, KernelFn::TaskAndThen) && *index == 0
        }
        ClosureSite::Other => false,
    }
}

/// A closure's parameters, return type and body, borrowed from its IR node.
pub type ClosureParts<'e> = (&'e [(Symbol, IrType)], &'e IrType, &'e Expr);

/// The parameters, return type and body of an [`Expr::OnceLambda`] at an admitted `site`.
///
/// `None` for any other expression, and for an `OnceLambda` at a position
/// [`admits_once`] refuses. The backend's admitted emit paths read a once
/// closure only through this, so they share the lowerer's verdict.
#[must_use]
pub fn admitted_once_parts<'e>(expr: &'e Expr, site: &ClosureSite<'_>) -> Option<ClosureParts<'e>> {
    if let Expr::OnceLambda {
        params, ret, body, ..
    } = expr
        && admits_once(site)
    {
        Some((params.as_slice(), ret, body))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn immediate(arity: usize, args: usize) -> bool {
        admits_once(&ClosureSite::ImmediateApply { arity, args })
    }

    fn kernel_arg(kernel: KernelFn, index: usize) -> bool {
        admits_once(&ClosureSite::KernelArg {
            kernel: &kernel,
            index,
        })
    }

    #[test]
    fn immediate_apply_admits_only_a_saturated_non_empty_apply() {
        assert!(immediate(1, 1));
        assert!(immediate(2, 2));
        assert!(!immediate(2, 1), "an under-applied closure is a value");
        assert!(!immediate(1, 2), "a curried apply is not inlined");
        assert!(!immediate(0, 0), "a zero-argument apply is boxed");
    }

    #[test]
    fn kernel_arg_admits_only_the_task_and_then_continuation() {
        assert!(kernel_arg(KernelFn::TaskAndThen, 0));
        assert!(!kernel_arg(KernelFn::TaskAndThen, 1));
        assert!(!kernel_arg(KernelFn::TaskMap, 0));
        assert!(!kernel_arg(KernelFn::ListMap, 0));
    }

    #[test]
    fn other_never_admits() {
        assert!(!admits_once(&ClosureSite::Other));
    }

    #[test]
    fn admitted_parts_read_only_an_admitted_once_lambda() -> ipe_diagnostics::DResult<()> {
        let mut i = ipe_intern::Interner::new();
        let x = i.intern("x")?;
        let once = Expr::OnceLambda {
            params: vec![(x, IrType::Int)],
            ret: IrType::Int,
            body: Box::new(Expr::Var(x)),
            capture: MovedCapture {
                name: x,
                lo: 0,
                hi: 1,
            },
        };
        let lambda = Expr::Lambda {
            params: vec![(x, IrType::Int)],
            ret: IrType::Int,
            body: Box::new(Expr::Var(x)),
        };
        let admitted = ClosureSite::KernelArg {
            kernel: &KernelFn::TaskAndThen,
            index: 0,
        };
        assert!(admitted_once_parts(&once, &admitted).is_some());
        assert!(admitted_once_parts(&once, &ClosureSite::Other).is_none());
        assert!(admitted_once_parts(&lambda, &admitted).is_none());
        Ok(())
    }
}

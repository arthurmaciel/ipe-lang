//! The capture-clone rewrite for sequenced tasks, shared by the backend and the
//! lowerer.
//!
//! A `TaskSeq` (and `Task.andThen`) emits `task_and_then(effect, Box::new(move
//! |_| { rest }))`: Rust evaluates `effect` first, then builds the `move`
//! continuation. Every variable `rest` captures that `effect` would move is
//! rewritten to a `CloneVar` in `effect` ([`clone_targets_in_expr`]). The
//! lowerer's `IPE-L0135` gate must refuse exactly the programs whose rewrite
//! clones a value that has no `Clone` impl ([`seq_rewrite_clones_symbol`]), so
//! both read this one rewrite rather than parallel copies.

use std::collections::BTreeSet;

use ipe_intern::Symbol;

use crate::let_inline::{
    inlined_let_body, let_value_is_inlined, pat_binds_target, scan_free_target,
};
use crate::{Callee, Expr, KernelFn};

/// Shadow-aware IR rewrite: replace every free `Var(target)` with `CloneVar(target)`.
///
/// Recursion stops at any subtree where a binder rebinds `target` (that
/// occurrence is a different binding, not the captured one). Structurally
/// identical shadow-skip shape to `ipe_lower::rewrite_var_free_occurrences`,
/// with a `CloneVar` leaf action instead of the caller-supplied leaf.
///
/// Cloning a `Copy` value (Int/Bool/…) compiles to a bitwise copy — harmless —
/// so this never needs a Copy/non-Copy type check to stay sound; it only ever
/// clones a variable that a caller determined is genuinely captured (see
/// [`clone_targets_in_expr`]).
///
/// A read that only BORROWS `target` in an eager position — evaluated in place,
/// before the continuation is built — keeps a bare `Var`: a field read
/// `(w).tag`, a list index or a list length check ends its borrow before the
/// continuation moves `target`, so no whole-carrier clone is needed, and a
/// carrier with no `Clone` impl stays emittable. A borrow inside a closure, a
/// continuation, a match arm, an inlined `let` value, or a kernel/FFI argument
/// (which the emitter may defer into a `move` closure) is cloned as before.
///
/// `row_binders` is the enclosing function's set of row-generic parameter
/// binders (the symbols the Access emitter routes through a borrowing witness
/// getter `ipe_<field>()`). A whole-row `CloneVar` on such a receiver would
/// fall through the emitter's `Var`-only getter route to a raw struct-field
/// read on the opaque `R{n}` generic — the exit-0-then-cargo-fail class. The
/// getter borrows, so no whole-row clone is ever needed there: a row-generic
/// Access receiver is left a bare `Var`, upholding the invariant that a
/// row-generic value only ever reaches emission as `Access { record: Var(row) }`.
#[must_use]
pub fn clone_free_target(expr: Expr, target: Symbol, row_binders: &BTreeSet<Symbol>) -> Expr {
    rewrite(expr, target, row_binders, true)
}

/// Rewrite the base of a borrowing read: a bare `Var(target)` in an eager position stays a `Var`.
fn borrow_base(base: Expr, target: Symbol, row_binders: &BTreeSet<Symbol>, eager: bool) -> Expr {
    match base {
        Expr::Var(s) if eager && s == target => Expr::Var(s),
        other => rewrite(other, target, row_binders, eager),
    }
}

/// The [`clone_free_target`] walk; `eager` marks a position evaluated in place.
#[allow(clippy::too_many_lines)] // A recursive tree-walk over a large enum — necessarily long.
fn rewrite(expr: Expr, target: Symbol, row_binders: &BTreeSet<Symbol>, eager: bool) -> Expr {
    match expr {
        Expr::Var(s) if s == target => Expr::CloneVar(s),
        Expr::Var(_)
        | Expr::CloneVar(_)
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::PathLit(_)
        | Expr::CustomElementRef { .. }
        | Expr::Char(_)
        | Expr::Unit
        | Expr::FuncValue { .. } => expr,
        Expr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op,
            lhs: Box::new(rewrite(*lhs, target, row_binders, eager)),
            rhs: Box::new(rewrite(*rhs, target, row_binders, eager)),
        },
        Expr::Let { name, value, body } => {
            // An inlined value is re-evaluated at each use site in `body`,
            // possibly inside a closure, so it is not an eager position.
            let value_eager = eager && !let_value_is_inlined(name, &value, &body);
            let new_value = Box::new(rewrite(*value, target, row_binders, value_eager));
            let new_body = if name == target {
                body
            } else {
                Box::new(rewrite(*body, target, row_binders, eager))
            };
            Expr::Let {
                name,
                value: new_value,
                body: new_body,
            }
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            let new_value = Box::new(rewrite(*value, target, row_binders, eager));
            let new_body = if pat_binds_target(&binder, target) {
                body
            } else {
                Box::new(rewrite(*body, target, row_binders, eager))
            };
            Expr::Destructure {
                binder,
                value: new_value,
                body: new_body,
            }
        }
        Expr::If { cond, then_, else_ } => Expr::If {
            cond: Box::new(rewrite(*cond, target, row_binders, eager)),
            then_: Box::new(rewrite(*then_, target, row_binders, eager)),
            else_: Box::new(rewrite(*else_, target, row_binders, eager)),
        },
        Expr::Match(m) => Expr::Match(m.map_bodies(
            |scrutinee| rewrite(scrutinee, target, row_binders, eager),
            // An arm body may be emitted inside a deferred `move` thunk, so
            // arm bodies and guards are never eager positions.
            |pat, body, guard| {
                let binds = pat_binds_target(pat, target);
                let new_body = if binds {
                    body
                } else {
                    rewrite(body, target, row_binders, false)
                };
                // Preserve the list-length guard, rewriting it too when the arm
                // pattern does not bind `target`.
                let new_guard = guard.map(|g| {
                    if binds {
                        g
                    } else {
                        rewrite(g, target, row_binders, false)
                    }
                });
                (new_body, new_guard)
            },
        )),
        Expr::Call {
            callee,
            args,
            pin,
            on_form,
        } => {
            // A kernel or FFI emitter may wrap an argument in a deferred `move`
            // closure; only a user function's arguments are evaluated in place.
            let args_eager = eager && matches!(callee, Callee::Func(_));
            Expr::Call {
                callee,
                args: args
                    .into_iter()
                    .map(|a| rewrite(a, target, row_binders, args_eager))
                    .collect(),
                pin,
                on_form,
            }
        }
        Expr::Tuple(items) => Expr::Tuple(
            items
                .into_iter()
                .map(|e| rewrite(e, target, row_binders, eager))
                .collect(),
        ),
        Expr::List { elem, items } => Expr::List {
            elem,
            items: items
                .into_iter()
                .map(|e| rewrite(e, target, row_binders, eager))
                .collect(),
        },
        Expr::Cons { head, tail } => Expr::Cons {
            head: Box::new(rewrite(*head, target, row_binders, eager)),
            tail: Box::new(rewrite(*tail, target, row_binders, eager)),
        },
        Expr::ListIndexClone { list, index } => Expr::ListIndexClone {
            list: Box::new(borrow_base(*list, target, row_binders, eager)),
            index,
        },
        Expr::ListLenCheck { list, len, exact } => Expr::ListLenCheck {
            list: Box::new(borrow_base(*list, target, row_binders, eager)),
            len,
            exact,
        },
        Expr::Record { fields, ty } => Expr::Record {
            fields: fields
                .into_iter()
                .map(|(s, e)| (s, rewrite(e, target, row_binders, eager)))
                .collect(),
            ty,
        },
        Expr::Access {
            record,
            field,
            field_ty,
        } => {
            // A row-generic Access receiver stays a bare `Var`: the witness
            // getter `ipe_<field>()` BORROWS, so a whole-row `CloneVar` here is
            // both spurious and unroutable (the emitter's getter route matches
            // `Var` alone). Leaving it `Var(row)` is what upholds the invariant
            // uniformly at emit time. An eager read of `target` itself also
            // borrows (see [`borrow_base`]).
            let new_record = match *record {
                Expr::Var(s) if row_binders.contains(&s) => Expr::Var(s),
                other => borrow_base(other, target, row_binders, eager),
            };
            Expr::Access {
                record: Box::new(new_record),
                field,
                field_ty,
            }
        }
        Expr::Update { record, fields } => Expr::Update {
            record: Box::new(rewrite(*record, target, row_binders, eager)),
            fields: fields
                .into_iter()
                .map(|(s, e)| (s, rewrite(e, target, row_binders, eager)))
                .collect(),
        },
        Expr::Lambda { params, ret, body } => {
            let new_body = if params.iter().any(|(s, _)| *s == target) {
                body
            } else {
                Box::new(rewrite(*body, target, row_binders, false))
            };
            Expr::Lambda {
                params,
                ret,
                body: new_body,
            }
        }
        Expr::SharedLambda { params, ret, body } => {
            let new_body = if params.iter().any(|(s, _)| *s == target) {
                body
            } else {
                Box::new(rewrite(*body, target, row_binders, false))
            };
            Expr::SharedLambda {
                params,
                ret,
                body: new_body,
            }
        }
        Expr::Apply { func, args } => Expr::Apply {
            func: Box::new(rewrite(*func, target, row_binders, eager)),
            args: args
                .into_iter()
                .map(|a| rewrite(a, target, row_binders, eager))
                .collect(),
        },
        Expr::TaskSeq { effect, rest } => Expr::TaskSeq {
            effect: Box::new(rewrite(*effect, target, row_binders, eager)),
            // `rest` runs inside the emitted `move |_| { … }` continuation.
            rest: Box::new(rewrite(*rest, target, row_binders, false)),
        },
        Expr::Ctor {
            home,
            ty,
            variant,
            args,
        } => Expr::Ctor {
            home,
            ty,
            variant,
            args: args
                .into_iter()
                .map(|a| rewrite(a, target, row_binders, eager))
                .collect(),
        },
        Expr::TailLoop { params, body } => {
            let new_body = if params.iter().any(|(s, _)| *s == target) {
                body
            } else {
                Box::new(rewrite(*body, target, row_binders, false))
            };
            Expr::TailLoop {
                params,
                body: new_body,
            }
        }
        Expr::TailRecur { args } => Expr::TailRecur {
            args: args
                .into_iter()
                .map(|a| rewrite(a, target, row_binders, eager))
                .collect(),
        },
    }
}

/// Fold [`clone_free_target`] over every symbol in `targets`.
///
/// Each fold step only ever rewrites bare `Var` occurrences into `CloneVar` — the passes
/// don't interfere with each other regardless of order (a `CloneVar` leaf is
/// never re-matched by a later target's pass).
///
/// `row_binders` is the enclosing function's set of row-generic parameter
/// binders. A row-generic Access receiver is left a bare `Var` (never cloned)
/// so the borrowing witness getter still routes — see [`clone_free_target`].
#[must_use]
pub fn clone_targets_in_expr(
    expr: Expr,
    targets: &BTreeSet<Symbol>,
    row_binders: &BTreeSet<Symbol>,
) -> Expr {
    targets
        .iter()
        .fold(expr, |e, &t| clone_free_target(e, t, row_binders))
}
/// Does the sequenced-task capture-clone rewrite clone `sym` anywhere in `expr`?
///
/// Mirrors the emitter exactly: at every `TaskSeq { effect, rest }` and every
/// `Task.andThen cont effect` where `sym` is free in the continuation, the
/// emitter rewrites `effect` with [`clone_free_target`]; a `CloneVar(sym)` in
/// that result renders `sym.clone()`. For a value with no `Clone` impl that is
/// an exit-0-then-cargo-fail, so the lowerer refuses it. A `let` the emitter
/// inlines is walked in its inlined form. The walk skips every subtree where a
/// binder shadows `sym`.
#[must_use]
pub fn seq_rewrite_clones_symbol(sym: Symbol, expr: &Expr) -> bool {
    let no_rows = BTreeSet::new();
    let clones_in_effect = |effect: &Expr, continuation: &Expr| {
        let (uses, cloned) = scan_free_target(continuation, sym);
        (uses > 0 || cloned)
            && scan_free_target(&clone_free_target(effect.clone(), sym, &no_rows), sym).1
    };
    let walk = |e: &Expr| seq_rewrite_clones_symbol(sym, e);
    match expr {
        Expr::Var(_)
        | Expr::CloneVar(_)
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::PathLit(_)
        | Expr::CustomElementRef { .. }
        | Expr::Char(_)
        | Expr::Unit
        | Expr::FuncValue { .. } => false,
        Expr::TaskSeq { effect, rest } => {
            clones_in_effect(effect, rest) || walk(effect) || walk(rest)
        }
        Expr::Call { callee, args, .. } => {
            let and_then_hazard = matches!(callee, Callee::Kernel(KernelFn::TaskAndThen))
                && matches!(args.as_slice(), [cont, effect] if clones_in_effect(effect, cont));
            and_then_hazard || args.iter().any(walk)
        }
        Expr::Ctor { args, .. } | Expr::TailRecur { args } => args.iter().any(walk),
        Expr::BinOp { lhs, rhs, .. } => walk(lhs) || walk(rhs),
        Expr::Let { name, value, body } => inlined_let_body(*name, value, body).map_or_else(
            || walk(value) || (*name != sym && walk(body)),
            |inlined| walk(&inlined),
        ),
        Expr::Destructure {
            binder,
            value,
            body,
        } => walk(value) || (!pat_binds_target(binder, sym) && walk(body)),
        Expr::If { cond, then_, else_ } => walk(cond) || walk(then_) || walk(else_),
        Expr::Match(m) => {
            walk(m.scrutinee())
                || m.arms().iter().any(|arm| {
                    !pat_binds_target(&arm.pat, sym)
                        && (walk(&arm.body) || arm.guard.as_ref().is_some_and(walk))
                })
        }
        Expr::Tuple(items) | Expr::List { items, .. } => items.iter().any(walk),
        Expr::Cons { head, tail } => walk(head) || walk(tail),
        Expr::ListIndexClone { list, .. } | Expr::ListLenCheck { list, .. } => walk(list),
        Expr::Record { fields, .. } => fields.iter().any(|(_, e)| walk(e)),
        Expr::Access { record, .. } => walk(record),
        Expr::Update { record, fields } => walk(record) || fields.iter().any(|(_, e)| walk(e)),
        Expr::Lambda { params, body, .. }
        | Expr::SharedLambda { params, body, .. }
        | Expr::TailLoop { params, body } => !params.iter().any(|(s, _)| *s == sym) && walk(body),
        Expr::Apply { func, args } => walk(func) || args.iter().any(walk),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use ipe_intern::{Interner, Symbol};

    use super::{clone_free_target, seq_rewrite_clones_symbol};
    use crate::{CallPin, Callee, Expr, FuncId, IrType, KernelFn, OnFormKind};

    fn call(callee: Callee, args: Vec<Expr>) -> Expr {
        Expr::Call {
            callee,
            args,
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }
    }

    fn read(record: Expr, field: Symbol) -> Expr {
        Expr::Access {
            record: Box::new(record),
            field,
            field_ty: IrType::Int,
        }
    }

    fn user(args: Vec<Expr>) -> Expr {
        call(Callee::Func(FuncId::from_raw(0)), args)
    }

    fn kernel(args: Vec<Expr>) -> Expr {
        call(Callee::Kernel(KernelFn::StringAppend), args)
    }

    fn seq(effect: Expr, rest: Expr) -> Expr {
        Expr::TaskSeq {
            effect: Box::new(effect),
            rest: Box::new(rest),
        }
    }

    fn thunk(body: Expr) -> Expr {
        Expr::Lambda {
            params: Vec::new(),
            ret: IrType::Int,
            body: Box::new(body),
        }
    }

    fn symbols() -> (Symbol, Symbol) {
        let mut interner = Interner::new();
        let w = interner.intern("w").expect("intern");
        let tag = interner.intern("tag").expect("intern");
        (w, tag)
    }

    /// An eager field read keeps a borrow; a deferred one clones the carrier.
    #[test]
    fn eager_field_read_borrows_deferred_read_clones() {
        let (w, tag) = symbols();
        let rows = BTreeSet::new();
        let rewrite = |e: Expr| clone_free_target(e, w, &rows);
        let borrowed = || read(Expr::Var(w), tag);
        let cloned = || read(Expr::CloneVar(w), tag);

        assert_eq!(rewrite(user(vec![borrowed()])), user(vec![borrowed()]));
        assert_eq!(rewrite(kernel(vec![borrowed()])), kernel(vec![cloned()]));
        assert_eq!(rewrite(thunk(borrowed())), thunk(cloned()));
        assert_eq!(
            rewrite(user(vec![Expr::Var(w)])),
            user(vec![Expr::CloneVar(w)])
        );
    }

    /// The hazard check flags exactly the sequenced tasks whose rewrite clones.
    #[test]
    fn seq_hazard_tracks_the_emitted_rewrite() {
        let (w, tag) = symbols();
        let borrowed = || read(Expr::Var(w), tag);
        let consume = || user(vec![Expr::Var(w)]);
        let hazard = |e: &Expr| seq_rewrite_clones_symbol(w, e);

        assert!(!hazard(&seq(user(vec![borrowed()]), consume())));
        assert!(hazard(&seq(kernel(vec![borrowed()]), consume())));
        assert!(hazard(&seq(consume(), consume())));
        assert!(!hazard(&seq(kernel(vec![borrowed()]), Expr::Unit)));
        let shadowed_rest = Expr::Lambda {
            params: vec![(w, IrType::Int)],
            ret: IrType::Int,
            body: Box::new(consume()),
        };
        assert!(!hazard(&seq(kernel(vec![borrowed()]), shadowed_rest)));
        let and_then = call(
            Callee::Kernel(KernelFn::TaskAndThen),
            vec![thunk(consume()), kernel(vec![borrowed()])],
        );
        assert!(hazard(&and_then));
    }
}

//! Binder-aware free-variable analysis over the typed IR, shared by the backend
//! and the lowerer.
//!
//! The emitter decides which captures to clone from these sets, and the
//! lowerer's `IPE-L0135` gate must mirror that decision, so both read this one
//! implementation.

use std::collections::BTreeSet;

use ipe_intern::Symbol;

use crate::{Expr, Pat};

/// Collect every symbol a pattern binds (recursively) into `out`.
///
/// Same traversal shape as [`crate::let_inline::pat_binds_target`], gathering
/// the full bound set in one pass rather than testing one target.
pub fn pat_bound_symbols(pat: &Pat, out: &mut BTreeSet<Symbol>) {
    match pat {
        Pat::Var(s) => {
            out.insert(*s);
        }
        Pat::Wildcard | Pat::Int(_) | Pat::Bool(_) | Pat::Char(_) | Pat::Str(_) => {}
        Pat::Alias(inner, s) => {
            out.insert(*s);
            pat_bound_symbols(inner, out);
        }
        Pat::Ctor { args, .. } => {
            for p in args {
                pat_bound_symbols(p, out);
            }
        }
        Pat::Tuple(elems) => {
            for p in elems {
                pat_bound_symbols(p, out);
            }
        }
        Pat::Record(fields) => {
            for (_, p) in fields {
                pat_bound_symbols(p, out);
            }
        }
        Pat::Slice { prefix, rest } => {
            for p in prefix {
                pat_bound_symbols(p, out);
            }
            if let Some(p) = rest {
                pat_bound_symbols(p, out);
            }
        }
        // Every alternative of an or-pattern binds the same names, so the first
        // alternative's binders are the whole set.
        Pat::Or(alts) => {
            if let Some(first) = alts.first() {
                pat_bound_symbols(first, out);
            }
        }
    }
}

/// The set of symbols that occur free in `expr`.
///
/// Every binder (`Let`/`Destructure`/`Lambda`/`TailLoop`/`Match` arm
/// patterns) removes its bound name(s) from the free set of the scope it
/// introduces. A `Match` arm guard counts as part of its arm. Both `Var` and
/// `CloneVar` occurrences are free uses. String literals and record field
/// names are opaque leaves, never mistaken for a variable occurrence.
#[must_use]
pub fn free_vars(expr: &Expr) -> BTreeSet<Symbol> {
    let mut out = BTreeSet::new();
    collect_free_vars(expr, &mut out);
    out
}

/// Add the free symbols of `expr` to `out` (see [`free_vars`]).
#[allow(clippy::too_many_lines)] // A recursive tree-walk over a large enum — necessarily long.
pub fn collect_free_vars(expr: &Expr, out: &mut BTreeSet<Symbol>) {
    match expr {
        Expr::Int(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::PathLit(_)
        | Expr::CustomElementRef { .. }
        | Expr::Char(_)
        | Expr::Unit
        | Expr::FuncValue { .. } => {}
        Expr::Var(s) | Expr::CloneVar(s) => {
            out.insert(*s);
        }
        Expr::Ctor { args, .. } | Expr::Call { args, .. } | Expr::TailRecur { args } => {
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::BinOp { lhs, rhs, .. } => {
            collect_free_vars(lhs, out);
            collect_free_vars(rhs, out);
        }
        Expr::Let { name, value, body } => {
            collect_free_vars(value, out);
            let mut body_free = BTreeSet::new();
            collect_free_vars(body, &mut body_free);
            body_free.remove(name);
            out.extend(body_free);
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            collect_free_vars(value, out);
            let mut bound = BTreeSet::new();
            pat_bound_symbols(binder, &mut bound);
            let mut body_free = BTreeSet::new();
            collect_free_vars(body, &mut body_free);
            for b in &bound {
                body_free.remove(b);
            }
            out.extend(body_free);
        }
        Expr::If { cond, then_, else_ } => {
            collect_free_vars(cond, out);
            collect_free_vars(then_, out);
            collect_free_vars(else_, out);
        }
        Expr::Match(m) => {
            collect_free_vars(m.scrutinee(), out);
            for arm in m.arms() {
                let mut bound = BTreeSet::new();
                pat_bound_symbols(&arm.pat, &mut bound);
                let mut body_free = BTreeSet::new();
                collect_free_vars(&arm.body, &mut body_free);
                if let Some(guard) = &arm.guard {
                    collect_free_vars(guard, &mut body_free);
                }
                for b in &bound {
                    body_free.remove(b);
                }
                out.extend(body_free);
            }
        }
        Expr::Tuple(items) | Expr::List { items, .. } => {
            for e in items {
                collect_free_vars(e, out);
            }
        }
        Expr::Cons { head, tail } => {
            collect_free_vars(head, out);
            collect_free_vars(tail, out);
        }
        Expr::ListIndexClone { list, .. } | Expr::ListLenCheck { list, .. } => {
            collect_free_vars(list, out);
        }
        Expr::Record { fields, .. } => {
            for (_, e) in fields {
                collect_free_vars(e, out);
            }
        }
        Expr::Access { record, .. } => collect_free_vars(record, out),
        Expr::Update { record, fields } => {
            collect_free_vars(record, out);
            for (_, e) in fields {
                collect_free_vars(e, out);
            }
        }
        Expr::Lambda { params, body, .. }
        | Expr::SharedLambda { params, body, .. }
        | Expr::TailLoop { params, body } => {
            let mut body_free = BTreeSet::new();
            collect_free_vars(body, &mut body_free);
            for (s, _) in params {
                body_free.remove(s);
            }
            out.extend(body_free);
        }
        Expr::Apply { func, args } => {
            collect_free_vars(func, out);
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::TaskSeq { effect, rest } => {
            collect_free_vars(effect, out);
            collect_free_vars(rest, out);
        }
    }
}

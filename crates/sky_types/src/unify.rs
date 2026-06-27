//! Unification of two solver variables, ported from the M0-relevant arms of
//! `Sky.Type.Unify` (derivative of elm/compiler's `Type.Unify`, BSD-3-Clause).
//!
//! Eager, in-place unification over the union-find arena. A flexible variable
//! adopts the other side's content; two structures must agree head-to-head and
//! then recurse on their children. A flexible variable bound to a structure
//! that *contains* it is an infinite type — rejected via an occurs check
//! (mirrors `Occurs.occurs`), so the read-back ([`crate::ty`] zonking) always
//! terminates.
//!
//! Every step decrements the shared [`Budget`]; an adversarial constraint set
//! that drives the unifier into a blow-up trips [`TypeError::BudgetExceeded`]
//! instead of exhausting the heap.

use sky_diagnostics::{DResult, Diagnostic, Span, TypeError};

use crate::solve::Budget;
use crate::ty::{Content, FlatType};
use crate::unionfind::{UnionFind, VarId};

/// Maximum structural depth an occurs check will walk before declaring the
/// candidate cyclic. Bounds recursion on adversarial input (belt-and-braces
/// alongside the budget).
const OCCURS_DEPTH_LIMIT: u32 = 10_000;

/// Unify the types of variables `a` and `b` in place.
///
/// `span` is attached to any [`TypeError::Mismatch`] for diagnostics.
///
/// # Errors
/// * [`TypeError::Mismatch`] when two incompatible structures meet, or when a
///   bind would create an infinite type.
/// * [`TypeError::BudgetExceeded`] when the step budget is exhausted.
/// * [`Diagnostic::CompilerBug`] on a union-find invariant violation.
pub fn unify(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    span: Span,
    a: VarId,
    b: VarId,
) -> DResult<()> {
    budget.tick()?;
    if uf.equivalent(a, b)? {
        return Ok(());
    }
    let ra = uf.find(a)?;
    let rb = uf.find(b)?;
    let ca = uf.content(ra)?;
    let cb = uf.content(rb)?;
    match (ca, cb) {
        // Two flexes collapse into one flex class.
        (Content::Flex, Content::Flex) => uf.union(ra, rb, Content::Flex),
        // A flex adopts the other side's structure (occurs-checked).
        (Content::Flex, structure @ Content::Structure(_)) => {
            occurs_guard(uf, budget, ra, rb)?;
            uf.union(ra, rb, structure)
        }
        (structure @ Content::Structure(_), Content::Flex) => {
            occurs_guard(uf, budget, rb, ra)?;
            uf.union(ra, rb, structure)
        }
        (Content::Structure(fa), Content::Structure(fb)) => {
            unify_flat(uf, budget, span, ra, rb, fa, fb)
        }
    }
}

/// Unify two concrete structures that share representatives `ra` / `rb`.
fn unify_flat(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    span: Span,
    ra: VarId,
    rb: VarId,
    fa: FlatType,
    fb: FlatType,
) -> DResult<()> {
    match (fa, fb) {
        (FlatType::Unit, FlatType::Unit) => uf.union(ra, rb, Content::Structure(FlatType::Unit)),
        (FlatType::Fun(a1, r1), FlatType::Fun(a2, r2)) => {
            // Merge the roots first so a recursive reference resolves, then
            // unify argument-with-argument and result-with-result.
            uf.union(ra, rb, Content::Structure(FlatType::Fun(a1, r1)))?;
            unify(uf, budget, span, a1, a2)?;
            unify(uf, budget, span, r1, r2)
        }
        (
            FlatType::Con {
                module: m1,
                name: n1,
                args: as1,
            },
            FlatType::Con {
                module: m2,
                name: n2,
                args: as2,
            },
        ) => {
            if m1 != m2 || n1 != n2 || as1.len() != as2.len() {
                return Err(mismatch(span));
            }
            uf.union(
                ra,
                rb,
                Content::Structure(FlatType::Con {
                    module: m1,
                    name: n1,
                    args: as1.clone(),
                }),
            )?;
            for (x, y) in as1.iter().zip(as2.iter()) {
                unify(uf, budget, span, *x, *y)?;
            }
            Ok(())
        }
        _ => Err(mismatch(span)),
    }
}

/// Reject binding flexible `var` to `structure` if `var` occurs inside it.
fn occurs_guard(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    var: VarId,
    structure: VarId,
) -> DResult<()> {
    let target = uf.find(var)?;
    if occurs(uf, budget, target, structure, OCCURS_DEPTH_LIMIT)? {
        Err(mismatch(Span::DUMMY))
    } else {
        Ok(())
    }
}

/// Whether `target`'s representative appears within the structure rooted at
/// `node`. Depth-bounded to keep adversarial inputs from unbounded recursion.
fn occurs(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    target: VarId,
    node: VarId,
    depth: u32,
) -> DResult<bool> {
    budget.tick()?;
    if depth == 0 {
        // Treat exhaustion as "possibly cyclic" — conservative, rejects.
        return Ok(true);
    }
    let here = uf.find(node)?;
    if here == target {
        return Ok(true);
    }
    match uf.content(here)? {
        Content::Flex | Content::Structure(FlatType::Unit) => Ok(false),
        Content::Structure(FlatType::Fun(a, r)) => {
            Ok(occurs(uf, budget, target, a, depth - 1)?
                || occurs(uf, budget, target, r, depth - 1)?)
        }
        Content::Structure(FlatType::Con { args, .. }) => {
            for arg in args {
                if occurs(uf, budget, target, arg, depth - 1)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

const fn mismatch(span: Span) -> Diagnostic {
    Diagnostic::Type {
        span,
        msg: TypeError::Mismatch,
    }
}

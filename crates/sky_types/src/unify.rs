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
//! that drives the unifier into a blow-up trips [`TypeError::StepBudgetExceeded`]
//! instead of exhausting the heap.
//!
//! Parse-don't-validate: a type error is built into an **owned** diagnostic
//! payload at the failure point — the diverging types are zonked + resolved into
//! [`TyDoc`](sky_diagnostics::TyDoc)s here (via [`crate::doc`]), so the reporter
//! never touches the interner or the arena.

use sky_diagnostics::{DResult, Diagnostic, Span, TypeError};
use sky_intern::Interner;

use crate::constrain::zonk;
use crate::doc::{VarNamer, ty_to_doc};
use crate::solve::Budget;
use crate::ty::{Content, FlatType};
use crate::unionfind::{UnionFind, VarId};

/// Unify the types of variables `a` and `b` in place.
///
/// `span` is attached to any [`TypeError::TypeMismatch`] for diagnostics; `a` is
/// the *found* (actual) side and `b` the *expected* side, matching the
/// `Constraint { lhs, rhs }` convention used by the constraint generator.
///
/// # Errors
/// * [`TypeError::TypeMismatch`] when two incompatible structures meet.
/// * [`TypeError::InfiniteType`] when a bind would create a cyclic type.
/// * [`TypeError::StepBudgetExceeded`] when the step budget is exhausted.
/// * [`Diagnostic::CompilerBug`] on a union-find invariant violation.
pub fn unify(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    interner: &Interner,
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
            occurs_guard(uf, budget, interner, span, ra, rb)?;
            uf.union(ra, rb, structure)
        }
        (structure @ Content::Structure(_), Content::Flex) => {
            occurs_guard(uf, budget, interner, span, rb, ra)?;
            uf.union(ra, rb, structure)
        }
        // A flex adopts the other side's rigid (skolem). No occurs check is
        // needed: a rigid carries no transitive structure, so the merge cannot
        // build a cycle. Mirrors the Haskell `(FlexVar, _)` / `(_, FlexVar)` arms.
        (Content::Flex, Content::Rigid) | (Content::Rigid, Content::Flex) => {
            uf.union(ra, rb, Content::Rigid)
        }
        // A rigid unifies only with itself (caught by the `equivalent` check
        // above) or with a flex (handled above). Against a concrete structure or
        // a *different* rigid it is a mismatch — the annotation promised a fully
        // parametric variable the body is now trying to pin down. Mirrors the
        // Haskell `(RigidVar _, _)` / `(_, RigidVar _)` reject arms.
        (Content::Rigid, _) | (_, Content::Rigid) => {
            Err(mismatch(uf, budget, interner, span, ra, rb))
        }
        (Content::Structure(fa), Content::Structure(fb)) => {
            unify_flat(uf, budget, interner, span, ra, rb, fa, fb)
        }
    }
}

/// Unify two concrete structures that share representatives `ra` (found) / `rb`
/// (expected).
#[allow(clippy::too_many_arguments)]
fn unify_flat(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    interner: &Interner,
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
            unify(uf, budget, interner, span, a1, a2)?;
            unify(uf, budget, interner, span, r1, r2)
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
                return Err(mismatch(uf, budget, interner, span, ra, rb));
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
                unify(uf, budget, interner, span, *x, *y)?;
            }
            Ok(())
        }
        (FlatType::Tuple(es1), FlatType::Tuple(es2)) => {
            // Tuples unify only at the same arity, element-wise. Merge the roots
            // first so a recursive reference resolves before the children unify.
            if es1.len() != es2.len() {
                return Err(mismatch(uf, budget, interner, span, ra, rb));
            }
            uf.union(ra, rb, Content::Structure(FlatType::Tuple(es1.clone())))?;
            for (x, y) in es1.iter().zip(es2.iter()) {
                unify(uf, budget, interner, span, *x, *y)?;
            }
            Ok(())
        }
        (FlatType::Record(fs1), FlatType::Record(fs2)) => {
            // Closed records unify only when their field-name SETS are identical,
            // then field-by-field. A differing field set (a missing or extra
            // field) is a mismatch — there is no row variable to absorb it. Both
            // maps are keyed by `Symbol`, so equal key sequences mean equal sets.
            if !fs1.keys().eq(fs2.keys()) {
                return Err(mismatch(uf, budget, interner, span, ra, rb));
            }
            uf.union(ra, rb, Content::Structure(FlatType::Record(fs1.clone())))?;
            for (name, v1) in &fs1 {
                // Present in `fs2` because the key sets are equal.
                if let Some(v2) = fs2.get(name) {
                    unify(uf, budget, interner, span, *v1, *v2)?;
                }
            }
            Ok(())
        }
        _ => Err(mismatch(uf, budget, interner, span, ra, rb)),
    }
}

/// Reject binding flexible `var` to `structure` if `var` occurs inside it,
/// surfacing an owned [`TypeError::InfiniteType`] at the real `span`.
fn occurs_guard(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    interner: &Interner,
    span: Span,
    var: VarId,
    structure: VarId,
) -> DResult<()> {
    let target = uf.find(var)?;
    if occurs(uf, budget, target, structure)? {
        Err(infinite_type(uf, budget, interner, span, target, structure))
    } else {
        Ok(())
    }
}

/// Whether `target`'s representative appears within the structure rooted at
/// `node`.
///
/// **Iterative.** Walks an explicit heap-allocated work stack instead of
/// recursing, so adversarial nesting cannot overflow the native stack; the
/// shared [`Budget`] (ticked per node) bounds total work. The structure is
/// acyclic at every call site (occurs runs *before* the bind that could create
/// a cycle), so the walk terminates.
fn occurs(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    target: VarId,
    node: VarId,
) -> DResult<bool> {
    let mut stack: Vec<VarId> = vec![node];
    while let Some(n) = stack.pop() {
        budget.tick()?;
        let here = uf.find(n)?;
        if here == target {
            return Ok(true);
        }
        match uf.content(here)? {
            // Leaves: a flexible or rigid variable and `Unit` carry no children.
            Content::Flex | Content::Rigid | Content::Structure(FlatType::Unit) => {}
            Content::Structure(FlatType::Fun(a, r)) => {
                stack.push(a);
                stack.push(r);
            }
            Content::Structure(FlatType::Con { args, .. }) => {
                for arg in args {
                    stack.push(arg);
                }
            }
            Content::Structure(FlatType::Tuple(elems)) => {
                for elem in elems {
                    stack.push(elem);
                }
            }
            Content::Structure(FlatType::Record(fields)) => {
                for v in fields.values() {
                    stack.push(*v);
                }
            }
        }
    }
    Ok(false)
}

/// Build an owned [`TypeError::TypeMismatch`] from the two diverging variables.
/// `found` is the actual type, `expected` the wanted one; both are zonked +
/// rendered through a shared [`VarNamer`] so a shared variable reads identically
/// on both sides. If the read-back itself hits an internal invariant, that bug
/// is surfaced instead of the mismatch.
fn mismatch(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    interner: &Interner,
    span: Span,
    found: VarId,
    expected: VarId,
) -> Diagnostic {
    let mut namer = VarNamer::new();
    let expected_ty = match zonk(uf, budget, expected) {
        Ok(t) => t,
        Err(bug) => return bug,
    };
    let found_ty = match zonk(uf, budget, found) {
        Ok(t) => t,
        Err(bug) => return bug,
    };
    let expected_doc = match ty_to_doc(&expected_ty, interner, &mut namer) {
        Ok(d) => d,
        Err(bug) => return bug,
    };
    let found_doc = match ty_to_doc(&found_ty, interner, &mut namer) {
        Ok(d) => d,
        Err(bug) => return bug,
    };
    Diagnostic::Type {
        span,
        msg: TypeError::TypeMismatch {
            expected: Box::new(expected_doc),
            found: Box::new(found_doc),
            definition: None,
            path: Box::new([]),
        },
    }
}

/// Build an owned [`TypeError::InfiniteType`] naming the offending variable and
/// the structure it would have to equal.
fn infinite_type(
    uf: &mut UnionFind<Content>,
    budget: &mut Budget,
    interner: &Interner,
    span: Span,
    var: VarId,
    structure: VarId,
) -> Diagnostic {
    let mut namer = VarNamer::new();
    // Name the variable first so it reads as `a` and any same var inside the
    // structure renders consistently.
    let var_ty = match zonk(uf, budget, var) {
        Ok(t) => t,
        Err(bug) => return bug,
    };
    let var_name = match ty_to_doc(&var_ty, interner, &mut namer) {
        Ok(sky_diagnostics::TyDoc::Var(v)) => v,
        // The occurs target is always flexible at the guard site; any other
        // shape (or a read-back bug) is an internal invariant violation.
        Ok(_) => Box::from("?"),
        Err(bug) => return bug,
    };
    let structure_ty = match zonk(uf, budget, structure) {
        Ok(t) => t,
        Err(bug) => return bug,
    };
    let structure_doc = match ty_to_doc(&structure_ty, interner, &mut namer) {
        Ok(d) => d,
        Err(bug) => return bug,
    };
    Diagnostic::Type {
        span,
        msg: TypeError::InfiniteType {
            var: var_name,
            ty: Box::new(structure_doc),
        },
    }
}

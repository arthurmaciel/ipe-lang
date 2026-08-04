//! Unification of two solver variables, ported from the relevant arms of
//! `Ipe.Type.Unify` (derivative of elm/compiler's `Type.Unify`, BSD-3-Clause).
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
//! [`TyDoc`](ipe_diagnostics::TyDoc)s here (via [`crate::doc`]), so the reporter
//! never touches the interner or the arena.

use ipe_diagnostics::{DResult, Diagnostic, Span, TypeError};
use ipe_intern::Interner;

use crate::constrain::zonk;
use crate::doc::{VarNamer, ty_to_doc};
use crate::solve::Budget;
use crate::ty::{Content, FlatType, Ty, TyBounds};
use crate::unionfind::{UnionFind, VarId};

/// Whether a concrete structure `flat` satisfies a variable's super-type
/// obligations `bounds` at the head.
///
/// This is the *pin* check: a super-typed flex variable may collapse onto a
/// concrete type only when that type really supports the operations the body
/// performed. Numeric obligations (`+ - *`) are met by `Int` / `Float`;
/// ordering (`< > <= >=`) is met by the scalar primitives `Int` / `Float` /
/// `Char` / `String` / `Bool` — both require a bare type constructor.
///
/// Equality (`== /=`) is far more permissive: structural equality is total over
/// every non-function type, so an equality obligation pins to any structure that
/// is not a function head (tuples, records, enums, and the primitives all derive
/// Rust's `PartialEq`). A *nested* function inside such a structure still makes
/// it non-equatable, but that is caught by the post-solve deep gate
/// ([`crate::concrete_super_ok`]), which has the resolved type in hand; here at
/// the head a function is the only outright rejection for equality.
fn super_concrete_ok(interner: &Interner, bounds: TyBounds, flat: &FlatType) -> bool {
    // A function supports none of the super-types: not numeric, not ordered, and
    // not equatable (Rust never derives `PartialEq` for a function).
    if matches!(flat, FlatType::Fun(_, _)) {
        return false;
    }
    // Numeric / ordering need a bare scalar primitive. Equality imposes no head
    // restriction beyond the non-function rejection above.
    let prim = match flat {
        FlatType::Con { module, name, args } if module.is_empty() && args.is_empty() => {
            interner.resolve(*name)
        }
        _ => None,
    };
    let number_ok = matches!(prim, Some("Int" | "Float"));
    let ord_ok = matches!(prim, Some("Int" | "Float" | "Char" | "String" | "Bool"));
    // `++` accepts `String` (bare scalar) or `List _` (one type arg). The `prim`
    // path already covers `String`; `List` must be checked separately because it
    // carries one argument and the `args.is_empty()` guard above excludes it.
    let appendable_ok = matches!(prim, Some("String"))
        || matches!(flat,
            FlatType::Con { module, name, args }
                if module.is_empty()
                    && args.len() == 1
                    && interner.resolve(*name) == Some("List")
        );
    // A `Set` element / `Dict` key obligation is a Ipê `comparable` — the same
    // scalar set ordering admits. `Float` passes here (it IS `comparable` in
    // Ipê, so the typing follows Ipê), and the Rust-backend reality that `f64`
    // is neither `Ord` nor `Hash` is enforced at lowering with a dedicated
    // diagnostic — not as a confusing type mismatch at this head pin.
    (!bounds.has_number() || number_ok)
        && (!bounds.has_ord() || ord_ok)
        && (!bounds.has_comparable_key() || ord_ok)
        && (!bounds.has_append() || appendable_ok)
}

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
    // Two finds, reused for both the same-class short-circuit and the content
    // reads below (efficiency-audit §2 low: `equivalent(a, b)` + two more
    // finds performed up to 6 union-find traversals where 2 suffice).
    // `equivalent` is exactly `find(a)? == find(b)?`, so the short-circuit is
    // identical and path compression still runs on both chains.
    let ra = uf.find(a)?;
    let rb = uf.find(b)?;
    if ra == rb {
        return Ok(());
    }
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
        // A flex adopts a super-typed variable's obligations wholesale.
        (s @ Content::Super { .. }, Content::Flex) | (Content::Flex, s @ Content::Super { .. }) => {
            uf.union(ra, rb, s)
        }
        // Two super-typed variables meet.
        //
        // Same rigidity → merge, the survivor owing the union of both obligation
        // sets.  DIFFERENT rigidity (one an annotation skolem, the other an
        // inference flex) → mismatch, mirroring `Rigid` vs `Structure`.
        //
        // The rigid super is an annotation type variable the body promised to keep
        // fully generic (surfacing its obligation as a trait bound); a super-typed
        // *flex* is an inference variable that WILL resolve to a concrete number —
        // most often a numeric literal (`Super { Number }`).  Letting the two merge
        // would silently accept `f : a -> a; f x = x + 1`, where the literal `1`
        // forces the annotated-generic `a` to a concrete numeric representation.
        // Elm (and the Ipê reference) reject exactly this: `a` was annotated fully
        // parametric, so a body that adds a concrete literal to it is a type error.
        // (`double : a -> a; double x = x + x` — no literal — never reaches this
        // arm: its operand super meets the rigid `x` as `Super{flex}` vs
        // `Content::Rigid` in the arm below, which correctly adopts the obligation
        // as a bound.)
        (
            Content::Super {
                rigid: r1,
                bounds: b1,
            },
            Content::Super {
                rigid: r2,
                bounds: b2,
            },
        ) => {
            // Number (`+ - *`) and Append (`++`) require the variable to resolve
            // to a concrete type at monomorphisation / lowering. Letting either
            // cross onto a rigid annotation var would pin an annotation-generic
            // to a concrete representation — the exact unsoundness we guard
            // against. All other obligations (Eq, Ord, Stringify, comparable-key)
            // lower as pure generic trait bounds, always valid Rust, so they may
            // accumulate across rigidity and survive as trait bounds on the param.
            let concrete_dispatch =
                b1.has_number() || b2.has_number() || b1.has_append() || b2.has_append();
            let can_union = (r1 == r2) || !concrete_dispatch;
            if can_union {
                uf.union(
                    ra,
                    rb,
                    Content::Super {
                        rigid: r1 || r2,
                        bounds: b1.union(b2),
                    },
                )
            } else {
                Err(mismatch(uf, budget, interner, span, ra, rb))
            }
        }
        // A super-typed FLEX meeting a rigid skolem: the rigid adopts the
        // obligations and stays rigid, so the body's super-typed use of an
        // annotated `a` becomes a trait bound on `a` rather than a rejection.
        (
            Content::Super {
                rigid: false,
                bounds,
            },
            Content::Rigid,
        )
        | (
            Content::Rigid,
            Content::Super {
                rigid: false,
                bounds,
            },
        ) => uf.union(
            ra,
            rb,
            Content::Super {
                rigid: true,
                bounds,
            },
        ),
        // A super-typed variable meeting a concrete structure. A flex pins to the
        // structure when it satisfies the obligations; a rigid cannot be pinned
        // (the annotation promised a generic), and a structure that fails the
        // obligations is a mismatch either way.
        (Content::Super { rigid, bounds }, structure @ Content::Structure(_))
        | (structure @ Content::Structure(_), Content::Super { rigid, bounds }) => {
            let pins = match &structure {
                Content::Structure(flat) => !rigid && super_concrete_ok(interner, bounds, flat),
                _ => false,
            };
            if pins {
                uf.union(ra, rb, structure)
            } else {
                Err(mismatch(uf, budget, interner, span, ra, rb))
            }
        }
        // A rigid unifies only with itself (caught by the `equivalent` check
        // above) or with a flex (handled above). Against a concrete structure, a
        // *different* rigid, or a super-typed RIGID (which would conflate two
        // distinct annotation variables) it is a mismatch — the annotation
        // promised a fully parametric variable the body is now trying to pin
        // down. Mirrors the Haskell `(RigidVar _, _)` / `(_, RigidVar _)` reject
        // arms.
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
            // An empty module path (`module = []`) is used both for
            // kernel/builtin types and for user-defined type names that appear
            // in a module without a corresponding import (the canonicaliser
            // falls back to `unwrap_or_default()` → `[]` for unknown names).
            // Two `Con` nodes unify when they have the same name and arity; a
            // module conflict is only fatal when *both* sides carry a non-empty,
            // differing path.  Whichever side carries the more specific
            // (non-empty) path wins as the canonical representation — this
            // matches the Haskell oracle's behaviour.
            let modules_compat = m1 == m2 || m1.is_empty() || m2.is_empty();
            if !modules_compat || n1 != n2 || as1.len() != as2.len() {
                return Err(mismatch(uf, budget, interner, span, ra, rb));
            }
            // Prefer the non-empty (more specific) module path as canonical.
            // m1 and m2 are not used after this expression; move them directly.
            let canonical_module = if m1.is_empty() { m2 } else { m1 };
            uf.union(
                ra,
                rb,
                Content::Structure(FlatType::Con {
                    module: canonical_module,
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
        // ── Open-record unification (row-poly) ───────────────────────────────
        //
        // Faithful port of `unifyRecords` from
        // `../ipe/src/Ipe/Type/Unify.hs:468-512`.
        //
        // Algorithm (four cases):
        //   1. Unify every field present on BOTH sides pairwise.
        //   2. A CLOSED side cannot absorb the other's extra fields → mismatch
        //      (IPE-T0001).
        //   3. If both sides carry identical field sets → unify the two extension
        //      variables and merge.
        //   4. Both sides are open with differing fields → mint a fresh flex
        //      tail, merge as the union of both field maps under it (so future
        //      unifications can still constrain the merged open tail).
        //
        // `is_empty_record`: follow `v` to its root; `true` iff the content is
        // `FlatType::EmptyRecord` (the closed-tail sentinel).
        (FlatType::Record(fs1, ext1), FlatType::Record(fs2, ext2)) => {
            // Step 1 — unify shared fields pairwise.
            // Both `.get()` calls are infallible: `name` came from `fs1.keys()`
            // and the filter already proved `fs2.contains_key(name)`.
            for name in fs1.keys().filter(|k| fs2.contains_key(*k)) {
                // Unreachable else: both keys are confirmed present.
                let Some((v1, v2)) = fs1.get(name).copied().zip(fs2.get(name).copied()) else {
                    continue;
                };
                unify(uf, budget, interner, span, v1, v2)?;
            }

            // Partition into only-on-left and only-on-right.
            let only1: Vec<(_, _)> = fs1
                .iter()
                .filter(|(k, _)| !fs2.contains_key(*k))
                .map(|(k, v)| (*k, *v))
                .collect();
            let only2: Vec<(_, _)> = fs2
                .iter()
                .filter(|(k, _)| !fs1.contains_key(*k))
                .map(|(k, v)| (*k, *v))
                .collect();

            // Is the extension variable a closed tail (`EmptyRecord`)?
            let closed1 = is_empty_record(uf, ext1)?;
            let closed2 = is_empty_record(uf, ext2)?;

            // Step 2 — closed side cannot absorb the other's extras.
            let extras1_illegal = closed2 && !only1.is_empty();
            let extras2_illegal = closed1 && !only2.is_empty();
            if extras1_illegal || extras2_illegal {
                return Err(mismatch(uf, budget, interner, span, ra, rb));
            }

            if only1.is_empty() && only2.is_empty() {
                // Step 3 — identical field sets: merge first, then unify tails.
                // `fs1` is moved into the union; no clone needed.
                uf.union(ra, rb, Content::Structure(FlatType::Record(fs1, ext1)))?;
                unify(uf, budget, interner, span, ext1, ext2)?;
            } else {
                // Step 4 — both open, differing extras: mint a fresh flex tail
                // that absorbs any still-unspecified optional fields.
                let new_ext = uf.fresh(Content::Flex)?;
                // Build the merged field map (union of both sides).
                // Move `fs1` into `merged`; no clone needed since this branch
                // is mutually exclusive with step 3.
                let mut merged = fs1;
                for (k, v) in only2 {
                    merged.insert(k, v);
                }
                uf.union(
                    ra,
                    rb,
                    Content::Structure(FlatType::Record(merged, new_ext)),
                )?;
            }
            Ok(())
        }
        // Two closed-tail sentinels: identical structures, merge and succeed.
        // Mirrors the Haskell `(EmptyRecord1, EmptyRecord1) -> return ()` arm
        // in `../ipe/src/Ipe/Type/Unify.hs`. Without this arm both roots would
        // fall through to the wildcard mismatch, producing a spurious
        // "TypeMismatch { expected: Unit, found: Unit }" (zonk renders
        // EmptyRecord as Unit for display purposes).
        (FlatType::EmptyRecord, FlatType::EmptyRecord) => {
            uf.union(ra, rb, Content::Structure(FlatType::EmptyRecord))
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

/// Whether the extension variable `v` resolves to [`FlatType::EmptyRecord`]
/// (the closed-tail sentinel).
///
/// Mirrors `isClosedRecordExt` from `../ipe/src/Ipe/Type/Unify.hs:505`.
/// A record is closed iff this returns `true`; open iff it returns `false`
/// (the extension is still a flex variable or has been merged into another open
/// record).
fn is_empty_record(uf: &mut UnionFind<Content>, v: VarId) -> DResult<bool> {
    let root = uf.find(v)?;
    Ok(matches!(
        uf.content(root)?,
        Content::Structure(FlatType::EmptyRecord)
    ))
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
            // Leaves: a flexible, rigid, or super-typed variable, `Unit`, and the
            // `EmptyRecord` closed-tail sentinel carry no children.
            Content::Flex
            | Content::Rigid
            | Content::Super { .. }
            | Content::Structure(FlatType::Unit | FlatType::EmptyRecord) => {}
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
            Content::Structure(FlatType::Record(fields, ext)) => {
                for v in fields.values() {
                    stack.push(*v);
                }
                // Also walk the extension variable: a row tail that points back
                // to the record itself would be a cyclic open record.
                stack.push(ext);
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
    // A managed-loop `view` returns `Html` where the shape requires `Element`
    // (or the symmetric case): render the tailored IPE-T0020, which tells the
    // user to return an `Element` or wrap raw `Html` with `Ui.html`, instead of
    // the generic type-mismatch. `Ui.layout` / `Ui.layoutWith` turn an
    // `Element` into `Html`; the shape applies that wrapping itself, so a `view`
    // body ending in one is a first-order authoring mistake worth its own hint.
    if is_element_html_clash(interner, &expected_ty, &found_ty) {
        return Diagnostic::Type {
            span,
            msg: TypeError::WebViewReturnsHtml,
        };
    }
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

/// Whether the two diverging types are the `Element` / `Html` pair (in either
/// order) — a managed-loop `view` returning `Html` where an `Element` is
/// required (or the symmetric case). Both are unqualified nominal cons
/// (`self.builtins.element` / `html_con` carry no module for the emitted type),
/// so a name-string match is exact and cannot collide with a user type.
fn is_element_html_clash(interner: &Interner, a: &Ty, b: &Ty) -> bool {
    let con_name = |t: &Ty| match t {
        Ty::Con { name, .. } => interner.resolve(*name),
        _ => None,
    };
    matches!(
        (con_name(a), con_name(b)),
        (Some("Element"), Some("Html")) | (Some("Html"), Some("Element"))
    )
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
        Ok(ipe_diagnostics::TyDoc::Var(v)) => v,
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

#[cfg(test)]
mod tests {
    use ipe_diagnostics::Span;
    use ipe_intern::Interner;

    use super::*;
    use crate::solve::Budget;
    use crate::ty::TyBounds;
    use crate::unionfind::UnionFind;

    fn super_var(uf: &mut UnionFind<Content>, rigid: bool, bounds: TyBounds) -> VarId {
        uf.fresh(Content::Super { rigid, bounds })
            .expect("fresh var")
    }

    fn do_unify(
        uf: &mut UnionFind<Content>,
        a: VarId,
        b: VarId,
    ) -> Result<(), ipe_diagnostics::Diagnostic> {
        let mut budget = Budget::unbounded();
        let interner = Interner::new();
        unify(uf, &mut budget, &interner, Span::DUMMY, a, b)
    }

    /// Eq (rigid) ∪ Stringify (flex) — both are pure trait-bound obligations,
    /// so cross-rigidity union is allowed. Result: rigid:true, bounds = Eq|SHOW.
    #[test]
    fn super_super_eq_rigid_stringify_flex_unions() {
        let mut uf = UnionFind::new();
        let a = super_var(&mut uf, true, TyBounds::eq());
        let b = super_var(&mut uf, false, TyBounds::show());
        assert!(do_unify(&mut uf, a, b).is_ok(), "Eq+Stringify should union");
        let rep = uf.find(a).expect("find");
        let content = uf.content(rep).expect("content");
        assert_eq!(
            content,
            Content::Super {
                rigid: true,
                bounds: TyBounds::eq().union(TyBounds::show()),
            },
            "merged content must be rigid with Eq|SHOW"
        );
    }

    /// Add (rigid) ∪ Add (flex) — Add is a concrete-dispatch (Number) obligation.
    /// Cross-rigidity merge is forbidden: result is MISMATCH.
    #[test]
    fn super_super_number_rigid_number_flex_mismatch() {
        let mut uf = UnionFind::new();
        let a = super_var(&mut uf, true, TyBounds::add());
        let b = super_var(&mut uf, false, TyBounds::add());
        assert!(
            do_unify(&mut uf, a, b).is_err(),
            "Add rigid+flex cross-rigidity must mismatch"
        );
    }

    /// Append (rigid) ∪ Eq (flex) — Append is a concrete-dispatch obligation.
    /// Even though Eq is not, Append on EITHER side blocks cross-rigidity union.
    #[test]
    fn super_super_append_rigid_eq_flex_mismatch() {
        let mut uf = UnionFind::new();
        let a = super_var(&mut uf, true, TyBounds::appendable());
        let b = super_var(&mut uf, false, TyBounds::eq());
        assert!(
            do_unify(&mut uf, a, b).is_err(),
            "Append (rigid) ∪ Eq (flex) must mismatch"
        );
    }

    /// Eq (flex) ∪ Ord (flex) — same rigidity, always unioned regardless of
    /// bounds. Result: flex with Eq|Ord.
    #[test]
    fn super_super_same_rigidity_always_unions() {
        let mut uf = UnionFind::new();
        let a = super_var(&mut uf, false, TyBounds::eq());
        let b = super_var(&mut uf, false, TyBounds::ord());
        assert!(
            do_unify(&mut uf, a, b).is_ok(),
            "same-rigidity must always union"
        );
        let rep = uf.find(a).expect("find");
        let content = uf.content(rep).expect("content");
        assert_eq!(
            content,
            Content::Super {
                rigid: false,
                bounds: TyBounds::eq().union(TyBounds::ord()),
            },
        );
    }
}

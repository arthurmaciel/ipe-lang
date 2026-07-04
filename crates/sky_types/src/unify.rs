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
use crate::ty::{Content, FlatType, TyBounds};
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
    // A `Set` element / `Dict` key obligation is a Sky `comparable` — the same
    // scalar set ordering admits. `Float` passes here (it IS `comparable` in
    // Sky, so the typing follows Sky), and the Rust-backend reality that `f64`
    // is neither `Ord` nor `Hash` is enforced at lowering with a dedicated
    // diagnostic — not as a confusing type mismatch at this head pin.
    (!bounds.has_number() || number_ok)
        && (!bounds.has_ord() || ord_ok)
        && (!bounds.has_comparable_key() || ord_ok)
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
        // A flex adopts a super-typed variable's obligations wholesale.
        (s @ Content::Super { .. }, Content::Flex) | (Content::Flex, s @ Content::Super { .. }) => {
            uf.union(ra, rb, s)
        }
        // Two super-typed variables merge: the survivor owes the union of both
        // obligation sets, and is rigid if either was (an annotation skolem that
        // meets an inferred super-flex must stay generic).
        (
            Content::Super {
                rigid: r1,
                bounds: b1,
            },
            Content::Super {
                rigid: r2,
                bounds: b2,
            },
        ) => uf.union(
            ra,
            rb,
            Content::Super {
                rigid: r1 || r2,
                bounds: b1.union(b2),
            },
        ),
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
        // ── Open-record unification (#108 T2 / #56 row-poly) ─────────────────
        //
        // Faithful port of `unifyRecords` from
        // `../sky/src/Sky/Type/Unify.hs:468-512`.
        //
        // Algorithm (four cases):
        //   1. Unify every field present on BOTH sides pairwise.
        //   2. A CLOSED side cannot absorb the other's extra fields → mismatch
        //      (SKY-T0001).
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
                let Some((v1, v2)) =
                    fs1.get(name).copied().zip(fs2.get(name).copied())
                else {
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
        // in `../sky/src/Sky/Type/Unify.hs`. Without this arm both roots would
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
/// Mirrors `isClosedRecordExt` from `../sky/src/Sky/Type/Unify.hs:505`.
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

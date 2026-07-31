Status: Accepted

# 0018. Every record reaching the backend has a fully-pinned concrete field set

## Context

The backend resolves records by *exact sorted-field-set match* and panics on a
miss (divergence A7: fail-loud, versus the reference's superset-widening
fallback). This is sound only if every record reaching the backend has a
fully-pinned concrete field set — i.e. row-polymorphic functions either resolve
to concrete shapes before lowering or are rejected at type-check. No formal
proof of that invariant existed, so a future generalization effort (ADR 0008's
module-boundary scheme promotion) could silently break it.

## Decision

Establish the invariant with an empirical proof matrix plus mechanized gates,
pinning the four mechanisms that keep records pinned: (1) open-record unification
(faithful to the reference's `unifyRecords`); (2) deferred field access (subset
patterns/access legal by construction); (3) open-record kernel schemes mirrored
from stdlib (Web cfg); (4) monomorphic env pinning (unannotated bindings pin on
first concrete use). Five regression fixtures gate the invariant, including two
*rejection* fixtures (`row_poly_two_supersets_neg`, closed-superset) that act as
the ADR-0008 tripwire.

Rejected alternatives: adopting the reference's **superset-widening fallback**
(it enables exit-0-then-cargo-fail, which the seal forbids), or **assuming
safety without proof** (the A7 "soundness > completeness" rationale is only valid
with the pinning invariant proven).

## Consequences

- **Invariant that must keep holding:** every record type reaching the backend
  has a fully-pinned concrete field set. It is currently ironclad
  (`CLocal` monomorphic; `promote_untyped_boundaries` excludes obligated roots).
- **ADR-0008 coupling tripwire:** module-boundary generalization of unannotated
  bindings must either (a) keep record-row tails monomorphic at the boundary
  (matching the reference's within-module behavior) or (b) add per-record-shape
  callee monomorphisation to the backend first. The two rejection fixtures must
  keep *rejecting* until backend monomorphisation lands — flipping them to accept
  without that machinery reintroduces the A7 miss (a panic-on-unknown-shape).
- One completeness gap is recorded, not a runtime hazard: row-var annotation
  syntax `{ r | f : T }` does not parse (`IPE-P0001`).

Status: Accepted

# 0008. Untyped top-level bindings generalize only at the module boundary

## Context

An untyped polymorphic top-level binding (no signature) must stay *monomorphic
within its home module* — same-module reuse of such a helper at two different
types is rejected. The same helper may be instantiated at different types across
module boundaries, and the type-checker must accept those valid cross-module
cases.

The forcing constraint is soundness in the conservative direction: over-acceptance
is ruled out entirely (a program that should be rejected must not compile), and
the fix must not open a soundness hole.

## Decision

Generalize untyped bindings **at module-boundary completion only** ("boundary
scheme promotion"): each untyped binding shares one inference variable for all
same-module references (staying monomorphic in its home module); when the module
finishes, residual plain-`Flex` variables — excluding `Super`-bounded and
`Rigid`-contaminated ones — are quantified into a scheme, and cross-module
references instantiate that scheme fresh via a union-find copy-walk, exactly as
typed bindings already do. Ipê is pure, so no value restriction applies (the
reference agrees).

Rejected alternatives:

- **Rank/level-based let-generalization** — independently rejected by three
  reasoners as strictly *more permissive* than the reference (it would accept
  same-module reuse the reference rejects), a genuine soundness risk, and the
  highest-blast-radius change (touching the exhaustive-match infrastructure in
  `unify.rs`).
- **Full def-level dependency-order generalization** — also extends acceptance
  into same-module reuse, which the reference rejects.

## Consequences

- Same-module reuse at two types stays rejected
  (`untyped_polymorphic_use_at_two_types_is_rejected` pins this). Cross-module
  reuse at different types is accepted and E2E-verified; chained cross-module
  calls prove the discharge-before-generalize ordering.
- `Super`-bounded and rigid-contaminated defs stay program-monomorphic (an
  under-acceptance deferred to a later phase — the safe direction).
- Ambiguous instantiation (a use-site region with free vars not covered by the
  enclosing def's own generics) fails closed with `IPE-L0102` — stricter than
  the reference, sanctioned under "prefer concrete codegen."
- **Invariant that must keep holding:** the quantified-var population never
  collides with `Ty::Var` or union-find representatives; nothing is copied
  unless actually quantified, so programs with no boundary-free untyped defs
  stay byte-identical. Any counting that decides *set membership* (is this var
  quantifiable?) may over-approximate; the direction of drift must always be
  toward under-acceptance, never over-acceptance past the reference.

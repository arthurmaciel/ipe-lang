Status: Accepted
Date: 2026-07-11

# 0016. The andMap curried-payload restriction is a type obligation, not a syntactic check

## Context

ADR 0015 narrowed the function-payload ban to two call-site gates; this ADR
designs one of them — the restriction that `Maybe.andMap`/`Result.andMap`'s
function payload must be arity-1 (`a -> b`), never a curried arity-≥2 arrow that
would fail `FnOnce(A) -> B` at runtime. Three earlier implementations that
matched the `andMap` call *syntactically* were reverted (2026-07-10): a curried
`andMap` reference can pass through `let`-bindings, bare point-free top-level
aliases, higher-order arguments, and record fields — an open-ended enumeration.
The real hazard is a *property of a value's solved type* (arity ≥ 2 arrow), not
the syntax at any reference point.

## Decision

Two-tier design:

- **Tier 2 (primary) — a structural type obligation.**
  `TyBounds::and_map_payload()` is attached to the payload-result slot (`b` in
  `Maybe.andMap : Maybe a -> Maybe (a -> b) -> Maybe b`) at
  `constrain_var_kernel` time, using the same `Content::Super` mechanism already
  proven for `Set`/`Dict` key comparability. The obligation is checked post-solve
  against a fully concrete type and survives arbitrary aliasing, including
  cross-module generalization (it reuses the existing "lift bound onto annotation
  skolem" path, and `promote_untyped_boundaries` excludes obligation-carrying
  roots from quantification).
- **Tier 1 (defense-in-depth) — a lowering backstop.** Restore the check inside
  `lower_callee`, the single exhaustive funnel through which both direct calls
  and bare-value references pass, so any Tier-2 miss is a cargo-fail backstop,
  never a silent acceptance.

Rejected alternatives:

- **AST-shape matching at the call site** — open-ended by construction; every
  intermediate form (direct call, pipe, `let`, top-level point-free, higher-order
  arg, record field) needs separate handling. Three reverted incidents proved
  enumeration fails.
- **Only a lowering-time check** — inherits the cross-module generalization
  residual: a genuinely severed wrapper (annotated, reused at two different
  arity-1 payload types) is invisible to `region_ty` because generalization
  mints fresh vars per external call site. Tier 2's obligation closes this.

## Consequences

- **Invariant that must keep holding:** the arity restriction is enforced on the
  *solved type*, so it holds across every aliasing shape; `lower_callee` remains
  the single lowering funnel that backstops any future syntactic form by
  construction. Any counting/obligation that decides membership must never be
  quantified away at a module boundary (ADR 0008 excludes obligated roots).
- **Documented precision trade-off (conservative, never permissive):** an
  unannotated cross-module wrapper reused at two *different but individually
  safe* arity-1 payload types is rejected. Workaround: annotate the wrapper
  explicitly, routing it through the re-propagated path instead of `CLocal`.

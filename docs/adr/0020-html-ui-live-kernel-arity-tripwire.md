Status: Accepted

# 0020. Html/Ui/Live kernels are schemed only when three arity sources agree

## Context

43 kernels in the Ipe.Html / Ipe.Ui / Ipe.Web rendering family were typed as
`Ty::Var(u32::MAX)` — the fallback hole that lets a call slip past the
type-checker and fail at `cargo`. Closing them requires a correct
`stdlib_scheme` arm per kernel, and the scheme's arrow-count must agree with the
rest of the compiler pipeline. Deriving those arms surfaced both ready kernels
and a registry bug.

## Decision

A kernel enters the schemed set (`FIRST_SCHEMED`) only when its arity is
triple-verified against three authoritative sources: (a) the kernel registry
`decl().arity`, (b) the lowerer `callee_arity`, and (c) the actual runtime
function parameter count. Of the 43, 35 triple-agree and are schemed as a batch.
Seven node kernels (`Html.div/span/a/button/p/input/img`) are **blocked**: the
registry `decl().arity` is off by one while the runtime and lowerer agree; the
derived schemes are correct, so only the registry is corrected (a one-line
per-kernel fix, restoring `arrow-count == decl().arity == callee_arity`).
`Live.appRouted` is a special case — not a simple curried arrow but a closed
record of 9 fields under one Ipê-level arity — and needs an explicit decision
(dedicated closed-record arm vs. exclude as `REACHABLE_BUT_UNLOWERED`) before
enrollment.

Rejected alternatives: scheming the seven node kernels without fixing the
registry (ships incorrect `decl().arity`, violating the tripwire), or leaving
`Live.appRouted` untyped (advertises a kernel the lowerer refuses — fail-closed
but inconsistent).

## Consequences

- **Invariant that must keep holding:** `arrow-count == decl().arity ==
  callee_arity` is the fail-closed tripwire. A future regression (a runtime
  signature change not mirrored in the registry, or a dropped `stdlib_scheme`
  arm) manifests as a tripwire failure in the drift tests, never as a silent
  `Ty::Var(MAX)` hole that passes `ipe` and fails `cargo`.
- Kernel enrollment is atomic: the triple-verified schemes are trustworthy *by
  construction* — the three-source agreement is the proof. This ADR is the
  companion for the rendering family to ADR 0009's single-source-arity decision.

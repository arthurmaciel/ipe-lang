Status: Accepted
Date: 2026-07-14

# 0007. Clone-hoist across match/if arms uses per-arm snapshot/restore, not a shared MAX counter

## Context

ADR 0002 established the non-`Copy` move seal (clone every owned read except the
last, which moves) via a `remaining` counter. Backlog #193 extended the same
seal to reused non-`Copy` bindings captured into `move` closures across the
Task/CLI pipeline, which double-moved to `cargo` E0507/E0382. The general fix
lives in `crates/sky_lower/src/lower.rs` (`rewrite_multiuse_clones`) plus a
scoped `emit_expr.rs` change for the `ui_on_input_`/`ui_on_change_` inline-wrap
sites. It is implemented (the `golden_i193_*` suite:
`asymmetric_arms_cloneok`, `nonclone_fn_once_per_arm`, `oninput_reused_capture`,
`update_base_after_move`, `taskseq_reuse`, `nested_capture_outer_arg`). The code
is the source of truth for the *how*; this ADR records the *why*, and in
particular why the obvious fix is unsound.

A prior design (v1) rested on false claims about the reference and prescribed a
MAX-seeded shared counter; it was adversarially rejected. This ADR captures the
corrected v2 decision.

## Decision

`rewrite_multiuse_clones` threads ONE `&mut remaining` DFS through the whole
expression: at each `Var(sym)` read it emits `CloneVar` and decrements while
`remaining > 1`, and a bare move on the last (`remaining == 1`), with an early-out
at `remaining == 0`.

**Match/If arms are handled by per-arm SNAPSHOT/restore of `remaining`, NOT by
seeding the shared counter with `MAX(arm uses)`.**

### Why MAX-on-a-shared-counter is unsound

The counter is sequential and *spent* as the DFS walks arm bodies left-to-right
in declaration order (the `map_bodies` callback is `FnMut` iterated in order).
Counterexample — once-in-arm-A, twice-in-arm-B, MAX seed = `max(1,2) = 2`:

1. Arm A first read: `remaining` 2 → 1, emits `CloneVar` (spurious — A uses it
   once, should be a bare move).
2. Arm B first read: `remaining` 1 → 0, emits **bare `Var`** (wrong — B has a
   second use coming, this one must clone).
3. Arm B second read: `remaining == 0` → early-out returns it untouched → bare
   `Var`. Arm B moves `sym` twice → E0382, the exact #193 class.

MAX is order-dependent: swapping to twice-A/once-B moves the failure to a
different arm. A shared *spent* counter can never be correct for
mutually-exclusive arms, because spending in one arm robs another.

### The chosen mechanism

Arms are mutually exclusive — exactly one runs at runtime. So: thread the shared
counter through the scrutinee (evaluated unconditionally), then rewrite each arm
body with its **own** counter seeded from its **own** use count
(`count_var_uses`), restoring the shared `remaining` between arms. Each arm's
last use becomes a bare move regardless of sibling arms' counts. The counters
that drive the seed MAX across arms (worst-case path), while the *rewrite* snapshots
per arm — the two must not be conflated.

This mirrors the reference's deliberate two-function split
(`collectFreeVarLocalsMulti` SUMs to drive the prelude set membership — a safe
over-approximation; `collectVarLocalsMulti` MAXes arms to drive the use-site
clone set), which was the fact v1 got backwards.

## Consequences

- The seal now holds for reused non-`Copy` bindings across match/if arms and
  `move`-closure captures, not just straight-line code (ADR 0002).
- The invariant that must keep holding: any counting that decides *set
  membership* (is this var multi-use anywhere?) may SUM/over-approximate, but
  the sequential clone-emitting counter that is *spent* during the rewrite must
  be snapshot/restored per mutually-exclusive arm — never seeded with a MAX and
  shared. Conflating the two reopens the double-move hole.
- The fail-closed `reject_fn_value_reuse` gate (a non-`Clone` function value
  reused more than once per arm) must stay strict under the per-arm counting;
  `golden_i193_nonclone_fn_once_per_arm` pins that it is not silently relaxed.

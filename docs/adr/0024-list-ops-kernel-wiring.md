Status: Accepted

# 0024. The nine unwired list operations are kernels, not pure-Ipê routing

## Context

Nine list operations (`take`, `drop`, `append`, `concat`, `concatMap`, `zip`,
`cons`, `isEmpty`, `indexedMap`) were registered in canon's prelude-qualifier
member array but lacked kernel registry entries, lowerer arms, and constrain
schemes, so calling any produced `error[IPE-L0108]: kernel function not
available yet`. The design question was whether to complete kernel wiring or
route these to the pure-Ipê `Ipe.List` bodies.

## Decision

Wire all nine as kernels. The forcing fact: `List.x` resolves through canon's
prelude-qualifier install to `VarHome::Kernel` *unconditionally* — it never
reaches the compiled `Ipe.List` source. Kernel is the only wiring that (a)
makes the name callable, (b) yields a fail-closed constrain scheme (no
`Ty::Var(u32::MAX)` fallback, mandatory under the seal), and (c) reuses the
proven kernel emission path already used by the 10 wired List kernels. For the
non-HOF ops the runtime fns are *iterative* Rust implementations (constant
stack, output-identical to a recursive implementation); the HOF runtime fns
(`concatMap`, `indexedMap`) already exist. `indexedMap`, missing from canon's member array, is added.

Rejected alternative: **pure-Ipê routing** — it would require re-pointing every
canon `List` member from `VarHome::Kernel` to `VarHome::TopLevel`, guaranteeing
`Ipe.List` is compiled as a dep (it currently is not), and surviving the
"cannot infer T2" cross-module higher-order-function inference hole for the
function-carrying members. Strictly larger, higher-risk, zero
security/correctness/soundness benefit.

## Consequences

- **Invariant that must keep holding:** every one of the nine ops has a
  fail-closed scheme; none falls to `Ty::Var(u32::MAX)`. A missing `stdlib_scheme`
  arm returns `None` → `Diagnostic::Lower` (an IPE error), never
  exit-0-then-cargo-fail; the `FIRST_SCHEMED` gate catches a future accidental
  drop.
- The iterative Rust implementations are a recorded efficiency-only divergence
  (constant stack, output-identical).
- Adjacent same-class gaps (`any`, `all`, `find`) are filed as same-family
  follow-up, not left as latent `IPE-L0108`.

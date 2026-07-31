Status: Accepted

# 0015. Function values in constructor payloads: lift the blanket ban, keep two narrow fail-closed gates

## Context

The original gate `IPE-L0114` rejected *all* function-valued constructor
payloads (`Ok (\x -> x+1)`, `Just someFn`) to prevent emit-layer seal violations:
function values render as `Box<dyn Fn>`, which cannot derive `Clone`, `Debug`,
`PartialEq`, or serde. But the *construction* was never the hazard — runtime
`IpeMaybe<T>`/`IpeResult<E, A>` already carry generically-bounded derives that
compile even with a non-derivable type argument; only the *uses* are dangerous.
The gate also predates the derive-demotion machinery that now absorbs
non-derivable payloads in user enums.

## Decision

Replace the blanket construction ban with two narrower fail-closed gates:

1. A **reuse gate** (`IPE-L0127`) rejecting multiple consuming uses of a
   function-carrying value (`Box<dyn Fn>` is not `Clone`).
2. An **`andMap` call-site arity gate** (designed in ADR 0016) rejecting curried
   payloads that would not satisfy `FnOnce(A) -> B` at runtime.

The construction lift is sound because (a) runtime enums have bounded generic
derives, (b) concrete user enums drop auto-derives when non-derivable (the
derive-demotion fixpoint), and (c) upstream obligations (`==`, `toString`, serde,
Web Model) are already guarded upstream of lowering (the type-checker's
`ty_is_equatable` rejection, the Model gate, `ir_type_is_serde` poisoning).

Rejected alternatives:

- **Add `Maybe`/`Result` to the opaque-wrapper exemption** — wrong: they are not
  opaque (fields are pattern-matched and stringified), and it would silently
  bless all functions under them, including multi-arity shapes that fail at
  `andMap`.
- **Arc payload representation** — dual representation (Box in params, Arc in
  payloads) needs seams at every construction/extraction boundary, and
  `Arc<dyn Fn>` still lacks `Debug`/`PartialEq`/serde, so no use-gate goes away.
- **Bare `fn` pointers** — silently forbids capturing closures, which Ipê
  semantics require; this breakage is documented in `docs/divergences-from-sky.md`.

## Consequences

- The reuse gate preserves the invariant "every non-`Copy` value is either
  `Clone` or linearly used" (ADR 0002) for function payloads until the general
  clone-hoist pass (ADR 0007) subsumes it.
- **This is a sanctioned divergence from upstream's `fn`-pointer approach**:
  Ipê's `Box<dyn Fn>` with captures is strictly more general and the right
  trade-off for Ipê semantics (recorded in `docs/divergences-from-sky.md`).
- **Invariant that must keep holding:** the construction lift stays sound only
  while the three upstream guards (runtime bounded derives, the derive-demotion
  fixpoint, and the type-checker/Model/serde gates) remain in place; weakening
  any of them reopens an exit-0-then-cargo-fail hole.

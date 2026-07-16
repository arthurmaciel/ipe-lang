Status: Accepted
Date: 2026-07-02

# 0006. Secret is a sealed newtype that cannot leak or be `==`-compared

## Context

The language needs a `Secret` type for auth tokens, API keys, and passwords —
values that must never leak through logs/`Debug`/`Display` and must never be
compared in variable time. It is implemented as an opaque built-in primitive
(`runtime/src/sky_runtime/secret.rs`, `IrType::Secret`, the four `Secret.*`
kernels, and the SKY-T0014 gate). The code is the source of truth for the *how*;
this ADR preserves the soundness/security *why*.

## Decision

**`Secret` is modelled exactly like the existing opaque built-in primitives
`Bytes` and `Db`** — a distinct `IrType` leaf, a runtime type re-exported through
`pub use sky_runtime::*`, kernels in the closed `sky_kernels` registry, typed in
`sky_types::constrain`, dispatched in `sky_lower` — with one crucial difference:

**Its runtime representation is a sealed newtype `struct Secret(String)`, not a
transparent alias**, *because a transparent alias would inherit `String`'s
`Display`/`Debug`/`PartialEq` and leak.* The newtype gives it a redacting
`Debug`, no `Display`, and no `PartialEq`; the only equality is an explicit
constant-time compare (`secret_constant_time_eq`).

Because `emit_expr.rs` dispatches every non-HOF kernel generically, the four
`Secret` kernels need no `emit_expr` change — they emit as plain `fn(args)`
calls.

**`==` on a `Secret` is rejected at type-check (SKY-T0014).** `ty_is_equatable`
previously returned `true` for any zero-arg `Ty::Con` (empty args ⇒ vacuously
`all` true), so `secret == secret` would type-check — then either fail `cargo`
(no `PartialEq` derive: the exit-0-then-cargo-fail class) or, worse, if `Secret`
ever derived `PartialEq`, *silently permit a variable-time compare*. Both violate
the security principle. The gate makes `ty_is_equatable(&Secret) == false`, so an
equality obligation on a `Secret` fails closed with SKY-T0014 at type-check
time. This makes "a secret compared with `==`" **unrepresentable at the Sky
level** — the developer must reach for the constant-time kernel.

## Consequences

- The redaction and constant-time-compare guarantees are enforced by the *type*
  (no `Display`, no `PartialEq`), not by developer discipline — a leak or a
  timing side-channel through `Secret` is unrepresentable rather than merely
  discouraged. This invariant must survive any future change to the newtype: do
  not derive `Display`/`PartialEq` on `Secret`, and do not make it a transparent
  alias.
- `ty_is_equatable` is now security-load-bearing. It is the same function the
  broader opaque-`Con` denylist extends; add opaque non-equatable primitives via
  a named helper so the check is appended, never re-derived. A regression that
  makes it return `true` for `Secret` silently reopens the timing/leak hole.
- Optional additive hardening (zeroize-on-drop) layers on top without changing
  this contract.

Status: Accepted

# 0017. Error-constructor payloads have nominal type identity, not anonymous records

## Context

The `FfiPanic`/`TypeMismatch` error constructors and the `Error` payload
argument were registered in the type-checker as *anonymous structural records*
(`{ message : String, stack : List String }`), but the Rust runtime defines them
as *nominal* structs (`IpePanicInfo`, `IpeTypeInfo`, `IpeErrorInfo`). A
structural record literal type-checked, but lowering synthesized a different
struct than the constructor expected, so the emitted Rust failed to compile —
an exit-0-then-cargo-fail seal breach.

## Decision

Give the three payloads nominal `Ty::Con` identities (`PanicInfo`, `TypeInfo`,
`ErrorInfo`), making a raw record-literal construction a clean type mismatch at
`ipe` time. Reuse two existing recipes: field access on opaque nominal Cons
(the `Request` type's `FieldAccess` with fixed field tables) for
`panicInfo.message` etc., and a builtin nominal leaf registration (the
`ErrorDetails` recipe) threaded through canon/lower/IR/backend.

Rejected alternative: **backend coercion** (`emit_ctor` converts record-typed
fields to the nominal runtime struct). It fixes construction but not the escape
direction — a pattern-bound nominal payload flowing into any position typed by
the structural shape (e.g. an unannotated helper over `p : IpePanicInfo`)
re-creates the exit-0-then-cargo-fail without type-directed record-literal
emission in the lowerer, a much larger change through a concurrent lane.

## Consequences

- `FfiPanic { … }`/`TypeMismatch { … }` now produce a clean `ipe`-time
  mismatch. Construction stays via the smart constructors (`Error.io`, …,
  `Error.withDetails`), matching the reference's smart-constructor discipline;
  payloads are runtime-origin values.
- Field access (`p.message`, `p.stack`) keeps working via fixed field tables.
- Record *update* (`{ p | message = … }`) is rejected at `ipe` time with the
  dedicated `IPE-T0017` ("built-in type — fields readable but cannot be rebuilt
  with record-update syntax").
- **Invariant that must keep holding:** one Ipê type, one Rust lowering — the
  structural/nominal mismatch is unrepresentable by construction. An unannotated
  helper over a pattern-bound payload lowers its parameter to the nominal type,
  agreeing with the call site.

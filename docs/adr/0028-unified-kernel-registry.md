Status: Accepted

# 0028. Unified kernel registry — closed `StdlibKernel` enum + `KernelId`

## Context

The original kernel identity was an unparsed `(qualifier, name)` string pair re-matched
at three divergent sites (canon `env.rs`, types `constrain.rs`, lower `lower.rs`) plus
four more (naming, `is_*` predicates, `native_ir_type`, the zero-arity classifier) —
seven hand-maintained tables that silently drift. The types table failed open:
`constrain.rs` had a `_ => Ty::Var(u32::MAX)` catchall giving any un-schemed kernel a
single flexible unification variable, so `ipe` accepted the call without type-checking
its arguments — exit-0-then-cargo-fail, ~231 holes across 14 families. The FFI subsystem
(`docs/adr/0033-ipe-rust-ffi-subsystem.md`) also required that stdlib kernels and FFI
kernels share one representation from the start.

## Decision

Resolve `(qualifier, name) → KernelId` **once at canonicalisation** (parse, don't
validate); every downstream stage holds a typed `KernelId` and never re-matches a
string.

`KernelId = Stdlib(StdlibKernel) | Ffi(FfiKernelId)` — a two-tier sum: a **closed**
`StdlibKernel` enum for stdlib (compile-time exhaustive match, `ALL` slice) and an
**opaque index** for FFI (open, data-driven). The registry lives in the leaf crate
`src/compiler/kernels` (deps `ipe_intern` + `ipe_diagnostics` only), consulted by
canon / types / lower / backend with no dependency cycle.

The `Ty::Var(u32::MAX)` fallback is deleted. A canon-listed-but-unschemed kernel
becomes a compile error (non-exhaustive match or an explicit `IPE-L0108` "kernel not
available yet"), never a silent runtime hole. Backend dispatch is an exhaustive `match
KernelId`. Every new kernel must update ALL anti-drift sites simultaneously
(DEVELOPMENT.md §0b): the enum, `decl()`, `ALL`, the constrain scheme, the arity
table, `naming.rs`, `ir::pretty`, `stdlib.rs` module registration.

Migration is family-by-family behind a fail-closed transitional path; the build stays
green and behavioral parity is golden-pinned at every commit.

## Consequences

- An un-schemed kernel is now a compile-time error in the Ipê compiler itself (the
  anti-drift suite `golden_stdlib_module_seal.rs` enforces this).
- Adding a kernel without updating all seven anti-drift sites produces a CI failure on
  the non-exhaustive `match`, not a silent incorrect runtime.
- FFI kernels (`Ffi(FfiKernelId)`) enter the same `match` with a total default
  (over-drop: a bindable symbol silently omitted rather than emitted wrong).
- The `KernelClass` field on each entry replaces the old `is_db()/is_ui()` boolean
  predicates and their `+wildcard` routing.
- Downstream: `parity-matrix` tool uses `StdlibKernel::ALL` as its authoritative
  kernel list; the `ipe-index parity` sub-command surfaces coverage gaps.

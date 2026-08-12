# kernel-registration — every anti-drift site

A kernel's facts have one source; drift is a compile-time or CI error, never a
deferred cargo failure. Adding or changing a kernel touches every mirrored site,
and a tripwire catches a miss.

## Sites to update

- `src/compiler/kernels` — the `StdlibKernel` enum + `decl()` + `ALL`.
- `src/compiler/types/constrain.rs` — the type scheme (as `const TyShape` data;
  out of the `KNOWN_UNBACKED` bucket). A resolved-but-unschemed kernel is an
  IPE-L0108 compile-time error, never a silent `_` catch-all.
- `src/compiler/lower` — the arity table (+ `REGISTRY_ONLY_ALLOWLIST` for alias
  kernels).
- `src/compiler/backend/rust/naming.rs` — the emitted runtime symbol.
- `src/compiler/ir` pretty-printing; `src/compiler/canon`
  (`STDLIB_MODULE_QUALIFIERS`) module registration.

## Tripwires

The byte-identity scheme oracle, emit-symbol-defined, arity-vs-scheme coherence,
and the module seals (`golden_stdlib_module_seal`) together make a missed update
loud. **When you add to a registry, keep its tripwire** — an unguarded table is
where drift hides.

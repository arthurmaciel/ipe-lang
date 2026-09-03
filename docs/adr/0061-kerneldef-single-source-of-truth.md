Status: Accepted
Date: 2026-08-30

# 0061. One KernelDef descriptor is the source of truth for a kernel's facts

## Context

Every stdlib kernel carries several facts the compiler needs at different stages:
its canonical qualifier and name, its Ipê-level arity, which subsystem emits it,
the Rust runtime symbol it lowers to, the security capability it exercises, the
runtime module that symbol lives in, and its type scheme. When these facts live in
separate large `match kernel { … }` tables across the kernels, types, and backend
layers, a single kernel is described in several places at once. Two failure modes
follow: the tables drift (an emitter name that disagrees with the pretty-printer
name is a silent divergence), and an incoherent kernel becomes representable (a
kernel with a capability but no scheme, or a scheme whose arity disagrees with the
declared one) because no single value holds a whole kernel's row.

## Decision

A single `KernelDef` descriptor is the one place a kernel's facts are assembled:

```rust
pub struct KernelDef {
    pub qualifier: &'static str,
    pub name: &'static str,
    pub arity: u8,
    pub class: KernelClass,
    pub runtime_fn: &'static str,
    pub capability: Option<Capability>,
    pub runtime_module: Option<RuntimeModule>,
    pub scheme: SchemeKey,
    pub shape: Option<&'static TyShape>,
}
```

`def()` builds the whole row for a kernel, and every consumer derives from it rather
than keeping its own table: the backend's kernel name is a one-line delegation to
`def().runtime_fn`, and the IR pretty-printer's display name delegates the same way.
A test pins the delegation so a future refactor cannot silently reintroduce a second
name table.

*Rejected:* independent per-fact tables (name, capability, arity, scheme) in each
layer — they must move in lockstep with the kernel set, so every kernel addition pays
a multi-edit tax and any missed edit is a silent inconsistency the type system does
not catch.

## Consequences

- Adding or changing a kernel is one `KernelDef` entry; the name, capability, arity,
  runtime symbol, and (where structural) the type scheme all flow from it.
- The invariant to preserve: no consumer may reintroduce a parallel table of a fact
  `KernelDef` already owns — a fact about a kernel is read through `def()`, nowhere
  else. The delegation test guards the name case; new facts get the same treatment.
- Type schemes are only partly folded in: `shape: Option<&TyShape>` carries a kernel's
  scheme when it is structurally expressible, and the remaining polymorphic schemes
  still resolve through the `stdlib_scheme` table keyed by `SchemeKey`. Completing that
  migration (every scheme expressible as a shape, retiring the fallback table) is the
  outstanding follow-on this decision enables but does not yet finish.

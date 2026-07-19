Status: Accepted
Date: 2026-07-03

# 0029. Compiled-source stdlib modules

## Context

Stdlib modules could be shipped two ways: (1) as kernel-only stubs — a qualified
call resolves to a `KernelFn` variant, no source, no parse — or (2) as real `.ipe`
source embedded in the compiler and parsed, canonicalised, type-checked, and lowered
exactly like user modules. The kernel-only path was the only exit-0-safe wiring at
the time: a qualified stdlib call lacking a `KernelFn`/lower-arm/scheme emitted
`IPE-L0108`. But several stdlib modules (`Ipe.Css`, `Ipe.Error`, `Ipe.Palette`, …)
have rich internal structure — helper functions, ADTs, recursive combinators — that
would require dozens of new kernels if encoded kernel-only, each needing its own
type scheme, arity entry, backend emit arm, and runtime function. The source path
is strictly better for these modules: one parse-and-compile pass handles all their
complexity without new kernel slots.

The load-bearing invariant: a compiled-source module either resolves to exactly the
same pipeline result as a user module (parse → canon → infer → lower → emit →
`cargo build`) or it produces a clean `IPE-N…`/`IPE-T…` diagnostic. There is no
third state; in particular "exit-0 then cargo-fail" is unrepresentable.

## Decision

Introduce `inject_compiled_std_closure` in `src/ipe-cli/src/project.rs`: when the
import graph transitively reaches a compiled-source stdlib module, the compiler
injects its embedded source as if the user had written it. The module is fully
annotated (every top-level binding carries a type annotation) so inference cannot
produce a surprising deep-stdlib unification failure. The trust tag on the module
path exempts it from the IPE-N0025 reserved-namespace gate (it legitimately
declares `module Ipe.…`).

Hybrid modules are allowed: a compiled-source module may contain both pure Ipê
bodies and `Ffi.kernel "Name"` aliases whose call sites route to existing kernels.
This lets a module like `Ipe.Error` define its ADT in Ipê while delegating runtime
helpers to kernels.

## Consequences

- New stdlib modules with internal structure should use compiled-source, not new
  kernels; the rule is: if it needs more than a signature + a runtime dispatch slot,
  write it in Ipê.
- A compiled-source module with any un-annotated top-level binding is a
  compiler-internal error at canonicalisation — not an inference mystery at the
  user's call site.
- The LSP and `ipe watch` consume the same injection path; stdlib source edits get
  incremental re-analysis through the same salsa query graph as user source.
- Adding a compiled-source module requires only: embedding the source, adding its
  path to the injection allowlist, and writing tests. No new kernel slots, no new
  anti-drift sites (unless the module uses `Ffi.kernel` aliases, in which case those
  kernels must already exist in the registry).

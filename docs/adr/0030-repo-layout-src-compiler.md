Status: Accepted

# 0030. Repository layout — `src/compiler/` crate tree

## Context

The original codebase was structured with a `crates/` flat workspace (Cargo convention)
and a separate `runtime/` tree. As the compiler grew — parser, canonicaliser, type
checker, lowerer, IR, backend, kernels, diagnostics, FFI, salsa DB, LSP — the flat
layout made it hard to see the acyclic crate-stage pipeline and the runtime/backend
split. The move to `src/` was Step A of a three-step endgame (layout → rename →
namespace flatten) designed so each step leaves the tree green and bisectable.

## Decision

Relocate compiler crates under `src/compiler/<name>` (as `ipe_<name>`), the driver
CLI under `src/ipe-cli/`, the runtime under `src/runtime/rust/`, the LSP crates under
`src/lsp/`, the stdlib source under `src/stdlib/`, and the backend under
`src/compiler/backend/`. Move is `git mv` only — no renames, no import changes in Step A.

The acyclic pipeline is now visible as a directory listing:
`canon → db → diagnostics → intern → ir → kernels → lower → parse → syntax → types → watch → backend`

The runtime lives at `src/runtime/rust/` — separate from the compiler, consumed by the
backend emitter which copies it into each emitted project as `src/ipe_runtime/`.

`tools/` stays at the repo root for standalone binaries (`ipe-index`, `oracle`,
`parity-matrix`, `refresh-oracle`, `ipe-ffi-inspector`).

## Consequences

- `Cargo.toml` workspace members and all `path = "…"` dep references updated.
- Scripts that refer to crate paths (`tools/scripts/lib/env.sh`, CI workflows) updated once;
  subsequent renames (Step B) and namespace flattening (Step C) happen in separate
  commits that each leave the gate green.
- The step-A relocation is purely mechanical — `rustc` verifies the rename total via
  the `[package] name` fields; no logic changes.
- A developer can locate any compiler stage by `ls src/compiler/`; runtime behaviour
  by `ls src/runtime/rust/src/`.
- The legacy `crates/` and `runtime/` paths are gone; links in old docs, comments, or
  external references must be updated to the new paths.

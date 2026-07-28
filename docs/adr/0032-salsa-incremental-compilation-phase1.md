Status: Accepted

# 0032. Salsa incremental compilation — phase 1

## Context

The compiler pipeline (`ipe_parse → ipe_canon → ipe_types → ipe_lower → ipe_ir →
ipe_backend_rust`) ran one-shot on every invocation: a single-character edit to any
source file re-ran the entire DAG for every module. This made `ipe watch` unusably
slow for large projects and blocked the LSP from sharing the same checker. The goal
is a salsa query graph where a body-only edit to a module re-runs only the stages
downstream of that module, with correctness invariant: **under-invalidation (a stale
build that looks correct) is a correctness violation and outranks every efficiency
gain.**

## Decision

Phase 1 introduces the `ipe_db` crate (`src/compiler/db/`) as the salsa database.
The earliest front-end stages sit behind memoised salsa queries: `parse_module`,
`extract_imports`, and the per-module canonicalisation steps. A cut-point at
`ipe_ir` (the whole-program IR after linking) separates the salsa graph from the
emit→cargo boundary; the emit side uses deterministic write-if-different + delete-
orphans reconciliation so that unchanged emitted files produce no filesystem churn
and no spurious `cargo` rebuild.

The `SourceFile` and `SourceRoot` salsa inputs are the only places an editor or
watcher writes into the DB; everything else is a derived query. A `VerifiedEdit`
gate (typed wrapper around a `SourceFile` update) prevents raw string injection
without going through the parse boundary.

FFI introspection stays outside the salsa graph — it is a coarse content-hash
cache regenerated only by explicit `ipe add/install`, not a query. Its typed
`.ipei`/`kernel.json` interface feeds the kernel registry as if it were a stdlib
module.

The LSP (`ipe lsp`) and `ipe watch` are second consumers of the same salsa DB;
they never reimplement parsing, name resolution, or type inference.

## Consequences

- Under-invalidation is the primary hazard: a new cross-module dependency (e.g. a
  type alias export added to a dep) that the salsa dependency graph does not track
  will produce a stale build that silently passes the type checker. The test suite
  for `ipe_db` (`tests/phase2_incrementality.rs`) regression-tests known
  cross-module dependency edges.
- The emit→cargo boundary is intentionally outside salsa; cargo's own dependency
  tracking handles incremental Rust recompilation. Mixing salsa and cargo into one
  graph would duplicate cargo's work and risk cache-coherence bugs.
- Phase 2 (the `emit_rust_file(RustFileId)` query) and Phase 3 (the `ipe watch`
  file-watcher integration) are separate efforts; Phase 1 delivers the DB + front-
  end queries that both require.
- Hot-reload (`ipe watch` + `ipe lsp`) must never open a dynamic-code / `eval` hole:
  Ipe.Web's no-`data-ipe-eval` + strict-CSP (no `unsafe-eval`) posture is a hard
  invariant at every reload level.

# Roadmap & velocity model (post-M0)

> **Status:** plan. Written 2026-06-27.
> **Goal:** the best, fully-principled Sky compiler in the world. **Maximise
> throughput without loosening rigour** — speed comes from parallel fan-out, never
> from relaxing the gates.

## The principle that governs velocity

Rigour and speed conflict only if you serialize the wrong thing. We go fast by
**moving the serialization points**, not by removing gates. The gates below are
**non-negotiable** (they *are* "fully principled") and run per work-unit:

- guardian (security-soundness) review of every diff,
- behavioural parity vs the Go reference (the oracle — `PRINCIPLES.md` §2),
- `forbid(unsafe)` + clippy-hardest + Miri (changed crates) + `fmt --check` green,
- parse-don't-validate; no `String` error channel; deterministic iteration.

Speed is bought by running *many* work-units through those gates **concurrently**,
and by shrinking the sequential core.

## The parallel-execution model (round structure)

Each round (a milestone or a batch within one) runs:

1. **Sequential CORE step (short, one agent).** Land only the *shared-contract*
   additions a round needs — new `sky_ir` variants, new diagnostic codes, new
   shared AST nodes, new kernel-registry rows. Freeze them. This is the *only*
   serialized code step; keep it small and fast.
2. **Parallel FAN-OUT — DISJOINT FILE SETS ONLY (hard rule).** Two agents run
   concurrently **iff their file sets do not intersect.** No shared file is ever
   edited by more than one agent in a round. Before fan-out, write down each
   agent's exact file list; if any file appears twice, those agents are **not**
   parallel — serialize them (or move the shared edit into the CORE step / a single
   owner). This eliminates merge conflicts by construction, not by resolution.
   Each agent owns one **vertical feature slice** (parse→canon→types→lower→backend
   for one feature) or one **leaf artifact** (one explain page, one runtime kernel
   + its parity test). Worktree isolation (`isolation: 'worktree'`) still backs
   each agent so working-copy/index state never overlaps even transiently.
3. **Parallel guardian reviews (read-only).** One per slice, concurrently — no
   write races. Use a `pipeline` so each slice verifies the moment its review is
   ready, not at a barrier.
4. **Serialized merge + gate.** Merge slices one at a time: rebase, run the full
   gate, land. The second (and last) serialization point.

What stays serial: the **core step** and the **merge gate**. Everything between
fans out. The classic conflict source — two agents editing the *same* shared enum
— is eliminated because enum *definitions* change only in the core step; feature
agents add *match arms / new files*, partitioned so they don't collide (one owner
per shared match-file per round, or split the file).

### Throughput knobs
- Workflow concurrency cap is `min(16, cores−2)`; size fan-out to it.
- Guardian reviews + per-slice parity run concurrently (read-only / isolated).
- Prefer `pipeline()` over barrier `parallel()` so fast slices don't wait on slow ones.
- The **kernel registry** (decided in the diagnostics spec backlog) is the key
  enabler for M4/M5: kernels become independent table rows + runtime mirrors, so
  dozens of them parallelize cleanly instead of churning one giant enum.

## Roadmap (completeness axis) — each milestone is a batch of parallel slices

> Ordering is by dependency, not priority. Within a milestone, slices fan out.
> Every slice lands behind the full gate; behavioural parity vs Go is the spec.

- **M0 — spine** ✅ ADT + `case` + a kernel + `println`, end-to-end, runs.
- **F — diagnostics finish** (in flight after E): 50 explain pages (embarrassingly
  parallel — one agent per page-batch in worktrees), `skyc explain`, `--emit-ir`
  pretty-printer, CI (`.forgejo/workflows` + hosted light), suggestion/auto-patch
  infra, humble/limitation messaging.
- **M1 — core language:** `let…in`, lambdas + first-class functions, `if`/multi-way
  `if`, tuples, the full binop set (`* / == /= < > <= >= ++ :: && ||`), records +
  access + update, type aliases. (Each is a vertical slice → parallel.)
- **M2 — polymorphism:** type variables end-to-end, generic functions, same-module
  re-instantiation, the wildcard-`any` soundness gate (mirror the Haskell
  v0.15 type-directed lowering semantics).
- **M3 — full ADTs & patterns:** non-nullary constructors, nested/cons/tuple/record/
  literal/alias/wildcard patterns, exhaustiveness over all of them.
- **M4 — stdlib breadth (via the kernel registry — COMMITTED):** `List/Maybe/
  Result/Dict/Set/String/Math/Char/...` — fan out one slice per module; each slice
  = registry rows + Rust runtime mirror of the Go runtime + parity tests + ledger
  entries. The registry (`Callee::Kernel(KernelId)` + a `KernelId → entry` table,
  replacing the M0 flat `KernelFn` enum) is now a **committed invariant**, because
  **FFI is its sibling** — an FFI binding is a kernel sourced from crate
  introspection. See `docs/architecture/ffi-design.md`.
- **M4.5 — Sky→Rust FFI:** the isolated, fail-closed `sky_ffi` subsystem
  (introspect → `.skyi` + registry table + `catch_unwind` wrapper), `sky add`,
  dynamic emitted-`Cargo.toml` deps, the FFI security gates. Rides the same kernel
  registry as M4. Full design: `docs/architecture/ffi-design.md`.
- **M5 — effects & runtime:** `Task` everywhere, `Cmd/Sub`, `Http`, `File`,
  `System`, `Process`, `Db`, `Crypto`, `Time`, `Random` — mirror `runtime-go`
  module-for-module per `docs/parity/runtime-parity.md`.

**Backlog/coverage oracle for M4+M5: `skydex`** (`sky/tools/skydex`, bounded ~64 MB).
Run `skydex update` then `skydex parity --gaps` from the sky repo to get the
computed kernel backlog (go-only/rust-only with go=/rust=/route= file:line);
`skydex locate <sym>` + `skydex covers <kernel>` to find impls + coverage. Drives
the parity ledger. Dev tool, outside the trust path. See memory `skydex-tool`.
- **M6 — app shapes:** `Sky.Http.Server`, `Sky.Live`, `Sky.Tui`, `Sky.Webview` —
  the big integrations; each its own sub-roadmap.
- **Cross-cutting (fold in as needed):** DCE, auto-TCO, monomorphisation (also the
  Wall #2 demand-driven path FFI reuses). The FFI subsystem is M4.5 above, not a
  loose cross-cut — designed-for from M0 (`docs/architecture/ffi-design.md`).

## How a milestone runs (concrete)

```
phase Core:   one agent lands the round's new sky_ir variants + diagnostic codes + registry rows (frozen)
phase Build:  pipeline over slices — each: failing test → implement (worktree) → per-slice gate
phase Verify: parallel guardian review + behavioural-parity per slice
phase Land:   serialized merge; full-workspace gate; behavioural-parity sweep; tag
```

The existing `sky-rust-backend` skills (`build-sweep`/`run-sweep`/`web-sweep`/
`perf-sweep`/`keep-go-parity`/`sync-with-upstream`) are the per-slice and
per-milestone harness.

## Guardrails on going faster (so rigour holds)

- **Disjoint file sets only (hard rule).** Parallel agents must edit
  non-overlapping files; any shared file (a shared enum, root `Cargo.toml`, a
  shared match-arm file) is edited only in the sequential CORE step or by one
  owner. If two tasks want the same file, they run in sequence — full stop.
- The guardian gate and behavioural-parity oracle are **never** skipped to save
  time. If a slice can't pass, it doesn't land — it goes back to the queue.
- Worktree disk cost is real (~1.5 GB each): cap concurrent worktrees, prune after
  each cherry-pick (per the CLAUDE hygiene rules), and watch `df` before fan-out.
- The sequential core step must stay *small*; if it grows, the round is too broad —
  split it.
- One change at a time to any shared match-file per round (assign an owner) to keep
  merges conflict-free.

## One-line summary

Freeze a small shared core per round, then **fan out only agents with disjoint
file sets**, in isolated worktrees, through unchanged guardian + parity +
clippy/Miri/fmt gates; merge serially; roadmap M1→M6 by dependency, each milestone
a parallel batch, behavioural parity vs Go as the spec — fast *because* the gates
are mechanical and the work is partitioned by file, not *despite* them.

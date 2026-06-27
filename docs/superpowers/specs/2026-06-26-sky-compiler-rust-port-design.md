# Sky Compiler — Rust Port: Design

**Date:** 2026-06-26
**Status:** Approved (brainstorm complete; guardian ruling: APPROVE-WITH-CONSTRAINTS)
**Design authority / reviewer:** `security-soundness-guardian` agent
**Repo:** `/home/arthur/Documentos/comp/sky-rust` (greenfield)
**Reference:** `/home/arthur/Documentos/comp/sky` (Haskell Sky compiler, ~65k LOC, 92 modules)

## 1. Goal

Produce a full, working Sky compiler written in Rust that mirrors the observable
behaviour of the existing Haskell compiler. The compiler emits a backend-agnostic
typed IR from which Rust code is generated today, with the boundary kept clean so
additional backends can be added later. The reference Haskell compiler already
emits Rust (`src/Sky/Generate/Rust/*`) and ships a complete runtime crate
(`runtime-rust/`); that existing Rust emission is reused as a golden oracle, and
`runtime-rust` is the fixed link target.

## 2. Governing principles (strict tie-breaker order)

Per `PRINCIPLES.md`, when two principles conflict the higher-numbered yields:

1. **Security** 2. **Correctness** 3. **Soundness** 4. **Efficiency**
5. **Completeness** 6. **Readability**

For this project specifically:

- **Correctness** = the Rust compiler's output matches the Haskell reference's
  observable behaviour, ideally byte-for-byte on emitted code; any deliberate
  divergence is documented.
- **Soundness** has two faces: **(a)** the compiler itself never panics / unwraps /
  indexes out of bounds / hits UB (Miri-clean, clippy at hardest); **(b)** it
  preserves Sky's guarantee that well-typed Sky never panics in generated code.

**Cross-cutting law — "parse, don't validate":** every stage boundary converts
untyped/looser input into a more precise type that makes illegal states
unrepresentable. The IR in particular is constructed so that emitting unsound Rust
is impossible by construction, not prevented by a later check.

## 3. Architecture — Cargo workspace, acyclic stage crates

Dependencies flow strictly downward; no cycles. Each crate has one clear purpose,
a well-defined interface, and is independently testable.

| Crate | Responsibility | Depends on |
|---|---|---|
| `sky_intern` | String/symbol interning, deterministic ids | — |
| `sky_diagnostics` | Typed error enums (NO `String` errors), spans, rendering | `sky_intern` |
| `sky_syntax` | Source AST + spans | `sky_intern`, `sky_diagnostics` |
| `sky_parse` | Lex + layout filter + parse → Source AST | `sky_syntax` |
| `sky_canon` | Name resolution, import validation → Canonical AST | `sky_syntax` |
| `sky_types` | HM inference: union-find, unify, solve, constrain → `SolvedTypes` (env + region types) | `sky_canon` |
| `sky_ir` | **Backend-agnostic typed IR** — the boundary | `sky_intern`, `sky_diagnostics` |
| `sky_lower` | Canonical AST + `SolvedTypes` → IR (port of `Compile.hs`); Monomorphise, DCE, TCO | `sky_canon`, `sky_types`, `sky_ir` |
| `sky_backend` | `trait Backend` — the only abstraction a backend sees, over `sky_ir` | `sky_ir` |
| `sky_backend_rust` | Rust emitter (port of `Generate/Rust/*`) | `sky_backend`, `sky_ir` |
| `sky_ffi` | External-crate introspection + bindings generation (isolated, fail-closed) | `sky_diagnostics` |
| `skyc` | CLI driver / orchestration | all stage crates |
| `sky_lsp`, `sky_doc`, `sky_fmt` | Later phases | frontend crates |

**The boundary:** `sky_ir` plus the `sky_backend` trait are the *only* things a
backend may see. This is what keeps "Rust now, other backends later" clean — the
frontend never names a backend, the backend never names the frontend.

## 4. IR design

The Rust port introduces a **clean typed IR with phase typestates**, rather than a
literal port of the 16k-LOC `Compile.hs` lowering tangle. Requirements:

- IR construction enforces invariants **by construction** (parse-don't-validate):
  e.g. every reference is already resolved, every node carries a concrete emitted
  type, pattern matches are already exhaustiveness-checked. Codegen receives an IR
  it *cannot* misuse.
- Emission is a **total function** `IR -> EmittedCode` with no failure arm for
  "malformed IR" — malformedness is unrepresentable.
- Phase typestates encode pipeline progress in the type system (e.g. a pre-DCE IR
  and a post-DCE IR are distinct types) so stages cannot be skipped or reordered.

The Haskell type-directed lowering (`RegionTypes` / `globalRegionTypes`,
per-module env maps) is reproduced as explicit data threaded through `sky_lower`,
not as global mutable `IORef`s.

## 5. Compiler-soundness engineering rules

These are non-negotiable and enforced workspace-wide:

- `#![forbid(unsafe_code)]` in every crate.
- Workspace-level clippy deny-table at the hardest setting (`clippy::pedantic` +
  `clippy::nursery` selected lints + `clippy::unwrap_used` + `clippy::panic` +
  `clippy::indexing_slicing` + `clippy::expect_used` denied). CI fails on any warn.
- **Miri** runs in CI over the unit/IR/runtime test surface.
- **No `String` errors.** All errors are typed enums in `sky_diagnostics`.
- **ICE-as-value.** No `panic!` / `unwrap` / `expect` / raw indexing in compiler
  code. Fallible operations return `Result`. An internal invariant violation
  becomes a typed `CompilerBug` diagnostic value, surfaced cleanly — never a crash.
- **Determinism.** No observable `HashMap` iteration order. Use `BTreeMap` /
  `IndexMap` where iteration is observed (codegen, caching). Required for stable
  build hashes and byte-identical output.
- **Bounds on untrusted input.** Solver step budget (mirrors `SKY_SOLVER_BUDGET`),
  parser/recursion depth caps, IR size caps — a remote `.sky` cannot exhaust the host.
- Interning/arena for ASTs to avoid deep clones on hot paths (Efficiency, below the
  three gates above).

## 6. Correctness verification strategy

Layered, with the Haskell compiler as a free oracle:

1. **Golden byte-diff oracle.** Run the Haskell compiler with the Rust backend on
   each example, store emitted Rust as golden; the Rust port must reproduce it
   byte-for-byte (documented divergences allowed, tracked).
2. **Differential run-equivalence.** Compile + run both compilers' output for each
   example; compare stdout / exit code.
3. **Ported non-regression specs.** Port the 410+ cabal specs (parse, canon, type,
   build) to Rust unit/integration tests.
4. **Runtime soundness reuse.** `runtime-rust`'s existing soundness/proptest suites
   guard guarantee (b).
5. **Parser fuzzing + Miri** for the compiler's own soundness (a).

## 7. Security surfaces

- **Untrusted `.sky` source:** DoS bounds (§5), path-traversal-safe import
  resolution (imports cannot escape the project root).
- **`sky_ffi` (highest risk):** introspects external crates. Runs in its own
  process, fails closed on any inspector error, no shell/TOML/path injection from
  crate metadata, and emits an explicit warning that adding an FFI crate can run
  build-script arbitrary code (supply-chain reality).
- **Emitted code:** all runtime effects routed through the hardened `runtime-rust`
  kernels; emitted Rust itself passes the same clippy/`forbid(unsafe)` gate.

## 8. Milestone 0 — the spine (proves architecture before widening)

End-to-end compile of the CLAUDE.md canonical snippet:

```elm
module Main exposing (main)
import Sky.Core.Prelude exposing (..)

type Msg = Increment | Decrement

update : Msg -> Int -> Int
update msg count =
    case msg of
        Increment -> count + 1
        Decrement -> count - 1

main =
    println (String.fromInt (update Increment 0))
```

Exercises: ADT + `case` exhaustiveness + a kernel call (`String.fromInt`) +
`println` at `main`, through **every** stage (parse → canon → types → lower → IR →
Rust emit), linking the real `runtime-rust`. **Exit criterion:** emitted Rust is
byte-identical to the Haskell reference's Rust emission for this program, builds,
and runs with matching output. Explicitly **excludes** generics, FFI, records,
Sky.Live.

## 9. Swarm decomposition & data-race protocol

Contracts-first to avoid parallel-write conflicts:

- **Sequential freeze (one agent each, in order):** `sky_intern` → `sky_diagnostics`
  → `sky_ir` types → `sky_backend` trait. These are the shared contracts; nothing
  parallelizes until they are frozen.
- **Parallel after freeze (independent crates, worktree-isolated):** `sky_parse`,
  `sky_types`, `sky_backend_rust` (built against golden IR *fixtures* before the
  lowerer exists), `sky_ffi`, the verification harness.
- **Sequential integration point:** `sky_lower` — it joins canon + types + IR and
  is where the spine closes; one agent owns it.
- Orchestrated via `sky-rust-backend:autonomous-swarm` / `Workflow`. Each agent
  works in an isolated git worktree; merges are serialized.

## 10. Guardian gates (blocking, enforced before any phase is "done")

1. Typed-IR phase boundary intact (no backend reaches behind `sky_ir`).
2. `#![forbid(unsafe_code)]` + clippy deny-table green across the workspace.
3. Golden byte-diff oracle passes for the phase's covered programs.
4. FFI fail-closed and process-isolated.
5. Miri clean on the covered surface.

## 11. Out of scope (this spec)

LSP, doc server, formatter, watch mode, and the full stdlib/FFI breadth are
later phases. Each gets its own spec → plan → implementation cycle after the spine
and core language are green.

# AGENTS.md — working on the Ipê compiler

@PRINCIPLES.md

**Ipê** is an Elm-family pure-functional language that compiles to Rust; this repo
is the compiler, runtime, and stdlib (a Rust workspace). This file is a small
orienting map — it links down rather than restating. Read the linked source when
you need depth.

Work as a seasoned Rust and Elm software architect and engineer: concise, objective, and responsible. Weigh every decision by PRINCIPLES.md, and fact-check against the code before affirming anything — verify, don't assume. When something looks wrong, assume the fault is your own — your change, your reasoning, your orchestration — until you have verified otherwise; suspect your own work before the tools, CI, or environment. Treat this repo as production infrastructure that real users and downstream code depend on: correctness and security come first.

- **Working *on the compiler* (Rust)?** Use the map below.
- **Writing Ipê itself** (`.ipe` stdlib, examples, fixtures) or asking what the
  compiler accepts? That's a separate reference: `ipe doc <Module>` for the language
  surface, and `src/ipe-cli/templates/AGENTS.md.in` (what `ipe init` ships).
- **Rules & enforcement:** `PRINCIPLES.md` is the SSOT (  now).
  `docs/adr/` holds history and rationale (the only place archaeology belongs).

## Compiler pipeline — where a change lands

An acyclic chain of crates; most changes touch one stage.

| Stage | Crate | Look here when… |
|-------|-------|-----------------|
| Parse | `src/compiler/syntax`, `src/compiler/parse` | surface syntax, AST, syntactic `TypeAnnotation` |
| Resolve | `src/compiler/canon` | name resolution, module wiring, canonical `Type` |
| Infer | `src/compiler/types` (`constrain.rs`) | HM inference, `Ty`, kernel type schemes |
| Lower | `src/compiler/lower` | `Ty` → `IrType`, arity tables |
| IR | `src/compiler/ir` | lowered `IrType`, pretty-printing |
| Emit | `src/compiler/backend/rust` | emitted Rust, `naming.rs` (runtime symbols) |

Cross-cutting: `src/compiler/kernels` (the `KernelDef` registry — a compile-time
tripwire fails the build if a kernel's mirrored sites drift), `src/compiler/diagnostics`
(IPE codes + `explain/*.md`), `src/compiler/{db,intern}` (salsa), `src/compiler/{sandbox,ffi,watch}`,
`src/lsp/*`. `src/stdlib` is the `.ipe` stdlib; `src/runtime/rust` is what the backend
emits into; `src/ipe-cli` is the `ipe` binary. **Four type reps, in order:**
`TypeAnnotation` → `Type` → `Ty` → `IrType` — confusing one for another is the
classic early mistake.

## Fast gate (a PR must pass — minutes)

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --workspace -- -D warnings
cargo nextest run -p ipe                 # + `-p <crate>` for each crate you changed
```

`--profile ci` for slow emit tests (default 120s false-times-out; ci gives 600s).
`IPE_E2E=1` makes emit tests build+run the emitted project (THE SEAL: `ipe`-accepts
⇒ `cargo`-builds). Goldens are byte-exact emitted Rust — regenerate with
`cargo run -p regen-goldens`, never hand-edit.

## Tooling — use first

- **`tools/scripts/ipe-index locate|wakeup|deps|rdeps|links|neighbors`** — pre-built structural index;
  use before `rg` for "where is X / who calls Y / kernel gaps".
- **Backlog = GitHub issues** via `tools/scripts/github/issue-ticket.sh add|list|close`.

## PR workflow

`main` is green by construction. Branch based on "development" → fast gate → fix until green → PR →`gh pr merge <N> --auto
--squash` (merges when green + current). One PR per unit; check `gh pr list` first.
Versions + `CHANGELOG.md` are release-please automated from Conventional Commits —
never bump by hand.

### Documentation & code standards

- **No archaeology.** Docs and comments state what the rule or design IS now,
  never how it got there: no dates, no task numbers, no phase/milestone/
  campaign labels, no "was X, now Y", no incident stories (ADRs are the one
  sanctioned history home). Git history already records when and why. A
  rationale, when needed, is structural ("a target outside the prune root is
  invisible to reclaim"), not narrative.
- **Comments say WHAT, not HOW — and only when non-obvious.** Names are
  self-explaining to a first-time reader; a comment restating the code, or a
  name that needs a comment, is a smell.

### Mechanical enforcement — comply by construction

When a lint or gate fires, fix the code — never the lint level, never the gate.

- **Clippy deny-set** — enforced by root `Cargo.toml` `[workspace.lints.clippy]`
  (the SSOT: the broad groups + a cherry-picked `restriction` slice, with two
  `cargo` lints allowed as workspace noise). Change the policy there, never in a
  command — every `cargo clippy` is just `-- -D warnings`. `pedantic`
  includes `doc_markdown`: code identifiers in doc (`///`/`//!`) and `//`
  comments MUST be backticked (`` `CloneOk` ``, `` `Vec<T>` ``,
  `` `--all-targets` ``). Applies to `tests/*.rs` too (the `--all-targets`
  end-state). `runtime/src/lib.rs` additionally carries
  `#![cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::panic,
  clippy::unreachable, clippy::todo, clippy::unimplemented,
  clippy::panic_in_result_fn))]`.
- **Escape hatch:** per-site `#[allow(lint)] // one-line why` ONLY — never a
  crate- or gate-wide relaxation. Every ledgered production allow carries an
  `IPE-RUST-AUDIT:ACCEPTED` marker; `tools/panic-scan` is the single source of
  truth for that inventory, so the set is read from the code, not restated here.
- **`unsafe` is forbidden.** Exactly ONE sanctioned `unsafe` block exists —
  `prctl(PR_SET_PDEATHSIG)` in `live::console_proxy` — the only reason the
  runtime is not crate-wide `forbid(unsafe_code)`. Every other module is
  `unsafe`-free and stays that way.
- **Edition 2024** — workspace crates and every emitted project.

### No `dyn Any` — concrete over generic

The backend NEVER emits `dyn Any` / `.downcast` / type-erasure. Wildcard `any`
is not polymorphism — it has exactly ONE concrete lowering (an opaque carrier
type chosen per position, e.g. `Dict String String` in pub/sub payload
position), emitted at EVERY position (enum field, pattern binder, fn/decoder
param, Db row arg, eta lambda param, return). Only genuine named type
variables (`a`, `msg`) become Rust generics, monomorphized by rustc. A generic
emitted where a concrete was possible passes a mechanical gate but can ship a
silent runtime bug (e.g. a `TypeId`-keyed broker needs publisher and
subscriber on the same concrete `T`) — always emit concrete when concrete is
possible. Sanctioned runtime-internal *container* exceptions (payload itself
never erased or downcast): `runtime/src/ipe_runtime/cache.rs` and
`runtime/src/ipe_runtime/live/pubsub.rs`.


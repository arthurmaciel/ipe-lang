# AGENTS.md — working on the Ipê compiler

> **Ipê** is an Elm-family pure-functional language that compiles to Rust. This
> repo is the compiler, runtime, and stdlib — a Rust workspace. This file orients
> an agent working *on the compiler*; it links down rather than duplicating
> `PRINCIPLES.md` (the enforcement SSOT), `docs/internals/` (deep references), or
> `README.md` (the project intro).
>
> **Need the Ipê language itself** — to write `.ipe` stdlib modules, examples, or
> test fixtures, or to understand what the compiler accepts? That is a separate,
> non-overlapping reference: `src/ipe-cli/templates/AGENTS.md.in` (the same file
> `ipe init` ships into a scaffolded project). This doc does not restate the
> language surface; read that one when you write Ipê code.

## The compiler at a glance

The pipeline is an acyclic chain of crates; a change usually lands in one stage.

| Stage | Crate | Look here when… |
|-------|-------|-----------------|
| Parse | `src/compiler/syntax`, `src/compiler/parse` | surface syntax, the AST, the syntactic `TypeAnnotation` |
| Resolve | `src/compiler/canon` | name resolution, module wiring, the canonical `Type` |
| Infer | `src/compiler/types` (`constrain.rs`) | HM inference, `Ty`, kernel type schemes |
| Lower | `src/compiler/lower` | `Ty` → `IrType`, arity tables |
| IR | `src/compiler/ir` | the lowered `IrType`, pretty-printing |
| Emit | `src/compiler/backend/rust` | the emitted Rust, `naming.rs` (runtime symbols) |

Cross-cutting: `src/compiler/kernels` (the `KernelDef` registry), `src/compiler/diagnostics`
(IPE-N/IPE-L codes + `explain/*.md`), `src/compiler/db` + `src/compiler/intern`
(salsa incremental), `src/compiler/sandbox`, `src/compiler/ffi`, `src/lsp/*`,
`src/compiler/watch`. `src/stdlib` is the `.ipe` stdlib source; `src/runtime/rust`
is the runtime the backend emits calls into; `src/ipe-cli` is the `ipe` binary.

**Four type representations, in pipeline order:** syntactic `TypeAnnotation`
(parser) → canonical `Type` (canon) → `Ty` (inference term, interned symbols +
vars) → `IrType` (lowered, concrete, renders to Rust). Confusing one for another
is the most common early mistake.

## Build, test, gate

The fast gate (what a PR must pass — target: minutes):

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --workspace -- -D warnings
cargo nextest run -p ipe                 # CLI integration + goldens
cargo nextest run -p <touched-crate>     # plus each crate you changed
```

- **`--profile ci`** for slow emit tests: the default 120 s terminate false-times-out
  heavy emit tests; `--profile ci` gives 600 s.
- **Goldens** are byte-identical emitted Rust (`tests/golden/<name>/…`). After an
  emit-changing change, regenerate with `cargo run -p regen-goldens` (or `-- <name>…`).
  Never hand-edit a golden.
- **THE SEAL / E2E:** `IPE_E2E=1` makes emit tests build *and run* the emitted
  project — `ipe`-accepts ⇒ `cargo`-builds. `-p ipe --test static_emit` covers musl.
- **Clippy:** `--all-targets` lints test code; the workspace bans `panic!`/`unreachable!`/
  `todo!` even in `#[cfg(test)]`. Fix the code, never the lint level.

Full gate mechanics, guard daemons, and the release pipeline: **`docs/internals/dev-ops.md`**.

## Registering a kernel

A kernel's facts have one source; touching or adding a kernel requires updating every
mirrored site (kernels enum, type scheme, arity table, emitted symbol, module
registration). A tripwire makes a missed site a compile-time or CI error. Full
site list and tripwire details: **`docs/internals/kernel-registration.md`**.

## Tooling — use first

- **`tools/scripts/ipe-index locate|refs|parity|wakeup`** — a pre-built structural
  index; use it before `rg` for "where is X / who calls Y / kernel gaps".
  (`rg` pitfall: never `rg -r`/`-rn` — ripgrep's `-r` is `--replace`; use `rg -n`.)
- **Backlog = GitHub issues** via `tools/scripts/github/issue-ticket.sh add|list|close`.

## Governance

- **`PRINCIPLES.md`** is the enforcement SSOT: the precedence order
  (Security > Correctness > Soundness > Efficiency > Completeness > Readability),
  parse-don't-validate, make-invalid-states-unrepresentable, no `panic`/`unwrap`/
  `expect`/raw-index in production code, and THE SEAL. Read it; this file does not
  restate it.
- **Comments say WHAT and WHY, not HOW; no archaeology** (no dates, issue/PR
  numbers, phase/milestone labels, or "was X now Y") outside `docs/adr/`. Names
  self-explain. A hook enforces this on docs.
- **No public reference to the private reference implementation** — its name or
  its module namespace — anywhere in code, comments, issues, or docs, except
  Attribution and the README intro.
- **`docs/adr/`** is the only place for history and rationale.

## PR workflow

`main` is green by construction. Branch → open a PR → the fast gate runs →
`gh pr merge <N> --auto --squash`; it merges when the gate is green and the branch
is current. One PR per unit of functionality (check `gh pr list` first; extend an
existing PR rather than opening a parallel one). Versions and `CHANGELOG.md` are
automated by release-please from Conventional Commit messages — never bump by
hand. Slow checks run post-merge and nightly; detail in `docs/internals/dev-ops.md`.

## Non-regression invariants (the test suite enforces these)

- No `Result String a` / `Task String a` in public surfaces — use
  `Result Error a` / `Task Error a`.
- No runtime panic from well-typed Ipê code.
- No `dyn Any` / `.downcast` / type-erasure in the backend — concrete over
  generic; a wildcard `any` has exactly one concrete lowering per position.
- Secrets are typed — never `Debug`/`Display` a secret into a log or error.
- Record field enumeration sorts by field index before any order-dependent emit.
- THE SEAL: `ipe build` ⇒ the emitted Rust `cargo build`s — every acceptance path
  fails closed at `ipe` time, never open at `cargo` time.

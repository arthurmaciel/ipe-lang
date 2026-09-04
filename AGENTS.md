# AGENTS.md — working on the Ipê compiler

**Ipê** is an Elm-family pure-functional language that compiles to Rust; this repo
is the compiler, runtime, and stdlib (a Rust workspace). This file is a small
orienting map — it links down rather than restating. Read the linked source when
you need depth.

- **Working *on the compiler* (Rust)?** Use the map below.
- **Writing Ipê itself** (`.ipe` stdlib, examples, fixtures) or asking what the
  compiler accepts? That's a separate reference: `ipe doc <Module>` for the language
  surface, and `src/ipe-cli/templates/AGENTS.md.in` (what `ipe init` ships).
- **Rules & enforcement:** `PRINCIPLES.md` is the SSOT (read it — not restated here).
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

- **`tools/scripts/ipe-index locate|parity|wakeup`** — pre-built structural index;
  use before `rg` for "where is X / who calls Y / kernel gaps".
- **Backlog = GitHub issues** via `tools/scripts/github/issue-ticket.sh add|list|close`.

## PR workflow

`main` is green by construction. Branch → PR → fast gate → `gh pr merge <N> --auto
--squash` (merges when green + current). One PR per unit; check `gh pr list` first.
Versions + `CHANGELOG.md` are release-please automated from Conventional Commits —
never bump by hand.

Status: Proposed
Date: 2026-08-04

# 0055. AGENTS.md is a bootstrap-and-interrogation reference, not a stdlib mirror

## Context

`ipe init` and `ipe upgrade-agents` write the repository-root `AGENTS.md`
verbatim into every scaffolded project (`include_str!` of the root file). It is
the authoring reference an AI agent reads before writing Ipê programs.

The file has grown past 1100 lines, and most of that mass is transcribed
standard-library signatures — the web section alone is ~320 lines, the layout
section ~200 — each entry a hand-copied type signature and description.

Meanwhile the compiler already exposes its own authoritative, always-current
views of the same information:

- `ipe doc <Module>` prints each exposed value's **checker-inferred** signature
  and its doc-comment.
- `ipe doc --list` enumerates every stdlib and project module.
- `ipe check` type-checks without building or running (fast inner loop).
- `ipe verify` runs the whole project gate (format, type-check, build, test).
- `ipe explain <CODE>` explains a diagnostic.
- `ipe capabilities` prints the capability/trust model.

Two forces are in tension. The file must be **self-contained** enough that an
agent can write a correct program from zero. But every signature transcribed
into it is a maintenance and drift liability: the transcription is a second
source of truth that silently rots as the stdlib evolves. The header already
carries a temporary "modules currently `Ipe.*`-prefixed pending rename" note —
exactly the kind of statement `ipe doc --list` renders correctly on every
invocation and a static file cannot.

## Decision

`AGENTS.md` teaches only what the agent **cannot introspect**, and delegates
everything the compiler can emit authoritatively. Two layers:

**Layer 1 — static, lives in the file (target ~300 lines):**
- Surface syntax, as a dense example-driven cheat-sheet (show, don't explain).
- The interrogation workflow loop — `check` / `doc` / `explain` / `verify` /
  `fmt` — placed first, framed as "the compiler is your ground truth."
- The architecture map the type system enforces but will not teach: the shape
  model, The Elm Architecture, Task-everywhere effects, the no-CSS layout DSL.
- A thin **module index**: one line per module (name, one-line purpose,
  pure/effect tag) — names only, not signatures.
- The handful of mandatory idioms that a signature cannot express (e.g. the
  password-form pattern, input preservation across re-renders).

**Layer 2 — delegated to the live compiler:**
- Full per-function signatures → `ipe doc <Module>`.
- Error meanings → `ipe explain <CODE>`.
- Capability/trust model → `ipe capabilities`.

Two supporting rules make this durable:
- The module index in the generated `AGENTS.md` is **produced by
  `ipe upgrade-agents` from `ipe doc --list`**, so it cannot drift from the
  compiler's registry.
- When an agent **parses** command output it must pass `--json` (a stable,
  documented schema). The human and `--plain` forms are for reading, not a
  machine contract. `AGENTS.md` states this once, in the workflow section.

Alternatives rejected:
- *Keep the exhaustive mirror.* Rejected: it drifts against the compiler, is
  unmaintainable at 1100+ lines, and duplicates what `ipe doc` already emits
  authoritatively. The rename/consolidation work in progress rots it fastest.
- *Go fully minimal ("just run `ipe doc`").* Rejected: not self-contained. An
  agent with no syntax and no architecture map cannot bootstrap a first correct
  program, and cannot be assumed to reach the CLI before it has learned the
  loop. The irreducible core must stay in the file.

## Consequences

`AGENTS.md` shrinks from ~1100 to ~300 lines. Signatures and error text have a
single source of truth — the checker — so they cannot drift; the ongoing
namespace rename and kernel consolidation no longer rot the authoring doc. The
temporary "pending rename" archaeology can be deleted, since `ipe doc --list`
carries the current names.

Invariant that must continue to hold: `AGENTS.md` must not transcribe any
per-function signature or diagnostic text the compiler can emit, and its module
index must be generated, not hand-maintained. A reviewer who sees a signature
table growing back into `AGENTS.md` should reject it and point the content at
`ipe doc`.

This surfaces a dual-audience wart to fix in the same change: the embedded copy
must contain only authoring-relevant sections. Compiler/runtime development
rules (the workspace gate, contributor process) belong in `PRINCIPLES.md` and
`DEVELOPMENT.md`, not in a file shipped into every downstream project.

Depends on the `ipe doc` query path resolving every module `ipe doc --list`
advertises (a list-vs-query registry drift is tracked separately); the generated
module index relies on that path being sound.

## Conventions

ADRs describe Ipê on its own terms. This decision stands alone: `AGENTS.md` is
the language's authoring surface, and the compiler is its own reference oracle.

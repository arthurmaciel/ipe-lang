Status: Accepted
Date: 2026-08-05

# 0056. Governance-doc topology — onboarding, authoring, enforcement, ops

## Context

The project's agent/contributor governance is spread across four documents whose
responsibilities overlap and are mis-filed:

- `AGENTS.md` serves two audiences at once — it is the *language authoring*
  reference `ipe init` ships into every scaffolded project (`include_str!`), yet
  it also sits at the repo root where an agent working *on the compiler* reads
  it. One file cannot be both without shipping compiler internals into user
  projects.
- `DEVELOPMENT.md` is agent-facing dev-ops that duplicates `PRINCIPLES.md`'s
  mechanics (the two-tier gate, the write-boundary) and carries stale references
  (`crates/`, the Go/Haskell `../ipe` oracle).
- `PRINCIPLES.md` — the entrenched values/principles/rules — is diluted with
  operational procedure (two-tier-gate step lists, write-boundary paths,
  agent-lane rules) and stale reference-implementation citations.

Extends [0055](0055-agents-md-bootstrap-not-mirror.md), which established that the
shipped `AGENTS.md` is a bootstrap-and-interrogation reference, not a stdlib
mirror.

## Decision

Four documents, one purpose each; each fact lives in exactly one of them.

1. **Root `AGENTS.md` — contributor/agent onboarding for the compiler repo.**
   The crate map + pipeline, the build/test/gate commands, the kernel
   anti-drift/tripwire discipline, and pointers to the docs below. Short,
   links down. Cross-links **one-way** to the authoring reference for when a
   contributor writes `.ipe` code (stdlib, examples, fixtures).

2. **`src/ipe-cli/templates/AGENTS.md.in` — the language authoring reference.**
   Shipped by `ipe init`; the `include_str!` in `src/ipe-cli/src/init.rs`
   repoints here from the root. Syntax, the `ipe check`/`doc`/`verify`
   interrogation loop, the architecture/shape model, a generated module index,
   the mandatory idioms, and a **"Writing idiomatic Ipê"** section that states
   and briefly exemplifies parse-don't-validate, make-invalid-states-
   unrepresentable, single-source-of-truth, and fix-the-structure — so authored
   Ipê is secure/correct/sound/efficient/complete/readable on the first write.

3. **`docs/internals/dev-ops.md` — the deep operational procedures.** The
   mem-guard/disk-guard daemons and tuning, the two-tier-gate mechanics,
   end-of-mission cleanup, and the release-please/cargo-deny pipeline.
   `DEVELOPMENT.md` is **deleted**: its frequently-needed essentials condense
   into root `AGENTS.md`, its depth lands here.

4. **`PRINCIPLES.md` — values, principles, and rules only.** The operational
   *mechanics* (two-tier-gate step lists, write-boundary paths, agent-lane
   operational procedure) move to `dev-ops.md`, leaving the *rule* plus a link.
   The stale reference-implementation citations are removed: the Correctness
   principle is restated on Ipê's own terms (deterministic output; deliberate
   divergence documented, never silent) with no external "Go reference" oracle;
   the `../ipe` READ-ONLY line, the `ipedex` reference, and `crates/` (now
   `src/compiler/`) are dropped. **The entrenched values and principles
   themselves are unchanged — they are eternity clauses; only mis-filed
   procedure and stale references are touched.**

Alternatives rejected:
- *One combined `AGENTS.md`.* Leaks compiler internals into every scaffolded user
  project, and forces a 1000-line monolith against the layered-docs house style.
- *Keep `DEVELOPMENT.md`.* Duplicates `PRINCIPLES.md`'s mechanics and is not the
  filename agents look for; its content splits cleanly by depth (onboarding →
  `AGENTS.md`, deep ops → `dev-ops.md`).
- *Leave the mechanics in `PRINCIPLES.md`.* Churny operational procedure does not
  belong in a stable eternity-clause document; it drifts and dilutes the values.

## Consequences

Each fact has one home: a contributor onboarding (`AGENTS.md`), a language
authoring reference (the template), enforcement doctrine (`PRINCIPLES.md`), and
operational procedure (`dev-ops.md`). No dual-audience leak; `PRINCIPLES.md`
becomes a pure, stable statement of what Ipê stands for and its enforced rules.

Invariants to hold going forward: `PRINCIPLES.md` contains no operational
procedure and no language-authoring directions; the shipped template contains no
compiler internals; the language-authoring directions are stated once, in the
template, and the enforcement facts once, in `PRINCIPLES.md`/`dev-ops.md`. A
reviewer who sees procedure creeping back into `PRINCIPLES.md`, or a signature
table growing into either `AGENTS.md`, rejects it and points the content at its
one home.

## Conventions

ADRs describe Ipê on its own terms. This decision stands alone: the four
governance documents partition by audience and by depth, with the enforcement
doctrine kept pure and stable.

Status: Accepted
Date: 2026-07-20

# 0036. Native Rust formatter built during emission

## Context

The Rust backend concatenates fixed templates with genuinely type-directed
expression emission. The emitted text is not laid out to a canonical width — a
single deeply-nested expression can exceed the line width without wrapping. Until
now the backend closed that gap by piping every generated `.rs` file through a
`rustfmt` subprocess after emission (`project::run_rustfmt`).

Three forces make that subprocess pass a liability, in decreasing order of
severity:

1. **Soundness.** The emit path produces text, then re-parses it with an external
   tool. Any bug that drops or reorders a token between what the emitter *meant*
   to write and what lands on disk is invisible until a downstream `cargo build`
   fails — the exit-0-then-cargo-fail shape the SEAL exists to forbid. A dropped
   parenthesis around a binary operator is exactly this class of defect.

2. **Speed.** The subprocess is forked once per generated file. Spawning an
   external process, piping the source in, and reading it back dominates emit
   time, and it is pure overhead on the hot `watch` loop.

3. **Stability.** Because layout is delegated to an external formatter, the
   emitted crate is only formatting-clean if that formatter agrees with the one
   the CI `cargo fmt` gate runs. When they drift, the gate reds on emitted output
   the backend has no direct control over.

A fourth, incidental benefit: the in-browser compiler cannot spawn a subprocess
at all, so the playground currently ships unformatted Rust.

## Decision

Emit a document algebra, not a string, and render it with an owned deterministic
formatter.

Each per-node emitter returns a `Doc` — a frozen seven-variant Wadler/Leijen-style
document (`Text`, `Concat`, `Line`, `Softline`, `Nest`, `Group`, `Chain`) — built
during the same owned-IR walk that already produces the output. A single renderer
lays the `Doc` out to width-canonical bytes. Every token the emitter emits,
including every parenthesis, is carried as a `Text` leaf, so the rendered
leaf-sequence is a checkable structural invariant (the SEAL): the
whitespace-normalized concatenation of a document's leaves must equal the
whitespace-normalized string the token-level emitter produces. A dropped or
reordered token fails that property at build time, not as a downstream compile
error.

The one construct a generic flat-if-fits-else-break group cannot express is a
binary-operator chain, whose layout mixes an operator glued to a multiline
operand's closing line with later operators broken one-per-line to a single shared
indent. That earns the dedicated `Chain` variant. Its layout rule, settled
empirically against the canonical-format ground truth:

- Line 1 packs the maximal left-nested prefix that fits the width, retaining all
  leading open-parens.
- The first operator that would overflow breaks; from there every subsequent
  operator breaks one-per-line to one shared indent (the chain's begin-line indent
  plus one block step), non-accumulating and independent of paren-nesting depth —
  this is not a remaining-width test, so a tiny trailing operand still breaks.
- The sole inline exception is an operator immediately following a multiline
  operand, glued at that operand's closing-line column when it fits; a single-line
  operand ends the glue region, so the operator after it breaks.

**Alternatives rejected.** *Keep the subprocess and accept the cost* — rejected:
it leaves the token-drift class undetectable and keeps the hot loop paying the
fork. *Reimplement a general-purpose formatter* — rejected: the renderer only ever
sees the handful of construct shapes the emitter produces, so a general formatter
is far more surface than the problem needs; it renders exactly those shapes and no
others. *Fold the chain into a generic group* — rejected: a group is
all-flat-or-all-break and provably cannot render the glued-then-broken chain
layout, so the `Chain` variant is load-bearing, not convenience.

## Consequences

The emitted crate is formatting-canonical by construction, so the formatting gate
can never red on emitted output, and the browser compiler emits canonical Rust
with no subprocess. The per-file fork leaves the native emit path. The
token-preservation SEAL becomes a structural property test over an all-variant
fixture matrix, catching a missing or drifted builder at build time.

The renderer must reproduce the canonical layout for exactly the emitter's
construct shapes; the binding gate is byte-equality against the checked-in golden
corpus, over every emitted `.rs` file including multi-module outputs. The goldens
are never re-blessed to match the renderer — the renderer is fixed to match them.
A width or style change is a configuration value carried on the emit context, so a
future width change is a config edit and a deliberate golden regeneration, never a
silent layout shift.

**Invariant that must hold.** Every builder carries the exact token sequence its
construct emits — every parenthesis is a leaf, never elided by the layout logic.
The SEAL property test and the byte-golden gate together enforce this; a builder
that drops a token fails both.

## Conventions

ADRs describe Ipê on its own terms. Do not reference any prior or external
implementation, parity with another system, or project ancestry — state each
decision as a standalone Ipê decision.

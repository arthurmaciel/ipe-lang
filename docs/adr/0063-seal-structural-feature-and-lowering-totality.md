Status: Accepted
Date: 2026-09-02

# 0063. THE SEAL as a structural guarantee, not per-case patching

## Context

The pipeline carries one load-bearing invariant, THE SEAL: if the compiler
accepts a program (exit 0), the Rust it emits must `cargo build`. A SEAL
violation is the compiler exiting 0 on a program whose emitted Rust does not
compile — a representable-but-illegal pipeline state whose only symptom appears
one stage too late, at `cargo` time.

The tempting way to handle a violation is to find it in the example sweep,
diagnose the one failing shape, and add a guard keyed to that symptom. This is
unsound as a *strategy*: the trigger set is unbounded and the detector is
incidental. Two failure classes made this concrete — a runtime feature staying
off because no usage predicate covered "a type merely mentions this carrier"
(emit referenced a `#[cfg(feature = …)]` module that was not compiled in), and
emitted Rust that moved a non-`Copy` value and then read it again. A patch keyed
to one symptom silences one shape and leaves the generative cause intact; the
identical failure returns one type variant, one runtime module, or one
borrow-shape over.

The question is what structural property, if it held, would make each violation
class impossible to *represent* or impossible to *merge*, rather than merely
observed after the fact.

## Decision

Close each SEAL-violation class at its source with a totality obligation the
type system enforces, so a new acceptance path fails closed at compile-accept
time instead of open at cargo time.

- **The feature universe is a closed typed enum whose variants are exactly the
  runtime crate's declared features.** A feature the crate does not declare
  cannot be named; a declared feature with no selection path is a dead variant
  the exhaustiveness check surfaces.

- **Feature selection walks the IR type with a single total recursion that has
  no wildcard arm.** A new carrier variant in the IR type is therefore a
  compile error at the walk, not a silently-missed descent that leaves a
  feature off. The mapping from "a type that needs a runtime feature" to "that
  feature selected" is forced total by the absence of a catch-all.

- **The lowering and pattern machinery are likewise exhaustive** so that an
  un-lowered construct is a compile error in the compiler, not an emit of Rust
  that cannot type-check.

Rejected alternative — the discover-then-patch loop keyed to sweep failures. It
treats each violation as an incident rather than a class, and because the
detector is a specific example rather than a structural property, coverage is
whatever the sweep happens to exercise. It cannot bound the class it is meant to
close.

## Consequences

Adding a runtime capability that introduces a new carrier type or a new feature
forces the author, at compile time, to extend the total walk and the feature
enum — the pipeline will not build otherwise. The cost of a new acceptance path
is paid up front, in the compiler, by whoever adds it.

The invariant that must continue to hold: every path that can influence what the
emitter references — feature carriers, lowerable constructs, match arms — is
reached by a total, wildcard-free traversal over a closed universe. The moment a
wildcard arm or an open stringly-typed feature name is introduced, the guarantee
degrades back to discover-then-patch, and the same violation classes return.

A misclassification that is *too conservative* (selecting a feature or lowering
a construct that a program does not strictly need) is merely wasteful; a
misclassification that is *too permissive* is a SEAL violation. The bias is
therefore always toward selecting/lowering, never toward assuming absence.

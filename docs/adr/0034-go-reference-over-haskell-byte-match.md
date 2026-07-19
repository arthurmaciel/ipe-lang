Status: Accepted
Date: 2026-06-27

# 0034. Go reference as correctness oracle — behaviour parity over Haskell byte-match

## Context

The project started as a fork that included the Haskell Sky backend's Rust emitter
(`Generate/Rust/*`). An early correctness strategy was byte-matching the fork's
Haskell-emitted Rust output. This was circular: it proved "I reproduced my own earlier
output," and faithfully reproduced its bugs. The Haskell backend and the in-progress
Rust backend diverge structurally (host language differs; strategies and invariants
port, literal code does not). Meanwhile, the Go backend and runtime (`../sky/runtime-go/`)
are the mature, external, maintained reference that the Haskell frontend also targets.

## Decision

Abandon the fork's Haskell Rust backend as a byte-diff oracle. The **Go backend and
runtime** (`../sky`, as the `sky` binary + `go` toolchain) are the authoritative
correctness reference. PRINCIPLES.md §2 Correctness defines: "same well-typed Ipê
program + same input ⇒ Rust output matches the Go reference's observable behaviour,
ideally byte-for-byte." The Haskell `../sky` source tree is READ-ONLY reference for
language semantics (parse/canon/types/lower) and for the Haskell→Rust backend's
strategies — we port strategies and invariants, never literal Haskell.

The `../sky` Rust backend (`feat/runtime-rust`) is consulted as a strong prior:
its behaviour is a parity oracle where it covers a construct, but ipê is not required
to match its emit shape — only the Go runtime's observable output.

Deliberate divergences from Go output are: (a) impossible — Go panics on the shape;
(b) sanctioned — a stricter or more correct Rust/Unicode/modern choice, recorded in
`docs/divergences-from-sky.md` with rationale and its own golden test. A silent
divergence is never acceptable.

## Consequences

- Every golden test's `expected_go.txt` is the oracle; the Haskell-emitted Rust
  output plays no role in any gate.
- The `../sky` Haskell source is READ-ONLY; editing it to make ipê tests pass is a
  forbidden shortcut.
- Consulting the `../sky` Haskell backend or `../sky` Rust backend for "how does the
  reference handle X?" is a mandatory first step before designing any new emit shape
  (DEVELOPMENT.md §0a, MANDATORY).
- The runtime (`src/runtime/rust/`) is a vendored fork of `../sky/runtime-rust/` and
  syncs verbatim for the ~51 shared modules; divergences from that sync are the same
  sanctioned-divergence process.

# 37. No authored abrupt-failure; two-gate enforcement; provenance-attributed package scan

Date: 2026-07-21

## Status

Accepted.

## Context

Ipê advertises a language with no runtime exceptions. For that to be more than a
slogan, the compiler and runtime we ship must themselves contain no *authored*
abrupt-failure construct, and the property must be mechanically verifiable — so a
skeptic can run one command and get zero. Two facts shape the design:

- **A test's `assert!` panic is the harness working**, not a defect. The claim is
  about production code; tests are irrelevant to it and are not policed the same
  way (they keep the assert family; setup uses `-> Result` + `?`).
- **clippy cannot cover everything.** It has no `assert!` lint, it exempts
  `#[cfg(test)]` from the panic family, and it only runs on our compiled
  workspace — not on the Rust we *generate*, nor on third-party FFI Rust.

## Decision

**Authored abrupt-failure is forbidden in production Rust.** The full set:
`panic!`, `unreachable!`, `todo!`, `unimplemented!`, the `assert!`/`assert_eq!`/
`assert_ne!`/`debug_assert*` family, `.unwrap()`, `.expect()`, `.unwrap_err()`,
`.expect_err()`, `.unwrap_unchecked()`, `panic_any`, `process::abort`,
`unreachable_unchecked`, and indexing panics. `process::exit` is boundary-only
(the CLI `main`). Indexing and arithmetic overflow are covered by clippy
(`indexing_slicing`, checked arithmetic); std/dependency internal panics are the
documented boundary of the claim ("no *authored* panic", not "cannot panic").

Enforced by **two independent gates**:

1. **clippy** on our workspace — `[workspace.lints]` denies `unwrap_used`,
   `expect_used`, `panic`, `indexing_slicing`, `unreachable`, `todo`,
   `unimplemented`; `clippy.toml` `disallowed-methods` adds `unwrap_unchecked`,
   `process::abort`, `panic_any`, `unreachable_unchecked`. Tests are exempted for
   the unwrap/expect family (`allow-*-in-tests`).
2. **`tools/panic-scan`** — a proc-macro2 token scanner (not a grep, so
   string- and comment-mentions are invisible and line-split constructs are still
   caught; proven false-positive- and false-negative-free against an adversarial
   fixture). It is region-aware (skips `#[cfg(test)]`) and covers what clippy
   cannot: the `assert!` family in production, and any Rust *text* — including
   generated and third-party code.

**Package gate, attributed by provenance.** Every third-party package is scanned:

- **Pure-Ipê package** → scan the *emitted* Rust. A hit is a **compiler bug** (our
  codegen must never emit abrupt failure from pure Ipê); it fails our CI.
- **Ipê + Rust (FFI) package** → scan the *author-supplied* Rust (identified by the
  `ModuleOrigin::FfiInterface` boundary). A hit is a **user error** — a rejecting
  diagnostic. The scanner is lexical, so no package compilation is needed.

**We do not conform to tolerated insecurity, even when documented.** The exception
ledger is tracked debt whose target is zero — a documented `#[allow]` is not an
accepted state, only a temporary marker until the construct is removed. Every
class is reworked to eliminate the construct, *including* provably-infallible ones
(threaded through a `Result` channel or made infallible by type) rather than left
asserted. Current classes and their elimination plan are in
`docs/internals/rust-abrupt-failure-ledger.md`; all four are scheduled for removal,
not retention.

## Consequences

- A reviewer can attest the property with `panic-scan` + clippy; the README states
  the precise, defensible claim (authored code; std/deps excluded).
- New abrupt failure cannot land in production, generated code, or an accepted
  package — on any commit.
- The ledger burndown, `clippy::exit` for the runtime `process::exit` sites, the
  emitted-`abort()` FFI path, `math::ipe_int_div` divide-by-zero (should follow
  Elm's total `//`), and flipping the scanner CI from report-only to a hard gate
  are tracked follow-ups.

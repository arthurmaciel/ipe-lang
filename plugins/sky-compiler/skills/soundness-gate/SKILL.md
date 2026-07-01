---
name: soundness-gate
description: Use for EVERY change to the Sky compiler, Rust backend, or Rust runtime (sky-rust crates/, backend, or runtime-rust). Runs the deterministic soundness gate (fmt/clippy/build/test/Miri/E2E/Go-parity) FAIL-FAST on cheap models with auto-fix, then the expensive adversarial guardian review only on what tools can't catch. Trigger: sky-compiler:soundness-gate.
---

# soundness-gate — verify cheaply, reason expensively

The discipline for reviewing any Sky-compiler / Rust-backend / Rust-runtime change.
Cheap deterministic tools catch the easy bugs; the expensive guardian is reserved
for the irreducible soundness reasoning tools cannot do. Run it on every change.

## Why this order
- fmt / clippy(`-D`) / build / `cargo test` / Miri / golden-E2E / Go-parity are
  cheap and catch a whole class of issues mechanically. Re-deriving them with
  Opus reasoning wastes the budget.
- BUT the hardest bugs (partial-application, nested-lambda, recursive types,
  char-escape, function-in-generic-record) are **exit-0-then-cargo-fail /
  soundness-floor** issues that PASS clippy/Miri/unit-tests — they need the
  guardian's adversarial `SKY_E2E` probing + reasoning. So: don't make the
  guardian re-run rote tools; let it spend its tokens on the irreducible part.

## The process (run in order)

1. **Mechanical gate — cheap (Haiku), fail-fast.** Run the script:
   `plugins/sky-compiler/scripts/mechcheck.sh [WORKSPACE] [--fmt-fix] [--miri] [--e2e] [--parity] [--all]`
   - Works in any cargo workspace: `sky-rust` (compiler + backend) and `runtime-rust`.
   - It stops at the first failing check and prints the check + its log path
     (`$TMPDIR/mechcheck/<step>.log`). Exit 0 = all green.
   - For a sky-rust change use `--fmt-fix --e2e --parity` (and `--miri` on
     mutation/recursion crates); for runtime-rust, `--fmt-fix` + the cargo gates
     (+`--miri`) suffice (no E2E/parity there).
   - **Always pass `--fmt-fix` in the dev/agent loop**: it APPLIES `cargo fmt`
     instead of `--check`, so formatting is fixed mechanically and never becomes
     a gate failure a Sonnet fix agent wastes tokens re-formatting. CI omits it
     (keeps `--check` to enforce that contributors formatted).
2. **On mechanical FAIL → fix fast (Sonnet), loop.** Dispatch a Sonnet fix agent
   against the printed failing log, apply the minimal fix, re-run mechcheck. No
   expensive model touches it until mechcheck is ALL GREEN. (Fail-fast, fix-fast.)
3. **Adversarial guardian review — expensive (Opus, high effort), only now.**
   Given the green mechanical report, the `security-soundness-guardian` does ONLY
   what tools can't:
   - design + run **adversarial `SKY_E2E` soundness probes** — hunt the
     exit-0-then-cargo-fail class (value laundered through a type var / payload /
     generic; recursive/mutual types; literal-escape edges; closures in data),
   - review **principle adherence** (Security > Correctness > Soundness >
     Efficiency > Completeness > Readability), **parse-don't-validate**, IR
     invariants, determinism, no-`String`-errors,
   - give a PASS/FAIL verdict + concrete required_fixes.
   It does NOT re-run the rote gates (already green from step 1).

## Hard invariants the gate enforces
- `#![forbid(unsafe_code)]`; no `unwrap`/`expect`/`panic`/raw-indexing/`todo`.
- Exhaustiveness is a soundness floor: a non-exhaustive match is caught
  (SKY-T0010) BEFORE emit — Rust must never see E0004; no `_ => unreachable!()`
  fallback.
- Behavioural parity vs the Go reference is the correctness oracle.
- Reader-facing pages timeless (no project archaeology); code comments WHAT/WHY.

## In Workflow scripts
Encode steps 1–3 as the reviewGate: a Haiku `agent` runs mechcheck (+ drives the
Go-parity sweep) returning a structured report; on failure a Sonnet `agent`
fixes + re-runs; then an Opus `agent` (agentType `security-soundness-guardian`)
does the adversarial review. Cheap catching + cheap fixing; Opus reserved for
reasoning.

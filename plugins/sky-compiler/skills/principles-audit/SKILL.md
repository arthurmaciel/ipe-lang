---
name: principles-audit
description: Use when you want a recurring, whole-codebase audit against security/correctness/soundness principles — sweeping every in-scope file before a release, after a large change, or on a schedule — tracked in a living per-file ledger, and when found problems must be fixed AND build-verified, not just listed.
---

# principles-audit

## Overview

A swarm-driven, **incremental** audit of a whole codebase against the project's
principle order — **security > correctness > soundness > efficiency >
completeness > readability** — recorded in a persistent per-file ledger
(`<docs>/CODE-REVIEW.md`). Built to run *often*: each pass re-audits only what
changed since that file's last audit, so the ledger never goes stale and no human
has to keep up.

**Core principle:** a finding isn't *resolved* until it is **fixed and the
integration build is green**, OR explicitly **deferred with a reason + date**.
"Authored a fix" ≠ "fixed".

## Modes

Two modes share **one mechanism** (the loop, the deterministic tier, the
specialist re-pass, the verification gates, the anti-race fix protocol, the
ledger — everything below). They differ only in **scope** and **how many tokens
they spend per file**. The default is cost-bounded; exhaustive is opt-in for when
tokens are plentiful and you want maximum depth on demand.

| | **cost-effective** (default) | **tokens-available / exhaustive** (opt-in) |
|---|---|---|
| **Trigger** | nothing / `mode=cost-effective` | `mode=exhaustive` (aliases `tokens-available`, `deep`); `budget=med\|high\|max` scales the depth knobs |
| **Scope** | delta only — files changed since their `Last audited`, **with** the audit's-own-fix exclusion (§1) | **ALL in-boundary files**, leaf-first; **ignores** delta-scoping **and** the audit's-own-fix exclusion (re-reads even files whose only change was a prior fix) |
| **Deterministic tier (§1b)** | first, free, seeds shortlist | **same** — runs first; its hits seed the exhaustive shortlist too |
| **Broad sweep (§3)** | 1 generalist agent/file, six principles at once | **same** breadth pass over the full set |
| **Specialist pass (second pass)** | high-risk subset, 1 adversarial agent/file | **deepened** — see knobs below |
| **Cost vs default** | 1× | **~5–12×** (scales with `budget`; capped so it terminates) |
| **Same invariants** | ✅ | ✅ — deterministic-tier-first · anti-race fix · build-gate before ✅ · ledger recording · boundary discipline (Go / `app/` / `Main.hs` out) all unchanged |

### Exhaustive depth knobs — what each extra token *buys*

The point of exhaustive mode is **not re-reading once more** — it is spending the
budget on independent perspectives + adversarial verification + iteration until
the high-risk set stops yielding. Each knob is mapped to the **finding-class a
single broad-sweep pass misses**; scale each with `budget` up to its cap.

| Knob | What it does | Finding-class it catches (that single-pass misses) | Scale (med → max) | Cap |
|---|---|---|---|---|
| **Perspective-diverse lenses** | per high-risk file, fan out N specialists each with ONE lens — `injection` / `authz` / `crypto-timing` / `memory-safety+UB` / `DoS+resource` — instead of one generalist | the *outranking* principle a six-at-once generalist under-weights (e.g. the `console.rs` Bearer timing oracle the broad sweep skimmed past) | 2 → 5 lenses | 5 (the lens set) |
| **Adversarial N-vote** | each candidate finding is re-judged by N independent skeptics; **majority-refute kills it** | false positives that survive ONE skeptic — e.g. a "missing" check actually applied at a middleware layer | 3 → 5 votes | 5 (odd, to break ties) |
| **Loop-until-dry** | re-sweep the high-risk set until **K consecutive rounds find nothing new** (a fix reshapes a file → new round) | second-order issues unlocked only *after* an earlier fix changes the file — which delta-scoped runs never revisit | K = 1 → 3 | max 5 rounds total |
| **Completeness critic** | one final read-only agent asks "what modality / trust boundary / file-class did we NOT cover?" against the boundary inventory | an entire untouched class — a boundary nobody was assigned a lens for | always on | 1 pass |

Confirmed findings (survived the N-vote) flow into the **same** step-4 fix loop
(anti-race + build-gate) and the ledger. A clean verdict from a lens is recorded
as `—`, same as the default — exhaustive depth makes a `—` *more* trustworthy,
never an excuse to invent findings.

### Cost shape & budget scaling

Rough multiplier vs default ≈ `lenses × votes` on the high-risk subset, repeated
≤ rounds, plus one critic. `budget=med` ≈ 5×, `budget=high` ≈ 8×, `budget=max` ≈
12×. The caps (5 lenses, 5 votes, 5 rounds) guarantee termination regardless of
budget — never an unbounded loop. Below the high-risk subset, the broad sweep is
still 1 agent/file, so the multiplier applies to ~15-20 files, not all ~130.

## When to use / not

- **Use:** before a release; after a large change / merge / upstream sync; on a
  cadence; when you want one durable place that answers "is every file sound, and
  what's outstanding?"
- **Don't:** for a single file or PR (just review it inline); for mechanical
  lint-able rules (automate those).

## The ledger — `<docs>/CODE-REVIEW.md`

One row per in-scope file, grouped by component, **leaf-first** within each
(foundational/depended-upon files first; aggregators/`mod.rs`/facades last).

| File | Purpose | Risk | Last audited | Findings | Suggested fix | Fix |
|---|---|---|:-:|---|---|:-:|

- **Last audited** `YYYY-MM-DD` — staleness signal; empty = never audited. A
  non-empty date *is* the "agent reviewed it" mark (no separate checkbox).
- **Findings** — concrete issues, fn/line cited; `—` when genuinely clean.
- **Fix** — `✅`(fixed+built) · `⏸️`(deferred, +reason) · `➖`(no-defect) ·
  `❌`(attempted, won't verify), each with a date.

There is **no "human review" column** — a recurring autonomous audit makes it rot.

## The loop

1. **Re-index + scope the delta.** `skydex update --repo .`; compute which
   in-scope files changed since their row's **Last audited** (git diff) — only
   those (plus never-audited rows) need work this pass. First run = all files.
   **Audit's-own-fix exclusion (the #1 repeat-run cost leak):** a file whose
   ONLY post-`Last audited` commit IS the audit's own recorded fix (the
   ledger's `Fix` cell already cites that commit) must NOT re-trigger — the
   change *is* the recorded resolution, not new unreviewed code. Re-audit it
   only when a LATER, unrelated commit touches it. Without this, the fix commits
   from the previous run re-flag nearly every file (e.g. 2026-06-19's fixes made
   a naive since-date show ~59 files when the true delta was a handful).
1b. **Deterministic tier FIRST (≈free, before any LLM).** Run
   `sky-rust-backend:quality-audit` — its `[SECGREP]` (timing-oracle /
   injection-sink / un-guarded `reqwest::Client`) and `[SUPPLY]`
   (`cargo-audit` CVE/unmaintained + `cargo-deny` bans/sources) tiers catch the
   whole CVE + secret-compare + sink class at zero LLM cost. Feed their hits into
   the high-risk shortlist; the swarm then spends tokens only on human-judgment
   security the greps/clippy provably can't decide. (Install once:
   `cargo install cargo-audit cargo-deny`; both are guarded — skip-with-note if
   absent.)
2. **Inventory (new/empty rows only).** A swarm skims each file → `Purpose` +
   `Risk`; insert leaf-first into its component section. Update the scope inventory.
3. **Review.** A swarm reads each in-scope-delta file **line-by-line** against the
   six principles; fills `Findings` + `Suggested fix`; stamps **Last audited**.
   `—` (clean) is a valid, valuable result — never pad.
4. **Fix — under the anti-race protocol.** Fixer agents author **DISJOINT files
   only, never build, never touch the ledger or shared seams**. The **orchestrator
   runs the per-component integration build + tests**, marks `Fix` only on green,
   reverts/defers anything that breaks it, and **commits per component**. (This is
   `sky-rust-backend:autonomous-swarm`'s protocol — reuse it.)

A small helper parses each swarm's `{file, …}` JSON and writes the cells (don't
re-improvise the table edit each run).

## The second pass — security/soundness specialist re-review (high-value, ~10% cost)

The step-3 sweep is **one agent per file, all six principles at once** — cheap,
parallel, and good at *cross-principle* findings (a soundness bug that's also a
security bug). But a generalist reviewing a whole file can under-weight the two
principles that **outrank all others**. So after step 3, run a focused
**second pass** over only the **high-risk subset** — files that touch
**auth / secrets / crypto / cookies / SQL / network input / `unsafe` / FFI /
codegen-that-emits-code** (typically ~15-20 files, not all 130).

- **Why not one-agent-per-principle instead?** That re-reads the *whole*
  codebase once per principle (~6× the file-reads + worse parallelism) AND
  fragments cross-principle reasoning. The two-pass shape gets the specialist
  depth for ~10% of that cost: breadth once (step 3), then depth only where the
  blast radius is largest.
- **Adversarial lens.** Each specialist agent is **read-only** and prompted to
  *break* its files: auth/CSRF bypass, secret leakage into logs/errors,
  injection (SQL / shell / path / header / log), panic-from-untrusted-input,
  `unsafe`/UB, TOCTOU/races on shared mutable state, missing-bound DoS, weak
  crypto/RNG, **non-constant-time secret compares** (`==`/`!=` on tokens — see
  the CLAUDE.md learning). It returns each finding with a *concrete exploit* +
  severity + fix, or declares the file sound (a clean verdict is valuable).
- **This pass routinely corrects the broad sweep** — both false-positives (a
  per-file reviewer missing a protection applied at a *middleware layer*) and
  false-negatives (a timing oracle the generalist skimmed past). The 2026-06-19
  run found both.
- Confirmed findings flow into the same step-4 fix loop (anti-race + build-gate)
  and the ledger; an exploit that survives an *independent* skeptic is real.
- **Exhaustive mode deepens THIS pass** (not a separate procedure): it swaps the
  single adversarial agent for the perspective-lenses + N-vote + loop-until-dry +
  completeness-critic knobs in `## Modes`. Same subset, same fix loop — more
  independent eyes, run until dry.

## Verification gates (a fix is ✅ only past these)

| Component | Gate (what it proves) |
|---|---|
| Rust runtime | `cargo build/test --features full` (compiles + unit-total) **AND** a feature-MINIMAL example build (catches `--features full`-only resolution) |
| Haskell codegen | `cabal build exe:sky` + build ≥1 example per shape on `--backend rust` (codegen emits compiling Rust) |
| Rust tools | `cargo build` per crate |
| Scripts | `bash -n` / `py_compile` / `node --check` + a functional smoke |
| Docs/prose | claims re-checked against the code they describe |

Build green = it compiles. Behavioural Go≡Rust parity is the CI sweep's job — say
which gate proved what; don't overclaim.

## Common mistakes

- **Letting fixer agents build.** They share one `CARGO_TARGET_DIR`/`.skycache` —
  parallel builds clobber + `resource busy`. Authors author; the orchestrator
  builds once per component.
- **Trusting a `--features full` green.** A fix that adds a crate dep to a
  shared/always-compiled module passes `--features full` but breaks a
  feature-minimal project (E0433). Always build a minimal example too.
- **Marking ✅ on "authored".** Only after the integration build is green.
- **Re-auditing everything every run.** Use **Last audited** + git diff; that's
  what makes it cheap enough to run often.
- **Padding `—` rows with invented findings.** Clean is a result.
- **One giant fix-swarm, one giant build.** Batch by component; build + commit
  per batch so a break is bisectable and progress is durable.

## Reference

Reuses `sky-rust-backend:autonomous-swarm` (anti-race protocol) and the project
boundary + build env in `runtime-rust/CLAUDE.md`. The deliverable lives at
`runtime-rust/docs/CODE-REVIEW.md`.

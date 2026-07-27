# Progressive-Development Pipeline — Algorithm Spec

**Version: v4 (2026-07-07)** — living document; bump the version + add a Changelog
row on every tuning change. The numbers below are *starting points* chosen from
the cost model, to be fine-tuned against measured land-rates and wall-clock.

---

## 0. The cost model (the invariant that shapes every decision)

Gating steps, cheapest → most expensive (measured/observed on this box):

| Step | Cost | Currency |
|---|---|---|
| ipe type-check / lowering (one example) | sub-sec–secs | compute (~0 tok) |
| `remeasure.sh` sweep (incremental/cached) | ~6–30 s | compute |
| no-panic fuzzer (30 iters) | ~1.5 min | compute |
| `cargo clippy -p <crate> --all-targets -- -D clippy::cargo -D clippy::complexity -D clippy::correctness -D clippy::pedantic -D clippy::perf -D clippy::style -D warnings` (sccache-warm) | ~30 s–2 min | compute |
| adversarial **review** (1 Opus agent, no build) | ~1–3 min | **tokens (~$0.5)** |
| **audit** (1 Opus agent, all landed diffs) | ~1–3 min | **tokens (~$0.5–1)** |
| `cargo clippy --workspace -- -D clippy::cargo -D clippy::complexity -D clippy::correctness -D clippy::pedantic -D clippy::perf -D clippy::style -D warnings` (incremental, warm) | ~min | compute |
| **`cargo test --workspace`** | **min warm → ~50 min cold** | **compute (the ceiling)** |

Load-bearing facts:
1. **`cargo test --workspace` is the serial bottleneck** — memory-bound (mem-guard
   6 GB/proc → one at a time on this box). System throughput ≈ 1 / gate-time,
   *regardless* of authoring width.
2. **Authoring parallelizes** (API/token-bound); **gating does not** (compute-bound).
3. **Failures are dominated by adversarial REVIEW, not the gate.** `cargo test`
   cannot catch unsoundness (a hack compiles + tests green — the Grid case). So
   review MUST run *before* the expensive gate.
4. **cargo/clippy are crate-level, not file-level.** `-p <crate>` is the finest
   scope. Unchanged crates can't gain new lints (per-source-deterministic), so
   final clippy is safely "changed + dependents" (which incremental `--workspace`
   already is). **`cargo test` is NOT scopeable** — a fix can break a distant
   crate's test, so the final test stays `--workspace`.
5. **Per-lane builds need a target per lane** → only disk-bounded via shared
   **sccache** (deps compiled once, reused; each lane recompiles only its crate).

---

## 1. Roles & models (per stage)

| Stage | Model | Notes |
|---|---|---|
| Triage | Opus | classify blockers → mechanical / guardian-{typesystem,runtime,security}; dedup |
| Design | Opus | **guardian only**, class-routed; produces a written plan, no code; early-out if unsound |
| Implement | Sonnet | follows the design (or the reference, for mechanical); writes code + regression test |
| Review | Opus | class-routed adversarial refute; default REJECT |
| Reconcile | Opus | union-merge conflicts (mechanical registry appends) |
| Audit | Opus | end-of-run meta-review of landed commits for hacks |

Token logic: Opus for the *reasoning* (design, review, audit); Sonnet for the
*typing* (impl). Never Opus for implementation.

---

## 2. Per-item pipeline

```
MECHANICAL:  [no design] → Sonnet impl → clippy -p → review → ipe-verify → INTEGRATE
GUARDIAN:    Opus design(+early-out) → Sonnet impl → clippy -p → review → ipe-verify → INTEGRATE
```

**Per-lane self-checks (parallel, sccache, per-lane target — NO workspace test):**
1. `cargo clippy -p <changed-crate> --all-targets -- -D clippy::cargo -D clippy::complexity -D clippy::correctness -D clippy::pedantic -D clippy::perf -D clippy::style -D warnings` — compile + lint the changed
   crate. Bail on error/warning. (~1 min)
2. **review** (Opus, reads diff, no build) — the *dominant kill*; runs before any
   ipe rebuild so doomed fixes don't pay for a compiler build. Bail on REJECT.
3. **ipe-verify** — build ipe + rerun the example: blocker cleared + example
   builds/runs. Priciest per-lane step → last, only on review-survivors.

**Order rationale:** cheapest-that-kills-most first. clippy-p catches
non-compiling; review catches unsoundness (most failures); ipe-verify confirms
it actually works — each gate strictly cheaper-before-costlier.

**When a per-lane filter fails (who owns it):**
- `clippy -p` (compile error / lint) → **Sonnet iterates IN-LANE** (fix + rerun,
  cap ~3). Implementation-level, Sonnet's job, cheap, has the context. No restart.
- `ipe-verify` (example blocker not cleared) → **Sonnet iterates IN-LANE**
  (cap ~3): "make the target actually compile." Still failing after the cap ⇒ the
  DESIGN is wrong → bounce to design (consumes an attempt).
- **review REJECT** (unsound / hack / wrong approach) → **NEVER a Sonnet patch.**
  A soundness rejection means the *approach* is wrong; patching it breeds hacks.
  It consumes one of the 2 attempts; the NEXT attempt **re-designs (Opus)** with
  the reviewer's critique as resume context, then Sonnet re-implements.

Rule: failures WITHIN Sonnet's competence (compile / lint / make-it-work) →
iterate in-lane; failures ABOVE it (soundness / design) → bounce to design as a
fresh attempt. A good design keeps the in-lane loops short (impl passes clippy-p
+ verify with minor touch-ups; review only rejects on genuine unsoundness).

---

## 3. Parallelism & batching (starting numbers)

| Knob | Mechanical | Guardian | Why |
|---|---|---|---|
| Author lanes | **3** (→4 if land-rate holds) | **up to 3** (disjoint) | mech batches union-reconcile at high p, so +lanes → bigger batches → the serial gate **amortizes over MORE items** (throughput scales with lanes, not gate-bound); guardian is low-p + conflict-prone → lanes don't help past the survivor buffer |
| Lane disjointness | union-OK (registry appends) | **disjoint subsystems required** | deep-logic diffs don't union-reconcile; two guardians on the solver clash |
| Integrate batch | **3–4** | **1–2** | batch pass ≈ p^k; guardian p≈0.5–0.7 → batch small; mech p≈0.95 → 3–4 (0.95⁴≈0.81) |
| On red batch | bisect (log₂k gates) | near-1 so no bisect | attribution |
| Integration cadence | pipeline, continuous, union-reconcile | round-based: author disjoint round → review-filter → integrate survivors 1–2 at a time, **re-gate between** | guardian BASE moves per landing (staleness) |
| Resuming attempts | 2 | **2** | 3rd try has diminishing returns (failures are review-dominated; a twice-refuted fix rarely lands on try 3) → escalate to **phase-4** (human Opus→Sonnet→Opus) instead |

**Do NOT batch ~10.** p^10 collapses (0.9¹⁰≈0.35); a red batch loses attribution
(bisect or revert-all). Small batches keep the gate attributable + the interaction
surface tiny.

**Lane count is gate-bound on this box** — the serial `cargo test` caps throughput,
so >3 lanes just builds a stale queue. The leverage is the *ordering* (§4), not width.

### 3.1 Parallel guardian — parallel author, serial gate (PLANNED, flag-gated)

Guardian is serial today (one item design→impl→review→gate at a time). The plan
parallelizes **authoring only**; the gate stays serial (correct, not a limitation).

**Shape** — batch N items (`GUARDIAN_LANES`, default **1** = today; opt-in 2):
- **Phase A (parallel author):** each item runs design→impl→review concurrently,
  each in its **own worktree + own cargo target** (`GUARDIAN_TARGET-$i`), writing
  ACCEPT/REJECT/ESCALATE + branch to a per-item result file.
- **Barrier:** wait for all authors.
- **Phase B (serial gate):** gate only ACCEPTED items, one at a time — merge +
  nextest + doctest + clippy + fuzz on the single shared `GATE_TARGET`, revert to
  `$gpre` on RED (per-item safety unchanged).

**Why the gate stays serial (not a limitation):** cargo locks the target dir,
merges mutate one HEAD, and — Amdahl — the gate is the wall-clock floor, so
parallelizing it buys nothing. Parallelism only helps the authoring portion.

**Safety mechanisms (where the risk lives):**
1. **Per-lane targets** added to `KEEP_TARGETS` via prefix-match so `reclaim_disk`
   never deletes an in-use target.
2. **Memory bound** — N concurrent `cargo build -p ipe` peak ~2–4 GB each; N=2
   safe on the 15 GB box (mem-guard backstop). Disk = N×~10 GB → check floor
   before spawning.
3. **Serial worktree setup** before launching authors (no concurrent
   `git worktree add` races).
4. **Disjoint-crate batching** — only co-schedule items whose descs touch
   *different* crates (parse the crate hint from the desc). This is the
   "integration cost < serial cost" gate: same-crate items stay serial so
   parallel impls don't collide and serial merges rarely conflict; on a merge
   conflict, that item re-queues (`guardian_failed`), never corrupts.
5. **Mock-test the concurrency flow** (stub `agent()` with an echo) to validate
   launch/barrier/result-collection/serial-gate **before** any real agent spend.

**Expected win:** ~2× on authoring (the 10–45 min design+impl+review overlaps);
gate stays serial → net wall-clock **~25–40%** (depends on the author:gate ratio).

**Rollout:** flag-gated (`GUARDIAN_LANES=1` default → zero risk) → mock-test →
**supervised** first parallel run (watch one batch land) → default to 2.

**Interaction with design-reuse (§2):** attempt-1 items are the parallel candidates;
a retry (design reused) is cheap enough to keep serial if it simplifies scheduling.

---

## 4. Gate frequency — the whole point

| Gate | Now (per item) | v1 (per item) |
|---|---|---|
| `cargo test --workspace` | **~8–12** (agent self-gate iters ×3 attempts + master + per-lane) | **~0.5–1** (once per integration batch, on survivors only) |
| `clippy --workspace` | per lane + per gate | once per batch, incremental (changed+dependents) |
| `clippy -p <crate>` | — | per lane (cheap pre-filter) |
| review | after self-gate | **before** any workspace build |

Rules:
- **`cargo test --workspace` runs ONCE per integration batch** — never inside an
  authoring agent, never doubled (self-gate + master), never before review.
- Authoring agents self-check with **`clippy -p` + ipe-verify only**.
- The final **clippy is incremental `--workspace`** (= changed + dependents; the
  sound minimum). The per-lane `clippy -p` pre-filters so it's cheap confirmation.
- **Bisect** a red batch; don't revert-all.

---

## 5. The full loop (autopilot / orchestrate / run)

```
autopilot (until 2 dry passes = converged):
  cycle:
    0. safety: mem-guard up? disk ≥ floor (reclaim; graceful-stop if still critical)?
       stop-flag? runaway backstop (MAX_CYCLES=100)?
    1. convergence check: HEAD unchanged since last cycle AND 0 actionable? dry++; 2 → STOP.
    2. MECHANICAL burn (if actionable mechanical):
         orchestrate: 3 author-only lanes (Sonnet, sccache, per-lane target)
           each: impl → clippy -p → review → ipe-verify   (survivors only)
         integrate batch 3–4: union-reconcile → ONE cargo test --workspace
           + incremental clippy --workspace + fuzzer(30). red → bisect. keep green.
         continue (loop)
    3. MEASURE (no mechanical left):
         audit landed commits (Opus) — only if HEAD advanced since last audit;
           VIOLATION → STOP for human.
         remeasure sweep (cached) ; fuzzer(30) — a panic files a guardian-runtime item.
         triage (Opus) — classify + dedup new blockers into the queue.
         new mechanical? → continue.
    4. GUARDIAN burn (only guardian left):
         pick ≤3 DISJOINT-subsystem items (MAX_GUARDIAN/cycle)
         each lane (parallel, per-lane target, sccache):
           Opus design (class-routed) → early-out if unsound → escalate
           Sonnet impl → clippy -p → review (Opus, class-routed) → ipe-verify
         integrate survivors 1–2 at a time (serial, re-gate between):
           merge → ONE cargo test --workspace + incremental clippy + fuzzer(30)
           green → LANDED ; red → revert to captured pre-sha, save resume
         per item: 2 resuming attempts (each resumes from saved design + failure),
           then ESCALATE to phase-4 / human (artifact preserved).
```

---

## 6. Safety invariants (non-negotiable, already implemented)

- **Revert to a captured pre-merge SHA**, never `git reset --hard HEAD~1`; never
  gate an in-progress merge (require the reconcile to have committed).
- **Disk**: bounded target set + `reclaim_disk` below floor + **graceful stop
  before any build** if still critical (no mid-build ENOSPC).
- **mem-guard** required (auto-dispatched).
- **Suppression / convergence**: ESCALATED/BLOCKED, or mechanical 2×-ATTEMPTED /
  guardian 3×, drop from actionable → the loop converges.
- **No git while a run holds the checkout** (human side).
- **Output is human-readable**: agent stream-json rendered via `render-stream.sh`;
  run logs untracked+gitignored; resume reasons rendered (not raw json).

---

## 7. Open knobs to tune (what versioning fine-tunes)

- Author-lane counts (2 / 3), integration batch sizes (2–4 / 1–2), attempt caps.
- Gate timeouts; fuzzer iter count at the gate vs measure.
- Whether mechanical also gets a design-lite step for borderline items.
- **Bigger box**: the one knob that unlocks more lanes is **concurrent gate
  targets** (RAM+disk for 2–3 parallel `cargo test`). Then lanes follow; here the
  serial gate caps it.
- Guardian subsystem-partitioning heuristic (how to prove two items are disjoint).
- **Liveness UX**:
  - `log()` timestamp shortened to `HH:MM`.
  - `spin <label> <cmd>` around the SILENT long ops (`cargo test`/clippy gate,
    remeasure) — ASCII `|/-\` + elapsed on a tty, a pulse line every ~20 s in a
    file/background — so a 20-min gate never reads as stalled. (Agent calls
    already stream via render-stream.)
  - **Status header** — a gitignored `progdev-status.txt` autopilot rewrites on
    every phase transition; watch.sh renders it fixed at top; also printed as a
    compact heartbeat line per transition; `cat` it any time:
    ```
    task    [guardian-typesystem] 12-skyvote: IPE-T0012 record has no field …
    type    guardian/typesystem   attempt 1/2   started 04:06
    phase   REVIEW    model opus    elapsed 3m10s
    ```
    Helpers: `set_task(type, desc)` at item start; `set_phase(phase, model)` at
    each transition (design=opus · impl=sonnet · pre-filter=∅ · review=opus ·
    integrate/gate=∅). Every value is already known to the script — it routes the
    model per phase and the class per item — so this is pure recording, no
    detection.

---

## Changelog

- **v5 (2026-07-07)** — filed §3.1 **parallel guardian** (parallel author, serial
  gate; flag-gated `GUARDIAN_LANES`, mock-tested, supervised rollout). Records the
  batch of shipped changes it builds on: guardian E2BIG fix (design→file, not argv);
  **design-reuse on retry** (attempt-1 saves the plan, a retry reuses it + gets the
  rejection reason — cuts the biggest cost lane); rg-shim for agents (headless
  `claude -p` ignores PreToolUse hooks → PATH shim); persistent per-cycle **cost
  ledger** (survives /tmp overwrite); **gate on nextest + `--doc` + nightly
  `-Zthreads=8` + mold** (parallel frontend speeds clippy + test builds, mold speeds
  link); clippy `--no-deps --jobs 4`, `--all-targets` dropped; nightly clippy drift
  reconciled (2 `pedantic` fixes) so the nightly gate is clean, not permanently red.
- **v4 (2026-07-07)** — added the status-header design (progdev-status.txt + watch.sh fixed header + heartbeat markers): task / type / start / phase / model / attempt, all script-known.
- **v3 (2026-07-07)** — filed the per-lane failure ownership rule (clippy-p /
  ipe-verify → Sonnet iterates in-lane, cap ~3; review REJECT → re-design as a
  fresh attempt, never a Sonnet patch). Added the liveness-UX item (HH:MM
  timestamps + a spinner/pulse around silent gates).
- **v2 (2026-07-07)** — mechanical author lanes **2→3** (→4 if land-rate holds),
  mechanical batch **2–4→3–4**: mechanical batches union-reconcile at high p, so
  more lanes → bigger batches → the serial gate amortizes over more items
  (throughput scales with lanes — the earlier "gate-bound at 2" was wrong for the
  *batched* mechanical case; it only holds for per-item gating). Disk is not the
  binding constraint (author-only lanes + sccache + proven 3-build+1-doc
  concurrency + 47 GB). Guardian resume attempts **3→2**: 3rd try has diminishing
  returns on review-dominated failures → escalate to phase-4 instead. Guardian
  lane cap stays low — for p^k + conflict + attribution reasons, NOT disk.
- **v1 (2026-07-07)** — initial spec. Derived from the cost model + the observed
  ~8–12→~1 `cargo test` reduction, review-before-gate (failures are review-
  dominated), sccache-backed per-lane `clippy -p` + ipe-verify, small
  attributable integration batches, mechanical-vs-guardian differentiation.
  Not yet implemented — current code still self-gates with workspace `cargo test`.

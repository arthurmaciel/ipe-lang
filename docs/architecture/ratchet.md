# Ratchet — gated fresh-context autonomous burndown

Ratchet is our variant of the "fresh-context loop" (a.k.a. the Ralph technique):
a shell loop that spawns a new `claude -p` process each iteration, reads durable
state from disk, and lands ONE backlog item as a green committed increment — or
discards its work and logs why. Progress accumulates in git + `backlog.md`, not
in a growing conversation.

**Why "Ratchet":** a ratchet-and-pawl only advances, never slips back. That is
the one invariant we enforce above all else — the tree only moves forward through
a green gate; a failed iteration resets to the last green commit. No regression
is possible by construction.

## Files
| File | Role |
|---|---|
| `scripts/ratchet.sh` | the loop = OUTER safety harness (disk/mem/budget/iteration-cap/kill-switch/single-writer) |
| `scripts/ratchet-prompt.md` | the per-iteration playbook the fresh agent executes (pick → fix → gate → land-or-discard → log) |
| `docs/architecture/backlog.md` | the work list; only **sweep-front** / **[ratchet-safe]** items are eligible |
| `docs/architecture/ratchet-log.md` | append-only per-iteration outcomes + attempt counts (created on first run) |
| `docs/architecture/ratchet-escalations.md` | items the loop refused (excluded class) + fix sketches (created on first run) |

## Guardrails (why this is safe to leave unattended)
1. **Gate-on-green commit (the pawl).** An iteration commits only if `cargo test --workspace` + `cargo clippy -D warnings` pass on the isolated gate target (and, for a sweep blocker, the example's original diagnostic is gone). Red → `git reset --hard`, log the reason. The tree is always green between iterations.
2. **Stop-and-escalate on the hard class.** Security tier (Secret/SqlFragment/CSRF/fuzzer), feature gaps needing type-system+backend+runtime co-design (e.g. erased-`any` payloads), oracle *divergences*, `unsafe`/FFI, or anything relaxing a soundness gate → the iteration refuses, writes an escalation, and moves on. This is the ex27 lesson encoded: a fresh agent under "pressure-cooker" pressure will otherwise produce a plausible **unsound** hack. The loop only touches mechanical, reference-backed wiring.
3. **Per-item attempt cap (anti-thrash).** 3 failed attempts on one item → mark BLOCKED, escalate, move on. Prevents infinite grinding on an intractable item.
4. **Single writer.** A lockfile + a dedicated `ratchet/run-*` branch; no concurrent swarm during a run (concurrent commits/builds race — proven by the shared-target stale-rlib thrash we hit). Commits land on the branch; a human fast-forwards to master after reviewing the run.
5. **Resource preconditions every iteration.** mem-guard alive, free disk ≥ 15 GB, timeout-bounded builds, no background processes. The CLAUDE.md non-negotiables become loop invariants.
6. **Kill-switch + caps.** `touch ratchet.stop` for a clean exit; `RATCHET_MAX_ITERS` and per-iteration `timeout` bound the blast radius; `--once` validates a single iteration before unleashing the loop.
7. **Isolated gate target.** Always `~/.cache/master-gate-target`, never the shared lane target — the stale-rlib thrash cannot fool the gate.
8. **Idempotent / crash-safe.** All state is on disk; a crash mid-run just means the next iteration re-reads `backlog.md` + `ratchet-log.md` and continues. Nothing lives only in memory.

## Operating it
```bash
# 0. mem-guard must be running; tree clean; on master (or your base).
scripts/ratchet.sh --once          # validate ONE iteration, inspect the branch
scripts/ratchet.sh                 # run the loop (default 20 iters)
touch ratchet.stop                 # stop after the current iteration
# review the run, then fast-forward:
git switch master && git merge --ff-only ratchet/run-<ts>
```

## The CLAUDE.md cost — the central economic question

**The problem.** A fresh `claude -p` process auto-loads project memory at
SessionStart: the repo `CLAUDE.md` (this project's is very large — the app-shape
matrix, the full stdlib reference, env-var tables, the effect-boundary tier
list, …), the global `~/.claude/CLAUDE.md`, the memory index, and skill blobs.
For an *interactive* session that pays off — you might do anything. For a Ratchet
iteration whose entire job is "wire one known-missing kernel," ~90% of that
preamble is dead weight, and — this is the crux — it is billed **cold, at full
input rate, on every iteration**, because each `claude -p` is a new process with
a cold cache.

**The math that matters.** Call the fixed preamble `F` and the per-item working
context `W`.
- A **warm long session** pays `F` roughly once (cached at ~10% after the first
  turn) but accumulates an unbounded conversation tail `T` that is re-sent every
  turn — long sessions die of `T`, not `F`.
- A **cold Ratchet loop** pays `F + W` fresh per iteration and has **no tail**
  (`T = 0`). Over N iterations: `N·(F + W)` vs the warm session's
  `F + Σ(W_k + T_k)`.

So Ratchet wins decisively on the tail (its whole point) but can **lose on the
fixed part** if `F` is large and N is high: a 25k-token `F` over 50 iterations is
1.25M tokens of preamble alone. The entire economic game is **shrinking `F`** and
**keeping the cache warm across iterations.**

**What we do about it (in priority order):**

1. **Don't feed the giant CLAUDE.md to the loop.** `ratchet-prompt.md` is a lean,
   purpose-built instruction (~1–2k tokens) that distills only the non-negotiables
   an iteration needs (boundary, gate commands, escalation rules, safety). Run the
   loop so the big project `CLAUDE.md` is **not** auto-loaded — e.g. from a working
   context without it, or via a system-prompt override — and let the iteration pull
   the ~5% of project detail it actually needs by *reading specific files as tool
   calls* (backlog.md, the reference file, the crate it's editing). You pay for
   what an item touches, not the whole manual. (The global `~/.claude/CLAUDE.md`
   still loads; it is comparatively small. Consider a loop-specific minimal global
   if you run this heavily.)

2. **Keep the fixed prefix byte-stable and iterate fast to ride the prompt cache.**
   Anthropic's prompt cache keys on an exact prefix with a **5-minute TTL**. If the
   preamble (`ratchet-prompt.md` + whatever system content) is identical each
   iteration AND iterations start < 5 min apart, the fixed part is served at ~10%
   across *separate* invocations — turning cold reloads back into warm reads. Hence
   `RATCHET_COOLDOWN` defaults to 20s (well under the TTL), not minutes.

3. **Read volatile state via tool calls, never in the cached prefix.** `backlog.md`
   and `git status` change every iteration; if they sat in the system preamble they
   would bust the cache each time. The playbook has the agent *read them as its
   first actions*, so the volatile bytes land after the cacheable prefix and don't
   invalidate it.

4. **One item per iteration, small `W`.** Narrow scope keeps the variable context
   small, so even a cold iteration is cheap. Batching would inflate `W` and raise
   the odds of a red gate (discarded work = pure waste).

**Bottom line.** Ratchet is cheaper than a long warm session *iff* you shrink `F`
and hold the cache — otherwise the repeated cold preamble can cost more than the
tail you were trying to avoid. The lean prompt + stable-prefix + fast-cadence +
one-item design is what turns the technique from "surprisingly works" into
"cheaper AND safe." Measure it: `claude -p --output-format json` reports per-call
token usage; sum it over a `--once` run and compare against a warm session doing
the same item before trusting the loop with a long unattended run.

## What Ratchet is NOT for
Design decisions, security-critical code, oracle divergences, anything needing
human/guardian judgment. Those are escalated, never brute-forced. Ratchet grinds
the safe, reference-backed 80%; the risky 20% stays with a human + the guardian
review. That division of labour is the whole point.

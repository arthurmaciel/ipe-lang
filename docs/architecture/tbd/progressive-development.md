# Progressive Development — gated fresh-context autonomous burndown

Progressive Development is our variant of the "fresh-context loop" (a.k.a. the Ralph technique):
a shell loop that spawns a new `claude -p` process each iteration, reads durable
state from disk, and lands ONE backlog item as a green committed increment — or
discards its work and logs why. Progress accumulates in git + `BACKLOG.md`, not
in a growing conversation.

**Why "Progressive Development":** a ratchet-and-pawl only advances, never slips back. That is
the one invariant we enforce above all else — the tree only moves forward through
a green gate; a failed iteration resets to the last green commit. No regression
is possible by construction.

## Files
| File | Role |
|---|---|
| `misc/scripts/progressive-development/autopilot.sh` | THE loop (the only one) — per cycle it authors up to `PROGDEV_LANES` (2) items CONCURRENTLY (design→impl→review, each in its own worktree + cargo target) then INTEGRATES them SERIALLY (git-mutating gate, one lane at a time); + disk/mem/kill-switch/convergence safety harness |
| `misc/scripts/progressive-development/context.md` | the operating contract every dispatched lane agent reads (`--append-system-prompt-file`) |
| `misc/scripts/progressive-development/backlog.jsonl` | the work source (SSOT); queried via the `backlog.sh` interface (`ready`/`claim`/`close`/…) |
| `misc/scripts/progressive-development/watch.sh` | live monitor — per-lane status header (task + elapsed), narrator line, `1`/`2` key-switching between lanes |
| `docs/architecture/progressive-development-log.md` | append-only per-iteration outcomes + attempt counts (created on first run) |
| `docs/architecture/progressive-development-escalations.md` | items the loop refused (excluded class) + fix sketches (created on first run) |

## Guardrails (why this is safe to leave unattended)
1. **Gate-on-green commit (the pawl).** An iteration commits only if `cargo test --workspace` + `cargo clippy -D warnings` pass on the isolated gate target (and, for a sweep blocker, the example's original diagnostic is gone). Red → `git reset --hard`, log the reason. The tree is always green between iterations.
2. **Stop-and-escalate on the hard class.** Security tier (Secret/SqlFragment/CSRF/fuzzer), feature gaps needing type-system+backend+runtime co-design (e.g. erased-`any` payloads), oracle *divergences*, `unsafe`/FFI, or anything relaxing a soundness gate → the iteration refuses, writes an escalation, and moves on. This is the ex27 lesson encoded: a fresh agent under "pressure-cooker" pressure will otherwise produce a plausible **unsound** hack. The loop only touches mechanical, reference-backed wiring.
3. **Per-item attempt cap (anti-thrash).** 3 failed attempts on one item → mark BLOCKED, escalate, move on. Prevents infinite grinding on an intractable item.
4. **Concurrent authoring, serial integration.** A lockfile guards against a second autopilot. Within one run, up to `PROGDEV_LANES` lanes AUTHOR in parallel (each in its own worktree + its own cargo target — never the shared one), but the git-mutating GATE runs SERIALLY, one lane at a time, on the shared checkout (single-writer where it matters). A lane that loses a merge race with an earlier-landed lane is requeued (no penalty). Landed lanes commit straight to the base branch through the green gate.
5. **Resource preconditions every iteration.** mem-guard alive, free disk ≥ 15 GB, timeout-bounded builds, no background processes. The AGENTS.md non-negotiables become loop invariants.
6. **Kill-switch + caps.** `touch progressive-development.stop` for a clean exit; `PROGDEV_MAX_ITERS` and per-iteration `timeout` bound the blast radius; `--once` validates a single iteration before unleashing the loop.
7. **Isolated gate target.** Always `~/.cache/master-gate-target`, never the shared lane target — the stale-rlib thrash cannot fool the gate.
8. **Idempotent / crash-safe.** All state is on disk; a crash mid-run just means the next iteration re-reads `BACKLOG.md` + `progressive-development-log.md` and continues. Nothing lives only in memory.

## Operating it
```bash
# 0. mem-guard must be running; tree clean; on master (or your base).
misc/scripts/progressive-development/autopilot-run.sh          # supervised first run (1 cycle, live watch.sh attached)
misc/scripts/progressive-development/autopilot-run.sh --full   # full run using autopilot's native caps
PROGDEV_LANES=2 misc/scripts/progressive-development/autopilot.sh   # or invoke directly
misc/scripts/progressive-development/autopilot-stop.sh         # graceful stop after the current phase (touch autopilot.stop)
# lanes commit straight to the base branch through the green gate — no fast-forward step.
```

## Monitoring a run
No dashboard — durable log files + git. Three surfaces:

1. **Loop heartbeat (live, coarse).** `run.sh`'s stdout: `── iteration N/20 ──`,
   each verdict (`LANDED <sha>` / `FAILED` / `ESCALATED` / reset), disk/mem
   aborts. Run it in a terminal, or `tail -f` its stdout if backgrounded.
2. **Landed work (durable).** Each green iteration = one commit on the run branch:
   ```bash
   watch -n10 'git log --oneline master..$(git branch --list "progressive-development/run-*" | tail -1 | tr -d " *")'
   git show <sha>   # the diff of any landed item
   ```
3. **Per-iteration full output.** `docs/architecture/progressive-development-iter-<n>.log`
   holds the agent's complete output for iteration N; `progressive-development-escalations.md`
   holds what it refused.
   - **Default (text) mode:** `claude -p` buffers, so this log only fills when the
     iteration *ends* — a post-mortem, not a live feed.
   - **`PROGDEV_STREAM=1`:** switches to `--output-format stream-json --verbose`, so
     every step (reasoning, tool calls, gate output) is written to the iter-log
     **as it happens** — a true live feed.

**One-command monitor:** `misc/scripts/progressive-development/watch.sh` prints the
header (branch + landed commits + escalations) then follows the newest iter-log
live, pretty-printing stream-json steps via `jq` (falls back to raw tail). Run it
in a second terminal:
```bash
# terminal 1 — start the loop (auto-attaches watch.sh on a tty)
misc/scripts/progressive-development/autopilot.sh
# terminal 2 (optional) — a foreground watch supports 1/2 lane-switching + a auto
misc/scripts/progressive-development/watch.sh
```

## The AGENTS.md cost — the central economic question

**The problem.** A fresh `claude -p` process auto-loads project memory at
SessionStart: the repo `AGENTS.md` (this project's is very large — the app-shape
matrix, the full stdlib reference, env-var tables, the effect-boundary tier
list, …), the global `~/.claude/AGENTS.md`, the memory index, and skill blobs.
For an *interactive* session that pays off — you might do anything. For a Progressive Development
iteration whose entire job is "wire one known-missing kernel," ~90% of that
preamble is dead weight, and — this is the crux — it is billed **cold, at full
input rate, on every iteration**, because each `claude -p` is a new process with
a cold cache.

**The math that matters.** Call the fixed preamble `F` and the per-item working
context `W`.
- A **warm long session** pays `F` roughly once (cached at ~10% after the first
  turn) but accumulates an unbounded conversation tail `T` that is re-sent every
  turn — long sessions die of `T`, not `F`.
- A **cold Progressive Development loop** pays `F + W` fresh per iteration and has **no tail**
  (`T = 0`). Over N iterations: `N·(F + W)` vs the warm session's
  `F + Σ(W_k + T_k)`.

So Progressive Development wins decisively on the tail (its whole point) but can **lose on the
fixed part** if `F` is large and N is high: a 25k-token `F` over 50 iterations is
1.25M tokens of preamble alone. The entire economic game is **shrinking `F`** and
**keeping the cache warm across iterations.**

**What we do about it (in priority order):**

1. **Stop AGENTS.md loading at the CLI — you cannot "ignore" it.** An auto-loaded
   `AGENTS.md` is injected into the fresh agent's system prompt *before it acts*,
   so a prompt instruction to "ignore AGENTS.md" is futile (the tokens are already
   billed) AND counter-productive (naming the file can trigger a wasteful re-`Read`).
   The only real fix is to change *what loads*, and the Claude CLI has flags for
   exactly this. autopilot's `agent()` invokes each lane stage (design/impl/review)
   as:
   ```
   claude --safe-mode --permission-mode auto \
          --append-system-prompt-file misc/scripts/progressive-development/context.md \
          -p "<the stage prompt>"
   ```
   (`--permission-mode auto` — not `acceptEdits`: the latter auto-approves only
   file edits, so the iteration would stall on the first `cargo`/`git` bash call
   with no human to approve it. `auto` runs bash/git/edits unattended via the
   auto-approval classifier while still gating dangerous ops. If it blocks a
   routine command, append an `--allowedTools 'Bash(cargo *) Bash(git *) …'`
   allowlist via `PROGDEV_CLAUDE_ARGS`.)
   - **`--safe-mode`** disables AGENTS.md auto-discovery (project AND global),
     skills, plugins, and **hooks** — while keeping normal auth (OAuth/keychain),
     so it works with a subscription login. (`--bare` is leaner — keeps skills,
     still drops AGENTS.md + hooks + auto-memory — but requires `ANTHROPIC_API_KEY`;
     override via `PROGDEV_CLAUDE_ARGS` if you auth that way.)
   - **`--append-system-prompt-file`** injects the lean contract
     (`context.md`, ~1–2k tokens: six principles, two
     rules, the seal, boundary, gate) as the *only* project instruction.
   Hooks-off is a bonus: the stop-hook can't interfere with the loop. The iteration
   then pulls the ~5% of project detail it needs by *reading specific files as tool
   calls* (backlog.md, the `../sky` reference, the crate it edits) — you pay for what
   an item touches, not the whole manual. This runs in the main checkout (warm build
   target), no worktree needed.

2. **Keep the fixed prefix byte-stable and iterate fast to ride the prompt cache.**
   Anthropic's prompt cache keys on an exact prefix with a **5-minute TTL**. If the
   preamble (`prompt.md` + whatever system content) is identical each
   iteration AND iterations start < 5 min apart, the fixed part is served at ~10%
   across *separate* invocations — turning cold reloads back into warm reads. Hence
   `PROGDEV_COOLDOWN` defaults to 20s (well under the TTL), not minutes.

3. **Read volatile state via tool calls, never in the cached prefix.** `BACKLOG.md`
   and `git status` change every iteration; if they sat in the system preamble they
   would bust the cache each time. The playbook has the agent *read them as its
   first actions*, so the volatile bytes land after the cacheable prefix and don't
   invalidate it.

4. **One item per iteration, small `W`.** Narrow scope keeps the variable context
   small, so even a cold iteration is cheap. Batching would inflate `W` and raise
   the odds of a red gate (discarded work = pure waste).

**Bottom line.** Progressive Development is cheaper than a long warm session *iff* you shrink `F`
and hold the cache — otherwise the repeated cold preamble can cost more than the
tail you were trying to avoid. The lean prompt + stable-prefix + fast-cadence +
one-item design is what turns the technique from "surprisingly works" into
"cheaper AND safe." Measure it: `claude -p --output-format json` reports per-call
token usage; sum it over a `--once` run and compare against a warm session doing
the same item before trusting the loop with a long unattended run.

## What Progressive Development is NOT for
Design decisions, security-critical code, oracle divergences, anything needing
human/guardian judgment. Those are escalated, never brute-forced. Progressive Development grinds
the safe, reference-backed 80%; the risky 20% stays with a human + the guardian
review. That division of labour is the whole point.

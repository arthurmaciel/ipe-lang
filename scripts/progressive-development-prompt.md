# Progressive Development — one iteration

You are ONE iteration of the Progressive Development loop: a fresh-context, gated, autonomous
burndown of the Ipê (Sky→Rust) compiler backlog. You have no memory of prior
iterations — all durable state is on disk. Do exactly one unit of work, prove
it, commit it or discard it, and exit. The next iteration starts fresh.

## Prime invariant (the pawl)
**The tree only moves forward through a green gate.** You either land ONE
backlog item as a green, committed increment, or you leave the tree exactly as
you found it (last green commit). You never commit red. You never `git push`,
force-push, rewrite history, or touch `main`/`master` outside the progressive-development branch.

## Read state first (these are volatile — read them every time)
1. `git rev-parse --abbrev-ref HEAD` — confirm you are on a `progressive-development/*` branch. If not, STOP and write an escalation (see below); do nothing else.
2. `git status --short` — the tree MUST be clean. If dirty, run `git stash` (a prior iteration left a mess) and note it in the log.
3. `docs/architecture/backlog.md` — the work list.
4. `docs/architecture/progressive-development-log.md` — what prior iterations did (outcomes + per-item attempt counts).
5. `docs/architecture/progressive-development-escalations.md` — items already escalated; do NOT retry these.

## Pick exactly ONE item — eligibility (all must hold)
- It is in `backlog.md` under the **sweep front** or a section explicitly tagged **[progdev-safe]**.
- It is **mechanical**: wire a known-missing kernel across the layers, fix a fixture, register a module — work with a clear Haskell/Go reference (`../sky`, READ-ONLY). NOT a design decision.
- It is **NOT** in any of these excluded classes → if the best-available item is one of these, escalate it and pick another, or if none remain, exit with `PROGDEV: DRY`:
  - Security tier (Secret type, SqlFragment, CSRF, fuzzer — anything touching auth/secrets/SQL/crypto).
  - A feature gap needing type-system + backend + runtime co-design (e.g. erased-`any` enum payloads).
  - An oracle **divergence** from the Go/Haskell reference (behaviour change, not a wiring gap).
  - `unsafe`, FFI, or anything that relaxes a soundness/panic gate.
  - Anything already at **3 failed attempts** in the log (mark it BLOCKED, escalate, pick another).

Prefer the lowest-risk item with the clearest reference. One item. Do not batch.

## Do the work — root cause only
- Match the reference. No suppression, no workaround that hides a contract violation (CLAUDE.md §3/§4/§7). If the only fix you can find is a hack, that means it belongs in an excluded class → escalate it, don't hack it.
- New AST/IR/kernel work must update EVERY arm the compiler's exhaustiveness enforces (lean on the type checker — if it compiles, you got them all).

## Gate — the only thing that authorises a commit
Run, from the repo root, timeout-bounded, on the ISOLATED gate target (never the shared lane target):

```
touch runtime/tests/*.rs crates/skyc/tests/*.rs
CARGO_TARGET_DIR="$HOME/.cache/master-gate-target" timeout 3000 cargo test --workspace
CARGO_TARGET_DIR="$HOME/.cache/master-gate-target" timeout 1200 cargo clippy --workspace --all-targets -- -D warnings
```

Both MUST be exit 0. If the item was an example-sweep blocker, ALSO rebuild that
one example with the freshly-built `skyc` and confirm its original diagnostic is
gone (note the NEW blocker for the backlog).

## Land or discard — then log — then exit
- **Green** → update `backlog.md` (mark the item done / advance its blocker), `git add -A`, `git commit` with a message stating root cause + fix + new blocker. Append a `LANDED` line to `progressive-development-log.md`. Exit `PROGDEV: LANDED <sha>`.
- **Red** (gate failed, or you couldn't reach a clean fix) → `git reset --hard HEAD` (discard all changes — the pawl holds), append a `FAILED` line to `progressive-development-log.md` with the item id, the attempt number, and the CONCRETE reason/error (so the next iteration reads it and tries differently or hits the 3-attempt cap). Exit `PROGDEV: FAILED <item>`.
- **Excluded/none eligible** → append an entry to `progressive-development-escalations.md` (item + why it's excluded + a fix sketch if you have one) and exit `PROGDEV: ESCALATED <item>`, or `PROGDEV: DRY` if there is genuinely no eligible mechanical work left.

## Hard safety (abort the iteration, exit `PROGDEV: ABORT <reason>`, if violated)
- `mem-guard.sh` is not running, OR free disk < 15 GB, OR you are not on a `progressive-development/*` branch, OR the gate target is being written by another process (no concurrent builds — you are the single writer).
- Never run background processes (`&`, `nohup`, `run_in_background`). Every command is foreground + `timeout`.

Log lines are one line each: `<ISO-time> | <LANDED|FAILED|ESCALATED|ABORT|DRY> | <item> | <detail/sha>`. Keep the log append-only.

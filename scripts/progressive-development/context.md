# Progressive Development — operating contract

**This file is the loop's COMPLETE instruction set.** The repo's large
`CLAUDE.md` is written for interactive general-purpose work and is NOT your
contract — this is. Where they conflict, follow this. Everything else you need
is `scripts/progressive-development/backlog.jsonl` (query via `backlog.sh`)
+ the `../sky` reference + the code you touch.

(Distilled from `PRINCIPLES.md`, the enforcement SSOT — read it fully. If the
human meant a different subset, they will edit this file.)

## 0. Above all — never traded for speed or tokens
**Security > correctness > soundness > everything else.** If a fix would weaken
any of these, STOP and escalate. Do not land it. This outranks "make progress".

## 1. The seal (the core mandate)
**No exit-0-then-cargo-fail.** If `ipe` accepts a program (exit 0), the emitted
Rust MUST `cargo build`. Never emit codegen that type-checks in ipe but fails
`cargo`. Closing that gap is the point of most mechanical items.

## 2. The six principles (PRINCIPLES.md)

Read PRINCIPLES.md fully and also follow the practical rules below.

1. **If it compiles, it works** — no runtime panic from well-typed Ipê. Every panic class has a regression test.
2. **Root-cause only** — never suppress a type error or warning; a defensive cover-up that hides a contract violation IS a violation.
3. **Match the reference** — Go/Haskell (`../sky`) parity is the default. Diverge ONLY where strictly better (Rust/Unicode/modern) AND recorded in `docs/divergences-from-sky.md`. A hack is never a "divergence".
4. **Production-grade + maintainable** — scales to Stripe-SDK-scale FFI; stays clean.
5. **Typed effects & secrets** — effects are `Task Error a`; no `Result String`/`Task String` in public surfaces; secrets are typed, never `fmt`-stringified.
6. **Exhaustive wiring** — new AST/IR/kernel work updates EVERY match arm the compiler enforces; no `_ ->` catch-all cover-ups. Lean on the type checker to prove you got them all.

## 3. The rules
1. **Test-first** — every new feature/bug becomes a regression test before the fix lands; the failing test is the discovery artefact.
2. **No deferral** — "pre-existing" / "known edge case" is never a shipping excuse. Root-cause it, or escalate it; never paper over it.
3. **Structural fix, not symptom** — repair the generative cause (the missing invariant, the drifting table, the special-case that should be a general rule) so the WHOLE class cannot recur; an ad-hoc patch that silences the visible symptom resurfaces one shape over. Ask "what structural property, if it held, would make this class impossible?" and establish it.
4. **No shortcuts (PRINCIPLES.md §0)** — removing/skipping the file, example, test, or line that TRIGGERS a bug is NOT a fix. Never edit a reference example/fixture/`.ipe` to dodge a compiler gap, weaken a gate, or `#[allow]` a real violation. Two acceptable outcomes only: root-cause it, or report it as an honest tracked blocker. A green obtained by deleting the red is a FAILURE. "Make the sweep green" means make the programs actually compile+run, not make red rows disappear.

## 4. Resource non-negotiables (loop preconditions)
`mem-guard.sh` MUST be running. Every build/test wrapped in `timeout`. No
background processes (`&`/`nohup`). No sub-agent dispatch — you are the lane,
not an orchestrator. Abort if free disk < 15 GB.

## 4b. Command & output hygiene (enforced — these waste whole builds when broken)
- **Tee high-cost output to a file, then re-read. NEVER tail-pipe one-shot.**
  `cargo build`/`nextest`/`clippy`/`ipe build` etc.:
  `cmd > /tmp/<step>.log 2>&1` then `rg <pat> /tmp/<step>.log` as many times as
  needed. `cmd 2>&1 | tail` DISCARDS the output — a different slice means
  RE-RUNNING the multi-minute build.
- **No Monitor tool. No self-poll (`until`/`while pgrep …; do sleep`) on your
  own build.** A monitor/poll detaches, outlives you, and becomes an unkillable
  zombie that relaunches killed builds. Run the build in the FOREGROUND under
  `timeout` and report when it returns. (Extends §4's `&`/`nohup` ban.)
- **Never `pgrep` your OWN command name to detect completion** — `pgrep -f`
  matches the waiter itself → false "still running" forever. Trust the
  foreground command's exit code.
- **Isolated `CARGO_TARGET_DIR` when your change touches compiled code.** Set
  your own per-lane target under `~/.cache/ipe/`; do NOT build on the shared
  gate target mid-work — it races concurrent lanes (phantom errors).
  Leaf-only/doc edits may share.
- **Never `cargo fmt`** (whole-crate OR `--`-scoped) — it reformats the ENTIRE
  workspace. Use `rustfmt <exact file>` on ONLY the files you touched.
- **Never `git checkout --` / `git restore` / `git stash` to clear state** — you
  can silently destroy another lane's or the prior in-progress work. The ONLY
  sanctioned reset is §6's `git reset --hard` on YOUR OWN red gate. Unexpected
  dirty state → STOP and escalate; discard nothing.

## 5. Boundary
Only the Rust-port surface: `crates/`, `runtime/`, sky-stdlib compiled-source,
`examples/` fixtures, `docs/`. `../sky` is READ-ONLY reference — never edit the
Haskell/Go backend or upstream. Cargo targets go under `~/.cache/ipe/` only
(PRINCIPLES.md §Write-boundary).

## 6. The gate (the only thing that authorises a commit)
```
touch runtime/tests/*.rs src/ipe-cli/tests/*.rs
CARGO_TARGET_DIR="${MASTER_GATE_TARGET:-$HOME/.cache/ipe/gate-target}" timeout 3000 cargo nextest run --workspace
CARGO_TARGET_DIR="${MASTER_GATE_TARGET:-$HOME/.cache/ipe/gate-target}" timeout 1800 cargo nextest run -p sky-runtime-rust --features full
CARGO_TARGET_DIR="${MASTER_GATE_TARGET:-$HOME/.cache/ipe/gate-target}" timeout 600 cargo test --doc --workspace
CARGO_TARGET_DIR="${MASTER_GATE_TARGET:-$HOME/.cache/ipe/gate-target}" timeout 1200 cargo clippy --workspace --all-targets -- -D warnings
```
`cargo test --workspace` is banned — `cargo nextest run --workspace` is the
parallel runner and dramatically faster here; nextest does not run doctests,
so the `cargo test --doc` line covers those. The `--features full` runtime
lane is LOAD-BEARING, not optional: the runtime's `default = []`, and
workspace feature-unification does NOT switch on `live` (or `db`/`tui`/…), so
the workspace run silently skips every `#[cfg(feature = "…")]`-gated test —
including the ENTIRE `sky_runtime::live::*` surface (`style_inject`
CSS-injection sink gates, SSE/session/dispatch) and the `spawn_blocking`
regression modules. A default-only gate can show green while those are red
(mirror of CI's `runtime-full-features` job). All four lines exit 0. For a
sweep blocker, also rebuild the example with the fresh `ipe` and confirm the
original diagnostic is gone (note the new blocker). Green → commit. Red →
`git reset --hard` + log the reason. The tree only advances.

## 6b. Isolated-worktree lanes — NEVER `git stash`
You are one of autopilot's CONCURRENT lanes, running in your OWN git worktree.
Do NOT run `git stash` under any circumstance — `refs/stash` is SHARED across
every worktree of one repo (a git limitation, not per-worktree state), so two
lanes stashing at the same moment can collide and silently swap or lose each
other's in-progress diffs. A freshly-created lane worktree is never dirty at
start; if you find one dirty, that is a worktree-isolation violation — abort
and escalate, do not stash it away. (autopilot authors lanes in parallel
worktrees but INTEGRATES them serially, so the stash-ban is about the parallel
AUTHORING phase you are in.)

## 7. Output style — caveman-ultra (mandatory)
Your prose is watched live. Be EXTREMELY terse. Drop articles, filler, hedging,
pleasantries. Fragments fine. One line where one line does. No emojis. No
preamble ("I'll now…", "Let me…", "Sure"). State the action or the finding, not
the intent. Code, paths, identifiers, and error text stay EXACT and verbatim —
never abbreviate those. Terseness never trades away correctness, the gate, or a
required verdict line. Final line is always the verdict (LANDED / ESCALATED /
DONE / REVIEW: … / TRIAGE: … / AUDIT: …).

## Codegen — concrete over generic (PRINCIPLES.md §No `dyn Any`)

Emit CONCRETE (monomorphized) over generic, ALWAYS. Wildcard `any` is NOT
polymorphism — it has exactly ONE concrete lowering (opaque carrier:
`IrType::Json`/`JsonVal`, or `Dict String String`/`HashMap<String,String>` in
pub/sub payload position); emit that carrier at EVERY position (enum field,
pattern binder, fn/decoder param, Db row arg, eta lambda param, return). ONLY
genuine named type variables (`a`, `msg`) become Rust generics (`fn f<T>`),
rustc-monomorphized. NEVER `dyn Any`/`.downcast`/type-erasure. A generic where
concrete was possible passes a mechanical gate but can ship a silent runtime
bug (e.g. `Broker<T>` keyed by `TypeId` needs publisher+subscriber same
concrete `T`).

## skydex — code-relation index (use before rg on the reference)

`skydex` on PATH indexes the ../sky READ-ONLY reference. Query it FIRST for
reference relations (faster than rg, gives cross-lang routes):
- `skydex locate <sym>` — occurrences + kernel parity route (Sky→Haskell→Go→Rust impl paths).
- `skydex rdeps <mod>` / `skydex deps <mod>` — reverse/forward module deps.
- `skydex covers <kernel>` — fixtures/examples covering a kernel.
- `skydex parity` — Go-vs-Rust kernel parity gaps.
Use it to find "what does the reference do for X". Falls to `rg` if no hit.
Our OWN port (crates/) is NOT indexed by skydex — use ipe-index.

## ipe-index — OUR Rust def index (use before rg on crates/runtime)

`ipe-index` on PATH indexes THIS project's Rust (crates/ runtime/ tools/).
- `ipe-index def <sym>`  — definition site(s): file:line kind (one answer, not every text hit).
- `ipe-index refs <sym>` — all word occurrences (rg-backed).
- `ipe-index kind <fn|struct|enum|trait|type|impl|macro>` — list defs of a kind.
Query it FIRST to find where our own code defines X. skydex = reference; rg =
fallback. Rebuilt at loop start (reflects landed fixes).

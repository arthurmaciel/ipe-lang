# dev-ops — deep operational procedures

The operational HOW for working on the Ipê compiler: the guard daemons, the
two-tier gate mechanics, end-of-mission cleanup, the timeout discipline, and the
release pipeline. The *rules* these procedures implement live in `PRINCIPLES.md`
(the enforcement source of truth); this file is the depth behind them. Day-to-day
onboarding — the crate map, the fast gate, kernel registration — is in the root
`AGENTS.md`.

## Memory safety — the mem-guard daemon

A runaway compiler-tooling process can exhaust host memory. A memory-guard
daemon watches for this and kills the offender before it takes the host down;
treat a missing guard like a missing `set -e` during a heavy dev session.

The doctrine it enforces (16 GB host): a per-process kill at 6 GB RSS for
compiler tooling (`cargo`/`rustc`/`cc1`/`cc1plus`/`cc`/`collect2`/`ld`/`ld.lld`/
`lld`/`ipe`/`ipe-ffi-inspector`/`rust-analyzer`); a higher panic tier for the
dev-session processes themselves; a system-pressure floor a little above zero
free. Never silence a kill by raising the threshold — a kill means the process
was on a path to exhaust the machine; fix the underlying bug.

## Disk hygiene — pruning and the reclaim order

The write-boundary (cargo targets under `~/.cache/ipe/` only; source/doc/test
edits under the repo tree only) is a `PRINCIPLES.md` rule. Operationally, every
per-purpose or per-lane target lives under `~/.cache/ipe/<purpose>-target`, and
every dispatched-agent brief must set `CARGO_TARGET_DIR` under that root — a
target elsewhere is invisible to reclaim and fills the disk to 100%.

A disk-guard daemon reclaims disposable caches before the disk fills, in a fixed
safety order: `~/.cache/sccache` first (self-healing), then orphaned cargo target
dirs (identified by their `CACHEDIR.TAG` content, not by name), never a directory
a live rustc/cargo process is still writing to.

**Pre-build disk check — before any full build, test suite, or example sweep.**
Run `df -h /`; if under roughly 15–20 GB free, reclaim first: `rm -rf
"$CARGO_TARGET_DIR"`, prune stray targets under `~/.cache/ipe/`, prune
per-example artifacts (`out/`). A near-full disk dies mid-run with ENOSPC *after*
type-check and codegen succeed, surfacing as a file-copy or "build failed" error
that masquerades as a codegen regression and wastes the whole run on a
misdiagnosis — always read the actual build log before blaming a code change.
`tools/scripts/examples-sweep.sh` aborts with a `< 5G free` guard before starting. A
host under 5 GB free must abort the next agent spawn until cleanup completes —
an ENOSPC mid-build leaves half-written artifacts worse than a clean rebuild.

## Background-task hygiene — clean up before declaring "done"

Orphaned background wait-loops exhaust the per-uid process table (`fork: retry:
Resource temporarily unavailable`) and silently kill the memory guard.
End-of-mission checklist:

```bash
# Orphan polling loops
ps -u $USER -o pid,command | awk '/while pgrep|until ! pgrep/ && /\/bin\/zsh -c/ {print $1}' | xargs -n1 kill -9 2>/dev/null

# Stray sleeps + verification leftovers
ps -u $USER -o pid,ppid,command | awk '$3 == "sleep" && $2 != 1 {print $1}' | xargs -n1 kill -9 2>/dev/null
pkill -f "playwright"; pkill -f "chromium"
pkill -f "examples/.*/out/app"
```

The orchestrator watches long-running work through its own monitor rather than a
`run_in_background` + polling loop; dispatched lanes are foreground-only (a
`PRINCIPLES.md` agent-lane rule).

## Timeout gate — every long-running command is timeout-bounded

A hung test or build is a silent time-waster. Rules:

- **Full gate under timeout.** Every `cargo nextest run` / `cargo test` in the
  gate is wrapped in `timeout` (the workspace run uses roughly `timeout 3000`).
  Not enough time means a flaky test — bisect it, don't widen the ceiling.
- **Per-step timeouts.** Any step executing a subprocess (`ipe build` / `ipe run`
  / `ipe watch`) wraps the child in `timeout`. A step that cannot time out cannot
  be re-run.
- **The example sweep already bounds every stage:** `ipe build` `timeout
  ${IPE_SWEEP_BUILD_TIMEOUT:-900}`, `cargo build` `timeout 900`, emitted-app run
  `timeout 8` (`exercise_cli` in `tools/scripts/lib/checks.sh`) — don't remove or widen
  without a real reason.
- **Background shell commands** waiting on a process must `kill -KILL` after a
  finite wait (default 600 s); never `wait $PID` unbounded.
- **Monitors** in dev-loop tooling (`ipe watch`) need a heartbeat / max-wait so a
  wedged child cannot poison the parent.

A process running over 30 minutes without justification is killed and filed,
never waited out.

## The two-tier gate — operational detail

The rule (a cheap per-lane gate versus one authoritative full gate, and the
components of each) is a `PRINCIPLES.md` rule. Master only advances to a
full-gate-certified sha.

**Cheap gate** — merges the lane into an integration worktree, then checks +
tests + lints only the touched crates:

- `cargo +nightly check -p ipe`
- `cargo +nightly nextest run <-p touched-crates>` (scoped; no `IPE_E2E`)
- `cargo +nightly clippy <-p touched-crates> --no-deps -- -D warnings`

**Full gate** — run every N cycles, or the instant pending work drains:

- `cargo +nightly nextest run --workspace` (`IPE_E2E=1` for the SEAL builds)
- `cargo +nightly nextest run -p ipe-runtime-rust --features full`
  (LOAD-BEARING — the runtime's `default = []` means the workspace run skips every
  feature-gated test, including the entire `live::*` surface)
- `cargo +nightly test --workspace --doc`
- `cargo +nightly clippy --workspace --all-targets -- -D warnings`
- fuzz (`tools/scripts/fuzz-well-typed.sh`) and the full examples sweep

Full-green certifies the batch and advances master; full-red resets to the last
certified sha and re-queues. The two gates must agree on lint scope, and the
cheap gate is never *stricter* than the full gate. Both gates run clippy
`--all-targets` (which catches test-binary lint debt); never add `--all-targets`
to one gate without the other.

**Goldens.** A golden is `tests/golden/<name>/Main.ipe` plus its expected emit,
byte-compared. The default run is byte-identity of the emit (fast, no cargo);
`IPE_E2E=1` builds and runs the emitted project (THE SEAL: an `ipe`-exit-0
program must `cargo build`). After an emit-changing compiler change, regenerate
every golden's expected emit with `cargo run -p regen-goldens` (or `-- <name>…`
for named goldens). It emits through the `ipe` library — the same path the
goldens assert on — so on an unchanged compiler it is a no-op (`git status` stays
clean) and touches only the emitted artifacts, never the Ipê sources. The emit
templates the codegen embeds live in `src/compiler/backend/rust/templates/` — a
hand-maintained source, not a golden — so no golden is ever an input to codegen.

## Build & cache tuning

On an 8-core / 15 GB-RAM host the build is RAM-bound, not core-bound.
`~/.cargo/config.toml` sets `rustc-wrapper = sccache`, the `mold` linker,
`incremental = false`, and `jobs = 2` — an OOM guard *per cargo invocation* (two
concurrent lanes already run roughly four parallel `rustc`, near the RAM ceiling;
raising `jobs` multiplies per lane and OOMs). Never override `RUSTFLAGS`: the
config's `mold`-only flags are part of the sccache cache key, so extra flags fork
the key into cold recompiles and more RAM pressure. All cargo targets live under
`~/.cache/ipe/`. A `cargo nextest run -p ipe` recompiles every `ipe` test binary
— scope to `--test <name>` when you need only one.

## No-deferral — pipeline mechanics

The rule ("pre-existing" is never a shipping excuse; fix first; only an explicit
user override ships a known issue) is `PRINCIPLES.md` §0. Mechanics:

- **Spotted is filed.** Any test/sweep failure, runtime panic, or log error
  creates a task on the spot.
- **Group related fixes** into the next patch release to cut notification noise;
  don't tag per fix.
- **Closing requires an actual fix.** A documented workaround is a temporary
  bridge, never permanent.
- A hard problem is a reason to *start* (root cause → architecturally correct
  approach → execute, even across sessions), not to defer.

## Release pipeline

`main` is green by construction: changes land through PRs, and a PR merges only
when the fast required gate is green. Slow checks run post-merge and nightly, so
they never block a PR — a regression they catch is on `main`, not in the merge
queue.

**The fast required gate** (target: minutes — the checks branch protection
requires):

- `fmt` — `cargo fmt --all -- --check`
- `clippy` — `cargo clippy --all-targets --workspace -- -D warnings`. The command
  carries no lint flags of its own: the enforced set is the source of truth in
  root `Cargo.toml` `[workspace.lints.clippy]` (the broad groups plus a
  cherry-picked `restriction` slice, with two `cargo` lints allowed as workspace
  noise) plus `clippy.toml`'s `disallowed-methods` (`process::abort`,
  `panic_any`, the `*_unchecked` UB paths). Change the policy there, in one place.
  Fix the code; never `#[allow]` around a lint (tests may `unwrap`/`expect` per
  `clippy.toml`).
- `test` — the nextest unit/integration suite (E2E tests no-op without `IPE_E2E`).
- `cargo-deny` — the supply-chain gate (below).
- `seal-smoke` — build the compiler, then take one small example end to end
  (`ipe build` → `cargo build` the emitted crate → run it → assert output). It is
  a fast PR PROXY for THE SEAL over a SINGLE example, **not** the SEAL gate: it
  proves the emit→build→run floor still holds on that one example in a couple of
  minutes, but it cannot catch a regression in any of the other goldens. The real
  SEAL is the multi-shard `e2e` job aggregated by `e2e-all` (below).

**Slow checks** run on push-to-`main`, in the merge queue, and on a nightly
`schedule`, never on a plain PR: the full multi-shard `e2e` (THE SEAL in full,
every golden's emitted crate built and run), `miri`, the runtime feature-combo /
full-feature builds, and `examples-sweep`. `wasm-floor` is off the
always-required PR path — on a PR it runs only when wasm-relevant files change (a
`paths` filter), plus nightly. Triggers live in
`.github/workflows/{ci,security,examples-sweep,static}.yml`.

**`e2e-all` is the SEAL gate.** The `e2e` job shards THE SEAL across six runners
(`fail-fast: false`), so no single shard name is a stable required-check target
and a lone shard failure is easy to miss. The `e2e-all` job depends on every
shard and is green only when all six pass — one stable context name that CAN be
made a required status check so a SEAL regression blocks the merge queue. Making
it required needs repo-admin and is done deliberately (see Branch protection
below), not by CI. Until then `seal-smoke` remains the only SEAL-adjacent
required PR check and the full SEAL gates only post-merge/merge-queue.

**`cargo-deny`** is the one supply-chain gate. Its `advisories` check subsumes
`cargo-audit` (the same RustSec DB), and it also covers `licenses` / `bans` /
`sources` in one lockfile-only pass. Policy and every accepted exception (with a
written justification) live in `deny.toml` at the repo root. A real finding fails
the PR; an unfixable advisory is handled by a documented, reviewed ignore — never
by downgrading the gate.

**Branch protection** is enabled by running
`tools/scripts/ci/enable-branch-protection.sh` (required checks = the fast jobs,
`strict` up-to-date branch, PRs required, auto-merge on). It is deliberately run
by hand, not by CI — flip it on only after in-flight direct-push lanes drain.

**Cancel in-progress CI runs on `main` before pushing** (a newer commit
supersedes them; never cancel a release/tag run):

```bash
gh run list --branch main --status in_progress --workflow CI --json databaseId --jq '.[].databaseId' \
    | xargs -I{} gh run cancel {} 2>/dev/null
git push origin main
```

## Versioning & releases

Versioning, the changelog, and release cutting are automated by
[release-please](https://github.com/googleapis/release-please)
(`.github/workflows/release-please.yml`), driven by the same Conventional Commit
messages the PR workflow already uses. Contributors never bump a version or edit
`CHANGELOG.md` by hand.

**How a release ships.** On every push to `main`, release-please maintains one
standing **release pull request**. It bumps the workspace version in `Cargo.toml`
and prepends the next `CHANGELOG.md` section, derived from the commits merged
since the last release. That PR is inert while open — nothing is released until it
merges. **Merging the release PR is the act of shipping:** it tags `vX.Y.Z`,
creates the GitHub release with the changelog as its body, and that tag's release
event triggers `release.yml` to build the platform binaries and attach them.
There is exactly one release object per tag — release-please creates it;
`release.yml` only uploads assets to it.

**Pre-1.0 bump rules** (config in `release-please-config.json`):

| Commit | Version effect (pre-1.0) |
|---|---|
| `feat`, `fix`, `perf` | **patch** (`0.1.0` → `0.1.1`) |
| `!` suffix / `BREAKING CHANGE:` footer | **minor** (`0.1.0` → `0.2.0`) |
| `docs`, `chore`, `ci`, `test`, `refactor`, `style`, `build` | no release entry |

`1.0.0` is **never** reached automatically — while `0.x`, a breaking change only
bumps the minor. Cutting `1.0` is a deliberate, hand-made stability promise: the
public surface is committed to, and breaking changes thereafter bump the major
per SemVer. Until then, treat any `0.x` release as potentially breaking.

**Ship a release for every user-facing change.** Any change to the CLI (commands,
flags, help text), diagnostics and `explain` pages, the formatter's output, or a
public API is shipped as a release: merge the standing release PR once the change
lands, so users receive it under a version rather than only on `main`.

## Release checklist

1. Rebuild the driver: `cargo build --release -p ipe`; `source
   tools/scripts/lib/env.sh` exports `IPEC_BIN` + `IPE_RUNTIME_DIR`.
2. Full gate green — one authoritative run (the two-tier gate above).
3. Example sweep green — `tools/scripts/examples-sweep.sh` (per example: `ipe build` →
   `cargo build` the emitted crate → run `ipe-app`). VERDICT PASS iff zero red
   rows (THE SEAL end to end).
4. CI parity — `.github/workflows/{ci,examples-sweep,security}.yml` runs the same
   gate; cancel superseded in-progress `main` runs before pushing.

Any step failing means fix the root cause and re-run from step 1. Never tag with
a known build or runtime failure.

## Workflow reminders

- **Always run mem-guard** during a dev session.
- **Always clean up background tasks** before declaring "done".
- **`cargo fmt --all` after editing Rust source** (CI gates on `cargo fmt --all
  -- --check`).
- **Use `-f` with `rm`/`cp`** to avoid interactive prompts.
- **Never add co-author wording** to commits.
- **Never tag a release** without an explicit ask.
- **Run `ipe build` on an example from its own dir** (`cd examples/NN-name`),
  never from the repo root — `--out out/rust` writes relative to the current
  directory.
- **When the language surface changes** (stdlib, syntax, or the CLI), update the
  authoring reference (`src/ipe-cli/templates/AGENTS.md.in`) in the same commit.

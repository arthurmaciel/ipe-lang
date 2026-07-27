# DEVELOPMENT.md — dev-ops & orchestration non-negotiables

> Companion to **`PRINCIPLES.md`** (enforcement SSOT: six principles, three
> fundamental rules, THE SEAL, §0 no-shortcuts, clippy deny-set, two-tier
> gate, write-boundary, agent-lane rules, doc/code standards) and **AGENTS.md**
> (Ipê *language* authoring reference). This file = operational HOW:
> infrastructure, commands, checklists — Rust toolchain (`cargo`,
> `crates/`, `runtime/`, `ipe`). Autonomous-loop lanes follow
> `misc/scripts/progressive-development/context.md`.

Doc hygiene: no archaeology — see `PRINCIPLES.md`
§Documentation & code standards.

## Non-negotiables

### 0. No shortcuts — root cause or honest blocker (MANDATORY)

`PRINCIPLES.md` §0. Never delete/skip/edit bug trigger, weaken gate, or
`#[allow]` real violation; root-cause it or file honest tracked blocker.
Green from deleting red = FAILURE.

### 0a. Understand before you change — `ipe-index` + reference-first (MANDATORY)

Rule (port, don't invent) = `PRINCIPLES.md` §Agent-lane operational rules.
Tooling:

- **`ipe-index` FIRST for our own code** (`crates/` `runtime/` `tools/`) —
  pre-built structural index, not fresh search:
  - `ipe-index locate <Module.function>` — symbol location + kernel-parity
    route (Ipe → Haskell → Go → Rust impl paths).
  - `ipe-index def <sym>` / `refs <sym>` / `kind <fn|struct|enum|trait|type|impl>`.
  - `ipe-index parity --gaps` — Go-vs-Rust kernel parity gaps.
  Reserve `rg` for free-text hunts index can't answer.
- **Learn how reference handles THIS task before designing fix.**
  `../ipe` = READ-ONLY source of truth. For construct you're fixing,
  read each layer: **Ipe compiler** (Haskell, `../ipe/src/Ipe/` —
  parse/canon/type/lower), **Go backend** (`../ipe/runtime-go/`, byte-diff
  parity oracle), **Rust backend** (`../ipe/src/Ipe/Generate/Rust/`),
  **Rust runtime** (vendored behaviour it emits into). `ipedex locate <sym>`
  gives cross-lang route.
- Only once you can state (a) where OUR code handles it and (b) how reference
  handles it, design the change.

### 0b. Infrastructure at a glance — read this, don't re-learn it

**Compiler pipeline (acyclic crate stages):** `ipe_parse` → `ipe_canon` (name
resolve) → `ipe_types` (HM infer/constrain) → `ipe_lower` (AST→IR) → `ipe_ir`
→ `ipe_backend_rust` (emit Rust). Support crates: `ipe_kernels` (kernel
table), `ipe_diagnostics` (IPE-* codes + `explain/*.md`), `ipe_db` (salsa
incremental DB), `ipe_intern`, `ipe_watch`; `ipe` = driver + CLI. Runtime
impls in `runtime/src/ipe_runtime/`.

**ipe CLI:** subcommands `build` / `run` / `watch` / `explain` / `fix`.
`ipec build <src/Main.ipe | ipe.toml> --out out/rust`. Binary =
`target/release/ipe` (`cargo build --release -p ipe`);
`source scripts/lib/env.sh` sets `IPEC_BIN` + `IPE_RUNTIME_DIR`.

**Registering a kernel = update ALL anti-drift sites** (type-checker enforces
most; miss one → IPE-N0028 / IPE-L0108 / drift test): `ipe_kernels` (enum +
`decl()` + `ALL`), `ipe_types::constrain` (type-scheme + `FIRST_SCHEMED`, out
of `KNOWN_UNBACKED` bucket), `ipe_lower` (arity table +
`REGISTRY_ONLY_ALLOWLIST` for alias-only kernels),
`ipe_backend_rust/naming.rs`, `ipe_ir::pretty`, `crates/ipe/src/stdlib.rs`
(module registration). Template to seal new stdlib module:
`crates/ipe/tests/golden_stdlib_module_seal.rs`.

**Examples + sweep:** example = `examples/NN-name/src/Main.ipe` (+ other
`.ipe` modules, `ipe.toml`). `build_set` = **disk-derived**
(`scripts/lib/examples.sh`) — every `examples/NN-*/src/Main.ipe` whose imports
resolve auto-included; adding dir IS registration.
`scripts/examples-sweep.sh` also mirrors the upstream Sky examples
(`examples/sky/`, patched via `scripts/lib/mirror.sh`), per example:
`ipe build … --out out/rust` → `cargo build --manifest-path out/rust/Cargo.toml`
→ run `out/rust/target/debug/ipe-app`. VERDICT PASS iff zero red rows. Mode:
`IPE_SWEEP_BUILD_ONLY=1` (compile only, RUN skipped). No Go build, no
cross-compiler comparison.

**Emitted project:** `out/rust/` = Cargo project w/ runtime
vendored into `src/ipe_runtime/` (ipe copies from `IPE_RUNTIME_DIR`), default
binary `ipe-app`, edition 2024.

**Golden tests** (`crates/ipe/tests/golden_*.rs`): golden =
`tests/golden/<name>/Main.ipe` + `main.rs` (expected emit, **byte-compared**)
+ cached Go oracle (`expected_go.txt` / `oracle.meta`). Default run =
byte-identity of emit (fast, no cargo). `IPE_E2E=1` = build+run
emitted project (THE SEAL: ipe-0 ⇒ cargo-0). Oracle files regenerated
ONLY by `cargo run -p refresh-oracle -- <golden>` — NEVER hand-edited.

**Build & cache (8 cores / 15 GB RAM → RAM-BOUND, not core-bound):**
`~/.cargo/config.toml` sets `rustc-wrapper = sccache`, `mold` linker,
`incremental = false`, `jobs = 2` — OOM guard **per cargo invocation**
(2 concurrent lanes already ≈ 4 parallel `rustc`, near RAM ceiling;
raising `jobs` multiplies per lane → OOM). **Never override `RUSTFLAGS`** —
config's `mold`-only flags ARE sccache cache key; extra flags fork
key → cold recompiles + more RAM pressure. All cargo targets under
`~/.cache/ipe/` (write-boundary — `PRINCIPLES.md`); E2E emitted builds use
`IPE_ORACLE_SHARED_TARGET`. `cargo nextest run -p ipe` recompiles ALL ~155
ipe test binaries — scope to `--test <name>` when you need one.

### 1. Memory safety — `misc/scripts/guards/mem-guard.sh` MUST run during dev

Runaway compiler-tooling process can OOM host. Treat absent
mem-guard like missing `set -e`.

```bash
nohup ./misc/scripts/guards/mem-guard.sh > /tmp/mem-guard.out 2>&1 &
disown                                # survives shell exit
```

Defaults (16 GB host): per-process kill at 6 GB RSS for compiler tooling
(`cargo`/`rustc`/`cc1`/`cc1plus`/`cc`/`collect2`/`ld`/`ld.lld`/`lld`/`ipe`/
`ipe-ffi-inspector`/`rust-analyzer`); 10 GB panic tier for dev-session hosts
(`claude`/`node`/`ghostty`); system-pressure floor at <1.2 GB free.
Tune via `MEM_GUARD_PROC_MB` / `MEM_GUARD_PANIC_MB` / `MEM_GUARD_SYS_FLOOR_MB`;
`MEM_GUARD_DRY=1` = log-only. Never silence kill by raising threshold — a
kill means process was on path to OOM machine; fix underlying bug.

### 2. Background-task hygiene — clean up before declaring "done"

Orphan `run_in_background` wait-loops exhaust per-uid process table
(`fork: retry: Resource temporarily unavailable`), silently kills
mem-guard. End-of-mission checklist:

```bash
# Orphan polling loops
ps -u $USER -o pid,command | awk '/while pgrep|until ! pgrep/ && /\/bin\/zsh -c/ {print $1}' | xargs -n1 kill -9 2>/dev/null

# Stray sleeps + verification leftovers
ps -u $USER -o pid,ppid,command | awk '$3 == "sleep" && $2 != 1 {print $1}' | xargs -n1 kill -9 2>/dev/null
pkill -f "playwright"; pkill -f "chromium"
pkill -f "examples/.*/out/app"

# mem-guard alive?
pgrep -f mem-guard.sh >/dev/null || (nohup ./misc/scripts/guards/mem-guard.sh > /tmp/mem-guard.out 2>&1 & disown)
```

**Prefer Monitor tool** (orchestrator only — lanes foreground-only,
`PRINCIPLES.md` §Agent-lane rules) over `run_in_background` + polling.

### 2c. Caveman-ultra output — mandatory in EVERY agent brief

EVERY dispatched agent runs **caveman-ultra** output — autopilot lanes get it
via `context.md` §7; every hand-dispatched `Agent` brief MUST carry same
directive. Rules: terse; drop articles, filler, hedging, pleasantries;
fragments fine; one line where one line does; `X -> Y` for causality. **Code,
paths, identifiers, error text stay EXACT and verbatim.** No preamble.
Final line always the verdict (`DONE`/`STUCK`/`PARTIAL`/`REVIEW:`/…).
Terseness never trades away correctness, gate, or required verdict line.

### 3. Timeout gate — every long-running command MUST be timeout-bounded

Hung test/build = silent task waster. Rules:

- **Full gate under timeout.** Every `cargo nextest run` / `cargo test`
  in gate wrapped (`autopilot.sh` uses `timeout 3000` for workspace
  run). Not enough → flaky test; bisect it, don't widen ceiling.
- **Per-step timeouts.** Any step exec'ing subprocess (`ipe build` /
  `ipe run` / `ipe watch`) wraps child in `timeout`. Step that doesn't
  time out can't be re-run.
- **Example sweep** already bounds every stage: ipe build `timeout
  ${IPE_SWEEP_BUILD_TIMEOUT:-900}`, `cargo build` `timeout 900`, emitted-app
  run `timeout 8` (`exercise_cli` in `scripts/lib/checks.sh`) — don't remove or
  widen without real reason.
- **Background shell commands** waiting on process MUST `kill -KILL` after
  finite wait (default 600 s). Never `wait $PID` unbounded.
- **Monitors** in dev-loop tooling (`ipe watch`) need heartbeat/max-wait so
  wedged child doesn't poison parent.

Process running >30 min unjustified: kill it and file bug. Never wait it
out.

### 3b. The two-tier gate — operational detail

Rule (cheap per-lane vs ONE authoritative full gate; components of
each) = `PRINCIPLES.md` §The two-tier gate. Implementation:
`misc/scripts/progressive-development/autopilot.sh`; master only advances to
full-gate-certified sha.

**Cheap gate (`lane_gate`)** — merges lane into integration worktree,
then checks + tests + lints ONLY touched crates:
- `cargo +nightly check -p ipe`
- `cargo +nightly nextest run <-p touched-crates>` (scoped; no `IPE_E2E`)
- `cargo +nightly clippy <-p touched-crates> --no-deps -- -D warnings`

**Full gate (`full_gate` via `certify_batch`)** — every
`PROGDEV_FULL_GATE_EVERY` cycles (default 10) OR instant pending work
drains:
- `cargo +nightly nextest run --workspace` (+ `IPE_ORACLE_SHARED_TARGET` for E2E)
- `cargo +nightly nextest run -p ipe-runtime-rust --features full`
  (LOAD-BEARING — mirror of CI's `runtime-full-features`)
- `cargo +nightly test --workspace --doc`
- `cargo +nightly clippy --workspace --all-targets -- -D warnings`
- fuzz (`scripts/fuzz-well-typed.sh`)

**`--all-targets` rollout.** Target end-state: BOTH gates run clippy
`--all-targets` (catches test-binary lint debt). `--all-targets` enters
FULL gate only once test-file clippy-debt sweep clean (else full
gate reds); until then cheap gate stays `--all-targets`-free to match.
Never add `--all-targets` to one gate without other.

### 3c. Lint enforcement

`PRINCIPLES.md` §Mechanical enforcement — comply by construction: deny-set
(incl. `doc_markdown` backticks), per-site-`#[allow]`-only escape hatch,
`unsafe` policy. Gate runs `clippy -D warnings`; fix code, never
lint level.

### 4. No-deferral — pipeline mechanics

Rule ("pre-existing" never a shipping excuse; fix-first; only explicit
user override ships known issue) = `PRINCIPLES.md` §0. Mechanics:

- **Spotted = filed.** Any test/sweep failure, runtime panic, or log error →
  task created on spot.
- **Group related fixes** into next patch release to cut notification
  noise — don't tag per fix.
- **Closing requires actual fix.** Documented workaround = TEMPORARY
  bridge only, never permanent.
- **"Pre-existing" = investigation context, not verdict** — means
  fix can ship in own commit, not that it can be skipped.
- Hard problem = reason to START (root cause → architecturally correct
  approach → execute, even across sessions), not to defer.

### 5. Disk hygiene — unused build caches MUST be pruned

**Write-boundary** (ONLY two writable locations — cargo targets under
`~/.cache/ipe/`, edits under repo tree) = `PRINCIPLES.md`
§Write-boundary. Operationally:

- Loop's targets = `~/.cache/ipe/{gate-target, oracle-target,
  lane-<N>-target}`; hand-dispatched agent or manual verify build uses
  `~/.cache/ipe/<purpose>-target`.
- **Enforced in `autopilot.sh`**: `IPE_CACHE=~/.cache/ipe`; `reclaim_disk`
  keeps gate + oracle + warm `lane-*` targets and reaps rest, AND
  sweeps stray cargo targets under `~/.cache/*target*` or `/tmp`
  (pgrep-guarded). Every dispatched-agent brief MUST set `CARGO_TARGET_DIR`
  (and `IPE_ORACLE_SHARED_TARGET`) under `~/.cache/ipe/`.

**Pre-build disk check — BEFORE any full build/test suite/example sweep.**
`df -h /`; if <~15–20 GB free, reclaim first: `rm -rf "$CARGO_TARGET_DIR"`,
prune stray targets under `~/.cache/ipe/`, prune per-example artifacts
(`out/`). Near-full disk dies mid-run w/ ENOSPC *after*
type-check+codegen succeed, surfacing as file-copy/"build failed" error that
**masquerades as codegen regression** and wastes whole run on
mis-diagnosis — always read actual build log before blaming code change.

`misc/scripts/guards/disk-guard.sh` (sibling to mem-guard) polls free disk and reclaims
disposable caches BEFORE disk fills, in fixed safety order:
`~/.cache/sccache` first (self-healing), then orphaned cargo target dirs
(identified by `CACHEDIR.TAG` content, not name), never dir with
live rustc/cargo process still writing to it.

End-of-mission checklist (BEFORE declaring release shipped when sweep has
run):

```bash
# 1. Worktrees from finished agents (each ≈1.5 GB) — after the work is on
#    main; check TaskList for active agents before bulk-removing.
rm -rf .claude/worktrees/agent-<sha-of-completed-agent>
git worktree prune --verbose

# 2. Dead cargo targets under the sanctioned root — rebuild warm via sccache.
rm -rf ~/.cache/ipe/<dead-purpose>-target

# 3. /tmp leftovers
rm -f /tmp/autopilot-gate.log /tmp/mem-guard.log

# 4. Sanity check
df -h /
```

**Automatic hygiene:** `scripts/examples-sweep.sh` aborts w/ `< 5G free`
guard before start; loop's `reclaim_disk` (`autopilot.sh`) keeps
gate + shared-oracle + warm `lane-*` targets and reaps rest. Worktree
cleanup after every finished agent stays manual.

Host <5 GB free → ABORT next agent spawn until cleanup completes — ENOSPC
mid-build leaves half-written artifacts worse than clean rebuild.

### 6. Project qualities

(Six principles, three rules, seal, root-cause-only live in
`PRINCIPLES.md`.)

1. **If it compiles, it works.** Every known runtime panic class has
   regression test in `runtime/tests/*.rs` (e.g. `core_soundness.rs`,
   `kernel_soundness.rs`) or `crates/*/tests/` golden. Defence in depth
   (panic recovery + `Err`-return at Task boundaries) = floor, not
   foundation.
2. **Dev experience first.** Clear errors, predictable behaviour, no
   user-written FFI.
3. **Production-grade architecture.** Scales to Stripe SDK (76k FFI
   symbols). Stays maintainable.
4. **AI-written Ipê code defaults to Ipe.Ui + Ipe.Auth + Ipe.Db** — each
   reviewed for security+scalability; UI/UX/DX/security not afterthoughts.

### 7. Non-regression rules (enforced by the workspace test suite)

- **No `Result String a` / `Task String a`** in public surfaces — use
  `Result Error a` / `Task Error a`.
- **No runtime panic from well-typed Ipê code.**
- **No silent numeric coercion** — fallible checked variant = default;
  lenient display-only helpers marked as such.
- **No `dyn Any` / `.downcast` / type-erasure** in backend — concrete over
  generic (`PRINCIPLES.md` §No `dyn Any`). Wildcard `any` has exactly one
  concrete lowering per position.
- **Record field enumeration sorts by field index** before any emission
  depending on field order.
- **Secrets are typed** — `Auth.signToken` / `verifyToken` take `String`; no
  `Debug`/`Display`-formatting a secret into log or error.
- **`ipe build` ⇒ emitted Rust `cargo build`s** (THE SEAL — every
  acceptance path fails closed at ipe time, never open at cargo time).
- **Registering a kernel or new acceptance path updates ALL anti-drift sites**
  (enumerated in §0b: `ipe_kernels`, `ipe_types::constrain`, `ipe_lower`,
  `ipe_backend_rust/naming.rs`, `ipe_ir::pretty`, `crates/ipe/src/stdlib.rs`).
  Resolved-but-unschemed kernel = compile-time error, never deferred
  cargo failure — never silent `_` catchall.

### 8. Testing rules

- **Every new feature / bug becomes regression test** before fix lands;
  failing test = discovery artefact.
- **`crates/*/tests/*.rs`** (incl. `crates/ipe/tests/golden_*.rs`) for
  compile-time + codegen behaviour; **`runtime/tests/*.rs`** for runtime-kernel
  soundness/parity; goldens byte-compared and `IPE_E2E=1` builds+runs
  emitted project (THE SEAL).
- **Runtime verification.** Example sweep (`scripts/examples-sweep.sh`) builds
  AND runs each example headless per shape (cli/server/live/tui/webview/wasm);
  wasm examples drive the emitted SPA in headless Chromium
  (`scripts/lib/wasm-verify.mjs`). Build-only check doesn't catch "click is a
  no-op" regression class.

### Release checklist (non-negotiable)

1. Rebuild driver: `cargo build --release -p ipe`; `source
   scripts/lib/env.sh` exports `IPEC_BIN` + `IPE_RUNTIME_DIR`.
2. Full gate green — ONE authoritative run (§3b): `cargo +nightly nextest
   run --workspace`, `cargo +nightly nextest run -p ipe-runtime-rust --features
   full`, `cargo +nightly test --workspace --doc`, `cargo +nightly clippy
   --workspace --all-targets -- -D warnings`, fuzz.
3. Example sweep green — `scripts/examples-sweep.sh` (per example: ipe build →
   `cargo build` emitted crate → run `ipe-app`). VERDICT PASS iff zero red rows
   (THE SEAL end-to-end).
4. CI parity — `.github/workflows/{ci,examples-sweep,security}.yml` runs
   same gate; cancel superseded in-progress `main` runs before pushing (see
   Workflow rules).

Any step failing → fix root cause, re-run from step 1. Never tag w/ known
build or runtime failure.

## Contributing / PR workflow

`main` is green by construction: changes land through PRs, and a PR merges
only when a FAST required gate is green. Slow checks run post-merge and
nightly, so they never block a PR — a regression they catch is on `main`, not
in the merge queue.

**Flow:** branch → open a PR → the fast gate runs → set the PR to auto-merge
(`gh pr merge <N> --auto --squash`) → it merges the moment the gate is green
and the branch is up to date with `main`.

**The fast required gate** (target: minutes — the checks branch protection
requires):

- `fmt` — `cargo fmt --all -- --check`
- `clippy` — `cargo clippy --all-targets --workspace -- -D clippy::cargo -D clippy::complexity -D clippy::correctness -D clippy::pedantic -D clippy::perf -D clippy::style -D warnings`. The `-D` lint set is belt-and-braces over the `[workspace.lints.clippy]` deny-set — `unwrap_used` / `expect_used` / `panic` / `indexing_slicing` / `unreachable` / `todo` / `unimplemented` / `pedantic` / `nursery` / `perf` / `cargo` — plus `clippy.toml`'s `disallowed-methods` (`process::abort`, `panic_any`, the `*_unchecked` UB paths). Fix the code; never `#[allow]` around them (tests may `unwrap`/`expect` per `clippy.toml`).
- `test` — the nextest unit/integration suite (E2E tests no-op without
  `IPE_E2E`)
- `cargo-deny` — the supply-chain gate (see below)
- `seal-smoke` — build the compiler, then take one small example
  end to end (`ipe build` → `cargo build` the emitted crate → run it → assert
  output). A fast proxy for THE SEAL; the full 6-shard `e2e` runs post-merge.

**Slow checks** run on push-to-`main` + a nightly `schedule`, never on a PR:
the full 6-shard `e2e` (THE SEAL in full), live `sky-parity`, `miri`, the
runtime feature-combo / full-feature builds, and `examples-sweep`. `wasm-floor`
is off the always-required PR path — on a PR it runs only when wasm-relevant
files change (a `paths` filter), plus nightly. Triggers live in
`.github/workflows/{ci,security,examples-sweep,static}.yml`.

**`cargo-deny`** is the one supply-chain gate. Its `advisories` check subsumes
`cargo-audit` (same RustSec DB), and it also covers `licenses` / `bans` /
`sources` in one lockfile-only pass. Policy + every accepted exception (with a
written justification) live in `deny.toml` at the repo root. A real finding
fails the PR; an unfixable advisory is handled by a documented, reviewed
ignore — never by downgrading the gate.

**Branch protection** is enabled by running
`scripts/ci/enable-branch-protection.sh` (required checks = the five fast jobs,
`strict` up-to-date branch, PRs required, auto-merge on). It is deliberately
run by hand, not by CI — flip it on only after in-flight direct-push lanes
drain.

## Versioning & Releases

Versioning, changelog, and release cutting are automated by
[release-please](https://github.com/googleapis/release-please)
(`.github/workflows/release-please.yml`), driven by the same Conventional
Commit messages the PR workflow already uses. Contributors never bump a version
or edit `CHANGELOG.md` by hand.

**How a release ships.** On every push to `main`, release-please maintains one
standing **release pull request**. It bumps the workspace version in
`Cargo.toml` and prepends the next `CHANGELOG.md` section, derived from the
commits merged since the last release. That PR is inert while open — nothing is
released until it merges. **Merging the release PR is the act of shipping:**
merging it tags `vX.Y.Z`, creates the GitHub release with the changelog as its
body, and that tag's release event triggers `release.yml` to build the
5-platform binaries and attach them to the release. There is exactly one release
object per tag — release-please creates it; `release.yml` only uploads assets to
it.

**Pre-1.0 bump rules** (config in `release-please-config.json`, current version
`0.1.0`):

| Commit | Version effect (pre-1.0) |
|---|---|
| `feat`, `fix`, `perf` | **patch** (`0.1.0` → `0.1.1`) |
| `!` suffix / `BREAKING CHANGE:` footer | **minor** (`0.1.0` → `0.2.0`) |
| `docs`, `chore`, `ci`, `test`, `refactor`, `style`, `build` | no release entry |

`1.0.0` is **never** reached automatically — while `0.x`, a breaking change
only bumps the minor. Cutting `1.0` is a deliberate, hand-made stability
promise: the public surface is committed to and breaking changes thereafter bump
the major per SemVer. Until then, treat any `0.x` release as potentially
breaking.

**Ship a release for every user-facing change.** Any change to the user
interface — the CLI (commands, flags, help text), diagnostics and `explain`
pages, the formatter's output, or a public API — is shipped as a release: merge
the standing release PR once the change lands, so users receive it under a
version rather than only on `main`.

## Workflow rules

- **Always run mem-guard** (§1).
- **Always clean up background tasks** before declaring "done" (§2).
- **`cargo fmt --all` after editing Rust source** (CI gates on `cargo fmt
  --all -- --check`).
- **`-f` flag with `rm`/`cp`** to avoid interactive prompts.
- **Never add co-author wording** to commits.
- **Never tag a release** without explicit user ask.
- **Run `ipe build` on an example from its own dir** (`cd examples/NN-name`),
  never from repo root — `--out out/rust` writes relative to cwd.
- **Cancel in-progress CI runs on `main` before pushing** (newer commit
  supersedes them; never cancel release/tag runs):

```bash
gh run list --branch main --status in_progress --workflow CI --json databaseId --jq '.[].databaseId' \
    | xargs -I{} gh run cancel {} 2>/dev/null
git push origin main
```
- **`AGENTS.md` tracks language surface.** When stdlib, syntax, or CLI
  change, update Ipê authoring reference (`AGENTS.md`) in same commit.
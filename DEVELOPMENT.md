# DEVELOPMENT.md — dev-ops & orchestration non-negotiables

> Companion to **`PRINCIPLES.md`** (the enforcement SSOT: six principles, three
> fundamental rules, THE SEAL, §0 no-shortcuts, clippy deny-set, the two-tier
> gate, write-boundary, agent-lane rules, doc/code standards) and **CLAUDE.md**
> (the Ipê *language* authoring reference). This file holds the operational
> HOW: infrastructure, commands, checklists — the Rust toolchain (`cargo`,
> `crates/`, `runtime/`, `skyc`). Autonomous-loop lanes follow
> `scripts/progressive-development/context.md`.

Documentation hygiene: no archaeology — see `PRINCIPLES.md`
§Documentation & code standards.

## Non-negotiables

### 0. No shortcuts — root cause or honest blocker (MANDATORY)

`PRINCIPLES.md` §0. Never delete/skip/edit the thing that triggers a bug,
weaken a gate, or `#[allow]` a real violation; root-cause it or file an honest
tracked blocker. A green obtained by deleting the red is a FAILURE.

### 0a. Understand before you change — `ipe-index` + reference-first (MANDATORY)

The rule (port, don't invent) is `PRINCIPLES.md` §Agent-lane operational
rules. The tooling:

- **`ipe-index` FIRST for our own code** (`crates/` `runtime/` `tools/`) — a
  pre-built structural index, not a fresh search:
  - `ipe-index locate <Module.function>` — symbol location + kernel-parity
    route (Sky → Haskell → Go → Rust impl paths).
  - `ipe-index def <sym>` / `refs <sym>` / `kind <fn|struct|enum|trait|type|impl>`.
  - `ipe-index parity --gaps` — Go-vs-Rust kernel parity gaps.
  Reserve `rg` for free-text hunts the index cannot answer.
- **Learn how the reference handles THIS task before designing the fix.**
  `../sky` is the READ-ONLY source of truth. For the construct you are fixing,
  read each layer: the **Sky compiler** (Haskell, `../sky/src/Sky/` —
  parse/canon/type/lower), the **Go backend** (`../sky/runtime-go/`, the
  byte-diff parity oracle), the **Rust backend**
  (`../sky/src/Sky/Generate/Rust/`), the **Rust runtime** (the vendored
  behaviour it emits into). `skydex locate <sym>` gives the cross-lang route.
- Only once you can state (a) where OUR code handles it and (b) how the
  reference handles it should you design the change.

### 0b. Infrastructure at a glance — read this, don't re-learn it

**Compiler pipeline (acyclic crate stages):** `sky_parse` → `sky_canon` (name
resolve) → `sky_types` (HM infer/constrain) → `sky_lower` (AST→IR) → `sky_ir`
→ `sky_backend_rust` (emit Rust). Support crates: `sky_kernels` (kernel
table), `sky_diagnostics` (IPE-* codes + `explain/*.md`), `sky_db` (salsa
incremental DB), `sky_intern`, `sky_watch`; `skyc` = driver + CLI. Runtime
impls live in `runtime/src/sky_runtime/`.

**skyc CLI:** subcommands `build` / `run` / `watch` / `explain` / `fix`.
`skyc build <src/Main.ipe | sky.toml> --out sky-out/rust`. Binary =
`target/release/skyc` (`cargo build --release -p skyc`);
`source scripts/lib/env.sh` sets `SKYC_BIN` + `IPE_RUNTIME_DIR`.

**Registering a kernel = update ALL anti-drift sites** (type-checker enforces
most; miss one → IPE-N0028 / IPE-L0108 / a drift test): `sky_kernels` (enum +
`decl()` + `ALL`), `sky_types::constrain` (type-scheme + `FIRST_SCHEMED`, out
of the `KNOWN_UNBACKED` bucket), `sky_lower` (arity table +
`REGISTRY_ONLY_ALLOWLIST` for alias-only kernels),
`sky_backend_rust/naming.rs`, `sky_ir::pretty`, `crates/skyc/src/stdlib.rs`
(module registration). Template to seal a new stdlib module:
`crates/skyc/tests/golden_stdlib_module_seal.rs`.

**Examples + sweep:** an example = `examples/NN-name/src/Main.ipe` (+ other
`.ipe` modules, `sky.toml`). The `build_set` is **disk-derived**
(`scripts/lib/examples.sh`) — every `examples/NN-*/src/Main.ipe` whose imports
resolve is auto-included; adding the dir IS the registration.
`scripts/equivalence-checks/examples-sweep.sh`, per example: `skyc build … --out sky-out/rust` →
`cargo build --manifest-path sky-out/rust/Cargo.toml` → run
`sky-out/rust/target/debug/sky-app`. VERDICT PASS iff zero red rows. Modes:
`IPE_SWEEP_BUILD_ONLY=1` (compile only), `IPE_SWEEP_NO_EQUIV=1` (build+run, no
Go), default (+ Go≡Rust via cached `expected_go.txt`).

**Emitted project:** `sky-out/rust/` = a Cargo project with the runtime
vendored into `src/sky_runtime/` (skyc copies from `IPE_RUNTIME_DIR`), default
binary `sky-app`, edition 2024.

**Golden tests** (`crates/skyc/tests/golden_*.rs`): a golden =
`tests/golden/<name>/Main.ipe` + `main.rs` (expected emit, **byte-compared**)
+ a cached Go oracle (`expected_go.txt` / `oracle.meta`). Default run =
byte-identity of the emit (fast, no cargo). `IPE_E2E=1` = build+run the
emitted project (THE SEAL: skyc-0 ⇒ cargo-0). Oracle files are regenerated
ONLY by `cargo run -p refresh-oracle -- <golden>` — NEVER hand-edited.

**Build & cache (8 cores / 15 GB RAM → RAM-BOUND, not core-bound):**
`~/.cargo/config.toml` sets `rustc-wrapper = sccache`, the `mold` linker,
`incremental = false`, and `jobs = 2` — an OOM guard **per cargo invocation**
(2 concurrent lanes already ≈ 4 parallel `rustc`, near the RAM ceiling;
raising `jobs` multiplies per lane → OOM). **Never override `RUSTFLAGS`** —
the config's `mold`-only flags ARE the sccache cache key; extra flags fork the
key → cold recompiles + more RAM pressure. All cargo targets live under
`~/.cache/ipe/` (write-boundary — `PRINCIPLES.md`); E2E emitted builds use
`IPE_ORACLE_SHARED_TARGET`. `cargo nextest run -p skyc` recompiles ALL ~155
skyc test binaries — scope to `--test <name>` when you need one.

### 1. Memory safety — `scripts/guards/mem-guard.sh` MUST run during dev

A runaway compiler-tooling process can OOM the host. Treat absence of
mem-guard like a missing `set -e`.

```bash
nohup ./scripts/guards/mem-guard.sh > /tmp/mem-guard.out 2>&1 &
disown                                # survives shell exit
```

Defaults (16 GB host): per-process kill at 6 GB RSS for compiler tooling
(`cargo`/`rustc`/`cc1`/`cc1plus`/`cc`/`collect2`/`ld`/`ld.lld`/`lld`/`skyc`/
`sky-ffi-inspect`/`rust-analyzer`); 10 GB panic tier for dev-session hosts
(`claude`/`node`/`ghostty`); system-pressure floor at <1.2 GB free.
Tune via `MEM_GUARD_PROC_MB` / `MEM_GUARD_PANIC_MB` / `MEM_GUARD_SYS_FLOOR_MB`;
`MEM_GUARD_DRY=1` = log-only. Never silence a kill by raising a threshold — a
kill means the process was on a path to OOM the machine; fix the underlying
bug.

### 2. Background-task hygiene — clean up before declaring "done"

Orphan `run_in_background` wait-loops exhaust the per-uid process table
(`fork: retry: Resource temporarily unavailable`), which silently kills
mem-guard. End-of-mission checklist:

```bash
# Orphan polling loops
ps -u $USER -o pid,command | awk '/while pgrep|until ! pgrep/ && /\/bin\/zsh -c/ {print $1}' | xargs -n1 kill -9 2>/dev/null

# Stray sleeps + verification leftovers
ps -u $USER -o pid,ppid,command | awk '$3 == "sleep" && $2 != 1 {print $1}' | xargs -n1 kill -9 2>/dev/null
pkill -f "playwright"; pkill -f "chromium"
pkill -f "examples/.*/sky-out/app"

# mem-guard alive?
pgrep -f mem-guard.sh >/dev/null || (nohup ./scripts/guards/mem-guard.sh > /tmp/mem-guard.out 2>&1 & disown)
```

**Prefer the Monitor tool** (orchestrator only — lanes are foreground-only,
`PRINCIPLES.md` §Agent-lane rules) over `run_in_background` + polling.

### 2c. Caveman-ultra output — mandatory in EVERY agent brief

EVERY dispatched agent runs **caveman-ultra** output — autopilot lanes get it
via `context.md` §7; every hand-dispatched `Agent` brief MUST carry the same
directive. Rules: terse; drop articles, filler, hedging, pleasantries;
fragments fine; one line where one line does; `X -> Y` for causality. **Code,
paths, identifiers, and error text stay EXACT and verbatim.** No preamble.
Final line is always the verdict (`DONE`/`STUCK`/`PARTIAL`/`REVIEW:`/…).
Terseness never trades away correctness, the gate, or a required verdict line.

### 3. Timeout gate — every long-running command MUST be timeout-bounded

A hung test/build is a silent task waster. Rules:

- **The full gate under a timeout.** Every `cargo nextest run` / `cargo test`
  in the gate is wrapped (`autopilot.sh` uses `timeout 3000` for the workspace
  run). Not enough → that's a flaky test; bisect it, don't widen the ceiling.
- **Per-step timeouts.** Any step that execs a subprocess (`skyc build` /
  `skyc run` / `skyc watch`) wraps the child in `timeout`. A step that doesn't
  time out cannot be re-run.
- **Example sweep** already bounds every stage: skyc build `timeout
  ${IPE_SWEEP_BUILD_TIMEOUT:-900}`, `cargo build` `timeout 900`, emitted-app
  run `timeout 8` (`exercise_cli` in `scripts/lib/checks.sh`) — don't remove or
  widen without a real reason.
- **Background shell commands** waiting on a process MUST `kill -KILL` after a
  finite wait (default 600 s). Never `wait $PID` unbounded.
- **Monitors** in dev-loop tooling (`skyc watch`) need a heartbeat/max-wait so
  a wedged child doesn't poison the parent.

A process running >30 min unjustified: kill it and file a bug. Never wait it
out.

### 3b. The two-tier gate — operational detail

The rule (cheap per-lane vs the ONE authoritative full gate; components of
each) is `PRINCIPLES.md` §The two-tier gate. Implementation:
`scripts/progressive-development/autopilot.sh`; master only advances to a
full-gate-certified sha.

**Cheap gate (`lane_gate`)** — merges the lane into the integration worktree,
then checks + tests + lints ONLY the touched crates:
- `cargo +nightly check -p skyc`
- `cargo +nightly nextest run <-p touched-crates>` (scoped; no `IPE_E2E`)
- `cargo +nightly clippy <-p touched-crates> --no-deps -- -D warnings`

**Full gate (`full_gate` via `certify_batch`)** — every
`PROGDEV_FULL_GATE_EVERY` cycles (default 10) OR the instant pending work
drains:
- `cargo +nightly nextest run --workspace` (+ `IPE_ORACLE_SHARED_TARGET` for E2E)
- `cargo +nightly nextest run -p sky-runtime-rust --features full`
  (LOAD-BEARING — mirror of CI's `runtime-full-features`)
- `cargo +nightly test --workspace --doc`
- `cargo +nightly clippy --workspace --all-targets -- -D warnings`
- fuzz (`scripts/fuzz-well-typed.sh`)

**`--all-targets` rollout.** Target end-state: BOTH gates run clippy
`--all-targets` (catches test-binary lint debt). `--all-targets` enters the
FULL gate only once the test-file clippy-debt sweep is clean (else the full
gate reds); until then the cheap gate stays `--all-targets`-free to match.
Never add `--all-targets` to one gate without the other.

### 3c. Lint enforcement

`PRINCIPLES.md` §Mechanical enforcement — comply by construction: the deny-set
(incl. `doc_markdown` backticks), the per-site-`#[allow]`-only escape hatch,
the `unsafe` policy. The gate runs `clippy -D warnings`; fix the code, never
the lint level.

### 4. No-deferral — pipeline mechanics

The rule ("pre-existing" is never a shipping excuse; fix-first; only an
explicit user override ships a known issue) is `PRINCIPLES.md` §0. Mechanics:

- **Spotted = filed.** Any test/sweep failure, runtime panic, or log error →
  task created on the spot.
- **Group related fixes** into the next patch release to cut notification
  noise — don't tag per fix.
- **Closing requires the actual fix.** A documented workaround is a TEMPORARY
  bridge only, never permanent.
- **"Pre-existing" is investigation context, not a verdict** — it means the
  fix can ship in its own commit, not that it can be skipped.
- A hard problem is a reason to START (root cause → architecturally correct
  approach → execute, even across sessions), not to defer.

### 5. Disk hygiene — unused build caches MUST be pruned

**Write-boundary** (the ONLY two writable locations — cargo targets under
`~/.cache/ipe/`, edits under the repo tree) is `PRINCIPLES.md`
§Write-boundary. Operationally:

- The loop's targets are `~/.cache/ipe/{gate-target, oracle-target,
  lane-<N>-target}`; a hand-dispatched agent or manual verify build uses
  `~/.cache/ipe/<purpose>-target`.
- **Enforced in `autopilot.sh`**: `IPE_CACHE=~/.cache/ipe`; `reclaim_disk`
  keeps the gate + oracle + warm `lane-*` targets and reaps the rest, AND
  sweeps stray cargo targets under `~/.cache/*target*` or `/tmp`
  (pgrep-guarded). Every dispatched-agent brief MUST set `CARGO_TARGET_DIR`
  (and `IPE_ORACLE_SHARED_TARGET`) under `~/.cache/ipe/`.

**Pre-build disk check — BEFORE any full build/test suite/example sweep.**
`df -h /`; if <~15–20 GB free, reclaim first: `rm -rf "$CARGO_TARGET_DIR"`,
prune stray targets under `~/.cache/ipe/`, prune per-example artifacts
(`sky-out/`). A near-full disk dies mid-run with ENOSPC *after*
type-check+codegen succeed, surfacing as a file-copy/"build failed" error that
**masquerades as a codegen regression** and wastes the whole run on
mis-diagnosis — always read the actual build log before blaming a code change.

`scripts/guards/disk-guard.sh` (sibling to mem-guard) polls free disk and reclaims
disposable caches BEFORE the disk fills, in a fixed safety order:
`~/.cache/sccache` first (self-healing), then orphaned cargo target dirs
(identified by their `CACHEDIR.TAG` content, not by name), never a dir with a
live rustc/cargo process still writing to it.

End-of-mission checklist (BEFORE declaring a release shipped when a sweep has
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

**Automatic hygiene:** `scripts/equivalence-checks/examples-sweep.sh` aborts with a `< 5G free`
guard before it starts; the loop's `reclaim_disk` (`autopilot.sh`) keeps the
gate + shared-oracle + warm `lane-*` targets and reaps the rest. Worktree
cleanup after every finished agent remains manual.

Host <5 GB free → ABORT the next agent spawn until cleanup completes — ENOSPC
mid-build leaves half-written artifacts worse than a clean rebuild.

### 6. Project qualities

(The six principles, three rules, seal, and root-cause-only live in
`PRINCIPLES.md`.)

1. **If it compiles, it works.** Every known runtime panic class has a
   regression test in `runtime/tests/*.rs` (e.g. `core_soundness.rs`,
   `kernel_soundness.rs`) or a `crates/*/tests/` golden. Defence in depth
   (panic recovery + `Err`-return at Task boundaries) is the floor, not the
   foundation.
2. **Dev experience first.** Clear errors, predictable behaviour, no
   user-written FFI.
3. **Production-grade architecture.** Scales to the Stripe SDK (76k FFI
   symbols). Stays maintainable.
4. **AI-written Sky code defaults to Std.Ui + Std.Auth + Std.Db** — each
   reviewed for security+scalability; UI/UX/DX/security are not afterthoughts.

### 7. Non-regression rules (enforced by the workspace test suite)

- **No `Result String a` / `Task String a`** in public surfaces — use
  `Result Error a` / `Task Error a`.
- **No runtime panic from well-typed Sky code.**
- **No silent numeric coercion** — the fallible checked variant is the default;
  lenient display-only helpers are marked as such.
- **No `dyn Any` / `.downcast` / type-erasure** in the backend — concrete over
  generic (`PRINCIPLES.md` §No `dyn Any`). Wildcard `any` has exactly one
  concrete lowering per position.
- **Record field enumeration sorts by field index** before any emission that
  depends on field order.
- **Secrets are typed** — `Auth.signToken` / `verifyToken` take `String`; no
  `Debug`/`Display`-formatting a secret into a log or error.
- **`skyc build` ⇒ the emitted Rust `cargo build`s** (THE SEAL — every
  acceptance path fails closed at skyc time, never open at cargo time).
- **Registering a kernel or new acceptance path updates ALL anti-drift sites**
  (enumerated in §0b: `sky_kernels`, `sky_types::constrain`, `sky_lower`,
  `sky_backend_rust/naming.rs`, `sky_ir::pretty`, `crates/skyc/src/stdlib.rs`).
  A resolved-but-unschemed kernel is a compile-time error, never a deferred
  cargo failure — never a silent `_` catchall.

### 8. Testing rules

- **Every new feature / bug becomes a regression test** before the fix lands;
  the failing test is the discovery artefact.
- **`crates/*/tests/*.rs`** (incl. `crates/skyc/tests/golden_*.rs`) for
  compile-time + codegen behaviour; **`runtime/tests/*.rs`** for runtime-kernel
  soundness/parity; goldens are byte-compared and `IPE_E2E=1` builds+runs the
  emitted project (THE SEAL).
- **Runtime verification.** The example sweep (`scripts/equivalence-checks/examples-sweep.sh`)
  builds AND runs each example, then diffs Rust≡Go stdout/body via the cached
  oracle; web scenarios drive Sky.Live + Sky.Http.Server through Playwright
  (`scripts/equivalence-checks/web-verify.mjs`, `scripts/equivalence-checks/verify-scenarios.mjs`). A build-only
  check doesn't catch the "click is a no-op" regression class.

### Release checklist (non-negotiable)

1. Rebuild the driver: `cargo build --release -p skyc`; `source
   scripts/lib/env.sh` exports `SKYC_BIN` + `IPE_RUNTIME_DIR`.
2. Full gate green — the ONE authoritative run (§3b): `cargo +nightly nextest
   run --workspace`, `cargo +nightly nextest run -p sky-runtime-rust --features
   full`, `cargo +nightly test --workspace --doc`, `cargo +nightly clippy
   --workspace --all-targets -- -D warnings`, fuzz.
3. Example sweep green — `scripts/equivalence-checks/examples-sweep.sh` (per example: skyc build →
   `cargo build` the emitted crate → run `sky-app` → Rust≡Go equivalence). VERDICT
   PASS iff zero red rows (THE SEAL end-to-end).
4. CI parity — `.github/workflows/{ci,examples-sweep,security}.yml` runs the
   same gate; cancel superseded in-progress `main` runs before pushing (see
   Workflow rules).

Any step failing → fix root cause, re-run from step 1. Never tag with a known
build or runtime failure.

## Workflow rules

- **Always run mem-guard** (§1).
- **Always clean up background tasks** before declaring "done" (§2).
- **`cargo fmt --all` after editing Rust source** (CI gates on `cargo fmt
  --all -- --check`).
- **`-f` flag with `rm`/`cp`** to avoid interactive prompts.
- **Never add co-author wording** to commits.
- **Never tag a release** without explicit user ask.
- **Run `skyc build` on an example from its own dir** (`cd examples/NN-name`),
  never from the repo root — `--out sky-out/rust` writes relative to the cwd.
- **Cancel in-progress CI runs on `main` before pushing** (a newer commit
  supersedes them; never cancel release/tag runs):

```bash
gh run list --branch main --status in_progress --workflow CI --json databaseId --jq '.[].databaseId' \
    | xargs -I{} gh run cancel {} 2>/dev/null
git push origin main
```
- **`CLAUDE.md` tracks language surface.** When stdlib, syntax, or the CLI
  change, update the Ipê authoring reference (`CLAUDE.md`) in the same commit.

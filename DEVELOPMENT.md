# DEVELOPMENT.md — dev-ops & orchestration non-negotiables

> Companion to **CLAUDE.md** (the Ipê *language* authoring reference). This file
> holds the **development-workflow** non-negotiables that were relocated out of
> CLAUDE.md so the language reference stays lean. Preserved from the pre-Ipê
> (Sky→Go) CLAUDE.md — some items name the Haskell/Go toolchain (`cabal`,
> SkyDeploy, examples `00`-`31`); treat those as historical and adapt to the Rust
> port (`cargo`, `crates/`, `runtime/`) as it matures. The tool-agnostic rules —
> mem-guard, background-task hygiene, timeout gate, no-deferral, disk hygiene,
> root-cause-only, non-regression, test-first — carry over unchanged.
>
> For the autonomous-loop agent contract see
> `scripts/progressive-development/context.md` (it distills the seal + principles
> + the command/output hygiene rules for dispatched agents).

## Documentation hygiene — state the contract, not its history

Every doc here (and every code comment) states what the rule or design **is now**,
never how it got there. **No archaeology:** no dates, no `(2026-07-15)`, no "was X,
now Y", no incident post-mortems, no changelog notes in prose. Git history and
`git blame` already record *when* and *why* a line changed — duplicating that in
the text only rots and bloats it. When a rule needs a rationale, give the
*structural* reason it holds ("a cargo target outside the prune root is invisible
to reclaim and fills the disk"), not the story of the day it was learned.

## Non-negotiables

### 0. Understand before you change — `ipe-index` + reference-first (MANDATORY)

Before writing or changing ANY compiler/runtime code, you MUST understand both the
code you are about to touch AND how the reference already handles the task. A fix
that doesn't match the reference's proven logic is a guess — **port, don't invent**.

- **`ipe-index` FIRST for our own code** (`crates/` `runtime/` `tools/`) — it is a
  pre-built structural index, not a fresh search. Use it before `rg`:
  - `ipe-index locate <Module.function>` — where a symbol lives + its kernel-parity
    route (Sky → Haskell → Go → Rust impl paths).
  - `ipe-index def <sym>` / `refs <sym>` / `kind <fn|struct|enum|trait|type|impl>`.
  - `ipe-index parity --gaps` — Go-vs-Rust kernel parity gaps.
  Reserve `rg` for free-text/substring hunts `ipe-index` cannot answer.
- **Learn how the reference handles THIS task before designing the fix.** `../sky`
  is the READ-ONLY source of truth for the whole pipeline. For the construct you
  are fixing, read how each layer deals with it:
  - the **Sky compiler** (Haskell, `../sky/src/Sky/`) — parse/canon/type/lower;
  - the **Go backend** (`../sky/runtime-go/`) — the byte-diff parity oracle target;
  - the **Rust backend** (`../sky/src/Sky/Generate/Rust/`) — emission + lowering;
  - the **Rust runtime** (the runtime crate) — the vendored behaviour it emits into.
  (`skydex` on PATH indexes this reference: `skydex locate <sym>` gives the cross-
  lang route.)
- Only once you can state (a) where OUR code handles it and (b) how the reference
  handles it across compiler + Rust backend + Rust runtime should you design the
  change. This is the discovery step that turns a "guess-and-check" lane into a
  faithful port.

### 1. Memory safety — `scripts/mem-guard.sh` MUST run during dev

A runaway `sky`/`cabal`/`ghc`/`haskell-language-server` process previously
force-powered-off the host Mac. Treat absence of mem-guard like a missing
`set -e`.

```bash
nohup ./scripts/mem-guard.sh > /tmp/mem-guard.out 2>&1 &
disown                                # survives shell exit
```

Defaults (16 GB Mac): per-process kill at 6 GB RSS for compiler tooling
(`sky`/`cabal`/`ghc`/`ghc-iserv`/`cc1`/`ld`/`haskell-language-server`/
`hls-wrapper`/`gopls`/`sky-ffi-inspect`); 10 GB panic tier for dev-session
host (`claude`/`node`/`ghostty`); system-pressure floor kicks in when
free+inactive+speculative memory <1.2 GB. Tune via `MEM_GUARD_PROC_MB` /
`MEM_GUARD_PANIC_MB` / `MEM_GUARD_SYS_FLOOR_MB`. `MEM_GUARD_DRY=1` =
log-only mode. Never silence a kill by raising threshold — kill means the
process was on a path to OOM the machine; fix the underlying compiler bug.

### 2. Background-task hygiene — clean up before declaring "done"

Long sessions accumulate orphan `run_in_background` zsh wait-loops that
exhaust per-uid process table (`fork: retry: Resource temporarily
unavailable`) → `mem-guard.sh` silently dies → user's binaries killed
instantly on launch.

End-of-mission checklist:

```bash
# Orphan polling loops
ps -u $USER -o pid,command | awk '/while pgrep|until ! pgrep/ && /\/bin\/zsh -c/ {print $1}' | xargs -n1 kill -9 2>/dev/null

# Stray sleeps + verification leftovers
ps -u $USER -o pid,ppid,command | awk '$3 == "sleep" && $2 != 1 {print $1}' | xargs -n1 kill -9 2>/dev/null
pkill -f "playwright"; pkill -f "chromium"
pkill -f "examples/.*/sky-out/app"

# mem-guard alive?
pgrep -f mem-guard.sh >/dev/null || (nohup ./scripts/mem-guard.sh > /tmp/mem-guard.out 2>&1 & disown)
```

**Prefer the Monitor tool** over `run_in_background` + polling — delivers
events without leaving a wait-loop subprocess.

### 2c. Caveman-ultra output — mandatory in EVERY agent brief

Token discipline (non-negotiable). EVERY dispatched agent runs **caveman-ultra**
output — autopilot lanes get it via `context.md §7`; every HAND-dispatched `Agent`
MUST carry the same directive in its brief. Rules: terse; drop articles, filler
(just/really/basically), hedging, pleasantries; fragments fine; one line where one
line does; `X -> Y` for causality. **Code, paths, identifiers, and error text stay
EXACT and verbatim — never abbreviate those.** No preamble ("I'll now…", "Let me…").
Final line is always the verdict (`DONE`/`STUCK`/`PARTIAL`/`REVIEW:`/…). Terseness
never trades away correctness, the gate, or a required verdict line. When you spawn
an agent, adding the caveman-ultra directive is not optional.

### 3. Test / build timeout gate — every long-running command MUST be timeout-bounded

A hung test/build is a silent task waster (already lost 7h waiting on a
stuck `Sky.Cli.Watch` subprocess). Never again.

Rules:
- **`cabal test` MUST run under `timeout`**:
  `timeout 3600 cabal test` (60 min hard ceiling). If 60 min is
  not enough, that's a flaky test — bisect it, don't widen the
  ceiling.
- **Per-spec timeouts.** Hspec specs that exec subprocesses
  (`sky build` / `sky watch` / `sky test`) MUST wrap the child in
  `timeout 60` or use Hspec's `Test.Hspec.Wai`-style timeout
  combinators. A test that doesn't time out cannot be re-run.
- **Example sweep already enforces this** via
  `run_with_timeout 10` in `scripts/example-sweep.sh`. Don't
  remove or widen those calls without a real reason.
- **Background `run_in_background` shell commands** that wait
  on a process MUST `kill -KILL` it after a finite wait —
  default 600 s ceiling. Never `wait $PID` unbounded.
- **Monitors** in dev-loop tooling (sky watch, sky doctor)
  watching for state changes MUST have a heartbeat / max-wait
  so a wedged child doesn't poison the parent.

If a process runs >30 min unjustified, kill it and file a bug. Never wait
it out.

### 3b. The progressive-development two-tier gate (cheap per-lane / full every-N)

`scripts/progressive-development/autopilot.sh` gates landed work at two tiers.
Master only ever advances to a **full-gate-certified** sha.

**Cheap gate (`lane_gate`) — per lane, fast (~minutes).** Merges the lane into
the integration worktree, then builds + lints ONLY the crates the change touched:
- `cargo +nightly build -p skyc`
- `cargo +nightly nextest run <-p touched-crates>` (scoped; no `SKY_E2E`)
- `cargo +nightly clippy <-p touched-crates> --no-deps -- -D warnings`

A cheap-green lane lands on integration but stays *uncertified* (added to the
batch, kept claimed).

**Full gate (`full_gate` via `certify_batch`) — every `PROGDEV_FULL_GATE_EVERY`
cycles (default 10) OR the instant pending work drains, whichever first.** The
authoritative check over the whole accumulated batch:
- `cargo +nightly nextest run --workspace` (+ `SKY_ORACLE_SHARED_TARGET` for E2E)
- `cargo +nightly nextest run -p sky-runtime-rust --features full` (LOAD-BEARING —
  `default=[]` means the workspace run skips every `#[cfg(feature)]`-gated test,
  incl. the whole `live::*` surface; mirror of CI's `runtime-full-features`)
- `cargo +nightly test --workspace --doc`
- `cargo +nightly clippy --workspace --all-targets -- -D warnings`
- fuzz (`scripts/fuzz-well-typed.sh`)

Full-green → close the batch + fast-forward master to that sha. Full-red → reset
integration to the last certified sha + re-queue the batch.

**`--all-targets` policy.** The two gates MUST agree on lint scope,
and the cheap gate must never be *stricter* than the full gate (else it rejects
lanes the authority would accept). Target end-state: **both** gates run clippy
`--all-targets` (so test-binary lint debt — `clippy::panic`/`expect_used`/
`map_unwrap_or`/`doc_markdown` in `tests/*.rs` helpers — is caught, per
PRINCIPLES §comply-by-construction). Because that debt accumulated while the gate
ran `--workspace` only, `--all-targets` goes into the FULL gate ONLY once the
test-file clippy-debt sweep is clean (else the full gate reds); until then the
cheap gate stays `--all-targets`-free to match. Do not add `--all-targets` to one
gate without the other.

### 3c. Lint enforcement — comply by construction, never lower the lint

The gate runs `clippy -D warnings`; a firing lint is fixed in the **code**, never
silenced by lowering the lint level. Workspace denies (root `Cargo.toml`
`[workspace.lints.clippy]`): `unwrap_used`, `expect_used`, `panic`,
`indexing_slicing`, `unreachable`, `todo`, `unimplemented`, `pedantic`, `nursery`.
`pedantic` includes `doc_markdown` — **code identifiers in doc (`///`/`//!`) and
`//` comments MUST be backticked** (`` `CloneOk` ``, `` `Vec<T>` ``, `` `--all-targets` ``).
These apply to `tests/*.rs` too (the `--all-targets` end-state, §3b). The only
sanctioned escape is a **per-site** `#[allow(lint)] // one-line why` (e.g. the
runtime HMAC-infallible sites, a shared-test-helper `dead_code`) — never a crate-
or gate-wide relaxation.

### 4. No-deferral principle — every known bug enters the pipeline

> Sky Lang aspires to be industrial best-in-class language+toolchain for
> fullstack pure-functional dev. SkyDeploy is the moat platform startups
> build on. A "known broken edge case" today compounds tomorrow. Session
> value = architectural progress, not the tag — a fix taking
> multi-session/days/weeks is a reason to START, not defer. **Default
> response to a hard problem: analyse root cause → research the
> architecturally correct approach (roadmap docs/RFCs/improvement plans)
> → execute, even across multiple sessions.** Tactical workaround forbidden
> unless user explicitly accepts the trade-off after hearing it.

Any bug surfacing during dev/sweep/CI/testing — **whether introduced by
current work or pre-existing** — MUST enter the task pipeline immediately,
fixed in next appropriate patch release. "Pre-existing flake" / "defer to
v0.X" / "known issue, ignore" are forbidden shipping excuses.

Rules:
- **Spotted = filed.** Any test/sweep failure, runtime panic, log error →
  task created on the spot. No "I'll look at it later".
- **Pipeline groups related fixes** into next patch release (v0.15.x) to
  cut notification noise — don't tag per fix.
- **Closing requires actual fix, not workaround.** A documented workaround
  in CLAUDE.md is acceptable as TEMPORARY bridge only, never permanent.
- **"Pre-existing" is investigation context, not a verdict** — tells you
  the fix can ship in its own commit (not bundled with unrelated work),
  does NOT excuse skipping it.

The user has the right to interrupt with "ship this without
fixing X" — only that explicit override allows shipping with a
known unfixed issue. Default is fix-first.

### 5. SkyDeploy redeploy follows every Sky release

Every tagged (`vX.Y.Z`) Sky compiler/stdlib release MUST pair with a
SkyDeploy redeploy of the matching version:

```bash
cd ~/works/playground/skydeploy
# 1. Bump SKY_VERSION in all 5 refs:
#    - sky-tools/Dockerfile
#    - deploy/Dockerfile
#    - agent-service/Dockerfile
#    - build-image/Dockerfile
#    - control-plane/deploy/setup-remote.sh
# 2. Commit + push origin main.
# 3. Bounded redeploy:
timeout 1200 bash control-plane/deploy/deploy.sh
```

**Graceful degradation on auth failure.** If `gcloud` auth expired (token
revoked/refresh needed/SSO challenge) or any deploy-side blocker fires, do
NOT retry indefinitely:

1. Detect via bounded `timeout`'s exit code OR `gcloud auth` stderr
   complaints.
2. **Park the redeploy** — bump commit on skydeploy `main` already
   pushed; that's the durable artifact.
3. **Warn user explicitly**: "SkyDeploy redeploy parked due to `<reason>`
   — please `gcloud auth login` and re-run
   `control-plane/deploy/deploy.sh` when convenient. Sky compiler work
   continuing." Include the exact gcloud command needed.
4. **Continue Sky compiler/stdlib work** without blocking on deploy.

Deploy = downstream consumption of the release; the release itself (tag +
GitHub release) is the authoritative artifact. Sky's flow doesn't block on
operational state outside the compiler repo.

### 6. Disk hygiene — unused build caches MUST be pruned

**Write-boundary — the ONLY two locations anything (agent or human) may write
(MANDATORY).** Cargo targets and scratch build state go under
`~/.cache/ipe/` and NOWHERE else; source/doc/test edits go under the repo working
tree and nowhere else. Concretely:

- **`CARGO_TARGET_DIR` MUST live under `~/.cache/ipe/`** — never `/tmp`, never
  `$HOME` root, never a bare `~/.cache/<name>-target`. The loop's targets are
  `~/.cache/ipe/{gate-target, oracle-target, lane-<N>-target}`; a hand-dispatched
  agent or a manual verify build uses `~/.cache/ipe/<purpose>-target`.
- **Why:** the disk-prune logic scans exactly two roots — `~/.cache/ipe/` (the
  sanctioned targets) and, as a safety net, strays. A target written *outside*
  `~/.cache/ipe/` is invisible to the normal prune, so it grows unbounded and can
  fill the disk to 100 %, which crashes the loop mid-run. In an emergency you
  prune ONE place: `rm -rf ~/.cache/ipe/*` (rebuilds warm).
- **Enforced in `autopilot.sh`**: `IPE_CACHE=~/.cache/ipe`; `reclaim_disk` keeps
  the gate + oracle + warm `lane-*` targets there and reaps the rest, AND sweeps
  stray cargo targets found under `~/.cache/*target*` or `/tmp` (pgrep-guarded) so
  a boundary violation can never fill the disk again. Every dispatched-agent brief
  MUST set `CARGO_TARGET_DIR` (and `SKY_ORACLE_SHARED_TARGET`) under `~/.cache/ipe/`.

**Pre-build disk check — run BEFORE any full build/test suite/example
sweep.** Check free space (`df -h /`); if low (rule of thumb: <~15-20 GB
free, else the run rebuilds a freshly-cleaned go-build cache), reclaim
BEFORE starting: `go clean -cache`, `rm -rf "$CARGO_TARGET_DIR"` (or
`~/.cache/sky-rust-target`), prune example artifacts
(`sky-out`/`.skycache`/`.skydeps`/`target`). A long build on a near-full
disk dies mid-run with `resource exhausted (No space left on device)`
AFTER type-check+codegen succeed — surfaces as a *file-copy/install/
"build failed"* error and **masquerades as a build/codegen regression**,
wasting the whole run (`cabal test` ≈40 min) on mis-diagnosis. Learned
2026-06-22: clean type change looked like a 26-example sweep failure
until build log showed ENOSPC at runtime copy step, not codegen. Always
read the actual build log before blaming a code change; check `df`
before the build.

Go toolchain on macOS does NOT auto-prune its build cache — in one
session, `~/Library/Caches/go-build` grew to 202 GB, pushed a 927 GB disk
to 100% full, blocking every subsequent build/test/agent task.

End-of-mission checklist (run BEFORE declaring a release shipped when a
sweep has run):

```bash
# 1. Worktrees from finished agents — wipe the directories
#    after the cherry-pick is on main. Each carries a full
#    .skycache + sky-out ≈ 1.5 GB.
for wt in $(ls .claude/worktrees/ 2>/dev/null); do
    # Skip the one currently running an agent; check via TaskList
    # before bulk-removing.
    : keep-if-active
done
rm -rf .claude/worktrees/agent-<sha-of-completed-agent>

# 2. Tell git about it
git worktree prune --verbose

# 3. Go build cache — safe; rebuilds on next `go build`. Reclaims
#    multi-GB after a sweep; multi-tens-of-GB after multiple
#    sweeps.
go clean -cache

# 4. /tmp leftovers — sweep logs + deploy artifacts.
rm -f /tmp/sky-build-*.log /tmp/cabal-*.log /tmp/skydeploy-cp-linux /tmp/skydeploy-*.log

# 5. Sanity check
df -h /
```

NOT to do without explicit user ask:

- `go clean -modcache` (`~/go/pkg/mod`) — deletes ~50-70 GB but every
  project re-downloads modules next build (slow + needs network).
- `rm -rf dist-newstyle/` — cabal full rebuild ≈5 min.
- Wiping `.skycache/ffi/` in `examples/13-skyshop/` — 15+ min Stripe SDK
  introspection on next sweep.

**Automatic hygiene** (2026-06-03 PR13). `scripts/build.sh` AND
`scripts/example-sweep.sh` end with a 5-GB-threshold check on
`~/Library/Caches/go-build`; over-threshold auto-triggers `go clean
-cache`. Cache caps at ~5 GB after any rebuild/sweep; manual hygiene no
longer required for normal workflows. Recipe above still the escape
hatch for aggressive reclaim (e.g. before spawning many agents). Worktree
dir cleanup after EVERY agent cherry-pick remains manual.

Host <5 GB free → ABORT next agent spawn until cleanup completes — ENOSPC
mid-build leaves half-written artifacts harder to recover than a clean
build.

### 7. Core principles

1. **If it compiles, it works.** Every known runtime panic class has a
   regression test in `runtime-go/rt/*_test.go` or `test/Sky/**Spec.hs`.
   Defence in depth (panic recovery + `Err`-return at Task boundaries) is
   the floor, not the foundation.
2. **Dev experience first.** Clear errors, predictable behaviour, no
   user-written FFI.
3. **Root-cause fixes only.** Never suppress type errors/warnings. A
   defensive cover-up hiding a contract violation IS a violation.
4. **Production-grade architecture.** Scales to the Stripe SDK (76k FFI
   symbols). Stays maintainable.
5. **AI-written Sky code defaults to Std.Ui + Std.Auth + Std.Db.** Each
   reviewed for security+scalability — UI/UX/DX/security not afterthoughts.

### 8. Non-regression rules (enforced by `cabal test`)

- **No `Result String a` / `Task String a`** in public surfaces.
  Use `Result Error a` / `Task Error a`.
- **No `Std.IoError`, no `RemoteData`** — both deleted pre-v1.
- **No runtime panic from well-typed Sky code.**
- **No silent numeric coercion** — `AsIntChecked` is the fallible
  variant; `OrZero` suffix marks display-only lenient helpers.
- **No raw `.(T)` assertions on any-typed thunks** — route via
  `rt.Coerce[T]`.
- **Record field enumeration sorts by `_fieldIndex`** before any
  emission that depends on field order.
- **Secrets are typed** — `Auth.signToken` / `verifyToken` take
  `String`, not `any`. `fmt.Sprintf("%v", secret)` is forbidden.
- **`sky check` ≡ `sky build`** — both invoke `go build` on the
  emitted Go.
- **New AST nodes require explicit walker arms** in
  `Canonicalise/{Expression,Pattern,Type}.hs`,
  `Type/Constrain/{Expression,Pattern}.hs`,
  `Type/Exhaustiveness.hs`, `Format/Format.hs`, `Build/Compile.hs`,
  and the LSP's `exprTokens` / `exprIdents` / `exprAllRefs` /
  `refsInExpr` / `collectSemTokens` / `collectReferences`. Don't
  rely on `_ -> []` catchalls.

### 9. Testing rules

- **Every new feature / bug becomes a regression test** before the
  fix lands. The failing test is the discovery artefact.
- **Cabal specs** for compile-time behaviour;
  `runtime-go/rt/*_test.go` for runtime helpers;
  `tests/**/*Test.sky` for stdlib semantics; `sky test <file>` is
  the user-facing runner.
- **Runtime verification on every push.** `sky verify` builds AND
  runs each example; `scripts/verify-all-web.sh` drives the
  Sky.Live + Sky.Http.Server scenarios through Playwright;
  `scripts/verify-cli.sh` covers CLI / Sky.Cli / Sky.Tui apps.
  `--build-only` doesn't catch the "click is a no-op" class of
  regression.


### Release checklist (non-negotiable)

1. Rebuild: `cabal install --overwrite-policy=always --installdir=./sky-out --install-method=copy exe:sky`
2. Smoke-test: `sky-out/sky --version` (must print version, not start a server)
3. Cabal test sweep: `cabal test` — zero failures, pending count matches prior
4. Clean-build every example: loop over `examples/*/`, `rm -rf sky-out .skycache .skydeps`, `sky build src/Main.sky`
5. **Sky.Live + Sky.Http.Server runtime verify** — `scripts/verify-all-web.sh` (Playwright + the structural-events + round-trip-dispatch checks)
6. **CLI / Tui / Cli runtime verify** — `scripts/verify-cli.sh`
7. **`sky check`** on the largest example: `cd examples/12-skyvote && sky check`
8. **From-scratch flow** — `sky init mytest` in a temp dir, `sky build && sky run`, `sky add fmt`, `sky remove fmt`, `sky upgrade`
9. **CI parity** — `.github/workflows/ci.yml` matches the local verify scripts

Step 5/6 failure → fix root cause, re-run from step 1. Never tag with a
known runtime failure.


## Workflow rules

- **Always run mem-guard.** See "Non-negotiables" above.
- **Always clean up background tasks** before declaring "done".
- **`sky fmt` after editing `.sky`/`.skyi` files.** Two passes
  byte-identical (formatter idempotent).
- **`-f` flag with `rm`/`cp`** to avoid interactive prompts.
- **Never add co-author wording** to commits.
- **Never tag a release** without explicit user ask.
- **Never run `sky build` from repo root** — overwrites compiler binary
  in `sky-out/`.
- **Cancel in-progress CI runs on `main` before pushing** (newer commit
  supersedes them; never cancel release/tag runs).

```bash
gh run list --branch main --status in_progress --workflow CI --json databaseId --jq '.[].databaseId' \
    | xargs -I{} gh run cancel {} 2>/dev/null
git push origin main
```


## Template sync (non-negotiable)

When stdlib, syntax, Sky.Live APIs, or CLI commands change,
**`templates/CLAUDE.md`** + **matching `docs/*` files** MUST update in
same commit. AI assistants use these to write Sky code in user projects.

| Concern | User doc |
|---|---|
| Stdlib reference | `docs/stdlib.md` |
| `Std.Auth` | `docs/skyauth/overview.md` |
| `Std.Db` | `docs/skydb/overview.md` |
| Sky.Live runtime | `docs/skylive/overview.md` + `docs/skylive/architecture.md` |
| `Std.Ui` | `docs/skyui/overview.md` |
| Sky.Tui | `docs/skytui/overview.md` |
| CLI commands | `docs/tooling/cli.md` |
| LSP capabilities | `docs/tooling/lsp.md` |
| `sky.toml` schema | `docs/sky-toml.md` |
| Brand-new module | "What's in the box" in `README.md` |


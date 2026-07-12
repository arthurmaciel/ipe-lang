# CLAUDE.md

> Sky = Elm-family functional lang → typed Go, via Haskell compiler (GHC 9.4.8).
> Branch: **v0.16.6 RC**. Bundled console reads `Hub_currentIdentity`; runtime
> tenant-prefix SQL enforcement closes multi-tenant defense-in-depth loop;
> two-app hub demo at `examples/39-hub-demo`. Baseline (v0.15, carries forward):
> type-directed lowering, Go generics on parametric record aliases, same-module
> polymorphic re-instantiation, wildcard-`any` soundness gate. Source of truth:
> verification sweep (39 examples + Sky.Test assertions + 410+ cabal specs) —
> green-everywhere is a hard release gate.

## Current state (v0.16.6)

| Surface | Status |
|---|---|
| Type-directed lowering + Go generics on parametric record aliases | ✅ shipped baseline (v0.15) — `Compile.hs` `globalRegionTypes` + `LowerCtx` |
| Same-module polymorphic re-instantiation + wildcard-`any` soundness gate | ✅ shipped baseline (v0.15) |
| Layer 3 stdlib (every kernel module surfaced as Sky source) | ✅ shipped — `sky-stdlib/{Sky/Core,Std,Sky/Http}/*.sky` |
| `Ffi.kernel` mechanism + auto-TCO | ✅ shipped |
| `sky doc` (terminal + HTTP server) / `sky watch` / `sky doctor` / `sky console` | ✅ shipped |
| Sky Console embedded mode + sub-app mount + observability federation | ✅ shipped — v0.16.0 inline; v0.16.1 isolated SSE + HubExporter |
| `sky console-serve` hub (OTLP receivers + SQLite hot store) | ✅ shipped — v0.16.4 |
| Hub UI — multi-service dashboard, drill-down tabs, SSE updates | ✅ shipped — v0.16.4-5 (`runtime-go/rt/console_app/main.go` regenerated from `sky-bundled/console/src/`) |
| `Hub_currentIdentity` kernel + Sky.Live session identity persistence (gob round-trip) | ✅ shipped — v0.16.5 |
| Runtime tenant-prefix SQL enforcement (`HubStoreReaderWithTenant`) | ✅ shipped — v0.16.6 |
| Sky.Webview v0.1 (desktop, macOS) | ✅ shipped — `runtime-go/rt/webview.go`, `sky-stdlib/Std/Webview.sky` |
| 39-example sweep + 410+ cabal specs | ✅ green |

## When users ask for an app — the architecture decision matrix

**Claude is the front line for "build me X in Sky."** Before writing more
than a one-file PoC, align with user on the six decisions below.
Production-grade code does not survive guesswork.

### The six decisions to confirm

1. **App shape** — match the matrix. Sky.Live=web UI, Sky.Http.Server=headless
   API, Sky.Cli=one-shot/cron, Sky.Tui=terminal UI, Sky.Webview=desktop.
2. **Persistence** — SQLite (single-file, embeds) / PostgreSQL (Cloud SQL) /
   Firestore / Redis / none.
3. **Auth** — none / `Std.Auth` (cookies+JWT, you own users) / OAuth
   (Google/GitHub via Go SDK) / external (Auth0/Clerk/Cognito).
4. **Sky.Live session store** — memory (dev only) / sqlite / redis / postgres
   / firestore. Required even when user picks a different primary DB.
5. **Deployment target** — local binary / Docker / Cloud Run via SkyDeploy /
   Kubernetes / VM under systemd.
6. **Observability scope** — local logs only / per-app embedded console /
   push to central `sky console-serve` hub / OTel collector (Honeycomb /
   Tempo / Datadog).

### App shape matrix

| User wants…                              | Use                | Entry point shape                  | Notes |
|------------------------------------------|--------------------|------------------------------------|-------|
| Web app (forms, real-time, UI state)     | **Sky.Live**       | `Std.Live.app cfg`                 | HTTP-first; SSE patches; sessions + cookies + routing built in. |
| HTTP / JSON API (no browser UI)          | **Sky.Http.Server**| `Server.listen 8000 [...]`         | Routes + middleware (CORS / rate-limit / logging / basic-auth). |
| Multi-tenant SaaS / dashboard            | **Sky.Live + auth-app gate** | `Live.app { consoleAuth = … }` | Pair with `sky console-serve` hub for shared telemetry; tenant scope enforced at SQL layer (v0.16.6). |
| Background job / cron worker             | **Sky.Cli**        | `main = Task.run scheduledWork`    | No UI loop; `Task.parallel` for fan-out. |
| Terminal UI (TUI)                        | **Sky.Tui**        | `Std.Tui.app cfg`                  | Same view code as Sky.Live. |
| One-shot CLI tool                        | **Sky.Cli**        | `main = Task.run cliCmd`           | Argparse via `System.args`. |
| Desktop app                              | **Sky.Webview**    | `Std.Webview.app cfg`              | macOS in v0.1; Linux / Windows in v0.2. |
| WebSocket-driven feed                    | **Sky.Http.Server.WebSocket** | `Server.upgrade req` | Bidirectional; `nhooyr.io/websocket`. |
| Server-sent stream (LLM tokens, SSE)     | **Sky.Http.Server.Stream** | `Server.Stream.emit` | Mirror of `Sky.Core.Http.Stream`. |

### Pinned defaults (always apply unless the user overrules)

| Concern              | Default                                                          |
|----------------------|------------------------------------------------------------------|
| View layer           | `Std.Ui` (typed no-CSS DSL).  `Std.Html` only for wrapping raw markup. |
| Auth                 | `Std.Auth` — bcrypt + HS256 JWT cookies.  Never `fmt.Sprintf("%v", secret)`. |
| Forms with passwords | `Ui.form [Ui.onSubmit DoSignIn]` with typed record arg.  Never per-keystroke `onInput` on password. |
| DB                   | `Std.Db` + SQLite for prototypes; PostgreSQL for multi-instance deploys. |
| Money / decimals     | `Std.Money` on `Std.Decimal`.  Never raw `Float` for currency. |
| Concurrency          | `Cmd.batch` / `Task.parallel`.  In-process pub/sub via `Cmd.publish` + `Sub.subscribeTopic`. |
| Observability        | `Std.Log` structured logs; `/_sky/console` auto-mounted; `OTEL_EXPORTER_OTLP_ENDPOINT` for external collector. |
| Errors               | `Result Error a` / `Task Error a`.  Never `String` as error type. |
| No raw HTML / JS     | `Std.Ui` HTML-escapes everything.  `data-sky-eval` forbidden. |

### `sky.toml` shape per decision

```toml
name = "<project>"
version = "0.1.0"
entry = "src/Main.sky"

[live]                          # Sky.Live apps only
port = 8000
store = "sqlite"                # memory / sqlite / redis / postgres / firestore
storePath = "sessions.db"
ttl = "30m"

[database]                      # persistence != none
driver = "sqlite"               # sqlite / postgres
url = "DATABASE_URL"

[auth]                          # auth != none
cookie = "sky_sid"
ttl = "24h"
# secret comes from SKY_AUTH_TOKEN_SECRET — never commit it

[log]
format = "json"
level  = "info"
```

### Production gate — surface to the user

Sky locks down dev console + banner + metrics endpoint when `ENV` is
anything other than unset/`dev`/`development`/`local`. When user mentions
"deploy"/"production"/"Cloud Run"/"Kubernetes":

* Confirm `ENV=production` will be set on runtime.
* Confirm `SKY_AUTH_TOKEN_SECRET` ≥32 bytes.
* Confirm `SKY_CONSOLE_AUTH` set (`token` or `app`) — production + unset
  emits warn log, refuses to mount `/_sky/console`.
* Confirm session store NOT memory when >1 replica.

### When in doubt — one focused question

Ask one focused question per ambiguity, don't guess heroically.
Production-grade = survives restart, scales horizontally without losing
state, refuses cross-tenant reads (v0.16.6 SQL-WHERE gate), no permanent
error banner on transient failures, structured logs every operator can
trace. Achievable with stdlib defaults — but only if the six questions
got asked first.

**Examples (32 total — `examples/00`-`examples/31`).** Each builds clean
from wiped slate (`rm -rf sky-out .skycache .skydeps && sky build`).
`examples/00-standard-libs`=stdlib smoke test (120 assertions).
`examples/13-skyshop`=Stripe-SDK-scale benchmark (76k FFI symbols).
`examples/26-ui-showcase`=every Std.Ui layout primitive (visual-regression).
`examples/30-sse-server-demo`=`Sky.Http.Server.Stream`.
`examples/31-webview-stopwatch-ui`=Sky.Webview (macOS).
Categories: CLI(8), Sky.Tui(5), Sky.Live+Sky.Http.Server(13), GUI(2 —
Fyne+Sky.Webview), build-only fixtures(2), Sky.Webview WebGL2 spike(1).

## Non-negotiables

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

## Effect boundary — Task-everywhere (v0.10.0+)

Single rule: **every observable side effect returns `Task Error a`.**

| Tier | Type | Examples |
|---|---|---|
| Pure | bare `a` | `String.length`, `List.map`, `Crypto.sha256`, `Encoding.base64Encode`, `Time.timeString`, `System.getenvOr` |
| Fallible-pure | `Result e a` / `Maybe a` | `String.toInt`, JSON decoders, `Encoding.base64Decode`, `Auth.hashPassword` |
| Effects | `Task Error a` | `File.*`, `Http.*`, `Process.run`, `Io.*`, `Db.*`, `Auth.{register, login, setRole}`, `Crypto.{randomBytes, randomToken}`, `Time.{sleep, now, unixMillis}`, `Random.*`, `Log.*`, `System.*` (except `getenvOr`) |
| Diverging | `Int -> a` | `System.exit` (polymorphic return — never comes back) |

**Default-supplied helpers stay bare** — `System.getenvOr key def : String`,
`Maybe.withDefault`, `Result.withDefault`, `Db.getString`/`getInt`/`getBool`:
default plugs the failure case at call site.

**Auto-force `let _ = TaskExpr`.** The lowerer wraps the discarded
expression in `rt.AnyTaskRun` so the side effect fires:

```elm
let
    _ = println "step 1"             -- auto-forced
    _ = Log.infoWith "saving" [...]  -- auto-forced
in
    continue
```

Top-level module bindings of Task-typed values still require explicit
`Task.run`:

```elm
apiKey =
    System.getenv "OPENAI_KEY" |> Task.run |> Result.withDefault ""
```

**Result/Task bridges:**

| Helper | Type |
|---|---|
| `Task.fromResult` | `Result e a -> Task e a` |
| `Task.andThenResult` | `(a -> Result e b) -> Task e a -> Task e b` |
| `Result.andThenTask` | `(a -> Task e b) -> Result e a -> Task e b` |
| `Task.mapError` | `(e -> e2) -> Task e a -> Task e2 a` |
| `Task.onError` | `(e -> Task e2 a) -> Task e a -> Task e2 a` |

No `Result.fromTask` by design — keep effectful pipelines in Task; runtime
entry boundary (CLI `main`, `Cmd.perform`, HTTP handler return) executes
them.

**Two-level error pattern** (`07-todo-cli` + `18-job-queue`):

1. `errId = Crypto.randomToken 4` — short correlation ID
2. `Log.errorWith op [ "errId", errId, "error", Error.toString e ]` — server-side structured log
3. `Task.fail (Error.unexpected ("Operation failed (ref " ++ errId ++ ")"))` — user-facing message

Per app shape: CLI → `Task.run … |> Task.onError reportError`;
Sky.Http.Server → `Task.onError` recovers to a 4xx/5xx Response;
Sky.Live → `Cmd.perform task ResultMsg`, dispatch updates
`notification` / `historyError` in Model.

## Type-directed lowering (v0.15.x)

Compiler propagates HM types through to Go IR. Sub-expressions at lambda
bodies, record-field inits, list elements, call args lower with slot's
typed Go form. Closes the parametric-record-alias bug class (Surfaces
1/2/3 shipped — full write-up: `docs/v1-rfc/type-soundness-deep-analysis.md`).

### Mechanics

- **Solver writes per-region types.** `Sky.Type.Solve` carries a
  `RegionTypes :: Map A.Region T.Type` IORef alongside existing state.
  Post-unification, every constrained region has a concrete type readable
  from `globalRegionTypes` during lowering.
- **`LowerCtx`** carries optional "expected type for this position" down
  through `exprToGoExpectGo`. Known typed shape (record-field, call-arg,
  list-element) → child expr sees it, lowers with that shape.
- **`coerceToFieldType`** elides redundant `rt.Coerce` wraps when emitted
  Go expr's static type already matches target (Stage D — saves runtime
  work + codegen noise).

### Go generics on parametric record aliases

`type alias Cfg msg = { onSubmit : msg, label : String, ... }` emits:

```go
type Cfg_R[T1 any] struct {
    OnSubmit T1
    Label    string
    ...
}
```

Per-instance instantiations carry type args explicitly: `Cfg_R[Msg]`,
`Cfg_R[Int]`, etc. Callback fields keep typed callee param (no
`func(any) any` fallback); cross-alias passing works without alias-chain
workaround.

Subset-record cases (function uses only some fields) synthesise
`_skysynth_<alias>_<var>` TVars so alias's missing params still flow as
Go T-vars through inferred sig.

### Same-module polymorphic re-instantiation

Sibling refs to **polymorphic** annotated TypedDefs in same module emit
`CForeign` + alpha-rename per call site — `f : Cfg msg -> msg` called
with `msg=Int` AND `msg=Bool` in same module both work. Non-polymorphic/
wildcard-only sigs still use `CLocal` (shared env var) — identity-based
unification on nominal aliases needs the shared path; wildcard-`any`
binding needs body↔caller UF var chain for soundness.

### Wildcard-`any` soundness gate

`Sky.Canonicalise.Type.freeTypeVars` collects EVERY type-var name
including `"any"`. `Instantiate.fromAnnotation` filters `"any"` out;
`buildEnv` gives each occurrence its own fresh UF var — load-bearing pair
for `any`'s wildcard semantics. Any new "is this annotation polymorphic?"
gate MUST check `any (/= "any") freeVars`, NOT `not (null freeVars)`.
Mis-gating → wildcard-only sigs treated as polymorphic → body↔caller UF
vars diverge under fresh-per-call-site re-instantiation → silently wrong
return types accepted.

## Go reserved-name rewriting

Sky's identifier rules are stricter than Go's (Sky bans keywords at parse
time; Go *tolerates* shadowing predeclared types like `string`/`error`).
To keep emitted Go safe from accidental-shadow gotchas, every Sky
identifier in `reservedGoNames` (`src/Sky/Build/Compile.hs:4058`) is
rewritten at codegen with trailing `_`.

```
init → init_       (Go's func init() is auto-called at package load)
string → string_   (avoid shadowing Go's predeclared type)
error → error_     (avoid shadowing Go's predeclared interface)
for → for_         (Go syntactic keyword)
true → true_       (Go predeclared constant)
```

List covers four tiers:

1. `init` — special-cased w/ code comment; load-bearing for Sky.Live +
   Sky.Webview's `init = …` TEA convention.
2. **Predeclared funcs** — `new`, `make`, `len`, `cap`, `copy`, `append`,
   `delete`, `panic`, `recover`, `print`, `println`, `clear`, `min`,
   `max`, `complex`, `imag`, `real`, `close`.
3. **Reserved keywords** — all 23 Go keywords (`for`, `case`, `type`,
   `func`, …). `if`/`else`/`nil` excluded — Sky parser rejects them as
   identifiers first.
4. **Predeclared types+constants** — `bool`, `byte`, `rune`, `string`,
   `error`, `any`, `comparable`, every `int*`/`uint*`/`float*`/`complex*`
   size, `true`, `false`, `iota`, `nil`.

**Rule for AI-written Sky code.** `init = init` is safe (LHS = record-field
key → Go field `Init`; RHS = binding ref → Go identifier `init_`). Same
for `view = view`, `update = update` — every TEA app uses this idiom, lowers
correctly.

**Special-cased outside the list.** `main` = program entry — Sky binding
`main` in `module Main exposing (main)` emits as Go's `func main()`, not
`main_`. A user-named `main` in any other module module-prefixes to
`Mod_main`, never collides.

**Module-prefix safety net.** Every top-level Sky binding becomes
`<Mod>_<name>` in Go (`Main_view`, `Std_Ui_layout`) — reserved list only
matters for locals+params within functions. Audit gate before adding any
new entry: grep `examples/*/sky-out/main.go` for the bare identifier
outside any `Mod_…` token — no hits = patch is purely future-proofing.

## Memory safety + efficiency audit (v0.15.x)

### Stdlib stack behaviour

| Tier | Functions | Status |
|---|---|---|
| O(1) pure Sky | `head`, `tail`, `cons`, `isEmpty`, `Maybe.withDefault`, `Maybe.map`/`andThen`/`isJust`/`isNothing`/`map2-5`/`andMap`, `Result.withDefault`/`map`/`andThen`/`mapError`/`map2-5`/`andMap` | Stack-safe always |
| Tail-recursive (auto-TCO) | `foldl`, `find`, `any`, `all`, `member`, `drop`, `reverseHelp`, `indexedMapHelp` | Compiles to `for { ... continue }`; constant stack |
| Non-tail-recursive (pure Sky) | `map`, `filter`, `foldr`, `length`, `concat`, `concatMap`, `take`, `append`, `range`, `zip`, `indexedMap`, `Maybe.combine`, `Result.combine` | O(N) stack — fine for typical UI lists; theoretical risk at 1M+ elements. For huge inputs prefer `foldl`-based accumulator patterns. |

**TCO mechanism.** Lowerer detects tail-position self-calls in
`Can.Case`/`Can.If`/`Can.Let` bodies via
`Sky.Build.TailCallOpt.isTailRecursive`. Tail-recursive bodies emit as
`[GoForever <stmts>]`: each tail call → `<param reassignment+coercion>;
continue`; other tail positions → `return <coerceReturnExprT goRetType
expr>`. Func-typed params skip `coerceArg` (its `eraseTypeParams` would
rewrite `T1 → any`); other params use existing coercion path.

### Runtime hot paths

- **FFI boundary** — every `Ffi.callTask` / `Ffi.callPure` is wrapped
  in `runWithRecover` (panic → `Err`).
- **`rt.SkyCall`** — reflect.MakeFunc per HOF call site, ~100 ns per
  element. Bounded.
- **`rt.AsList` / `rt.AsListT[T]`** — bounded slice cast / per-element
  coercion.
- **`rt.Coerce[T]`** — type assertion fast-path; reflect-backed
  map→struct narrowing when needed (closes the Db.query → typed-record
  panic class).
- **TEA dispatch** — `SkyTuple2` fast-path before reflect fallback
  (~40 % faster on Apple M1).

### Session store bounds

| Store | Bounded by |
|---|---|
| `memory` | `sync.Map` + TTL cleanup goroutine; user count × session size |
| `sqlite` | Disk + connection pool |
| `redis` / `postgres` | External service config |
| `firestore` | GCP quota |

### Compile-time + runtime memory protections

- `[live] maxBodyBytes` (default 5 MiB) — POST cap on
  `/_sky/event` (raise for file uploads).
- `SKY_LIVE_QUEUE_MAX` (default 50) — POST retry queue cap.
- **HM solver budget** — `SKY_SOLVER_BUDGET` (default
  `max(5,000,000, constraint_count × 200)`). Caps `solveHelp`
  invocations per `solve` call; trips with a clear error rather
  than letting unbounded heap consumption OOM the host.
- **DCE** — whole-program Sky-side dead-code elimination prunes
  unreachable defs + FFI bindings before lowering.

### Synchronous-panic gate (v0.15.43)

Every emitted `func main()` starts with `defer rt.LogPanicAndExit()`.
Deferred `recover()` catches whatever escaped the synchronous Sky path
(Sky.Cli/Sky.Tui/batch jobs — every non-server `main = Task.run …`
shape), classifies panic (DivisionByZero, TypeMismatch, CoerceFailure,
ComparisonMismatch, IndexOutOfRange, NilDereference, CompilerBug,
Unexpected), emits structured Error log line w/ 4-byte errId, exits 1 —
no raw Go stack dump. `SKY_LOG_FORMAT=json` honours JSON shape.

Reachable-from-Sky panic sites: `rt.IntDiv`/`rt.Rem`/`rt.Div`
(div-by-zero), `rt.AsInt`/`AsFloat`/`AsBool` (heterogeneous slice/
untyped FFI return), `rt.cmp`, `rt.Coerce` (3 variants),
`rt.skyCallDirect`, plus Go-runtime `index out of range`/nil-deref.
Compiler-bug-contract panic sites: `coerceInner`, `Unreachable`,
`Ffi.kernel` — surface as `CompilerBug` w/ "please report" hint. Full
audit: `docs/v0.15.x-hardening/audits/CYCLE-06-PC-panic-site-audit.md`.

Sky.Http.Server handlers already have per-request defer/recover (at
`rt.go:6863`) — emit 500 instead of crashing. `Cmd.perform` goroutine
wraps `rt.SafeGo`. Top-level recover closes remaining synchronous surface.

## Build & test

```bash
sky init [name]                    # new project
sky build src/Main.sky             # compile → sky-out/app
sky run src/Main.sky               # build + run
sky watch src/Main.sky             # file-watch rebuild + restart
sky check src/Main.sky             # type-check + go build
sky fmt src/Main.sky               # opinionated formatter
sky test tests/MyTest.sky          # Sky.Test runner
sky db status                      # Std.Db migrations: applied / pending / drift
sky db migrate                     # apply pending Std.Db migrations, then exit
sky doc Module                     # terminal docs
sky doc --serve [--port 8080]      # browsable HTTP doc server (auto-opens browser)
sky doc --tui                      # interactive terminal doc browser (Sky.Tui)
sky doc --list                     # list every documented module
sky doctor [--fix] [--verbose]     # project / environment health checks
sky console [--port 8025]          # standalone Std.Ui Sky Console
sky console --tui                  # same source, Sky.Tui backend
sky add github.com/some/package    # add Go FFI binding
sky remove <package>
sky install                        # regen missing FFI + go.mod deps
sky update                         # update deps
sky upgrade                        # self-upgrade binary
sky upgrade-claude                 # refresh ./CLAUDE.md from binary's embedded template
sky clean                          # remove sky-out/ dist/
sky lsp                            # JSON-RPC LSP server (stdio)
sky --version                      # `sky dev` on local builds; CI injects release version
```

**Never run `sky build` from repo root** — overwrites compiler binary in
`sky-out/`. Always `cd` into example dir first:

```bash
cd examples/01-hello-world && sky build src/Main.sky
```

### `sky watch` rules

- Watched scope (strict allowlist, no `.skywatchignore`): `sky.toml` +
  entry-point's directory (recursive `.sky` walk) + `tests/` if present.
  Generated dirs excluded (`sky-out/`, `.skycache/`, `.skydeps/`,
  `dist-newstyle/`, `node_modules/`, `.git/`).
- Build-error policy: failing rebuild → previously-running binary stays
  alive; next successful build kills+respawns.
- Caches: `.skycache/source.hash` (full short-circuit on unchanged
  source), `.skycache/lowered/` (per-module IR), `.skycache/ffi/*.skyi`
  (HM types — never regenerated, explicit `sky add/install` step).
  Typical warm rebuild: 1-3s.

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

## Environment variables

Configuration precedence: **process env > `.env` > `sky.toml`**.

### Sky.Live (`[live]` section)

| Env var | sky.toml key | Default |
|---|---|---|
| `SKY_LIVE_PORT` | `port` | 8000 |
| `SKY_LIVE_TTL` | `ttl` | `30m` |
| `SKY_LIVE_STORE` | `store` | `memory` (memory \| sqlite \| redis \| postgres \| firestore) |
| `SKY_LIVE_STORE_PATH` | `storePath` | `DATABASE_URL` / `REDIS_URL` fallback |
| `SKY_LIVE_STATIC_DIR` | `static` | none |
| `SKY_LIVE_INPUT` | `input` | none |
| `SKY_LIVE_MAX_BODY_BYTES` | `maxBodyBytes` | 5242880 (5 MiB) |
| `SKY_LIVE_BANNER` | — | `on` (off / 0 / false to disable) |
| `SKY_LIVE_RETRY_BASE_MS` | — | 500 |
| `SKY_LIVE_RETRY_MAX_MS` | — | 16000 |
| `SKY_LIVE_RETRY_MAX_ATTEMPTS` | — | 10 |
| `SKY_LIVE_QUEUE_MAX` | — | 50 |
| `SKY_LIVE_HELLO_TIMEOUT_MS` | — | 8000 |
| `SKY_LIVE_HEARTBEAT_TTL_MS` | — | 35000 |
| `SKY_LIVE_SSE_BUFFER` | — | 16 (clamped to [1, 1024]; drops surfaced as `sky_live_sse_drops_total{session}`) |
| `SKY_LIVE_BASE_PATH` | — | (set by `MountSubApp`) |

### Logging (`[log]` section)

| Env | sky.toml | Values |
|---|---|---|
| `SKY_LOG_FORMAT` | `format` | `plain` (default) \| `json` |
| `SKY_LOG_LEVEL` | `level` | `debug` \| `info` (default) \| `warn` \| `error` |

### Auth, Console, Production gate

```dotenv
ENV=production              # gates dev console+banner OFF + /_sky/metrics behind auth
SKY_AUTH_TOKEN_SECRET=…     # ≥32 bytes — Sky errors at startup if shorter
SKY_AUTH_TOKEN_TTL=24h
SKY_AUTH_COOKIE=sky_sid

SKY_CONSOLE_EMBED=on        # off/0/false suppresses dev console mount
SKY_DEV_BANNER=on           # off suppresses floating link (keeps mount)
SKY_CONSOLE_URL=/_sky/console
SKY_SUBAPP_VERBOSE=0        # 1 forwards spawned-child stdout/stderr to parent terminal
SKY_BIN=…                   # override `sky` binary path for SpawnSkyConsole
SKY_ADMIN_TOKEN=…           # /_sky/metrics + /_sky/console require Bearer in production
                            # (legacy: SKY_METRICS_TOKEN / SKY_CONSOLE_TOKEN_SECRET honoured)
SKY_CONSOLE_AUTH=token      # v0.16.0+ three-mode console auth gate:
                            #   token → __Host-sky_console cookie + login POST form
                            #           (HKDF-derived signing key from SKY_CONSOLE_TOKEN)
                            #   app   → row-poly consoleAuth callback on Live.app cfg
                            #           (Request -> Task Error (Maybe Identity))
                            #   off   → console doesn't mount at all
                            # Production (ENV != dev/development/local) AND unset →
                            # mount declines, emits `console.disabled reason=auth-unset`
                            # warn log. Dev mode + unset → preserves v0.15.x open-in-dev.
SKY_CONSOLE_TOKEN=…         # v0.16.0+ secret deriving __Host- cookie HMAC key via
                            # HKDF-SHA256(secret, build-commit, "sky-console-cookie").
                            # Token-mode login form accepts THIS value verbatim.
SKY_CONSOLE_EMBED_ORIGIN=…  # v0.16.0+ opt-in for URL handshake (?token=<JWT> → session
                            # cookie). Must = EXACT origin of embedding iframe (SkyDeploy
                            # control-plane). Unset → handshake disabled entirely. Closes
                            # cookie/JWT confusion attack surface from security review.
SKY_CONSOLE_DB_PATH=…       # when set, telemetry dual-writes every log/metric/span to
                            # SQLite file at this path (WAL mode, 24h log/span retention,
                            # 7d metric retention). SkyDeploy injects `/data/console.db`
                            # on Pro+ tenants so bundled console mini-app renders history
                            # beyond 10k-line/1k-span in-RAM caps. Unset → pure in-RAM.

# v0.16.1+ — HubExporter (in-process OTLP push to a remote console hub)
SKY_CONSOLE_HUB=…           # https://… OTLP endpoint. Unset → exporter off.
SKY_CONSOLE_HUB_TOKEN=…     # ≥32-byte bearer. Refuses to start if shorter.
SKY_CONSOLE_BATCH_INTERVAL_MS=2000  # 2s on VMs; 200ms in serverless.
SKY_CONSOLE_SPOOL_MODE=auto # auto | file | memory. Auto-detects via
                            # K_SERVICE / AWS_LAMBDA_FUNCTION_NAME → memory.
SKY_CONSOLE_SPOOL_PATH=…    # file mode. Default: /var/lib/sky/console-spool.db (linux) /
                            # ~/Library/Application Support/sky/… (macOS)
SKY_CONSOLE_SPOOL_RETENTION=168h    # delete rows older than this
SKY_CONSOLE_SPOOL_MAX_BYTES=104857600  # 100 MB hard cap; oldest evicted
```

Production gate = `ENV` then `SKY_ENV` fallback. Unset or `dev`/
`development`/`local` → dev mode. Anything else (`production`, `prod`,
`staging`, `qa`, `preview`, …) → production mode (console+banner gone,
metrics auth on). Same gate governs all three — no dev-surface leak.

### Env prefix (multi-tenant)

```toml
[env]
prefix = "MYAPP"            # internal SKY_*_ vars become MYAPP_*_
```

Only Sky's internal namespace affected. User code calling
`System.getenv "DATABASE_URL"` reads the raw name. `System.setenv name
value : Task Error ()` / `System.unsetenv` = runtime escape hatch.

### Compiler internals (build-time only)

`SKY_DCE=0` disables DCE. `SKY_SOLVER_BUDGET=N` overrides HM solver step
cap (0=disable; default = constraint-count × factor).
`SKY_SOLVER_BUDGET_FACTOR=K` overrides multiplier (default 200).

## Standard library — Layer 3 (every kernel module is Sky source)

Source: `sky-stdlib/{Sky/Core,Std,Sky/Http}/*.sky`.

Each binding is either:
1. **Pure Sky** — recursive/case-based impl (lists, Maybes, Results).
2. **`Ffi.kernel "Name"` alias** — Sky-source decl w/ HM sig; compiler's
   Stage-4 rewrite routes call sites directly to existing typed kernel
   dispatch (no runtime overhead; `sky doc` still surfaces the entry).

### Pure (no I/O, no Task wrap)

| Module | Path | Key functions |
|---|---|---|
| `Basics` | `Sky.Core.Basics` (autoloaded via `Sky.Core.Prelude`) | identity, always, not, toString, modBy, clamp, fst, snd, compare, negate, abs, sqrt, min, max |
| `String` | `Sky.Core.String` | 38 entries — length, reverse, append, split, join, contains/containsIn, startsWith/startsWithIn, endsWith/endsWithIn (haystack-first In-suffixed, v0.15.47), toInt, fromInt, toFloat, fromFloat, toUpper, toLower, trim/trimStart/trimEnd, replace, slice, dropLeft, dropRight (v0.16.31, Elm-shaped rune-based), isEmpty, fromChar, toList, fromList, repeat, padLeft, padRight, casefold, equalFold, isEmail, isUrl, words, lines, concat |
| `List` | `Sky.Core.List` | map, filter, foldl, foldr, length, head, tail, take, drop, append, concat, concatMap, reverse, member, any, all, range, zip, find, isEmpty, indexedMap, cons + reverseHelp/indexedMapHelp |
| `Dict` | `Sky.Core.Dict` (kernel) | empty, insert, get, remove, member, keys, values, toList, fromList, map, foldl, union |
| `Set` | `Sky.Core.Set` (kernel) | empty, insert, remove, member, union, diff, intersect, fromList, toList, size |
| `Maybe` | `Sky.Core.Maybe` | withDefault, map, andThen, map2-5, andMap, combine, isJust, isNothing |
| `Result` | `Sky.Core.Result` | withDefault, map, andThen, mapError, map2-5, andMap, combine |
| `Math` | `Sky.Core.Math` | 36 entries — abs, min, max; sqrt, pow, cbrt, hypot; exp, exp2, log, log2, log10; floor, ceil, round, trunc; sin, cos, tan; asin, acos, atan, atan2; sinh, cosh, tanh, asinh, acosh, atanh; mod, remainder; pi, e, phi, sqrt2, inf, nan |
| `Regex` | `Sky.Core.Regex` | match, find, findAll, replace, split |
| `Char` | `Sky.Core.Char` | isAlpha, isDigit, isLower, isUpper, toUpper, toLower |
| `Path` | `Sky.Core.Path` | base, dir, ext, isAbsolute |
| `Crypto` | `Sky.Core.Crypto` | sha256, sha512, sha1, md5, hmacSha256, hmacSha512, rsaSha256Sign, rsaSha256Verify, constantTimeEqual (pure); aesGcmEncrypt/Decrypt, chacha20Encrypt/Decrypt, aesKeyFromPassword, chachaKeyFromPassword (Result Error String, symmetric encryption/AEAD); randomBytes, randomToken (Task, entropy) |
| `Bytes` | `Sky.Core.Bytes` | empty, length, isEmpty, fromString/toString (UTF-8 lossy via Maybe), fromHex/toHex, fromBase64/toBase64, append, slice |
| `Jwt` | `Sky.Core.Jwt` | encode, decode (HS256+RS256, sig+`exp`/`nbf` checked); `hs256`/`rs256` algos; `claims` builder — issuer/subject/audience/expiresAt/notBefore/issuedAt/jwtId/withClaim |
| `Encoding` | `Sky.Core.Encoding` | base64Encode/Decode, urlEncode/Decode, hexEncode/Decode |
| `JsonEnc` | `Sky.Core.Json.Encode` | string, int, float, bool, null, list (Elm-style `(a -> Value) -> List a -> Value`), object, encode |
| `JsonDec` | `Sky.Core.Json.Decode` | string/int/float/bool, decodeString, field, at, index, list, map, andThen, succeed, fail, oneOf, map2-4 |
| `JsonDecP` | `Sky.Core.Json.Decode.Pipeline` | required, optional, custom, requiredAt |
| `Uuid` | `Sky.Core.Uuid` | v4, v7 (bare zero-arg, called w/o `()`), parse |
| `Decimal` | `Std.Decimal` | Arbitrary-precision arith (shopspring/decimal). 42 entries. Banker's round, percent helpers. |
| `Money` | `Std.Money` | Currency-typed Money on Decimal + ISO 4217 enum (50+ codes+crypto). 44 entries. `allocate` (fair split), conversion rates. |

### Effects (`Task Error a`)

| Module | Path | Key functions |
|---|---|---|
| `Task` | `Sky.Core.Task` | succeed, fail, map, andThen, perform, sequence, parallel, lazy, run, fromResult, andThenResult, mapError, onError; **retryWith** + `RetryPolicy e` + `ShouldRetry e` ADT (RetryAlways \| RetryWhen (e -> Bool)). Build via linearBackoff/exponentialBackoff/defaultRetryPolicy; decorate via withJitter/withMaxAttempts/withBaseMs/withKind/withRetryOn (alias for retryOn). v0.15.50+ ShouldRetry is HM-pure (portable to Rust/WASM backends). |
| `Cmd` | `Std.Cmd` | none, batch, perform, publish (echo-by-default pub/sub from update return), publishNoEcho (opt-out echo, broker skips publisher's own subscription) |
| `Sub` | `Std.Sub` | none, every, batch, subscribeTopic (pub/sub receive) |
| `PubSub` | `Std.PubSub` | publish (Task-shaped, callable from raw `api` handlers/post-init/scheduled jobs; complements `Cmd.publish` bound to update-returns), publishNoEcho (Task-shaped no-echo, sets broker's SkipOrigin bit for v0.16+ cross-process tier propagation) |
| `Time` | `Sky.Core.Time` | now, sleep, every, unixMillis, format/formatISO8601/formatRFC3339/formatHTTP, addMillis, diffMillis, timeString |
| `Std.Time` | `Std.Time` | 32 entries. IANA zones, addMonths/Years (month-end CLAMPED), dayOfWeek (ISO Mon=1..Sun=7), weekOfYear (ISO 8601), startOfDay/Week/Month/Year, diffDays/Hours/Minutes/Seconds. v0.15.48+ adds `*Utc` infallible companions (`dayOfWeekUtc`/`startOfDayUtc`/`yearUtc`/etc — `Int -> Int` shape, plug "UTC" at call site so server-internal callers skip `Result.withDefault 0`). |
| `Random` | `Sky.Core.Random` | int, float, range, choice, shuffle, weighted (entropy-backed); seed, seededInt, seededFloat, seededChoice (deterministic splitmix64) |
| `Http` | `Sky.Core.Http` | get, post, request (custom method/headers/body/timeout via `HttpRequest`), defaultRequest/withMethod/withHeader/withTimeout/withBody builders, parseQuery; typed `HttpResponse = { status : Int, body : String, headers : Dict String String }` |
| `File` | `Sky.Core.File` | readFile, readFileLimit, readFileBytes, writeFile, append, exists, remove, mkdirAll, readDir, isDir, tempFile, tempDir, copy, rename |
| `Io` | `Sky.Core.Io` | readLine, writeStdout, writeStderr |
| `System` | `Sky.Core.System` | args, getArg, getenv, getenvOr (bare), getenvInt, getenvBool, setenv, unsetenv, cwd, loadEnv, exit |
| `Process` | `Sky.Core.Process` | run (subprocess) |
| `Db` | `Std.Db` | open, connect, close, exec, execRaw, query, insertRow, getById, updateById, deleteById, findOneByField, findManyByField, findByConditions, unsafeFindWhere, queryDecode, withTransaction, migrate (versioned forward-only schema migrations + `_sky_migrations` + checksum guard), getField, getString, getInt, getBool. **v0.16.26+ typed param binding**: `SqlValue` ADT (`SqlString`/`SqlInt`/`SqlFloat`/`SqlBool`/`SqlBytes`/`SqlDecimal`/`SqlTime`/`SqlMoney`/`SqlNull SqlValue`) — mixed-type SQL params as homogeneous `List SqlValue`, closes no-workaround gap for `INSERT … VALUES (?, ?, ?)` mixing `String + Maybe Int + Bool`. 8 `fromMaybe*` helpers for nullable columns. `SqlField` (`SetField SqlValue`/`OmitField`) + `Db.updateFields conn table whereCols setFields` for PATCH w/ column-omit; `Db.insertFields conn table fields` = INSERT counterpart, `OmitField` cols drop from SQL so DB applies DEFAULT (all-omit → `INSERT … DEFAULT VALUES`); `Db.insertFieldsReturning conn table fields projection decoder` (#586) appends `RETURNING <projection>`, decodes via `Std.Db.Decode` — picks up assigned autoincrement ids/DEFAULTs at INSERT time (SQLite ≥3.35/PostgreSQL). Money serialises lossless as `"ISO_CODE AMOUNT"` TEXT, paired w/ `Db.Decode.money` for round-trip. |
| `Auth` | `Std.Auth` | register, login, setRole (Task) + hashPassword, hashPasswordCost, verifyPassword, passwordStrength, signToken, verifyToken (Result); v0.15.48+ signTokenWithClaims/verifyTokenWithAlgorithm — typed-builder aliases over Sky.Core.Jwt for fine-grained algorithm+claims control |
| `Log` | `Std.Log` | println, debug, info, warn, error, debugWith, infoWith, warnWith, errorWith |
| `Trace` | `Std.Trace` | span, event, attr — opt-in app-level tracing spans. Tier-1 spans (HTTP/session/Msg/DB/Auth/Http/File) automatic; see `docs/observability.md` |
| `Server` | `Sky.Http.Server` | param, queryParam, header, getCookie, static (Layer 3 surface); higher-level `get/post/listen/text/json/html` stay kernel-only |
| `Stream` | `Sky.Http.Server.Stream` | stream, emit, finish, withContentType — server-side streaming HTTP responses (SSE/LLM token forwarding/chunked downloads). Mirror of `Sky.Core.Http.Stream` (reads upstream bodies as Sub events). See `docs/skylive/http-streaming.md` §"Server-side" + `examples/30-sse-server-demo`. Sync bridge: `Sky.Core.Http.Stream.forEachChunk hdl body` (v0.15.41+) drains upstream stream from inside a plain Sky.Http.Server handler goroutine — needed for relay shape (upstream chunks → `Server.Stream.emit` downstream chunk-for-chunk, no Sky.Live update loop). See §"Synchronous relay" + `examples/32-sse-relay`. |
| `Middleware` | `Sky.Http.Middleware` | withCors, withLogging, withBasicAuth, withRateLimit |
| `Head` | `Std.Live.Head` | v0.15.58+. Per-page `<head>` injection — `title`/`meta name content`/`metaProperty property content` (OG)/`link [(k, v)...]`/`canonical href`/`jsonLd body`/`themeColor color`/`rss href title`. Opt in via optional `head : Model -> List (Html msg)` field on `Live.app` cfg; runtime splices rendered list into `<head>` after baseline meta, before inline `<style>`. Absent field → byte-identical to pre-v0.15.58 output. |
| `Console` | `Std.Live.Console` | v0.16.0+. `Identity` type alias (`{ subject, email, claims : Dict String String }`) for optional row-poly `consoleAuth : Request -> Task Error (Maybe Identity)` field on `Live.app` cfg. Framework calls callback before mounting `/_sky/console` when `SKY_CONSOLE_AUTH=app`. `Nothing` → 403 + `console.auth.denied` audit log; `Just identity` → set `__Host-sky_console` cookie + allow. Same row-open pattern as v0.15.58 `head` — absent field → byte-identical to pre-v0.16.0 output. |
| `RateLimit` | `Sky.Http.RateLimit` | allow |
| `WebSocket` | `Sky.Core.WebSocket` (client) + `Sky.Http.Server.WebSocket` (server) | v0.15.46+. Bidirectional sockets — collab editor ops, multiplayer, bidirectional LLM chat, financial feeds. Client: `connect`/`connectWith`/`send`/`sendBinary`/`close`/`closeWithCode` (Task-tier) + `onOpen`/`onMessage`/`onClose`/`onError` (Sub-tier). Server: `upgrade` (returns from Sky.Http.Server handler) + `sendToClient`/`sendBinaryToClient`/`broadcast`/`closeClient`. Built on `nhooyr.io/websocket`. Default 30s heartbeat + 1 MiB max message + 64-frame read buffer. Server production gate: empty `originPatterns` returns 403 when `ENV=production`. **Stdlib typed-record convention (v0.15.46+): every typed record ships `default*` ctor + `with*` builder per field — always compose via builders so future field additions don't break call sites.** See `examples/33-websocket-echo`. |
| `Cache` | `Std.Cache` | v0.15.47+. LRU+TTL in-memory cache, `Cache k v` parametric on key+value. `CacheCfg` ships w/ `defaultCfg` + `withMaxEntries`/`withTTL`/`withMaxBytes` per v0.15.46 convention. `new`/`get`/`put`/`remove`/`clear`/`size`/`stats` (monotone hits/misses/evictions). Backed by `hashicorp/golang-lru/v2`; lazy TTL expiry (no background goroutine). |
| `Email` | `Std.Email` | v0.15.47+. Resend/SES/SendGrid/SMTP under one `EmailProvider` ADT. `EmailMessage`+`Attachment` typed records ship w/ `defaultMessage { from, to, subject }` + `with*` builders (`withCc`/`withBcc`/`withTextBody`/`withHtmlBody`/`withAttachment`/`withReplyTo`). `Email.send provider msg : Task Error String` returns provider message id. `SKY_EMAIL_DRY_RUN=1` short-circuits tests; `SKY_EMAIL_ENDPOINT_<PROVIDER>` overrides URLs for fixtures. |
| `Compression` | `Std.Compression` | v0.15.47+. `gzip`/`gunzip` (RFC 1952) + `zstdCompress`/`zstdDecompress` (RFC 8478). Operates on `String` (Bytes alias). Built on `compress/gzip` (stdlib) + `klauspost/compress/zstd`. |
| `Csv` | `Std.Csv` | v0.15.47+. `parse`/`parseWithDelimiter` (returns `Csv = { header, rows }`), `encode`/`encodeWithDelimiter` (RFC 4180 quoting), `parseStreamFromFile` for buffered large-file reading. Built on `encoding/csv` (stdlib). |
| `Config` | `Std.Config` | v0.15.47+. Typed TOML/YAML/JSON decoders mirroring `Sky.Core.Json.Decode`'s shape — same `string`/`int`/`float`/`bool`/`nullable`/`field`/`at`/`list`/`succeed`/`fail`/`map`/`andThen` combinators. `decodeToml`/`decodeYaml`/`decodeJson` + `loadFromFile` (extension dispatch). Backends: `BurntSushi/toml`+`gopkg.in/yaml.v3`+stdlib `encoding/json`. |
| `ToString` | `Sky.Core.ToString` | v0.15.48+. Naming-consistency surface: `fromInt`/`fromFloat`/`fromBool`/`fromTime` route to canonical kernels — zero overhead, exists for editor/`sky doc` discoverability. AI-written code encouraged to default to `ToString.fromInt n` over memorising per-type kernel sub-namespace. |
| `Pure` | `Sky.Core.Pure` | v0.15.50+. Uniform `() -> Task Error a` companion surface for runtime-arity-0 stdlib bindings (`uuidV4`/`uuidV7`/`timeNow`/`timeUnixMillis`/`systemArgs`/`systemCwd`/`systemLoadEnv`/`ioReadLine`/`dbConnect`). Closes Limitation #7 for new code without renaming existing surface — every `Pure.*` is tail-call alias to canonical kernel, typed `SkyTask[Error, T]` end-to-end. Existing names+shapes unchanged. |

### Diverging

`System.exit : Int -> a` — process termination, polymorphic return.

### Prelude (autoloaded via `Sky.Core.Prelude exposing (..)`)

`Result (Ok/Err)`, `Maybe (Just/Nothing)`, `identity`, `not`, `always`,
`fst`, `snd`, `clamp`, `modBy`, `errorToString`.

## Sky.Live + Sky.Http.Server

### Live.app shape

```elm
main =
    Live.app
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [ route "/" HomePage, route "/about" AboutPage ]
        , notFound = HomePage
        }
```

HTTP-first (full HTML on load, patches on events), SSE subscriptions,
session stores (memory/sqlite/redis/postgres/firestore), type-safe
events, VNode diffing.

**`init` is per-session, not per-page-reload.** First request from a
browser with no `sky_sid` cookie fires `init`. Browser reload while
session alive RESTORES Model from session store — `init` does NOT run.
Force fresh `init` (demo reset/e2e bootstrap): `Cmd.perform
(Cookie.expire "sky_sid")` then reload. If goal is "my other tab missed
an update", use `Cmd.publish` instead — reload-as-resync is a missing
broadcast, not a feature gap. Details: `docs/skylive/overview.md`
§"Session lifecycle — when `init` runs".

### init's `req` shape (v0.16.7 #417 + v0.16.8 #423)

`init` receives a `req` value carrying full request context:

| Field | Type | Source |
|---|---|---|
| `req.path` | `String` | URL path |
| `req.query` | `String` | raw `?...` (no parser yet — parse via `Sky.Core.Http.parseQuery` if needed) |
| `req.params` | `Dict String String` | matched-route `:name` segments (#417) |
| `req.method` | `String` | request method (#423) |
| `req.headers` | `Dict String String` | request headers, canonical case (#423) |
| `req.cookies` | `Dict String String` | parsed cookies (#423) |

Session bootstrap in init is now a one-line read:

```elm
init req =
    let sid = Maybe.withDefault "" (Dict.get "sky_sid" req.cookies) in
    ( { session = lookupSession sid }, Cmd.none )
```

No `Cmd.perform /api/whoami` round-trip needed for first render. Apps
ignoring `req` build byte-identical to pre-v0.16.7 shape (row-poly
extension).

### Per-page `<head>` injection (v0.15.58+)

Optional `head : Model -> List (Html msg)` field on `Live.app` cfg.
Runtime calls it once per full GET (initial load + sky-nav navigation),
splices returned list into `<head>` AFTER required `<meta charset>`/
`<meta viewport>`/`<meta sky-base>` tags, BEFORE inline `<style>` reset.
HM sig is row-open (`appExt` row var) — apps omitting the field
type-check+build unchanged, byte-identical to pre-v0.15.58 wrap.

```elm
import Std.Live.Head as Head

main =
    Live.app
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [ route "/" HomePage, route "/blog/:slug" BlogPostPage ]
        , notFound = HomePage
        , head = headFor
        }


headFor : Model -> List (Html Msg)
headFor model =
    [ Head.title (titleFor model.page)
    , Head.meta "description" (descriptionFor model.page)
    , Head.canonical (canonicalFor model.page)
    , Head.metaProperty "og:title" (titleFor model.page)
    , Head.metaProperty "og:image" "https://example.com/og.png"
    , Head.themeColor "#1a1a2e"
    , Head.rss "/rss.xml" "Site Blog"
    , Head.jsonLd (jsonLdFor model.page)  -- raw JSON string body
    ]
```

`Std.Live.Head` helpers (all return `Html msg` so they compose
into the same list):

| Helper | Emits |
|---|---|
| `title : String -> Html msg` | `<title>…</title>` |
| `meta : String -> String -> Html msg` | `<meta name="…" content="…">` |
| `metaProperty : String -> String -> Html msg` | `<meta property="…" content="…">` (Open Graph, Facebook) |
| `link : List (String, String) -> Html msg` | `<link …>` with arbitrary attr pairs |
| `canonical : String -> Html msg` | `<link rel="canonical" href="…">` |
| `jsonLd : String -> Html msg` | `<script type="application/ld+json">…</script>` (raw JSON body) |
| `themeColor : String -> Html msg` | `<meta name="theme-color" content="…">` |
| `rss : String -> String -> Html msg` | `<link rel="alternate" type="application/rss+xml" …>` |

Pair with `Std.Html.node "link" […] []` for cases helpers don't cover
(preload hints, custom favicon shapes, …).

**SSE patches scope to `<body>`** — head updates require full reload.
Matches typical case (head derived from page identity; in-app navigation
already triggers sky-nav fetch + full-body patch + history push). For UI
swapping `<head>` on every Msg: drop `head` field, emit `<title>`/`<meta>`
inside `view` via `Html.node` — diff layer patches normal DOM nodes
regardless of position.

### URL routing + history

`routes` field maps URL paths to Page values. Runtime matches incoming
URLs in declaration order, captures `:param` segments, reflect-calls Page
constructor w/ captured values (always `String`). Declaration order
matters — literals before patterns (`/apps/new` before `/apps/:slug`, or
"new" matches as a slug).

```elm
type Page
    = LoginPage
    | DashboardPage
    | NewAppPage
    | AppDetailPage String         -- :slug delivers a String
    | InsightsPage

routes =
    [ route "/" LoginPage
    , route "/auth/sign-in" LoginPage
    , route "/apps" DashboardPage
    , route "/apps/new" NewAppPage             -- literal before pattern
    , route "/apps/:slug" AppDetailPage        -- ctor: String -> Page
    , route "/insights" InsightsPage
    ]
notFound = LoginPage
```

**URL-from-Page** (address bar in step with programmatic `Navigate`
Msgs): emit sentinel `<div>` w/ `data-sky-path` on every render. Runtime
pushes/replaces history when value differs from `location.pathname` —
called from BOTH `__skyPatch` (full-body/`sky-nav` fetches) AND
`__skyApplyPatches` (SSE patches), so all in-app navigation updates URL.

```elm
import Std.Html as Html
import Std.Html.Attributes as Attr

urlSync : Model -> Element msg
urlSync model =
    Ui.html
        (Html.node "div"
            [ Attr.attribute "data-sky-path" (currentPath model) ]
            []
        )

-- Place urlSync inside the view's top-level column, next to the shell.
```

`data-sky-path` is typed (no JS-in-string, no `new Function()`, works
under strict CSP, no XSS surface). Leave element in DOM after runtime
processes it — removing it orphans its `sky-id`, next attribute patch
silently skips (patch's `querySelector('[sky-id=…]')` returns null).
Path-check (`location.pathname !== p`) keeps call idempotent.

For **link navigation**, add `sky-nav` to `<a>` — runtime intercepts
click, fetches URL w/ `X-Sky-Nav: 1`, full-body-patches, pushes history.
No app code needed.

```elm
Html.a [ Attr.href "/apps", Attr.attribute "sky-nav" "" ] [ Html.text "Dashboard" ]
```

**Back/Forward** handled by runtime: popstate listener re-fetches URL
w/ `X-Sky-Nav: 1` and patches. App code needs nothing for Back to work.

`data-sky-eval` (older, runs attribute via `new Function()`) is
CSP-incompatible (`script-src` w/o `'unsafe-eval'` blocks it) AND only
fires from `__skyPatch`, not SSE patches. Use `data-sky-path` for URL
updates; specific-purpose typed attributes for other one-off post-patch
effects.

**Auth gates around routes.** For public-vs-authenticated apps:

- Let Sky.Live route the URL to a page as usual.
- In `pageBody`/view, outer-case on `model.session`: signed-out always
  renders sign-in surface regardless of page.
- Use single `currentPath : Model -> String` (not per-page `pathForPage`)
  returning sign-in URL when `session = Nothing`, else dispatches on
  `model.page` — address bar follows what user actually sees.

```elm
currentPath : Model -> String
currentPath model =
    case model.session of
        Nothing -> "/auth/sign-in"
        Just _ ->
            case model.page of
                LoginPage          -> "/apps"            -- authed at sign-in → bounce
                DashboardPage      -> "/apps"
                NewAppPage         -> "/apps/new"
                AppDetailPage slug -> "/apps/" ++ slug
                InsightsPage       -> "/insights"
                AdminUsersPage     -> "/users"
```

**Slug ↔ subdomain convention.** Apps deployed under wildcard domain
(`*.platform.app`) → prefer slug-keyed URLs (`/apps/<slug>`) matching
subdomain (`<slug>.platform.app`) — bookmarkable, follows renames. Carry
slug on Page constructor; handlers needing numeric id resolve via
`findBySlug` helper.

### Async commands

`update msg model` returns `(Model, Cmd Msg)`. `Cmd.perform task toMsg`
runs task in goroutine; result dispatches back as Msg through SSE.

```elm
update msg model =
    case msg of
        FetchData ->
            ( { model | loading = True }
            , Cmd.perform (Http.get "/api/data") DataLoaded )
        DataLoaded result ->
            ( { model | loading = False, data = Result.withDefault "" result }
            , Cmd.none )
```

### Wire-event arg shapes

| Event | Element | Args |
|---|---|---|
| `click`, `focus`, `blur`, `mouseover`/`mouseout` | any | `[]` |
| `input`/`change` | checkbox | `[checked : Bool]` |
| `input`/`change` | radio | `[checked : Bool]` (use `onClick` per radio instead — see below) |
| `input`/`change` | number / range | `[value : Float]` |
| `input`/`change` | text / textarea / select | `[value : String]` |
| `submit` | form | `[formData]` — Dict String String OR typed record alias |
| `keydown`/`keyup`/`keypress` | any | `[key : String]` |

### Radio convention — `onClick` per label, not `onInput`

A radio's `input` event reports `checked=True` (Bool), not chosen value.
Bind fully-applied Msg per choice via `onClick`:

```elm
label [ for "role-guardian", onClick (UpdateRole "guardian") ]
    [ input [ type "radio", name "role", value "guardian", id "role-guardian" ] []
    , text "Guardian"
    ]
```

`for`/`id` pairing lets browser toggle radio natively; `onClick` carries
typed Msg.

### Forms with passwords (mandatory pattern)

**Use `onSubmit` w/ form data, NOT `onInput` per keystroke on password
fields.**

```elm
type alias AuthCreds = { email : String, password : String }
type Msg = UpdateEmail String | DoSignIn AuthCreds

view model =
    form [ onSubmit DoSignIn ]
        [ input [ type "email", name "email", value model.email, onInput UpdateEmail ] []
        , input [ type "password", name "password" ] []  -- no value, no onInput
        , button [ type "submit" ] [ text "Sign in" ]
        ]
```

Three reasons:

1. **Password managers** (1Password/Bitwarden/browser autofill) watch DOM
   mutations on password inputs. Server-driven re-render w/ `value=…`
   triggers re-prompt/re-fill cycle.
2. **Secret never lives in Model** — no `onInput UpdatePassword` Msg →
   no Model field → never serialised into Redis/Postgres/Firestore
   session stores.
3. **Race-free submit** — form submit reads live DOM value, not a
   debounced keystroke.

`DoSignIn AuthCreds` ctor takes a typed record. Wire driver decodes form
data directly into Go struct via case-insensitive `json.Unmarshal`
(`State_AuthCreds_R{Email, Password}`). No per-Msg decoder boilerplate.

### Connection status banner

Bottom-pinned, three states:

- **connected** — `display:none`.
- **reconnecting** — amber `Reconnecting…`. Shown when SSE drops or POST
  `/_sky/event` fails. 500ms grace before painting.
- **offline** — red `Connection lost — refresh to retry`. Reached after
  `SKY_LIVE_RETRY_MAX_ATTEMPTS` (default 10, ~2 min). Runtime keeps
  retrying in background so a healed proxy recovers without refresh.

POST failures while reconnecting land in `__skyEventQueue` (FIFO, capped
at `SKY_LIVE_QUEUE_MAX`), replay on SSE `hello`. Server seq ordering
tolerates late delivery.

**Reverse-proxy hardening.** Every `/_sky/sse` sends `X-Accel-Buffering:
no` + 2 KB padding + immediate `event: hello` handshake + heartbeats
every 15s. Every `/_sky/event` POST carries `X-Sky-Live: 1`. Client:
`connected` only flips on `hello` (never raw `EventSource.open`); 8s
watchdog reopens on missing hello; 35s watchdog reopens on missing
heartbeat. POST 200 OK without `X-Sky-Live: 1` treated as wedged-proxy,
rerouted.

Localise via `status = { reconnecting = "Reconnexion…", offline =
"Connexion perdue" }` on `Live.app`'s cfg record. Partial overrides fall
back to English defaults. Strings JSON-encoded, rendered via
`textContent` (never `innerHTML`).

### Input preservation across re-renders

Three failure modes closed:

1. **Empty patches** JSON-ack instead of HTML-fallback (preserves
   uncontrolled fields like password).
2. **Full-body swap** preserves EVERY uncontrolled INPUT/TEXTAREA/SELECT,
   not just `document.activeElement`.
3. **Open `<select>` defence** — `__skyApplyPatches` skips any patch
   where target is the focused select/contains it/is contained by it.
   Tick subscriptions accumulate state server-side; next user interaction
   reconciles.

### Sky.Http.Server

```elm
main =
    Server.listen 8000
        [ Server.get "/" (\_ -> Task.succeed (Server.text "Hello!"))
        , Server.get "/api/users/:id" getUser
        , Server.post "/api/data" handlePost
        , Server.static "/assets" "./public"
        ]
```

Routes: `get/post/put/delete/any` | groups w/ prefix | cookies (HttpOnly,
Secure, SameSite) | extractors: `param`, `queryParam`, `header`,
`getCookie` | responses: `text`, `json`, `html`, `withStatus`, `redirect`
| middleware: `Handler -> Handler`.

**Handler annotation (v0.16.4+).** Named handlers ascribe at head
position w/ `Handler` alias:

```elm
import Sky.Http.Server exposing (Handler)

getUser : Handler
getUser req = ...
```

`Handler` is a transparent alias for `Request -> Task Error Response`,
exported from `Sky.Http.Server`. Long-form `: Request -> Task Error
Response` still works — pick whichever reads better. Same pattern works
for any function-typed alias: `view : Renderer Msg`, `decodeUser :
Decoder User`, etc. Canonical Elm shape; head-position alias unfolding
closed by contributor PR #123.

## Sky Console + sub-app mount + observability

Every Sky.Live/Sky.Http.Server app auto-mounts a Std.Ui dev console at
`/_sky/console` in dev mode, w/ structured logging, Prometheus metrics,
distributed tracing — no separate stack to stand up.

| Surface | What it is |
|---|---|
| `🔍 Console` link | Floating bottom-right anchor injected into every dev-mode page. Same-origin link to `/_sky/console`. |
| `/_sky/console/*` | Reverse-proxied to a bundled Sky.Live mini-app spawned as a child process. |
| `/_sky/metrics` | Prometheus scrape endpoint (Bearer-gated in production). `sky_live_requests_total{route,status}`, `sky_live_request_seconds`, error counters. |
| `/_sky/healthz` · `/_sky/readyz` | Liveness + readiness probes. |
| `/_sky/buildinfo` | Commit SHA, build timestamp, Sky version. |
| `/_sky/observability/ingest` | Sub-app log/metric/span push endpoint. |
| Structured logs | Every `Log.*` carries level + message + request-correlation ID. HTTP access log automatic. |
| Trace spans | Every HTTP request opens a span; `rt.RecordTrace` adds child spans. Exported to OpenTelemetry if `OTEL_EXPORTER_OTLP_ENDPOINT` is set. |

### `rt.MountSubApp`

```go
import "your-app/rt"
rt.MountSubApp(mux, "/billing", rt.SpawnBinary("./billing-app"))
rt.MountSubApp(mux, "/admin",   rt.SpawnBinary("./admin-app"))
rt.MountSubApp(mux, "/docs",    rt.SpawnBinary("./hugo-server"))
```

Each child runs as own process — own session store, update loop,
cookies, zero shared state. Reverse proxy gives user one port + one
origin. Cost: ~5 MB RAM + ~5 ms/request hop.

### Sub-app observability federation

Each sub-app spawns `rt.PushExporter` (background goroutine) batching
logs/metrics/spans, POSTs every 2s to
`<parent>/_sky/observability/ingest` w/ namespace labelling. Single
Prometheus scrape on parent covers the tree. Auth: shared secret via
`X-Sky-Ingest-Token` (auto-generated per parent boot; constant-time
compare).

### Production gate

`productionFromEnv()` reads `ENV` then `SKY_ENV`. Unset/`dev`/
`development`/`local` → dev mode. Anything else → production
(console+banner gone, metrics auth on).

**`SKY_LIVE_BASE_PATH`** — set automatically when Sky.Live app runs as
sub-app. Causes: page wrap injects `<meta name="sky-base">` so inlined
JS prefixes `/_sky/event` etc; dev banner suppressed;
`MountObservabilityEndpoints` skipped (parent owns endpoints);
`maybeAutoMountConsole` early-returns (no recursive auto-mounts).

## Std.Ui — typed no-CSS layout DSL

Layered above `Std.Html`; renders to inline-styled HTML server-side. Pick
`row`/`column`/`el` for layout, attach typed attrs from `Background`/
`Border`/`Font`/`Region` sub-modules, never write CSS.

```elm
import Std.Ui as Ui
import Std.Ui.Background as Background
import Std.Ui.Border as Border
import Std.Ui.Font as Font

view model =
    Ui.layout []
        (Ui.row
            [ Ui.spacing 12, Ui.padding 16
            , Background.color (Ui.rgb 255 102 0)
            , Border.rounded 4
            ]
            [ Ui.button [] { onPress = Just Decrement, label = Ui.text "−" }
            , Ui.el [ Font.size 24, Font.bold ] (Ui.text (String.fromInt model.count))
            , Ui.button [] { onPress = Just Increment, label = Ui.text "+" }
            ])
```

### Three idioms AI tooling MUST get right

1. **Forms with sensitive inputs use `Ui.form` + `onSubmit DoSignIn`, NOT
   `onInput` per keystroke on password fields.** See password pattern in
   Sky.Live section above.

2. **Real `<input>` elements use `Ui.input`, NOT `Ui.el [htmlAttribute
   "type" "text"]`.** `Ui.el` builds a Node rendering as `<div>` —
   browsers ignore `type=`/`value=` on non-inputs.

3. **Std.Ui-heavy modules (~25+ polymorphic `Element Msg` helpers) MUST
   be split across multiple modules.** Monolithic `Main.sky` can blow
   the HM type-checker heap (Limitation #17). Canonical split:
   `State.sky` (types, no Std.Ui imports) / `Update.sky` /
   `View/Common.sky` / one View module per page / `Main.sky` dispatcher.
   See `examples/19-skyforum`'s 8-module form.

4. **`Input.*` size/layout attrs apply to wrapper, form attrs stay on
   inner control.** Every `Std.Ui.Input.*` call (text/multiline/email/
   username/search/currentPassword/newPassword/slider/checkbox/radio/
   radioRow) routes layout attrs (`Ui.width`/`Ui.height`/`Ui.padding`/
   `Ui.spacing`/`Ui.alignX`/`Ui.alignY`/`Ui.nearby`/`Ui.pointer`/
   `Ui.overflow`) to outer wrapper `wrapWithLabel` emits, while form/
   event/visual attrs stay on inner `<input>`/`<textarea>`. So
   `Input.multiline [Ui.height Ui.fill] {...}` inside a column-fill
   parent fills the parent; `Background.color (Ui.rgb 240 240 240)`
   colours the textarea itself, not wrapper.

### `Ui.fill` emission (v0.15.55+, refined v0.15.56)

`Ui.fill` lowers asymmetrically per parent's flex direction:

| Position | CSS emitted |
|---|---|
| Main-axis fill | `flex-grow: N; min-{w,h}: 0;` |
| Cross-axis HEIGHT fill (row child) | nothing — relies on flex default `align-items: stretch` |
| Cross-axis WIDTH fill (column / el / textColumn child) | `width: 100%;` |

Asymmetry closes a real bug class. CSS Flexbox §9.8 resolves `%` against
parent's USED size only when "definite"; flex-grow-derived height is
indefinite. Row parents commonly have indefinite heights → pre-v0.15.55
`height: 100%` on cross-axis fill collapsed every child to text-content
height (issue #63 — three-pane app shell, Input.multiline → 22/51px).
Width keeps `100%` because column-parent widths typically definite AND
it survives `[Ui.width fill, Ui.centerX]` cascade — canonical
centred-page-content shape.

**v0.15.56 F4 `align-self` single-emission contract.** Cross-axis fill
emitters dropped redundant `align-self: stretch` declaration — `stretch`
is default `align-items` value (no-op) AND created cascade conflict w/
explicit alignment attrs (`Ui.centerX/Y`, `alignLeft/Right/Top/Bottom`).
Post-F4 invariant: at most ONE `align-self` declaration per element,
sourced from `alignSelfX/Y` only. Rendering identical to v0.15.55; code
order-independent.

### Void-element pseudo-class / animation / transition / media-
### query style hoist (v0.15.57+ — #409)

Pseudo-class rules (`Background.activeColor`/`hoverColor`/`focusColor`),
CSS transitions (`Std.Ui.Transition.attribute`), keyframe animations
(`Std.Ui.Animation.attribute`), breakpoint media queries
(`Ui.breakpoint Ui.mobile [...]`) all emit sky-id-scoped `<style>`
element to apply rule. Pre-v0.15.57 runtime prepended that `<style>` as
FIRST CHILD of carrying element — fine for `<div>`/`<button>`/etc, but
silently DROPPED on void HTML elements (`<input>`, `<img>`, `<br>`,
`<hr>`, …) because `renderVNode` skips children for void tags
(self-closing `/>` ends the element).

Post-v0.15.57: style block hoisted to SIBLING slot immediately after
void element. CSS selector still keys off void element's sky-id, rule
applies correctly. This means:

```elm
Input.text
    [ Background.color (Ui.rgb 240 240 240)
    , Background.activeColor (Ui.rgb 200 100 50)   -- now works on <input>
    , Background.hoverColor  (Ui.rgb 50 50 200)    -- @media (hover: hover) gate
    ]
    cfg
```

`<input>` inside `Input.text`'s wrapper renders w/ sibling `<style
data-sky-pc="<input-sky-id>">` carrying `:active`+`:hover` rules. No
call-site change needed for existing code — runtime fix transparent.

### `Ui.layoutWith` — wrapper customisation (v0.15.56)

```elm
Ui.layoutWith { wrapperAttrs : [Attr msg], rootAttrs : [Attr msg] } -> Element msg -> Html
```

Additive entry point. `wrapperAttrs` reach outer 100vh `<div>` page
wrapper (Background.color for page-wide dark mode, Font.color/
Font.family for document-wide typography, Border/class/aria-*/data-*
for analytics/a11y landmark routing). `rootAttrs` apply to root element
(same as `Ui.layout`'s argument).

`Ui.layout attrs el` is now `Ui.layoutWith { wrapperAttrs = [],
rootAttrs = attrs } el` — byte-identical for existing call sites. Reach
for `layoutWith` when wrapper needs visual styles (dark page, custom
font cascade, page background image).

### Surface highlights

Full reference: `docs/skyui/overview.md`.

- **Entry points**: `layout : List Attr -> Element -> Html` +
  `layoutWith : { wrapperAttrs : List Attr, rootAttrs : List Attr } -> Element -> Html`
  (v0.15.56 — page wrapper for dark mode/Font cascade/flex-direction
  override).
- **Layout**: `el`, `row`, `column`, `wrappedRow`, `grid` + `gridColumns
  N` (CSS-Grid auto-fit), `paragraph`, `textColumn`, `text`, `none`,
  `html`.
- **Sized elements**: `link { url, label }`, `image { src, description
  }`, `button { onPress, label }`, `input`, `form onSubmit`.
- **Length**: `px`, `fill`, `fillPortion Int`, `content`, `shrink`,
  `minimum Int Length`, `maximum Int Length`, `vh Int`, `vw Int`.
- **Padding**: `padding Int`, `paddingXY x y` (X-first, Y-second),
  `paddingEach { top, right, bottom, left }`, `spacing Int`.
- **Alignment**: `centerX`, `centerY`, `alignLeft`, `alignRight`,
  `alignTop`, `alignBottom`, `pointer`.
- **Overflow**: `clip`, `clipX`, `clipY`, `scrollbars`, `scrollbarX`,
  `scrollbarY`.
- **Nearby**: `above`, `below`, `onLeft`, `onRight`, `inFront`, `behind`
  (absolute-positioned overlays).
- **Events**: `onClick msg`, `onSubmit msg`, `onInput (String -> msg)`,
  `onChange`, `onFocus`, `onMouseOver/Out`, `onKeyDown`, `onFile (String
  -> msg)`, `onImage (String -> msg)`.
- **File/image upload hints**: `fileMaxSize Int` (bytes, browser-side
  cap not security), `fileMaxWidth Int`, `fileMaxHeight Int`.
- **Colour**: `rgb`, `rgba`, `white`, `black`, `transparent`.
- **Sub-modules**: `Background` (color, image, linearGradient,
  hoverColor/focusColor/focusVisibleColor/activeColor/disabledColor),
  `Border` (color, width, widthEach, rounded, solid/dashed/dotted,
  shadow, glow, innerShadow, hoverColor/focusColor/activeColor/
  hoverWidth/hoverRounded), `Font` (color, family, size, weight,
  bold/semiBold/regular/light/extraBold/black, italic, underline,
  noDecoration, letterSpacing, alignLeft/Right/Center/Justify,
  sansSerif/serif/monospace, hoverColor/focusColor/activeColor/
  disabledColor/hoverSize), `Region` (semantic landmarks routed to
  `<h1..h6>`, `<main>`, `<nav>`, `<aside>`, `<footer>`, aria-*), `Input`
  (button, text, multiline, email, username, search, currentPassword,
  newPassword, checkbox, radio, radioRow, slider), `Lazy` (LRU-cached
  subtrees, `SKY_UI_LAZY_CAP=N`), `Keyed` (sky-key for diff identity),
  `Responsive` (classifyDevice, adapt — Model-driven branching needing
  typed Msg dispatch).
- **Pseudo-classes** (`:hover`/`:focus-visible`/`:active`/`:disabled`) —
  per sub-module `on<State>` helpers above + generic `Ui.onPseudo :
  PseudoClass -> List (Attribute msg) -> Attribute msg` escape hatch for
  selector combos no sub-module covers. `PseudoClass`: `Ui.hover`,
  `Ui.focus`, `Ui.focusVisible`, `Ui.active`, `Ui.disabled`. `focusColor`
  targets `:focus-visible` (safer default — only fires on keyboard nav,
  never click-induced focus rings); use `Ui.onPseudo Ui.focus [...]` for
  sticky-focus. `:hover` rules AUTO-WRAPPED in `@media (hover: hover)`
  by runtime so no sticky-hover on touch devices (classic mobile
  "tap-and-stay-hovered" bug). Renders sky-id-scoped `<style
  data-sky-pc=...>` child via same pattern as media queries. Composes
  w/ `Ui.breakpoint` via natural nesting — breakpoint wraps element,
  pseudo-rule attaches to element inside.
- **Media queries + breakpoints** (`Ui.mediaQuery`/`Ui.breakpoint`/
  `Breakpoint` ADT) — CSS-driven viewport-conditional styling w/ instant
  CSS-engine reactivity (no JS round-trip, no Model field, no
  re-render). Typed `Breakpoint`: `Mobile`, `Tablet`, `Desktop`,
  `SmAndUp`, `MdAndUp`, `LgAndUp`, `XlAndUp` (Tailwind cuts), `DarkMode`,
  `LightMode`, `ReducedMotion`, `TouchDevice`, `Portrait`, `Landscape`,
  `Custom Int Int` (minPx maxPx; 0=unset). `Ui.mediaQuery query [attrs]
  child` = escape hatch for raw CSS media-query string. Renders wrapper
  `<div>` + sky-id-scoped `<style>` child: `<style
  data-sky-mq="<sid>">@media <q> { [sky-id="<sid>"] { <rules> } }</style>`
  — two breakpoints on same page can't cross-contaminate. Composes via
  nesting; Sky.Tui silently ignores `<style>`; Sky.Webview honours media
  queries identically to Sky.Live. Pick `Ui.breakpoint` when layout
  transition needs no typed Msg; pick `Std.Ui.Responsive` when it does.
- **Transitions + animations** (`Std.Ui.Transition`/`Std.Ui.Animation`/
  `Std.Ui.Transform`) — typed CSS transitions + keyframe animations
  declared on a Sky.Ui element. Browser handles frame timing — no JS
  round-trip, no Model field. Both rules AUTO-WRAPPED in `@media
  (prefers-reduced-motion: no-preference)` by default for a11y; opt out
  via `Transition.attributeUnsafe`/`respectReducedMotion = False` on
  Animation Spec ONLY when motion semantically required (loading
  spinner, progress indicator). `Transition.attribute [property
  "background-color", duration 200, easing easeOut]` builds CSS
  transition shorthand from typed `Step`s; pair w/ `Background.hoverColor`
  so browser animates change between base+`:hover` states.
  `Animation.attribute { name, duration, easing, delay, iterations,
  fillMode, respectReducedMotion, keyframes }` builds keyframe spec;
  `keyframes : List (Int, List Transform.Prop)` is `[(percent,
  [Transform.opacity 0.0, Transform.translateY 10]), ...]`.
  `Transform.{translateX, translateY, translate, scale, scaleXY, rotate,
  skewX, skewY, opacity}` = typed property helpers — `transform`-shaped
  ones join into ONE `transform:` shorthand per keyframe, `opacity`
  emits standalone. Two elements naming animation `"fadeIn"` w/ different
  keyframes don't collide globally — runtime auto-suffixes @keyframes
  name w/ element's sky-id-derived ident (`fadeIn__r_1_div_0`). Renders
  sky-id-scoped `<style data-sky-tr=...>` + `<style data-sky-anim=...>`
  child via same pattern as pseudo-classes/media queries.
- **Aspect ratio + grid tracks** (`Ui.aspectRatio`/`Ui.aspectRatioWH`/
  `Ui.square`/`Ui.widescreen`/`Ui.fullHd`/`Ui.cinemascope` +
  `Std.Ui.Grid.tracks`/`Grid.columns`/`Grid.rows`) — typed proportional
  sizing + explicit CSS-grid track lists. `Ui.aspectRatio 1.777`/
  `Ui.aspectRatioWH 16 9` lock element to width-to-height ratio (pair w/
  `Ui.width Ui.fill` so unset axis auto-scales). `Std.Ui.Grid` exposes
  typed `Track` ADT (`fr`, `px`, `auto`, `minContent`, `maxContent`,
  `minmax`, `repeat`, `repeatAutoFit`, `repeatAutoFill`) + attribute
  entry points; reach for it on sidebar layouts (`[fr 1, px 200, fr
  1]`), content-aware columns (`[auto, fr 1]`), or responsive card grids
  (`[repeatAutoFit (minmax (px 240) (fr 1))]`). Lighter-weight
  `Ui.gridColumns N` (auto-fill `minmax(Npx, 1fr)`) stays for common-case
  product-card grid. Both lower to inline CSS via existing AttrStyle
  channel — no runtime injection pass.

| Need | Reach for |
|---|---|
| Square avatars, 16:9 video embeds | `Ui.square` / `Ui.widescreen` / `Ui.aspectRatioWH w h` |
| Custom decimal ratio (e.g. 2.35:1 cinemascope) | `Ui.aspectRatio Float` |
| Product-card grid (all tracks same min-width) | `Ui.gridColumns N` |
| Sidebar layout / mixed track types | `Std.Ui.Grid.columns [ fr 1, px 200, fr 1 ]` |
| Responsive card grid (re-flow on resize) | `Grid.columns [ Grid.repeatAutoFit (Grid.minmax (Grid.px 240) (Grid.fr 1)) ]` |
| Both axes set explicitly | `Grid.tracks cols rows` |

```elm
-- Mobile-first: column on phones, row above 768.
Ui.breakpoint Ui.mobile
    [ Ui.htmlAttribute "style" "flex-direction: column;" ]
    (Ui.row [ Ui.spacing 16 ] [ sidebar, main ])

-- Dark-mode background, no model field required.
Ui.breakpoint Ui.darkMode
    [ Background.color (Ui.rgb 18 18 24) ]
    pageBody

-- Raw query for cases no typed Breakpoint covers.
Ui.mediaQuery "(min-resolution: 2dppx)"
    [ Background.image "hero@2x.png" ]
    hero
```

### File / image upload pattern

```elm
Ui.input
    [ Ui.htmlAttribute "type" "file"
    , Ui.htmlAttribute "accept" "image/*"
    , Ui.onImage AvatarSelected           -- AvatarSelected : String -> Msg
    , Ui.fileMaxSize   2_000_000          -- 2 MB cap (browser-side)
    , Ui.fileMaxWidth  800                -- resize before upload (JPEG @ 0.85)
    , Ui.fileMaxHeight 800
    ]
```

Callback receives data URL. Decode w/ `Std.Encoding.base64Decode` →
upload via `Http.post`. Ensure `[live] maxBodyBytes` ≥ your
`fileMaxSize`.

## Sky.Tui v1

TEA backend rendering `Std.Ui` to ANSI cells. Same
`init`/`update`/`view`/`subscriptions` shape as `Sky.Live`.

```elm
type alias Cfg model msg =
    { init          : () -> (model, Cmd msg)
    , update        : msg -> model -> (model, Cmd msg)
    , view          : model -> Element msg
    , subscriptions : model -> Sub msg
    , onKey         : KeyEvent -> msg                  -- optional
    , guard         : msg -> model -> Result Error ()  -- optional; same as Live.app's guard
    , canvasWidth   : Int                              -- default 1280 logical px
    , canvasHeight  : Int                              -- default 720
    }

main = Tui.app cfg |> Task.run
```

**Logical-pixel canvas** — `canvasWidth × canvasHeight` defines design
surface. Runtime computes `pxPerCell*` from terminal size, converts
`Ui.padding 8`/`Ui.px N` to cells. Default 1280×720 matches typical web
canvas.

**Coverage**: ~95%+ of Std.Ui primitives. Unsupported attrs (gradients,
fine letter-spacing, image fills) emit deduped `tuiWarn`;
`SKY_TUI_QUIET=1` suppresses. Wide chars (CJK+emoji+ZWJ) via
`github.com/rivo/uniseg`. Bracketed paste capped at 1 MiB. Modified
arrows (Ctrl/Shift/Alt) pass to user `onKey`.

**Reliability floor**: `safeGo` restores TTY on panic; external signals
(SIGTERM/SIGHUP/SIGQUIT/SIGINT) trapped → teardown → `exit
128+signum`; `sanitiseRune` strips control bytes from user text;
`tuiMaxContentH = 50,000` hard cap w/ 10,000 soft warn; `TERM=dumb`/
non-TTY stdin refused w/ friendly error.

**Sky.Cli password mode** — `Cli.readPassword : () -> Task Error String`
reads stdin w/ echo disabled (`golang.org/x/term`'s `ReadPassword`).
Password never echoes; never lands in scrollback.

## Sky.Webview v0.1 (desktop)

Cross-backend mirror of `Live.app`+`Tui.app` — same TEA shape, native
desktop window via system webview (WKWebView macOS, WebView2 Windows,
WebKitGTK Linux) using `webview_go`. No HTTP server, no SSE, no session
store — bridge is in-process `Bind`+`Eval`.

```elm
import Std.Webview as Webview

main =
    Webview.app
        { init = init
        , update = update
        , view = view                  -- view : Model -> Element msg
        , subscriptions = subscriptions
        , window = { title = "Sky App", size = ( 800, 600 ) }
        }
        |> Task.run
```

Reuses Sky.Live's renderer (`HtmlToVNode`, `assignSkyIDs`, `renderVNode`,
`diffTrees`) — same `view` fn paints identically across Sky.Live (web),
Sky.Tui (terminal), Sky.Webview (desktop). XSS hardening parity:
focus-preserving DOM replacer, `__skyReviveScripts` for late-injected
`<script>` tags.

`WindowCfg` is closed (`{ title : String, size : (Int, Int) }`) in v0.1
for clean missing-field type errors. v0.2 reopens it for
`alwaysOnTop`/`transparent`/`decorated` + adds tray icons, global
hotkeys, native file dialogs, Windows+Linux smoke validation. v0.1
ships macOS only.

Sky-stdlib path: `sky-stdlib/Std/Webview.sky`. Runtime:
`runtime-go/rt/webview.go` (build tag `cgo && darwin` for v0.1; widens
v0.2 w/ smoke for Linux/Windows). Stub at `webview_stub.go` covers
`!cgo || !darwin` so non-macOS builds link cleanly, surface runtime `Err
Error` on call. Example: `examples/31-webview-stopwatch-ui`.

**`sky build` cgo-detect.** Normally `sky build` runs `CGO_ENABLED=0 go
build` first (static-binary preference), retries w/ cgo only on failure.
When emitted `main.go` contains `rt.Webview_app` (project uses
Sky.Webview), build runner flips straight to `CGO_ENABLED=1` on first
attempt — else stub would compile cleanly and binary would silently
exit at runtime. Look for `(built with cgo — Sky.Webview requires it;
…)` in build log to confirm.

**Std.Ui convention** — `view` fn MUST wrap output in `Ui.layout []
(...)` to convert `Element` → `Html` before renderer (`HtmlToVNode`)
processes it. Raw `Ui.column [...]` body produces blank window. Same
convention as Sky.Live (see `examples/19-skyforum`,
`examples/26-ui-showcase`).

## Language syntax

```elm
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
import Sky.Core.Task as Task
import Std.Log exposing (println)

type Msg = Increment | Decrement

update : Msg -> Int -> Int
update msg count =
    case msg of
        Increment -> count + 1
        Decrement -> count - 1

main =
    println (String.fromInt (update Increment 0))
```

`|>` `<|` pipelines | `::` cons | `\x -> x + 1` lambdas | `let…in`
| `case…of` (exhaustiveness checked) | `{ record | field = value }`
update | `module M exposing (..)` | `import M as Alias exposing (func)`.

**Multiline strings** — triple-quoted, `{{expr}}` interpolation:

```elm
html = """<div class="card">
    <h1>{{title}}</h1>
    <p>{{description}}</p>
</div>"""
```

Single `{` is literal. Interpolation exprs can be identifiers, field
access (`{{record.field}}`), qualified names (`{{String.fromInt n}}`),
or function calls.

Escape w/ backslash: `\{{` emits literal `{{` (no interpolation). Use to
ship Mustache/Handlebars/shell-script placeholders downstream without
Sky hijacking them. `\\` collapses to single literal backslash; other
`\X` sequences preserved verbatim (regex `\d+`, paths `\test`, etc).

## Active limitations

Real current compiler limitations users must work around. v0.15 closed
several earlier-listed items; surviving list below verified against
HEAD.

1. **No higher-kinded types.** HM only.
2. **No `where` clauses.** Use `let…in`.
3. **No custom operators.**
4. **Negative literal args need parens.** `f -1` parses as subtraction.
   Use `f (-1)`.
5. **`Dict.toList` typed-key inference is inline-only.** `Dict.toList
   (Dict.fromList [(1, "a")])` chained in same expression returns real
   `Int` keys (v0.15.45 closed soundness hole for that shape). For
   let-bound intermediates — `let d = Dict.fromList […] in Dict.toList
   d` — solver doesn't expose `d`'s typed shape at use-site's region, so
   routing falls back to legacy String-key path. Workaround: inline the
   chain, or wrap result in typed accessor (`d |> Dict.toList`). v0.16+
   tracking covers let-region propagation fix.
6. **`sky check` does not fully model Go interface satisfaction.** Opaque
   FFI types unify with each other; concrete-satisfies-interface checks
   fall through.
7. **Zero-arg calls follow binding's declared type, not FFI-vs-kernel
   origin.** Bare `Uuid.v4` works because stdlib sig is `v4 : String`.
   `Time.now ()`/`Time.unixMillis ()`/`FyneApp.new ()` are *all* needed
   because their sigs are `() -> Task Error a`/`() -> any`. Calling a `:
   String` binding w/ `()` triggers known codegen bug for arity-0
   kernels (`Uuid.v4 ()` mis-applies the unit); stick to declared shape.
   Dict/Set/Maybe/Result stay bare for `empty`/`none` etc — non-function
   types too.

   **v0.15.50 mitigation — `Sky.Core.Pure`.** New code targeting uniform
   `() -> Task Error a` shape can import `Sky.Core.Pure as Pure`, call
   additive companions — `Pure.uuidV4 ()`/`Pure.uuidV7 ()`/
   `Pure.timeNow ()`/`Pure.timeUnixMillis ()`/`Pure.systemArgs ()`/
   `Pure.systemCwd ()`/`Pure.systemLoadEnv ()`/`Pure.ioReadLine ()`/
   `Pure.dbConnect ()`. Existing names+shapes unchanged. Pure.* lowers to
   canonical kernel w/ typed `SkyTask[Error, T]` shape (no `any`
   widening).
8. **Non-tail-recursive list ops are O(N) on Go stack.** `map`,
   `filter`, `foldr`, `length`, `concat`, `take`, `append`, `range`,
   `zip`, `concatMap`, `indexedMap`, `Maybe.combine`, `Result.combine`
   recurse. Tail-recursive ops (`foldl`, `find`, `any`, `all`, `member`,
   `drop`, `reverseHelp`) auto-TCO'd to constant stack. For very large
   lists (200k+ elements) prefer tail-recursive accumulator pattern.
9. **Zero-arg `Css.*` keyword constants require `()`** — `Css.zero ()`,
   `Css.auto ()`, `Css.none ()`. Bare form now a clean type error
   (was silent function-pointer leak), `()` still required.
10. **Multi-line function signatures.** `name\n    : T` (`:` on
    continuation line) parses cleanly. Continuation INSIDE type body
    (`T1\n    -> T2`) unsupported — extract a `type alias` for the whole
    arrow type.
### Closed in v0.16 (kept here for grep)

- ~~`Std.Db.exec`/`query` reject mixed-type param lists (E2001, List
  homogeneous)~~ — closed v0.16.26 (#582). New `SqlValue` ADT
  (`SqlString`/`SqlInt`/`SqlFloat`/`SqlBool`/`SqlBytes`/`SqlDecimal`/
  `SqlTime`/`SqlMoney`/`SqlNull SqlValue`, 9 variants) — `List SqlValue`
  flows through `Db.exec`/`Db.query` w/ full per-column type fidelity.
  8 `fromMaybe*` helpers. Money round-trips via `"ISO_CODE AMOUNT"` TEXT
  + `Db.Decode.money`.
- ~~PATCH needs 3 states/field (value/NULL/omit); Db.exec only modeled
  2~~ — closed v0.16.26 (#582). `SqlField` ADT (`SetField SqlValue`/
  `OmitField`) + `Db.updateFields db table whereCols setFields` builder;
  column-name validation rejects non-`[A-Za-z0-9_.]` idents (no
  injection vector); all-OmitField short-circuits to 0 rows.
- ~~`Db.exec`/`query` reject `Maybe a` params ("unsupported type
  rt.SkyMaybe[int]")~~ — closed v0.16.24 (#574). Runtime `dbBindArg`
  reflect-walks SkyMaybe shape, substitutes `nil`/unwrapped value at
  `Db_exec`+`Db_query` (`Db_queryDecode` inherits).
- ~~`import Std.Db.Decode exposing (Decoder, ...)` errors "does not
  expose type Decoder" though it's a kernel-implicit Prelude type~~ —
  closed v0.16.24 (#576). `Canonicalise.Module.checkItem` accepts 15
  kernel-implicit Prelude types (`Decoder`, `Value`, `Attribute`,
  `Handler`, `Middleware`, `Session`, `Store`, `Route`, `VNode`,
  `Request`, `Response`, `Cmd`, `Sub`, `Db`, `Error`) as no-op in
  `exposing (...)`. Regression: `ExposingSpec` "#576".
- ~~`Std.Db.Decode.nullable` requires double-naming column (`nullable
  "age" (int "age")`), mis-gates on mismatch~~ — closed v0.16.24 (#577).
  Breaking sig change: `nullable : Decoder a -> Decoder (Maybe a)`
  (drops column-name arg); `DbDecoder` gains `cols` field, combinators
  propagate via `dbUnionCols`.
- ~~sky-nav click + popstate handlers skip `r.ok` check before
  `__skyPatch`, so a 404 "session not found" body replaces `<body>`
  verbatim~~ — closed v0.16.16. Both `.then` chains in `liveJSWithCfg…`
  gate on `r.ok`; non-OK → click navigates to link URL, popstate
  reloads current URL (both hit runtime's initial-page handler, always
  succeeds). Regression: `TestSkyNavFetchChecksOk`.
- ~~Unannotated cross-module `view : Cfg msg -> Element msg` miscompiles
  to `any(cfg).(Cfg_R[any])` casts panicking at runtime~~ — closed
  Issue #521. Lowerer pushes enclosing Go fn's typeParams into
  `LowerCtx` via `withScopedEnclosingTypeParams` (Compile.hs) before
  body's GoExpr tree built; closes sibling `Foo_R[any]`-cast-panic
  class (#261/#262/#263/#461/#463/#465/#467). Regression:
  `Sky.Build.UnannotatedParametricCfgViewSpec`.

### Closed in v0.15 (kept here for grep)

- ~~Head-position type alias of function sig dropped params at
  canonicalisation~~ — closed v0.16.4 via PR #123. `unfoldHeadAlias`
  (`Sky.Canonicalise.Module`) peels `TAlias` at annotation head before
  split; `view : Renderer Msg` over `type alias Renderer msg = Model ->
  Element msg` now works. `Sky.Http.Server.Handler` moved here as
  canonical home. Regression: `Sky.Canonicalise.HeadAliasFunctionSig`
  (5 cases).
- ~~Cons-pattern length-guard shared between arms (#402)~~ — closed
  v0.15.54. Codegen emitted only `len(subj) >= 1` per cons step, so
  `a::b::c::_` and `a::b::_` shared `>= 2` guard, 2-elem list hit
  `IndexOutOfRange`. `consChainLength` now emits correct `>= N`/`== N`
  per arm.
- ~~Same-named local lambdas across modules pollute typed lowerer's
  region snapshot~~ — closed v0.15.30 via scoped `LowerCtx` cascade
  (per-module env ledger `Solve.SolvedTypes._stPerModuleEnv`, sentinel
  `GoDeclRaw` entries switch `globalCurrentDepModule` during render).
- ~~Anonymous records in function signatures~~ — closed v0.13
  (`processReq : Int -> { name : String, age : Int } -> String` parses).
- ~~Let bindings w/ params after multi-line case~~ — `let mark j = …`
  after `case … of` arm now parses.
- ~~Zero-arity functions reading env vars memoised at init()~~ —
  `apiKey = System.getenvOr "K" "def"` now reads runtime env.
- ~~`exposing (Type(..))` for user-module ADT ctors~~ — user `type
  Color = Red | Green` exporting `Color(..)` now exposes unqualified
  ctors.
- ~~`import X as Alias` leaks alias into codegen~~ — `import Lib.Db as
  Chat` now emits `Lib_Db_Message_R` from source module, not alias.
- ~~`let` bindings don't support forward references~~ — `let a = b + 1;
  b = 5 in a` now compiles+evaluates correctly.
- ~~Parametric record alias bugs (Surfaces 1, 2, 3)~~ — closed by v0.15
  type-directed lowering + Go generics on parametric records. See
  `docs/v1-rfc/type-soundness-deep-analysis.md`.
- ~~Same-module polymorphic call pinned by first instantiation~~ —
  sibling refs to polymorphic annotated TypedDefs now alpha-rename per
  call site (`f : Cfg msg -> msg` w/ `msg=Int` AND `msg=Bool` both work).
- ~~Wildcard-`any` return type silently accepted against typed slot~~ —
  `view : Model -> any` returning String against expected `Model ->
  Html msg` now correctly surfaces as type error (v0.15.1 same-mod
  CForeign wildcard-gate fix).
- ~~Unknown qualified name (`NotARealModule.foo`) silently passed
  canonicaliser~~ — closed v0.15.42 (audit §3.1). Canonicaliser flags
  any qualified ref whose qualifier is neither kernel module, import
  alias, nor in `_qualVars`/`_qualCtors`, w/ Did-you-mean via
  Levenshtein distance.
- ~~"Compilation successful" printed before `go build` ran~~ — closed
  v0.15.42 (audit §3.4). Sky lowering prints "Sky lowering succeeded";
  "Compilation successful" only fires after Go returns 0.
- ~~User `type Result a = Just a | Nothing` silently shadows
  Prelude-exposed Maybe/Result ctors~~ — closed v0.15.42 (audit §3.2).
  Canonicaliser rejects user ADT whose type/ctor name collides w/
  Prelude-exposed entry, hard error naming canonical stdlib origin.

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

## Project layout

```
src/                              -- Sky compiler (Haskell, GHC 9.4+)
  Sky/Parse/                      -- lexer, layout filter, parser
  Sky/Canonicalise/               -- name resolution, import validation
  Sky/Type/                       -- HM inference, exhaustiveness
  Sky/Build/                      -- orchestration, FFI generator, TCO
  Sky/Generate/Go/                -- Go IR + printer
  Sky/Lsp/                        -- language server
  Sky/Format/                     -- opinionated formatter (Elm-compatible)
  Sky/Doc/                        -- sky doc — index, terminal, HTTP render
app/Main.hs                       -- CLI entry point
runtime-go/rt/                    -- Go runtime (embedded via TH)
sky-stdlib/                       -- Sky-side stdlib (embedded via TH)
sky-bundled/console/              -- Sky Console mini-app
sky-bundled/doc/                  -- sky doc HTTP server mini-app
tools/sky-ffi-inspect/            -- Go package introspector (TH-embedded)
templates/CLAUDE.md               -- Template for `sky init` projects
examples/                         -- 27 example projects
docs/                             -- User + contributor documentation
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

## Agent learnings

Verified, generalizable pitfalls found during dev. Each entry **correct +
sound** — verified by a passing test. Do NOT blind-append; update/dedupe
when related work arrives.

### Decimal: two distinct rounding modes for two distinct public APIs

**Verified** by `runtime/tests/decimal_parity.rs` (commit b9794d7).

`Std.Decimal` exposes two rounding entry points w/ DIFFERENT Go strategies:

| Sky function | Go oracle | Rust strategy |
|---|---|---|
| `Decimal.round` | `shopspring.RoundBank` (banker's/half-to-even) | `MidpointNearestEven` |
| `Decimal.toStringFixed` | `shopspring.StringFixed → Round` (half-away-from-zero) | `MidpointAwayFromZero` |
| `Decimal.formatWith` (rounding step) | same as `StringFixed` | `MidpointAwayFromZero` |

Trap: `round` (banker's) and `toStringFixed`/`formatWith` (half-away)
look related but go through **different** shopspring primitives
(`RoundBank` vs `Round`). Using `MidpointNearestEven` in
`toStringFixed`/`formatWith` gives wrong result at tie values — e.g.
"2.545" at 2dp → "2.54" (banker's) instead of "2.55" (Go). Real
money-precision divergence.

**Rule**: any new Decimal kernel using `StringFixed`/`Round` in Go MUST
use `MidpointAwayFromZero` in Rust. Only kernels using `RoundBank` in Go
use `MidpointNearestEven`.

### Decimal: division precision for non-terminating fractions is a documented divergence

Go shopspring uses `DivisionPrecision = 16` digits for non-exact
divisions (e.g. `1/3 = "0.3333333333333333"`). rust_decimal's
`checked_div` uses different algorithm, may produce different digit
count. Affects only non-terminating decimals; exact fractions
(money-scale ops, powers-of-10 denominators) give bit-identical results.

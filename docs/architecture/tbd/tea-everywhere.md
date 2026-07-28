# TEA everywhere — an opt-in program shape for every Ipê backend

Status: design (spec only; no code, no build implied).
Scope: make The Elm Architecture (`init` / `update` / `view` /
`subscriptions`) an **opt-in** program shape available to every backend —
most importantly the CLI / headless-worker shape — without changing any
existing entry point.

## Goals and constraints

- **Least-intrusive.** Existing entries stay byte-identical:
  `main = Task.run cmd` (one-shot CLI), `Web.app`, `Server.listen`,
  `Tui.app`, `WebView.app`. TEA-for-CLI is a *new value you may choose*,
  never a mode forced onto anything.
- **Easiest-to-implement.** Maximum reuse of the already-ported TEA
  runtime. The smallest possible new surface.
- **Sound by construction.** `update` stays pure `(Model, Cmd)`; every
  effect is a `Cmd` / `Task Error a`; the headless loop terminates
  soundly with no busy-wait, no deadlock, and no runtime panic from
  well-typed Ipê.

## Grounding fact (why this is small)

The ported runtime already contains a headless-capable TEA loop:

```
src/runtime/rust/src/tea.rs
    cli_program(init, update, view, subscriptions, on_line) -> SkyTask<E, ()>
```

It already folds stdin `Line`/`Eof`, `Sub.every` tickers, and
`Cmd.perform` results through `update`; it shares `SubManager`,
`cli_run_cmd`, and the `SkyCmd` / `SkySub` / `SubSpawn` carriers with the
Tui backend; and it is generic over a concrete model type `M` (not
`any`). Critically it **returns `SkyTask<E, ()>`**, so an opt-in entry
built on it slots into the existing `main = … |> Task.run` boundary with
**zero compiler / lowering change**. That single fact is the anchor of
this design: we generalise an existing loop rather than build one.

Two runtime warts noted here and resolved by this design:
- the stdin reader is a detached thread that treats read-`Err`/EOF as
  loop termination (`tea.rs` ~238–248, ~273) — correct for a pipe-fed
  tool, a *bug* for a daemon whose stdin is `/dev/null`;
- `view(model)` is written unconditionally on every step
  (`tea.rs` ~280) — duplicate output on no-op updates.

## Precedent — Elm's `Platform.worker`

Elm ships `Platform.worker { init, update, subscriptions }`: headless TEA
with **no `view`**, for CLI / backend / worker programs. It reacts to
events (ports, timers, task results) through `update`, issuing `Cmd`s,
and has no DOM. This design mirrors that model directly: the headless
Ipê entry is view-less, and output is an ordinary `Cmd`.

---

## The shape-by-shape verdict

| Shape | TEA today? | TEA option | How |
|---|---|---|---|
| Ipe.Web (web) | yes | unchanged | `Web.app { init, update, view, subscriptions, routes, … }` |
| Ipe.Tui (terminal) | yes | unchanged | `Tui.app cfg` |
| Ipe.WebView (desktop) | yes | unchanged | `WebView.app cfg` |
| **Reactive / long-running CLI, daemon, worker** | no | **NEW — opt-in** | `Ipe.Worker.program { init, update, subscriptions } \|> Task.run` (headless, no view; output via `Cmd`) |
| One-shot CLI (`main = Task.run cmd`) | n/a | **declined** | keep the one-shot entry; a pure transform gains nothing from a loop |
| Ipe.Http.Server (routes + handlers) | no | **declined in the request path**; optional **sidecar** | `Handler = Request -> Task Error Response` stays; a `Ipe.Worker.program` may run *alongside* the server owning shared state, coordinated via pub/sub |

One-sentence rule for the matrix: **reach for TEA when a long-lived Model
evolves over a stream of events; keep one-shot / handlers when the
lifecycle is one-shot (a pure CLI transform) or request-scoped (an HTTP
handler).**

---

## Q1 — The opt-in entry shape

**Decision.** Ship a single headless entry:

```elm
type alias WorkerCfg model msg =
    { init          : Flags -> ( model, Cmd msg )
    , update        : msg -> model -> ( model, Cmd msg )
    , subscriptions : model -> Sub msg
    }

Ipe.Worker.program : WorkerCfg model msg -> Task Error ()
```

- **No `view` field.** Output is an effect — `Io.writeStdout` / `Log.*`
  lifted through `Cmd.perform`.
- `Flags` is a **row-open record** `{ args : List String }` (argv),
  extensible later (`cwd`, `env`) without breaking call sites — same
  discipline as `Live`'s `req` evolution.
- The cfg is `Web.app`'s cfg **minus** `view` / `routes` / `notFound`,
  i.e. exactly Elm's `Platform.worker`.

**Rationale.** A `Model -> String` that the runtime auto-writes to stdout
is a side-channel around the `Cmd` tier and reintroduces an
append-vs-repaint ambiguity that a single field cannot resolve; removing
it makes the "worker that half-renders" state unrepresentable and honours
"every effect is a Cmd." Argv handed to `init` up front is
parse-don't-validate: `init` parses raw argv into a typed Model with no
`System.args` round-trip.

**Coexistence.** `Worker.program cfg` returns `Task Error ()`, so it uses
the identical `|> Task.run` tail already used by `Tui.app` / `WebView.app`.
The compiler needs no new `main`-recognition rule. The default one-shot
`main = Task.run cmd` is untouched.

**OPEN DECISION D1 — a view-bearing CLI entry.** A cooked-mode line
REPL / progress-line CLI (`read → process → print`, pipe-friendly, not
Tui's raw-mode canvas) is a legitimate shape. It is **deferred**, not
part of v1. If it ships it must be a *separate* entry whose view is
**explicitly append-line** (never screen-repaint — that is Ipe.Tui's
job) and whose stdin is an ordinary `Stdin.lines` Sub (never a `view`+
`onLine` pair coupled in one cfg). A future "render only on change"
variant additionally needs a `Model: PartialEq`-shaped bound and is out
of scope until the demand is concrete.

## Q2 — Subscription sources for a headless loop

**Delivery model.** The browser transport (SSE outbound + `POST
/_sky/event` inbound) does not exist headless. Every source is a producer
that pushes a `Msg` into the **single in-process mpsc mailbox** the loop
drains (`CliEvent<M>` — already the transport in `tea.rs`, not SSE). The
Sub *registry* (`SubManager`) is reused verbatim; only the ingress
changes. One mailbox ⇒ `update` is never re-entered concurrently
(TEA's single-threaded-update guarantee holds for free); per-source order
is preserved, cross-source interleaving is nondeterministic (documented).

**Decision — the CLI/worker subscription set.**

| Source | Carrier | Delivery | Status |
|---|---|---|---|
| `Time.every ms msg` | `SkySub::Every` | ticker task → mailbox | **reuse** (exists) |
| Cmd / Task completion | `cli_run_cmd` Perform | spawned task result → mailbox | **reuse** (exists; this is the Cmd path, not a user Sub) |
| `Sub.subscribeTopic` (in-proc pub/sub) | `SkySub::Source` | broker → mailbox | **reuse** (broker exists; load-bearing for the Q5 sidecar) |
| WebSocket `onMessage` / `onOpen` / `onClose` | `SkySub::Source` | `ws_client.rs` builder | **free reuse** (already produces a Source) |
| `Http.Stream` chunks (upstream body) | `SkySub::Source` | `http_stream.rs` builder | **free reuse** (already produces a Source) |
| `Stdin.lines` | `SkySub::Source` | blocking line reader task → mailbox | **new shim** (generalise the hardwired reader) |
| `Signal.on*` (SIGINT / SIGTERM / SIGHUP) | see below | process-global handler → mailbox | **new shim** |
| `File.watch` | `SkySub::Source` | notify/inotify task → mailbox | **deferred to v0.2** (only source needing a new crate) |

- **stdin is opt-in, never auto-installed.** In `Worker.program` stdin is
  only present if the app returns a `Stdin.lines` Sub. This
  simultaneously fixes the detached-thread leak and the
  daemon-dies-on-closed-stdin bug: a daemon that does not subscribe to
  stdin gets no reader and no spurious EOF-exit.
- **stdin event type is parsed, not validated:**
  `StdinEvent = Line String | InvalidUtf8 | Eof`. EOF is an *explicit*
  event (closes the "blocks forever after pipe close" hazard); invalid
  UTF-8 is a distinct typed event, not silently coerced to EOF as the
  current reader does. The app parses the `String` payload in `update` —
  no decoder is baked into the Sub (a line is always a well-formed
  `String` at the transport layer; the domain parse belongs to the app).
- **signals are a closed typed enum** (`Interrupt | Terminate | Hangup`)
  — user code cannot inject arbitrary signal numbers. Signal ingress is
  installed **process-global once at loop start** and only *enqueues* a
  typed signal event into the mailbox; it never runs `update`
  re-entrantly. The `Signal.on*` Sub merely maps a signal to a Msg.
  Installing once (rather than via the `SubManager` abort-and-respawn
  cycle) closes the abort→respawn event-loss window without special-casing
  the sub registry.
- **signal ingress MUST be async-signal-safe (G4).** The bridge from OS
  signal to mailbox uses `tokio::signal` (or, failing that, a self-pipe
  written with the async-signal-safe `write(2)`), where the *async task*
  it wakes does the `mpsc::send`. A raw POSIX `sigaction` handler that
  itself calls `mpsc::send` (which allocates / takes locks) is
  **forbidden** — doing non-async-signal-safe work inside a signal handler
  is undefined behaviour (deadlock against an interrupted allocator, or
  memory corruption). The signal source task that awaits `tokio::signal`
  is a normal source task, so it carries the same `live_sources`
  drop-guard as every other source (see G1 above / the termination proof).

**Rejected as ill-fitting:** raw keypresses / cursor / cell rendering
(that is Ipe.Tui's raw-mode reader — do not absorb it); DOM / SSE events
(Ipe.Web only); an HTTP request as a Sub (that is the Server handler
lifecycle — see Q5). Boundary rule: a **Sub** is an ongoing event stream;
a one-shot read ("read this whole file") is a **`Cmd.perform`**, not a
Sub.

## Q3 — The loop and the sound termination rule

**Loop (event-driven, zero busy-wait).**

1. `(model, cmd0) = init flags`; dispatch `cmd0` (spawn tasks; increment
   the pending counter per spawned Perform).
2. Reconcile subscriptions: start/stop sources by diffing
   `subscriptions model` against the previous set (reuse `SubManager`).
3. **Block** on `mailbox.recv().await` — the only blocking point; no
   polling anywhere.
4. On event: `(model', cmd') = update msg model`; dispatch `cmd'`;
   reconcile subscriptions to `model'`.
5. Evaluate the termination rule (below). If not met, go to 3.

**Termination rule — two complementary paths.**

- **Explicit — `Cmd.quit : Int -> Cmd msg`.** A **new `SkyCmd::Quit(i32)`
  variant of the *existing* `Cmd` type**, drained by `cli_run_cmd`.
  `update`'s `(Model, Cmd msg)` signature is unchanged, so the TEA shape
  stays identical across Live / Tui / Webview / Worker. (A `Done`-sum
  return type was rejected precisely because it would fork that shape.)
  On quit the loop stops accepting new events, runs a **bounded-deadline
  drain** of in-flight Cmds, stops all sources, flushes stdout, and the
  returned `Task` resolves with the exit code.
- **Implicit — quiescence.** After an update settles, if
  `live_sources == 0` **and** `pending_cmds == 0` **and** the mailbox is
  empty (non-blocking `try_recv`), then no future `Msg` is reachable, so
  the loop exits `0` instead of blocking forever.

  *`live_sources` is source-TASK liveness, NOT subscription cardinality.*
  This is the load-bearing correction. A source that self-terminates yet
  stays *subscribed* — `Stdin.lines` after `Eof`, `Http.Stream` after
  upstream close, `WebSocket` after `onClose` — is a **dead producer that
  `SubManager` still counts as an active subscription**. Keying quiescence
  on the subscription-set size (`SubManager` cardinality) is therefore
  *unsound*: it leaves the loop blocked forever on `recv()` with zero live
  producers — non-termination from well-typed Ipê. `live_sources` is
  instead a counter over source *tasks that can still enqueue*: it is
  incremented **before** a source task is spawned and decremented from a
  **drop-guard held inside that task** (the exact inc-before-spawn /
  dec-in-drop-guard shape used for `pending_cmds`). The guard fires when
  the source task exits for *any* reason — clean end-of-stream (`Eof`,
  upstream close, socket close), `Err`, panic-caught, or `SubManager`
  abort — so a subscription that is still registered but whose task has
  finished contributes **0** to `live_sources`. Reconciliation
  (`subscriptions model` diffing) still starts/stops tasks; it just no
  longer *defines* liveness.

  *Soundness.* The only enqueuers are (i) source tasks that have not yet
  exited and (ii) in-flight Cmd tasks. `live_sources == 0` ⇒ every source
  task has run its drop-guard ⇒ no source can enqueue again (a
  self-terminated-but-subscribed source is dead, not live).
  `pending_cmds == 0`, where the counter is likewise decremented from a
  drop-guard (so it fires on success / `Err` / panic-caught /
  cancellation), ⇒ no Cmd result is in flight. Both counters are mutated
  **only on the single loop thread** (spawn/reconcile are loop-thread-only)
  and published with `Release` stores paired with `Acquire` loads at the
  post-update check, so the `try_recv`-empty test is TOCTOU-free: an empty
  mailbox observed with both counters zero means no enqueue can be racing
  it. Therefore blocking would be a permanent deadlock, and terminating is
  correct.

  *Live signal sources keep the loop alive.* A registered `Signal.on*`
  subscription is a **live source until it is unsubscribed** — its source
  task is parked awaiting a signal, so its drop-guard has not fired and it
  contributes to `live_sources`. This is what makes a *signal-only daemon*
  correct: an app that subscribes only to `Sub.onSignal` and returns
  `(m, Cmd.none)` from `init` has `live_sources == 1`, so it does **not**
  satisfy quiescence and does **not** exit 0 immediately — it blocks on
  `recv()` waiting for SIGTERM/SIGINT, exactly as a daemon must. Only when
  the app drops the signal sub (or the signal fires and drives a
  `Cmd.quit`) does the loop wind down. This is also what makes the D2
  "timer-only worker" discussion coherent: a live `Time.every` ticker is a
  live source for the identical reason.

  *Degenerate case.* `init` returning `(m, Cmd.none)` with
  `subscriptions = \_ -> Sub.none` spawns no source tasks, so
  `live_sources == 0` at the first post-init check and the worker
  terminates immediately after init — cleanly degenerating to a one-shot
  `main = Task.run`. The worker is a strict superset of the one-shot
  shape. (Contrast the signal-only daemon above: `Sub.onSignal` makes
  `live_sources == 1`, so that shape does *not* degenerate.)

**Both liveness counters are panic-safe (drop-guard).** A `Cmd.perform`
task that faults is caught by the Task-boundary recover and produces *no*
`Msg` (`tea.rs` ~186–193). A naive "decrement on result enqueue" would
then leave `pending > 0` forever and wedge quiescence — a real deadlock
trap. `pending_cmds` and `live_sources` are therefore *both* incremented
**before** spawn and decremented from a **drop-guard held inside the
spawned task**, so each fires on success, `Err`, panic-caught, and
cancellation alike. This makes the termination invariant independent of
the recover-and-dispatch wiring being correct — and, for `live_sources`,
independent of whether the app happens to drop a self-terminated
subscription. A swallowed fault additionally emits a structured
`Log.warn`-shaped line (awaiting the `JoinError`) so it is observable,
not silent.

**Counter memory ordering (G3).** `pending_cmds` and `live_sources` are
mutated **only on the single loop thread** — spawn and subscription
reconciliation both run inside the loop, never from a worker task — while
the drop-guards run on the spawned tasks. Each guard's decrement is a
`Release` store; the loop's post-update quiescence read is an `Acquire`
load of both counters. That pairing gives the loop a happens-before edge
to every prior enqueue, so the `try_recv`-empty check cannot observe a
stale "0" while a `Msg` is still racing into the mailbox. Because the only
*increments* are loop-thread-local (before-spawn), there is no
increment/decrement race to reconcile — only the decrement→observe edge,
which `Release`/`Acquire` closes.

**Quit-drain discards late Msgs (minor).** During the bounded-deadline
drain after `Cmd.quit`, the loop has **stopped accepting events**: any
`Msg` a still-in-flight Cmd produces before the deadline is **discarded**,
and **no post-quit `update` fires**. The drain waits only for side effects
(a pending `Io.writeStdout` flush, an in-flight DB write) to finish or the
deadline to expire; it never folds their results back through `update`.
Quit is terminal by construction, so "one more update after quit" is
unrepresentable.

**Hazard ledger.**

| Hazard | Foreclosed by |
|---|---|
| Busy-wait / CPU spin | Loop blocks on `recv().await`; quiescence checked only post-update via non-blocking `try_recv`. |
| Infinite hang (no live sources, no pending) | Quiescence keyed on `live_sources` (task liveness) → exit 0. |
| **Dead-but-subscribed source hangs the loop** (`Stdin` post-`Eof`, `Http.Stream` post-close, `WebSocket` post-`onClose`) | `live_sources` counts source *tasks*, not subscription cardinality; the source task's drop-guard decrements it on exit, so a still-registered dead source contributes 0 → quiescence, no dependence on the app dropping the sub. |
| Faulting Cmd wedges quiescence forever | Drop-guard decrements `pending_cmds` on panic/cancel. |
| Faulting/exiting source wedges quiescence forever | Drop-guard decrements `live_sources` on end-of-stream/`Err`/panic/abort. |
| Deadlock (block while nothing can deliver) | Loop only blocks when a live source or pending Cmd can still enqueue; else quiesces. |
| Signal-only daemon exits 0 immediately (never waits for SIGTERM) | A live `Signal.on*` sub keeps `live_sources > 0`, so quiescence is not satisfied; the loop blocks awaiting the signal. |
| stdin closed → forever block | Explicit typed `Eof` event; the reader task exits and its drop-guard drops `live_sources` → quiescence (independent of whether `update` drops the sub). |
| Daemon dies on closed stdin | Worker installs no stdin reader unless the app subscribes. |
| Counter TOCTOU (stale 0 read while Msg races in) | Decrement is `Release`, post-update read is `Acquire`; increments are loop-thread-local (G3). |
| Signal handler UB (non-async-signal-safe work in handler) | Signal ingress via `tokio::signal` / self-pipe, never an allocating `mpsc::send` in a raw POSIX handler (G4). |
| Panic from a Cmd/Task | `runWithRecover` → `Err` Msg; `update` is pure. |
| Panic escaping the synchronous path | Inherits `LogPanicAndExit` deferred recover at the `main` boundary. |
| In-flight write dropped on quit | Bounded-deadline drain before exit (not immediate detach). |
| Teardown hangs on a stuck task | Deadline expiry → hard exit. |
| Impatient double Ctrl-C wedges teardown | Second signal (or deadline expiry) → hard `exit 130`. |
| Unbounded mailbox OOM (fast producer) | Bounded channel + backpressure for stdin / file-watch, drops surfaced as a metric — **except** signals, which coalesce into a single always-deliverable pending-quit flag (never dropped/backpressured). |
| Forced intrusion on existing shapes | Feature is additive; no existing entry / `main` rule / codegen path changes. |

**OPEN DECISION D2 — default disposition of an *unsubscribed* signal.**
Two positions:
- *(leaning)* install a default handler so an unsubscribed SIGINT/SIGTERM
  injects a terminal quit → graceful teardown → `exit 130`. Prevents an
  unkillable timer-only worker (an availability defect).
- *(dissent)* leave POSIX default disposition (immediate `exit 130`) when
  unsubscribed — least-surprise; the graceful path can otherwise look
  "slow/wedged." The bounded-deadline drain + double-signal escalation
  makes the leaning option safe, but the choice is not final.

**OPEN DECISION D3 — teardown deadline value.** The bounded-drain ceiling
(and whether it is configurable via a cfg field / env var) is open. Drain
(not detach) is decided; the number is not.

## Q4 — Reuse vs new surface

**Reused verbatim.**
- `SkyCmd` / `SkySub` / `SubSpawn` carrier types.
- `SubManager` (spawn / abort / respawn of tickers + sources).
- `cli_run_cmd` (Cmd firing: None / Batch / Perform / Publish).
- The mpsc `CliEvent<M>` mailbox — this *is* the CLI transport.
- `Time.every` ticker; the in-proc pub/sub broker
  (`live/pubsub.rs`); the `ws_client.rs` and `http_stream.rs` `Source`
  builders — WebSocket-feed and stream-consumer workers therefore work
  **day one with zero new code**.

**Correctly absent.** VNode diff/patch, `assignSkyIDs`, the renderer, SSE
endpoints, session store, HTTP transport, cookies.

**New surface (ranked by cost).**
1. Refactor `cli_program → run_tea_loop { install_stdin, has_view,
   quiescence }` — a *subtraction* of `view`/`on_line` plus the two new
   checks, not new logic. `Worker.program` = `run_tea_loop { no stdin,
   no view, quiescence }`.
   **Tui non-regression (G5).** This refactor touches shipped-green
   Ipe.Tui, which is folded behind the same `run_tea_loop`. The
   `quiescence` flag is **OFF / inert on the Tui path** (`Tui.app` =
   `run_tea_loop { …, quiescence = false }`): Tui's lifecycle is
   render-loop + explicit exit, and it must never auto-exit because the
   subscription set happened to go idle. Unifying Worker / Cli / Tui
   behind one loop therefore cannot regress Tui — the new termination
   logic is dead code on that path. The `live_sources` counter may still
   be maintained on the Tui path but is never *consulted* for exit there.
2. `SkyCmd::Quit(i32)` + one arm in `cli_run_cmd` + loop break /
   exit-code plumbing + bounded-deadline drain.
3. Quiescence: a `live_sources` counter maintained by a **drop-guard on
   each source task** (task liveness, *not* `SubManager` cardinality) + a
   `pending_cmds` counter with the same **drop-guard** + `Release`/`Acquire`
   ordering on both + the post-update `try_recv`-empty predicate.
   (~40 lines.)
4. Source shims: `Stdin.lines` (typed `Eof` / `InvalidUtf8`; generalise
   the hardwired reader) and `Signal.on*` (process-global handler).
   **No** `File.watch` in v1; **no** new WS / stream code (free reuse).
5. Thin Ipê stdlib: `Ipe.Worker` (cfg wrapper → `Task Error ()`),
   `Cmd.quit`, `Sub.stdin` / `Sub.onSignal` — `Ffi.kernel` aliases.

**Backend wiring.** Surfacing the entry needs one `KernelFn` row
(`CliProgram`/`WorkerProgram`) + a `naming.rs` entry (contrast
`KernelFn::TuiProgram => "tui_app"`; there is currently no `CliProgram`
row) so the symbol is emittable. The parametric cfg-record lowering
(Go-generics path) must be exercised the same way `live_app` / `tui_app`
already were.

**Deliberately NOT done now:** extracting the loop core out of `live/` or
unifying all four backends behind one loop. The reusable spine is
`cli_program` (already transport-free and separate from Live), so
generalising it is the least-intrusive path. Fold Worker / Cli / Tui
behind the refactored `run_tea_loop`; write the seam (a model-change hook
+ transport-neutral mailbox) so Live *could* later share it. See D4.

**OPEN DECISION D4 — cross-backend loop unification.** Routing Live
behind the shared `run_tea_loop` is architecturally attractive but
touches shipped, green SSE-entangled code. Treated as a **follow-up RFC,
not a prerequisite** for this additive feature.

**OPEN DECISION D5 — `init` signature vs the ported `Fn(())`.** The
current `cli_program` takes `Fn(())`. This design specifies
`init : Flags -> (Model, Cmd)` with `Flags = { args : List String }`
(row-open) on parse-don't-validate grounds. This is a small runtime
signature addition; whether to add `cwd` / `env` to `Flags` immediately
is open.

## Q5 — All the other shapes

- **One-shot `main = Task.run cmd`:** unchanged; the correct model for
  "do X, exit." TEA is opt-in *only* for genuinely reactive / long-lived
  programs. Do not wrap trivial scripts in a loop.
- **Ipe.Http.Server — per request: NO.** A request is
  `Request -> Task Error Response`; wrapping each request in its own
  init/update/subs/quiescence loop is a lifecycle category error and pure
  overhead. Routes + handlers stay.
- **Ipe.Http.Server — global shared state: YES, as an optional sidecar.**
  A `Ipe.Worker.program` runs *alongside* `Server.listen`, owning the
  shared Model (scheduler, cache warmer, metrics aggregator, rate-limit
  buckets, in-memory room). Handlers push Msgs to it via the existing
  pub/sub broker (`Cmd.publish` / `Ipe.PubSub.publish` →
  `Sub.subscribeTopic`). TEA lives at the *state-owner* boundary, never
  in the request path.

  **Read-path note (design gap flagged for implementation).** The
  handler→worker *write* path is clean pub/sub. The *read* path (a handler
  needs the worker's current state to build a response) is **not** solved
  by pub/sub without a request/reply round-trip. The intended pattern is
  for the worker to publish immutable snapshots that handlers read without
  contending with each other. This must be named in the sidecar recipe so
  users are not silently pushed toward a round-trip.

  **Snapshot mechanism (G6).** Use an **atomic `Arc` swap** (`arc-swap`'s
  `ArcSwap<Snapshot>`), *not* `Arc<RwLock<Snapshot>>`. An `RwLock` read is
  a **shared lock**, not lock-free — it still writes the lock's reader
  count (a contended atomic RMW), so calling it "lock-free" is wrong and
  under a write-heavy worker it serialises readers against the writer.
  `ArcSwap::load` is a genuinely wait-free read that hands back an
  `Arc<Snapshot>`; the worker publishes a new state with a single
  `store(Arc::new(next))`. `Snapshot` must be `Send + Sync` (it crosses
  the worker→handler thread boundary and is shared across concurrent
  handler tasks) and should be cheap to `Clone` / share by `Arc` so a
  handler can hold its loaded snapshot across `.await` points without
  pinning the worker's next publish.
- **Ipe.Web / Tui / Webview:** already TEA; unchanged.

## Q6 — Migration and opt-in ergonomics

**Opt-in gesture — one import + one entry:**

```elm
import Ipe.Worker as Worker

main =
    Worker.program
        { init          = \flags -> ( initModel flags.args, Cmd.none )
        , update        = update
        , subscriptions = \model ->
            Sub.batch
                [ Time.every 1000 Tick
                , Sub.onSignal (\_ -> RequestQuit)   -- SIGINT/TERM as a Msg
                ]
        }
        |> Task.run
```

- **Zero migration cost.** Purely additive; no existing app changes; no
  existing entry point touched; no `main`-recognition change. `update`
  keeps `(Model, Cmd msg)`; termination is a new *Cmd*, not a new return
  type.
- **Zero new concepts for existing users.** `init` / `update` /
  `subscriptions` carry their Live/Tui signatures. The only new ideas:
  *no view — print via `Cmd`*, and *quit via `Cmd.quit` or run to
  quiescence*. `Cmd.batch`, `Cmd.perform`, `Task.parallel`, `Sub.batch`,
  `Time.every`, `Sub.subscribeTopic`, WebSocket + Http.Stream Subs all
  carry over unchanged.
- **Builder convention.** `Ipe.Worker` ships `defaultCfg` + `with*`
  builders per the stdlib typed-record convention, so future cfg fields
  (`onQuiesce`, exit-code policy) are additive.

**Shape-matrix delta — one row** (append to AGENTS.md "App shape matrix"):

| User wants… | Use | Entry point shape | Notes |
|---|---|---|---|
| Reactive / long-running CLI, daemon, cron worker, feed consumer | **Ipe.Worker** | `Worker.program { init, update, subscriptions } \|> Task.run` | Headless TEA (no view). Output via `Cmd` (`Io.writeStdout` / `Log.*`). Subs: timers / signals / pub-sub / stdin / WebSocket / Http.Stream / Cmd results. Terminates on `Cmd.quit code` or quiescence. Not for one-shot scripts or HTTP request handling. |

**Template / docs sync (non-negotiable, same commit).** `docs/stdlib.md`
(new `Ipe.Worker`, `Cmd.quit`, `Sub.stdin` / `Sub.onSignal`);
`docs/tooling/cli.md` (the reactive-CLI shape); `docs/sky-toml.md` if any
`[worker]` keys are added; the "App shape matrix" + "Effect boundary" +
"Active limitations" sections and their mirrors in `templates/AGENTS.md`;
`README.md` "What's in the box." A worked example
(`examples/NN-headless-worker`) demonstrating a SIGTERM-graceful daemon +
a self-terminating stdin processor, the way `30-sse-server-demo`
demonstrates streaming.

---

## Open decisions (summary)

- **D1** — a view-bearing CLI entry (deferred; append-line only, stdin as
  a Sub, separate entry if it ships).
- **D2** — default disposition of an unsubscribed signal (graceful
  injected quit vs POSIX default exit 130).
- **D3** — bounded-drain teardown deadline value / configurability.
- **D4** — Live-behind-shared-loop unification (follow-up RFC, not a
  prerequisite).
- **D5** — `init : Flags -> …` row-open shape vs the ported `Fn(())`, and
  whether `Flags` carries `cwd` / `env` in v1.

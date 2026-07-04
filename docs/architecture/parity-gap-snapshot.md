# Parity-Gap Snapshot — skyc examples sweep

**Date:** 2026-07-04  
**Binary:** `/home/arthur/.cache/sky-rust-target-2/debug/skyc` (built 2026-07-04T02:43, HEAD `1806aa2` = task #114)  
**Environment:** `SKY_RUNTIME_DIR=/home/arthur/Documentos/comp/sky-rust/runtime/src/sky_runtime`  
**Scope:** All 35 in-scope example directories (`examples/00` → `examples/test_pkg`).  
**Measurement:** `skyc build src/Main.sky --out /tmp/parity-sweep/<name>/rust` with `timeout 120`. Exit-0 = skyc-0; anything else = FAIL with first diagnostic recorded.  

> **Caveat — skyc-0 only.** OK means skyc exited 0 (canonicaliser + type-checker + emit all passed). It does NOT mean the emitted Rust compiled via cargo. Cargo-level failures (exit-0-then-cargo-fail, seal class) are a separate and ongoing concern tracked by tasks #89/#94/#95/#99/#104/#112. This sweep measures the earlier gate.

> **Caveat — #108 known cases.** Examples 09-live-counter and 34-multi-tier-console are documented to produce T0001 due to `#108` (RoutedLiveApp open-record) being pending. Example 10-live-component also shows T0001 but for a *different* reason (cross-module ADT case-arm payload type).

---

## (a) Full Example Table

| Example | Exit | First error code | First blocker (exact message) | Layer | Task-ref / note |
|---|---|---|---|---|---|
| 00-standard-libs | 1 | SKY-N0005 | `` `Task` has no member `perform` `` | L1b | Task.perform, retryWith, linearBackoff, Decimal, Money all missing |
| 01-hello-world | **0** | — | — | OK | |
| 02-go-stdlib | 1 | SKY-N0005 | `` `Time` has no member `timeString` `` | L1b | |
| 04-local-pkg | **0** | — | — | OK | |
| 06-json | 1 | SKY-L0106 | `top-level function needs a type signature` | L5 | Untyped-functions feature not supported |
| 09-live-counter | 1 | SKY-T0001 | `app: expected cfg with routes/notFound, found non-routed shape` | L3 | **#108** known case |
| 10-live-component | 1 | SKY-T0001 | `Counter.update: expected Main.Msg, found Counter.Msg` | L3 | Cross-module ADT case-arm payload inference bug; distinct from #108 |
| 12-skyvote | 1 | SKY-N0004 | `unknown module \`Auth\`` | L1a | Std.Auth absent; **#111** |
| 14-task-demo | 1 | SKY-L0102 | `` `_` in lambda stays fully polymorphic — type cannot be determined `` | L3 | Polymorphic `_` in lambda body |
| 15-http-server | 1 | SKY-T0004 | `` `handleHome` has type Handler — more parameters than signature describes `` | L3 | `Handler` head-alias not unfolded at declaration |
| 16-skychess | 1 | SKY-N0005 | `` `List` has no member `filterMap` `` | L1b | |
| 17-skymon | 1 | SKY-T0001 | `expected String, found SqlValue` (bad span — points to import line) | L3 | Db.* kernel scheme SqlValue return-type mismatch; diagnostics span bug; **#34** |
| 18-job-queue | 1 | SKY-T0001 | `Db.exec: expected Int, found SqlValue` | L3 | Db.exec return scheme is `Task Error SqlValue` instead of `Task Error Int`; **#34** |
| 19-skyforum | 1 | SKY-N0004 | `unknown module \`Region\`` | L1a | Std.Ui.Region absent; new task needed |
| 20-cli-counter | 1 | SKY-N0004 | `unknown module \`Cli\`` | L1a | Std.Cli absent (import exists, module unrecognised); **#111** |
| 21-tui-stopwatch | 1 | SKY-T0001 | `Tui.program: expected { kind : String, value : String }, found String` | L3 | Tui.program kernel scheme wrong — expects wire-format sub shape, not typed Sub |
| 22-tui-stopwatch-ui | 1 | SKY-L0108 | `` `Ui.button` kernel not available yet `` | L1c | |
| 23-tui-todo | 1 | SKY-N0005 | `` `Font` has no member `lineThrough` `` | L1b | |
| 24-tui-kitchen-sink | 1 | SKY-N0005 | `` `Font` has no member `lineThrough` `` | L1b | |
| 25-sky-console | 1 | SKY-L0108 | `` `route` kernel not available yet (blank line span) `` | L1c | route kernel backed by **#108** |
| 26-ui-showcase | 1 | SKY-N0004 | `unknown module \`Input\`` | L1a | Std.Ui.Input absent; new task needed |
| 27-multi-session-chat | 1 | SKY-N0005 | `` `Sub` has no member `subscribeTopic` `` | L1b | Pub/sub kernels (subscribeTopic, Cmd.publish) missing |
| 28-streaming-chat | 1 | SKY-N0004 | `unknown module \`HttpStream\`` | L1a | Sky.Core.Http.Stream absent; **#111** |
| 29-webview-threejs-spike | 1 | SKY-T0001 | `` `Html.node`: expected a, found Html b `` | L3 | `any` wildcard return-type unification failing |
| 30-sse-server-demo | 1 | SKY-N0004 | `unknown module \`Stream\`` | L1a | Sky.Http.Server.Stream absent; **#111** |
| 31-webview-stopwatch-ui | 1 | SKY-L0108 | `` `Ui.button` kernel not available yet `` | L1c | |
| 32-sse-relay | 1 | SKY-N0004 | `unknown module \`ServerStream\`` | L1a | Sky.Http.Server.Stream absent; **#111** |
| 33-websocket-echo | 1 | SKY-N0001 | `not found in scope` (inside triple-quoted string HTML) | L5 | Parser bug: triple-quoted string terminates at first inner `"` |
| 34-multi-tier-console | 1 | SKY-T0001 | `app: expected cfg with routes/notFound, found non-routed shape` | L3 | **#108** known case |
| 37-composite-live-shop | 1 | SKY-N0004 | `unknown module \`Region\`` | L1a | Std.Ui.Region absent |
| 38-composite-ui-multibackend | 1 | SKY-N0004 | `unknown module \`Region\`` | L1a | Std.Ui.Region absent |
| simple | 1 | SKY-N0005 | `` `Task` has no member `perform` `` | L1b | Same as 00-standard-libs |
| spike-css-source | **0** | — | — | OK | |
| spike-std-source | **0** | — | — | OK | |
| test_pkg | **0** | — | — | OK | |

---

## (b) Counts

### By exit code

| Status | Count | Examples |
|---|---|---|
| skyc-0 (OK) | **5** | 01, 04, spike-css-source, spike-std-source, test_pkg |
| FAIL | **30** | all others |

### By SKY error code

| Code | Count | Short description |
|---|---|---|
| SKY-N0004 | 9 | unknown module |
| SKY-T0001 | 8 | type mismatch |
| SKY-N0005 | 7 | module has no such member |
| SKY-L0108 | 3 | kernel not available yet |
| SKY-L0106 | 1 | top-level function needs type sig |
| SKY-L0102 | 1 | polymorphic type undetermined |
| SKY-T0004 | 1 | more params than signature |
| SKY-N0001 | 1 | value not in scope (parser bug) |

### By blocker layer

| Layer | Count | Description |
|---|---|---|
| **L1** | **19** | Missing kernel / module / member (N0004 + N0005 + L0108) |
|  L1a — N0004 | 9 | Module not registered at all: Std.Auth, Std.Cli, Std.Ui.Region, Std.Ui.Input, Sky.Core.Http.Stream, Sky.Http.Server.Stream (×2), Sky.Http.Server.Stream alias ServerStream |
|  L1b — N0005 | 7 | Module exists but member missing: Task.perform, Time.timeString, List.filterMap, Font.lineThrough ×2, Sub.subscribeTopic |
|  L1c — L0108 | 3 | Module registered but kernel not backed: Ui.button ×2, route kernel |
| **L3** | **9** | Typing / semantics errors |
|  T0001 | 7 | #108 routed cfg (×2), SqlValue scheme (×2), cross-module ADT (×1), Tui.program scheme (×1), Html.node any-unification (×1) |
|  T0004 | 1 | Handler head-alias unfold |
|  L0102 | 1 | Polymorphic `_` lambda |
| **L5** | **2** | Parser / other |
|  L0106 | 1 | Untyped top-level function |
|  N0001 | 1 | Triple-quoted string parser bug (inner `"` terminates) |

---

## (c) Critical Path to Sweep-Green

Ordered by number of first-blockers each fix would remove. Second-order unblocks noted separately.

### Fix 1 — #111: Effect stdlib modules (Std.Auth, Std.Cli, Sky.Http.Server.Stream, Sky.Core.Http.Stream)

**Direct unblock: 5 examples** → 12-skyvote, 20-cli-counter, 28-streaming-chat, 30-sse-server-demo, 32-sse-relay

All five fail with N0004 because their primary import is to one of these absent modules. The compiled-source stdlib (`crates/skyc/stdlib/`) contains no `Std/Cli.sky`, no `Sky/Http/Server/Stream.sky`, no `Sky/Core/Http/Stream.sky`, no `Std/Auth.sky` — confirmed by `find`. Task #111 is the filed work item; it covers Cli, both Stream variants, WebSocket, and Auth as a batch.

Note on Std.Cli specifically: example 20-cli-counter has the explicit `import Std.Cli as Cli` import statement, so this is genuinely an absent module, not a missing qualifier registration.

### Fix 2 — New task: Register Std.Ui.Region + Std.Ui.Input

**Direct unblock: 4 examples** → 19-skyforum, 26-ui-showcase, 37-composite-live-shop, 38-composite-ui-multibackend

Three examples (19, 37, 38) fail on N0004 for `Region` (imported as `Std.Ui.Region`). Example 26 fails on N0004 for `Input` (Std.Ui.Input). Neither module appears in the compiled-source stdlib or kernel registry. This is NOT covered by any existing pending task — it needs a new issue. Likely the same implementation pattern as other Std.Ui sub-modules.

### Fix 3 — #108: RoutedLiveApp (routes/notFound row-poly cfg + route kernel)

**Direct unblock: 3 examples** → 09-live-counter, 25-sky-console, 34-multi-tier-console

- 09 and 34: T0001 because `app`'s constrain scheme always expects `routes : List LiveRoute` + `notFound : Page` fields in the cfg record, but both examples provide the non-routed shape. Fixing #108 (open row-poly cfg) makes these fields optional.
- 25-sky-console: provides routes + notFound but gets L0108 on the `route` kernel itself (backed by #108's implementation).

### Fix 4 — Task.perform + retry kernels (N0005)

**Direct unblock: 2 examples first-blocker** → 00-standard-libs, simple

Both fail immediately on `Task.perform`. However, fixing this is a **prerequisite for many other examples' secondary blockers** — nearly every CLI, Live, and Tui app eventually calls `Task.perform` or `Task.run`. Second-order impact is high. The retry family (`retryWith`, `linearBackoff`, `exponentialBackoff`, `withJitter`) and the two Std.Time members (`isLeapYear`, `daysInMonth`) as well as `Std.Decimal` and `Std.Money` are further blockers in 00-standard-libs after this first one clears.

### Fix 5 — Ui.button + Font.lineThrough + Sub.subscribeTopic kernels

**Direct unblock: 5 examples** → 22-tui-stopwatch-ui (Ui.button), 23-tui-todo (Font.lineThrough), 24-tui-kitchen-sink (Font.lineThrough), 27-multi-session-chat (Sub.subscribeTopic), 31-webview-stopwatch-ui (Ui.button)

These are three independent L1b/L1c gaps that each block specific examples. Batching them in one PR would be efficient.

---

### Remaining blockers (1 example each)

After the top-5, the remaining FAIL examples each have a unique blocker:

| Example | Blocker | Classification | Notes |
|---|---|---|---|
| 02-go-stdlib | `Time.timeString` N0005 | L1b | Also second-blocker in 27 |
| 06-json | Untyped top-level function (L0106) | L5 | Needs untyped-functions feature or example update |
| 10-live-component | Cross-module ADT case-arm T0001 | L3 | `Counter.Msg` payload has wrong type in arm body |
| 14-task-demo | Polymorphic `_` L0102 | L3 | `\_ -> Task.succeed` lambda wildcard |
| 15-http-server | `Handler` head-alias unfold T0004 | L3 | Type alias at head position not unfolded |
| 16-skychess | `List.filterMap` N0005 | L1b | |
| 17-skymon | SqlValue scheme mismatch + bad span | L3 | **#34** — Db.* return type wrong in constrain scheme; also a diagnostics bug (span on import line) |
| 18-job-queue | `Db.exec` T0001: expected Int, found SqlValue | L3 | **#34** — Db.exec return type scheme is `Task Error SqlValue` not `Task Error Int` |
| 21-tui-stopwatch | `Tui.program` T0001: wrong sub shape | L3 | Tui.program kernel scheme expects wire-format `{kind, value}` not typed `Sub` |
| 29-webview-threejs-spike | `Html.node` T0001: `expected a, found Html b` | L3 | `view : Model -> any` return annotation — `any` wildcard unification failing |
| 33-websocket-echo | Triple-quoted string parser bug | L5 | `"` inside `"""..."""` terminates string early; N0001 on inner HTML content |

---

## Summary of impact by fix

| Fix | Examples directly unblocked |
|---|---|
| #111 (effect stdlib modules) | 5 |
| Std.Ui.Region + Std.Ui.Input (new task) | 4 |
| #108 (RoutedLiveApp) | 3 |
| Task.perform + retry family | 2 (many secondary) |
| Ui.button + Font.lineThrough + Sub.subscribeTopic | 5 |
| **Top 5 combined** | **19 of 30** |

---

## How many examples does #108 unblock?

**3 examples** (09-live-counter, 25-sky-console, 34-multi-tier-console).

- 09 and 34 are the canonical #108 T0001 cases where `app` requires `routes/notFound` always.
- 25-sky-console provides `routes/notFound` and passes the type check but hits L0108 on the `route` kernel, which is part of #108's implementation scope.
- 10-live-component also shows T0001 but the error is "Counter.Msg vs Main.Msg" — a different inference bug unrelated to #108.

---

## Sweep environment notes

- Binary HEAD: commit `1806aa2` (task #114, unary-negation parser support). All tasks up to and including #114 are in the binary.
- Tasks #78 and #80 (register Cli/Stream/ServerStream/HttpStream in canon) are marked completed, but the sweep confirms `Std.Cli`, `Sky.Http.Server.Stream`, and `Sky.Core.Http.Stream` are still absent from both the compiled-source stdlib and the kernel registry. These remain open as the first-blockers for examples 20, 28, 30, 32. Task #111 is the appropriate owner.
- `Std.Ui.Region` and `Std.Ui.Input` absence is a new gap not covered by any existing pending task.
- The triple-quoted string parser bug (example 33) is new — a `"` inside `"""..."""` terminates the outer string, causing the inner HTML text to be parsed as Sky identifiers.
- The `Db.exec` / `Db.*` SqlValue scheme mismatch (examples 17, 18) correlates to task #34 (SqlValue 7→9 variants + exhaustive emit_db_call). The return type of `Db.exec` in the constrain scheme emits `Task Error SqlValue` instead of `Task Error Int`, causing a downstream type clash.

---

*Generated by read-only sweep — no source files modified.*

# Parity-Gap Snapshot — skyc examples sweep

---

## Snapshot 2026-07-05 — HEAD `a2059fd` (#115 Basics numerics + -x, #94 Msg gate, #121 arity-exact FuncValue+captures, #123 Order/compare, #120 route diags, #124/#117 Input+Region, #111-A3 Auth/Stream lowering, #126 symbol fixes, L0106 unannotated fns, #132 interpolation + Task.perform/subscribeTopic + Ui.button/lineThrough + Math.isNaN + filterMap/sortBy)

**Date:** 2026-07-05
**Binary:** `/home/arthur/.cache/sky-rust-target-3/debug/skyc` (built 2026-07-05, HEAD `a2059fd`)
**Environment:** `SKY_RUNTIME_DIR=/home/arthur/Documentos/comp/sky-rust/runtime/src/sky_runtime`
**Scope:** All 35 in-scope example directories (`examples/00` → `examples/test_pkg`).
**Measurement:** `skyc build <sky.toml|src/Main.sky> --out <dir>/sky-out/rust` with `timeout 120`. Exit-0 = OK; anything else = FAIL with first diagnostic recorded.

> **Caveat — skyc-0 only.** OK means canonicaliser + type-checker + emit all passed. Cargo-level compilation of emitted Rust is a separate gate.

### (a) Full Example Table

| Example | Exit | First error code | First blocker (exact message) | Layer | Notes |
|---|---|---|---|---|---|
| 00-standard-libs | 1 | SKY-N0005 | `` `Task` has no member `retryWith` `` | L1b | **Advanced** — was `Task.perform`; perform now present, retryWith next |
| 01-hello-world | **0** | — | — | OK | |
| 02-go-stdlib | 1 | SKY-N0005 | `` `Time` has no member `timeString` `` | L1b | Unchanged |
| 04-local-pkg | **0** | — | — | OK | |
| 06-json | **0** | — | — | **OK** | **NEW PASS** — was L0106; L0106 unannotated-fn fix cleared |
| 09-live-counter | **0** | — | — | **OK** | **NEW PASS** — was L0106; L0106 unannotated-fn fix cleared |
| 10-live-component | 1 | SKY-T0001 | `Counter.update: expected Main.Msg, found Counter.Msg` | L3 | Unchanged |
| 12-skyvote | 1 | SKY-N0003 | `Error kind info: unknown constructor` | L2 | Unchanged |
| 14-task-demo | 1 | SKY-L0102 | `` `\_` stays fully polymorphic `` | L3 | Unchanged |
| 15-http-server | 1 | SKY-T0004 | `` `handleHome` has type Handler — more parameters than signature describes `` | L3 | Unchanged |
| 16-skychess | 1 | SKY-L0108 | `` `Std.Sub` import: kernel not available yet `` | L1c | Unchanged |
| 17-skymon | 1 | SKY-T0001 | `expected String, found SqlValue` (bad span on import line) | L3 | Unchanged |
| 18-job-queue | 1 | SKY-T0001 | `` `Db.exec`: expected Int, found SqlValue `` | L3 | Unchanged |
| 19-skyforum | 1 | SKY-N0005 | `` `Ui` has no member `name` `` | L1b | Unchanged |
| 20-cli-counter | 1 | SKY-L0102 | `` `\_` in Cmd.perform: stays fully polymorphic `` | L3 | Unchanged |
| 21-tui-stopwatch | 1 | SKY-T0001 | `` `Tui.program`: expected `{ kind : String, value : String }`, found String `` | L3 | Unchanged |
| 22-tui-stopwatch-ui | 1 | SKY-T0001 | `` `Tui.app`: expected `{ kind : String, value : String }`, found String `` | L3 | **Advanced** — was L0108 Ui.button; Ui.button added, Tui.app scheme now the gate |
| 23-tui-todo | 1 | SKY-T0001 | `` `++`: expected String, found List { done : Bool, id : a, label : String } `` | L3 | **Advanced** — was N0005 Font.lineThrough; lineThrough added, list-concat type now the gate |
| 24-tui-kitchen-sink | 1 | SKY-N0005 | `` `Ui` has no member `style` `` | L1b | **Advanced** — was N0005 Font.lineThrough; lineThrough added, Ui.style now the gate |
| 25-sky-console | 1 | SKY-L0108 | `blank-span kernel not available` (route kernel) | L1c | Unchanged |
| 26-ui-showcase | 1 | SKY-N0005 | `` `Ui` has no member `cinemascope` `` | L1b | **Advanced** — was N0004 Input; Input registered, Ui.cinemascope now the gate |
| 27-multi-session-chat | 1 | SKY-N0005 | `` `Time` has no member `timeString` `` | L1b | **Advanced** — was N0005 Sub.subscribeTopic; subscribeTopic added, Time.timeString now the gate |
| 28-streaming-chat | 1 | SKY-N0005 | `` `HttpStream` has no member `chunks` `` | L1b | Unchanged |
| 29-webview-threejs-spike | 1 | SKY-T0001 | `` `Html.node`: expected a, found Html b `` | L3 | Unchanged |
| 30-sse-server-demo | 0 | — | — | — | **OK** (77d70cd: StreamWriter reclassified CopyLeaf; skyc + cargo verified) |
| 31-webview-stopwatch-ui | 1 | SKY-T0001 | `` `Ui.layout`: expected a, found Html Main.Msg `` | L3 | **Advanced** — was L0108 Ui.button; Ui.button added, Ui.layout return-type now the gate |
| 32-sse-relay | 0 | — | — | — | **OK** (77d70cd: StreamWriter reclassified CopyLeaf; skyc + cargo verified) |
| 33-websocket-echo | 1 | SKY-N0004 | `` unknown module `Ws` `` | L1a | Unchanged |
| 34-multi-tier-console | **0** | — | — | OK | |
| 37-composite-live-shop | 1 | SKY-N0004 | `` unknown module `Responsive` `` | L1a | Unchanged |
| 38-composite-ui-multibackend | 1 | SKY-N0004 | `` unknown module `Chart` `` | L1a | **Advanced** — was N0004 Input; Input registered, Chart (Std.Ui) absent now the gate |
| simple | **0** | — | — | **OK** | **NEW PASS** — was N0005 Task.perform; Task.perform added (#132) |
| spike-css-source | **0** | — | — | OK | |
| spike-std-source | **0** | — | — | OK | |
| test_pkg | **0** | — | — | OK | |

### (b) Counts

**17/35 OK** (+10-live-component — #139 poly-tvar map + Access-clone; cargo-sealed). Lanes in flight: #141 (28/37/38 round-2), #140 (17 codegen clusters). Types/canon surface hardened: #136 review closed, #138 total resolution (SKY-N0002 + did-you-mean).

| Status | Count | Examples |
|---|---|---|
| skyc-0 (OK) | **9** | 01, 04, 06, 09, 34, simple, spike-css-source, spike-std-source, test_pkg |
| FAIL | **26** | all others |

#### By SKY error code

| Code | Count | Δ vs 10611cb | Short description |
|---|---|---|---|
| SKY-T0001 | 8 | +3 | type mismatch (22/23/31 advanced past L0108/N0005 into type errors) |
| SKY-N0005 | 7 | −1 | module member absent |
| SKY-N0004 | 3 | −1 | unknown module |
| SKY-L0102 | 2 | 0 | polymorphic wildcard |
| SKY-L0108 | 2 | −4 | kernel not available yet |
| SKY-L0126 | 2 | +2 | non-Clone capture in closure (new class — 30/32 advanced past stream kernel gap) |
| SKY-T0004 | 1 | 0 | more parameters than signature |
| SKY-N0003 | 1 | 0 | constructor not found |
| SKY-L0106 | 0 | −2 | **(cleared)** unannotated-fn fix unblocked 06, 09; also removed as second-blocker in 00/simple |

#### By blocker layer

| Layer | Count | Description |
|---|---|---|
| **L1** | **12** | Missing kernel / module / member |
|  L1a — N0004 | 3 | Unknown module: Ws (Sky.Core.WebSocket), Responsive (Std.Ui.Responsive), Chart (Std.Ui.Chart) |
|  L1b — N0005 | 7 | Member absent: Task.retryWith (00), Time.timeString (02, 27), Ui.name (19), Ui.style (24), Ui.cinemascope (26), HttpStream.chunks (28) |
|  L1c — L0108 | 2 | Kernel not backed: Std.Sub (16), route (25) |
| **L3** | **12** | Typing / semantics |
|  T0001 | 8 | Tui scheme ×2 (21, 22), SqlValue ×2 (17, 18), cross-module ADT (10), list `++` (23), Html.node (29), Ui.layout (31) |
|  L0102 | 2 | Polymorphic `_` lambda (14, 20) |
|  T0004 | 1 | Handler head-alias (15) |
|  N0003 | 1 | Error ADT constructor (12) |
| **L4** | **2** | Language feature gap |
|  L0126 | 2 | Non-Clone capture in closures (30, 32) |

### (b) Delta from `10611cb`

| Example | Was | Now | Driver |
|---|---|---|---|
| **06-json** | FAIL L0106 | **OK** | L0106 unannotated-fn fix |
| **09-live-counter** | FAIL L0106 | **OK** | L0106 unannotated-fn fix |
| **simple** | FAIL N0005 (Task.perform) | **OK** | Task.perform kernel added (#132) |
| 00-standard-libs | FAIL N0005 (Task.perform) | FAIL N0005 (Task.retryWith) | Task.perform added; retryWith not yet |
| 22-tui-stopwatch-ui | FAIL L0108 (Ui.button) | FAIL T0001 (Tui.app scheme) | Ui.button added; Tui subscription scheme now the gate |
| 23-tui-todo | FAIL N0005 (Font.lineThrough) | FAIL T0001 (`++` list type) | Font.lineThrough added; list-concat type mismatch now the gate |
| 24-tui-kitchen-sink | FAIL N0005 (Font.lineThrough) | FAIL N0005 (Ui.style) | Font.lineThrough added; Ui.style now the gate |
| 26-ui-showcase | FAIL N0004 (Input) | FAIL N0005 (Ui.cinemascope) | Input module registered; cinemascope kernel absent now the gate |
| 27-multi-session-chat | FAIL N0005 (Sub.subscribeTopic) | FAIL N0005 (Time.timeString) | subscribeTopic added (#132); Time.timeString now the gate |
| 30-sse-server-demo | FAIL L0108 (Stream.stream) | FAIL L0126 (non-Clone capture) | stream kernel added; non-Clone writer capture now the gate |
| 31-webview-stopwatch-ui | FAIL L0108 (Ui.button) | FAIL T0001 (Ui.layout return type) | Ui.button added; Ui.layout `any` return unification now the gate |
| 32-sse-relay | FAIL L0108 (ServerStream.stream) | FAIL L0126 (non-Clone capture) | stream kernel added; non-Clone writer capture now the gate |
| 38-composite-ui-multibackend | FAIL N0004 (Input) | FAIL N0004 (Chart) | Input registered; Std.Ui.Chart absent now the gate |

### (c) New dominant blocker classes

Ranked by example count:

1. **T0001 — type mismatch (8 examples):** Tui.program/Tui.app subscription scheme (21, 22 — same root cause; expects wire-format `{kind,value}` not typed Sub), Db.exec/Db.* SqlValue return-type mismatch (17, 18 — same task #34), cross-module ADT case-arm inference (10), list `++` homogeneity (23), Html.node `any` unification (29), Ui.layout `any` return (31). Eight examples, five distinct root causes.

2. **N0005 — missing module member (7 examples):** Task.retryWith (00), Time.timeString (02, 27), Ui.name (19), Ui.style (24), Ui.cinemascope (26), HttpStream.chunks (28). All in already-registered modules; each is a kernel addition.

3. **L0102 + L0126 — language lowering gaps (4 examples):** Polymorphic wildcard `_` in lambda (14, 20), non-Clone capture in andThen chain (30, 32). Both require deep lowering work; not simple kernel additions.

4. **N0004 — unknown module (3 examples):** Ws (Sky.Core.WebSocket), Responsive (Std.Ui.Responsive), Chart (Std.Ui.Chart). All need module registration + kernel families from scratch.

5. **L0108 — kernel not backed (2 examples):** Std.Sub (16), route (25). Reduced from 6 → 2 since 10611cb — significant progress.

### (d) Top-5 next fixes by unblock impact

| Rank | Fix | Examples directly unblocked | Notes |
|---|---|---|---|
| 1 | **`Time.timeString` kernel** | 2 — 02, 27 | Single missing kernel in already-registered Time module; low risk, high ROI |
| 2 | **Tui.program/Tui.app subscription scheme** (T0001) | 2 — 21, 22 | Same root cause in both; expects wire-format `{kind,value}` for typed Sub arg; fix the constrain scheme |
| 3 | **L0102 polymorphic wildcard `_` in lambda** | 2 — 14, 20 | `\_ -> Task.succeed` and `\_ -> NoOp` in Cmd.perform; needs per-wildcard fresh type var rather than shared polymorphic slot |
| 4 | **L0126 non-Clone closure capture** | 2 — 30, 32 | SSE writer forwarded across `Task.andThen (\_ -> ...)` chain; requires Clone-on-capture or explicit Arc wrapping strategy |
| 5 | **`Task.retryWith` + `Task.linearBackoff` kernels** | 1 direct — 00 (high secondary) | Task.perform cleared (simple now passes); retry family is the next gate in 00-standard-libs; unblocks real retry patterns elsewhere |

---

## Snapshot 2026-07-04 — HEAD `10611cb` (batch-111 + batch-117 + batch-119)

**Date:** 2026-07-04  
**Binary:** `/home/arthur/.cache/sky-rust-target-3/debug/skyc` (built 2026-07-04T15:35, HEAD `10611cb`)  
**Environment:** `SKY_RUNTIME_DIR=/home/arthur/Documentos/comp/sky-rust/runtime/src/sky_runtime`  
**Scope:** All 35 in-scope example directories (`examples/00` → `examples/test_pkg`).  
**Measurement:** `skyc build <sky.toml|src/Main.sky> --out <dir>/sky-out/rust` with `timeout 120`. Exit-0 = OK; anything else = FAIL with first diagnostic recorded.

> **Caveat — skyc-0 only.** OK means skyc exited 0 (canonicaliser + type-checker + emit all passed). Cargo-level compilation of the emitted Rust is a separate gate not measured here.

### (a) Full Example Table

| Example | Exit | First error code | First blocker (exact message) | Layer | Notes |
|---|---|---|---|---|---|
| 00-standard-libs | 1 | SKY-N0005 | `` `Task` has no member `perform` `` | L1b | Unchanged |
| 01-hello-world | **0** | — | — | OK | |
| 02-go-stdlib | 1 | SKY-N0005 | `` `Time` has no member `timeString` `` | L1b | Unchanged |
| 04-local-pkg | **0** | — | — | OK | |
| 06-json | 1 | SKY-L0106 | `top-level function needs a type signature` | L5 | Unchanged |
| 09-live-counter | 1 | SKY-L0106 | `update: top-level function needs a type signature` | L5 | **Advanced** — was T0001 #108; #108 fixed, now L0106 untyped-functions |
| 10-live-component | 1 | SKY-T0001 | `Counter.update: expected Main.Msg, found Counter.Msg` | L3 | Unchanged |
| 12-skyvote | 1 | SKY-N0003 | `Error kind info: unknown constructor` | L2 | **Advanced** — was N0004 `Auth` absent; #111 registered Auth, now Error ADT constructor unknown |
| 14-task-demo | 1 | SKY-L0102 | `\_ stays fully polymorphic` | L3 | Unchanged |
| 15-http-server | 1 | SKY-T0004 | `` `handleHome` has type Handler — more parameters than signature describes `` | L3 | Unchanged |
| 16-skychess | 1 | SKY-L0108 | `` `Std.Sub` import: kernel not available yet `` | L1c | **Advanced** — was N0005 `filterMap`; #119 added filterMap, now Sub.every absent |
| 17-skymon | 1 | SKY-T0001 | `expected String, found SqlValue` (bad span on import line) | L3 | Unchanged |
| 18-job-queue | 1 | SKY-T0001 | `` `Db.exec`: expected Int, found SqlValue `` | L3 | Unchanged |
| 19-skyforum | 1 | SKY-N0005 | `` `Ui` has no member `name` `` | L1b | **Advanced** — was N0004 `Region` absent; #117 added Region, now Ui.name absent |
| 20-cli-counter | 1 | SKY-L0102 | `` `\_ -> NoOp` in Cmd.perform: stays fully polymorphic `` | L3 | **Advanced** — was N0004 `Cli` absent; #111 registered Cli, now L0102 polymorphic `_` |
| 21-tui-stopwatch | 1 | SKY-T0001 | `` `Tui.program`: expected `{ kind : String, value : String }`, found String `` | L3 | Unchanged |
| 22-tui-stopwatch-ui | 1 | SKY-L0108 | `` `Ui.button` kernel not available yet `` | L1c | Unchanged |
| 23-tui-todo | 1 | SKY-N0005 | `` `Font` has no member `lineThrough` `` | L1b | Unchanged |
| 24-tui-kitchen-sink | 1 | SKY-N0005 | `` `Font` has no member `lineThrough` `` | L1b | Unchanged |
| 25-sky-console | 1 | SKY-L0108 | `blank-span kernel not available` (route kernel) | L1c | Unchanged — still L0108 on route; #108 partial |
| 26-ui-showcase | 1 | SKY-N0004 | `unknown module \`Input\`` | L1a | Unchanged |
| 27-multi-session-chat | 1 | SKY-N0005 | `` `Sub` has no member `subscribeTopic` `` | L1b | Unchanged |
| 28-streaming-chat | 1 | SKY-N0005 | `` `HttpStream` has no member `chunks` `` | L1b | **Advanced** — was N0004 `HttpStream` absent; #111 registered HttpStream, now `chunks` kernel absent |
| 29-webview-threejs-spike | 1 | SKY-T0001 | `` `Html.node`: expected a, found Html b `` | L3 | Unchanged |
| 30-sse-server-demo | 1 | SKY-L0108 | `` `Stream.stream` kernel not available yet `` | L1c | **Advanced** — was N0004 `Stream` absent; #111 registered Stream, now `stream` kernel not backed |
| 31-webview-stopwatch-ui | 1 | SKY-L0108 | `` `Ui.button` kernel not available yet `` | L1c | Unchanged |
| 32-sse-relay | 1 | SKY-L0108 | `` `ServerStream.stream` kernel not available yet `` | L1c | **Advanced** — was N0004 `ServerStream` absent; #111 registered ServerStream, now `stream` kernel not backed |
| 33-websocket-echo | 1 | SKY-N0004 | `unknown module \`Ws\`` | L1a | **Advanced** — was N0001 (triple-quoted parser bug); parser bug fixed, now Ws (Sky.Core.WebSocket) absent |
| 34-multi-tier-console | **0** | — | — | **OK** | **NEW PASS** — was T0001 #108; now fully passes |
| 37-composite-live-shop | 1 | SKY-N0004 | `unknown module \`Responsive\`` | L1a | **Advanced** — was N0004 `Region`; #117 added Region, now Responsive absent |
| 38-composite-ui-multibackend | 1 | SKY-N0004 | `unknown module \`Input\`` | L1a | **Advanced** — was N0004 `Region`; #117 added Region, now Input absent |
| simple | 1 | SKY-N0005 | `` `Task` has no member `perform` `` | L1b | Unchanged |
| spike-css-source | **0** | — | — | OK | |
| spike-std-source | **0** | — | — | OK | |
| test_pkg | **0** | — | — | OK | |

### (b) Counts

**6/35 OK** (up from 5/35 at `1806aa2`).

| Status | Count | Examples |
|---|---|---|
| skyc-0 (OK) | **6** | 01, 04, **34**, spike-css-source, spike-std-source, test_pkg |
| FAIL | **29** | all others |

#### By SKY error code

| Code | Count | Δ vs 1806aa2 | Short description |
|---|---|---|---|
| SKY-N0005 | 8 | +1 | module member absent (gained examples that advanced past N0004) |
| SKY-L0108 | 6 | +3 | kernel not available yet (registered modules, unimplemented kernels) |
| SKY-T0001 | 5 | −3 | type mismatch |
| SKY-N0004 | 4 | −5 | unknown module |
| SKY-L0106 | 2 | +1 | top-level function needs type signature |
| SKY-L0102 | 2 | +1 | polymorphic type undetermined |
| SKY-T0004 | 1 | — | more parameters than signature |
| SKY-N0003 | 1 | — | constructor not found |
| SKY-N0001 | 0 | −1 | **(cleared)** triple-quoted parser bug fixed by batch-111 |

#### By blocker layer

| Layer | Count | Description |
|---|---|---|
| **L1** | **18** | Missing kernel / module / member |
|  L1a — N0004 | 4 | Unknown module: Input (×2), Responsive, Ws |
|  L1b — N0005 | 8 | Module exists but member absent: Task.perform (×2), Font.lineThrough (×2), Time.timeString, Sub.subscribeTopic, HttpStream.chunks, Ui.name |
|  L1c — L0108 | 6 | Module registered, kernel unimplemented: Ui.button (×2), Sub.every, route, Stream.stream, ServerStream.stream |
| **L3** | **9** | Typing / semantics |
|  T0001 | 5 | SqlValue scheme (×2), cross-module ADT (×1), Tui.program scheme (×1), Html.node any-unification (×1) |
|  L0102 | 2 | Polymorphic `_` lambda (×2) |
|  T0004 | 1 | Handler head-alias unfold |
|  N0003 | 1 | Error ADT constructor pattern |
| **L5** | **2** | Language feature gap |
|  L0106 | 2 | Untyped top-level function |

### (b) Delta from `1806aa2`

| Example | Was | Now | Driver |
|---|---|---|---|
| **34-multi-tier-console** | FAIL T0001 (#108) | **OK** | #108 partial fix unblocked this example |
| 09-live-counter | FAIL T0001 (#108) | FAIL L0106 | #108 fix advanced past routing check; untyped-functions now the gate |
| 12-skyvote | FAIL N0004 (Auth) | FAIL N0003 (Error ctor) | #111 registered Std.Auth; Error ADT constructor pattern now the gate |
| 16-skychess | FAIL N0005 (filterMap) | FAIL L0108 (Sub.every) | #119 added filterMap; Sub.every now the gate |
| 19-skyforum | FAIL N0004 (Region) | FAIL N0005 (Ui.name) | #117 added Region; Ui.name now the gate |
| 20-cli-counter | FAIL N0004 (Cli) | FAIL L0102 (polymorphic `_`) | #111 registered Cli; `\_ -> NoOp` lambda type now the gate |
| 28-streaming-chat | FAIL N0004 (HttpStream) | FAIL N0005 (HttpStream.chunks) | #111 registered HttpStream; `chunks` kernel now the gate |
| 30-sse-server-demo | FAIL N0004 (Stream) | FAIL L0108 (Stream.stream) | #111 registered Stream module; `stream` kernel not backed |
| 32-sse-relay | FAIL N0004 (ServerStream) | FAIL L0108 (ServerStream.stream) | #111 registered ServerStream; `stream` kernel not backed |
| 33-websocket-echo | FAIL N0001 (parser bug) | FAIL N0004 (Ws) | Triple-quoted string parser bug fixed; Ws (WebSocket) module absent |
| 37-composite-live-shop | FAIL N0004 (Region) | FAIL N0004 (Responsive) | #117 added Region; Responsive (Std.Ui.Responsive) now the gate |
| 38-composite-ui-multibackend | FAIL N0004 (Region) | FAIL N0004 (Input) | #117 added Region; Input (Std.Ui.Input) now the gate |
| 25-sky-console | FAIL L0108 (route, blank span) | FAIL L0108 (route, blank span) | No change |
| 26-ui-showcase | FAIL N0004 (Input) | FAIL N0004 (Input) | No change |
| all other FAILs (17) | same code | same code | No change |

### (c) New dominant blocker classes

Ranked by example count for this snapshot:

1. **N0005 — missing module member (8 examples):** Task.perform (00, simple), Font.lineThrough (23, 24), Time.timeString (02), Sub.subscribeTopic (27), HttpStream.chunks (28), Ui.name (19). All in already-registered modules; need kernel implementations.

2. **L0108 — kernel registered but not backed (6 examples):** Ui.button (22, 31), Sub.every (16), route (25), Stream.stream (30), ServerStream.stream (32). Module is in the registry; the kernel dispatch entry is absent.

3. **T0001 — type mismatch (5 examples):** SqlValue return scheme (17, 18 — task #34), cross-module ADT inference (10), Tui.program Sub shape (21), Html.node `any` unification (29).

4. **N0004 — unknown module (4 examples):** Std.Ui.Input (26, 38), Std.Ui.Responsive (37), Sky.Core.WebSocket alias `Ws` (33). All need module registration + kernel family.

5. **L0106 + L0102 — language feature gaps (4 examples):** Untyped top-level functions (06, 09), polymorphic `_` in lambda (14, 20).

### (d) Top-5 next fixes by unblock impact

| Rank | Fix | Examples unblocked | Notes |
|---|---|---|---|
| 1 | **`Font.lineThrough` + `Ui.button` kernels** | 4 — 22, 23, 24, 31 | Two missing kernels in existing modules; simple kernel-registration work; each unblocks 2 examples |
| 2 | **`Task.perform` + `Task.retryWith`/`linearBackoff`** | 2 direct — 00, simple; high secondary | Task.perform is a prerequisite for async flows in nearly every real app; clearing it exposes the next layer |
| 3 | **Register `Std.Ui.Input` + implement `Input.text`/`Input.multiline` kernels** | 2 — 26, 38 | Both fail at the same N0004 `Input`; module registration + ~4 kernels |
| 4 | **`Sub.every` + `Sub.subscribeTopic` kernels** | 2 — 16 (Sub.every via L0108), 27 (subscribeTopic via N0005) | Pub/sub family; both likely in the same batch |
| 5 | **`Stream.stream` + `ServerStream.stream` + `HttpStream.chunks` kernels** | 3 — 28, 30, 32 | All three are SSE/streaming examples; #111 registered the modules, now the kernels need backing |

---

## Snapshot 2026-07-04 — HEAD `1806aa2` (task #114)

**Date:** 2026-07-04  
**Binary:** `/home/arthur/.cache/sky-rust-target-2/debug/skyc` (built 2026-07-04T02:43, HEAD `1806aa2` = task #114)  
**Environment:** `SKY_RUNTIME_DIR=/home/arthur/Documentos/comp/sky-rust/runtime/src/sky_runtime`  
**Scope:** All 35 in-scope example directories (`examples/00` → `examples/test_pkg`).  
**Measurement:** `skyc build src/Main.sky --out /tmp/parity-sweep/<name>/rust` with `timeout 120`. Exit-0 = skyc-0; anything else = FAIL with first diagnostic recorded.  

> **Caveat — skyc-0 only.** OK means skyc exited 0 (canonicaliser + type-checker + emit all passed). Cargo-level failures are a separate gate tracked by tasks #89/#94/#95/#99/#104/#112.

> **Caveat — #108 known cases.** Examples 09-live-counter and 34-multi-tier-console were documented to produce T0001 due to `#108` (RoutedLiveApp open-record). See 10611cb snapshot for updated status.

### (a) Full Example Table

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

### (b) Counts

#### By exit code

| Status | Count | Examples |
|---|---|---|
| skyc-0 (OK) | **5** | 01, 04, spike-css-source, spike-std-source, test_pkg |
| FAIL | **30** | all others |

#### By SKY error code

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

#### By blocker layer

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

### (c) Critical Path to Sweep-Green (as of 1806aa2)

Ordered by number of first-blockers each fix would remove. Second-order unblocks noted separately.

#### Fix 1 — #111: Effect stdlib modules (Std.Auth, Std.Cli, Sky.Http.Server.Stream, Sky.Core.Http.Stream)

**Direct unblock: 5 examples** → 12-skyvote, 20-cli-counter, 28-streaming-chat, 30-sse-server-demo, 32-sse-relay

All five fail with N0004 because their primary import is to one of these absent modules. The compiled-source stdlib (`crates/skyc/stdlib/`) contains no `Std/Cli.sky`, no `Sky/Http/Server/Stream.sky`, no `Sky/Core/Http/Stream.sky`, no `Std/Auth.sky` — confirmed by `find`. Task #111 is the filed work item; it covers Cli, both Stream variants, WebSocket, and Auth as a batch.

Note on Std.Cli specifically: example 20-cli-counter has the explicit `import Std.Cli as Cli` import statement, so this is genuinely an absent module, not a missing qualifier registration.

#### Fix 2 — New task: Register Std.Ui.Region + Std.Ui.Input

**Direct unblock: 4 examples** → 19-skyforum, 26-ui-showcase, 37-composite-live-shop, 38-composite-ui-multibackend

Three examples (19, 37, 38) fail on N0004 for `Region` (imported as `Std.Ui.Region`). Example 26 fails on N0004 for `Input` (Std.Ui.Input). Neither module appears in the compiled-source stdlib or kernel registry. This is NOT covered by any existing pending task — it needs a new issue. Likely the same implementation pattern as other Std.Ui sub-modules.

#### Fix 3 — #108: RoutedLiveApp (routes/notFound row-poly cfg + route kernel)

**Direct unblock: 3 examples** → 09-live-counter, 25-sky-console, 34-multi-tier-console

- 09 and 34: T0001 because `app`'s constrain scheme always expects `routes : List LiveRoute` + `notFound : Page` fields in the cfg record, but both examples provide the non-routed shape. Fixing #108 (open row-poly cfg) makes these fields optional.
- 25-sky-console: provides routes + notFound but gets L0108 on the `route` kernel itself (backed by #108's implementation).

#### Fix 4 — Task.perform + retry kernels (N0005)

**Direct unblock: 2 examples first-blocker** → 00-standard-libs, simple

Both fail immediately on `Task.perform`. However, fixing this is a **prerequisite for many other examples' secondary blockers** — nearly every CLI, Live, and Tui app eventually calls `Task.perform` or `Task.run`. Second-order impact is high. The retry family (`retryWith`, `linearBackoff`, `exponentialBackoff`, `withJitter`) and the two Std.Time members (`isLeapYear`, `daysInMonth`) as well as `Std.Decimal` and `Std.Money` are further blockers in 00-standard-libs after this first one clears.

#### Fix 5 — Ui.button + Font.lineThrough + Sub.subscribeTopic kernels

**Direct unblock: 5 examples** → 22-tui-stopwatch-ui (Ui.button), 23-tui-todo (Font.lineThrough), 24-tui-kitchen-sink (Font.lineThrough), 27-multi-session-chat (Sub.subscribeTopic), 31-webview-stopwatch-ui (Ui.button)

These are three independent L1b/L1c gaps that each block specific examples. Batching them in one PR would be efficient.

---

#### Remaining blockers (1 example each)

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

### Summary of impact by fix

| Fix | Examples directly unblocked |
|---|---|
| #111 (effect stdlib modules) | 5 |
| Std.Ui.Region + Std.Ui.Input (new task) | 4 |
| #108 (RoutedLiveApp) | 3 |
| Task.perform + retry family | 2 (many secondary) |
| Ui.button + Font.lineThrough + Sub.subscribeTopic | 5 |
| **Top 5 combined** | **19 of 30** |

---

### How many examples did #108 unblock? (as of 1806aa2)

**3 examples** (09-live-counter, 25-sky-console, 34-multi-tier-console).

- 09 and 34 are the canonical #108 T0001 cases where `app` requires `routes/notFound` always.
- 25-sky-console provides `routes/notFound` and passes the type check but hits L0108 on the `route` kernel, which is part of #108's implementation scope.
- 10-live-component also shows T0001 but the error is "Counter.Msg vs Main.Msg" — a different inference bug unrelated to #108.

---

### Sweep environment notes

- Binary HEAD: commit `1806aa2` (task #114, unary-negation parser support). All tasks up to and including #114 are in the binary.
- Tasks #78 and #80 (register Cli/Stream/ServerStream/HttpStream in canon) are marked completed, but the sweep confirms `Std.Cli`, `Sky.Http.Server.Stream`, and `Sky.Core.Http.Stream` are still absent from both the compiled-source stdlib and the kernel registry. These remain open as the first-blockers for examples 20, 28, 30, 32. Task #111 is the appropriate owner.
- `Std.Ui.Region` and `Std.Ui.Input` absence is a new gap not covered by any existing pending task.
- The triple-quoted string parser bug (example 33) is new — a `"` inside `"""..."""` terminates the outer string, causing the inner HTML text to be parsed as Sky identifiers.
- The `Db.exec` / `Db.*` SqlValue scheme mismatch (examples 17, 18) correlates to task #34 (SqlValue 7→9 variants + exhaustive emit_db_call). The return type of `Db.exec` in the constrain scheme emits `Task Error SqlValue` instead of `Task Error Int`, causing a downstream type clash.

---

*Generated by read-only sweep — no source files modified.*

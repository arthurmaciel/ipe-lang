# Kernel Registration Backlog — N0004 / N0005 Sweep Frontier

**Date:** 2026-07-04  
**Author:** Doc Lane (read-only research pass)  
**Scope:** All examples blocked by `SKY-N0004` (unknown module) or `SKY-N0005`
(module has no such member) as of the current sweep run.  
**Cross-reference:** every symbol verified against `../sky/sky-stdlib/` or
`../sky/runtime-go/rt/` before being listed.

---

## 1. Per-Example Blocker Table

| Example | Error | Primary Blocker(s) | Upstream Source |
|---|---|---|---|
| `12-skyvote` | N0004 | `Std.Auth` | `sky-stdlib/Std/Auth.sky` |
| `19-skyforum` | N0004 | `Std.Ui.Region` | `sky-stdlib/Std/Ui/Region.sky` |
| `20-cli-counter` | N0004 | `Std.Cli` | `runtime-go/rt/cli.go` (no .sky source) |
| `26-ui-showcase` | N0004 | `Std.Ui.Input`, `Std.Ui.Transition`, `Std.Ui.Animation`, `Std.Ui.Transform`, `Std.Ui.Grid`, `Std.Ui.Chart` | `sky-stdlib/Std/Ui/{Input,Transition,Animation,Transform,Grid,Chart}.sky` |
| `28-streaming-chat` | N0004 | `Sky.Core.Http.Stream` | `sky-stdlib/Sky/Core/Http/Stream.sky` |
| `30-sse-server-demo` | N0004 | `Sky.Http.Server.Stream` | `sky-stdlib/Sky/Http/Server/Stream.sky` |
| `32-sse-relay` | N0004 | `Sky.Http.Server.Stream` + `Sky.Core.Http.Stream` | both above |
| `38-composite-ui-multibackend` | N0004 | `Std.Live.Head` | `sky-stdlib/Std/Live/Head.sky` |
| `00-standard-libs` | N0004+N0005 | `Std.Decimal`, `Std.Money` (N0004); `Task.perform`, `Task.retryWith`, `Task.linearBackoff`, `Task.exponentialBackoff`, `Task.withJitter`, `Stime.isLeapYear`, `Stime.daysInMonth` (N0005) | `sky-stdlib/Std/{Decimal,Money,Time}.sky`, `sky-stdlib/Sky/Core/Task.sky` |
| `02-go-stdlib` | N0005 | `Time.timeString` | `sky-stdlib/Sky/Core/Time.sky` (Go kernel) |
| `16-skychess` | N0005 | `Error.ErrorKind(..)` (rich ADT) | `sky-stdlib/Sky/Core/Error.sky` (task #85) |
| `23-tui-todo` | N0005 | `Font.lineThrough` | `sky-stdlib/Std/Ui/Font.sky` |
| `24-tui-kitchen-sink` | N0005 | `Font.lineThrough` | `sky-stdlib/Std/Ui/Font.sky` |
| `27-multi-session-chat` | N0005 | `Sub.subscribeTopic`, `Cmd.publish`, `Time.timeString` | enum reserved variants + Go kernel |
| `simple` | N0005 | `Task.perform` | `sky-stdlib/Sky/Core/Task.sky` |

**Total affected examples:** 15 (8 × N0004, 7 × N0005)

---

## 2. Deduped Missing Symbols — Grouped by Module and Classification

### Classification key

- **KERNEL** — needs a new `StdlibKernel` enum variant, a constrain-scheme entry, a canon
  qualifier registration, and a runtime emit function. Lane A work.
- **COMPILED-SOURCE** — pure Sky wrapper over existing kernels; goes into
  `crates/skyc/stdlib/*.sky` via the task #98 pipeline. Lane B work.
- **ALIAS-GAP** — the `StdlibKernel` variant already exists in the enum (reserved for M6)
  but is excluded from `StdlibKernel::ALL` and has no canon qualifier entry. The gap is
  wiring, not implementation. Lane A (light wire-up) work.
- **RICH-ADT** — tracked separately under task #85; involves redesigning the `Error` type
  hierarchy. Heavy, architectural.

---

### Module: `Sky.Core.Task` → qualifier `Task`

| Missing member | Upstream signature | Classification | Size |
|---|---|---|---|
| `perform` | `Task e a -> (Result e a -> msg) -> Cmd msg` | KERNEL | thin |
| `retryWith` | `RetryPolicy e -> Task e a -> Task e a` | KERNEL | medium |
| `linearBackoff` | `RetryPolicy e` (builder) | KERNEL | medium |
| `exponentialBackoff` | `RetryPolicy e` (builder) | KERNEL | medium |
| `withJitter` | `RetryPolicy e -> RetryPolicy e` | KERNEL | medium |

**Note on `Task.perform`:** In Sky convention `Cmd.perform` is the normal dispatch;
`Task.perform` with a 1-arg arity-1 form (no `toMsg`) appears in `simple/src/Main.sky`
and `00-standard-libs`. Both resolve to the same runtime operation. Whether this should
be a single kernel registered under both `Cmd` and `Task` qualifiers, or a distinct
`Task.perform` variant, must be confirmed before implementation. Cheapest: alias under
both qualifiers pointing to the same variant.

---

### Module: `Sky.Core.Time` → qualifier `Time`

| Missing member | Upstream signature | Classification | Size |
|---|---|---|---|
| `timeString` | `Int -> String` (formats a Unix-ms Int) | KERNEL | thin |

---

### Module: `Std.Time` → qualifier `Time` (same qualifier, separate module)

| Missing member | Upstream signature | Classification | Size |
|---|---|---|---|
| `isLeapYear` | `Int -> Bool` | KERNEL | thin |
| `daysInMonth` | `Int -> Int -> Result Error Int` | KERNEL | thin |

**Note:** `Std.Time` maps to the same `"Time"` qualifier as `Sky.Core.Time` in the canon
table. These members simply need additional kernel variants added to the Time group.

---

### Module: `Std.Ui.Font` → qualifier `Font`

| Missing member | Upstream signature | Classification | Size |
|---|---|---|---|
| `lineThrough` | `Attribute msg` (text-decoration: line-through) | KERNEL | thin |

Confirmed absent from `StdlibKernel` enum (no `FontLineThrough` variant).  
Blocks: `23-tui-todo`, `24-tui-kitchen-sink`.

---

### Module: `Std.Cmd` → qualifier `Cmd`

| Missing member | Upstream signature | Classification | Size |
|---|---|---|---|
| `publish` | `String -> Dict String String -> Cmd msg` | ALIAS-GAP | thin |
| `publishNoEcho` | `String -> Dict String String -> Cmd msg` | ALIAS-GAP | thin |

`CmdPublish` and `CmdPublishNoEcho` variants exist in `StdlibKernel` but are excluded
from `StdlibKernel::ALL`. The `"Cmd"` qualifier IS registered in `STDLIB_MODULE_QUALIFIERS`.
Wire-up only: add to `ALL` + constrain scheme entry.  
Blocks: `27-multi-session-chat`.

---

### Module: `Std.Sub` → qualifier `Sub`

| Missing member | Upstream signature | Classification | Size |
|---|---|---|---|
| `subscribeTopic` | `String -> (Dict String String -> msg) -> Sub msg` | ALIAS-GAP | thin |

`SubSubscribeTopic` exists in `StdlibKernel` but excluded from `ALL`. The `"Sub"`
qualifier IS registered. Wire-up only.  
Blocks: `27-multi-session-chat`.

---

### Module: `Sky.Core.Error` → qualifier `Error`

| Missing member / construct | Upstream signature | Classification | Size |
|---|---|---|---|
| `ErrorKind` ADT + constructors | `type ErrorKind = Io \| Network \| Ffi \| ...` | RICH-ADT | heavy |
| `ErrorInfo` / `ErrorDetails` | companion record types | RICH-ADT | heavy |

Tracked as **task #85**. The rich ADT is a full architectural change to `Error` (currently
an opaque type). The `16-skychess` import `exposing (Error(..), ErrorKind(..))` requires
`ErrorKind` constructors to be user-visible.

---

### Module: `Std.Live.Head` → qualifier `Live.Head` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| Entire module | `title, meta, metaProperty, link, canonical, jsonLd, themeColor, rss` | COMPILED-SOURCE | thin |

All 8 functions are thin wrappers over `Html.node` / `Html.a` calls. The upstream
`Std/Live/Head.sky` is pure Sky. This is a Lane B job: copy + compile via task #98 pipeline.  
Blocks: `38-composite-ui-multibackend`.

---

### Module: `Std.Ui.Region` → qualifier `Region` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| Entire module | `mainContent, navigation, footer, aside, heading, label, announce, announceUrgently` | KERNEL | thin |

Semantic HTML landmark attribute builders (`role=`, `aria-*`). Small fixed set, all emit
attribute structs. Thin but needs 8 new kernel variants.  
Blocks: `19-skyforum`.

---

### Module: `Std.Ui.Input` → qualifier `Input` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| Entire module | `button, text, multiline, email, username, search, currentPassword, newPassword, checkbox, radio, radioRow, slider` | KERNEL | medium |

Typed form control builders. 12 functions emitting `Element msg` with wrapper+label
structure. Medium: each control has distinct attribute routing (wrapper vs inner `<input>`).  
Blocks: `26-ui-showcase`.

---

### Module: `Std.Ui.Transition` → qualifier `Transition` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| Entire module | `attribute, attributeUnsafe, property, duration, easing, easeIn, easeOut, easeInOut, linear` | KERNEL | medium |

CSS transition builder. Emits `<style data-sky-tr=...>` sibling nodes. Medium: needs
interaction with the `<style>` injection mechanism.  
Blocks: `26-ui-showcase`.

---

### Module: `Std.Ui.Animation` → qualifier `Animation` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| Entire module | `attribute, Spec, keyframes`, duration/easing/delay/iterations/fillMode/respectReducedMotion fields | KERNEL | medium |

CSS keyframe animation builder. Interacts with `@keyframes` generation and the sky-id
scoped `<style data-sky-anim=...>` injection. Similar complexity to Transition.  
Blocks: `26-ui-showcase`.

---

### Module: `Std.Ui.Transform` → qualifier `Transform` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| Entire module | `translateX, translateY, translate, scale, scaleXY, rotate, skewX, skewY, opacity` | KERNEL | thin |

Pure CSS `transform:` property value builders. No emission mechanism of their own —
they feed into `Animation.keyframes`. Thin.  
Blocks: `26-ui-showcase`.

---

### Module: `Std.Ui.Grid` → qualifier `Grid` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| Entire module | `tracks, columns, rows, fr, px, auto, minContent, maxContent, minmax, repeat, repeatAutoFit, repeatAutoFill` | KERNEL | thin |

CSS Grid track-list builders. Emit inline-style via the existing `AttrStyle` channel.
12 functions, no new injection mechanism needed (AttrStyle already works).  
Blocks: `26-ui-showcase`.

---

### Module: `Std.Ui.Chart` → qualifier `Chart` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| Entire module | SVG chart rendering (bar, line, pie, etc.) | KERNEL | heavy |

SVG rendering, axis computation, legend management. The heaviest Ui sub-module.  
Blocks: `26-ui-showcase`.

---

### Module: `Std.Auth` → qualifier `Auth` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| Entire module | `register, login, setRole, hashPassword, hashPasswordCost, verifyPassword, passwordStrength, signToken, verifyToken, signTokenWithClaims, verifyTokenWithAlgorithm` | KERNEL | heavy |

Effect-heavy: bcrypt, JWT, database writes. Tracked under **task #111** (effect modules).  
Blocks: `12-skyvote`.

---

### Module: `Std.Cli` → qualifier `Cli` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| Entire module | TEA CLI backend (`program`, `run`, `readLine`, `readPassword`, `exit`) | KERNEL | heavy |

No Sky source. Implemented entirely in `runtime-go/rt/cli.go`. The Rust port needs a
full TEA-CLI driver. Tracked under **task #111**.  
Blocks: `20-cli-counter`.

---

### Module: `Sky.Core.Http.Stream` → qualifier `Http.Stream` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| `StreamId`, `ChunkEvent(..)`, `open`, `chunks`, `close`, `forEachChunk` | ADT + Task effects | KERNEL | heavy |

Streaming HTTP client (upstream chunked reads as `Sub` events). Tracked under **task #111**.  
Blocks: `28-streaming-chat`, `32-sse-relay`.

---

### Module: `Sky.Http.Server.Stream` → qualifier `Server.Stream` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| `stream, emit, finish, withContentType` | server-side SSE / chunked write | KERNEL | heavy |

Server-side streaming HTTP response writer. Tracked under **task #111**.  
Blocks: `30-sse-server-demo`, `32-sse-relay`.

---

### Module: `Std.Decimal` → qualifier `Decimal` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| Entire module | 30+ functions: `fromString, fromInt, round, toStringFixed, add, sub, mul, div`, etc. | KERNEL | heavy |

Arbitrary-precision arithmetic backed by `shopspring/decimal` in Go. Runtime parity
work: must match two distinct rounding strategies (see `Agent learnings` in CLAUDE.md).  
Blocks: `00-standard-libs`.

---

### Module: `Std.Money` → qualifier `Money` (new)

| Missing | Upstream | Classification | Size |
|---|---|---|---|
| Entire module | `Money(..), Currency(..)`, `fromMinor, fromMajor, add, sub, allocate, format`, 44+ entries | KERNEL | heavy |

Currency-typed arithmetic on `Decimal`. Depends on `Std.Decimal` being registered first.  
Blocks: `00-standard-libs`.

---

## 3. Deduped Missing Symbol Count

| Category | Count |
|---|---|
| Missing module registrations (N0004) | 14 modules |
| Missing member registrations in already-registered modules (N0005) | 13 members |
| **Total distinct gaps** | **27** |

Breakdown by classification:

| Classification | Modules/Items |
|---|---|
| ALIAS-GAP (wire-up only) | 3 members (`Cmd.publish`, `Cmd.publishNoEcho`, `Sub.subscribeTopic`) |
| COMPILED-SOURCE (Lane B) | 1 module (`Std.Live.Head`) |
| KERNEL — thin | 7 items (`Font.lineThrough`, `Time.timeString`, `Time.isLeapYear`, `Time.daysInMonth`, `Std.Ui.Region`, `Std.Ui.Transform`, `Std.Ui.Grid`) |
| KERNEL — medium | 7 items (`Task.perform/retryWith/linearBackoff/exponentialBackoff/withJitter`, `Std.Ui.Input`, `Std.Ui.Transition`, `Std.Ui.Animation`) |
| KERNEL — heavy | 8 items (`Std.Auth`, `Std.Cli`, `Sky.Core.Http.Stream`, `Sky.Http.Server.Stream`, `Std.Decimal`, `Std.Money`, `Std.Ui.Chart`, `Error.ErrorKind`) |
| RICH-ADT (task #85) | 1 item (`Error.ErrorKind` + companions) |

---

## 4. Suggested Execution Order — Cheapest-Highest-Impact First

The ordering criterion: (unblocked examples gained) × (effort cost)^-1.  
Alias-gaps and thin kernels first; heavy architectural work last.

### Tier 1 — Immediate wins (wire-up / thin, unblocks multiple examples)

| Priority | Item | Classification | Unblocks | Estimated effort |
|---|---|---|---|---|
| 1 | `Time.timeString` | KERNEL thin | `02-go-stdlib`, `27-multi-session-chat` | 1–2 h |
| 2 | `Cmd.publish`, `Cmd.publishNoEcho` | ALIAS-GAP | `27-multi-session-chat` (partial) | 1 h |
| 3 | `Sub.subscribeTopic` | ALIAS-GAP | `27-multi-session-chat` (completes) | 30 min |
| 4 | `Font.lineThrough` | KERNEL thin | `23-tui-todo`, `24-tui-kitchen-sink` | 1 h |
| 5 | `Time.isLeapYear`, `Time.daysInMonth` | KERNEL thin | `00-standard-libs` (partial) | 2 h |

After Tier 1: `02-go-stdlib`, `23-tui-todo`, `24-tui-kitchen-sink`, `27-multi-session-chat` are
fully unblocked (4 examples with ~6 h of work).

### Tier 2 — Thin new modules (new qualifier + small kernel set)

| Priority | Item | Classification | Unblocks | Estimated effort |
|---|---|---|---|---|
| 6 | `Std.Ui.Transform` | KERNEL thin | `26-ui-showcase` (partial) | 2 h |
| 7 | `Std.Ui.Grid` | KERNEL thin | `26-ui-showcase` (partial) | 2–3 h |
| 8 | `Std.Ui.Region` | KERNEL thin | `19-skyforum` | 3 h |
| 9 | `Std.Live.Head` | COMPILED-SOURCE | `38-composite-ui-multibackend` | 2 h |

After Tier 2: `19-skyforum`, `38-composite-ui-multibackend` fully unblocked.
`26-ui-showcase` partially unblocked (still needs Input/Transition/Animation/Chart).

### Tier 3 — Medium kernels (new modules, non-trivial logic)

| Priority | Item | Classification | Unblocks | Estimated effort |
|---|---|---|---|---|
| 10 | `Task.perform` (+retryWith/backoff variants) | KERNEL medium | `simple`, `00-standard-libs` (partial) | 1 day |
| 11 | `Std.Ui.Input` | KERNEL medium | `26-ui-showcase` (partial) | 1–2 days |
| 12 | `Std.Ui.Transition` | KERNEL medium | `26-ui-showcase` (partial) | 1 day |
| 13 | `Std.Ui.Animation` | KERNEL medium | `26-ui-showcase` (mostly done after 11–13) | 1 day |

After Tier 3: `simple` fully unblocked. `26-ui-showcase` unblocked except `Chart`.

### Tier 4 — Heavy modules (architectural / multi-day effort)

| Priority | Item | Classification | Unblocks | Task ref |
|---|---|---|---|---|
| 14 | `Std.Decimal` | KERNEL heavy | `00-standard-libs` (partial) | — |
| 15 | `Std.Money` | KERNEL heavy | `00-standard-libs` (completes) | — |
| 16 | `Std.Cli` | KERNEL heavy | `20-cli-counter` | task #111 |
| 17 | `Sky.Core.Http.Stream` | KERNEL heavy | `28-streaming-chat`, `32-sse-relay` | task #111 |
| 18 | `Sky.Http.Server.Stream` | KERNEL heavy | `30-sse-server-demo`, `32-sse-relay` | task #111 |
| 19 | `Std.Auth` | KERNEL heavy | `12-skyvote` | task #111 |

### Tier 5 — Architectural (requires design-ahead work)

| Priority | Item | Classification | Unblocks | Task ref |
|---|---|---|---|---|
| 20 | `Error.ErrorKind` + full rich-ADT | RICH-ADT heavy | `16-skychess` | task #85 |
| 21 | `Std.Ui.Chart` | KERNEL heavy | `26-ui-showcase` (final piece) | — |

---

## 5. Top-5 Highest-Impact Registrations

Ranked by (examples unblocked) × (implementation cheapness):

1. **`Time.timeString`** — 1–2 h kernel addition, unblocks 2 examples (`02-go-stdlib`,
   `27-multi-session-chat`). The function is a pure format call (Go kernel already
   exists as `TimeTimeString` — just add the variant and wire constrain/emit).

2. **`Cmd.publish` + `Cmd.publishNoEcho` + `Sub.subscribeTopic`** (3 alias-gaps as one
   unit) — ~1.5 h total wire-up, completes unblocking of `27-multi-session-chat`. The
   variants exist in the enum; only `ALL` membership + constrain scheme entry needed.

3. **`Font.lineThrough`** — 1 h new kernel variant, unblocks `23-tui-todo` AND
   `24-tui-kitchen-sink`. Pure CSS text-decoration attribute; trivially mirrors
   `FontUnderline`/`FontNoDecoration`.

4. **`Std.Ui.Region`** — 3 h thin new module (8 variants), unblocks `19-skyforum`.
   All functions emit semantic `role=` / `aria-*` HTML attributes, structurally
   identical to existing `Background`/`Border` attribute emitters.

5. **`Std.Live.Head`** — 2 h Lane B job: copy `Std/Live/Head.sky` from upstream into
   `crates/skyc/stdlib/Std/Live/Head.sky`, add to `COMPILED_STD_MODULES`, register
   `["Std", "Live", "Head"]` → `"Live.Head"` in `STDLIB_MODULE_QUALIFIERS`. Unblocks
   `38-composite-ui-multibackend` and brings a widely-used SEO feature into the sweep.

---

## 6. Lane Grouping

### Lane A — Kernel implementation (crates/sky_kernels, crates/sky_canon, runtime)

All `KERNEL` and `ALIAS-GAP` items. Work order follows the tier table above.

**Quick wins within Lane A** (no new module, no new qualifier):
- `Time.timeString` (TimeTimeString variant)
- `Cmd.publish`, `Cmd.publishNoEcho` (wire existing variants into ALL)
- `Sub.subscribeTopic` (wire existing variant into ALL)
- `Font.lineThrough` (FontLineThrough variant)
- `Time.isLeapYear`, `Time.daysInMonth` (two new Time variants)

**New qualifier registrations required** (add to `STDLIB_MODULE_QUALIFIERS`):
- `["Std", "Ui", "Region"]` → `"Region"`
- `["Std", "Ui", "Input"]` → `"Input"`
- `["Std", "Ui", "Transition"]` → `"Transition"`
- `["Std", "Ui", "Animation"]` → `"Animation"`
- `["Std", "Ui", "Transform"]` → `"Transform"`
- `["Std", "Ui", "Grid"]` → `"Grid"`
- `["Std", "Ui", "Chart"]` → `"Chart"`
- `["Std", "Auth"]` → `"Auth"`
- `["Std", "Decimal"]` → `"Decimal"`
- `["Std", "Money"]` → `"Money"`
- `["Sky", "Core", "Http", "Stream"]` → `"Http.Stream"`
- `["Sky", "Http", "Server", "Stream"]` → `"Server.Stream"`
- `["Std", "Live", "Head"]` → `"Live.Head"` *(also needed for Lane B)*

**Deferred to task #111** (effect-module architecture required first):
- `Std.Cli` (TEA-CLI driver, no Sky source)
- `Sky.Core.Http.Stream` (streaming HTTP client)
- `Sky.Http.Server.Stream` (streaming HTTP server)
- `Std.Auth` (bcrypt/JWT/DB)

**Deferred to task #85** (rich-ADT redesign required):
- `Error.ErrorKind` + `ErrorInfo` + `ErrorDetails`

### Lane B — Compiled-source stdlib modules (crates/skyc/stdlib/, task #98)

All `COMPILED-SOURCE` items. These are pure Sky; they flow through the normal
compile pipeline with no new kernel variants required.

| Module | Sky source location | Qualifier to register |
|---|---|---|
| `Std.Live.Head` | `sky-stdlib/Std/Live/Head.sky` | `"Live.Head"` |

No other modules in the current blocker set qualify as compiled-source.
`Std.Ui.Region`, `Std.Ui.Input`, etc., use FFI-kernel primitives in the upstream
implementation; they cannot be compiled from Sky source without those primitives
first registered.

---

## 7. Cross-References to Open Tasks

| Task | Relevant blockers |
|---|---|
| **#85** (Error rich-ADT) | `Error.ErrorKind`, `Error.ErrorInfo`, `Error.ErrorDetails` — blocks `16-skychess` |
| **#98** (compiled-source stdlib) | `Std.Live.Head` — the only pure-Sky module in this backlog |
| **#111** (effect modules) | `Std.Auth`, `Std.Cli`, `Sky.Core.Http.Stream`, `Sky.Http.Server.Stream` |

Tasks #77 (Log.*With / Debug.toString) and the M6 wiring (pub/sub) contribute to
the ALIAS-GAP items (`Cmd.publish`, `Cmd.publishNoEcho`, `Sub.subscribeTopic`).

---

*Document generated by read-only Doc Lane pass. No crate edits were made.*

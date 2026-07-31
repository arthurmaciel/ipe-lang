# Extensions to Elm

> **Framing.** Ipê is an **Elm-family** language: it inherits Elm's core syntax,
> Hindley–Milner discipline, and The Elm Architecture. The goal is to **cover as
> much of the official `elm/*` package set as is relevant to Ipê's target**, and
> to keep every departure **small, deliberate, and documented**. This is the
> single top-level ledger. It has two jobs:
>
> 1. A **coverage matrix** — every `elm/*` module mapped to a per-value status:
>    `present` (with the Ipê name), `diverged` (how + why), `missing` (a real
>    gap), or `excluded` (irrelevant to Ipê's target, with the justification).
> 2. A **narrative of the deliberate departures** — the effect model, the
>    server-driven runtime, the typed-`Error` mandate, and the native stdlib
>    that Elm core has no counterpart for.
>
> The prioritized plan for closing the `missing` rows lives in
> [`gap-plan.md`](gap-plan.md). The exhaustive
> per-value `elm/core` table lives in
> [`elm-core-coverage.md`](elm-core-coverage.md); this doc summarizes it
> and extends the audit to the rest of the `elm/*` org.

Ipê's target — typed Rust producing server, native, TUI, and desktop binaries —
differs from Elm's (a sandboxed browser client). Most divergences fall out of
that one root fact. Elm's choices are correct for Elm's target; the departures
below are stated as engineering trade-offs, not corrections.

---

## 1. Coverage at a glance

| `elm/*` package | Ipê relationship | Verdict |
|---|---|---|
| `elm/core` | Broad coverage; `List`/`Dict`/`Set`/`String`/`Maybe`/`Result`/`Task`/`Cmd`/`Sub` present, few remaining gaps | **partial** — see §2 |
| `elm/json` | Role filled by `Ipe.Config` decoders (TOML/YAML/JSON in one surface) | **diverged** — §3.1 |
| `elm/time` | `Ipe.Time` — reshaped (server clock, IANA zones, formatting), no `Posix`/`Zone` ADTs | **diverged** — §3.2 |
| `elm/random` | `Ipe.Random` — `Task`-based generation + a seeded pure surface, not `Generator`/`step` | **diverged** — §3.3 |
| `elm/http` | `Ipe.Http` — builder-style client `Task`; server side is `Ipe.Http.Server` | **diverged** — §3.4 |
| `elm/url` | Only `Http.parseQuery`; no `Url`/`Url.Builder`/`Url.Parser` | **mostly missing** — §3.5 |
| `elm/bytes` | `Ipe.Bytes` — value + hex/base64 codecs, no `Bytes.Decode`/`Encode` combinators | **diverged** — §3.6 |
| `elm/file` | `Ipe.File` — full native filesystem, not the browser `File`/`Download`/`Select` model | **diverged** — §3.7 |
| `elm/regex` | `Ipe.Regex` — built-in (Elm 0.19 de-blessed it), reshaped surface | **diverged** — §3.8 |
| `elm/parser` | No counterpart | **missing** — §3.9 |
| `elm/html` | No counterpart; replaced by `Ipe.Ui` (elm-ui-derived) | **excluded** — §4 |
| `elm/svg` | No counterpart | **excluded** — §4 |
| `elm/virtual-dom` | Internal to the runtimes, not a user surface | **excluded** — §4 |
| `elm/browser` | No counterpart; replaced by `Ipe.Web` / `Ipe.Tui` / `Ipe.Webview` | **excluded** — §4 |

Headline `elm/core` count (from [`elm-core-coverage.md`](elm-core-coverage.md),
264 exposed values across 17 modules): **185 present · 15 diverged · 61 missing ·
3 n/a**, plus 13 of 20 exposed types present. (The `List`/`Dict`/`Set`/`Result`/
`Char`/`String` pure-`elm/core` fills closed the bulk of the former gap; the
remaining `missing` rows are `Dict.merge` and the whole absent modules — see §2.)

---

## 2. `elm/core` — module-by-module

Statuses are `✓ present` / `~ diverged` / `✗ missing` / `n/a excluded`. The
authoritative per-value table with signatures is
[`elm-core-coverage.md`](elm-core-coverage.md); only the summary and the
decisions live here.

| Module | Present | Diverged | Missing | Notes |
|---|---|---|---|---|
| `Basics` | 34 | 6 | 15 | numerics live in `Ipe.Math`, not the auto-prelude |
| `List` | 37 | 0 | 0 | complete — `sum`/`product`/`maximum`/`minimum`/`singleton`/`repeat`/`intersperse`/`partition`/`unzip`/`sort`/`sortWith`/`map2`–`map5` now present |
| `Dict` | 21 | 0 | 1 | `update`/`singleton`/`foldr`/`filter`/`partition`/`intersect`/`diff` now present; still missing `merge` |
| `Set` | 17 | 0 | 0 | complete — `isEmpty`/`singleton`/`foldl`/`foldr`/`map`/`filter`/`partition` now present |
| `String` | 41 | 2 | 1 | code-point semantics (§5); `left`/`right`/`cons`/`uncons`/`pad`/`indexes` + the char-fold family now present |
| `Maybe` | 7 | 0 | 0 | complete |
| `Result` | 10 | 0 | 0 | complete — `toMaybe`/`fromMaybe` bridges now present |
| `Char` | 9 | 2 | 2 | `toUpper`/`toLower` return `String`; `isAlphaNum`/`isHexDigit`/`isOctDigit` now present |
| `Task` | 7 | 1 | 0 | fixed `Error` channel (§6); `map2..5` + `attempt` now present |
| `Platform.Cmd` | 3 | 0 | 0 | complete — `Cmd.map` now present |
| `Platform.Sub` | 3 | 0 | 0 | complete — `Sub.map` now present |
| `Array` | 0 | 0 | 18 | **whole module absent** |
| `Bitwise` | 0 | 0 | 7 | **whole module absent** |
| `Tuple` | 0 | 2 | 4 | `fst`/`snd` in `Basics`; module absent |
| `Debug` | 0 | 1 | 2 | no dev-helper module; `Ipe.Log` is a `Task` logger |
| `Platform` | 0 | 0 | 0 (3 n/a) | effect-manager plumbing — excluded (§6) |
| `Process` | 0 | 1 | 2 | only `sleep` (as `Time.sleep`); no `spawn`/`kill`/`ProcessId` |

### Decisions carried in `elm/core`

- **Numerics namespace.** `round`/`floor`/`sqrt`/trig/`e`/`pi` live under
  `Ipe.Math` (renamed `ceiling→ceil`, `truncate→trunc`), not the zero-import
  `Basics`. `Basics.compare`/`modBy`/`negate` are registered qualifiers with no
  typed kernel arm today. The ambient-`Basics` set (which numerics/math are
  auto-imported unqualified) is fixed by
  [`docs/adr/0047-basics-and-tiered-auto-import.md`](../adr/0047-basics-and-tiered-auto-import.md).
- **`Order` ADT.** `compare` returns a three-way result; `Order` with
  `LT`/`EQ`/`GT` is part of the ambient core surface fixed by ADR 0047.
  `List.sortWith` is a kernel.
- **No composition/power operators.** `(>>)`, `(<<)`, `(^)` are absent; only the
  `(|>)`/`(<|)` pipes exist.

---

## 3. Reshaped `elm/*` packages (the deliberate departures)

Each of these is a `diverged` verdict: Ipê covers the package's *role* but with a
surface shaped for the server/native target. The rule is **small and documented**
— a reviewer should be able to see both the divergence and its justification.

### 3.1 `elm/json` → `Ipe.Config`

Elm splits decoding (`Json.Decode`, ~30 values) and encoding (`Json.Encode`).
Ipê's `Ipe.Config` is a single decoder surface — `string`/`int`/`float`/`bool`/
`nullable`/`field`/`at`/`list`/`succeed`/`fail`/`map`/`andThen` — that decodes
**JSON, TOML, and YAML** through the same `Decoder a` (`decodeJson`/`decodeToml`/
`decodeYaml`/`loadFromFile`).

- **Present (renamed):** the decoder combinator core (`Decoder`, `string`, `int`,
  `float`, `bool`, `nullable`, `field`, `at`, `list`, `succeed`, `fail`, `map`,
  `andThen`, `map2..8`, `oneOf`, `maybe`, `index`, `keyValuePairs`, `dict`).
- **Missing:** `array`, `lazy`, `value`/`Value`, `decodeValue`, `errorToString`,
  `oneOrMore`.
- **Missing (whole surface):** `Json.Encode` — there is no first-class typed JSON
  *encoder* value surface; serialization is per-effect (`Http` body helpers, DB).
- **Justification for the merge:** config-file decoding (TOML/YAML/JSON) is the
  dominant server use case; one `Decoder` over all three avoids three parallel
  APIs. The load-bearing `map2..8`/`oneOf`/`maybe`/`index`/`keyValuePairs`/`dict`
  combinator set is now present, closing the record/union-decoding gap.

Two runtime **behaviour** divergences (audited in
[`behaviour-verdicts.md`](behaviour-verdicts.md)):

- **`int` is strict.** `Decode.int` / `Config.int` yield a typed `Err` on a
  non-integer JSON number (`1.5`) or one past `Int` range (`1e21`); an integral
  float (`1.0`) still decodes. This matches Elm and satisfies parse-don't-validate
  — a decoder yields an integer or a rejection, never a silent truncation.
- **Object keys encode sorted.** `Encode.object` emits keys in lexicographic
  order (serde's `BTreeMap`), not the insertion order Elm preserves. Both are
  deterministic; sorted matches the Go oracle the example sweep diffs against, so
  Correctness keeps it. *(keep-ours divergence.)*

### 3.2 `elm/time` → `Ipe.Time`

Elm models time as pure values (`Posix`, `Zone`, `Weekday`, `Month`) plus `Cmd`
effects (`now`, `here`, `every`). Ipê's `Ipe.Time` is `Task`-based
(`now : Task Error Posix`-shaped, `sleep`, `every`) with formatting
(`format`/`formatISO8601`/`formatRFC3339`/`formatHTTP`) and calendar math
(`addMillis`/`diffMillis`, `unixMillis`).

- **Present (reshaped):** `now`, `every`, millis conversions.
- **Missing / different:** the `Posix`/`Zone`/`Weekday`/`Month`/`ZoneName` ADTs
  and their accessors (`toYear`/`toMonth`/`toHour`/…), `here`/`utc`/`customZone`/
  `getZoneName`/`millisToPosix`/`posixToMillis` as named values. Ipê exposes IANA
  zone handling and formatting instead of the decomposed accessor family.
- **Justification:** server time is clock-and-format heavy; the decomposed
  `Weekday`/`Month` accessor family is lower value than ISO/RFC formatting. The
  **absence of a public `Posix`/`Zone` type** is the notable gap — decide whether
  to surface them for calendar-arithmetic code. Filed.

### 3.3 `elm/random` → `Ipe.Random`

Elm is generator-based and pure: `Generator a`, `step`, `Seed`, `map`/`andThen`/
`list`/`pair`, driven to effect via `Random.generate : (a -> msg) -> Generator a
-> Cmd msg`. Ipê offers **two surfaces**: `Task`-based generation (`int`, `float`,
`range`, `choice`, `shuffle`, `weighted`) and a **seeded pure** surface (`Seed`,
`seed`, `seededInt`, `seededFloat`, `seededChoice`).

- **Present (reshaped):** `int`, `float`, `weighted`, a `Seed` type, seeded draws.
- **Missing:** the composable `Generator a` monad — `map`/`map2..5`/`andThen`/
  `constant`/`uniform`/`list`/`pair`/`lazy`, `initialSeed`/`independentSeed`,
  `minInt`/`maxInt`.
- **Justification:** effectful randomness (`Task`) is the common server path; the
  seeded surface covers reproducibility. The **composable `Generator` monad** is
  the real gap for structured/property-style generation. Filed.

### 3.4 `elm/http` → `Ipe.Http` (+ `Ipe.Http.Server`)

Elm's `Http` is a `Cmd`/`Task` client with `Body`/`Expect`/`Resolver`/`Progress`/
`Response` ADTs. Ipê's `Ipe.Http` is a **builder-style client** (`defaultRequest`
`|> withUrl |> withMethod |> withHeader |> withBody |> …`, then `get`/`post`/
`request` returning `Task Error HttpResponse`). The **server** side
(`Ipe.Http.Server`: routes, middleware, cookies, streaming, WebSocket) has no Elm
counterpart at all (a browser cannot open a socket).

- **Present (reshaped):** request construction, `get`/`post`/`request`, headers,
  body, timeout, redirects.
- **Missing:** the typed `Expect`/`expectJson`/`expectString`/`expectBytes`
  decoding-on-response family, `Progress`/`track`, `multipartBody`/`Part`,
  `Resolver`/`task`, `riskyRequest`.
- **Excluded from parity:** `Ipe.Http.Server` is a deliberate extension beyond
  Elm's remit, not a divergence to reconcile.

### 3.5 `elm/url` → `Ipe.Http.parseQuery` (mostly missing)

Only query-string parsing (`Http.parseQuery`) exists. The whole `Url` value type,
`Url.Builder` (typed URL construction), `Url.Parser` (the `</>`/`<?>` routing
combinators), and `Url.Parser.Query` are absent.

- **Justification for what's missing:** Ipê's routing is server-side (`Ipe.Web`
  path handling, `data-ipe-path`), so the client-side `Browser.application` URL
  model is less load-bearing — but a typed `Url` + builder is genuinely useful for
  the HTTP **client** and is a real gap. Filed.

### 3.6 `elm/bytes` → `Ipe.Bytes`

Elm exposes `Bytes` + full `Bytes.Decode`/`Bytes.Encode` binary combinator DSLs
(`unsignedInt16`, `float64`, `sequence`, endianness, `loop`). Ipê's `Ipe.Bytes` is
a byte-buffer value with **codec helpers** (`fromString`/`toString`, `fromHex`/
`toHex`, `fromBase64`/`toBase64`, `append`/`slice`/`length`/`isEmpty`).

- **Present:** the `Bytes` value + hex/base64/utf-8 conversions + slicing.
- **Missing:** the structured binary `Decode`/`Encode` combinator surface and the
  `Endianness` type.
- **Justification:** hex/base64/utf-8 covers the overwhelmingly common
  serialization need; a full binary DSL is a heavier, lower-frequency surface.
  Filed as lower priority.

### 3.7 `elm/file` → `Ipe.File`

Elm's `File` is a **browser** upload/download model (`File.Select.file`,
`File.Download.string`, `toUrl`). Ipê's `Ipe.File` is a **native filesystem**:
`readFile`/`writeFile`/`append`/`exists`/`remove`/`mkdirAll`/`readDir`/`isDir`/
`tempFile`/`tempDir`/`copy`/`rename`, all `Task Error a`. Every path argument is
a typed `Ipe.Path.Path` (built by `Path.fromString`, which rejects `..`
traversal escapes and NUL bytes), never a raw `String` — see the language
book's [filesystem chapter](../language/filesystem.md).

- **Justification:** the browser `File`/`Select`/`Download` model is
  target-specific (see §4 exclusions). The native filesystem is the correct
  analogue and is a strict superset for the server target. The browser upload
  surface is **excluded**; a future WASM/webview target may reintroduce a
  select/download shim.

### 3.8 `elm/regex` → `Ipe.Regex`

Elm 0.19 **de-blessed** `elm/regex` (steering users to `elm/parser`). Ipê keeps
regex first-class and, like Elm, compiles a pattern once into an opaque `Regex`:
`compile : String -> Result Error Regex` is the sole construction boundary, so an
invalid pattern is a typed `Err` — never a silent no-match. The operations then
take the compiled handle: `match`/`find`/`findAll`/`replace`/`split` (vs Elm's
`fromString`/`contains`/`find`/`replace`/`split` with a `Regex`/`Match`/`Options`
type + `*AtMost` variants).

- **Missing / different:** the `Match` record (capture groups, indices) and
  `Options` type, `fromStringWith` options, and the
  `findAtMost`/`replaceAtMost`/`splitAtMost` count-limited variants.
- **Justification:** the compiled-once `Regex` handle matches Elm; count limits
  and a richer `Match` are reasonable additions, filed as low priority.

### 3.9 `elm/parser` — missing

No parser-combinator library. `Parser`/`Parser.Advanced` (`succeed`/`|=`/`|.`/
`oneOf`/`chompWhile`/`loop`/`run`/…) have no Ipê counterpart. This is a genuine
gap for hand-written text parsing; filed as medium priority.

---

## 4. Justified exclusions (irrelevant to Ipê's target)

These `elm/*` surfaces are **excluded on purpose**, not silently dropped. Each is
target-specific to Elm's sandboxed browser client and is covered — where relevant
— by an Ipê-native subsystem.

| Elm surface | Why excluded | Ipê equivalent, if any |
|---|---|---|
| `elm/html`, `elm/svg` | Direct DOM/SVG node construction is a client-render concern. Ipê never emits a client VDOM program authored in HTML nodes. | `Ipe.Ui` (elm-ui-derived `row`/`column`/`el`), server-rendered to inline-styled HTML / ANSI / webview |
| `elm/virtual-dom` | The VNode/diff layer is a runtime internal, not a user API. | Internal to `Ipe.Web`/`Ipe.Tui`/`Ipe.Webview`; users never call it |
| `elm/browser` (`Browser.application`/`document`/`element`/`sandbox`, `Browser.Dom`, `Browser.Events`, `Browser.Navigation`) | Program entry + DOM/focus/viewport/nav are browser-runtime concepts. | `Ipe.Web.app` / `Ipe.Tui.app` / `Ipe.Webview.app` are the entry points; navigation is server-side `data-ipe-path` |
| `Platform.worker`, `Platform.sendToApp`/`sendToSelf`, `Router` | User-defined effect managers are an Elm kernel-package privilege that does not exist in Ipê (which binds native effects via FFI + `Task`). | `Ffi.kernel` + the `Task` effect stdlib |
| `Process.spawn`/`kill`, `ProcessId` | Elm's green-thread handles are a runtime-scheduler surface. | Concurrency is `Task.parallel` / `Cmd`; `Process.sleep` is `Time.sleep` |
| `File.Select`/`File.Download`, `File.toUrl` | Browser upload/download. | Native `Ipe.File`; a select/download shim may return with WASM/webview |
| `Debug.log`/`Debug.todo` | Dev-only, stripped from Elm `--optimize` builds. | `Ipe.Log` (production `Task` logger) + `Ipe.Trace` |

**Boundary rule for future work:** an exclusion is only legitimate when a
Ipê-native subsystem covers the *user need* (UI, navigation, concurrency, IO) or
the surface is target-specific with no need on the server/native target. A missing
value that has no such justification is a **gap** (§2–§3), not an exclusion, and
belongs in the plan.

---

## 5. String / Unicode semantics

Every Ipê `String` length/index/slice operation counts **Unicode code points**
(runes — Rust `char`, a Unicode scalar value), uniformly across the module. Elm
0.19 `String` is JS-backed and counts **UTF-16 code units**, so astral-plane
characters (emoji, some CJK) count as 2 and can be split mid-character. Ipê counts
such a character as length 1 and never splits it (verified in
`runtime/src/ipe_runtime/string.rs`). The unit is code points, not grapheme
clusters; `Ipe.Tui` uses grapheme segmentation separately for terminal display
width, which does not affect `String` semantics.

Ipê also adds `casefold`/`equalFold`/`isEmail`/`isUrl` and the haystack-first
`containsIn`/`startsWithIn`/`endsWithIn` pipeline companions beyond Elm's set.

`String.fromFloat` follows Go's `strconv.FormatFloat(f,'g',-1,64)` shape rather
than Elm's JS `String(f)`. It agrees with Elm on the common cases (an integral
float drops its fraction; the shortest round-tripping digits are used, so
`0.1 + 0.2` is `"0.30000000000000004"`) and diverges on two shape details: Go
pads the exponent to two digits (`1e-07` vs Elm's `1e-7`) and keeps negative
zero's sign (`-0` vs Elm's `0`). Go-oracle Correctness keeps ours; see
[`behaviour-verdicts.md`](behaviour-verdicts.md). *(keep-ours divergence.)*

---

## 6. Effect-model & error-type departures (the structural ones)

These are not per-value gaps but whole-model choices; they explain why several
`elm/core` rows read `diverged` or `n/a`.

- **Task-everywhere.** Every observable side effect returns `Task Error a`, and
  `Task` is directly runnable at the program entry boundary. In Elm, `Task` is
  inert until converted to a `Cmd` by the runtime. This is why Ipê ships
  `File`/`Http`/`Db`/`Time`/`Random`/`Crypto`/`Io`/`System` as `Task`.
- **Fixed `Error` channel.** Every `Task`/`Result` combinator fixes the error slot
  to a canonical typed `Error` (`Task Error a`, not polymorphic `Task x a`);
  `Result String a` / `Task String a` are forbidden in public surfaces and this is
  test-enforced. There is no `Never` type, so `Task Never a` and `Basics.never`
  have no analogue.
- **Server-driven TEA.** `init`/`update`/`view` run on the **server**; the browser
  receives HTML on load and VNode-diff patches over SSE (`Ipe.Web`), ANSI cells
  (`Ipe.Tui`), or a native webview (`Ipe.Webview`). This is why `elm/browser` is
  excluded rather than reshaped.
- **Native stdlib with no Elm-core counterpart.** `Ipe.Db`, `Ipe.Auth`,
  `Ipe.Crypto`, `Ipe.Jwt`, `Ipe.Money`/`Decimal`, `Ipe.Email`, `Ipe.Cache`,
  `Ipe.Compression`, `Ipe.Csv`, `Ipe.Http.Server`, `Ipe.Log`/`Trace` are additive
  extensions justified by the server/native target. They are **extensions**, not
  divergences to reconcile against Elm.
- **Stricter Tier-C import surface.** Only `Ipe.Basics` and the core type
  vocabulary (`List`/`Maybe`/`Result` + their constructors, `Bool`, `Order`) are
  ambient. Every other stdlib module requires an explicit `import` and is used
  qualified — where Elm makes stdlib modules ambiently available for qualified
  use. Deliberate no-magic choice: the import list is a complete inventory of a
  file's capabilities. Decided in
  [`docs/adr/0047-basics-and-tiered-auto-import.md`](../adr/0047-basics-and-tiered-auto-import.md).
- **TEA namespace is `Ipe.Tea`, not `Platform`.** Ipê collects the four
  managed-update-loop shapes under `Ipe.Tea.{Web,WebView,Tui,Console}`, and the
  distinction "is this a TEA app?" is one structural rule — *does the module
  import anything under `Ipe.Tea.*`?*. Elm names its runtime plumbing `Platform`
  and has no non-TEA program shape, so every Elm program is a TEA program; Ipê
  additionally has a plain `main : Task …` **Program** shape, so the meaningful
  axis is specifically *the managed loop*, which `Tea` names and `Platform` does
  not. Decided in
  [`docs/adr/0048-tea-shape-relocation.md`](../adr/0048-tea-shape-relocation.md).
- **Per-shape typed `.app`, not a unified entry.** Each `Ipe.Tea.<Shape>` exposes
  its own precisely-typed `.app` (Tui also `program`), because the `view` type
  genuinely differs per shape (`Element` for the graphical shapes, `String` for
  `Tui.program`/`Console`). This mirrors Elm's split of `Browser.sandbox` /
  `element` / `document` into separate typed entries rather than one config ADT,
  and is what keeps invalid states unrepresentable without higher-kinded types.
  Same ADR.

---

## 7. Shared-with-Elm constraints (not divergences)

Listed so they are not mis-sold as Ipê inventions — they match Elm 0.19.x: no
higher-kinded types, no custom operators, no `where` clauses, negative-literal
arguments need parens (`f (-1)`), exhaustive `case…of`, extensible record
annotations + record update, and identical core syntax (pipelines, cons, lambdas,
`let`/`case`, module/import, `type`/`type alias`). The ambient unqualified names
(`Ok`/`Err`/`Just`/`Nothing`/`identity`/`always`/`not`/`fst`/`snd`/`clamp`/
`modBy`) match Elm's `Basics` exposure; the import surface *around* them is
stricter (see §6, ADR 0047).

A **shipped** language divergence refuses a catch-all arm — a wildcard `_` or a
bare variable binder — in a `case` whose scrutinee is a **closed, finite-variant
union** (a user `type`, or a Prelude built-in like `Maybe` / `Result`) when the
catch-all absorbs at least one constructor no earlier arm named. Elm accepts such
a catch-all silently, so after a variant is added to a union, every `case` that
handled the old variants with a trailing `_ ->` keeps compiling and the new
variant inherits the catch-all's behaviour with no signal. Ipê rejects it
(IPE-T0018, an error): a silently-swallowed new variant is a *representable wrong
state*, and the fundamental rule is to make invalid states unrepresentable. The
safe outcome is the default; the permissive outcome is opt-in through an
`ipe fix` expansion into per-constructor arms and (staged behind the directive
engine) a per-site `-- @allow(open-case) <reason>` escape hatch — fail-closed by
construction. `Bool`, `List`, and the open domains (`Int` / `Float` / `Char` /
`String`) keep the catch-all: their variant sets are frozen or infinite, so the
evolution-safety argument does not apply. The rule is judged from the pattern
column, so a bare `_ ->`-only `case` over a closed union is not yet flagged (a
known limitation, not a guarantee). Run `ipe explain IPE-T0018` for the full
rationale.

One **shipped** syntax extension goes beyond Elm's `case…of`: **or-patterns**
(`|` alternatives), e.g. `Up | Down -> "vertical"` and, with shared bindings,
`Circle r | Square r -> area r`. Elm has no or-patterns; this is Rust / OCaml
parity. Every alternative must bind the identical set of variables at identical
types (checked as IPE-T0019 / a type mismatch), and an or-pattern participates
in exhaustiveness by row expansion — `Red | Green | Blue` counts as full
coverage of a three-constructor union. This is ledgered in
[`../divergences-from-sky.md`](../divergences-from-sky.md) §6.3.

A second **shipped** divergence narrows a collection-key constraint. `Float` is
`comparable`, so Elm accepts `Set Float` and `Dict Float v`; Ipê's type checker
accepts them too, but lowering rejects them (IPE-L0117). Two reasons compound: a
`Float` key is a silent correctness footgun — exact-key lookups over *computed*
floats miss (`0.1 + 0.2 ≠ 0.3`) — and Rust's `f64` is neither `Ord` (backs
`Set` as `BTreeSet`) nor `Hash`/`Eq` (backs `Dict` as `HashMap`). This is
deliberate and permanent: a total-order wrapper would make the collection
representable but not safe, trading a compile error for a silent runtime bug —
which the precedence order (Correctness over Completeness) forbids. Use an
`Int`/`String`/`Char`/`Bool` key, key on a stable identifier and store the
`Float` in the value position, or quantise to `Int` minor units. Run
`ipe explain IPE-L0117` for the full rationale.

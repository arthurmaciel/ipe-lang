# Divergences from Elm

> **Framing.** ipê is an **Elm-family** functional language. It inherits Elm's
> core surface syntax (`let…in`, `case…of`, `|>`/`<|` pipelines, `::` cons,
> `\x -> …` lambdas, `{ r | f = v }` record update, `type` / `type alias`,
> `module … exposing (…)` / `import … as … exposing (…)`), Elm's
> Hindley–Milner type discipline, and **The Elm Architecture** (TEA):
> `init` / `update` / `view` / `subscriptions` with `Model` + `Msg` +
> `Cmd` + `Sub`. This document is the durable ledger of the **deliberate
> departures** — where ipê adds beyond Elm, reshapes shared surface, or omits
> an Elm feature on purpose. Differences are stated neutrally as engineering
> trade-offs; Elm's choices are correct for Elm's target (a sandboxed
> browser client). ipê's target is different (typed Go/Rust producing
> server, native, desktop, and planned WASM binaries), and most divergences
> fall out of that one root fact.
>
> **Naming note.** The current source tree still uses the historical `Ipê` /
> `Std` module prefixes (e.g. `Ipe.List`, `Ipe.Ui`, `Ipe.Live`). This
> document uses the project name **ipê** for the language and preserves the
> `Ipê.*` / `Ipe.*` module names as they appear in the code today. A planned
> post-parity rename collapses these prefixes into one flat auto-imported
> namespace; when that lands, update the module names here.
>
> Items the author could not confirm against the docs were previously marked
> **[UNVERIFIED]**; STR1, R4, and R5 have since been verified against the
> ipê stdlib + runtime source (see the resolved Verification backlog below).

---

## 1. Effect-model divergences

The single largest family of departures. Elm's effect model is **managed and
sandboxed**: the only way user code performs a side effect is to return a
`Cmd msg` (or subscribe via `Sub msg`), and the Elm runtime — living inside
the browser — is what actually performs it. `Task x a` exists in `elm/core`
but is **inert**: a `Task` value does nothing until converted to a `Cmd`
through `Task.perform` / `Task.attempt` and handed back to the runtime. Elm
`elm/core` ships **no** file IO, **no** database, **no** subprocess, **no**
arbitrary environment access — effects are limited to what the sandboxed
packages (`elm/http`, `elm/time`, `elm/random`, `elm/browser`, `elm/file`)
expose through `Cmd`.

ipê inverts this into **"Task-everywhere"** (v0.10.0+): *every observable
side effect returns `Task Error a`*, and `Task` is a first-class, directly
runnable effect at the program's entry boundary.

| # | Divergence from Elm | Rationale |
|---|---|---|
| E1 | **Direct Task-returning effect stdlib.** ipê ships `File.*`, `Http.*`, `Db.*`, `Process.run`, `Io.*`, `System.*`, `Crypto.{randomBytes,randomToken}`, `Time.{now,sleep}`, `Random.*`, `Log.*` all as `Task Error a`. Elm core exposes none of these; the ones that exist (Http, Time, Random) are reachable only via `Cmd`. | ipê targets the server/native runtime where filesystem, sockets, DB handles, and subprocesses are ordinary capabilities. A managed-Cmd sandbox would be a straitjacket outside the browser. |
| E2 | **A four-tier effect taxonomy.** Bindings are classified: **Pure** (bare `a` — `String.length`, `List.map`, `Crypto.sha256`), **Fallible-pure** (`Result e a` / `Maybe a` — `String.toInt`, JSON decoders, `Auth.hashPassword`), **Effects** (`Task Error a`), **Diverging** (`Int -> a` — `System.exit`). Elm has the pure vs. `Cmd` split but no first-class "fallible-pure vs. effectful" tiering nor a diverging tier. | Makes the failure mode legible in the type: a signature says whether a call can fail, must be sequenced as an effect, or never returns. |
| E3 | **`Task` is run, not just perform-ed.** Program entry points (`main = Task.run …` for CLI/cron/worker) actually execute the `Task`. Top-level Task-typed module bindings require an explicit `Task.run`. Elm has no user-invokable task runner; only the runtime executes effects. | ipê `main` is a real process entry (Go `func main`), not a browser bootstrap handed a sandbox. |
| E4 | **Auto-forced discarded tasks.** `let _ = TaskExpr in …` is auto-wrapped so the side effect fires (`rt.AnyTaskRun`). In Elm a discarded `Task` is dead code — it can produce no effect without becoming a `Cmd`. | Ergonomic sequencing of fire-and-forget effects (logging, tracing) in a `let` chain. |
| E5 | **Result/Task bridges as a named surface.** `Task.fromResult`, `Task.andThenResult`, `Result.andThenTask`, `Task.mapError`, `Task.onError`, plus `RetryPolicy` / `ShouldRetry` retry combinators. By design there is **no** `Result.fromTask` — effectful pipelines stay in `Task`. Elm's `Task`/`Result` interconversion surface is thinner and retry is not in core. | Codifies the "keep effects in Task; the entry boundary executes them" discipline as an API, not a convention. |
| E6 | **`Cmd.publish` / `Sub.subscribeTopic` in-process pub/sub + `Cmd.perform task toMsg`.** ipê's TEA runtimes (Ipe.Live / Ipe.Tui / Ipe.Webview) drive async effects by `Cmd.perform` dispatching a `Msg` back through the loop — structurally like Elm's `Cmd`, but the loop and the pub/sub broker live on the **server / host process**, not in the browser. Elm's `Cmd`/`Sub` are browser-runtime-mediated with no server broker. | Server-driven TEA (see §4) needs a host-side command executor and cross-session/broker fan-out that a client runtime cannot provide. |
| E7 | **`System.exit : Int -> a` (Diverging tier).** Process termination with a polymorphic never-returns type. Elm (browser) has no process-exit concept at all. | Native/CLI programs must set an exit code; the polymorphic return marks the non-returning control flow in the type. |

---

## 2. Error-type divergences

| # | Divergence from Elm | Rationale |
|---|---|---|
| ER1 | **`Result String a` / `Task String a` are forbidden in public surfaces.** ipê mandates `Result Error a` / `Task Error a` with a canonical typed `Error` (constructors like `Error.unexpected`, classifier + `Error.toString`). This is a *non-regression rule enforced by the test suite*, not just a style guide. Elm freely uses `String` errors and ad-hoc custom error types; `elm/core` itself hands back `Result String a` in places and each package defines its own error type (`Http.Error`, `Json.Decode.Error`). | Stringly-typed errors lose structure at the boundary and cannot be pattern-matched or correlated. A single typed `Error` gives uniform classification, correlation IDs, and structured logging across the whole effect stdlib. |
| ER2 | **Deleted error shims.** `Ipe.IoError` and `RemoteData` were removed pre-v1; both are common Elm-ecosystem patterns (`RemoteData` is a well-known community package). | Consolidation on the one typed `Error` + `Task`/`Result`; `RemoteData`'s four-state modelling is expressible with `Maybe (Result Error a)` in Model without a dedicated type. |
| ER3 | **Two-level error pattern is idiomatic.** Short correlation id (`Crypto.randomToken`) + server-side `Log.errorWith` + user-facing `Task.fail (Error.unexpected …)`. Elm has no logging in core and no prescribed correlation-id convention. | Operable production errors: the operator gets the detail (log), the user gets a reference id, and no internal detail leaks. |

---

## 3. Syntax divergences

Most surface syntax is **identical** to Elm (see §5). The departures:

| # | Divergence from Elm | Rationale |
|---|---|---|
| S1 | **`Ffi.kernel "Name"` declarations.** A Ipê-source binding with an HM signature whose body routes to a typed kernel; and the auto-FFI mechanism binds foreign (Go/Rust) crates directly. Elm has **no** foreign-function binding — its only interop is **ports** (typed `Cmd`/`Sub` message passing to hand-written JS) plus a sandboxed, restricted `Platform` layer reserved for `elm/*` kernel packages. User Elm code cannot declare a kernel. | ipê compiles to Go/Rust and must surface a large native stdlib (and third-party crates) with real HM types; ports would be intractable at Stripe-SDK scale (76k symbols). |
| S2 | **String interpolation in triple-quoted strings.** `"""… {{expr}}…"""` interpolates identifiers, field access (`{{r.f}}`), qualified names (`{{String.fromInt n}}`), and calls; `\{{` escapes a literal `{{`; single `{` is literal. Elm **has** triple-quoted multiline string literals but **no** interpolation of any kind — you concatenate with `++`. | Templating (HTML, SQL-adjacent, shell placeholders) is a first-class server use case; the escape rule lets Mustache/Handlebars payloads pass through untouched. |
| S3 | **Reserved-name rewriting in codegen** (`init → init_`, `string → string_`, `type → type_`, …). A compilation-model divergence: ipê identifiers are rewritten with a trailing `_` when they collide with Go predeclared/keyword names. Elm→JS mangling exists but there is no equivalent user-observable reserved list, and the failure modes differ. | Emitted Go must be safe from accidental shadowing of predeclared types (`string`, `error`) and from Go's auto-called `func init()`. Largely invisible to user code (module-prefixing shields top-level names). |
| S4 | **Continuation-inside-type-body not supported.** `name\n : T` (colon on a continuation line) parses, but `T1\n -> T2` inside the type body does not — extract a `type alias` for the whole arrow. Elm's parser accepts multi-line arrow types more freely. | Current parser limitation (Active limitation #10), not a design choice; listed for honesty. |
| S5 | **Head-position type-alias-of-a-function-signature** (`view : Renderer Msg` where `type alias Renderer msg = Model -> Element msg`) is canonical Elm and now works in ipê (closed v0.16.4) — noted because it was historically an ipê-only *gap*, now parity. | Parity restoration; no longer a divergence, kept for grep. |

Note: **negative-literal args need parens** (`f (-1)`), **no custom
operators**, **no `where` clauses** — these are **the same as Elm 0.19**, not
divergences (see §5).

---

## 4. Stdlib / platform divergences (the big one)

### 4.1 Compile target and runtime location

| # | Divergence from Elm | Rationale |
|---|---|---|
| P1 | **Compile target: typed Go (and Rust in the active port) → native binaries.** Elm compiles to **JavaScript** for a browser client. ipê emits typed Go/Rust producing server binaries, native CLI/TUI executables, desktop apps, and (planned) WASM. | This is the root divergence from which §1 and most of §4 follow: a native/server target has capabilities and constraints a browser sandbox does not. |
| P2 | **Server-driven TEA over SSE (`Ipe.Live`).** The `init`/`update`/`view` loop runs on the **server**; the browser receives full HTML on load and **VNode-diff patches over Server-Sent Events**; sessions, cookies, routing, and stores live server-side. Elm's TEA (`Browser.element` / `document` / `application`) runs **entirely in the browser** as client JS with no server loop. | Keeps application state and secrets on the server (never serialized to the client), enables the same `view` to render across web/terminal/desktop, and removes the client build step. |
| P3 | **One `view`, three backends.** The same `Ipe.Ui` `view` renders to (a) inline-styled HTML with SSE patches (`Ipe.Live`), (b) ANSI terminal cells (`Ipe.Tui`), and (c) a native desktop window via system webview (`Ipe.Webview`, macOS in v0.1). Elm has one render target (the DOM). | Cross-surface reuse of pure view code; terminal + desktop are outside Elm's remit. |

### 4.2 Ipe.Ui vs. elm-ui

| # | Divergence from Elm (elm-ui / mdgriffith) | Rationale |
|---|---|---|
| U1 | **Server-rendered, not client-rendered.** `Ipe.Ui` is elm-ui-derived (`row`/`column`/`el`, `Background`/`Border`/`Font`/`Region`, `Input.*`) but renders to **inline-styled HTML on the server**. elm-ui builds the layout in the browser at runtime. | Follows from server-driven TEA (P2). |
| U2 | **Added surface elm-ui lacks:** typed pseudo-classes (`:hover`/`:focus-visible`/`:active`/`:disabled`), CSS media queries + a typed `Breakpoint` ADT, CSS transitions + keyframe animations (auto-wrapped in `prefers-reduced-motion`), CSS-grid track ADT, aspect-ratio helpers — all lowered to inline CSS / sky-id-scoped `<style>` blocks. | Production web UI needs responsive + motion + grid without dropping to raw CSS; keeps the "never write CSS" contract. |
| U3 | **`Ui.fill` lowers asymmetrically by flex axis** (main-axis `flex-grow`; cross-axis width `100%`; cross-axis height relies on flex `stretch`). elm-ui's fill model differs. | Closes a real CSS Flexbox §9.8 indefinite-height bug class specific to HTML/flex emission. |
| U4 | **`data-sky-eval` is forbidden; `Ipe.Ui` HTML-escapes everything; `data-sky-path` (typed) drives URL sync.** No `new Function()` / eval sink; CSP-strict by default. elm-ui/elm has no such directive because it never emits server HTML that could carry an injection sink. | XSS/CSP hardening is a server-HTML concern that does not arise in Elm's client model. |

### 4.3 Effect + platform modules with no Elm-core counterpart

ipê ships a large native stdlib that Elm core (and often the Elm ecosystem)
does not have. Each is a divergence in surface area, all justified by the
server/native target:

- **Persistence & data:** `Ipe.Db` (SQLite/Postgres, typed `SqlValue`/`SqlField`
  ADTs, migrations, tenant-prefix SQL enforcement), `Ipe.Csv`, `Ipe.Config`
  (TOML/YAML/JSON decoders), `Ipe.Compression` (gzip/zstd).
- **Security:** `Ipe.Auth` (bcrypt + HS256/RS256 JWT cookies), `Ipe.Crypto`
  (sha/hmac/rsa/AEAD/entropy), `Ipe.Jwt`, `Ipe.Uuid`,
  `Ipe.Encoding`, `Ipe.Bytes`. Elm has no crypto in core (browser
  code would use SubtleCrypto via ports).
- **Money/precision:** `Ipe.Decimal` (arbitrary precision) + `Ipe.Money`
  (currency-typed on Decimal, ISO 4217). Elm core has only `Float`; there is
  no decimal/money type in core.
- **Servers & networking:** `Ipe.Http.Server` (routes, middleware, cookies,
  streaming), `Ipe.Http.Server.WebSocket` + `Ipe.WebSocket` (client),
  `Ipe.Http.Server.Stream` (SSE / chunked). Elm cannot open a server socket.
- **Observability:** `Ipe.Log`, `Ipe.Trace`, auto-mounted `/_sky/console`,
  Prometheus metrics, OTLP export. No Elm-core analog (`Debug.log` is the
  nearest, and it is dev-only + removed from production builds).
- **Runtime services:** `Ipe.Cache` (LRU+TTL), `Ipe.Email` (Resend/SES/
  SendGrid/SMTP), `Ipe.Time` (IANA zones, calendar math), `Ipe.File`,
  `Ipe.Io`, `Ipe.System`, `Ipe.Process`, `Ipe.Path`,
  `Ipe.Regex` (built-in — Elm 0.19 removed `elm/regex` from the blessed
  set in favor of `elm/parser`).

### 4.4 Reshaped shared modules (elm/core names ipê renames or extends)

| # | Divergence from Elm | Rationale |
|---|---|---|
| R1 | **Haystack-first `*In` String companions.** `String.containsIn`, `startsWithIn`, `endsWithIn` mirror Elm's needle-first `contains`/`startsWith`/`endsWith` with reversed argument order for `\|>` pipelines. Both forms ship. Elm has only the needle-first form. | Pipeline ergonomics without breaking Elm-compatible call sites. |
| R2 | **`Ipe.ToString` discoverability surface.** `ToString.fromInt`/`fromFloat`/`fromBool`/`fromTime` alias the canonical kernels. Elm removed the polymorphic `toString` in 0.19 (replaced by `String.fromInt`/`fromFloat` + dev-only `Debug.toString`); ipê keeps `Basics.toString` **and** a discoverable `ToString.*` namespace. | Editor/`ipe doc` discoverability; steers AI-written code to one obvious name. |
| R3 | **`Ipe.Pure` arity-0 Task companions.** `Pure.uuidV4 ()`, `Pure.timeNow ()`, `Pure.dbConnect ()`, … give a uniform `() -> Task Error a` shape. This exists to work around ipê **Active limitation #7** (zero-arg calls follow the binding's declared type, so `Uuid.v4` is bare but `Time.now ()` needs the unit). Elm has no such split — nullary values are simply values. | Bridges a current codegen limitation without renaming existing bindings; honest about being a workaround. |
| R4 | **`Task` normalizes to a unary internal shape** and rejects `Task String a`/`Task Int a` (see ER1). Elm's `Task x a` is binary with a free error slot and commonly `Task Never a` for `Cmd.perform`. **ipê exposes no `Never`-error task form.** Verified: every signature in the `Task` surface (`crates/ipe/stdlib/Ipê/Core/Task.ipe`) fixes the error slot to `Error` (`succeed : a -> Task Error a`, `map : (a -> b) -> Task Error a -> Task Error b`, etc.); there is no `Never` type in the stdlib or canon prelude, and no `Process` module. | Enforces the typed-`Error` mandate at the type level — the error channel is a fixed `Error`, never a free or `Never` slot. |
| R5 | **Modules ipê omits from elm/core.** No `Array`, `Bitwise`, `Tuple` (module — `fst`/`snd` live in `Basics`), or `Debug` module. Verified absent: the stdlib module set (`crates/ipe/stdlib/Ipê/Core/`) contains none of them, no `Array.ipe`/`Bitwise.ipe`/`Tuple.ipe`/`Debug.ipe` exists anywhere in the tree, and the closed kernel registry (`crates/sky_kernels`) surfaces no kernel under those names — so none is present under a different name. `elm/browser`, `elm/url` (only `parseQuery` present), and Elm **ports**/`Platform`/`Platform.Worker` likewise have no ipê counterpart (replaced by FFI + Task + the server runtimes). | Server/native target makes browser modules (`elm/browser`) and the port model moot; `Array`/`Bitwise`/`Tuple`/`Debug` are unported (`List`/`Dict` cover the container need; `Debug` is dev-only in Elm and superseded by `Ipe.Log`). |

### 4.5 String / Unicode semantics

| # | Divergence from Elm | Rationale |
|---|---|---|
| STR1 | **Code-point-based String operations.** Every ipê `String` length / index / slice operation counts **Unicode code points** (runes — Rust `char`, a Unicode scalar value), uniformly across the module. Elm 0.19 `String` is backed by JavaScript strings, so `String.length` and slicing count **UTF-16 code units** — astral-plane characters (emoji, some CJK) count as 2 and can be split. ipê counts such a character as length 1 and never splits it. Verified in the String kernel (`runtime/src/sky_runtime/string.rs`): `length` = `chars().count()`; `reverse` = `chars().rev()`; `slice`/`left`/`right`/`dropLeft`/`dropRight`/`padLeft`/`padRight`/`toList` and the empty-separator `split` case all iterate `chars()` (code points), with rune-index clamping. The unit is code points, **not** grapheme clusters — `Ipe.Tui` separately uses `uniseg` grapheme segmentation for terminal display width, which is a distinct concern and does not affect `String` semantics. | UTF-8/code-point semantics are the natural Go/Rust representation and avoid Elm's surrogate-pair splitting hazard. |
| STR2 | **Extra String surface:** `casefold`, `equalFold`, `isEmail`, `isUrl`, `words`, `lines`, `padLeft`/`padRight`, `repeat` beyond Elm's set. | Server text handling (validation, normalization) is a common need; keeps it in the typed stdlib rather than user regex. |

---

## 5. Shared-with-Elm constraints (context, **not** divergences)

Listed so the README does not mis-sell these as ipê inventions or ipê-only
limits — they match Elm 0.19.x:

- **No higher-kinded types** — both are Hindley–Milner / rank-1.
- **No custom (user-defined) operators** — Elm 0.19 disallows them in
  application code; ipê likewise.
- **No `where` clauses** — neither language has them; use `let…in`.
- **Negative-literal arguments need parens** — `f (-1)` in both (bare `f -1`
  parses as subtraction).
- **Exhaustive `case…of`** — both check pattern exhaustiveness.
- **Extensible record type annotations** (`{ a | field : T }`) and record
  update (`{ r | f = v }`) — present in both; ipê uses row polymorphism for
  optional cfg fields (`head`, `consoleAuth`) the same way Elm uses extensible
  records.
- **Core syntax** — pipelines, cons, lambdas, `let`/`case`, module/import
  syntax, `type`/`type alias` — identical.
- **Prelude names** — `Result (Ok/Err)`, `Maybe (Just/Nothing)`, `identity`,
  `always`, `not`, `fst`, `snd`, `clamp`, `modBy` match Elm's `Basics`/
  `Maybe`/`Result` exposure.

---

## 6. README-liftable summary table

| Aspect | Elm 0.19.x | ipê | Why |
|---|---|---|---|
| Compile target | JavaScript (browser client) | Typed Go / Rust → server, CLI, TUI, desktop, planned WASM | Native/server capabilities & performance |
| Effect model | Managed `Cmd`/`Sub`; `Task` inert until made a `Cmd` by the runtime | Task-everywhere: every side effect is `Task Error a`, runnable at entry | Real filesystem/DB/socket/process capabilities |
| Effect tiers | Pure vs. `Cmd` | Pure / Fallible-pure / Effects / Diverging | Failure & effect mode visible in the type |
| Error type | `String` errors + per-package error types, freely | Typed `Error` mandated; `Result String a`/`Task String a` forbidden | Structured, correlatable, matchable errors |
| TEA runtime | Client-side in the browser | Server-driven over SSE (Live) / ANSI (Tui) / native webview | State + secrets stay server-side; multi-surface |
| Foreign interop | Ports (typed JS message passing) | `Ffi.kernel` + auto-binding of Go/Rust crates | HM-typed native stdlib at SDK scale |
| String interpolation | None (`++` concat) | `"""… {{expr}} …"""` with `\{{` escape | First-class server templating |
| UI library | elm-ui (client-rendered) | `Ipe.Ui` (server-rendered inline CSS; +pseudo/media/anim/grid) | No client build; CSP-safe; cross-surface |
| String semantics | UTF-16 code units (JS-backed) | Code-point (rune) based over UTF-8, uniform across the module | Avoids surrogate-pair splitting; Go/Rust-native |
| Regex | `elm/regex` de-emphasized (prefers `elm/parser`) | Built-in `Ipe.Regex` | Common server need in the typed stdlib |
| Crypto / Auth / DB / Money | Not in core (browser via ports / packages) | `Ipe.Auth`, `Crypto`, `Jwt`, `Ipe.Db`, `Ipe.Money`/`Decimal` | Server target requires them first-class |
| Servers / sockets | Cannot open a server socket | `Ipe.Http.Server`, WebSocket, SSE stream | Server target |
| Observability | `Debug.log` (dev-only) | `Ipe.Log`, `Ipe.Trace`, console, metrics, OTLP | Production operability |
| Process exit | N/A (browser) | `System.exit : Int -> a` (Diverging) | Native process needs an exit code |
| HKT / custom operators / `where` | Absent | Absent (**same**) | Shared HM discipline |

---

## Verification backlog — resolved

- **STR1** — RESOLVED. The counting unit is **Unicode code points (runes)**,
  uniform across the `String` kernel (`runtime/src/sky_runtime/string.rs`):
  `length`/`reverse`/`slice`/`left`/`right`/`dropLeft`/`dropRight`/`padLeft`/
  `padRight`/`toList` and the empty-separator `split` all iterate `chars()`
  (code points), not grapheme clusters and not UTF-16 code units. The
  rune-based claim holds beyond `dropLeft`/`dropRight`.
- **R5** — RESOLVED. `Array`, `Bitwise`, `Tuple` (module), and `Debug` are
  genuinely absent — no `.ipe` module, and no kernel under those names in the
  closed registry (`crates/sky_kernels`).
- **R4** — RESOLVED. No `Never`-error task form exists; the `Task` surface
  (`crates/ipe/stdlib/Ipê/Core/Task.ipe`) fixes every error slot to `Error`,
  and no `Never` type is defined in the stdlib or canon prelude.

---

## 7. Planned divergences (filed, not yet implemented)

### 7.1 Closed-union `case` refuses catch-all arms

Elm (and the Ipê reference, which follows Elm here) accepts a wildcard `_ ->`
or bare-variable arm as a catch-all over any scrutinee type. ipê will refuse
a catch-all arm when the scrutinee's solved type is a **closed union** (a
user-declared ADT, `Maybe`, or `Result`) and the arm absorbs at least one
constructor no earlier arm matched — so adding a variant becomes a compile
error at every top-level match site instead of silently taking the catch-all
branch (the classic silent-`update` TEA failure). Wildcards remain required/
allowed over open domains (`Int`, `Float`, `String`, `Char`) and permitted
over `Bool` and `List` (their constructor sets cannot grow). Scope is the
arm's top-level pattern only; nested payload positions are covered by an
opt-in lint rule instead (combinatorial-explosion trade, stated in the
design). Escape hatch: a per-site `-- @allow(open-case) <reason>` directive
with a mandatory reason — the rule guards program evolution, not runtime
soundness (exhaustiveness itself, IPE-T0010, stays unsuppressible), which is
why a reasoned opt-out is admissible. Counterweights: a machine-applicable
fix/code action expanding the catch-all into one arm per hidden constructor
reusing the catch-all's own body (semantics-preserving), and an "add missing
arms" LSP action for the no-catch-all case. New diagnostic IPE-T0018 with a
progressive explain page; IPE-T0010's page (which already *teaches* this
philosophy ahead of the implementation) is corrected in the same change.
Recommendation: adopt-with-opt-out. Design + spec + phased plan (incl. the
in-repo corpus migration):
`docs/architecture/exhaustive-case-finite-adt-design-2026-07-16.md`. Filed
2026-07-16 (backlog #208); implementation not started.

# CLAUDE.md — Ipê language authoring reference

> **Ipê** — an Elm-family, pure-functional language whose compiler + stdlib
> modules are currently `Sky.*`-prefixed pending a final rename, so use those
> names verbatim in code. Source language is Elm-shaped; the compiler emits
> Rust. This document is a self-contained authoring reference: everything an
> agent needs to write correct, compiling Ipê programs. Every import path,
> module name, kernel name, type name, and function name below is the exact
> identifier the current compiler accepts.

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
Ipê hijacking them. `\\` collapses to single literal backslash; other
`\X` sequences preserved verbatim (regex `\d+`, paths `\test`, etc).

### Prelude (autoloaded via `Sky.Core.Prelude exposing (..)`)

`Result (Ok/Err)`, `Maybe (Just/Nothing)`, `identity`, `not`, `always`,
`fst`, `snd`, `clamp`, `modBy`, `errorToString`.

## When users ask for an app — the architecture decision matrix

Before writing more than a one-file PoC, align with the user on the six
decisions below. Production-grade code does not survive guesswork.

### The six decisions to confirm

1. **App shape** — match the matrix. Sky.Live=web UI, Sky.Http.Server=headless
   API, Sky.Cli=one-shot/cron, Sky.Tui=terminal UI, Sky.Webview=desktop.
2. **Persistence** — SQLite (single-file, embeds) / PostgreSQL / Firestore /
   Redis / none.
3. **Auth** — none / `Std.Auth` (cookies+JWT, you own users) / OAuth
   (Google/GitHub) / external (Auth0/Clerk/Cognito).
4. **Sky.Live session store** — memory (dev only) / sqlite / redis / postgres
   / firestore. Required even when the user picks a different primary DB.
5. **Deployment target** — local binary / Docker / Cloud Run / Kubernetes / VM.
6. **Observability scope** — local logs only / per-app embedded console / OTel
   collector (`OTEL_EXPORTER_OTLP_ENDPOINT`).

Ask one focused question per ambiguity; don't guess heroically.

### App shape matrix

| User wants…                              | Use                | Entry point shape                  | Notes |
|------------------------------------------|--------------------|------------------------------------|-------|
| Web app (forms, real-time, UI state)     | **Sky.Live**       | `Std.Live.app cfg`                 | HTTP-first; SSE patches; sessions + cookies + routing built in. |
| HTTP / JSON API (no browser UI)          | **Sky.Http.Server**| `Server.listen 8000 [...]`         | Routes + middleware (CORS / rate-limit / logging / basic-auth). |
| Multi-tenant SaaS / dashboard            | **Sky.Live + auth-app gate** | `Live.app { consoleAuth = … }` | Tenant scope enforced at SQL layer. |
| Background job / cron worker             | **Sky.Cli**        | `main = Task.run scheduledWork`    | No UI loop; `Task.parallel` for fan-out. |
| Terminal UI (TUI)                        | **Sky.Tui**        | `Std.Tui.app cfg`                  | Same view code as Sky.Live. |
| One-shot CLI tool                        | **Sky.Cli**        | `main = Task.run cliCmd`           | Argparse via `System.args`. |
| Desktop app                              | **Sky.Webview**    | `Std.Webview.app cfg`              | macOS today; Linux / Windows later. |
| WebSocket-driven feed                    | **Sky.Http.Server.WebSocket** | `Server.upgrade req` | Bidirectional. |
| Server-sent stream (LLM tokens, SSE)     | **Sky.Http.Server.Stream** | `Server.Stream.emit` | Mirror of `Sky.Core.Http.Stream`. |

### Pinned defaults (always apply unless the user overrules)

| Concern              | Default                                                          |
|----------------------|------------------------------------------------------------------|
| View layer           | `Std.Ui` (typed no-CSS DSL).  `Std.Html` only for wrapping raw markup. |
| Auth                 | `Std.Auth` — bcrypt + HS256 JWT cookies. Secrets are typed `String`. |
| Forms with passwords | `Ui.form [Ui.onSubmit DoSignIn]` with typed record arg.  Never per-keystroke `onInput` on password. |
| DB                   | `Std.Db` + SQLite for prototypes; PostgreSQL for multi-instance deploys. |
| Money / decimals     | `Std.Money` on `Std.Decimal`.  Never raw `Float` for currency. |
| Concurrency          | `Cmd.batch` / `Task.parallel`.  In-process pub/sub via `Cmd.publish` + `Sub.subscribeTopic`. |
| Observability        | `Std.Log` structured logs; dev console auto-mounted at `/_sky/console`; `OTEL_EXPORTER_OTLP_ENDPOINT` for external collector. |
| Errors               | `Result Error a` / `Task Error a`.  Never `String` as error type. |
| No raw HTML / JS     | `Std.Ui` HTML-escapes everything.  `data-sky-eval` forbidden. |

### `sky.toml` shape per decision

An author configures at most these sections. Precedence: **process env >
`.env` > `sky.toml`**. Secrets never go in `sky.toml` — auth secret comes
from `SKY_AUTH_TOKEN_SECRET` (≥32 bytes).

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

[log]
format = "json"                 # plain (default) / json
level  = "info"                 # debug / info / warn / error
```

### Production gate — surface to the user

Dev console + banner + metrics endpoint lock down when `ENV` (then
`SKY_ENV`) is anything other than unset/`dev`/`development`/`local`. When
the user mentions "deploy"/"production"/"Cloud Run"/"Kubernetes":

* Confirm `ENV=production` will be set on runtime.
* Confirm `SKY_AUTH_TOKEN_SECRET` ≥32 bytes.
* Confirm `SKY_CONSOLE_AUTH` set (`token` or `app`) — production + unset
  refuses to mount `/_sky/console`.
* Confirm session store is NOT `memory` when >1 replica.

Production-grade = survives restart, scales horizontally without losing
state, refuses cross-tenant reads (SQL-WHERE gate), no permanent error
banner on transient failures, structured logs every operator can trace.

## Effect boundary — Task-everywhere

Single rule: **every observable side effect returns `Task Error a`.**

| Tier | Type | Examples |
|---|---|---|
| Pure | bare `a` | `String.length`, `List.map`, `Crypto.sha256`, `Encoding.base64Encode`, `Time.timeString`, `System.getenvOr` |
| Fallible-pure | `Result e a` / `Maybe a` | `String.toInt`, JSON decoders, `Encoding.base64Decode`, `Auth.hashPassword` |
| Effects | `Task Error a` | `File.*`, `Http.*`, `Process.run`, `Io.*`, `Db.*`, `Auth.{register, login, setRole}`, `Crypto.{randomBytes, randomToken}`, `Time.{sleep, now, unixMillis}`, `Random.*`, `Log.*`, `System.*` (except `getenvOr`) |
| Diverging | `Int -> a` | `System.exit` (polymorphic return — never comes back) |

**Default-supplied helpers stay bare** — `System.getenvOr key def : String`,
`Maybe.withDefault`, `Result.withDefault`, `Db.getString`/`getInt`/`getBool`:
default plugs the failure case at the call site.

**Auto-force `let _ = TaskExpr`.** The compiler forces the discarded
expression so the side effect fires:

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

No `Result.fromTask` by design — keep effectful pipelines in Task; the
runtime entry boundary (CLI `main`, `Cmd.perform`, HTTP handler return)
executes them.

**Two-level error pattern:**

1. `errId = Crypto.randomToken 4` — short correlation ID.
2. `Log.errorWith op [ "errId", errId, "error", Error.toString e ]` — server-side structured log.
3. `Task.fail (Error.unexpected ("Operation failed (ref " ++ errId ++ ")"))` — user-facing message.

Per app shape: CLI → `Task.run … |> Task.onError reportError`;
Sky.Http.Server → `Task.onError` recovers to a 4xx/5xx Response;
Sky.Live → `Cmd.perform task ResultMsg`, dispatch updates
`notification` / `historyError` in Model.

## Standard library

Source: `sky-stdlib/{Sky/Core,Std,Sky/Http}/*.sky`. `sky doc Module`
surfaces every entry.

Each stdlib binding is either pure Sky (a recursive/case-based impl) or an
`Ffi.kernel "Name"` alias — a Sky-source decl with an HM signature whose body is
`Ffi.kernel "Mod_func"`; the compiler routes such call sites directly to the
existing typed runtime kernel (no runtime overhead, `sky doc` still lists it).
You only touch `Ffi.kernel` when authoring/registering stdlib modules, not in
normal app code.

### Pure (no I/O, no Task wrap)

| Module | Path | Key functions |
|---|---|---|
| `Basics` | `Sky.Core.Basics` (autoloaded via `Sky.Core.Prelude`) | identity, always, not, toString, modBy, clamp, fst, snd, compare, negate, abs, sqrt, min, max |
| `String` | `Sky.Core.String` | length, reverse, append, split, join, contains/containsIn, startsWith/startsWithIn, endsWith/endsWithIn (haystack-first In-suffixed), toInt, fromInt, toFloat, fromFloat, toUpper, toLower, trim/trimStart/trimEnd, replace, slice, dropLeft, dropRight (Elm-shaped rune-based), isEmpty, fromChar, toList, fromList, repeat, padLeft, padRight, casefold, equalFold, isEmail, isUrl, words, lines, concat |
| `List` | `Sky.Core.List` | map, filter, foldl, foldr, length, head, tail, take, drop, append, concat, concatMap, reverse, member, any, all, range, zip, find, isEmpty, indexedMap, cons + reverseHelp/indexedMapHelp |
| `Dict` | `Sky.Core.Dict` (kernel) | empty, insert, get, remove, member, keys, values, toList, fromList, map, foldl, union |
| `Set` | `Sky.Core.Set` (kernel) | empty, insert, remove, member, union, diff, intersect, fromList, toList, size |
| `Maybe` | `Sky.Core.Maybe` | withDefault, map, andThen, map2-5, andMap, combine, isJust, isNothing |
| `Result` | `Sky.Core.Result` | withDefault, map, andThen, mapError, map2-5, andMap, combine |
| `Math` | `Sky.Core.Math` | abs, min, max; sqrt, pow, cbrt, hypot; exp, exp2, log, log2, log10; floor, ceil, round, trunc; sin, cos, tan; asin, acos, atan, atan2; sinh, cosh, tanh, asinh, acosh, atanh; mod, remainder; pi, e, phi, sqrt2, inf, nan |
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
| `Decimal` | `Std.Decimal` | Arbitrary-precision arith. Banker's round, percent helpers. |
| `Money` | `Std.Money` | Currency-typed Money on Decimal + ISO 4217 enum (50+ codes+crypto). `allocate` (fair split), conversion rates. |

### Effects (`Task Error a`)

| Module | Path | Key functions |
|---|---|---|
| `Task` | `Sky.Core.Task` | succeed, fail, map, andThen, perform, sequence, parallel, lazy, run, fromResult, andThenResult, mapError, onError; **retryWith** + `RetryPolicy e` + `ShouldRetry e` ADT (RetryAlways \| RetryWhen (e -> Bool)). Build via linearBackoff/exponentialBackoff/defaultRetryPolicy; decorate via withJitter/withMaxAttempts/withBaseMs/withKind/withRetryOn. |
| `Cmd` | `Std.Cmd` | none, batch, perform, publish (echo-by-default pub/sub from update return), publishNoEcho (opt-out echo) |
| `Sub` | `Std.Sub` | none, every, batch, subscribeTopic (pub/sub receive) |
| `PubSub` | `Std.PubSub` | publish (Task-shaped, callable from raw `api` handlers/post-init/scheduled jobs; complements `Cmd.publish`), publishNoEcho (Task-shaped no-echo) |
| `Time` | `Sky.Core.Time` | now, sleep, every, unixMillis, format/formatISO8601/formatRFC3339/formatHTTP, addMillis, diffMillis, timeString |
| `Std.Time` | `Std.Time` | IANA zones, addMonths/Years (month-end CLAMPED), dayOfWeek (ISO Mon=1..Sun=7), weekOfYear (ISO 8601), startOfDay/Week/Month/Year, diffDays/Hours/Minutes/Seconds; `*Utc` infallible companions (`dayOfWeekUtc`/`startOfDayUtc`/`yearUtc`/etc — `Int -> Int`, plug "UTC" at call site). |
| `Random` | `Sky.Core.Random` | int, float, range, choice, shuffle, weighted (entropy-backed); seed, seededInt, seededFloat, seededChoice (deterministic) |
| `Http` | `Sky.Core.Http` | get, post, request (custom method/headers/body/timeout via `HttpRequest`), defaultRequest/withMethod/withHeader/withTimeout/withBody builders, parseQuery; typed `HttpResponse = { status : Int, body : String, headers : Dict String String }` |
| `File` | `Sky.Core.File` | readFile, readFileLimit, readFileBytes, writeFile, append, exists, remove, mkdirAll, readDir, isDir, tempFile, tempDir, copy, rename |
| `Io` | `Sky.Core.Io` | readLine, writeStdout, writeStderr |
| `System` | `Sky.Core.System` | args, getArg, getenv, getenvOr (bare), getenvInt, getenvBool, setenv, unsetenv, cwd, loadEnv, exit |
| `Process` | `Sky.Core.Process` | run (subprocess) |
| `Db` | `Std.Db` | open, connect, close, exec, execRaw, query, insertRow, getById, updateById, deleteById, findOneByField, findManyByField, findByConditions, unsafeFindWhere, queryDecode, withTransaction, migrate (versioned forward-only schema migrations + `_sky_migrations` + checksum guard), getField, getString, getInt, getBool. **Typed param binding**: `SqlValue` ADT (`SqlString`/`SqlInt`/`SqlFloat`/`SqlBool`/`SqlBytes`/`SqlDecimal`/`SqlTime`/`SqlMoney`/`SqlNull SqlValue`) — mixed-type SQL params as homogeneous `List SqlValue` for `INSERT … VALUES (?, ?, ?)` mixing `String + Maybe Int + Bool`. 8 `fromMaybe*` helpers for nullable columns. `SqlField` (`SetField SqlValue`/`OmitField`) + `Db.updateFields conn table whereCols setFields` for PATCH w/ column-omit; `Db.insertFields conn table fields` = INSERT counterpart (`OmitField` cols drop from SQL so DB applies DEFAULT; all-omit → `INSERT … DEFAULT VALUES`); `Db.insertFieldsReturning conn table fields projection decoder` appends `RETURNING <projection>`, decodes via `Std.Db.Decode`. Money serialises lossless as `"ISO_CODE AMOUNT"` TEXT, paired w/ `Db.Decode.money`. `Maybe a` params bind directly (nil/unwrapped). `nullable : Decoder a -> Decoder (Maybe a)`. |
| `Auth` | `Std.Auth` | register, login, setRole (Task) + hashPassword, hashPasswordCost, verifyPassword, passwordStrength, signToken, verifyToken (Result); signTokenWithClaims/verifyTokenWithAlgorithm — typed-builder aliases over `Sky.Core.Jwt` for fine-grained algorithm+claims control |
| `Log` | `Std.Log` | println, debug, info, warn, error, debugWith, infoWith, warnWith, errorWith |
| `Trace` | `Std.Trace` | span, event, attr — opt-in app-level tracing spans. Tier-1 spans (HTTP/session/Msg/DB/Auth/Http/File) automatic. |
| `Server` | `Sky.Http.Server` | param, queryParam, header, getCookie, static (Layer 3 surface); higher-level `get/post/listen/text/json/html` are kernel-only |
| `Stream` | `Sky.Http.Server.Stream` | stream, emit, finish, withContentType — server-side streaming HTTP responses (SSE/LLM token forwarding/chunked downloads). Mirror of `Sky.Core.Http.Stream`. Sync bridge: `Sky.Core.Http.Stream.forEachChunk hdl body` drains an upstream stream from inside a plain Sky.Http.Server handler (relay shape). |
| `Middleware` | `Sky.Http.Middleware` | withCors, withLogging, withBasicAuth, withRateLimit |
| `Head` | `Std.Live.Head` | Per-page `<head>` injection — `title`/`meta`/`metaProperty` (OG)/`link`/`canonical`/`jsonLd`/`themeColor`/`rss`. Opt in via optional `head : Model -> List (Html msg)` field on `Live.app` cfg. |
| `Console` | `Std.Live.Console` | `Identity` type alias (`{ subject, email, claims : Dict String String }`) for optional row-poly `consoleAuth : Request -> Task Error (Maybe Identity)` field on `Live.app` cfg. |
| `RateLimit` | `Sky.Http.RateLimit` | allow |
| `WebSocket` | `Sky.Core.WebSocket` (client) + `Sky.Http.Server.WebSocket` (server) | Bidirectional sockets. Client: `connect`/`connectWith`/`send`/`sendBinary`/`close`/`closeWithCode` (Task-tier) + `onOpen`/`onMessage`/`onClose`/`onError` (Sub-tier). Server: `upgrade` (returns from a Sky.Http.Server handler) + `sendToClient`/`sendBinaryToClient`/`broadcast`/`closeClient`. Server production gate: empty `originPatterns` returns 403 when `ENV=production`. |
| `Cache` | `Std.Cache` | LRU+TTL in-memory cache, `Cache k v` parametric on key+value. `CacheCfg` w/ `defaultCfg` + `withMaxEntries`/`withTTL`/`withMaxBytes`. `new`/`get`/`put`/`remove`/`clear`/`size`/`stats`. |
| `Email` | `Std.Email` | Resend/SES/SendGrid/SMTP under one `EmailProvider` ADT. `EmailMessage`+`Attachment` records w/ `defaultMessage { from, to, subject }` + `with*` builders (`withCc`/`withBcc`/`withTextBody`/`withHtmlBody`/`withAttachment`/`withReplyTo`). `Email.send provider msg : Task Error String`. |
| `Compression` | `Std.Compression` | `gzip`/`gunzip` (RFC 1952) + `zstdCompress`/`zstdDecompress` (RFC 8478). Operates on `String` (Bytes alias). |
| `Csv` | `Std.Csv` | `parse`/`parseWithDelimiter` (returns `Csv = { header, rows }`), `encode`/`encodeWithDelimiter` (RFC 4180 quoting), `parseStreamFromFile`. |
| `Config` | `Std.Config` | Typed TOML/YAML/JSON decoders mirroring `Sky.Core.Json.Decode`'s shape — `string`/`int`/`float`/`bool`/`nullable`/`field`/`at`/`list`/`succeed`/`fail`/`map`/`andThen`. `decodeToml`/`decodeYaml`/`decodeJson` + `loadFromFile`. |
| `ToString` | `Sky.Core.ToString` | `fromInt`/`fromFloat`/`fromBool`/`fromTime` route to canonical kernels — default to `ToString.fromInt n` over memorising per-type kernels. |
| `Pure` | `Sky.Core.Pure` | Uniform `() -> Task Error a` companions for arity-0 stdlib bindings (`uuidV4`/`uuidV7`/`timeNow`/`timeUnixMillis`/`systemArgs`/`systemCwd`/`systemLoadEnv`/`ioReadLine`/`dbConnect`). |

### Diverging

`System.exit : Int -> a` — process termination, polymorphic return.

**Stdlib typed-record convention.** Every typed-record surface ships a
`default*` ctor + `with*` builder per field — always compose via builders
so future field additions don't break call sites.

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
session stores (memory/sqlite/redis/postgres/firestore), type-safe events,
VNode diffing.

**`init` is per-session, not per-page-reload.** First request from a
browser with no `sky_sid` cookie fires `init`. Browser reload while the
session is alive RESTORES Model from the session store — `init` does NOT
run. Force fresh `init` (demo reset/e2e bootstrap): `Cmd.perform
(Cookie.expire "sky_sid")` then reload. If the goal is "my other tab missed
an update", use `Cmd.publish` instead.

### init's `req` shape

`init` receives a `req` value carrying full request context:

| Field | Type | Source |
|---|---|---|
| `req.path` | `String` | URL path |
| `req.query` | `String` | raw `?...` (parse via `Sky.Core.Http.parseQuery` if needed) |
| `req.params` | `Dict String String` | matched-route `:name` segments |
| `req.method` | `String` | request method |
| `req.headers` | `Dict String String` | request headers, canonical case |
| `req.cookies` | `Dict String String` | parsed cookies |

Session bootstrap in init is a one-line read:

```elm
init req =
    let sid = Maybe.withDefault "" (Dict.get "sky_sid" req.cookies) in
    ( { session = lookupSession sid }, Cmd.none )
```

Apps ignoring `req` build unchanged (row-poly extension).

### Per-page `<head>` injection

Optional `head : Model -> List (Html msg)` field on `Live.app` cfg.
Runtime calls it once per full GET (initial load + sky-nav navigation),
splices the returned list into `<head>` after required `<meta charset>`/
`<meta viewport>`/`<meta sky-base>` tags. HM sig is row-open — apps
omitting the field type-check and build unchanged.

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

`Std.Live.Head` helpers (all return `Html msg`):

| Helper | Emits |
|---|---|
| `title : String -> Html msg` | `<title>…</title>` |
| `meta : String -> String -> Html msg` | `<meta name="…" content="…">` |
| `metaProperty : String -> String -> Html msg` | `<meta property="…" content="…">` (Open Graph) |
| `link : List (String, String) -> Html msg` | `<link …>` with arbitrary attr pairs |
| `canonical : String -> Html msg` | `<link rel="canonical" href="…">` |
| `jsonLd : String -> Html msg` | `<script type="application/ld+json">…</script>` (raw JSON body) |
| `themeColor : String -> Html msg` | `<meta name="theme-color" content="…">` |
| `rss : String -> String -> Html msg` | `<link rel="alternate" type="application/rss+xml" …>` |

Pair with `Std.Html.node "link" […] []` for cases the helpers don't cover.

**SSE patches scope to `<body>`** — head updates require a full reload.
For UI swapping `<head>` on every Msg: drop the `head` field and emit
`<title>`/`<meta>` inside `view` via `Html.node` — the diff layer patches
normal DOM nodes regardless of position.

### URL routing + history

`routes` maps URL paths to Page values. The runtime matches incoming URLs
in declaration order, captures `:param` segments, and constructs the Page
with captured values (always `String`). Declaration order matters —
literals before patterns (`/apps/new` before `/apps/:slug`, or "new"
matches as a slug).

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

**URL-from-Page** (address bar in step with programmatic `Navigate` Msgs):
emit a sentinel `<div>` w/ `data-sky-path` on every render. The runtime
pushes/replaces history when the value differs from `location.pathname`.

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

`data-sky-path` is typed (no JS-in-string, works under strict CSP, no XSS
surface). Leave the element in the DOM after the runtime processes it —
removing it orphans its `sky-id`. The path-check keeps the call idempotent.

For **link navigation**, add `sky-nav` to `<a>` — the runtime intercepts
the click, fetches the URL, full-body-patches, pushes history. No app code
needed. **Back/Forward** is handled by the runtime's popstate listener.

```elm
Html.a [ Attr.href "/apps", Attr.attribute "sky-nav" "" ] [ Html.text "Dashboard" ]
```

`data-sky-eval` (runs an attribute via `new Function()`) is
CSP-incompatible — use `data-sky-path` for URL updates.

**Auth gates around routes.** For public-vs-authenticated apps:

- Let Sky.Live route the URL to a page as usual.
- In `pageBody`/view, outer-case on `model.session`: signed-out always
  renders the sign-in surface regardless of page.
- Use a single `currentPath : Model -> String` (not per-page
  `pathForPage`) returning the sign-in URL when `session = Nothing`, else
  dispatching on `model.page` — the address bar follows what the user sees.

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

**Slug ↔ subdomain convention.** Apps under a wildcard domain
(`*.platform.app`) → prefer slug-keyed URLs (`/apps/<slug>`) — bookmarkable,
follows renames. Carry the slug on the Page constructor; handlers needing
a numeric id resolve via a `findBySlug` helper.

### Async commands

`update msg model` returns `(Model, Cmd Msg)`. `Cmd.perform task toMsg`
runs the task asynchronously; the result dispatches back as a Msg.

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

A radio's `input` event reports `checked=True` (Bool), not the chosen
value. Bind a fully-applied Msg per choice via `onClick`:

```elm
label [ for "role-guardian", onClick (UpdateRole "guardian") ]
    [ input [ type "radio", name "role", value "guardian", id "role-guardian" ] []
    , text "Guardian"
    ]
```

`for`/`id` pairing lets the browser toggle the radio natively; `onClick`
carries the typed Msg.

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

1. **Password managers** watch DOM mutations on password inputs.
   Server-driven re-render w/ `value=…` triggers a re-prompt/re-fill cycle.
2. **The secret never lives in Model** — no `onInput UpdatePassword` Msg →
   no Model field → never serialised into the session store.
3. **Race-free submit** — form submit reads the live DOM value, not a
   debounced keystroke.

The `DoSignIn AuthCreds` ctor takes a typed record; form data is decoded
directly into it, case-insensitively. No per-Msg decoder boilerplate.

### Connection status banner

Bottom-pinned, three states:

- **connected** — hidden.
- **reconnecting** — amber `Reconnecting…`. Shown when SSE drops or a POST
  fails; 500 ms grace before painting.
- **offline** — red `Connection lost — refresh to retry`. The runtime keeps
  retrying in the background so a healed proxy recovers without a refresh.

Localise via `status = { reconnecting = "Reconnexion…", offline =
"Connexion perdue" }` on the `Live.app` cfg record. Partial overrides fall
back to English defaults. Strings are rendered via `textContent` (never
`innerHTML`).

### Input preservation across re-renders

The runtime preserves uncontrolled inputs across patches: empty patches
are JSON-acked (preserving password/uncontrolled fields), full-body swaps
preserve every uncontrolled INPUT/TEXTAREA/SELECT, and patches targeting a
focused/open `<select>` are skipped and reconciled on the next
interaction. Author takeaway: leave password fields uncontrolled (no
`value`, no `onInput`) and they survive re-renders.

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

**Handler annotation.** Named handlers ascribe at head position w/ the
`Handler` alias:

```elm
import Sky.Http.Server exposing (Handler)

getUser : Handler
getUser req = ...
```

`Handler` is a transparent alias for `Request -> Task Error Response`,
exported from `Sky.Http.Server`. Long-form `: Request -> Task Error
Response` still works. The same pattern works for any function-typed alias:
`view : Renderer Msg`, `decodeUser : Decoder User`, etc.

### Dev console

Every Sky.Live/Sky.Http.Server app auto-mounts a `Std.Ui` dev console at
`/_sky/console` in dev mode, alongside structured logging, a Prometheus
`/_sky/metrics` endpoint, and distributed tracing. In production
(`ENV`≠dev) the console + banner are removed and metrics require auth.

## Std.Ui — typed no-CSS layout DSL

Layered above `Std.Html`; renders to inline-styled HTML server-side. Pick
`row`/`column`/`el` for layout, attach typed attrs from `Background`/
`Border`/`Font`/`Region` sub-modules — never write CSS.

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

### Four idioms to get right

1. **Forms with sensitive inputs use `Ui.form` + `onSubmit DoSignIn`, NOT
   `onInput` per keystroke on password fields.** See the password pattern
   in the Sky.Live section.

2. **Real `<input>` elements use `Ui.input`, NOT `Ui.el [htmlAttribute
   "type" "text"]`.** `Ui.el` builds a Node rendering as `<div>` — browsers
   ignore `type=`/`value=` on non-inputs.

3. **Std.Ui-heavy modules (~25+ polymorphic `Element Msg` helpers) MUST be
   split across multiple modules.** A monolithic `Main.sky` can blow the HM
   type-checker heap. Canonical split: `State.sky` (types, no Std.Ui
   imports) / `Update.sky` / `View/Common.sky` / one View module per page /
   `Main.sky` dispatcher.

4. **`Input.*` size/layout attrs apply to the wrapper; form attrs stay on
   the inner control.** Every `Std.Ui.Input.*` call
   (text/multiline/email/username/search/currentPassword/newPassword/
   slider/checkbox/radio/radioRow) routes layout attrs
   (`Ui.width`/`Ui.height`/`Ui.padding`/`Ui.spacing`/`Ui.alignX`/
   `Ui.alignY`/`Ui.nearby`/`Ui.pointer`/`Ui.overflow`) to the outer
   wrapper, while form/event/visual attrs stay on the inner
   `<input>`/`<textarea>`. So `Input.multiline [Ui.height Ui.fill] {...}`
   inside a column-fill parent fills the parent, and
   `Background.color (Ui.rgb 240 240 240)` colours the textarea itself.

### `Ui.fill` semantics

`Ui.fill` lowers asymmetrically per the parent's flex direction: main-axis
fill grows; cross-axis HEIGHT fill (row child) stretches by default;
cross-axis WIDTH fill (column/el/textColumn child) sets `width: 100%`.
Authoring takeaway: `[Ui.width fill, Ui.centerX]` is the canonical
centred-page-content shape and works as expected.

### `Ui.layoutWith` — wrapper customisation

```elm
Ui.layoutWith { wrapperAttrs : [Attr msg], rootAttrs : [Attr msg] } -> Element msg -> Html
```

`wrapperAttrs` reach the outer 100vh `<div>` page wrapper (Background.color
for page-wide dark mode, Font.color/Font.family for document-wide
typography, Border/class/aria-*/data-*). `rootAttrs` apply to the root
element (same as `Ui.layout`'s argument). `Ui.layout attrs el` ≡
`Ui.layoutWith { wrapperAttrs = [], rootAttrs = attrs } el`. Reach for
`layoutWith` when the wrapper needs visual styles (dark page, custom font
cascade, page background image).

### Surface highlights

- **Entry points**: `layout : List Attr -> Element -> Html` +
  `layoutWith : { wrapperAttrs : List Attr, rootAttrs : List Attr } -> Element -> Html`.
- **Layout**: `el`, `row`, `column`, `wrappedRow`, `grid` + `gridColumns N`
  (CSS-Grid auto-fit), `paragraph`, `textColumn`, `text`, `none`, `html`.
- **Sized elements**: `link { url, label }`, `image { src, description }`,
  `button { onPress, label }`, `input`, `form onSubmit`.
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
  `onChange`, `onFocus`, `onMouseOver/Out`, `onKeyDown`, `onFile (String ->
  msg)`, `onImage (String -> msg)`.
- **File/image upload hints**: `fileMaxSize Int` (bytes, browser-side cap
  not security), `fileMaxWidth Int`, `fileMaxHeight Int`.
- **Colour**: `rgb`, `rgba`, `white`, `black`, `transparent`.
- **Sub-modules**: `Background` (color, image, linearGradient,
  hoverColor/focusColor/focusVisibleColor/activeColor/disabledColor),
  `Border` (color, width, widthEach, rounded, solid/dashed/dotted, shadow,
  glow, innerShadow, hoverColor/focusColor/activeColor/hoverWidth/
  hoverRounded), `Font` (color, family, size, weight,
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
  PseudoClass -> List (Attribute msg) -> Attribute msg` escape hatch.
  `PseudoClass`: `Ui.hover`, `Ui.focus`, `Ui.focusVisible`, `Ui.active`,
  `Ui.disabled`. `focusColor` targets `:focus-visible` (fires only on
  keyboard nav); use `Ui.onPseudo Ui.focus [...]` for sticky-focus.
  `:hover` rules are auto-wrapped in `@media (hover: hover)` (no
  sticky-hover on touch). Works on void elements (`<input>`, `<img>`, etc)
  too. Composes w/ `Ui.breakpoint` via nesting.
- **Media queries + breakpoints** (`Ui.mediaQuery`/`Ui.breakpoint`/
  `Breakpoint` ADT) — CSS-driven viewport-conditional styling with no JS
  round-trip, no Model field, no re-render. Typed `Breakpoint`: `Mobile`,
  `Tablet`, `Desktop`, `SmAndUp`, `MdAndUp`, `LgAndUp`, `XlAndUp`,
  `DarkMode`, `LightMode`, `ReducedMotion`, `TouchDevice`, `Portrait`,
  `Landscape`, `Custom Int Int` (minPx maxPx; 0=unset). `Ui.mediaQuery
  query [attrs] child` = escape hatch for a raw CSS media-query string.
  Sky.Tui ignores `<style>`; Sky.Webview honours media queries identically
  to Sky.Live. Pick `Ui.breakpoint` when the transition needs no typed Msg;
  pick `Std.Ui.Responsive` when it does.
- **Transitions + animations** (`Std.Ui.Transition`/`Std.Ui.Animation`/
  `Std.Ui.Transform`) — typed CSS transitions + keyframe animations on a
  Sky.Ui element; the browser handles frame timing. Both are auto-wrapped
  in `@media (prefers-reduced-motion: no-preference)` by default; opt out
  via `Transition.attributeUnsafe`/`respectReducedMotion = False` only when
  motion is semantically required (spinner, progress). `Transition.attribute
  [property "background-color", duration 200, easing easeOut]` builds a
  transition; pair w/ `Background.hoverColor`. `Animation.attribute { name,
  duration, easing, delay, iterations, fillMode, respectReducedMotion,
  keyframes }` builds a keyframe spec; `keyframes : List (Int, List
  Transform.Prop)` is `[(percent, [Transform.opacity 0.0,
  Transform.translateY 10]), ...]`. `Transform.{translateX, translateY,
  translate, scale, scaleXY, rotate, skewX, skewY, opacity}` are typed
  helpers.
- **Aspect ratio + grid tracks** (`Ui.aspectRatio`/`Ui.aspectRatioWH`/
  `Ui.square`/`Ui.widescreen`/`Ui.fullHd`/`Ui.cinemascope` +
  `Std.Ui.Grid.tracks`/`Grid.columns`/`Grid.rows`). `Ui.aspectRatio 1.777`/
  `Ui.aspectRatioWH 16 9` lock a width-to-height ratio (pair w/ `Ui.width
  Ui.fill`). `Std.Ui.Grid` exposes a typed `Track` ADT (`fr`, `px`, `auto`,
  `minContent`, `maxContent`, `minmax`, `repeat`, `repeatAutoFit`,
  `repeatAutoFill`). Lighter-weight `Ui.gridColumns N` stays for the
  common-case product-card grid.

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

The callback receives a data URL. Decode w/ `Std.Encoding.base64Decode` →
upload via `Http.post`. Ensure `[live] maxBodyBytes` ≥ your `fileMaxSize`.

## Sky.Tui

TEA backend rendering `Std.Ui` to ANSI cells. Same
`init`/`update`/`view`/`subscriptions` shape as Sky.Live.

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

**Logical-pixel canvas** — `canvasWidth × canvasHeight` defines the design
surface; the runtime converts `Ui.padding 8`/`Ui.px N` to cells. Covers
~95%+ of Std.Ui primitives; unsupported attrs (gradients, fine
letter-spacing, image fills) emit a deduped warning (`SKY_TUI_QUIET=1`
suppresses). Wide chars (CJK+emoji+ZWJ) supported.

**Sky.Cli password mode** — `Cli.readPassword : () -> Task Error String`
reads stdin with echo disabled; the password never echoes and never lands
in scrollback.

## Sky.Webview (desktop)

Cross-backend mirror of `Live.app`+`Tui.app` — same TEA shape, native
desktop window via the system webview. No HTTP server, no SSE, no session
store.

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

The same `view` fn paints identically across Sky.Live (web), Sky.Tui
(terminal), and Sky.Webview (desktop). `WindowCfg` is closed (`{ title :
String, size : (Int, Int) }`) today; macOS is supported first.

**Std.Ui convention** — the `view` fn MUST wrap its output in `Ui.layout []
(...)` to convert `Element` → `Html` before rendering. A raw `Ui.column
[...]` body produces a blank window (same convention as Sky.Live).

## Active limitations

Real current compiler limitations you must work around when writing code.

1. **No higher-kinded types.** HM only.
2. **No `where` clauses.** Use `let…in`.
3. **No custom operators.**
4. **Negative literal args need parens.** `f -1` parses as subtraction.
   Use `f (-1)`.
5. **`Dict.toList` typed-key inference is inline-only.** `Dict.toList
   (Dict.fromList [(1, "a")])` chained in one expression returns real `Int`
   keys. For let-bound intermediates — `let d = Dict.fromList […] in
   Dict.toList d` — routing falls back to the String-key path. Workaround:
   inline the chain, or pipe (`d |> Dict.toList`).
6. **`sky check` does not fully model FFI interface satisfaction.** Opaque
   FFI types unify with each other; concrete-satisfies-interface checks
   fall through.
7. **Zero-arg calls follow the binding's declared type.** Bare `Uuid.v4`
   works because its sig is `v4 : String`. `Time.now ()`/`Time.unixMillis
   ()` are needed because their sigs are `() -> Task Error a`. Calling a `:
   String` binding with `()` triggers a codegen bug for arity-0 kernels
   (`Uuid.v4 ()` mis-applies the unit); stick to the declared shape.
   Dict/Set/Maybe/Result stay bare for `empty`/`none` etc. For uniform
   `() -> Task Error a` shape, import `Sky.Core.Pure as Pure` and call the
   additive companions — `Pure.uuidV4 ()`/`Pure.uuidV7 ()`/`Pure.timeNow
   ()`/`Pure.timeUnixMillis ()`/`Pure.systemArgs ()`/`Pure.systemCwd ()`/
   `Pure.systemLoadEnv ()`/`Pure.ioReadLine ()`/`Pure.dbConnect ()`.
8. **Non-tail-recursive list ops are O(N) on the call stack.** `map`,
   `filter`, `foldr`, `length`, `concat`, `take`, `append`, `range`, `zip`,
   `concatMap`, `indexedMap`, `Maybe.combine`, `Result.combine` recurse.
   Tail-recursive ops (`foldl`, `find`, `any`, `all`, `member`, `drop`) are
   constant-stack. For very large lists (200k+ elements) prefer a
   tail-recursive accumulator pattern.
9. **Zero-arg `Css.*` keyword constants require `()`** — `Css.zero ()`,
   `Css.auto ()`, `Css.none ()`. The bare form is a type error.
10. **Multi-line function signatures.** `name\n    : T` (`:` on the
    continuation line) parses cleanly. Continuation INSIDE the type body
    (`T1\n    -> T2`) is unsupported — extract a `type alias` for the whole
    arrow type.

## Build & test CLI

```bash
sky init [name]                    # new project
sky build src/Main.sky             # compile → sky-out/app
sky run src/Main.sky               # build + run
sky watch src/Main.sky             # file-watch rebuild + restart
sky check src/Main.sky             # type-check + build
sky fmt src/Main.sky               # opinionated formatter (run after editing .sky/.skyi)
sky test tests/MyTest.sky          # Sky.Test runner
sky db status                      # Std.Db migrations: applied / pending / drift
sky db migrate                     # apply pending Std.Db migrations, then exit
sky doc Module                     # terminal docs
sky doc --serve [--port 8080]      # browsable HTTP doc server
sky doc --tui                      # interactive terminal doc browser
sky doc --list                     # list every documented module
sky doctor [--fix] [--verbose]     # project / environment health checks
sky console [--port 8025]          # standalone Std.Ui console (--tui for the Sky.Tui backend)
sky add <package>                  # add an FFI binding
sky remove <package>
sky install                        # regen missing FFI + deps
sky update                         # update deps
sky clean                          # remove sky-out/ dist/
sky lsp                            # JSON-RPC LSP server (stdio)
sky --version
```

**Never run `sky build` from the repo root** — it overwrites the compiler
binary in `sky-out/`. Always `cd` into the project/example dir first:

```bash
cd examples/01-hello-world && sky build src/Main.sky
```

`sky check` ≡ `sky build` (both invoke the Rust build on the emitted code).
Run `sky fmt` after editing `.sky`/`.skyi` files (the formatter is
idempotent).

# Typed cross-cutting config — one `config` binding for the whole app

Status: design proposal. Every fenced Ipê block illustrates the **proposed
surface**, not shipped API. This doc extends the framework-config design in
`config-design.md`; where the two touch, this doc defers to it and says so
explicitly (see *Reconciliation*).

## The problem this solves

Cross-cutting configuration — how the app logs, its database, its host binding,
its session/CSRF policy, its telemetry, its analytics consent, its email
transport — is expressed three different ways today, each with its own reader,
its own precedence, and (for the per-module records) its own stringly loader:

1. **Compiler-reserved shape settings** (`Web.appWith [ Host.bind …, Db.url … ]`)
   — already a typed, phantom-shape-checked front door, resolved once at startup.
2. **Per-module ad-hoc config records** (`Analytics.Config`, `Email.SmtpConfig`,
   `Email.SesConfig`) — plain records the app threads by hand, with **plaintext
   `String` credential fields**.
3. **~40 loose `IPE_*` environment variables** read directly at their use sites
   across the runtime, each with an inline default and no typed front door.

The goal is a single top-level `config` binding: one typed value, one
documented precedence, one parse-once boundary, where a missing required secret
is a load-time failure rather than a silent empty string, and where a secret
field is *structurally distinguishable* so it can never be printed by accident.

---

## Current-state enumeration

### A. Compiler-reserved shape settings (the existing front door)

The `Web.appWith` settings list is a real, implemented surface. The builder
symbols are compiler-reserved (not stdlib `.ipe`), the argument enums are closed
reserved ADTs, and the whole thing resolves through one runtime carrier.

| Surface | Configures | Expressed as | Read at |
|---|---|---|---|
| `Web.appWith settings { … }` entry | attaches a settings list to a Web app | reserved TEA app entry | `src/compiler/canon/src/resolve.rs:1284` (`("Web","appWith")` in `TEA_APP_ENTRIES`) |
| `Host.bind : HostMode -> Setting a` | host bind mode | reserved builder + closed `HostMode` ADT | carrier `src/runtime/rust/src/app_config.rs:101`; resolved `app_config.rs:264` (`resolve_host_bind`) |
| `Log.level : LogLevel -> Setting a` | min log severity | reserved builder + closed `LogLevel` ADT | carrier `app_config.rs:112`; resolved `app_config.rs:292` |
| `Db.url : Secret -> Setting a` | database URL | reserved builder, `Secret`-typed arg | carrier `app_config.rs:132`; resolved `app_config.rs:495` (`resolve_db_url_override`) + read in `config.rs:20` |
| `Web.csrf : CsrfMode -> Setting Web` | CSRF posture | reserved builder + closed `CsrfMode` ADT (no disabling variant) | carrier `app_config.rs:141`; resolved `app_config.rs:313` |
| `Web.sessionTtl : Int -> Setting Web` | session lifetime (s) | reserved builder | carrier `app_config.rs:147`; resolved `app_config.rs:347` |
| `Web.authMaxLifetime : Int -> Setting Web` | absolute token cap (s) | reserved builder | carrier `app_config.rs:155`; resolved `app_config.rs:377` |
| `Web.authSlideWindow : Int -> Setting Web` | rolling re-issue window (s) | reserved builder | carrier `app_config.rs:164`; resolved `app_config.rs:422` |
| `Web.withRevocation : RevocationMode -> Setting Web` | per-request revocation gate | reserved builder + closed `RevocationMode` ADT | carrier `app_config.rs:175`; resolved `app_config.rs:471` |
| `App.fromEnv : String -> Secret` | seals a named env var into a `Secret` | reserved builder | `app_config.rs:123` (`ipe_app_from_env`) |

Reserved names live in `src/compiler/canon/src/resolve.rs` (the closed config-tag
ADTs `HostMode`/`LogLevel`/`CsrfMode`/`RevocationMode` at `resolve.rs:241`,
`resolve.rs:447`). The process-wide resolved value is a single `ResolvedConfig`
installed once via `install_web` behind a `OnceLock` (`app_config.rs:202`,
`app_config.rs:215`, `app_config.rs:222`) — a redundant second install is a
no-op. This is the good pattern the rest of config should converge onto.

### B. Per-module ad-hoc config records (the stringly gap)

These are plain records the app constructs and threads by hand. They are the
main offenders: **credentials as plaintext `String`**, no env sourcing, no
front-door composition.

| Surface | Configures | Expressed as | Defined at | Security note |
|---|---|---|---|---|
| `Email.SmtpConfig` | SMTP host/port/user/pass | `type alias { host, port, user, pass : String/Int }` | `src/stdlib/Ipe/Email.ipe:103` | `pass : String` — plaintext secret in source |
| `Email.SesConfig` | SES region/key/secret | `type alias { region, key, secret : String }` | `src/stdlib/Ipe/Email.ipe:95` | `key`/`secret : String` — plaintext secret in source |
| `Email.EmailProvider` | which transport | `Resend String \| Ses … \| SendGrid String \| Smtp …` | `Email.ipe:112` | `Resend`/`SendGrid` API keys are bare `String` |
| `Analytics.Config` | sink + consent + identity | `type alias { sink, consent, userId, traits }` | `src/stdlib/Ipe/Analytics.ipe:272` | not a secret, but a per-module `defaultConfig`/`withX` loader; consent defaults `Pending` (fail-closed) |

`Email` builders (`defaultSmtpConfig`, `withSmtpPass`, `withSesSecret`) at
`Email.ipe:222`–`259` set these plaintext fields. `IPE_EMAIL_DRY_RUN` gates
send at runtime (`src/runtime/rust/src/email.rs:162`).

### C. Loose environment variables (the ~40-var stringly surface)

Read directly at their use sites, each with an inline default. Grouped by
subsystem; representative file:line for each. This is the surface the issue
calls "unchecked — a typo is a silent fallback." An operator has no typed
manifest of what exists; the list is discoverable only by grepping the runtime.

| Subsystem | Env vars | Read at (representative) |
|---|---|---|
| Host / server | `IPE_HTTP_BIND`, `IPE_WEB_PORT`/`IPE_LIVE_PORT`, `IPE_HTTP_MAX_BODY_BYTES`, `IPE_WEB_MAX_BODY_BYTES`/`IPE_LIVE_*`, `IPE_TRUSTED_PROXY`, `IPE_HTTP_DENY_PRIVATE` | `app_config.rs:266`, `web/mod.rs:2248`, `http_client.rs:499`, `server.rs:732/831`, `ssrf.rs:42` |
| Sessions / auth | `IPE_WEB_TTL`/`IPE_LIVE_TTL`, `IPE_AUTH_MAX_LIFETIME`, `IPE_AUTH_SLIDE_WINDOW`, `IPE_AUTH_REVOCATION`, `IPE_WEB_MAX_SESSIONS` | `app_config.rs:348/381/426/472`, `web/mod.rs:532` |
| CSRF | `IPE_CSRF` | `web/csrf.rs:77` |
| Database | `DATABASE_URL`, `IPE_DB_MAX_CONNECTIONS`, `IPE_DB_MAX_POOLS` | `config.rs:24`, `config_postgres.rs:24`, `db.rs:787/800` |
| Logging | `IPE_LOG_LEVEL`, `IPE_LOG_FORMAT` | `log.rs:38/51`, `core.rs:121` |
| Telemetry / console | `ENV`/`IPE_ENV`, `IPE_DEV_BANNER`, `IPE_CONSOLE_URL`/`_EMBED`/`_AUTH`/`_TOKEN`/`_DB_PATH`, `IPE_ADMIN_TOKEN`, `IPE_METRICS_TOKEN`, `IPE_INGEST_TOKEN`, `IPE_TRACE` | `telemetry.rs:163/200/216`, `web/console.rs:214/223/376`, `trace.rs:21` |
| Email | `IPE_EMAIL_DRY_RUN` | `email.rs:162` |
| WebSocket | `IPE_WS_MAX_MESSAGE_BYTES`, `IPE_WS_SEND_BUFFER`, `IPE_WS_HEARTBEAT` | `ws_client.rs:246`, `server.rs:1141/1152` |
| Ingest limits | `IPE_FILE_READ_MAX`, `IPE_CONFIG_MAX_BYTES`, `IPE_YAML_MAX_BYTES`, `IPE_DECOMPRESS_MAX_BYTES`, `IPE_CSV_MAX_ROWS`, `IPE_CSV_SANITIZE_FORMULAS`, `IPE_HTTP_MAX_BODY_BYTES`, `IPE_PROCESS_OUTPUT_MAX`, `IPE_RECURSION_LIMIT` | `file.rs:68`, `config_decode.rs:116/231`, `compression.rs:36`, `csv.rs:86/121`, `system.rs:168`, `core.rs:929` |

Note the tokens (`IPE_ADMIN_TOKEN`, `IPE_METRICS_TOKEN`, `IPE_INGEST_TOKEN`,
`IPE_CONSOLE_TOKEN`) — these are secrets sourced from env today with no typed
carrier at all.

### D. Two things that are NOT in scope (keep as-is)

- **`Ipe.Config`** (`src/stdlib/Ipe/Config.ipe`) — a typed `Decoder` over an
  *external* toml/yaml/json file the user's own app reads. This is concern 3 in
  `config-design.md`: already principled (parse-don't-validate at the app's own
  I/O boundary). The umbrella `config` must not collide with this name.
- **`Ipe.Env.public`** (`src/stdlib/Ipe/Env.ipe:32`) — the narrow allowlisted
  build-time public-config substitute for wasm bundles. Distinct, narrower
  surface; unchanged.

---

## Reconciliation with `config-design.md`

`config-design.md` already decided the load-bearing questions and delivered the
first slice. This doc does **not** re-litigate them; it inherits them:

- **Config is written in Ipê, not TOML.** Inherited verbatim.
- **Three concerns kept separate** (manifest / framework-runtime-config /
  user-app-config-files). This doc is entirely within *concern 2*
  (framework runtime config). It does not touch the `package.ipe` manifest
  (concern 1) or `Ipe.Config` file decoders (concern 3).
- **Phantom shape-tagged `Setting shape`.** Inherited. The umbrella `config`
  binding is a `List (Setting shape)` under the hood — this doc adds no second
  representation.
- **One precedence: `env > setting-in-code > built-in fallback`.** Inherited
  and, critically, **not duplicated**: the umbrella binding produces the exact
  same `List (Setting …)` that `Web.appWith` already resolves, so there is no
  second precedence to reconcile.
- **Secrets are `fromEnv`-only; no literal-`String` credential type-checks.**
  Inherited, and this doc *extends* it to the per-module records in section B
  (which today violate it with plaintext `String` fields).

**What this doc adds on top of `config-design.md`:**

1. A single named top-level `config` binding (a convention + one new reserved
   entry point) so the whole app's cross-cutting wiring reads from one place,
   rather than the settings living only inline in the `appWith` call.
2. Folding the per-module ad-hoc records (Email, Analytics — section B) into
   that same `Setting`-list vocabulary, replacing their stringly loaders and
   plaintext-secret fields.
3. Naming the loose-env surface (section C) as the built-in-fallback tier under
   the one precedence, and giving the security-relevant ones (tokens, db url,
   smtp/ses/api credentials) a `Secret`-typed setting so they stop being bare
   env reads with no typed carrier.

Where `config-design.md` says "attach settings via the additive `appWith`
field," this doc's `config` binding is the *source* of that list — it desugars
into exactly that field. It **supersedes nothing**; it is the ergonomic and
composition layer above the mechanism `config-design.md` already shipped.

---

## The unified `config` binding

### Shape

A program declares one top-level binding named `config`, whose value is built
from the same reserved `Setting` vocabulary the front door already uses. It is a
**`List (Setting shape)`** — plain data, no closures (see *Builder verdict*).

```elm
config : List (Setting Web)
config =
    [ Host.bind Host.loopback
    , Log.level Level.warn
    , Db.url (App.fromEnv "DATABASE_URL")      -- Secret: env-sourced, never inlined
    , Web.csrf Web.strict
    , Web.sessionTtl 3600
    , Web.withRevocation Store

    -- per-module cross-cutting config, same vocabulary (section B, migrated):
    , Email.smtp
        { host = "smtp.example.com", port = 587, user = "postmaster" }
        (App.fromEnv "SMTP_PASSWORD")           -- Secret arg, not a record field
    , Analytics.sink (Analytics.Jsonl "/var/log/events.jsonl")
    , Analytics.consent Analytics.Pending
    ]


main : Program …
main =
    Web.appWith config
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = routes, notFound = NotFound
        }
```

The binding is ordinary Ipê data: a `List` of `Setting shape` values. Its shape
tag is fixed by the app entry it is passed to (`Web.appWith` fixes `Web`), so
the list is homogeneously shape-checked — a `Terminal` app listing a `Web.csrf`
setting is a compile-time type error, exactly as `config-design.md` specifies.

`config` is a **convention name, plus one reserved recognition site** so the
toolchain (and a future `ipe config` inspector) can find the app's cross-cutting
wiring without executing it. Recognition is by the same discipline the compiler
already uses to read a reserved builder literal: a top-level binding named
`config` whose type is `List (Setting shape)`. Passing it to `appWith` is what
installs it; a `config` binding that is never passed to an app entry is inert
data (a lint may warn, mirroring the discarded-task lint posture).

### How per-module config composes in

Each subsystem module keeps owning its settings (SSOT: one definition site) and
exposes them as `Setting`-producing builders, so no module invents its own
stringly loader. Cross-cutting settings are `Setting any`; shape-specific ones
carry the shape tag. The per-module records from section B become
`Setting`-producing constructors instead of hand-threaded records:

```elm
-- Email (replaces SmtpConfig/SesConfig plaintext records):
Email.resend  : Secret -> Setting any                       -- API key, env-sourced
Email.sendgrid: Secret -> Setting any
Email.smtp    : { host : String, port : Int, user : String } -> Secret -> Setting any
Email.ses     : { region : String } -> Secret -> Secret -> Setting any   -- key, secret

-- Analytics (replaces the threaded Config record for the cross-cutting parts):
Analytics.sink    : Sink         -> Setting any
Analytics.consent : ConsentState -> Setting any             -- default Pending (fail-closed)
```

The non-cross-cutting parts of `Analytics.Config` (per-call `userId`/`traits`
identity threaded through a specific `track` call) stay as a call-local value —
they are request state, not app config, and do not belong in the umbrella. Only
the *app-lifetime* choices (sink, consent posture) migrate.

### Sourcing and parse-once semantics

Every value in `config` is one of exactly three kinds, and each is resolved
**once** at startup by the runtime resolvers that already exist
(`resolve_host_bind`, `resolve_db_url_override`, …):

1. **Literal in source** — a closed ADT constructor (`Host.loopback`,
   `Web.strict`, `Level.warn`) or a plain scalar (`Web.sessionTtl 3600`). These
   are the "setting-in-code" tier.
2. **Env-with-typed-parse** — `App.fromEnv "VAR"` seals a named env var into a
   `Secret`. This is the *only* way a secret enters config (section B's
   plaintext fields are removed). The seal happens once at startup
   (`ipe_app_from_env`, `app_config.rs:123`).
3. **Layered default+override** — the runtime applies the one precedence
   `env > setting-in-code > built-in fallback` per setting at resolution time.
   The loose-env surface (section C) *is* the top (env) and bottom (fallback)
   tiers of this ladder; the `config` binding is the middle tier.

Parse-once means: the app holds a validated `List (Setting …)` folded into the
immutable `ResolvedConfig` behind the `OnceLock` (`app_config.rs:222`). No use
site re-reads or re-validates — `resolve_*` reads the already-installed value
(applying only the env-override check, which is a lookup, not a re-parse). This
is the parse-don't-validate boundary for configuration.

### Fail-closed on missing required

The design distinguishes three failure postures, chosen per setting by its type,
never by a silent `""`:

- **Missing optional** → the built-in fallback stands (e.g. absent
  `Web.sessionTtl` → the safe default TTL). Already how the resolvers behave
  (`session_ttl_from` drops a non-positive/absent value to the caller default).
- **Missing required non-secret** → a **load-time typed diagnostic**. A setting
  the app *lists* but whose env source is absent is not silently defaulted where
  a default would mask a misconfiguration. The resolver returns a
  `ConfigError` variant naming the setting and its expected source; startup
  fails closed (the server does not bind).
- **Missing required secret** → **always fail-closed, never a silent default.**
  A `Db.url (App.fromEnv "DATABASE_URL")` where `DATABASE_URL` is unset must not
  silently fall back to a local sqlite string in a context where the app
  declared a required remote db. Today `ipe_app_from_env` seals the empty string
  on a missing var (`app_config.rs:123`) — acceptable as a fail-*safe* default
  for a genuinely optional secret, but the umbrella adds a **required** marker so
  a declared-required secret that resolves empty is a startup error, not an
  empty connection string that fails obscurely later. (See *Secret handling*.)

The rule: **a default is only chosen where a default is safe.** For anything
security-relevant (a secret, a bind mode, a token) the absence of a value is an
error, surfaced once at load with a typed diagnostic that names the setting.

---

## Secret handling (defense in depth)

The runtime already has the right primitive: `Secret` (`src/runtime/rust/src/secret.rs:82`)
is a newtype that (a) deliberately does **not** derive `Debug` — its `Debug`
prints a fixed `REDACTED` placeholder (`secret.rs:92`); (b) its `IpeStringify`
(`toString`/interpolation/`Log.*With`) also yields `REDACTED` (`secret.rs:103`);
(c) it zeroizes on drop (`secret.rs:114`); (d) it is never serde-serializable, so
a `Secret` in a Web `Model` is a compile-time rejection, not a session-store
leak (`app_config.rs:60`). Reveal is only via the greppable
`Ipe.Secret.Unsafe.unsafeReveal` escape hatch (`src/stdlib/Ipe/Secret/Unsafe.ipe:34`).

The umbrella design makes secret config **structurally distinguishable** by
building on this:

1. **Every credential setting takes a `Secret`, not a `String`.** `Db.url`
   already does (`app_config.rs:132`). The migration extends this to Email
   (`smtp` pass, `ses` key/secret, `resend`/`sendgrid` API keys) and to the
   token settings (admin/metrics/ingest/console tokens — section C), replacing
   their bare `String`/env reads. A hard-coded `String` credential does not
   type-check; a secret can only enter via `App.fromEnv`.
2. **A secret is never logged or serialized by default.** Guaranteed by the
   existing `Secret` type — no new mechanism needed, only *using* it everywhere
   a credential lives. This is the concrete payoff of section B's migration:
   `Email.SmtpConfig.pass : String` can be `toString`'d into a log line today;
   `Email.smtp … (App.fromEnv "SMTP_PASSWORD") : Setting` cannot.
3. **Required-secret fail-closed** (above): a `required` marker on a `fromEnv`
   secret turns "env var absent" from a silent empty seal into a named load-time
   error. Proposed as `App.fromEnvRequired "VAR" : Secret` (same seal, but the
   resolver treats an empty result as a `ConfigError` rather than an empty
   secret). `App.fromEnv` stays the fail-safe optional form.

No secret value ever appears in the `config` binding source — only a `fromEnv`
reference does. This is the "secrets unrepresentable as literals" invariant from
`config-design.md`, now enforced across *all* credential-bearing config, not
just `Db.url`.

---

## Builder verdict — desugars to data (no function-in-record)

The issue's sketch shows a `Config.default |> Config.withDatabase … |>
Config.withCsrf …` pipeline. **The recommended shape is a plain `List (Setting
shape)`, and the pipeline builders are rejected as unnecessary (YAGNI).** The
reasoning:

- **L0107 / TEA-only-state:** a `config` binding must be *data*, never a record
  of closures or a fold over handler functions. A `Config.withX : a -> Config ->
  Config` pipeline is a chain of *functions applied to data* — that is fine on
  its own — but it exists only to build a record, and if that record ever grew a
  function-typed field (a validator, a lazy source) it would violate L0107. The
  `List (Setting …)` shape makes the function-in-record failure mode
  **unrepresentable**: a `Setting` is a closed reserved carrier that holds only
  scalars, closed ADT tags, and `Secret`s (`app_config.rs:63`) — it cannot hold
  a function.
- **The list already *is* the composition.** `Config.withDatabase (Db.url …)` and
  `[ …, Db.url … ]` carry identical information. The `withX` layer buys nothing
  over list literal syntax — it adds a second vocabulary (`withDatabase` vs
  `Db.url`) for the same act, splitting SSOT. Cons-ing a setting onto a list is
  the composition; no builder type is needed.
- **If a builder pipeline were kept, its only sound desugaring is to `List
  (Setting …)`.** `Config.default` ⇒ `[]`; `cfg |> Config.withX v` ⇒ `Setting_v
  :: cfg` (append). It would desugar to exactly the plain list, i.e. it is pure
  sugar with a cost (extra names, a nominal `Config` type that must stay in sync
  with the setting vocabulary). Under YAGNI it is dropped.

**Verdict: builders dropped. The `config` binding is a `List (Setting shape)`
literal — plain typed data, function-in-record unrepresentable by construction.**
This also keeps a single vocabulary (`Host.bind`, `Web.csrf`, `Email.smtp`, …)
that is already the one the front door uses.

---

## Migration table (each current surface → its slot)

| Current surface | Kind | Unified slot | Additive / breaking |
|---|---|---|---|
| `Web.appWith [ Host.bind … ]` inline | reserved | becomes the value of the top-level `config` binding (or stays inline) | additive (inline still allowed) |
| `Host.bind` / `Log.level` / `Web.csrf` / `Web.sessionTtl` / `Web.authMaxLifetime` / `Web.authSlideWindow` / `Web.withRevocation` / `Db.url` | reserved | unchanged — already in the vocabulary | additive |
| `Email.SmtpConfig` record | ad-hoc record | `Email.smtp { host, port, user } (App.fromEnv "…")` → `Setting` | breaking (record removed; deprecation window) |
| `Email.SesConfig` record | ad-hoc record | `Email.ses { region } (App.fromEnv …) (App.fromEnv …)` → `Setting` | breaking |
| `Email.EmailProvider` (`Resend`/`SendGrid` `String` keys) | ad-hoc ADT | `Email.resend (App.fromEnv …)` / `Email.sendgrid (App.fromEnv …)` → `Setting` | breaking |
| `Analytics.Config` (sink + consent) | ad-hoc record | `Analytics.sink …` / `Analytics.consent …` → `Setting`; identity stays call-local | breaking for the app-lifetime parts |
| `IPE_HTTP_BIND`, `IPE_LOG_LEVEL`, `IPE_CSRF`, `IPE_WEB_TTL`, `IPE_AUTH_*`, `DATABASE_URL` | env | already the env tier of settings that have a `Setting` — no change to the var, now with a typed in-code sibling | additive |
| Tokens: `IPE_ADMIN_TOKEN`, `IPE_METRICS_TOKEN`, `IPE_INGEST_TOKEN`, `IPE_CONSOLE_TOKEN` | env (secret) | new `Secret`-typed settings sourced via `App.fromEnvRequired` | additive (env still read as fallback) |
| Telemetry/console: `IPE_CONSOLE_URL`/`_EMBED`/`_AUTH`/`_DB_PATH`, `IPE_DEV_BANNER`, `IPE_TRACE`, `ENV`/`IPE_ENV` | env | new `Telemetry.*` / `Console.*` cross-cutting settings (`Setting any`) | additive |
| WebSocket: `IPE_WS_*` | env | new `WebSocket.*` settings, or left env-only if genuinely operator-only | additive |
| Ingest limits: `IPE_*_MAX_BYTES`, `IPE_CSV_*`, `IPE_RECURSION_LIMIT`, `IPE_FILE_READ_MAX` | env | left env-only (operator hard limits, not app wiring) — documented as the env tier, no in-code setting needed | additive (no change) |
| `Ipe.Config` file decoders | separate concern | out of scope — unchanged | n/a |
| `Ipe.Env.public` | separate concern | out of scope — unchanged | n/a |

**Overall: additive for the reserved settings and the env surface; breaking only
for the per-module ad-hoc records (Email, Analytics), which is the point — their
plaintext-secret fields must go.** The breaking part ships with a deprecation
window: the old `Email.SmtpConfig`/`Analytics.Config` record builders stay for
one release, emit a deprecation diagnostic pointing at the `Setting`-producing
replacement, then are removed. Not-yet-migrated env vars keep working as the
fallback tier throughout — nothing an operator relies on breaks silently.

---

## Approaches considered

### Approach 1 — Named `config` binding as a `List (Setting shape)` (recommended)

One top-level `config` binding, a plain `List (Setting shape)` literal, passed to
the app entry (`Web.appWith config { … }`). Per-module config becomes
`Setting`-producing builders on each owning module. Secrets are `Secret`-typed
settings sourced via `App.fromEnv`/`App.fromEnvRequired`.

- **Pros:** one vocabulary (extends the already-shipped front door, zero new
  representation); function-in-record unrepresentable (a `Setting` holds no
  function); one precedence inherited from `config-design.md` with no second
  ladder; the existing `Secret` type does all the secret-hiding work; migration
  is mostly additive; a future `ipe config` inspector can read the named binding
  syntactically without executing it.
- **Cons:** breaking for Email/Analytics records (mitigated by a deprecation
  window); a `config` binding is a convention plus one recognition site, so the
  toolchain must learn the name (small, mirrors `main`/`package`).

### Approach 2 — A nominal `Config` record + `withX` builder pipeline (issue's sketch)

`Config.default |> Config.withDatabase … |> Config.withCsrf …`, a nominal
`Config` record type.

- **Pros:** the fluent pipeline reads nicely; discoverable via `Config.` autocomplete.
- **Cons:** a second vocabulary (`withDatabase` vs `Db.url`) splitting SSOT; the
  nominal `Config` type must stay in lockstep with the setting set; it desugars
  to exactly Approach 1's list, so it is pure sugar with a maintenance cost; a
  record type is one grep away from someone adding a function-typed field, which
  is the L0107 failure mode the `List (Setting …)` shape structurally forbids.
  **Rejected under YAGNI** (see *Builder verdict*).

### Approach 3 — Leave inline `appWith` settings; add only the per-module + secret migration

Do not introduce a named `config` binding at all; keep settings inline in the
`appWith` call, and only (a) migrate Email/Analytics records to `Setting`s and
(b) add `Secret`-typed token settings.

- **Pros:** smallest change; no new recognition site; entirely within the shipped
  mechanism.
- **Cons:** does not deliver the issue's headline ("ONE typed top-level `config`
  binding"); cross-cutting wiring stays buried in the app entry, and a large
  settings list inline with `{ init, update, view, … }` is hard to read; no
  single place for a `ipe config` inspector to read. Partial.

### Recommendation

**Approach 1.** It delivers the issue's single-`config`-binding goal, extends
(does not fork) the shipped front door, keeps one vocabulary and one precedence,
makes function-in-record unrepresentable, and turns the plaintext-secret records
into `Secret`-typed settings. Approach 2's builders are dropped (YAGNI). Approach
3 is the fallback if the `config` recognition site proves too costly, but it
leaves the headline feature undelivered.

---

## Implementation checklist (for a later lane)

1. **Reserve the `config` recognition site.** Teach canon to recognise a
   top-level binding named `config` typed `List (Setting shape)` and thread it as
   the settings argument when the app entry is `Web.appWith config { … }`.
   Keep inline `appWith [ … ] { … }` working (additive). Add a lint for a
   `config` binding never passed to an app entry (discarded-config, mirroring the
   discarded-task lint).
2. **Extend the setting vocabulary — cross-cutting `Setting any`:** add
   `Telemetry.*` / `Console.*` settings for the console/telemetry env vars
   (section C), and `Secret`-typed token settings
   (`Console.adminToken`/`ingestToken`/`metricsToken`) sourced via
   `App.fromEnvRequired`. Wire each into the existing `install_web` fold and a
   `resolve_*` reader following the one precedence. Guardian review (tokens, bind).
3. **Add `App.fromEnvRequired : String -> Secret`** (required variant): same seal
   as `App.fromEnv`, but the resolver treats an empty result as a typed
   `ConfigError` at startup (fail-closed), not an empty secret. Add a
   `ConfigError` diagnostic variant naming the missing setting + its env source.
4. **Migrate Email:** replace `SmtpConfig`/`SesConfig`/`EmailProvider`-with-
   `String`-keys with `Email.smtp`/`ses`/`resend`/`sendgrid` `Setting`-producing
   builders taking `Secret` credentials. Keep the old record builders for one
   release with a deprecation diagnostic. Update `src/runtime/rust/src/email.rs`
   to read the resolved settings instead of the threaded record.
5. **Migrate Analytics:** add `Analytics.sink`/`Analytics.consent` `Setting`
   builders; keep call-local identity (`userId`/`traits`) as request state. Old
   `Config`/`defaultConfig` deprecated one release.
6. **Fail-closed audit:** for every security-relevant setting (secrets, bind
   mode, tokens, CSRF), confirm the resolver errors (or holds the strict
   fallback) on a missing/malformed value — never a silent permissive default.
   Unit-test each `resolve_*`/`*_from` split (as `app_config.rs` already does).
7. **Docs:** one page listing every setting, its env-var sibling, its default,
   and the one precedence — the typed manifest the issue asks for. Point
   `config-design.md` at this doc as the concern-2 ergonomics layer.
8. **Golden + E2E:** extend `golden_app_settings_front_door.rs` with a named
   `config`-binding fixture (accepted + `cargo build`s under `IPE_E2E=1`), a
   hard-coded-`String`-credential-in-Email rejection, and a required-secret
   missing-env fail-closed proof.
9. **Full gate** (`cargo fmt`/`clippy --all-targets --workspace`/`nextest -p ipe`
   + touched crates, `--profile ci` for emit, `IPE_E2E=1` seal) + a
   security-soundness-guardian review before merge (this is a config +
   secret-handling surface — a language security boundary).

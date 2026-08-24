# Configuration in Ipê — end to end

Status: design proposal. Every fenced Ipê block illustrates the **proposed
surface**, not shipped API — none of it is runnable today.

## Decision: configuration is written in Ipê, not a stringly file format

Framed the simple way — *which option makes Ipê most adherent to `PRINCIPLES.md`?*
— the answer is unambiguous. Under the precedence `Security > Correctness >
Soundness > Efficiency > Completeness > Readability`, config-as-Ipê wins on the
top four axes and loses only on tooling-convenience, which ranks last:

- **Security (1).** Secrets become *unrepresentable* as source literals (only a
  `fromEnv` reference yields a secret-typed value); cross-shape misconfiguration
  becomes a type error, not a runtime surprise; a total, effect-free config has
  no injection or arbitrary-eval surface.
- **Correctness (2) / SSOT.** One type system validates config. No second parser,
  no schema that can drift from the code. Config is a *validated typed value*
  (parse-don't-validate), not a bag of strings a typo silently defaults.
- **Soundness (3).** The type checker *is* the validator — there is no untyped
  intermediate that can disagree with the types.
- **Readability (6).** One language. A developer who knows Ipê already knows how
  to read and write config; no context-switch to TOML semantics.

It would be quietly hypocritical for a language whose whole pitch is "the type
checker, not discipline, makes it safe; one value is the SSOT" to configure
itself in untyped TOML. The pragmatic costs (machine-rewriting config, external
tools that can only parse TOML) are real but rank below Security/Correctness/
Soundness, and are *addressed* below rather than decisive.

## Three distinct concerns — keep them separate

The current scattered state (ipe.toml sections + `IPE_*` env + per-module records)
is bad precisely because it **conflates three concerns that have different
readers, lifetimes, and trust models**. The design's first job is to separate
them:

| # | Concern | Reader | Lifetime | Home |
|---|---------|--------|----------|------|
| 1 | **Manifest** — name, version, dependencies, DB driver, shape/target | the *toolchain*, before compiling | build-time | `package.ipe` (new) |
| 2 | **Framework runtime config** — log, telemetry, database, host-bind, sessions, CSRF, jobs | the *runtime*, at startup | runtime (compiled in) + env overlay | shape-typed settings on the shape `app` |
| 3 | **User app config files** — a user's *own* app reading *its* config from an external toml/yaml/json | the *user's app*, at runtime | runtime | existing `Ipe.Config` `Decoder` — **keep as-is** |

Concern 3 already exists and is already principled (`Ipe.Config` is a typed
`Decoder` over an external file — parse-don't-validate at the app's own I/O
boundary). It is **out of scope** here except to note: the framework-config
surface (concern 2) must NOT collide with the `Ipe.Config` name.

---

## Concern 2 — framework runtime config (the config front door; the first slice)

The load-bearing runtime settings — how the app logs, its database, its session
and CSRF policy, its telemetry windows, its host binding, its job concurrency —
are today split across ipe.toml, env, and per-module records with no single
front door and no one precedence. The design gives them a **typed, shape-aware
front door attached to the shape's `app`**.

### Shape-typed settings (make invalid states unrepresentable)

Settings carry a phantom shape tag so a setting only type-checks for the shapes
it applies to. A `Terminal` app cannot set `csrf` (there is no server); a `Web`
app can. This is the security/correctness win over one flat `Config` record.

```elm
type Setting shape          -- opaque, phantom in `shape`

-- cross-cutting: valid for every shape (defined in the subsystem modules — SSOT)
Log.level        : LogLevel -> Setting any
Telemetry.window : Duration -> Setting any
Db.url           : Secret   -> Setting any        -- Secret, so only fromEnv reaches it
Jobs.concurrency : Int      -> Setting any
Host.bind        : HostMode -> Setting any        -- Loopback | AllInterfaces | env-driven

-- shape-specific: only valid for the shape whose module exposes them
Web.sessionTtl      : Duration -> Setting Web
Web.authMaxLifetime : Duration -> Setting Web   -- hard cap on a session token's age; default 8 h
Web.csrf            : CsrfMode -> Setting Web
WebView.window   : WindowOpts -> Setting WebView
```

The tag arguments (`HostMode` / `LogLevel` / `CsrfMode`) are **closed ADTs**, not
bare `Int`s — an out-of-range tag is a compile-time type error, not a value the
runtime must fall closed on. Their only values are these constructors:

```elm
Host.loopback / Host.allInterfaces / Host.envDriven   : HostMode
Level.debug / Level.info / Level.warn / Level.error    : LogLevel   -- `Level.*`, distinct from the `Log.*` logging kernels
Web.strict / Web.inheritCsrf                           : CsrfMode   -- no disabling variant
```

`CsrfMode` deliberately has **no disabling variant**: a setting cannot express
turning CSRF off, mirroring the runtime's stricter-only monotonicity — that
property is unrepresentable at the type level. Each constructor projects to the
raw `Int` tag the runtime resolver consumes; the projection is total.

`Setting any` unifies with any shape; `Setting Web` unifies only with `Web`. A
`Terminal` app that lists `Web.csrf …` is a **compile-time type error** — invalid
config is unrepresentable, not validated-and-rejected at runtime.

### Attaching config to the app (additive)

The shape `app` constructors are arity-1 record builders (`Web.app { init, update,
view }`). Config attaches via an **additive** field so existing apps are
unchanged:

```elm
main =
    Web.appWith
        [ Log.level Level.warn
        , Db.url (App.fromEnv "DATABASE_URL")   -- secret: env only
        , Host.bind Host.loopback
        , Web.sessionTtl 3600
        , Web.csrf Web.strict
        ]
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [], notFound = CounterPage
        }
```

`settings : List (Setting shape)` where `shape` is fixed by the constructor
(`Web.app` fixes it to `Web`), so the list is homogeneously shape-checked. Absent
/empty `settings` = built-in defaults (existing behaviour). The phantom tag makes
the field additive *and* safe.

### One precedence, applied by the runtime

At startup the runtime resolves each setting with a single documented order:

> **operator env  >  `settings` in code  >  `package.ipe` default  >  built-in fallback**

Env always wins (operators override deploys without a rebuild); a `fromEnv`
secret is *only* resolvable here (never inlined). The precedence is one rule for
every setting, replacing today's per-source ad-hoc order.

### Secrets are unrepresentable as literals

`Db.url`, JWT secret, etc. take a `Secret`, and the recommended constructor of a
`Secret` for config is `App.fromEnv "VAR"` (reusing the existing reserved
`Secret`/`Password` unencodable category). There is no `Db.url "postgres://user:pw@…"`
overload — a hard-coded `String` credential does not type-check. (Security
priority 1, made structural.)

`Secret.fromString : String -> Secret` also exists and is the **sanctioned
escape hatch**: it seals an already-in-hand plaintext (a value the program
derived at runtime, or a test fixture) into a `Secret`. It is deliberately kept
rather than removed — a program legitimately needs to seal a runtime-obtained
string — and made greppable-by-design so a reviewer can audit every
literal-credential seal by searching the one `Secret.fromString` symbol. For
config credentials, `App.fromEnv` remains the recommended source (it keeps the
credential out of source entirely); `Secret.fromString "literal"` type-checks but
inlines the plaintext, so it is the wrong tool for a config secret.

### Module placement (answering "Ipe.Tea.Web.Config, or elsewhere?")

Not a separate `Ipe.Tea.Web.Config` module (and there is no `Ipe.Tea` namespace).
Instead, by SSOT:

- **Cross-cutting settings live in the subsystem module that owns them** — `Log`,
  `Telemetry`, `Db`, `Jobs`, `Host`. Each is defined once, `Setting any`.
- **Shape-specific settings live in the shape module** — `Web` exposes
  `Web.sessionTtl`/`Web.csrf`, `WebView` exposes `WebView.window`, etc., typed
  `Setting Web` / `Setting WebView`.
- **The `Setting` type + `fromEnv` live in a small shared module** — proposed
  `Ipe.App` (NOT `Ipe.Config`, which is the file-decoder). `Ipe.App` exposes
  `Setting`, `fromEnv`, and the shared enums (`LogLevel`, `HostMode`, …).

This keeps every setting with its owner (one definition site) while the shape tag
enforces where each may be used.

---

## Concern 1 — `package.ipe` (the manifest; a later, breaking slice)

Replaces `ipe.toml`. Config-as-Ipê for the manifest too, so the whole project is
described in one language. The subtlety is the **bootstrap**: the toolchain must
learn the *dependencies* before it can compile anything, so it cannot *evaluate*
Ipê (which needs the deps) to read them.

Resolution: the manifest is **read syntactically, not evaluated** — the toolchain
extracts a literal `package` binding from the AST, exactly the discipline the
compiler already uses to read a `Codec.auto` witness or a reserved builder
literal. `package.ipe` therefore:

- declares a single top-level `package` binding built from a blessed `Ipe.Package`
  vocabulary (no third-party imports — it must be readable before deps exist);
- holds only **statically-literal** values for the bootstrap-critical fields
  (name, version, dependencies, `rust.dependencies`, DB driver, shape/target);

```elm
-- package.ipe (proposed)
package =
    Package.named "my-app"
        |> Package.version "0.3.0"
        |> Package.dependencies
            [ Package.dep "ipe-http" "1.2"
            , Package.dep "ipe-postgres" "0.4"
            ]
        |> Package.rustDependencies [ Package.rustDep "uuid" "1" ]
        |> Package.database Package.postgres
```

- **Security:** read-not-run means an untrusted cloned `package.ipe` executes
  nothing at build time (no eval, no effects, guaranteed total). This is the
  strongest reason to read the manifest syntactically rather than evaluate it.
- **Tooling cost (acknowledged):** `ipe package add foo` must AST-rewrite the
  `dependencies` list rather than edit TOML; external indexers cannot parse Ipê.
  Ipê owns its parser, so the rewrite is tractable; the external-parser loss is
  the accepted price of the SSOT/typed win. This is why the manifest is a
  **separate, breaking slice**, gated on explicit go — it is not bundled into the
  additive concern-2 slice.

---

## Security invariants (enforced, not documented)

- No hard-coded secret type-checks (secret = `fromEnv` only).
- No cross-shape misconfiguration type-checks (phantom shape tag).
- The manifest is read syntactically → no build-time evaluation of untrusted code
  (total, effect-free by construction).
- One precedence, env-wins → operators override without a rebuild; no ambiguous
  layering.

## Migration

- Concern 2 (this slice) is **additive**: apps with no `settings` are unchanged;
  ipe.toml runtime keys are read as the `package.ipe`-default tier under the new
  precedence until removed.
- Concern 1 (`package.ipe`) ships with `ipe migrate config` that rewrites an
  `ipe.toml` into a `package.ipe` (mechanical, name-for-name), mirroring the
  existing store-migration tooling posture.

## Implementation slices

1. **Framework runtime config — additive, ships first.** `Ipe.App` (`Setting`,
   `fromEnv`, shared enums); cross-cutting settings on `Log`/`Telemetry`/`Db`/
   `Jobs`/`Host`; shape-specific on `Web`/`WebView`/`Terminal`; the additive
   `settings` field on each shape `app`; runtime resolution with the one
   precedence; `Db.url`/secrets as `fromEnv`-only. Delivers the config front door
   and lets the dev host bind loopback by default. Security-guardian review
   required (secrets, csrf, bind).
2. **`package.ipe` manifest — breaking, separate go.** Syntactic manifest reader,
   `Ipe.Package` vocabulary, `ipe migrate config`, ipe.toml removal. Own design
   pass + guardian.
3. **Retire the scattered runtime keys from ipe.toml** once slice 1 covers them.

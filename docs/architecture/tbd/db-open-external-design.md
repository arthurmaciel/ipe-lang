# `Db.open` — connecting to an arbitrary external database

Status: accepted; slices landing. The typed `Dsn`, the reserved `Connection`
handle, `Ipe.Db.open`, and the read path over `Connection ReadOnly` have
shipped; the `unsafeOpen` raw-string hatch is dropped (§7, resolved). Fenced
Ipê blocks illustrate the surface and fenced Rust blocks an intended seam;
neither is a verbatim transcript of the shipped code. Tracker references belong
in the delivering pull request, not in this timeless doc.

This design decides the placement, capability, typed surface, lifecycle, and
failure semantics of a stdlib path that connects to a database the *application
was not built against* — a user-configured external source (a monitoring app
querying a customer's Postgres, an ETL job reading a warehouse). It builds on the
Kernel Row descriptor (`kernel-row-design.md`), the `Ipe.<M>.Unsafe` escape +
capability convention (`unsafe-escape-convention-design.md`), the codec/store
layer (`codec-and-store-design.md`), and the compiled-source placement policy
(ADR 0029 / ADR 0057, `stdlib-placement-policy.md`). It links those rather than
restating them.

## The gap, precisely

`Db.connect : () -> Task Error Db` opens exactly one thing: the application's own
configured connection, whose backend (SQLite or Postgres — the two sqlx drivers
the runtime links) is fixed **at build time** from `ipe.toml` and monomorphised
into the runtime `Db` handle — `Db` is an alias (`pub type Db = DbPool`) for the
single concrete pool type the build selected (`SqlitePool` by default, `PgPool`
on a Postgres build). There is one URL (`ipe_db_url`), one pool, one dialect.

A `Db.open` kernel already exists (`Db.open : String -> String -> Task Error Db`,
`Database` capability), but it is a **near-alias of `connect`, not an external
connector**: it takes a driver string and a path string, treats the driver as
*informational*, and connects a pool of the build-fixed backend type using the
path as a URL. On a SQLite-built app, `Db.open "postgres" dsn` cannot produce a
Postgres connection — the concrete pool type is not Postgres, so the driver name
is a comment. That is the exact capability gap: an external Postgres a monitoring
app must query has no typed path; the port that surfaced this routed such SELECTs
through a read-only gate against the *app* connection and treated the external
DSN as a label. The current `Db.open` signature also violates
parse-don't-validate twice over — two bare `String`s, neither parsed into a
typed value at the boundary.

So this is not a greenfield add: it is **replacing an under-designed alias** with
a real, typed, capability-disclosed external connector.

## 1. Placement — a native `Ipe.Db` kernel row

**Verdict: a native Kernel Row on the safe `Ipe.Db` surface. The issue's
suggested path is validated, with one refinement: the *unsafety* is not
intrinsic to "connecting elsewhere" — it is intrinsic to one input shape (an
unparsed DSN string) and one lost guarantee (the configured pool's read-only
posture). The typed, parsed form is the safe `Ipe.Db` member; a raw-string form
*would* belong behind `Ipe.Db.Unsafe`, but that analysis resolves in its favour
being dropped (§2, §7), leaving the typed member the whole connect surface.**

Why native kernel, not compiled-source `.ipe` and not a package:

- It cannot be compiled-source `.ipe` (ADR 0029). Opening a connection is an
  effectful primitive over the sqlx pool — the same category as `db_connect`,
  which is a native kernel. Compiled-source is for *pure combinators over
  existing kernels*; there is no lower kernel for "construct a new pool of a
  driver-selected dialect" to compose over. This is new trusted runtime surface,
  so it is a kernel by the placement policy's security-defence rule.
- It cannot be a third-party package. A package cannot mint the reserved
  connection handle nor open a network/file resource without *some* stdlib
  kernel underneath; the primitive has to exist in the trusted core first. A
  package could later wrap it ergonomically, but the capability must originate in
  the stdlib.
- It is a Kernel Row (`kernel-row-design.md`): one `KernelDef` carrying its
  qualifier/name/arity, its `Database` (and see §2) capability, its emit symbol,
  its runtime residency, and its scheme key. Authoring it as a row is what makes
  the invariant tests gate it — the emit-symbol-defined test would have caught
  that today's `Db.open` scheme is wired but its intended external behaviour is
  unbacked. The existing `Db.open` row is *reshaped*, not added: same enum
  variant, new signature and semantics.

Why a raw-string form *would be* `Ipe.Db.Unsafe`, had one shipped: by the escape
convention's secure-before-you-mark gate, a hatch that a validator could remove
must be fixed, not labelled. Applying that gate here weighs **two candidate
members**:

- A **DSN string** would be an irreducible raw sink in the same sense as
  `unsafeFragment`: an arbitrary connection string can carry credentials, a
  `host` an SSRF target, driver-specific options (`sslmode=disable`, a local
  socket path, `options=-c...`) that no total parser can prove safe. Minting a
  live external connection from an unchecked `String` is provenance-by-assertion
  — the escape shape an `Ipe.Db.Unsafe.unsafeOpen` disclosing `unsafe` *would*
  take.
- A **typed, parsed `Dsn`** built through validating constructors (§3) *is*
  secure by construction — the parse ran, host/port/database are typed fields,
  no free-form option string is interpolated. That form is the safe
  `Ipe.Db.open` and discloses only `network` + `database`, never `unsafe`.

The gate resolves in favour of the typed member alone: the `Dsn` parse *is* the
sanitizer, and no residual opaque-string need has been demonstrated, so the raw
member is dropped (§7). The parallel to the SQL boundary — `Sql.column`
(validated) vs `Ipe.Db.Unsafe.unsafeFragment` (asserted), both minting the
reserved `SqlFragment` — is the shape a future raw member would take if a real
opaque-string need appears, not a shipped second connector.

## 2. Capability — `network`, unioned with `database`

**Verdict: the axis is `network` (new for this member's disclosure), unioned with
the existing `database`. No new `externalDb` axis. The shipped connect surface
is the typed-`Dsn` form and discloses `network` + `database`, never `unsafe`; a
raw-string form would additionally disclose `unsafe`, but none ships (§7).**

Reasoning, against the closed coarse vocabulary the capability model mandates:

- **`database` alone is insufficient and misleading.** Today every `Db` kernel
  tags `Database`, which a reader reads as "touches *the app's* configured DB".
  An external connection reaches an *arbitrary network host*, which is the
  `network` resource axis the OS jail can actually isolate (host/port egress) —
  the same axis `Http` requests disclose. Opening `postgres://198.51.100.7:5432`
  is a network act. So the safe `Ipe.Db.open` unions **`network` + `database`**:
  `database` says "speaks SQL", `network` says "to a host of the program's
  choosing". A local-file SQLite `Dsn` is the one case that need not add
  `network` (no host) — see the open question in §6.
- **No `externalDb` axis.** A per-purpose capability that grows every time a new
  external-resource shape appears is an *open* vocabulary a manifest gate cannot
  reason about exhaustively — the same argument the escape convention used to
  reject `unsafe-html`/`unsafe-sql`. `network` already names the enforceable
  resource; "external DB" is recoverable, for free, as `database ∧ network` plus
  the disclosed import set. No vocabulary growth.
- **A raw form would disclose `unsafe`, import-derived.** Were one added, an
  `Ipe.Db.Unsafe.unsafeOpen` in an `.Unsafe` submodule would infer the `unsafe`
  capability by the convention's import-derived rule — alias-proof, fail-loud,
  disclosed by `ipe capabilities` and gated by the package manifest, so a
  monitoring app reaching for a raw DSN would be *visibly* an unsafe-escape user
  before it runs. The shipped typed `Ipe.Db.open` imports no `.Unsafe` module and
  so never discloses `unsafe` — the incentive gradient the convention wants,
  reached here by making the typed member the *only* member.

**Can a sanitizer make a raw form safe-in-place?** Fully, for every demonstrated
case: parsing driver + host + port + database + typed options into a closed
`Dsn` (§3) *is* the sanitizer, and it *is* the safe member. What a sanitizer
could not neutralise is a fully opaque, free-form connection string with
arbitrary driver options — but no such need has surfaced, and the connect path
is `Driver`-closed regardless, so that residual hatch is dropped rather than
shipped (§7). The design is therefore: parse into `Dsn`, and there is no raw
member to escape to.

**Preserving the injection barrier across an external connection.** The
external-DB path changes *which pool* runs a query, never *how* a query is built.
Every query issued against a `Connection` from `open` routes through the identical
audited surface: identifiers through `valid_sql_ident` (the parameterised-SQL
barrier, ASCII-restricted, dotted-ref aware) and values as positional binds,
never string-concatenation. `Db.open` supplies a connection; it does not open a
second, weaker query path. The `Cond`→`SqlFragment` lowering and `Store`'s
generated statements (`codec-and-store-design.md`) work unchanged against an
external `Connection` because they emit only bound parameters and validated
identifiers regardless of the pool behind them. The raw-SQL door against an
external connection remains exactly `Ipe.Db.Unsafe.unsafeExecRaw` /
`unsafeQuery` — no new injection surface is introduced by connecting elsewhere.

## 3. Typed surface — parse the driver and the DSN once

**Verdict: a closed `Driver` ADT, a parsed opaque `Dsn`, and a reserved
`Connection` handle distinguished from the app connection by a phantom access
mode.** `Driver`, `Dsn`, and `Connection` are proposed-new types — none is a
reserved builtin today.

### Signatures

```elm
-- Safe path: the Dsn was parsed once, at its constructor, into a typed value.
Ipe.Db.open        : Dsn -> Task Error Connection

Ipe.Db.close       : Connection -> Task Error ()
```

`open` takes a `Dsn`, not `Driver -> String`: the driver is *inside* the parsed
`Dsn` (a Postgres DSN is a Postgres driver — the pair cannot disagree), which
kills a whole class of "driver says mysql, string is a postgres URL" mismatch.
The typed `Dsn` is the sole connect path — there is no raw-string escape (§7,
resolved).

### `Driver` — a closed ADT, not a free string

```elm
type Driver = Postgres | Sqlite      -- exactly the sqlx drivers the runtime links
```

The set is closed to **exactly the sqlx drivers the runtime actually links**:
`sqlx` is compiled with the `sqlite` and `postgres` features (the `db` feature
unions both so a postgres build still keeps a sqlite side table), and the
concrete `DbPool` is monomorphised to one of `SqlitePool` / `PgPool` at build
time. **MySQL is not linked** — there is no `mysql` sqlx feature in the runtime,
so a `MySql` variant would be unrepresentable-in-the-runtime and must not appear
in the safe ADT; a MySQL backend is a deliberate future kernel change (add the
sqlx `mysql` feature + a `MySqlPool` arm) gated behind its own driver work, not a
`Driver` variant the type promises today. Listing a driver the runtime cannot
dial would be advertising an unimplemented capability.

Parse-don't-validate: the current `driver : String` is an unparsed value the
runtime string-compares (`driver == "sqlite"`). A closed ADT makes an unsupported
driver unrepresentable at the type level — `Db.open` cannot be handed
`"postgre"`. The set is closed to what the runtime can actually dial; growing it
is a deliberate kernel change, not a user string.

### `Dsn` — parsed once, an invalid DSN unrepresentable

```elm
-- Opaque. The ONLY way to obtain one is a validating parse. No `Dsn` exists that
-- did not pass the parse — parse-don't-validate at the boundary.
type Dsn

Ipe.Db.Dsn.parse   : String -> Result Error Dsn      -- from a full URL string
Ipe.Db.Dsn.build   :                                 -- from typed parts
    { driver   : Driver
    , host     : String
    , port     : Int
    , database : String
    , user     : String
    , password : Secret            -- typed; never Debug/Display'd into a log
    , tls      : TlsMode           -- Require | Prefer | Disable (explicit, not a string flag)
    } -> Result Error Dsn
```

A raw `String` DSN is **not** threaded downstream: it is parsed once into `Dsn`
and every consumer reads the typed value. `build` from typed parts is the
preferred constructor (no string to mis-escape); `parse` covers the
user-pastes-a-URL case and still yields the same opaque `Dsn`, so a malformed URL
is a typed `Err` at the parse point, never a runtime connect surprise. The
password is a `Secret` (the reserved secret type) so it cannot leak into a log or
error render — the DSN's most sensitive field is unrepresentable as plain text.
`TlsMode` is an explicit closed ADT, not a `sslmode=` substring, so "disable TLS"
is a visible, greppable decision, not a buried option.

Because `Dsn` is parsed and typed, `Ipe.Db.open : Dsn -> …` is the **secure
member** (§1): there is no unchecked string reaching the connector. The escape
convention's rule-1 gate is satisfied — the validator exists, so the validated
form is not an escape hatch.

### `Connection` — reserved, and distinguished from the app connection

An external connection is **not** interchangeable with the app's `Db`. The app
connection carries the app's configured dialect, its migration ledger, its
read/write posture; an external connection is an untrusted foreign source that
should be **read-only by default** (a monitoring/ETL reader must not mutate a
customer's DB unless it explicitly asks to). Encode that in the type rather than
in discipline:

```elm
type Connection mode          -- mode is a phantom access-mode tag

type ReadOnly                 -- external default: SELECT / query only
type ReadWrite                -- explicitly requested, discloses more loudly

Ipe.Db.open          : Dsn -> Task Error (Connection ReadOnly)
Ipe.Db.openReadWrite : Dsn -> Task Error (Connection ReadWrite)   -- opt-in mutation
```

The app connection is `Connection ReadWrite` (or its own distinguished alias);
`open` yields `Connection ReadOnly`. Mutating kernels
(`exec`/`insert`/`update`/`delete`, `Store.insert`/`update`/`delete`) require
`Connection ReadWrite` in their signature, so a read-only external connection
**cannot type-check into a write** — the read-only gate the port hand-rolled
becomes a *type*, make-invalid-states-unrepresentable. This is strictly better
than a runtime read-only flag: the violation is a compile error, not a caught
runtime `Err`. `Connection` is a proposed reserved type (un-shadowable, minted
only by `open`/`connect`), joining `Db` in the reserved registry; the phantom
`mode` is erased at emit (no `dyn`, one concrete pool per position).

### Lifecycle — explicit `close`, Task-scoped, pooled per DSN

- **Not cached like the app connection.** `db_connect` returns a process-wide
  cached pool (`connect_cached`) — correct for the *one* app DB. An external
  `open` must **not** silently share a global cache keyed on URL: two callers
  opening the same host with different credentials, or a credential rotation,
  must not alias. Each `open` yields its own pool handle (a small bounded pool),
  identified by pool identity, not by URL string.
- **Explicit `close`.** `Ipe.Db.close : Connection mode -> Task Error ()` returns
  the pool. A monitoring loop that opens per-scrape must close, or bound its
  connections. Soundness: `close` is idempotent and total — closing twice is
  `Ok ()`, never a panic.
- **Task-scoped, fail-closed.** The pool lives as long as its `Connection` value
  is reachable; a dropped `Connection` drops its pool (the sqlx pool's own
  `Drop`). No connection outlives its owning value, and a body future dropped
  mid-use rolls back per the existing cancellation-safety discipline.

## 4. Interaction with `Ipe.Db.Store`

**Verdict: `Store` can target an external `Connection`, gated by access mode. The
codec is dialect-agnostic by design, so it needs no change; the `Driver` inside
the `Dsn` selects the dialect the existing `Db` layer already maps.**

From `codec-and-store-design.md`: a `Codec a` supplies an abstract `Shape` /
`ColType`, and the *dialect mapping* (TEXT vs VARCHAR, BOOL vs 0/1) lives
entirely in the `Ipe.Db` kernels, keyed on the connection's backend. An external
`Connection` simply carries a different backend; the same codec drives it.
Consequences:

- **Read paths work against any `Connection mode`.** `Store.all`, `Store.get`,
  `Store.query …|> toList`, `selectRaw` take a `Connection ReadOnly` or wider —
  the common monitoring/ETL case (typed reads from a foreign DB via one codec) is
  fully supported, injection-safe, no hand-written SQL.
- **Write paths require `Connection ReadWrite`.** `Store.insert`/`update`/
  `delete`/`upsert`/`create`/`migrate` take `Connection ReadWrite`, so they are
  unavailable on the read-only external default and available only on
  `openReadWrite` — the mass-assignment and destructive-migration guards
  (`codec-and-store-design.md`) apply unchanged, now additionally mode-gated.
- **`Store.migrate` against an external DB is opt-in and additive-only.** The
  additive/idempotent migration is already the safe default; requiring
  `Connection ReadWrite` means a reader never accidentally schema-touches a
  foreign DB.

So `open` is **not** raw-query-only: it is a first-class connection the whole
typed `Store`/`Cond`/`Codec` stack composes over, with mutation gated by the
access-mode type. Raw arbitrary SQL against it remains the disclosed
`Ipe.Db.Unsafe.unsafeQuery`/`unsafeExecRaw` door.

## 5. Migration / failure semantics — fail closed, typed `Err`

- **Unreachable / mis-authed host is a typed `Err`, never a panic.** `open`
  returns `Task Error Connection`; a refused connection, a DNS failure, a bad
  password, an unsupported-driver dial all surface as `Task.fail Error`. The
  error render follows the existing db-error discipline — structural, value-free:
  no DSN, no credentials, no `Secret` field ever appears in the `Error` (the
  runtime already strips driver-message values; the `Secret`-typed password
  cannot be `Display`'d by construction). Soundness: no `unwrap`/`expect` on the
  connect result; the failure is data.
- **Fail-closed by default.** Absent proof the external source is reachable and
  authorised, the connection does not exist — there is no partial/degraded
  `Connection`. A read-only default means the conservative posture (no writes) is
  the *reachable* one; mutation is the opt-in branch. TLS defaults to a secure
  mode (`Require`/`Prefer`), never silent `Disable`.
- **No migration of the external schema is implied.** Connecting to a foreign DB
  never runs the app's migration ledger against it. External schema evolution, if
  any, is the explicit opt-in `Store.migrate` on a `ReadWrite` connection —
  additive-only, human-gated for destructive change.

## 6. Minimal first slice + recommendation

**Verdict: a minimal, SEAL-clean, security-reviewable first slice exists that
closes the capability gap now — the safe typed `Ipe.Db.open` for a `Postgres`
external `Dsn`, deferring MySQL to a future driver kernel. Recommend
implementing the first slice rather than leaving the tracked gap
design-deferred.** The reason the tracker named ("no first-party need yet") is
not a technical blocker — the design is settled, the driver is already linked,
and the slice is small enough for a single security review.

Why closeable-now rather than deferred:

- **The runtime already links the driver.** `sqlx` is compiled with `postgres`
  in the `db` feature; the missing piece is not a dependency but a *second pool
  of a different dialect* alongside the build-fixed `DbPool`. That is a bounded
  runtime change (a `Connection` handle wrapping an independently-built
  `PgPool` / `SqlitePool`), not a new client integration.
- **The type surface is fully specified above** — `Driver`, `Dsn`, `Connection
  mode`, `Secret` password, `TlsMode` — and reuses reserved-type and
  `.Unsafe`-disclosure machinery that already exists. No new language mechanism
  is invented; the slice is an application of settled conventions.
- **The injection barrier is unchanged** (§2): the same `valid_sql_ident` +
  bound-parameter surface runs against the new pool, so the security surface to
  review is narrow and self-contained — connection provenance, credential
  handling, TLS default — not a new query path.

### What the first slice ships

The smallest slice that closes the gap and is worth reviewing as one unit:

1. `Ipe.Db.open : Dsn -> Task Error (Connection ReadOnly)` — the safe member,
   the sole connect path.
2. `Ipe.Db.Dsn.build` / `Ipe.Db.Dsn.parse` producing the opaque `Dsn`, with
   `Secret` password and explicit `TlsMode` (secure default).
3. `Ipe.Db.close : Connection mode -> Task Error ()`.
4. `Driver = Postgres | Sqlite` — the two linked drivers only.
5. Read paths of `Store`/`Cond`/`Codec` against `Connection ReadOnly`
   (mutation and `openReadWrite` can be a fast follow, since the read-only
   default is the safe posture and covers the monitoring/ETL case that surfaced
   the gap).

MySQL, `openReadWrite`, a raw-string connect escape, and any process-wide
external pool cache are all out of the slice; each is its own follow-up (the
raw escape resolved as dropped — §7).

### Sequenced implementation plan

Each step is independently SEAL-gatable; the security review sits between the
type surface landing and any raw-string hatch being exposed.

1. **`Dsn` + `Driver` + `TlsMode` + `Secret` password, parse-only, no connect.**
   Pure typed constructors and the parse; a `Dsn` cannot be built from an
   invalid string and cannot render its password. No I/O, no capability yet —
   reviewable in isolation, and nothing external can be opened until it lands.
2. **`Connection mode` reserved handle + `Ipe.Db.open` safe member + `close`.**
   The kernel row that builds an independent pool from a typed `Dsn`, returns a
   `Connection ReadOnly`, discloses `network + database`, and closes total /
   idempotent. Failure is a typed `Err` that leaks no `Secret`. **Security
   review gate here** — connection provenance, TLS default, credential
   non-leak, fail-closed.
3. **Read-path `Store`/`Cond`/`Codec` over `Connection ReadOnly`.** Wire the
   existing typed read surface to accept an external connection; confirm
   `valid_sql_ident` + bound parameters run unchanged against the new pool.
   This is the step that makes the port's hand-rolled read-only gate a *type*.
4. **Follow-ups (separate tracker items):** `openReadWrite` +
   write-path mode gating; a `MySql` driver (sqlx `mysql` feature + `MySqlPool`
   arm); any caller-visible pool-size knob.

Steps 1–3 alone close the gap: an external Postgres/SQLite source becomes typed,
injection-safe, read-only-by-type, and composable with the codec stack. Step 4
is future capability, not the gap.

## 7. Open questions for the user

1. **SQLite-file `Dsn` and the `network` axis.** A local-file SQLite external
   `Dsn` reaches no network host — should `Ipe.Db.open` of a file-backed `Dsn`
   disclose only `database` (+ `filesystem`?) and not `network`? Cleanest is to
   derive the axis from the parsed `Driver`/host: `Sqlite` file ⇒
   `database + filesystem`; a networked driver ⇒ `database + network`. This keeps
   disclosure honest per-DSN but makes one member's capability
   *value-dependent* — confirm that is acceptable, or force all `open` to
   disclose the superset `network + database + filesystem` for a simpler gate.
2. **Read-only default strictness.** Is `Connection ReadOnly` the right default
   for *all* `open`, forcing `openReadWrite` for any mutation — or should the
   access mode be a field of the `Dsn`/an argument, so one `open` returns the
   requested mode? The phantom-type default is safest (writes are a compile error
   unless asked) but adds a second entry point.
3. **Pool sizing / lifetime policy for external connections.** Per-`open` bounded
   pool with explicit `close` is proposed; confirm no process-wide URL cache for
   external connections (the app connection keeps its cache). A monitoring loop's
   open/close cadence may want a caller-visible pool-size knob on the `Dsn`
   builder.
4. **Should `unsafeOpen` exist at all, or is typed-`Dsn`-only sufficient?**
   *Resolved: dropped.* The connect surface is typed-`Dsn`-only. `unsafeOpen`
   was `Driver`-constrained — a closed ADT of the two linked drivers — so it
   could never reach a new engine the typed path cannot; its only added power
   was a raw, unparsed option string. That is not a demonstrated need: `Dsn.build`
   / `Dsn.parse` round-trip real Postgres/SQLite connection strings, so the raw
   form buys nothing but an extra `unsafe` disclosure and the emit weight of a
   second connector plus `Driver`-tag marshalling. Deferred, not foreclosed: if a
   genuinely opaque driver-option string with no total parser becomes a real need,
   reintroduce it then — most cleanly with `Driver` promoted to a reserved builtin
   so the tag marshalling is a type, not a hand-rolled integer.

## Divergence ledger — prior art, with skepticism

Prior art informs the *vocabulary* only; every behaviour is re-derived against
PRINCIPLES.

| Kept | Rejected / diverged (and why) |
|---|---|
| The idea of a stdlib `open <driver> <dsn>` for an external source | **`driver : String`, `dsn : String`** (two unparsed strings) → closed `Driver` ADT + opaque parsed `Dsn` (parse-don't-validate; a mismatched driver/URL and a malformed DSN both unrepresentable) |
| An external connection reads through the same SQL surface | **Routing external SELECTs through a read-only gate against the *app* connection** (the port's workaround) → a real second `Connection`, read-only by *type* (phantom access mode), not by a hand-rolled runtime gate |
| Connecting elsewhere is a privileged act | **Disclosing it as `database` only** → `network + database` (the enforceable resource axis), with `unsafe` on the raw-string form only |
| A raw connection-string escape may be irreducible | **Making raw string the *only* form** → typed `Dsn` is the secure default; and, since the connect path is `Driver`-closed and no opaque-string need was demonstrated, even the disclosed raw hatch is dropped (§7), not merely demoted |

## Summary

Reshape the existing under-designed `Db.open` alias into a real external
connector authored as a Kernel Row: a **safe `Ipe.Db.open : Dsn -> Task Error
(Connection ReadOnly)`** whose `Dsn` (closed `Driver`, `Secret` password,
explicit `TlsMode`) is parsed once at the boundary and discloses `network +
database` — the sole connect path, with no raw-string escape (a `Driver`-closed
`unsafeOpen` was considered and dropped for want of a demonstrated
opaque-string need, §7). The reserved `Connection mode` handle makes the
external read-only posture a
*type* rather than a runtime gate, so writes to a foreign DB are a compile error
unless explicitly requested. The whole `Codec`/`Store`/`Cond` stack composes over
an external connection unchanged, the `valid_sql_ident` + bound-parameter
injection barrier is preserved because `open` supplies a pool and never a new
query path, and every failure is a typed `Err` that leaks no credential.

**Recommendation: close the gap now, not deferred.** The runtime already links
the Postgres driver, the type surface is settled, and the injection barrier is
untouched, so a minimal read-only-by-type first slice (`Dsn`/`Driver`/`Secret`
→ `Ipe.Db.open` → read-path `Store`, §6) is small enough for one security
review and closes the capability gap. `Driver` is `Postgres | Sqlite` only — the
two drivers the runtime actually links; MySQL and write-mode `openReadWrite` are
sequenced follow-ups (§6), not part of the closing slice. The raw-string
`unsafeOpen` hatch is dropped (§7), not deferred as a follow-up.

# `Ipe.Codec` + `Ipe.Db.Store` — one codec, two consumers

Status: design proposal, no implementation yet. Every fenced Ipê block is
**illustrative of the proposed surface** — none is shipped API. Every fenced Rust
block illustrates the *intended emit or runtime seam*, not verified code.

This document supersedes `codec-auto-derive-design.md`, which designed only the
`Codec.auto` derive in isolation. That earlier draft is absorbed here: its
compile-time-derive decision is kept and sharpened, its `map` signature is
corrected (it is invariant, not covariant — see the Codec type section), and its
DB half is replaced by a concrete `Store` design that *drives* the codec shape
rather than merely consuming it. Read this doc; the earlier one is retained only
as the derive's first sketch.

This design depends on the consolidated type vocabulary (`kernel-row-design.md`),
coordinates with the `Ipe.<M>.Unsafe` escape convention
(`unsafe-escape-convention-design.md`) and the compiled-source placement policy
(`stdlib-placement-policy.md`, ADR 0029), and builds directly on the existing
injection-safe `Ipe.Db` / `Ipe.Db.Sql` runtime surface (`SqlFragment`,
`Sql.column`, `valid_sql_ident`, `Db.findWhere`, parameterised binds).

## The one idea

A type's *wire shape* (JSON) and its *storage shape* (DB columns) are the same
fact, written twice in most codebases and drifting the moment one changes. Ipê
writes it **once**, as a `Codec a`, and makes both consumers read that single
value:

- **JSON** reads the codec's encoder + decoder.
- **DB** reads the codec's *structural shape* for its column list and types, and
  the same encoder/decoder for the row round-trip.

`Ipe.Db.Store` is the persistence layer built on that codec: a `Store a` is one
table whose schema, reads, and writes all derive from one `Codec a`, with **no
hand-written SQL and no second decoder**. The design goal that governs every
decision below: *Store's real needs shape Codec*, so the two fit as one story
rather than two modules bolted together.

The precedence order (Security > Correctness > Soundness > Efficiency >
Completeness > Readability) is the tie-breaker throughout. The load-bearing
consequences: every SQL identifier is validated and every value is a bound
parameter (Security); one codec means encode/decode/persist cannot disagree
(Correctness); a `Secret` field is *unencodable by construction* (Security); an
underivable field is a compile-time rejection, never a partial codec
(make-invalid-states-unrepresentable).

---

## Part 1 — `Ipe.Codec`

### The type: an invariant codec, honest by construction

```elm
type Codec a =
    Codec
        { enc : a -> Value        -- encode: a value -> a JSON node
        , dec : Decoder a         -- decode: a JSON node -> a value (typed errors)
        , shp : Shape             -- structural shape: how DB columns derive
        }
```

`Codec a` is **invariant** in `a`: it holds both a producer (`enc : a -> Value`)
and a consumer (`dec : Decoder a`) of `a`. This is the type that keeps
bidirectionality honest *by the types, not by discipline* — you cannot construct
a `Codec a` that encodes but does not decode, or vice versa. A separate
encoder/decoder pair (the rejected alternative below) can silently drift; a
single invariant record cannot.

The invariance forces the correct shape on `map`. Adapting a `Codec a` to a
`Codec b` needs a **bijection**, both directions:

```elm
-- CORRECT (invariant): both directions supplied, so the round-trip is preserved.
map : (a -> b) -> (b -> a) -> Codec a -> Codec b

-- WRONG (what the superseded draft wrote): a one-way map cannot encode a `b`,
-- because `enc : b -> Value` needs `b -> a` to reach the underlying encoder.
-- map : (a -> b) -> Codec a -> Codec b        -- does not type-check honestly
```

The correction matters: `Codec.map` is the newtype-wrapper combinator (`UserId`
over `String`, a `Bool` stored as 0/1), and it *must* carry the inverse or it
cannot encode. This is `make-invalid-states-unrepresentable` at the API level —
the type of `map` makes the drift the earlier draft permitted impossible to
write.

### The DB-shape vocabulary

```elm
-- The abstract column type a codec maps to. Dialect-neutral: the Db layer maps
-- these to SQLite/Postgres types. `CNull t` is a nullable column of `t`.
type ColType = CText | CInt | CReal | CBool | CBlob | CNull ColType

-- A codec's structural shape:
--   SRecord  — top-level scalar fields become typed columns.
--   SScalar  — a bare scalar codec is one typed column.
--   SBlob    — nested record / list / data-carrying ADT: one JSON TEXT column.
type Shape
    = SRecord (List (String, ColType))
    | SScalar ColType
    | SBlob
```

`ColType` is deliberately *abstract*. The codec never names `TEXT` vs `VARCHAR`,
`BOOLEAN` vs `INTEGER 0/1` — that dialect choice belongs to `Ipe.Db`, which
already binds a `SqlBool` correctly per backend. The codec supplies the *shape*;
the Db layer supplies the *dialect*. One codec therefore works on every backend
(Correctness: no per-dialect codec to keep in sync).

### The surface

```elm
module Ipe.Codec exposing
    ( Codec, ColType(..), Shape(..)
    , shape, toValue, toJson, fromJson, fromJsonSafe
    , string, int, float, bool, decimal, money
    , maybe, list, map
    , object, field, buildObject          -- record builder (pipeline)
    , enum                                 -- nullary enum <-> queryable TEXT
    , taggedUnion, Variant, var0, var1, var2, var3  -- data-carrying ADTs
    , auto, autoCamel, autoWith            -- compile-time record derive
    )
```

`shape`/`toValue`/`toJson`/`fromJson`/`fromJsonSafe` are thin projections/runs.
The primitives, `maybe`/`list`/`map`, and the `object`/`field`/`buildObject`
builder are ordinary pure Ipê over the existing `Ipe.Json.Encode.Value` and
`Ipe.Json.Decode.Decoder` kernels — **zero compiler involvement, no new
kernels**.

#### `fromJsonSafe` — the untrusted-input door

```elm
-- Reject input longer than `maxChars` BEFORE parsing — a size guard for
-- untrusted bodies (request payloads, webhooks). Parser nesting depth is
-- separately bounded by the JSON kernel.
fromJsonSafe : Int -> Codec a -> String -> Result Error a
```

Security note carried into the doc, not just the signature: to avoid
mass-assignment, decode untrusted input into a *dedicated input record* holding
only client-settable fields, never straight into a persistence record. `Store`
enforces this structurally — its write path takes a typed `a`, so a client can
never set a `generated` column (see Part 2).

#### The record builder (pipeline, applicative)

```elm
type alias User =
    { id : String, age : Int, active : Bool, nick : Maybe String }

userCodec : Codec User
userCodec =
    Codec.object User
        |> Codec.field "id"     .id     Codec.string
        |> Codec.field "age"    .age    Codec.int
        |> Codec.field "active" .active Codec.bool
        |> Codec.field "nick"   .nick   (Codec.maybe Codec.string)
        |> Codec.buildObject
```

`object` seeds the builder with the record constructor; each `field` supplies a
JSON key, a getter `.f`, and the field's codec, contributing **simultaneously**
to `enc` (key ↔ getter), `dec` (applicative `map2`-chained decode), and `cols`
(key ↔ `ColType`). Because all three are appended in one call, the encode,
decode, and column mapping for a field *cannot* be written inconsistently —
single-source at the combinator level. This is the invertible-by-construction
core; `auto` is a shorthand that elaborates to exactly this chain.

#### Nullary enums are queryable columns; data ADTs are blobs

```elm
type Rank = Bronze | Silver | Gold

rankCodec : Codec Rank
rankCodec =
    Codec.enum
        [ (Bronze, "bronze"), (Silver, "silver"), (Gold, "gold") ]
```

`enum` maps a nullary enum to a readable TEXT scalar (`SScalar CText`) — so it is
a *queryable* column (`Store.where_ (Store.eq "rank" (SqlString "gold"))`), not
an opaque blob. **Divergence from prior art (totality):** the prior-art `enum`
returns `""` for an unmapped value on encode — a silent partial function. Ipê's
`enum` takes the *whole* constructor set and the compiler's exhaustiveness check
over the pairs list is surfaced as a lint: an `enum` missing a constructor is an
`IPE-…` diagnostic, so `enumName` is total by construction and never emits an
empty string. (The pairs form is kept over a bare `List String` because the
value→name map must be explicit to survive constructor reordering — SSOT for the
wire name.)

Data-carrying ADTs use `taggedUnion` + `varN` (JSON `["Tag", arg0, …]`,
`SBlob`). **Divergence (SSOT):** the prior-art `taggedUnion` writes the encode
side (`toTagged`) and the decode side (`varN` list) *separately* — two sources of
truth for one union, which can drift (a renamed tag, a reordered arg). Ipê's
`taggedUnion` is specified so each `Variant` carries *both* its encoder and its
decoder (a `var1 name ctor project codec` supplies the projection `v -> a` and
the constructor `a -> v`), and the top-level encoder is *derived from the variant
list*, not passed separately — one list, one source of truth. (Detailed variant
signatures are in the Codec surface appendix once the pattern is prototyped; the
invariant is the constraint: no second hand-written case-of on encode.)

### `Codec.auto` — a compile-time derive, not runtime reflection

```elm
userCodec : Codec User
userCodec = Codec.auto blankUser        -- derived at compile time; one line
```

This is the design's sharpest decision and is kept from the superseded draft.
The prior art derives `auto` by **runtime reflection**: it reflects over a
zero-value witness using struct tags plus a runtime constructor registry, both
emitted by the compiler to work around type erasure. **Ipê rejects that model
entirely** (Security + Soundness + Efficiency, and the project's `no dyn Any /
concrete over generic` rule): there is no runtime witness reflection, no
per-field tag in the emitted struct, no runtime constructor registry, no
reflection kernel.

Instead `Codec.auto blank` is a **canon-stage elaboration keyed on the solved
type**. The compiler recognises the reserved `Ipe.Codec.auto` binding at its call
site (the same recognition path `Ffi.kernel` aliases use), reads the witness's
solved, closed record type — field names in field-index order, each field's type
— and **rewrites the `auto` call into the ordinary `object |> field … |>
buildObject` chain the developer could have written by hand**. That derived
expression then flows through the *identical* inference → lower → emit path as a
hand-written codec.

The consequences that make this the right choice:

- **Zero runtime cost.** The derived chain monomorphises to the same native
  field-access + `Json.Encode`/`Decode` calls a hand-written codec emits. An
  anti-drift test asserts `Codec.auto blank` and the equivalent explicit builder
  produce **byte-identical emit**.
- **THE SEAL holds by construction.** The derive emits *no bespoke Rust*. If the
  assembled chain type-checks it lowers exactly as the hand-written builder does
  (so `cargo build` succeeds); if a field is underivable, inference rejects it
  *at ipe time* with a clean diagnostic. There is no ipe-exit-0-then-cargo-fail
  path because there is no emit the hand-written builder does not already
  exercise.
- **The witness only names the type.** `auto`'s argument is elaboration input (a
  type carrier), never read at runtime — its sole job is to make the record type
  inferable at the call site.

Field classification (which leaf codec a field's type selects) binds to the
consolidated `TyShape`/`BuiltinTag` vocabulary the Kernel Row owns — it does *not*
grow a second "is this an `Int` field?" predicate. Per field:

| Field type | Derived leaf | Column |
|---|---|---|
| `String` / `Int` / `Float` / `Bool` | `Codec.{string,int,float,bool}` | `SScalar C{Text,Int,Real,Bool}` |
| `Decimal` / `Money` | lossless string-encoded exact decimal | `SScalar CText` (never `Float`) |
| `Maybe t` | `Codec.maybe <derive t>` | scalar → `CNull t`; else `SBlob` |
| `List t` / nested record | `Codec.list …` / recurse | `SBlob` (JSON-in-TEXT) |
| nullary enum | generated name↔ctor codec (inline `match`, no registry) | `SScalar CText` |
| **`Secret`** | **compile-time REJECTION** | — |
| data-carrying ADT / bare function / self-recursive record | **compile-time REJECTION**, points at `taggedUnion`/explicit codec | — |

`Money`/`Decimal` get first-class arms because a persistence/JSON record routinely
carries a price and a `Float` round-trip would violate Correctness (lossy). The
`Secret` rejection reuses the existing `SecretOrSink` reserved-type category in
`resolve.rs` (SSOT — no parallel "is this a secret?" list); it is a *feature*, not
a limitation, because encoding a secret to JSON or a column is exactly the leak
the Security principle forbids. `autoWith [ (col, codec) ]` is `auto` with
per-field overrides for the rare wrong-derivation (a `Bool` stored as 0/1).

`auto` emits **snake_case** columns/keys (`priceMinor` → `price_minor`, the DB
convention); `autoCamel` keeps camelCase for a camelCase API. The case transform
is a pure compile-time string function.

---

## Part 2 — `Ipe.Db.Store`

`Store` is where the design earns its keep and where it *drives* Codec: the Store
needs a per-field column type (so `Shape`/`ColType` exist on the codec), a
per-field nullability (so `CNull` exists), a queryable scalar for an enum column
(so `enum` is `SScalar CText`, not a blob), and a lossless price column (so
`Money`/`Decimal` are `CText`, not `CReal`). Every one of those Codec decisions
above is a Store requirement.

### The `Store a` handle — a codec plus the DB-only facts

```elm
type Store a =
    Store
        { name  : TableName            -- validated identifier (parse-don't-validate)
        , codec : Codec a              -- THE source of columns, reads, writes
        , spec  : List ColumnSpec      -- per-column DB-only facts (typed, not stringly)
        , pk    : Maybe ColumnName      -- primary key column (validated)
        }
```

A `Store a` is built from a table name and a codec, then refined with the facts
the *type* cannot express — primary key, uniqueness, serial/auto-id, defaults,
indexes. Everything else (the column list, their types, nullability, the row
round-trip) derives from the one codec.

```elm
users : Store User
users =
    Store.fromCodec "users" userCodec
        |> Store.primaryKey "id"
        |> Store.unique "email"
        |> Store.defaultNow "created_at"
```

#### Typed column specs — the key divergence

**Divergence from prior art (make-invalid-states-unrepresentable, Security).**
The prior-art Store encodes column facts as **stringly-typed flags** mangled into
a colspec string — `"!"` for serial, `"u"` for unique, `"dnow"`,
`"dint=" ++ show n`, `"|"`-delimited — parsed back apart at DDL time. That is a
stringly protocol with impossible states representable (`"dint=abc"`, a flag
typo) and a hand-rolled parser on the DDL seam. Ipê replaces it with a **typed
ADT**:

```elm
type ColumnSpec
    = PrimaryKey ColumnName
    | Serial ColumnName                 -- DB assigns; INSERT omits the column
    | Unique ColumnName
    | Index ColumnName
    | DefaultNow ColumnName             -- DB stamps on INSERT
    | TouchOnUpdate ColumnName          -- DB re-stamps on every UPDATE
    | DefaultValue ColumnName SqlValue  -- typed default, bound not interpolated
    | ComputedOnInsert ColumnName (() -> SqlValue)  -- app-side default (e.g. UUID pk)
```

Each builder (`Store.primaryKey`, `Store.serial`, `Store.unique`, …) is a total
function `ColumnName -> Store a -> Store a` appending one typed `ColumnSpec`. The
name is resolved against the codec's column list and *validated as a
`ColumnName`* on the way in (a name absent from the codec is a `Task`-free build
error — `parse-don't-validate` at the store-construction boundary, so no
downstream path re-checks it). No string flag, no DDL-time parse, no
representable garbage.

### CRUD — typed, injection-safe by construction

The whole point: the Store surface issues **no hand-written SQL string
concatenation with a caller identifier in it**. Every generated statement routes
its identifiers through the validated `Sql.column` / `valid_sql_ident` surface
and every value through a positional bind — reusing the *existing*
`Ipe.Db.Sql` `SqlFragment` machinery (`Db.findWhere`, `sql_eq`, `sql_and`, …),
not a new SQL builder.

```elm
Store.create      : Db -> Store a -> Task Error ()
Store.migrate     : Db -> Store a -> Task Error (List String)  -- additive-only, idempotent
Store.insert      : Db -> Store a -> a -> Task Error Int
Store.insertMany  : Db -> Store a -> List a -> Task Error Int
Store.upsert      : Db -> Store a -> a -> Task Error Int        -- INSERT … ON CONFLICT(pk) …
Store.update      : Db -> Store a -> a -> Task Error Int        -- by pk, whole record
Store.get         : Db -> Store a -> SqlValue -> Task Error (Maybe a)  -- by pk
Store.all         : Db -> Store a -> Task Error (List a)
Store.delete      : Db -> Store a -> SqlValue -> Task Error Int        -- by pk
Store.selectRaw   : Db -> Codec row -> SqlFragment -> Task Error (List row)  -- JOIN/aggregate escape
```

Notes that carry the principles:

- **`get`/`delete`/`update` key on the primary key**, whose column is the
  validated `pk : ColumnName` on the store — never a caller-supplied identifier
  string. The value binds as a `SqlValue` param. **Divergence (Security):** the
  prior-art `findBy`/`delete` interpolate a caller `col` string directly into the
  SQL text on a fast path that bypasses its own column guard — safe only because
  callers happen to pass constants. Ipê closes that seam: the by-key operations
  use the store's already-validated `pk`, and the general path (below) routes
  every column through `Sql.column`.
- **`insert`/`update` honour `Serial`/`DefaultNow`/`ComputedOnInsert` specs** by
  *omitting* those columns from the statement (via the existing `SqlField`
  `OmitField` / `insertFields` mechanism), so the DB or the app-side generator
  fills them. A client value can never overwrite a `generated` column because the
  write path takes a typed `a` and the omit list is derived from the store's
  specs, not from the payload — mass-assignment closed structurally.
- **`selectRaw` is the honest escape** for JOINs/aggregates a single-table store
  cannot model: you own the `SqlFragment` (still parameterised/validated), the
  codec owns the row→record mapping. Raw arbitrary SQL text remains the disclosed
  `Ipe.Db.Unsafe.unsafeQuery` hatch — Store makes it rare, never removes the
  audited door.

### The query builder — a `Cond` that compiles to a `SqlFragment`

```elm
type Cond    -- opaque; build with the leaves, combine with and_/or_/not_

Store.where_   : Cond -> Query a -> Query a
Store.eq, neq, gt, gte, lt, lte : ColumnName -> SqlValue -> Cond
Store.like      : ColumnName -> String -> Cond      -- pattern BOUND, wildcards stay data
Store.isNull, notNull : ColumnName -> Cond
Store.inList    : ColumnName -> List SqlValue -> Cond   -- empty ⇒ (1=0), never `IN ()`
Store.and_, or_ : List Cond -> Cond
Store.not_      : Cond -> Cond
Store.orderAsc, orderDesc : ColumnName -> Query a -> Query a
Store.limit, offset : Int -> Query a -> Query a
Store.toList    : Db -> Query a -> Task Error (List a)
Store.toMaybe   : Db -> Query a -> Task Error (Maybe a)
Store.count     : Db -> Query a -> Task Error Int
```

```elm
recentGold : Db -> Task Error (List User)
recentGold conn =
    Store.query users
        |> Store.where_ (Store.eq "rank" (SqlString "gold"))
        |> Store.where_ (Store.gt "age" (SqlInt 18))
        |> Store.orderDesc "created_at"
        |> Store.limit 20
        |> Store.toList conn
```

**Divergence + structural reuse (Security).** The prior-art `Cond` embeds the SQL
operator as a *string* in the ADT (`CondOp col "=" v`) and renders it by
string-building. Ipê's `Cond` uses a **typed operator** and — critically —
**lowers the whole `Cond` to a `SqlFragment` via the existing `Sql.*`
combinators** (`sql_eq (sql_column col) (sql_param v)`, `sql_and`, `sql_or`,
`sql_not`, `sql_in_list`, `sql_like`), then hands that `SqlFragment` to
`Db.findWhere`. Consequences:

- Every column name flows through `Sql.column` → `valid_sql_ident`, so a
  malformed identifier *poisons the fragment* and surfaces as a typed
  `Task.fail`, never as injected SQL. The `ColumnName` type means the leaf
  builders already receive a validated name; the `SqlFragment` layer is
  defence-in-depth.
- Every value is a positional bind (`sql_param`), never interpolated — untrusted
  `LIKE` wildcards stay data.
- The parenthesisation/precedence and the empty-`IN` → `(1=0)` safety are the
  runtime's already-tested `SqlFragment` behaviour, not re-implemented in the
  Store. Store adds *no new injection surface*; it is a typed façade over the
  audited fragment builder.

### Schema derivation + migrations

`Store.create` emits dialect-correct DDL from the codec's `Shape` (columns +
`ColType`) plus the store's `ColumnSpec`s (constraints/defaults). `Store.migrate`
is **additive-only and idempotent**: it creates a missing table and ADDs new
(nullable) columns the record type gained; it never drops, renames, or retypes a
column — those stay explicit (a destructive migration is a human decision, per
fail-closed). This rides the existing `Db.migrate` versioned-migration ledger
(`_ipe_migrations` + checksum guard); Store contributes the *derived* column set,
the ledger contributes safety and replay.

Because the DDL column list is derived from the *same* codec the reads/writes
use, the schema cannot drift from the row shape — the one-codec invariant made
structural at the persistence layer.

---

## Placement — where each piece lives (ADR 0029 / ADR 0057)

Security-defence and performance-critical primitives stay **native kernels**;
pure value logic is **compiled-source `.ipe`**. The split:

| Piece | Residency | Why |
|---|---|---|
| `Ipe.Codec` type, primitives, `maybe`/`list`/`map`, `object`/`field`/`buildObject`, `enum`, `taggedUnion`/`varN`, projections | **compiled-source `.ipe`** | pure combinators over existing `Json.Encode`/`Decode` kernels; rich internal structure; zero new kernels (ADR 0029's "source is strictly better" rule) |
| `Codec.auto`/`autoCamel`/`autoWith` | **canon-stage compiler elaboration** | neither a runtime kernel (no reflection to host) nor plain `.ipe` (a function can't read its argument's static field list); its "body" is the synthesised builder chain — one new recogniser, no new kernel/`naming.rs`/`ir::pretty` arm |
| `Ipe.Db.Store` surface, `ColumnSpec`, `Cond`→`SqlFragment` lowering, CRUD orchestration | **compiled-source `.ipe`** | pure combinators over the `Db`/`Sql` capability; no new injection surface |
| `SqlFragment`, `Sql.column`/`valid_sql_ident`, `Db.findWhere`, bind, dialect DDL, migration ledger | **existing native kernels (reused, unchanged)** | Security-critical identifier validation + parameterised binding + dialect mapping already audited in `db.rs`; Store must not re-implement them |

So the whole feature adds **one** compiler-side artifact (the `auto` recogniser)
and **zero** new runtime kernels on the happy path — everything else is
compiled-source Ipê composing an already-audited safe surface. This is the
minimum-new-trusted-code posture the precedence order wants.

---

## Alternatives considered

**A1 — Separate `Encoder a` + `Decoder a` instead of an invariant `Codec a`.**
Rejected. Two values are two sources of truth: nothing stops an encoder and a
decoder for the same type from disagreeing (a renamed field on one side only),
and the DB shape would need a *third* artifact. The invariant `Codec a` makes
encode/decode/shape one value; drift becomes unrepresentable. Cost: `map` needs a
bijection (both directions) — accepted, because that is exactly the honesty we
want.

**A2 — Runtime-reflection `auto` (the prior-art model): compiler-emitted struct
tags + a runtime constructor registry read by a reflection kernel.** Rejected on
three principles at once. Security/Soundness: a reflection kernel plus a runtime
type registry is new trusted, type-erasing runtime surface (the project forbids
`dyn Any`/downcast). Efficiency: reflection at run time versus a monomorphised
derived chain. Correctness: tags are a second encoding of the field list that can
drift from the type. The compile-time derive costs one canon recogniser and emits
the *same* code as a hand-written codec — strictly better on every axis that
ranks above Completeness.

**A3 — Store as free functions keyed by a `Codec a` (no `Store a` handle):
`Store.insert conn "users" codec record`.** Rejected for the DB-only facts. The
primary key, uniqueness, serial-omit set, and defaults are *not* in the codec (a
type can't express "id is auto-assigned") and must live somewhere; threading them
as extra arguments to every call re-introduces the drift the design exists to
kill (an `insert` that forgets the omit set writes a client value into a serial
column). The `Store a` handle is the single place those facts are declared once
and every operation reads them — SSOT for the table's DB contract. A codec-keyed
free function is kept only for the `selectRaw` escape, where there is no table
identity to carry.

**A4 — A macro/derive-attribute syntax (`@derive Codec` on the type
declaration).** Rejected as premature and less flexible than `auto` at the use
site. Ipê has no attribute-derive surface today, and `Codec.auto blank` already
gives the one-line derive without new syntax, while `autoWith` and the explicit
builder cover the escapes an attribute couldn't. Revisit only if a
declaration-site derive is wanted language-wide (out of scope here).

---

## Security analysis

- **SQL injection.** No Store operation concatenates a caller-supplied identifier
  or value into SQL text. Identifiers route through `Sql.column` /
  `valid_sql_ident` (ASCII alnum + `_`/`.`, non-empty; a violation *poisons* the
  fragment into a typed error); the by-key operations use the store's
  pre-validated `pk`; values are always positional binds via `sql_param`. The
  `Cond`→`SqlFragment` lowering means the whole WHERE clause inherits the
  runtime's audited parenthesisation, empty-`IN` guard, and poison propagation.
  The only raw-SQL door is the disclosed `Ipe.Db.Unsafe` hatch, unchanged.
- **Mass-assignment.** `fromJsonSafe` + decoding into a dedicated input record is
  the documented pattern; `Store`'s write path takes a typed `a` and derives the
  omit set from its own specs, so a client payload cannot set a `generated`/serial
  column.
- **Secret leakage.** A `Secret` field reaching `Codec.auto` is a compile-time
  rejection (reusing the `SecretOrSink` category); there is no representable codec
  that serialises a `Secret` to JSON or a column. `SqlFragment`'s `Debug` already
  prints bind *count*, never bind *values*, so a fragment carrying a revealed
  secret can't leak it into a log.
- **Resource exhaustion.** `fromJsonSafe` bounds untrusted input size before
  parsing; JSON nesting depth is bounded by the parser kernel; `inList []` emits
  `(1=0)` rather than a pathological/invalid statement.
- **Round-trip soundness.** The correctness law `fromJson (toJson x) == Ok x`
  holds over a fuzzed record corpus (a property test); the derive's
  byte-identical-emit test proves `auto` adds no emit the hand-written builder
  doesn't already exercise, so THE SEAL holds by construction.

---

## Implementation plan (sliced, guardian-gated)

Both codec landings sequence **after** the Kernel Row (the field classifier binds
to the consolidated `TyShape`/`BuiltinTag` vocabulary). Each slice is
independently landable and green under the full CI-replica gate; each language
boundary (the `auto` elaboration; the `Cond`→`SqlFragment` lowering) gets a
mandatory security-soundness-guardian review before merge.

**Slice 1 — `Ipe.Codec` surface + JSON direction, no compiler work.** Ship
compiled-source `Ipe.Codec` (type, primitives incl. `decimal`/`money`,
`maybe`/`list`/`map` with the *invariant* `map`, `object`/`field`/`buildObject`,
`enum`, `taggedUnion`/`varN`, projections, `fromJsonSafe`). Prove
`fromJson (toJson x) == Ok x` over a fuzzed corpus. No `auto` yet. This is the
**recommended first slice** — it delivers a usable, hand-written codec surface
with the corrected combinator types and zero compiler risk, and it lets Store's
design be validated against a real `Codec` before the derive exists.

**Slice 2 — the `auto` derive.** Add the canon-stage recogniser + type-driven
rewrite of `auto`/`autoCamel`/`autoWith` into the builder chain; the
field-classifier binds to `TyShape`/`BuiltinTag`; `Secret`/data-ADT/recursion/
open-record rejections; the byte-identical-to-hand-written anti-drift test.

**Slice 3 — `Ipe.Db.Store`.** The `Store a` handle, `ColumnSpec` typed builders,
the `Cond`→`SqlFragment` lowering, CRUD over `Db.findWhere` + `insertFields`/
`updateFields`, `create`/`migrate` DDL from `Shape` + specs, `selectRaw`. Guardian
review focuses on the lowering (every identifier validated, every value bound) and
the generated-column omit set (no mass-assignment).

---

## Divergence ledger — prior art, with skepticism

Prior art (a private reference implementation) informs the *surface vocabulary*
only; every behaviour is re-derived against PRINCIPLES, never transcribed.

| Kept (idea worth keeping) | Rejected / diverged (and why) |
|---|---|
| The invariant `Codec a = { enc, dec, shp }` bundling wire + storage shape in one value | **Runtime-reflection `auto`** → compile-time canon derive: no reflection kernel, no runtime registry, no struct tags (Security/Soundness/Efficiency; `no dyn Any`) |
| The `object`/`field`/`buildObject` applicative record builder | **Covariant `map`** (as the superseded Ipê draft wrote it) → **invariant `map` with a bijection** — a one-way map can't encode (make-invalid-states-unrepresentable) |
| Nullary `enum` as a queryable TEXT scalar (not a blob) | **`enum` returning `""` on an unmapped value** → total by construction via exhaustiveness (no partial function) |
| `taggedUnion`/`varN` for data ADTs; `fromJsonSafe` size guard | **Separate encode `toTagged` + decode `varN`** (two sources of truth) → each `Variant` carries both directions; encoder derived from the variant list (SSOT) |
| `Store a` = codec + DB-only facts; `serial`/`defaultNow`/`touchOnUpdate` semantics; additive idempotent `migrate`; `selectRaw` escape | **Stringly colspec flags** (`"!"`, `"u"`, `"dnow"`, `"dint="`, `"|"`-delimited, parsed at DDL time) → typed `ColumnSpec` ADT (make-invalid-states-unrepresentable) |
| The `Cond`/`Query` builder shape (leaves + `and_`/`or_`/`not_` + ordering/paging) | **`Cond` embedding the SQL operator as a string + string-built SQL** and **`findBy`/`delete` interpolating a caller `col` on a guard-bypassing fast path** → typed operator; whole `Cond` lowered to the audited `SqlFragment` via `Sql.*`; every identifier through `valid_sql_ident`, every value bound (Security) |
| Abstract `ColType` so one codec works on every dialect | Dialect mapping stays entirely in the existing `Ipe.Db` kernels — Store re-implements no dialect logic (SSOT for the parameterised-SQL boundary) |

---

## Affected issues

| Issue | Relationship |
|---|---|
| 663 | **IMPLEMENTS** — this design is `Ipe.Codec`: the invariant `Codec a` surface + the compile-time `Codec.auto` derive; supersedes `codec-auto-derive-design.md`. |
| 680 | **IMPLEMENTS** — `Ipe.Db.Store`: codec-driven typed persistence, `ColumnSpec`, `Cond`→`SqlFragment`, CRUD over the audited `Db`/`Sql` surface. |
| 641 | **COORDINATES** — `Db.open <driver> <dsn>` supplies the connection Store reads/writes against; the codec is dialect-agnostic, the driver selects the dialect. |
| (Kernel Row) | **DEPENDS ON** — the derive's field classifier binds to the consolidated `TyShape`/`BuiltinTag` vocabulary + `RowOpen` carrier. |
| (ADR 0029 / 0057) | **EXTENDS** — `Codec`/`Store` ship compiled-source; `auto` adds exactly one canon recogniser; all Security-critical primitives stay the existing native kernels. |
| (Unsafe convention) | **COORDINATES** — `autoWith` override, `selectRaw`, and raw SQL route to the `Ipe.<M>.Unsafe` convention; the safe derived path is the default that makes those escapes rare. |
| (reserved-type registry) | **REUSES** — `Secret` rejection + `Money`/`Decimal` handling read the existing `SecretOrSink` / reserved-type categories in `resolve.rs`; no parallel opaque-type list. |

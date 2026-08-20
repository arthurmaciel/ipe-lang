# The typed database: the safest and easiest data layer

Status: design proposal. Every fenced Ipê block illustrates the **proposed
surface**, not shipped API; every fenced Rust/SQL block illustrates the intended
seam, not verified code.

This is the single unifying spec for Ipê's data layer. The foundation surfaces —
the one-codec core (`Codec a` driving JSON + columns + reads + writes,
`Codec.auto`, the typed `ColumnSpec` ADT, CRUD, the `Cond`→`SqlFragment` query
lowering, additive `Store.migrate`) and row security as **data** (a `Policy`
algebra, deny-by-default, enforced in-query and in-DB) — are specified in the
Appendix at the end. Explicit data-preserving column/table renames are already
shipped. This body adds the pillars that make the whole thing type-safe end to
end.

The new material here: **accessor-typed columns** (the query builder becomes
fully type-checked, not stringly), **first-class per-column newtypes**
(cross-row-type confusion becomes unrepresentable), the **auto-migration engine**
(the schema is derived, diffed, classified, and proven to match the code), and
the **input/persistence type split** (mass-assignment closed by construction).
The last section sequences all of it.

## The thesis: safest because easiest, easiest because safest

The claim this spec must earn: *the shortest program a developer can write is
also the most secure one, and the type checker — not discipline, not review, not
a linter — is what makes it so.* Every other data layer trades one for the other:
raw SQL is powerful but injection-prone; an ORM is convenient but reintroduces
drift between the object, the schema, and the migration; a query builder is safe
but verbose and stringly. Ipê collapses the trade because **one value — the
`Codec a` — is the single source of truth for six surfaces**, and each surface is
derived, never hand-synced:

```
                     ┌─ JSON encode / decode        (the API boundary)
                     ├─ column set + types + DDL     (the table)
   Codec a  ───────► ├─ row read (decode) / write (encode)   (CRUD)
   (write it once)   ├─ typed column handles         (the query builder)
                     ├─ the migration target schema  (evolution)
                     └─ the row-security column basis (authorization)
```

The precedence order (Security > Correctness > Soundness > Efficiency >
Completeness > Readability) decides every conflict. The load-bearing
consequences, stated once and enforced everywhere below:

- **No injection is representable.** Every identifier is validated at construction
  and re-validated where it reaches SQL (defense in depth); every value is a
  bound parameter. There is no code path that concatenates a caller string into
  SQL text.
- **No drift is representable.** Encode, decode, columns, DDL, query columns, and
  the migration target are one derivation from one codec. A field added, renamed,
  or retyped changes exactly one place.
- **No silent data loss is representable.** A destructive migration is never
  generated silently; a rename is an explicit directive; a decode mismatch is a
  typed error, not a wrong value.
- **No leak is representable.** A `Secret`/`Password` field is unencodable to JSON
  or a column *by construction* (compile-time rejection), reusing the existing
  reserved-type category.

"Easiest" is then a corollary, not a separate goal: because everything derives
from the codec, the happy path is three lines, and the type checker catches the
mistakes that other stacks catch only in production.

```elm
users : Store User
users =
    Store.fromCodec "users" (Codec.auto blankUser)
        |> Store.primaryKey .id
        |> Store.unique .email
```

Those three lines yield: a migratable table, JSON encode/decode, injection-safe
CRUD, a fully-typed query builder, and a derived migration target — with a
`.email` that is checked to exist and be `String`, and an `.id` checked to exist.

---

## Pillar 1 — Accessor-typed columns: the query builder stops being stringly

This is the crux of "safest." The existing store design already validates a
stringly column name against the codec's column list at build time, so
`primaryKey "emial"` is a build error. That closes *wrong name*. It does not
close *wrong value type* (`eq "age" (SqlString "old")`) or *cross-type id
confusion* (`eq "owner_id" someFoodId`), because the column is a bare string and
the value is an untyped `SqlValue`. Acadia closes both by writing `primary = .id`
— a real accessor, type-checked against the record. Ipê adopts that, and goes
one step further: the accessor yields a **phantom-typed column handle** that
makes the *value* side type-check too.

### The `Column` handle

```elm
-- An opaque, validated reference to one column of a `row`-typed table, carrying
-- the field's type `t`. You never construct it from a string; you write `.field`.
type Column row t     -- opaque; phantom in both row and t
```

`Column User Int` is "the `age` column of the `User` table, holding `Int`". It
carries, internally, the validated SQL identifier and the field's codec (so a
value of type `t` can be bound as a parameter). It is phantom in `row` so a
`User` column cannot be used to query a `Post` table, and phantom in `t` so the
comparison value must match the column's type.

### Accessor recognition: the one compiler feature that unlocks the surface

`.field` in Ipê is already `\r -> r.field`. To turn `.email` into a
`Column User String`, the compiler must read the field *name* and *type* at the
call site. This is the **same canon-stage elaboration `Codec.auto` uses** — the
recognition path for reserved bindings (`Ffi.kernel` aliases). One new rule:

> When an accessor literal `.field` appears where a `Column row t` is expected,
> canon rewrites it to the column handle for `field`: it reads the solved record
> type `row`, confirms `field` exists (else a clean `IPE-…` diagnostic naming the
> field and the type), takes the field's type as `t`, validates the derived SQL
> identifier (snake_case transform, `valid_sql_ident`), and emits the handle.

This is a *literal* recognition, exactly like `Codec.auto`'s witness: `.field`
must be an accessor literal, not an arbitrary `row -> t` function (an arbitrary
projection has no column name). A non-literal in column position is a clean
rejection pointing at the `.field` form — parse-don't-validate at the surface.

One feature, whole surface. Every place that took a stringly column now takes an
accessor: `primaryKey .id`, `unique .email`, `index .createdAt`,
`defaultNow .createdAt`, `orderAsc .rank`, and every query leaf below. The column
name string still exists — as the *derived*, validated identifier inside the
handle — so the `SqlFragment` layer and its `valid_sql_ident` re-check are
unchanged (defense in depth intact).

### The typed query leaves

```elm
Store.eq  : Column row t -> t -> Cond row
Store.neq : Column row t -> t -> Cond row
Store.gt, gte, lt, lte : Column row comparable -> comparable -> Cond row
Store.like     : Column row String -> String -> Cond row   -- wildcards stay data
Store.isNull   : Column row (Maybe t) -> Cond row          -- only nullable columns
Store.inList   : Column row t -> List t -> Cond row        -- empty ⇒ (1=0)
Store.and, or  : List (Cond row) -> Cond row
Store.not       : Cond row -> Cond row
Store.orderAsc, orderDesc : Column row t -> Query row -> Query row
```

Now the type checker rejects, at compile time and with a field-named diagnostic:

```elm
Store.query users
    |> Store.where (Store.eq .rank Gold)          -- ok: .rank : Column User Rank, Gold : Rank
    |> Store.where (Store.gt .age 18)             -- ok: Int vs Int
    |> Store.where (Store.eq .age "old")          -- TYPE ERROR: String ≠ Int
    |> Store.where (Store.isNull .email)          -- TYPE ERROR if email : String (not Maybe)
    |> Store.where (Store.eq .ownerId aFoodId)    -- TYPE ERROR: FoodId ≠ UserId  (see Pillar 2)
```

The value binds through the column's field codec, so `Store.eq .rank Gold` binds
the enum's wire name `"gold"` as a parameter — the developer never touches
`SqlValue`. `comparable` on the ordering leaves is the existing built-in
comparability constraint, so you cannot `gt` a blob column.

**Divergence from prior art (Correctness + MISU).** Prior-art query builders
carry the column as a string and the operator as a string inside the `Cond` ADT,
rendered by string-building; a wrong column or a mistyped value is a runtime
error or silent wrong result. Ipê's `Cond row` carries a *typed* column handle
and a *typed* operator, and still lowers to the audited `SqlFragment` via
`Sql.eq (Sql.column …) (Sql.param …)` — so the type safety is additive over, not
a replacement for, the injection safety.

---

## Pillar 2 — First-class per-column newtypes: id confusion is unrepresentable

A `UserId` and a `PostId` are both `String` on the wire and `TEXT` in the column,
but must never be interchangeable in code. This is the swap the make-invalid-
states-unrepresentable rule exists to forbid: "a value's role lives in its type,
never in a bare primitive another value of the same shape could stand in for."

Ipê already has the pieces: a newtype `type UserId = UserId String` and
`Codec.map (\s -> UserId s) (\(UserId s) -> s) Codec.string : Codec UserId`. This
pillar makes the pattern first-class and threads it through the whole store so the
distinctness is *enforced*, not merely *available*:

```elm
type UserId = UserId String
type PostId = PostId String

userIdCodec : Codec UserId
userIdCodec = Codec.id UserId (\(UserId s) -> s)   -- sugar for map over string

type alias Post =
    { id : PostId, author : UserId, title : String, body : Text }
```

Because `author : UserId`, the derived `Column Post UserId` for `.author` makes
`Store.eq .author somePostId` a type error, and `Store.get posts somePostId` a
type error if `posts`'s pk is `UserId` — cross-row-type id confusion has no
representation. `Codec.auto` derives the column from the newtype's underlying
codec (so the storage/wire shape is unchanged — still `TEXT`), while the Ipê type
stays distinct. `Codec.id ctor project` is the one-liner for the overwhelmingly
common "opaque id/handle over a scalar" case (invertible by construction, since
`map` carries both directions).

**Divergence (MISU, applied deeper than prior art).** Prior art offers a
newtype-over-scalar codec but stops at JSON — the *query* surface still takes a
bare string, so id confusion re-enters at the `where`/`get`/`delete` seam. Ipê
carries the newtype through the typed column handle to the query leaves, so the
guarantee holds at the exact seam where ids get mixed up.

**Reserved-role fields keep their existing protections.** A `Secret`/`Password`
field is still a compile-time encode rejection (the leak the Security principle
forbids), reusing the `SecretOrSink` reserved category — no parallel list. A
`Password`-typed field additionally routes through hash-on-write (it is
*unreadable* back to plaintext by construction), leveraging the runtime's crypto
roles rather than a stringly "this is a password" flag.

---

## Pillar 3 — Row-level security as data (integration)

Row security's surface is in the Appendix; this section records the principle and
how it composes with the pillars above:

- **A policy is DATA, not closures.** Acadia encodes a policy as a record of
  functions (`Security.policy { access = \user row -> … }`). Ipê **forbids
  functions in records** (that decision stands: TEA is the only state mechanism),
  so a policy is a small algebra — `ownerColumn .author`, `publicRead`,
  `immutable .createdAt`, combined with `and` — over *validated column names*.
  Data is testable, comparable, showable, and lowers to a `SqlFragment`; a closure
  is none of those and would need a whole-program capture proof to stay sound.
- **Deny by default.** The absence of a policy is denial; a `Secured` store cannot
  be reached without attaching one.
- **Defense in depth.** Layer 1: an authenticated, opaque `Principal` (mintable
  only from a verified session, never a raw client value). Layer 2: the policy
  compiles to a `WHERE … = $principal` fragment `AND`-ed into every read/write, so
  the database filters and the app never fetches out-of-policy rows. Layer 3
  (optional): native DB RLS for a second enforcement boundary.

Composition with the pillars: policy columns are written as accessors
(`ownerColumn .author`), so they gain the same compile-time existence + type
check as query columns (a policy over a non-existent or non-id column is a build
error). A policy keyed on a `UserId` principal and an `.author : UserId` column
is type-checked to match. The `…As` secured operations (`Store.allAs`,
`Store.insertAs`, …) take a `Principal` and a `Secured` and are the only way to
touch a secured store — the unsecured operations do not accept a `Secured`, so a
secured store cannot be read unfiltered even by mistake.

---

## Pillar 4 — The auto-migration engine: the schema is derived, diffed, and proven

This is the largest new subsystem and the second half of "easiest + safest."
Today `Store.migrate` is additive-only (create table, add nullable columns) and
renames are an explicit shipped directive. The engine generalizes this into a
derive → introspect → diff → classify → review → apply pipeline that **never
loses data silently** and lets CI **prove the live schema matches the code**.

### The target schema is a pure derivation

```elm
-- The schema a store's type REQUIRES, derived purely from the codec Shape +
-- ColumnSpecs. No I/O. This is the SSOT the whole engine diffs against.
Store.targetSchema : Store a -> Schema

type alias Schema =
    { table : TableName
    , columns : List ColumnDef       -- name, ColType, nullable, default
    , primaryKey : Maybe ColumnName
    , unique : List (List ColumnName)
    , indexes : List (List ColumnName)
    }
```

### The pipeline (`ipe db plan` → review → `ipe db migrate`)

```
   Store.targetSchema   ─┐
   (derived from codec)  ├─► diff ─► classify (safe/gated/blocked) ─► render ─► a REVIEWABLE
   live DB schema       ─┘          per the hazard rules below       plan file   migration file
   (introspection)                                                              (checksummed, committed)
```

1. **Introspect** the live DB: SQLite `PRAGMA table_info`/`index_list`, Postgres
   `information_schema.columns`/`pg_indexes`. Dialect-aware, same seam as the DDL
   renderer.
2. **Diff** target vs current by column/index name → add / drop / change ops.
3. **Classify** each op against fixed hazard rules (below) as **safe** (auto),
   **gated** (generated but requires explicit opt-in), or **blocked** (refused;
   the tool tells you the safe expand/contract sequence to write by hand).
4. **Render** dialect-correct DDL and wrap it as a checksummed, versioned step for
   the existing idempotent ledger — **generated, not applied**. The developer
   reviews the plan and commits it, so CI runs the exact migration production will.

### The hazard rules (each a fail-closed default)

- **Rename is indistinguishable from drop+add → never auto-drop.** A column
  present in current but absent from target is reported as a *pending drop*,
  never auto-executed. The developer resolves it either as a rename — the shipped
  explicit directive, now written with accessors, `Migrate.renameColumn .oldName
  .newName` (emits `ALTER … RENAME`, preserves data) — or as an intentional drop
  behind an explicit `Migrate.dropColumn` opt-in. The default keeps the orphan
  column. (This is where the shipped rename work plugs into the engine: the
  planner reads rename directives to emit `RENAME` instead of `drop+add`.)
- **Destructive / lossy ops are opt-in only.** Drop column, narrow a type, drop a
  constraint that can reject data — generated but emitted commented-out behind a
  `DESTRUCTIVE` marker and an explicit confirmation, never silently applied.
- **A new NOT NULL column needs a safe path.** `ADD COLUMN x NOT NULL` fails on a
  non-empty table without a default. The engine refuses the bare form and emits
  either a `DEFAULT` or the three-step expand → backfill → contract sequence.
- **Type changes ride a fixed lattice.** widen (`int→text`, `int→bigint`) = safe;
  narrow (`text→int`, `bigint→int`) = gated with a `USING`/rebuild cast that can
  fail on bad rows; incompatible = blocked. SQLite (weak `ALTER`) renders the
  table-rebuild dance (new table → copy → drop → rename) inside a transaction;
  Postgres renders `ALTER COLUMN … TYPE … USING`.
- **Blob (JSON) columns evolve in the app, not the DDL.** A nested/ADT field is
  one `TEXT` column; changing its shape does not change the column, so old rows
  hold old-shape JSON. The codec's decoder must tolerate old shapes (optional
  fields, variant aliases); the engine flags a blob-shape change as an
  app-level-migration note, not a DDL op.

### Out of the box: the migration is a proof that the DB matches the type

Because the target schema is *derived* from the codec (SSOT) and the ledger
records what has been applied, `ipe db plan --check` is a **drift oracle**: it
exits non-zero if the derived target differs from the live/committed schema with
no covering migration. Wire that into CI and **schema drift becomes
undeployable** — you cannot merge code whose types no longer match the committed
schema without either a migration or an explicit acknowledgment. The type is not
merely *reflected* in the schema; it is *proven equal* to it, continuously. No
other data layer offers this, because no other derives the schema from the one
value the code already uses.

### The plan/sign ceremony (borrowed, scaled to risk)

For additive/safe plans, `plan` → review → `migrate` suffices. For any gated
(destructive/lossy) op, applying requires an explicit approval step — an
`ipe db sign` that records reviewer intent alongside the plan (Acadia's
plan→sign→publish, scaled so the ceremony appears only when data is at risk, not
on every additive change). This keeps the safe path frictionless and reserves the
ritual for exactly the operations that can lose data.

---

## The input / persistence split: mass-assignment closed by types

Untrusted input and a persistence row are different types, and the store enforces
it. Decoding a request body straight into a `User` (the persistence record) is
the mass-assignment bug — a client sets `id`, `role`, or `createdAt`. The store's
write path already takes a typed `a` and omits generated columns, but the deeper
guarantee is a *type-level* input/row split:

```elm
type alias NewUser = { email : String, nick : Maybe String }   -- client-settable ONLY
type alias User    = { id : UserId, email : String, nick : Maybe String, createdAt : Instant, role : Role }

Store.insertNew : Db -> Store User -> (NewUser -> User) -> NewUser -> Task Error UserId
```

`fromJsonSafe` decodes untrusted input into `NewUser` (size-bounded before
parsing); the pure `NewUser -> User` fill supplies server-controlled fields
(generated id, `now`, default role). A client literally cannot set `role`: there
is no field for it in the type it can reach. Mass-assignment is closed by the
type boundary, not by a denylist that can miss a field.

---

## Test discipline: the producer is asserted, not assumed

The one hard lesson already filed (the producer regression guard): a golden that
hard-codes DDL and *claims* to match the generator is a tautology that hides a
producer bug. Every derivation in this spec — `targetSchema`, the accessor→column
elaboration, `Codec.auto`, the migration classifier — ships with a test that runs
the **real** producer and asserts its output, and (per the SSOT rule) an
anti-drift test asserting the derived form and the hand-written equivalent emit
identically. A derivation with only a hand-transcribed expectation is not
considered covered.

---

## Implementation phasing (dependency-ordered)

Each phase is independently shippable, guardian-gated at every emit/language-
boundary step, and leaves `main` green. Renames (shipped) already compose onto
whatever store shape lands.

- **Phase A — the codec + store core** (Appendix surface):
  `Codec` type, primitives, `object/field/buildObject`, `enum`, `taggedUnion`,
  `Codec.auto`; `Store a`, `fromCodec`, the typed `ColumnSpec` ADT, CRUD, the
  `Cond`→`SqlFragment` lowering, additive `migrate`. This is the foundation; land
  it first, stringly specs and all.
- **Phase B — accessor recognition + typed columns** (Pillar 1): the canon-stage
  `.field`→`Column row t` elaboration and the phantom-typed query leaves; migrate
  the Phase-A specs from `"id"` to `.id`. One compiler feature, whole surface.
- **Phase C — first-class newtypes** (Pillar 2): `Codec.id`, newtype field
  derivation, and threading the newtype through the typed column handle to the
  query/get/delete seams.
- **Phase D — row security** (Pillar 3 + Appendix): the
  `Policy` algebra, `Principal`, `Secured`, the `…As` operations, in-query +
  in-DB enforcement — written with accessors from Phase B.
- **Phase E — the auto-migration engine** (Pillar 4): `targetSchema`,
  introspection, diff, the hazard classifier, the reviewable plan, `ipe db plan
  [--check]` / `sign` / `migrate`, and the CI drift oracle. Depends on the codec
  shape (A), accessor renames (B), and the shipped rename directive.

The input/persistence split and the test discipline are cross-cutting: applied
within each phase, not a phase of their own.

---

## Appendix — foundation surfaces

The condensed surface the phases implement. Signatures are the contract; the
rationale is the body above.

### `Ipe.Codec` (Phase A)

```elm
type Codec a           -- opaque: holds enc : a -> Value, dec : Decoder a, shp : Shape
type ColType = CText | CInt | CReal | CBool | CBlob | CNull ColType
type Shape   = SRecord (List (String, ColType)) | SScalar ColType | SBlob

shape : Codec a -> Shape
toValue : Codec a -> a -> Value ;  toJson : Codec a -> a -> String
fromJson : Codec a -> String -> Result Error a
fromJsonSafe : Int -> Codec a -> String -> Result Error a   -- size-bounded before parse

string : Codec String ;  int : Codec Int ;  float : Codec Float ;  bool : Codec Bool
decimal : Codec Decimal ;  money : Codec Money              -- lossless, string-encoded (never Float)
maybe : Codec a -> Codec (Maybe a) ;  list : Codec a -> Codec (List a)
map  : (a -> b) -> (b -> a) -> Codec a -> Codec b           -- INVARIANT: both directions
id   : (s -> a) -> (a -> s) -> Codec a                      -- sugar: newtype over a scalar (Pillar 2)

object : ctor -> ObjectCodec ...                            -- applicative record builder;
field  : String -> (rec -> f) -> Codec f -> ObjectCodec ... -> ObjectCodec ...   --  one call feeds
buildObject : ObjectCodec a -> Codec a                      --  enc + dec + column-shape together

enum : (a -> a -> Bool) -> List (a, String) -> Codec a      -- SScalar CText (queryable); total
taggedUnion : List (Variant a) -> Codec a                   -- SBlob; each Variant carries enc+dec
auto : a -> Codec a                                         -- compile-time derive (Phase A2)
autoCamel, autoWith : …                                     -- snake_case default; camel / per-field override
```

Field→leaf derivation for `auto`: `String/Int/Float/Bool`→primitive; `Decimal/Money`→exact string;
`Maybe t`→scalar `CNull`, else `SBlob`; `List/nested record`→`SBlob`; nullary enum→`SScalar CText`;
`Secret`→**compile-time rejection**; data-ADT/function/self-recursive→**rejection** (points at `taggedUnion`).

### `Ipe.Db.Codec` — the codec↔SQL row seam (Phase A/B)

The seam that lets one `Codec a` produce a row's binds and decode a DB row back
to `a`, without a second decoder and without a string round-trip. `Ipe.Codec`
stays SQL-free; this module is the only place a codec meets `SqlValue`, importing
both `Ipe.Codec` and the reserved `SqlValue` type.

```elm
codecToBinds : Codec a -> a -> Result Error (List (String, SqlValue))
codecFromRow : Codec a -> Dict String String -> Result Error a   -- Row = Dict String String
```

Both directions reuse the codec's OWN JSON encoder/decoder over an in-memory
`Value` (design A: the in-memory Value walk), on top of two minimal new
`Ipe.Json.Decode` seams:

- `value : Decoder Value` — the identity decoder, yielding the raw JSON node.
- `decodeValue : Decoder a -> Value -> Result Error a` — run a decoder against
  an in-memory `Value`. This is exactly the tail of `decodeString` after its
  parse step (the SAME `(decoder.run)(&val)` path) — NOT a second decoder.

`codecToBinds` runs `enc` to ONE `Value`, requires the codec's `Shape` to be an
`SRecord [(col, colType)]` (else `Err`, fail-closed), and for each column reads
that field back out of the `Value` with the JSON decoder matching its `ColType`,
coercing to a **bound** `SqlValue`. `codecFromRow` parses each row cell into the
JSON scalar its `ColType` implies, assembles ONE `Value` object, and runs the
codec's own `Decoder` on it via `decodeValue`.

The coercion table (both directions symmetric):

| `ColType`     | encode → `SqlValue`                       | decode: cell → JSON node                 |
|---------------|-------------------------------------------|------------------------------------------|
| `CText`       | JSON string → `SqlString`                 | verbatim → JSON string                   |
| `CInt`        | JSON int → `SqlInt`                        | `String.toInt` → JSON number (else Err)  |
| `CReal`       | JSON number → `SqlFloat`                   | `String.toFloat` → JSON number (else Err)|
| `CBool`       | JSON bool → `SqlInt` 0/1 (bound flag)     | `0`/`1`/`true`/`false` → JSON bool        |
| `CBlob`       | node → compact JSON as `SqlString` TEXT   | cell parsed as JSON TEXT → node           |
| `CNull inner` | JSON null → `SqlNull <inner witness>`; else coerce as `inner` | absent/`NULL` → JSON null; else parse as `inner` |

A `Decimal`/`Money` field's codec declares `CText` (it encodes to a JSON string),
so it round-trips as TEXT through the `CText` arm — **never** coerced through a
lossy `Float`.

**Fail-closed / injection-safety argument.** Every value the bridge produces is a
BOUND `SqlValue` parameter; the bridge builds NO SQL text, so it adds no
injection surface over `Ipe.Db.Sql`'s already-parameterised binds (`SqlValue`
never crosses the JS seam either — it is a sink type). A codec whose `Shape` is
not an `SRecord` cannot name columns and is a typed `Err`, not a guess. A scalar
whose encoded JSON does not match its `ColType`, a cell that does not parse to
its declared shape, or a row missing a required column are all typed `Err`s —
schema drift surfaces as an error, never a silently mis-typed bind or a wrong
value. `Secret` stays unencodable (no codec path binds it), so no `Secret`
reaches a bind here.

### `Ipe.Db.Store` (Phase A/B)

```elm
type Store a           -- opaque: name (validated), codec : Codec a, spec : List ColumnSpec, pk
type ColumnSpec        -- typed, NOT a stringly flag protocol
    = PrimaryKey ColumnName | Serial ColumnName | Unique ColumnName | Index ColumnName
    | DefaultNow ColumnName | TouchOnUpdate ColumnName
    | DefaultValue ColumnName SqlValue | ComputedOnInsert ColumnName (() -> SqlValue)

fromCodec : String -> Codec a -> Store a
primaryKey, serial, unique, index, defaultNow, touchOnUpdate : Column a t -> Store a -> Store a  -- ACCESSOR (Pillar 1)

create : Db -> Store a -> Task Error ()
migrate : Db -> Store a -> Task Error (List String)            -- additive/idempotent (+ shipped renames)
insert : Db -> Store a -> a -> Task Error Int ;  insertMany : Db -> Store a -> List a -> Task Error Int
upsert : Db -> Store a -> a -> Task Error Int                  -- ON CONFLICT(pk)
update : Db -> Store a -> a -> Task Error Int                  -- by pk, whole record
get : Db -> Store a -> t -> Task Error (Maybe a)               -- by pk (typed key)
all : Db -> Store a -> Task Error (List a) ;  delete : Db -> Store a -> t -> Task Error Int
selectRaw : Db -> Codec row -> SqlFragment -> Task Error (List row)   -- JOIN/aggregate escape

-- Query builder (Pillar 1: accessor-typed columns → Cond lowered to SqlFragment)
type Column row t      -- opaque, phantom; written as an accessor literal `.field`
type Cond row ;  type Query row
query : Store a -> Query a
where : Cond row -> Query row -> Query row
eq, neq : Column row t -> t -> Cond row
gt, gte, lt, lte : Column row comparable -> comparable -> Cond row
like : Column row String -> String -> Cond row ;  isNull : Column row (Maybe t) -> Cond row
inList : Column row t -> List t -> Cond row      -- empty ⇒ (1=0)
and, or : List (Cond row) -> Cond row ;  not : Cond row -> Cond row
orderAsc, orderDesc : Column row t -> Query row -> Query row ;  limit, offset : Int -> Query row -> Query row
toList : Db -> Query row -> Task Error (List row) ;  toMaybe : Db -> Query row -> Task Error (Maybe row)
count : Db -> Query row -> Task Error Int
```

Write-path safety: `Serial`/`DefaultNow`/`ComputedOnInsert` columns are OMITTED from the emitted
statement (existing `SqlField`/`OmitField` seam), and `insertNew : Db -> Store a -> (input -> a) -> input
-> Task Error t` takes a client-settable `input` record + a pure fill, so a client cannot set a
generated column (mass-assignment closed by the type boundary).

### `Ipe.Db.Store` row security (Phase D)

```elm
type Policy            -- opaque, NO (..); a conjunction of Rules over validated columns
type Rule = OwnerColumn ColumnName | PublicRead | Immutable ColumnName
type Principal         -- opaque; the authenticated subject, mintable ONLY from a verified session
type Secured           -- opaque; a Store + a Policy — the ONLY value the `…As` ops accept
type UnsecuredStore    -- opaque; a store with no policy attached

unrestricted : Policy ;  ownerColumn : Column a t -> Policy ;  and : Policy -> Policy -> Policy
secured : Policy -> UnsecuredStore -> Result Error Secured     -- re-validates policy columns; deny-by-default
allAs, insertAs, updateAs, deleteAs, getAs : Principal -> Db -> Secured -> …   -- policyFragment AND-ed in
```

`policyFragment : Principal -> Policy -> SqlFragment` binds the principal as a parameter and `AND`s
into every read/write (defense layer 2, over the authenticated `Principal` of layer 1). A secured store
is unreachable through the unsecured operations, so it cannot be read unfiltered.

## PRINCIPLES ledger

- **Security (1):** no injection representable (validated identifiers + bound
  params, defense in depth); `Secret`/`Password` unencodable by construction;
  deny-by-default RLS enforced in-query and in-DB; mass-assignment closed by the
  input/row type split; untrusted decode size-bounded before parsing.
- **Correctness (2):** one codec ⇒ encode/decode/columns/DDL/query/target cannot
  disagree; reads decode through the codec so schema drift is a typed error, not a
  wrong value; the migration engine proves the live schema equals the type.
- **Soundness (3):** the derives emit no bespoke Rust (the SEAL holds by
  construction — an underivable field is an ipe-time rejection, never an
  exit-0-then-cargo-fail); typed columns and newtypes make illegal comparisons
  unrepresentable rather than deferred.
- **Efficiency (4):** derives monomorphize to the same native access + bind a
  hand-written codec emits (asserted byte-identical); no runtime reflection, no
  constructor registry, no `dyn Any`.
- **Completeness (5):** `selectRaw` and `Ipe.Db.Unsafe` remain the audited escapes
  for JOINs/aggregates and arbitrary SQL, so the typed surface never blocks a
  legitimate query — it just makes the raw door rare and disclosed.
- **Readability (6):** three lines to a fully-typed table; accessor columns read
  like field access; a policy and a migration plan are inspectable data.

## Divergences from prior art (recorded)

- **Codec.auto is a compile-time canon elaboration, not runtime reflection** — no
  witness reflection, no per-field struct tag, no constructor registry (the prior
  art's model, rejected on Security + Soundness + Efficiency).
- **Column facts and query columns are typed, not stringly** — the prior art's
  colspec-flag string and string-column `Cond` are replaced by the `ColumnSpec`
  ADT and the accessor-derived `Column row t` handle.
- **Row security is data, not closures** — Acadia's function-record policy is
  rejected because Ipê forbids functions in records; a `Policy` algebra lowers to
  a `SqlFragment` and is testable/inspectable.
- **Migrations are derived, classified, reviewed, and proven** — never auto-drop,
  never silently lossy; the derived target makes drift a CI-blockable oracle
  rather than a production surprise.

# `Codec.auto` — a compile-time record derive, zero runtime reflection

Status: design proposal, no implementation yet. Every fenced block below is
**illustrative of the proposed surface or the intended emit** — none is shipped
API or a verified command. Issue references use bare numbers in the affected-issues
table only; prose uses "issue NNN". This design depends on the Kernel Row
descriptor (`docs/architecture/tbd/kernel-row-design.md`) and coordinates with the
`Ipe.<M>.Unsafe` escape convention
(`docs/architecture/tbd/unsafe-escape-convention-design.md`).

## The problem, and the one decision that shapes everything

Ipê is pulling Sky's `Std.Codec` (issue 663) and `Std.Db.Store` (issue 680)
forward. A `Codec a` bundles one bidirectional mapping — an encoder `a -> Value`,
a decoder `Decoder a`, and a structural `Shape` — so a type's wire form is written
**once** and reused for JSON *and* the DB column layout, with no drift between the
two shapes. The hand-written builder (`Codec.object`/`Codec.field`/`buildObject`)
is unremarkable pure Ipê over the existing `Ipe.Json.Encode` / `Ipe.Json.Decode`
kernels. The whole design tension sits in one member: `Codec.auto blank`, which
produces a full codec for a record type *without* the field-by-field pipeline.

Sky implements `auto` by **runtime reflection**. Its `auto` calls FFI kernels
(`Ffi.kernel "Codec_autoEnc"`, `"Codec_autoDecoder"`, `"Codec_autoCols"`) that,
at run time, reflect over a zero-value *witness* record using Go struct tags
(`sky:"…"` emitted on every field) plus an emitted ADT constructor registry. The
tags exist precisely because Go erases the information reflection needs: a nullary
enum lowers to a bare `int` indistinguishable from a real `Int`; a data-carrying
ADT loses its name→constructor table. Sky closes that "erasure wall" with two
compiler-emitted runtime artifacts (per-field type tags + an `init()`-registered
constructor table) that reflection then reads.

**The owner's decided constraint for Ipê rejects that model.** `Codec.auto blank`
is a **compile-time derive**: a concrete, monomorphised encode/decode/shape
generated *per record type* — like a derived instance — never runtime type
reflection. There is no witness reflected at run time, no `sky:"…"`-equivalent tag
in the emitted struct, no runtime constructor registry, no reflection kernel. The
reasoning is already settled and is exactly the project's `concrete over generic`
principle (PRINCIPLES.md §No `dyn Any`): the field-assembly monomorphises to the
same native code a hand-written codec would emit, so `auto` costs **zero runtime
perf** versus the explicit builder. The JSON parse/serialise core and the DB I/O
stay native kernels; only the *field wiring* is generated, and it is generated at
compile time as ordinary Ipê-shaped IR. This document designs that derive.

The consequence that drives the placement decision: because there is no runtime
reflection, the *witness argument's only job is to name the type*. `Codec.auto`
cannot be a normal function — a normal function receives its argument as a runtime
value and has no access to that value's static field list. `auto` must be a
**compiler-elaborated form**: the compiler reads the record's field list from the
solved type at its call site and *replaces* the `auto` call with the concrete codec
expression it derives. The witness is elaboration input (a type carrier), not a
runtime input.

## Surface — what `Codec` and `Codec.auto` look like in `.ipe`

The surface is a compiled-source stdlib module `Ipe.Codec` (ADR 0029), byte-for-byte
the shape Sky ships, minus the reflection internals:

```
module Ipe.Codec exposing
    ( Codec, ColType(..), Shape(..)
    , toJson, fromJson, fromJsonSafe, toValue, shape
    , string, int, float, bool, maybe, list, map
    , object, field, buildObject
    , auto, autoCamel, autoWith
    )

-- A bidirectional codec: encoder + decoder + structural shape.
type Codec a = Codec { enc : a -> Value, dec : Decoder a, shp : Shape }

-- The DB-side column type a codec maps to.
type ColType = CText | CInt | CReal | CBool | CBlob | CNull ColType

-- Record -> columns; scalar -> one typed column; nested/list/ADT -> a JSON blob.
type Shape = SRecord (List (String, ColType)) | SScalar ColType | SBlob
```

Everything except `auto`/`autoCamel`/`autoWith` is ordinary Ipê with no compiler
involvement. `Codec.toJson`/`fromJson`/`toValue`/`shape` are thin projections;
`string`/`int`/`float`/`bool`/`maybe`/`list`/`map` and the `object`/`field`/
`buildObject` builder are pure combinators over `Ipe.Json.Encode.Value` and
`Ipe.Json.Decode.Decoder` (the `Decoder<E,T>` runtime carrier already exists and is
shared by `Ipe.Config`). Sky's `taggedUnion`/`varN`/`enum` builders port unchanged
as pure builders; they are orthogonal to `auto` and out of this design's critical
path.

`auto`/`autoCamel` differ from every other member: they have no ordinary Ipê body.
Their *signature* is `auto : a -> Codec a`, but the body is the elaboration target
(below). At a call site:

```
type alias User = { id : String, age : Int, active : Bool, nick : Maybe String }

userCodec : Codec User
userCodec = Codec.auto blankUser        -- one line; derived at compile time
```

`autoWith : List (String, Codec b) -> a -> Codec a` is `auto` with named-field
overrides: derive everything from the type EXCEPT the listed columns, which take the
supplied codec. It is the single-field escape hatch (a `Bool` stored as 0/1, a
custom wire enum) without hand-writing the whole record codec. Elaboration-wise it
is `auto` with a per-field substitution applied before assembly.

Naming convention matches Sky: `auto` emits **snake_case** column / JSON keys
(`priceMinor` → `price_minor`, the DB convention); `autoCamel` keeps camelCase. The
case transform is a pure compile-time string function applied to each field name
during elaboration.

## The elaboration — how the compiler derives a concrete codec

### Where in the pipeline

The derive is a **canon-stage elaboration keyed on a solved type**, realised as a
small dedicated pass sitting where canon hands off to inference/lowering — the same
seam ADR 0029's `inject_compiled_std_closure` already occupies. Concretely, a
`Codec.auto <witness>` application is recognised structurally in canon (the callee
resolves to the reserved `Ipe.Codec.auto` binding, the same way `Ffi.kernel`
aliases are recognised), and is rewritten — after the witness's type is solved —
into the ordinary `Codec.object … |> Codec.field … |> Codec.buildObject`
expression the developer *could* have written by hand. That is the whole trick:
**`auto` elaborates to the existing hand-written builder call**, so nothing
downstream (inference of the result, lowering, emit) needs to know `auto` ever
existed. The derived expression type-checks, lowers, and emits through exactly the
same path a hand-written codec does — which is what makes the SEAL hold by
construction (see Failure modes).

Why canon-adjacent and not lower: the derive needs the *field list with each
field's type*, and it must produce source-level Ipê that inference then checks. It
must run late enough that the witness's record type is solved (field names + field
types known), but its output must re-enter inference so the assembled
`object/field/buildObject` chain is verified like user code — a derived codec is
never trusted, it is *checked*. This is the "no third state" invariant of ADR 0029
applied to a derived expression: the elaboration either produces an expression that
resolves to the identical pipeline result as the hand-written form, or it produces a
clean `IPE-N…`/`IPE-T…` diagnostic. There is no exit-0-then-cargo-fail path because
there is no bespoke emit — the emit is the builder's emit.

### Enumerating fields and emitting per-field encode/decode

The field-enumeration machinery **already exists and is reused, not duplicated**.
The lowerer walks solved records today (`collect_records_in_ty` /
`ir_type_from_ty` in `lower.rs`) to surface every concrete record shape for struct
emission, and `from_canon` / `zonk` read a record's field map (a `BTreeMap` keyed by
field `Symbol`, fixing iteration order). The derive reads the witness's solved
`Ty::Record(fields, RowTail::Closed)` and, **per field in field-index order**
(non-regression rule: record field enumeration sorts by field index before any
order-dependent emission), selects the field's leaf codec by the field's solved
type:

| Field type | Derived leaf codec | Shape contribution |
|---|---|---|
| `String` | `Codec.string` | `SScalar CText` |
| `Int` | `Codec.int` | `SScalar CInt` |
| `Float` | `Codec.float` | `SScalar CReal` |
| `Bool` | `Codec.bool` | `SScalar CBool` |
| `Maybe t` | `Codec.maybe <derive t>` | scalar → `CNull`; else `SBlob` |
| `List t` | `Codec.list <derive t>` | `SBlob` (JSON-in-TEXT) |
| nested record `{…}` | `<derive that record>` (recursion) | `SBlob` |
| nullary enum `E` | generated enum codec (name ↔ constructor) | `SScalar CText` |

The output for a record is precisely the `Codec.object Ctor |> field "col₀" .f₀ c₀
|> … |> buildObject` chain, one `field` per record field, `colₙ` the case-transformed
name, `.fₙ` the getter (a record accessor, already a first-class canon form), `cₙ`
the recursively-derived leaf codec. Because this chain is *ordinary Ipê source*, the
per-field encode/decode monomorphises through the normal lowerer to the same native
field-access + `Json.Encode`/`Decode` kernel calls a hand-written codec produces —
the zero-runtime-cost claim is literally "the emit is byte-identical to the
hand-written builder's emit". No new backend arm, no new runtime function.

### Reusing the Kernel Row scheme machinery (explicit coordination)

The derive introduces **no new kernels** on the happy path — it composes existing
`Ipe.Json.Encode` / `Ipe.Json.Decode` / `Ipe.Codec`-builder members, each already a
Kernel Row or a compiled-source binding. The one place it touches the Kernel Row is
type-shape selection: to pick a field's leaf codec, the derive must classify a
solved field `Ty` against the primitive vocabulary (`Int`/`Float`/`Bool`/`String`/
`Maybe`/`List`/record/enum). That classification MUST read the **same**
`SchemeSpec`/`TyShape` builtin vocabulary the Kernel Row consolidates — it must not
grow a second, drifting "is this an Int field?" predicate.

Coordination is concrete: the Kernel Row's structural `TyShape` encoding
(`TyShape::{Int,Float,Bool,String,List,Maybe,Con(BuiltinTag,…),RowOpen,Var}`,
owned in `ipe_kernels`, interpreted in `ipe_types`) is exactly the closed type
vocabulary the derive's field-classifier needs. The derive's classifier is written
**against `TyShape`/`BuiltinTag`**, reusing the single `ipe_types` interpreter that
resolves a shape's builtins — so "how the compiler names `Int`/`List`/`Maybe`/a
record row" lives in **one** place shared by kernel schemes and codec derivation.
This is the direct dependency stated in the phased plan: the derive **sequences
after** the Kernel Row so the classifier binds to the consolidated vocabulary rather
than to the ~180-field `Builtins` cache the Row is dismantling. The derive also
reuses the Row's `RowOpen` carrier as the single row-polymorphism touchpoint: a
row-polymorphic *witness* (an open record) is rejected at derive time (see Failure
modes), and that rejection is expressed in `TyShape` terms, not a bespoke check.

### Nested records, unions, `Maybe`, lists — and the reserved opaque types

- **Nested record** → recurse: the derive emits the inner record's derived codec
  inline, contributing `SBlob` at the outer field (nested shapes serialise as a
  JSON-in-TEXT column, per Sky's storage rule). Recursion is bounded by the same
  `collect_records_in_ty` `seen`-set discipline the lowerer already uses, so a
  self-referential record type is *detected*, not looped on (Failure modes).
- **`Maybe t`** → `Codec.maybe <derive t>`; `Nothing` ↔ JSON `null`; a `Maybe
  scalar` stays a nullable scalar column (`CNull`), anything deeper is `SBlob`.
- **`List t`** → `Codec.list <derive t>`; JSON array; `SBlob` column.
- **Nullary enum** → a generated name↔constructor codec: encode to the readable
  constructor name (JSON string / `CText` column), decode by matching the name back
  to the constructor. Crucially, unlike Sky this needs **no runtime registry** — the
  compiler has the enum's constructor list at derive time and emits a concrete
  `match`/lookup inline, monomorphised. This is the derive's answer to Sky's
  "erasure wall": Ipê never erases the names into an `int` alias at the derive layer,
  because the derive runs *before* any lowering that would erase them.
- **Data-carrying ADTs** → **not auto-derivable** (matches Sky). The derive emits a
  clean `IPE-T…` diagnostic pointing the author at an explicit `Codec.taggedUnion` /
  `varN` codec. Fail-closed: absent a derivable shape, reject at derive time, never
  emit a partial codec.
- **Reserved opaque types** are handled by category, keyed on the reserved-type
  registry in `resolve.rs` (`RESERVED_BUILTIN_TYPES` / the opaque set / the
  seal-rejection category):
  - **`Money`, `Decimal`** are *encodable* but need a canonical, lossless leaf
    codec (a string-encoded exact decimal — never a lossy `Float`), contributing
    `SScalar CText`. These get a first-class derive arm because a persistence/JSON
    record routinely carries a price; a `Float` round-trip would violate Correctness.
  - **`Secret` is NOT encodable — by construction.** A field of type `Secret`
    reaching the derive is a **compile-time rejection** (`IPE-T…`), because encoding
    a secret to JSON or a DB column is exactly the secret-leak the Security principle
    and the non-regression rule ("no `Debug`/`Display`-formatting a secret into log
    or error") forbid. `Secret` is already in the seal-rejection
    `SecretOrSink` category in `resolve.rs`; the derive's field-classifier reuses
    that category — it does not maintain its own "is this a secret?" list (single
    source of truth). This is fail-closed: a record with a `Secret` field cannot be
    auto-derived, and the diagnostic tells the author to split the secret out of the
    persistence/wire record. (An author who genuinely must persist an encrypted blob
    uses an explicit codec over the already-sealed ciphertext type, not `Secret`.)

## The DB-mapping half — one codec, two consumers

The single derived codec drives **both** JSON and the dialect-safe DB column mapping
with no second shape to keep in sync — Sky's "no drift between JSON and DB shapes"
is structural here, because both consumers read the *same* `Codec`'s three fields:

- **JSON** reads `enc`/`dec` (via `Codec.toJson`/`fromJson`).
- **DB** reads `shp : Shape` — `SRecord [(col, ColType)]` for a record — and the
  same `enc`/`dec` for row read/write. `Ipe.Db.Store` (issue 680) consumes `shape
  codec` to derive its column list, column types, and nullability, and consumes
  `enc`/`dec` for the row round-trip. A `Table`/`Store` declaration then adds only
  the DB-design facts the *type* cannot express (primary key, unique, index,
  ordering, defaults) — everything else derives from the one codec.

Dialect safety is the DB backend's existing concern, not the codec's: `ColType`
is an abstract column type (`CText`/`CInt`/`CReal`/`CBool`/`CBlob`/`CNull`), and the
`Ipe.Db` layer already maps abstract column types + values to dialect-safe,
**parameterised** SQL (a `Bool` binds as `SqlBool` → Postgres `BOOLEAN` / SQLite
`0|1`; every value is a bind, never interpolated — the `GuardedSql` /
`SqlFragment` discipline). The codec supplies the *shape*; the Db layer supplies the
*dialect*. This split keeps the codec dialect-agnostic (one codec works on every
backend) and keeps injection safety where it already lives. Raw SQL and a
custom-format single column stay the `Ipe.Db.Unsafe` /
`autoWith`-override escape hatches respectively — the rare, marked exceptions the
unsafe-escape convention already governs.

## Placement — compiled-source surface, elaboration derive

Reconciling with the ADR 0029 placement policy (which cleanly separates
kernel-only stubs from compiled-source `.ipe` modules), `Codec` is a **hybrid** in
the exact sense ADR 0029 sanctions, and the split is principled:

- **The surface (`Ipe.Codec` type, combinators, builders, projections) ships as
  compiled-source `.ipe`** (ADR 0029) — it has rich internal structure (an ADT, a
  builder, recursive combinators) and needs zero new kernels, so the source path is
  strictly better (ADR 0029's stated rule).
- **`auto`/`autoCamel`/`autoWith` are a compiler elaboration** — neither a runtime
  kernel (there is no runtime reflection to host) nor plain `.ipe` (a plain function
  cannot read its argument's static field list). This is a *third* residency, and it
  is legitimate because the elaboration **produces** compiled-source-shaped Ipê:
  `auto`'s "body" is the derived `object/field/buildObject` chain, which is exactly
  what a compiled-source module contains. So `auto` is a compiled-source member whose
  body is *synthesised at its call site from the argument's type* instead of being
  fixed text. The reserved binding `Ipe.Codec.auto` is recognised in canon the same
  way `Ffi.kernel` aliases are; the recognition + rewrite is the only compiler-side
  addition. No new kernel slot, no new `naming.rs`/`ir::pretty` arm, no runtime
  function — consistent with ADR 0029's "adding a compiled-source module requires no
  new anti-drift sites" consequence, extended by exactly one elaboration recogniser.

## Failure modes — and why the SEAL holds

Every failure is a **compile-time diagnostic at derive time**, never a deferred cargo
failure and never a silent partial codec (fail-closed, PRINCIPLES.md §Security /
Make-invalid-states-unrepresentable):

- **A field whose type has no codec** (a bare function, a tuple past the supported
  arity, an unsupported opaque) → `IPE-T…` at the `auto` call site, naming the field
  and its type, suggesting an explicit `Codec.field`/`autoWith` override. Fail-closed:
  no codec is emitted for an underivable field.
- **A `Secret` field** → rejected by the reserved-type category (above); the
  diagnostic says a secret cannot be encoded and to remove it from the wire/persistence
  record. This is the one rejection that is a *feature*, not a limitation.
- **A data-carrying ADT field** → rejected with a pointer to `taggedUnion`/`varN`.
- **Recursion** (a record type reachable from its own field, directly or through a
  nested record/list) → detected by the `seen`-set walk; a *finite* recursive codec
  needs an explicit `Codec.recursive`-style fixpoint the derive cannot synthesise
  blindly, so the derive rejects with `IPE-T…` and points at the explicit form. It
  never loops and never emits a non-terminating codec.
- **A row-polymorphic / open-record witness** (`RowTail::Open`) → rejected: `auto`
  requires a *concrete, closed* record type so the field list is fully known. The
  rejection is expressed in `TyShape`/`RowOpen` terms (Kernel Row coordination), not a
  bespoke check.
- **The SEAL — a derived codec must never exit-0-then-cargo-fail.** This holds *by
  construction*, and that is the load-bearing property of the whole design: the derive
  emits **no bespoke Rust**. It rewrites `auto` into an ordinary `object/field/
  buildObject` expression that then flows through the identical inference → lower →
  emit path as a hand-written codec. If the assembled expression does not type-check
  (an underivable field), inference rejects it *at ipe time* with a clean diagnostic;
  if it type-checks, it lowers and emits exactly as the hand-written builder does, so
  `cargo build` succeeds for the same reason the hand-written builder's does. There is
  no acceptance path that produces Ipê-exit-0 with cargo-fail, because there is no emit
  the hand-written builder does not already exercise. An anti-drift test asserts, over
  a corpus of record shapes, that `Codec.auto blank` and the equivalent explicit
  builder produce **byte-identical emit** — the mechanical proof that the derive adds
  no new emit surface (and the round-trip law `fromJson (toJson x) == Ok x` over a
  fuzzed corpus is the correctness proof).

## Phased plan

Both phases sequence **after** the Kernel Row lands (hard dependency: the field
classifier binds to the consolidated `TyShape`/`BuiltinTag` vocabulary, not to the
`Builtins` cache it replaces). Each phase is independently landable and green under
the full CI-replica gate.

**First landing — the derive elaboration + `Codec a` surface + the JSON direction.**
Ship compiled-source `Ipe.Codec` (type, combinators, `object`/`field`/`buildObject`,
`toJson`/`fromJson`, the pure `taggedUnion`/`enum` builders) with no compiler
involvement; then add the canon-stage `auto` recogniser + the type-driven derive that
rewrites `auto`/`autoCamel`/`autoWith` into the builder chain. Scope the derive to
records of scalars / `Maybe` / `List` / nested records / nullary enums / `Money` /
`Decimal`, with `Secret` and data-ADT and recursion and open-record rejections. Prove
JSON round-trips (`fromJson (toJson x) == Ok x`) over a fuzzed record corpus, plus the
byte-identical-to-hand-written-builder anti-drift test. This phase delivers the whole
`auto` mechanism; the DB half is unused until the next landing. **JSON is deliberately
first** — it is the simpler target with no schema-migration confound.

**Second landing — the DB-mapping direction feeding `Ipe.Db.Store`.** Wire `shape
codec`'s `SRecord`/`ColType` into `Ipe.Db.Store` (issue 680): derive the column list,
types, nullability, and row read/write from the one codec; add the `Table`/`Store`
declaration surface for the DB-only facts (primary key, unique, index, ordering) the
type cannot express. Depends on the first landing (needs a `Codec`) and on
`Db.open`-style connectivity (issue 641). The dialect mapping is the Db layer's
existing parameterised-SQL path; this landing adds no new injection surface.

**Blocking-feature note.** The derive itself needs **no first-class functions and no
new row-polymorphism**: it emits *first-order* `object/field/buildObject` calls with
record accessors (`.field`) as getters, all already first-class canon forms, and it
*rejects* open records rather than deriving over them. The `Codec a` surface's `enc :
a -> Value` field is a function-typed record field — supported today for concrete
records via the existing function-value-in-record carrier (the `field`/`object`
builder already relies on it). If a future recursive-codec (`Codec.recursive`) member
is added, *that* would lean on first-class-function support (issue 665), but it is out
of this design's critical path — `auto` explicitly rejects recursion rather than
requiring the fixpoint. So neither phase blocks on issue 665 or on new row-poly work;
both block only on the Kernel Row.

## Affected issues

| Issue | Relationship | One-line annotation |
|---|---|---|
| 663 | **IMPLEMENTS** | This design *is* `Ipe.Codec`: the pure `Codec a` surface + the `Codec.auto` compile-time record derive (no runtime reflection), replacing Sky's tag+registry reflection model (see docs/architecture/tbd/codec-auto-derive-design.md). |
| 680 | **FEEDS (second landing)** | `Db.Store` consumes the derived codec's `Shape`/`ColType` for columns + `enc`/`dec` for row read/write — one codec drives JSON and the dialect-safe column map with no second shape. |
| 664 | **CONSUMES** | `Ipe.Analytics`' typed-payload / event-record persistence uses the `Codec.auto` derive for its event records instead of Sky's `trackEvent` reflective derive; same compile-time-derive model, no reflection kernel. |
| 641 | **COORDINATES** | `Db.open <driver> <dsn>` supplies the connection the second landing's `Db.Store` read/write runs against; the codec is dialect-agnostic, so `Db.open`'s driver choice selects the dialect mapping under the shared `ColType`. |
| 665 | **INDEPENDENT (of `auto`)** | First-class functions are NOT needed by the derive (it emits first-order `object/field` calls); only a future `Codec.recursive` fixpoint would need issue 665, and `auto` rejects recursion rather than requiring it. |
| 666 | **COORDINATES** | `Html.Unsafe.unsafeRaw` and the codec share the "safe default + rare marked escape" posture: a custom-format single column is the `autoWith` override, raw SQL is `Ipe.Db.Unsafe` — the codec makes hand-written SQL the rare escape, mirroring issue 666's raw-HTML escape. |
| (Kernel Row) | **DEPENDS ON** | The derive's field-classifier binds to the Row's consolidated `TyShape`/`BuiltinTag` type vocabulary and reuses its single `ipe_types` interpreter + `RowOpen` carrier; both phases sequence after the Row so the classifier never re-grows an "is-this-an-Int-field" predicate or touches the `Builtins` cache the Row dismantles. |
| (ADR 0029) | **EXTENDS** | `Ipe.Codec` ships as a compiled-source module; `auto` adds exactly one new residency — a compiled-source member whose body is synthesised at its call site from the argument's type — reconciled as a single canon-stage elaboration recogniser, no new kernel/anti-drift site. |
| (Unsafe convention) | **COORDINATES** | The codec's escape hatches (`autoWith` override, raw SQL) route to the `Ipe.<M>.Unsafe` convention; the safe derived path is the default that makes those escapes rare. |
| (reserved-type registry) | **REUSES** | `Secret` rejection + `Money`/`Decimal` handling read the existing `RESERVED_BUILTIN_TYPES` / `SecretOrSink` categories in `resolve.rs`; the derive maintains no parallel opaque-type list (single source of truth). |

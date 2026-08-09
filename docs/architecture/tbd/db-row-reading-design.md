# Reading typed values out of a DB row

Status: design proposal, no implementation yet. Every fenced block is
**illustrative of the proposed surface** — the signatures below are intended
shapes and names, not shipped API or verified commands. Tracker references
belong in the delivering pull request, not in this timeless doc.

## The problem: a row is untyped, so every read is a parse that can fail

A database row arrives as `Dict String String` — every column value is a string,
regardless of its declared SQL type. Reading a column as an `Int`, `Bool`, or
`Float` is therefore a **parse**, and a parse over external data can fail three
ways:

- the field is absent from the row,
- the value is present but not of the read type (`"abc"` read as `Int`),
- the value is present and numeric but outside the target range (`"1e30"` read
  as a 64-bit `Int`).

Today the raw per-field getters model none of these failures in their types.
`Ipe.Db.Unsafe` (EXISTS, `src/stdlib/Ipe/Db/Unsafe.ipe`) exposes a TOTAL family:

```elm
unsafeGetString : String -> Dict String String -> String
unsafeGetInt    : String -> Dict String String -> Int
unsafeGetBool   : String -> Dict String String -> Bool
unsafeGetField  : String -> Dict String String -> String
```

Each returns a bare value with a hidden default on failure. The runtime backing
(EXISTS, `db_get_int` / `db_get_string` / `db_get_bool` / `db_get_field` in
`src/runtime/rust/src/db.rs`) returns `0` for a non-parsing `Int`, `false` for
any non-truthy `Bool`, and `""` for a missing `String`.

This is a make-invalid-states-representable violation. A defaulted `0` is
indistinguishable, **at the type**, from a genuinely stored `0`. A caller that
uses the returned id to look the row back up silently operates on the wrong row
(or none). An interim runtime fix stopped a worse failure — an out-of-range
magnitude was being `as`-cast, which SATURATES `"1e30"` to `i64::MAX` and reports
it as a value — but the honest replacement (return `0` and set an `eprintln`
flag) is still MISU-violating: the flag is a side-channel the program cannot
observe or handle, and the caller still receives a fabricated `0`. The
range-check reasoning that the interim fix established is correct; only its
*outcome type* is wrong. This design replaces the lenient total getter with a
`Maybe`-returning getter as the end state.

## The canonical path: decode the row once at the boundary (EXISTS)

Parse-don't-validate says: turn the untyped row into a typed record **once**, at
the boundary, and let the failure be a typed value the caller must handle. The
decoder family already exists and is the recommended path.

`Db.Decode` (EXISTS — registered in `src/compiler/kernels/src/lib.rs`, runtime in
`src/runtime/rust/src/db.rs`) is a `DbDec`/`Decoder` combinator surface. A
`DbDec a` reads a `Dict String String` row and yields `Result Error a`; the
runner (`Db.queryDecode` and siblings) applies it to every row and surfaces the
first decode error as a `Task Error`, never a fabricated value.

Confirmed surface, by column type and by composition:

| Concern | Kernel (EXISTS) |
| --- | --- |
| scalar reads | `Db.Decode.string`, `.int`, `.float`, `.bool` |
| nullable / optional | `Db.Decode.nullable`, `.optional` |
| domain scalars | `Db.Decode.money`, `.bytes` |
| record composition | `Db.Decode.succeed`, `.map`, `.map2`, `.map3`, `.map4`, `.andThen`, `.required` |
| explicit failure | `Db.Decode.fail` |

`Db.Decode.int` (runtime `db_decode_int`, EXISTS) already does the honest thing:
a decimal source truncates toward zero, but an out-of-range magnitude is a typed
decode **error**, not a saturated value. This is exactly the range check the
interim getter fix reasoned about — the decoder was already fail-closed.

```elm
type alias User = { id : Int, name : String, active : Bool }

userDecoder : DbDec User
userDecoder =
    Db.Decode.succeed User
        |> Db.Decode.required "id"     Db.Decode.int
        |> Db.Decode.required "name"   Db.Decode.string
        |> Db.Decode.required "active" Db.Decode.bool
```

**Gaps.** The scalar reads cover `string / int / float / bool`. There is no
`Db.Decode.field` alias for "read a column as its raw string" — `Db.Decode.string`
already fills that role, so the getter family's `unsafeGetField` maps to
`Db.Decode.string` on the decoder path. The composition surface (`succeed` +
`required` + `mapN`) covers record assembly; no gap for the record case. This
design does not propose new decoder kernels; the decoder path is complete for the
migration below.

## The escape hatch, made MISU-honest: getters return `Maybe`

The decoder is the recommended path, but the raw getters have a legitimate role:
a one-off read that deliberately bypasses the schema/decoder. They stay behind
the `unsafe` namespace (importing it discloses the `unsafe` capability — see the
escape-convention design). What changes is the **return type**: no hidden
default, so the caller must handle absence/mismatch.

PROPOSED-NEW signatures for the whole family (illustrative):

```elm
unsafeGetString : String -> Dict String String -> Maybe String
unsafeGetInt    : String -> Dict String String -> Maybe Int
unsafeGetBool   : String -> Dict String String -> Maybe Bool
unsafeGetField  : String -> Dict String String -> Maybe String
```

`Nothing` means the read failed — field absent, value not of the type, or (for
`Int`) out of range. The three failure modes collapse to one `Nothing`; a caller
that needs to distinguish them uses a decoder, which carries a message. There is
no `unsafeGetFloat` today (EXISTS check: the getter family is String/Int/Bool/
Field only; `float` lives only on the decoder path as `Db.Decode.float`). If a
raw float getter is wanted for symmetry, it is added as
`unsafeGetFloat : String -> Dict String String -> Maybe Float` — otherwise the
float case is decoder-only, which is the recommended direction anyway.

Optional sugar, making any default **explicit at the call site** (illustrative):

```elm
unsafeGetIntOr : Int -> String -> Dict String String -> Int
unsafeGetIntOr fallback key row =
    unsafeGetInt key row |> Maybe.withDefault fallback
```

This is nothing but `Maybe.withDefault`; it exists only so a caller who genuinely
wants a default writes it where the reader can see it, instead of the compiler
fabricating one invisibly. It is optional — `... |> Maybe.withDefault 0` at the
call site is equally honest and needs no new member.

## Killing the implicit default and the side-channel

The hidden default and the `eprintln` flag both go away, because both are MISU
violations:

- A hidden default puts an unrepresentable-should-be-invalid state (a defaulted
  `0`) into a type (`Int`) that cannot record that it is defaulted. `Maybe Int`
  makes the invalid state (`Nothing`) representable and forces the caller to
  handle it — parse-don't-validate at the return type.
- An `eprintln` flag is a side-channel: the program's value-level control flow
  cannot observe it, so it cannot recover, retry, or report. A `Nothing` is
  in-band and total.

The out-of-range/non-integral reasoning from the interim saturation fix maps
cleanly onto `Nothing` in the getter and onto a typed decode error on the
decoder path — the runtime already computes it (`db_decode_int`'s checked
float-to-`i64`); the getter simply returns `None` where it currently returns the
defaulted `0`.

## Migration

Call sites use the getter as a bare value today (`Db.unsafeGetInt "x" row : Int`).
Under the new signature the site is a `Maybe Int` and must be resolved:

- **Mechanical, behaviour-preserving:** `Db.unsafeGetInt "x" row` becomes
  `Db.unsafeGetInt "x" row |> Maybe.withDefault 0` (and `""` for string,
  `False` for bool). This preserves today's defaulting but makes it visible.
- **Recommended:** migrate the read to a `DbDec` decode at the query boundary,
  deleting the per-field getter entirely for rows that have a known shape.

Footprint (EXISTS, measured): the getter family is used across the mirrored
example corpus and the DB golden fixtures — on the order of ~30 example modules
(the shop example alone accounts for most call sites, e.g. its cart, admin, and
orders pages) plus ~20 `tests/golden/db_*` fixtures and `src/stdlib/Ipe/Db/
Unsafe.ipe` itself. Every call site edited to `|> Maybe.withDefault …` (or a
decoder) re-blesses the affected golden bytes; golden re-bless is cheap and
automated and is not a cost factor in this decision.

Deprecation path for the old total signature: because the return type changes
(`Int` → `Maybe Int`), the old and new signatures cannot coexist under one name —
this is a breaking change, not an additive one. The old total getters are
**removed**, not soft-deprecated; a call site left unmigrated fails to type-check
(a `Maybe Int` where an `Int` is expected), which is the desired fail-closed
diagnostic. No silent behaviour drift is possible.

## Sequencing

Changing a getter's return type touches its **kernel row** — the type scheme in
`ipe_types::constrain` (`DB_GET_INT : String -> Dict String String -> Int`
becomes `… -> Maybe Int`), and correspondingly the surface signature in
`Ipe.Db.Unsafe`. The kernels crate is currently owned by the in-flight
first-class-function collection-carrier work; this change is **blocked behind
that landing** and starts afterward. The single-row descriptor that makes such a
scheme edit a one-line change is the subject of the kernel-row design; the
`unsafe`-namespace placement is the subject of the escape-convention design. See:

- [Kernel row descriptor](kernel-row-design.md)
- [The `Ipe.<Module>.Unsafe` escape convention](unsafe-escape-convention-design.md)
- [`Ipe.Codec` + `Ipe.Db.Store`](codec-and-store-design.md)
- [`Db.open` — external databases](db-open-external-design.md)

## Review gate

Changing the getter family's return type and any decoder-surface change is a
language-boundary and soundness change: it alters a typed contract that untyped
external data crosses. It requires **security-soundness-guardian** review before
merge — the review must confirm the `Nothing`/decode-error mapping is exhaustive
(absent / mismatch / out-of-range all fail closed), that no defaulting or
side-channel survives, and that the removed-total-signature diagnostic fails
closed rather than silently coercing.

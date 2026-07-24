# Additive Stdlib Features — Design

> Read-only design of the additive stdlib surfaces named in
> `docs/divergences-review.md` §6 (missing divergences) and the
> `Array`/`Bitwise`/`Tuple` gap from `docs/divergences-from-elm.md` R5,
> against the Elm-core catalog in `docs/architecture/elm-core-coverage.md`.
> Nothing here is implemented — this is the shape, the backing, the registry
> fit, the effort, and the roadmap slot for each.
>
> **Principle order (applied throughout):** security > correctness > soundness
> > efficiency > completeness > readability. Two governing rules: *parse,
> don't validate*; *make invalid states unrepresentable*.
>
> **Public-artifact voice.** Elm and Go are framed neutrally as *what differs +
> why*; neither is characterized as wrong. No upstream-contribution notes.

---

## 0. Grounding — how a surface reaches into the compiler

Every proposal below is written to fit the mechanisms that already exist in the
tree, so nothing needs a new subsystem:

- **Two ways to ship a binding.**
  1. **Pure Ipê source** — a `.ipe` module embedded via `include_str!` in
     `src/ipe-cli/src/stdlib.rs` (the 17 `Ipe.*` modules today), with an
     `exposing (…)` list. Bodies are recursive/`case`-based Ipê, or
     `Ffi.kernel "Name"` aliases whose HM signature is written in Ipê and whose
     call sites route to a kernel.
  2. **Kernel** — a variant of the closed `StdlibKernel` enum
     (`src/compiler/kernels/src/lib.rs`). Each variant's `decl()` returns
     `StdlibDecl { qualifier, name, arity, class, emit }`; the variant is listed
     in `StdlibKernel::ALL`; its type is built in `stdlib_scheme(k)`
     (`src/compiler/types/src/constrain.rs`) from interned `builtins`; a runtime
     symbol named by `emit` lives in `src/runtime/rust/src/*`.
- **The scheme is a `Ty` built from `builtins`.** `stdlib_scheme` uses closures
  `int()`, `float()`, `string()`, `bool_ty()`, `list(t)`, `maybe(t)`,
  `result(e,a)`, `dict(k,v)`, `set(a)`, `bytes()`, `char()`, `tuple2(a,b)`,
  `task(a)`, `dec(inner)`, `fun(a,b)`, `var(id)`, plus opaque nullary
  `Ty::Con { module:[], name: builtins.<x>, args:[] }`. A **new opaque type**
  (e.g. `Array`, `SqlFragment`) means one new interned `Symbol` field on
  `builtins` and one closure — the same pattern `db`/`sqlvalue`/`sqlfield`
  already follow.
- **Fail-closed invariants that constrain these designs.**
  - `StdlibKernel::ALL` has a canon-equality tripwire (`canon_equals_registry`)
    and a `stdlib_scheme_matches_legacy` parity tripwire — a new kernel must be
    wired in *both* tables or the build fails. This is the mechanism that keeps
    "ipe exits 0, cargo fails" from happening.
  - `decl().arity` must equal the arrow count of `stdlib_scheme`. Arity-0
    kernels currently miscompile (Active limitation #7) — **every design below
    avoids nullary kernels** and takes an explicit first argument.
  - `SkyError = String` at the runtime level (`Ipê/Core/Task.ipe` header):
    the typed `Error` is a stringly value under the hood today. The sub-domain
    error taxonomy (§7) is designed around that constraint.
- **Existing parse-don't-validate exemplars to reuse, not reinvent.**
  - `SqlIdent` (`db.rs:1100`) — a validated `[A-Za-z0-9_]` identifier newtype;
    an unvalidated name is unrepresentable past the boundary. §6 extends this
    from *identifiers* to *whole query fragments*.
  - `SqlValue` / `SqlField` ADTs (`Ipe.Db`) — already make typed NULL and
    column-omit unrepresentable-as-a-string. §6 composes with them.
  - `char_kernel.rs` already depends (non-optionally) on
    `unicode-general-category`; the runtime already carries `unicode-width`
    (behind the `tui` feature). The Unicode proposals (§1, §2) name their new
    crate precisely rather than assuming one is already vendored.

Each section states: **surface · lives where (kernel vs Ipê) · runtime backing
· registry/scheme fit · security/soundness · effort · roadmap slot · open
decisions.**

---

## 1. Grapheme API — `String.graphemes` (opt-in segmentation)

**Why separate from the default surface.** STR1 fixed the default `String`
semantics to Unicode **code points** (runes): `length` = `chars().count()`,
`slice`/`left`/`right` iterate `chars()`. That is the natural Go/Rust unit and
avoids UTF-16 surrogate splitting. It is *not* grapheme-correct: a ZWJ emoji
sequence (`👩‍👩‍👧`) or a base+combining pair (`e` + U+0301) is several code
points. Grapheme segmentation is a **different, heavier unit** with its own
Unicode-version-dependent table. Folding it into `length` would (a) silently
change every existing call site's numbers, (b) make the common case pay for the
rare one, and (c) couple `String` semantics to a UAX-29 table version. So this
is an **additive, explicitly-named surface** — the default stays rune-based;
the README says "rune-correct," and points to `graphemes` for cluster work.

**Surface (new pure-Ipê module `Ipe.String.Grapheme`, or a namespaced
extension of `Ipe.String`):**

| Function | Signature | Meaning |
|---|---|---|
| `graphemes` | `String -> List String` | split into extended grapheme clusters (UAX-29) |
| `graphemeLength` | `String -> Int` | cluster count (≤ `String.length`) |
| `graphemeSlice` | `Int -> Int -> String -> String` | slice on cluster boundaries, rune-clamped |
| `graphemeReverse` | `String -> String` | reverse without splitting clusters (fixes the emoji-reversal hazard `reverse` still has at cluster level) |

Naming with a `grapheme*` prefix keeps the code-point default the obvious one
and makes the segmentation opt-in legible at the call site.

**Lives where.** Kernels. Each takes an explicit `String` first arg (arity ≥ 1,
so no #7 exposure). `decl`: `d("String", "graphemes", 1, Pure, "string_graphemes")`,
etc. (or a `Grapheme` qualifier if a submodule is preferred — a qualifier is
just a string in `decl`).

**Runtime backing.** New dep **`unicode-segmentation`** (`Graphemes` iterator,
`graphemes(true)` = extended clusters). *Correction to a note in
`divergences-review.md` §6:* the runtime today vendors `unicode-width` (display
width, `tui` feature only) — **not** a segmentation crate. `graphemes` needs
`unicode-segmentation` added. Keep it behind a default-off `unicode-seg`
feature so non-string-heavy builds don't link the UAX-29 table; the emitted
project's Cargo.toml enables it when a `String.graphemes*` kernel is reached
(same reached-feature wiring `crypto`/`json` use).

**Registry/scheme fit.** `graphemes : fun(string(), list(string()))`;
`graphemeLength : fun(string(), int())`; `graphemeSlice :
fun(int(), fun(int(), fun(string(), string())))`. All from existing builtins —
no new interned type. Wire into `ALL` + both tripwire tables.

**Security / soundness.** Total by construction: clustering never panics; slice
indices clamp to `[0, graphemeLength]` (mirror the rune-clamp `string.rs`
already does). No allocation derived from untrusted size beyond the input
length. Pin the `unicode-segmentation` version and record the UAX/Unicode
version as a fixture note so a table bump that shifts a boundary is caught by
golden diff, not silently shipped.

**Effort.** Small (~4 kernels + 1 dep + goldens). **Roadmap slot: post-DONE**
(additive; does not block parity).

**Open decision.** Submodule `Ipe.String.Grapheme` vs `grapheme*`-prefixed
entries directly on `Ipe.String`. Prefix is fewer moving parts; submodule
reads cleaner. Lean prefix (matches the existing flat `String` surface).

---

## 2. Unicode normalization — `String.normalize`

**Why.** Ipê is full-Unicode on *casing* (B5, `ß→SS`) but code-point-**literal**
on *equality*: composed `"é"` (U+00E9) ≠ decomposed `"e"`+U+0301, so `==`,
`equalFold`, `isEmail`, and dict/set keying can disagree with a human's notion
of "same string." A normalizing form makes equality decidable the way
Swift/Python/Go's `x/text` do. This is additive — it does **not** silently
normalize `==` (that would change dictionary/set identity semantics
project-wide and break byte-round-trips); it is an explicit function callers
reach for at a boundary (parse-don't-validate: normalize once at ingest, store
the NFC form, compare downstream).

**Surface (parse-don't-validate at the type level):**

```elm
type NormalForm = NFC | NFD | NFKC | NFKD   -- closed ADT, invalid form unrepresentable

normalize : NormalForm -> String -> String
```

A closed `NormalForm` ADT beats a `String`/`Int` mode argument — the four forms
are the whole domain, and an illegal mode cannot be constructed. Optional
convenience aliases `normalizeNfc : String -> String` etc. keep the common case
one call.

**Lives where.** The `NormalForm` type + aliases in pure Ipê
(`Ipe.String` or a `Ipe.Unicode` module); `normalize` as a kernel
taking `(NormalForm, String)` — arity 2, explicit args.

**Runtime backing.** New dep **`unicode-normalization`** (`.nfc()`/`.nfd()`/
`.nfkc()`/`.nfkd()` iterators). Same default-off feature-gate treatment as §1.
Kernel matches on the `NormalForm` discriminant → picks the iterator.

**Registry/scheme fit.** `NormalForm` is a user-style ADT emitted like any Ipê
enum (the discriminant reaches the kernel as the runtime enum repr). `normalize`
schemes as `fun(normalform(), fun(string(), string()))`. `NormalForm` needs the
same treatment as any Ipê-declared ADT the kernel pattern-matches on — either
declared in Ipê source (preferred; then it is an ordinary type the kernel reads
by tag) or, if kept opaque, one interned builtin. Prefer Ipê-source declaration
so exhaustiveness + `ipe doc` come for free.

**Security / soundness.** Total (normalization never fails). NFKC/NFKD are
**compatibility** decompositions — lossy (`①`→`1`, ligatures split); document
that they are for matching/search, not for storage of user-facing text. Security
angle: normalizing an identifier/username at ingest before an authz or
uniqueness check closes a homoglyph/confusable *duplication* vector (two
visually-distinct byte strings that a naive `==` treats as different accounts) —
but NFKC is **not** a confusables-skeleton and must not be sold as anti-spoofing;
scope the claim to "canonical equality," not "homoglyph defense."

**Effort.** Small (1 kernel + 1 ADT + 1 dep). **Roadmap slot: post-DONE.**

**Open decision.** Whether `equalFold`/`isEmail` gain an internal
NFC-normalize step (behavior change vs. today) or stay literal with `normalize`
left to the caller. Recommendation: keep them literal (no silent behavior
change), expose `normalize` + document the compose pattern.

---

## 3. `Array` module — immutable indexed vector (genuine Elm-core gap, R5)

**Why.** `elm/core`'s `Array` gives O(log n) indexed access; Ipê ships only
`List` (O(n) indexing, O(n) non-tail ops per Active limitation #8). On a
native/data-workload backend this is a real capability gap, not a clean
omission — `docs/divergences-review.md` §6.3 flags it. Elm-compatible surface,
so existing Elm code and muscle memory port directly.

**Surface (Elm-parity, opaque type):**

```elm
type Array a   -- opaque

empty      : Array a
initialize : Int -> (Int -> a) -> Array a
repeat     : Int -> a -> Array a
fromList   : List a -> Array a
isEmpty    : Array a -> Bool
length     : Array a -> Int
get        : Int -> Array a -> Maybe a          -- fallible-pure: out-of-range → Nothing
set        : Int -> a -> Array a -> Array a      -- out-of-range → unchanged (Elm semantics)
push       : a -> Array a -> Array a
append     : Array a -> Array a -> Array a
slice      : Int -> Int -> Array a -> Array a     -- negative indices from end (Elm semantics)
toList     : Array a -> List a
toIndexedList : Array a -> List (Int, a)
map, indexedMap, filter, foldl, foldr            -- as Elm
```

The **soundness win is in `get`**: Elm/Ipê type it `Int -> Array a -> Maybe a`,
so out-of-range indexing is a `Nothing`, never a panic — parse-don't-validate at
the access site. No `unwrap`, no raw index reaches the runtime.

**Lives where.** Opaque `Array` type (one interned `builtins.array` symbol,
`Ty::Con { name: array, args: [elem] }`); operations as kernels (each takes an
explicit arg — no nullary except `empty`, which is the one arity-0 case). **`empty`
handling:** because arity-0 kernels miscompile (#7), model `empty` the way
`Dict.empty`/`Set.empty` already work (their non-function type lets them stay
bare — verify `Array a` resolves the same way; if not, ship `Array.empty` via
the same mechanism `Dict.empty` uses, or as a Ipê-source `empty = fromList []`).

**Runtime backing (OPEN — see below).** Candidates:
- **`im::Vector`** (RRB-tree persistent vector) — O(log n) get/set/push/split,
  structural sharing, mature. Adds `im` (+ `bitmaps`, `sized-chunks`).
- **`rpds::Vector`** — persistent, no_std-friendly, lighter dep tree, also RRB.
- **Custom `Arc<Vec<T>>` copy-on-write** — trivial, but `set`/`push` are O(n)
  (defeats the whole point); only acceptable as a placeholder.

**Registry/scheme fit.** One new `builtins.array` interned symbol + one closure
`array = |t| Ty::Con { name: array, args: vec![t] }`. Schemes: `get :
fun(int(), fun(array(a), maybe(a)))`; `push : fun(a, fun(array(a), array(a)))`;
`foldl : fun(fun(a, fun(b, b)), fun(b, fun(array(a), b)))`; etc. ~18 kernels
into `ALL` + both tripwires. Type-param handling mirrors `List`'s.

**Security / soundness.** `get`→`Maybe` = no index panic. `slice` clamps like
Elm. `initialize`/`repeat` allocate from an `Int` size — **guard against
negative and oversized `n`** (clamp negative to 0; cap or fail on sizes that
would exhaust memory, mirroring the DoS bounds `decimal`/`regex` already apply).
Persistent structure = no aliasing/mutation UB.

**Effort.** Medium (new opaque type across constrain/lower/emit + ~18 kernels +
a dep + Elm-parity goldens). **Roadmap slot: post-DONE** (additive; the
`elm-core-coverage` "whole modules absent" item).

**Open decisions.**
1. **Backing crate: `im` vs `rpds` vs custom RRB.** Recommendation: `rpds` for
   the lighter dependency footprint unless a bench shows `im` materially faster
   on the target workloads. **Genuinely open — needs a bench + dep-audit call.**
2. Whether `Array` is worth it before a `List.sort`/`filterMap` pass (both are
   also missing per `elm-core-coverage` §b and cheaper).

---

## 4. `Bitwise` module — integer bit ops (Elm-parity, pure)

**Why.** Crypto, protocol/framing, flags, and hashing code want bit ops; Ipê
has none (`elm-core-coverage` Bitwise = 0/7). Small, pure, Elm-parity.

**Surface (Elm-identical):**

| Function | Signature |
|---|---|
| `and` | `Int -> Int -> Int` |
| `or` | `Int -> Int -> Int` |
| `xor` | `Int -> Int -> Int` |
| `complement` | `Int -> Int` |
| `shiftLeftBy` | `Int -> Int -> Int` |
| `shiftRightBy` | `Int -> Int -> Int` (arithmetic / sign-extending) |
| `shiftRightZfBy` | `Int -> Int -> Int` (logical / zero-fill) |

**Lives where.** Kernels under a `Bitwise` qualifier, arity 1–2 (no nullary).
`d("Bitwise", "and", 2, Pure, "bitwise_and")`, etc.

**Runtime backing.** Plain Rust on `i64` (the Int repr). No dep. **The one
subtlety that must be pinned to Elm semantics** and covered by goldens:
- Elm runs bit ops on **32-bit** values (asm.js/JS `| 0` semantics):
  `shiftLeftBy` masks the shift by 31, `complement` is 32-bit, `shiftRightZfBy`
  is a 32-bit logical shift. Ipê `Int` is **64-bit** (`i64`). **This is a
  genuine semantic fork** and must be a *decided, recorded* divergence, not an
  accident:
  - **Option A (Go-parity, recommended):** 64-bit ops matching Go's
    `&`/`|`/`^`/`&^`/`<<`/`>>` on `int64`; `shiftRightBy` = arithmetic,
    `shiftRightZfBy` = cast-to-`u64`-shift-back. File as a numbered
    `oracle_divergence` ("Ipê Bitwise is 64-bit / Go `int64`-parity; Elm is
    32-bit") — same class as §8.
  - **Option B:** replicate Elm's 32-bit masking for source-portability.
  Recommendation: **A** — it matches the backend's own `Int` and Go; document
  the width explicitly so no one assumes Elm's 32-bit wraparound.

**Security / soundness.** Total; no panic. **Shift-amount UB guard:** Rust
`i64 << n` panics (debug) / is UB-adjacent for `n ≥ 64`. Mask the shift amount
(`n & 63`) or saturate — **decide and pin** (Go masks by `& 63` for `int64`;
match that). Negative shift counts: define (Go treats the count as unsigned;
mask handles it). This guard is mandatory — a raw `<<` on an untrusted `n` is a
panic vector.

**Effort.** Small (7 pure kernels, no dep). **Roadmap slot: post-DONE.**

**Open decision.** 32-bit-Elm vs 64-bit-Go semantics (recommend 64-bit-Go,
recorded as a numbered divergence).

---

## 5. `Tuple` module — pair helpers (Elm-parity, pure)

**Why.** `first`/`second` exist only as `Basics.fst`/`snd`; `pair`/`mapFirst`/
`mapSecond`/`mapBoth` are absent (`elm-core-coverage` Tuple = 0 ✓). Trivial,
pure, closes the module for grep-parity with Elm.

**Surface (Elm-identical):**

| Function | Signature |
|---|---|
| `pair` | `a -> b -> (a, b)` |
| `first` | `(a, b) -> a` |
| `second` | `(a, b) -> b` |
| `mapFirst` | `(a -> x) -> (a, b) -> (x, b)` |
| `mapSecond` | `(b -> y) -> (a, b) -> (a, y)` |
| `mapBoth` | `(a -> x) -> (b -> y) -> (a, b) -> (x, y)` |

**Lives where.** **Pure Ipê source** — no kernel needed. All six are one-liners
over tuple patterns:

```elm
pair a b = (a, b)
first (a, _) = a
second (_, b) = b
mapFirst f (a, b) = (f a, b)
mapSecond g (a, b) = (a, g b)
mapBoth f g (a, b) = (f a, g b)
```

`first`/`second` can alias `Basics.fst`/`snd` (keep both names — `Tuple.first`
for Elm-portability, `Basics.fst`/`snd` for the Ipê prelude, matching the
`ToString.*` discoverability pattern R2). Uses `Ty::Tuple` (already in
`stdlib_scheme` via `tuple2`).

**Registry/scheme fit.** None — pure Ipê, embedded in `stdlib.rs` `include_str!`
like `Basics.ipe`. Zero kernel-registry churn. Relies only on the existing
tuple-pattern support.

**Security / soundness.** Total, pure, no allocation surprises. Nothing to
guard.

**Effort.** Trivial (one ~10-line `.ipe` file). **Roadmap slot: post-DONE**
(bundle with the `List.sort`/`filterMap` pure-Ipê gap-fill pass).

**Open decision.** None. (Only: 2-tuple-only, matching Elm — no 3-tuple
helpers; Ipê tuples beyond 2 are rare.)

---

## 6. `SqlFragment` — parameterized-query newtype (HIGHEST security value)

**Why (security-first).** `Ipe.Db` today has two typed, injection-safe write
paths (`SqlValue` params, `updateFields`/`insertFields` with identifier
validation via `SqlIdent`), **and** a raw escape hatch:

```elm
unsafeFindWhere : Db -> String -> String -> List a -> Task Error (List (Dict String String))
--                                    ^^^^^^ raw WHERE clause as a String
findByConditions : Db -> String -> Dict String String -> Task Error (...)  -- equality-only
```

`unsafeFindWhere`'s WHERE clause is a **raw `String`** — the one place where
string-concatenated SQL is *representable*, so `"age > " ++ userInput` compiles
and injects. `findByConditions` is safe but equality-only (no `>`, `LIKE`,
`IN`, `OR`). The gap: expressing a **complex-but-parameterized** predicate
without dropping to a raw string. `SqlFragment` closes it by making
raw-string-as-query **unrepresentable** — you cannot build a fragment except
through constructors that keep SQL text and bound values structurally separate.
This is *parse, don't validate* applied at the query boundary and *make invalid
states unrepresentable* at its strongest: SQL injection becomes a type error.

**Surface (opaque type — no `String -> SqlFragment` back door):**

```elm
type SqlFragment   -- opaque; carries (sql_text_with_placeholders, ordered_bound_values)

-- Leaves — the ONLY ways to introduce a value; each emits a "?" placeholder
-- and pushes the value into the bound-parameter list. There is deliberately
-- NO `raw : String -> SqlFragment` constructor.
column   : String -> SqlFragment          -- a validated column reference (SqlIdent-gated)
param    : SqlValue -> SqlFragment          -- a bound value → "?" + push (reuses §the SqlValue ADT)
int      : Int -> SqlFragment               -- sugar for param (SqlInt …)
string   : String -> SqlFragment            -- sugar for param (SqlString …) — bound, NOT interpolated
lit      : String -> SqlFragment            -- a whitelisted SQL keyword/operator token, keyword-gated

-- Combinators — build predicates compositionally
eq, ne, gt, lt, gte, lte : SqlFragment -> SqlFragment -> SqlFragment
and_, or_ : SqlFragment -> SqlFragment -> SqlFragment
not_      : SqlFragment -> SqlFragment
inList    : SqlFragment -> List SqlValue -> SqlFragment   -- col IN (?, ?, …)
like      : SqlFragment -> String -> SqlFragment          -- col LIKE ? (pattern is a bound param)
group     : SqlFragment -> SqlFragment                    -- ( … )

-- Consumers — the query builders that accept a fragment where a raw WHERE used to go
findWhere : Db -> String -> SqlFragment -> Task Error (List (Dict String String))
deleteWhere : Db -> String -> SqlFragment -> Task Error Int
```

Key property: **`column`/`lit` are the only text-producing constructors, and
both are gated** — `column` through the existing `SqlIdent` `[A-Za-z0-9_]`
parse (reused verbatim), `lit` through a **closed whitelist** of operator/keyword
tokens (`=`, `<`, `>`, `AND`, `OR`, `IN`, `LIKE`, `IS NULL`, `ASC`, `DESC`, …).
Every *value* enters through `param`/`int`/`string`/`inList`/`like`, which emit
a `?` and bind — **user data never becomes SQL text**. There is no
`raw : String -> SqlFragment`; that omission is the whole security guarantee.

**Lives where.** Opaque `SqlFragment` type (one interned `builtins.sqlfragment`,
next to the existing `sqlvalue`/`sqlfield`); constructors + combinators as
kernels (all arity ≥ 1); consumers as `Ffi.kernel` over the existing `Db_*`
runtime. `SqlValue` is reused as-is — `param` takes a `SqlValue`, so the typed
NULL / Money / Time fidelity already built carries straight through.

**Runtime backing.** A `SqlFragment` runtime value is
`struct SqlFragment { sql: String, binds: Vec<SqlValue> }` (text with `?`
placeholders + ordered binds). Combinators concatenate `sql` and append `binds`;
the consumer hands `(SELECT * FROM {SqlIdent} WHERE {frag.sql}, frag.binds)` to
the **same** sqlx parameterized-execution path `updateFields`/`insertFields`
already use (the total `SqlParam→query` binder at `db.rs`). No new SQL-execution
surface — only a new *safe constructor* feeding the existing one.

**Registry/scheme fit.** One new interned `builtins.sqlfragment` + closure
`sqlfrag = || Ty::Con { name: sqlfragment, args: [] }`. Schemes:
`param : fun(sqlvalue(), sqlfrag())`; `eq : fun(sqlfrag(), fun(sqlfrag(),
sqlfrag()))`; `findWhere : fun(db(), fun(string(), fun(sqlfrag(),
task(list(dict(string(), string()))))))`. ~18 kernels into `ALL` + both
tripwires.

**Security / soundness.** This is the point of the feature:
- **Injection-by-construction impossible.** No constructor turns untrusted
  `String` into SQL text. `column`/`lit` are whitelist-gated; everything else
  binds. The escape hatch (`unsafeFindWhere`) can then be *documented as legacy*
  and, eventually, `#[deny]`-flagged in favor of `findWhere`.
- **Placeholder/bind ordering is structural**, so a mismatch (N placeholders,
  M binds) is unrepresentable — the fragment carries both together; there is no
  way to desync them.
- **`inList` with N values** emits exactly N placeholders from the list length —
  no manual `?,?,?` string-building, no off-by-one.
- Reuses `SqlIdent` (already audited) and `SqlValue` (already typed-NULL-safe),
  so no new trust boundary is introduced — only an existing raw one is closed.

**Effort.** Medium (new opaque type + ~18 kernels + runtime fragment struct;
but the execution path and `SqlValue` are reused). **Roadmap slot: PRE-PUSH
candidate** — unlike the rest of this doc (post-DONE additive), this is a
security win that closes a representable-injection hole and could land earlier.
Sequence it after the `SqlValue`/`updateFields` work it builds on.

**Open decisions.**
1. Scope of the `lit` whitelist (which operators/keywords) — start minimal
   (comparison + `AND`/`OR`/`NOT`/`IN`/`LIKE`/`IS NULL`/`ASC`/`DESC`) and grow
   on demand; never add a raw passthrough.
2. Whether to also offer `orderBy`/`limit` fragments (bound `LIMIT ?`) in v1 or
   defer.
3. Whether `unsafeFindWhere` gets a deprecation lint once `findWhere` ships
   (recommend: yes, but keep it — some dynamic reporting genuinely needs it,
   gated behind its `unsafe` name).

---

## 7. Sub-domain error taxonomy — richer typed `Error`

**Why.** ER1 mandates typed `Error` over stringly errors — but the current
`Error` is effectively opaque (`SkyError = String` at runtime; the Ipê surface
has `Error.unexpected`, a classifier, `Error.toString`). Callers can't
`case`-match *why* something failed (parse vs decode vs DB vs HTTP vs auth), so
recovery logic re-inspects strings — the exact anti-pattern parse-don't-validate
forbids. A closed sub-domain ADT lets `update`/`onError` branch on failure
*class* by pattern-match, pushing make-invalid-states-unrepresentable into the
error channel itself.

**Surface (closed ADT, backward-compatible):**

```elm
type ErrorKind
    = ParseError                 -- malformed input at a boundary
    | DecodeError                -- JSON/CSV/Config decode failure
    | DbError                    -- persistence layer
    | HttpError Int              -- carries status where known
    | AuthError                  -- authn/authz denial
    | NotFound
    | Timeout
    | Unexpected                 -- the current catch-all (default)

-- Error stays the canonical type; it GAINS a kind + message, keeping toString.
kind      : Error -> ErrorKind          -- classify (total; legacy strings → Unexpected)
withKind  : ErrorKind -> Error -> Error  -- tag at construction
message   : Error -> String              -- the human/log string (today's toString body)

-- Constructors per kind (parse-don't-validate: fail with the right class)
parseError  : String -> Error
decodeError : String -> Error
dbError     : String -> Error
httpError   : Int -> String -> Error
authError   : String -> Error
notFound    : String -> Error
timeout     : String -> Error
unexpected  : String -> Error            -- exists today; stays the default
```

**Compat with existing `Error`.** The single `Error` type stays (Task's error
slot is still `Error` — R4 mandate intact). Internally `Error` becomes a
`{ kind: ErrorKind, message: String }` (or, while `SkyError = String`, a
tagged-string encoding `"<kind>\u{1}<message>"` that `kind` parses back — an
interim that avoids widening the runtime error repr before it's ready). Legacy
`Error.unexpected`/`toString` keep working: `unexpected` = `withKind Unexpected`,
`toString` = `message`. **No public signature changes**, so the ER1
non-regression rule and every existing call site are untouched; callers *opt in*
to `case kind e of …`.

**Lives where.** `ErrorKind` ADT + constructors in pure Ipê (`Ipe.Error`,
newly surfaced); `kind`/`withKind`/`message` as kernels if the runtime repr is
involved, or pure Ipê if the tagged-string interim is used. Prefer pure-Ipê
tagged-string interim first (zero runtime-repr change), migrate to a structured
`Error` runtime value when `SkyError` widens.

**Registry/scheme fit.** `ErrorKind` is an ordinary Ipê ADT. `kind :
fun(error(), errorkind())`; `httpError : fun(int(), fun(string(), error()))`.
Reuses `builtins.error`; adds `builtins.errorkind` only if `ErrorKind` is
opaque (prefer Ipê-declared → no new builtin).

**Security / soundness.** Closed ADT = exhaustive `case` (no `_ -> …` needed
per project walker rules). **The security win:** an operator can branch
`AuthError` → generic 403 without leaking internals, `DbError` → correlation-id
log + generic 500 — the two-level error pattern (ER3) becomes type-directed
instead of string-sniffed. No secret stringification: `message` is the only
text surface and constructors control it. Classifier is total (unknown/legacy →
`Unexpected`).

**Effort.** Small–medium (ADT + ~10 constructors/accessors; interim needs no
runtime change). **Roadmap slot: post-DONE** (additive; sequence with the
`Error` runtime-repr widening if/when that happens).

**Open decisions.**
1. Tagged-string interim vs. widen `SkyError` to a struct now. Recommend interim
   first (no runtime churn, no risk to the byte-for-byte Task path).
2. Whether `HttpError`/`DbError` carry structured payloads (status is proposed
   for HTTP; keep others message-only in v1).
3. Whether stdlib effect kernels (`Db.*`, `Http.*`, `Auth.*`) are updated to tag
   their failures with the right kind (high value, but a broad touch — stage
   after the ADT lands so it's opt-in per module).

---

## 8. Decimal division-precision — pin as a numbered `oracle_divergence`

**Why.** The 16-dp division-precision boundary between `rust_decimal`
(`checked_div`, ~28 significant digits) and Go `shopspring` (`DivisionPrecision
= 16`) is documented in AGENTS.md Agent-learnings and the ledger notes, **but is
not filed as a numbered divergence** with a regression that pins it. Per the
no-deferral principle and `docs/architecture/divergence-policy.md` (which
records divergences via `oracle_divergence = true` + a `divergence_reason`), an
un-pinned numeric boundary is a latent surprise: a future `rust_decimal` bump or
a new division-bearing kernel could shift digit counts and no test would catch
the drift from Go.

**What to pin.** For non-terminating divisions (`1/3`, `Money.getRate`
auto-inverse, any non-power-of-10 denominator), Ipê rounds the quotient to
**16 decimal places** (half-away-from-zero, matching shopspring's
`DivisionPrecision`) to hold Go byte-parity. Exact fractions
(money-scale, powers-of-10) are already bit-identical and unaffected.

**Where it lives.** `decimal.rs` division kernel(s) — apply
`.round_dp(16, MidpointAwayFromZero)` after `checked_div` for the non-exact
case. Cross-reference the two-rounding-mode learning already recorded (banker's
`RoundBank`/`MidpointNearestEven` for `Decimal.round`; away-from-zero for
`StringFixed`/`formatWith`) — division uses the shopspring `DivisionPrecision`
path, i.e. away-from-zero at 16 dp.

**Registry/scheme fit.** No surface change — this is a runtime-semantics pin on
an existing kernel, plus:
1. A **numbered `oracle_divergence`** entry (next free number in the B-series /
   the divergence ledger) with `divergence_reason = "sanctioned: Go shopspring
   DivisionPrecision=16 parity for non-terminating decimal division"`.
2. A **regression fixture** in the Go-oracle corpus: `1/3`, `2/3`, `10/3`, a
   `Money.getRate` inverse, asserting the 16-dp result byte-for-byte vs the Go
   oracle — the discovery artifact that makes the boundary non-silent.

**Security / soundness.** No security surface; correctness/parity only. Division
already guards divide-by-zero (checked, `Err`). Pinning removes a silent
cross-backend divergence class on the money path — the highest-value
correctness domain.

**Effort.** Trivial (one `round_dp` call already characterized + one fixture +
one ledger line). **Roadmap slot: near-term** — it's a *recording* task, not a
feature; do it whenever the divergence ledger is next touched, independent of
DONE.

**Open decision.** None — the behavior is already known; this only files it.

---

## 9. Summary table

| # | Feature | Kernel vs Ipê | New dep | New builtin type | Effort | Slot | Security-relevant |
|---|---|---|---|---|---|---|---|
| 1 | `String.graphemes` | Kernel | `unicode-segmentation` | no | S | post-DONE | indirect (no panic) |
| 2 | `String.normalize` | Kernel + `NormalForm` ADT | `unicode-normalization` | no (Ipê ADT) | S | post-DONE | scoped (canonical eq, not anti-spoof) |
| 3 | `Array` | Kernel + opaque type | `rpds`/`im` (OPEN) | `Array` | M | post-DONE | `get→Maybe` (no index panic) |
| 4 | `Bitwise` | Kernel | none | no | S | post-DONE | shift-UB guard mandatory |
| 5 | `Tuple` | **pure Ipê** | none | no | Trivial | post-DONE | no |
| 6 | **`SqlFragment`** | Kernel + opaque type | none | `SqlFragment` | M | **PRE-PUSH** | **YES — closes SQL-injection hole** |
| 7 | Error taxonomy | Ipê ADT (+maybe kernels) | none | maybe `ErrorKind` | S–M | post-DONE | YES — typed authz/failure branching |
| 8 | Decimal 16-dp pin | runtime pin + ledger | none | no | Trivial | near-term | no (parity) |

---

## 10. Cross-cutting notes

- **Every design avoids nullary kernels** (Active limitation #7). `Array.empty`
  is the only arity-0 case; model it on `Dict.empty`/`Set.empty` (non-function
  types stay bare) — verify that path resolves for a parametric `Array a` before
  committing, else route through a Ipê-source `empty = fromList []`.
- **Every new kernel wires into three places or the build fails:** the
  `StdlibKernel` enum + `decl()` arm, `StdlibKernel::ALL`, and `stdlib_scheme`
  — the `canon_equals_registry` and `stdlib_scheme_matches_legacy` tripwires
  enforce this mechanically (the fail-closed property A4).
- **New deps are default-off feature-gated** (`unicode-segmentation`,
  `unicode-normalization`, `rpds`/`im`) and enabled by the emitted project only
  when a reaching kernel is used — the same reached-feature wiring `crypto`/
  `json`/`tui` already use, so a bit-manipulation-free program links none of
  the Unicode tables.
- **Open decisions to resolve before implementation:** (a) `Array` backing crate
  (`rpds` vs `im` vs custom RRB) — needs a bench + dep-audit; (b) `Bitwise`
  32-bit-Elm vs 64-bit-Go width (recommend 64-bit-Go, filed as a numbered
  divergence); (c) grapheme submodule vs prefix; (d) error taxonomy tagged-string
  interim vs immediate `SkyError` struct-widening; (e) `SqlFragment` `lit`
  whitelist scope.
- **Security-relevant items: `SqlFragment` (§6) is the headline** — it makes SQL
  injection a type error by removing the raw-`String`→query representable path.
  The error taxonomy (§7) is secondary security value (type-directed authz/failure
  branching without string-sniffing or secret leakage). §2 has a *scoped* security
  note (canonical equality closes a duplication vector but is explicitly **not**
  homoglyph/confusables defense). The rest are correctness/soundness/efficiency.
</content>
</invoke>

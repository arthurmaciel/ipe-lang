# Full Float Set/Dict keys (lifting IPE-L0117) + locale-correct case mapping (D.3)

> Backlog item D.3 (Longer-horizon): "Full floating-point Set/Dict keys
> (ordered-float) + locale-correct case mapping. Lifts IPE-L0117."
> Spec+plan written 2026-07-10. Two independent sub-designs share this
> doc because the backlog row couples them; they are separately
> landable. Design-only; no code has changed.
>
> **One-line decisions:** (a) key-position `Float` lowers to
> `ordered_float::OrderedFloat<f64>` (total order, NaN == NaN, NaN
> sorts greatest), wrapped/unwrapped at the collection boundary in
> emitted code, retiring IPE-L0117; (b) default `toUpper`/`toLower`/
> `casefold` stay locale-independent; locale-aware mapping ships as a
> new explicit-locale surface (`Locale` opaque type +
> `String.toUpperIn/toLowerIn : Locale -> String -> String`) backed by
> ICU4X `icu_casemap`, feature-gated so programs that never use it pay
> nothing.

## Part A — Float keys in Set/Dict

### Problem statement

Ipê's `Float` is `comparable`, so the type checker accepts `Set Float`
and `Dict Float v` — but the Rust backend rejects both at lowering with
IPE-L0117, because its backings have trait bounds `f64` cannot meet:

- Diagnostic: `src/compiler/diagnostics/src/code.rs:194-195` ("Float is
  not a valid Set element or Dict key on the Rust backend"); explain
  page `src/compiler/diagnostics/explain/IPE-L0117.md`.
- Gates: `src/compiler/lower/src/lower.rs:4840-4852` (Dict annotation
  path), `4854-4864` (Set), the parallel `Decoder` path, and the
  post-inference `reject_float_keyed_collection` at `5919-5938`.
- Backings: `Dict` = `HashMap<K, V>` with `K: Hash + Eq`
  (`src/runtime/rust/src/dict.rs:15,24`; ordered reads sort with
  `K: Ord` at 40–60); `Set` = `BTreeSet<A>` with `A: Ord`
  (`src/runtime/rust/src/set.rs:20-70`). `f64` implements none of
  `Ord`/`Eq`/`Hash` (NaN breaks all three).
- Ledger: divergence **A17** (`docs/divergences-from-sky.md:551-566`)
  records the fail-closed gate and names "total-order Float set/dict
  (e.g. via an ordered-float wrapper)" as the tracked enhancement.

The gate was the right fail-closed call (soundness > completeness), but
it is a completeness hole against both the type system's own promise
(`comparable` includes Float) and both references: the Go runtime
accepts Float keys (Set stringifies via `fmt.Sprintf("%v", …)`,
`upstream:runtime-go/rt/stdlib_extra.go:32-82`; typed Dict maps use Go's
IEEE comparison where a NaN key is silently unretrievable), and Elm
accepts them (with famously undefined NaN-key behaviour in its AVL
Dict). Both references are *broken* at NaN; we can be complete AND
sound.

### Decision

**A1 — Representation.** Key/element-position `Float` lowers to
`ordered_float::OrderedFloat<f64>` (crate `ordered-float`, new runtime
dependency — small, no-unsafe-by-default, widely audited; NOT currently
in `Cargo.lock`, verified). It provides `Ord + Eq + Hash` with
`NaN == NaN` and NaN ordered greatest — a total order, so `BTreeSet`
and `HashMap` invariants hold for every input including NaN, -0.0/0.0
(which compare equal, matching IEEE and Ipê `==`), and infinities.

**A2 — Boundary discipline.** `OrderedFloat` never escapes the
collection: emitted code wraps at construction/insert/lookup call sites
and unwraps (`.into_inner()`) in everything the program reads back
(`keys`, `toList`, `foldl` key argument, …). The runtime dict/set
functions stay generic exactly as they are — the *emitter* picks
`K = OrderedFloat<f64>` when the settled key type is Float and inserts
the conversions, the same type-directed pattern the backend already
uses for coercions. Prefer `From`/`Into` impls (`OrderedFloat` ships
them) over any new runtime helper.

**A3 — Semantics (normative, and the divergence entries).**

| Case | Ipê (this design) | Go reference | Elm |
|---|---|---|---|
| `Dict.get NaN` after `insert NaN v` | `Just v` | typed map: `Nothing`-equivalent (NaN ≠ NaN); Set path: stringified `"NaN"` dedup | unretrievable (broken invariants) |
| NaN in sort order (`keys`/`toList`) | deterministic, greatest | unspecified (`rt.cmp` leaves NaN incomparable, `rt.go:2725-2748`) | unspecified |
| `-0.0` vs `0.0` as keys | same key | same key (Go `==`) | same key |

Both NaN rows are **sanctioned divergences** (Ipê strictly more
correct): add them to `docs/divergences-from-sky.md` superseding A17's
restriction text, and to `docs/divergences-from-elm.md`. Programs not
using NaN keys are behaviour-identical to both references.

**A4 — Retire IPE-L0117.** Remove the three lowering gates and the
`Feature::FloatKeyedCollection` plumbing; mark the diagnostic code
retired following the IPE-L0105 precedent (code stays reserved, explain
page rewritten to "retired — Float keys are fully supported since
<version>; historical restriction was …", so old links keep teaching).
Update `IPE-L0117.md`'s workaround text accordingly.

**Scope note:** only leaf `Float` keys. Composite comparable keys
(tuples containing Float) follow the same wrapper at leaf position
automatically *if/when* tuple keys are supported at all — that is a
separate completeness item; do not widen it here.

**Alternatives rejected:** (1) keep the gate — a permanent completeness
hole vs the language's own `comparable`; (2) Go-style key
stringification — lossy, unsound representation and inherits Go's NaN
dedup-by-string quirk; (3) hand-rolled `TotalF64` newtype — re-implements
a solved problem, more unaudited `Ord`/`Hash` code we own; (4) IEEE-754
`totalOrder` bit-pattern ordering — distinguishes -0.0 from 0.0 and NaN
payloads, diverging from Ipê `==` on keys.

### Implementation plan (Part A)

1. Add `ordered-float` to `runtime/Cargo.toml` (and the emitted-project
   Cargo template the backend generates).
2. Emitter: map key-position `IrType::Float` → `OrderedFloat<f64>` in
   the Dict/Set (and their `Decoder`) type-rendering paths; insert
   wrap/unwrap conversions at kernel-call boundaries (audit every
   dict/set kernel emission site; `dict_keys`/`dict_to_list`/
   `set_to_list` read-backs unwrap into `Vec<f64>`).
3. Remove the gates (`lower.rs:4840-4852`, `4854-4864`, Decoder path,
   `reject_float_keyed_collection` at `5919-5938` + its call site
   ≈5981); delete `Feature::FloatKeyedCollection`; retire the code per
   A4. rustc's exhaustive-match friction enumerates the arms.
4. Ledger + explain-page updates per A3/A4, same commit.

## Part B — Locale-correct case mapping

### Problem statement

Current case mapping is Unicode-correct but locale-independent
everywhere: `string_to_upper`/`to_lower`
(`src/runtime/rust/src/string.rs:28-33`) and `string_casefold`
(`string.rs:339-346`) use Rust `to_uppercase()`/`to_lowercase()`;
`char_to_lower`/`char_to_upper`
(`src/runtime/rust/src/char_kernel.rs:24-69`) likewise. The Go
reference is identical in kind (`strings.ToUpper`/`ToLower`). The gap
is the locale-*sensitive* tailorings Unicode itself defines
(SpecialCasing.txt): Turkish/Azerbaijani dotted-İ/dotless-ı, Lithuanian
dot-above accumulation — plus Greek final sigma which Rust already
handles positionally. `docs/architecture/divergence-policy.md:233-250`
explicitly deferred this ("would need ICU-style data"); Elm's
counterparts `Char.toLocaleUpper`/`toLocaleLower` are absent (see
`elm-core-coverage.md`).

### Decision

**B1 — Defaults do not move.** `String.toUpper`/`toLower`/`casefold`
and `Char.toUpper`/`toLower` stay locale-independent root-locale
Unicode mappings: deterministic, reference-parity, and the only sane
behaviour for protocol/identifier text. This is a hard invariant — a
program's output must never depend on the host's locale environment.

**B2 — Locale is an explicit, typed argument.** New surface (final
naming to be reconciled with C.2's flat-namespace redesign; shapes
below assume the current namespace):

```elm
-- Ipe.Locale (new module)
type Locale                                  -- opaque
fromTag : String -> Maybe Locale             -- BCP-47, parse don't validate
toTag   : Locale -> String

-- String additions
toUpperIn  : Locale -> String -> String
toLowerIn  : Locale -> String -> String
casefoldIn : Locale -> String -> String     -- Turkic 'T' foldings when applicable
```

`fromTag` is the parse-don't-validate boundary: an unknown/malformed
tag is `Nothing` at the boundary, never a fallback-at-use-site. No
`Char`-level locale variants: locale-sensitive mappings are
context-dependent (can change string length, need neighbouring chars —
Lithuanian), so a per-`Char` API would be semantically wrong; this
deliberately does NOT mirror Elm's `Char.toLocaleUpper` (which is also
host-locale-implicit — see B3).

**B3 — Divergence from Elm, recorded.** Elm's `toLocaleUpper` uses the
*host's ambient locale* — output depends on the machine it runs on.
Ipê rejects ambient-locale APIs outright (violates B1's determinism
invariant). Entry for `docs/divergences-from-elm.md`: explicit-locale
API instead of host-locale API, rationale = reproducibility/security
(locale-dependent output is an input the operator didn't declare).

**B4 — Backend: ICU4X `icu_casemap`.** Pure Rust, maintained by the
Unicode consortium's ICU4X project, compiled-in data (casing data is
small — the locale-sensitive tailorings are only tr/az/lt), no C
dependency (keeps E.2 static compilation clean). Gate the dependency
behind a runtime cargo feature (e.g. `locale-case`) that the backend
enables in the generated project **only when the program uses the
`*In` kernels** — the backend already knows kernel usage at emit time;
programs that never touch locale pay zero binary/data cost.

**Alternatives rejected:** (1) hand-porting SpecialCasing.txt — small
today, but unversioned-Unicode-fork maintenance forever, and casefold
Turkic variants + Lithuanian context rules are exactly the fiddly parts
worth outsourcing; (2) `rust_icu` (C ICU bindings) — C dependency,
breaks static-binary posture, security surface; (3) changing default
`toUpper`/`toLower` to take a locale — breaks every program and B1;
(4) host-locale detection à la Elm — violates determinism (B3).

### Implementation plan (Part B)

1. Runtime: `locale.rs` (Locale = validated wrapper over
   `icu_locid::LanguageIdentifier`) + `*In` kernels in `string.rs`
   under `#[cfg(feature = "locale-case")]`.
2. Kernel registry: new `StdlibKernel` rows (Locale.fromTag/toTag,
   String.toUpperIn/toLowerIn/casefoldIn) through the full 5-layer
   recipe (canon registration + constrain schemes + lower arms +
   naming + runtime), sealed per the no-exit-0-then-cargo-fail
   mandate; backend flips the cargo feature on use.
3. Ledger entries per B3; the `elm-core-coverage.md` rows for the locale
   case functions move from absent to `intentional(explicit-locale)`.

## Test plan

Part A (`runtime/tests/` + `tests/golden/`):
- `float_key_dict_roundtrip` (runtime unit): insert/get/remove over
  negatives, -0.0/0.0 (same key), infinities, NaN (`get NaN` →
  `Just v`), plus `dict_keys` order deterministic with NaN last.
- `float_key_set_ops` (runtime unit): union/diff/intersect/member with
  NaN present; sorted iteration.
- Goldens `d3_dict_float_keys` / `d3_set_float` (`IPE_E2E=1`): a Ipê
  program exercising `Dict Float String` + `Set Float` end-to-end;
  non-NaN fixture byte-equivalent to the Go oracle; NaN fixture marked
  `oracle_divergence = true` with the A3 reason.
- Negative-regression: the old IPE-L0117 fixture flipped from
  expect-diagnostic to expect-success (the lift proof).
- Property test (proptest, matching #66 house pattern): for arbitrary
  `Vec<f64>` (NaN/inf-inclusive), `Set.member x (Set.fromList xs)` ⇔
  `xs` contains a key-equal element; no panic (soundness).

Part B:
- `locale_case_parity` (runtime unit): `toUpperIn tr "i"` → `"İ"`,
  `toLowerIn tr "I"` → `"ı"`, az same, `toLowerIn lt` dot-above case,
  root locale ≡ default `toUpper`/`toLower` on ASCII + Greek sigma;
  `casefoldIn tr` Turkic fold.
- `locale_from_tag`: valid tags parse; garbage/empty → `Nothing`
  (parse-don't-validate pin).
- Golden `d3_locale_case` (`IPE_E2E=1`): Ipê-level round trip; no Go
  oracle exists (new surface) → recorded as Ipê-only expected output
  per the Go-failure/new-surface convention.
- Feature-gating pin: a golden NOT using locale kernels asserts the
  generated Cargo.toml does not enable `locale-case`.

# Sound emission of the full IPE-L0131 row-polymorphic set

Status: **tbd/ proposal — not shipped.** Every fenced Rust block below is a
*proposed illustrative emission sketch*, not verified compiler output; exact
spellings are fixed by the golden tests of each increment when it lands. No
part of this document lifts the `IPE-L0131` gate in code; it specifies how to
lift it, sub-case by sub-case, soundly.

Motivated by the open SEAL-audit issue "row-polymorphic record-update function
passed as a value to a typed HOF may emit a signature mismatch" (referred to
below as the *HOF-value SEAL audit*). Companion to the already-partly-shipped
`docs/internals/design/row-polymorphism-design.md` (the witness-trait strategy + its
6-increment plan) and to `docs/internals/design/generic-function-carrier-design.md`
(function values crossing generic call boundaries). This document extends the
first to the **entire** IPE-L0131 set and reconciles the row-poly-function-as-a-
value case with the second.

Governance: this design obeys the PRINCIPLES.md order
(Security > Correctness > Soundness > Efficiency > Completeness > Readability),
"parse, don't validate", and "make invalid states unrepresentable". It honours
the standing project rule: **no `dyn Any`, no runtime reflection, no per-field
constructor registry, no type erasure to an untyped bag.** Every open row is
erased to a *statically typed, rustc-monomorphised generic* bounded by a
synthesised witness trait — never a dynamic value.

---

## 0. Where the code actually stands today (read before designing)

The witness-trait substrate is **already shipped** for the simplest sub-case.
Precise inventory (paths current as of this writing):

| Piece | State | Where |
| --- | --- | --- |
| `IrType::RowGeneric(Symbol)` — the row var in type position | shipped | `src/compiler/ir/src/ir.rs:1044` |
| `Func::row_params: Vec<RowParam>`, `RowParam { var, fields }` | shipped | `src/compiler/ir/src/ir.rs:849,866` |
| Witness **getter** trait synth `IpeHas<F> { type F; fn ipe_f(&self) -> &Self::F; }` + one impl per registry struct carrying `f` | shipped | `src/compiler/backend/rust/src/emit_types.rs:1215` (`emit_row_witnesses`), naming at `naming.rs:329,337` |
| Witness-name disjointness gate (E0428 guard) | shipped | `src/compiler/backend/rust/src/lib.rs:2330` (`assert_row_witness_names_disjoint`) |
| Body **field-read-only** escape analysis on a row-typed value | shipped | `src/compiler/lower/src/lower.rs:1882` (`escapes`) |
| Call-site argument shape check (`ty_can_satisfy_row_witness`, non-record → clean error) | shipped | `src/compiler/lower/src/lower.rs:1836`, `check_row_param_caller_fields` |
| **The gate** `canon_sig_has_unsupported_open_row` / `canon_type_has_open_row` | shipped, fail-closed | `src/compiler/lower/src/lower.rs:1773,1796`; call sites at `:13877,:14227,:14739,:14794,:17406` |

**Supported today (builds + runs):** a single- or multi-field open row in
**argument position** whose field types are themselves closed, used in the body
only as the immediate receiver of a field read (`rec.name`), the function called
**directly**. Pinned by `row_poly_greet`, `row_poly_multi`, `row_poly_subset_access`,
`row_poly_subset_pattern`, and the acceptance canary
`row_var_annotation_lowers_to_witness_generic`
(`src/ipe-cli/tests/g_record_generic/golden_row_poly_records.rs:327`).

**Gated by IPE-L0131 (the remaining set this doc designs):**

- **G1 return-position** — `{ r | n : Int } -> { r | n : Int }`, even a
  pass-through body.
- **G2 record-update through a row** — `{ rec | n = rec.n + 1 }` where
  `rec : R` (no setter method exists; the concrete struct is unknown at emit).
- **G3 nested under a container** — `List { r | n : Int } -> …`,
  `Maybe { r | n : Int }`.
- **G4 nested under a record/tuple** — `{ outer : { r | n : Int } }`,
  `({ r | n : Int }, Int)`.
- **G5 field type embeds an open row** — `{ r | inner : { s | k : Int } }`.
- **G6 the HOF-value carrier** (the SEAL audit) — `List.map bump xs`: `bump`
  passed **by name as a value** into a monomorphised higher-order kernel.
- **G7 field-less row** `{ r | }` — degenerate, stays gated (§8).

The audit's repro `bump : { r | n : Int } -> { r | n : Int }` with body
`{ rec | n = rec.n + 1 }` used in `List.map bump xs` trips **G1 + G2 + G6 at
once**: return position, an update body, and a HOF-value use. It is the
capstone, not the base case.

---

## 1. The emission model — the structural core

### 1.1 One rule, applied everywhere

An open row `r` in a function signature is a **genuine, user-written type
variable naming a residual field set**. It lowers to a real Rust generic type
parameter `R{n}`, bounded by exactly the witness traits for the fields the
annotation names — a getter witness `IpeHas<F><F = T>` for every field read,
and (new here, §2.2) a *functional-update* witness `IpeWith<F>` for every field
the body updates. rustc monomorphises `R{n}` to the caller's concrete record
struct at each call site; the extra fields survive because `R{n}` **is** the
caller's whole struct, not a projection of it.

The whole design is the disciplined observation that **once the row is a real
generic type parameter `R`, `R` propagates structurally through every type
constructor Rust already has** — return position, `Vec<R>`, a field of a generic
struct, a tuple element, a nested associated-type bound. There is no new
mechanism per sub-case; there is one mechanism (`R` is a generic) and the
existing Rust type grammar carries it. That is why a single coherent solution
covers the entire IPE-L0131 set.

The base case, restated in the target shape (*proposed, not shipped*):

```rust
// bump : { r | n : Int } -> { r | n : Int }   (return-position + update)
pub trait IpeHasN  { type N; fn ipe_n(&self) -> &Self::N; }
pub trait IpeWithN: IpeHasN { fn ipe_with_n(self, v: Self::N) -> Self; }

pub fn main_bump<R1>(rec: R1) -> R1
where
    R1: IpeHasN<N = i64> + IpeWithN + Clone,
{
    // { rec | n = rec.n + 1 }
    let v = *rec.ipe_n() + 1;
    rec.ipe_with_n(v)
}
```

`R1 -> R1` is the return-position case (G1). `ipe_with_n(self, …) -> Self` is
the update case (G2). Neither needs the concrete struct name: `Self` **is** the
caller's struct, chosen by rustc. The `..self` functional-update that rebuilds
the extra fields lives inside each per-struct impl (§2.2), where the struct name
*is* known.

### 1.2 Validating the rule against every IPE-L0131 sub-case

The invariant to preserve (pinned-records ADR 0018): **every `IrType::Record`
the backend materialises as a struct is closed.** The open row never becomes a
`Record` in the backend — it is erased to `IrType::RowGeneric` → a bounded
generic *before* the struct registry can see it. Each sub-case below keeps that
invariant; the open row is always a generic `R`, never a struct.

- **G1 return position.** The return type is the same `R1`. `fn f<R1: …>(rec: R1) -> R1`.
  Rust returns a generic value directly. Already true of the body escape analysis'
  `tail_sym` exemption (`escapes`, `lower.rs:1870`): a bare `Var(row_sym)` in tail
  return position is *already known safe* — the emitter simply never had a
  return-position lowering to route it to. This slice is small.

- **G2 update through a row.** The `IpeWith<F>` functional-update witness (§2.2).
  `{ rec | n = v }` on `rec : R1` emits `rec.ipe_with_n(v)`; a multi-field update
  chains: `rec.ipe_with_n(a).ipe_with_label(b)`. No struct-update syntax at the
  call site; each impl does the `Self { n: v, ..self }` rebuild.

- **G3 nested under a container.** The container **carries** `R`:
  `List { r | n : Int } -> List { r | n : Int }` emits `Vec<R1>` where
  `R1: IpeHasN<N = i64>`. `Maybe { r | n } → Option<R1>`. No new witness — the
  container is Rust's own `Vec`/`Option` generic over the same `R1`. `List.map`
  over such a list is exactly G6 (§4).

- **G4 nested under a record/tuple.** The enclosing type is **generic over `R`**.
  `{ outer : { r | n : Int } } -> Int` — the enclosing record struct becomes a
  *generic* registry struct `RecOuter<T1>` (which the registry already supports,
  `type_params`, `lib.rs:630`) instantiated at `T1 = R1`. A tuple element:
  `({ r | n : Int }, Int)` → `(R1, i64)`. The row generic slots into the
  registry's existing generic-struct machinery; no struct is minted for the open
  row itself.

- **G5 field type embeds an open row.** A *second* row generic + a chained
  associated-type bound. `{ r | inner : { s | k : Int } }` →
  `R1: IpeHasInner, R1::Inner: IpeHasK<K = i64>`. The inner row is its own `R2`
  where it appears as a standalone parameter, or a chained bound on `R1::Inner`
  where it is a field type. Deliberately the last slice (§7): it only multiplies
  bound-rendering cases, no new mechanism.

- **Closed at the call site → concrete, not generic.** When the caller passes a
  fully known record and no annotation forces the row open, the existing
  concrete/monomorphised path stays: the exact-field-set struct is resolved by
  the registry (`record_by_fieldset`) and the field read is a plain `.n`, not a
  witness getter. The generic path is **the fallback taken only when the row is
  genuinely open**. This is the standing "prefer concrete; genuine type vars
  become rustc-monomorphised generics, never `dyn Any`" policy, unchanged.

### 1.3 `List.map bump xs`, end to end (*proposed sketch*)

Source (the HOF-value SEAL-audit repro):

```elm
bump : { r | n : Int } -> { r | n : Int }
bump rec = { rec | n = rec.n + 1 }

main : Task Error ()
main =
    let xs = [ { n = 1, label = "a" }, { n = 2, label = "b" } ]
    in Io.println (Debug.toString (List.map bump xs))
```

`xs` pins the element shape concretely: `RecLabelN` (sorted field set
`{label, n}`), a closed registry struct. `List.map`'s callback slot is
monomorphic *at this call site* — it wants `Fn(RecLabelN) -> RecLabelN`. `bump`
is a generic function `main_bump<R1>`; passed as a value it must be
**instantiated at `R1 = RecLabelN`** so the carrier and the call site agree.
That instantiation is the whole of the SEAL audit (§4).

Emitted crate (*proposed, not shipped*):

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct RecLabelN { pub label: String, pub n: i64 }

pub trait IpeHasN  { type N; fn ipe_n(&self) -> &Self::N; }
pub trait IpeWithN: IpeHasN { fn ipe_with_n(self, v: Self::N) -> Self; }

impl IpeHasN for RecLabelN { type N = i64; fn ipe_n(&self) -> &i64 { &self.n } }
impl IpeWithN for RecLabelN {
    fn ipe_with_n(self, v: i64) -> Self { RecLabelN { n: v, ..self } }
}

pub fn main_bump<R1>(rec: R1) -> R1
where R1: IpeHasN<N = i64> + IpeWithN + Clone {
    let v = *rec.ipe_n() + 1;
    rec.ipe_with_n(v)
}

pub fn main() -> /* Task Error () lowering */ {
    let xs: Vec<RecLabelN> = vec![
        RecLabelN { n: 1, label: "a".to_string() },
        RecLabelN { n: 2, label: "b".to_string() },
    ];
    // List.map with the callback instantiated at the pinned element shape:
    let mapped: Vec<RecLabelN> =
        xs.into_iter().map(main_bump::<RecLabelN>).collect();
    ipe_println(ipe_debug_to_string(&mapped))
}
```

The key line is `main_bump::<RecLabelN>` (equivalently
`.map(|x| main_bump(x))`): the callback is instantiated at the pinned element
type, so its emitted signature is `Fn(RecLabelN) -> RecLabelN` — **identical**
to what `List.map`'s monomorphic slot expects. No erased `fn(any) -> any` twin,
no mismatch, no `dyn Any`. rustc monomorphises `main_bump` once for `RecLabelN`;
`ipe_with_n` returns `RecLabelN { n: v, ..self }`, preserving `label`. Output:
`[{ n = 2, label = "a" }, { n = 3, label = "b" }]`.

---

## 2. Trait synthesis rules

### 2.1 Getter witness (shipped) — restated for SSOT

For every field name `f` that appears in **any** row bound anywhere in the
program (`row_witness_field_names`, `emit_types.rs:1191`), synthesise **once**:

```rust
pub trait IpeHasF { type F; fn ipe_f(&self) -> &Self::F; }
```

and one impl for **every registry struct carrying `f`** (total over the struct
namespace, no reachability analysis — correctness needs none). The associated
type (not a type parameter on the trait) is what lets one trait serve `f` at
every type it occurs at across structs; the *bound* `R: IpeHasF<F = T>` does the
type-checking. Naming: `field_witness_trait_name("name") = "IpeHasName"`,
`field_witness_getter_name = "ipe_name"` (`naming.rs:329,337`), keyword-mangled,
disjointness-gated (`assert_row_witness_names_disjoint`).

**SSOT / no duplicate traits.** Two functions sharing a field `n` in their row
annotations reuse the **one** `IpeHasN` trait — synthesis is keyed on the field
name, program-globally, not per-function. This is already how
`row_witness_field_names` collects (a `BTreeSet<Symbol>` union across all
`row_params`). Adding update witnesses (§2.2) keeps the same keying: one
`IpeWithN` per field name, program-wide.

### 2.2 Update witness (new — unblocks G2)

For every field name `f` that appears in a row bound **and is updated** in some
row-poly body (a new bit on `RowParam`, §5.2: `updated_fields: BTreeSet<Symbol>`),
synthesise a **second** trait that supertraits the getter:

```rust
pub trait IpeWithF: IpeHasF { fn ipe_with_f(self, v: Self::F) -> Self; }
```

and one impl per registry struct carrying `f`, which is the *only* place the
concrete struct name and its full field list are known, so the `..self`
functional update is emitted there:

```rust
impl IpeWithN for RecLabelN {
    fn ipe_with_n(self, v: i64) -> Self { RecLabelN { n: v, ..self } }
}
```

Design decisions, each load-bearing:

- **`self`-by-value, returns `Self`.** Functional update, not mutation — matches
  Ipê's immutable record semantics and the pinned-records ADR. `..self` moves
  the untouched fields (no clone of the residual), so it is also the efficient
  shape (Efficiency, principle 4).
- **Supertrait `IpeWithF: IpeHasF`.** An updatable field is always readable;
  the bound `R: IpeWithN` implies `R: IpeHasN`, so the body may both read and
  update. This makes the *invalid* state ("updatable but unreadable") structurally
  unrepresentable.
- **Getter borrows, setter consumes.** Read is `&self -> &Self::F` (no clone
  unless the body needs an owned copy — composes with the existing Copy-elision
  at `emit_expr.rs` Access arm); update is `self -> Self`. Ipê value semantics
  choose which per use, exactly as for concrete records.
- **Impl totality unchanged.** Still one impl per (field, struct-having-it) pair,
  emitted next to the getter impl in the same `emit_row_witnesses` loop. No new
  registry, no `dyn`.

The existing kernel precedent `ws_server_with_on_error`
(`emit_types.rs:526`) already shows a `with_*(…) -> Self` setter shape in the
codebase; `IpeWithF` is the row-generic generalisation of that pattern.

### 2.3 Impl generation for generic registry structs

A registry struct may itself be generic (`RecValue<T1>`). The getter impl
already renders the field type in the struct's own `GenericScope`
(`emit_types.rs:1244`); the setter impl reuses the identical scope. So
`{ r | value : a } -> a` composes `T1` (the field's own type var) with `R1`
(the row var) with zero new machinery:

```rust
pub fn main_get<T1, R1>(rec: R1) -> T1
where R1: IpeHasValue<Value = T1> + Clone { rec.ipe_value().clone() }
```

---

## 3. Why NO `dyn Any` — the soundness argument

The SEAL is Ipê's core invariant: **a program that type-checks (ipe exit-0) MUST
emit buildable Rust (cargo exit-0).** This design holds the SEAL *by emitting
right*, not by rejecting.

- **The type says `R -> R`; the signature is `R -> R`.** The Ipê type of `bump`
  is "any record with `n : Int`, preserved". The emitted Rust type is exactly a
  generic `R1: IpeHasN<N = i64> + …` mapped `R1 -> R1`. There is no gap between
  the checked type and the emitted type, so there is no exit-0-then-cargo-fail
  seam. (Contrast the pre-existing bug class in
  `generic-function-carrier-design.md`: a *wrong* carrier decision — two sites
  disagreeing — is the miscompile-adjacent hazard this design must not create.
  §4 closes it by instantiation, not erasure.)

- **Every emitted bound is a theorem of the type checker.** The only bounds are
  witnesses the unifier already proved present, with the right associated type,
  at every call site (`resolve_deferred`, `types/src/lib.rs:1589`; the update
  obligation via `resolve_one_record_update`), plus `Clone` (total over the
  registry, §6). So "the impl exists" is not a hope — it is the same theorem the
  closed-record path rests on. No bound can be emitted that a call site fails to
  satisfy.

- **Security — no reflection surface.** The emitted code is pure record data +
  static trait dispatch. There is no `Any`, no `downcast`, no type-id table, no
  string-keyed field lookup — nothing an attacker can probe or confuse. A
  `dyn Any` design would introduce exactly such a runtime type-identity surface
  and an `unwrap`-on-downcast panic path (a soundness hole, principle 3). This
  design has neither.

- **Efficiency — zero runtime dispatch.** rustc monomorphises each
  `(row-poly fn × concrete shape)` pair to a direct, inlinable call. `ipe_n()`
  is a field load; `ipe_with_n` is a struct rebuild with `..self` moves. No vtable,
  no boxing, no refcount. A `dyn Any` design pays a heap box + dynamic downcast
  per field touch — strictly worse on every axis.

- **Correctness — deterministic.** No dynamic dispatch means no
  implementation-defined dispatch order; output is fixed by the monomorphised
  code.

**Contrast table:**

| | witness-trait generic (this design) | rejected `dyn Any` / registry |
| --- | --- | --- |
| Static typing | full — `R -> R`, checked by rustc | erased to an untyped bag |
| Reflection surface | none | type-id downcast (security + panic risk) |
| Runtime dispatch | none (monomorphised) | box + downcast per field |
| SEAL | holds by construction (bounds = theorems) | downcast can fail at runtime |
| Extra fields preserved | yes (`R` is the whole struct) | only if re-boxed correctly |
| Project policy | compliant | forbidden |

The trait-generic model is **strictly better** on Security, Soundness, and
Efficiency, and is the only one compatible with the standing no-`dyn Any` rule.

---

## 4. Interaction with typed HOFs / the generic carrier (the SEAL audit)

This is the crux the audit issue names, and the one place a naive extension would
*create* a SEAL break rather than close one.

### 4.1 The hazard, precisely

`List.map bump xs` passes `bump` **as a value** into a kernel whose callback slot
is monomorphic. Two things must agree:

1. the **carrier** — how the callback value is emitted (a `Box<dyn Fn>`, a
   direct generic function item, an instantiated function pointer);
2. the **call site** — the concrete type `List.map` expects for its callback,
   fixed by the pinned element type of `xs`.

The bug the audit fears: the carrier is emitted **erased** (`fn(any) -> any`, or
a `Box<dyn Fn>` over the unmonomorphised body) while the call site is emitted
**monomorphised** (`Fn(RecLabelN) -> RecLabelN`) — `rustc` rejects the pair
(E0308 / E0277). This is the same erased-vs-monomorphised disagreement that
`generic-function-carrier-design.md` §1 documents for `unwrap (wrap f)`.

### 4.2 The resolution: instantiate the carrier at the pinned shape

The rule (a total function of types at the seam, matching the carrier family's
philosophy in `generic-function-carrier-design.md` §3):

> A row-poly function passed as a value into a slot whose solved type is a
> **concrete arrow** `Fn(Rec) -> Rec` is emitted as its **generic function item
> instantiated at that concrete shape** — `main_bump::<RecLabelN>` — so the
> carrier's emitted signature **is** the call site's expected signature. No
> erasure, no `Box<dyn>` over an unmonomorphised body.

Why this is sound and total:

- The surrounding expression (`List.map bump xs`) is **monomorphically pinned**:
  `xs : List RecLabelN` fixes the element shape, hence the callback's concrete
  arrow (mechanism-4 pinning from the pinned-records ADR). So the instantiation
  type is *always available* at the seam — this is not a heuristic.
- The existing `Expr::FuncValue` path already emits generic function items used
  as values and lets rustc resolve the instantiation from the target type
  (`row-polymorphism-design.md` §4.5, "Row-poly fn used as a value"). Row-poly
  functions are ordinary generic functions with a richer bound; the same path
  carries them once the target arrow is known concrete.
- **Fail-closed frontier.** A context that genuinely **cannot** pin the shape —
  a row-poly function stored in a record-of-functions field whose type is *itself*
  an open row, or handed to a fully-polymorphic slot with no concrete arrow —
  keeps the existing rejection (`generic-function-carrier-design.md`'s A1 gate,
  IPE-L0107; and this doc's IPE-L0131 for the unpinnable row case). Never a
  `dyn Any` fallback, never a silent erased carrier.

### 4.3 Reconciling with `generic-function-carrier-design.md`

That doc's §6 "Row polymorphism" risk note anticipated this exact junction and
left the direction open. This design chooses the **witness-trait** direction it
flagged, and answers its concern:

- Its worry ("a witness-trait emission instead would deepen the bound problem —
  a `Clone` witness per row") is bounded: the bound set is one getter witness +
  optionally one update witness per *field*, plus `Clone` — all proven by the
  solver, all total over the registry. It does not grow with call sites, only
  with distinct field names in annotations.
- Its A1 gate (`reject_fn_through_generic_slot`, IPE-L0107) **remains the
  backstop**: a *function-typed field* flowing through a generic slot is still
  its concern, orthogonal to a *record* flowing through a row generic. The two
  gates partition the space; neither is weakened. The audit's case is a *record*
  (`RecLabelN`) flowing through `bump`'s row var — pure record data, no function
  in the carried value — so A1 does not fire and must not; the row-instantiation
  rule (§4.2) is what makes it build.
- Both designs share the one invariant: **single carrier per instantiation.**
  §4.2 instantiates `main_bump` once per pinned element shape, so every site that
  performs the same instantiation agrees by construction — no two-carrier E0308.

### 4.4 SEAL golden (regardless of verdict)

The audit's repro enters the SEAL golden corpus as a **positive** golden
(`row_poly_map_update`): ipe exit-0, cargo build green, run output
`[{ n = 2, label = "a" }, { n = 3, label = "b" }]`. If any slice regresses to
an erased carrier, this golden's cargo build fails loudly — the cell is pinned.

---

## 5. Lifting IPE-L0131 — order, remnant, diagnostics

### 5.1 Lift order (each slice narrows the gate, none advertises an unshipped form)

The gate `canon_sig_has_unsupported_open_row` is narrowed **sub-case by
sub-case**; the `IPE-L0131` explain page is rewritten at every narrowing so it
never claims support for a form not yet shipped (project rule: never advertise
unimplemented).

1. **G1 return-position** lifts first (§7 P1). `canon_sig_has_unsupported_open_row`'s
   trailing-`ret` arm (`lower.rs:1816`) stops reporting a bare `RecordOpen` whose
   fields are closed; the `escapes` tail exemption already permits the body.
2. **G2 update-through-row** lifts with the `IpeWith<F>` witness (§7 P1, same
   slice — needed for `bump`). The body-escape analysis gains an "update receiver"
   exempt shape alongside the field-read shape.
3. **G6 HOF-value carrier** lifts with the instantiation rule (§4, §7 P2).
4. **G3 container** lifts (§7 P3): the `Con { args }` arm of
   `canon_type_has_open_row` stops reporting `List`/`Maybe`/`Set` of a supported
   row.
5. **G4 record/tuple** lifts (§7 P4): the `Record`/`Tuple` arms stop reporting a
   supported nested row; the enclosing struct becomes generic over `R`.
6. **G5 field-embeds-row** lifts last (§7 P5): chained associated-type bounds.

### 5.2 What stays gated, and why

- **G7 field-less row `{ r | }`** — carries **no** witness obligation, so it is
  a pure "some record, we know nothing about it" with no readable/updatable
  field. It has no useful emission (a fully-unbounded `R` with zero bounds is
  just `T: Clone`, indistinguishable from a plain type var, and the annotation
  conveys nothing the checker can enforce). Kept rejected as degenerate — a
  clean "an open row must name at least one field" diagnostic.
- **Row unifying with an opaque/kernel record** — stdlib opaque records
  (`HttpRequest`, server types) and the Web-cfg open-row schemes are **not**
  registry structs and get no witness impls; a user annotation forced to unify
  with one is rejected by the type layer today and stays so
  (`row-polymorphism-design.md` §4.5, mechanism 3). Never routed into witness
  synthesis.
- **Row-poly fn in a truly unpinnable value slot** — §4.2's frontier: stays
  fail-closed (IPE-L0107 / IPE-L0131), never `dyn`.

### 5.3 Diagnostic changes

- `IPE-L0131`'s message + explain page narrow at each slice to name **exactly**
  the still-unshipped forms; when only G7 + the opaque-record remnant remain, its
  message becomes "an open row must name at least one field, and cannot unify
  with an opaque/kernel record type", and it is no longer reachable for G1–G6.
- New `RowParam.updated_fields` metadata (§2.2) drives a fresh, teachable body
  error if a body updates a field the annotation does not name ("the annotation
  only guarantees `n`; this update needs `label`") — mirrors the existing
  rigid-row field-read message.
- The non-record-argument message (`ty_short_name`, `lower.rs:1848`) is unchanged.

---

## 6. Edge cases / risks (each handled soundly)

- **Trait coherence / orphan rules.** *Not a problem.* Both the witness traits
  (`IpeHas*`, `IpeWith*`) and every impl'd struct (`RecLabelN`, …) are
  **synthesised into the same emitted crate**. The impls are local-trait +
  local-type — orphan rules are trivially satisfied. No blanket impls, no
  impls on foreign types. Overlap is prevented by construction: one impl per
  (field, struct) pair, structs have distinct names.
- **Monomorphisation blowup.** One machine copy per `(row-poly fn × distinct
  used shape)` — identical in kind to the existing generic-function cost, and
  strictly smaller than the rejected source-level specialisation (which
  duplicates *source*, not just machine code). Impl fan-out is one impl per
  (row-used field name × struct-having-it), bounded by the program's own
  annotations; pruning to structs that actually flow into rows is a later
  *efficiency-only* refinement, never needed for correctness.
- **Recursion.** A row-poly fn calling itself keeps the *same* rigid `R` — no
  new instantiation, so rustc terminates monomorphisation (the recursive call is
  at `R = R`, not a growing type). A row-poly fn calling a *different* row-poly
  fn: the caller's rigid row cannot grow, so the callee's required fields must
  appear in the caller's own annotation (type checker enforces), and the caller's
  emitted bound set syntactically covers the callee's — rustc accepts the inner
  call with no extra machinery.
- **Co- and contravariant use of one row.** `{ r | n : Int } -> { r | n : Int }`
  uses `R` in both argument (contravariant) and return (covariant) position with
  the **same** `R1`. Because it is one generic parameter, rustc unifies both
  occurrences to the caller's single concrete struct — shape-preserving is
  automatic. A row used at *two different* concrete shapes across two call sites
  instantiates `R1` twice (two monomorphisations), each internally consistent.
- **Empty / fully-open cases.** G7 (field-less) stays gated (§5.2). A fully-open
  row with fields is the ordinary supported case.
- **`Clone` totality.** Every registry struct is `Clone` by construction
  (`RecordStruct::is_clone`, `lib.rs:660`). If a non-`Clone` carrier could ever
  enter the registry, the lowering adds a fail-closed "this record cannot flow
  into a row-polymorphic call" guard rather than risk a post-exit-0 cargo fail —
  pinned by the first slice's guard test (this is already the shipped posture,
  `row-polymorphism-design.md` §4.4).
- **Update witness × generic struct.** `ipe_with_f` on a generic struct
  `RecValue<T1>` rebuilds `RecValue { value: v, ..self }` in the struct's
  `GenericScope` — the `..self` carries the residual generic fields untouched.
  No specialisation needed.
- **Getter borrow vs owned need.** `*rec.ipe_n() + 1` needs an owned `i64`
  (Copy — deref, no clone). A `String` field read that must be owned emits
  `rec.ipe_name().clone()`, composing with the existing Copy-elision discipline.
  The setter consumes `self`, so a body that reads *then* updates must order the
  read before the move (the lowering already sequences `let v = read; update`,
  as in §1.1).

---

## 7. Phased implementation plan

Each phase is independently landable, guardian-reviewable, and keeps the SEAL
(exit-0 ⇒ cargo-green, by acceptance for the covered set and rejection for the
rest). Every phase: **failing golden/unit tests first → minimal change → full
gate** (workspace nextest, clippy deny-set, golden E2E byte-diff, the SEAL E2E
fixtures, examples sweep for zero new rejections). Golden re-blessing is cheap
and automated; byte-diff churn is never a reason to reshape a phase. Carrier
work (P2) is **guardian-gated**: the security-soundness reviewer must see the
single-carrier-per-instantiation argument and build the emitted crates
independently.

**P1 — Return position (G1) + update-through-row (G2).** The slice that makes
`bump` itself compile as a *direct call*.
- *Failing tests first:* golden `row_poly_passthrough`
  (`touch : { r | n : T } -> { r | n : T }`, pass-through body, two shapes);
  golden `row_poly_update` (`bump` called directly, `{ rec | n = rec.n + 1 }`,
  verified at two shapes); unit tests for `IpeWith<F>` trait+impl synthesis
  naming, supertrait declaration, `..self` rebuild rendering, and the
  updated-field-not-in-annotation body error.
- *Change:* `RowParam.updated_fields`; `IpeWith<F>` synthesis in
  `emit_row_witnesses`; return-position `RowGeneric` lowering; the body-escape
  "update receiver" exempt shape; narrow the gate's `ret` arm + the update path.
- *Gate:* full; `row_poly_passthrough` + `row_poly_update` flip from IPE-L0131
  to acceptance; the `*_neg` rejection goldens stay rejecting.

**P2 — HOF-value carrier (G6, the SEAL audit).** The capstone for `bump` as a
*value*.
- *Failing tests first:* the audit repro as positive SEAL golden
  `row_poly_map_update` (build + run, output pinned); an over-rejection tripwire
  (`List.map bump xs` at a *second* element shape in the same program — two
  instantiations, both build); a fail-closed tripwire (row-poly fn into a
  genuinely unpinnable slot → clean IPE-L0107/IPE-L0131, no `dyn`).
- *Change:* the instantiation rule (§4.2) in the `Expr::FuncValue` /
  callback-carrier path — emit the generic item instantiated at the pinned
  concrete arrow; reconcile with `reject_fn_through_generic_slot` (partition, not
  overlap).
- *Gate:* full + **guardian carrier review** (independent build of the emitted
  crate; the single-carrier argument).

**P3 — Row under a container (G3).** `List { r | … }`, `Maybe`, `Set`.
- *Failing tests first:* goldens `row_poly_list_arg`
  (`List { r | n : Int } -> List Int`) and `row_poly_maybe`.
- *Change:* the `Con { args }` arm of `canon_type_has_open_row` stops reporting
  supported rows under containers; container rendering carries `R`.
- *Gate:* full.

**P4 — Row under a record/tuple (G4).** Enclosing type generic over `R`.
- *Failing tests first:* goldens `row_poly_in_record`
  (`{ outer : { r | n : Int } } -> Int`) and `row_poly_in_tuple`.
- *Change:* the `Record`/`Tuple` arms stop reporting a supported nested row; the
  enclosing registry struct becomes generic over `R1` (reuse `type_params`).
- *Gate:* full.

**P5 — Field type embeds an open row (G5).** Chained associated-type bounds.
- *Failing tests first:* golden `row_poly_nested`
  (`{ r | inner : { s | k : Int } }`), verified at two outer shapes.
- *Change:* chained `R1::Inner: IpeHasK<K = …>` bound rendering; a second row
  generic where the inner row stands alone.
- *Gate:* full.

**P6 — Retire the reachable gate; docs.**
- *Change:* remove `IPE-L0131` from the reachable surface for G1–G6 (keep only
  the G7 field-less + opaque-record remnant, §5.2); supersede the pinned-records
  ADR with a successor stating the refined invariant — *every `IrType::Record`
  reaching the backend is closed; open rows reach it only as witness-bounded
  generics*; update `docs/divergences-from-elm.md` and the record chapters.
- *Gate:* full + docs-sync sweep.

---

## 8. Open risks needing the maintainer's decision

1. **G7 field-less row: reject forever, or a future "opaque record" use?** This
   design keeps `{ r | }` rejected as degenerate (§5.2). If a genuine use appears
   (a fully-parametric "some record" passed straight through), it would emit as a
   plain `T: Clone` generic with no witness — indistinguishable from a bare type
   var, so arguably it should just *be* a bare type var in the source. **Proposed:
   keep rejected**; maintainer confirms.

2. **P2 guardian-gate depth.** The carrier instantiation (§4.2) is the one place
   a mistake re-creates a SEAL break (erased-vs-monomorphised). This plan
   guardian-gates P2 with an independent crate build. **Confirm** that the
   guardian must also run the `--features full` and examples-sweep passes for P2
   (per the "wait for guardian verdict; test --features full" project rule),
   given HOFs appear across the example corpus.

3. **Efficiency pruning of impl fan-out.** Correctness needs one witness impl per
   (field, struct-having-it) pair, program-wide. On a large program this is
   O(fields × structs) impls in the emitted source. Pruning to structs that
   *actually* flow into a row is a pure efficiency refinement (§6). **Proposed:
   ship un-pruned (correct, readable), add pruning only if emitted source size
   measurably bites**; maintainer confirms the ordering (Efficiency below
   Completeness/Readability here).

4. **Interaction with a future first-class-function-in-record feature.** If FCF
   record fields ever land (currently gated, `no-functions-in-records` standing
   decision), a row whose field type is a function would need the `SharedFun`
   carrier *inside* the witness associated type. Out of scope here (G5 field types
   are data); flagged so the two designs are reconciled before either widens into
   the other's territory.

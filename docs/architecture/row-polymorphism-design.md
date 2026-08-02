# Row polymorphism: open records, first-class accessors, and their Rust monomorphisation

A function annotated `greet : { r | name : String } -> String` accepts every
record carrying at least a `name : String` field. The bare accessor `.name` is
a value of type `{ r | name : a } -> a`, so `List.map .name people` works. Both
are standard Elm surface. This document specifies how the remaining half —
emitting a genuinely row-polymorphic function as **concrete, monomorphised
Rust** with no dynamic dispatch — is designed, and the ordered, test-first plan
to land it.

All Rust snippets in this document are **illustrative sketches of the intended
emission shape**, not verified compiler output; exact spellings are fixed by
the golden tests of each increment.

## 1. Where the compiler already stands

Most of the pipeline is in place; only the backend gap remains.

| Layer | State | Where |
| --- | --- | --- |
| Parse: `{ r \| f : T }` annotation | shipped (`TRecordOpen`) | `src/compiler/parse/src/parser.rs:969` (opener probe at `:992`) |
| Parse: first-class accessor `.f` | shipped — desugars to the getter lambda `\ipe_accessor_arg -> ipe_accessor_arg.f` | `src/compiler/parse/src/parser.rs:1283` (`parse_field_accessor`) |
| Canon | shipped (`canon::Type::RecordOpen(row_var, fields)`; row var collected as a quantified type variable) | `src/compiler/lower/src/lower.rs:1206` (`collect_type_vars`) |
| Type layer: row model | shipped — `Ty::Record(BTreeMap<Symbol, Ty>, RowTail)` with `RowTail::Closed / Open(var)` | `src/compiler/types/src/ty.rs:89,150` |
| Type layer: unification | shipped — mirrors the reference `unifyRecord` four-case split (both closed / one open / both open with a fresh bridging tail); `EmptyRecord` is the closed-tail sentinel | `src/compiler/types/src/unify.rs:246` (`unify_flat`), `:335–:433` |
| Type layer: deferred obligations | shipped — `FieldAccess` / `RecordUpdate` recorded during constraint build, discharged post-solve against the settled record type | `src/compiler/types/src/constrain.rs:1436,1463`; `src/compiler/types/src/lib.rs:1589` (`resolve_deferred`) |
| Lowering gate | **fail-closed** — any open row in a signature raises `IPE-L0131` before emission | `src/compiler/lower/src/lower.rs:1224` (`canon_type_has_open_row`), `:8716` (gate), `:9511` (`ir_type_from_canon` ICE arm if the gate is bypassed) |
| Backend records | closed shapes only — one Rust struct per exact sorted field-name set (`record_by_fieldset`), miss = compiler bug; generic shapes carry `type_params` matched by template | `src/compiler/backend/rust/src/lib.rs:403` (`RecordStruct`), `:1720` (`render_record_use`), `:1810` (`record_struct_by_key`) |
| Backend generics | named type variables emit rustc generics: `Func::type_params: Vec<(Symbol, BoundSet)>` → `pub fn name<T1: Bounds, …>`; rustc monomorphises | `src/compiler/ir/src/ir.rs:677` |

The invariant this design must preserve is the pinned-records ADR
(`docs/adr/0018-row-poly-records-pinned-before-lowering.md`): the backend
resolves records by exact field-set match and fails loud on a miss. The design
below **keeps every `IrType::Record` closed** — an open row never reaches the
struct registry at all; it is erased into a bounded Rust generic instead.

## 2. Surface syntax and semantics

Elm's extensible-record design is the reference. What Ipê adopts unchanged:

- **Annotation form.** `{ r | f1 : T1, f2 : T2 }` — a lowercase row variable,
  `|`, then the required fields. The row variable is an ordinary quantified
  type variable naming the open tail. Already parsed and canonicalised.
- **Meaning.** "Any record with at least these fields at these types." Extra
  fields flow into `r`. Within one annotation, every occurrence of `r` denotes
  the *same* residual field set: `{ r | name : String } -> { r | name : String }`
  is shape-preserving.
- **No open construction.** A record literal always has a closed, exact shape.
  There is no syntax to build "a record with at least these fields", and no
  field addition or deletion (Elm removed those; Ipê never had them). Record
  update `{ base | f = v }` keeps the exact field set of `base`.
- **Accessors.** `.name` in atom position is a getter value. Applied to any
  record with a `name` field it yields that field.
- **No lacks/presence constraints.** Because fields can never be added, the
  classic `lacks` constraint (needed by Purescript-style row extension) has no
  use. Duplicate fields in one annotation are a static error. This keeps the
  row algebra the simple Elm subset.

Divergences from Elm, all forced by concrete Rust codegen, recorded here and to
be mirrored in `docs/divergences-from-elm.md`:

| # | Elm | Ipê | Why |
| --- | --- | --- | --- |
| D1 | One compiled function; JS property access is dynamic | One rustc-generic function per row-poly def; rustc emits one machine copy per record shape used | No dynamic field lookup exists in the emitted Rust; project rule: concrete monomorphised codegen, never `dyn Any` / reflection |
| D2 | `.name` is one polymorphic runtime value | Each `.name` *occurrence* desugars to its own getter lambda and is typed independently; an occurrence pins monomorphically unless its context carries a row annotation | Keeps the accessor on the ordinary deferred-field-access path; no new node anywhere (`parser.rs:1267`) |
| D3 | An unannotated function used at two record shapes generalises | Unannotated bindings still pin on first concrete use; row polymorphism is opt-in **via annotation** | Preserves the pinned-records ADR's monomorphic-env pinning and its tripwire fixtures unchanged |

D2's practical consequence: `let f = .name` used at two shapes without an
annotation stays a type error (shape mismatch), while
`f : { r | name : String } -> String` + `f = .name` is accepted. That is the
documented, teachable rule: *polymorphic reuse of one binding requires the row
annotation*.

## 3. Type system

The solver already models everything required; this section fixes the intended
semantics so the lowering can rely on it.

**Unification.** Open/closed record unification stays as shipped
(`unify.rs:246` and the empty-record sentinel handling at `:335–:433`,
faithful to the reference's four cases). Closed×closed demands exact field-set
equality; open×closed pins the open side's tail to the closed remainder (and
fails if the open side demands a field the closed record lacks); open×open
bridges through a fresh tail.

**Rigidity.** Inside the body of an annotated def, the annotation's row
variable is rigid, like any annotated type variable: the body may *use* the
guaranteed fields, but any operation that would force the row to contain a
field the annotation does not list is a type error blamed at that operation
("the annotation only guarantees `name`; this use needs `age`"). This is what
makes the backend bound-set computable from the annotation alone (§4): a
well-typed body can only ever touch the annotated fields of a row-typed value.

**Deferred obligations.** `record.field` on a row-typed parameter discharges
against the annotation's field map exactly as it does today
(`resolve_deferred`); a missing field stays the existing no-such-field error
with the record's settled type printed. `{ base | f = v }` on a row-typed base
type-checks when `f` is among the annotated fields (`resolve_one_record_update`,
`types/src/lib.rs:1744`).

**Generalisation.** The row variable is quantified with the def's other type
variables (canon already collects it — `lower.rs:1206`); each call site
instantiates it fresh, so two calls at `{name, age}` and `{name, id}` both
check. No change to the scheme machinery.

**Accessor typing.** Unchanged: the desugared lambda's parameter is inferred.
Under a row annotation the parameter takes the annotated open-record type; bare
occurrences pin at their use site. No accessor-specific typing rule exists or
is added.

**Error surface.** No new type errors. One lowering diagnostic changes
meaning: `IPE-L0131` narrows from "any open row in a signature" to exactly the
not-yet-shipped forms as each increment lands, and retires with the last one
(its explain page rewritten at every narrowing — never advertising an
unshipped form as supported).

## 4. Monomorphisation to Rust — the crux

### 4.1 Chosen strategy: per-field witness traits + rustc generics

A row-polymorphic function lowers to an ordinary **rustc-generic function**
whose row parameter is a fresh generic type bounded by synthesised *field
witness traits* — one tiny trait per field name. rustc then monomorphises each
call site to the concrete record struct, exactly as it already does for
`Func::type_params`. Static dispatch only; no `dyn`, no reflection, no
runtime field lookup.

For each field name `f` that appears in any row bound anywhere in the program,
the backend synthesises once, next to the record structs (illustrative):

```rust
pub trait IpeHasName {
    type Name;
    fn ipe_name(&self) -> &Self::Name;
}
```

and for **every synthesised record struct that has the field** (generic structs
included), an unconditional impl (illustrative):

```rust
impl IpeHasName for RecAgeName {
    type Name = String;
    fn ipe_name(&self) -> &String { &self.name }
}
impl<T1> IpeHasValue for RecValue<T1> {
    type Value = T1;
    fn ipe_value(&self) -> &T1 { &self.value }
}
```

The associated type (rather than baking the field type into the trait) is what
lets one trait serve every field type: impls are type-agnostic and total over
the struct registry, and the *bound* does the type checking. The def

```text
greet : { r | name : String } -> String
```

emits (illustrative)

```rust
pub fn greet<R1: IpeHasName<Name = String> + Clone>(rec: R1) -> String
```

and a body field read `rec.name` emits `(rec).ipe_name().clone()` — or a bare
deref `*(rec).ipe_name()` when the field type is unconditionally `Copy`,
composing with the existing type-directed Copy elision
(`emit_expr.rs:6782` Access arm, the emitter clone/borrow-discipline ADR §3).
A call site `greet(person)` needs no annotation: rustc infers
`R1 = RecAgeName` from the argument, and the impl exists because the struct
has the field.

Why this is the right fit here:

- **It *is* the existing generic path.** The project already emits named type
  variables as rustc generics and lets rustc monomorphise
  (`Func::type_params`, `ir.rs:663–677`). A row variable *is* a named,
  user-written type variable; bounding it with field witnesses is the same
  mechanism with a richer bound. Composition with existing generics is
  therefore free: `{ r | value : a } -> a` emits
  `fn get<T1, R1: IpeHasValue<Value = T1>>(rec: R1) -> T1` (illustrative).
- **The pinned-records ADR survives refined, not repealed.** Every
  `IrType::Record` the backend sees is still closed; the struct registry and
  its exact-key resolution are untouched. The open row never becomes a record
  type in the backend — it is erased to a bounded generic before the registry
  can miss.
- **Bound satisfaction is a theorem of the type checker.** The only bounds
  emitted are field witnesses the unifier has already proven present with the
  right types at every call site (plus `Clone`, §4.4). So exit-0 ⇒ cargo-green
  (the SEAL) rests on the same argument the closed-record path uses.

### 4.2 Rejected alternative: compiler-side specialisation copies

Cloning the def once per concrete call-site shape (`greet__RecAgeName`,
`greet__RecIdName`, call sites rewritten) was considered and rejected:

- it needs whole-program call-graph shape collection and a name-mangling
  scheme, duplicated across incremental recompilation;
- it cannot specialise a row-poly function that escapes as a *value* (passed
  to `List.map`, stored in a binding) without reimplementing exactly the
  instantiation analysis rustc already performs;
- it duplicates bodies in the emitted *source*, hurting readability and
  compile time, where rustc's monomorphisation duplicates only machine code.

The witness-trait design delegates the specialisation bookkeeping to rustc,
which is the same trade the existing generic-function path already made.

### 4.3 IR and lowering changes

- **New IR type leaf** `IrType::RowGeneric(Symbol)` — the row variable in
  type position (parameter, return, nested under `List`/`Maybe`/…). A
  distinct variant, not a reuse of `IrType::Generic`, so every existing
  `Generic` match arm keeps its meaning and the compiler forces each consumer
  to decide the row case explicitly (invalid states unrepresentable; the
  `Record` doc comment's "open rows are intentionally not representable here"
  moves to this variant's contract: *representable, but never as a `Record`*).
- **New func metadata** `Func::row_params: Vec<RowParam>` with
  `RowParam { var: Symbol, fields: BTreeMap<Symbol, IrType> }` — the single
  source of truth for the variable's required fields, in quantification order
  after `type_params` (positional naming `R1, R2, …` mirroring the `T1…`
  discipline of `ir.rs:671`).
- **Lowering** (`split_typed_sig` / `ir_type_from_canon`): a
  `canon::Type::RecordOpen(row_var, fields)` in a *supported* position lowers
  to `IrType::RowGeneric(row_var)` + a `RowParam` entry (fields lowered
  through the ordinary path — nested closed records inside the field types
  still register their shapes). Unsupported positions keep the fail-closed
  `IPE-L0131`, narrowing increment by increment; the `ir_type_from_canon` ICE
  arm (`lower.rs:9511`) stays as the last-resort invariant check.
- **Field access on a row-typed value**: the lowerer already knows the base's
  solved type; when it is a row generic, the Access emission (backend) routes
  through the witness getter instead of the struct field. Subset *patterns*
  on a row-typed parameter lower to getter-based destructuring the same way.

### 4.4 Bounds beyond field witnesses

The emitted bound set for a row generic is: one field witness per annotated
field, **plus `Clone`**, plus whatever `TyBounds`-style obligations the body
adds on the *whole* record (`PartialEq` for `==`, the stringify trait if the
body stringifies it) — mirrored from the existing `BoundSet` mechanism.

`Clone` is included whenever the body's value-semantics emission would clone
the parameter (which is the common case — same rule the emitter already
applies to concrete record uses). Every synthesised record struct is `Clone`
by construction (the derived set, or the hand-written impl for
function-carrying records — `RecordStruct::is_clone`, `lib.rs:442`). Records
whose carrier is *not* Clone cannot currently exist in the registry; if that
ever changes, the lowering adds a fail-closed check ("this record cannot flow
into a row-polymorphic call") rather than risking a post-exit-0 cargo failure.
That guard is pinned by a dedicated test in the first increment.

### 4.5 The hard cases

- **Row-poly fn applied to a record (the base case).** `greet(person)` —
  rustc infers the instantiation from the argument type. Nothing emitted at
  the call site changes.
- **Row-poly fn used as a value.** `List.map greet people` / storing `greet`
  in a binding: the *context's* solved type is concrete (monomorphic pinning
  of the surrounding expression fixes the element shape), so the existing
  `Expr::FuncValue` path emits the function item and rustc resolves the
  instantiation from the target type, exactly as for existing generic
  functions used as values. A context that genuinely cannot pin (storing a
  row-poly function in a record-of-functions field typed with an open row)
  stays fail-closed.
- **Shape-preserving returns.** `touch : { r | name : String } -> { r | name : String }`
  — the return type is the same `R1`; a pass-through body moves the value.
  Legal as soon as return-position `RowGeneric` lowering lands.
- **Record update through a row.** `{ rec | name = v }` where `rec : R1`
  cannot use Rust struct-update syntax (the concrete struct is unknown at emit
  time). The witness trait gains a second method in the update increment:
  `fn ipe_with_name(self, v: Self::Name) -> Self` (each impl rebuilds
  `Self { name: v, ..self }` — illustrative). The update expression emits a
  chained `rec.ipe_with_name(v).ipe_with_age(w)`. Until that increment,
  updates on row-typed bases stay gated.
- **Nested rows.** `{ r | address : { s | city : String } }` — the inner row
  becomes its own generic with the bound chained through the associated type:
  `R1: IpeHasAddress, R1::Address: IpeHasCity<City = String>` (illustrative).
  Deliberately late in the plan: it multiplies bound-rendering cases while the
  annotation is rare.
- **Row-poly calling row-poly.** The caller's rigid row means the callee's
  required fields must appear in the caller's own annotation (the type checker
  enforces this — a rigid row cannot grow), so the caller's emitted bound set
  syntactically covers the callee's and rustc accepts the inner call with no
  extra machinery.
- **Open rows meeting opaque/kernel record types.** The stdlib's opaque
  records (`HttpRequest`, server types) and the kernel cfg open-row schemes
  (Web cfg) are *not* registry structs and get no witness impls. The kernel
  schemes already resolve during solving (pinned-records ADR, mechanism 3)
  and must keep using their existing path; a user row annotation that would
  have to unify with an opaque record type is rejected by the type layer
  today and stays so.

### 4.6 Naming and namespace hygiene

Trait names derive from the keyword-mangled field identifier
(`naming::record_struct_name` discipline): field `name` → `IpeHasName`, with
the same collision-avoidance and disjointness assertions the struct namespace
already gets (`assert_record_structs_disjoint_from_type_namespace`,
`lib.rs:1687`, extended to cover the witness-trait names). Getter methods are
prefixed (`ipe_name`), so they can never collide with a Rust struct field or
an inherent method on a registry struct.

## 5. Scope

**Ships first:**

1. Accessors + row annotations in **argument position**, field **access and
   subset patterns** only, direct calls and pinned function-value uses.
2. Multi-field rows, rows with generic field types, row-poly → row-poly calls.

**Ships later (same design, later increments):** shape-preserving returns;
record update through rows (`ipe_with_*`); nested rows; rows under containers
(`List { r | … }`).

**Non-goals (deliberately excluded):**

- Generalising **unannotated** bindings over rows (the module-boundary
  scheme-promotion ADR's boundary stays; its two rejection fixtures keep
  rejecting — they exercise the unannotated path, which this design does not
  touch).
- Open-record **literals**, field addition/deletion, lacks constraints,
  first-class polymorphic accessor *values* without an annotation (D2).
- Row polymorphism over the kernel cfg schemes or opaque stdlib records.
- Any dynamic representation. There is no fallback path.

## 6. Implementation plan (ordered increments, test-first, each lands green)

Golden re-blessing is cheap and automated; byte-diff churn is never a reason
to reshape an increment. Every increment ends with the full gate: workspace
tests, the clippy deny-set, golden E2E (`ipe` exit-0 ⇒ `cargo build` green ⇒
run-output match), and the SEAL.

1. **Witness substrate + single-field argument-position rows (vertical
   slice).**
   *Failing tests first:* a golden fixture `row_poly_greet` — one
   `greet : { r | name : String } -> String` called with two different
   concrete shapes, expected to exit 0, cargo-build, and print both results
   (today it fails with `IPE-L0131`); unit tests for trait/impl synthesis
   naming, associated-type rendering, impl-per-struct totality, and namespace
   disjointness; a lowering unit test that `RecordOpen` in argument position
   produces `RowGeneric` + `RowParam`; the `is_clone` fail-closed guard test.
   *Change:* `IrType::RowGeneric`, `Func::row_params`, the narrowed lowering
   gate (argument-position single-field only), backend trait+impl synthesis,
   bound rendering, getter-routed Access emission.
   *Gate:* full; flip `row_var_annotation_is_ipe_l0131`
   (`src/ipe-cli/tests/g_record_generic/golden_row_poly_records.rs:326`) into
   the acceptance test; assert `two_different_supersets_is_ipe_t0001` and
   `closed_superset_is_ipe_t0001` still reject.

2. **Multi-field rows, generic field types, row-poly→row-poly, whole-record
   bounds.**
   *Failing tests first:* golden `row_poly_multi` (two required fields, a
   helper row-poly call inside the body, an `==` on the record forcing
   `PartialEq`); a typecheck test that a body demanding an unannotated field
   errors with the rigid-row message; a golden with `{ r | value : a } -> a`
   composing `T1`/`R1`.
   *Change:* bound-set union with `BoundSet`-style obligations; multi-witness
   rendering; subset-pattern destructuring via getters.
   *Gate:* full.

3. **Shape-preserving returns + accessor-annotated bindings.**
   *Failing tests first:* golden `row_poly_passthrough`
   (`touch : { r | n : T } -> { r | n : T }` with pass-through body used at
   two shapes); golden `getName : { r | name : String } -> String` defined as
   `getName = .name` and reused at two shapes.
   *Change:* return-position `RowGeneric` lowering; the accessor lambda under
   a row annotation types its parameter at the open record (no backend change
   expected beyond return rendering).
   *Gate:* full.

4. **Record update through rows.**
   *Failing tests first:* golden `row_poly_update` (`{ rec | name = v }`
   inside a row-poly body, verified at two shapes).
   *Change:* `ipe_with_*` setter method on the witness traits + impls;
   `Expr::Update` emission on a row-generic base chains setters.
   *Gate:* full.

5. **Nested rows + rows under containers.**
   *Failing tests first:* goldens for `{ r | address : { s | city : String } }`
   and `List { r | name : String } -> List String`.
   *Change:* chained associated-type bounds; container rendering of
   `RowGeneric`.
   *Gate:* full.

6. **Retire the gate; docs.**
   *Change:* remove `IPE-L0131` from the reachable diagnostic surface (the
   explain page is rewritten at each earlier narrowing so no increment ever
   advertises an unshipped form); supersede the pinned-records ADR with a
   successor stating the refined invariant — *every `IrType::Record` reaching
   the backend is closed; open rows reach it only as witness-bounded
   generics*; update `docs/divergences-from-elm.md` (D1–D3) and the record
   chapters of the language docs.
   *Gate:* full + docs-sync sweep.

## 7. Risks and cost

- **SEAL (highest).** Any emitted bound the type checker did not prove =
  exit-0-then-cargo-fail. Contained by construction: witness bounds mirror
  proven unifications; `Clone` is total over the registry; every other
  whole-record bound reuses the proven `BoundSet` path. The first increment's
  guard test pins the containment.
- **Bound-rendering complexity.** Associated-type bounds, chained nested-row
  bounds, and `T`/`R` interleaving are the fiddly emission surface — the
  reason nested rows are isolated in their own late increment.
- **Impl fan-out.** One trait per row-used field name, one impl per
  (field, struct-having-it) pair. Bounded by the program's own annotations;
  no reachability analysis needed for correctness, and pruning to structs
  that actually flow into rows is a later efficiency refinement if emitted
  source size ever matters.
- **rustc monomorphisation blowup.** One machine copy per (row-poly fn ×
  used shape) — identical in kind to the existing generic-function cost, and
  strictly smaller than the source-level duplication the rejected alternative
  would emit.
- **Error-message quality.** The rigid-row "annotation only guarantees these
  fields" blame and the no-such-field message on row-typed bases must stay as
  teachable as the closed-record ones; the second increment carries the
  dedicated tests.
- **Interaction with kernel cfg rows.** The Web cfg open-record schemes must
  never route into witness synthesis; they resolve during solving. A misroute
  would surface as a spurious `RowParam` — the first increment's lowering
  unit tests pin the boundary.
- **Function-value edge.** A row-poly function stored where no context pins
  its shape stays fail-closed; if user demand appears, it becomes a scoped
  follow-up (explicit instantiation), never a `dyn` fallback.

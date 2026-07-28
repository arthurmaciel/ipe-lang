# Row polymorphism (337) — general user-facing rows + first-class record accessors

Status: design + implementation plan. Increment 1 (accessor) landed; Increment 2
front half (parser + AST + canon + `from_canon` + type layer + `IPE-L0131`
lowering gate) landed; Increment 2 backend monomorphisation deferred.

## Goal (mirror Elm)

Two user-facing surfaces, both standard in Elm:

1. **First-class accessor.** `.field` is a value of type `{ r | field : a } -> a`.
   Enables `List.map .name people`, `f .x`, etc. This directly unblocks the
   `f .x` accessor request (issue 335).
2. **Row-polymorphic annotation.** `greet : { r | name : String } -> String`
   accepts any record carrying *at least* those fields.

## Current state (verified against the tree, not re-derived)

The **type layer already models rows correctly** and is not the blocker:

- `RowTail::Closed` / `RowTail::Open(u32)` on `Ty::Record`
  (`src/compiler/types/src/ty.rs`); a faithful `unifyRecords` port; deferred
  field access (`FieldAccess`) and open-record growth in `resolve_deferred`
  (`src/compiler/types/src/lib.rs`).
- Open records are *produced* today only for three kernel schemes
  (`Web.app` / `Tui.app` / `Tui.program`) via `RowTail::Open(3)` in
  `constrain.rs`.

The **end-to-end gaps** (the work) are:

| Layer | Gap | File |
|---|---|---|
| Parser — accessor | bare `.field` at atom position rejected at parse | `parse/src/parser.rs` (`parse_atom`) |
| Parser — annotation | `{ r \| f : T }` rejected — only closed records parse | `parse/src/parser.rs` (`parse_record_type`) |
| `TypeAnnotation` AST | no row-var record arm | `syntax/src/ast.rs` |
| `canon::Type` | no row-var record arm | `canon/src/ast.rs` |
| `from_canon` | hardwires `RowTail::Closed` for ALL user annotations | `types/src/ty.rs` |
| Backend | emit of a genuinely row-poly (unpinned) function — ADR-0018 rejects it | `backend/rust/src/lib.rs` (`canonicalise_shape`) |

## The backend design decision (called out explicitly)

**How does a row-polymorphic function emit?** Weighed against PRINCIPLES
(soundness > completeness) and the project memory *prefer concrete/monomorphized
codegen over generic*:

**Decision: monomorphize per call-site (pin at the call boundary), NOT a Rust
generic / trait bound.** Each concrete use of a row-poly function supplies a
concrete record; that record shape reaches the backend and resolves through the
existing A7 exact-sorted-field-set struct registry. This is exactly what ADR-0018
mechanism 4 ("monomorphic env pinning") already does for *unannotated* getters
(`row_poly_subset_access` proves it end-to-end). A Rust generic over a
row-variable would require synthesising a trait per accessed-field-set — more
machinery, worse readability, and it buys nothing the pinning path lacks for the
cases the surface can express.

### Two increments, split by ADR-0018 exposure

- **Increment 1 — the accessor `.field` (THIS LANE).**
  `.field` **desugars at parse time** to `\<fresh> -> <fresh>.field`, i.e.
  `Lambda([PVar(fresh)], Access(VarLocal(fresh), field))`. This introduces **no
  new type, canon, or backend node** — it reuses the fully-proven `Access` path:
  deferred field access (mechanism 2) makes the subset access legal by
  construction, and monomorphic env pinning (mechanism 4) pins the record
  parameter to the one concrete shape at each call site. `List.map .name people`
  pins `.name`'s parameter to `people`'s element shape, which reaches the backend
  concrete. **ADR-0018's invariant is untouched** — an accessor is just sugar for
  a getter the ADR already blesses. This is why the accessor lands first and
  fully.

- **Increment 2 — the `{ r | f : T }` annotation (FOLLOW-UP).**
  Parsing + `from_canon` producing `RowTail::Open` are mechanical.
  The hard part is a row-poly function that is *called at two different superset
  shapes within one module* — `row_poly_two_supersets_neg` currently rejects
  exactly this (proof row P6), and flipping it to accept **requires
  per-record-shape callee monomorphisation in the backend** (emit one specialised
  copy of the callee per distinct concrete record shape at its call sites, each
  resolving through the A7 registry). Until that machinery lands, accepting the
  annotation syntax would be a SEAL violation (exit-0-then-cargo-fail via an A7
  exact-key miss). So the annotation increment is gated on backend
  monomorphisation and is deferred.

### ADR-0018 stance

**The invariant is preserved, not amended, by Increment 1.** "Every record
reaching the backend has a fully-pinned concrete field set" still holds: the
accessor desugars to a getter whose parameter pins monomorphically. No tripwire
fixture flips.

**Increment 2 will require an ADR-0018 amendment** — specifically adding
mechanism (b) from its own "ADR-0008 coupling tripwire" consequence:
per-record-shape callee monomorphisation. Only *then* may
`row_poly_two_supersets_neg` be flipped to accept. This lane does **not** flip
it; it stays a rejecting tripwire.

## Implementation plan (TDD, bite-sized)

Illustrative fenced blocks below are marked; only commands actually run are
unmarked.

### Step 1 — accessor parser (red → green)

- Test (parser unit): `.name` at atom position parses to
  `Lambda([PVar(_)], Access(VarLocal(_), name))`; `List.map .name xs` parses;
  `f .x` parses. A leading `.` followed by a non-ident still errors.
- Impl: in `parse_atom`, when the next token is `Tok::Dot` at atom position
  (leading dot, no preceding expr), consume `.` + ident, mint a fresh param
  symbol via the interner, and build the desugared lambda. Handle a dotted chain
  `.a.b` as nested `Access` exactly like the postfix path does.
- Fresh symbol: use an interner-minted name that cannot collide with a source
  binder (leading char illegal in source identifiers).

### Step 2 — accessor end-to-end golden (red → green)

- New golden fixture `tests/golden/row_poly_accessor` (`Main.ipe`):
  `List.map .name people` over `{ name, age }` records; `IPE_E2E=1` prints the
  names. Assert `ipe build` accepts and emits a concrete getter; assert the
  emitted Rust `cargo build`s (SEAL).
- Wire into `golden_row_poly_records.rs` matrix (new accept row).

### Step 3 — annotation parser + AST (LANDED)

- `TypeAnnotation::TRecordOpen(row_var: Symbol, fields)` arm;
  `parse_record_type` reads an optional `<lowerVar> |` prefix (two-token
  `{ <lowerVar> | … }` lookahead), mirroring Sky's `Sky.Parse.Type`
  open-record arm.
- `canon::Type::RecordOpen(Symbol, Vec<(Symbol, Type)>)` + canonicaliser
  lowering (the row var is quantified like a field type variable).
- `from_canon` produces `RowTail::Open(row_var.as_raw())` for the open arm.
  `instantiate_in` already freshens the open tail per use site, so the
  annotation type-checks with the reference's open-row semantics.
- `row_poly_annotation_gap` now flips from a PARSE reject (`IPE-P0001`) to a
  LOWERING reject (`IPE-L0131`): the annotation parses and type-checks, and is
  failed closed at the layer that cannot yet emit. This is NOT the full flip —
  the program still does not build. The whole-chain flip (accepting + building)
  waits on Step 4.

### Step 4 — backend per-shape monomorphisation (follow-up, DEFERRED)

Gated at lowering by `IPE-L0131` (`Feature::RowPolyRecordAnnotation`), raised by
`canon_type_has_open_row` at the signature boundary in `ipe_lower::lower`. The
remaining work:

- Emit one specialised callee copy per distinct concrete record shape observed at
  call sites; each resolves through the A7 registry. Today `split_typed_sig`
  lowers each annotated parameter from the ANNOTATION (`ir_type_from_canon`),
  which for an open row would key the struct registry on the annotation's SUBSET
  field set and miss the concrete superset — the A7 exact-key miss. The pass must
  instead lower a row-poly parameter to the CONCRETE solved shape at each call
  site and clone the callee per distinct shape.
- Amend ADR-0018 (add mechanism (b): per-record-shape callee monomorphisation)
  and only THEN flip `row_poly_two_supersets_neg` + drop the `IPE-L0131` gate.
- Until then the two rejection tripwires keep rejecting and no ADR amendment is
  made.

## Guards

- SEAL: accessor path emits a concrete getter (proven-shape struct), so
  exit-0 ⇒ cargo build. The open annotation never reaches emit — it fails closed
  at lowering (`IPE-L0131`), so no unbuildable Rust is produced.
- Golden suite unchanged except the accessor fixture (Increment 1) and the
  `row_poly_annotation_gap` fixture's flip from `IPE-P0001` to `IPE-L0131`
  (Increment 2 front half).
- ADR-0018 tripwires (`row_poly_two_supersets_neg`, `row_poly_closed_superset_neg`)
  keep rejecting as `IPE-T0001`; `row_poly_annotation_gap` now rejects at
  lowering (`IPE-L0131`) instead of parse. No tripwire flips to ACCEPT and no
  ADR-0018 amendment is made — that waits on Step 4's backend monomorphisation.

# Row-poly subset/superset record resolution — verification & gate (#56, A7 watch)

> Backlog item #56: "Prove row-poly subset/superset record resolution (A7
> watch) + gate on sweep". Investigated 2026-07-10 by direct probe against
> both compilers (ipê `skyc` @ `a58412e`, reference `sky v0.16.29`).
> **Verdict: no defect found.** Every row-polymorphic subset/superset shape
> reachable through ipê's surface today resolves correctly end-to-end
> (skyc-accept → cargo-build → run → reference-identical output) or is
> rejected with the same verdict the reference gives (fail-loud parity).
> One completeness gap vs the reference was found and filed: the row-var
> record **annotation syntax** `{ r | f : T }` does not parse (SKY-P0001).
> This spec records the semantics, the proof matrix, the A7-guard safety
> argument, the class-1 coupling tripwire, and the gate tests that pin all
> of it on the sweep. Implementation (the 5 golden-test fixtures below) is
> NOT part of this spec — it is a separate, has-spec BACKLOG.md item for the
> next swarm round.

## Problem statement

Divergence **A7** (`docs/divergences-from-sky.md`): ipê resolves every
record shape reaching the backend by **exact sorted-field-set match** and
raises a `CompilerBug` on a miss
(`crates/sky_backend_rust/src/lib.rs::record_struct_by_key`); the
reference's Rust backend instead widens to the best **superset** row and
falls back to `"String"` on a miss. The A7 rationale (soundness >
completeness) is only valid if no legal row-polymorphic program can make a
record reach the backend with a field set the synthesised-struct registry
does not carry — i.e. if the "subset function meets superset record" class
either (a) resolves to fully-pinned concrete shapes before lowering, or
(b) is rejected at type-check. #56 is the proof obligation for that
invariant, plus a sweep gate so a future change (most plausibly the class-1
"Boundary Scheme Promotion" generalization work) cannot silently break it.

## The mechanisms (how row-poly actually flows through ipê)

Row polymorphism enters the pipeline through **four distinct mechanisms**,
none of which is the annotation syntax:

1. **Open-record unification** — `crates/sky_types/src/unify.rs`
   (`FlatType::Record(fields, ext)` arm) is a faithful port of the
   reference's `unifyRecords` (`../sky/src/Sky/Type/Unify.hs:468-512`),
   four cases: shared fields unify pairwise; a CLOSED side (ext =
   `EmptyRecord` sentinel) cannot absorb the other side's extras →
   SKY-T0001; identical field sets merge and unify tails; two open sides
   with differing extras merge as the field-map union under a fresh flex
   tail.
2. **Deferred field access** — `rec.field` and the field-pun record
   pattern `{ x, y }` (`constrain.rs` `PRecord` arm) do NOT force an
   exact-shape unification; each pulls the field out through the deferred
   `FieldAccess` channel resolved after the main solve, so a subset
   pattern/access over a wider record is legal by construction.
3. **Open-record kernel schemes** — row-open cfg records the reference
   expresses with an `appExt` row var in stdlib HM signatures (`Live.app`
   cfg with optional `head`/`consoleAuth` fields, routed-vs-non-routed
   Model) are mirrored in ipê's constrain scheme table as open records plus
   deferred checks (`RoutedLiveCheck`, resolved post-solve). Covered by the
   existing `golden_m7_live_*` gates.
4. **Monomorphic env pinning** — an unannotated binding is monomorphic
   (one inference var per home module; class-1 spec §Semantics), so all its
   use sites share one record shape. The open tail introduced by a field
   access is **pinned by the first concrete use** and every record type
   that survives to the lowerer carries a fully-solved concrete field set.

At lowering, `ir_type_from_ty` (`crates/sky_lower/src/lower.rs`) drops the
solver's row tail — "the optional-field mechanism works through the open
row var at type-check time, not at codegen" — and the emitter resolves
struct names by exact field set. For record **patterns** that bind only a
subset of the scrutinee's fields, the lowerer completes the pattern to the
full solved field set before emission
(`lower.rs` record-pattern completion; the emitter then renders
`RecAgeName { age: _, name, .. }` — see `render_record_pat`,
`emit_expr.rs`).

## Proof matrix (empirical, both compilers, 2026-07-10)

| # | Shape | ipê `skyc` | reference `sky v0.16.29` | Verdict |
|---|---|---|---|---|
| P2 | Unannotated `getName rec = rec.name`, called once with superset `{ name, age }` | accept; cargo builds; prints `Ada` | accept; prints `Ada` | **parity, end-to-end** |
| P5 | Subset `case` record pattern `{ name }` over `{ name, age }` scrutinee | accept; emits `RecAgeName { age: _, name, .. }`; prints `Ada` | accept; prints (same) | **parity, end-to-end** |
| P7 | Subset lambda record pattern through a HOF: `List.map (\{ name } -> name) [{name, age}, …]` | accept; cargo builds; prints `Ada, Bo` | accept | **parity, end-to-end** |
| P4 | CLOSED annotation `{ name : String }` called with superset `{ name, age }` | reject SKY-T0001 | reject E2001 (extra `age`: "expected <missing>, actual Int") | **parity, fail-loud** |
| P3 | Unannotated fn called with TWO different supersets (`{name,age}` then `{name,id}`) | reject SKY-T0001 | reject E2001 (`expected { age : Int, name : String | ...}`) | **parity, fail-loud** — the reference does not generalize unannotated top-levels over the row either |
| P6 | Same as P3 but let-bound lambda | reject SKY-T0001 | reject E2001 | **parity, fail-loud** — no let-generalization over rows in either compiler |
| P1 | Row-var ANNOTATION `getName : { r | name : String } -> String` + superset call | **reject SKY-P0001** (`found '|', expected ':'`) | **accept**, monomorphises the callee ("Monomorphisation: 1 instances"), prints `Ada` | **completeness gap** — filed, see §Gap below |

Corpus impact of P1: **zero** row-var annotations exist in the upstream
example corpus and Sky-source stdlib (`rg` over `../sky/examples`,
`../sky/sky-stdlib`, `../sky/sky-bundled`: every `{ x | … }` hit is a
record-update expression). The reference's own row-open stdlib signatures
live in kernel schemes, which ipê mirrors natively (mechanism 3). The gap
is therefore **not sweep-blocking**.

## A7-guard safety argument (why the exact-key miss is unreachable)

For the exact-key lookup to miss on the row-poly class, a record would have
to reach the backend with a field set never surfaced to the struct
registry. The proof matrix closes every entry road:

* Record **literals** always carry their full concrete field set, surfaced
  by `collect_record_types` from the solver's region/env types.
* Record **patterns** binding a subset are completed by the lowerer to the
  scrutinee's full solved field set before `render_record_pat` resolves the
  struct (P5/P7 — the emitted pattern names the superset struct with `..`).
* A **function parameter** can only be record-typed via (a) a closed
  annotation — exact set, P4 rejects superset args; (b) inference — the
  monomorphic env var pins ONE shape across all uses (P3/P6 reject a second
  shape), and the single pinned shape is fully concrete at lowering; or (c)
  a row-var annotation — unreachable, SKY-P0001 (P1).
* **Kernel-scheme** open rows (Live cfg) never materialise as synthesised
  structs; they are consumed structurally by the Live emitters.

Hence today's invariant: **every record type reaching the backend has a
fully-pinned concrete field set**, and the A7 fail-loud guard stays
correct as-is. The reference-style superset-widening fallback remains
unneeded (and unwanted: it is the exit-0-then-cargo-fail shape THE SEAL
forbids).

## Coupling tripwire — class-1 Boundary Scheme Promotion

The safety argument leans on mechanism 4 (monomorphic pinning). The
class-1 work (`docs/architecture/class1-inference-fix-spec-2026-07-09.md`)
introduces **module-boundary generalization** of unannotated bindings. If
an unannotated `getName rec = rec.name` is exported unused and generalized
with an **open row** in its scheme, two importing modules could
instantiate it at two different record shapes — exactly the state the
exact-key registry cannot represent (one Rust fn, two param structs; a row
var cannot become a plain Rust generic because field access on it needs a
trait or per-shape monomorphisation). The class-1 implementer MUST either:

* generalize only genuine type VARS and keep record-row tails
  monomorphic/pinned at the module boundary (matching what the reference
  observably does within a module — P3), or
* add per-record-shape callee monomorphisation to the backend first (the
  reference's "Monomorphisation: N instances" pass) — the same machinery
  the P1 annotation gap needs.

The gate tests below are the tripwire: the two rejection fixtures
(`row_poly_two_supersets_neg`, and the cross-module variants in the
class-1 test matrix) must keep rejecting until the backend can
monomorphise per shape; flipping them to accept without that machinery
reintroduces the A7 miss as an ICE (best case) or an emitted-code type
error (seal violation).

## Gap filed — row-var record annotation syntax `{ r | f : T }`

* **Reference behaviour:** parses (`../sky/src/Sky/Parse/Type.hs`
  `peekRowPolyIntro` → `Src.TRecord fields (Just rowVar)`), types the row
  var as part of the scheme, and monomorphises the callee per record-shape
  instantiation in the Go backend.
* **ipê behaviour:** SKY-P0001 at parse — fail-loud, no unsound
  acceptance path.
* **What a correct implementation needs (in order):**
  1. Parser: admit the `lowerVar |` row intro in type-record position
     (`sky_parse`), carrying the row var on the record type node
     (`sky_syntax` needs the ext slot).
  2. Canon: bind the row var like a free annotation type var
     (`sky_canon` type resolution).
  3. Constrain: instantiate the annotation as
     `Ty::Record(fields, RowTail::Var(r))` — the unifier (mechanism 1)
     already handles everything from there.
  4. Backend: per-record-shape callee monomorphisation (Rust generics
     cannot express row field access) — THE design decision, shared with
     the class-1 tripwire above. Until it exists, steps 1-3 alone would
     accept programs the backend cannot emit — a seal violation — so the
     syntax must stay fail-closed.
* **Filed as** the `#56b` backlog row (Post-completion; corpus-unused,
  non-sweep-blocking). The `row_poly_annotation_gap` canary test asserts
  SKY-P0001 today and will fail the moment the syntax starts parsing,
  forcing whoever lands it to re-read this spec (and the ledger) before
  shipping.

## Test plan (the sweep gate) — `crates/skyc/tests/golden_row_poly_records.rs`

Fixtures under `tests/golden/` (NOT yet created — this is the plan for the
next has-spec swarm round, not part of this design pass):

| Fixture | Gate | Asserts |
|---|---|---|
| `row_poly_subset_access` | accept | skyc exit-0; emitted code resolves the superset struct; `SKY_E2E=1`: cargo builds + prints `Ada` (hand-verified reference oracle, documented in-test) |
| `row_poly_subset_pattern` | accept | skyc exit-0; emitted `main.rs` contains the completed superset struct pattern (`{ age: _, name, .. }` shape) proving the A7 exact-set resolution path; `SKY_E2E=1`: prints `Iri: Ada, Bo` (reference oracle) |
| `row_poly_closed_superset_neg` | reject | pipeline diagnostic SKY-T0001 (reference parity: E2001) — never a panic/ICE |
| `row_poly_two_supersets_neg` | reject | SKY-T0001 (reference parity: E2001) — the class-1 tripwire |
| `row_poly_annotation_gap` | reject | SKY-P0001 — the completeness-gap canary (see §Gap) |

All five run in the default `cargo nextest` suite (the two E2E run bodies
are `SKY_E2E=1`-gated per house convention, so the sweep exercises the
full seal). No compiler code changes needed for #56 itself: it is
verification + hardening (5 regression fixtures), matching the reference
by observation, not by assumption.

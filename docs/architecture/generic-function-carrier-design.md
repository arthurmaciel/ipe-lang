# Function values crossing generic call boundaries: carrier design

Status: accepted design; implementation staged (gate first, carrier
propagation second).

Companion to `docs/architecture/tbd/first-class-functions-design.md`, which
designs function *storage* in concrete composites. This document covers the
seam that design cannot see: a function value instantiating a **declared
type variable** of a generic user function.

## 1. The gap

The motivating program (the `function_field_gate` soundness fixture,
`tests/golden/function_field_gate/Main.ipe`):

```
wrap : a -> { value : a }
unwrap : { value : a } -> a
main = let f = unwrap (wrap (\n -> n + 1)) in ...
```

The emitted shape (illustrative sketch, not a runnable block):

```rust
pub fn main_wrap<T1: Clone>(x: T1) -> RecValue<T1> { ... }
pub fn main_unwrap<T1: Clone>(r: RecValue<T1>) -> T1 { (r).value.clone() }
```

The call instantiates `T1 = Box<dyn Fn(i64) -> i64 + Send + Sync + 'static>`,
which is not `Clone` — E0277 at monomorphization. `ipe` exits 0, `cargo`
fails: a SEAL breach. The record is incidental; the same breach hits any
bare-variable slot (`always : a -> b -> a` applied to a lambda), because the
backend injects `Clone` on **every** emitted type parameter
(`render_fn_generics`, `src/compiler/backend/rust/src/emit_expr.rs`: field
reads emit `.clone()`, so every `T{n}` must be cloneable) and the
clone/borrow discipline conservatively clones every `Generic`-typed field
read (ADR 0011 §3).

**The broken class, precisely:** a value whose type embeds a function, bound
at a call site to a callee's declared *bare* type variable (not a declared
arrow), for a callee emitted as a Rust generic.

## 2. Why no local seam can fix it

Every existing carrier decision is made from a type visible at one site:

- **Type side, declaration-local:** `normalize_record_fun_carriers` /
  `normalize_enum_payload_fun_carrier` (`src/compiler/lower/src/lower.rs`)
  flip a `Fun` that is *syntactically present* in the declared field/payload
  type to `SharedFun` (`Arc<dyn Fn>`). A generic record's field is
  `IrType::Generic` — there is no `Fun` to flip; the synthesised
  `RecValue<T1>` cannot carry the decision.
- **Value side, construction-local:** `promote_fn_field_value_carrier` /
  `promote_ctor_arg_fn_carrier` re-carrier an argument whose *solved region
  type* is a direct arrow filling a slot the type side flipped. At
  `wrap (\n -> n + 1)` the argument's slot is the callee's `a` — the type
  side flipped nothing, so the value side correctly leaves `Box`.
- **Region-based gate:** `reject_function_through_type_var` consults
  `region_ty(span)` — but regions are already **monomorphized**
  (`{ value : Int -> Int }`); the generic origin is erased. A direct record
  fn field is a shape the record storage rule made legal, so the gate
  correctly stays silent — it cannot distinguish "field declared
  `Int -> Int`" (flipped, sound) from "field declared `a`, instantiated
  `Int -> Int`" (unflipped, broken).

The correctness condition spans four sites at once: the caller (carrier of
the argument), the callee's emitted signature (the `Clone` bound), the
callee's body (the field-read clone), and every downstream consumer of the
result (carrier of the extracted value). Consistency across a call boundary
is a whole-program property; no rewrite at any single one of those sites can
restore it. That is why this seam is a design pass, not a patch on the
construction-site carrier family.

## 3. Candidate approaches

### A. Carrier decision at the generic-call boundary (lowerer)

One new total rule, the generic-boundary completion of the storage design's
carrier normalization: **a type variable instantiated to a fn-embedding type
instantiates on the `Arc` carrier (`SharedFun`), everywhere.** Totality is
what makes it sound — every call site anywhere that performs the same
instantiation agrees by construction, so no two-carrier `E0308` between
generic calls can arise.

Two independently landable layers:

**A1 — fail-closed gate.** At each direct call to a user def
(`lower_call_uniform`'s `VarTopLevel` arm and the partial-application /
over-application reshapes), recover the callee's *declared* signature
template: the canon annotation (`canon::Def::Typed { ty, free_vars }`) or,
for an unannotated def, the solved scheme in `types.env[(home, name)]` —
both already held by the lowerer, no new pass. Match each declared parameter
template against the argument's solved region type (the backend's
`match_template` is the model), yielding a substitution `var -> Ty`. If any
binding satisfies `ty_contains_fun`, reject with
`Feature::FirstClassFunctions` (IPE-L0107) and a message naming the generic
boundary ("a function value instantiates the callee's type variable `a`;
generic slots cannot yet carry functions").

*Narrowness (the HOF question).* Higher-order functions are safe by the
matching itself, not by heuristics: for `apply : (a -> b) -> a -> b` the
function argument matches the declared *arrow* `(a -> b)`, binding
`a := Int, b := Int` — no variable binds an arrow, nothing is flagged, and
the emitted parameter is the already-supported `Box<dyn Fn>` /
`FN{i}: Fn(..)` form. Only a variable bound to a fn-embedding type is
flagged, and every such instantiation is cargo-broken today (the
unconditional `Clone` bound), so the gate rejects exactly the
currently-failing set: zero over-rejection by construction, provided the
template is faithful (risks §6).

This is the shipped precedent's shape: `lower_update` rejects a generic
record update (`Feature::BoundedRecordUpdate`, IPE-L0111) for the identical
reason — the emitted copy needs a `Clone`-bounded parameter the backend
cannot yet discharge. Same posture: a clean diagnostic, never broken Rust.

**A2 — build + run via `Arc` instantiation.** Where A1 would reject, instead:

1. **Flip the argument.** A fn-valued argument bound to a variable re-carriers
   to `Arc` exactly as `promote_ctor_arg_fn_carrier` does at enum
   constructors: literals via `promote_fn_field_value_carrier`
   (`Lambda -> SharedLambda`, `FuncValue` re-stamped), non-literal leaves via
   the eta wrapper (`SharedLambda` over fresh binders applying the leaf).
2. **Re-stamp the result.** Apply the flipped substitution to the callee's
   declared return template and stamp the call expression with it, so
   `unwrap (wrap f)` is known `SharedFun`, not region-derived `Fun`.
3. **Propagate downstream, syntax-directed.** A binder whose initializer
   carries a flipped type takes the initializer's type over the
   region-derived one; `Apply` on a `SharedFun` value and `.clone()` on an
   `Arc` field are the already-shipped fn-value-reuse paths
   (`fun_value_arc_promotable`, `src/compiler/ir/src/ir.rs`; the backend
   reconcile). No fixpoint: propagation is per-def over the lowered tree.
4. **Close the frontiers.** A flipped value reaching a `Box`-declared slot
   (a def's declared `Fun` return, a kernel `Fun` parameter) takes the O(1)
   `Arc -> Box` adapter (`Box::new(move |..| shared(..))`); the reverse
   direction is construction-site re-stamping, already total.

Emitted effect on the motivating program: signatures unchanged; the call
constructs the argument with `Arc::new`; `T1` becomes
`Arc<dyn Fn(i64) -> i64 + Send + Sync>`, which satisfies `Clone`;
`unwrap`'s `.clone()` is a refcount bump; the extracted value calls through
the shared-fn path; output `42`.

Anything outside the frontier subset A2 has proven falls back to A1's
rejection — fail-closed at every widening step, never a silent carrier
mismatch.

### B. Emitter move-vs-clone + `Clone`-bound minimization (backend)

Attack the *reason* the bound exists instead of the carrier:

1. **Move out of owned aggregates.** `main_unwrap` takes `r` by value; its
   sole consuming read could emit `(r).value` (a move) instead of
   `.clone()`. This extends the ADR 0002 last-use move analysis from
   variable reads to field projections of owned aggregates, and ADR 0011 §3's
   copy elision from "provably-`Copy` types" to "provably-last-use reads".
2. **Bound inference.** Stop injecting `Clone` unconditionally in
   `render_fn_generics`; emit it per type parameter only when required: a
   surviving clone of a value mentioning the parameter, a kernel/derive
   demand, or — transitively — a call forwarding the variable to another
   generic function that requires it. An interprocedural fixpoint over the
   call graph (the `BoundSet` plumbing exists; today it is populated
   locally).

With both, the motivating program emits an unbounded `fn main_wrap<T1>(..)`
and a moving `fn main_unwrap<T1>(r) -> T1` body, and builds at
`T1 = Box<dyn Fn>` — the leanest possible output: no `Arc`, no adapters, no
refcounts.

**Why B cannot stand alone:** a program that genuinely duplicates the
generic value — reads the field twice, copies a `Clone`-derived composite —
still needs the bound, so a fn instantiation of that parameter must *still*
be rejected or `Arc`-flipped. B shrinks the broken class; only A closes it.
And B's machinery lives in the seal-critical move/clone discipline: every
dropped clone is a potential E0382, every wrongly dropped bound a new E0277,
and the bound fixpoint is the only genuinely new whole-program analysis in
either approach. Corpus-wide signature and body changes follow (re-blessing
is cheap; *reviewing* move-safety across every changed body is not).

### Interaction with the existing gates and carrier family

- A1 extends the fail-closed family (L0107/L0111/L0114): same
  `unsupported(span, Feature)` mechanism, same never-broken-Rust contract.
  The soundness fixture (`tests/golden/function_field_gate`,
  `src/ipe-cli/tests/g_fn_pattern/golden_function_field_gate.rs`) pins
  IPE-L0107 as the accepted rejection.
- A2 completes the carrier family's philosophy: declared types decide
  concrete storage (`normalize_*`), solved arrows decide construction sites
  (`promote_*`), instantiation shape decides generic boundaries (new). All
  three are total functions of types at their seam — no containment or
  escape analysis.
- `reject_function_through_type_var` keeps its current duties (payload and
  collection regions); the new gate covers exactly what region
  monomorphization hides from it.
- B leaves the gates in place and later *narrows their firing*: a parameter
  proven bound-free never triggers the flip or the rejection. Compounding,
  not conflicting.

## 4. Recommendation

**A, staged A1 then A2; B deferred as an independent efficiency effort.**

Under the principle order: Security is untouched by all candidates.
Correctness/Soundness demand the SEAL — today's accept-then-cargo-fail is
the violation, and a *wrong* carrier decision (two sites disagreeing) is the
miscompile-adjacent class that must never ship. Over-rejection costs only
Completeness, and the matching-based gate provably rejects only programs
that already fail to build. So:

- **Minimum sound outcome — A1.** Small, lowerer-local, zero carrier risk,
  restores the SEAL for the whole class with a teaching diagnostic. Cost:
  the motivating program is rejected, not run.
- **Better outcome — A2.** Makes the class build and run at the price of the
  instantiation-substitution plumbing and frontier adapters, landed
  subset-first with A1 as the permanent fallback. Cost: refcount on flipped
  values (bounded, matches the storage design's economics) and the
  propagation machinery.
- **B later.** Real efficiency value independent of functions (deep-copy
  elision), but wrong first move: highest analysis risk, touches the seal
  discipline, and still needs A's gate for the residual class.

## 5. Implementation plan (test-first; each step lands green)

Every step: failing test first → minimal change → clippy + full nextest +
the E2E gate fixtures + golden-corpus byte-diff. Carrier work is
guardian-gated: the security-soundness review must see the
single-carrier-per-instantiation argument, and the adversarial reviewer
builds the emitted crates independently.

- **Fixture lattice (tests only).** The existing
  `function_field_gate` / `function_payload_gate` goldens pin
  reject-or-run. Add: a bare-variable shape (`always`/`id` applied to a
  lambda — same E0277, no record), a collection shape (variable
  instantiated to `List (Int -> Int)`), and the **over-rejection
  tripwires**: `apply : (a -> b) -> a -> b`, function composition, and a
  generic-record call at a *non-function* instantiation — all must stay
  accepted and running. The tripwires gate every later step.
- **Gate step (A1).** Failing state: gate fixtures cargo-fail under the E2E
  run. Change: template recovery (annotated + unannotated callee) +
  declared-param/region matching + `Feature::FirstClassFunctions` rejection
  in the direct-call, partial-application, and over-application paths. Exit
  gate: all fixture-lattice rejections are IPE-L0107, tripwires untouched,
  golden corpus byte-identical (the gate only rejects programs that had no
  golden), examples sweep shows zero new rejections.
- **Substitution plumbing.** Unit tests on the lowerer: computed
  substitution for annotated, unannotated, alias-unfolded, and
  `any`-wildcard callees; declared-arrow (HOF) bindings never flagged.
  Pure refactor of the gate's matcher into a reusable instantiation map; no
  behavior change (corpus byte-identical).
- **Contained flip (A2 core).** Failing test: the field-gate fixture
  graduated to a positive golden expecting `42` (the fixture's `Ok` branch
  already asserts it). Change: argument flip + result re-stamp + binder
  takeover, enabled only where the flipped result stays within the def
  (field access, apply, let). Everything escaping falls back to the gate.
  Guardian carrier review before merge.
- **Frontier adapters (A2 widening).** `Arc -> Box` adapters at
  declared-`Fun` returns and parameters; widen the subset; graduate the
  bare-variable and payload generic fixtures. Each widening keeps the
  fallback for the remainder.
- **B track (separate design).** Last-use move for owned projections +
  per-parameter bound inference; own fixtures, own guardian review; revisit
  how far it retires the A2 flips.

SEAL statement: after the gate step, `ipe` exit-0 implies `cargo` exit-0
for this class by rejection; after the flip and adapter steps, by acceptance
for the covered subset and rejection for the rest. Golden implications: the
gate and plumbing steps are byte-neutral by construction; the flip and
adapter steps add new goldens and graduate gate fixtures; only the B track
causes corpus-wide churn (cheap to re-bless, expensive to review — which is
why it is separate).

## 6. Risks

- **Over-rejection via an unfaithful template.** The gate is exactly-narrow
  only if the recovered signature is the real quantification: unannotated
  solved schemes, alias-unfolded signatures
  (`annotation_is_function_alias` path), `any`-wildcard parameters
  (`fresh_any_param_symbol`), and row-open annotations each risk a spurious
  variable binding. Controls: the tripwire fixtures and a zero-new-rejection
  examples sweep in the gate step's exit gate.
- **Call-shape coverage.** Partial application, over-application
  (`saturate_over`), and a generic call feeding another generic call each
  reshape arguments away from the direct path; any missed path leaks the
  E0277. The gate step must enumerate every call-lowering entry and route it
  through the one matcher.
- **Carrier disagreement under A2** — a flipped value reaching an unflipped
  slot is an emitted-crate E0308. Loud, but still a SEAL break. Controls:
  the total rule, the fail-closed fallback for anything outside the proven
  subset, and the guardian's independent build.
- **Bound/move analysis under B** — dropped clones (E0382) and dropped
  bounds (E0277) are silent-until-cargo classes inside the ADR 0002/0011
  discipline; the transitive bound fixpoint is new whole-program machinery.
  Quarantined to its own later track.
- **Row polymorphism.** No dedicated row design document exists yet; the
  open-row annotation path fails closed (`Feature::RowPolyRecordAnnotation`).
  If row support lands via per-shape callee monomorphisation (the direction
  that gate's comment sketches), generic record slots become concrete at
  emission and the record half of this class dissolves into the shipped
  record-field flip; a witness-trait emission instead would deepen the bound
  problem (a `Clone` witness per row). Either way the A1 gate remains the
  backstop, and nothing here may pin the synthesised record-struct layout.

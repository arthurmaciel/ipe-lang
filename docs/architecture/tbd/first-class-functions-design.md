# First-class function values in records, lists, and constructor payloads

Status: design proposal, no implementation yet. Ipê sketches show the intended
surface; they are not runnable today.

## 1. The problem

A function is an ordinary value in Ipê's type system (`a -> b` unifies, passes
as an argument, binds with `let`), but the Rust backend cannot yet *store* one
inside data. Three fail-closed gates draw today's boundary:

- **IPE-L0107** — a function value in a record field (`{ format = \n -> … }`).
- **IPE-L0114** — a function value in a user-union constructor payload
  (`Parser (\s -> …)`).
- **IPE-L0125/L0126/L0127** — a non-`Clone` function capture / composite
  double-move, which also rejects a `List` of functions walked by value.

The gates exist because the emitted carrier, `Box<dyn Fn(..) -> R + Send + Sync
+ 'static>`, satisfies none of `Clone`/`Debug`/`PartialEq` — the derives every
synthesised record struct and union enum expects — so admitting the value would
emit Rust that fails to compile: an ipe-exit-0-then-cargo-fail SEAL break. The
gates are sound. They are also the single blocker for a whole idiom family:

- **Combinator libraries.** Every `X = State -> (Result, State)`-shaped library
  stores `X` in a `List` and combines via `oneOf : List X -> X`: parser
  combinators (`Ipe.Url.Parser`, a general `Ipe.Parser`), property-test fuzzers
  (`Fuzzer a` = seed-consuming generator + rose-tree shrinker), JSON decoders,
  random generators, codecs. All one shape, all unbuildable the natural way.
- **Records of functions as values** — dispatch tables / vtables, event-handler
  maps (`Dict String (Event -> Msg)`), middleware chains
  (`List (Request -> Request)`), effect interpreters, strategy objects.
- **Higher-order data** — `List (a -> b)` transform pipelines, thunks
  (`() -> a`), stored continuations.

The design goal: make these compile, without giving up concrete-over-generic
codegen, the six-principle order, or byte-exactness of every existing golden.

## 2. Baseline machinery

The compiler already carries three function representations and one working
precedent for "function stored in data":

| IR type | Rust carrier | `Clone` | Produced by |
|---|---|---|---|
| `IrType::Fun` | `Box<dyn Fn(..) -> R + Send + Sync + 'static>` | no | default first-class lowering |
| `IrType::SharedFun` | `Arc<dyn Fn(..) -> R + Send + Sync + 'static>` | yes (refcount bump) | fn-value-reuse promotion, contained record-of-functions only |
| `IrType::FnOnceChain` | nested `Box<dyn FnOnce>` | no | five curried decoder kernels only |

**The `SharedFun` precedent** (`ipe_lower::shared_fun_promotable_shapes` +
`flip_promotable_record_slots`): a reused record-of-functions has its `Fun`
fields flipped to `SharedFun`, the struct gets a **hand-written `impl Clone`**
(the `is_clone && !is_derivable` path in `emit_types`), and the reuse works. The
promotion is guarded by a whole-program **containment analysis**: the flip is
per field-name set, and every occurrence of a set program-wide must agree on
`Arc` vs `Box` or the backend meets an `E0308`. Containment ("appears only in
parameter or bare-value position, never returned, never nested in a composite")
is what guarantees a promoted value never reaches a `Box`-carried sibling.

**Key insight:** the containment restriction is not intrinsic to `Arc<dyn Fn>`.
It exists only because promotion is *selective* — two carriers for one Ipê type
must never meet. A rule that assigns the carrier *totally and deterministically
from the type's shape* needs no containment analysis at all: every occurrence
agrees by construction.

**The opaque-wrapper precedent:** `Decoder`/`Task`/`Cmd`/`Sub` are built-in
types that already store functions behind a non-deriving struct
(`runtime json::Decoder { run: Box<dyn Fn(..)>, fields }`) and are explicitly
exempt from the gates (`is_opaque_boxed_wrapper`). Functions-in-data already
works — for a closed, kernel-managed set of types. This design generalises that
capability to user data.

**The derive machinery generalises:** records already split
`is_derivable` (CDPeq) / `is_clone` (hand-written `Clone`) / `is_serde`;
`IpeStringify` is hand-written and total even for non-derivable shapes. Enums
have the derivable/non-derivable split but not yet the hand-written-`Clone`
middle tier.

## 3. Representation approaches

### A. Uniform storage-carrier normalization (`SharedFun` broadening) — recommended

One total rule, applied as an IR-type canonicalization: **a function type in a
storage position — under `Record`, `Enum`, `List`, `Set`-element, `Dict`-value,
`Tuple`, `Maybe`, `Result` — is always `SharedFun`; a function type in a direct
position — parameter, `let` binding, callee, bare return — stays `Fun`.** The
rule is a function of the type tree alone (exactly
`flip_promotable_record_slots` made unconditional), so every occurrence of a
type agrees on the carrier program-wide with no analysis, no candidate sets, no
disqualification lists. Where the two carriers meet, a total O(1) adapter
converts (§4.4).

- *Performance:* virtual call + one allocation per stored closure — identical
  to today's `Box` carrier in call cost; `Clone` is an atomic refcount bump.
  No regression versus any code that compiles today; direct calls keep their
  current (non-boxed, monomorphized) paths untouched.
- *Invariants:* preserves concrete-over-generic where it means something — the
  parameter and return types of the trait object stay fully concrete and
  monomorphized; only the *code pointer* is indirect, which is the semantic
  content of "a function chosen at runtime". No `dyn Any`, no type erasure of
  data.
- *Type system:* zero surface change. `a -> b` remains one Ipê type; the
  carrier is an IR/codegen detail.
- *SEAL:* byte-neutral by construction (§4.7).
- *Cost:* small — the mechanism, derive split, and reconciliation already
  exist from the fn-value-reuse promotion; the work is mostly *deleting*
  restrictions and auditing frontiers.

### A′. Single carrier everywhere (every `Fun` becomes `Arc`)

Delete `Fun` entirely; all function values are `Arc<dyn Fn>`. Maximal
simplicity: no frontiers, no adapters, the L0125/L0126/L0127 family disappears
for functions. Rejected for now: it rewrites the emitted bytes of essentially
every existing golden fixture (mass re-bless, a correctness-review event, not a
diff review), and it puts an atomic refcount on hot paths that today use a lean
`Box` move. Worth revisiting as an end-state simplification once the feature is
proven; the normalization rule of A is forward-compatible with it (A′ is A with
"storage position" broadened to "everywhere").

### B. Defunctionalization (global sum type + `apply`)

Enumerate every function value that reaches a storage position; generate one
enum per signature (`enum Fn_Str_Int { F0, F1(CapturedEnv), … }`) plus a
concrete `fn apply(&self, ..) -> R` match. No `dyn`, no refcount; enums derive
`Clone`, can derive `Debug`/`PartialEq` when captures do (function equality
becomes *possible*), and the `apply` match is a jump table the optimizer can
see through.

Rejected as the primary representation:

- **Whole-program coupling.** Adding one lambda anywhere regenerates the enum
  for its signature — every module holding that signature re-emits. This works
  directly against the salsa incremental-compilation roadmap and makes emitted
  output hypersensitive to unrelated edits (SEAL churn on every fixture that
  shares a signature).
- **Frontier adapters survive anyway.** The runtime's callback slots
  (`ui::element::Event`, Task combinators) take `Arc<dyn Fn>`/boxed closures;
  a defunctionalized value must be wrapped in a closure at every kernel
  boundary — so the `dyn` indirection is not actually eliminated where it
  matters, only moved.
- **Polymorphism multiplies enums.** A generic `List (a -> b)` in a generic
  function needs an enum per monomorphic instantiation, entangling
  defunctionalization with monomorphization ordering.
- Highest implementation cost of all candidates (a new whole-program pass, new
  IR value forms, new emission strategy) for a performance delta that is
  marginal in the target idioms (parsers/decoders are allocation- and
  branch-dominated, not call-dominated).

Retained as a *future, semantics-preserving optimization tier*: a per-module
defunctionalization of provably-local function sets could replace `Arc` with a
concrete enum without any language-visible change.

### C. Closure-conversion to per-site concrete enums

B's flow-partitioned sibling: only the functions that flow into one storage
*site* form that site's enum, keeping the sum local. Sound until two sites
exchange values (a `oneOf` result stored into another combinator's list), which
re-couples the partitions through unification — precisely the common case in
combinator libraries. Degenerates into B with extra bookkeeping. Rejected for
the same reasons plus instability of partition boundaries.

### D. Function-pointer tier / id-addressed registry

A capture-free function value is representable as a plain Rust `fn(..) -> R`
pointer: `Copy`, `'static`, zero allocation, comparable. A registry
(id-indexed table of monomorphic functions) is the same idea with an integer
handle. Rejected as *the* representation — partial application and closures
are pervasive in Elm-style code, so the capture-free subset is too narrow, and
a mixed pointer/closure representation reintroduces the two-carrier frontier
problem in a worse form (three carriers). Retained as a possible later
optimization *inside* the A rule: `Arc::new(top_level_fn)` sites could become
shared statics; observable bytes change, so it waits for a deliberate re-bless.

### E. Opaque user newtypes (generalising the `Decoder` exemption)

Let a user union whose single constructor wraps a function
(`type Parser a = Parser (State -> Step a)`) emit the same non-deriving,
hand-managed struct shape as `Decoder`. This unblocks combinator *libraries*
(they all name their function type anyway) at very low cost — but not raw
`{ f : a -> b }` records, not `Dict String (Event -> Msg)`, not
`List (Request -> Request)`, and it institutionalises a special case ("fix the
symptom") instead of the general rule. Rejected as the design; subsumed by A
(under A, the newtype works *and* the raw forms work).

### Decision matrix

| | A (normalize) | A′ (one carrier) | B/C (defunc.) | D (fn-ptr) | E (newtype) |
|---|---|---|---|---|---|
| Soundness story | proven (#SharedFun path) | proven | new machinery | partial | proven |
| Idiom coverage | full | full | full | capture-free only | libraries only |
| SEAL byte-neutrality | yes | no (mass re-bless) | no (signature coupling) | no | yes |
| Incremental (salsa) friendly | yes | yes | no | yes | yes |
| Inlining / no-indirection | no (same as today) | no | partial | yes | no |
| Function `==`/`Debug` | gated (typed diagnostic) | gated | derivable sometimes | comparable | gated |
| Implementation cost | low | medium | high | medium | low |

**Recommendation: A.** It is the principled broadening of the shipped
mechanism: keep the carrier, delete the containment crutch, and replace a
selective analysis with a total rule — the "fix the structure" move. B's real
advantages (derives, inlining) are recoverable later as optimizations under
the same surface semantics; nothing in A forecloses them.

## 4. Design

### 4.1 Carrier normalization rule

A single total function `normalize_fun_carriers : IrType -> IrType` (the
unconditional generalisation of `flip_promotable_record_slots`) stamps every
`Fun` that sits under a data constructor to `SharedFun`:

- Under `Record`, `Enum`, `Tuple`, `List`, `Maybe`, `Result`, `Set` (element),
  `Dict` (value position): → `SharedFun`.
- Direct positions — a function's own parameter list, a bare `let`/def
  binding, the callee of an application, a bare (non-composite) return type:
  stay `Fun`.
- Inside another function type (`(a -> b) -> c`): the inner `a -> b` is a
  parameter/return of the outer, i.e. a direct position — stays `Fun`.
- Opaque boxed wrappers (`Decoder`/`Task`/`Cmd`/`Sub`) and `FnOnceChain`
  kernels: untouched (their payloads are runtime-managed, not synthesised
  structs).

Applied at the same point the promotion flip runs today — after solving,
before emission — to every solved type, region, and value expression
(`promote_fn_field_value_carrier` becomes equally unconditional: stored
`Lambda` → `SharedLambda`, stored `FuncValue` re-stamped). Because the rule
depends only on the type tree, the struct/enum synthesised for a shape is
identical at every occurrence — the property the containment analysis
laboriously approximated becomes true by construction.

### 4.2 Type-system treatment

No surface syntax, no new Ipê type, no opt-in annotation. The stored/direct
boundary is **inferred from position**, never written by the author — an
Elm-experienced author's program simply compiles. This follows the
"explicit-in-cfg over magic, but no ceremony for the common case" line: the
carrier is not a semantic distinction (both are "a function"), so surfacing it
would be noise.

Generics: a stored function type inside a generic composite
(`List (a -> b)` under type variables) renders
`Arc<dyn Fn(T1) -> T2 + Send + Sync + 'static>`, which requires `T1: 'static`
etc. on the emitted generic signature. Phase 1 covers concrete instantiations
(the region type at every use site is concrete for the target idioms);
genuinely polymorphic *emitted* signatures storing functions add `+ 'static`
to their type-parameter bounds in a later phase, or fail closed with the
existing diagnostic until then.

### 4.3 Derive capability, not blanket rejection

Storing a function costs `Debug`/`PartialEq`/serde on the containing shape.
Today that cost is paid by rejecting the program; under this design it is paid
by *narrowing what you can do with the value*, each narrowing a typed
diagnostic at the operation, not the definition:

- `Clone`: kept — hand-written `impl Clone` (records: exists; enums: add the
  same `is_clone && !is_derivable` tier to `emit_enum`).
- `IpeStringify`: kept total — a function slot renders a fixed placeholder
  (e.g. `<function>`), matching the existing hand-written-stringify pattern.
- `==` / `/=` / `compare`: **compile-time rejection** when either operand's
  type embeds a function (region-typed gate, same mechanism as
  `reject_function_through_type_var`). Strictly better than Elm, which crashes
  at runtime on function equality — fail-closed at `ipe` time.
- `Dict`/`Set` **keys**: functions rejected (no `Ord`) — the exact shape of
  the shipped float-keyed-collection gate. `Dict` *values* are fine.
- Serialization frontiers: the Web model gate already classifies
  `Fun`/`SharedFun`/`FnOnceChain` as `ModelLeaf::Function` and excludes them
  from the serialized model; ports/JS boundary and any persisted state keep
  rejecting functions. Unchanged, and deliberately so (§7).
- List kernels: `CloneOk` elements make the by-value walks sound as-is; the
  handful of kernels whose registry signature requires `PartialEq`/`Ord` on
  elements (`member`, `sort`, …) gate on fn-embedding element types with the
  equality diagnostic above. Requires a one-time kernel-registry audit tagging
  each list/dict kernel with its required element capability — making the
  capability requirement explicit in the registry rather than implicit in the
  emitted code, per "make invalid states unrepresentable".

### 4.4 Carrier frontiers

Two carriers still meet at re-stamp boundaries. Both directions are total,
O(1), and local:

- `Box → Arc` (a direct value flows into storage): `Arc::from(boxed)` — a
  supported unsizing-preserving conversion — or construction-site re-stamping
  (`Arc::new(..)` directly, as the promotion does today), which is preferred
  and covers every literal/lambda/top-level-reference case with no conversion
  at all.
- `Arc → Box` (a value read out of storage flows into a `Box`-typed slot,
  e.g. a kernel parameter): `Box::new(move |a, ..| shared(a, ..))` — one
  wrapper closure, emitted by the same reconciliation walk that today handles
  `SharedFun`-vs-`SharedFun` (`reconcile` in the backend). Since `Arc<dyn Fn>`
  is itself callable, direct calls of a stored function need no adapter at
  all.

The existing `deferred_fun_captures` / `promotable_fn_binders` machinery — the
careful routing around IPE-L0126 — simplifies: a captured stored-function
value is `CloneOk` outright, so the classifier stops special-casing pure-`Fun`
captures whose binder is promotable and instead promotes *every* fn-typed
`let`/param binder that is captured or stored (the current shadow-rebind
mechanism, applied unconditionally on demand).

### 4.5 Effect/Task runtime interaction

No runtime changes required. Task combinators consume boxed closures (an
`Arc → Box` wrap at the frontier, or a direct call); UI/Web event slots are
already `Arc<dyn Fn + Send + Sync + 'static>` — the same carrier, zero-cost.
`Msg` values carrying functions (event-handler maps) are `Clone` via the
hand-written tier, so the TEA update loop is unaffected. The `Send + Sync +
'static` bounds are unchanged from today's `Box` carrier, so the set of legal
captures is exactly today's set — carrier-transparent, as the fn-value-reuse
work already established.

### 4.6 What gets deleted

The structural payoff — each deletion removes a whole defect class rather than
patching around it:

- `shared_fun_promotable_shapes`, `walk_shape_contain`, `ShapePosition`, the
  candidate/disqualified sets — the whole containment analysis.
- `reject_function_valued_field` (IPE-L0107) and the record-field half of
  `reject_function_through_type_var`; `con_payload_carries_function`
  (IPE-L0114) once enums land.
- The L0107 exemption special cases for app-config records
  (`lower_app_cfg_record` et al.) — app-config literals become ordinary
  records of functions.
- The `RetryPolicy`/`shouldRetry` field-name sniff — the general rule covers
  it.
- The fn-specific arms of IPE-L0125/L0126/L0127 (non-fn non-`Clone` captures
  — `Task`, `Decoder` values — keep their gates unchanged).

Diagnostic codes L0107/L0114 are retired, not repurposed; their explain pages
become "this compiles now" tombstones per the compiler-as-kind-teacher
convention.

### 4.7 SEAL / golden byte-exactness

Byte-neutrality for every existing fixture follows from a case split:

1. Programs the gates *rejected* — no golden exists; new goldens only.
2. Programs using the fn-value-reuse promotion — the total rule performs the
   identical flip on those shapes (the promotion's flip is the rule restricted
   to its candidates), so `fn_record_reuse_promoted/main.rs` is unchanged.
   `fn_record_reuse_escapes` / `fn_record_reuse_mixed` flip from fail-closed
   to compiling — their fixtures graduate from gate tests to positive goldens
   (escaping is legal under a total rule; the mixed record's `Task` field
   keeps it non-`Clone`, so single-use compiles and reuse still fails L0127).
3. All other compiling programs — contain no `Fun` under a data constructor
   (the gates guaranteed it), so `normalize_fun_carriers` is the identity on
   every type they emit. Bytes unchanged.

The Phase-1 exit gate is mechanical: the full golden corpus diff must be empty
outside the two graduated fixtures.

## 5. Phased implementation plan

Dependency-ordered; each phase lands green (build + clippy + full golden corpus
+ E2E) before the next starts.

- **Phase 1 — records (the L0107 core).** Make the carrier flip total for
  `Record`; delete the containment analysis and the record gates; extend
  reconciliation with the `Arc → Box` wrapper; add the equality/`Dict`-key
  gates for fn-embedding record types. Goldens: dispatch-table record,
  record returned from a function (the graduated escape fixture), record
  nested in `Maybe`.
- **Phase 2 — constructor payloads (L0114).** Hand-written `Clone` tier for
  enums; flip under `Enum`; delete the ctor-payload gate. Goldens: a
  `type Parser a = Parser (State -> Step a)` newtype, pattern-matching the
  function out and calling it.
- **Phase 3 — collections.** Flip under `List`/`Maybe`/`Result`/`Tuple`/
  `Dict`-value/`Set`-element; kernel-registry capability audit (which kernels
  need `PartialEq`/`Ord` on elements) with typed per-operation gates.
  Goldens: `List (Int -> Int)` fold-apply pipeline, `oneOf`-shaped combinator
  over a stored list, `Dict String (Event -> Msg)` dispatch.
- **Phase 4 — captures.** Collapse the deferred-capture special-casing into
  unconditional binder promotion; extend promotable binders to destructure /
  match-arm patterns (the L0126 residue). Goldens: closure forwarding a
  stored function; match-arm-bound function captured by a lambda.
- **Phase 5 — polymorphism.** `+ 'static` bounds on emitted generic
  signatures that store functions; generic combinator (`oneOf : List (P a) ->
  P a`) goldens across two instantiations.
- **Phase 6 — stdlib exploitation.** `Ipe.Parser` (elm/parser port), the
  typed-Url routing parser, `Fuzzer`/generators for property tests, decoder
  ergonomics on the general mechanism. Each is its own tracked effort; this
  design only has to leave them unblocked.
- **Deferred, explicitly out of scope:** A′ single-carrier unification;
  fn-pointer statics for capture-free values; per-module defunctionalization —
  all byte-visible optimizations gated on a deliberate golden re-bless.

## 6. What this unblocks

- **#399** — exact-Elm `Ipe.Url.Parser`: needs `List (Parser a)` + `oneOf`
  (Phases 2–3).
- **#397** — `Ipe.Parser` (elm/parser port): same shape plus stored
  continuations (Phases 2–4).
- **#273** — property tests: `Fuzzer a = Seed -> (RoseTree a, Seed)` stored in
  lists and records of generators (Phases 2–3, generics in 5).
- JSON decoder/codec ergonomics, random generators, middleware chains,
  event-handler maps, effect interpreters — no tracked issues yet; the idioms
  simply start compiling.

## 7. Constraints — where the feature stays fail-closed

- Functions never cross a **serialization frontier**: the Web model
  snapshot/schema, ports/JS values, persisted state, FFI values. Existing
  `ModelLeaf::Function` classification stays authoritative.
- Function **equality/ordering** is a compile-time error, including through
  type variables (region-typed gate). No runtime crash path, no structural
  comparison of closures — ever.
- `Dict`/`Set` **keys** must not embed functions.
- Captures keep the `Send + Sync + 'static` bound — no new capture shapes are
  admitted, only new storage positions for already-legal values.
- "Unknown ⇒ reject" is preserved at every gate that remains: any position the
  equality/serialization walks cannot classify fails closed.

## 8. Risks

1. **Silent carrier mismatch (E0308) at an unaudited frontier** — a stored
   function reaching a `Box`-typed slot the reconciliation walk misses. This
   is the SEAL-break class; mitigated by making re-stamping type-directed (one
   `reconcile` chokepoint), a frontier-focused golden matrix per phase, and
   the independent-reviewer build step.
2. **Transitive derive ripple** — a record containing a record containing a
   function must lose `PartialEq`/`Debug` *transitively*; a shallow
   `is_derivable` computation would emit a derive that fails on the inner
   field. The derive flags must be computed as a fixpoint over the type graph
   (the enum emitter's variant-table recursion is the template).
3. **Equality-gate escape through a type variable** — `contains (==) list`
   instantiated at a fn-embedding element type inside a *generic* stdlib
   function, where no concrete region exposes the function. Needs the gate at
   instantiation boundaries (kernel-registry capability tags), not only at
   literal `==` sites.
4. **Kernel-registry blind spots** — a list/dict kernel whose hand-written
   Rust assumes `PartialEq`/`Ord`/serde on elements without the registry
   saying so. The Phase-3 audit converts this from unknown to enumerated.
5. **Model/TEA edge** — any runtime path that compares models for
   change-detection would silently degrade for fn-carrying models; audit for
   equality-based short-cuts before Phase 1 exits (current classification
   suggests none, since models exclude functions at the serialization gate —
   confirm whether *non-serialized* local UI state may carry functions).
6. **Golden churn discipline** — phases must keep the "identity on old
   programs" property; any incidental formatting drift in emitted adapters
   would show as corpus-wide diffs. The empty-diff exit gate per phase is the
   control.

## 9. Open questions

- Should `toString` on a function-carrying value be *rejected* rather than
  rendering `<function>`? Rendering matches the existing total-`IpeStringify`
  posture and Elm's `Debug.toString`; rejection is stricter. Proposed:
  render, with the placeholder documented in the explain page.
- Do `Tuple` positions occur enough to be in Phase 1 rather than 3? (Cheap
  either way; grouped with collections for audit hygiene.)
- `FnOnceChain` interaction: the five curried kernels keep their bespoke
  carrier — confirm no user-visible type can store a `FnOnceChain` (today it
  cannot; the normalization rule must keep it that way).
- Whether Phase 5's `+ 'static` on generic parameters is observable in any
  emitted public signature a golden pins today (expected no — generic emitted
  signatures already carry bounds lists; verify on the record-generic corpus).
- A′ end-state: after the feature stabilises, is the two-carrier economy still
  paying for itself, or should a deliberate re-bless collapse to one carrier?
  Revisit with real programs and measurements, not now.

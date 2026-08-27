# L0126 / L0127 / L0115 gate relaxation vs the `SharedFun` / `Decoder` carriers

Status: design/recon, no implementation. This scopes the narrowing of three
function-value lowering gates — `IPE-L0126` (non-`Clone` capture), `IPE-L0127`
(fn-value reuse), `IPE-L0115` (refutable-tuple / record pattern) — so the pure-Ipê
`Ipe.Codec` composite builders (`object`/`field`/`buildObject`, `enum`,
`taggedUnion`/`varN`) type-check and emit sound Rust, while every case that is
still genuinely non-emittable stays fail-closed. It is the gate-side companion to
the carrier work that already shipped: the `SharedFun` `Arc<dyn Fn>` fill/read
carrier and the clonable `Arc` `Decoder` carrier.

The relaxation is a soundness-gate relaxation. THE SEAL is the invariant: an
`ipe`-accept must `cargo`-build. The whole design is anchored on **not** opening
an accept-then-cargo-fail — every newly-admitted case is proved to reach a carrier
that is `Clone` (and `Send + Sync + 'static`) in the emitted Rust, and every case
that cannot is kept rejected by construction.

## Executive summary — three stale predicates, one boundary

Empirical probing of the current compiler (the merged `Decoder`-`Arc` carrier in
place) isolates the composite-builder blockers to **three over-broad predicates**,
each now soundly relaxable, and confirms the boundary that must stay closed:

| # | Predicate (today) | Site | Blocks | Now sound because |
|---|---|---|---|---|
| P1 | `Decoder` is non-`Clone` | `clone_class` + `carrier_is_clone` (`Decoder(_) => NonClone/false`) | `field` / `map` / record builder (a `Codec { enc, dec : Decoder }` captured/reused) | the runtime `Decoder<E,T>` is now an unconditional-`Clone` `Arc` carrier |
| P2 | a `Generic`-typed **value** capture is non-`Clone` in a closure | the capture classifiers' `clone_class(Generic) => NonClone` branch | `enum` (a generic value flows into a `filter` closure) | the emitted closure bounds every capture `T: Clone`; a bare `Generic` capture clones under that bound (already sanctioned for reuse by `param_is_multiuse_clonable`) |
| P3 | a `Decoder`-typed `let` is force-thunked and a deferred fn-capture of it is re-raised `L0126` | `lower_let_pvar` decoder arm + its `deferred_capture` re-raise | reuse of / capture of a bare `Decoder` binding | with P1, a `Decoder` binding is `CloneOk` and takes the ordinary clone/promotion path — the rebuild thunk is obsolete |

The boundary that **stays** L0126/L0127: a capture/reuse of a `Task` / `Cmd` /
`Sub`, an FFI foreign opaque handle, or a bare `Fun` in a *non-storage* direct
position — none of which has a `Clone` carrier. A record/enum that carries any such
member stays non-`Clone` and stays gated.

L0115 (tuple / record pattern) is confirmed **not** a blocker for the builder
shapes and is largely already relaxed — it is documented here for completeness and
to fix its boundary in the test plan, not to change it.

## 1 — Each gate today: predicate, unsound emit, why it was correct

### IPE-L0126 — non-`Clone` capture in a closure

Raised by `rewrite_captured_clones` (and the `deferred_fun_captures` re-raise
sites) in `src/compiler/lower/src/lower.rs`. A closure lowers to `Box<dyn Fn(..)
-> R + Send + Sync + 'static>`, whose captures must be re-usable across calls
(`Fn`, not `FnOnce`), i.e. each capture must be `Clone`. A captured symbol whose
type is `CloneClass::NonClone` is admitted **bare only in direct-callee position at
closure depth 0** (`Fn::call` borrows `&self`); anywhere else the closure would
move a non-`Clone` value out of its `&self` env per call.

Unsound emit it prevents (illustrative Rust): capturing a bare `Box<dyn Fn>` and
forwarding it (storing it, returning it, passing it on) —

```rust
// captured `f: Box<dyn Fn(A)->B>`, forwarded (not called) inside a `Fn` closure
let g = move |x| record_with(f);   // moves `f` out of &self → E0507 / FnOnce → E0525
```

Correct pre-carrier: a `Box<dyn Fn>` genuinely is not `Clone`, and there was no
alternative carrier, so any forward of it out of a `Fn` closure was a real
`cargo`-fail; failing closed at `ipe` time upheld THE SEAL.

### IPE-L0127 — a value holding a function is used more than once

Raised by `reject_fn_value_reuse` via `apply_move_ownership`. A binding whose IR
type embeds a function (`ir_contains_fun`) and is `CloneClass::NonClone` renders as
(or contains) a `Box<dyn Fn>`, which is not `Clone`. Calling it is unlimited
(`Fn::call` borrows); a second **consuming** use (a second forward, a second store)
moves an already-moved value.

Unsound emit it prevents (illustrative Rust):

```rust
let pair = (mf, mf);   // second move of a non-Clone `Box<dyn Fn>` value → E0382
```

Correct pre-carrier: a `NonClone` fn-carrying value had no sound duplicating
rewrite (`.clone()` is `E0599` on a `Box<dyn Fn>`), so reuse was a real
`cargo`-fail unless rejected.

### IPE-L0115 — refutable-tuple / record pattern

Raised by `lower_destructure_pat` (a refutable element in an irrefutable
destructure) and `lower_arm_pat` / `tuple_case_supported` (an unsupported product
`case` shape: a record head, a multi-arm record match, a non-literal-tuple
scrutinee). A refutable element in a `let` / parameter destructure could fail to
match at run time; a `Box`-shaped refutable `let` is not a sound Rust binding.

Unsound emit it prevents: a `let (Just x, y) = e` has no total Rust `let` form —
the `Just` could fail — so binding it irrefutably would be unsound.

Correct: sound and orthogonal to carriers — it is a pattern-totality gate, not a
carrier gate. The builder shapes destructure through **irrefutable single-arm**
`case c of Codec r -> …` / `Variant r -> …`, which L0115 already admits.

## 2 — What the carriers now change (per gate, with soundness sketch + probe)

Two carriers are the enabling facts, both landed:

- **`SharedFun` (`IrType::SharedFun`)** — a function stored in a record field, a
  collection element, a tuple component, or an enum payload is normalized to
  `Arc<dyn Fn(..) -> R + Send + Sync + 'static>` (`normalize_record_fun_carriers`
  + the storage-element flip), unconditionally by position. `Arc` is `Clone` (a
  refcount bump), so a stored-function slot is `CloneOk` and never poisons its
  enclosing composite. Reads out of a storage carrier that flow into a direct
  `Box`/`impl Fn` slot are eta-demoted back to `Box` at the boundary
  (`demote_shared_fn_read`); fills into a storage slot are eta-promoted to `Arc`
  (`promote_stored_fn_carrier`). The fill/read frontier is total and O(1), so the
  two carriers meet only through the adapter — no `Arc`-vs-`Box` `E0308` frontier.

- **`Arc` `Decoder`** — the runtime `Decoder<E, T>` now carries `run:
  Arc<dyn Fn(&JsonVal) -> IpeResult<E,T> + Send + Sync>` with a hand-written
  unconditional `Clone` (independent of `E`/`T`). A stored `Decoder` clones by
  refcount bump and is read back out and reused; the kernels that consume a
  decoder (`decode_list`, `decode_map*`) take it by value and borrow its `run`.

### L0126 → admit: a stored `Fun`/`Decoder` captured through a closure

The now-admissible case-class: a closure captures a **whole composite** (a record,
an enum payload, a tuple) whose only non-derivable members are functions (carried
`SharedFun`) and/or decoders (carried `Arc` `Decoder`). The composite is `CloneOk`,
so the capture clones at the closure boundary and the closure stays `Fn`.

Soundness sketch: the emit clones the captured composite per closure boundary; the
`SharedFun` slot's `Arc::clone` is a refcount bump, the `Decoder` slot's `Clone` is
the hand-written `Arc`-refcount bump, every scalar/collection slot is already
`Clone`. The whole struct/enum derives (or hand-writes, for the `SharedFun` tier)
`Clone`. Every emitted carrier bounds captures `Send + Sync + 'static`, unchanged
across the `Box`→`Arc` swap, so no capture that was legal becomes illegal.

Probe (current compiler, `Arc` `Decoder` in place) — this exact `.ipe` is
**rejected `L0126`** today:

```ipe
type Codec a = Codec { enc : a -> Value, dec : Decoder a }
mapCodec f g c =
    case c of
        Codec r -> Codec { enc = \b -> r.enc (g b)          -- captures r
                         , dec = Decode.map f r.dec }        -- consumes r.dec
```

The closure `\b -> r.enc (g b)` captures `r`, and `r`'s clone class is `NonClone`
**solely** because `dec : Decoder a` is classed non-`Clone`. Probed: replacing
`dec : Decoder a` with a second **function** field (both `SharedFun`) — the
identical two-fn-field record shape — **accepts and builds** today; replacing it
with a scalar `tag : Int` field **accepts and builds**. The sole differentiator is
the stale `Decoder` clone class (P1). Once `Decoder` is `CloneOk`, `r` is
`CloneOk`, the capture clones, and the case joins the already-admitted two-fn-field
shape.

### L0126 → admit: a bare `Generic` value captured through a closure

The `enum` builder's `enumName` closes a `filter` predicate over a generic value
`v : a` and the constructor set — this exact `.ipe` is **rejected `L0126`** today:

```ipe
enumName pairs eq v =
    List.filter (\pair -> case pair of (c, _) -> eq c v) pairs   -- captures v : a
```

while the identical shape monomorphized to `v : Int` **accepts and builds**
(probed). A bare `Generic` capture is `NonClone` in `clone_class`, yet the emitted
closure's captures carry an unconditional `T: Clone` bound (`render_fn_generics`'
`with_clone` — the same bound `param_is_multiuse_clonable` already relies on to
admit a bare-`Generic` *reuse*). Under that bound the inserted `.clone()` on the
captured `T` always type-checks; a non-`Clone` instantiation is rejected at the
caller by the `T: Clone` bound before the clone is reached, so admitting the
capture loses no diagnostic — it only closes the current spurious rejection.

Soundness sketch: identical to the sanctioned reuse admission. The capture is added
to the closure's clone-set (a `CloneVar` / `.clone()` per boundary), which
type-checks under the emitted `T: Clone`. This is the value-capture analogue of a
predicate already trusted on the reuse side — a single-source alignment, not a new
capability.

### L0127 → admit: reuse of a `CloneOk` composite (already, plus `Decoder`)

The reuse gate is already correctly narrow: it fires only for a value that
`ir_contains_fun` **and** is `CloneClass::NonClone`. A record-of-functions is
`CloneOk` (via `SharedFun`) and its reuse is already admitted (the shipped
`fn_record_reuse_promoted` / `_escapes` goldens). The only change P1 forces is
transitive: a record/enum that carries a `Decoder` becomes `CloneOk`, so its reuse
stops tripping L0127 — the same relaxation as the capture case, through the same
single predicate. No change to `reject_fn_value_reuse` itself is required; it reads
`clone_class`, which P1 corrects at the leaf.

### L0115 — no change

The builder shapes destructure through irrefutable single-arm `case … of Codec r
-> …`, which L0115 admits; a match-arm fn-typed binder is already a promotable fn
binder (arm-pvar registration) with a read-frontier registry. Probing `varN`'s
`case variant of Variant r -> Decode.map ctor r.dec` **accepts** today. L0115 stays
exactly as-is; the test plan pins its boundary so the relaxation cannot silently
widen it.

## 3 — The narrowed predicate (make-invalid-states-unrepresentable, fail-closed)

The relaxation is expressed as **corrections to two leaf carrier predicates**, so
every consumer (capture classifier, reuse gate, promotion pass) inherits the fix
from one source — no per-site special-case, no new gate branch to keep in sync.

### P1 — `Decoder` is a `Clone` carrier

`carrier_is_clone(IrType::Decoder(_)) = true` and
`clone_class(IrType::Decoder(_)) = CloneOk`.

Structural justification: the SSOT is the runtime type. The runtime `Decoder<E,T>`
carries `Arc<dyn Fn + Send + Sync>` + a hand-written `Clone` that bounds neither
`E` nor `T`. The IR carrier predicates must mirror the runtime carrier's `Clone`
reality — leaving them stale is precisely a representable-but-illegal pipeline
state (the emit already `.clone()`s a projected decoder field; the classifiers
disagree). This is additive to what emits: the emit side already treats a stored
`Decoder` as clonable, so P1 removes a false rejection, it does not enable a new
emit shape the emitter cannot serve.

Fail-closed delineation: this flips **only** `Decoder`. `Task`/`Cmd`/`Sub` (pinned
futures / effect descriptors, genuinely non-`Clone` runtime carriers), bare `Fun`
(the `Box<dyn Fn>` default carrier), `FnOnceChain` (consume-once by type),
`Generic`/`RowGeneric` (P2 handles the capture case under the emitted bound), and
an FFI foreign opaque handle stay non-`Clone`. A composite carrying any of those
stays `NonClone` and stays gated — the compositional `clone_class` propagation is
unchanged, so the boundary is exactly "does every member have a `Clone` carrier".

### P2 — a bare `Generic` value capture clones under its emitted bound

In the capture classifiers (`rewrite_lambda_captures`, `lower_let_pvar_decoder`'s
thunk-body classifier, and the arm-body classifier), route a captured symbol whose
IR type is a **bare** `IrType::Generic(_)` into the `clone_set` (clone at the
boundary) instead of leaving it silently dropped / fail-closed. This mirrors
`param_is_multiuse_clonable`'s bare-`Generic` admission exactly, and MUST stay
single-sourced with it (both rely on `render_fn_generics`' unconditional
`with_clone`; if one changes the other must).

Fail-closed delineation: **bare** `Generic` only. A composite carrying a generic
(`List Generic`, `Tuple(.., Generic)`) still floors to `NonClone` through the
generic leaf and stays out of scope — the wider blast radius of flipping
`clone_class(Generic)` itself is deliberately not taken. A generic instantiated to
a non-`Clone` type is rejected at the caller by the `T: Clone` bound, so no
unsound clone can reach the emit.

### P3 — retire the obsolete `Decoder` rebuild-thunk fail-close

In `lower_let_pvar`, the decoder arm force-wraps every `Decoder`-typed `let` in a
zero-arg rebuild thunk and re-raises `L0126` when a deferred fn-capture named that
binding (on the premise "`Decoder` is `!Clone`"). With P1 the premise is false: a
`Decoder` binding is `CloneOk` and must take the ordinary `let` path — the T5
multi-use clone rewrite and, when captured/reused as a value, the promotion path —
exactly as any other `CloneOk` binding. `Decoder` `let` bindings are also
registered as promotable binders so a captured decoder promotes rather than
fail-closing.

Fail-closed delineation: retiring the thunk removes a rejection; it cannot open an
unsound accept because the resulting binding is genuinely `CloneOk`. The
curried-pipeline `FnOnceChain` decoder slots (`decode_pipeline_*` / `db_decode_*`)
are a **distinct** carrier (`IrType::FnOnceChain`, consume-once) and are untouched —
they keep their existing lowering.

### The invariant, stated once

> A value may be captured into a closure, or reused as a value, **iff** every
> carrier it embeds is `Clone` in the emitted Rust. That set is now exactly:
> scalars, the transparent composites over `Clone` members, `SharedFun`
> (`Arc<dyn Fn>`), `Decoder` (`Arc`), and a bare `Generic` (under its emitted
> `T: Clone`). Everything else — `Task`/`Cmd`/`Sub`/`Fun`/`FnOnceChain`/foreign
> handle — is non-`Clone`, and a composite inheriting one is non-`Clone`. Absent a
> `Clone` carrier the outcome is rejection, never a deferred `cargo`-fail.

## 4 — The composite-builder unblock

The `Ipe.Codec` type is `Codec { enc : a -> Value, dec : Decoder a, shp : Shape }`
wrapped in a single-arg user enum `Codec`. Its emitted carrier is
`Enum{Codec, [Record{ enc: SharedFun, dec: Decoder(Arc), shp: … }]}`. Under P1 that
record is `CloneOk` (both non-derivable members have `Clone` carriers), so the
`Codec` value is `CloneOk`.

- **`object`/`field`/`buildObject`** (the applicative record builder). `field`
  projects `enc` + `dec` off the accumulator `Codec` and threads them into new
  closures (an `enc` composed with the getter; a `dec` chained via `Decode.map2`).
  The projected record binder `r` is captured by the composing closure ⇒ **lands on
  the L0126 admit of §2** (P1 makes `r` `CloneOk`). Confirmed by probe: the
  two-fn-field analogue already builds; the `Decoder`-field variant is blocked only
  by P1.

- **`taggedUnion` / `varN`**. Each `Variant` carries an encoder + a decoder; the
  decode thunk forwards a captured constructor / codec via `Decode.map ctor r.dec`.
  The `Variant r -> …` arm binder is a promotable fn/decoder binder ⇒ **already
  admitted** (the `varN`-shaped probe builds today); P1 covers the transitive
  `Decoder`-in-record case where a variant is stored in a record accumulator.

- **`enum`**. `enumName` closes a `filter`/`find` predicate over a generic value
  `v : a` and the pairs list ⇒ **lands on the L0126 `Generic`-capture admit (P2)**.
  The pairs list is a `List (a, String)` (a `Clone` composite once `a` is
  `Clone`-bounded), so its capture is covered by the same bound.

Each newly-admitted builder lands on a §2 case; none requires a new emit shape the
carrier machinery does not already serve.

## 5 — Interaction with the `Sync`-propagation fix

The gate relaxation here is an `ipe`-time **gate** relaxation; the companion
`Sync`-propagation fix (the generic-fn bound synthesizer gaining a `with_sync`
arm for a type var that flows into a `Send + Sync` carrier slot) is an emit-time
**bound** completion. They are orthogonal, but the `Codec.maybe` / `Codec.dict`
decode path needs both:

- The gate relaxation makes the builder *type-check and accept* at `ipe` time (the
  capture/reuse gates stop firing).
- A builder that threads a free type var `a` through the optional/nullable decode
  path (`JsonDecP.optional` / `Db.Decode.optional`, which `maybe`/`dict` use) then
  needs the emitted generic signature to carry `T: Sync` — the exact gap the
  `Sync`-propagation fix closes. Without it those two builders are an
  accept-then-`cargo`-fail on `T: Sync` **regardless** of this relaxation.

So: the record / `taggedUnion`/`varN` / `enum` builders (this doc's cases) do
**not** require the `Sync` fix — their type vars do not route through the
optional-decoder `Sync` slot, and their emitted decoders are `Send + Sync` by the
`Arc` `Decoder` carrier. `Codec.maybe`/`dict` require the `Sync` fix to land first.
The implementation must gate the `maybe`/`dict` builder slice on it, and the SEAL
golden for those two must assert both that the gate accepts (this doc) and that the
crate `cargo`-builds under `IPE_E2E=1` (the `Sync` fix).

## 6 — Test plan (SEAL goldens)

Every newly-admitted case gets an `IPE_E2E=1` round-trip golden (accept + emit +
`cargo build` + run); every still-rejected case gets a fail-closed golden asserting
the exact diagnostic. Full unfiltered golden re-bless; byte-neutrality is expected
on the whole existing corpus except the fixtures that graduate from red to green.

Newly-admitted (must build + round-trip):

1. `codec_field_builder` — `object |> field … |> buildObject` over a 3–4 field
   record (`String`/`Int`/`Bool`/`Maybe`); encode/decode round-trips. Exercises P1
   (record with `dec : Decoder`, captured into the `field` composing closure).
2. `codec_map_bijection` — `Codec.map f g` over a newtype; round-trips. P1.
3. `codec_enum` — `Codec.enum [(Bronze,"bronze"),…]` on a nullary enum; encodes to
   TEXT and decodes back; the missing-constructor lint stays a warning. P2 (generic
   value capture in the `enumName` filter).
4. `codec_tagged_union` — `taggedUnion` + `var1`/`var2` over a 2–3 variant ADT;
   round-trips the `["Tag", arg…]` wire form. Already-admitted arm-binder path + P1.
5. `decoder_record_capture` — a bare `Codec`/record captured into a returned
   closure (not only a kernel arg) and the closure reused. P1 + §2 capture.

Still-rejected (must fail closed with the named code):

6. `capture_task_forward` — a record/closure capturing a `Task`/`Cmd`/`Sub` and
   forwarding it ⇒ `IPE-L0126`. (Probed: a `Task`-field record forwarded through a
   closure is rejected today; it must stay rejected.)
7. `reuse_record_with_task` — a record with a `Task` field reused as a value ⇒
   `IPE-L0127` (the shipped `fn_record_reuse_mixed` already covers this; keep it).
8. `capture_foreign_handle` — an FFI opaque handle captured/forwarded ⇒ `L0126`
   (or `IPE-L0130` on the reuse path).
9. `l0115_refutable_destructure` — `let (Just x, y) = e` and a multi-arm record
   `case` ⇒ `IPE-L0115` (pin the unchanged L0115 boundary).

Over-narrow / over-broad checks:

- **Over-narrow (regression):** the shipped `fn_record_reuse_promoted` /
  `_escapes`, `decoder_storage_reuse`, `decoder_record_destructure`, and the
  `json_dec_*` pipeline goldens must re-emit byte-identically (the record/decoder
  paths they cover are unaffected by P1/P2/P3 except where they were the point).
- **Over-broad (soundness):** cases 6–8 must stay rejected; a bare `Generic`
  *inside a composite* capture (`List a` captured-and-forwarded through a
  non-`Clone` frontier) must stay gated (P2 is bare-`Generic` only). A `Task`-field
  record must stay `NonClone`.
- **SEAL sweep:** build + `cargo build` the full existing corpus under `IPE_E2E=1`
  to confirm no accept-then-`cargo`-fail was opened at a carrier frontier.

## 7 — Risks, open questions, recommended slicing

### Risks

- **Byte-drift from P3 (thunk retirement).** Retiring the `Decoder` rebuild-thunk
  changes the emitted shape of any `Decoder`-typed `let` that previously thunked.
  Some `json_dec_*` goldens may re-bless from a `(d)()` thunk to a `.clone()` /
  direct read. This is expected and cheap (golden regen is automated); it must be
  reviewed as a *shape* change, not waved through — confirm each re-blessed golden
  still round-trips under `IPE_E2E=1`, and that no thunk retirement lands on a
  `FnOnceChain` pipeline slot (which must keep its thunk).
- **P2 single-source coupling.** The bare-`Generic` capture admission and
  `param_is_multiuse_clonable` both depend on `render_fn_generics`' unconditional
  `with_clone`. A test must assert their agreement, or a future tightening of the
  emitted generic bound silently reintroduces a double-move / clone-non-`Clone`.
- **Carrier-frontier corner (the standing FCF risk).** The `Arc`↔`Box` fill/read
  reconciliation is total by construction, but a builder shape that mixes a stored
  `SharedFun`/`Decoder` read with a sibling `Box`-carried function value at one
  unification slot is where a subtle `E0308` could hide. The SEAL sweep on the full
  corpus is the guard; treat the first frontier `cargo`-fail as evidence the
  frontier is incomplete, not a one-off.

### Open questions

- Does any shipped golden rely on the `Decoder` rebuild-thunk shape as its
  *asserted* behaviour (not merely incidentally)? If so it graduates; enumerate
  them in the impl PR so the re-bless is a reviewed set, not a silent diff.
- Should P2 be widened to a `List Generic` / `Tuple(.., Generic)` capture in a
  follow-up? Deliberately out of scope here (wider blast radius); the `enum` builder
  needs only the bare-`Generic` value capture. File separately if a later HOF
  library needs the composite-generic capture.

### Recommended implementation slicing

1. **P1 — `Decoder` `Clone` carrier.** Flip `carrier_is_clone` +
   `clone_class` for `IrType::Decoder`. Add `codec_field_builder` /
   `codec_map_bijection` goldens. Smallest, highest-leverage; unblocks the record
   builder and `map`. Empty-golden-diff on everything except the graduating
   fixtures (guardian-check the full corpus).
2. **P3 — retire the `Decoder` thunk + register `Decoder` `let`s as promotable
   binders.** Depends on P1. Re-bless the `json_dec_*` shape changes; add
   `decoder_record_capture`. Confirm `FnOnceChain` slots untouched.
3. **P2 — bare-`Generic` value capture admission.** Independent of P1/P3. Add
   `codec_enum` golden + the single-source agreement test with
   `param_is_multiuse_clonable`.
4. **`taggedUnion`/`varN` golden** (mostly already-admitted; verify + pin).
5. **`Codec.maybe`/`dict` slice — gated on the `Sync`-propagation fix.** Do not
   attempt before that `Sync` bound lands; its SEAL golden asserts both the gate
   accept and the `T: Sync` `cargo`-build.

Each slice is guardian-reviewed (soundness-gate relaxation) and lands with its
fail-closed goldens (cases 6–9) so the boundary is pinned before, not after, the
admit is widened.

# First-class functions (Approach A) — turnkey implementation plan

Status: implementation plan. This operationalizes the vetted design; it does not
re-decide it. It assumes zero prior context and is executable slice by slice.

Read the two authoritative designs first — this plan links to them rather than
restating their reasoning:

- `first-class-functions-design.md` — the chosen representation (Approach A: a
  total `Fun`→`SharedFun` storage-carrier normalization) and its full rationale,
  the derive-capability posture, the frontier machinery, and the SEAL
  byte-neutrality argument. The section numbers cited below (§4.1, §4.3, §4.4,
  §4.7, §8) are that document's.
- `codec-combinator-compiler-unblock.md` — the recon that maps each codec
  combinator to the exact gap that blocks it, with per-gap difficulty, blast
  radius, and the single load-bearing risk. The "Gap 1 / Gap 2 / Gap 3" and
  "slice order" language below is that document's.
- `codec-and-store-design.md` — the end-state `.ipe` combinator surface and the
  `Ipe.Db.Store` design that slices 4 and 6 realize.
- `codec-auto-derive-design.md` — the compile-time `Codec.auto` derive that slice
  5 realizes (partly superseded by `codec-and-store-design.md`; that file is the
  SSOT for the derive decision, the auto-derive doc for its elaboration sketch).

## Goal

Make the whole higher-order-function idiom family — records of functions, unions
whose payload is a function, and collections of functions — compile through the
Rust backend, without surface-syntax change, without giving up concrete-over-
generic codegen, and with byte-exact re-emission of every existing golden. The
concrete acceptance is: the full pure-`.ipe` codec combinator surface
(`object`/`field`/`buildObject`, `enum`, `taggedUnion`/`varN`, `maybe`/`list`/
`dict`, `map`) and the `Ipe.Db.Store` layer compile and pass their round-trip
property test, with no new trusted kernels for the combinators themselves.

## Architecture — where the work lands, and what already shipped

The mechanism is a single total IR-type canonicalization: a function type in a
**storage position** (under `Record`, `Enum`, `Tuple`, `List`, `Set`-element,
`Dict`-value, `Maybe`, `Result`) is always `IrType::SharedFun` (`Arc<dyn Fn + Send
+ Sync + 'static>`, which is `Clone` via a refcount bump); a function in a
**direct position** (parameter, `let` binding, callee, bare return) stays
`IrType::Fun` (`Box<dyn Fn>`). Because the carrier is a pure function of the type
tree, every occurrence of a shape agrees on `Arc` vs `Box` by construction — no
containment analysis. Where the two carriers meet, a total O(1) adapter converts
(design §4.4).

Ground truth of the current tree (verify before starting; the design document's
staging labels are not the state of the code — read the code):

- **Record normalization — LANDED.** `normalize_record_fun_carriers`
  (`src/compiler/lower/src/lower.rs:7853`) is the unconditional generalization of
  the old promotion flip and is wired at the record-type lowering sites
  (`lower.rs:11272`, `:12328`) and the value side via
  `promote_fn_field_value_carrier` (`:7930`, called at `:13144`, `:13359`,
  `:14469`). The record gate `embeds_nonderivable_function` (`lower.rs:1231`)
  already treats a bare `Ty::Fun` record field as storable (its `Ty::Record` arm
  recurses per-field and no longer flags a direct arrow).
- **Enum-payload normalization — LANDED.** `normalize_enum_payload_fun_carrier`
  (`lower.rs:7906`) is wired at `:9609`, `:11071`, `:12143`; a *bare* function
  under an enum-like head is already tolerated (`is_enum_like_con_head`,
  `lower.rs:1200`). The enum hand-written `Clone` tier exists
  (`emit_enum_clone_impl`, `src/compiler/backend/rust/src/emit_types.rs:887`;
  gated by the `is_clone && !is_derivable` tier read at
  `src/compiler/backend/rust/src/lib.rs:1272`, `:2002`).
- **The Gap-3 `Send` matcher for generic `Decoder`-returning helpers — DONE.**
  The `body_boxes_generic_callback`-shaped obligation walk (`lower.rs:5019`) plus
  `render_bounds` in `src/compiler/backend/rust/src/emit_expr.rs` already stamp
  `Send + 'static` on a generic tvar flowing into a `Decoder<E, tv>`. **Do not
  re-plan Gap 3.** This plan starts from a tree where record + enum-payload FCF
  and the Gap-3 `Send` obligation are in place.

What is therefore genuinely open, and what this plan covers:

1. **Collection carrier (Gap 1, IPE-L0114).** The `Ty::Con` collection arm of
   `embeds_nonderivable_function` (`lower.rs:1273`, the `List`/`Dict`/`Set` blanket
   check) still flags a function element/value, and `con_payload_carries_function`
   (`lower.rs:1303`) surfaces `Feature::CtorPayloadFunction`. The type-side
   normalizers do **not** yet flip under a collection head
   (`normalize_enum_payload_fun_carrier` explicitly leaves a collection/tuple
   payload on the `Box` carrier; `normalize_record_fun_carriers` recurses into
   `List`/`Dict`/`Set`/`Tuple` but only flips a *record field*, not a collection
   *element*). This slice makes the flip total for collection/tuple positions and
   adds the kernel-registry element-capability audit.
2. **Capture normalization (Gap 2, IPE-L0126).** `rewrite_captured_clones`
   (`lower.rs:2520`) allows a non-`Clone` captured symbol bare only in direct
   callee position at depth 0; forwarding it is `Err(Feature::NonCloneCapture)`.
   The promotion machinery (`promotable_fn_binders` `lower.rs:7813`,
   `deferred_fun_captures` `:7821`, `apply_param_move_ownership` `:11395`,
   `fun_value_arc_promotable`) exists but defers-then-re-raises L0126 for captures
   whose binder cannot carry the promotion. This slice makes binder promotion
   unconditional-on-demand and extends it to pattern binders.
3. Exploitation and consumers (slices 4–6): the full `.ipe` combinator surface,
   the `Codec.auto` compile-time derive, and `Ipe.Db.Store`.

Because record + enum-payload + the Gap-3 `Send` obligation are landed, the
recon's "prerequisite" FCF base is already satisfied; **Slice 1 below is a
verification-and-consolidation slice, not a build-from-scratch slice.**

## Global constraints (apply to every slice; from `PRINCIPLES.md` + `AGENTS.md`)

- **THE SEAL.** If `ipe` accepts a program (exit 0), the emitted Rust MUST
  `cargo build`. Every new acceptance path fails closed at `ipe` time. A carrier
  mismatch reaching a `Box`-typed slot the reconciliation walk misses is the SEAL
  break class (design §8, risk 1) — treat the first carrier-frontier E2E failure
  as evidence the rule is not yet total, never as a one-off patch.
- **Empty-golden-diff gate, FULL unfiltered corpus.** The `Fun`→`SharedFun` rule
  is the identity on every stored-function-free program. After any slice that
  touches the normalizer or a frontier, the full golden corpus (`tests/golden/*/`,
  byte-compared `main.rs` + `Cargo.toml`; ~530 fixtures) must re-emit
  byte-for-byte, EXCEPT the deliberately-graduated fixtures that slice names.
  Regenerate with `cargo run -p regen-goldens` — on an unchanged-behaviour program
  it is a no-op and `git status` stays clean. **A single unexpected byte drift
  means the rule is not yet total: STOP and fix the generative cause; never
  re-bless to make the diff go away** (design §4.7; recon "single biggest risk").
- **No shortcuts (§0).** Never edit a fixture/golden/gate to dodge a gap. Two
  outcomes only: root-cause, or file an honest tracked blocker.
- **No new trusted kernels for the combinators.** The combinators are pure `.ipe`
  over the already-audited `Json.Encode`/`Decode`, `SqlFragment`,
  `valid_sql_ident` surface. Native kernels stay reserved for that Security-
  critical surface the combinators *compose*, never the combinators themselves
  (recon "the native-kernel alternative, and why it loses").
- **Anti-drift discipline.** Any kernel signature/scheme/arity/naming/pretty-
  print change updates every mirrored site and keeps its tripwire (`AGENTS.md`
  "Registering a kernel"). Slice 3's element-capability tag is a new registry fact
  and gets its own coherence test.
- **Comments say WHAT/WHY, no archaeology** (no dates, issue/PR numbers,
  process-stage labels) outside `docs/adr/`. No public reference to the private
  reference implementation.
- **TDD.** For every slice, write the failing test first (a `.ipe` fixture that
  today errors with the named diagnostic, or a `cargo`-fails E2E), watch it fail
  for the expected reason, then make the change, then watch it pass.

## The mandatory per-slice gate

Every slice, before it is called done, runs the full gate (timeout-bounded, under
`~/.cache/ipe/<slice>-target` per `dev-ops.md`). The blocks below are the exact
verified commands (`static_emit` is a real test target in `src/ipe-cli/tests/`;
`IPE_E2E`/`IPE_RUNTIME_DIR` are the real env vars; `-p ipe` is the CLI crate;
`regen-goldens` is a real package):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings   # bans panic!/unwrap!/todo! even in tests
cargo nextest run --profile ci -p ipe                    # CLI integration + goldens (ci profile: 600s)
cargo nextest run -p <each-touched-crate>                # lower, backend/rust, ir, kernels as touched
IPE_E2E=1 IPE_RUNTIME_DIR=<rt> cargo nextest run -p ipe  # THE SEAL: ipe-accepts ⇒ cargo-builds
cargo nextest run -p ipe --test static_emit              # musl static path
```

Plus, for any slice touching the normalizer or a carrier frontier: the
**empty-golden-diff over the full unfiltered corpus**:

```bash
cargo run -p regen-goldens
git status --porcelain tests/golden        # MUST be empty outside the slice's named graduated fixtures
```

Plus a **panic-scan** (`rg -n 'unwrap\(\)|expect\(|panic!|unreachable!|todo!'`
over the touched `.rs`) — every hit outside `#[cfg(test)]` is a blocker; use
`assert!(matches!(…))` for test asserts.

**Security-guardian requirement.** Slices 3 (kernel-registry capability audit — a
soundness frontier), 4, 5, and 6 (language/data boundaries: codec decode of
untrusted input, `Codec.auto` elaboration, and the SQL-generating Store) are
language-boundary slices and MUST pass a `security-soundness-guardian` review
before merge, briefed to read `PRINCIPLES.md` + `docs/internals/dev-ops.md` (per
the standing rule that every dispatched agent references both).

## The two load-bearing risks the recon named, and which slice guards each

- **Arc↔Box carrier-frontier reconciliation (design §8 risk 1; recon "single
  biggest risk").** A stored `Arc<dyn Fn>` read out and flowed into a `Box`-typed
  slot (a kernel parameter) needs the `Box::new(move |a,…| shared(a,…))` wrapper,
  emitted by the same reconciliation walk that handles `SharedFun`-vs-`SharedFun`
  today. A missed frontier is an `E0308`/`ipe`-0-then-cargo-fail. **Guarded by:**
  Slice 3's frontier golden matrix (a `List`-of-functions value passed into every
  list/dict kernel that takes a boxed element callback) run under `IPE_E2E=1`, and
  the empty-golden-diff gate catching any incidental adapter-formatting drift.
- **Enum `Clone`-tier × derive-demotion interaction (design §8 risk 2).** A record
  containing a record containing a function must lose `PartialEq`/`Debug`
  *transitively*; a shallow `is_derivable` would emit a derive that fails on the
  inner field. The derive flags must be a fixpoint over the type graph. **Guarded
  by:** Slice 1's transitive-derive verification fixtures (nested composites
  carrying a function at depth ≥ 2) plus the empty-diff gate. This is already
  landed for records/enums; Slice 3 extends the fixpoint over collection element
  types and must re-verify it, not assume it.

## Slices

Ordering follows the recon's "proposed slice order". Slices 1–3 are strictly
sequential in construction (each sits on the prior's carrier rule). Slices 4–6 are
sequential and gate on 1–3 (and each other): 4 needs the combinators to compile, 5
elaborates to 4's builder chain, 6 consumes 4/5's codec.

---

### Slice 1 — FCF base: verify + consolidate record + constructor-payload carrier

**What it unblocks:** nothing new by itself — it certifies the landed record +
enum-payload flip is total and correct as the foundation the later slices stand
on, and closes any residual record/enum frontier hole before collections widen the
surface. (The recon calls this "the general design's own base"; in the current
tree it is already in, so this slice is a verification-and-hardening pass.)

**Files to inspect/modify (site-precise):**
- `src/compiler/lower/src/lower.rs`: confirm `normalize_record_fun_carriers`
  (`:7853`) and `normalize_enum_payload_fun_carrier` (`:7906`) are invoked on
  every path that lowers a record type (`:11272`, `:12328`) and an enum payload
  (`:9609`, `:11071`, `:12143`), and that `promote_fn_field_value_carrier`
  (`:7930`) is invoked on every record/enum *value* construction (`:13144`,
  `:13359`, `:14469`). A record/enum type built on a path the value normalizer
  does not cover is an `Arc`-vs-`Box` `E0308` — add the missing call if found.
- `src/compiler/backend/rust/src/emit_types.rs`: confirm `emit_enum_clone_impl`
  (`:887`) is emitted for every `is_clone && !is_derivable` enum and that the
  transitive `is_derivable`/`is_clone` fixpoint (`lib.rs:1272`, `:2002`) already
  demotes a record-in-record-with-function.

**Transformation:** none expected if the audit is clean. If a frontier is missing,
route the value through the existing normalizer (do not add a special case).

**Tests (TDD — write failing first):**
- Add positive goldens: (a) a dispatch-table record `{ run : Int -> Int, name :
  String }` stored and reused; (b) a record returned from a function (the
  graduated "escape" case — under a total rule, escaping is legal); (c) a record
  carrying a function nested one level under `Maybe`; (d) **transitive-derive
  guard**: a record whose field is a record whose field is `Int -> Int`, asserting
  the outer struct derives neither `PartialEq` nor `Debug` and gets a hand-written
  `Clone`. Each new golden's `main.rs` must be produced by `regen-goldens`, never
  hand-written.
- Graduate the fn-value-reuse gate fixtures the design §4.7 names
  (`fn_record_reuse_escapes`, `fn_record_reuse_mixed`): the escape case flips from
  fail-closed to a positive golden; the mixed record keeps its `Task` field so its
  single-use compiles and its reuse still fails L0127. Move them out of the gate
  test list into the golden corpus.

**Acceptance:** a record or single-constructor enum whose field/payload is a bare
function compiles, is reusable (`Clone`), and stringifies with the `<function>`
placeholder. `Codec a`'s two-function record (`{ enc, dec }`) already compiles —
this slice adds the reuse/escape/nesting coverage that proves the base is total.

**Gate:** full per-slice gate above + **empty-golden-diff over the full corpus**
(only the two graduated reuse fixtures + the four new positive goldens may appear
in `git status`). Transitive-derive fixtures are the risk-2 guard.

---

### Slice 2 — Capture normalization (Gap 2, IPE-L0126)

**What it unblocks:** every combinator that closes over an inner encoder/decoder/
`eqV` and hands it *onward* rather than calling it in place — `maybe`, `list`,
`dict`, `taggedUnion`/`varN`, and the forwarding `field` step of the record
builder. (`map` already survives because it only *calls* its captures.)

**Files to modify (site-precise):**
- `src/compiler/lower/src/lower.rs`, `rewrite_captured_clones` (`:2520`): today a
  symbol in `noncl_set` is `Err(Feature::NonCloneCapture)` unless it is the direct
  callee at depth 0 (`:2534`, `:2573`). Under the total rule a captured *stored-
  function* value is `CloneOk` outright, so a fn-typed captured binder must be
  promoted to the `Arc`/`SharedFun` carrier rather than rejected. The change: stop
  routing a promotable fn-typed capture into `noncl_set`'s reject arm; route it
  into the shadow-rebind promotion instead.
- The promotion routing: `promotable_fn_binders` (`:7813`),
  `deferred_fun_captures` (`:7821`), `with_promotable_fn_binders` (`:11365`), the
  deferral insert (`:11342`), and `apply_param_move_ownership` (`:11395`, which
  currently *removes* a `deferred_fun_captures` entry `:11402`). Make binder
  promotion **unconditional-on-demand**: every fn-typed `let`/param binder that is
  captured or forwarded is promoted to `SharedFun` (`fun_value_arc_promotable` is
  the eligible set), deleting the "defer then re-raise L0126" path. A non-fn
  non-`Clone` capture (`Task`, `Decoder` value) keeps its L0126 gate unchanged.
- Extend promotable binders to **destructure / match-arm patterns** — the `varN`
  projection binds the inner codec by destructure (`Codec r -> …`), so a pattern
  binder that binds a fn-typed value must also be promotable. Touch the
  pattern-binding arms of `rewrite_captured_clones` (`:2665`, the
  `pat_binds_any_in` branch) and `with_promotable_fn_binders`.

**Root-cause framing (do not patch the symptom):** the structural property to
establish is "a captured function value is `Clone` by carrier", which makes
forwarding it a refcount bump instead of a move that collapses the enclosing
closure to `FnOnce`. Establishing that property (unconditional `SharedFun`
promotion of captured fn binders) deletes the whole L0126-for-functions class; do
not special-case the forwarding site.

**Tests (TDD):**
- Failing first: a `.ipe` fixture where a closure forwards a captured function to
  another HOF (e.g. `\inner -> List.map (\x -> inner x) xs` where `inner` is
  itself passed onward, not just called) — today `IPE-L0126`. And a `varN`-shaped
  fixture that destructures `Codec r` in a case arm and forwards `r.dec`.
- After the change: both compile; add as positive goldens.
- Regression: a genuinely non-`Clone` non-fn capture (a bare runtime `Decoder`
  value forwarded) must STILL fail `IPE-L0126` — assert the gate is narrowed to
  functions, not removed.

**Acceptance:** `maybe`, `list`, `dict`, `taggedUnion`/`varN`, and the forwarding
`field` step compile at the `.ipe` level (they still need Slice 3 for the
`List`-accumulating builder, but the capture-forwarding half is unblocked here).

**Gate:** full per-slice gate + empty-golden-diff. The carrier is transparent
(`Arc` and `Box` share `Send + Sync + 'static`), so the set of legal captures is
unchanged — assert no *new* capture shape is admitted, only a new carrier.

---

### Slice 3 — Collection carrier (Gap 1, IPE-L0114) + kernel-registry capability audit

**What it unblocks:** the applicative record builder — `object`/`field`/
`buildObject` accumulate per-field encoders/decoders into a growing `List` of
field contributions carried inside the builder's type until `buildObject` folds
them; also `list`, `dict`. A `List (a -> Value)` / `List (String -> Result …)` in
a builder payload is the exact shape L0114's collection arm rejects.

**Files to modify (site-precise):**
- `src/compiler/lower/src/lower.rs`, `embeds_nonderivable_function` (`:1231`), the
  `Ty::Con { args, .. }` collection arm (`:1273`): a function *element* of a
  `List`/`Set`, or *value* of a `Dict`, is now storable on `SharedFun`. Narrow the
  blanket check so a bare function element/value is no longer flagged; keep
  flagging a genuinely non-storable nested carrier (a `FnOnceChain`, a runtime
  `Decoder` value). Update `con_payload_carries_function` (`:1303`)
  correspondingly.
- Extend the type-side flip to collection/tuple positions:
  `normalize_record_fun_carriers` (`:7853`) currently recurses into
  `List`/`Set`/`Dict`/`Tuple`/`Result`/`Maybe` but only flips a `Fun` that is a
  *record field*; add the flip for a `Fun` that is a collection **element / dict
  value / tuple element / set element**. Do the same in
  `normalize_enum_payload_fun_carrier` (`:7906`, which today leaves a
  collection/tuple payload on `Box`). The rule stays a pure function of the type
  tree.
- Value side: `promote_fn_field_value_carrier` (`:7930`) handles a record field
  value; add the element/value analogue so a `List`/`Dict`/`Set`/tuple *literal*
  of functions constructs its elements with `Arc::new`/`SharedLambda`, matching
  the flipped element type (else `Arc`-vs-`Box` `E0308` at the literal).
- **Kernel-registry capability audit (a new SSOT fact).** In
  `src/compiler/kernels/` add an explicit per-kernel **element-capability tag** to
  each list/dict/set kernel's `KernelDef` recording which element capability it
  requires (`CloneOk` — the default, sound for `Arc` elements — vs
  `RequiresPartialEq` for `member`/`sort`/`unique`/… vs `RequiresOrd` for keyed
  ops). Kernels tagged `Requires*` gate on a fn-embedding element type with the
  equality diagnostic (the region-typed gate, same mechanism as
  `reject_function_through_type_var`); the rest are sound as-is because `Arc`
  elements are `CloneOk`. This makes the requirement explicit in the registry
  rather than implicit in the hand-written Rust (make-invalid-states-
  unrepresentable). Add the coherence tripwire: a test asserting every
  list/dict/set kernel carries a capability tag (an untagged one is a
  compile-time/CI error), mirroring the existing scheme/arity coherence oracles.
- Carrier reconciliation: confirm the `reconcile` walk in the backend
  (`src/compiler/backend/rust/`) emits the `Box::new(move |a,…| shared(a,…))`
  wrapper for an `Arc` element read out and passed into a `Box`-typed kernel
  parameter. This is the risk-1 frontier.

**Tests (TDD):**
- Failing first: (a) a `List (Int -> Int)` fold-apply pipeline; (b) an
  `oneOf`-shaped combinator over a stored `List` of function-carrying values; (c)
  a `Dict String (Event -> Msg)` dispatch; (d) the `object`/`field`/`buildObject`
  builder from `codec-and-store-design.md` §"record builder". All today
  `IPE-L0114`.
- **Frontier golden matrix (risk-1 guard):** a `List`-of-functions value passed
  into *every* list/dict kernel that takes a boxed element callback (`map`,
  `filter`, `foldl`, `foldr`, `concatMap`, `Dict.map`, …), each built and run
  under `IPE_E2E=1` to prove the `Arc → Box` wrapper is emitted at every frontier.
- Capability-gate tests: `List.member` / `List.sort` / `Set.insert` over a
  fn-embedding element type must fail with the equality/`Ord` diagnostic
  (fail-closed), not cargo-fail; `List.map` / `List.foldl` over the same must
  compile.
- Untagged-kernel tripwire test: adding a list kernel without a capability tag
  fails the coherence oracle.

**Acceptance:** `object`/`field`/`buildObject`, `list`, `dict` compile; a stored
`List`/`Dict` of function values is `Clone`, foldable, and mappable; equality-
requiring kernels over fn-embedding elements fail closed at `ipe` time.

**Gate:** full per-slice gate + **empty-golden-diff over the full unfiltered
corpus** (this is the highest-risk slice for byte drift — the frontier wrapper's
formatting must be identity on programs that had no stored collection-of-functions)
+ security-guardian review of the capability audit (a soundness frontier).

---

### Slice 4 — Replace minimal `Ipe.Codec` with the full pure-`.ipe` combinator surface

**What it unblocks:** the shipped end-state of `src/stdlib/Ipe/Codec.ipe` — the
full combinator surface expressed as pure `.ipe`, no new kernels. Gated on the
round-trip property test and on the `Ipe.Db.Store` design's needs (slice 6).

**Files to modify:**
- `src/stdlib/Ipe/Codec.ipe`: replace the minimal module (currently exposing
  `Codec(..), toValue/toJson/fromJson/fromJsonSafe, string/int/bool/float, map`)
  with the full surface from `codec-and-store-design.md` §"The surface":
  `maybe`, `list`, `dict`, `object`/`field`/`buildObject`, `enum`, `taggedUnion`/
  `Variant`/`var0..var3`, `fromJsonSafe`, `shape`/`ColType`/`Shape`. All are pure
  Ipê over the existing `Ipe.Json.Encode.Value` / `Ipe.Json.Decode.Decoder`
  kernels — **no new kernels**. (To author `.ipe`, follow the language reference
  `src/ipe-cli/templates/AGENTS.md.in`, per `AGENTS.md`.)
- Honour the two divergence decisions the design records: `enum` takes the *whole*
  constructor→wire-name pair set and is total (a missing constructor is an
  `IPE-…` diagnostic, never an empty-string encode); `taggedUnion`'s top-level
  encoder is *derived from the variant list*, so each `Variant` carries both its
  encoder and decoder — one list, one source of truth, no second hand-written
  case-of on encode.
- `fromJsonSafe : Int -> Codec a -> String -> Result Error a` rejects input longer
  than `maxChars` *before* parsing (a size guard for untrusted bodies) and carries
  the mass-assignment security note (decode untrusted input into a dedicated input
  record, never straight into a persistence record).

**Tests (TDD):**
- **The round-trip property test is the acceptance gate** (`property-tests-design.md`
  is the harness): `fromJson codec (toJson codec x) == Ok x` for every codec built
  by the surface — primitives, `maybe`/`list`/`dict`, the `object` builder, `enum`,
  and `taggedUnion`. Write it failing (the combinators don't exist yet), then land
  the module.
- A golden for a representative user codec (the `User` record builder from the
  design) proving the emit is the expected concrete monomorphized code.
- Negative: `fromJsonSafe` rejects an over-limit body with `tooLargeError`, never a
  panic; a malformed document is an `Err`, never a crash.

**Acceptance:** the full combinator surface compiles and round-trips; the minimal
`Codec` is gone, replaced by the full pure-`.ipe` module with no new trusted
kernels.

**Gate:** full per-slice gate + the round-trip property test + security-guardian
review (untrusted-input decode boundary).

---

### Slice 5 — `Codec.auto` compile-time derive

**What it unblocks:** `Codec.auto blank` — a full codec for a record type without
the field-by-field pipeline, generated **at compile time** per record type (a
derived instance), with zero runtime reflection (`codec-auto-derive-design.md`;
decision SSOT in `codec-and-store-design.md` §"`Codec.auto`").

**Files to modify (site-precise):**
- `auto` cannot be a normal function — it has no runtime access to the witness's
  static field list. It is a **compiler-elaborated form**: at its call site the
  compiler reads the record's field list from the *solved* type and *replaces* the
  `auto` call with the concrete `object`/`field`/`buildObject` chain it derives.
  This is a canon/lower elaboration, not a kernel. Add the elaboration where the
  solved type is available at the call site — a new elaboration keyed on the
  `Codec.auto`/`autoCamel`/`autoWith` head; site it alongside the existing
  stdlib-form elaborations in `src/compiler/lower/` (or `canon` if the field list
  is only stable post-resolve — determine from where the solved record type is
  first fully known). `autoWith` takes a key-mapping; `autoCamel` is `autoWith`
  with the camelCase mapping baked.
- The elaboration emits exactly the Slice-4 builder chain, so it inherits the
  round-trip property by construction and adds no new emit strategy.

**Tests (TDD):**
- Failing first: `Codec.auto User` where `User` is a record — today unresolved/
  unsupported. After: it elaborates to the same codec the hand-written builder
  produces.
- **Equivalence golden/property:** the `auto`-derived codec and the explicit
  `object`/`field`/`buildObject` chain for the same record produce byte-identical
  emit (or at minimum behaviourally-identical round-trip) — the design's claim that
  `auto` is a shorthand for the explicit chain.
- Negative: `auto` on a non-record type is a typed diagnostic at elaboration time,
  fail-closed, never a deferred cargo-fail.

**Acceptance:** `Codec.auto`/`autoCamel`/`autoWith` produce a working codec for a
record type; the emit is concrete/monomorphized (no `dyn Any`, no runtime
reflection, no emitted struct tag, no runtime constructor registry).

**Gate:** full per-slice gate + equivalence test + security-guardian review (a
compile-time elaboration boundary — must fail closed on any shape it cannot derive,
never emit an incomplete codec).

---

### Slice 6 — `Ipe.Db.Store`

**What it unblocks:** the persistence layer from `codec-and-store-design.md` Part
2 — a `Store a` is a codec plus DB-only facts, driving typed, injection-safe CRUD.

**Sub-slices (land in order; each is its own gate cycle):**

1. **`Store a` handle + typed `ColumnSpec`.** New `src/stdlib/Ipe/Db/Store.ipe`
   exposing `Store a` (a codec + `table : String` + `spec : List ColumnSpec`) and
   the typed `ColumnSpec` ADT (`codec-and-store-design.md` §"Typed column specs").
   Column facts are a *typed ADT*, never stringly — the key divergence from prior
   art. Refinement is `ColumnName -> Store a -> Store a` appending one typed
   `ColumnSpec`.
2. **`Cond` → `SqlFragment` (injection-safe).** The query builder: a `Cond` ADT
   that lowers to a `SqlFragment` via the existing `Ipe.Db.Sql` surface (`sql_eq`,
   `sql_and`, `sql_or`, `sql_not`, `sql_in_list`, `sql_like`). Every identifier
   goes through the validated `Sql.column` / `valid_sql_ident` surface; every value
   is a parameterised bind. **NEVER `Ipe.Db.Unsafe`** for the built-in `Cond` path
   — the unsafe hatch stays reserved for the documented raw-`SqlFragment` escape
   (`selectRaw`), never the query builder. This is the Security-critical sub-slice:
   fail closed on any identifier `valid_sql_ident` rejects.
3. **CRUD.** `create`, `insert`/`insertMany`, `upsert`, `update`, `get`, `all`,
   `delete`, `selectRaw` (`codec-and-store-design.md` §CRUD). The write path takes
   a typed `a`, so a client can never set a generated column (structural
   mass-assignment defence). Identifiers validated, values bound.
4. **`migrate` fail-closed.** `Store.migrate : Db -> Store a -> Task Error (List
   String)` — additive-only, idempotent. It never drops or rewrites a column; a
   destructive schema delta is rejected (fail closed), not applied.

**Files:** `src/stdlib/Ipe/Db/Store.ipe` (new); consumes the audited
`Ipe.Db`/`Ipe.Db.Sql` runtime (`SqlFragment`, `Sql.column`, `valid_sql_ident`,
`Db.findWhere`, parameterised binds) — no new trusted DB kernels beyond that
surface.

**Tests (TDD):**
- Failing first per sub-slice: a `.ipe` program building a `Store User` from
  `userCodec`, refining a column, and running each CRUD op against a test DB (an
  E2E fixture under `IPE_E2E=1`).
- **Injection tests (Security, the load-bearing tests here):** a `Cond` over a
  hostile column name / hostile string value must produce a *parameterised*
  `SqlFragment` with the value bound, never interpolated — assert the emitted SQL
  contains a bind placeholder and the identifier passed `valid_sql_ident`; a
  hostile identifier is *rejected* at build, never smuggled into the SQL text.
- `migrate` idempotence: running it twice is a no-op; a destructive delta is
  rejected with a typed `Error`.
- Mass-assignment: the write path cannot set a generated column (the type forbids
  it).

**Acceptance:** `Store a` CRUD compiles and runs against a real SQLite DB;
injection tests prove parameterisation; `migrate` is additive-only and idempotent.

**Gate:** full per-slice gate per sub-slice + the injection test suite +
**mandatory security-guardian review of every sub-slice** (this is the highest-
Security-tier slice; the guardian gates on no injection, no timing oracle,
fail-closed on any unvalidated identifier, and no `Ipe.Db.Unsafe` in the built-in
paths).

---

## Dependency graph and parallelism

```
Slice 1 (verify base) ──> Slice 2 (capture) ──┐
                     └───> Slice 3 (collections) ──> Slice 4 (combinators) ──> Slice 5 (auto)
                                                                          └──> Slice 6 (Store)
```

- Slice 1 first (certifies the landed base is total).
- **Slices 2 and 3 can run in parallel** after Slice 1 (both edit `lower.rs`; use
  isolated `CARGO_TARGET_DIR`s and reconcile the small `lower.rs` merge). Slice 4
  needs *both* landed (the builder needs Slice 3's collections and Slice 2's
  forwarding captures).
- Slice 5 depends on Slice 4 (elaborates to its builder chain).
- Slice 6 depends on Slice 4 (consumes the codec); sub-slice 6.3 (CRUD) depends on
  6.1 (handle) and 6.2 (`Cond`). Slices 5 and 6 can run in parallel after Slice 4.

## Explicitly out of scope (deferred, byte-visible optimizations)

Per `first-class-functions-design.md` §5 "Deferred": A′ single-carrier
unification, fn-pointer statics for capture-free values, and per-module
defunctionalization — all gated on a deliberate golden re-bless, none in this plan.
The polymorphic-signature `+ 'static` bound on a genuinely polymorphic
function-storing emitted signature is only needed if a combinator is emitted with
such a signature; the target codec idioms instantiate concretely, so it is
deferred until a polymorphic-combinator golden actually requires it (add it as a
Slice-3 companion only if a Slice-4/5 fixture surfaces the need).

## Resolved design decisions

These close the open questions the plan flagged. An implementer should treat them
as settled.

1. **`Codec.auto` elaboration site → lowering (post-inference).** Synthesize the
   derived codec in `lower`, where the solved record field-list (names + concrete
   types) is stable and generic instantiation is already resolved. Do not attempt
   it in `canon` (field types are not fully solved there).

2. **`toString` / `IpeStringify` on a function-carrying value → render a fully
   opaque `<function>`.** Do not embed the binding name, the type signature, the
   arity, or the captured environment. Rationale (Security first, then the
   no-runtime-reflection stance): a name or type-signature leaks internal symbols
   into logs/UI (information disclosure) and reintroduces runtime type information;
   a captured environment can hold a `Secret` and must never be printed; a
   function has no return value to show without applying it. Opaque `<function>`
   is the only safe rendering.

3. **Polymorphic `+ 'static` bound → deferred.** Target codec/Store idioms
   instantiate concretely, so no pinned golden requires a genuinely-polymorphic
   function-storing emitted signature. Add the handling only if a Slice 4/5
   fixture surfaces one; treat that as a companion to the collection slice, not a
   prerequisite.

4. **Functions in the TEA Model → hard-forbidden (a new slice).** A `Model` type
   that transitively contains a function is rejected at compile time with a
   diagnostic that points to defunctionalization (store a data ADT you interpret
   in `update`, not a closure). This is deliberately stricter than the existing
   `==`-on-function rejection: the Model must stay serializable, comparable
   (so `lazy`/diff work), inspectable, and testable, and defunctionalization
   cleanly expresses every real case (a queued continuation becomes a
   `PendingAction` variant). First-class functions remain values everywhere else;
   only the Model-type position is function-free. Preserve the existing
   equality-rejection of functions as well.

   **Added slice — Model-function-free gate.** A structural check on the Model
   type (recovered from the app config's `view`/`update`, the same place the
   routing `page`-field detection reads) that transitively rejects a function
   leaf, plus its explain diagnostic. Additive and independent of the collection
   and capture slices; gate it on the full-corpus golden diff (no existing
   function-free Model changes) plus a negative test (a function-carrying Model is
   rejected with the defunctionalization diagnostic) and a positive test (the
   defunctionalized equivalent compiles).

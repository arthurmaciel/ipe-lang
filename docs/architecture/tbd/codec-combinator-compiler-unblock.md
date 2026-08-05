# Codec combinators — the compiler work that unblocks pure-`.ipe` HOF factories

Status: design/recon, no implementation. This scopes the compiler gaps that stop
`codec-and-store-design.md`'s generic combinators — `object`/`field`/`buildObject`
(the applicative record builder), `enum`, `taggedUnion`/`varN`, and
`maybe`/`list`/`dict` — from being written as pure `.ipe` generic factories, and
recommends a path per gap.

The shipped slice (`src/stdlib/Ipe/Codec.ipe`) proved the design's own claim
false: the combinators are *not* "zero compiler involvement, no new kernels" on
today's compiler. Three language gaps block them. Two are already the subject of
a general design (`first-class-functions-design.md`, Approach A — total
`Fun`→`SharedFun` storage-carrier normalization); the third is a small,
independent, must-fix-anyway backend obligation. This doc is the codec-scoped
reading of that landscape, not a second general design.

## Why the shipped slice is minimal

`Ipe.Codec` today stores the decode side as a **clonable runner** `String ->
Result Error a` rather than a bare `Decoder a`, precisely because the runtime
`Decoder` is a single-shot, non-clonable carrier. Every primitive is a
self-contained `Codec` **literal** (`string`, `int`, `bool`, `float`), and `map`
is a bijection that closes over `r.enc`/`r.dec` and only ever **calls** them. That
is the exact subset the current compiler can lower: no factory *forwards* a
captured encoder/decoder, and no `Codec` value is ever collected into a `List`.
The moment a combinator does either, it hits one of the three gaps below.

## Gap 1 — `List` of function values in a constructor payload (IPE-L0114)

**What it blocks.** The applicative record builder. The natural shape of
`object`/`field`/`buildObject` accumulates per-field encoders/decoders into a
growing structure — a `List` of field contributions, each of which is (or
contains) a function value — carried inside the builder's own type until
`buildObject` folds them. A `List (a -> Value)` / `List (String -> Result …)` in
a builder payload is rejected.

**Where.** The gate is `embeds_nonderivable_function` +
`con_payload_carries_function` in `src/compiler/lower/src/lower.rs`, surfaced as
`Feature::CtorPayloadFunction` (diagnostic `IPE-L0114`, code table
`src/compiler/diagnostics/src/code.rs`, explain page
`src/compiler/diagnostics/explain/IPE-L0114.md`). Enum-like heads (`Maybe`,
`Result`, user unions) already tolerate a *bare* function argument
(`is_enum_like_con_head`); the residual gate is specifically the **collection**
head — `List`/`Dict`/`Set` of functions.

**Root cause (genuine, not over-conservative).** A `List (a -> b)` emits a real
`Vec<Box<dyn Fn(..) -> ..>>`, and `Box<dyn Fn>` is not `Clone` — several
collection kernels blanket-`.clone()` their element (`DictGet`, `ListMap`, …),
which is `E0599` on a non-`Clone` element. Accepting the value would be an
`ipe`-exit-0-then-cargo-fail SEAL break. The gate is sound.

**Fix.** This is exactly the collections step of `first-class-functions-design.md`:
normalize a `Fun` in `List`/`Dict`-value/`Set`-element position to `SharedFun`
(`Arc<dyn Fn>`, which *is* `Clone` — a refcount bump), plus the one-time
kernel-registry capability audit tagging which list/dict kernels genuinely need
`PartialEq`/`Ord` on elements (those gate with the equality diagnostic; the rest
become sound as-is because `Arc` elements are `CloneOk`). The mechanism
(`SharedFun`, the hand-written `Clone` tier, the carrier reconciliation) already
ships for contained record-of-functions.

- **Fixable in compiler:** yes. **Difficulty: L** (depends on the FCF design's
  record + constructor-payload steps landing first — enum-payload `SharedFun` and
  the enum hand-written-`Clone` tier — and carries the kernel-registry audit).
- **Blast radius:** medium–high. Touches `lower` (carrier normalization),
  `ir`/`backend` (enum `Clone` tier, reconciliation `Arc↔Box` wrapper), and the
  kernel registry (capability tags). SEAL byte-neutrality holds by construction
  for existing goldens (the rule is the identity on any type with no stored
  `Fun`), but it is a broad structural change, not a leaf edit.

## Gap 2 — a stored closure may CALL a captured function but not FORWARD it (IPE-L0126)

**What it blocks.** Every combinator that closes over an inner
encoder/decoder/`eqV` and hands it *onward* rather than calling it in place —
`maybe`, `list`, `dict`, `taggedUnion`/`varN`, and the `field` step of the record
builder. The bijection `map` survives only because it *calls* `r.enc`/`r.dec`; a
combinator that passes the inner codec's function to another HOF (a `List.map`
over sub-codecs, an applicative `map2`-chained decode threading the field
decoder) forwards the capture and is rejected.

**Where.** `rewrite_captured_clones` in `src/compiler/lower/src/lower.rs`: a
`noncl_set` (non-`Clone`) captured symbol is allowed **bare only in direct callee
position at closure depth 0** (`Fn::call` borrows `&self`); anywhere else — as an
argument, stored in a record, returned — it is `Err(Feature::NonCloneCapture)`
(`IPE-L0126`, explain page `IPE-L0126.md`). Related fail-close sites cluster
around the `deferred_fun_captures` / `promotable_fn_binders` routing (same file).

**Root cause (genuine).** Ipê closures lower to `Box<dyn Fn>`, which requires
captures to be `Clone` so the closure is re-callable (`Fn`, not `FnOnce`).
Forwarding a `Box<dyn Fn>` capture *consumes* it, collapsing the enclosing
closure to `FnOnce` where a `Fn` slot is required (`E0525`/`E0507`). Sound gate.

**Fix.** Same Approach A, its capture step. The compiler already carries an
`Arc`-promotion path (`SharedFun`/`SharedLambda`, `apply_param_move_ownership`,
`fun_value_arc_promotable`) that shadow-rebinds a fn-typed `let`/param binder to
the `Clone` `Arc` carrier when its reads exceed a bare `Box` — but it is wired
only to `promotable_fn_binders` (plain `let` names, def/lambda params) and still
*defers-then-re-raises* L0126 for captures whose binder cannot carry the
promotion. Under the total storage-carrier rule a captured stored-function value
is `CloneOk` outright, so the classifier stops special-casing pure-`Fun`
captures and promotes **every** fn-typed binder that is captured or forwarded —
the shadow-rebind mechanism, applied unconditionally on demand. The residue is
extending promotable binders to destructure / match-arm patterns (the `varN`
projection case binds the inner codec by destructure).

- **Fixable in compiler:** yes. **Difficulty: M** (the promotion machinery
  exists; the work is *deleting* the deferral special-case and broadening binder
  eligibility, plus the pattern-binder extension).
- **Blast radius:** medium, concentrated in `lower`. The carrier is transparent
  (`Arc` and `Box` share the same `Send + Sync + 'static` bounds), so the set of
  legal captures is unchanged; the change is which carrier a captured fn takes,
  not what may be captured.

**Coupling.** Gaps 1 and 2 are the same fix (Approach A) seen from two angles —
Gap 1 is stored functions in a *collection carrier*, Gap 2 is stored functions
across a *capture frontier*. Landing A's collection + capture steps clears both
together; they are not independently worth a bespoke fix.

## Gap 3 — missing `Send` bound on a generic `Decoder`-producing helper

**What it blocks.** Any generic `.ipe` helper that *returns* a `Decoder a` built
from a caller-supplied piece — the design's `custom`/`enum` decoders, and the
`dec` side of any combinator that keeps a bare `Decoder` in play. The emitted
crate fails `cargo build` — a SEAL break, distinct from the two `ipe`-time gates
above.

**Where.** `render_bounds` in `src/compiler/backend/rust/src/emit_expr.rs` builds
each emitted generic type parameter's bound list from a `BoundSet`. It *has* a
`has_send()` arm, but the `SEND` obligation is set (`with_send`,
`src/compiler/ir/src/ir.rs`) by exactly one matcher in `lower.rs` — the
`ws_open_msg_matcher` for a `WebSocket.onOpen` `msg` moved into a `Sub::Source`
closure. There is **no** obligation for "this generic tvar flows into a
`Decoder<E, tv>` value." The runtime `Decoder<E, T>` (`src/runtime/rust/src/json.rs`)
holds `run: Box<dyn Fn(..) -> IpeResult<E, T> + Send>`; its auto-derived `Send`
therefore requires `T: Send`. A generic helper emits `fn custom<A: Clone>(…) ->
Decoder<IpeError, A>` with **no `A: Send`**, so the `Decoder` value is not `Send`
wherever the runtime requires it, and `cargo` rejects it.

**Root cause (over-conservative, and independent).** Not a lowering-soundness
constraint — the `Send` mechanism (`BoundSet::SEND`, `render_bounds`) already
exists and is correct; only the *inference of the obligation* is too narrow. It
does not recognise the `Decoder`-return shape. This is a missing matcher, not a
missing capability.

**Fix.** Add a `BoundSet` obligation that stamps `Send` (and its companion
`'static`) on a generic type var that appears in a `Decoder<E, tv>` (equivalently
any opaque-boxed-wrapper carrier whose payload the runtime bounds `Send`) in the
emitted signature — the same structural-walk shape as `body_boxes_generic_callback`
already uses for the `'static` callback obligation. `Send + 'static` is satisfied
by every concrete Ipê type (owned, never borrows), so no caller-side failure.

- **Fixable in compiler:** yes. **Difficulty: S.** One new obligation matcher in
  `lower.rs` plus its `render_bounds` wiring (which already exists).
- **Blast radius:** small, localised to the bound-inference walk and the backend
  bound list. Additive — it only *adds* a bound to signatures that today
  cargo-fail, so it cannot regress a passing golden (a stricter bound on an
  already-concrete monomorphic instantiation is a no-op).
- **Independence:** fully independent of Gaps 1–2. It must be fixed *regardless*
  of the Approach-A decision, because even the native-kernel alternative (below)
  would emit generic `Decoder`-returning helpers.

## Recommendation

**Per gap:**

- **Gap 3 — fix in compiler now, unconditionally.** It is small, independent, and
  a live SEAL break that any codec design (pure-`.ipe` *or* native-kernel) trips.
  There is no native-kernel alternative that avoids it. Do it first.
- **Gaps 1 & 2 — fix the structure (Approach A), not a native kernel.** These are
  the two faces of one gap and are already the subject of a vetted general design
  whose mechanism (`SharedFun` promotion, the enum `Clone` tier, carrier
  reconciliation) largely ships. Fixing the language unblocks the codec
  combinators *and* every other HOF library the same idiom blocks — parser
  combinators (`Ipe.Url.Parser`, `Ipe.Parser`), property-test fuzzers, random
  generators, middleware chains, dispatch tables. That is the
  fix-structure-over-symptom, concrete-over-generic move the precedence order
  wants.

**The native-kernel alternative, and why it loses.** One could implement
`object`/`field`/`buildObject`/`enum`/`taggedUnion`/`maybe`/`list`/`dict` as
native combinator kernels over the existing `Json.Encode`/`Decode` runtime —
each a hand-written Rust function on the audited surface, sidestepping the
`.ipe`-factory carrier problem. It is *expedient* (unblocks Codec/Store without
waiting on Approach A) but it is the rejected posture on three counts: it adds a
whole family of new trusted kernels (each an anti-drift site — scheme, arity,
naming, pretty-print, module seal — per `AGENTS.md`), it institutionalises a
special case exactly like the rejected Approach E ("fix the symptom"), and it
does nothing for the next HOF library, which re-hits the same wall. It buys one
module at the cost of new trusted surface and a precedent against the general
fix.

**Overall verdict.** Fix Gap 3 in the backend immediately. Route Gaps 1 & 2
through Approach A of `first-class-functions-design.md` — do **not** open a
parallel native-kernel codec track. Native kernels stay reserved for what they
already own in the codec/store design: the Security-critical, already-audited
`Json.Encode`/`Decode`, `SqlFragment`, and `valid_sql_ident` surface the
combinators *compose* — never the combinators themselves.

The one case for the native path is scheduling: if Codec/Store must ship *before*
Approach A can land, the native combinators are a disclosed, deletable stopgap —
but they must be built to be *removed* when A arrives (documented as such,
covered by the same round-trip property test the `.ipe` combinators will use so
the swap is behaviour-preserving), never as the permanent home.

## Proposed slice order

1. **Codec-Send (Gap 3).** Backend `BoundSet` obligation for a generic tvar in a
   `Decoder<E, tv>` return; stamp `Send + 'static`. Independent, S, ships now.
   Unblocks the generic `custom`/`enum` decoder helpers on their own.
2. **FCF record + constructor-payload carrier normalization (prerequisite).** The
   `SharedFun` normalization for record and constructor-payload positions and the
   enum `Clone` tier — the general design's own base. Not codec-specific, but
   Gaps 1–2 sit on it.
3. **FCF collection-carrier normalization (Gap 1).** Flip under
   `List`/`Dict`-value/`Set`-element + the kernel capability audit. Unblocks the
   `List`-accumulating `object`/`field`/`buildObject` builder and `list`/`dict`.
4. **FCF capture normalization (Gap 2).** Unconditional binder promotion +
   pattern-binder extension. Unblocks `maybe`/`taggedUnion`/`varN` and the
   forwarding `field` step.
5. **Codec combinator slice (exploitation).** With 1–4 landed, replace the
   minimal `Ipe.Codec` with the full pure-`.ipe` combinator surface from
   `codec-and-store-design.md`; the round-trip property test is the acceptance
   gate. (The FCF design's polymorphic-signature `+ 'static` step is the general
   companion to slice 1 for the polymorphic-combinator case.)

## The single biggest risk to weigh

**Approach A's SEAL byte-neutrality claim is load-bearing and unproven at scale.**
The whole case for fixing the language over adding native kernels rests on the
total `Fun`→`SharedFun` rule being the *identity* on every type that has no
stored function today, so the entire existing golden corpus re-emits byte-for-byte
(only the two fn-value-reuse fixtures graduate). That reduces the review from a
correctness event to an empty-diff check — but only if the rule truly never
touches a compiling program's emitted bytes. The carrier reconciliation
(`Arc↔Box` frontier wrappers) and the enum `Clone`-tier interaction with existing
derive-demotion are where a subtle byte drift, or worse a real
`ipe`-0-then-cargo-fail on an untested carrier-frontier shape, could hide. A human
committing to the compiler-fix path should demand the record-step empty-golden-diff
gate be met on the *full* unfiltered corpus (not a subset) before trusting the
byte-neutrality argument, and should treat the first carrier-frontier E2E failure
as evidence the rule is not yet total — not as a one-off patch.

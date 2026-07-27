# Function-value reuse: relaxing IPE-L0127

**Design-review status.** This design passed an adversarial security/soundness
review only after a rewrite. The review found the first draft's core mechanism —
"instantiate the composite's type parameter as `Arc` at the promoted binding" —
unrealizable, because the type renderer is a pure function of the IR type with no
per-binding state and there is no Arc-fn type at all; and it found the composite
consumer story unsound as a local allowlist. The design below reflects the
required changes: a first-class `IrType::SharedFun` carrier, and a hard
whole-value-containment precondition. The one part the review confirmed sound —
capture-transparency of the `Box`→`Arc` carrier swap — is retained unchanged.

## Problem

A binding whose type embeds a function — a bare `a -> b`, or a function held
inside a `Maybe` / `Result` / user-union constructor payload, or a record field —
is rejected by the fail-closed gate **IPE-L0127**
(`Feature::FunctionValueReuse`, the "T4" gate) whenever it is used more than once
in a value-consuming (non-call) position. `examples/sky/ipe/47-func-field-record`
— a record whose fields hold two function values — is blocked on the composite
side of this gate.

The restriction is real, not cosmetic. A first-class Ipê function value must be
able to capture arbitrary state, so its only sound representation is a boxed
trait object. The backend emits the type

```text
Box<dyn Fn(T0, …) -> R + Send + Sync + 'static>
```

(`src/compiler/backend/rust/src/emit_types.rs:420-451`). `Box<dyn Fn>` is not
`Clone`, so it moves exactly once. **Calling** is unlimited — `Fn::call` borrows
via `&self` — but a second non-call use (storing it again, forwarding it,
returning it twice) moves an already-moved value, which `cargo` rejects as
`E0382`. Emitting such code after `ipe` reported exit 0 would break THE SEAL, so
the gate fails closed: absent a sound rewrite, it rejects.

The gate is implemented as `reject_fn_value_reuse`
(`src/compiler/lower/src/lower.rs:4479-4493`), reached through the single
move-safety entry point `apply_move_ownership`
(`lower.rs:4462-4477`). It fires only when the binding's IR type
`ir_contains_fun` **and** its `clone_class` is `NonClone`
(`lower.rs:4486`), and only when `count_fn_value_uses` — which, unlike the
general `count_var_uses`, does **not** count a direct-callee `Apply` position
(`lower.rs:4403-4414`) — exceeds one.

## What already exists, and what the type system actually admits

The bare-`Fun` half of the reuse story is already built, but the mechanism is
**not** what a naive reading suggests, and this distinction is load-bearing for
the composite extension.

The general multi-use machinery classifies every binding by `CloneClass`
(`lower.rs:1284-1288`): `CopyLeaf` (bit-copy), `CloneOk` (derives `Clone` — the
rewrite inserts `.clone()` at every use but the last, via
`rewrite_multiuse_clones` driven by a `remaining` last-use counter), and
`NonClone` (no sound duplicating rewrite — fail closed). A `String`/`List`/record
binding reused N times is made move-safe by cloning it N−1 times and leaving the
genuine last use bare; the `remaining` counter is exactly the last-vs-earlier
discriminator IPE-L0127.md points at.

Function values were then given a `Clone`-carrier of their own — **but only at
the expression level, not the type level.** This is the crux the first draft got
wrong:

- **The type renderer is pure of the type.** `render_type`
  (`emit_types.rs:174`) has signature `(ctx, ty, generics)` — it carries **no
  per-binding state**. Given `IrType::Fun`, it unconditionally emits
  `Box<dyn Fn(..) -> R + Send + Sync + 'static>` (`emit_types.rs:420-451`).
  There is **no** Arc-fn `IrType` variant. So no amount of binding-site analysis
  can make the *same* `IrType::Fun` render as `Box` here and `Arc` there.
- **The bare-`Fun` promotion works only because the value is a lambda literal.**
  The lowerer rewrites that binding's `Expr::Lambda` node to `Expr::SharedLambda`
  — an **Expr-level** carrier. `emit_shared_lambda` (`emit_expr.rs:8248-8271`)
  builds the string `Arc<dyn Fn(..) -> R + Send + Sync + 'static>` directly, by
  hand, **bypassing `render_type`'s `Fun` arm entirely**. The Arc-ness rides on
  the *expression* being a `SharedLambda`; the binding's *type* is still
  `IrType::Fun`. The regression suite is
  `src/ipe-cli/tests/golden_i221_fn_value_carrier.rs`, whose
  `fn_value_reuse_promoted` fixture is a pure fn-typed `let` consumed more than
  once — already green.
- **A composite value has no node to flip.** The value of a `Maybe (a->b)` /
  record-of-fn binding is an `Expr::Ctor` / `Expr::Record`, and its **type**
  renders through `render_type`'s composite arms to `IpeMaybe<Box<dyn Fn…>>` /
  a struct with a `Box<dyn Fn…>` field. There is no `Expr::Lambda` sub-node whose
  rewrite to `SharedLambda` would flip the carrier. The first draft's step
  "instantiate the type parameter as `Arc` at the promoted binding" is therefore
  **impossible as written**: the fn slot's carrier is chosen inside `render_type`
  purely from the `IrType`, and today there is no `IrType` that renders to an
  Arc fn.

A precedent for the `Arc` carrier at *type position* does ship, but only for two
hard-wired nominal shapes: server-request handlers and websocket callbacks render
as `Arc<dyn Fn(..) -> Task<..> + Send + Sync + 'static>` via bespoke early arms in
`render_type` (`emit_types.rs:385-419`), selected by `wants_arc_ctor`
(`emit_expr.rs:8111-8124`). These are pattern-matched on the *exact* param/ret
shape (`[ServerRequest] -> Task ServerResponse`, etc.), not a general Arc-fn
carrier. They confirm the `Arc<dyn Fn + Send + Sync>` **string** is a shipping,
tested representation — but they are not a reusable carrier the composite path can
hang off.

**The residual gap** — what example 47 needs — is the **composite** case, and
closing it requires a real type-system change, described next.

## Representation decision: a first-class `IrType::SharedFun` carrier

The `Arc`-vs-`Box` carrier choice cannot be a per-binding side-bit, because the
three authorities that must agree on it (below) each receive **only the type**.
The only place a carrier decision can live so that all of them see it is **in the
type itself.** So the design introduces a new IR variant:

```text
IrType::SharedFun(params, ret)   // renders Arc<dyn Fn(..) -> R + Send + Sync + 'static>
```

`SharedFun` is `IrType::Fun`'s `Arc`-carried sibling: same params/ret, `Clone` by
construction (an `Arc::clone` is a refcount bump). It is the type-level carrier
the first draft wrongly assumed already existed. The promotion pass rewrites a
promotable binding's fn slots from `Fun` to `SharedFun` **in the type**, and the
composite over it (`Maybe (SharedFun …)`, a record field of `SharedFun`) then
renders with `Arc` in that slot automatically — because `render_type` gets a
different `IrType`, not because it grew per-binding state.

Introducing the variant is a **walker-arm obligation**: every exhaustive match on
`IrType` must grow a `SharedFun` arm. The authorities that must be updated in
lockstep (each verified below to be a pure function of the type, hence unable to
distinguish a "promoted" `Fun` any other way):

- `render_type` (`emit_types.rs:174`) — emit `Arc<dyn Fn(..) -> R + Send + Sync
  + 'static>`; place the arm so it does not collide with the ServerHandler/WS
  early arms.
- `carrier_is_clone` (`ir.rs:1441`) — `SharedFun ⇒ true` (Arc is `Clone`), where
  the bare `Fun` arm (`ir.rs:1485`) is `false`.
- `clone_class` (`lower.rs:1302`) — `SharedFun ⇒ CloneOk`, where `Fun`
  (`lower.rs:1369`) is `NonClone`.
- `ir_contains_fun` (`lower.rs:1189`) — `SharedFun ⇒ true`, same as `Fun`
  (`lower.rs:1192`); the gate must still see it as fn-bearing.
- `fun_value_arc_promotable` (`ir.rs:1528`) — recognise the *source* shapes now
  promotable to `SharedFun` (see the containment precondition below), not just
  bare `Fun`.
- `param_is_multiuse_clonable` (`lower.rs:1434`) — **the third authority the
  first draft missed.** It gates whether `rewrite_multiuse_clones` runs at all in
  `apply_move_ownership` (`lower.rs:4469`): a shape it does not accept as
  `CloneOk` falls straight through to `reject_fn_value_reuse`. Because it delegates
  to `clone_class` (`lower.rs:1435`), making `clone_class(SharedFun) = CloneOk`
  suffices — but the delegation must be confirmed to cover the composite-carrying-
  `SharedFun` shapes, or the N−1 clone never fires and the value double-moves
  (E0382) after ipe exit 0.

Because `SharedFun` answers `Clone` by construction in every authority, there is
**no promotion side-bit and no hand-synced SSOT** — the thing the review flagged
as a PRINCIPLES violation is designed out. The carrier decision is the variant.

Trade-offs, unchanged from the sound parts of the first draft:

- **`Arc<dyn Fn + Send + Sync>` (chosen).** `Clone` via refcount bump; the string
  already ships (ServerHandler/WS/SharedLambda); capture-transparent (proven
  below). Cost: one atomic refcount, paid only on promoted bindings.
- **`Rc<dyn Fn>` (rejected).** `Rc` is neither `Send` nor `Sync`; it fails the
  `Send + Sync` obligation the runtime imposes on every fn-value slot (Task
  combinators, UI callbacks). A SEAL break by construction.
- **Uniform `Arc` for every fn value (rejected).** Pays the atomic on the
  never-reused majority (Efficiency), and `Arc<F>` does not satisfy `impl Fn`
  (`ir.rs:1409`), so it would break the HOF-kernel surface unless every consumer
  got a re-dispatch wrapper. Rejected on Soundness + Efficiency.
- **Keep `Box` + re-materialize at each use (rejected as the general answer).**
  Works only for a cheap-to-rebuild top-level reference with no captured state;
  cannot duplicate a closure over runtime values. Stays as user-facing advice in
  the diagnostic, not a compiler strategy.
- **A second, distinct nominal `…Arc` composite type per shape (rejected).**
  Doubles the type surface; `SharedFun` inside the *existing* generic composite is
  the single-source-of-truth alternative.

## Whole-value containment precondition (hard, not a mitigation)

The first draft proposed re-dispatching at "every consumer" of a composite via a
local positive allowlist. The review showed this is unsound, because a composite
fn value is consumed by **extracting** the fn at a pattern-match or field-access —
which binds a **new symbol in a new scope, possibly in a different function or
module the lowerer never co-analyzes.** Two escape paths break THE SEAL and are
**not detectable at the local binding site**:

- **(a) Cross-function flow.** The composite is returned, stored in a larger
  structure, or passed onward, then matched elsewhere. The extraction site — where
  the fn would need re-dispatch from `Arc` to `Box`/`impl Fn` — is outside the
  binding's defining function body. A local allowlist cannot see it.
- **(b) Polymorphic unification.** An `IpeMaybe<Arc<dyn Fn…>>` (from `SharedFun`)
  and an `IpeMaybe<Box<dyn Fn…>>` (from `Fun`) are **distinct Rust types**. If
  both flow into one polymorphic parameter `f : Maybe (a -> b) -> …`, rustc must
  unify them and cannot — `E0308` **after `ipe` exit 0.** `render_type` is a pure
  function of the type, so the two instantiations are genuinely different Rust
  types with no coercion; and the unification frontier is a *caller's* type, not
  visible at the promoted binding.

Therefore the composite carrier flip is admissible **only** under a
whole-value-containment precondition, stated as a hard gate — not a best-effort
consumer sweep:

> Promote a composite fn value to a `SharedFun`-carrying composite **only when the
> value provably never escapes its defining function body AND never flows into any
> `Generic` / polymorphic parameter position.** Otherwise the binding stays
> fail-closed under IPE-L0127.

Under containment, every consumer of the promoted composite is, by construction,
in the same function body the lowerer is analyzing, so each extraction site is
locally visible and can be re-dispatched to `Box`/`impl Fn` where a default-carrier
slot needs it. The polymorphic frontier is excluded by the same precondition, so
the E0308 unification hazard cannot arise. Anything that escapes or generalises is
outside what the lowerer can prove, so it stays IPE-L0127 — Completeness yields to
Soundness at exactly that boundary.

## Whole-composite `clone_class`: a non-fn `NonClone` member forces fail-closed

`clone_class` and `carrier_is_clone` poison the **whole** composite to `NonClone`
if **any** member is `NonClone` (`clone_class_composite`,
`lower.rs:1439-1456`, returns `NonClone` on the first non-clone part;
`carrier_is_clone`'s composite arms use `.all(...)`, `ir.rs:1493-1497`). So a
record `{ f : a -> b, t : Task e x }` is `NonClone` **as a whole even after its fn
slot becomes `SharedFun`**, because `Task` is `NonClone` (`clone_class`
`lower.rs:1373`; `carrier_is_clone` `ir.rs:1487`) — and cloning that record for
reuse is `E0382`.

Hence composite promotability requires more than "flip the fn slots": after
rewriting fn slots to `SharedFun`, **every non-fn member must already be `CloneOk`
or `CopyLeaf`.** Any `NonClone` non-fn member forces fail-closed. The offending
members are the same non-clone set the authorities already name:

- `Task` / `Cmd` / `Sub` (`lower.rs:1373-1376`),
- `Decoder` (`lower.rs:1374`),
- `FnOnceChain` (a curried `Box<dyn FnOnce>` tower — consume-once, `lower.rs:1372`),
- `Generic` (`lower.rs:1377` — also excluded by the containment precondition),
- an FFI foreign-interface opaque `Enum` (`Rust.*` home, `lower.rs:1401-1403`).

`fun_value_arc_promotable` must encode this: the *only* `NonClone` members allowed
in a promotable composite are the fn slots themselves (which the flip converts to
`CloneOk` `SharedFun`); a `NonClone` non-fn member disqualifies the whole binding.

## Verified: `ir_contains_fun` already covers composites (not a pre-existing hole)

Finding to confirm before relaxing the gate: does `ir_contains_fun`
(`lower.rs:1189`) recurse into `Maybe` / `Result` / `Record` / `Enum` arms? If it
did **not**, composite fn values would be silently exempt from IPE-L0127 today — a
pre-existing SEAL hole that would have to be fixed **first**, independently.

**It does recurse** (`lower.rs:1256-1265`): `Enum { args } ⇒ args.any(...)`,
`Maybe`/`List` ⇒ recurse on the element, `Result` ⇒ either side, `Dict` ⇒ either
side, `Set` ⇒ element, `Tuple` ⇒ `elems.any(...)`, `Record` ⇒
`fields.values().any(...)`. Combined with `clone_class`'s whole-composite
`NonClone` poisoning, a composite holding a bare `Fun` is `ir_contains_fun == true`
and `clone_class == NonClone`, so `reject_fn_value_reuse` (`lower.rs:4486`) **does**
fire on composite reuse today. The gate already covers composites; this is a
relaxation of a live gate, **not** the plugging of a hole. No prerequisite PR is
needed on this axis. (Adding the new `SharedFun` arm to `ir_contains_fun` keeps
the coverage intact: a promoted composite is still fn-bearing.)

## The soundness crux, and why it is already answered

The central worry: if the carrier becomes `Arc<dyn Fn + … + Sync>`, is *every
value an Ipê closure can capture* actually `Send + Sync`? If some capturable value
were `Send` but not `Sync`, a uniform `+ Sync` carrier would fail to compile in the
emitted crate — a SEAL violation.

The decisive fact: **the emitted `Box` carrier already carries `+ Send + Sync +
'static`** (`emit_types.rs:448`). rustc already requires every captured free
variable of every emitted `move` closure to be `Send + Sync + 'static` — that
constraint is live today, independent of this feature. `Sync` was added to the
boxed carrier so a callback parameter could be forwarded into the runtime's
UI/Live event-handler slots, which are themselves `Arc<dyn Fn + Send + Sync>`
(`emit_types.rs:404-419`; runtime slots via `ipe_runtime::ui::element::Event`;
regression gate `26-ui-showcase::regression_gates_input_multiline_fill`).

Therefore the carrier swap `Box<dyn Fn + Send + Sync>` → `Arc<dyn Fn + Send +
Sync>` (i.e. `Fun` → `SharedFun`) is **capture-transparent**: it changes only *who
owns* the trait object (a heap box vs a refcounted heap box), not *what the closure
may close over*. The `Send + Sync + 'static` obligation on the captures is
byte-identical before and after. A closure capturing a `Send`-not-`Sync` value
(e.g. an `IpeTask` — `Pin<Box<dyn Future + Send>>`, `Send` but not `Sync`,
`src/runtime/rust/src/core.rs:23`) **does not compile today** under `Box` and
**still does not compile** under `Arc` — it is rejected by rustc's auto-trait
inference on the closure, upstream of the carrier choice. The relaxation admits no
capture the current representation forbids. This is the one part of the first draft
the review confirmed sound; it is unchanged.

What *would* be unsound, and stays fail-closed, is any path that produces a
`SharedFun`/`Arc<dyn Fn>` where a consumer needs a bare `impl Fn` or `Box<dyn Fn>`
— std has no `impl Fn for Arc<F>` (`ir.rs:1409`). The bare-`Fun` machinery handles
this by re-dispatching Arc-carried reads through fresh closures; the containment
precondition is what makes the composite version of that discipline *complete*
(every extraction site is in-body and therefore reachable by the pass).

## Soundness argument (summary)

For the relaxation to be sound, all of the following must hold; each is enforced by
construction:

- **The captures stay `Send + Sync + 'static`.** True and unchanged — the `Box`
  carrier already imposes it (`emit_types.rs:448`); `Fun`→`SharedFun` is
  capture-transparent. A `Send`-not-`Sync` capture (`IpeTask`, `core.rs:23`) is
  rejected by rustc on the closure today and stays rejected.
- **No carrier side-bit, no table drift.** The `Arc`-ness is the `SharedFun`
  variant, so `render_type`, `carrier_is_clone`, `clone_class`,
  `param_is_multiuse_clonable`, `ir_contains_fun`, and `fun_value_arc_promotable`
  all dispatch on it structurally. There is no hand-synced promotion flag to drift.
- **The whole composite is `Clone` before it is cloned.** Composite promotability
  requires every non-fn member `CloneOk`/`CopyLeaf` after the fn-slot flip; any
  `NonClone` non-fn member (`Task`/`Cmd`/`Sub`/`Decoder`/`FnOnceChain`/foreign
  `Enum`/`Generic`) forces fail-closed, so no `E0382` on a partially-clonable
  composite.
- **The value is contained.** Promote only when the value never escapes its
  defining function body and never enters a `Generic`/polymorphic position — so
  every consumer is locally visible for re-dispatch and no `Arc`-vs-`Box`
  polymorphic unification (`E0308`) frontier exists.
- **Anything not provably rewritable stays fail-closed.** IPE-L0127 still fires
  for every shape, member, or flow outside the precondition. Completeness yields to
  Soundness at that boundary.

## PRINCIPLES analysis (precedence order)

- **Security (1):** unaffected. No new untrusted-input surface. Neutral.
- **Correctness (2):** the promoted program must produce the Go reference's
  observable value. `47-func-field-record` has a Go oracle; behavioral parity is
  the acceptance bar, not just a green build.
- **Soundness (3):** the governing principle. Preserved because the carrier swap is
  capture-transparent (no new UB, no move-after-move), the `SharedFun` variant
  removes the side-bit-drift hazard, the whole-composite `Clone` requirement
  removes the partial-clone `E0382`, and the containment precondition removes the
  cross-function / polymorphic `E0308`. Soundness is never traded for Completeness —
  the gate relaxes only inside the precondition.
- **Efficiency (4):** one atomic refcount per **promoted** fn value; never-reused,
  uncontained, or non-clonable-composite values keep the plain `Box` — no atomic.
  A refcount bump is strictly cheaper than deep-cloning captured state (impossible
  for `Box<dyn Fn>` anyway).
- **Completeness (5):** the win — accept `47-func-field-record` and the contained
  composite fn-value-reuse family. Ranks below Soundness and Efficiency and is
  delivered without compromising either.
- **Readability (6):** the design adds one honest type variant with exhaustive
  walker arms (the make-invalid-states-unrepresentable discipline) rather than a
  parallel promotion channel with a synced bit. The twin-plus-one authority set and
  the single move-safety entry point `apply_move_ownership` are preserved.

## Implementation + verification plan

Files that change:

- `src/compiler/ir/src/ir.rs` — add `IrType::SharedFun(params, ret)`; add its arm
  to `carrier_is_clone` (`:1441`, ⇒ `true`); extend `fun_value_arc_promotable`
  (`:1528`) to the contained, whole-clonable composite source shapes; correct the
  `carrier_is_clone` doc comment (`:1407-1414`), see below.
- `src/compiler/lower/src/lower.rs` — add `SharedFun` arms to `clone_class`
  (`:1302`, ⇒ `CloneOk`) and `ir_contains_fun` (`:1189`, ⇒ `true`); confirm
  `param_is_multiuse_clonable` (`:1434`) accepts the composite-`SharedFun` shapes
  via its `clone_class` delegation; teach the promotion pass (the `SharedLambda`
  machinery around `:2600-2799`) to flip contained composite fn slots to
  `SharedFun` and re-dispatch in-body extractions to `Box`/`impl Fn`; ensure
  `reject_fn_value_reuse` (`:4479`) becomes a no-op precisely for the now-promotable
  contained composites and still fires for everything else.
- `src/compiler/backend/rust/src/emit_types.rs` / `emit_expr.rs` — add the
  `SharedFun` arm to `render_type` (`:174`) rendering `Arc<dyn Fn(..) -> R + Send +
  Sync + 'static>`, ordered after the ServerHandler/WS early arms; extend the
  ctor-selection (`wants_arc_ctor` / `emit_shared_lambda`, `:8111-8271`) so a
  `SharedFun`-slotted composite constructs with `Arc::new` in that slot; correct the
  stale `emit_shared_lambda` doc comment (`:8241`, see below).
- `src/compiler/diagnostics/explain/IPE-L0127.md` — correct the carrier from
  `+ Send + 'static` to `+ Send + Sync + 'static` (stale at lines 7 and 53); narrow
  the gate's stated scope to the shapes that genuinely remain fail-closed (escaping,
  polymorphic, or non-clonable-composite fn values).

Carrier-bound single-source-of-truth (doc-only correction, worth a code follow-up):
`carrier_is_clone`'s doc comment (`ir.rs:1407-1414`) and `emit_shared_lambda`'s
(`emit_expr.rs:8241`) both say the default fn carrier is `+ Send + 'static` (no
`Sync`), and IPE-L0127.md repeats it (lines 7, 53). The emitter actually emits
`+ Send + Sync + 'static` (`emit_types.rs:448`). The comments are stale and must be
corrected. To stop the doc and emitter drifting again, define the carrier-bound
suffix (`Send + Sync + 'static`) as **one named constant** the emitter and any doc
generator reference, rather than re-spelling the string in `render_type`,
`emit_shared_lambda`, the ServerHandler/WS arms, and the prose.

Fixtures / tests that prove it (each must build AND run behind `IPE_E2E=1`, and be
byte-stable as a golden):

- `examples/sky/ipe/47-func-field-record` goes green **and** prints the Go
  reference value under the examples sweep (behavior parity, not just build).
- Extend `golden_i221_fn_value_carrier.rs` with **contained** composite reuse
  fixtures: a reused `Maybe (a -> b)`, a reused `Result e (a -> b)`, a user-union
  constructor holding a function, and a record field holding a function reused in
  two in-body consumers.
- Golden byte-identity for every non-promoted program is unchanged — promotion is
  opt-in per contained binding, so untouched programs emit identical Rust (the new
  `SharedFun` arm is never reached for them).

Adversarial **negative controls** — each must fail at `ipe` time with a **typed
IPE-L0127**, never a raw rustc `E0308`/`E0382`/`E0507` after exit 0:

- **(a) `Send`-not-`Sync` capture.** A composite fn value whose closure captures a
  `Send`-not-`Sync` value (e.g. a `Task`) — must fail, surfaced as a typed
  diagnostic at lowering (the pre-promotion capture check), not as a leaked rustc
  `E0277`.
- **(b) Cross-function / polymorphic-unification escape.** A promotable-looking
  composite that (i) is returned or stored and matched in another function, or
  (ii) flows into a polymorphic `f : Maybe (a -> b) -> …` alongside a `Box`-carried
  sibling — must stay fail-closed IPE-L0127, never emit an `Arc` that E0308-fails
  against a `Box` at the unification site.
- **(c) Mixed composite.** A record `{ f : a -> b, t : Task e x }` (Arc-eligible fn
  slot + `NonClone` `Task` field) reused as a value — must stay fail-closed
  IPE-L0127 (whole-composite `NonClone`), never an `E0382` on the partially-clonable
  record.

Interaction with the eta / fresh-symbol pool: the in-body re-dispatch wrappers mint
fresh symbols from the same interner pool the bare-`Fun` promotion uses. The
adversarial pool test `adversarial_eta_pool_collision_grow_shrink_regrow`
(`src/ipe-cli/tests/adversarial_review_parity_probe.rs`) protects fresh-name
collision avoidance across warm-incremental revisions. The composite path must route
**all** fresh-symbol minting through the same `set_fresh_avoid` discipline — no
ad-hoc name synthesis — and add a composite-specific pool-collision fixture.

## Open questions / risks

- **Containment analysis precision.** The precondition ("never escapes the defining
  function body, never enters a `Generic`/polymorphic position") is the whole
  soundness load-bearer. It must be computed conservatively: any flow the analysis
  cannot classify defaults to *escapes* (fail-closed), never to *contained*. The
  risk is over-conservatism (rejecting sound programs), never unsoundness — but the
  analysis must be explicit that "unknown ⇒ escapes."
- **`SharedFun` walker-arm completeness.** Adding an `IrType` variant obliges every
  exhaustive match on `IrType` across the compiler to grow a `SharedFun` arm (no
  wildcard). A missed match arm is a build error, which is the desired
  make-invalid-states-unrepresentable behaviour — but the change set is wide; the
  six authorities above are the soundness-critical ones, and the rest are
  mechanical.
- **`Send`-not-`Sync` capture diagnostic quality.** The capture is rejected by rustc
  today; the pre-promotion capture check must surface it as a typed ipe diagnostic
  at lowering (parse-don't-validate / typed-error-channel), verified by negative
  control (a).
- **Divergence bookkeeping.** `fn_capture_eta_promoted` is already a recorded
  strictly-better divergence from the Go backend (`docs/divergences-from-sky.md`).
  Any contained composite shape ipe accepts that the Go reference rejects must be
  recorded there too, with the hand-computed language-semantics value as the parity
  oracle.

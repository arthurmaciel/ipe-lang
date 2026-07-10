# #90 — SKY-L0114: function values in constructor payloads (`Ok f` / `Just f`)

> **Status: IMPLEMENTED (2026-07-10).** Stage 1 (T1/T2, this document) and
> the T3 `andMap` curried-payload gate (revised two-tier design, see
> [`ctor-payload-andmap-arity-gate-design.md`](./ctor-payload-andmap-arity-gate-design.md))
> landed together, along with the restored T4 fn-value-reuse gate
> (`SKY-L0127`). See that companion document's own status note for the T3
> implementation findings (diagnostic-code split between the eager-pin
> `SKY-T0001` and the forwarder-path `SKY-T0014`, the confirmed-accepted
> cross-module wrapper fixture, and the confirmed-not-constructible import-
> alias row). §2's hazard table below is now accurate for the SHIPPED
> behaviour, not merely the predicted design — every row was independently
> confirmed against `crates/skyc/tests/golden_l0114_ctor_payload_function.rs`
> and `crates/sky_lower/tests/unsupported.rs`.
>
> Original design-pass status note (superseded, kept for history):

> Design (Design Lane, read-only study of the crates; no code
> written, no build run). **Seal-touching: YES** — this lifts two fail-closed
> lowering gates whose entire purpose is the `skyc` exit-0 ⇒ `cargo` exit-0
> seal, so Lane A implementation requires the Opus adversarial review before
> commit (same protocol as #87/#93/#104).
>
> **Verdict up front:** SKY-L0114 is an **over-restriction** for the shapes #90
> names (`Ok (\x -> x+1)`, `Just someFn`, and every 1-ary function payload) and
> a **justified guard** for exactly two residual shapes: (a) a *curried /
> multi-arity* payload flowing through `andMap` (the IR flattens arrows, so the
> emitted kernel call would not typecheck), and (b) *reuse* of a fn-carrying
> value (no clone is available for `Box<dyn Fn>`; the #104 clone pass cannot
> save it). The fix is to narrow the gate to those two shapes — with the
> arity-≥2 gate moved from the construction site to the `andMap` call site —
> not to keep the blanket rejection. Everything else the gate nominally
> protects (derives, `==`, `toString`, serde, Live Model) is **already**
> guarded by machinery that ships today: generic-bounded derives on the
> runtime `SkyMaybe`/`SkyResult`, the #87/#93 derive-demotion fixpoint, the
> type-checker's `ty_is_equatable` rejection of fn-embedding operands, and the
> #91 Model gate.

---

## 1. The exact gate — where SKY-L0114 fires and why

`Feature::CtorPayloadFunction` (`crates/sky_diagnostics/src/diagnostic.rs:528`,
mapped to `SKY_L0114` at `diagnostic.rs:924`; code constant
`crates/sky_diagnostics/src/code.rs:186`; message `code.rs:298` + renderer
`crates/sky_diagnostics/src/render.rs:594-597`; explain page
`crates/sky_diagnostics/explain/SKY-L0114.md`). It is raised from **two**
places in `crates/sky_lower/src/lower.rs`:

1. **Declaration-site gate** — `lower_enum`, `lower.rs:1581-1583`. Each
   constructor field lowers to IR; if `ir_contains_fun(&ir)` (`lower.rs:268`)
   the whole union is rejected. Catches `type Box = Mk (Int -> Int)` — a
   *declared* function payload.

2. **Use-site region gate** — `reject_function_through_type_var`,
   `lower.rs:2856-2871`, called unconditionally at the top of `lower_expr`
   (`lower.rs:2878`) for **every** expression. If the expression's solved
   region type satisfies `embeds_nonderivable_function` (`lower.rs:184-209`)
   the expression is rejected; `con_payload_carries_function`
   (`lower.rs:227-231`) picks the blame label — a `Ty::Con`-headed region gets
   `CtorPayloadFunction` (SKY-L0114), anything else (record-headed) gets
   `FirstClassFunctions` (SKY-L0107). This is the gate that fires on
   `Ok (\x -> x+1)` and `Just someFn`: the region type is
   `Con { Result, [e, Int -> Int] }` and the `Ty::Con` arm
   (`lower.rs:202-204`) flags any type argument for which
   `ty_contains_fun` (`lower.rs:124-132`) holds.

   The only exemption is `is_opaque_boxed_wrapper` (`lower.rs:152-157`):
   `Decoder` / `Task` / `Cmd` / `Sub` heads short-circuit to `false`, which is
   why the applicative **decoder** pipeline
   (`JsonDec.succeed makeUser |> required …`, payload
   `Decoder (a -> b -> …)`) already works — regression test
   `crates/sky_lower/tests/unsupported.rs:519+`
   (`function_inside_opaque_boxed_wrapper_is_accepted`).

**Why it was gated.** The doc-comments (`lower.rs:159-183`,
`lower.rs:2824-2855`) and the golden test
(`crates/skyc/tests/golden_m3a_function_payload_gate.rs`) state the original
threat precisely: a synthesised Rust enum derives
`Clone`/`Debug`/`PartialEq` + hand-impls `SkyStringify`, a function value
lowers to `Box<dyn Fn(..) -> R + Send + 'static>`
(`crates/sky_backend_rust/src/emit_types.rs:186-205`), and a `Box<dyn Fn>`
field "satisfies none of those" — so accepting it *at the time the gate was
written* (M3a, **before** the #87 derive-demotion existed) would have emitted
Rust that cargo rejects. The gate is fail-closed seal armor from an era when
the emit layer had no way to produce a derive-free enum.

Note the built-in irony that makes #90 easy: `Ok` / `Just` don't even produce
a *synthesised* enum. `emit_ctor` routes built-in Maybe/Result constructors to
the **runtime** enums (`emit_expr.rs:3093-3107` — `SkyMaybe::Just(..)`,
`SkyResult::Ok(..)`), whose derives are declared once in the runtime with
**generic-bounded impls**:

* `runtime/src/sky_runtime/core.rs:224-228` —
  `#[derive(Clone, Debug, PartialEq, serde::Serialize)] pub enum SkyMaybe<T>`
  (custom bounded `Deserialize` below it);
* `runtime/src/sky_runtime/core.rs:396-400` —
  `#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
  pub enum SkyResult<E, A>`.

A derive macro on a generic enum emits *conditionally bounded* impls
(`impl<T: Clone> Clone for SkyMaybe<T>` …), so the **type**
`SkyMaybe<Box<dyn Fn(i64) -> i64 + Send + 'static>>` is perfectly valid Rust
that compiles today; only *calling* `.clone()` / `==` / `sky_show` /
serialisation on it fails — and each of those uses is independently gated
(§3). The construction itself was never the problem.

The `andMap` kernels the gate blocks are already fully implemented and
generic enough to take a boxed closure payload:

* registry: `crates/sky_kernels/src/lib.rs:993` (`MaybeAndMap` →
  `maybe_and_map`, arity 2, Pure) and `lib.rs:1004` (`ResultAndMap` →
  `result_and_map`);
* canon→kernel dispatch: `crates/sky_lower/src/lower.rs:4703` /
  `lower.rs:4714`;
* runtime: `runtime/src/sky_runtime/core.rs:599-610`
  (`result_and_map<E, A, B, F: FnOnce(A) -> B>`) and `core.rs:725`
  (`maybe_and_map<A, B, F: FnOnce(A) -> B>`). A
  `Box<dyn Fn(A) -> B + Send + 'static>` implements `FnOnce(A) -> B`, so the
  generic instantiates without any runtime change.

---

## 2. Over-restriction vs real blocker — hazard-by-hazard

For the payload representation `Box<dyn Fn(..) -> R + Send + 'static>` (the
existing `IrType::Fun` rendering, `emit_types.rs:186-205`):

| Hazard the gate nominally protects | Actual state | Verdict |
|---|---|---|
| `#[derive(Clone/Debug/PartialEq)]` on the carrier fails | Built-in `SkyMaybe`/`SkyResult`: bounded derives in the runtime — type always compiles (`core.rs:224`, `core.rs:396`). Generic user enum (`type Opt a = Som a` at `Opt (Int->Int)`): derive macro emits bounded impls, instantiation compiles. Concrete user enum (`type Box = Mk (Int->Int)`): **#87 derive-demotion already handles it** — `emit_enum` drops the auto-derive when `enum_is_derivable` is false (`emit_types.rs:349-380`, fixpoint at `sky_backend_rust/src/lib.rs:392-451`, predicate `ir_type_is_derivable` with `IrType::Fun → false` at `crates/sky_ir/src/ir.rs:699-751`), and the hand-written `SkyStringify` impl binds a non-derivable payload with `_` and renders `"<fn>"` (`emit_types.rs:305-313`). Tested by `sky_backend_rust/tests/seal_derivability.rs`. | **Over-restriction** — the machinery the gate predates now exists. |
| `==` on a fn-carrying value emits E0369/E0277 | Rejected at **type-check**: the Equatable obligation pins to `ty_is_equatable` (`crates/sky_types/src/lib.rs:319-327`), which returns `false` for `Ty::Fun` anywhere — including through `Ty::Con` args, so `Just f == Just g` is a clean SKY-T0014 before lowering (`lib.rs:280`, `lib.rs:306`; constrain sites `sky_types/src/constrain.rs:976-981`). | Already guarded upstream. |
| `toString` / `Log.*With` on a fn-carrying value | Same predicate: the Stringify obligation reuses `ty_is_equatable` ("showable iff it contains no function", `sky_types/src/lib.rs:308-311`; `constrain.rs:1563-1596`). The *emitted impls* stay total regardless: user enums use the `<fn>` placeholder, runtime `SkyMaybe`'s `SkyStringify` is bounded. | Already guarded upstream. |
| serde / Live session store | `ir_type_is_serde` (`ir.rs:796-839`) has `Fun → false` and poisons the `Maybe`/`Result` carriers (`ir.rs:829-831`); the #93 seal gates the serde derive on it (`emit_types.rs:356-375`) and the #91 Model gate rejects a fn-carrying Model with SKY-L0120 (`sky_backend_rust/src/emit_model_gate.rs`). | Already guarded. |
| Reuse (double-move, E0382) — `Box<dyn Fn>` is not `Clone` | **Real.** `Expr::Var` emits a bare identifier (`emit_expr.rs:2792`); there is *no* general clone-on-reuse pass (documented in `docs/architecture/seal-noncopy-move-design.md` §1.2, #104 — design-only, commit 9eda834). #104's stated invariant "every non-`Copy` value is `Clone`" is **already false today** for `Task`/`Cmd`/`Sub`/`Decoder` (`SkyTask<E,A> = Pin<Box<dyn Future…>>`, `core.rs`; all `false` in `ir_type_is_derivable`, `ir.rs:723-734`) — the opaque-wrapper exemption already lets non-Clone values flow linearly. A fn-carrying `Maybe` joins that existing risk class. **Calling** an extracted fn does not consume it (`Box<dyn Fn>` implements `Fn`, call is by `&self`), so only a second *argument-position* use double-moves. | **Justified — but as a narrow reuse gate, not a construction ban** (§3 step 4). |
| Curried / multi-arity payload through `andMap` | **Real.** The IR **flattens** curried arrows into one multi-parameter `Fun` (`lower.rs:2214-2223`, `lower.rs:2704-2712`; `ty_arrow_arity` `lower.rs:3440-3448`), and partial application of a first-class value fails closed as `Feature::PartialOverApplication` (`lower.rs:3419-3423`). So `Just (\a b -> …) |> andMap (Just x)` would emit `maybe_and_map(ma, mf)` where `F = Box<dyn Fn(A, B) -> C>`, which does **not** satisfy `F: FnOnce(A) -> B` — an exit-0-then-cargo-fail if construction were lifted blindly. (The decoder pipeline sidesteps this with the runtime `curry1..curry10` adapters, `runtime/src/sky_runtime/json.rs:799-822`, invoked as `decode_succeed(curryN(f))` — precedent for Stage 2.) | **Justified — gate it at the `andMap` call site** (§3 step 3). |
| Collections of functions (`List (a->b)`, `Dict k (a->b)`, `Set`) | **Real for now.** These are also `Ty::Con` heads caught by the same arm today. Several collection kernels blanket-`.clone()` their argument (e.g. `DictGet`, `emit_expr.rs:2911-2913`; the #104 doc §1.2 lists more), which is E0599 on a `Box<dyn Fn>`-bearing value. | Keep gated at Stage 1 — the lift predicate must whitelist *enum-like* heads only. |

**Overall verdict: over-restriction**, with two residual sub-shapes that must
stay fail-closed and are better gated at their actual unsound sites.

### 2.1 How the upstream Haskell Rust backend handles it (`../sky/src/Sky/Generate/Rust`)

Upstream renders a function type three ways
(`Builder/TypeRenderer.hs:281-326`):

1. event-handler shape (`String/Bool -> msgVar`) →
   `Arc<dyn Fn + Send + Sync>` (capturing, Clone; stored only inside the
   runtime's opaque `Event` enum);
2. Task-returning → `Arc<dyn Fn(..) -> SkyTask<..> + Send + Sync>` (the
   `Handler` shape);
3. **everything else — a pure function stored in an ADT variant or record
   field — a bare `fn(..) -> R` pointer** (`TypeRenderer.hs:324-326`,
   `TypeEmitter.hs:238-260`): `ShouldRetry e = RetryWhen (e -> Bool)` emits
   `RetryWhen(fn(e) -> bool)`. `fn` pointers are `Copy + Clone + Debug +
   PartialEq`, so the ADT keeps its full derive — but the payload is limited
   to **non-capturing** closures (`TypeEmitter.hs:256-259` says so
   explicitly).

So upstream *does* allow `Ok (\x -> x+1)` (non-capturing → coerces to `fn`)
and ships `ShouldRetry`'s fn-payload in the stdlib — but a *capturing* pure
closure in a ctor payload is broken there too (the m3a golden's doc-comment
records that the Go-oracle *Go backend* also rejects `Mk (\n -> n+1)` at
`go build`). Our `Box<dyn Fn>` payload is strictly more general than
upstream's `fn` pointer (captures allowed) at the cost of losing
`Clone/Debug/PartialEq` — which our #87 demotion + type-level use-gates
absorb. That trade is the right one for Sky semantics (Sky closures capture
freely); record it in `docs/divergences-from-sky.md` as a sanctioned
divergence (Rust-side improvement, same class as the m3a note).

---

## 3. The fix path — Stage 1 (this issue) + Stage 2 (curried payloads)

### Stage 1 — lift the blanket gate, keep two narrow fail-closed gates

All Stage-1 changes are in `sky_lower` (+ diagnostics bookkeeping). **No
emit-layer or runtime change is required** — construction
(`emit_ctor` builtin path `emit_expr.rs:3097-3107`; lambda emission
`emit_lambda` `emit_expr.rs:3986-4010`; named-fn-as-value `emit_func_value`
`emit_expr.rs:3965-3976`), pattern matching (`emit_ctor_arm_pat`), the #87/#93
derive machinery, and the `andMap` kernels already handle the shapes.

1. **Narrow the use-site region gate** (`embeds_nonderivable_function`,
   `lower.rs:184-209`, and its mirror `con_payload_carries_function`,
   `lower.rs:227-231`). In the `Ty::Con` arm, replace the blanket
   `ty_contains_fun(a)` with a predicate that allows a `Ty::Fun` type argument
   when the head is **enum-like** — the built-in `Maybe` / `Result` (resolve
   the same way `ir_type_from_ty` classifies the head, `lower.rs:1661+`;
   builtin symbols are interned at `lower.rs:1256-1268`) **or a user union**
   — and keeps flagging it when the head is a collection
   (`List` / `Dict` / `Set`) or anything else. The `Ty::Record` arm keeps
   `ty_contains_fun` unchanged (SKY-L0107, record fields, is out of #90's
   scope; same analysis applies later).

2. **Lift the declaration-site gate** (`lower_enum`, `lower.rs:1581-1583`):
   delete the `ir_contains_fun` rejection. A declared fn payload of any
   flattened arity is sound in isolation — construction emits a matching
   flat `Box<dyn Fn(..)>`, a match arm extracts and fully applies it, and an
   under-application already fails closed as `PartialOverApplication`
   (`lower.rs:3419-3423`). #87 demotes the enum's derives; the `SkyStringify`
   impl uses the `<fn>` placeholder. This is what the stdlib's
   `ShouldRetry e = RetryAlways | RetryWhen (e -> Bool)` (CLAUDE.md: "HM-pure,
   portable to Rust backends") needs when `Task.retryWith` is ported.

3. **Add the `andMap` call-site arity gate** — the place the *curried* hazard
   is actually unsound. **Superseded design, 2026-07-10** — this step's
   original AST-shape-matching approach was implemented and reverted three
   times (see `BACKLOG.md`'s `#90` row for the incident log); the current,
   revised design lives in a companion document:
   [`ctor-payload-andmap-arity-gate-design.md`](./ctor-payload-andmap-arity-gate-design.md).
   Summary: a two-tier fix — a primary `TyBounds`-style type-checker
   obligation on the `andMap` kernel scheme's payload-result slot (survives
   arbitrary aliasing, including generalization, by construction — mirrors
   the existing `Math.min`/`Set`/`Dict`-key obligation mechanism), plus a
   lowering-time backstop re-anchored to `lower_callee` (the actual single
   funnel every kernel/top-level reference resolves through, not just the
   `Call`-node arm the reverted attempts used). Read the companion doc before
   implementing T3.

4. **Add a minimal fn-carrier reuse gate** so the lift opens no new seal
   hole ahead of #104. In the lowerer, for each binding whose solved type
   embeds a `Ty::Fun` under a carrier (the same predicate as step 1, plus a
   bare fn-typed binding passed by value): count **consuming** uses —
   occurrences in argument / payload / return position; an `Expr::Apply`
   *callee* position is non-consuming (`Box<dyn Fn>` calls by `&self`) and
   stays unlimited. More than one consuming use → fail closed with a new
   code **SKY-L0121** (`Feature::FunctionValueReuse`; next free slot after
   L0120 per `code.rs:196` + `code.rs:431` registry), message: "a value
   holding a function is used more than once — function values cannot be
   copied yet". Explain page with the `let g = mf in (use, use)` example and
   the workaround (re-construct at each use / restructure linearly).
   * This gate is deliberately conservative and is **superseded by #104**'s
     general last-use analysis, which must treat non-Clone-rendering types
     (fn-embedding + `Task`/`Cmd`/`Sub`/`Decoder`/`Db`) as un-clonable →
     diagnostic, and Clone types → `.clone()`. File that as an explicit
     requirement on #104's design (its "every non-Copy value is Clone"
     invariant is already false for the Task class; #90 does not create the
     problem, it widens an existing class by one member).

5. **Diagnostics + docs bookkeeping**: rewrite
   `explain/SKY-L0114.md` (the "not supported yet" examples become the
   supported examples; the residual curried-`andMap` shape becomes the
   documented limit); add `SKY-L0121` + explain page; update
   `render.rs`/`diagnostic.rs`/`code.rs` registries (the walker-arm rule —
   no wildcard arms); divergence-ledger entry (§2.1);
   `docs/architecture/parity-gap-snapshot.md` refresh if it lists #90.

### Stage 2 — curried payloads (full applicative chains) — separate issue

To make `Just (\a b -> …) |> andMap (Just x) |> andMap (Just y)` green:

* **Don't flatten** an arrow that sits in a ctor-payload type argument:
  `IrType::Fun` can already represent `Fun([A], Fun([B], C))` structurally —
  the flatten (`lower.rs:2214-2223`) is a lowering choice, applied per
  position. A lambda flowing into a curried slot emits nested boxes
  (`Box::new(move |a| Box::new(move |b| …))`).
* Choose the inner-layer trait: recommend `Box<dyn FnOnce … >` for the inner
  links (exactly the runtime's `curryN` shape, `json.rs:799-822` — the inner
  closure moves its captured earlier argument), which is sound because
  Stage 1's reuse gate already enforces linear use; the alternative
  (`dyn Fn` + `Clone`-bounded captures) forces `A: Clone` for no user gain.
* `ty_arrow_arity` / `PartialOverApplication` and `saturate_over`
  (`lower.rs:3526-3560`) become curried-aware at these positions.
* Reuses the `curryN` precedent; possibly reuses `curryN` itself at
  construction sites of multi-param lambdas into curried slots.
* Ctor-as-function (`Just User` with `User` a payload ctor referenced bare,
  `Feature::CtorAsFunction`, `lower.rs:2915`) is the *other* half of the full
  Elm applicative idiom — separate existing gate, file alongside Stage 2.

### Rejected alternatives

* **Add `Maybe`/`Result` to `is_opaque_boxed_wrapper`** — wrong: they are not
  opaque (their payloads are pattern-matched, stringified, compared), and it
  would also lift the gate for collections via nothing (the exemption is
  head-only, but it would silently bless *every* fn under them, including
  arity-≥2 → cargo-fail at `andMap`).
* **`Arc<dyn Fn>` payload representation** (make the carrier Clone, dissolve
  the reuse gate) — dual representation (Box in params/lets vs Arc in
  payloads) needs conversion seams at every construction/extraction boundary
  (`Arc::from(box)` in, re-wrap `Box::new(move |a| (*f)(a))` out), and
  `Arc<dyn Fn>` still lacks `Debug`/`PartialEq`/serde, so no use-gate goes
  away. A *global* Arc switch (Sky values are shared-immutable; restores
  #104's "everything clones" invariant) is principled but changes every
  golden and every runtime closure seam (`Arc` doesn't implement `FnOnce`) —
  file as a possible future unification, not #90.
* **Bare `fn` pointers (upstream's choice)** — keeps derives but silently
  forbids capturing closures, which is a semantic hole (Sky lambdas capture);
  upstream itself documents the breakage class. Our divergence is the sound
  direction.

---

## 4. Interaction with the derive-seal (#87/#93) and the other seals

The non-negotiable: lifting L0114 must not reintroduce an
exit-0-then-cargo-fail. Checklist, per emitted artifact:

| Artifact | Why it still cargo-builds |
|---|---|
| `SkyMaybe<Box<dyn Fn…>>` / `SkyResult<E, Box<dyn Fn…>>` types + construction | Runtime enums with bounded derives (`core.rs:224`, `core.rs:396`); `emit_ctor` builtin path emits `SkyMaybe::Just(<boxed>)` (`emit_expr.rs:3097-3107`); lambda/named-fn args already emit boxed (`emit_expr.rs:3986`, `3965`). |
| Concrete user enum w/ declared fn payload | #87: `enum_is_derivable` fixpoint → no `#[derive]` (`emit_types.rs:349-380`); `SkyStringify` `<fn>` placeholder (`emit_types.rs:305-313`); field renders `Box<dyn Fn…>` via `render_type` `IrType::Fun` arm. Covered by `seal_derivability.rs` patterns — extend with an fn-payload case. |
| Generic user enum instantiated at a fn type | Declared derives are per-parameter bounded (macro-generated `impl<T1: Clone>`); the `SkyStringify` impl's `T1: SkyStringify + Debug` bound (`emit_types.rs:335-347`) only bites on *use*, and every such use is type-rejected (`ty_is_equatable`). |
| serde under `uses_live` | #93: `enum_is_serde ⊊ enum_is_derivable`; `IrType::Fun → false` in `ir_type_is_serde` (`ir.rs:821`) poisons the carriers (`ir.rs:829-831`) — no forced serde derive. `SkyMaybe`'s unconditional-but-bounded `Serialize` derive is safe by construction (`core.rs:212-224` comment). |
| Live/Tui/Webview Model holding `Maybe (a->b)` | #91 Model gate → SKY-L0120 (`emit_model_gate.rs`; predicates as above). Already fail-closed, no change. |
| `==` / `toString` / `Log.*With` / Set-elem / Dict-key on fn-carriers | Type-checker obligations (`sky_types/src/lib.rs:280`, `306`, `319-327`) reject before lowering. Lane A adds negative fixtures to *prove* the Set/Dict comparable-key path also rejects (expected via the same recursive predicate family — verify, don't assume). |
| Reuse of a fn-carrier | New SKY-L0121 gate (Stage 1, step 4) — fail-closed until #104's general pass lands with the non-Clone-type requirement. |
| ≥2-arity payload meeting `andMap` | New call-site gate (Stage 1, step 3) — fail-closed until Stage 2. |

The `golden_m3a_function_payload_gate.rs` test is **already written for this
lift** (`skyc/tests/golden_m3a_function_payload_gate.rs:70-93`): it accepts
either the clean L0114 *or* full acceptance, in which case (under `SKY_E2E=1`)
the emitted crate must build and print `2`. Stage 1 flips it from the reject
branch to the build-and-run branch with zero test-logic change.

---

## 5. Red→green fixtures

New golden dirs under `tests/golden/` (Main.sky + expected stdout + golden
`main.rs` where byte-pinning is useful), each with an `SKY_E2E=1` build+run
leg and a Go-oracle parity check where the Go backend supports the shape:

| Fixture | Program (sketch) | Today | After Stage 1 | Go oracle |
|---|---|---|---|---|
| `result_and_map_fn_payload` | `Ok (\x -> x + 1) \|> Result.andMap (Ok 2) \|> Result.withDefault 0` → `println` | red: SKY-L0114 at `Ok (\x…)` | green: prints `3` | Go prints `3` — real parity |
| `maybe_and_map_fn_payload` | `Just (\x -> x * 2) \|> Maybe.andMap (Just 21)` → withDefault → `42` | red: SKY-L0114 | green: `42` | parity |
| `just_named_fn` | `inc n = n + 1` … `Just inc` then `case … of Just f -> f 41` | red: SKY-L0114 | green: `42` (exercises `emit_func_value` inside a ctor arg) | parity |
| `m3a_function_payload_gate` (existing) | `Mk (\n -> n + 1)` through `type Box a = Mk a`, unwrap+apply | red branch of the either-test | green branch: prints `2` | **divergence** — Go backend fails `go build` (documented in the test header); ledger entry |
| `ctor_decl_fn_payload` | `type Retryish e = RetryWhen (e -> Bool) \| RetryAlways`, construct + match + call | red: SKY-L0114 at decl | green (enum emits derive-free, `<fn>` stringify never called) | parity check vs Go `ShouldRetry` behaviour |
| `and_map_curried_stays_gated` | `Just (\a b -> a + b) \|> Maybe.andMap (Just 1) \|> Maybe.andMap (Just 2)` | red: SKY-L0114 at construction | **still red, by design** — the call-site arity gate fires at the *first* `andMap`, clean diagnostic, never a cargo fail | n/a (negative) |
| `fn_carrier_reuse_gated` | `let mf = Just (\x -> x+1) in (consume mf, consume mf)` | red: SKY-L0114 | red: **SKY-L0121** (reuse), never E0382 | n/a (negative) |
| `fn_extracted_called_twice` | `case Just (\x -> x+1) of Just f -> f 1 + f 2` | red: SKY-L0114 | green: `5` (callee-position uses are non-consuming) | parity |
| negative type-level pins | `Just f == Just g`, `toString (Just f)`, fn-carrier as Live Model | red (T-codes / L0120) | unchanged — assert the codes to prove the use-gates hold post-lift | n/a |

Unit-level: extend `sky_lower/tests/unsupported.rs` (the existing
`function_inside_opaque_boxed_wrapper_is_accepted` pattern gives the region-map
harness) with: Con-head-Maybe accepted, Con-head-List still rejected,
Record-head still SKY-L0107, `andMap`-arity gate, reuse gate. Extend
`seal_derivability.rs` with the declared-fn-payload enum emission.

---

## 6. Lane A task breakdown

**Seal-touching: YES** (every task below except T6/T7 moves the exit-0 ⇒
cargo-0 boundary) → Opus guardian design review of the final diff before
commit; Haiku mech-check (clippy + full test suite + `SKY_E2E=1` golden legs)
first, per the backend wiring protocol.

| # | Task | Files | Depends on | Status |
|---|---|---|---|---|
| T1 | Narrow `embeds_nonderivable_function` / `con_payload_carries_function` Con arm to enum-like heads (Maybe/Result/user unions); keep collections + records gated | `sky_lower/src/lower.rs:184-231` | — | **DONE** |
| T2 | Delete `lower_enum`'s `ir_contains_fun` decl gate | `lower.rs:1581-1583` | T1 (shared fixtures) | **DONE** |
| T3 | `andMap` payload-arity gate — see [`ctor-payload-andmap-arity-gate-design.md`](./ctor-payload-andmap-arity-gate-design.md) for the implemented design (its own `T3a`-`T3g` task breakdown) | `sky_types/src/{ty,lib,constrain}.rs` (primary) + `sky_lower/src/lower.rs` (backstop) + diagnostics | T1 | **DONE** |
| T4 | Fn-carrier consuming-use-count gate → new `Feature::FunctionValueReuse` / **SKY-L0127** (the actual free slot at implementation time — SKY-L0121 was already taken by an unrelated kernel by the time this landed) + explain page | `sky_lower` walk + `sky_diagnostics` (code.rs, diagnostic.rs, render.rs, explain/) | T1 | **DONE** |
| T5 | Fixtures + goldens of §5 (incl. flipping `golden_m3a_function_payload_gate` to its green branch); `unsupported.rs` + `seal_derivability.rs` units; negative type-level pins | `tests/golden/*`, `sky_lower/tests/`, `sky_backend_rust/tests/` | T1-T4 | **DONE** — `crates/skyc/tests/golden_l0114_ctor_payload_function.rs` + `sky_lower/tests/unsupported.rs` |
| T6 | Rewrite `explain/SKY-L0114.md`; divergence-ledger entry (Box-payload vs upstream fn-pointer; m3a Go-oracle divergence); parity-snapshot refresh | `sky_diagnostics/explain/`, `docs/` | T1-T4 | **DONE** — B22 restored in `docs/divergences-from-sky.md`; parity-snapshot refresh not yet done (file separately if `parity-gap-snapshot.md` lists #90) |
| T7 | File the #104 requirement: last-use pass must diagnose (not clone) non-Clone-rendering types; file Stage 2 (curried payloads + ctor-as-function) as its own issue with §3-Stage-2 as the seed design | tracker | — | still open — Stage 2 (full curried applicative chains) remains a separate, un-started issue |

Estimated blast radius: `sky_lower` only (+ diagnostics registry); zero
emit/runtime changes in Stage 1; goldens change only where a red fixture turns
green.

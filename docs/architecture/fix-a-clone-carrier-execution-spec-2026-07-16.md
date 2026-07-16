# Fix A — clonable function-value carrier: execution-ready spec

Status: Ready to execute (design only — no code in this change).
Closes: sweep red #221 (`36-composite-server`, SKY-L0126).
Root cause: `docs/architecture/sweep-red-221-l0126-root-cause-2026-07-16.md`.
Restructure it lands inside: `docs/architecture/clone-relay-class-macro-design-2026-07-16.md`.

The single move: the general first-class function-value carrier flips from
non-`Clone` `Box<dyn Fn>` to `Clone` `Arc<dyn Fn>`. Once a captured function
value is `Clone`, the entire L0125/L0126/`reject_fn_value_reuse` fail-close
family is over-representation and dissolves. This spec pins every edit,
resolves the two open questions the root-cause doc flagged (`FnOnceChain` /
`Decoder` payloads; sequencing against clone-relay), and gives an implementer
a gated phase plan.

---

## 0. The one invariant

> **Every value type a Sky closure can capture implements `Clone`.**
> Equivalently: acceptance of a well-typed capture never depends on its
> syntactic position (callee vs argument, depth 0 vs depth ≥ 1).

Secondary invariant (kills the parallel-table drift named in the root-cause
doc c1): **`clone_class` is DERIVED from the emitter's carrier table, not
maintained in parallel.** A shape that renders as an `Arc<…>` (Clone) carrier
can never again be classified `NonClone`.

Both are the make-invalid-states-unrepresentable rule (PRINCIPLES.md §
"three fundamental rules"). This is the structural fix the #172 ledger entry
demands — carrier-level, not a per-site depth-1 special-case.

---

## 1. Carrier decision — one source of truth

### 1.1 The new render (the only carrier change)

`crates/sky_backend_rust/src/emit_types.rs:292-323` — the general
`IrType::Fun(params, ret)` arm currently renders:

```rust
format!("Box<dyn Fn({}) -> {ret} + Send + Sync + 'static>", parts.join(", "))
```

New render:

```rust
format!("Arc<dyn Fn({}) -> {ret} + Send + Sync + 'static>", parts.join(", "))
```

`Send + Sync + 'static` bounds are UNCHANGED — they are exactly the bounds
`Arc<dyn Fn>` needs (`Arc<T>: Send + Sync` requires `T: Send + Sync`), and
the existing comment block already justifies them; keep it, retarget the
"boxed trait object" wording to "Arc'd trait object".

### 1.2 Collapse the two special-case arms INTO the general arm

Both special cases exist ONLY because the general arm was non-Clone Box. Once
the general arm is `Arc<dyn Fn>`, they are redundant:

- **`ServerHandler<SkyError>` arm** (`emit_types.rs:257-262`). `ServerHandler<E>`
  is `Arc<dyn Fn(ServerRequest) -> SkyTask<E,ServerResponse> + Send + Sync>`
  (`runtime/src/sky_runtime/server.rs:89`). The general arm would now render
  `Arc<dyn Fn(ServerRequest) -> SkyTask<SkyError, ServerResponse> + Send + Sync + 'static>`
  — structurally identical modulo the alias name and the extra `'static`
  (harmless; `ServerHandler`'s `dyn` is `'static` by default). **Decision:
  KEEP this arm for now** (it renders the shorter alias and the runtime's
  `IntoServerHandler` impls at `server.rs:100-131` accept both the alias and a
  bare `Arc<F: Fn>`, so registration is unaffected). It is now a *readability*
  alias, no longer a correctness carrier. Deleting it is a safe follow-up but
  churns every Server golden's type strings; do NOT bundle that churn here.
- **WsServerCfg-callback arm** (`emit_types.rs:276-291`) already renders
  `Arc<dyn Fn(...) -> ... + Send + Sync + 'static>` — now byte-identical to
  what the general arm produces for those shapes. **Decision: DELETE this arm**
  — it is pure duplication after the flip. Its paired `wants_arc_ctor` WS
  clause (emit_expr.rs:7742-7756) is handled in §3.

Net: one general arm renders `Arc`; the ServerHandler alias arm stays as a
naming convenience; the WS arm is deleted.

### 1.3 `clone_class` becomes derived (secondary invariant)

`crates/sky_lower/src/lower.rs:932` `clone_class`. Today `IrType::Fun(_,_)`
sits in the `NonClone` bucket (lower.rs:991). The flip moves it to `CloneOk`.
But moving it by hand re-creates the drift. Instead, introduce ONE shared
predicate that BOTH the emitter carrier choice and `clone_class` consult, so
they cannot disagree:

- Add `fn carrier_is_clone(t: &IrType) -> bool` in a location both crates can
  see. `sky_lower` and `sky_backend_rust` both depend on `sky_ir` (where
  `IrType` lives) — put it in `sky_ir` next to `IrType`, or in a small shared
  module. It returns `true` for every `IrType` whose emitted carrier is
  `Clone` — including `Fun` (now Arc), and every existing `CloneOk` scalar/
  composite.
- `clone_class`'s `Fun(_,_)` classification becomes: `if carrier_is_clone →
  CloneOk`. The emitter's carrier choice for `Fun` is Arc BECAUSE
  `carrier_is_clone(Fun) == true`. One boolean, two readers — the tables can
  no longer drift.
- Mechanism name: a plain `const fn` / free `fn` predicate (NOT a trait — HM
  only, PRINCIPLES readability). Keep it exhaustive over `IrType` with no `_`
  arm so a future variant forces a decision (SEAL §"make-invalid-states-
  unrepresentable").

This is the minimum that discharges the secondary invariant. A fuller
"emit the carrier type and its clone-class from one match" refactor is
possible later but is not required to close #221.

---

## 2. `FnOnceChain` and `Decoder(Fun)` — the flagged open question

Both were examined in the current tree. **Neither migrates to Arc.**

### 2.1 `FnOnceChain` — STAYS `Box<dyn FnOnce>`

- Type: `IrType::FnOnceChain(params, ret)`, rendered by
  `render_fn_once_chain` (dispatched at `emit_types.rs:335`) as a nested
  `Box<dyn FnOnce(A1) -> Box<dyn FnOnce(A2) -> … + Send> + Send>` tower.
- Runtime shape it must match: the curried one-shot chains built by
  `curry2..curryN` and consumed by the decoder-pipeline `next_decoder`
  parameters — `runtime/src/sky_runtime/json.rs:801-1010, 1312-1435` and
  `runtime/src/sky_runtime/db.rs:509-547`, all typed
  `Decoder<E, Box<dyn FnOnce(A) -> B + Send>>`.
- **Justification (Correctness > Efficiency):** an `FnOnceChain` is
  *consume-once by construction* — each pipeline step calls the boxed `FnOnce`
  exactly once and it is gone. It is never captured and re-read across a
  re-callable `Fn` boundary, so the L0126 family never touches it: it has no
  reuse to reject. Migrating it to Arc would (a) require the runtime's
  `curryN`/`db_decode_*`/`decode_pipeline_*` signatures to accept `Arc<dyn Fn>`
  instead of `Box<dyn FnOnce>` — a `FnOnce` cannot be re-called, so this is a
  semantic change, not a rename — and (b) add a refcount bump for a value used
  once (Efficiency loss with zero soundness gain). `clone_class(FnOnceChain)`
  therefore **stays `NonClone`**, and `carrier_is_clone(FnOnceChain) == false`
  keeps the two consistent.
- Exact site to leave untouched: `lower.rs:992-994` (the `FnOnceChain` line in
  the `NonClone` bucket) — but only after §1.3 splits `Fun` out of that shared
  bucket line. Split the arm so `Fun` leaves and `FnOnceChain` remains.

### 2.2 `Decoder(_)` — STAYS a struct with a `Box<dyn Fn>` field; NOT a `Fun`

- `IrType::Decoder(_)` renders as the runtime struct `Decoder<E, T>`
  (`runtime/src/sky_runtime/json.rs:21-24`), whose `run` field is
  `Box<dyn Fn(&JsonVal) -> SkyResult<E,T> + Send>`. This is a *named nominal
  carrier*, not the anonymous first-class `Fun` carrier — a Sky `Decoder a`
  value is not a Sky function value. The `Box<dyn Fn>` inside `run` is a
  runtime implementation detail invisible to `clone_class`, which sees
  `IrType::Decoder`, never the field.
- **"`Decoder(Fun)` payload"** (a decoder whose decoded value is itself a
  function, e.g. `Decoder (A -> B)`): the payload rides in the `T` slot as a
  first-class `Fun`, so `T` now renders `Arc<dyn Fn>` automatically from §1.1
  — no `Decoder`-specific work. The `Decoder` struct is generic over `T`, so
  `T = Arc<dyn Fn(...)>` needs `Decoder<E, T>: Send` to still hold: `Arc<dyn Fn
  + Send + Sync>` is `Send`, and the struct's `Send` derivation (json.rs:20
  comment) is preserved. Verify with a golden (§6) but expect no signature
  edit.
- **Justification (Correctness):** `Decoder` values ARE currently reused
  (a decoder combinator reads its inner decoder more than once), and the
  `Decoder` struct is *already* effectively shareable via clone of the struct
  where needed; the runtime owns that discipline. Forcing the `run` field to
  `Arc<dyn Fn>` would be a runtime-internal change with no bearing on the L0126
  capture family (decoders are not captured as bare Sky function values). So
  **`clone_class(Decoder) stays NonClone`** and `carrier_is_clone(Decoder) ==
  false`. (If a future sweep shows a decoder captured across a `Fn` boundary,
  that is a separate, currently-unobserved item — do not pre-solve it here.)

**Locked #2 summary:** only `IrType::Fun` migrates to the Arc carrier.
`FnOnceChain` and `Decoder` keep their current carriers and stay `NonClone` —
each for a Correctness reason (one-shot semantics / nominal non-function
carrier), consistent with Efficiency (no gratuitous refcount).

---

## 3. Closure construction + capture pre-clone (emit_expr.rs)

The carrier flip forces the pointer constructor and the `wants_arc_ctor`
predicate to change. Enumerated sites:

1. **`wants_arc_ctor`** (`emit_expr.rs:7742-7756`). Today it returns `true`
   ONLY for the ServerHandler and WS shapes; everything else falls to
   `Box::new`. After the flip, EVERY `IrType::Fun` wants `Arc::new`.
   **Change:** `wants_arc_ctor(ty)` returns `true` for all
   `matches!(ty, IrType::Fun(..))` (subsumes the two special shapes). Rename
   to `fn_ctor(ty) -> &str` returning `"Arc"`/`"Box"` if clearer, but the
   two-line predicate suffices. It should consult the same `carrier_is_clone`
   predicate from §1.3 rather than re-listing shapes — single source of truth.
2. **`emit_lambda`** (`emit_expr.rs:7831-7861`). Builds
   `{ let __sky_fn: {typed} = {ctor}::new({inner}); __sky_fn }`. With
   `wants_arc_ctor` now true for all `Fun`, `ctor == "Arc"` for every general
   lambda. `typed` comes from `render_type` (§1.1 → `Arc<dyn Fn>`), so the
   annotation and constructor agree. **No structural change beyond the ctor
   predicate** — the machinery already dispatches on it.
3. **`emit_func_value`** (`emit_expr.rs:7758-7773`). Same
   `{ctor}::new({name})` shape for a bare function-item value; same automatic
   flip to `Arc::new` via the shared predicate.
4. **`emit_shared_lambda`** (`emit_expr.rs:7880-7902`) already emits
   `::std::sync::Arc::new(...)` with a hand-built `Arc<dyn Fn + Send + Sync>`
   type string. After §1.1, the general arm renders the identical string, so
   `emit_shared_lambda` MAY be simplified to route through `emit_lambda`'s
   `render_type` path. **Decision: leave `emit_shared_lambda` in place for
   Phase 1** (it is the S4b Arc-promotion emitter and is orthogonal to the
   carrier flip); revisit merging it only if the clone-relay Stage 2 lands
   (§8). Note the byte output must stay identical — verify in §6.
5. **Capture pre-clone (the `arcWrapClosure` mirror).** The reference
   pre-clones every captured outer var before an Arc'd `move` closure so the
   Arc owns `'static` captures and the outer closure stays re-callable. In our
   pipeline this pre-clone is ALREADY synthesized by `sky_lower`, not
   `emit_expr`: `rewrite_captured_clones` (lower.rs:1261) turns a captured
   `CloneOk` read into `CloneVar`, and the emitter renders `CloneVar(s)` as
   `s.clone()`. Once `clone_class(Fun) = CloneOk` (§1.3), a captured function
   value flows through this SAME path — it becomes a `CloneVar`, emitted as
   `arc.clone()` (a refcount bump). **So no new pre-clone emission site is
   needed in emit_expr**; the existing `CloneVar` → `.clone()` rendering IS
   the `arcWrapClosure` behaviour, now reached because the classifier admits
   `Fun`. This is the payoff of doing the fix at the carrier/classifier level.
6. **`arc_callback_wrap`** (`emit_expr.rs:2373`) and the UI/HTML event-slot
   Arc wraps (`emit_expr.rs:4982-5293`) are UNAFFECTED — they already Arc-wrap
   Box'd callbacks to fit `Arc<dyn Fn>` runtime slots. After the flip, the
   incoming `f_s` is already `Arc<dyn Fn>`; passing an `Arc` where the wrap
   expects a callable still compiles (`Arc<dyn Fn>: Fn` via `Deref`), and the
   wrap `Arc::new(move |_x| (f)(_x))` re-wraps once. **Watch item:** a
   double-Arc (`Arc<dyn Fn>` re-wrapped in another `Arc`) is sound but a minor
   Efficiency wart; if a golden shows it, thread the already-Arc case to skip
   the re-wrap. Flag, do not pre-optimise.
7. **`task_and_then` continuation** (`emit_expr.rs:6100`,
   `TaskSeq`) emits `Box::new(move |_| …)` for a `FnOnce` continuation — this
   is the FnOnceChain family (§2.1), **stays `Box::new`**. Do not flip.

---

## 4. lower.rs simplifications that fall out

Once `clone_class(Fun) = CloneOk`, these fail-close mechanisms have nothing
left to reject for function values. Precise disposition:

| Site | lower.rs | Disposition |
|---|---|---|
| `noncl_set` construction in `rewrite_captured_clones` | :1261-1336 | **SIMPLIFY.** `Fun` symbols no longer enter `noncl_set` (they are `CloneOk`, so they land in `clone_set` and become `CloneVar`). The `noncl_set` still carries the genuinely-non-Clone `FnOnceChain`/`Task`/`Decoder`/`Cmd`/`Sub`/`Generic` captures, so the parameter and the depth-0 exemption STAY — do not delete the whole mechanism. |
| Depth-0 callee exemption `Expr::Var(s) if noncl_set.contains(&s) && depth == 0` | :1313 | **STAYS** (narrowed reach). Still needed for `Task`/`Cmd`/`Sub`/`Generic`/`FnOnceChain` captures used as depth-0 callees. `Fun` no longer relies on it (a `Fun` capture is now `CloneVar` at any depth). |
| L0125/L0126 fail-close arm `Err(unsupported(.., NonCloneCapture))` | :1276 | **STAYS** as the fail-close for the *remaining* NonClone set (Task/Decoder/Cmd/Sub/Generic/FnOnceChain captured off-callee at depth ≥ 1). It STOPS firing for `Fun` because `Fun` is no longer in `noncl_set`. That is the #221 fix: the arm is unreachable for the `wrap`/`guarded` shape. |
| `reject_fn_value_reuse` | :4026-4034 | **STAYS but goes dormant for `Fun`.** Its guard is `ir_contains_fun(ir_ty) && clone_class(ir_ty) == NonClone`. After §1.3, a pure `Fun` type is `CloneOk`, so the guard is false and multi-use of a plain function value is admitted (clones per use). It still fires for a NonClone-carrying composite (e.g. a `Task`-returning function value where the *outer* type is `Task`, not `Fun`) — keep it. Its reach shrinks to exactly the still-uncloneable carriers. |
| `eta_expand_value_partial` | :10885-10944 | **STAYS, simplifies.** The `slot_classes` NonClone branch (:10932-10934, "function/task var forwarded by move… no E0525") still handles Task/Cmd/etc.; for a `Fun` slot the class is now `CloneOk`, so the `Var → CloneVar` branch (:10930) fires and the residual closure clones the Arc per call — exactly correct. No deletion; the existing `CloneOk` arm now covers the `Fun` case that previously took the NonClone arm. |
| `eta_expand_partial_ctor` (:11025), `eta_expand_over_partial` (:11318), `eta_expand_partial` | family | **STAY.** These still synthesize residual lambdas for partial application — eta-expansion is a completeness requirement (a partially-applied value must become a closure regardless of Clone-ness), independent of the carrier. What changes is only that their captured `Fun` args now clone cleanly instead of hitting the reject. No structural edit; re-verify their emitted output shifts Box→Arc (§6). |

**Nothing is deleted outright in Phase 1.** The mechanisms narrow their reach
because `Fun` leaves the NonClone set; the SEAL fail-close stays intact for
the carriers that genuinely cannot clone (§2). Deleting `reject_fn_value_reuse`
entirely is tempting but WRONG — it still guards the true-NonClone remnant.

---

## 5. Runtime kernel signatures (`runtime/src/sky_runtime/`)

Inventory of concrete `Box<dyn Fn>` (not `impl Fn`, not `FnOnce`) that could
be reached by the Arc carrier:

- **HOF kernels take `impl Fn`, NOT `Box<dyn Fn>`** — `list.rs:83-181`
  (`list_filter_map`/`list_foldl`/`list_foldr`/`list_indexed_map`/
  `list_filter`/`list_any`/…) all take `f: impl Fn(...) -> ...`. An
  `Arc<dyn Fn>` satisfies `impl Fn` (blanket `impl Fn for Arc<F: Fn>` via
  `Deref`), so **these need NO change** — they accept the Arc carrier as-is.
  The `list.rs:113-122` comment and the `list.rs:309-332` test-only
  `Box<dyn Fn>` bindings are illustrative/test scaffolding, not signatures.
- **`task_map` / `task_map_error` prelude-shim params** — the EMITTED prelude
  (golden `main.rs` ~line 126-141) declares `task_map(f: Box<dyn Fn(A)->B…>)`
  and forwards to `sky_runtime::task::task_map(f, t)`. The runtime
  `task_map`/`task_map_error` take `impl Fn` (confirm at `runtime/.../task.rs`
  — they must, since the shim forwards a boxed value today and it compiles).
  These prelude shim SIGNATURES are EMITTED by `render_type` on `IrType::Fun`,
  so they flip to `Arc<dyn Fn>` automatically (§1.1) and still forward into the
  `impl Fn` runtime kernel. **No runtime edit; automatic golden churn (§6).**
- **`Decoder.run: Box<dyn Fn>`** (`json.rs:22,27`) — STAYS (§2.2), nominal
  carrier, not reached by the `Fun` flip.
- **`Box<dyn FnOnce>` towers** (`json.rs:689,801-1010,1312-1435`;
  `db.rs:521,547`; `tea.rs:19-32`) — STAY (§2.1), one-shot semantics.
- **`ServerHandler<E> = Arc<dyn Fn>`** (`server.rs:89`) — already Arc;
  `IntoServerHandler` impls (`server.rs:100-131`) accept both alias and bare
  `Arc<F: Fn>`. **No change.**

**Verdict:** ZERO runtime `.rs` signature edits are required for Fix A. Every
runtime seam that receives a function value is either `impl Fn` (accepts Arc),
already `Arc`, or a deliberately-`Box<dyn FnOnce>` one-shot that stays. This is
the reason the fix is carrier-local: the runtime was already generic over the
pointer. Confirm the `impl Fn`-ness of `task_map`/`task_map_error`/
`task_on_error` in `task.rs` during Phase 0 (cheap grep) before relying on it.

---

## 6. Golden regen scope + strategy

**Reach: effectively the entire 70-golden byte suite.** Every emitted
`main.rs` carries the prelude shim whose `task_map`/`task_map_error` params
render `Box<dyn Fn>` (confirmed `tests/golden/m0/main.rs:126-141`), plus every
golden that emits a user closure or first-class function value carries a
`{ let __sky_fn: Box<dyn Fn…> = Box::new(…) }` (confirmed
`m4a_fns_foldl/main.rs:239`, `m1_lambdas/main.rs:239`,
`m1_firstclass/main.rs:238`). The `Box<dyn FnOnce>` continuations
(`task_and_then` at `m0/main.rs:135`) stay Box. So the diff per golden is a
mechanical `Box<dyn Fn(` → `Arc<dyn Fn(` and `Box::new(` → `Arc::new(` at
exactly the `Fn` (not `FnOnce`) carrier sites — NOT a wholesale replace.

Strategy:

1. **Phase 0 baseline:** `cargo run -p refresh-oracle -- --all` is NOT the
   golden-emit regen — `refresh-oracle` recaptures the *Go oracle value*
   (`tools/refresh-oracle/src/main.rs`), not the emitted Rust. The emitted
   `main.rs` goldens are regenerated by the backend's own golden test in
   BLESS mode. Confirm the bless env/flag from
   `crates/sky_backend_rust/tests/` (the byte-compare test) — it is the
   golden harness the clone-relay doc calls "fresh emit + diff". Run it once
   at HEAD to confirm a clean baseline (all 70 byte-identical) BEFORE editing.
2. **After the carrier edits:** run the golden harness in bless mode to
   regenerate all 70 `main.rs`, then `git diff --stat tests/golden` and
   eyeball that EVERY hunk is a `Box`→`Arc` at a `Fn` carrier and nothing
   else. Any golden whose diff touches program *logic* (not just the carrier
   type/ctor) is a RED FLAG — stop and investigate (that would mean the flip
   changed emitted control flow, which it must not).
3. **Re-run `refresh-oracle --all` is NOT needed** — the oracle VALUES
   (program output) do not change; only the emitted Rust type strings do. The
   goldens still run to the same output, so `oracle.meta` stays valid. (If any
   golden's runtime output shifts, that is a correctness bug in the flip.)
4. **Goldens whose CHANGE needs review, not just mechanical accept:**
   - Any Server/WS golden (if present) — the ServerHandler alias arm stays but
     the WS arm is deleted (§1.2); confirm WS callback goldens stay
     byte-identical (they already rendered Arc).
   - `i216_partial_app`, `m1_partial`, `m1_firstclass` — partial-application
     and first-class-value goldens exercise `eta_expand_*` and
     `emit_func_value`; their diff proves the eta path flips cleanly.
   - The `l0105_*` move-seal goldens — these encode the OLD fail-close
     behaviour; a `Fun`-capture reject that they assert must now become a
     GREEN emit. If any `l0105_*` golden is a *negative* (expected-red) test
     for a `Fun` capture, it must be RE-CLASSIFIED to green or deleted with a
     note. Audit each `l0105_*` by hand.

Add the two NEW goldens from the root-cause doc §(f): the 9-line minimal
trigger (was red, now green) and its full-application/top-level-`wrap`
companions (stay green).

---

## 7. The residual latent family in example 36

Root-cause doc lines 252-256 predicts more members hiding behind the
fail-fast. Post-carrier prediction:

- **`kont` continuation capture** (Routes auth) — exempt today only by being a
  depth-0 callee. After the flip it is a `CloneOk` `Fun` capture: it clones
  the Arc at whatever depth it is read. **Dissolves.** No longer relies on the
  depth-0 exemption.
- **`wrap` / `guarded` multi-use (2–4×)** — today would trip
  `reject_fn_value_reuse` (`count_fn_value_uses > 1`). After §1.3 the guard
  `clone_class == NonClone` is false for a plain `Fun`, so multi-use is
  admitted and each use clones the Arc. **Dissolves.**

**The one re-diagnosis round (expected, not a surprise):** after the carrier
edits, `36-composite-server` may reveal a member whose OUTER type is NOT a
plain `Fun` (e.g. a value typed `ServerRequest -> Task Error Response` whose
`clone_class` still routes through a NonClone-carrying composite, or an
`FnOnceChain`/`Decoder` interaction). Instrumentation point to pre-arm the
implementer: the SAME site the root-cause doc instrumented —
`rewrite_captured_clones` at lower.rs:1276 (the `Err(unsupported(.., NonClone
Capture))` construction) and `reject_fn_value_reuse` at lower.rs:4031. Add a
temporary `eprintln!("L0126 fire sym={sym:?} class={:?} depth={depth}", …)`
at both (throwaway, remove before commit) and rebuild `36`. If it fires, the
printed `class` tells you which remaining carrier is involved; cross-check §2
(FnOnceChain/Decoder are intentional stays) vs a genuinely new gap. **Expect
zero fires for the `wrap`/`guarded`/`kont` family; a fire on a Task/Decoder
carrier is a separate, out-of-scope item to file, not to patch under #221.**

---

## 8. Sequencing vs. the clone-relay restructure

Read of `clone-relay-class-macro-design-2026-07-16.md`:

- Clone-relay is about the **move/clone RELAY discipline** — where the
  `let s = s.clone()` pre-clone gets minted at each closure boundary, unified
  from per-binder-site to one boundary-keyed pass. It operates over the
  EXISTING carriers; it does not change what carrier a `Fun` renders as.
- Fix A is about the **CARRIER** — flipping `Fun` from Box to Arc so it is
  `Clone` at all. It does not change where relays are minted.

**Relationship: ORTHOGONAL, with a one-way interaction.** They touch the same
file (`lower.rs`) and adjacent machinery, but different axes: carrier-clone-ness
vs relay-placement. The interaction is one-directional and beneficial: once
Fix A makes `Fun` `CloneOk`, clone-relay's `param_is_multiuse_clonable`
eligibility (which is `CloneOk ∪ bare Generic`) automatically starts admitting
function-typed binders into the lean multi-use relay path — i.e. Fix A
*enlarges the set clone-relay handles uniformly*, and clone-relay's Stage-1
`apply_move_ownership` then subsumes `reject_fn_value_reuse`'s now-dormant
`Fun` case for free. Fix A is NOT a subset or superset of clone-relay; it is
the carrier precondition that lets clone-relay's uniform discipline reach
function values.

**Definitive land order:** land **Fix A FIRST**, then clone-relay. Rationale:

1. Fix A is smaller and self-contained (carrier + derived classifier + golden
   churn), and it is what actually closes the #221 sweep red. Landing it first
   turns `36` green (or reduces it to the §7 re-diagnosis) on its own.
2. Clone-relay's Stage-1 `apply_move_ownership` and Stage-2 `OwnershipClass`
   enum are cleaner to author AFTER `Fun` is `CloneOk`, because the
   `NonCloneFn` arm they carve out (clone-relay §2.3 S-param-fn, §4 `OwnershipClass::NonCloneFn`) then covers ONLY the genuine remnant (Task/
   Decoder/Cmd/Sub/FnOnceChain), not the whole `Fun` family. Sequencing the
   other way would make clone-relay build machinery for a `Fun`-NonClone case
   that Fix A is about to delete.
3. The root-cause doc's own ⚠ OVERLAP note ("land clone-relay first, then
   re-test 36") was written BEFORE this spec resolved that Fix A is the
   narrower, sweep-closing change. This spec supersedes that ordering hint:
   **Fix A first.** Re-test `36` after Fix A; run clone-relay as the
   follow-on campaign that generalises the discipline.

**One campaign or two: TWO.** Fix A ships and greens the sweep independently;
clone-relay is a larger structural pass (its own two stages, its own 70-golden
sweep, its S6 destructure-breach fix) that should not be gated on #221. Keep
them as two sequential campaigns, Fix A → clone-relay, each with its own gate.
Do NOT merge them into one spec — clone-relay's design doc already stands on
its own and its scope (S1–S7 binder sites, destructure breach) is strictly
larger than the carrier flip.

---

## 9. Phase plan (implementer-followable, gated)

Each phase ends on a cheap gate: `cargo check --workspace` +
`cargo nextest run -p <crate>` scoped to the touched crate. The ex-36 seal +
full byte-golden sweep is the FINAL gate. Tag `fix-a-pre` at HEAD for rollback.

**Phase 0 — baseline + recon (no edits).**
- Confirm `task_map`/`task_map_error`/`task_on_error` in `runtime/.../task.rs`
  take `impl Fn` (accept Arc). If any takes a concrete `Box<dyn Fn>`, it must
  be widened to `impl Fn` — add to the plan.
- Run the byte-golden harness at HEAD; confirm 70/70 clean baseline.
- Gate: green baseline recorded.

**Phase 1 — shared predicate + classifier (`sky_ir`/`sky_lower`).**
- Add `carrier_is_clone(&IrType) -> bool` (exhaustive, no `_`), returning
  `true` for `Fun` and every current `CloneOk` type, `false` for
  `FnOnceChain`/`Task`/`Decoder`/`Cmd`/`Sub`/`Generic`.
- Rewrite `clone_class`'s `Fun`/`FnOnceChain` split (lower.rs:991-999) to
  derive from `carrier_is_clone`: `Fun → CloneOk`, `FnOnceChain` stays
  `NonClone`.
- Gate: `cargo check --workspace` + `nextest -p sky_lower`. (No emit yet; unit
  tests on `clone_class` should flip for `Fun`.)
- **SEAL risk:** none yet — this only widens acceptance in the classifier; the
  emitter still says Box, so a skyc-green program could now cargo-FAIL
  (E0308 Arc-annotation-vs-Box-value). **Phase 1 and Phase 2 MUST land
  together in one commit** — do not commit Phase 1 alone.

**Phase 2 — carrier render + ctor (`sky_backend_rust`).**
- `emit_types.rs:319-322`: `Box`→`Arc` in the general `Fun` arm.
- `emit_types.rs:276-291`: delete the WS special-case arm; keep the
  ServerHandler alias arm (:257-262).
- `emit_expr.rs:7742-7756`: `wants_arc_ctor` returns true for all `Fun` (route
  through `carrier_is_clone`).
- Gate: `cargo check --workspace` + `nextest -p sky_backend_rust` (byte
  goldens will FAIL here — that is expected pre-bless).
- **SEAL risk point:** this is where skyc-green-but-cargo-red is caught. After
  bless (Phase 3), a scoped `SKY_E2E=1` build of a `Fun`-heavy golden proves
  the annotation and value agree. Do not proceed past Phase 3 with any golden
  that emits but does not cargo-build.

**Phase 3 — golden regen + audit.**
- Bless-regenerate all 70 `main.rs`; `git diff --stat` review; confirm every
  hunk is a `Fn`-carrier Box→Arc and nothing logical (§6).
- Hand-audit `l0105_*` (move-seal negatives) and `i216_partial_app`/
  `m1_partial`/`m1_firstclass` (§6).
- Add the 3 new goldens (§6 minimal trigger + 2 companions).
- Gate: full `nextest -p sky_backend_rust` green; `git diff` is carrier-only.

**Phase 4 — reach-narrowing in `sky_lower` (dormant-code confirmation).**
- Confirm (do not delete) that `reject_fn_value_reuse`, the depth-0 exemption,
  and the L0126 arm are now unreachable for `Fun` (they stay for the remnant).
  Adjust the `noncl_set` construction so `Fun` symbols route to `clone_set`.
- Gate: `nextest -p sky_lower`; the i164/i168/i172/i193/i199/i218 E2E families
  stay green (they exercise the relay discipline the carrier flip must not
  regress).

**Phase 5 — example 36 seal + full sweep.**
- Build+run `36-composite-server` under the sweep. Expect green OR the §7
  one re-diagnosis round (instrument, read `class`, confirm any residual fire
  is a genuine out-of-scope carrier, file it — do NOT patch under #221).
- FINAL gate: full byte-golden sweep (70+3), `SKY_E2E=1` on the `Fun`-heavy
  goldens, the ex-36 example row green, workspace `cargo check` + `clippy`
  (pedantic/nursery deny — the carrier flip must not introduce a lint).
- **SEAL final check:** every golden that skyc-accepts must cargo-build. The
  byte-golden harness's build step IS this check; do not declare done until
  it is green end-to-end.

Rollback: `git reset --hard fix-a-pre` (Phases 1+2 are one commit, so a
rollback is atomic; Phase 3 goldens revert with it).

---

## Confidence + residuals

Confidence **high** on the carrier locus (single arm, emit_types.rs:319), the
derived-classifier mechanism, the #2 keep-decisions (FnOnceChain/Decoder
carriers inspected in-tree), and the zero-runtime-signature-change finding
(HOF kernels are `impl Fn`; confirmed `list.rs`, `server.rs`). Confidence
**medium-high** on golden byte-neutrality-beyond-carrier (the flip SHOULD be
type-string-only, but the full bless-diff in Phase 3 is the proof, not this
prediction) and on `36` greening in one round (§7 reserves one re-diagnosis).

Residuals (honest):
1. `task_map`/`task_map_error`/`task_on_error` `impl Fn`-ness is asserted from
   the forwarding shim's current compilation, not re-read line-by-line —
   Phase 0 confirms it.
2. The double-Arc wart at UI/HTML event slots (§3.6) is sound but possibly a
   minor Efficiency wart; deferred to a golden-driven follow-up.
3. Deleting the ServerHandler alias arm and merging `emit_shared_lambda` into
   the general path are safe simplifications left OUT of Phase 1 to keep the
   golden churn purely mechanical; both are follow-ups.
4. Any `36` residual on a Task/Decoder carrier (§7) is out of scope for #221
   and filed separately, per the no-symptom-patch rule.

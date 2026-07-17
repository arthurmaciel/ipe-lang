# Fix A — clonable function-value carrier: execution-ready spec

Status: LANDED as §B6 option 1 (the lowered-IR, binder-site promotion — see
§B7 at the end for the implemented design). §§1–5 (universal Arc) remain
FALSIFIED history per §B1; read §B1–B7 as the authoritative design.
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
render `Box<dyn Fn>` (confirmed `tests/golden/basics/main.rs:126-141`), plus every
golden that emits a user closure or first-class function value carries a
`{ let __sky_fn: Box<dyn Fn…> = Box::new(…) }` (confirmed
`fns_foldl/main.rs:239`, `lambdas/main.rs:239`,
`firstclass/main.rs:238`). The `Box<dyn FnOnce>` continuations
(`task_and_then` at `basics/main.rs:135`) stay Box. So the diff per golden is a
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
   - `partial_app`, `partial`, `firstclass` — partial-application
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
- Hand-audit `l0105_*` (move-seal negatives) and `partial_app`/
  `partial`/`firstclass` (§6).
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

---

## Adversarial review amendments

This section supersedes the body above wherever they conflict. Each item is an
in-tree verification of a spec claim, or a correction where the tree differs
from what the spec assumed. Dated design context is sanctioned in an
architecture doc.

### A1. The affected NEGATIVE goldens are `l0127_*`, not `l0105_*` (spec §6 step 4 mis-targeted)

The spec's §6 step 4 says "hand-audit each `l0105_*`; a `Fun`-capture reject
that they assert must now become a GREEN emit." **In-tree audit finds this
target is empty and the real target is elsewhere.** Every `l0105_neg_*`
fixture is a REFUTABILITY / parse negative (`SKY-P0001` / `SKY-T0015`), NOT a
`Fun`-capture reject:

| Fixture | Rejects with | Touched by carrier flip? |
|---|---|---|
| `neg_int_lambda` | SKY-P0001 (`\1 ->` bare literal param) | No |
| `neg_list_lambda` | SKY-P0001 (`\[a] ->`) | No |
| `neg_ctor_lambda` | SKY-T0015 (`\(Just x) ->` refutable) | No |
| `neg_cons_lambda` | SKY-T0015 (`\(x :: xs) ->`) | No |
| `neg_nested_tuple` | SKY-T0015 (`\(a, Just x) ->`) | No |
| `neg_ctor_def` | SKY-T0015 (`f (Just x) =`) | No |
| `neg_money_ctor_param` | SKY-T0015 (`amount (Money d _) =`) | No |

All seven reject at the parser / irrefutability gate, strictly BEFORE lowering's
`clone_class` runs, so the carrier flip cannot turn any of them into an accept.
`alias_move_seal` and `param_patterns` are POSITIVE (`main.rs`
present) — they byte-churn Box→Arc mechanically, no re-classification.

**The negatives that DO flip are the `l0127_*` (fn-value-reuse gate),**
asserted in `crates/skyc/tests/golden_l0114_ctor_payload_function.rs`:

| Test fn | Fixture | Reused type | Post-flip disposition |
|---|---|---|---|
| `fn_carrier_reuse_gated` | `fn_carrier_reuse_gated` | `Maybe (Int -> Int)` let-binding, `consume mf + consume mf` | **Was SKY-L0127; NOW ACCEPT + run.** `clone_class(Maybe(Fun))` = `clone_class_named_composite([Fun])` = `CloneOk` once `Fun` is `CloneOk`; the `Maybe (Arc<dyn Fn>)` clones per use, cargo-builds. Re-classify: assert exit-0 + emitted runs, output `4`. |
| `lambda_param_reuse_gated` | `lambda_param_reuse_gated` | `Maybe (Int -> Int)` lambda param reused | **Was SKY-L0127; NOW ACCEPT + run**, same reason, output `4`. |
| `lambda_param_call_twice_accepted` | `lambda_param_call_twice_accepted` | `f 1 + f 2` (callee) | Stays GREEN, unchanged (already accepted; callee-position). |

Both `*_reuse_gated` re-classifications are **make-invalid-states-unrepresentable
in the correct direction**: the state they rejected is no longer invalid once
the carrier is `Clone`. This is NOT the §0 no-shortcuts trap (deleting a red to
fake-pass) — the program genuinely now compiles and runs the reference answer.
The test bodies must be rewritten to assert the accept + the runtime value, and
a `main.rs` golden captured for each. **Their comments (which say "must stay
SKY-L0127") are rewritten to state the new contract.** This is a required,
audited change — record it explicitly in the implementation commit message so a
reviewer does not mistake it for a weakened gate.

**Correction to spec §6 step 4:** replace "audit each `l0105_*`" with "audit
each `l0127_*` and the `l0114_and_map_*_stays_gated` set." The `l0114_and_map_*`
cases are curried `andMap` chains carried as `FnOnceChain` — `clone_class`
stays `NonClone` (§2.1), so `reject_fn_value_reuse` still fires; they STAY
gated. Verify each `assert_hof_curried_rejected` still holds post-flip (it must,
structurally, because `FnOnceChain` never enters `carrier_is_clone`'s true set).

### A2. `reject_fn_value_reuse` narrows correctly for composites — no soundness hole

The spec §4 keeps `reject_fn_value_reuse` "dormant for `Fun`." Adversarial
composite audit confirms the narrowing is sound in BOTH directions:

* Its guard is `ir_contains_fun(t) && clone_class(t) == NonClone`.
* A composite carrying a `Fun` AND a genuine NonClone (a tuple/record mixing
  `Fun` with `Task`/`Cmd`/`Sub`/`Decoder`/`FnOnceChain`): `clone_class` is
  floored to `NonClone` by the NonClone member (via `clone_class_composite`),
  so the guard STAYS true → still rejected. Correct: the value genuinely cannot
  clone, and the `Fun` member is irrelevant to that verdict.
* A composite of only-clonable members (`(Int -> Int, String)`): now `CloneOk`,
  guard false → accepted, each use clones. Sound (`(Arc<dyn Fn>, String): Clone`).

So `reject_fn_value_reuse` is NOT dead after the flip — it retains exactly the
true-uncloneable remnant. **Delete-vs-dormant decision (spec §4, brief Q3):
KEEP it, not as dead code but as live-narrowed code.** Its guard is a runtime
predicate over the composite's actual clone-class, not a `Fun`-shaped syntactic
match, so it self-narrows the instant `clone_class(Fun)` flips — there is no
provably-unreachable arm to delete here. Deleting it WOULD reopen the
`(Fun, Task)`-tuple / `Task`-returning-composite reuse class (E0382). The
"dormant" framing in §4 is imprecise; the accurate framing is "live, its reach
shrinks to the still-uncloneable carriers." Amend §4's `reject_fn_value_reuse`
row wording accordingly.

### A3. The depth-0 exemption + L0126 arm: KEEP (live-narrowed), do not delete

Same verdict as A2 for the depth-0 callee exemption (lower.rs:1313) and the
L0126 fail-close arm (lower.rs:1276). After the flip, a `Fun` capture is
classified `CloneOk` at the `captured_locals` split (lower.rs:8208-8218,
14565-14574, 14624-14634) — it enters `clone_set`, never `noncl_set` — so it
reaches `rewrite_captured_clones` as a `CloneVar` at ANY depth and never touches
line 1276 or the 1313 exemption. But `noncl_set` still receives
`Task`/`Cmd`/`Sub`/`Decoder`/`FnOnceChain`/`Generic` captures, for which the
depth-0 exemption and the L0126 arm remain load-bearing. **Both stay; neither
is dead.** This is the brief's Q3 "make-invalid-states-unrepresentable"
tension resolved by evidence: the arms are unreachable FOR `Fun` but reachable
for the genuine-NonClone set, so they are not provably-dead code — deleting them
reopens the Task/Decoder capture class. The structural guarantee that `Fun`
cannot reach them is discharged by the SINGLE `carrier_is_clone` predicate
routing `Fun` into `clone_set`, which is where make-invalid-states-unrepresentable
actually lands — not by deleting a still-reachable arm.

**No new emit_expr classifier edit is needed** for the three `captured_locals`
sites: they all already `match ir_ty.as_ref().map(clone_class)` and route
`CloneOk → clone_set`. Flipping `clone_class(Fun) → CloneOk` re-routes `Fun`
automatically. Confirmed at all three sites in-tree.

### A4. The S4b Arc-promotion path is the ONE real byte-stability / SEAL risk — sequencing

The tree already contains clone-relay Stage 1 (`apply_move_ownership`,
lower.rs:4016 — the spec §8 "land clone-relay first" is partly overtaken: Stage
1 has landed; Fix A still lands orthogonally ON TOP). The live interaction:
`lower_let_pvar`'s else-branch (lower.rs:14661-14730) now does, for a `Fun`-typed
`let` binding:

1. `apply_move_ownership(name, ir_ty, acc, ...)` — with `Fun` now
   `param_is_multiuse_clonable` (CloneOk), runs `rewrite_multiuse_clones`,
   turning all-but-last `Var(name)` into `CloneVar(name)`. **Before Fix A this
   took the `reject_fn_value_reuse` else-arm** (no rewrite for single-use).
2. THEN the S4b block: `if matches!(ir_ty, IrType::Fun(..)) && (needs_shared_capture
   || flows_into_sync_kernel_call)` → `force_shared_capture_clones(name, acc)` +
   promote to `SharedLambda` (Arc carrier) + `promote_unification_sibling_lambdas`.

Two questions this raises, and the pre-registered answers:

* **Soundness (no double-consume):** `rewrite_multiuse_clones` and
  `force_shared_capture_clones` both operate by inserting `CloneVar`/relay
  shadows; neither removes a use. `CloneVar` reads are idempotent under a second
  clone pass (`force_shared_capture_clones` treats `Var` and `CloneVar` leaves
  identically at line 2872-2873 and only ADDS relay shadows at lambda
  boundaries). An `Arc::clone` of an `Arc::clone` is still a refcount bump — no
  double-move, no E0507. SOUND.
* **Byte-stability (SEAL / golden churn):** this composition is the ONE place the
  flip can produce output that is NOT a pure Box→Arc type-string swap — the
  depth-0 `rewrite_multiuse_clones` may now insert a `CloneVar` that the old
  NonClone path did not. **This is EXPECTED and CORRECT** (an Arc carrier SHOULD
  clone per non-last use), but it means a golden exercising a multi-use `Fun`
  `let`-binding that ALSO hits `needs_shared_capture`/`flows_into_sync_kernel_call`
  (the S4b `#164`/`#168`/`#172` families: `i164_*`, `i168_*`, `i172_*`,
  `input_arc_capture`) may show a diff BEYOND the type string. **Phase 3
  must eyeball these specific goldens by hand, not bless-accept blindly.** If a
  diff there is a `Var→CloneVar` at a non-last multi-use position, it is the
  intended lean clone and is accepted; if it changes control flow or drops a
  relay, STOP (that is a real regression). This is the spec's medium-high
  confidence item made precise: the risk is localized to the S4b-∩-multiuse
  goldens, nowhere else.

**Ordering within `lower_let_pvar` stays as-is** (move-ownership THEN S4b
promotion): reversing it would make `needs_shared_capture` count over an
un-rewritten body — no benefit, and the current order is what the landed Stage 1
established. Do not reorder.

### A5. eta-expand `Var→CloneVar` flip for `Fun` slots — comment debt + the `Err` fallback

At `eta_expand_partial` (lower.rs:10684-10761) and the sibling eta sites
(10918, 11069, 11358), `slot_classes` computes `clone_class` per supplied arg.
Post-flip, a `Var(sym)` in a `Fun` slot classifies `CloneOk` and rewrites to
`CloneVar(sym)` (line 10703) instead of staying bare. **This is correct** — the
forwarded `Arc<dyn Fn>` is cloned into the eta-lambda, and the runtime HOF takes
`impl FnOnce` (accepts a moved-in Arc clone), so it compiles and is sound. But:

* The comment blocks at 10706-10713 ("a function/task/decoder variable forwarded
  … moving the Var in is a plain ownership transfer") and 10735-10741 ("a NonClone
  fresh construction, safe to inline") are now STALE for the `Fun` sub-case. They
  describe the pre-flip world. **Update them to say: `Fun` slots are now `CloneOk`
  and forward via `CloneVar`; only `Task`/`Cmd`/`Sub`/`Decoder`/`FnOnceChain`
  remain the bare-move NonClone forward.** Comment-only; no logic change (the
  match already routes `CloneOk → CloneVar`).
* **The `Err(_) if matches!(slot_ty, Ty::Fun(_, _)) => Some(CloneClass::NonClone)`
  fallback (10694, 10918, 11358) — KEEP as NonClone, do NOT flip to CloneOk.**
  Rationale: this arm fires ONLY when `ir_type_from_ty` FAILED, i.e. the slot's
  arrow carries an unresolved nested `Ty::Var` (a polymorphic `Task Error a`
  result, etc.). A bare-move forward of such a value into a fresh (non-nested)
  eta-lambda is always sound (plain ownership transfer, the eta-lambda is not
  captured inside another closure), and we cannot render a `CloneVar` clone for
  a type we could not resolve to an `IrType` anyway. Flipping it would be
  guessing `Clone`-ness of an unresolved type — a soundness risk for a NonClone
  instantiation. The resolvable path (10689 `Ok(ir_ty) => clone_class(&ir_ty)`)
  already flips `Fun → CloneOk` correctly; the `Err` fallback stays conservative.
  This is `parse-don't-validate`: resolve when you can (flip), stay fail-safe
  when you cannot (bare NonClone forward, which is sound for a fresh eta-lambda).

### A6. `carrier_is_clone` — exhaustive, in `sky_ir`, the single authority

Placement: `sky_ir` (both `sky_lower` and `sky_backend_rust` depend on it;
`IrType` lives there). Signature `fn carrier_is_clone(t: &IrType) -> bool`,
exhaustive over all 43 variants with NO `_` arm (a future variant forces a
decision — SEAL make-invalid-states-unrepresentable). Its `true` set is EXACTLY
today's `CopyLeaf` ∪ `CloneOk` leaves PLUS `Fun`, recursing on composites the
same way `clone_class` does. The `false` set is
`FnOnceChain`/`Task`/`Decoder`/`Cmd`/`Sub`/`Generic` and any composite
transitively carrying one.

Crate-direction constraint: `clone_class` lives in `sky_lower`, and `sky_ir`
must NOT depend on `sky_lower`. So `carrier_is_clone` is the PRIMARY authority
in `sky_ir` and `clone_class` (in `sky_lower`) CONSULTS it — not the reverse.
Concretely:

* `clone_class`'s `Fun` arm moves from the `NonClone` bucket (lower.rs:991-999)
  to the `CloneOk` bucket; `FnOnceChain` stays in `NonClone`. The
  `CopyLeaf`-vs-`CloneOk` three-way refinement stays entirely in `clone_class`
  (a `sky_lower` concern the emitter never needs).
* `carrier_is_clone` in `sky_ir` returns `false` for
  `FnOnceChain`/`Task`/`Decoder`/`Cmd`/`Sub`/`Generic` and composites carrying
  them; `true` for every `CopyLeaf`/`CloneOk` leaf and `Fun`.
* The emitter (`render_type` Arc-vs-Box for `Fun`; `wants_arc_ctor` Arc-vs-Box
  ctor) consults `carrier_is_clone` DIRECTLY — so a shape that renders Arc is
  `carrier_is_clone == true`, and `clone_class` classifies that same shape
  non-`NonClone` because it reads the same predicate. One boolean in `sky_ir`,
  two readers; the tables provably cannot drift.
* Add a `debug_assert` / unit test that `carrier_is_clone(&t) == (clone_class(&t)
  != NonClone)` over a representative variant set, so a future edit to one that
  forgets the other fails a test rather than reopening the drift.

### A7. `emit_shared_lambda` type-string vs the general arm — byte, not SEAL

`emit_shared_lambda` (emit_expr.rs:7895) emits `::std::sync::Arc<dyn Fn(..) ->
R + Send + Sync + 'static>`; post-flip `render_type(Fun)` emits `Arc<dyn Fn(..)
-> R + Send + Sync + 'static>` (bare `Arc`, project prelude `use`s it). These are
the SAME Rust type — a `::std::sync::Arc` / `Arc` mismatch at a unification slot
is not an E0308 (path aliases resolve identically). So NO SEAL risk. It IS a
byte difference if a golden's `SharedLambda` sits next to a `render_type(Fun)`
string. §3.4's "leave `emit_shared_lambda` in place for Phase 1" stands; note
the two now emit type-equal (not byte-equal) strings, which is fine. Merging
them is a follow-up (residual #3). Do NOT touch in this change.

### A8. SEAL atomicity — the ONLY skyc-green-cargo-red window

The classifier edit (`clone_class(Fun) = CloneOk` via `carrier_is_clone`) and
the emitter edit (`render_type(Fun)` Box→Arc + `wants_arc_ctor` all-`Fun`) MUST
land in ONE commit. Between them lies a real SEAL breach:

* Classifier-only: a reused `Fun` `let`-binding now gets `.clone()` inserted
  (`CloneVar`), but `render_type` still emits `Box<dyn Fn>` (not `Clone`) →
  emitted `arc.clone()` on a `Box` value → cargo E0599 `clone` (Box<dyn Fn> has
  no Clone). skyc-green, cargo-red.
* Emitter-only: `render_type` emits `Arc<dyn Fn>` but `wants_arc_ctor` still
  says `Box` for a general `Fun` → `let __sky_fn: Arc<dyn Fn> = Box::new(..)` →
  cargo E0308. skyc-green, cargo-red.

Confirmed there is NO OTHER emit site that renders the general `Fun` carrier
type-string (audit: `emit_types.rs:320` is the sole `Box<dyn Fn(` **emission**;
every other `Box<dyn Fn(` match is a comment or the `FnOnceChain` /
`shouldRetry`-field / retry-adapter path, none of which the `Fun` flip reaches).
The `shouldRetry` field (`emit_expr.rs:1513`) is a `RetryPolicy` runtime-struct
field typed `Box<dyn Fn(SkyError) -> bool>`, populated from a `Fun` VALUE via
its own adapter — after the flip the incoming value is `Arc<dyn Fn>`, and the
adapter re-wraps (like `arc_callback_wrap`); it does not render the general
carrier. Verify its golden (`i164`/retry family) in Phase 3; expect a re-wrap,
not a raw carrier mismatch.

Phase 1+2 = one commit (as spec §9 says). This is confirmed as the complete
SEAL risk map — no third window exists.

### A9. Verdict on the brief's re-decision asks

* **FnOnceChain stays Box (§2.1): CORRECT, Correctness-necessary, not merely
  conservative.** The runtime `curryN`/`decode_pipeline_*`/`db_decode_*` seams
  take `Box<dyn FnOnce>` towers (json.rs, db.rs) — a `FnOnce` is
  consume-once by TYPE. Migrating to `Arc<dyn Fn>` is a semantic change (a `Fn`
  can be re-called; the pipeline's one-shot `next_decoder` contract does not
  want that) requiring runtime signature edits, for zero soundness gain (the
  chains are never captured across a re-callable `Fn` boundary — they have no
  reuse to admit). KEEP Box.
* **Decoder stays a struct with a `Box<dyn Fn>` field (§2.2): CORRECT.** A Sky
  `Decoder a` is a nominal runtime carrier, not a first-class `Fun` value;
  `clone_class` sees `IrType::Decoder`, never the field. The `Decoder (A -> B)`
  payload rides in the `T` slot and flips to `Arc<dyn Fn>` automatically (T is a
  `Fun`); `Decoder<E, Arc<dyn Fn + Send + Sync>>: Send` holds. No Decoder-specific
  work. KEEP.
* **A family member that DOES dissolve beyond the spec's list:** the
  `l0127_*_reuse_gated` fn-value-REUSE class (A1) — the spec framed
  `reject_fn_value_reuse` as merely going dormant, but in fact a whole class of
  previously-rejected programs (a `Maybe (Int -> Int)` or any composite-of-only-
  clonables reused N×) now COMPILES. That is a genuine completeness gain the
  spec undersold; it is captured by the same carrier flip, no extra work, and it
  is why the two `l0127` negatives must be re-classified rather than left red.

### A10. Position-independence proof (brief Q4, #172 anti-pattern)

The invariant "acceptance of a well-typed capture never depends on syntactic
position" holds by construction after the flip: a `Fun` capture is classified
`CloneOk` at the `captured_locals` split regardless of whether it is read as a
depth-0 callee, a depth-≥1 forwarded arg, a sibling partial-app residual, or a
multi-use value — all four routes reach `rewrite_captured_clones` /
`rewrite_multiuse_clones` and emit `CloneVar` (Arc clone). No arm inspects
"callee vs arg" or "depth 0 vs ≥1" FOR a `Fun` anymore (the depth-0 exemption
and the L0126 arm are reached only by the genuine-NonClone set, A3). The #172
anti-pattern (coerce inline-lambda siblings only) is structurally avoided:
`promote_unification_sibling_lambdas` already eta-expands EVERY function-typed
leaf (not just inline lambdas), and after the flip even that path is only
exercised for the S4b Arc-promotion sub-case; the ordinary `Fun` capture needs
no sibling coercion at all because every `Fun` renders the same Arc carrier
unconditionally. Position-independence is a THEOREM of the single-carrier +
single-classifier design, not a per-site patch.

---

## Implementation outcome — the universal-Arc premise is UNSOUND; adopt the reference's lean position-typed carrier

This section is the authoritative correction. It SUPERSEDES the universal
Box→Arc carrier flip in §1–§3 and the "zero runtime signature edits" claim in
§5. The universal flip was implemented and empirically FALSIFIED at the cargo
build (THE SEAL), then reverted. The correct target is the reference's
lean, position-typed carrier model. Evidence and design below.

### B1. `Arc<dyn Fn>` does NOT satisfy `impl Fn` — the SEAL break the spec missed

Spec §5 asserts "an `Arc<dyn Fn>` satisfies `impl Fn` (blanket `impl Fn for
Arc<F: Fn>` via `Deref`)". **That blanket impl does not exist in std.** std
provides `impl<A, F: Fn<A> + ?Sized> Fn<A> for Box<F>` and `... for &F`, but
NOT for `Arc<F>`. Verified by a standalone `rustc` probe:

* `Box<dyn Fn(i64)->i64>` as an `impl Fn` arg → compiles.
* `Arc<dyn Fn(i64)->i64>` as an `impl Fn` arg → `E0277: expected a Fn(_) closure,
  found Arc<dyn Fn…>`.
* `&*arc` (`&dyn Fn`) as an `impl Fn` arg → compiles.

Consequence: after the universal flip, EVERY HOF-kernel call whose function
argument is `impl Fn` (`list_map`, `list_filter`, `list_foldl`, `list_foldr`,
`task_map`, `task_and_then`, … — 53 `impl Fn`/`impl FnOnce` param sites across
the runtime) fails `cargo build` with E0277. Confirmed by ISOLATED clean builds
(fresh per-golden target, no cache contamination) of the freshly-emitted
`fns_map` and `fns_foldl`: both `error[E0277]` — `Arc<dyn Fn>` where the
kernel wants `impl Fn`. This is a skyc-exit-0-then-cargo-fail across the entire
HOF surface — the exact class THE SEAL forbids. The universal-Arc flip is
therefore rejected outright.

(Caution recorded for the implementer: a SHARED cargo target gives FALSE PASSes
here — the emitted binary artifact name (`sky-app`) collides across goldens, so
cargo reuses a stale prior compile and never rebuilds the changed `main.rs`.
Only an ISOLATED per-golden target reveals the real E0277. This masked the break
on the first pass.)

### B2. The reference does NOT use a universal Arc carrier — it is position-typed and lean

`../sky/src/Sky/Generate/Rust/Builder/ExprEmitter.hs` renders a function value
by POSITION, in the leanest form each position admits — matching the user
directive "if we don't need Arc or Box, don't use it":

* **HOF `impl Fn` arg (the common case):** a lambda renders as a BARE `move
  |..| ..` closure — no `Box`, no `Arc`, zero heap indirection. Captured non-Copy
  vars inside still `.clone()` at use (`ecCloneVars`). This is `argToRustString
  ctx noCloneFn a`; the `isListHofClosurePos` / kernel list (`list_map`,
  `list_filter`, `list_foldl`, …) marks these arg slots.
* **Stored into an `Arc<dyn Fn>` field / event-callback / Handler slot:**
  `wrapStoredFn` Arc-wraps (`Arc::new(move ..)` + pre-clone captures) — ONLY when
  the slot type is genuinely `Arc<dyn Fn>` (`isHandlerArcParam` / `isEventCbParam`
  gate on the rendered param type).
* **A function VALUE (an already-built `Arc<dyn Fn>`, e.g. a `Handler` binding)
  flowing into an `impl Fn` slot:** RE-DISPATCH through a fresh concrete closure
  `{ let __hcb = v.clone(); move |a| __hcb(a) }` (ExprEmitter.hs ~2118-2137).
  The fresh closure `impl Fn`s; it captures the `Clone` Arc. (Probe-verified:
  `move |a,b| (arc.clone())(a,b)` satisfies `impl Fn + Clone`.)
* **Captured function value forwarded across a closure boundary (the #221
  shape):** `arcWrapClosure` — Arc-wrap + pre-clone the captured outer vars, so
  the Arc owns `'static` captures and the outer closure stays re-callable.

So the reference's Arc is applied ONLY where a value is STORED or CAPTURED-AND-
FORWARDED — never at a bare HOF-arg position. Our current tree pins `Box<dyn
Fn>` universally (which happens to satisfy `impl Fn`), with Arc special-cases at
Server/WS slots. The correct move is toward the reference's leaner model, not a
universal Arc.

### B3. The narrow #221 fix and WHY the carrier alone cannot deliver it

#221 (`36-composite-server`, minimal `wrap`/`guarded`) is a CAPTURE problem: a
function value `wrap` is captured as a callee at lambda-nesting depth 1 inside
`guarded`'s eta-expanded residual. At depth 1 a bare `Box<dyn Fn>` capture is
unsound (the inner `move` closure steals it from the outer env → E0525), so the
lowerer fail-closes SKY-L0126. The ONLY sound bare position is a depth-0 callee.

The lean fix is to Arc-promote a fn-value captured at depth ≥ 1 (Arc is `Clone`,
so each capturing closure clones the pointer — a refcount bump). The tree
ALREADY has this machinery for `let`-bound lambda LITERALS: `needs_shared_capture`
→ `Expr::SharedLambda` (Arc) + `force_shared_capture_clones` (the pre-clone
relay). Two structural blockers were found by probing, and BOTH must be solved
for a sound fix:

1. **Ordering / look-ahead.** The capture classifier (`rewrite_captured_clones`
   in `lower_lambda`) runs when `guarded` is lowered and classifies `wrap` via
   `clone_class(Fun) = NonClone` → L0126 — BEFORE `lower_let_pvar` ever promotes
   `wrap`. So `wrap`'s Arc promotion is decided too late to inform the classifier.
   Widening `needs_shared_capture` to fire on any depth ≥ 1 does NOT fix this:
   the L0126 still fires first. **The classifier must KNOW, when it classifies a
   capture, that the captured symbol will be Arc-carried.** This needs a lowerer
   pre-pass that computes the Arc-promotion set for a scope's fn-typed bindings
   BEFORE lowering the bindings that capture them, threaded into `captured_locals`
   so a promotion-bound symbol classifies `CloneOk` (→ `CloneVar`) not `NonClone`.

2. **Promotion must cover NON-lambda-literal fn-values.** Probe: widening
   `needs_shared_capture` alone made `force_shared_capture_clones` emit a
   pre-clone `{ let g = g.clone(); … }` for `g = f 1` (a partial-app VALUE, a
   `Call` not an `Expr::Lambda`). The `SharedLambda` promotion is gated `if let
   Expr::Lambda = value`, so `g` stayed `Box<dyn Fn>` — and `g.clone()` on a
   `Box<dyn Fn>` is `E0599` (Box<dyn Fn> is not `Clone`). Confirmed by isolated
   build of `partial_app`. So the promotion path must be generalised to
   change the CARRIER of any captured fn-value (lambda literal, partial-app
   `Call` result, top-level fn-item reference) to `Arc<dyn Fn>` — not just
   lambda literals — before any `.clone()` relay is minted on it.

These two are the real work #221 needs. Both are sound and bounded but exceed a
mechanical carrier flip: they rearchitect the fn-value promotion + capture-
classification path. A partial version (either alone) is SEAL-UNSOUND
(E0599/E0126), so it must land whole.

### B4. Corrected design (supersedes §1–§3)

Do NOT flip the general `Fun` carrier to Arc. Instead:

1. **Keep the default fn-value carrier lean.** Long-term target: bare closures at
   `impl Fn` HOF-arg positions (drop the `Box` pin there too — the reference
   does), matching the user directive. Minimum for #221: keep today's `Box`
   default (it satisfies `impl Fn`) EXCEPT at capture-and-forward positions.
2. **Arc ONLY at capture-and-forward positions.** Generalise the S4b promotion:
   a fn-typed binding captured at lambda-depth ≥ 1 (any fn-value shape, not only
   lambda literals) is promoted to an `Arc<dyn Fn>` carrier at its binding site,
   and every capturing closure reads it via `CloneVar` (`Arc::clone`).
3. **Look-ahead pre-pass.** In `lower_let` (and the top-level-def path for
   `36`'s `kont`/handler shapes), compute the depth-≥1-captured fn-typed symbol
   set from the CANON scope BEFORE lowering the capturing bindings; thread it
   into `captured_locals`'s classifier so those symbols route to `clone_set`
   (CloneVar), never `noncl_set` (L0126). This is the structural discharge of
   the ordering blocker (B3.1).
4. **`carrier_is_clone` stays** (already landed in `sky_ir`) as the single
   authority for "does this carrier implement `Clone`", consulted by both the
   classifier and the (position-typed) emitter. `Fun`'s carrier-clone-ness is now
   POSITION-DEPENDENT (Arc at capture positions → Clone; bare/Box at HOF-arg →
   not `Clone` but also never captured), so `carrier_is_clone` describes the
   Arc-capture carrier specifically; the HOF-arg bare closure is a distinct
   position the classifier never asks about (it is consumed, not captured).
5. **HOF-arg re-dispatch for Arc VALUES.** Where an already-Arc fn VALUE flows
   into an `impl Fn` kernel slot, emit the reference's re-dispatch `{ let v =
   v.clone(); move |a..| (v)(a..) }` so the concrete closure satisfies `impl Fn`.

The `l0127_*_reuse_gated` re-classification (A1) still holds in spirit but its
mechanism changes: a `Maybe (Int -> Int)` reused compiles once the inner `Fun`
is Arc-carried AT THE CAPTURE/STORE position; the reuse gate narrows the same
way. Re-audit against the position-typed carrier, not the universal one.

### B5. Status, and why this is an honest escalation (§0)

The universal-Arc approach the spec prescribed is unsound (B1) and was reverted
to a green baseline. The correct fix (B3/B4) is a bounded but substantial
rearchitecture of the fn-value promotion + capture-classification path — a
multi-stage change with its own golden sweep and SEAL gate — not the one-atomic-
commit carrier flip the spec scoped. Landing a partial version is SEAL-unsound
(proven: E0277 HOF-arg break, E0599 Box-clone break). Per PRINCIPLES §"root
causes only" and DEVELOPMENT.md §0 (an honest tracked block beats a fake seal),
`36-composite-server` remains RED pending the B4 rearchitecture; the diagnosis,
the falsification of the universal-Arc premise, and the corrected reference-
faithful design are the durable artifacts delivered here. The `carrier_is_clone`
predicate (sound, additive, unused-until-B4) is retained in `sky_ir`.

### B6. What WAS landed (partial, sound) and the exact blocker for the rest

Implemented and verified in `sky_lower` (byte-neutral across all 67 goldens,
`sky_lower`+`sky_ir` tests green, workspace `cargo check` green):

* `sky_ir::carrier_is_clone` — the single 2-valued carrier-`Clone` authority
  (exhaustive, no `_`), plus a `clone_class == !NonClone` agreement test.
* A **per-`let` canon look-ahead pre-pass** (`lower_let`): a fn-typed lambda
  `let` binding that is captured at lambda-depth ≥ 1 OR reused (> 1 non-callee
  use) IN ITS OWN CANON `let` SCOPE is recorded in `arc_promoted_fn_syms`. The
  capture classifier (`captured_locals` in `lower_lambda`) routes those symbols
  to `clone_set` (`CloneVar`) instead of SKY-L0126, and `lower_let_pvar` routes
  them to `rewrite_multiuse_clones` + the `SharedLambda` (Arc) promotion instead
  of `reject_fn_value_reuse` — one set drives all three, so the carrier and the
  `.clone()`s agree (no E0599/E0126). Helpers: `canon_sym_captured_at_depth_ge1`,
  `canon_fn_value_uses`.

This GREENS the DIRECT (non-eta-synthesized) #221 shape — the minimal
`wrap`/`guarded` trigger where the capturing lambda exists verbatim in canon:
`skyc` exit 0, emitted Rust cargo-builds, runs to the reference value. `wrap` is
Arc only where captured; `guarded` and the eta-lambdas stay `Box`. That is the
leanest correct carrier, exactly the user directive.

**The remaining blocker — proven by instrumentation, the reason `36` is still
red:** in `36-composite-server`, `wrap` trips SKY-L0127 with `count=2` NON-callee
uses that DO NOT EXIST IN CANON. They are SYNTHESIZED by eta-expansion DURING
lowering: `guarded h = wrap (rateLimit … h)` supplies 1 of `wrap`'s 2 flattened
args, so `eta_expand_value_partial` builds a residual `\eta_0 -> wrap(<partial>,
eta_0)` that CAPTURES `wrap` as a value inside a new lambda. A canon-level scan
(the pre-pass) cannot see these — they are not in the source AST. Probe:
`PREPASS-DECIDE name=wrap captured=false reuse_n=0`, yet post-eta
`reject_fn_value_reuse(wrap) count=2`. So a canon pre-pass is STRUCTURALLY
insufficient for the eta-synthesized capture/reuse; the promotion decision must
be made on the LOWERED IR, after eta-expansion, but the capture classification
currently runs DURING lowering — the ordering problem, now proven unfixable by a
pre-pass.

**The complete fix (for the next agent) is one of:**

1. **Lowered-IR promotion post-pass (recommended, lean).** After lowering a def
   body (before TCO), run a whole-`Func`-body pass that: (a) finds every fn-typed
   IR binding captured at closure-depth ≥ 1 or reused as a value > 1× (reusing
   the existing IR walkers `collect_lambda_capture_depths` / `count_fn_value_uses`
   — these see the eta-synthesized closures because they run post-lowering), (b)
   flips those bindings' carrier to `Arc<dyn Fn>` (`SharedLambda`), and (c)
   rewrites their reads to `CloneVar`. This is the def-level generalisation of the
   per-`let` pre-pass already landed — same logic, moved to post-lowering so it
   sees eta-synthesized captures. Keeps the lean carrier (Arc only where needed).
   Must also cover non-lambda fn-VALUES (`g = f 1`, B3.2) by minting the Arc
   carrier for any promoted fn binding, not only `Expr::Lambda` literals.
2. **Reference uniform-Arc + HOF-arg discipline (B2/B4).** Bigger: Arc every fn
   carrier, then add the `impl Fn` HOF-arg re-dispatch (`move |a| (v.clone())(a)`)
   + bare-closure-at-HOF-arg so the 53 `impl Fn` kernel sites still compile.

Option 1 is smaller, matches the lean directive, and directly extends the landed
per-`let` pre-pass; prefer it. The per-`let` pre-pass can be REPLACED by the
def-level post-pass (not kept alongside) once (1) lands.

## B7. Implemented design (option 1, landed — replaces the per-`let` canon pre-pass)

The promotion decision runs on the LOWERED IR at each BINDER site, which is
where the fully-lowered scope (eta-synthesized closures included) is in hand —
the "def-level post-lowering pass", realised scope-by-scope as lowering
unwinds, completing before TCO:

* **Single authority** — `sky_ir::fun_value_arc_promotable` (pure `IrType::Fun`
  only; `FnOnceChain`/`Decoder`/composites excluded) says which binding shapes
  may take the `Arc` carrier; `sky_ir::carrier_is_clone(Fun)` is now honestly
  `false` (the DEFAULT carrier is `Box<dyn Fn>`; the promoted `SharedLambda`
  carrier is the position-typed `Clone` exception) and agrees exactly with
  `clone_class`.
* **Deferral, not rejection** — `lower_lambda`'s capture classifier routes a
  captured pure-`Fun` symbol whose binder is REGISTERED promotable (plain `let`
  names + def/lambda params, via `promotable_fn_binders`) away from the
  SKY-L0126 fail-close, leaving reads bare; a signal (`deferred_fun_captures`)
  lets a binder that cannot resolve the `Fun` shape re-raise the original
  SKY-L0126 (fail-closed both ways). Destructure/match-arm-bound fn symbols
  are not registered — they keep today's honest fail-close.
* **Binder-site triggers, on the lowered scope** — a pure-`Fun` `let`
  (`lower_let_pvar`) or param (`apply_param_move_ownership`) is promoted when
  `fn_value_read_flags` finds a non-callee read at closure depth ≥ 1 (E0507
  class), a read at depth ≥ 2 (params; lets get this from
  `needs_shared_capture`), or `count_fn_value_uses > 1` (E0382 class). Lean
  shapes (depth-0 callee, single move) stay bare `Box`, byte-identically.
* **Read reconciliation** — a promoted binding's non-callee reads are
  re-dispatched through fresh `Box` closures (`shim_fn_value_reads` →
  `Box::new(move |a..| (sym.clone())(a..))`, the reference's `redispatch`
  shim), because `Arc<dyn Fn>` satisfies neither a `Box<dyn Fn>` slot nor an
  `impl Fn` bound; capture relays come from `force_shared_capture_clones`;
  reuse clones from `rewrite_multiuse_clones`; callee reads call the `Arc`
  by auto-deref. Reads inside `requires_sync_capture` kernel args are the
  sync-promotion path's slots and stay untouched (byte-pinned).
* **Carrier mint** — a promoted `let`'s lambda value flips to
  `Expr::SharedLambda` (the reference's `arcWrapClosure`); a promoted
  NON-lambda value or param shadow-rebinds via `eta_shared_rebind`
  (`Arc::new(move |a..| (value)(a..))`, moving the underlying value in once) —
  a param's SIGNATURE (and every caller) keeps the lean `Box`.
* **Flatten-invariant completion** (unmasked by the same family) — a lambda
  whose body COMPUTES a function (`wrap h = cors (withLogging h)` :
  `Handler -> Handler`) is padded with trailing eta params applying the
  computed value, so the closure's arity always equals its flattened type's —
  the arity every consumer (`ty_arrow_arity`, `eta_expand_value_partial`,
  `render_type`) already assumes.

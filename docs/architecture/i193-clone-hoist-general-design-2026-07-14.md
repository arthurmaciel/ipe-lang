# #193 — General clone-hoist for reused non-`Copy` bindings across the Task/CLI pipeline

**Status:** design v2 (no code). READ-ONLY recon re-verified against HEAD `99dbd82`.
**Backlog item:** #193 — reused non-`Copy` bindings captured into `move` closures across the
Task/CLI pipeline double-move → cargo `E0507`/`E0382`.
**Scope:** `crates/sky_lower/src/lower.rs` (counter + `rewrite_multiuse_clones` `Match` arm)
**plus** a scoped `crates/sky_backend_rust/src/emit_expr.rs` change for the `ui_on_input_` /
`ui_on_change_` inline-wrap sites (D5). New goldens under `tests/golden/`. No runtime change.
**Reference:** `../sky/src/Sky/Generate/Rust/Builder/ExprEmitter.hs`
(`collectVarLocalsMulti` :394 — arms MAX; `collectFreeVarLocalsMulti` :468 — arms SUM;
`varLocalRead` :782; `ecCloneVars` / `ecNoCloneVars` set-membership; `defToRustString` prelude
:3922/:3997).

> **This is design v2.** v1 (adversarial-reviewed) rested on two false claims and one wrong
> premise: (D1) it cited `collectFreeVarLocalsMulti` as MAXing arm bodies — it does not, it
> SUMs; (D2) it prescribed seeding a single shared `remaining` counter from `scrutinee_sum +
> arm_max`, which is UNSOUND for our sequential-DFS decrementing counter; (D4) it claimed a
> "driver-scope gap" that `lower_let_pvar` :13049 already closes. v2 re-derives every reference
> claim from live code (each verified below), replaces the MAX-on-shared-counter with a
> **per-arm snapshot/restore** of `remaining`, drops the redundant whole-body pass, and adds the
> genuinely-uncovered `ui_on_input_` emitter site.

---

## 0. TL;DR

The clone-insertion *traversal* is correct and complete. `rewrite_multiuse_clones`
(`lower.rs:3164`) recurses through every `Expr` shape — `TaskSeq` (`:3386`), `TaskSeqSync`
(`:3390`), `Call` (`:3298`), `Apply` (`:3303`), `Ctor` (`:3381`), `Record` (`:3341`) — and its
`Lambda` (`:3187`) / `SharedLambda` (`:3210`) arms already hoist the pre-clone `Let` **outside**
the `move` closure (gotcha #5). The four driver sites (per-param `:5959`/`:6121`, `lower_let_pvar`
`:13054`, match-arm pvars `:13304`) all invoke it, and `lower_let_pvar` runs it over the FULL
let body `acc` — reaching downstream `TaskSeq`/`Call`/`cmd_perform` closures.

Two REAL defects remain:

1. **The arm counters over-count (SUM, not per-arm-exclusive).** `count_var_uses`
   (`lower.rs:2504`, `Match`) and `count_fn_value_uses` (`lower.rs:3051`, `Match`) both `.sum()`
   the per-arm use counts, and their `If` arms (`:2501` / `:3046`) SUM `then_ + else_`. Arms are
   mutually exclusive: only ONE runs, so the whole-body peak consuming count is
   `count(scrutinee) + MAX(arm bodies)`, not the sum. Over-counting seeds `remaining` too high →
   the taken arm's genuine last use is clobbered into a `.clone()` (efficiency regression on
   `CloneOk`; `E0599` where the type is not actually `Clone`). **But the fix is NOT "MAX the
   shared counter"** — see §2.2 (D2): our decrementing counter is sequential, so MAX is
   order-dependent and unsound. The correct fix is per-arm SNAPSHOT/restore of `remaining` at
   the `Match`/`If` arm of `rewrite_multiuse_clones` itself, with the counters MAXing only to
   compute the *seed*.

2. **The `ui_on_input_` / `ui_on_change_` inline-wrap emitter sites are uncovered (D5).**
   `KernelFn::UiOnInput` (`emit_expr.rs:5051`) emits
   `ui_on_input_(Arc::new(move |_x| ({f_s})(_x)))` where the outer `move |_x|` is
   EMITTER-SYNTHESIZED, not an `Expr::Lambda` in the lowerer IR. A lowerer pre-clone around
   `f_e`'s inner Lambda lands INSIDE that synthesized move closure and cannot rescue an
   already-moved sibling capture (the #191 bug shape). The #191 `emit_arc_callback_field`
   (`:2387`) peel covers the `on_change` FIELD path only, NOT these two inline wraps.

**Chosen fix layers:**
- **Lowerer** — replace SUM with per-arm MAX in *both* counters (seed correctness) AND change the
  `Match` arm of `rewrite_multiuse_clones` to snapshot/restore `remaining` per arm (sound
  consumption). This is the minimal in-boundary fix; it keeps the counter architecture and does
  not adopt the reference's set-membership model.
- **Emitter (scoped)** — route the two `ui_on_input_`/`ui_on_change_` inline wraps through the
  existing `emit_arc_callback_field` peel (the exact #191 mechanism), so a lowerer-hoisted
  capture-clone `let` is lifted OUTSIDE the synthesized `Arc::new(move …)`.

The redundant whole-body `hoist_pipeline_clones` pass from v1 is **dropped** (D4): its premise is
false. The one residual lowerer gap it might have covered (`region_ty(...) == None` at
`lower.rs:13065`) is analyzed in §2.4 and is out of #193's scope (a missing type model, not a
missing clone).

---

## 1. The 7 defects — resolution index

| # | Defect | Verified? | Resolution |
|---|---|---|---|
| D1 | v1 cited `collectFreeVarLocalsMulti` (:468) as MAXing arms (`bodyMax`) | **FALSE — verified** | §2.1 re-derives: `collectFreeVarLocalsMulti` SUMs (`Map.unionWith (+)`, :497/:499); the MAX fold lives in the SEPARATE `collectVarLocalsMulti` (:394, `branchMax`/`bodyMax` via `Map.unionWith max`, :435/:442). Correct citations throughout; note the prelude analog intentionally SUMs. |
| D2 | MAX seed on a shared sequential `remaining` is unsound | **CONFIRMED** | §2.2 — mechanism = **per-arm snapshot/restore** of `remaining`. Soundness proof against both once-A/twice-B and twice-A/once-B orderings. |
| D3 | MAX silently relaxes the fail-closed `reject_fn_value_reuse` gate | **CONFIRMED** | §2.3 — gate analysis + golden `i193_nonclone_fn_once_per_arm`. |
| D4 | The new whole-body pass is redundant (`lower_let_pvar` :13049 already covers) | **CONFIRMED** | §2.4 — pass DROPPED; residual `region_ty==None` gap named + scoped out. |
| D5 | `ui_on_input_`/`ui_on_change_` inline wraps genuinely uncovered | **CONFIRMED** | §2.5 — scoped emitter change routing both through `emit_arc_callback_field`; golden `i193_oninput_reused_capture`. |
| D6 | Idempotence golden inadequate (linear, not asymmetric-arm) | n/a | §3 — idempotence golden IS the D2 asymmetric-arm shape, two-pass byte-diff. |
| D7 | Test plan misses D2/D3/D5 shapes | n/a | §3 — adds `i193_asymmetric_arms_cloneok`, `i193_nonclone_fn_once_per_arm`, `i193_oninput_reused_capture`. |

---

## 2. Per-defect resolution — functions, file:line, logic

### 2.1 D1 — correct reference derivation

**v1's claim (FALSE):** "`collectFreeVarLocalsMulti` (ExprEmitter.hs:468) MAXes arm bodies
(`bodyMax`) and SUMs the scrutinee."

**Live code (verified `../sky/src/Sky/Generate/Rust/Builder/ExprEmitter.hs`):**

- **`collectFreeVarLocalsMulti` (:468)** uses `Map.unionWith (+)` — SUM — for BOTH branches:
  - `Can.Case` (:497): `foldl (\a (CaseBranch pat b) -> Map.unionWith (+) a (go bound' b)) (go bound scrut) branches` — SUM over arms.
  - `Can.If` (:499): `foldl (\a (c,t) -> Map.unionWith (+) a (Map.unionWith (+) (go bound c) (go bound t))) …` — SUM of cond AND then.
  There is **no `bodyMax`/`branchMax`** in this function.

- **`collectVarLocalsMulti` (:394)** is the one with the MAX fold:
  - `Can.Case` (:435-437): `branchMax = foldl (\a (CaseBranch _ b) -> Map.unionWith max a (go bound b)) Map.empty branches`, then `Map.unionWith (+) (go bound scrut) branchMax` — scrutinee SUM, arms MAX.
  - `Can.If` (:440-442): `condSum` via `Map.unionWith (+)`, `bodyMax` via `Map.unionWith max`, combined with `+`.

**Why the reference splits into two functions (this is load-bearing for D2):** they feed
DIFFERENT clone mechanisms.
- `collectFreeVarLocalsMulti` (SUM) drives the **prelude** in `defToRustString` (:3922, :3997):
  `multi = [ v | (v,c) <- Map.toList counts, c >= 2 ]` → `clonePreludeFor` emits one
  `let v = v.clone();` per multi-use *free* var at the top of the def body. The prelude is a
  **set** ("which vars are multi-use?"), so an over-count from SUM only ever changes membership
  from "not cloned" to "cloned" — it never mis-sequences, because the prelude clones once at the
  top and every subsequent read is a `.clone()` via `ecCloneVars`. SUM is a safe over-approximation
  *for a set-membership consumer*.
- `collectVarLocalsMulti` (MAX arms) drives the **use-site** `ecCloneVars` set (:799, :3953,
  :4020, and `argToRustString` :801) — again SET membership, but here a spurious member means a
  spurious `.clone()` on a move-only type (`SkyCmd`/`SkySub` are not `Clone` → `E0599`), so it
  MUST NOT over-count across mutually-exclusive arms → MAX.

**The key architectural fact (drives D2):** the reference's clone decision is **set-membership**
via `varLocalRead` (:782-787):

```haskell
varLocalRead ctx noCloneFn n
    | n `Set.member` ecThunkVars ctx = rustSafeIdent n ++ "()"
    | (not noCloneFn) && n `Set.member` ecCloneVars ctx
      && not (n `Set.member` ecCopyVars ctx)
      && not (n `Set.member` ecNoCloneVars ctx) = rustSafeIdent n ++ ".clone()"
    | otherwise = rustSafeIdent n
```

Every in-`ecCloneVars` use-site emits `.clone()`; the LAST use is kept bare by a SEPARATE
mechanism — inserting the symbol into `ecNoCloneVars` (e.g. :2301). There is **NO decrementing
counter**. That is precisely why the reference can MAX arm counts: MAX only decides *set
membership*; it never has to be "spent" left-to-right. Our fork does have a spent counter, which
is what makes a naive MAX-of-the-seed unsound — see D2.

> **Correction recorded:** every "MAXes arm bodies" claim about `collectFreeVarLocalsMulti` is
> struck. The prelude-driver analog (`collectFreeVarLocalsMulti`) intentionally SUMs because its
> consumer is a top-of-body prelude set, not a spent counter.

### 2.2 D2 — the load-bearing fix: per-arm SNAPSHOT, not MAX-on-shared-counter

**D2 mechanism = SNAPSHOT.**

#### 2.2.1 Why MAX-on-shared-counter is unsound (the refutation, confirmed against live code)

Our `rewrite_multiuse_clones` (`lower.rs:3164`) threads ONE `&mut remaining` DFS through the
whole expression, with an early-out (`lower.rs:3165-3167`):

```rust
fn rewrite_multiuse_clones(sym: Symbol, remaining: &mut usize, expr: Expr) -> Expr {
    if *remaining == 0 { return expr; }
    …
    Expr::Var(s) if s == sym => {
        if *remaining > 1 { *remaining -= 1; Expr::CloneVar(s) }   // clone, decrement
        else              { *remaining -= 1; Expr::Var(s) }        // last use: bare move
    }
```

The `Match` arm (`lower.rs:3281-3296`) walks arm bodies IN SEQUENCE via `map_bodies`, whose arm
callback is `FnMut` iterated left-to-right (`crates/sky_ir/src/ir.rs:2234-2256` — a
`.into_iter().map(|arm| { arm_map(...) })`, so the same `&mut remaining` is threaded arm-by-arm
in declaration order):

```rust
Expr::Match(m) => {
    let m = m.map_scrutinee(|s| rewrite_multiuse_clones(sym, remaining, s));
    Expr::Match(m.map_bodies(|s| s, |pat, body, guard| {
        let new_body = if pat_binds_symbol(pat, sym) { body }
                       else { rewrite_multiuse_clones(sym, remaining, body) };
        (new_body, guard)
    }))
}
```

**Counterexample (once-in-arm-A, twice-in-arm-B), MAX seed = `max(1,2) = 2`:**
1. Enter arm A. First `Var(sym)`: `remaining` 2 → 1, emitted `CloneVar` (SPURIOUS clone — arm A
   only uses it once, should be a bare move).
2. Enter arm B. First `Var(sym)`: `remaining` 1 → 0, emitted **bare `Var`** (WRONG — arm B has a
   second use coming, this one should clone).
3. Arm B second `Var(sym)`: `remaining == 0` → the early-out at `:3165` returns the sub-tree
   untouched → **bare `Var`**. Arm B now moves `sym` twice → **E0382** — the exact #193 class.

MAX is order-dependent: swap to twice-A/once-B and the failure moves to a different arm, but a
shared spent counter can never be correct for mutually-exclusive arms, because "spending" in one
arm robs another. **Verdict: MAX cannot apply to a shared sequential `remaining`.**

#### 2.2.2 The SNAPSHOT mechanism (chosen)

Change the `Match` (and `If`) arm of `rewrite_multiuse_clones` so each arm body is rewritten with
its OWN independent count, then `remaining` is RESTORED to what it was after the scrutinee, before
the next arm. Concretely:

```rust
Expr::Match(m) => {
    // Scrutinee is evaluated unconditionally → threads the shared counter.
    let m = m.map_scrutinee(|s| rewrite_multiuse_clones(sym, remaining, s));

    // Arms are mutually exclusive: exactly one runs at runtime, so each arm
    // body gets its OWN counter seeded from its OWN use count, and the shared
    // `remaining` is RESTORED between arms (snapshot/restore). This makes each
    // arm's last use a bare move regardless of sibling arms' counts.
    let after_scrut = *remaining;              // snapshot value AFTER scrutinee
    let m = m.map_bodies(|s| s, |pat, body, guard| {
        if pat_binds_symbol(pat, sym) {
            (body, guard)                      // sym shadowed in this arm
        } else {
            let mut arm_remaining = count_var_uses(sym, &body);   // per-arm seed
            let new_body = rewrite_multiuse_clones(sym, &mut arm_remaining, body);
            (new_body, guard)
        }
    });

    // The match as a whole consumes `sym` at most `MAX(arm uses)` times on any
    // taken path; the shared counter must reflect the WORST case so a use of
    // `sym` AFTER the match (a sibling in an enclosing Call/TaskSeq) is still
    // sequenced correctly. Restore to: after_scrut MINUS the peak arm consumption.
    let peak = match_arm_peak_uses(sym, &m);   // = MAX over arm bodies (0 for shadowing arms)
    *remaining = after_scrut.saturating_sub(peak);
    Expr::Match(m)
}
```

`If` gets the analogous treatment: `cond` threads the shared counter; `then_` and `else_` each get
a snapshot-restored per-branch counter; the shared counter is decremented by `max(then_uses,
else_uses)`.

Two supporting facts make this exact:
- **Per-arm seed** must be recomputed from the *rewritten-scrutinee-free* arm body via
  `count_var_uses(sym, &body)`. Because arm bodies never contain the scrutinee, this is just the
  arm's own use count — identical to what the match-arm pvar site at `:13304` already does for a
  DIFFERENT symbol class (it calls `count_var_uses(sym, &arm_body)` then a fresh
  `remaining = n`). We are generalizing that already-proven per-arm pattern to the outer symbol.
- **Post-match decrement** uses `peak = MAX(arm uses)` (a helper `match_arm_peak_uses` = the same
  MAX fold now added to the counters, §2.1's derivation) so that a `sym` used in a sibling
  spliced AFTER the match at the same IR level (e.g. `TaskSeq { effect: Match{…}, rest: …sym… }`)
  still sees a correct residual `remaining` and clones/moves correctly. The scrutinee already
  decremented the shared counter during its own DFS.

#### 2.2.3 Soundness proof — both orderings

Let `S = count_var_uses(sym, scrutinee)`, `A = uses in arm A`, `B = uses in arm B`, and suppose
there are `T` uses in a tail spliced after the match at the same level. Whole-body peak
consumption on any single runtime path = `S + max(A, B) + T` (exactly one arm runs). The
enclosing driver seeds `remaining = count_var_uses(sym, whole_body)`, which after the §2.1 MAX fix
is `S + max(A,B) + T`.

*Scrutinee phase.* DFS spends `S`: `remaining` goes `S+max(A,B)+T → max(A,B)+T`. Each scrutinee
occurrence but the whole-body-last one clones; correct because the scrutinee runs unconditionally.

*Arm phase (snapshot).* `after_scrut = max(A,B)+T`. Each arm body is rewritten with its OWN
`arm_remaining`:

- **Once-A / twice-B (A=1, B=2):**
  - Arm A: `arm_remaining = 1`. Its single `Var`: `remaining` 1 → 0 → **bare move**. ✅ (arm A
    uses `sym` once; bare move is correct.)
  - Arm B: `arm_remaining = 2`. First `Var`: 2 → 1 → **clone**. Second `Var`: 1 → 0 → **bare
    move**. ✅ (exactly one clone + one final move; no E0382.)
  - Shared counter restored to `after_scrut - max(1,2) = (2+T) - 2 = T`. ✅ (tail sees `T`.)

- **Twice-A / once-B (A=2, B=1):**
  - Arm A: `arm_remaining = 2`. First `Var`: clone; second `Var`: bare move. ✅
  - Arm B: `arm_remaining = 1`. Its single `Var`: bare move. ✅
  - Shared counter restored to `after_scrut - max(2,1) = (2+T) - 2 = T`. ✅

In BOTH orderings every arm's genuine last use is a bare move and every non-last use clones;
sibling arms never rob each other because each arm has its own counter; the tail `T` is sequenced
by the restored shared counter. No E0382, no spurious `.clone()`. ∎

*Why not just reset `remaining = after_scrut` before each arm (no per-arm seed)?* Because
`after_scrut = max(A,B)+T` can exceed an arm's own use count; the smaller arm would then treat its
last use as non-last (`remaining > 1`) and spuriously clone. The per-arm seed = the arm's own
count is what makes the last-use-per-arm a bare move. Snapshot/restore of the SHARED counter is
only for the tail `T`.

> **`map_bodies` semantics change.** This is a change to the *caller's use* of `map_bodies`, not
> to `map_bodies` itself: the callback still receives each `(pat, body, guard)` in order; we
> simply give each invocation its own local counter and adjust the shared one after the fold.
> `map_bodies`' shape-preservation contract (`crates/sky_ir/src/ir.rs:2234`, tested at
> `ir.rs:2402`) is untouched.

### 2.3 D3 — MAX must NOT relax the fail-closed seal gate

`reject_fn_value_reuse` (`lower.rs:3139-3148`) gates on `count_fn_value_uses(sym, body) > 1`:

```rust
fn reject_fn_value_reuse(sym, ir_ty, body, span) -> DResult<()> {
    if !ir_contains_fun(ir_ty) || !matches!(clone_class(ir_ty), CloneClass::NonClone) { return Ok(()); }
    if count_fn_value_uses(sym, body) > 1 { return Err(unsupported(span, Feature::FunctionValueReuse)); }
    Ok(())
}
```

**The hazard (confirmed):** if we change `count_fn_value_uses`'s `Match` arm SUM→MAX
(`lower.rs:3051`), a `NonClone` fn-value used exactly once per arm across ≥2 arms drops from
SUM=2 (rejected) to MAX=1 (accepted). Since #193 inserts NO clone for `NonClone` (a `Box<dyn Fn>`
/ `SkyTask` is not `Clone`), acceptance with no compensating codegen = the value is move-captured
into two mutually-exclusive arm closures. **That is actually sound at runtime** (only one arm
runs, so only one move happens) — BUT it is a *widening of seal acceptance* with no test proving
the emitted Rust builds, and it silently stops firing `Feature::FunctionValueReuse` for a shape
that previously errored. Under "make-invalid-states-unrepresentable", we must not widen acceptance
by accident.

**Resolution — keep the fn-value gate SUM; only the CloneOk value counter goes MAX+snapshot.**
The two counters serve different masters:
- `count_var_uses` (CloneOk clone insertion) → MAX arms + the §2.2 snapshot, because over-count
  causes a spurious `.clone()` / `E0599`.
- `count_fn_value_uses` (the `reject_fn_value_reuse` gate) → **stays SUM** at the `Match`/`If`
  arms. A `NonClone` fn-value reused across ≥2 arms must continue to be REJECTED
  (`Feature::FunctionValueReuse`) until a separate item designs sound Arc-promotion for the
  once-per-arm case. SUM here is the conservative fail-closed choice: it rejects strictly more,
  never emits an unsound move.

This is a deliberate asymmetry and is recorded as such. It also means the §2.2 snapshot change to
`rewrite_multiuse_clones`'s `Match` arm is only ever REACHED for a `CloneOk` symbol (the
`NonClone` fn-value never enters the rewrite — it is gated out first at every driver site), so the
snapshot logic and the fn-value gate never interact.

**Golden `i193_nonclone_fn_once_per_arm` (D7-a):** a `NonClone` fn-typed local used once per arm
across two `case` arms. Assert it STILL fails closed with `Feature::FunctionValueReuse` (an
`unsupported.rs`-style diagnostic assertion, NOT an E2E build). This pins that the fn-value gate
keeps SUM and did not silently relax. (If a future item makes once-per-arm fn-value reuse
genuinely build via Arc-promotion, this golden flips to an E2E build — but that is out of #193.)

### 2.4 D4 — drop the redundant whole-body pass; name the residual gap

**v1's premise (FALSE):** "the four driver sites miss a `let`-bound local whose only reuse is
inside a pipeline closure."

**Live code (`lower.rs:13048-13060`, verified):** `lower_let_pvar`'s `else` branch already runs

```rust
let n = count_var_uses(name, &acc);
if n > 1 {
    let mut remaining = n;
    rewrite_multiuse_clones(name, &mut remaining, acc)
} else { acc }
```

over the FULL let-body accumulator `acc`. `acc` is the entire rest-of-scope, including any
downstream `TaskSeq` / `Call` / `cmd_perform` closure. `rewrite_multiuse_clones` recurses into
`TaskSeq` (`:3386`), `Call` (`:3298`), `Apply` (`:3303`), and hoists at nested `Lambda`
(`:3187`). So a let-local reused only inside a pipeline closure IS already reached. **The
"driver-scope gap" does not exist.** The v1 `hoist_pipeline_clones` pass is **dropped in full** —
it would have been a second traversal duplicating the existing coverage, risking the double-fire
it then had to defend against.

**Residual gap (named, and scoped OUT of #193):** the `else { acc }` at `lower.rs:13065` fires
when `region_ty(b.body.span)` returns `None` (type not modelled → `ir_type_from_ty` errors →
`ty_opt = None`). In that case NO clone rewrite runs at all — a reused `CloneOk` local of a
not-yet-modelled type would double-move. This is a **completeness gap in the type model**
(`ir_type_from_ty` should map the type), NOT a clone-hoist gap; papering it with an
unconditional rewrite would clone on unknown types (potentially a move-only type we cannot see →
`E0599`), which is the opposite failure. It is filed separately (type-model completeness), and
the same fail-open-on-unknown-type posture is already the deliberate choice at the match-arm site
(`:13297-13300`, `&& let Ok(ir_ty) = …` guard). No repro is added under #193 because the fix is a
type-model change, not a lowering change.

### 2.5 D5 — the `ui_on_input_` / `ui_on_change_` emitter sites (scoped emitter change)

**Live code (verified `crates/sky_backend_rust/src/emit_expr.rs`):**

```rust
KernelFn::UiOnInput => {               // :5040
    let [f_e] = args else { … };
    let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;    // :5049
    Ok(Some(format!(
        "sky_runtime::ui::helpers::ui_on_input_(::std::sync::Arc::new(move |_x| ({f_s})(_x)))"  // :5051
    )))
}
KernelFn::UiOnChange => { … same shape, :5065 }
```

The outer `move |_x|` at `:5051`/`:5065` is EMITTER-SYNTHESIZED. When `f_e` is a `Lambda` that
reuses a non-Copy capture (and a sibling attribute — e.g. a button `onPress` — reuses the same
binding), the lowerer's pre-clone `let v = v.clone(); Lambda{…}` around `f_e` renders as
`({ let v = v.clone(); <inner-closure> })(_x)` — the clone `let` is INSIDE the synthesized
`Arc::new(move |_x| …)`, so the outer `move` still move-captures the FREE outer `v`, and the
sibling `onPress` hits use-after-move. **This is exactly the #191 bug shape**, and the #191 fix
(`emit_arc_callback_field`, `:2387`) was applied ONLY to the `on_change` FIELD path
(`:4393`/`:4456`/`:4505`/…), NOT to these two inline wraps.

**Resolution — scoped emitter change (design v2 now permits it).** Route the `ui_on_input_` and
`ui_on_change_` inline wraps through `emit_arc_callback_field`, which already peels leading
pure-alias `let n = Var(v)` / `let n = CloneVar(v)` bindings OUTSIDE the `Arc::new(move …)`:

```rust
KernelFn::UiOnInput => {
    let [f_e] = args else { … };
    // #193/#191 parity: peel any lowerer-hoisted capture-clone `let`s OUTSIDE
    // the synthesized Arc's move closure, exactly as the on_change FIELD path
    // already does. emit_arc_callback_field emits `{ let v = v.clone(); Arc::new(move |_x| (INNER)(_x)) }`.
    Ok(Some(emit_ui_input_callback(ctx, f_e, indent, child, generics)?))
}
```

where `emit_ui_input_callback` is a thin wrapper mirroring `emit_arc_callback_field` but wrapping
the peeled inner in `ui_on_input_(Arc::new(move |_x| (INNER)(_x)))` instead of the bare
`arc_callback_wrap`. Two options, pick the cleaner at implementation:
  - **(preferred)** generalize `emit_arc_callback_field` to take the "how to wrap the inner" as a
    small enum/closure param (`ArcWrap::CallbackField` vs `ArcWrap::UiEventArg1`), so the peel
    logic is written once; or
  - add a sibling `emit_arc_ui_event_field` that shares the peel loop.

Either keeps the peel logic single-sourced (Principle 6) and does NOT introduce a second
clone-insertion authority: the LOWERER still decides *which* uses clone (its pre-clone `let`); the
emitter only relocates that already-decided `let` outward past the synthesized move — a pure text
hoist of a pure-alias `let`, semantics-preserving exactly as #191 established.

This is the ONLY emitter change #193 makes. `emit_lambda_unboxed` / `CloneVar` rendering
(`emit_expr.rs`) stay dumb.

**Golden `i193_oninput_reused_capture` (D7-c):** a non-Copy record used in a `Ui.onInput` closure
AND a sibling `Ui.onClick`/button `onPress` in the same view. Pre-fix: skyc-0 → cargo `E0382`.
Must skyc-0 → cargo-0 → run-correct. This is the `Ui.onInput` analog of the existing
`i191_input_arc_capture` (which covers the `onChange` FIELD path).

---

## 3. Test plan (D6 + D7)

All goldens live under `tests/golden/iNNN_slug/Main.sky`, driven by the shared harness
(`crates/skyc/tests/support/mod.rs`, `assert_emitted_project_matches_golden_dir` +
`RunOutcome`), with a `crates/skyc/tests/golden_iNNN_*.rs` driver. Precedent for the `.clone()`
snapshot assertions: `golden_i142_access_copy_elision.rs:91` already asserts
`!emitted.contains(".count.clone()")`.

### 3.1 New #193 goldens

- **`i193_asymmetric_arms_cloneok` (D7-a, THE D2 shape).** A `CloneOk` local (`String`) used
  **once in arm A, twice in arm B** of a `case` (asymmetric — the ordering-sensitive shape MAX
  would break). Assertions:
  1. skyc-0 → cargo-0 → run-correct (no `E0382`).
  2. **`.clone()`-count snapshot** on the emitted `main.rs`: arm A's occurrence is a bare move
     (no `.clone()` on that read); arm B has exactly ONE `.clone()` (its first read) and one bare
     final read. A SUM/MAX-on-shared-counter regression re-introduces a spurious clone in arm A
     or an E0382 in arm B, both caught here even where cargo might still build.
  Also include the twice-A/once-B mirror in the same golden (two match expressions) so both
  orderings are pinned.

- **`i193_nonclone_fn_once_per_arm` (D7-b, D3).** A `NonClone` fn-typed local used once per arm
  across two `case` arms. Assert (via the `unsupported.rs` diagnostic-assertion style) that
  lowering STILL fails closed with `Feature::FunctionValueReuse`. Pins that
  `count_fn_value_uses`'s `Match` arm keeps SUM and the seal gate did not relax (§2.3).

- **`i193_oninput_reused_capture` (D7-c, D5).** A non-Copy record captured into a `Ui.onInput`
  closure AND reused by a sibling `onClick`/`onPress` in the same view. Pre-fix: skyc-0 → cargo
  `E0382`. Must skyc-0 → cargo-0 → run-correct. The `Ui.onInput` analog of
  `i191_input_arc_capture`.

- **`i193_taskseq_reuse`.** A `CloneOk` local (`String`) bound once, used inside a `task_and_then`
  continuation AND again after it. Exercises `lower_let_pvar` :13054 reaching the `TaskSeq`
  recursion (`:3386`). Pre-fix: `E0382`. (Note: with D4's finding this may already build on HEAD;
  if so it is a *lock*, not a fix — the golden documents that the pipeline shape is covered.)

- **`i193_cmd_perform_reuse`.** A reused `String`/record captured into a `cmd_perform` `to_msg`
  lambda and read again in the same `update` arm. Exercises the `Call`/`Apply` recursion.

- **`i193_idempotent` (D6 — MUST be the asymmetric-arm shape).** Reuse the
  `i193_asymmetric_arms_cloneok` body (a `CloneOk` local used once-A/twice-B). Run the lowering
  pipeline once, snapshot the emitted `main.rs`; run it TWICE (a second `rewrite_multiuse_clones`
  over the already-rewritten body from the driver's re-entry), assert **byte-identical** output.
  Idempotence holds because `CloneVar(s)` counts as a use (`:2474`) and re-seeds the same per-arm
  counts, and the snapshot/restore is a pure function of the (unchanged) arm use counts. A linear
  or single-param shape (v1's mistake) cannot exercise the per-arm snapshot's fixpoint — the
  asymmetric-arm shape is required.

### 3.2 Regression goldens that MUST stay green (no behavioral change)

`i104_seal`, `i130_seal`, `i142_access_copy_elision` (Copy fields still NOT cloned),
`i164_poly_task_on_error_nested`, `i177_db_get_*` (db-get false-positive clones),
`i186_display_*`, `i191_input_arc_capture` (#191 on_change FIELD path unchanged),
`l0105_alias_move_seal`, `l0114_server_handler_arc`, `m5a_task*`, `m7_stdui_oninput_closure`
(the existing onInput closure lock — must stay green after the §2.5 emitter reroute),
`mixed_arm_task_run_elision_seal`, `tui_entry_case_seal`, `tco`. Plus the whole
`cargo test -p sky_lower` + `-p sky_backend_rust` + `-p skyc` golden suites: zero new failures.

### 3.3 Example sweep

`scripts/examples-sweep.sh` must stay VERDICT PASS. Phase-1 default:
`SKY_SWEEP_NO_EQUIV=1 bash scripts/examples-sweep.sh`. Special attention to Task/CLI/TUI shapes
(`20-cli-counter`, `23-tui-todo`) and any Sky.Live example wiring `Ui.onInput`/`Ui.onChange`
with a reused capture (the §2.5 reroute's blast radius).

---

## 4. What NOT to touch

- **`count_fn_value_uses` `Match`/`If` arms — stay SUM (§2.3, D3).** Only `count_var_uses` (the
  CloneOk counter) goes MAX. Widening the fn-value gate would relax the fail-closed seal.
- **`emit_arc_callback_field` (`emit_expr.rs:2387`) — the #191 fix.** §2.5 *reuses* it (extends
  it once for the UI-event-arg wrap shape, or adds a peel-sharing sibling); it does not rewrite
  the on_change FIELD path or its `on_change` call sites (`:4393` etc).
- **`reject_fn_value_reuse` (`lower.rs:3139`) and all `CloneClass::NonClone` gating**
  (`:5964`, `:6125`, `:13061`, `:13314`). Keep fail-closed. #193 is strictly `CloneOk` reuse.
- **`rewrite_captured_clones` (`lower.rs:13023` region) / decoder-thunk path / #164 `SharedLambda`
  promotion (`lower.rs:1852`-`:1907` Send+Sync machinery).** Orthogonal `!Clone` capture
  mechanisms.
- **`emit_lambda_unboxed` / `CloneVar` rendering.** Stay dumb — the lowerer remains the single
  clone-decision authority; the emitter only relocates an already-decided pure-alias `let`.
- **`region_ty == None` residual gap (`lower.rs:13065`).** Out of #193 scope (type-model
  completeness, §2.4). Do NOT paper it with an unconditional rewrite.

---

## 5. Coordination note — #184 / #195 (`SharedLambda` / Arc `+Send+Sync`)

`rewrite_multiuse_clones`'s `SharedLambda` arm (`lower.rs:3210`) hoists a pre-clone `Let{value:
CloneVar}` through the SAME node kind that #184/#195 govern. #184/#195 own the context-sensitive
`Arc<dyn Fn + Send + Sync>` promotion (`lower.rs:1852-1907`, `requires_sync_capture`). Two
interactions to re-validate when #195 lands:

1. **#193's §2.2 snapshot change** touches only the `Match`/`If` arm of `rewrite_multiuse_clones`;
   the `SharedLambda` arm itself is unchanged. But #193's `count_var_uses` MAX change alters the
   `remaining` seed that reaches the `SharedLambda` arm inside a match. If #195 changes when a
   lambda is promoted to `SharedLambda` (context-sensitive Send/Sync), a symbol may now be
   captured by a `SharedLambda` where it was a plain `Lambda` before — the per-arm snapshot must
   be re-checked against the new promotion sites (both arms of §2.2's proof still hold: they are
   agnostic to Lambda-vs-SharedLambda, since both consume exactly one `remaining` slot when they
   ref `sym`).

2. **§2.5's emitter reroute** for `ui_on_input_`/`ui_on_change_` wraps a callback whose runtime
   consumer requires `Send + Sync` (`Ui.onInput`/`Ui.onChange` are in the `requires_sync_capture`
   set, cf. `lower.rs:13083`). If #195's context-sensitive change alters the Send/Sync bound on
   these UI-event Arc callbacks, the peel-hoist must be re-validated (the peeled `let v =
   v.clone()` must still satisfy whatever bound the inner `Arc<dyn Fn + Send + Sync>` demands —
   it does today because the clone is of a `CloneOk` value that is already `Send + Sync`).

**Action:** whoever lands #195 re-runs the §3 goldens (esp. `i193_asymmetric_arms_cloneok`,
`i193_oninput_reused_capture`, `m7_stdui_oninput_closure`) and confirms the §2.2 proof against the
new `SharedLambda` promotion boundary. Filed as a cross-reference on #184/#195.

---

## 6. Divergence note (record in `docs/divergences-from-sky.md`)

We hoist clones in the LOWERER (`rewrite_multiuse_clones` + IR `CloneVar`) where the reference
hoists in the EMITTER (`clonePreludeFor` prelude + `ecCloneVars` use-site set). **Deeper
divergence than v1 stated:** the reference's clone decision is stateless SET-MEMBERSHIP
(`varLocalRead`: clone iff `∈ ecCloneVars ∧ ∉ ecNoCloneVars`), which is why it can MAX arm counts
freely; our fork uses a SPENT DECREMENTING counter, which is why mutually-exclusive arms require
per-arm SNAPSHOT/restore rather than a MAXed shared seed. Same observable Rust; different layer
AND different consumption model. Rationale: single ownership authority in the lowerer + reuse of
an already-correct IR traversal. The one emitter concession (§2.5) relocates a
lowerer-decided pure-alias `let`, not a second clone decision. Already partially recorded
(T5/#104/#112/#191); extend the entry to note #193's snapshot mechanism and the SUM(fn-value) vs
MAX(CloneOk) counter asymmetry.

---

## 7. Risk / blast-radius

Ordered by severity:

1. **Per-arm snapshot under-restores the shared counter → `E0382` on a post-match tail.** If
   `peak = MAX(arm uses)` under-counts the residual owed to a tail sibling, the tail's use is
   mis-sequenced. Mitigation: §2.2.3 proves the restore is `after_scrut - max(arm uses)` and the
   tail `T` was folded into the seed by the MAX-corrected `count_var_uses`. `i193_taskseq_reuse` +
   a match-with-tail golden guard it. **Any sweep red here = revert to SUM+no-snapshot and
   bisect, do not widen.**
2. **`count_fn_value_uses` accidentally MAXed → seal relaxes (D3).** Guarded by keeping it SUM
   (§2.3) + `i193_nonclone_fn_once_per_arm`. Do NOT touch that counter's arm folds.
3. **§2.5 emitter reroute regresses the on_change FIELD path or the plain onInput case.** The
   reroute must be byte-identical when there are NO leading pure-alias `let`s (the
   `emit_arc_callback_field` `hoisted.is_empty()` fast-path already guarantees this). Guarded by
   `m7_stdui_oninput_closure` (must stay green) + `i191_input_arc_capture`.
4. **Ordering vs TCO.** All lowerer changes stay at the existing driver sites (which already run
   BEFORE `analyze_tail_recursion`, `lower.rs:5952` "Run BEFORE TCO"); the snapshot is internal to
   `rewrite_multiuse_clones` and does not move relative to TCO. `tui_entry_case_seal`,
   `i111_cli_program_seal` guard it.
5. **Blast radius bounded to `lower.rs` (counters + one `rewrite_multiuse_clones` arm) +
   `emit_expr.rs` (two UI-event kernel arms routed through an existing helper).** No runtime, no
   scheme/kernel table. Any regression fails loudly at `cargo build` in a golden, never as a
   silent runtime divergence.

**Guardian ladder:** if the snapshot + MAX(CloneOk-only) cannot be made green across the golden
suite + sweep without a `Feature::FunctionValueReuse` newly firing on a shape that previously
built, that is "a principle is hurt → rethink within boundary"; if no in-boundary fix keeps BOTH
the E0382 class closed AND the over-clone class closed AND the seal gate un-relaxed, revert and
escalate (never trade one seal hole for another).

---

## Appendix A — reference ↔ fork mechanism map (corrected)

| Concern | Reference (`ExprEmitter.hs`) | Our fork (`lower.rs`) |
|---|---|---|
| Clone decision model | **set-membership** (`ecCloneVars`, `varLocalRead` :782) — no spent counter | **spent decrementing counter** (`rewrite_multiuse_clones` `remaining` :3164) |
| Prelude free-var multi-use | `collectFreeVarLocalsMulti` :468 — **arms SUM** (:497/:499); safe because prelude is a set | (no prelude; driver seeds the counter) |
| Use-site multi-use | `collectVarLocalsMulti` :394 — **arms MAX** (:435/:442), scrutinee SUM | `count_var_uses` :2472 — **arms SUM today (the bug)** → MAX per §2.1 |
| Mutually-exclusive arms | MAX (set membership; never spent) | MAX seed + **per-arm snapshot/restore** of the spent counter (§2.2) |
| Last use = move | `ecNoCloneVars` skip (separate set) | `remaining == 1` → bare (`:3172`) |
| Hoist site | prelude `let v=v.clone();` before closure text | IR `Let{value:CloneVar}` wrapping the `Lambda`/`SharedLambda` (`:3193`/`:3216`) |
| `!Clone` fn-value | `ecNoCloneVars` (move once) | `CloneClass::NonClone` + `reject_fn_value_reuse` fail-closed; counter stays **SUM** (§2.3) |
| UI-event Arc callback | prelude reaches it (same body) | lowerer pre-clone + **emitter peel** (`emit_arc_callback_field`, extended §2.5) |

## Appendix B — the four existing driver sites (all invoke `rewrite_multiuse_clones`)

| Site | file:line | Binder kind | Counter used |
|---|---|---|---|
| Typed fn params | `lower.rs:5959` | function parameter | `count_var_uses` |
| Untyped fn params | `lower.rs:6121` | function parameter | `count_var_uses` |
| `let`-body accumulator (full `acc`) | `lower.rs:13054` (`lower_let_pvar`) | `let` value; covers downstream pipeline closures (D4) | `count_var_uses` |
| Match-arm pattern vars | `lower.rs:13304` | `case … of` pattern binder; already per-arm `remaining` | `count_var_uses` |

No new driver site is added (v1's `hoist_pipeline_clones` dropped, D4). The fix is: (i) MAX the
`count_var_uses` arm folds so every driver's seed is correct; (ii) snapshot/restore per arm inside
`rewrite_multiuse_clones` so the spent counter is not robbed across mutually-exclusive arms;
(iii) route the two UI-event inline wraps through the existing emitter peel (D5).

# #193 — General clone-hoist for reused non-`Copy` bindings across the Task/CLI pipeline

**Status:** design (no code). READ-ONLY recon verified against HEAD `f179255`.
**Backlog item:** #193 — reused non-`Copy` bindings captured into `move` closures across the
Task/CLI pipeline double-move → cargo `E0507`/`E0382`.
**Scope:** `crates/sky_lower/src/lower.rs` only (plus new goldens under `tests/golden/`). No
`crates/sky_backend_rust/**` change. No runtime change.
**Reference:** `../sky/src/Sky/Generate/Rust/Builder/ExprEmitter.hs`
(`collectFreeVarLocalsMulti` :468, `clonePreludeFor` :764, `ecNoCloneVars`, `varLocalRead` :798).

---

## 0. TL;DR

The clone-insertion machinery is already correct and complete. `rewrite_multiuse_clones`
(`lower.rs:3164`) fully recurses through every `Expr` shape — including `TaskSeq`
(`lower.rs:3388`), `TaskSeqSync` (`:3392`), `Call` (`:3299`), `Apply` (`:3308`), `Ctor`
(`:3373`), `Record` (`:3340`) — and its `Lambda` arm (`:3187`) already performs the
**hoist-outside-the-move-closure** rewrite gotcha #5 demands:
`Let { name: sym, value: CloneVar(sym), body: Lambda }`.

The bug is **not** in the rewrite. It is in **who invokes it and how many uses it counts**:

1. **Driver-scope gap.** The rewrite is invoked only from four binder-anchored sites — fn
   params (`:5955`, `:6117`), the `let`-body accumulator (`lower_let_pvar` `:13045`), and
   match-arm pattern vars (`:13292`). A binding whose *only* reuse lives inside a
   `task_and_then`/`task_map`/`cmd_perform`/UI-handler `move`-closure continuation is still
   reached by the recursion **once an enclosing binder fires** — but a reused binding that is
   itself introduced by one of those closures' own scoping, or counted wrong, never triggers a
   rewrite. The real observed failures are the counting bug below manifesting through these
   pipeline shapes.

2. **`Match` arms SUM instead of MAX (gotcha #1 — load-bearing).** `count_var_uses`
   (`:2506-2519`) and `count_fn_value_uses` (`:3053-3066`) both `.sum()` the per-arm use
   counts. Mutually-exclusive arms must be **MAX**ed (the scrutinee is the only SUM). SUM
   over-counts, so `remaining` starts too high, and the *last real* use in the taken arm is
   rewritten to a `.clone()` instead of staying a bare move. On a `CloneOk` value that is a
   spurious-but-harmless extra clone (efficiency regression); on a value the type marks
   `CloneOk` but whose Rust type does not actually implement `Clone` in that position, or a
   move-only value that slipped the class, it is `E0599`/`E0382`.

**Chosen fix layer: the LOWERER.** Generalize the driver + fix the arm counting to MAX. This
keeps the architecture intact (emitter stays dumb: `CloneVar(sym)` → `sym.clone()` at
`emit_expr.rs:5748`), matches our fork's design decision to hoist in the IR, and reuses the
already-correct `rewrite_multiuse_clones` traversal wholesale.

---

## 1. Chosen layer + mechanism, with justification

### 1.1 The two candidate layers

**(A) Lowerer-driver generalization (CHOSEN).** Fix `count_var_uses`/`count_fn_value_uses` to
MAX arms, and add ONE whole-body clone-hoist pass at the end of `lower_def`
(`lower.rs:5694`) that runs `rewrite_multiuse_clones` over every reused `CloneOk`
local/param regardless of which binder introduced it, so the pipeline-closure shapes are
covered by construction.

**(B) Emitter-side prelude (REJECTED).** Port the reference's `collectFreeVarLocalsMulti` +
`clonePreludeFor` into `emit_lambda_unboxed`/`emit_arc_callback_field`/`TaskSeq` so each
emitted `move` closure grows a `let v = v.clone();` prelude at emit time.

### 1.2 Why (A), tied to our architecture

- **Our emitter is intentionally dumb about ownership.** `emit_lambda_unboxed`
  (`emit_expr.rs:7689`) emits `move |..| { body }` with **no** clone logic; `emit_shared_lambda`
  (`:7800`) routes through the same helper; `CloneVar(sym)` renders uniformly at
  `emit_expr.rs:5748`. The reference hoists in the emitter because its emitter *is* the
  ownership authority (`ecCloneVars`, `ecNoCloneVars`, `varLocalRead`). Our fork moved that
  authority into the lowerer deliberately (`CloneClass`, `rewrite_multiuse_clones`,
  `reject_fn_value_reuse`). Adding an emitter prelude would create **two** competing
  clone-insertion authorities that must agree on the exact count/last-use/`!Clone` decisions —
  a soundness hazard (double-clone, or clone-then-reject disagreement) and a readability
  regression (Principle 3 > Principle 6).
- **The traversal already exists and is correct.** `rewrite_multiuse_clones` recurses through
  `TaskSeq`/`Call`/`Ctor`/`Apply` and hoists at `Lambda`/`SharedLambda`. Reusing it means the
  fix is *scope widening + a counting correction*, not a new pass with its own bugs.
- **`CloneClass` is already the single `!Clone` gate.** The lowerer knows
  `NonClone`/`CopyLeaf`/`CloneOk` (`clone_class` `:764`). An emitter prelude would have to
  re-derive that from `IrType` at each site (duplicating gotchas #4/#7). Keeping it in the
  lowerer keeps one gate.
- **The seal.** A lowerer that inserts the exact clones needed and rejects the unsound reuse
  (`reject_fn_value_reuse`) *before* text emission is the "make-invalid-states-unrepresentable"
  posture the seal wants: skyc accepts ⇒ the emitted Rust owns/clones correctly ⇒ `cargo`
  builds. An emitter prelude that *usually* matches the lowerer is a representable-but-illegal
  divergence.

**Divergence note (record in `docs/divergences-from-sky.md`):** we hoist in the lowerer
(`rewrite_multiuse_clones` + IR `CloneVar`) where the reference hoists in the emitter
(`clonePreludeFor`). Same observable Rust; different layer. Rationale: single ownership
authority + reuse of an already-correct IR traversal. Already partially recorded (T5/#104/#112);
extend the entry to note #193 generalizes it.

---

## 2. Exact functions to change (names + file:line) and new logic

### 2.1 `count_var_uses` — SUM→MAX for match arms (`lower.rs:2472`, arm at `:2506-2519`)

Current (`:2506`):
```rust
Expr::Match(m) => {
    let in_scrut = count_var_uses(sym, m.scrutinee());
    let in_arms: usize = m.arms().iter()
        .map(|arm| if pat_binds_symbol(&arm.pat, sym) { 0 }
                   else { count_var_uses(sym, &arm.body) })
        .sum();                                   // ← BUG: SUM over arms
    in_scrut + in_arms
}
```
New:
```rust
Expr::Match(m) => {
    let in_scrut = count_var_uses(sym, m.scrutinee());
    // Arms are mutually exclusive: at runtime exactly ONE body executes, so
    // the peak consuming-use count of `sym` across the whole match is the
    // scrutinee's SUM contribution plus the MAX over arm bodies — never the
    // sum of arms (which would over-count and clone a real last use). Mirrors
    // the reference `bodyMax` fold (ExprEmitter.hs collectFreeVarLocalsMulti
    // If/Case: `Map.unionWith max` over branch bodies, `Map.unionWith (+)` for
    // the scrutinee/conditions).
    let in_arms = m.arms().iter()
        .map(|arm| if pat_binds_symbol(&arm.pat, sym) { 0 }
                   else { count_var_uses(sym, &arm.body) })
        .max().unwrap_or(0);                      // ← MAX over arms
    in_scrut + in_arms
}
```
Also fix the **`If`** arm (`:2503`) analogously: the scrutinee/conditions SUM, the
then/else branch bodies MAX. Reference does exactly this (`If` in
`collectFreeVarLocalsMulti`: `condSum` via `+`, `bodyMax` via `max`, combined with `+`). Our
current `If` arm SUMs `cond + then_ + else_`. Correct shape:
`count(cond) + max(count(then_), count(else_))`.

> **Interaction with `rewrite_multiuse_clones`.** The rewrite threads a single `&mut remaining`
> counter DFS through the match (`map_scrutinee` then `map_bodies`, `:3283`). With MAX-derived
> `remaining`, the counter must not be decremented independently per arm in a way that strands
> a taken arm short. Verify (test, not just reason): the rewrite decrements per *syntactic*
> occurrence as it walks each arm body, so a `remaining` seeded from `scrutinee_sum + arm_max`
> lets each arm's occurrences each see `remaining >= 1` at their own last use (because no arm
> body has more than `arm_max` occurrences). This is the same invariant the reference relies
> on. A golden (`i193_move_only_cmd_across_arms`) pins it.

### 2.2 `count_fn_value_uses` — SUM→MAX for match arms (`lower.rs:3018`, arm at `:3053-3066`)

Identical MAX correction to the `Expr::Match` arm, and the same `If` correction
(`:3049-3052`). The `Apply` arm delegates to `count_fn_value_uses_apply` (`:3114`) — leave
that unchanged (its direct-callee-borrow exemption is orthogonal and correct).

### 2.3 New whole-body driver in `lower_def` (`lower.rs:5694`)

Add ONE clone-hoist pass at the end of `lower_def`, AFTER the existing per-param T5 loop
(`:5953`/`:6115`) and BEFORE TCO (`analyze_tail_recursion`/`rewrite_tail_calls` at
`:5970`+). This pass covers reused **let-bound** locals whose only reuse is inside a
pipeline closure — the shape the four existing binder-anchored sites miss.

New helper (private fn in `lower.rs`):
```rust
/// #193: after lowering a def body, hoist clones for every CloneOk LOCAL that
/// is reused (count > 1) *and* whose reuse crosses a `move`-closure boundary
/// introduced by a pipeline combinator (task_and_then / task_map / cmd_perform
/// / UI handler / eta-lambda). The per-param loops (:5955/:6117) already cover
/// params; lower_let_pvar (:13045) covers `let` *values* at their binding site;
/// this covers `let`-bound names whose reuse is only reachable through a nested
/// closure the binding site's own count didn't see as multi-use.
///
/// Mechanism: for each such symbol, `count_var_uses` (now MAX-correct) gives the
/// true peak use count; if > 1, run `rewrite_multiuse_clones`, which recurses
/// into TaskSeq/Call/Ctor/Apply and, at each Lambda/SharedLambda that captures
/// the symbol non-last, emits `Let{name:sym, value:CloneVar(sym), body:Lambda}`
/// OUTSIDE the move closure (gotcha #5).
fn hoist_pipeline_clones(&self, body: Expr) -> DResult<Expr> { … }
```
Body logic:
1. Collect every `let`-bound / destructure-bound local `sym` in `body` together with its
   solved `IrType` (via `region_ty(binding_span)` → `ir_type_from_ty`), **excluding** every
   binder we must not clone-prelude (gotcha #2): lambda params, let/letrec names at their own
   scope, let-destruct pattern vars, tail-loop params, match-arm pattern vars. Reuse the
   binder-exclusion the existing counting already encodes (the `if *name == sym { 0 }` /
   `pat_binds_symbol` guards), so no symbol gets a clone at a scope where it is not live
   (`E0425`).
2. For each candidate `sym` with `clone_class(ir_ty) == CloneOk` and
   `count_var_uses(sym, &body) > 1`: `let mut remaining = n; body =
   rewrite_multiuse_clones(sym, &mut remaining, body);`.
3. For `NonClone` (`ir_contains_fun`) reused symbols: DO NOT clone. Delegate to the existing
   `reject_fn_value_reuse` (gotcha #7) — fail closed with `Feature::FunctionValueReuse`
   exactly as the four existing sites already do. `CopyLeaf` → skip (never needs `.clone()`).
4. Idempotence: the pass must be a no-op when the per-param/`lower_let_pvar` sites already
   rewrote a symbol. `rewrite_multiuse_clones`'s `CloneVar(s) if s == sym` arm (`:3180`) is a
   fixpoint (a `CloneVar` stays `CloneVar`), and re-counting a body that already has the right
   clones yields the same `remaining` seed, so a second traversal makes no change. **This must
   be a golden-verified invariant**, not merely asserted (run the pass, snapshot; run twice,
   diff-empty).

> **Why a separate pass rather than descending the four existing sites deeper?** The four sites
> are keyed to specific binder kinds and run mid-lowering with per-binder `IrType` in hand. A
> single post-lowering sweep over the finished `body` is simpler, has one place to reason about
> the MAX-count interaction, and cannot double-fire with the per-param loop (idempotence above).
> It is the minimal generalization.

### 2.4 How the pre-clone `Let` reaches each vulnerable closure shape

No emitter change is needed. Once `hoist_pipeline_clones` (or an enclosing binder site) invokes
`rewrite_multiuse_clones` on a body containing the shape, the existing recursion carries it:

| Emitter closure shape | IR shape traversed by `rewrite_multiuse_clones` | Arm (file:line) |
|---|---|---|
| `task_and_then(effect, move|_| { rest })` (`emit_expr.rs:6038`) | `Expr::TaskSeq { effect, rest }` | `:3388` recurses into both; `rest`'s captured `Lambda`s hoist at `:3187` |
| `{ let _ = task_run(effect); rest }` (`:6076`) | `Expr::TaskSeqSync` | `:3392` |
| `task_map`/Http conv (`:1017`) | lowers to `Call`/`Apply` around a `Lambda` | `:3299`/`:3308` → `:3187` |
| `cmd_perform(task, f)` (`:2056`) | `Expr::Call`/`Apply` with `f: Lambda` | `:3299`/`:3308` → `:3187` |
| `emit_lambda` general sink (`:7743`) | the `Lambda`/`SharedLambda` itself | `:3187`/`:3208` |
| `ui_on_input_(Arc::new(move|_x| (f)(_x)))` (`:5039`) | `f` is a `Lambda` inside a `Ctor`/`Call` arg | `:3373`/`:3299` → `:3187` |
| `emit_apply` eta-lambda (`:7554`) | `Expr::Apply { func, args }` | `:3308` |
| middleware (`:2199`) | `Call`/`Apply` around a `Lambda` | `:3299`/`:3308` |

The `Lambda` arm's `Let{name:sym, value:CloneVar(sym), body:Lambda}` (`:3193`) lands the
`let v = v.clone();` **outside** the `move` closure it wraps — which is exactly where
`emit_lambda_unboxed` will render it (it emits the surrounding `Let` before the `move |..|`),
satisfying gotcha #5.

---

## 3. The 7 gotchas — specific guard for each

1. **Case-arm MAX-not-SUM.** §2.1 + §2.2 change both counters' `Expr::Match` and `Expr::If`
   arms from `.sum()` to scrutinee-SUM + body-MAX, mirroring the reference `bodyMax`
   (`ExprEmitter.hs` `collectFreeVarLocalsMulti`). This is the load-bearing fix; a golden
   (`i193_move_only_cmd_across_arms`) proves no over-clone on a move-only `SkyCmd`.
2. **Binder exclusion.** §2.3 step 1 excludes lambda params, let/letrec/destruct names, match-arm
   pvars, tail-loop params before counting/hoisting — reusing the exact `if *name == sym`/
   `pat_binds_symbol` guards already in `count_var_uses` (`:2481`,`:2497`,`:2508`) and the
   rewrite (`:3253`,`:3268`,`:3283`). No `let v = v.clone()` at a scope where `v` is out of
   scope (`E0425`). Mirrors reference `collectFreeVarLocalsMulti`'s `bound` set (:468 doc).
3. **Scrutinee SUMS.** §2.1/§2.2 keep `in_scrut = count(scrutinee)` added with `+`, only the
   arm *bodies* switch to MAX. The reference `Map.unionWith (+) condSum bodyMax` is the exact
   shape.
4. **Single-/last-use = MOVE not clone.** Unchanged and preserved: `rewrite_multiuse_clones`
   keeps the final occurrence bare (`remaining == 1` → `Expr::Var`, `:3172`). With the MAX fix,
   `remaining` is now *correct*, so the genuine last use in the taken arm is not clobbered into a
   `.clone()`. This is exactly what mends the `E0599` on a move-only value.
5. **Hoist lands OUTSIDE the `move` closure.** Guaranteed by the `Lambda`/`SharedLambda` arms
   (`:3187`/`:3208`): the pre-clone `Let` wraps the closure, so `emit_lambda_unboxed` renders
   `let v = v.clone(); move |..| { … v … }`. This is the same construction the #191 fix used;
   we do not re-implement it.
6. **Only pure-alias lets are safe to peel; general case builds fresh `let v = v.clone()`.** We
   never peel arbitrary lets. The general hoist is the IR `Let{value:CloneVar}` — a *fresh*
   clone binding, not a peel. The pure-alias peel is a separate optimization living only in
   `emit_arc_callback_field` (`emit_expr.rs:2387`) which we DO NOT touch (§4).
7. **Non-`Clone` Task reuse needs Arc/re-thunk, not `.clone()`.** §2.3 step 3 keeps
   `CloneClass::NonClone` OFF the clone path and routes to `reject_fn_value_reuse` (fail-closed),
   exactly as the four existing sites do (`:5961`,`:6123`,`:13058`,`:13313`). A `Pin<Box<dyn
   Future>>` (`SkyTask`) is never `.clone()`d (`task.clone()` = `E0599`). The Arc-wrap/re-thunk
   for legitimately-shared non-Clone bindings stays owned by the decoder-thunk path
   (`lower_let_pvar` `:13024` `rewrite_captured_clones`) and the #164 `SharedLambda` promotion —
   this design adds nothing to those and does not weaken their fail-closed posture.

---

## 4. What NOT to touch

- **`emit_arc_callback_field` (`emit_expr.rs:2387`) — the #191 fix. Leave intact.** It correctly
  peels leading pure-alias lets OUTSIDE `Arc::new`. The general lowerer hoist does not conflict:
  it inserts `Let{value:CloneVar}` in the IR; if that `Let` is a pure alias, `emit_arc_callback_field`
  peels it further out — additive, still correct. Do not fold its logic into the lowerer.
- **`reject_fn_value_reuse` (`lower.rs:3139`) and all `CloneClass::NonClone` gating
  (`:5961`,`:6123`,`:13058`,`:13313`,`:3139`). Keep fail-closed.** #193 is strictly about
  `CloneOk` reuse; `NonClone` reuse must continue to error (`Feature::FunctionValueReuse`) until
  a separate item designs sound Arc-wrapping for it. Widening the clone path to `NonClone` here
  would open an `E0599`/`E0507` seal hole.
- **`rewrite_captured_clones` (`:1063`) / decoder-thunk path (`:13024`) / #164 `SharedLambda`
  promotion.** Orthogonal mechanisms for `!Clone` capture; not part of this fix.
- **The emitter's `CloneVar` rendering (`emit_expr.rs:5748`) and `emit_lambda_unboxed`
  (`:7689`).** They stay dumb; that is the whole point of choosing the lowerer layer.
- **`count_fn_value_uses_apply` (`:3114`).** Its direct-callee-borrow exemption is correct and
  independent of the SUM→MAX fix.

---

## 5. Test plan

All goldens live under `tests/golden/iNNN_slug/` (a `Main.sky`, optionally a hand-checked
`main.rs`), driven by the golden harness (skyc build → cargo build → run). New goldens:

### 5.1 New #193 goldens (must go skyc-0 → cargo-0 → run-correct)

- **`i193_taskseq_reuse`** — a `CloneOk` local (`String`) bound once, used inside a
  `task_and_then` continuation AND again after it (Task pipeline). Pre-fix: `E0382`/`E0507`.
  Verifies §2.3 + the `TaskSeq` recursion (`:3388`).
- **`i193_cmd_perform_reuse`** — a reused `String`/record captured into the `cmd_perform`
  `to_msg` lambda and read again in the same `update` arm. Verifies the `Call`/`Apply` path.
- **`i193_move_only_cmd_across_arms`** — a move-only `SkyCmd`-shaped (`NonClone`) or a `CloneOk`
  value used in DIFFERENT mutually-exclusive `case` arms with total occurrences ≥ 3 but per-arm
  ≤ 1. **Must NOT over-clone** (asserts the MAX fix): the taken arm's single use stays a bare
  move; no `.clone()` emitted. Pin the emitted `main.rs` (snapshot) so a regression to SUM is a
  golden diff, not just a build pass.
- **`i193_cli_pipeline_reuse`** — the CLI shape (a `Task.andThen` chain in `main`/`update` with a
  captured config string reused downstream), the analog of the reference's `07-todo-cli` shape.
  In this repo the nearest live example is `examples/20-cli-counter` / `examples/23-tui-todo`;
  the golden distills that pattern.
- **`i193_idempotent`** — a body that already carries the correct clones from the per-param loop;
  the new whole-body pass must leave it byte-identical (idempotence, §2.3 step 4).

### 5.2 Regression goldens that MUST stay green (no behavioral change)

- `i104_multiuse_let_clone`, `i112_closure_capture_reuse`, `i130_complex_arg_hoist`,
  `i142_copy_field_no_clone` (Copy fields must still NOT clone),
  `l0127_fn_carrier_reuse_gated`, `l0127_lambda_param_reuse_gated` (fail-closed still fails
  closed), `i177_*` (db-get false-positive clones), `i186_*`, `i191_input_arc_capture` (#191 arc
  path unchanged), `i164_poly_task_on_error_nested`, `i151_poly_task_on_error`,
  `aud04_taskseq_list`, `aud04_taskseqsync_move`, `mixed_arm_task_run_elision`,
  `tui_entry_case_taskrun`, `m5a_task_*`.
- Whole `cargo test -p sky_lower` + `-p sky_backend_rust` golden suites: zero new failures.

### 5.3 Example sweep

`scripts/examples-sweep.sh` (33 in-scope examples = 35 dirs minus Go-FFI) must stay
**VERDICT PASS** (BUILD ok + RUN ok; EQUIV per phase gate). Run
`SKY_SWEEP_NO_EQUIV=1 bash scripts/examples-sweep.sh` (phase-1 default). Special attention to
Task/CLI/TUI shapes: `20-cli-counter`, `23-tui-todo`, and any Sky.Live/Http examples with
`Cmd.perform`/`task_and_then` in `update`.

### 5.4 Explicit assertion for the MAX fix

Beyond build-pass, add a **snapshot** assertion in `i193_move_only_cmd_across_arms` counting the
literal `.clone()` occurrences in the emitted `main.rs` for the arm-reused symbol: it must be
`0` for a move-only last-use-per-arm value. A SUM regression re-introduces a `.clone()` and
fails the snapshot even if cargo (for a `CloneOk` type) still builds — catching the silent
efficiency/soundness regression the recon flags as most load-bearing.

---

## 6. Risk / blast-radius

This changes a **core codegen path** (`count_var_uses`/`count_fn_value_uses` drive every
existing T5 clone decision at four call sites plus TCO). Risks, ordered by severity:

1. **MAX under-counts a genuine SUM case → `E0382` (regression).** If any real shape needs the
   *sum* of arm uses (e.g. a value consumed in an arm body AND in a shared tail spliced after the
   match at the same IR level), MAX would under-clone. Mitigation: the arm bodies are mutually
   exclusive by construction (canonical `Match` has non-overlapping arms; the tail is not inside
   an arm body — it is a sibling the recursion counts separately). The scrutinee stays SUM.
   Every existing T5 golden (`i104`,`i112`,`i130`, all `i177`) plus the full sweep re-run is the
   backstop. **Any sweep red here = revert the MAX change and bisect, do not widen.**
2. **New whole-body pass double-clones (efficiency → potentially `E0505` on a borrow).** If the
   pass re-fires on a symbol the per-param loop already handled and mis-counts, it could insert a
   second `.clone()`. Mitigation: idempotence golden (`i193_idempotent`) + the fixpoint property
   of the `CloneVar` arm (`:3180`). The pass counts on the *post-per-param* body, so already-
   inserted `CloneVar`s are counted as uses (they are `CloneVar(s)` matched at `:2474`), keeping
   the seed stable.
3. **Ordering vs TCO.** The new pass must run BEFORE `analyze_tail_recursion`/`rewrite_tail_calls`
   (as the existing per-param loop does, `:5964` "Run BEFORE TCO"), so the loop rewrite sees
   correct clone nodes. Placing it after would let a tail-loop param reassignment move a value the
   next iteration needs. Mitigation: §2.3 explicitly sequences it before TCO; a TCO golden
   (`tui_entry_case_taskrun`, `i111_cli_program_seal`) guards it.
4. **`region_ty` failure for the whole-body candidate collection.** `ir_type_from_ty` can fail
   for not-yet-modelled types (as the match-arm site already tolerates, `:13300`). Mitigation:
   treat any type-lookup failure as "skip this symbol" (never clone on unknown type), identical
   to the existing site's `&& let Ok(ir_ty) = …` guard — fail *open on clone insertion* is safe
   here because the per-param/lower_let_pvar sites remain the primary coverage and a missed clone
   surfaces as a build error caught by the sweep, not a silent miscompile.
5. **Blast radius is bounded to `lower.rs`.** No emitter, no runtime, no scheme/kernel table
   touched — the seal surface (skyc-accept ⇒ cargo-build) is exercised entirely by the golden +
   sweep gates. If a shape regresses, it fails loudly at `cargo build` in a golden, never as a
   silent runtime divergence.

**Guardian ladder:** if the MAX change or the new pass cannot be made green across the full
golden suite + 33-example sweep without introducing a fail-closed `Feature::FunctionValueReuse`
on a shape that previously built, that is "a principle is hurt → rethink within boundary"; if no
in-boundary fix keeps both the E0382 class closed AND the over-clone class closed, revert and
escalate (do not ship a partial that trades one seal hole for another).

---

## Appendix A — reference ↔ fork mechanism map

| Concern | Reference (`ExprEmitter.hs`) | Our fork (`lower.rs`) |
|---|---|---|
| Where clones are decided | emitter (`clonePreludeFor` :764, `varLocalRead` :798) | lowerer (`rewrite_multiuse_clones` :3164) |
| Free-var multi-use count | `collectFreeVarLocalsMulti` :468 (arms MAX, scrutinee SUM) | `count_var_uses` :2472 / `count_fn_value_uses` :3018 (**arms SUM — the bug**) |
| Binder exclusion | `bound` set threaded through `go` | `if *name==sym`/`pat_binds_symbol` guards |
| Last use = move | `ecNoCloneVars` skip in `clonePreludeFor` | `remaining==1` bare (`:3172`) |
| Hoist site | prelude `let v=v.clone();` before the closure text | IR `Let{value:CloneVar}` wrapping the `Lambda` (`:3193`) |
| `!Clone` handling | `ecNoCloneVars` (move once) | `CloneClass::NonClone` + `reject_fn_value_reuse` (fail closed) |
| Re-thunk shared non-Clone | `ecThunkVars` (`name()`) | decoder-thunk `rewrite_var_to_apply` / #164 `SharedLambda` |

## Appendix B — the four existing driver sites (all invoke `rewrite_multiuse_clones`)

| Site | file:line | Binder kind |
|---|---|---|
| Typed fn params | `lower.rs:5955` | function parameter |
| Untyped fn params | `lower.rs:6117` | function parameter |
| `let`-body accumulator | `lower.rs:13045` (`lower_let_pvar`) | `let` value at its binding |
| Match-arm pattern vars | `lower.rs:13292` | `case … of` pattern binder |

Gap: none of these fire for a `let`-bound local whose *only* multi-use is reachable through a
`task_and_then`/`cmd_perform`/UI-handler `move` closure and whose arm-count was inflated by the
SUM bug. §2.1–§2.3 close both halves.

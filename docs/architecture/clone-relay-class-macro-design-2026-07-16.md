# Clone-relay class — macro root-cause + unified boundary-ownership design

Status: Proposed (design only — no code in this change)
Scope: `crates/sky_lower/src/lower.rs` move/clone ownership discipline
Subsumes: backlog #222 (match-arm n==1 relay gap); closes the whole
#104/#112/#164/#168/#172/#189/#193/#199/#218/#222 CLONE-RELAY class.

## 1. Problem

The lowerer guarantees that emitted Rust never fails `cargo` with
E0382/E0507/E0525 (use-of-moved-value / move-out-of-`Fn`-env) on a non-`Copy`
value — the SEAL applied to Rust move semantics. Today that guarantee is
discharged **per binder kind**: every construct that binds an owned symbol
(def param, lambda param, `let` name, match-arm pattern var, …) separately
invokes the count/clone/relay machinery. Five sites carry the same
`if n > 1 { multiuse } else { relay }` idea; #222 is the fifth-plus-one
occurrence of the same gap at a site whose guard shape differs slightly.

That is the smell: the obligation is a property of **closure boundaries**
(uniform), but the implementation triggers at **binder sites** (enumerable,
open-ended). Each new binder kind silently ships without the discipline until
a sweep finds the E0382/E0507.

**This design was validated empirically before writing it** (probes against
HEAD, `master-gate-target` ipe):

* **A 6th uncovered binder kind is a live SEAL breach today.** Destructure
  binders get NO ownership discipline at all:

  ```elm
  let (a, b) = pair in String.append a (String.append a b)
  ```

  emits `string_append(a, string_append(a, b))` — ipe exit 0, cargo
  E0382 `use of moved value: a`. (Probe: /tmp build of the emitted project.)
  This is exactly the failure mode the per-site architecture predicts.

* **The #218 `n == 1` branch over-clones at depth 0** (efficiency, P4 — not
  soundness). `report msg = Task.andThen (\errId -> … msg …) t` (one read,
  ONE boundary) now emits `{ let msg = msg.clone(); Box::new(move |errId| …
  msg.clone() …) }` — the prelude clone is dead weight: the capture is the
  last use of `msg`, so the pre-#218 bare capture was already sound and lean.
  The `n > 1` path treats the same shape leanly (last boundary stays bare);
  the `n == 1` branch does not, because `force_shared_capture_clones` wraps
  *every* directly-referencing lambda including the outermost one.

* All 70 byte-compared goldens are byte-identical at HEAD (fresh emit vs
  checked-in `main.rs`), so neither finding is currently visible in the
  golden suite — the over-clone shape is simply not exercised there.

## 2. Site + mechanism inventory (HEAD line numbers)

### 2.1 Binder-keyed trigger sites — the scattered class

| # | Binder kind | Site | n>1 multiuse | n==1 relay | T4 NonClone gate | Notes |
|---|---|---|---|---|---|---|
| S1 | def param (typed path) | `lower_def` `lower.rs:6822-6847` | ✓ `rewrite_multiuse_clones` | ✓ (#218) | ✓ `reject_fn_value_reuse` | eligibility = `param_is_multiuse_clonable` (CloneOk ∪ bare `Generic`, #189) |
| S2 | def param (untyped path) | `lower_def` `lower.rs:6998-7015` | ✓ | ✓ (#218) | ✓ | same as S1 |
| S3 | lambda param | `lower_lambda` `lower.rs:8094-8104` | ✓ (#199) | ✓ (#218) | ✓ (`:8118`) | eligibility = `CloneOk` ONLY — misses #189's bare-`Generic` admission (latent asymmetry vs S1/S2) |
| S4 | `let` PVar, non-fn CloneOk | `lower_let_pvar` `lower.rs:14488-14509` | ✓ | ✓ (#218) | ✓ (else-arm) | |
| S4b | `let` PVar, fn-typed | `lower_let_pvar` `lower.rs:14541-14569` | — | via `force_shared_capture_clones` | — | Arc promotion: `needs_shared_capture` (#164) ∨ `flows_into_sync_kernel_call` (#168) → `SharedLambda` + `promote_unification_sibling_lambdas` (#172) |
| S5 | match-arm pattern vars | `lower_case` `lower.rs:14756-14784` | ✓ | **✗ — #222** | ✓ (inside n>1 guard only) | type resolution nested inside `if n > 1 && let …` — the restructure #222 asks for |
| S6 | destructure binders (`let (a,b) = …`, single-arm `case … of (a,b) ->`, def-param destructure prologue) | `build_destructure_or_decoder_thunk` non-Decoder path `lower.rs:14377-14383`; `lower_case:14657-14667`; prologue folds `lower.rs:6987-6993`, `:8058-8064` | **✗ — live E0382 SEAL breach (probe-confirmed)** | ✗ | ✗ | only the `IrType::Decoder`-containing case gets a thunk (#125); every other component type gets nothing |
| S7 | C2 nested-cons bindings (`Just (h::t)` head/tail lets) | `lower_case` `lower.rs:14792-14798` | ✓ via S5 (`collect_arm_pat_pvars` walks the original pattern) | ✗ (same gap as S5) | ✓ via S5 | rides S5, so inherits S5's #222 gap |

Latent, unverified family members (enumerate now, verify during
implementation):

* **S3-generic**: a reused bare-`Generic` lambda param double-moves (S3 uses
  `CloneOk` only; S1/S2 use `param_is_multiuse_clonable`). Same E0382 class.
* **S-param-fn**: a NonClone fn-typed **param** (def or lambda) read once
  behind ≥ 2 move-closure boundaries. T4 rejects only n ≥ 2 consuming uses;
  the Arc-promotion path (S4b) exists only for `let`-bound lambda literals —
  a param has no literal to promote. Expected E0507; the sound outcome is
  either an `Arc` carrier at the def signature or a fail-closed diagnostic.

### 2.2 Boundary-keyed mechanisms — already uniform, keep

| Mechanism | Where | What it does |
|---|---|---|
| T3 `rewrite_captured_clones` (`lower.rs:1158`) | every `lower_lambda` (`:8036-8052`), decoder/task thunks (`:14403`, `:14462`), `eta_expand_partial` | use-site `CloneVar` for captured CloneOk reads INSIDE a re-callable `Fn` body (last *syntactic* use ≠ last *dynamic* use — a `Fn` body re-runs, so every consuming read must clone); depth-0 bare-callee exemption for NonClone fn captures |
| `wrap_shared_lambda_if_needed` (`lower.rs:3019`) | called from `force_shared_capture_clones` | the relay primitive: recurse-first, then pre-clone-shadow (`let sym = sym.clone() in <lambda>`) any lambda that directly references `sym` post-recursion — the #218 outward induction |
| emitter `emit_binding_stmts` clone-split + `Access` field-clone | `sky_backend_rust/emit_expr.rs` | ADR-0011 §1/§3 — alias whole+parts and field-read borrow discipline (orthogonal to this class; unchanged) |

## 3. The generative structure — why a relay per boundary

Four facts generate the entire class:

1. Every emitted Ipê closure is a **re-callable** `Box<dyn Fn>` /
   `Arc<dyn Fn>` (Go-parity; never `FnOnce`).
2. A Rust `move` closure takes every free variable **by value at
   construction**, regardless of how the body uses it.
3. A closure constructed **inside another closure's body** is re-constructed
   **per call** of the outer closure — so its capture-move consumes an outer
   env field through `&self` → E0507, *every* time, independent of use
   counts. The only fix is a fresh owned copy minted inside the outer body
   before the construction: `let s = s.clone();` — the relay.
4. By induction, a read at lambda-nesting depth *k* needs the relay at each
   of the *k−1* intermediate boundaries. Only the **outermost** (depth-0)
   boundary is a plain one-shot move in the binder's own scope, where
   ADR-0002's last-use rule applies (bare iff last consuming use).

So the obligation is: **closure construction is itself a consuming use of
everything it captures, at every nesting level.** Our IR models a depth-0
lambda capture as one consuming use (`count_var_uses`' Lambda arm = 1) but
has no first-class notion of the *inner* construction-uses — each patch
(#199 descent, #218 recurse-first + n==1 relay, #222) retrofits that missing
edge at one binder site.

### Reference comparison (`../sky`)

`ExprEmitter.hs:764` `clonePreludeFor` + the lambda-emission idiom at
`:794-813` (and its repeats at every closure-emitting production: let-bound,
call-arg, HOF-arg, top-level): at **each closure emission point**, collect
the body's free locals with the lambda-transparent `collectVarLocals`, emit a
`let v = v.clone();` prelude for **every** captured var, and thread
`ecCloneVars ∪ captured` into the body so every internal read also clones
(sub-A.10 C6). One rule, keyed on the **boundary**, applied wherever a
closure is emitted — binder kinds never participate, so a new binder kind
*cannot* be missed. The cost is blanket over-cloning (every boundary, every
capture, plus every use).

Our sanctioned divergence (ADR-0002) is the lean last-use discipline: N−1
clones instead of N, bare last move, borrow positions exempt. The port kept
the lean *rule* but moved the *trigger* from the boundary to the binder —
that relocation, not the lean rule, is what created the class. Lean and
boundary-keyed are compatible: the boundary walk has strictly more
information (it sees depth and successor uses), so it can decide "bare last
use at depth 0, relay everywhere deeper" exactly as leanly as the per-site
machinery — the `n > 1` path already proves it (its Lambda arm leaves the
last boundary bare and relays inside).

## 4. Design

Two stages. Stage 1 is small, mechanical, subsumes #222, fixes the confirmed
breaches, and removes the per-site branching. Stage 2 makes the invariant
structural so no future binder kind can miss it.

### Stage 1 — one discipline function, no per-site branch (tactical)

Key observation (verified against the machinery): `rewrite_multiuse_clones`
with `remaining = n` is **already correct and lean for every n ≥ 1**:

* `n > 1`: current behaviour (clone all but last; #199 inner descent).
* `n == 1`: the single occurrence stays bare (last use — no depth-0
  over-clone), and if it is a lambda capture, the Lambda arm's
  `force_shared_capture_clones(body)` descent installs the inner relays —
  precisely the #218 obligation, without the depth-0 prelude the current
  `else` branch adds.

So the entire per-site `if n > 1 { multiuse } else { force_shared }` split
collapses into one call. Introduce a single entry point:

```rust
/// The ONLY sanctioned way to make a freshly-bound owned symbol
/// move-safe in its scope. Every binder kind calls this — no site
/// carries its own n-branching.
fn apply_move_ownership(
    sym: Symbol,
    ir_ty: &IrType,
    scope: Expr,
    blame: Span,
) -> DResult<Expr> {
    if param_is_multiuse_clonable(ir_ty) {           // CloneOk ∪ bare Generic (#189)
        let mut remaining = count_var_uses(sym, &scope);
        Ok(rewrite_multiuse_clones(sym, &mut remaining, scope))
    } else {
        reject_fn_value_reuse(sym, ir_ty, &scope, blame)?;  // T4 fail-closed
        Ok(scope)
    }
}
```

Call sites (replacing the five open-coded blocks, adding the missing ones):

1. **S1/S2** def params: replace the block bodies with the call (behavioural
   change: removes the depth-0 over-clone in the `n == 1` lambda-capture
   case).
2. **S3** lambda params: same replacement; eligibility upgrades from
   `CloneOk`-only to `param_is_multiuse_clonable` (closes S3-generic).
3. **S4** let PVar: same replacement (the S4b Arc-promotion path is
   untouched — it is a value-carrier decision, not a relay decision; its
   `force_shared_capture_clones(name, acc)` stays because an Arc-promoted
   symbol needs the wrap at *every* directly-capturing lambda including
   depth 0, `Arc::clone` being the point of the promotion).
4. **S5** match arms — the #222 restructure: hoist type resolution out of
   the `n > 1` guard:

   ```rust
   for sym in collect_arm_pat_pvars(&br.pat.value) {
       let Some(span) = find_first_varlocal_span(sym, &br.body) else { continue };
       let Some(ty)   = self.region_ty(span)                    else { continue };
       let Ok(ir_ty)  = self.ir_type_from_ty(ty, span)          else { continue };
       arm_body = apply_move_ownership(sym, &ir_ty, arm_body, span)?;
   }
   ```

   (`apply_move_ownership` internally no-ops at `n == 0`, so unused pattern
   vars — and `CopyLeaf` via `param_is_multiuse_clonable` = false +
   `reject_fn_value_reuse` self-guarding — stay byte-identical.) S7 rides
   along for free.
5. **S6** destructure binders — the live breach: in
   `build_destructure_or_decoder_thunk`'s non-Decoder path (and thereby the
   single-arm `case` destructure and the `lower_let` pattern arm), after
   building the plain `Destructure`, run each `pat_bound_symbols` component
   through the same S5-style span→type→`apply_move_ownership` loop over the
   body. The def/lambda destructure-param prologues (`:6987`, `:8058`) are
   covered the same way (their component syms are currently invisible to the
   param loops, which see only the fresh binder symbol).

Also fold `param_is_multiuse_clonable`'s doc contract into
`apply_move_ownership`'s (single source of truth with
`render_fn_generics().with_clone()` — unchanged).

Stage-1 verification: full byte-golden suite (all 70 — this design's probe
harness already demonstrated a fresh-emit sweep is cheap), the
i164/i168/i172/i193/i199/i218 E2E families, a new golden for S6 (the probe
program verbatim), a new golden for #222's shape (case-bound var read once
through ≥ 2 boundaries), and the S3-generic/S-param-fn latent probes.

### Stage 2 — boundary-keyed ownership pass (structural)

Stage 1 still triggers per binder site; a *new* binder-producing lowering
path could still forget the call. Stage 2 moves the trigger into ONE
self-recursive pass so binder kinds are covered by construction.

**2a. Binder-class capture at lowering.** The pass cannot re-derive types
(arm/let component types come from canon spans via `region_ty`, unavailable
post-lowering), so the lowerer records each binder's ownership class in the
IR at the only moment it is known:

* `Func::params`, `Expr::Lambda::params`, `Expr::TailLoop::params` already
  carry `IrType` — no change.
* `Expr::Let` gains `class: OwnershipClass`.
* `Expr::Destructure` gains `bound: Vec<(Symbol, OwnershipClass)>`.
* `Arm` gains `bound: Vec<(Symbol, OwnershipClass)>`.

```rust
enum OwnershipClass {
    CopyLeaf,          // no discipline needed
    CloneOk,           // last-use lean + relay
    GenericClone,      // #189: T: Clone stamped by render_fn_generics
    ArcShared,         // Arc-promoted (S4b): relay = Arc::clone at every boundary
    NonCloneFn,        // T4 fail-closed on reuse; depth-0 callee exemption
    Unknown,           // type unresolvable: skip (today's behaviour, documented residual)
}
```

Adding these as **constructor-required fields** (not `Default`ed) makes "a
binder without a declared ownership class" unrepresentable — the same
must-preserve discipline `Expr::Call::pin` already established for IR→IR
rewrites. Rewrites that rebuild binders (`rewrite_multiuse_clones`,
`force_shared_capture_clones`, `promote_unification_sibling_lambdas`,
`rewrite_var_to_apply`, TCO) carry the fields through mechanically; relay
wraps mint `Let { class: same-as-wrapped-sym }`.

**2b. The pass.** One function, run once per lowered `Func` body in
`lower_def` (both typed and untyped paths), **before** TCO (the current
documented ordering), replacing every Stage-1 call site:

```
enforce_boundary_ownership(expr):
    match expr, and at each node that BINDS symbols
    (Func params seed the walk; Lambda/TailLoop params; Let; Destructure;
     Match arms):
        for each (sym, class) newly bound:
            scope = the subtree the binding scopes over
            scope = apply_move_ownership(sym, class, scope)   // Stage-1 fn
        recurse into children (exhaustive match over Expr — no `_`,
        SEAL §6: a future Expr variant is a compile error here)
```

Because the walk reaches every binder **through the IR itself**, any future
lowering path that produces binders through existing constructs is covered
automatically, and a genuinely new binder-carrying `Expr`/`Pat` variant
fails to compile until this pass (and the class fields) account for it.

`lower_lambda`'s T3 (`rewrite_captured_clones`) stays where it is initially
— it consumes canon-level capture info. A later stage may merge it too (the
pass's env already knows every in-scope symbol's class, so `captured =
free_vars(body) ∩ env` replaces `captured_locals`), but that is optional
consolidation, not required for the invariant.

**2c. The invariant** (the property that makes the class impossible):

> **Every move-closure boundary owns what it captures.** In the post-pass
> IR, for every `Lambda`/`SharedLambda` node L and every symbol `s` free in
> L's body:
> * if L sits at lambda-nesting depth ≥ 1 relative to `s`'s binder, a
>   pre-clone shadow (`Let { s, CloneVar(s), .. }`) sits between each pair of
>   adjacent boundaries on the binder→L path (the relay chain is complete);
> * if L sits at depth 0, its capture is bare only when it is the LAST
>   consuming use of `s` in the binder's scope (ADR-0002);
> * every consuming read of a captured CloneOk `s` inside a closure body is
>   `CloneVar` (re-callable `Fn` semantics, T3);
> * a `NonCloneFn` symbol never crosses a boundary except in depth-0 direct
>   callee position or behind an `ArcShared` promotion — anything else was
>   rejected fail-closed at lowering.

**2d. Mechanical enforcement.** A `#[cfg(any(test, debug_assertions))]`
validator `assert_boundary_ownership(&func)` walks the final IR and checks
the invariant, wired into the golden harness. A future regression then fails
at ipe time (a loud internal check), never at the downstream cargo build —
the SEAL's fail-closed direction.

### What stays deliberately lean (divergence preserved)

* Depth-0 last consuming use: bare move (reference clones it).
* Borrow positions (`Access` base, `Update` base, comparison/`++`): counted
  for liveness, never cloned (reference's coarser ≥2-uses set clones them).
* Boundaries a symbol does not cross: untouched (reference preludes every
  captured var at every boundary).
* The relay clone itself is unavoidable in both models — it is the E0507
  fix, not an over-clone.

ADR-0002's ledger entry remains accurate; this design changes *where* the
lean rule triggers, not the rule.

## 5. Risk

| Risk | Assessment | Mitigation |
|---|---|---|
| Byte-churn across goldens from re-homing the trigger | Low: per-sym rewrites are order-independent (`count_var_uses` treats `Var`/`CloneVar` alike; a relay wrap for `s1` introduces no new consuming occurrence of `s2`), so binder-order vs site-order commutes | Full 70-golden byte sweep is the gate (probe harness pattern: fresh emit + `diff`), plus E2E families |
| Stage-1 removal of the #218 depth-0 prelude changes emissions | Intended (restores lean); confirmed NOT visible in any current byte-golden | Re-run the i218 E2E family; the relay the fix exists for lives in the Lambda-arm descent, which is kept |
| S6 fix surfaces long-masked multiuse shapes (examples that never built) | That is the point — each is a today-latent E0382 | Sweep after landing; file new reds per sweep-to-green protocol |
| `OwnershipClass` fields drift through IR rewrites | Constructor-required fields (CallPin precedent) make dropping them a compile error | Validator (2d) catches value-level mistakes |
| `Unknown` class (unresolvable types) still skips discipline | Same residual as today — no regression, but not closed | Documented residual; the validator can WARN-count skips so the residual is measurable |
| S-param-fn (NonClone fn param behind ≥2 boundaries) not fixed by relay (can't clone) | Out of scope for the clone relay; needs Arc-at-signature or a fail-closed diagnostic | Enumerated as its own follow-up with a probe; the pass's NonCloneFn arm is where the gate lands |

## 6. Why this subsumes #222 and ends the class

* **#222**: match-arm binders become just another binder the ONE discipline
  reaches (Stage 1: restructured loop; Stage 2: `Arm::bound`). The n==1
  relay falls out of `rewrite_multiuse_clones(remaining = 1)`'s Lambda-arm
  descent — no fifth special case, and the type-resolution-inside-guard
  shape that made #222 need "a small restructure" disappears entirely.
* **Future sites**: the failure mode "new binder kind forgot to invoke the
  machinery" requires a binder the pass cannot see; binders only exist as IR
  constructs; the pass matches those exhaustively with no `_` arm, and the
  class fields are constructor-required. The invariant is then enforced at
  the boundary where the obligation is generated, exactly as the reference
  does — while keeping the lean clone discipline the reference lacks.

## 7. Confidence + residuals

Confidence: **high** on the class analysis, the site inventory, and the two
empirical findings (S6 breach and the depth-0 over-clone were both
reproduced against HEAD, and the 70-golden byte sweep at HEAD is clean).
**Medium-high** on Stage-1 byte-neutrality for goldens (commutativity argued,
not yet machine-checked) and on the S4b interaction (Arc promotion's
unconditional depth-0 wrap is intentionally kept — only the CloneOk n==1
branch loses its wrap).

Residuals (honest):

1. S3-generic and S-param-fn are inferred from code reading, not yet
   probe-confirmed.
2. `Unknown`-class binders (types `ir_type_from_ty` cannot model) remain
   undisciplined in both stages — the residual is inherited, now measurable.
3. `If`/`Match` branch-exclusive counting (`max`, not sum) is trusted as-is;
   the unified pass reuses it unchanged, so any latent defect there is
   neither fixed nor worsened.
4. Whether eta-synthesized params (`eta_expand_partial`) need their own
   registration in Stage 2b or are always covered as `Lambda::params` was
   not fully traced; flagged for the implementation lane.
5. T3 merger (2b's optional stage-3) left open — two collectors
   (canon-side `captured_locals`, IR-side env) coexist until then.

# Implementation Plan — Tail-Call Optimization (task #49)

Source design (already GO'd): `docs/architecture/tail-call-opt.md`. This plan
does **not** redesign; it turns that design into a mechanical, TDD, task-by-task
sequence. Every decision below is the spec's; anchors are re-verified against
HEAD.

Reference implementation mirrored throughout: **Sky's `Sky.Build.TailCallOpt`**
(`isTailRecursive`, `countTailSelfCalls`, `countNonTailSelfCalls`,
`rewriteTailCalls`). ipê mirrors its detect/rewrite mechanism and improves the
jump transport (typed IR nodes instead of a stringly kernel-name sentinel) and
the self-call identity (`FuncId` instead of `(module, name)`).

---

## Goal

Guarantee **constant Rust stack** for self-tail-recursive Sky functions, closing
a soundness hole: today a deep tail recursion (`foldl (+) 0` over ~10⁶ elements,
`Sky.Core.List`'s `foldl`/`find`/`any`/`all`/`member`/`drop`) overflows the
thread stack. A Rust stack overflow is **not a catchable panic** — it trips the
guard page and `abort()`s (`SIGABRT`), so the panic classifier
(`runtime/src/sky_runtime/core.rs:479-488`, `582-591`) never runs: no errId, no
structured Error, no `Err` at the Task boundary — an *unclassifiable process
death from well-typed Sky code*. TCO rewrites the qualifying shape to a Rust
`loop { … }` with `continue` jumps so the stack stays flat.

## Architecture

Three-stage change, one direction of data flow:

1. **IR** (`crates/sky_ir/src/ir.rs`) — add two typed `Expr` variants:
   `TailLoop { params, body }` and `TailRecur { args }`. Produced **only** by the
   lowerer's rewrite; carry their invariants by construction.
2. **Lower** (`crates/sky_lower/src/lower.rs`) — an analysis pass
   (`analyze_tail_recursion`) computed once per `Func`, and a rewrite
   (`rewrite_tail_calls`) that replaces each qualifying tail self-`Call` with
   `TailRecur` and wraps the body in `TailLoop`. Hooked in `lower_def`'s Typed
   arm.
3. **Emit** (`crates/sky_backend_rust/src/emit_expr.rs`) — `emit_func` detects a
   `TailLoop` body and emits `let mut`-shadowed params + `loop { … }`; a new
   tail-context emitter `emit_expr_tail` renders `return <expr>;` at leaf tail
   positions and a temporaries-first reassignment + `continue` at each
   `TailRecur`. Fail-closed `CompilerBug` if a `TailRecur`/`TailLoop` reaches the
   ordinary `emit_expr_at` path.

## Tech Stack

Rust (workspace crates `sky_ir`, `sky_lower`, `sky_backend_rust`, `skyc`);
`cargo test` per crate; end-to-end build+run via the shared harness
`crates/skyc/tests/support/mod.rs` (`build_and_run_emitted` →
`oracle::build_and_run_rust`, returning `RunOutcome { stdout, exit_code }` where
`exit_code == None` means killed by signal). Go-parity byte-diff via
`assert_go_parity` (cached oracle in `crates/skyc/tests/support/mod.rs:88`).

## Global Constraints

**PRINCIPLES order — apply in this priority when any step forces a trade-off:**
1. **Security** — no new exploitable surface. (TCO touches no untrusted-input
   boundary; the obligation is "introduce none".)
2. **Correctness** — TCO is **value-preserving**: the rewritten function returns
   byte-identical results to the non-TCO form and to the Go oracle. A test that
   only proves "does not crash" is insufficient; every regression pairs a value
   assertion.
3. **Soundness** — no path reaches UB, a panic from well-typed code, or a partial
   op outside its domain. The rewrite must never strand a self-`Call` outside the
   loop, never clobber a param mid-jump, never disqualify-then-rewrite.

**Two fundamental rules (non-negotiable):**
- **"If it compiles, it works."** Every panic/abort class closed here gets a
  regression test that fails pre-fix and passes post-fix — the failing test is
  the discovery artefact (Task 5 step 1 is the SIGABRT→exit-0 proof).
- **Make invalid states unrepresentable / parse-don't-validate.** The jump is a
  **typed** `TailRecur { args: Vec<Expr> }`, never the reference's `0x1F`-delimited
  `VarKernel "__tco_jump__"` sentinel string. The "should we TCO?" outcome is a
  two-constructor `enum TailRecursion`, a value — never a re-derived predicate.

**No wildcard swallow.** The two new `Expr` variants get **explicit** match arms
in every walker over `Expr`. Do NOT add or lean on any `_ =>`. Enforced
mechanically: all seven `expr_uses_*_kernel` walkers
(`lower.rs:268/336/399/464/508/556/602`) and `emit_expr_at`
(`emit_expr.rs:2187`) are currently exhaustive with explicit leaf enumeration, so
adding the variants **fails compilation** at each site until an arm is written
(Task 1 step 2 relies on exactly this).

**Fail-closed, not panic.** An out-of-place `TailRecur`/`TailLoop` on the ordinary
emit path is a `CompilerBug` **diagnostic** via the existing `bug(...)` helper
(`lower.rs:44`) → `Diagnostic::CompilerBug`, surfaced as an `Err`. Never
`panic!`/`unreachable!`/`todo!`.

**Scope (spec §4):** self-recursion only, keyed on `FuncId`. Mutual recursion
out. Non-tail recursion left as ordinary O(N)-stack Rust recursion (Limitation
#8). **Task-returning recursion excluded in v1** — `TaskSeq.rest` is treated as
non-tail (conservative: a Task-recursive fn is simply not TCO'd = today's
behaviour, no regression).

**Parallel-safety with the exit0 registry migration (spec §7):** file sets are
disjoint (TCO: `sky_ir`/`sky_lower`/`sky_backend_rust`; registry:
`sky_canon`/`sky_types`). The one coordination point is `ir.rs`: **Task 1 (the IR
variant addition) is the shared prerequisite — land it first** so both branches
compile against the widened `Expr` and neither papers over the new variants with
a wildcard.

**Public-artifact rule:** where the analysis mirrors Sky's `TailCallOpt`, cite it
as "the reference implementation". No disparagement; no contribute-upstream note.

---

## Task 1 — Add the two typed IR variants (shared prerequisite)

**Files:** `crates/sky_ir/src/ir.rs` (+2 `Expr` variants),
`crates/sky_lower/src/lower.rs` (7 walker arms),
`crates/sky_backend_rust/src/emit_expr.rs` (1 fail-closed arm in `emit_expr_at`).

**Interfaces — Produces:**
```rust
// crates/sky_ir/src/ir.rs — inside `pub enum Expr` (after `TaskSeq`, before the closing brace at ir.rs:800)

/// A tail-recursive function body wrapped for loop emission. Produced ONLY by
/// the lowerer's TCO rewrite; `params` are the enclosing `Func`'s parameters
/// (name + type) so emission can shadow them `let mut`. Invariant: contains ≥ 1
/// `TailRecur` in tail position and no self-`Call` to the enclosing `FuncId`
/// remains.
TailLoop {
    params: Vec<(Symbol, IrType)>,
    body: Box<Self>,
},
/// A tail self-call rewritten to a loop jump. `args` are the next-iteration
/// argument expressions, one per enclosing `TailLoop` parameter, in the same
/// order. Invariant: appears ONLY in tail position inside a `TailLoop`, and
/// `args.len() == TailLoop.params.len()`.
TailRecur {
    args: Vec<Self>,
},
```

**Consumes:** nothing yet (variants are unproduced until Task 3).

### Steps

1. **Write the variants.** Add the two arms above to `Expr` at `ir.rs:800`
   (immediately before the enum's closing `}`). No derive changes — both are
   `Clone + PartialEq + Debug` compatible (`Vec`/`Box`/`Symbol`/`IrType` all
   satisfy the derives already on the enum at `ir.rs:630`).

2. **Compile to discover every non-exhaustive site (the "failing test").**
   ```
   cargo build -p sky_ir -p sky_lower -p sky_backend_rust
   ```
   Expected: `error[E0004]: non-exhaustive patterns: TailLoop { .. } and TailRecur { .. } not covered` at each of:
   `lower.rs:269` (`expr_uses_db_kernel`), `lower.rs:337` (`expr_uses_tea_kernel`),
   `lower.rs:400` (`expr_uses_server_kernel`), `lower.rs:465` (`expr_uses_ui_kernel`),
   `lower.rs:509` (`expr_uses_live_kernel`), `lower.rs:557` (`expr_uses_tui_kernel`),
   `lower.rs:603` (`expr_uses_webview_kernel`), and `emit_expr.rs:2203`
   (`emit_expr_at`'s `match expr`). This compile failure IS the exhaustiveness
   test.

3. **Add explicit arms — kernel walkers.** In each of the seven
   `expr_uses_*_kernel` walkers, add (mirroring the existing `TaskSeq`/`If`
   recursion shape, e.g. `lower.rs:303`/`286`):
   ```rust
   Expr::TailLoop { body, .. } => expr_uses_db_kernel(body),
   Expr::TailRecur { args } => args.iter().any(expr_uses_db_kernel),
   ```
   (substitute the per-walker fn name: `..._tea_kernel`, `..._server_kernel`,
   `..._ui_kernel`, `..._live_kernel`, `..._tui_kernel`, `..._webview_kernel`).
   These recurse into the tail body / jump args exactly as the pre-TCO body would
   have, so kernel-presence detection is unchanged in meaning (a TCO'd `foldl`
   still reports the kernels its body uses).

4. **Add the fail-closed arm — `emit_expr_at`.** In the `match expr` at
   `emit_expr.rs:2203`, add (after the last real arm, still NOT a wildcard):
   ```rust
   // TCO nodes are produced by the lowerer's rewrite and consumed by
   // `emit_func`/`emit_expr_tail`; reaching one on the ordinary value-emit
   // path means the rewrite left a jump/loop outside a tail context — a
   // compiler bug, surfaced fail-closed (never a panic, never a wildcard).
   Expr::TailLoop { .. } | Expr::TailRecur { .. } => Err(Diagnostic::CompilerBug {
       where_: "sky_backend_rust::emit_expr_at",
       detail: "TailLoop/TailRecur reached the non-tail emit path".to_string(),
   }),
   ```
   (Use the `Diagnostic::CompilerBug` struct form directly — `emit_expr.rs`
   already constructs `Diagnostic::*` inline, e.g. `emit_expr.rs:2195`.)

5. **Run passes.**
   ```
   cargo build -p sky_ir -p sky_lower -p sky_backend_rust
   ```
   Expected: clean build, zero warnings.

6. **Unit test — variants construct + round-trip.** New file
   `crates/sky_ir/tests/tail_nodes.rs`:
   ```rust
   use sky_ir::{Expr, IrType};
   use sky_intern::Interner;

   #[test]
   fn tail_nodes_construct_and_clone() {
       let mut interner = Interner::new();
       let p = interner.intern("acc").unwrap();
       let loop_ = Expr::TailLoop {
           params: vec![(p, IrType::Int)],
           body: Box::new(Expr::TailRecur {
               args: vec![Expr::Int(1)],
           }),
       };
       assert_eq!(loop_.clone(), loop_);
   }
   ```
   Run:
   ```
   cargo test -p sky_ir --test tail_nodes
   ```
   Expected: `test tail_nodes_construct_and_clone ... ok`.

7. **Commit.** `git commit -m "sky_ir: add TailLoop/TailRecur IR nodes + explicit walker arms (#49)"`

---

## Task 2 — Detection pass `analyze_tail_recursion`

**File:** `crates/sky_lower/src/lower.rs` (free functions, near the
`expr_uses_*_kernel` cluster at `lower.rs:268`).

**Interfaces — Produces:**
```rust
/// Outcome of the tail-recursion analysis for one `Func`. Computed once; the
/// rewrite consumes it. Distinct constructors keep "should we TCO?" a value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TailRecursion {
    /// No self-call, or ≥ 1 self-call in non-tail position → leave as ordinary
    /// recursion (Limitation #8, O(N) stack).
    NotTailRecursive,
    /// Every self-call is a tail-position call at the correct arity, and there is
    /// ≥ 1 of them → safe to rewrite to a loop.
    TailRecursive,
}

/// Semantics mirror the reference's `isTailRecursive`
/// (`Sky.Build.TailCallOpt`): `tail_self_calls > 0 && non_tail_self_calls == 0`.
fn analyze_tail_recursion(self_id: FuncId, arity: usize, body: &Expr) -> TailRecursion;
```
**Consumes:** the enclosing `Func`'s `id`, `params.len()`, and lowered `body`
(available in `lower_def` Typed arm — Task 3).

**Tail-position rules (spec §3.1), the `in_tail = true` propagators:**
function body trailing expr; `If.then_` and `If.else_` (never `.cond`); every
`Match` arm body (never the scrutinee); `Let.body` and `Destructure.body` (never
their `value`). **Every other descent is `in_tail = false`**, critically
including `Lambda.body`, `Call` args, `Apply` func+args, `BinOp` operands, `Cons`,
`List` items, `Record`/`Update` fields, `Access.record`, `Tuple` elements, and
`TaskSeq.effect`/`.rest`.

A **qualifying tail self-call** is `Expr::Call { callee: Callee::Func(self_id),
args }` with `args.len() == arity`. A `Callee::Func(self_id)` at the wrong arity,
or appearing as `FuncValue`/`Apply` target, is **not** a jump — count it as a
non-tail self-reference so it disqualifies TCO (it is a genuine escape; the loop
must not touch it).

### Steps

1. **Write the failing unit tests first.** New file
   `crates/sky_lower/tests/tail_analysis.rs`. Because `TailRecursion` and
   `analyze_tail_recursion` are crate-private, expose them for test only: add
   `#[cfg(test)] pub(crate) use` is not enough across a `tests/` integration
   binary — instead gate a thin public shim behind a test-only feature. Simplest
   mechanical route: make the two items `pub` within a `pub mod tco` and re-export
   under `#[doc(hidden)]`:
   ```rust
   // in lower.rs, wrap the two items:
   #[doc(hidden)]
   pub mod tco_analysis {
       pub use super::{analyze_tail_recursion, TailRecursion};
   }
   ```
   Then the test constructs IR by hand (as `golden.rs` does, `golden.rs:9-16`):
   ```rust
   use sky_intern::Interner;
   use sky_ir::{Callee, Expr, FuncId, IrType};
   use sky_lower::tco_analysis::{analyze_tail_recursion, TailRecursion};

   // count n acc = if n == 0 then acc else count (n-1) (acc+1)
   fn build_count(i: &mut Interner) -> (FuncId, usize, Expr) { /* ... */ }

   #[test]
   fn count_is_tail_recursive() {
       let mut i = Interner::new();
       let (id, arity, body) = build_count(&mut i);
       assert_eq!(analyze_tail_recursion(id, arity, &body), TailRecursion::TailRecursive);
   }

   #[test]
   fn foldr_shape_is_not_tail_recursive() {
       // f x = g (f x)  — self-call is an ARG to g, non-tail
       // assert NotTailRecursive
   }

   #[test]
   fn self_call_in_lambda_is_not_tail() {
       // body = Lambda { body: Call(self_id, [..]) } placed in tail position
       // assert NotTailRecursive (inTail flips false entering Lambda.body)
   }

   #[test]
   fn wrong_arity_self_call_disqualifies() {
       // Call(self_id, args) with args.len() != arity → NotTailRecursive
   }

   #[test]
   fn no_self_call_is_not_tail_recursive() {
       // body with zero self-calls → NotTailRecursive (tail count == 0)
   }
   ```
   Run:
   ```
   cargo test -p sky_lower --test tail_analysis
   ```
   Expected: fails to compile (`analyze_tail_recursion` unresolved) — the red
   state.

2. **Minimal impl.** Add to `lower.rs` near `lower.rs:268`:
   ```rust
   #[derive(Clone, Copy, PartialEq, Eq, Debug)]
   enum TailRecursion { NotTailRecursive, TailRecursive }

   fn analyze_tail_recursion(self_id: FuncId, arity: usize, body: &Expr) -> TailRecursion {
       let mut tail = 0usize;
       let mut non_tail = 0usize;
       count_self_calls(self_id, arity, body, true, &mut tail, &mut non_tail);
       if tail > 0 && non_tail == 0 {
           TailRecursion::TailRecursive
       } else {
           TailRecursion::NotTailRecursive
       }
   }

   fn count_self_calls(
       self_id: FuncId,
       arity: usize,
       expr: &Expr,
       in_tail: bool,
       tail: &mut usize,
       non_tail: &mut usize,
   ) {
       match expr {
           // A direct call to the enclosing fn.
           Expr::Call { callee: Callee::Func(id), args } if *id == self_id => {
               if in_tail && args.len() == arity {
                   *tail += 1;
               } else {
                   *non_tail += 1;
               }
               // Arguments are ALWAYS non-tail, regardless of the call's position.
               for a in args {
                   count_self_calls(self_id, arity, a, false, tail, non_tail);
               }
           }
           // A call to a different fn / a kernel: not a self-call; args non-tail,
           // and the call itself is never in tail-jump position for OUR id.
           Expr::Call { args, .. } => {
               for a in args {
                   count_self_calls(self_id, arity, a, false, tail, non_tail);
               }
           }
           // A first-class reference to OUR fn that is not a direct Call = escape.
           Expr::FuncValue { callee: Callee::Func(id), .. } if *id == self_id => {
               *non_tail += 1;
           }
           Expr::FuncValue { .. } => {}
           Expr::Apply { func, args } => {
               count_self_calls(self_id, arity, func, false, tail, non_tail);
               for a in args { count_self_calls(self_id, arity, a, false, tail, non_tail); }
           }
           // Tail propagators.
           Expr::If { cond, then_, else_ } => {
               count_self_calls(self_id, arity, cond, false, tail, non_tail);
               count_self_calls(self_id, arity, then_, in_tail, tail, non_tail);
               count_self_calls(self_id, arity, else_, in_tail, tail, non_tail);
           }
           Expr::Match(m) => {
               count_self_calls(self_id, arity, m.scrutinee(), false, tail, non_tail);
               for arm in m.arms() {
                   count_self_calls(self_id, arity, &arm.body, in_tail, tail, non_tail);
               }
           }
           Expr::Let { value, body, .. } => {
               count_self_calls(self_id, arity, value, false, tail, non_tail);
               count_self_calls(self_id, arity, body, in_tail, tail, non_tail);
           }
           Expr::Destructure { value, body, .. } => {
               count_self_calls(self_id, arity, value, false, tail, non_tail);
               count_self_calls(self_id, arity, body, in_tail, tail, non_tail);
           }
           // Non-tail descents.
           Expr::Lambda { body, .. } => {
               count_self_calls(self_id, arity, body, false, tail, non_tail);
           }
           Expr::BinOp { lhs, rhs, .. } => {
               count_self_calls(self_id, arity, lhs, false, tail, non_tail);
               count_self_calls(self_id, arity, rhs, false, tail, non_tail);
           }
           Expr::Cons { head, tail: t } => {
               count_self_calls(self_id, arity, head, false, tail, non_tail);
               count_self_calls(self_id, arity, t, false, tail, non_tail);
           }
           Expr::Tuple(xs) | Expr::List { items: xs, .. } => {
               for x in xs { count_self_calls(self_id, arity, x, false, tail, non_tail); }
           }
           Expr::Record(fs) => for (_, v) in fs { count_self_calls(self_id, arity, v, false, tail, non_tail); },
           Expr::Update { record, fields } => {
               count_self_calls(self_id, arity, record, false, tail, non_tail);
               for (_, v) in fields { count_self_calls(self_id, arity, v, false, tail, non_tail); }
           }
           Expr::Access { record, .. } => count_self_calls(self_id, arity, record, false, tail, non_tail),
           Expr::Ctor { args, .. } => for a in args { count_self_calls(self_id, arity, a, false, tail, non_tail); },
           // Task recursion excluded in v1: BOTH sub-terms non-tail.
           Expr::TaskSeq { effect, rest } => {
               count_self_calls(self_id, arity, effect, false, tail, non_tail);
               count_self_calls(self_id, arity, rest, false, tail, non_tail);
           }
           // Leaves + the not-yet-produced TCO nodes (explicit, no wildcard).
           Expr::Int(_) | Expr::Bool(_) | Expr::Float(_) | Expr::Str(_) | Expr::Char(_)
           | Expr::Unit | Expr::Var(_) => {}
           Expr::TailLoop { body, .. } => count_self_calls(self_id, arity, body, in_tail, tail, non_tail),
           Expr::TailRecur { args } => for a in args { count_self_calls(self_id, arity, a, false, tail, non_tail); },
       }
   }
   ```
   Add the `#[doc(hidden)] pub mod tco_analysis` re-export from step 1.

3. **Run passes.**
   ```
   cargo test -p sky_lower --test tail_analysis
   ```
   Expected: all five tests `ok`.

4. **Commit.** `git commit -m "sky_lower: tail-recursion detection pass (#49)"`

---

## Task 3 — Rewrite `rewrite_tail_calls` + `lower_def` hook

**File:** `crates/sky_lower/src/lower.rs`.

**Interfaces — Produces:**
```rust
/// Wrap a proven-tail-recursive body for loop emission. `analyze_tail_recursion`
/// MUST have returned `TailRecursive` first (no non-tail self-call survives), so
/// this cannot strand a self-`Call` outside the loop. Mirrors the reference's
/// `rewriteTailCalls`.
fn rewrite_tail_calls(self_id: FuncId, arity: usize, params: Vec<(Symbol, IrType)>, body: Expr) -> Expr;
```
**Consumes:** `TailRecursion::TailRecursive` (Task 2), the Typed arm's `id`,
`params`, `lowered_body` (`lower.rs:1216-1223`).

### Steps

1. **Failing golden test — hook produces a `TailLoop`.** Extend
   `crates/sky_lower/tests/tail_analysis.rs` with a rewrite assertion:
   ```rust
   use sky_lower::tco_analysis::rewrite_tail_calls; // add to the doc-hidden re-export

   #[test]
   fn rewrite_wraps_in_tailloop_and_replaces_jump() {
       let mut i = Interner::new();
       let (id, arity, body) = build_count(&mut i);
       let params = /* the two (Symbol, IrType) params of count */;
       let out = rewrite_tail_calls(id, arity, params, body);
       // Top node is a TailLoop.
       let Expr::TailLoop { body, .. } = out else { panic!("expected TailLoop") };
       // The else-branch self-call is now a TailRecur, and no self-`Call` remains.
       assert!(!contains_self_call(id, &body));
       assert!(contains_tail_recur(&body));
   }
   ```
   (`contains_self_call` / `contains_tail_recur` are small recursive test
   helpers.) Run:
   ```
   cargo test -p sky_lower --test tail_analysis
   ```
   Expected: red (`rewrite_tail_calls` unresolved).

2. **Minimal impl.** Add to `lower.rs`:
   ```rust
   fn rewrite_tail_calls(self_id: FuncId, arity: usize, params: Vec<(Symbol, IrType)>, body: Expr) -> Expr {
       let rewritten = rewrite_in_tail(self_id, arity, body);
       Expr::TailLoop { params, body: Box::new(rewritten) }
   }

   fn rewrite_in_tail(self_id: FuncId, arity: usize, expr: Expr) -> Expr {
       match expr {
           // The one transformation: a qualifying tail self-call becomes a jump.
           Expr::Call { callee: Callee::Func(id), args } if id == self_id && args.len() == arity => {
               Expr::TailRecur { args }
           }
           // Tail propagators recurse in-tail; everything else is returned
           // untouched (non-tail subterms keep their ordinary shape).
           Expr::If { cond, then_, else_ } => Expr::If {
               cond,
               then_: Box::new(rewrite_in_tail(self_id, arity, *then_)),
               else_: Box::new(rewrite_in_tail(self_id, arity, *else_)),
           },
           Expr::Match(m) => Expr::Match(m.map_arm_bodies(|b| rewrite_in_tail(self_id, arity, b))),
           Expr::Let { name, value, body } => Expr::Let {
               name, value,
               body: Box::new(rewrite_in_tail(self_id, arity, *body)),
           },
           Expr::Destructure { binder, value, body } => Expr::Destructure {
               binder, value,
               body: Box::new(rewrite_in_tail(self_id, arity, *body)),
           },
           // Every non-tail form (incl. non-jump Calls, Apply, Lambda, leaves,
           // TaskSeq) is returned verbatim — the analysis proved no self-`Call`
           // survives in these, so nothing to rewrite.
           other => other,
       }
   }
   ```
   `Match::map_arm_bodies` (if absent) is a small helper on `Match` in `sky_ir`
   that rebuilds arms with mapped bodies while preserving the arm-set validation
   invariant (`Match::new`, `ir.rs:625`). If adding it to `sky_ir` is undesirable,
   destructure `m.arms()` and rebuild via `Match::new` inside `rewrite_in_tail`
   instead — same effect, no `sky_ir` change. **Prefer the local rebuild** to keep
   the `ir.rs` surface at exactly +2 variants for parallel-safety with the
   registry migration.

3. **Wire the hook — `lower_def` Typed arm.** In `lower.rs`, between the prologue
   fold (ends `lower.rs:1207`) and the `Func` construction (`lower.rs:1216`),
   insert:
   ```rust
   // TCO: if every self-call is a tail call, rewrite the body to a loop so the
   // Rust stack stays flat (mirrors Sky's TailCallOpt). Self-recursion only,
   // keyed on FuncId; Task-recursion excluded (see analyze_tail_recursion).
   let arity = params.len();
   if let TailRecursion::TailRecursive = analyze_tail_recursion(id, arity, &lowered_body) {
       lowered_body = rewrite_tail_calls(id, arity, params.clone(), lowered_body);
   }
   ```
   (`params` is the `Vec<(Symbol, IrType)>` from `split_typed_sig`,
   `lower.rs:1195`. Clone is required because `params` is also moved into the
   `Func` at `lower.rs:1221`.)

4. **Run passes.**
   ```
   cargo test -p sky_lower --test tail_analysis
   cargo build -p sky_lower
   ```
   Expected: rewrite test `ok`; clean build.

5. **Commit.** `git commit -m "sky_lower: rewrite tail self-calls to TailLoop/TailRecur + lower_def hook (#49)"`

---

## Task 4 — Emission (`emit_func` + `emit_expr_tail`) + emission goldens

**File:** `crates/sky_backend_rust/src/emit_expr.rs`.

**Interfaces — Produces:**
```rust
/// Emit an `Expr` in TAIL/STATEMENT context: leaf tail positions become
/// `return <expr>;`; a `TailRecur` becomes a temporaries-first reassignment plus
/// `continue`. `loop_params` are the enclosing `TailLoop`'s params (name+type),
/// giving each `TailRecur.args[i]` its destination parameter name.
fn emit_expr_tail(
    ctx: &EmitCtx,
    expr: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
    loop_params: &[(Symbol, IrType)],
) -> DResult<String>;
```
**Consumes:** a `Func` whose `body` is `Expr::TailLoop { params, body }` (Task 3);
`emit_apply`'s non-consuming closure-call shape (`emit_expr.rs:3081-3096`, `(f)(x)`
= `Fn::call(&f, …)`).

### Steps

1. **Failing positive golden.** New file
   `crates/sky_backend_rust/tests/tail_call.rs`. Build the `count` `Func` by hand
   (id, two params, `TailLoop` body) and assert the emitted text:
   ```rust
   #[test]
   fn tco_emits_loop_continue_and_mut_shadows() {
       let src = emit_the_count_func(); // via RustBackend::emit / emit_func on a hand-built Func
       assert!(src.contains("loop {"), "no loop:\n{src}");
       assert!(src.contains("continue;"), "no continue:\n{src}");
       assert!(src.contains("let mut n = n;"), "no mut shadow n:\n{src}");
       assert!(src.contains("let mut acc = acc;"), "no mut shadow acc:\n{src}");
       // No self-recursive Call to the fn's own emitted name survives.
       assert!(!src.contains("count("), "self-call leaked:\n{src}");
       // Temporaries-first jump.
       assert!(src.contains("__tco_0"), "no jump temp:\n{src}");
   }
   ```
   Run:
   ```
   cargo test -p sky_backend_rust --test tail_call
   ```
   Expected: red (`emit_func` still emits the body via `emit_expr`, producing a
   `CompilerBug` Err on the `TailLoop`, so the emit fails / no `loop {`).

2. **Impl `emit_expr_tail`.** Add near `emit_apply` (`emit_expr.rs:3081`):
   ```rust
   fn emit_expr_tail(
       ctx: &EmitCtx,
       expr: &Expr,
       indent: usize,
       depth: u16,
       generics: GenericScope,
       loop_params: &[(Symbol, IrType)],
   ) -> DResult<String> {
       let pad = "    ".repeat(indent);
       match expr {
           Expr::If { cond, then_, else_ } => {
               let c = emit_expr_at(ctx, cond, indent, depth + 1, generics)?;
               let t = emit_expr_tail(ctx, then_, indent + 1, depth + 1, generics, loop_params)?;
               let e = emit_expr_tail(ctx, else_, indent + 1, depth + 1, generics, loop_params)?;
               Ok(format!("if {c} {{\n{t}\n{pad}}} else {{\n{e}\n{pad}}}"))
           }
           Expr::Match(m) => { /* emit `match <scrut> { <pat> => { <tail arm> } ... }`,
                                  each arm body via emit_expr_tail; reuse the existing
                                  pattern-rendering used by the ordinary Match arm. */ }
           Expr::Let { name, value, body } => {
               let v = emit_expr_at(ctx, value, indent, depth + 1, generics)?;
               let b = emit_expr_tail(ctx, body, indent, depth + 1, generics, loop_params)?;
               Ok(format!("{pad}let {} = {v};\n{b}", ctx.emit_ident(*name)?))
           }
           Expr::Destructure { binder, value, body } => { /* `let <binder> = <value>;` then tail body */ }
           // The jump: temporaries-first reassignment + continue (spec §3.2).
           Expr::TailRecur { args } => {
               debug_assert_eq!(args.len(), loop_params.len());
               let mut temps = String::new();
               let mut writes = String::new();
               for (i, arg) in args.iter().enumerate() {
                   let a = emit_expr_at(ctx, arg, indent, depth + 1, generics)?;
                   temps.push_str(&format!("{pad}let __tco_{i} = {a};\n"));
               }
               for (i, (name, _ty)) in loop_params.iter().enumerate() {
                   writes.push_str(&format!("{pad}{} = __tco_{i};\n", ctx.emit_ident(*name)?));
               }
               Ok(format!("{temps}{writes}{pad}continue;"))
           }
           // Every other node is a leaf tail position → return its value.
           other => {
               let v = emit_expr_at(ctx, other, indent, depth + 1, generics)?;
               Ok(format!("{pad}return {v};"))
           }
       }
   }
   ```
   Note on the `other => return` arm: it is the intended value/statement split
   (the reference's `walk True` leaf case), NOT a wildcard over `Expr` variants
   for walker-exhaustiveness purposes — `emit_expr_at` inside it is the exhaustive
   walker. A `TailRecur` can never reach `other` (handled above); a `TailLoop`
   nested in tail position is impossible by construction (the rewrite wraps only
   at the top), and if one did it would route to `emit_expr_at`'s fail-closed arm
   (Task 1 step 4).

3. **Impl the `emit_func` detection + `let mut` shadows.** In `emit_func`
   (`emit_expr.rs:3217`), replace the body line (`emit_expr.rs:3260`):
   ```rust
   let body = emit_expr(ctx, &func.body, 1, generics)?;
   ```
   with:
   ```rust
   let body = match &func.body {
       Expr::TailLoop { params: loop_params, body: loop_body } => {
           let mut shadows = String::new();
           for (name, _ty) in loop_params {
               shadows.push_str(&format!("let mut {p} = {p};\n    ", p = ctx.emit_ident(*name)?));
           }
           let inner = emit_expr_tail(ctx, loop_body, 2, 1, generics, loop_params)?;
           format!("{shadows}loop {{\n{inner}\n    }}")
       }
       _ => emit_expr(ctx, &func.body, 1, generics)?,
   };
   ```
   The signature line (`emit_expr.rs:3262`) is untouched — mutability is
   introduced only by the local `let mut p = p;` shadow, so the public `fn`
   signature stays byte-identical to the non-TCO form (load-bearing for
   `FuncValue` boxing / trait-object slots, spec §3.2). The `loop { … }` ends only
   in `return`/`continue`, so it types as `!` and unifies with any `-> R` — no
   `break value`, no loop-as-expression typing.

4. **Run passes (positive golden).**
   ```
   cargo test -p sky_backend_rust --test tail_call
   ```
   Expected: `tco_emits_loop_continue_and_mut_shadows ... ok`.

5. **Failing negative golden.** Add to `tail_call.rs`:
   ```rust
   #[test]
   fn non_tail_recursion_is_untouched() {
       // A `foldr`-shaped Func (self-call is an arg to a Ctor/BinOp) — analyze
       // returns NotTailRecursive, so lower_def never wraps it; emit is ordinary.
       let src = emit_the_foldr_func();
       assert!(!src.contains("loop {"), "unexpected loop in non-tail fn:\n{src}");
   }

   #[test]
   fn self_call_in_lambda_not_rewritten() {
       // Only self-call sits in a Lambda body → NotTailRecursive → no loop.
       let src = emit_the_lambda_embedded_func();
       assert!(!src.contains("loop {"));
   }
   ```
   These build the `Func` **through the full lower path** (so `lower_def`'s
   analyze+rewrite gate runs) rather than hand-injecting a `TailLoop`. If a
   through-lower harness is heavier than hand-building, assert instead at the
   `analyze_tail_recursion` level (already covered in Task 2) AND hand-build a
   non-`TailLoop` `Func` here to confirm `emit_func` emits ordinary recursion for
   a plain body. Run:
   ```
   cargo test -p sky_backend_rust --test tail_call
   ```
   Expected: all `ok`.

6. **Commit.** `git commit -m "sky_backend_rust: emit TailLoop as loop+continue with mut shadows (#49)"`

---

## Task 5 — End-to-end regressions (soundness proof + parity + edges)

**Files:** `crates/skyc/tests/golden_tco.rs` (new), `tests/golden/tco_*/Main.sky`
(new golden fixtures), `crates/skyc/tests/support/mod.rs` (one new stack-limited
run helper).

**Interfaces — Consumes:** `build_and_run_emitted` (`support/mod.rs:57`, returns
`RunOutcome { stdout, exit_code }`, `exit_code == None` ⇔ killed by signal),
`assert_go_parity` (`support/mod.rs:88`).

### Steps

1. **Add the stack-limited run helper (determinism mechanism).** In
   `support/mod.rs`, add:
   ```rust
   /// Build the emitted project and run its binary with the MAIN-THREAD stack
   /// capped to `stack_kib` KiB (via `bash -c 'ulimit -s <kib>; exec …'`), so a
   /// deep recursion overflows deterministically at a few thousand frames instead
   /// of needing ~10^6. Linux/macOS CI only (the Rust backend's target).
   #[allow(dead_code)]
   pub fn build_and_run_stack_limited(golden: &str, dir: &Path, stack_kib: u32) -> RunOutcome { /* locate bin via oracle::build_and_run_rust's build half, then Command::new("bash").arg("-c").arg(format!("ulimit -s {stack_kib}; exec \"$0\"")).arg(bin) */ }
   ```
   (Reuse `oracle`'s build+locate; only the *run* invocation is wrapped. If
   `oracle::build_and_run_rust` does not expose a build-only entry, add a thin
   `oracle::build_rust_locate_bin` alongside it — same code path, split at the
   spawn.)

2. **Constant-stack regression — the soundness proof (RED first).** Fixture
   `tests/golden/tco_count/Main.sky`:
   ```elm
   module Main exposing (main)
   import Sky.Core.Prelude exposing (..)
   import Std.Log exposing (println)

   count : Int -> Int -> Int
   count n acc = if n == 0 then acc else count (n - 1) (acc + 1)

   main = println (String.fromInt (count 2000000 0))
   ```
   Test:
   ```rust
   #[test]
   fn tco_count_runs_to_completion_constant_stack() {
       let dir = compile_golden("tco_count"); // skyc → emitted Rust project
       let out = build_and_run_stack_limited("tco_count", &dir, 512); // 512 KiB main stack
       assert_eq!(out.exit_code, Some(0), "expected clean exit, got {:?}", out.exit_code);
       assert_eq!(out.stdout.trim(), "2000000");
   }
   ```
   **Prove it is a real regression:** with the Task 3 hook temporarily disabled
   (comment out the `if let TailRecursion::TailRecursive` block), the same test
   yields `exit_code == None` (SIGABRT on stack overflow at 512 KiB) — capture
   that as the discovery artefact in the commit message, then re-enable. Run:
   ```
   cargo test -p skyc --test golden_tco tco_count_runs_to_completion_constant_stack
   ```
   Expected post-fix: `ok`.

3. **Arg-swap foreclosure.** Fixture `tests/golden/tco_swap/Main.sky`:
   ```elm
   module Main exposing (main)
   import Sky.Core.Prelude exposing (..)
   import Std.Log exposing (println)

   go : Int -> Int -> List Int -> ( Int, Int )
   go a b xs =
       case xs of
           [] -> ( a, b )
           _ :: rest -> go b a rest

   main =
       let ( x, y ) = go 1 2 [ 0, 0, 0 ] in
       println (String.fromInt x ++ "," ++ String.fromInt y)
   ```
   With 3 elements the params swap an odd number of times → `(2, 1)`.
   ```rust
   #[test]
   fn tco_arg_swap_uses_temporaries_first() {
       let dir = compile_golden("tco_swap");
       let out = build_and_run_emitted("tco_swap", &dir);
       assert_eq!(out.exit_code, Some(0));
       assert_eq!(out.stdout.trim(), "2,1"); // clobber would give "1,1" or "2,2"
   }
   ```
   The wrong (naive sequential) assignment would print `1,1` — the temporaries-
   first shape is what makes it `2,1`.

4. **Value-param double-use (spec §5 edge 6).** Fixture
   `tests/golden/tco_double_use/Main.sky` — a jump argument that reads a value
   param also moved by another jump argument:
   ```elm
   module Main exposing (main)
   import Sky.Core.Prelude exposing (..)
   import Std.Log exposing (println)

   go : Int -> Int -> Int -> Int
   go n a b =
       if n == 0 then a + b
       else go (n - 1) (a + b) a   -- new `a` = a+b, new `b` = old a (both read a)

   main = println (String.fromInt (go 5 1 0))
   ```
   ```rust
   #[test]
   fn tco_value_param_double_use_compiles_and_computes() {
       let dir = compile_golden("tco_double_use");
       let out = build_and_run_emitted("tco_double_use", &dir);
       assert_eq!(out.exit_code, Some(0));
       assert_eq!(out.stdout.trim(), "8"); // Fibonacci-ish: 1,1,2,3,5,8
   }
   ```
   `Int` is `Copy`, so this compiles trivially; the value-of-interest is that the
   temporaries read the *current* params (spec §3.2). **If a non-`Copy` value
   param (a `Vec`/`String`) is double-used across jump args and fails to compile,
   the fix belongs in the shared argument-emission path (cf. the targeted
   `DictGet` clone at `emit_expr.rs:2344-2349`), NOT a TCO-local hack** — flag it
   and stop; do not clone-blanket inside `emit_expr_tail`.

5. **Go-parity byte-diff.** For each of `tco_count`, `tco_swap`, `tco_double_use`,
   add a parity assertion so TCO is proven **value-preserving**, not merely
   non-crashing:
   ```rust
   #[test]
   fn tco_count_matches_go_oracle() {
       let dir = compile_golden("tco_count");
       let out = build_and_run_emitted("tco_count", &dir);
       assert_go_parity("tco_count", golden_dir("tco_count"), &out.stdout);
   }
   ```
   Refresh the cached oracle first (`refresh-oracle` tool, per `support/mod.rs`
   docs) so `expected_go.txt` exists and the staleness gate passes. `tco_count`'s
   `2000000` may be slow for the Go oracle; if so, add a smaller-N sibling fixture
   `tco_count_small` (`count 1000 0` → `1000`) purely for the parity check and
   keep the 2 000 000 run for the stack proof.

6. **Analysis units already covered in Task 2** — cross-reference, no new work:
   `TailRecursive` for `count`/`foldl`, `NotTailRecursive` for `map`/`foldr` and
   the lambda-embedded self-call.

7. **Run the full new suite.**
   ```
   cargo test -p skyc --test golden_tco
   ```
   Expected: all TCO E2E tests `ok`.

8. **Commit.** `git commit -m "skyc: TCO end-to-end regressions — constant-stack, arg-swap, double-use, Go-parity (#49)"`

---

## Sequencing & parallel-safety recap

- **Task 1 lands first** (shared `ir.rs` prerequisite) so the exit0 registry
  migration compiles against the widened `Expr` and adds explicit arms rather
  than a wildcard.
- Tasks 2-5 are TCO-local (`sky_lower`, `sky_backend_rust`, `skyc/tests`) and do
  not touch `sky_canon`/`sky_types` — disjoint from the registry migration.
- After all tasks: run `cargo test --workspace` and (per the sweep discipline) let
  CI run the example sweep; do not run the full local example/perf sweep.

## Definition of done

- `cargo test --workspace` green.
- `tco_count` exits 0 with `2000000` under a 512 KiB stack; the same fixture
  SIGABRTs (`exit_code == None`) with the hook disabled (documented in the commit).
- Positive/negative emission goldens, arg-swap, value-double-use, and Go-parity
  all green.
- No `_ =>` arm added over `Expr`; `TailLoop`/`TailRecur` have explicit arms in
  all seven `expr_uses_*_kernel` walkers and `emit_expr_at`; the out-of-place case
  is a `CompilerBug` `Err`, never a panic.

# Tail-Call Optimization in the ipê Rust backend

Status: DESIGN (doc-only). Target window: pre-sweep pull-early, alongside the
exit0 registry migration. Task #49.

## 1. Why this is a soundness fix, not an efficiency tweak

ipê today lowers a tail-recursive Sky function to ordinary Rust recursion: one
Rust stack frame per Sky recursive step. `Sky.Core.List` builds `foldl`, `find`,
`any`, `all`, `member`, and `drop` on the assumption that self-tail-recursion is
constant-stack (this is stated in the compiler contract, "TCO mechanism", and in
Limitation #8, which only tolerates *non-tail* list ops as O(N) stack). Without
TCO those bindings recurse one frame deep per element, and a large-input example
(a `foldl` over ~10^6 elements, a hand-written counter) overflows the thread
stack.

A Rust stack overflow is **not a panic**. It trips the guard page, the Rust
runtime prints `thread '…' has overflowed its stack` and calls `abort()` →
`SIGABRT`. The panic classifier is installed via `std::panic::set_hook`
(`runtime/src/sky_runtime/core.rs:582`, `install_panic_classifier`) and only ever
sees *unwinding* panics; `classify_panic` (`core.rs:479-488`) maps
`divide by zero` / `index out of bounds` / `overflow` message text to a Sky error
kind and otherwise yields `"Unexpected"`. A stack overflow reaches **neither** —
the hook does not run and `catch_unwind` cannot absorb it. So today a deep
tail-recursion terminates as an **unclassifiable hard abort**: no errId, no
structured Error log, no `Err` value at the Task boundary. That is the soundness
hole (an unclassifiable process death from well-typed Sky code); TCO closes it by
guaranteeing constant stack for the self-tail-recursive shape.

This design **mirrors Sky's `Sky.Build.TailCallOpt`** (the reference
implementation, hereafter "the reference"): detect the "every self-call is in
tail position" pattern and rewrite the body as a loop where each tail self-call
becomes a simultaneous parameter reassignment plus a `continue`. ipê improves on
the reference's transport mechanism (see §3).

## 2. What ipê already has (investigation, cited)

- IR is purely expression-based. `Func` (`crates/sky_ir/src/ir.rs:338-368`)
  carries `id: FuncId`, `params: Vec<(Symbol, IrType)>`, `ret: IrType`,
  `body: Expr`. There is **no** loop / statement / `continue` IR node — `rg` for
  `Loop|Forever|Continue|While` in `ir.rs` returns nothing.
- `Expr` (`ir.rs:631-800`) has the tail-shaped nodes the analysis needs:
  `If { cond, then_, else_ }`, `Match(Match)` (arm bodies), `Let { name, value,
  body }`, `Destructure { binder, value, body }`, and the leaf value nodes.
- A self-call is `Expr::Call { callee: Callee::Func(id), args }`
  (`ir.rs:707-710`, `Callee` at `ir.rs:804-807`) where `id == func.id`. ipê keys
  self-recursion on a stable `FuncId`, not on a `(module, name)` pair as the
  reference must (`TailCallOpt.hs:82-87`). This is strictly more precise — see the
  closure-shadow edge in §5.
- Function bodies are assembled in `lower_def`
  (`crates/sky_lower/src/lower.rs:1168-1250`); the fully-lowered body and the
  arity (`params.len()`) are both in hand at `ir.rs`-build time
  (`lower.rs:1216` Typed arm). The untyped arm (`lower.rs:1239`) has no
  parameters, so it can never be a self-call-with-args and is out of scope by
  construction.
- Emission: `emit_func` (`crates/sky_backend_rust/src/emit_expr.rs:3217-3265`)
  prints `pub fn {name}{generics}({params}) -> {ret} {{ {body} }}` where `body`
  is a single expression. `emit_apply` (`emit_expr.rs:3081-3096`) emits a
  first-class call as `({f})({args})`; for a `Box<dyn Fn>` parameter this is a
  `Fn::call(&f, …)` auto-ref, i.e. **calling a closure parameter does not move
  it** — load-bearing for the func-param edge in §5.
- The backend is statically monomorphized; unlike the reference's Go target it
  does **not** wrap call arguments in a runtime `rt.Coerce`. The reference's
  `coerceArg` / `coerceReturnExprT` (and the "func-typed params skip coerceArg"
  carve-out) have no runtime analogue here — the parity obligation reduces to
  "emit the reassignment argument verbatim, do not wrap it" (§2 → §5 coercion).

## 3. Design

### 3.1 Detection — parse once into a typed result

Add a pass in `sky_lower` computed **once** per function and consumed by the
rewrite — never re-scanned ad hoc:

```rust
/// Result of the tail-recursion analysis for one `Func`. Computed once; the
/// rewrite consumes it. Making the two outcomes distinct constructors keeps the
/// "should we TCO?" decision a value, not a re-derivable predicate.
enum TailRecursion {
    /// No self-call, or at least one self-call in non-tail position → leave the
    /// body as ordinary recursion (Limitation #8, O(N) stack, documented).
    NotTailRecursive,
    /// Every self-call is a tail-position call with the correct arity, and there
    /// is ≥ 1 of them → safe to rewrite to a loop.
    TailRecursive,
}

fn analyze_tail_recursion(self_id: FuncId, arity: usize, body: &Expr) -> TailRecursion;
```

Semantics mirror the reference's `isTailRecursive`
(`TailCallOpt.hs:44-53`): `tail_self_calls > 0 && non_tail_self_calls == 0`.

**Tail position in ipê IR** (the `inTail = true` propagators, mirroring the
reference's `walk True` arms at `TailCallOpt.hs:67-78`):

- the function body's trailing expression;
- `If.then_` and `If.else_` (never `If.cond`);
- every `Match` arm body (never the scrutinee);
- `Let.body` and `Destructure.body` (never their `value`).

**Every other descent is non-tail** (`walk False`): `Call` arguments, `Apply`
func + args, `BinOp` operands, `Cons`, `List` items, `Record`/`Update` fields,
`Access` target, `Tuple` elements, and — critically — **`Lambda.body`**. Entering
a `Lambda` flips `inTail` to false so a self-call inside a closure is counted as
non-tail (it disqualifies TCO); see §5.

**Task recursion is out of scope for v1.** `TaskSeq` (`ir.rs:796-799`) sequences
effects; recursion through `task_and_then` does not consume Rust stack the way
value recursion does, and mixing a `loop`/`continue` jump with the async future
chain is a separate design. The analysis treats `TaskSeq.rest` as non-tail for
now (conservative: a Task-recursive fn simply is not TCO'd, exactly today's
behaviour — no regression).

A qualifying tail self-call is `Expr::Call { callee: Callee::Func(self_id), args }`
with `args.len() == arity`. Any `Callee::Func(self_id)` at the wrong arity, or as
a `FuncValue` (a first-class reference, `ir.rs:786-789`) rather than a `Call`, is
**not** a jump and does not qualify (it is a genuine escape of the function
value; the loop must not touch it — see §5).

### 3.2 Emission — Rust `loop` with `return` terminals and `continue` jumps

Two new IR nodes (§3.3) drive emission. `emit_func` checks whether the body is a
`TailLoop`; when it is, the emitted shape is:

```rust
pub fn foldl<...>(f: ..., acc: ..., xs: ...) -> R {
    let mut f = f;
    let mut acc = acc;
    let mut xs = xs;
    loop {
        // body emitted in TAIL/STATEMENT mode:
        //   every leaf tail position  ->  return <expr>;
        //   every tail self-call      ->  <simultaneous reassign>; continue;
    }
}
```

**Terminal shape: `return`, not `break`-with-value.** Each leaf tail position
emits `return <expr>;` (a direct mirror of the reference's `GoForever` + `return`
at the "TCO mechanism" contract). Every path inside the loop ends in either
`return` or `continue`, so the `loop { … }` never falls through — it types as `!`
and unifies with any `-> R`. This avoids the `break value` route's
loop-as-expression typing subtleties entirely and keeps the fn's public
signature byte-identical to the non-TCO form (important for `FuncValue` boxing and
trait-object slots).

**Parameter mutability by shadow, not by signature.** The signature stays
`fn foldl(f: …, acc: …, xs: …)`; mutability is introduced with a local
`let mut p = p;` shadow per parameter before the `loop`. This localizes `mut`,
keeps `emit_func`'s parameter rendering (`emit_expr.rs:3247-3254`) untouched, and
does not perturb the boxed-`Fn` coercion of a function passed by value elsewhere.

**Tail-context emitter.** Add `emit_expr_tail` (statement mode) recursing through
the tail propagators: an `If` emits `if <cond> { <tail then> } else { <tail
else> }` where each branch is itself emitted in tail mode; a `Match` emits arms
whose bodies are tail-mode; `Let`/`Destructure` emit the binding then tail-emit
the body; a leaf emits `return <expr>;`; a `TailRecur` emits the jump. This is the
statement/value split the reference expresses with `walk True` returning Go
`return`/`continue` statements.

**Arg-swap clobber foreclosure.** A tail call `foldl f (fn x acc) rest`
reassigns three params; a call like `swap a b = swap b a` would clobber under
naive sequential assignment (`a = b; b = a;` loses the old `a`). The jump
therefore binds **all** new-argument values to fresh temporaries first (reading
the *current* params), then assigns:

```rust
{
    let __tco_0 = <arg0>;   // reads current params
    let __tco_1 = <arg1>;
    let __tco_2 = <arg2>;
    f   = __tco_0;
    acc = __tco_1;
    xs  = __tco_2;
    continue;
}
```

The read phase strictly precedes the write phase, so no reassignment can observe
a half-updated parameter set. This is the direct foreclosure of the swap-clobber
trap.

### 3.3 IR changes — typed nodes, not a stringly sentinel

The reference transports the jump as a **sentinel** `Can.Call` whose callee is
`VarKernel "__tco_jump__" "<param-names-joined-by-0x1F>"`
(`TailCallOpt.hs:139-147, 162-185`), recovering the param list by splitting the
marker string in the lowerer. ipê **does not** adopt the stringly marker. Per
MAKE-INVALID-STATES-UNREPRESENTABLE, add two typed nodes to `Expr`
(`ir.rs:631`):

```rust
/// A tail-recursive function body wrapped for loop emission. Produced ONLY by
/// the lowerer's TCO rewrite; `params` are the enclosing `Func`'s parameters
/// (name + type) so emission can shadow them `let mut`. Invariant: contains ≥ 1
/// `TailRecur` in tail position and no self-`Call` remains.
TailLoop { params: Vec<(Symbol, IrType)>, body: Box<Self> },

/// A tail self-call rewritten to a loop jump. `args` are the next-iteration
/// argument expressions (one per enclosing `TailLoop` parameter, same order).
/// Invariant: appears ONLY in tail position inside a `TailLoop`, and
/// `args.len() == TailLoop.params.len()`.
TailRecur { args: Vec<Self> },
```

Rationale for two nodes over the reference's one-marker approach:

- The jump carries its arguments as real `Expr`s, not a re-parsed delimited
  string. No `0x1F`-split, no way to desync the param-name list from the args.
- The lowerer is the single producer; the two invariants above are established by
  construction at rewrite time.

**Fail-closed, never a wildcard swallow.** A `TailRecur` reached by the ordinary
(non-tail) `emit_expr_at` path is a structural impossibility if the rewrite is
correct; the emitter treats it as a `CompilerBug` **diagnostic** (an `Err`
`Diagnostic` via the existing `bug(...)` helper at `lower.rs:44`), not a
`panic!`/`unreachable!` and not a `_ => …` catch-all. The two new variants get
**explicit** match arms in every `Expr` walker — do not lean on any `_ =>`:
`emit_expr_at` (`emit_expr.rs:2187+`), the `expr_uses_*_kernel` walkers
(`lower.rs:294/365/435/494/540/586/639`), and any exhaustiveness/visit site over
`Expr`. This satisfies the project's "new AST/IR nodes require explicit walker
arms" non-regression rule.

### 3.4 Where it hooks

In `lower_def`, Typed arm (`lower.rs:1200-1224`), after `lowered_body` is built
and the prologue folded in:

```rust
let arity = params.len();
if let TailRecursion::TailRecursive =
    analyze_tail_recursion(id, arity, &lowered_body)
{
    lowered_body = rewrite_tail_calls(id, arity, params.clone(), lowered_body);
    // rewrite_tail_calls returns Expr::TailLoop { params, body: <rewritten> }
}
```

`rewrite_tail_calls` mirrors the reference's `rewriteTailCalls`
(`TailCallOpt.hs:155-185`): walk with `inTail` tracking, replace each qualifying
tail self-`Call` with `TailRecur { args }`, leave every non-tail subterm
untouched, and finally wrap the whole body in `TailLoop { params, body }`.
`analyze_tail_recursion` must have already proven no non-tail self-call survives,
so the rewrite cannot strand a self-`Call` outside the loop.

## 4. Scope

- **Self-recursion only**, keyed on `FuncId`. Mutual recursion is out (a call to a
  *different* `FuncId` is never a jump) — same boundary as the reference
  (`TailCallOpt.hs:19-23, 42-43`).
- **Non-tail recursion is left as-is**: ordinary Rust recursion, O(N) stack, per
  Limitation #8. `map`/`filter`/`foldr`/`length`/`concat`/`take`/`append`/`range`/
  `zip`/`concatMap` continue to emit as today.
- **Task-returning recursion excluded** in v1 (§3.1).

## 5. Soundness edges and how the design forecloses each

1. **Arg-swap clobber** (`f(b, a)` reassigning params in place). Foreclosed by the
   temporaries-first jump shape (§3.2): all new-argument values are bound before
   any parameter is written.

2. **Coercion mismatch with the reference.** The reference wraps non-func args in
   `coerceArg` and returns via `coerceReturnExprT`, skipping func-typed params
   (its `eraseTypeParams` would rewrite `T1 → any`). ipê is statically
   monomorphized and never runtime-coerces call args, so the parity obligation is
   simply *emit the reassignment argument verbatim*. A reassignment `p = <arg>`
   type-checks because `<arg>` already has `p`'s monomorphic type by construction;
   a func-typed param (`Box<dyn Fn(..) -> R>`) is moved/rebound, never wrapped —
   the "skip coercion on func params" carve-out is satisfied trivially.

3. **Closure-shadow false positive** (a self-call *inside* a lambda is not a tail
   call of the outer fn). Foreclosed two ways: (a) the analysis flips `inTail` to
   false on entering `Lambda.body`, so such a call is counted as non-tail and
   disqualifies the whole function; (b) ipê matches self-calls on `FuncId`, so a
   *local* binding that textually shadows the function name can never resolve to
   `Callee::Func(self_id)` — the name→id resolution already happened in canon.
   This is strictly stronger than the reference's name-based match
   (`TailCallOpt.hs:85-87`).

4. **Let/Destructure binding the function's name.** Same `FuncId` argument as (3):
   a shadowing local cannot forge a `Callee::Func(self_id)` call, so it cannot
   produce a spurious jump, and a genuine self-call keeps its real id even under a
   textual shadow.

5. **First-class self-reference escape** (`FuncValue { callee: Func(self_id) }`,
   or a `Callee::Func(self_id)` used as an `Apply` target). This is not a
   `Call`-in-tail-position and is left untouched; the emitted `fn` item still
   exists (the loop is *inside* it), so the escaped value stays valid. TCO never
   rewrites it.

6. **Borrow / move in the Rust loop.** Parameters are owned and re-bound `let
   mut`. Reassignment moves owned values (a `Vec` `xs = rest`, an `i64` `acc =
   acc + 1`) — sound because the temporaries hold them until the writes. Function
   parameters are called by reference (`emit_apply` → `(f)(x, acc)` is
   `Fn::call(&f, …)`, non-consuming, `emit_expr.rs:3081-3096`), so the standard
   combinator shape (`foldl`/`find`/`any`/`all`/`member`) reuses `f` every
   iteration without a move. The residual case — a **value**-typed parameter used
   by-move in one argument *and* read by a later argument of the same jump — is
   the *same* multi-use a normal (non-TCO) recursive call would face, and it is
   resolved by whatever clone-on-multi-use the ordinary argument-emission path
   already applies (cf. the targeted `DictGet` clone at `emit_expr.rs:2344-2349`).
   TCO introduces no new move class here; the temporaries only *tighten* ordering.
   Flagged for the implementer: add a focused test with a value param double-used
   across jump args (§6) and, if it does not compile, the fix belongs in the
   shared argument-emission path, not in a TCO-local hack.

No `IrType::Generic` fallback, no `Ty::Var`-style widening, and no wildcard match
arm is introduced anywhere in this design.

## 6. Test plan

Regression-first (the failing test is the discovery artefact):

1. **Constant-stack regression (the soundness proof).** A hand-written
   `count n acc = if n == 0 then acc else count (n - 1) (acc + 1)` (or `foldl (+)
   0` over a `1_000_000`-element list) built and *run*: pre-fix it `SIGABRT`s on
   stack overflow; post-fix it exits 0 with the correct value. Assert the spawned
   binary's exit status and stdout. Pair with a stack-size-limited run (small
   `RUST_MIN_STACK` / a spawned thread with a small stack) to make the overflow
   deterministic without needing 10^6 frames.
2. **Emission golden (positive).** The TCO'd function's emitted Rust contains
   `loop {` and `continue`, a `let mut` shadow per parameter, and **no**
   self-recursive `Call` to its own name.
3. **Emission golden (negative).** `map` / `foldr` emit ordinary recursion — no
   `loop {` — confirming non-tail recursion is untouched.
4. **Closure-shadow negative.** A function whose only self-call sits inside a
   `Lambda` body is **not** rewritten (no `loop {`), proving the `inTail` flip.
5. **Arg-swap foreclosure.** A `swap`-style tail call that flips two parameters
   (`go a b xs = case xs of [] -> (a, b); _ :: rest -> go b a rest`) produces the
   correct final values, exercising the temporaries-first jump.
6. **Value-param double-use.** A jump argument that reads a value param moved by
   another jump argument (§5 edge 6) — compiles and yields the correct value.
7. **Go-parity.** For each rewritten example, the produced value is byte-identical
   to the Go oracle golden (TCO must be value-preserving, not merely
   non-crashing). Fold the check into the standard oracle byte-diff sweep.
8. **Analysis units.** `analyze_tail_recursion` returns `TailRecursive` for
   `count` / `foldl`, `NotTailRecursive` for `map` / `foldr` and for the
   lambda-embedded self-call.

## 7. Roadmap timing and parallel-safety

This lands in the **pre-sweep pull-early window**, together with the exit0
registry migration, because the pre-sweep example set includes large-list
programs whose correctness depends on constant-stack `foldl`/`find`/etc. The two
efforts are **parallel-safe**: their file sets are disjoint in logic —

- TCO: `crates/sky_ir/src/ir.rs` (+2 `Expr` variants),
  `crates/sky_lower/src/lower.rs` (analysis + rewrite, hooked in `lower_def`),
  `crates/sky_backend_rust/src/emit_expr.rs` (`emit_func` check + `emit_expr_tail`
  + `TailLoop`/`TailRecur` arms).
- exit0 registry migration: `sky_canon` / `sky_types` (canon + registry keying).

The one coordination point is `ir.rs`: adding two `Expr` variants means every
`match` over `Expr` — including any the registry work introduces or edits — must
gain the two explicit arms rather than a `_ =>` swallow. Sequence the IR-node
addition first (or land it as a shared prerequisite) so both branches compile
against the widened `Expr`, and neither is tempted to paper over the new variants
with a wildcard.

## References

- Reference implementation: `Sky.Build.TailCallOpt` — `isTailRecursive`,
  `countTailSelfCalls`, `countNonTailSelfCalls`, `rewriteTailCalls`. ipê mirrors
  its detection/rewrite mechanism and improves the jump transport (typed IR nodes
  vs. a stringly kernel-name sentinel) and the self-call identity (`FuncId` vs.
  `(module, name)`).
- Panic classification (confirming the unclassifiable-abort hole):
  `runtime/src/sky_runtime/core.rs:479-488` (`classify_panic`),
  `core.rs:582-591` (`install_panic_classifier`).

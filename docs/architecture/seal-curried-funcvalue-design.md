# Seal hole #121 — first-class curried named fn: def-arity emission vs flattened `Fun` slot

> **Status:** design (Design Lane, 2026-07-04). READ-ONLY study; no crate code
> written. **Seal-touching** (`skyc` exit-0 MUST imply `cargo` exit-0) →
> **Opus adversarial review before commit** (same protocol as
> `seal-noncopy-move-design.md` §7).
>
> **Verified on HEAD** with the 2026-07-04 `skyc` (release, target-2): four
> /tmp repros emitted + cargo-built; error codes below are actual rustc output,
> not conjecture. Repro sources reproduced in §4 (fixtures).

---

## 0. The hole in one paragraph

A named fn defined with fewer parameter patterns than its annotation has arrows
(`mk : String -> (String -> Page)`, `mk s = \t -> Home s t`) emits at
**definition arity** (`fn main_mk(s: String) -> Box<dyn Fn(String) -> MainPage …>`,
via `split_typed_sig` peeling one arrow per pattern). But a **bare first-class
reference** to `mk` (let-stored, passed to a HOF) reifies as
`Expr::FuncValue { ty }` where `ty` is the solved region type **flattened** to
`Fun([Str, Str], Page)` — emitting
`let __sky_fn: Box<dyn Fn(String, String) -> MainPage + Send + 'static> = Box::new(main_mk)`
→ **E0593** ("function is expected to take 2 arguments, but it takes 1").
The same repro also fails with **E0507**: the curried def's own body
`Box::new(move |t| MainPage::Home(s, t))` consumes captured non-`Copy` `s`
inside an `Fn` closure. The E0507/E0525 sub-class is **general** — any source
lambda capturing and consuming a non-`Copy` binder fails (verified,
§1.4) — and is the "Fn/FnOnce + borrow" residue named in #121/#108-round-5.
Both are exit-0-cargo-fail seal violations. Fixtures use a plain `Page` ADT
with **no Live routes** (the #108 round-5 no-routes control shape).

---

## 1. Ground truth (file:line, all verified on HEAD)

### 1.1 Flatten sites — arrows → one multi-param `IrType::Fun`

| Site | File:line | What flattens |
|---|---|---|
| Solved-type path | `crates/sky_lower/src/lower.rs:3138-3147` (`ir_type_from_ty`, `Ty::Fun` arm) | curried `Ty::Fun` chain → `Fun([T0,…], R)` |
| JSON-aware variant | `lower.rs:3209-3218` (`ir_type_from_ty_json`) | same, `Ty::Var` → `Json` |
| Annotation path | `lower.rs:2535-2544` (`ir_type_from_canon`, `canon::Type::Lambda` arm) | curried annotation → `Fun([T0,…], R)` |
| Rendering | `crates/sky_backend_rust/src/emit_types.rs:214` | `Fun` → `Box<dyn Fn({params}) -> {ret} + Send + 'static>` |

### 1.2 Definition-arity emission — one arrow peeled per pattern

- `lower.rs:2195-2231` `split_typed_sig` — peels the annotation one
  `Type::Lambda` step **per parameter pattern**; the residual arrows flatten
  into the RETURN type (so `mk s = …` emits `fn(String) -> Box<dyn Fn(String) -> Page …>`).
- `lower.rs:2156-2181` `split_unannotated_sig` — same, over solved `Ty`.
- `lower.rs:5112-5125` `callee_arity(Callee::Func)` — **`patterns.len()` from
  the canon def**. All call-site reshaping keys off this def-arity:
  - exact → direct `Expr::Call` (`lower.rs:3914-3917`)
  - partial → `eta_expand_partial` (`lower.rs:3985-4048`) — captures supplied
    args **inline in the closure body**
  - over → `saturate_over` (`lower.rs:4072-4097`) — `(main_mk(a))(b)`, one
    `Apply` (sound because the flattened return `Fun` takes all the rest).
- `lower.rs:2604-2684` `lower_lambda` — collapses directly-nested lambdas into
  ONE flattened closure (the lambda-side twin of the invariant; comment at
  2600-2602 documents exactly the curried-vs-flattened cargo rejection).

### 1.3 Reify site — the bug

- `lower.rs:3418-3500` (`VarTopLevel`/`VarKernel` value-reference arm).
  **`lower.rs:3474-3475`**: `ty_ir` is the FLATTENED region type;
  `Ok(Expr::FuncValue { callee, ty: fun })` — **no def-arity check**.
- `crates/sky_backend_rust/src/emit_expr.rs:3218` dispatch →
  **`emit_expr.rs:4248-4273` `emit_func_value`**:
  `{ let __sky_fn: <flattened ty> = Box::new(<name>); __sky_fn }`.
  When def-arity < flattened arity: **E0593** (rustc: "required for the cast
  from `Box<fn(String) -> Box<dyn Fn(String) -> MainPage + Send> {main_mk}>` to
  `Box<dyn Fn(String, String) -> MainPage + Send>`"). Verified twice in repro A
  (let-stored and arg-passed positions).

### 1.4 The E0507/E0525 companion — captured-consume in `Fn` closures

- `emit_expr.rs:3040` — `Expr::Var(sym) => ctx.emit_ident(*sym)`: bare move,
  no clone pass (confirmed by `seal-noncopy-move-design.md` §1.2 — "there is
  no move-safety pass today").
- `emit_expr.rs:4280-4304` `emit_lambda_unboxed` / `4314-4325` `emit_lambda`
  — `Box::new(move |…| { body })`: captured binders consumed by the by-value
  body → closure is `FnOnce`, slot wants `Fn`.
- Verified failures (all skyc exit-0):
  - **repro A** (curried def body `\t -> Home s t`): `E0507: cannot move out
    of 's', a captured variable in an 'Fn' closure` at the def itself.
  - **repro B** (partial application `mk s` with non-`Copy` `s` — eta lambda
    `move |eta_0| main_mk(s, eta_0)`): `E0525: expected a closure that
    implements the 'Fn' trait, but this closure only implements 'FnOnce'`.
  - **repro C** (ordinary lambda `\x -> String.append prefix x` capturing a
    non-`Copy` fn param — no currying anywhere): `E0525`. **This is the
    everyday shape** and the 06-json class (06-json itself is currently
    blocked earlier by SKY-L0106 untyped top-level fns; once annotated or
    L0106 lifts, its decoder/format helpers land here).
  - **repro D control** (captured `Box<dyn Fn>` CALLED inside a lambda,
    `\x -> f x`): **builds and runs green today** — `Fn::call` borrows;
    function-typed captures in callee position need no clone and MUST NOT be
    cloned (`Box<dyn Fn>` is not `Clone`).
- `lower.rs:6280` — #89 Fix-C decoder **thunk** builds an `Expr::Lambda`
  directly (bypassing `lower_lambda`); its body has the same captured-consume
  exposure when the thunked decoder expression reads non-`Copy` locals.

### 1.5 Existing machinery to reuse

- `lower.rs:1116-1260` `rewrite_var_to_apply` + `lower.rs:1097` `pat_binds_symbol`
  — the exact shadow-aware free-`Var` IR-rewrite pattern the clone rewrite mirrors.
- `lower.rs:1318,1476,1567` `eta_params` pool — precedent for a synthetic-name
  pool (`__sky_cap_{i}` mirrors it).
- `lower.rs:268` `ir_contains_fun` — precedent for an `IrType` classifier.
- `crates/sky_ir/src/ir.rs:325/373` `BoundSet::{with_clone,has_clone}` +
  `emit_expr.rs:4378` — Clone bounds on generics already render.
- Runtime `curry1..curry10` (`runtime/src/sky_runtime/json.rs:799-1210`) —
  **scoped to the `decode_succeed` boundary** (`emit_expr.rs:2900-2969`);
  returns `Box<dyn Fn() -> Box<dyn FnOnce(A1) -> …>>` factory chains — NOT the
  `Fun`-slot calling convention.
- Upstream reference (`/home/arthur/Documentos/comp/sky`):
  `src/Sky/Generate/Rust/Builder/ModuleEmitter.hs:619-632` `defToRustItem`
  merges a lambda body's params into the def (peeling arrows via `peelArrows`,
  360-363) — upstream emits curried defs **uncurried/flat** and reshapes at
  call sites. Upstream's own fixture: `runtime-rust/tests/sky/44-curried-return`.

---

## 2. The decision — ONE arrow representation, adapters at reification

**Decision: `IrType::Fun` stays FLATTENED everywhere (status quo reaffirmed).
The single invariant added: _every `Expr::FuncValue` is arity-exact — its
callee's def-arity equals its flattened `Fun` param count — enforced at the
reify site by ETA-EXPANDING AT LOWER when def-arity < flattened arity._**
Companion soundness rule (required by the eta lambdas and owed to this issue's
"Fn/FnOnce + borrow" half): **captured non-`Copy`, Clone-renderable binders
consumed inside a closure body are cloned per call via a new `Expr::CloneVar`;
non-Clone captures outside callee position fail closed (new SKY-L0124).**

The eta adapter for `mk` (def-arity k=1, flattened N=2):

```rust
// instead of: Box::new(main_mk)                      → E0593
Box::new(move |eta_0: String, eta_1: String| -> MainPage {
    (main_mk(eta_0))(eta_1)                           // Call(first k) + ONE Apply(rest)
})
```

No captures → no clone interaction. One `Apply` for ALL residual args is
correct because `split_typed_sig` flattens the def's residual arrows into one
`Fun` return (`lower.rs:2230`) — the same soundness argument `saturate_over`
already documents (`lower.rs:4050-4056`).

### Why this option

| Option | Verdict | Reason |
|---|---|---|
| **Eta-expand at lower (reify-site adapter)** | **CHOSEN** | Zero change to the calling convention, `render_type`, `emit_apply`, arity gates, or goldens (no existing golden references a curried def bare — checked `tests/golden/m1_partial` exercises `over` only over-applied). Reuses `eta_params` + `Lambda` emission. Restores an invariant every downstream consumer already assumes. |
| curry{n}-wrap at reification | REJECTED | `curryN` returns `Box<dyn Fn() -> Box<dyn FnOnce(A1)->…>>` factory chains — type-incompatible with every `Fun` slot and with `emit_apply`'s `(f)(a, b)` (`emit_expr.rs:4212-4227`). Would fork the calling convention; curry{n} stays confined to `decode_succeed` (#89). |
| No-flatten `IrType` for fn-typed positions | REJECTED (for #121) | That is #90 **Stage-2**, design-only, deliberately scoped to ctor-payload slots (`ctor-payload-function-design.md` §3, lines 232-252). Globalising it rewrites `render_type`/`Apply`/partial-app gates and breaks upstream parity (upstream itself emits FLAT defs — ModuleEmitter.hs:619). |
| Uncurry at definition (upstream `defToRustItem` merge) | DEFERRED follow-up, not required for green | Fixes only lambda-bodied defs (upstream's guard `Ann.At _ (Can.Lambda …)`); a curried def whose body is a partial application still needs the reify adapter; requires `callee_arity` (canon `patterns.len()`, `lower.rs:5122`) and `lower_def` to change in lock-step; churns goldens (m1_partial). Value: fewer boxes/clones + upstream emission parity. File as a separate quality issue after #121 is green. |

### Coherence with #89 / #90 (the review's overlap warning)

- **#89 (`decode_succeed` curry path)** — `emit_expr.rs:2944-2956` Case-1 reads
  `n` from `FuncValue.ty`'s param count and emits `curry{n}(fn_name)`. Today a
  curried named ctor fn there is the same E0593 inside `curryN`'s `F: Fn(A1,…)`
  bound. Post-fix, FuncValue is arity-exact by construction → Case-1 stays
  correct unchanged; curried refs arrive as the eta `Lambda` → existing Case-2
  (`curry{n}(move |…| …)`, `emit_expr.rs:2958-2969`) handles them. Fix-C thunks
  (`lower.rs:6280`) get the same capture-clone rewrite applied (step T3).
  **No #89 regression; one new fixture proves it (§4 F7).**
- **#90 Stage-2 (no-flatten in ctor payload slots)** — served, not blocked:
  Stage-2's per-slot `FnOnce`-chain conversion becomes a **local adapter at
  construction sites over an always-arity-exact flattened value**; it never
  has to reason about def-arity. The arrow representation decision stays ONE
  (flattened), with two sanctioned boundary adapters: `decode_succeed`
  (shipped, #89) and ctor payload slots (future, #90 Stage-2).
- Diagnostic numbering per `design-coherence-review.md` C1: L0121 = #94
  (shipped), L0122 reserved #90, L0123 reserved #108 → **this design takes
  SKY-L0124** (`Feature::NonCloneCapture`).

---

## 3. The capture-clone rule (precise)

New IR node — `crates/sky_ir/src/ir.rs` next to `Var` (923):

```rust
/// A read of a CAPTURED binder inside a closure body that must not consume
/// the capture: renders as `{ident}.clone()`. Produced only by the lowerer's
/// capture-clone rewrite (lower_lambda / eta_expand_partial / Fix-C thunk);
/// never for `Copy`-classed or function-typed captures.
CloneVar(Symbol),
```

Classification — one function, next to `ir_contains_fun` (`lower.rs:268`):

```rust
enum CloneClass { CopyLeaf, CloneOk, NonClone }
fn clone_class(t: &IrType) -> CloneClass
```

- `CopyLeaf`: `Int | Float | Bool | Char | Unit` → leave bare (moves are copies;
  keeps existing goldens byte-identical, e.g. m1_partial's `move |b| (a + b)`).
- `NonClone`: `Fun(..)` and any type with `ir_contains_fun`; `Task(_)`;
  `Decoder(_)`; `Cmd(_)`/`Sub(_)`; Html/Ui/server opaques that do not derive
  `Clone` in the runtime (implementer verifies each against
  `runtime/src/sky_runtime` derives; when unsure → `NonClone`, fail-closed is
  the safe side).
- `CloneOk`: everything else (`Str, Bytes, List, Dict, Set, Tuple, Record,
  Enum, Maybe, Result, Json, Db(Arc-backed — verify)`). `Generic(_)`:
  `CloneOk` **plus** inject `with_clone()` into that type-param's `BoundSet`
  (`ir.rs:325`) for the enclosing def (T5); until T5 lands treat as `NonClone`.

Rewrite rule (mirrors `rewrite_var_to_apply`, `lower.rs:1116-1260`, incl.
`pat_binds_symbol` shadow discipline): inside a closure body, for each
**captured** symbol (free in the body — not bound by the closure's params or
any inner `Let`/`Destructure`/`Match`-arm/`Lambda` binder):

1. `CopyLeaf` → bare `Var` (unchanged).
2. `CloneOk` → every unshadowed read becomes `CloneVar`.
3. `NonClone` in **`Apply`-callee position** → bare `Var` (verified green today
   — repro D; `Fn::call` borrows).
4. `NonClone` anywhere else → `Err(unsupported(lambda_span, Feature::NonCloneCapture))`
   — SKY-L0124, converting today's cargo-fail into a skyc-time diagnostic.
   (Message: "a function/task/decoder value captured by a closure can only be
   called, not forwarded; bind the result outside the closure or wrap the
   forwarding in a named top-level function".)

Capture types come from the **canon body walk**: collect `canon::Expr_::VarLocal`
occurrences not bound within the lambda, and read each use-site's solved region
type (`self.types.regions[span]` → `ir_type_from_ty`, missing region → bare,
status-quo). Note the residual conservatism: a `CloneOk` capture whose only
reads are borrow-like still clones (over-clone, never unsound); #104's last-use
pass subsumes this later — the two compose (this rule is scoped to closure
bodies; #104 is straight-line reuse).

### Eta-lambda capture hoist (also fixes a latent semantics divergence)

`eta_expand_partial` currently inlines supplied arg EXPRESSIONS into the
closure body (`lower.rs:4022,4036-4042`) — so `mk (expensive x)` re-evaluates
`expensive x` **per call** of the partial (Sky/Go evaluates once). New shape:

- literal args (`Int/Float/Bool/Str/Unit`) → inline (goldens unchanged);
- `Var` args → inline, classed per the rule above (bare / `CloneVar` / L0124);
- any other expr → hoist: `Let { __sky_cap_i = <arg>, body: Lambda(… CloneVar(__sky_cap_i) …) }`
  (bare if `CopyLeaf`), with a `cap_params` pool mirroring `eta_params`
  (`lower.rs:1318,1476,1567`). Evaluate-once restored + E0525 closed.

---

## 4. Red→green fixtures (`tests/golden/i121_*`, harness mirrors `golden_i101_color_seal.rs`)

All red states verified by actual emission + cargo on HEAD (2026-07-04).

| # | Fixture | Shape | Red today | Green after |
|---|---|---|---|---|
| F1 | `i121_firstclass_curried` | `mk : String -> (String -> Page)`, `mk s = \t -> Home s t`; `let g = mk in g "x" "y"` AND `apply2 mk` with `apply2 : (String -> String -> Page) -> Page`; plain `Page` ADT, no routes | E0593 ×2 (`main.rs` let-store + arg positions) + E0507 (def body) | T1-T3 (E0507) + T6 (E0593) |
| F2 | `i121_firstclass_arity0` | `handler : String -> Page`, `handler = mk "x"` (nullary def, fn-typed value) referenced bare + applied | E0593 class (def-arity 0 vs `Fun([Str],Page)`) | T6 (adapter formula covers k=0: `(main_handler())(p0)`) |
| F3 | `i121_partial_noncopy` | `mk : String -> String -> String`, `mk a b = a ++ b`; `let s = String.append "he" "llo" in let f = mk s in f "!"` | E0525 (verified) | T4 |
| F4 | `i121_lambda_capture_noncopy` | `tag prefix items = List.map (\x -> String.append prefix x) items` | E0525 (verified) | T3 |
| F5 | `i121_capture_fn_called` (control) | `let f = add 1 in List.map (\x -> f x) xs` | **green today** (verified) — MUST stay green, byte-stable (no clone on `Fun` callee) | — |
| F6 | `i121_capture_fn_forwarded` (gate) | `\x -> applyTwice f x` capturing fn-typed `f` | cargo-fail today | **skyc-fail SKY-L0124** (never cargo-fail) |
| F7 | `i121_succeed_curried` (#89 boundary) | `mkLabel : String -> (Int -> String)`, `mkLabel n = \a -> …`; `JsonDec.succeed mkLabel |> required …` | E0593 inside `curry2` bound | T6 (arrives as Lambda → Case-2) |
| F8 | `i121_curried_three_arrows` | `mk3 : A -> B -> C -> D`, `mk3 a = \b -> \c -> …` bare ref + applied | E0593 | T6 — adapter `(main_mk3(p0))(p1, p2)` (ONE Apply) |
| F9 | `i121_decoder_thunk_capture` (#89 Fix-C boundary) | let-bound decoder built from a non-`Copy` LOCAL (`let field = … in let d = required field … in` decode twice) | E0525 in thunk body | T3 applied at `lower.rs:6280` |
| F10 | `i121_generic_curried` | `pairWith : a -> b -> (a, b)` def-arity 1, used first-class at concrete type | E0593 (+capture) | T5+T6 |
| F11 | shadow control (inside F4) | inner `let prefix = … in` rebinding the captured name — shadowed reads must stay bare | n/a (correctness of rewrite) | T3 |

Each fixture: `Main.sky` + byte-golden `main.rs` + `expected` stdout +
`oracle.meta`; E2E gated by `SKY_E2E=1` (per `skyc-runtime-dir-and-e2e`
memory: `SKY_RUNTIME_DIR` unset for the cargo-test harness).

---

## 5. Implementation steps (Sonnet-executable, in landing order)

**T1 — `Expr::CloneVar(Symbol)`** (`crates/sky_ir/src/ir.rs:923` vicinity).
Rustc exhaustiveness drives the walker arms — the matches have no catch-alls
(repo rule). Known arms to add (grouped with `Var`):
`sky_ir/src/pretty.rs:1169` (print `CloneVar {name}`);
`sky_lower/src/lower.rs` — `count_self_calls` leaf group (`:462-470`), the ten
`expr_uses_*` leaf-false groups (`:618,684,756,810,864,921,971,1017,1074`),
`rewrite_var_to_apply` (`:1137-1148` — add `Expr::CloneVar(_)` to the leaf
group; the thunk target is `NonClone` so a `CloneVar(target)` cannot exist),
`rewrite_in_tail` (leaf), and any `match` rustc flags;
`sky_backend_rust/src/emit_expr.rs:3040` sibling arm:
`Expr::CloneVar(sym) => Ok(format!("{}.clone()", ctx.emit_ident(*sym)?))`.
Unit test in `ir.rs` tests + a pretty-print case.

**T2 — `clone_class(&IrType) -> CloneClass`** (`lower.rs` next to
`ir_contains_fun:268`). Exhaustive over `IrType` (no `_` arm). Before writing
the `CloneOk` arms, verify each runtime type's `Clone` derive with
`rg "derive.*Clone" runtime/src/sky_runtime/<mod>.rs`; unsure → `NonClone`.
`Generic(_)` → `NonClone` in T2 (upgraded by T5). Unit-test the table.

**T3 — capture-clone rewrite in `lower_lambda`** (`lower.rs:2604-2684`, insert
after body lowering at `:2666`) + apply the same helper to the Fix-C thunk body
(`lower.rs:6280`). Two helpers:
(a) `captured_locals(&self, params, canon_body) -> DResult<Vec<(Symbol, Option<IrType>)>>`
— canon walk: `VarLocal` occurrences minus binders introduced inside (lambda
params, let names, case/destructure binders, inner lambda params); type from
`self.types.regions[use span]` via `ir_type_from_ty` (missing → `None`).
(b) `rewrite_captured_clones(clone_set, noncl_set, expr) -> DResult<Expr>` —
mirror `rewrite_var_to_apply` (`:1116-1260`) with `pat_binds_symbol` shadowing;
`clone_set` reads → `CloneVar`; `noncl_set` reads → bare when the read is the
direct `Apply.func`, else `Err(unsupported(span, Feature::NonCloneCapture))`.
Add `Feature::NonCloneCapture` → SKY-L0124 (`sky_diagnostics`: `diagnostic.rs:473`
enum + `code.rs` + `explain/SKY-L0124.md`, mirror SKY-L0121's files).
Greens F4/F9/F11; F5 byte-stable.

**T4 — eta capture hoist** (`lower.rs:3985-4048`). Extend
`eta_expand_partial` to receive the canon `args` (caller
`lower_call_uniform:3919` has them) for per-arg region types. Per supplied
arg: literal → inline; `Expr::Var` → bare/`CloneVar`/L0124 per `clone_class`;
other → hoist to `Expr::Let { __sky_cap_i … }` wrapping the `Lambda`, read via
`CloneVar` (bare if `CopyLeaf`). Add the `cap_params` pool alongside
`eta_params` (`:1318,1476,1567` — sized like eta: module's widest arity).
Greens F3. Re-verify `tests/golden/m1_partial` byte-identical (its eta args
are literals).

**T5 — Generic captures** (small, after T3): when the rewrite cloned a
`Generic(sym)`-typed capture, `lower_def` (`:1986-1990`) merges
`with_clone()` into that var's `BoundSet` before building `type_params`.
Flip `clone_class(Generic)` → `CloneOk`. Greens F10's capture half.

**T6 — the reify eta-adapter** (the headline fix; `lower.rs:3474-3475`).
Before constructing `FuncValue`:

```rust
if let IrType::Fun(params, ret) = &ty_ir {
    let def_arity = self.callee_arity(&callee)?;
    if def_arity < params.len() {
        return self.eta_adapt_funcvalue(callee, params, ret);
        // Lambda { params: eta_params[i] typed per params[i],
        //          ret: (*ret).clone(),
        //          body: Apply { func: Call(callee, vars[..def_arity]),
        //                        args: vars[def_arity..] } }
        // def_arity == 0 ⇒ func is Call(callee, []); still ONE Apply.
    }
    if def_arity > params.len() {
        return Err(bug("sky_lower::lower_expr",
            "def-arity exceeds the reference's flattened arity"));
    }
}
```

No captures → no T3 interaction. Kernels are arity-exact (their `callee_arity`
matches the native signature) → adapter never fires for them; the
`kernel_native_ir_type` fallback (`:3484-3491`) stays as-is. Greens
F1(E0593)/F2/F7/F8/F10. Post-invariant to state in `FuncValue`'s doc
(`ir.rs:1050-1058`): *a `FuncValue`'s flattened arity equals its callee's
def-arity, by construction.* Downstream simplification enabled (not required):
`emit_json_decoder_call` Case-1 (`emit_expr.rs:2944`) may assert it.

**T7 — fixtures + gate wiring**: land F1-F11 (`tests/golden/i121_*` +
`crates/skyc/tests/golden_i121_curried_seal.rs` mirroring
`golden_i101_color_seal.rs`), full `cargo test` + examples sweep. Update
`docs/architecture/parity-gap-snapshot.md` (06-json row: after L0106 lifts,
this class no longer masks) and the #121 task. No divergence-ledger entry
(observable behaviour matches Go); record ONE parity note: upstream normalises
at the definition (`ModuleEmitter.hs:619-632`), we adapt at reification —
equivalent observables; file the def-uncurry merge as a separate follow-up
quality issue (emission parity + fewer boxes/clones), explicitly NOT needed
for green.

Landing order: T1 → T2 → T3 → T4 → T5 → T6 → T7. T3/T4 are independently
green-able (F4/F9 then F3); T6 is independent of T3-T5 but its fixtures F1/F7
only go fully green with T3 in (the def-body E0507).

---

## 6. Seal flag + adversarial gate

**SEAL-TOUCHING: YES** — every fix converts an exit-0-cargo-fail into green or
into a skyc diagnostic; a mistake here mints new silent cargo failures.
Protocol: Opus adversarial review before commit; the review gate MUST actually
**emit + `cargo build` (+ run, `SKY_E2E=1`)** each shape below, not just
byte-diff:

1. F1 both positions (let-store, HOF-arg) + run output check.
2. F2 arity-0 fn-valued def (`(main_handler())(p0)` adapter).
3. F3 partial with non-`Copy` var; call the partial **twice** (per-call clone,
   not move-on-first-call).
4. F4 + F11 shadowing (clone must not touch shadowed reads); lambda invoked
   over a 2+ element list (the per-element second call is what E0525 protects).
5. F5 stays green AND byte-identical (no clone on `Fun`-callee capture).
6. F6 → SKY-L0124 at skyc time; assert cargo is never reached.
7. F7 `decode_succeed` boundary (both a curried NAMED fn and the existing
   arity-exact named fn — `curry{n}` path unregressed, m4h goldens byte-stable).
8. F8 three-arrow/one-pattern (single-`Apply` residual, NOT `(f(a))(b)(c)`).
9. F9 thunked decoder rebuilding from a cloned capture, decoded twice with
   different inputs (fresh decoder per use — #89 Fix-C unregressed).
10. F10 generic: emitted fn gains `T1: Clone` bound only when the capture is
    cloned; a capture-free generic def keeps its pre-existing bounds
    byte-identical (M2a goldens).
11. TCO interplay: a curried self-recursive def (`f : Int -> Int -> Int`,
    `f a = \b -> if b == 0 then a else f (a + 1) (b - 1)` — over-applied
    self-call) both direct-called and referenced bare — adapter + `TailLoop`
    must not strand a self-`Call` (guarded by `analyze_tail_recursion`,
    `lower.rs:340`).
12. Full 26-example sweep + `cargo test` (goldens untouched except new i121_*
    and any T4-churned eta goldens — expected: none, eta args in existing
    goldens are literals).

Residual accepted risks (documented, not silent): over-cloning of borrow-only
`CloneOk` captures (perf-only; #104 subsumes); `NonClone` misclassification
fails CLOSED (skyc diagnostic, never cargo-fail); missing region type for a
captured local degrades to today's behaviour (bare read) — flagged for the
#104 pass to sweep.

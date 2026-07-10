# T3 revised design — the `andMap` curried-payload arity gate

> **Status: IMPLEMENTED (2026-07-10, fourth landing).** T3a/T3b (Tier 2,
> `TyBounds::and_map_payload()` wired into `constrain_var_kernel`) and T3e
> (Tier 1 backstop, re-anchored inside `lower_callee`) landed together, plus
> the restored T4 lambda-param-reuse gate. Full aliasing-shape matrix (T3f)
> passes in `crates/skyc/tests/golden_l0114_ctor_payload_function.rs`,
> including the `T3.residual` cross-module-annotated-wrapper fixture (T3c) —
> **confirmed ACCEPTED, no false positive** — so **T3d is not needed**: the
> "lift bound onto annotation skolem" propagation already worked correctly
> via the same generic mechanism `Math.min`'s `Comparable` bound uses
> (`Content::Rigid` meeting `Content::Super` in `unify.rs`), with zero
> `and_map_payload`-specific code required beyond the `constrain_var_kernel`
> tie itself.
>
> **One correction to this design's own prediction, discovered empirically
> while implementing T3f**: §3.1 predicted every violation surfaces as
> `SKY-T0014`. In practice the diagnostic code depends on HOW the obligated
> variable is used, mirroring a split this compiler ALREADY has for
> `Math.min`/`Math.max` (`crates/skyc/tests/golden_m4c_math_gate.rs`, whose
> own doc comment states this explicitly: "Calling `Math.min` directly on
> two non-comparable values is the eager-pin sibling and surfaces SKY-T0001
> instead"). A DIRECT `andMap` call pins the obligated `b` straight to a
> concrete `Fun` structure at `unify.rs`'s own head-pin check
> (`super_concrete_ok`) — the "eager pin" case — surfacing a plain
> `SKY-T0001` (`TypeMismatch`). EVERY fixture in the aliasing-shape matrix
> (§4's table) calls `andMap` directly, so every one of them surfaces
> `SKY-T0001`, not `SKY-T0014`. `SKY-T0014` is reached only through an
> ANNOTATED GENERIC FORWARDER (the `SchemeApp`/`check_scheme_applications`
> path — exactly `Math.min`'s `pickMin` forwarder shape), added as its own
> fixture (`l0114_and_map_forwarder_curried_is_t0014`) to prove the
> friendly-message path genuinely exists. Both codes are equally sound —
> neither is a cargo-fail — so `assert_and_map_curried_rejected` in the
> golden test accepts `SKY-T0001` / `SKY-T0014` / `SKY-L0114` (Tier 1
> backstop) as any of the three acceptable clean-rejection outcomes. This is
> a diagnostic-quality nuance, not a soundness gap: deliberately NOT
> "fixed" by special-casing `unify.rs`'s head-pin check to defer
> `and_map_payload` violations to the nicer path for every shape, because
> doing so would diverge from the established, already-tested precedent
> every other `TyBounds` obligation in this compiler follows.
>
> **Import-alias row: confirmed genuinely not constructible in `sky-rust`**
> (not merely "expected non-issue, assumed"). `Result`/`Maybe` are entries in
> a fixed compiler-kernel-qualifier list (`crates/sky_canon/src/resolve.rs`),
> not backed by an importable Sky-source module in this milestone — there is
> no module for `import Result as R` to name. Recorded explicitly in the
> golden test's module doc comment and in `docs/divergences-from-sky.md`'s
> B22 entry rather than silently skipped.
>
> Original design-pass status note (superseded by the above, kept for
> history):

> Design only (read-only study of `crates/sky_types` and
> `crates/sky_lower`; no code written, no build run, no worktree created).
> Supersedes §3 step 3 ("Add the `andMap` call-site arity gate") and the `T3`
> row of the Lane A task table in
> [`ctor-payload-function-design.md`](./ctor-payload-function-design.md),
> which that document now links to. Written after T3 was implemented and
> reverted **three times** on 2026-07-10 (`f80f05a`/`dbd876b`,
> `39d9a57`/`73f33bc`, plus the fresh third bypass this session found before
> a fourth landing attempt) — see `BACKLOG.md`'s `#90` row for the full
> incident log. This document is the "real design pass, not another
> mechanical fix" the backlog entry demands.

---

## 0. The three bypasses, traced precisely

All three shapes reduce to the same source pattern: a `Result.andMap` /
`Maybe.andMap` reference whose *curried (arity ≥ 2) payload* only becomes
apparent at a call site that is syntactically distant from — or entirely
absent at — the literal kernel reference.

| # | Shape | Where the check lived | Why it missed |
|---|---|---|---|
| 1 | `Ok (\x y -> ...) \|> Result.andMap (Ok 1)` (direct + one pipe-desugared nesting) | AST pattern-match on 2 `Call` shapes at the call node | Any *third* syntactic shape (a `let`) wasn't one of the 2 matched patterns. |
| 2 | `let g = Result.andMap (Ok 1) in g (Ok add3)` | `lower_call_uniform`'s `VarKernel \| VarTopLevel` arm, keyed on the **resolved `Callee`** of *this* call node | `g`'s own call site resolves to `Callee::Local`/whatever a plain local var callee falls into — not `Callee::Kernel(ResultAndMap)` — so the `matches!(resolved, Callee::Kernel(..))` guard never fires. Fixed by re-anchoring the check to `andMap`'s own reference span (`region_ty(callee.span)`), which — because HM solving is one global constraint system with no generalization for a `let`-bound partial application inside the same function — already reflects the eventual concrete instantiation. |
| 3 | `myAndMap = Result.andMap` (top-level, unannotated, point-free) … later `myAndMap (Ok 1) (Ok add3)` | Same as #2 — `reject_curried_andmap_payload` called from `lower_call_uniform`'s `VarKernel \| VarTopLevel` Call-node arm only | `myAndMap (Ok 1) (Ok add3)`'s callee is `VarTopLevel(myAndMap)`, which resolves to `Callee::Func(myAndMap_id)` — a **user** function, not `Callee::Kernel(..)`. The guard skips. The kernel reference itself (`Result.andMap` inside `myAndMap`'s own body) is a **bare value reference**, lowered through the *other* `lower_expr` arm (`crates/sky_lower/src/lower.rs:5863`, `VarTopLevel { .. } \| VarKernel { .. } =>`), which never calls `lower_call_uniform` at all — so the check, wherever it lived inside `lower_call_uniform`, structurally cannot see this reference. |

Bug 2 and Bug 3 look similar but are **not** the same bug: Bug 2 was fixed by
re-anchoring the check to the kernel reference's *own* span instead of the
outer call's resolved callee. Bug 3 shows that fix was still only wired into
*one of the two* `lower_expr` arms that can produce a reference to
`Result.andMap`/`Maybe.andMap`. Two arms found so far; the risk (per the
open task) is a *third* arm nobody has hit yet (higher-order argument,
record-field extraction, import alias). §2 below establishes why that risk
is actually bounded, and §3 proposes closing it by construction rather than
by enumeration.

---

## 1. Why AST-shape / call-node matching cannot be exhaustive

The hazard is a **property of a value's solved type** (does the payload's
flattened arrow have arity > 1?), not of the **syntax** at any one
reference. Sky routes a value between its point of *origin* (the kernel
declaration) and its point of *use* (an `andMap` call fully saturating both
arguments) through an open-ended set of intermediate syntactic forms: direct
call, pipe, `let`, top-level point-free alias, function argument, record
field, tuple element, import alias, .... Every one of the three incidents
above is the *same* underlying mistake repeated: gate the check on "does
*this* AST node look like an `andMap` call/reference", which requires
**enumerating every intermediate form** a value can pass through. That
enumeration is open-ended by construction — Sky, like every ML-family
language, allows values to be aliased anywhere a value is allowed, which is
everywhere.

The fix has to stop asking "what does this syntax look like" and start
asking "what is this value's solved type, checked at every point the
compiler is *already* forced to look at that value's type regardless of the
syntax that produced it." §2 catalogues the two places in this compiler
that are already exhaustive in exactly that sense.

---

## 2. Architecture facts this design relies on (verified against HEAD)

### 2.1 `lower_callee` is *already* the single lowering-time funnel

`crates/sky_lower/src/lower.rs:8294`. Both of the only two `lower_expr`
sites that can produce a reference to a named kernel or top-level binding
call it:

* `lower_call_uniform`'s `VarKernel { .. } | VarTopLevel { .. }` arm
  (`lower.rs:6441`, direct/piped/eta-saturated application) —
  `let resolved = self.lower_callee(callee)?;`
* `lower_expr`'s bare-value arm (`lower.rs:5863`, "a top-level binding or
  kernel named as a bare *value* — passed, returned, or let-bound — rather
  than directly applied") — `let callee = self.lower_callee(e)?;`

`lower_callee`'s own doc comment on its final `_ =>` arm already states this
invariant explicitly (`lower.rs:9338-9341`, current HEAD after the revert):
*"both callers (the direct-call path in `lower_call` and the value-
reference arm in `lower_expr`) gate on `VarKernel`/`VarTopLevel` before
dispatching here"*. This was written for an unrelated invariant (that no
other callee shape reaches this function), but it is exactly the
exhaustiveness property T3 needs: **every literal AST occurrence of
`Result.andMap` / `Maybe.andMap`, in *any* syntactic position (direct
callee, piped callee, let-bound value, top-level point-free alias,
higher-order argument, record-field initializer, tuple element, re-exported
qualified name after `import … as …`), is resolved through this one
function, because canonicalisation has already collapsed all of those
positions down to two `canon::Expr_` shapes (`VarKernel` / `VarTopLevel`)
before lowering ever sees them.**

Bug 3 is not a counter-example to this — it is a demonstration of the
*previous* fix not having been placed at this actual funnel. The check was
called from *one of* `lower_callee`'s two call sites (`lower_call_uniform`),
not from inside `lower_callee` itself, so it only ever saw the callee
position of a `Call` node, never the bare-value position.

### 2.2 `region_ty` is solved-type lookup keyed by `(home, span)`, not by call node

`lower.rs:3065`. `self.types.regions.get(&(home, span))` — a flat map from
*every* region the constraint solver visited to its solved `Ty`, populated
once by `sky_types::infer` before lowering starts (global constraint
solving: "solving completes before lowering runs" per the existing doc
comment at `lower.rs:6598` (pre-revert)). Crucially this is keyed on `span`,
not on "the callee slot of a `Call` AST node" — so it answers "what is the
solved type of *this token*", identically whether that token is a call's
callee or a bare value reference, a `let` RHS, a record field, or an
argument expression. This is what makes Bug 2's fix ("peel `andMap`'s own
solved arrow off its own reference span, not the argument expressions")
generalize past `let`-binding: the fresh union-find variables created for a
same-module, monomorphic (unannotated) alias are **the same UF nodes**
across every occurrence, so whichever occurrence's span you inspect,
`region_ty` reflects the *final*, fully-solved type.

### 2.3 When does aliasing *actually* sever the connection? — `CLocal` vs. generalization

The one case where a *different* occurrence's `region_ty` can disagree with
another is genuine Hindley–Milner **let-generalization**: a binding's body
is type-checked *once*, then its free type variables are abstracted into a
scheme; each *external* reference to that binding re-instantiates the
scheme with brand-new, independent flexible variables (`instantiate_tracked`,
`crates/sky_types/src/constrain.rs:1699`, the "`CForeign`" path per its own
doc comment: *"calling `identity` at `Int` and at `Bool` in the same module
yields two independent, separately-satisfiable instantiations"*). Once that
happens, the *original* occurrence's region type (inside the generalized
binding's own body) is decoupled from any *later* external call site's
concrete instantiation — checking the original occurrence can no longer see
what the wrapper is eventually called with.

This compiler's actual generalization policy (`constrain_var_top_level`,
`constrain.rs:2203`) is narrower than textbook ML, and it matters a lot for
how exposed T3 actually is:

* **Typed (explicitly annotated) top-level binding** → real per-reference
  generalization (`instantiate_tracked`, fresh vars per external call).
* **Untyped (unannotated) binding, same-module use** → **`CLocal`
  semantics: one shared monomorphic variable.** Used at two different
  concrete types in the same module is a **hard type error**, proven by the
  existing test `untyped_polymorphic_use_at_two_types_is_rejected`
  (`crates/sky_types/src/lib.rs:2969`). This is exactly Bug 3's shape
  (`myAndMap = Result.andMap`, unannotated, used once) — under this
  semantics `Result.andMap`'s occurrence inside `myAndMap`'s body and
  `myAndMap`'s own call site share **one** UF variable, so `region_ty` at
  either span already reflects the same solved type.
* **Untyped binding, cross-module use** → **does** generalize
  (`promote_untyped_boundaries`, `constrain.rs:5606`; proven by
  `untyped_value_binding_generalizes_across_cross_module_uses`,
  `lib.rs:1521`). An unannotated wrapper around `andMap`, exported and
  imported into a different module, reused there at two different concrete
  payload arities, *is* a case where the original occurrence's region type
  cannot see the external call sites.

So the real residual is narrower than "any generalization anywhere": it is
specifically **cross-module reuse of a wrapper (annotated or not) at two or
more different concrete `andMap` payload shapes**. §3.2 below closes this
too, using a mechanism already proven for an analogous obligation
(`Math.min`'s `Comparable` bound).

### 2.4 Precedent: this compiler already has a "structural type obligation that survives generalization" mechanism

`TyBounds` (`crates/sky_types/src/ty.rs:157`) is a small bitset of
"super-type" obligations (`Number`, `Ord`, `Eq`, `SetElem`, `DictKey`,
`Show`, `Append`) attached to a union-find variable via `Content::Super`.
`constrain_var_kernel` (`constrain.rs:2292`) already uses exactly this
mechanism for a **structurally identical** problem: a specific scheme
argument slot (`Set`/`Dict`'s key, `var(0)` in every `Set`/`Dict` kernel
scheme) needs a restriction that ordinary unification cannot express
("comparable", i.e. "not a function, not a record"), and that restriction
must survive arbitrary aliasing including generalization. The working
pattern (`constrain.rs:2381-2391`):

```rust
if let Some(bound) = Self::key_obligation_for(k) {
    let ty = self.stdlib_scheme(k) /* ... */;
    let (var, vars) = self.instantiate_tracked(&ty)?;
    if let Some(&key_var) = vars.get(&0) {          // raw scheme-var id 0
        let s = self.super_var(bound, span)?;        // fresh bounded var
        self.eq(span, key_var, s);                    // tie them together
    }
    return Ok(var);
}
```

`Math.min`/`Math.max`'s doc comment confirms this survives *annotated*
generalization too: *"a generic use lifts the matching Rust trait bound onto
its annotation skolem"* — i.e. a user function `maxOf : Comparable a => ...`
(Sky spells this as a plain type variable; the compiler infers/propagates
the `Ord` bound) re-verifies the bound **at each of that function's own
external call sites**, not just once at its definition. And for
*unannotated* bindings, `promote_untyped_boundaries` explicitly **excludes**
obligation-carrying variables from quantification ("Roots still reachable
from a still-pending deferred obligation are excluded from quantification…
the existing 'single concrete use' gate fallback for these defs stays
intact", `constrain.rs:5618-5624`) — i.e. an obligated untyped binding is
*forced* to stay a single, monomorphic, concretely-checked instantiation,
even across a module boundary. Either branch — annotated-and-repropagated,
or untyped-and-forced-monomorphic — ends with the obligation checked
against a **fully concrete, solved type**, post-solve, regardless of how
many aliasing hops occurred.

---

## 3. Recommended design — two tiers, land together

### 3.1 Tier 2 (primary mechanism): a real `TyBounds` obligation on `andMap`'s payload-result slot

This is "Option 2" from the brief, made concrete using infrastructure that
already exists and is already tested for an equivalent problem (§2.4),
rather than inventing new HM machinery. It turns the hazard into a genuine
**type error** (`SKY-T0014`, the existing `SuperTypeUnsatisfied`
diagnostic family — same code Eq/Ord/Show/comparable-key violations already
use), raised at `sky_types::infer` time, strictly *before* lowering ever
runs.

**New `TyBounds` bit** (`crates/sky_types/src/ty.rs`, alongside `SET_ELEM`/
`DICT_KEY`):

```rust
const AND_MAP_PAYLOAD: u16 = 1 << 9;   // next free bit; 7 of 16 still spare

/// This variable is the RESULT of an `andMap` payload arrow (`b` in
/// `Con (a -> b)`); it must not itself be a function — a curried (arity ≥ 2)
/// `andMap` payload has no sound `FnOnce(A) -> B` lowering (see
/// docs/architecture/ctor-payload-andmap-arity-gate-design.md).
pub const fn and_map_payload() -> Self { Self(Self::AND_MAP_PAYLOAD) }
```

**Predicate** — add one line, in the SAME place every other bound's
predicate lives (`emitted_bound_satisfied` / `concrete_super_ok`,
`crates/sky_types/src/lib.rs:463`/`500`):

```rust
let not_curried_ok = !matches!(ty, Ty::Fun(_, _));
// ...
&& (!bounds.has_and_map_payload() || not_curried_ok)
```

(Deliberately **shallow** — `!matches!(ty, Ty::Fun(_,_))`, not the deep
`ty_is_equatable`-style "no function anywhere nested" check used for
`Eq`/`Show`. `Result e (List (Int -> Int))` is a *different*, already-gated
hazard — "collections of functions", §2 of the parent doc's hazard table —
unrelated to `andMap`'s arity restriction, which only cares whether `b`
*itself* is an arrow.)

**Wire it into `constrain_var_kernel`**, mirroring `key_obligation_for`
exactly, keyed on the payload-result slot (`var(1)` in both schemes — see
`constrain.rs:3520` "`var(0)=a, var(1)=b`" and `constrain.rs:3601` "same"):

```rust
if matches!(k, StdlibKernel::MaybeAndMap | StdlibKernel::ResultAndMap) {
    let ty = self.stdlib_scheme(k) /* existing MaybeAndMap/ResultAndMap fun(...) */;
    let (var, vars) = self.instantiate_tracked(&ty)?;
    if let Some(&payload_result_var) = vars.get(&1) {   // raw scheme-var id 1 = `b`
        let s = self.super_var(TyBounds::and_map_payload(), span)?;
        self.eq(span, payload_result_var, s)?;
    }
    return Ok(var);
}
```

(Exact placement: alongside the existing `key_obligation_for` block,
`constrain.rs:2381-2391` — same shape, different kernel family and scheme
slot. If `MaybeAndMap`/`ResultAndMap` are still direct-built inline rather
than routed through `stdlib_scheme`, hoist them into `stdlib_scheme` first,
matching the comment at `constrain.rs:2377` — "the base scheme is relocated
into `stdlib_scheme`" — that Dict/Set already went through.)

**Message.** Add `"NotCurried"` (or a clearer end-user label — "single-
argument function" reads better in prose) to `super_unsatisfied`'s class-
name join (`lib.rs:553-576`), so `Just (\a b -> a+b) |> Maybe.andMap ...`
reports something like *"an `andMap` payload obligated to: single-argument
function — found `a -> b -> c`"* rather than a generic Eq/Ord message. This
needs a small branch: today `super_unsatisfied` assumes the offending `ty`
itself is what fails the bound and calls `ty_to_doc(ty, ...)` to render it —
that already works unchanged for our case (`ty` here is exactly the curried
arrow that's the problem).

**Why this closes the residual (§2.3) that Tier 1 alone cannot:**

* Untyped, same-module (`myAndMap = Result.andMap`, one use) — Bug 3's exact
  shape — closed the same way Tier 1 closes it: the obligated `b` var and
  `myAndMap`'s own call-site `b` are the same UF variable under `CLocal`,
  checked post-solve.
* Untyped, cross-module, reused at 2+ different payload arities — per
  §2.4, `promote_untyped_boundaries` excludes an obligation-carrying root
  from quantification, forcing a single concrete pin; a second,
  incompatible external use is *already* a hard error before the
  `NotCurried` bound is even consulted (over-conservative relative to a
  hypothetical "reused at two SAFE arity-1 types" case, which is the one
  acknowledged, documented precision loss — see §3.3).
* Typed/annotated wrapper, reused at 2+ different payload arities across
  modules — per §2.4's `Math.min` precedent, the bound is meant to lift
  onto the annotation's own type parameter and re-verify at each of the
  wrapper's own external call sites, exactly like `Comparable`. This is the
  one sub-case that needs the fixture in §4's `T3.residual` row to CONFIRM
  the propagation is genuinely wired for a freshly-added bound (as opposed
  to Math.min/Dict having some kernel-specific special-casing) before this
  document can claim it closed — flag honestly, do not assume.
* Higher-order argument / record-field extraction / import alias — none of
  these change *how* `Result.andMap`/`Maybe.andMap` gets its type
  constrained (`constrain_var_kernel` fires on the literal `VarKernel`
  occurrence exactly once regardless of what larger expression contains
  it); they only change what *further* unifies with the resulting type.
  Closed by construction, same reasoning as §2.1.

### 3.2 Tier 1 (defense-in-depth backstop): relocate the lowering-time check into `lower_callee` itself

Even with Tier 2 landed, keep a lowering-time backstop — CLAUDE.md's
"defence in depth… is the floor, not the foundation" applies directly here,
and this tier is cheap (a few lines, no new diagnostic machinery, reuses
the already-reverted-and-known-correct peeling logic). Concretely:

* Restore `reject_curried_andmap_payload` (its span-peeling logic from
  `39d9a57` was correct — see the earlier trace in §0 — only its *call
  site* was wrong).
* Instead of calling it from `lower_call_uniform`'s `VarKernel|VarTopLevel`
  arm, move the call **inside `lower_callee`** (`lower.rs:8294`), wrapping
  the existing resolution logic:

```rust
fn lower_callee(&self, callee: &canon::Expr) -> DResult<Callee> {
    let resolved = self.lower_callee_resolve(callee)?;  // existing body, renamed
    self.reject_curried_andmap_payload(&resolved, callee)?;
    Ok(resolved)
}
```

(`lower_callee_resolve` = the current function body verbatim, under a new
private name; zero behavioural change to any existing match arm.) Because
*every* caller of `lower_callee` — today `lower_call_uniform`'s direct-call
arm and `lower_expr`'s bare-value arm, and any future third caller — now
gets the check automatically, this closes Bug 3 (bare-value re-export) and
every other position `lower_callee` is reached from, by construction,
without needing to know how many call arms exist. This is the same
"single-funnel" argument as Tier 2's `constrain_var_kernel`, one pipeline
stage later, over the already-fully-solved region type.

Do **not** treat Tier 1 as sufficient on its own — it inherits the
`CLocal`/generalization residual from §2.3 exactly as before (it is
*re-anchored* to a proven-exhaustive funnel, but it is still a lowering-time
check over `region_ty`, so a genuinely severed cross-module generalized
wrapper is invisible to it the same way it would be invisible to a
pre-Tier-2 world). Tier 2 is what makes the residual sound; Tier 1 is what
makes failures cheap to diagnose and gives skyc a second, independent line
of defense if Tier 2's obligation wiring has a bug.

### 3.3 Documented, accepted precision loss

An **unannotated**, cross-module-exported wrapper around `andMap` used at
two *different but individually safe* (both arity-1) payload types is
rejected under Tier 2, where a hypothetical fully-general qualified-type
system would accept it (each use is independently sound; only their
*conjunction* forces monomorphism because obligated vars don't
quantify under `promote_untyped_boundaries`). This is strictly
**more conservative**, never **more permissive** — it never lets the arity
≥ 2 hazard through, it only occasionally over-rejects a narrow, non-
idiomatic pattern (a bare kernel re-export reused generically across
modules). Workaround if it ever bites a real user: annotate the wrapper
explicitly (`myAndMap : Result e a -> Result e (a -> b) -> Result e b`),
which routes it through the annotated/re-propagated path instead of the
untyped/CLocal path. Record this trade-off in
`docs/architecture/ctor-payload-function-design.md`'s hazard table
(§2) when Tier 2 lands; it does not need a divergence-ledger entry (it is
strictly a Sky-side conservatism, not a Go-oracle behavioural difference).

---

## 4. Regression-test plan — proving exhaustiveness, not re-testing the 3 known bugs

The goal is coverage of every **aliasing category**, each as a red (still
correctly rejected) / green (arity-1 payload, same alias shape, accepted)
pair, so a fix that over-rejects is caught as fast as one that
under-rejects. All red fixtures assert the diagnostic code (`SKY-T0014`
once Tier 2 lands; `SKY-L0114` if only Tier 1 exists yet) rather than just
"build fails" — a fixture that started failing for the *wrong* reason
(e.g. a genuine type error unrelated to arity) must not silently pass.

| Category | Red fixture (curried payload) | Green twin (arity-1 payload, same alias shape) | Status before this design |
|---|---|---|---|
| Direct call | `Just (\a b -> a+b) \|> Maybe.andMap (Just 1) \|> Maybe.andMap (Just 2)` | `Just (\a -> a+1) \|> Maybe.andMap (Just 1)` | Covered (existing `and_map_curried_stays_gated`) |
| Pipe-desugared nested call | `Result.andMap (Ok 1) (Ok add3curried)` (explicit, non-piped 2-arg call) | same, arity-1 | Covered |
| `let`-bound partial application | `let g = Result.andMap (Ok 1) in g (Ok add3curried)` | same, arity-1 | Covered (Bug 2 fixture) |
| **Bare top-level point-free re-export** | `myAndMap = Result.andMap` … `myAndMap (Ok 1) (Ok add3curried)` | same, arity-1 | **New — reproduces Bug 3 exactly** |
| Higher-order argument | `applyAM : (Result e a -> Result e (a -> b) -> Result e b) -> Result e a -> Result e (a -> b) -> Result e b; applyAM f x y = f x y` … `applyAM Result.andMap (Ok 1) (Ok add3curried)` | same, arity-1 | **New** |
| Record-field extraction | `{ combiner = Result.andMap }.combiner (Ok 1) (Ok add3curried)` | same, arity-1 | **New** |
| Re-exported import alias | `import Result as R` … `R.andMap (Ok 1) (Ok add3curried)` | same, arity-1 | **New** — expected to be a non-issue (import aliasing is a name-resolution-time rewrite, resolves to the identical `VarKernel` node as `Result.andMap`), but must be *proven*, not assumed. |
| **Cross-module generalized wrapper, two arities** (`T3.residual`) | Module `Lib`: `andMapAlias = Result.andMap` (unannotated) OR `andMapAlias : Result e a -> Result e (a -> b) -> Result e b` (annotated), exported. Module `A` imports it, calls with an arity-1 payload. Module `B` imports it, calls with an arity-2 curried payload. | Same shape, both call sites arity-1 (different element types, e.g. `Int` in `A`, `String` in `B`) — must stay ACCEPTED to prove Tier 2 doesn't over-reject legitimate cross-module reuse. | **New — the fixture that determines whether §3.1's Tier-2 claim needs T3.2 follow-up work.** Run this FIRST, before declaring T3 done; if the annotated variant is rejected as a false positive, the "lifts onto the annotation skolem" propagation needs its own fix (file as `T3.2`, scoped narrowly — everything else in this table is unaffected). |
| Reuse of the extracted/aliased fn value (T4 interaction) | `let mf = Just (\x -> x+1) in (consume mf, consume mf)` — must still raise `SKY-L0127`, not a `NotCurried` false-positive from Tier 2 | `case Just (\x -> x+1) of Just f -> f 1 + f 2` (callee-position double-use, non-consuming) — must still be accepted | Existing T4 fixtures; re-run unchanged, confirms the two gates (`SKY-L0127` reuse, `SKY-T0014`/`SKY-L0114` arity) don't cross-fire on each other's fixtures |

**Unit-level** (`crates/sky_types/tests/` for Tier 2, `crates/sky_lower/tests/unsupported.rs` for Tier 1): a direct `infer()`/`region_ty` assertion per category above, without the `SKY_E2E=1` `cargo build` round-trip — faster feedback loop, and it is what proves the *type-checker* (not just the golden harness) sees the obligation.

**Golden E2E** (`crates/skyc/tests/golden_l0114_ctor_payload_function.rs`, extended): every green twin above gets an `SKY_E2E=1` build+run leg with a Go-oracle parity check (Go's own applicative-`andMap` idiom handles curried payloads today per the parent doc's §2.1 divergence note — recorded there already, no new divergence entries needed).

---

## 5. Task breakdown (supersedes the parent doc's `T3` row)

| # | Task | Files | Depends on | Status |
|---|---|---|---|---|
| T3a | Add `TyBounds::and_map_payload()` + `not_curried_ok` predicate in `emitted_bound_satisfied`/`concrete_super_ok` + class name in `super_unsatisfied` | `sky_types/src/ty.rs`, `sky_types/src/lib.rs` | — | **DONE** |
| T3b | Wire the obligation into `constrain_var_kernel` for `MaybeAndMap`/`ResultAndMap`, tying scheme-var `1` (`b`) to a bounded super-var, mirroring `key_obligation_for` | `sky_types/src/constrain.rs:2292` region | T3a | **DONE** |
| T3c | `T3.residual` fixture (cross-module generalized wrapper, both annotated and unannotated variants) — run and record the actual outcome | `tests/golden/`, `sky_types/tests/` | T3b | **DONE** — `l0114_and_map_cross_module_wrapper_accepted` (annotated variant, two arity-1 uses) confirmed ACCEPTED. The unannotated cross-module variant (§3.3's acknowledged precision-loss case) was not separately fixtured — it is documented, not tested, since §3.3 already predicts and accepts its rejection as conservative-but-sound. |
| T3d | If T3c's annotated variant is a false positive: extend the "lift bound onto annotation skolem" path … | TBD, audit first | T3c | **NOT NEEDED** — T3c's annotated variant was accepted on the first try, with zero `and_map_payload`-specific propagation code beyond the `constrain_var_kernel` tie in T3b. The generic `Content::Rigid` ⇄ `Content::Super` unification path (`unify.rs`) already lifts any bound onto an annotation skolem the same way it does for `Math.min`'s `Comparable` bound — no new machinery needed. |
| T3e | Restore `reject_curried_andmap_payload` (logic unchanged from `39d9a57`), relocate its call site inside `lower_callee` per §3.2 | `sky_lower/src/lower.rs` | — (independent of T3a/b, can land in parallel) | **DONE** |
| T3f | Full fixture matrix from §4 (7 aliasing categories × red/green, plus the T4-interaction pair) — both unit-level and golden E2E legs | `tests/golden/*`, `sky_types/tests/`, `sky_lower/tests/unsupported.rs`, `skyc/tests/golden_l0114_ctor_payload_function.rs` | T3a-e | **DONE** — every category in §4's table has a fixture; import-alias confirmed not constructible (documented, not skipped); direct/pipe, `let`-bound, bare-alias, higher-order-argument, and record-field-extraction all confirmed rejected (SKY-T0001, the eager-pin diagnostic — see this doc's top status note); an additional forwarder fixture proves the SKY-T0014 path also genuinely exists. |
| T3g | Diagnostics/docs: extend `explain/SKY-T0014.md` with the `andMap` obligation class; note in `explain/SKY-L0114.md` that Tier 1 is now defense-in-depth behind a type error, not the primary gate; record §3.3's precision-loss trade-off in the parent doc's hazard table | `sky_diagnostics/explain/`, `docs/architecture/ctor-payload-function-design.md` | T3a-f | **DONE** — `SKY-T0014.md` and `SKY-L0114.md` both rewritten; parent doc's task table marks T1-T6 done. |

Estimated blast radius: `sky_types` (new bound + one kernel-scheme tie,
same shape as the existing Set/Dict-key precedent) + `sky_lower` (one
function-signature split, zero new logic) + diagnostics/tests. No
emit/runtime changes — matches the parent doc's Stage 1 scope.

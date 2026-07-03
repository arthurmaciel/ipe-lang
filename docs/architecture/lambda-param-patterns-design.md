# SKY-L0105 — Lambda / function parameter patterns (conciliated design)

> Guardian reconciliation of three fresh designs (A) against upstream
> `../sky` v0.17.2 learnings (B). Principle order (STRICT): (1) security
> (2) correctness (3) soundness (4) efficiency (5) completeness
> (6) readability. Rules held verbatim: **PARSE, DON'T VALIDATE** and
> **MAKE INVALID STATES UNREPRESENTABLE**.

## Scope

Unblock parameter patterns in every binding position:

- `\_ -> e` — the dominant idiom (`Task.andThen (\_ -> …)`, `Cmd.perform`,
  `Server.get (\_ -> …)`).
- `\(a, b) -> e` — tuple destructure in a lambda.
- `\{ field } -> e` — record field-pun destructure in a lambda.
- `f _ = e`, `f (a, b) = e`, `f { x } = e` — the same shapes in a
  top-level / `let` function head.
- Multiple params, nested, and `p as name` aliases.

A **refutable** parameter (`\(Just x) ->`, `\1 ->`, `\[a] ->`, `\x::xs ->`)
is a **clean compile-time SKY error**, never a runtime match failure.

## Ground truth (verified against HEAD)

- **Parse** already yields full `Pattern` nodes in every param position
  (`crates/sky_parse` — lambda params, let-def heads, top-level def heads
  all funnel through the one pattern grammar). No change needed. This is
  PARSE-DON'T-VALIDATE already in place: the grammar accepts the whole
  syntactic class and defers meaning.
- **Canon** already binds pattern names and canonicalises each param
  (`crates/sky_canon/resolve.rs` lambda arm + def-param loop). The
  canonical `Pattern_` set (`ast.rs:240`) **omits `PFloat`** — float
  patterns are already unrepresentable post-canon (the port is ahead of
  upstream, which rejects them via a Haskell `error` crash). `PRecord`
  is documented "Always irrefutable" (field-pun only).
- **Types** already constrain each param via `constrain_pattern`
  (`constrain.rs` `constrain_lambda` + typed/untyped def loops), the same
  routine `constrain_case` uses — a param pattern's type IS its arg type.
- **Exhaustiveness** (`exhaust::check`, `sky_types/lib.rs:119`) runs
  **after** the solver settles and **before** lowering. Today its
  `check_expr` **Lambda arm drops the params** (`exhaust.rs:328`,
  `Lambda(_, body)`), and `check` never inspects def params.
- **Lower** is the actual gap. `pattern_var` (`lower.rs:1697`) rejects any
  non-`PVar` param; `split_typed_sig` (`1647`) special-cases only the
  all-binder tuple; `lower_lambda` (`1960`) calls `pattern_var` directly
  so even `\(a,b) ->` is rejected today though `f (a,b) =` half-works.
  Crucially the port already **fail-closes** unhandled shapes with real
  diagnostics **SKY-L0115** (tuple gap) / **SKY-L0116** (refutable ctor
  gap) — it does **not** emit a panicking `let … else`. This is strictly
  better than upstream and the design preserves it.

## Chosen approach

**Fresh design A#2 (irrefutability + soundness) as the spine**, grafted
with A#1's *single shared predicate* invariant and A#3's *globally-unique
synthetic-binder supply* + full idiom coverage, plus two genuinely-superior
`../sky` learnings (single-source enum-name bridging; typed record
struct-name resolution). A parameter is a **binding** position, not a
**discrimination** position — it must match every value of its type, i.e.
it must be **irrefutable**. We enforce that as a compile-time invariant so
the only param patterns that ever reach codegen are provably total; there
is no emitted `match`/`let…else` with a panic arm for a parameter, hence no
runtime match-failure and no DoS/500 surface (P1 security, P2 correctness,
P3 soundness all satisfied at the phase boundary).

### The three load-bearing pieces

1. **One syntactic classifier, shared by gate and lowerer.** Add
   `Pattern_::is_irrefutable(&self) -> bool` to `crates/sky_canon/ast.rs`
   (pure, total, ~8 lines):

   | Variant | irrefutable? |
   |---|---|
   | `PVar`, `PAnything` | `true` |
   | `PRecord` | `true` (field-pun; always matches once the record type is fixed) |
   | `PTuple(es)` | `all es.is_irrefutable` |
   | `PAlias(inner, _)` | `inner.is_irrefutable` |
   | `PCtor{..}`, `PInt`, `PBool`, `PChar`, `PStr`, `PList`, `PCons` | `false` |

   Deliberately **syntactic** (no type-directed single-ctor leniency) so
   the rule is total, predictable, and needs no type lookup. The **same**
   predicate is consumed by the exhaustiveness gate (reject otherwise) and
   by the lowerer (`bug()`-assert otherwise), so the two **cannot desync**:
   a gate that admitted a shape the lowerer can't emit would resurface as a
   lowerer ICE, which this shared predicate structurally forbids.

2. **The gate — in the exhaustiveness phase, before lower.** Extend
   `exhaust::check` to sweep every `Def` param pattern, and `check_expr`'s
   Lambda arm (`exhaust.rs:328`) to sweep every lambda param (today it
   drops them). Also close the latent hole at `exhaust.rs:298`: a `let`
   binder (`LetBinding.pat`) is *assumed* irrefutable — assert it with the
   same predicate. Any `!is_irrefutable` param/binder raises a new
   **`TypeError::RefutablePatternParameter { span }` → SKY-T0015**
   (`Severity::Error`) with a hint: *"a parameter pattern must be
   irrefutable; `Just x` can fail to match — bind the whole value and use
   `case`."* Because this runs before lower, a refutable param is
   **unrepresentable** in the lowerer's input.

3. **The lowering — reuse the existing irrefutable `Destructure`, one path
   for both binding sites.** No new pattern codepath, no `Expr::Match` for
   irrefutable params (a `match` would carry a fallible arm). Reuse the
   exact `Expr::Destructure` node that `lower_case`'s `is_destructure_head`
   path already collapses `case s of (a,b) -> e` into — an irrefutable
   product lowers to a Rust `let`, not a `match`.

## The exact desugaring

Canonical model (never materialised as a `Match` node — see below):

```
\pat -> body    ==>   \fresh -> (destructure <pat> = fresh in body)
f pat = body    ==>   f fresh  = (destructure <pat> = fresh in body)
```

Concretely, `pat` (proven irrefutable by the gate) lowers to
`Expr::Destructure { binder: <lowered pat>, value: Var(fresh), body }` —
the **same** node the shipped tuple-param prologue and `let (a,b) = e`
already emit. Per-shape:

```
\(a, b) -> body   ==>  \arg0 -> Destructure { (Var a, Var b) = arg0; body }
\{ x } -> body    ==>  \arg0 -> Destructure { {x: Var x, ..: Wildcard} = arg0; body }
\_ -> body        ==>  \arg0 -> body           (NO Destructure; arg0 unused)
\a -> body        ==>  \a    -> body           (PVar: the param IS the name — zero overhead)
\_ x (a,b) -> b   ==>  \arg0 x arg1 -> Destructure { (a,b) = arg1; body }
f (a,b) = body    ==>  f arg0 -> Destructure { (a,b) = arg0; body }   (typed def)
```

Notes:

- **`PVar` fast path**: the param *is* the name — no fresh binder, no
  statement, zero overhead (P4).
- **`\_ ->`** (the dominant case): `arg0` is simply unused. **No
  `Destructure` is emitted** — the synthetic param is covered by the
  emitted crate's `#![allow(unused)]` preamble, so no `let _ =` statement
  and no branch. `rustc` optimises it to nothing.
- **Record binder** recovers the **complete field set from the param's
  solved TYPE** (each punned field → `Pat::Var`, every other field →
  `Pat::Wildcard`), resolved from the arrow-arg `Ty` at the param region —
  **not** from a field-name-set heuristic (see grafted learning L4). A
  param has no value expression, so the type is the only sound source.
- **Refutable** patterns are never desugared — the gate rejected them
  first (SKY-T0015).

### Synthetic-binder supply (A#3, kills the collision argument)

Mint `fresh` binders from a **globally-unique** pool, not a
position-indexed one. Pre-count non-var param positions across **defs AND
every lambda** (extend the current `max_def_arity` walk in
`sky_lower/lib.rs` into `count_destructure_param_sites`), pre-mint that many
distinct `arg_N`, and hand them out through a monotonic `Cell<usize>`
cursor. Distinct-per-site names make cross-nesting collision (a def param
and a lambda param inside its body both wanting `arg_i`) **unrepresentable**
— we never lean on Rust shadowing. Overrun fails closed (`bug()`), never an
index panic.

### TCO interaction (verified low-risk)

`Destructure` is tail-transparent (`analyze_tail_recursion`,
`lower.rs:414`) and the prologue folds **inside** the `TailLoop` body
(`lower_def` folds the prologue then wraps in `TailLoop`;
`rewrite_in_tail` descends `Destructure.body`), so a tail-recursive
`f (a,b) = … f (c,d)` reassigns `arg0` and re-runs the destructure each
iteration. A nested-record tail-recursion golden locks this.

## Grafted `../sky` learnings

- **ADOPT — unified pattern grammar / `[Pattern]` on Lambda·Def.** Already
  true in the port; keep it. Storing the structured pattern (not a
  pre-flattened name) is PARSE-DON'T-VALIDATE.
- **ADOPT — single-variant-enum irrefutability check becomes
  assert-and-emit-plain-`let`.** Once the SKY-T0015 gate lands, every
  surviving param is provably irrefutable, so the lowerer emits a plain
  `let` destructure (no dead `else`, dodging rustc's
  `irrefutable_let_patterns` lint). Keep the set-membership check only for
  the single-variant-enum destructure emitted by the *documented extension*
  below.
- **ADOPT — single-source Maybe/Result enum-name bridging (`../sky`
  Pattern.hs:86-89).** The pattern renderer MUST resolve ctor enum names
  (`Just`/`Ok` → the runtime `SkyMaybe`/`SkyResult` idents) through the
  **same** naming function the value/type renderers use, or a ctor
  destructure emits an undefined `SkyCoreMaybeMaybe::Just` (E0433 — an
  exit-0-then-cargo-fail seal break). Enforce a single enum-naming source
  shared by pattern/expr/type emit in `sky_backend_rust`.
- **ADAPT — record param name recovery.** Upstream recovers the struct
  name by field-set heuristic (`matchStructByFieldsE`) — a soundness smell
  (two structs with the same field set collide; `..` hides mismatches).
  Resolve the struct name from the **solved type** of the param region
  instead, then emit the same `Struct { … }` pattern. Keep the emit shape,
  replace the heuristic.
- **ADAPT — float-pattern rejection.** The port already drops `PFloat` from
  canon (unrepresentable — better than upstream's `error` crash). Nothing
  to do here; noted so the invariant is not accidentally regressed.

## Rejected & why

- **REJECT — upstream's refutable param → `else { panic!("…non-exhaustive
  pattern match on a function argument") }` (`../sky` Pattern.hs:138).**
  A well-typed program that panics at runtime is a **soundness** hole
  (P3, strictly above completeness P5) and a **security** vector (P1: a
  refutable param reachable from untrusted input is a DoS/500 only masked
  by the recover boundary). Upstream's own comment admits the total fix is
  front-end rejection. We reject at the type phase with SKY-T0015; the
  classified panic survives **only** as an unreachable defense-in-depth
  floor, never primary behaviour.
- **REJECT — upstream's asymmetric lambda-vs-def lowering (`../sky`
  ExprEmitter.hs:3749; def uses the destructure prelude, lambda uses
  `patternToRustParam` and silently DROPS a ctor param's bindings to `_`).**
  A genuine upstream bug and a completeness gap: two binding sites disagree
  on what a pattern param means. We route **both** through the one
  `Destructure` desugaring so behaviour is identical — one code path can't
  disagree with itself (make-invalid-states-unrepresentable at the
  architecture level).
- **REJECT — desugar every non-var param to `\fresh -> case fresh of pat ->
  body` as a real `Expr::Match` (a literal reading of A#1/A#3's model).**
  For an irrefutable pattern a `Match` node would still carry a fallible
  shape and risk emitting a panic arm. We lower to `Expr::Destructure` (the
  irrefutable path) instead — the `case`-of framing is the *mental* model,
  `Destructure` is the *materialised* node. The gate guarantees the two
  coincide.
- **REJECT (first cut) — type-directed single-constructor leniency
  (`\(Wrapper x) ->` treated as irrefutable).** Kept conservative and
  syntactic: even a single-variant ctor param is a clean SKY-T0015 (a
  **sanctioned divergence from Elm**, recorded in
  `docs/divergences-from-{sky,elm}.md`). Rationale: a total, predictable,
  no-type-lookup rule keeps the gate and the lowerer's capability set
  perfectly aligned. **Documented sound extension** (only if a real example
  needs it): desugar an irrefutable-but-ctor-headed param to a *single-arm*
  `case fresh of pat -> body`, exhaustive by construction, gated by the
  usefulness algorithm so it can never outrun the lowerer — never widen the
  irrefutable-`let` path to emit a ctor destructure.
- **REJECT — retiring/renumbering SKY-L0105/L0115/L0116.** Keep the codes
  reserved; their lowerer arms become unreachable `bug()` invariants (a
  refutable param can no longer reach the lowerer). Flip their explain
  pages to historical/implemented status. Avoids code renumbering churn.

## Ordered build tasks

1. **PARSE — no change; lock it.** Add a parser doc-test asserting `\_ ->`,
   `\(a,b) ->`, `\{f} ->`, `f _ =`, `f (a,b) =` all produce full `Pattern`
   param nodes. Records the PARSE-DON'T-VALIDATE boundary.
2. **CANON — the shared classifier.** Add `Pattern_::is_irrefutable`
   (`crates/sky_canon/src/ast.rs`) with unit tests for every variant
   (incl. nested `PTuple`/`PAlias`). This single predicate is the contract
   both later phases consume.
3. **TYPES — the irrefutability gate (SKY-T0015).**
   - Add `TypeError::RefutablePatternParameter { span }`
     (`sky_diagnostics/diagnostic.rs`), code `SKY-T0015`
     (`sky_diagnostics/code.rs`), message text (`render.rs`), and
     `explain/SKY-T0015.md`.
   - Extend `exhaust::check` to sweep every `Def` param; extend
     `check_expr`'s Lambda arm (`exhaust.rs:328`) to sweep every lambda
     param; assert the `LetBinding.pat` invariant at `exhaust.rs:298`. Each
     `!is_irrefutable` → SKY-T0015 at the offending sub-pattern's span.
   - (Optional, cheap) record `regions.insert(p.span, v)` after each
     `constrain_pattern` on params so lower can read the solved param type
     for record field-completion.
   - Negative tests: `\(Just x) ->`, `\1 ->`, `\[a] ->`, `\x::xs ->`,
     `f (Just x) =` → one precise SKY-T0015 each, before lowering.
4. **LOWER — one path, reuse `Destructure`.**
   - `sky_lower/lib.rs`: add `count_destructure_param_sites` (defs +
     lambdas), size the `arg_` pool, hand out via a `Cell<usize>` cursor.
   - Add `lower_param(pat, ir_ty) -> (IrParam, Option<prologue>)`:
     `PVar`→bare param; `PAnything`→fresh param, no prologue;
     `PTuple`/`PAlias`→fresh + `lower_destructure_pat`;
     `PRecord`→fresh + typed `lower_record_pat` sourced from the param's
     solved record type; refutable→`bug()` (unreachable post-gate).
   - Generalise `split_typed_sig`'s non-`PVar` arm to call `lower_param`
     for every param.
   - Add the prologue path to `lower_lambda` (fold `Destructure`
     outermost-first across the flattened nested-lambda params).
   - `pattern_var`'s non-`PVar` branch + `lower_destructure_pat`'s
     refutable arms become fail-closed `bug()` invariants; flip
     SKY-L0105/L0115/L0116 explain pages to historical.
   - Backend: enforce single-source Maybe/Result enum-name resolution
     shared by pattern/expr/type emit; resolve record struct name from the
     solved type.
5. **GOLDEN + regression.**
   - Positive goldens: `\_ -> e`, `\(a,b) -> e`, `\{f} -> e`, `f _ = e`,
     `f (a,b) = e`, multi-param `\_ x (a,b) ->`, nested/alias, and a
     **tail-recursive `f (a,b) = … f (c,d)`** proving the destructure
     re-runs per iteration.
   - Negative goldens: SKY-T0015 for `\(Just x) ->`, `\1 ->`, `\[a] ->`.
   - Assert the emitted preamble keeps `#![allow(unused)]` so `\_ ->`
     stays warning-clean under `#![deny(warnings)]`.

## Out of scope / follow-ups

- **Untyped top-level `f _ = e` (no annotation)** stays blocked by the
  pre-existing untyped-function gate (there is no arrow to source param
  types). Lambdas `\_ -> e` and `let`-bound `f _ =` (which desugar to a
  lambda carrying an inferred arrow) ARE covered — this unblocks the
  reported 14-task-demo case and the `Task.andThen`/`Cmd.perform`/
  `Server.get` idioms.
- The single-constructor irrefutable extension (§Rejected) is filed as a
  documented, usefulness-gated follow-up, not shipped in the first cut.

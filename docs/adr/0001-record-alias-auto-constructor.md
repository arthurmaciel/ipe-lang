Status: Accepted
Date: 2026-07-10

# 0001. Record type-alias auto-constructor is a synthesized typed function

## Context

`type alias UserProfile = { username : String, age : Int, active : Bool }`
should, per Elm-family semantics, introduce an in-scope **value**
`UserProfile : String -> Int -> Bool -> { username, age, active }` (positional,
in *declared* field order). Before this decision the alias was registered only
in the *type* namespace, so a value use — the dominant sweep shape
`Decode.succeed UserProfile |> required "username" string |> …` — failed name
resolution with IPE-N0001 ("cannot find this value in scope"). This blocked the
Live / Http / Db examples in the skyc sweep.

The decision was guardian-reviewed against the strict principle order
**security > correctness > soundness > efficiency > completeness > readability**
and the two rules *PARSE, DON'T VALIDATE* and *MAKE INVALID STATES
UNREPRESENTABLE*. It is implemented (`golden_m82_record_ctor.rs`, task #82 /
IPE-N0001); the code in `sky_canon` is now the source of truth for the *how*.
This ADR preserves the *why*.

## Decision

**The auto-constructor is a synthesized typed function.** At the single canon
site where a record alias's source-order fields are known, parse the alias into
an ordinary `canon::Def::Typed` that IS the constructor. Register the alias name
in the value namespace (`VarHome::TopLevel`) so `resolve_var("UserProfile")`
succeeds. From that point on every downstream stage receives a plain,
fully-typed top-level function — HM, lowering, and the Rust backend need **zero**
special-casing and **no new IR node**.

This is the fresh design labelled *"correctness + field-order"* (source A,
proposal 1), grafted with the genuinely-superior ../sky ADOPT learnings (real
HM scheme; TRecord-gated value registration; cross-module value forwarding;
hard collision diagnostic).

### Why this beats the alternatives (principle-by-principle)

The two rival fresh designs (A2, A3) add a dedicated `Expr_::RecordCtor` canon
node + a `CtorScheme` registration + new `constrain` / `lower` / `backend`
arms. Both are *sound*, but they lose to the synthesized-`Def` approach on the
higher-and-equal principles:

* **Soundness (P3) + MAKE INVALID STATES UNREPRESENTABLE.** CLAUDE.md §8: *"New
  AST nodes require explicit walker arms … don't rely on `_ -> []`
  catchalls."* A dedicated `RecordCtor` node reintroduces exactly that burden —
  every walker (constrain, exhaustiveness, lower, backend, and any LSP
  token/ref walkers) gets a new arm, and each `_ =>` catchall that forgets it is
  a latent silent-mishandling bug. The synthesized-`Def` approach adds **no new
  node**, so "a stage that forgot record-ctor handling" is *unrepresentable*:
  the ctor is byte-for-byte indistinguishable from a hand-written typed
  function.
* **Efficiency (P4).** One synthesized def, lowered once as a shared function,
  DCE-pruned if unreferenced. Neutral-to-better than per-reference synthesis.
* **Completeness (P5).** Equal. All sweep shapes (monomorphic, parametric,
  nested, bare-ref-into-pipeline, partial application, cross-module) are covered
  by machinery that **already exists** and is keyed on `VarTopLevel`:
  `Expr::FuncValue` reification (lower.rs:2648, 2705) for the bare reference
  `Decode.succeed UserProfile`, `eta_expand_partial` (lower.rs:2782, 3038) for
  partial application, an ordinary `Call` for saturated application, and the
  existing dep value-injection path (`env.vars.insert(name,
  VarHome::TopLevel(dep_path))`, resolve.rs:683) for cross-module use.
* **Readability (P6).** Decisive. A reader who understands a normal typed
  function already understands the constructor; there is **no new IR
  vocabulary** to learn.

### Load-bearing facts verified in the tree (not assumptions)

1. `canon::Def::Typed` annotations are collected into a per-binding scheme and
   instantiated **fresh at every `VarTopLevel` reference**
   (`constrain.rs:781` collects `Def::Typed { name, ty, .. }`; `SchemeApp` is
   recorded per reference, lines 597-601). ⇒ the synthesized ctor's declared
   **field types are checked at each call site** — this is the *sound* behaviour
   upstream Sky omits (see "Rejected").
2. Canon records are **already** ordered `Vec<(Symbol, _)>`
   (`canon::Expr_::Record`, ast.rs:202; `canon::Type::Record`, ast.rs:305).
   There is no `Map name (index, type)` to re-sort — the ../sky
   `_fieldIndex`/Map Haskell-ism is already designed out; declared order is
   intrinsic.
3. `Ty::Record` / `FlatType::Record` are **closed** (name-keyed maps, no row
   variable). `FieldAccess` / `RecordUpdate` are deferred *precisely because*
   "closed records carry no row variable" (constrain.rs doc comments). ⇒ the
   ctor's result unifies only against an identical field-name set.

## The exact HM scheme

For `type alias T p0..pk = { f0 : A0, f1 : A1, …, fN : AN }` the synthesized def
carries the annotation

```
T : ∀ (q0..qm). A0 -> A1 -> … -> AN -> { f0:A0, f1:A1, …, fN:AN }
```

where `{q0..qm}` are the type variables occurring in the `A_i`
(⊆ the alias params `p0..pk`; phantom params drop out — sound). Encoded as a
`canon::Def::Typed`:

```
Def::Typed {
    home,
    name       = T,                                   // uppercase value name
    free_vars  = [q0..qm],                            // = alias params used in fields
    patterns   = [PVar(f0), PVar(f1), …, PVar(fN)],   // declared order
    body       = Expr_::Record([(f0, VarLocal f0), …, (fN, VarLocal fN)]),
    ty         = A0 -> A1 -> … -> AN -> Type::Record([(f0,A0), …, (fN,AN)]),
}
```

The whole thing is an *ordinary* typed binding, so the **existing**
`constrain_def` path applies with no changes:

* the annotation arrow is instantiated with one shared rigid map (a param `a`
  shared across fields stays one skolem);
* the body `Expr_::Record` is unified against the return `Ty::Record` by
  **exact field-name set** (closed-record unify);
* the binding generalises over `free_vars`.

**Per use site** (`VarTopLevel`), the scheme is instantiated fresh
(`SchemeApp`), so:

* **saturated** `T a0 … aN` — the surrounding `Call` folds the arrow in
  declared order, so `arg[i]` unifies with `A_i` (positional ⇒ field binding);
* **partial** `T a0` — residual `A1 -> … -> AN -> T`, produced by the existing
  eta path; currying preserves the declared order of the remaining fields;
* **bare** `Decode.succeed T` — the whole arrow is the value's type, reified as
  a boxed closure via `Expr::FuncValue`;
* **parametric** `Box a = { value : a, tag : String }` — generalises to
  `∀ a. a -> String -> { value:a, tag:String }`, alpha-renamed per site exactly
  like `Just : a -> Maybe a`, so `Box Int` and `Box Bool` each satisfy
  independently.

### Row-poly soundness

The result is a **closed** record. This compiler has **no row variable**:
`FlatType::Record` unifies only on identical field-name sets, then per-field by
name. Therefore the auto-constructor:

* opens **no** new row-poly surface — a missing field (too few args ⇒ residual
  arrow, not a record; fails a record slot), an extra field (over-application ⇒
  callee becomes a non-function ⇒ type error), or a mis-**typed** field (arg
  var tied to field var ⇒ mismatch) is a **compile error**, never silent
  acceptance;
* does **not** interact with the still-open subset/superset question (task
  #56), which lives only in deferred `FieldAccess` on already-closed records —
  a surface the constructor never emits.

If open rows are added later, the constructor's result stays the closed anchor
and only *consumers* would carry a row var; this design does not pre-empt that.

## Field-ordering guarantee (end-to-end)

Declared / `_fieldIndex` order is captured **exactly once** — from the
`src::TypeAnnotation::TRecord` field vec (source order) at the canon synthesis
site — and materialised into three **co-constructed** views from **one
iteration of the same ordered vec**:

1. the def's parameter patterns `[PVar(f0) … PVar(fN)]`,
2. the body record literal `Expr_::Record([(f0, VarLocal f0) …])`,
3. the arrow argument types `A0 -> … -> AN -> Record`.

Because all three come from one pass over the same ordered vector, positional
argument `i` is guaranteed to bind field `f_i`. There is **no data structure in
which the argument order and the field order can disagree** — a "constructor
whose arg order differs from its field order" is not constructible.

Downstream, order is *irrelevant to correctness* and cannot re-break it:
`Ty::Record` / `FlatType::Record` are name-keyed (order-erased) and unify purely
by field **name**; the backend resolves the struct by field-name **set** and
emits **named** struct fields (`Rec { username: f0, age: f1, … }`), so Rust
write-order is free. An alias whose declared order is neither alphabetical nor
interning order (`{ zebra : Int, apple : String }`) still binds `T 1 "a"` to
`zebra = 1, apple = "a"` — pinned by a regression that runs the emitted binary.

## MAKE INVALID STATES UNREPRESENTABLE / PARSE, DON'T VALIDATE

* **Parse, don't validate.** Whether an alias *has* a constructor is decided
  **once**, structurally, at declaration (source body is `TRecord`), and encoded
  as the presence of a `Def` + a value-namespace binding — not re-validated at
  any use site. A non-record alias (`type alias Count = Int`) gets no value
  binding, so using it as a value stays an ordinary IPE-N0001 name error (Elm
  parity). Head-alias-to-record (`type alias U = T` where `T` is itself a record
  alias) gets **no** ctor: the gate is strict on a *literal* `TRecord` body,
  matching Elm.
* **Make invalid states unrepresentable.** There is no "unresolved
  record-ctor reference" state anywhere — every stage sees only a well-typed
  function, so no stage can forget ctor-ness. Field order lives in exactly one
  place and is projected into patterns, body, and arrow together.
* **Collision is a hard diagnostic, not a silent skip.** Registering the ctor
  name folds into the existing `seen_values` set (resolve.rs:465-478); a
  user top-level value sharing the alias's exact name surfaces
  `NameError::DuplicateValue` at canon — **rejecting** upstream Sky's
  emit-time silent-skip (Compile.hs:9407-9409), which would make the ctor
  quietly vanish and surface as a confusing downstream error.
* **Function-typed fields fail closed.** A config-record alias
  (`{ onSubmit : msg }`) used as a positional auto-ctor builds a record literal
  with a function field ⇒ the pre-existing `FirstClassFunctions` gate
  (IPE-L0107) fires with a clean diagnostic — a pre-existing limitation, **not**
  a regression, and out of #82's data-record scope. The synthesized ctor must be
  DCE-eligible so an *unused* cfg-alias ctor is pruned rather than force-lowering
  a function-field body.

## Consequences

The auto-constructor requires no new IR vocabulary and inherits every existing
stage's handling of a typed top-level function; the invariant that must keep
holding is that a record alias's field order is captured *once* (source
`TRecord` vec) and projected into patterns/body/arrow together — never
re-derived at a second site. The following alternatives were considered and
**rejected**:

* **Dedicated `Expr_::RecordCtor` canon node + `CtorScheme` registration + new
  constrain/lower/backend arms** (fresh designs A2, A3). Sound, but reintroduces
  the CLAUDE.md §8 "new node ⇒ N walker arms + `_ =>` catchall drift" burden for
  no benefit the synthesized-`Def` approach doesn't already get from existing
  machinery. Loses on soundness/completeness/readability. A3's *per-reference
  synthesis at lowering* additionally re-derives field order in lower's zip —
  a second order-threading site where one synthesized def fixes it once.
* **Leave the ctor UNtyped in HM** (upstream Sky's real behaviour: absent from
  same-mod annots ⇒ fresh `CLocal` var; field types checked only by the backend
  compiler — Expression.hs:210-226, 509-527). **Rejected** — the third gate.
  `UserProfile 5 True` where fields are `String, Int` would type-check by
  structural inference and mis-build. Using the backend as the type oracle is a
  Go-quirk; for us the oracle is rustc (or nobody if the record literal is
  lenient). Violates PARSE-DON'T-VALIDATE and MAKE-INVALID-STATES-
  UNREPRESENTABLE. Our synthesized-`Def` inherently supplies the sound scheme.
* **Map + `_fieldIndex` field representation with `sortFieldsByIndex` sprinkled
  at every emission site** (Haskell-ism, TypeEmitter.hs:132 / Compile.hs:9331).
  **Rejected / already-designed-out** — canon records are ordered `Vec`, so
  "unsorted" is unrepresentable and no consumer can desync.
* **Emit the ctor as a Go generic `func Foo[T1 any](…) Foo_R[T1]` with TVar
  fields erased to `any`** (Compile.hs:9343-9389). **Rejected** — a Go-generics
  workaround; `any`-erasure defeats static typing and adds reflect-style runtime
  coercion. Parametric aliases map to genuine Rust generics / monomorphised
  records.
* **String-concatenation codegen for the ctor body** (ModuleEmitter.hs:104).
  **Rejected** — the task-#53 anti-pattern; the synthesized `Def` lowers a
  normal `Expr::Record` through the existing typed emitter, no bespoke string.
* **Silent skip on name collision** (existingNames guard, ModuleEmitter.hs:111).
  **Rejected** — a hard `DuplicateValue` canon diagnostic instead (see above).
* **Hardcoded `markerCfgAliases = {("Std.Webview","AppCfg")}` special-case**
  (ModuleEmitter.hs:82-86). **Rejected** — a Sky-runtime quirk; if a
  runtime-owned/zero-field cfg needs suppression, data-drive it from the
  kernel/runtime registry, never a literal module+name tuple.

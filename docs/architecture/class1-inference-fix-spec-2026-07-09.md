# Class 1 fix spec — untyped top-level binding generalization ("Boundary Scheme Promotion")

> Synthesis of 3 independent guardian-design reasoners (2026-07-09), reconciling
> `docs/architecture/class1-inference-questions-2026-07-09.md`'s question list.
> This spec is the implementable target — a fresh engineer should be able to
> execute it without re-deriving the design. Backlog item: `BACKLOG.md`
> "Sweep front" §2 ("untyped top-level binding shares ONE monomorphic var").

## Decision record (what was chosen and why)

**Chosen design: module-boundary generalization via placeholder isolation +
topo-ordered discharge ("Boundary Scheme Promotion").** All three reasoners
independently rejected rank/level-based let-generalization: it is strictly
*more permissive* than the reference Haskell compiler (which has no
rank-based let-gen either — its `CLet` never generalizes, verified directly
in `Solve.hs`), it would flip the existing regression
`untyped_polymorphic_use_at_two_types_is_rejected` into a genuine soundness
question (accepting programs the reference and the differential oracle both
reject), and it requires a `Content` level field + `unify.rs` merge-rule
changes touching AUD-12's exhaustive match — the highest-blast-radius option
for zero sweep value.

**Decisive evidence:** reasoner B differentially ran the actual reference
compiler (`sky v0.16.29`) against constructed repros and got empirical proof
of its exact observable semantics:

| Repro | Reference `sky v0.16.29` | Ipê `skyc` @ HEAD |
|---|---|---|
| Untyped `ident x = x`, used at `Bool` **in its own module** and `Int` from `Main` | **REJECT** | REJECT |
| Untyped `ident`, **unused** in its own module, used at `Bool` and `Int` from two *other* modules | **ACCEPT**, full build | REJECT |
| Untyped **value** `empty = []`, used at `List Bool` and `List Int` from two importers | **ACCEPT** (no value restriction) | REJECT |

This settles §0-Q1 (module-boundary generalization, not full let-gen, not
per-def dependency-order — the latter would extend acceptance into the first
row, which the reference rejects) and §C-Q18 (no value restriction) by direct
experiment rather than architectural argument alone.

**Critical re-diagnosis (do not skip):** re-running `skyc build` on
ex00/ex12/ex37 during this design pass found the backlog's original example
attribution is **stale**:
- **ex37** now builds skyc-clean (cleared by intervening landings).
- **ex00** now passes type-checking; first blocker is unrelated `SKY-L0114`
  (function value in ctor payload — same limitation as #90, filed there).
- **ex12**'s `SKY-T0012` is a **different bug**: `Error kind info`'s `info`
  binds a fresh, untied flex var via the ctor-pattern no-scheme fallback
  (`constrain.rs:2603-2617`) because `Error` (registered in canon, arity 2)
  has no `CtorScheme` in `constrain.rs`. The `untyped` map is not on this
  path. Filed separately as #160.

This spec's fix does **not** need to clear any of the three examples to be
correct or complete — its correctness is judged against the test matrix in
§5, not the sweep. #160 and #90 are prerequisite-free of this work and can
land independently, in parallel.

## Semantics (normative contract)

1. An unannotated top-level binding is **monomorphic within its home module**
   — every same-module reference (including self- and mutually-recursive
   ones) shares one inference variable. Unchanged from today; matches the
   reference's `CLocal` semantics exactly.
2. At its home module's constraint-solve completion, every **residual plain-
   `Flex`** variable reachable from the binding's root — and not reachable
   from any still-pending deferred obligation (field access / record update /
   route-witness / routed-live check) — is **quantified**.
3. Every **cross-module** reference instantiates the resulting scheme fresh
   (a UF copy-walk, not a `Ty`-level reify), exactly mirroring how typed
   annotated bindings already instantiate via `instantiate_tracked`.
4. `Super`-bounded vars (Number/Ord/Eq/Show obligations) and `Rigid`-
   contaminated vars are **not quantified in phase 1** — they stay
   program-wide shared, preserving today's behavior for them exactly. This
   is intentional under-acceptance (see Divergences).
5. No value restriction: zero-parameter bindings generalize identically to
   functions (the reference does this; Sky is pure, so it's sound).
6. Ambiguous instantiation (a use-site region still carrying a free var not
   covered by the enclosing def's own generics) **fails closed** with
   SKY-L0102 at the use site — stricter than the reference, which
   erasure-accepts via Go's `[]any`. Sanctioned (see Divergences).

## Data structures

**`crates/sky_types/src/constrain.rs`:**

```rust
// New, alongside Builder.untyped (unchanged: BTreeMap<(Vec<Symbol>, Symbol), VarId>)
pub struct PendingInstantiation {
    pub source: (Vec<Symbol>, Symbol), // defining (home, name)
    pub placeholder: VarId,            // fresh flex minted at the reference site
    pub use_home: Vec<Symbol>,         // module that owns the reference
    pub span: Span,                    // reference span, for blame attribution
}
// Builder gains: pending_instantiations: Vec<PendingInstantiation>
// Generated gains: pending_instantiations (carried out), module_order: Vec<Vec<Symbol>>
//   (distinct homes in first-encounter order over the linked module list —
//   already topo order per link.rs:22-25 / project.rs:261)

// Internal to the new discharge/generalize pass (never leaves sky_types):
struct UntypedScheme {
    root: VarId,
    quantified: BTreeMap<VarId, Symbol>, // UF root -> synthesized name ("a","b",...; never "any")
}
// BTreeMap<(Vec<Symbol>, Symbol), UntypedScheme>
```

**`crates/sky_types/src/lib.rs` (`SolvedTypes`) — two additive fields:**

```rust
pub untyped_type_params: BTreeMap<(Vec<Symbol>, Symbol), Vec<Symbol>>, // ordered; absent/empty = fully monomorphic def, lowerer behaves exactly as today
// poly_var_map (existing field) gains untyped-def entries: UF rep -> synthesized symbol
```

No new `Scheme` type. No `Ty::Var` reification with synthesized raw ids —
this deliberately avoids adding a third colliding population to AUD-13's
already-flagged `Ty::Var(u32)` conflation (interner raws vs UF reps). AUD-13
is **not** forced to land first; sequence independently.

## Algorithm

**Constrain phase — one change, at `constrain_var_top_level` (`constrain.rs:2015-2016`):**

```rust
} else if let Some(v) = self.untyped.get(&key).copied() {
    if key.0 == self.current_home {
        Ok(v)                                   // same-module: shared var, unchanged
    } else {
        let u = self.flex()?;                   // cross-module: isolate immediately
        self.pending_instantiations.push(PendingInstantiation {
            source: key, placeholder: u,
            use_home: self.current_home.clone(), span,
        });
        Ok(u)
    }
}
```

Nothing else in constraint generation changes. `regions` still records
whatever var this returns (`constrain.rs:2404`) — now the placeholder, which
zonks to the per-use concrete instantiation after discharge (strictly more
informative than today's program-wide-pinned type for every lowerer
consumer).

**Solve phase** — insert ONE new pass in `infer_with_budget_attributed`
(`lib.rs:164-371`), between the main `solve_attributed` call and
`resolve_deferred`:

```
for home in module_order:                              // topo order
    // (a) discharge this module's OUTGOING cross-module references
    for pi in pending_instantiations where pi.use_home == home:
        scheme = schemes[pi.source]         // exists: source module precedes in topo order
        inst = copy_var(uf, scheme.root, &scheme.quantified, &mut per_pi_fresh_map, budget)
        unify(uf, budget, interner, pi.span, inst, pi.placeholder)
            .map_err(|d| (d, pi.use_home))  // blame the USE module (9c44642 contract preserved)

    // (b) generalize this module's own untyped defs
    obligation_roots = roots reachable from every still-pending
                        field_access / record_update / route_witness / routed_live var
    for (name, shared) in untyped defs where home == this module:
        frees = reachable_flex_roots(uf, shared)          // structure walk, budget-ticked
                 .filter(|r| content(r) is plain Flex)     // excludes Super / Rigid / Structure
                 .filter(|r| !obligation_roots.contains(r))
        quantified = frees.map(|r| (r, mint_synth_symbol()))  // "a","b",... never "any"
        schemes[(home, name)] = UntypedScheme { root: shared, quantified }
```

`copy_var` (new, ~70 lines, sibling of `zonk` in `constrain.rs`): quantified
root → fresh `Flex` per discharge (a shared per-discharge map keeps repeated
occurrences of the same quantified var linked); `Structure` → clone with
recursively copied children, minting a fresh `EmptyRecord` sentinel per
record (mirrors the existing occurs-distinctness rule at
`constrain.rs:1265-1272`); any other var (Flex-not-quantified, `Super`,
`Rigid`) → **returned as-is, no copy**. This last rule is what makes every
program with no boundary-free untyped defs byte-identical to today: nothing
is copied unless it was actually quantified.

After this pass, the pipeline continues exactly as today: global
`resolve_deferred` fixpoint (now resolving importer-side field accesses on
freshly-discharged results), route-witness / routed-live / exhaustiveness
checks, numeric defaulting (only touches `Super` roots — never quantified,
since Super is excluded from generalization — no interaction, no reordering
needed), `typed_rigids`/`bounds`/`poly_var_map` assembly (extended to also
fold in `UntypedScheme`'s bounds-map, which is always empty in phase 1 since
`Super` vars aren't quantified — `check_scheme_applications` is naturally a
no-op for untyped schemes until phase 2), `check_scheme_applications`
(unchanged code, now also covers untyped-scheme uses for free), region
read-back (unchanged), env read-back (generalized defs read their scheme
`Ty` directly instead of zonking the shared var — this is the only
substantive change to read-back).

**Lower phase** (`crates/sky_lower/src/lower.rs`), untyped arm
(`:3707-3786`):

- If `SolvedTypes::untyped_type_params[(home,name)]` is present and
  non-empty: install `current_poly_tvars` from `poly_var_map[(home,name)]`
  around body lowering (mirror the typed arm's existing save/set/restore,
  `:3541-3556`); `split_unannotated_sig` (`:3873-3898`) gains a
  quantified-var map parameter — a `Ty::Var(raw)` present in the map lowers
  to `IrType::Generic(sym)`; absent, it still hits the existing SKY-L0102
  fail-closed arm (the gate stays closed for genuinely undetermined vars).
  `type_params` = the synthesized symbols, each with `bounds_for(...)`
  returning unbounded in phase 1 (no bounds map entries exist yet).
- Zero-param generalized value bindings take the identical path with
  `params: []` (backend already emits these as zero-arg fn calls per
  existing convention — no shared mutable cell, no memoization to break).
- Absent/empty entry: existing code path, byte-identical.
- **Ambiguity gate:** when lowering a *reference* to a generalized binding,
  if the use-site region type still contains a free `Ty::Var` not covered by
  the *enclosing* def's own `current_poly_tvars`, fail closed with
  SKY-L0102-ambiguous at the use span ("ambiguous instantiation of `<name>` —
  add a type annotation"). This is Divergence D1.

**Backend:** no changes required. Generic `Func.type_params` and generic
`RecShape.type_params` already exist and are exercised by typed polymorphic
bindings today.

## Recursion

Self-recursion and same-module mutual recursion: resolved via the shared var
during the module's own constrain/solve stage (monomorphic within the
group — required for HM decidability), then generalized together at the
module's boundary. Cross-module mutual recursion is unrepresentable (import
cycles are already rejected at link time), so module topo order is a
provably-sufficient upper bound on recursion groups — no def-level SCC
computation is needed.

## Divergences (author in `docs/divergences-from-sky.md` in the same commit)

- **D1 — ambiguous instantiation fails closed.** Where the reference accepts
  via Go's `[]any` erasure, Ipê rejects with SKY-L0102-ambiguous. Sanctioned:
  matches the repo's "prefer concrete over generic codegen" rule; strictly
  safer direction.
- **D2 — `Super`-bounded residual vars stay program-monomorphic in phase 1.**
  The reference generalizes `number`-bounded untyped bindings; Ipê defers
  this to phase 2 (quantify `Super{flex}` too, populate `bounds` keyed by
  synthesized symbols — every enforcement gate, `check_scheme_applications`,
  already exists and needs zero new code when phase 2 lands). Known
  under-acceptance; #110's oracle must whitelist it.
- **D3 — rigid-contaminated untyped defs stay unquantified.** A def whose
  body unifies with a typed sibling's skolem (`f : a -> a; f x = ident x`)
  leaves `ident`'s shared var rigid; phase 1 conservatively excludes rigid
  roots from generalization. Known under-acceptance, phase-2 item after a
  skolem-escape review.

All three are **under-acceptance** (Ipê rejects programs the reference
accepts) — the safe direction. Zero over-acceptance divergences: instantiated
scheme vars are always plain `Flex` (same shape typed instantiation already
produces), so no new Super-flex/Super-rigid meeting points exist and
`literal_added_to_parametric_skolem_is_rejected` is untouched.

## Test matrix (must all pass before landing)

**Existing, unchanged:** the full `sky_types` suite (87 tests) except one
deliberate update — `untyped_polymorphic_use_at_two_types_is_rejected` KEEPS
asserting rejection (same-module reuse; matches reference parity per the R1
repro above), doc comment updated from "M2a limitation" to "reference-parity
semantics."

**New unit tests (`sky_types`):**
1. Cross-module untyped helper used at 2 types from 2 importers — accepted,
   E2E run-verified.
2. Same-module untyped helper used at 2 types — still rejected.
3. Untyped value binding (`empty = []`) at 2 element types cross-module —
   accepted, E2E run-verified (proves no value restriction).
4. Chained cross-module untyped helper (`twice x = Lib1.ident (Lib1.ident x)`) —
   proves discharge-before-generalize ordering.
5. Same-module recursive + mutually-recursive untyped pair, used
   polymorphically from outside the group — accepted.
6. Obligation-gated def (`getName r = r.name`) — single-record-type use still
   accepted (gate fallback preserved), two-record-type cross-module use still
   rejected (D2/row-conservatism witness, #56 territory).
7. `Super`-bounded untyped helper (`plus a b = a + b`) used at `Int` in one
   module, `Float` in another — still rejected (D2 witness).
8. Rigid-contaminated def unchanged (D3 witness).

**Fuzzer additions (before landing, per backlog's own mandate):**
- Well-typed multi-module templates: cross-module 2-type reuse; value
  binding at 2 types; Number-bounded helper (documents D2); recursive pair.
- Ill-typed categories: same-module 2-type use must still reject (the
  #66-N false-acceptance canary); cross-module use at an incompatible
  instantiated type → ordinary SKY-T0001.
- A `#66` no-panic run over every new template, clean.

**Goldens:** byte-identical emission required for every program with no
boundary-free untyped defs (assert via a one-shot diff report across the
existing golden corpus). New goldens only for genuinely-newly-generic defs,
diffed and reviewed explicitly — never silently accepted churn.

**Instrumentation (build, keep as a permanent asset):** `SKY_DEBUG_UNTYPED=1`
dumps, per untyped def, its root/zonked-boundary-type/quantified-set/
exclusion-reason-per-excluded-var, and per cross-module discharge its
instantiation map. Cheap (~50 lines), pays for itself on the next hard
inference bug.

## Explicitly out of scope for this fix

- Local `let` generalization (permanent parity non-goal — the reference's
  `CLet` doesn't generalize either; already documented on SKY-L0102's
  explain page).
- Row-tail quantification (schemes never contain open tails — `zonk`/copy-walk
  read records back closed; #56 territory, co-design later if row-poly work
  needs it).
- AUD-13's `Ty::AnyWildcard` split (sequenced independently; this design
  adds no colliding var population).
- ex27's erased-`any` ctor-payload handling (different root cause entirely —
  land Class 1 first since it shrinks the free-`Ty::Var` population reaching
  ex27's target lowering gates, but the two designs never need to touch the
  same code).
- Phase 2 (bounds-carrying generalization, lifting D2/D3) — explicitly
  deferred, machinery is additive-only when it lands.

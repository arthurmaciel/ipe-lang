# Class 1 (inference-cluster #2) — guardian design question list

> Asker-stage output of the brainstorm→design→spec pipeline for the
> "untyped top-level binding shares ONE monomorphic var across the linked
> program" bug (blocks Tier-1's entire sweep-green→seal→#110→#37→#59→push
> chain). Produced by a Fable-model research agent, 2026-07-09. Next stage:
> 3 independent reasoners answer every question + propose a full design;
> orchestrator runs a cross-critique round; then synthesizes ONE spec.
>
> No fix is proposed here by design — this is the question list only.

## Verified core mechanism (context)

- Registration pass mints **one flex `VarId` per untyped top-level def**,
  stored in `untyped: BTreeMap<(home, name), VarId>` —
  `crates/sky_types/src/constrain.rs:1224-1227` (mint), `:867` (map decl),
  deferral rationale at `:1171-1180` ("needs rank-based
  let-generalisation, which the M2a solver does not yet model").
- Every `VarTopLevel` reference to an untyped binding returns that **same
  shared `VarId`** — `constrain.rs:2015-2016`, reached from the expression
  walk at `:2317-2319`.
- Typed bindings instantiate their annotation **fresh per reference** via
  `instantiate_tracked`, recording a `SchemeApp { home, name, vars, span }`
  — `constrain.rs:2006-2014`, instantiation machinery `:1490-1616`,
  `SchemeApp` struct `:915-929` (`home` field is the AUD-05 fix, `5f3594b`).
- The untyped def's body is tied to the shared var by one `eq` —
  `constrain.rs:1749-1771`.
- Read-back: `env` = typed annotations verbatim + zonk of each untyped
  shared var — `crates/sky_types/src/lib.rs:357-362`.
- The lowerer lowers untyped defs with `type_params: Vec::new()` and
  **fails closed with SKY-L0102** on any free `Ty::Var` in a param/return
  slot — `crates/sky_lower/src/lower.rs:3707-3786`, `:3858-3878`.
- The Haskell reference does **not** implement rank-based
  let-generalization either: `SolverState._rank` (`../sky/src/Sky/Type/Solve.hs:508`)
  only threads into fresh `Descriptor`s; `CLet` does env-save/solve/restore
  with **no generalize step** (`Solve.hs:1360-1405`); untyped names go
  through `CLocal` = shared env var (`Solve.hs:1300-1322`). Cross-module
  polymorphism comes from **per-module topo compilation +
  `generaliseToAnnotation`** at module export (`Compile.hs:11488-11494`),
  confirmed by this repo's own prior verdict
  (`docs/architecture/compiled-source-stdlib-modules.md:229-252`).

## §0 — Blocking contradictions (resolve first)

1. Backlog says "rank-based let-generalization"; the reference only
   generalizes at module boundaries (annotation-driven, not whole-program
   let-generalization per this repo's own prior verdict). Which semantics
   is the target: (a) reference parity — module/home-boundary
   generalization only, (b) full rank-based let-generalization including
   same-module/local `let` polymorphism (strictly more permissive than the
   reference — needs a `docs/divergences-from-sky.md` entry), or (c)
   per-def generalization in dependency order? Who decides?
2. Do ex12/ex37 (cross-module leaks) need only module-boundary
   generalization, while ex00 (single-module `Main.sky`) may need
   same-module generalization? Has anyone proven which boundary each
   crosses?
3. Generalized untyped bindings will have free `Ty::Var`s exactly where
   the lowerer's SKY-L0102 fail-closed gate currently fires
   (`lower.rs:3721-3724`, `:3864-3868`), and an untyped binding has no
   source-level type-var names to reuse. Emit-as-Rust-generics (mirroring
   typed `type_params`) vs per-call-site monomorphization (mirroring the
   reference's `CallInstanceRecord`/`Monomorphise.hs`) are mutually
   exclusive strategies — which, and gated on what?
4. `untyped_polymorphic_use_at_two_types_is_rejected`
   (`crates/sky_types/src/lib.rs:2609-2626`) currently asserts same-module
   untyped polymorphic use MUST be rejected, doc-commented "a sound
   rejection." Any same-module generalization flips this test — under
   which design option does it flip vs stay, and must it match the
   reference compiler's observable behavior for the differential
   oracle (#51/#110)?

## §A — Current architecture

5. Any consumers of `Generated::untyped` beyond `constrain.rs:1762,2015`,
   `lib.rs:360-362`? (LSP, doc, lowerer — confirm none break on a shape
   change from `VarId` to a scheme.)
6. What must `regions[(home, use_span)]` contain for a reference to a
   generalized untyped binding post-fix? Which lowerer paths read use-site
   regions of top-level references today, and what do they assume?
7. Can a use site legally be reached before its binding's scheme exists
   (forward reference — pass 1 at `:1181-1229` allows this today)? How do
   forward references to untyped bindings interact with generalization
   order?
8. `link::link` processes modules in dependency-first topo order
   (`crates/sky_canon/src/link.rs:22-25`, built by
   `crates/skyc/src/project.rs:261`, import cycles rejected). Is
   within-module source order sufficient for generalize-before-use, or
   does the design need a def-level dependency graph/SCC (mutually
   recursive same-module defs; forward-declared helpers)?
9. Constraints solve strictly in generation order in one batch
   (`crates/sky_types/src/solve.rs:5-9`, "no nested let-generalisation to
   model"). Generalizing needs a binding's body settled before
   instantiation elsewhere. Interleaved constrain-solve-generalize per
   def? Rank/pool bookkeeping inside one solve? A multi-round fixpoint
   (reference's "two-pass fixpoint solve",
   `compiled-source-stdlib-modules.md:69-70`)? What happens to
   `solve_attributed`'s home-attribution contract (`solve.rs:35-43`, the
   `9c44642` fix) under each option?
10. Local `let` is explicitly monomorphic today (`constrain.rs:2352-2357`,
    "M1 does not generalise let-bound names"). In scope, out of scope, or
    a recorded divergence? (Elm generalizes lets; the reference's `CLet`
    doesn't either.) SKY-L0102's explain page already documents
    `let f = identity` as a known gap.

## §B — Generalization mechanics

11. What exactly gets quantified — plain flex vars, `Super`-bounded vars
    (Number/Ord/Eq/Show, `constrain.rs:895-904`), open row tails, vars
    shared with other still-unsolved bodies? How does the design avoid
    generalizing a var that a later def's constraints should still pin
    (the classic rank/level escape problem)? If not ranks, what's the
    ownership criterion?
12. No rank/level notion currently exists in `crates/sky_types`
    (`unionfind.rs`'s rank is unrelated tree-balancing; `Content` has no
    level field). Cost of adding one, given `unify.rs`'s merge rules
    (which rank survives a union on merge — reference/Elm answer: min-rank)
    and AUD-12's exhaustive `Content` match (`lib.rs:253-304`)?
13. Typed bindings' bounds live in `SolvedTypes::bounds` keyed
    `(home,name)` (`lib.rs:316-337`), gate-checked by
    `check_scheme_applications` (`lib.rs:343-381`). Untyped defs never
    populate `typed_rigids` (`constrain.rs:1730-1731` is typed-arm only).
    Where do a generalized untyped binding's bounds live? Does numeric
    defaulting (`lib.rs:260-304`) run before or after its generalization?
14. Typed instantiation goes `Ty` → fresh UF vars via `instantiate_in`
    keyed on `Ty::Var(raw_symbol_id)` (`constrain.rs:1535-1616`, including
    the `"any"` wildcard exemption at `:1574-1591` that AUD-13 flags as
    fragile). A generalized untyped scheme has no symbol-named vars —
    reify as `Ty` with synthesized ids (risking AUD-13's exact
    interner-raw/ordinal collision, `constrain.rs:1576-1584`) or as a new
    `Scheme { quantified, body }` type? Does this force AUD-13's
    `Ty::AnyWildcard` split to land first?
15. Recursion: an untyped self-recursive def currently works because the
    shared var IS the recursive occurrence's type. Generalization must
    keep recursion monomorphic within the binding group. How are
    same-module mutually-recursive untyped defs grouped (SCC)? Is
    cross-module mutual recursion unrepresentable already (import cycles
    rejected), letting the design use module topo order as an SCC upper
    bound?

## §C — Downstream (lowerer/backend) consequences

16. If generalized as Rust generics: `lower_def`'s untyped arm hardcodes
    `type_params: Vec::new()` (`lower.rs:3753-3765,3781`); `bounds_for`
    reads `SolvedTypes::bounds` by annotation-var Symbol
    (`lower.rs:3807-3856`). Synthesized names, collision-freedom vs
    AUD-01's `any_param_binders` (`d6e0a7e`), and `poly_var_map`
    (`lib.rs:110`, `lower.rs:3529-3556`) population for untyped defs?
17. If per-call-site monomorphization instead: reference records
    `CallInstanceRecord`s at every CForeign solve (`Solve.hs:1324-1340`),
    monomorphizes in `Monomorphise.hs`. No equivalent ledger exists in
    `crates/sky_lower`. Given typed polymorphic bindings ALREADY emit as
    single generic fns rustc monomorphizes (`lib.rs:2390-2394`), why would
    untyped bindings deserve a different backend strategy?
18. Zero-param untyped value bindings (`lower.rs:3770-3785`) lower via
    `ir_type_from_ty`. Does the design generalize value bindings at all
    (value restriction), or only functions? Reference's
    `generaliseToAnnotation` quantifies every free TVar of a solved export
    — apparently no value restriction. Rust can't emit a generic value
    directly — what's the plan for `empty = []` used as two element types
    from two modules?
19. Enumerate lowerer sites that currently depend on the leak (read a
    concrete type only because some OTHER module's use pinned the shared
    var program-wide) — e.g. `split_unannotated_sig` succeeding today
    precisely because of this. Include `lower.rs:3470-3484`.

## §D — Example-specific behavior (confirm diagnosis before design)

20. **ex12:** exact var-unification chain from `Lib/AuthHandlers.sky:59`
    (`info.message`, deferred FieldAccess, `constrain.rs:944-968`,
    resolved at `lib.rs:200-207`; `Error` ctor payload is
    freshly-instantiated per use, `constrain.rs:1119-1165`) to
    `Page/Roadmap.sky:50`. Which untyped shared binding is the bridge?
    Instrument: print `(home,name)→VarId` for the `untyped` map + the
    union-find rep chain of the failing var, confirm it's precisely
    `untyped`-map sharing (not a second cause, per the #1-bug precedent
    that turned out to be span mis-attribution).
21. **ex37:** is the leaked `Dict String String` genuine form-data, or the
    `pin_any_in_ty` ctor-payload injection (`constrain.rs:1144-1156`)?
    Same resulting type, different machinery — which reaches `cart`?
22. **ex00:** minimal repro of `found Error, expected Var(a)` — is
    `Var(a)` a zonked still-flex var surfacing in a diagnostic (dubious
    UX regardless of fix)? Does it involve compiled-source stdlib modules
    (required fully-annotated, `compiled-source-stdlib-modules.md:254-258`)
    — can an annotated stdlib module still leak via its own untyped
    locals?
23. After the fix, do all three examples reveal NEW first blockers (per
    the batch-implement-then-remeasure "sweep peels like an onion"
    pattern)? Cheap way to predict the next blocker per example?

## §E — Scope of the "400+ tests" risk

24. Populations at risk: 87 tests in `crates/sky_types` (69 in `lib.rs`),
    plus lowerer/backend/E2E goldens ("byte-identical M2a goldens",
    `docs/architecture/seal-curried-funcvalue-design.md:392`), plus the
    examples sweep. Which categories change: diagnostics text/spans,
    accepted/rejected program sets, emitted Rust (golden churn from new
    generics), solver perf/budget (`SKY_SOLVER_BUDGET`, `solve.rs:21`)?
    Agreed budget for intentional golden churn, or byte-identical required
    for previously-unambiguous programs?
25. Does introducing generalized-then-instantiated untyped schemes create
    new Super-flex/Super-rigid meeting points that could re-open the
    parametric-generic gate (backlog #1 fix, regression
    `literal_added_to_parametric_skolem_is_rejected`)?
26. Any current soundness reliance on the monomorphism — a test/path where
    program-wide pinning is what REJECTS an ill-typed program, which fresh
    instantiation would now accept? Is
    `compiled-source-stdlib-modules.md:248-252`'s "monomorphism only
    over-rejects" argument still airtight once bounds-checking for
    untyped schemes is new code?

## §F — Verification strategy

27. Backlog mandates "#66 no-panic fuzzer + adversarial review, not just
    the gate." Do any `scripts/fuzz-well-typed.sh` templates exercise
    unannotated polymorphic bindings at multiple types? New template
    shapes needed BEFORE the fix lands: untyped helper used at 2 types
    cross-module; untyped helper with a Number bound; untyped value
    binding; recursive untyped pair.
28. Should `scripts/fuzz-ill-typed.sh` gain a category for "untyped
    binding used at two incompatible types in a truly monomorphic
    position" to prove the fix doesn't over-generalize (false acceptance —
    the #66-N soundness death mode)?
29. Differential-oracle caveat: programs where Ipê (post-fix) accepts and
    the reference rejects need sanctioned-divergence entries authored
    ALONGSIDE this fix, or #110's oracle will flag new acceptances as
    regressions.
30. Instrumentation plan for validating diagnosis before coding — a debug
    dump of `untyped`-map reps + which module's constraint first pinned
    each shared var, run over ex00/ex12/ex37. Build it if it doesn't
    exist; keep it as a solver-debugging asset.

## §G — Adjacent bugs: shared root cause or not?

31. ex27's proposed fix (bare wildcard `any` → `IrType::Json`) vs
    `pin_any_in_ty`'s existing ctor-payload pin to `Dict String String`
    (`constrain.rs:1144-1156`) — reads as a DIFFERENT root cause from
    untyped-sharing, but both touch how free/wildcard vars reach
    `lower.rs`'s SKY-L0102 gates (`:3721`,`:4612`,`:4648`). Sequencing:
    which first, and does ex27's `IrType::Json` decision constrain what a
    generalized-untyped `Ty::Var` may lower to?
32. Bug-29 (`lower.rs:3489-3528`, regions-not-env rationale `:3575-3579`)
    reads the body region's solved type to replace a spurious `any`
    generic, keyed on "a bare `Ty::Var` at this position means X"
    (`:3515-3521`). Does generalization make this heuristic mis-inject on
    legitimately-quantified vars of untyped view helpers?
33. AUD-13's `Ty::Var(u32)` conflation (`constrain.rs:1576-1584`) — does
    generalization's third var-id population force AUD-13's design to be
    joint rather than sequenced?
34. #56 (row-poly subset/superset) — generalization quantifies open row
    tails (`RowTail::Open`, `constrain.rs:1560-1571`). Co-design needed,
    or can rows stay conservatively monomorphic in phase 1?

## Meta-question for the reasoners

Given the reference's actual architecture (per-module solve + boundary
generalization) matches the Rust port's EXISTING module topo order
(`link.rs:22-25`, `project.rs:261`) and EXISTING instantiation machinery
(`instantiate_tracked`, `SchemeApp`, `check_scheme_applications`) — should
the baseline candidate be "generalize untyped defs at def-completion in
linked order, reusing the typed-binding instantiation path" (reference
parity), escalating to Elm-style ranks/pools only if that baseline
provably can't clear ex00? Cost both options against the same §E/§F bar.

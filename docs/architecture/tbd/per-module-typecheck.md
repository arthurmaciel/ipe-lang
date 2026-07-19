# Per-module `typecheck` — the solver-internal redesign

Status: design + implementation plan (checkpoint). The query SEAM
(`ipe_db::typecheck_module`) and its result-identity proof have landed; this
document specifies the remaining `ipe_types` work — the genuinely-per-module
solve that swaps in as `typecheck_module`'s BODY with zero consumer changes.

## 1. What landed, what remains

**Landed (the query contract):** `typecheck_module(db, root, entry, module)`
returns a `ModuleTypes` — the `(home, _)`-keyed slices of the whole-program
`SolvedTypes` filtered to one module (`env`/`regions`/`expected`/`bounds`). It
is memoized per `(root, entry, module)` and its result is byte-identical to
filtering the whole-program solve, proven by
`typecheck_module_projection_matches_whole_program`
(`src/compiler/db/tests/phase4_seams.rs`): the per-module slices partition the
whole-program maps exactly, and two same-named cross-module bindings
(`A.shared : Int` vs `B.shared : String`) project to distinct types. Every
home-keyed LSP handler (hover, completion, `expected_type_at`) reads this query
by name instead of whole-program `typecheck`; the spans it returns are already
home-scoped, so those handlers dropped their home-comparison bookkeeping.

**Today's dependency:** `typecheck_module` depends on the whole-program
`typecheck`, so it inherits the coarse invalidation floor — a semantic edit
anywhere re-solves the world and re-projects every module. The projection is
cheap and its value backdates (a module whose slice is byte-equal after an
unrelated edit cuts its dependents), but the solve itself is still O(program).

**Remains (the latency unlock):** narrow `typecheck_module`'s dependency from
the whole-program solve to *this module's own scoped solve seeded from its
deps' typed interfaces*, so a body-only edit re-solves only the edited module
and its importer closure. This is the `ipe_types` redesign the ADR 0034 and the
complete-LSP plan (§2/§7/F-6) track as a separate effort.

## 2. Why it is a redesign, not a refactor

`ipe_types::infer_attributed` builds ONE `unionfind`-backed constraint graph
over the ENTIRE linked module (`constrain::Builder::run`), and every post-solve
pass operates over that single joint constraint set:

- **Boundary Scheme Promotion** (`promote_untyped_boundaries`) already walks
  `module_order` dependency-first, generalizing each module's untyped defs only
  after its deps are generalized — but it discharges cross-module references
  (`PendingInstantiation`s) against the joint graph, not against a materialized
  per-module scheme table.
- The **field-access / record-update deferred fixpoint** (`resolve_deferred`)
  interleaves across module boundaries: a record update in one module can pin a
  field type a downstream module's access needs.
- **Route-witness** and **routed-`Live.app`** checks read settled variables that
  the joint solve produced.

A per-module `typecheck(ModuleId)` must therefore re-derive Ipê's cross-module
generalization semantics on top of a *scoped* solve seeded from deps' TYPED
interfaces (schemes — not the canon-level `ModuleExports` that `module_interface`
carries today). The byte-identity golden-oracle SEAL pins the exact
fresh-variable numbering the joint solve produces; a scoped solve must reproduce
it or the SEAL breaks. Under-invalidation (a stale build that looks correct) is
the primary hazard and outranks every latency gain (ADR 0032).

## 3. Design

### 3.1 A typed module interface query

Add `typed_interface(db, root, module) -> Arc<TypedInterface>`, the generalized
schemes of a module's *exported* bindings (and their bounds), projected out of
that module's own scoped solve. This is the typed analogue of the existing
canon-level `module_interface` (which carries only name/arity/alias-body export
shape). It is the backdating firewall for the typed tier: when a body-only edit
re-solves module A but its exported schemes come out equal, `typed_interface(A)`
backdates and A's importers' scoped solves stay valid.

`TypedInterface` = `BTreeMap<Symbol, Scheme>` where `Scheme` is a generalized
type plus its super-type bounds (the `(Vec<Symbol> quantified, Ty body,
BTreeMap<Symbol, TyBounds>)` shape already reconstructed in `infer_attributed`'s
read-back). Span-erased (an exported scheme's *body spans* must not be part of
interface identity, or a span shift over-invalidates — the same over-approximation
note `module_interface` carries).

### 3.2 A scoped per-module solve

Add `ipe_types::infer_module(module: &canon::Module, deps: &BTreeMap<ModPath,
TypedInterface>, interner) -> ModuleSolved`. It runs `Builder::run` over ONE
canonical module (not the linked merge), instantiating each cross-module
reference against the dep's scheme from `deps` (fresh per use site, exactly as
`instantiate_tracked` does today for in-module references), then runs the
post-solve passes scoped to this module's constraints. `typecheck_module`'s body
becomes: demand `typed_interface(dep)` for each resolved dep, then
`infer_module(this_module, dep_interfaces)`.

The whole-program `typecheck` (and thus `lower_program` and emission) stays on
`infer_attributed` over `linked_program` — the byte-identity SEAL path is
untouched. The two must agree on every module's result; a parity test asserts
`infer_module`'s per-module output equals `infer_attributed`'s whole-program
slice for that module, across the golden corpus (the same shape as the landed
`typecheck_module_projection_matches_whole_program`, but now proving the SCOPED
solve — not a projection — matches).

### 3.3 Sequencing hazard

`infer_module` for a module that participates in cross-module untyped-binding
generalization (Boundary Scheme Promotion across an import edge) needs its deps'
*generalized* schemes, which is exactly what `typed_interface` provides — the
dependency-first demand order (`typed_interface(dep)` before `infer_module(this)`)
reproduces `promote_untyped_boundaries`' `module_order` walk as a salsa
dependency edge instead of an in-pass loop. A cyclic import graph is already
rejected by the driver's IPE-N0021 gate before any solve, so the scoped demand
never hits salsa's dependency-cycle panic.

## 4. Test plan

- **Scoped-vs-whole parity** (correctness SEAL): `infer_module`'s output equals
  `infer_attributed`'s whole-program slice for every module, across the golden
  corpus + the adversarial edit sequences the clean-vs-incremental gate uses.
- **True per-module invalidation** (the unlock): editing module C's body in a
  `{A, C, Entry}` program where A and C share no import edge leaves
  `typecheck_module(A)`'s memo untouched — the assertion the current
  `typecheck_is_program_wide_not_per_module` test documents as NOT holding
  today flips to holding.
- **Typed-interface firewall**: a body-only edit to an exported binding that does
  not change its scheme backdates `typed_interface` and does not re-solve
  importers.
- **Byte-identity SEAL**: `clean_vs_incremental_parity` stays green (the
  whole-program emission path is unchanged).

## 5. Consequences

- Warm settled-edit diagnostics drop from one whole-program solve to one
  module's scoped solve + its importer closure — the plan's headline latency
  unlock (targets: settled body-edit diagnostics < 100 ms).
- `ipe watch` and `ipe build --watch` inherit the same win for free (they are
  the sibling consumers of the same queries).
- Zero LSP handler changes: every consumer already reads `typecheck_module`;
  only that query's body changes.

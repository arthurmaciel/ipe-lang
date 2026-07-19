Status: Accepted
Date: 2026-07-19

# 0035. Per-module typecheck behind closed typed interfaces

## Context

The `typecheck_module` query — the per-module types the language server reads
for hover, completion, and expected-type — was a projection of the
whole-program solve: one joint constraint graph over the linked merge of every
module, filtered to one module's home. Correct, but with a program-wide
invalidation floor: a settled body edit anywhere re-solved the world before
any module's slice could be served, bounding settled-edit editor latency by
program size.

Genuinely per-module solving is constrained by three facts of Ipê inference:

- **Boundary Scheme Promotion.** An unannotated top-level binding is
  monomorphic within its module and generalized at the module boundary — but
  only its obligation-free residual variables are quantified. A residual that
  carries an obligation (a `Number` variable, a pending field access, an open
  record tail) stays *shared program-wide*, so an importer's use site can pin
  it. Information flows AGAINST the import direction: `double x = x + x`
  becomes `Float -> Float` because an importer wrote `double 1.5`.
- **Deferred obligations.** Field-access and record-update obligations resolve
  in a joint fixpoint after the main solve; a chain can cross module
  boundaries through those same shared residuals.
- **Program-wide defaulting.** Numeric/SQL defaulting pins unconstrained
  obligation variables only after every module has spoken. A per-module solve
  that defaults early would report `Int -> Int` for a binding the joint solve
  makes `Float -> Float`.

No dependency-first per-module solve can reproduce reverse-edge information
flow. And the correctness bar is absolute: under-invalidation — or any scoped
answer the joint solve disagrees with — outranks every latency gain (ADR
0032); the language server must never show a type the build would not infer
(ADR 0034).

## Decision

`typecheck_module` gets a genuinely-per-module body, gated fail-closed by
**closed typed interfaces**:

- `ipe_types::TypedInterface` is a module's typed export surface: each
  exported binding's generalized scheme (a typed binding's normalized
  annotation with its recorded obligations; an untyped binding's
  boundary-promoted scheme) plus the module's union definitions (constructor
  payload types for an importer's references, patterns, and exhaustiveness).
  Schemes are span-free, and quantified variables are reified to canonical
  ids, so a body-only edit that preserves the exported schemes yields a
  byte-equal interface.
- `ipe_types::infer_module` solves ONE module's constraints, instantiating
  every cross-module reference against the dep's interface scheme fresh per
  use site — the same instantiation discipline as an annotated binding — and
  shares one inference core with the whole-program solve, so the two paths
  cannot drift.
- A module's scoped result is served ONLY when (a) its own scoped solve is
  green, (b) every exported scheme is **closed** — no reachable residual
  non-quantified variable, checked BEFORE defaulting so defaulting cannot
  disguise an importer-pinnable scheme as concrete — and (c) every dependency
  interface is closed (transitive by construction). Every other case falls
  back to the whole-program projection, which surfaces the joint solve's
  exact types and diagnostics. Closedness is precisely the condition under
  which the joint solve decomposes: every cross-module constraint then flows
  through a scheme instantiation both solves perform identically.
- In the query graph, `infer_module_scoped(module)` demands
  `typed_interface(dep)` for each resolved dep — the dependency-first
  generalization order is a memoized dependency EDGE, not an in-pass loop —
  and `typed_interface` is the typed tier's backdating firewall: a dep body
  edit that preserves the interface re-solves the dep alone and leaves every
  importer's memo standing.
- `ModuleTypes` values are **normalized** on both paths (canonical
  first-encounter renumbering of solver-variable ids, which no consumer
  reads): the scoped result and the joint projection become byte-comparable,
  and the fallback path's memos stop churning on joint-solve renumbering
  noise.
- The build path — whole-program `typecheck` → lowering → emission — is
  untouched; emitted bytes cannot change.

Alternatives rejected:

- **Generalize obligation-carrying exports into bounded schemes** (so
  `double : Number a => a -> a` instantiates per importer). Rejected: it
  changes program meaning — the joint solve gives every importer the ONE
  pinned type, and two importers at different types are a type error, not two
  instantiations. A scoped tier must not redefine the language.
- **Serve the scoped result and patch divergences case-by-case.** Rejected:
  the divergence set is open-ended; a gate that enumerates hazards fails
  open. Closedness is a structural property that makes the entire divergence
  class unrepresentable.
- **Alpha-equivalence comparison instead of normalization.** Rejected: a
  bespoke equality invites drift; canonical renumbering at the query boundary
  gives plain value equality to salsa and to tests.

## Consequences

- Settled body-edit latency for editor features on modular programs drops to
  one module's scoped solve plus its importer closure; an edit to an
  unrelated module leaves a module's memo untouched.
- A red edit in one module no longer blanks unrelated modules' hover and
  completion: their scoped results stand on closed interfaces, while
  diagnostics continue to come from the whole-program query, verbatim.
- Modules whose exported bindings are unannotated and importer-pinnable
  honestly refuse the scoped path and keep the whole-program floor — an
  incentive, not a penalty: annotating exports is what makes a module's
  boundary a real contract.
- The load-bearing invariant: wherever the scoped path engages, its result
  MUST equal the normalized whole-program projection. The scoped-vs-whole
  parity gate (`per_module_typecheck_parity`) enforces this across the golden
  corpus and an adversarial warm edit sequence; the clean-vs-incremental gate
  continues to pin the emission path byte-identically. Any future inference
  feature that adds a cross-module channel must either flow through the typed
  interface or mark the affected interfaces open.

# Incremental compilation — phases beyond the current graph

Status: design proposal. No implementation yet. This is a forward-looking spec:
it names the queries that would become tracked, the invalidation model, and a
dependency-ordered plan. It supersedes nothing already shipped — it extends the
salsa graph recorded in ADR 0032, ADR 0034, and ADR 0035.

## Problem statement

The compiler front end and back end already run as a salsa query graph
(`src/compiler/db/src/lib.rs`). A body-only edit to one module re-runs only the
stages downstream of that module, and a language server and `ipe watch` both
read the same memoized queries rather than re-analysing. The correctness bar is
absolute and inherited from ADR 0032: **under-invalidation — a stale build that
looks correct — outranks every efficiency gain**. Every phase below is judged
against that bar first (principle 3, soundness) and latency second (principle 4).

The open question this doc answers: given how far the graph already reaches,
which *further* stages should become incremental, at what granularity, and in
what order — without ever opening a stale-cache hole.

## Current baseline (what is already tracked)

The graph is deeper than "phase 1". As it stands today:

- **Inputs.** `SourceFile` (module path + text + trust origin) and `SourceRoot`
  (the in-scope file set) are the only write points; a driver-boundary
  `sync_source_root` reconciles them with byte-equal no-op suppression.
  `BuildConfig` is a *separate* input carrying emit-only fields (SQL driver, FFI
  emission bundle, target, wasm settings, production flag) at salsa field
  granularity.
- **Front end, per module.** `parse`, `imports`, `resolve_imports`,
  `canonicalize`, and `module_interface` are per-`SourceFile` tracked queries.
  `module_interface` is the canon-tier backdating firewall: a body-only edit
  whose export surface is unchanged re-runs `canonicalize` for the edited module
  but backdates its interface, so importers never re-canonicalise.
- **Typecheck, per module (ADR 0035).** `infer_module_scoped` solves ONE
  module over its deps' *closed* `typed_interface`s; `typed_interface` is the
  typed-tier firewall. `typecheck_module` serves the scoped result when it is
  provably faithful and otherwise falls back to the whole-program `typecheck`
  projection — fail-closed, never a scoped answer the joint solve disagrees
  with. The language server reads this per-module query.
- **Coarse whole-program spine.** `linked_program`, `typecheck`,
  `lower_program` are one-per-program seams. They memoize (a no-op re-save
  executes nothing) but any reachable edit re-runs them in full.
- **Emit, per Rust file.** `program_rust_file_ids`, `emit_spine_file`,
  `emit_rust_file(RustFileId)`, and `emit_manifest` split emission per output
  file. A body edit to an unrelated module re-executes its `emit_rust_file`
  against a byte-identical IR slice, produces a byte-identical string, backdates,
  and the on-disk write skips — preserving cargo's per-unit incrementality. A
  0-or-1-module program collapses to the byte-identical single-`main.rs` path.
- **Consumers.** `ipe watch` persists ONE warm `IpeDatabase` for the whole
  session, clones it per compile worker, and re-syncs inputs between settled
  batches (rust-analyzer's cancellation pattern). The language server holds one
  warm DB and mutates inputs on document change. Both are genuine warm-DB
  consumers today.
- **Cross-process cache.** `ipe build` still starts a *cold* DB every
  invocation; `compute_project_key` (a SHA-256 content address over every
  module's path/origin/text plus the emit-relevant config) fronts an on-disk
  cache of the emitted-project `String`s (`src/ipe-cli/src/cache.rs`). A cold
  start that hits the cache skips parse→…→emit entirely.

So the incremental story is complete for a warm, long-lived process editing one
module at a time. The remaining gaps are all at the edges: the coarse
whole-program floor (`typecheck`/`lower`), cross-*process* warmth, FFI as a
tracked input, and the reload/hotpatch payoff that warmth unlocks.

## The remaining gaps, ranked

1. **The lowering floor.** `typecheck` is genuinely per-module for the LSP, but
   `lower_program` (and therefore every emit query, transitively) still depends
   on the coarse whole-program `typecheck` + `linked_program`. A body edit
   anywhere re-runs the whole solver and the whole lowerer before any emit query
   can early-cut. For `ipe watch` on a large project this is the dominant
   settled-edit cost. This is the largest real latency gap.
2. **Cross-process cold start.** Every `ipe build` throws the salsa graph away.
   The on-disk `EmittedProject` cache recovers the *whole-project* case (nothing
   changed → full hit) but gives nothing for the common "one module changed"
   case: the key is whole-project, so a single edit misses the entire entry and
   re-runs the world cold.
3. **FFI as a hidden coarse input.** FFI introspection sits outside salsa as a
   content-hash cache regenerated by `ipe add/install`; the resulting bindings
   ride on `BuildConfig.ffi` as one opaque blob. A change to one installed
   crate's interface invalidates emission wholesale even when only one Ipê
   module imports it.
4. **Interface over-approximation.** `module_interface` carries exported alias
   *bodies with spans*, so a span shift in an exported alias re-canonicalises
   importers with no semantic change — over-invalidation (sound, but wasteful).
5. **Hotpatch / reload payoff.** Warmth is in place but nothing yet exploits it
   beyond skipping recompute: `ipe watch` still does a full `cargo build` +
   process restart on every settled batch. A per-module-diff'd reload is
   unbuilt.

## Approaches considered

### Approach A — persist the salsa graph to disk

Serialize salsa's memo tables so a fresh `ipe build` process resumes a warm
graph. Directly attacks gap 2 and would, in principle, give per-module cross-
process incrementality "for free".

Rejected as the primary path. Salsa's on-disk durability story is immature, and
every memoized value that embeds an `ipe_intern::Symbol` is a raw index into the
*producing process's* interner — meaningless in a fresh process (the exact
blocker `cache.rs` already documents for IR persistence). Persisting the graph
soundly demands a symbol-relocation pass over every memoized type, plus a
cache-coherence proof that a resumed graph cannot serve a value the current
inputs would not produce. That is a multi-session redesign with a large stale-
cache surface — the highest-risk option against principle 3, for the narrowest
consumer (`ipe build`, which already has the whole-project on-disk cache).

### Approach B — split the coarse floor into genuinely per-module lower/emit

Make `lower_program` and the emit tier depend on per-module `typecheck_module`
and a per-module `lower_module(ModuleId)`, so a body edit re-lowers and re-emits
only the edited module's file. Attacks gap 1 — the largest latency gap — and
composes with the emit split that already exists.

The blocker is real but bounded (recorded in `lib.rs`): `ipe_lower::lower` mints
fresh-symbol pools (`eta_`, `cap_`, `arg_`, …) sized from *whole-program* facts
(`max_def_arity`, destructure-site counts). A naive per-module lowering would
perturb the exact fresh-name numbering the golden-oracle SEAL pins. The fix is a
*deterministic per-module fresh-name allocation scheme* keyed on stable
per-module facts (module path + local def index), decoupled from whole-program
counts, with a clean-vs-incremental parity gate proving byte-identity against
the cold path. This is a structural change to `ipe_lower`, not a refactor — but
it is the change with the highest latency payoff and it *extends* the existing
graph shape rather than replacing it.

### Approach C — per-module cross-process artifact cache

Instead of persisting the salsa graph, extend the on-disk cache from a single
whole-project `EmittedProject` entry to a *per-Rust-file* entry keyed on that
module's content-address slice, mirroring `emit_rust_file`'s in-memory split.
A cold `ipe build` after a one-module edit then hits the cache for every
unchanged file and re-emits only the changed one. Attacks gap 2 with no salsa-
persistence risk: emitted files are pure `String`s (no `Symbol`, no interner
dependency), so they relocate across processes trivially — the property
`cache.rs` already relies on for the whole-project entry.

The subtlety: a per-file key must fold in *everything that file's content
depends on*, which after Approach B is "this module's own text + its transitive
deps' interfaces", not just its own text — otherwise a dep-interface change
serves a stale file. The key therefore composes with the interface firewalls
(`module_interface` / `typed_interface`): a per-file cache key is a hash of the
edited module's text plus its resolved deps' interface hashes.

### Recommendation

**Do B first, then C, then the edge cleanups (4, 3), and hotpatch (5) last.**

Approach A is rejected outright — its risk/consumer ratio is wrong. B is the
biggest single latency win and is a prerequisite for C being worthwhile (a
per-file *disk* cache is only useful once per-file *emit* is decoupled from the
whole-program lower floor). C then extends B's per-file granularity across the
process boundary using the already-proven pure-`String` relocation trick, with
none of A's symbol-relocation hazard. FFI-as-input (3) and span-erased
interfaces (4) are independent, small, and startable in parallel. Hotpatch (5)
is a genuine product feature that rides on B's per-module IR diff and is scoped
last.

## The design

### Which queries become tracked, and their invalidation granularity

| New/changed query | Key | Granularity | Firewall / backdating |
| --- | --- | --- | --- |
| `lower_module(root, entry, module)` | per `SourceFile` | per module | re-runs on this module's `typecheck_module` or `canonicalize` change; a dep-interface-preserving edit elsewhere leaves its memo standing |
| `lower_program` (rebuilt) | per program | assembles per-module IR | becomes a thin *assembler* over `lower_module` outputs; value-equal assembly still backdates emit |
| `emit_rust_file` (re-parented) | per `RustFileId` | per file | depends on `lower_module(that home)`, not the coarse `lower_program`; unrelated edit no longer forces re-execution, only re-*validation* |
| `ffi_package_interface(PackageId)` | per package | per installed crate | new salsa input; `kernel_types` and canon of FFI-importing modules union these in; a one-crate change invalidates only its importers |
| `module_interface` (span-erased) | per `SourceFile` | per module | exported alias bodies reified span-free → a pure span shift backdates instead of re-canonicalising importers |
| per-file disk cache entry | content-address | per file | key = hash(module text ⊕ resolved-dep interface hashes ⊕ emit config); a hit relocates a pure-`String` file across processes |

**Invalidation model: per-module, firewalled by interfaces.** The unit of
invalidation stays the module (`SourceFile`), never the individual declaration.
Per-decl granularity is explicitly out of scope: Ipê's Boundary Scheme Promotion
(ADR 0035) shares residual variables program-wide, so a decl-level solve cannot
reproduce reverse-edge information flow — the same reason `typecheck` stays
whole-program-faithful. The module is the coarsest unit at which a *closed*
interface exists, and a closed interface is precisely the condition under which
the joint solve decomposes. Backdating at the two interface firewalls
(`module_interface`, `typed_interface`, and a new `lowered_interface` if
Approach B needs one) is what turns "re-execute" into "re-validate then early-
cut" for every module the edit did not semantically touch.

### How the LSP and the batch compiler share the graph

Unchanged in shape, deepened in reach. Both consumers already read the same
queries from one warm DB. After Approach B:

- The **language server** gains nothing it must change — it reads
  `typecheck_module` today; `lower_module`/emit are below its diagnostics-only
  floor. Its settled-edit latency improves for free because the coarse floor it
  never touched shrinks (it still never lowers).
- **`ipe watch`** benefits directly: a settled batch re-lowers and re-emits only
  the edited module's file, so the `cargo build` it hands off touches one
  compilation unit. No orchestrator change — it already re-syncs inputs and
  demands `emit_manifest`.
- **`ipe build`** (cold) benefits only after Approach C: the per-file disk cache
  turns a one-module edit into one re-emit + N cache hits instead of a cold
  world rebuild. The warm-DB path (watch/LSP) and the cold-DB path (build) stay
  the same two consumers of the same query definitions; C adds a disk tier
  *below* the cold DB, not a second analyzer.

The load-bearing invariant from ADR 0034 holds throughout: no consumer computes
types, resolution, or lowering differently from the build. Every new query is a
memoized node on the one graph, never a second path.

### Interaction with the content-hash cache

The existing `compute_project_key` whole-project cache is *not* replaced — it
stays as the fast "nothing changed at all" path. Approach C adds a second,
finer tier *beneath* it:

1. Whole-project key hits → serve the whole `EmittedProject` (today's behaviour).
2. Miss → build the warm DB, but before emitting each file, check a **per-file**
   cache keyed on `hash(module text ⊕ dep-interface hashes ⊕ emit config)`.
   Unchanged files hit and are read as pure `String`s; only files whose key
   moved are re-emitted.
3. Store both tiers on success.

The per-file key must be a *superset* of everything the file's bytes depend on.
After Approach B a file depends on its own text and its transitive deps'
*interfaces* (not their bodies) — so the per-file key folds in the same
interface hashes the in-memory firewalls key on. This keeps the two tiers
coherent by construction: the disk key changes exactly when the salsa memo would
have re-executed. Durability interaction: `BuildConfig` field-granularity already
means a `db_driver`-only change re-emits without re-lowering; the per-file key
mirrors that by folding config into the per-file hash, so toggling an emit-only
field invalidates the emit tier but not the (config-independent) lower tier.

### Hotpatching / watch-mode payoff

Approach B makes per-module IR a first-class memoized value, which unlocks a
*diff*-driven reload for `ipe watch`: when a settled batch changes exactly one
module's `lower_module` output and the change is body-only (signatures and the
module's typed interface unchanged), the orchestrator can in principle swap that
one compilation unit rather than restarting the whole process. This is gated
hard by the ADR 0032 reload invariant — **no dynamic-code / `eval` hole, strict
CSP, no `unsafe-eval`** — and by a signature-stability check (a reload that
changed any exported scheme falls back to a full restart, fail-closed). Scoped
last because it is a product feature with its own safety surface, not a compiler-
internals change, and it depends on B landing first.

## Dependency-ordered phased plan

Work packages, marked **startable now** vs **blocked**.

**WP-1 — span-erased `module_interface` (startable now).** Reify exported alias
bodies span-free in `ModuleExports` so a pure span shift backdates. Small,
self-contained, independent of everything else. Parity gate: golden corpus
byte-identity + a dedicated span-shift-only edit that must NOT re-canonicalise
importers (assert via the event-callback memo-hit hook already used in
`phase2_incrementality.rs`).

**WP-2 — `ffi_package_interface(PackageId)` input (startable now).** Turn the
FFI content-hash bundle into a per-package salsa input that `kernel_types` and
FFI-importing canon union in. Independent of B/C. Blocked only in the sense that
its payoff is small until multiple FFI crates are common; do it when FFI lands as
a first-class dependency axis. Gate: a one-crate interface change re-executes only
its importers' queries (event-callback assertion).

**WP-3 — deterministic per-module fresh-name allocation in `ipe_lower`
(startable now; prerequisite for WP-4).** Replace whole-program-sized fresh
pools with a per-module scheme keyed on stable module facts. This is the hard,
load-bearing package. Gate: the clean-vs-incremental parity suite
(`clean_vs_incremental_parity.rs`) must prove byte-identity against the cold
path across the whole golden corpus *before* any per-module lower query is
wired — the fresh-name numbering is exactly what the SEAL pins.

**WP-4 — `lower_module` + re-parent emit (blocked on WP-3).** Add
`lower_module(module)`, rebuild `lower_program` as an assembler over it, and
re-parent `emit_rust_file` onto `lower_module(that home)`. This is where the
latency win from gap 1 lands. Gate: a body edit to an unrelated module must NOT
re-execute that module's `lower_module`/`emit_rust_file` (memo-hit assertion),
AND full golden byte-identity.

**WP-5 — per-file cross-process disk cache (blocked on WP-4).** Extend
`cache.rs` with a per-file tier keyed on module-text ⊕ dep-interface hashes ⊕
config. Gate: cold `ipe build` after a one-module edit reads N−1 files from cache
and re-emits exactly one; a dep-interface change must miss the importer's entry
(no stale file served — the primary stale-cache regression test).

**WP-6 — diff-driven watch reload (blocked on WP-4; product feature).**
Signature-stable single-module reload in `ipe watch`, fail-closed to full restart
on any interface change, hard-gated by the no-`eval`/strict-CSP reload invariant.
Gate: a signature-changing edit MUST trigger a full restart; a reload path must
never execute supplied strings.

Ordering rationale: WP-1/WP-2 are independent quick wins. WP-3 is the pivot — it
unblocks WP-4, which unblocks both WP-5 (cross-process) and WP-6 (reload). A is
not scheduled (rejected).

## Test and measurement strategy — proving incrementality saves work

Correctness gates (must pass before any package lands):

- **Clean-vs-incremental byte-identity.** Every package that touches lowering or
  emission re-runs `clean_vs_incremental_parity.rs`: warm output across an edit
  sequence must be byte-identical to a cold build over the full golden corpus.
  This is the SEAL guard; it fails loud on any fresh-name / interning drift.
- **Stale-cache regression tests (the soundness bar).** For each firewall, a
  test that a *semantic* dep change (exported scheme moved, alias body changed,
  FFI interface changed) DOES invalidate importers — under-invalidation is a
  correctness failure, so these are the load-bearing tests, not the memo-hit
  ones. WP-5 gets an explicit "dep-interface change must miss the importer's
  disk entry" case.

Incrementality gates (prove work was actually skipped):

- **Memo-hit assertions via the event callback.** `IpeDatabase::with_event_callback`
  already lets a test observe `WillExecute`. Each package asserts the *absence*
  of `WillExecute` for the queries an edit should not disturb (unrelated module's
  `lower_module`, `emit_rust_file`), the pattern `phase2_incrementality.rs` uses.
  This is the direct, deterministic proof — preferred over wall-clock timing.
- **Wall-clock corpus benchmark (informational, not a gate).** A settled-edit
  latency measurement on a large synthetic project (N modules, edit one), before
  vs after WP-4, to size the real payoff. Reported, not asserted, to avoid a
  flaky timing gate.

## Soundness risks (stale-cache correctness)

- **Fresh-name drift (WP-3/WP-4).** The single highest risk: a per-module lower
  scheme that numbers fresh symbols differently from the cold whole-program pass
  produces byte-different output — a correctness (SEAL) violation. Mitigation:
  WP-3 lands and passes the parity suite *before* any per-module lower query is
  wired; the scheme is keyed only on stable per-module facts.
- **Under-invalidation at a new firewall (WP-4/WP-5).** A `lower_module` or
  per-file cache key that omits a real dependency (a dep interface, a config
  field) serves a stale file that type-checks. Mitigation: the per-file key is a
  *superset* of the in-memory firewall's dependency set by construction, and each
  firewall gets a semantic-change-must-invalidate test.
- **Cross-process symbol identity (WP-5).** Any cached artifact that embeds an
  `ipe_intern::Symbol` is unsound across processes (the documented IR-cache
  blocker). Mitigation: WP-5 caches only pure-`String` emitted files, never IR —
  the same discipline the existing whole-project cache already follows. This is
  why Approach A (persist the graph) is rejected rather than deferred.
- **FFI input trust (WP-2).** An FFI package interface is attacker-influenced
  data (an installed crate). It must arrive through the same parse-don't-validate
  input boundary as `SourceFile` — a typed, driver-vouched input, never raw text
  a query re-parses — so a malformed interface fails closed at the boundary, not
  deep in a memoized query.
- **Reload safety (WP-6).** A hotpatch reload must never open a dynamic-code
  path; the ADR 0032 no-`eval`/strict-CSP invariant is a hard gate, and any
  signature change falls back to a full restart.

## References

- ADR 0032 — salsa incremental compilation, the graph + the under-invalidation
  bar.
- ADR 0034 — the language server as a second consumer, the no-second-analyzer
  invariant.
- ADR 0035 — per-module typecheck behind closed typed interfaces, the
  reverse-edge information-flow constraint.
- `src/compiler/db/src/lib.rs` — the query definitions this doc extends.
- `src/ipe-cli/src/cache.rs` — the content-hash cache and the documented
  IR-persistence blocker.

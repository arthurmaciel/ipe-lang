# Incremental compilation — state map and remaining phases

The authoritative map of what the salsa graph covers today, the design for the
phases that remain, and a test-first implementation plan. Companion documents:

- [`tbd/incremental-phases-design.md`](tbd/incremental-phases-design.md) — the
  original forward-looking spec for these phases (approach analysis in depth).
- [`tbd/incremental-horizons.md`](tbd/incremental-horizons.md) — the breadth
  catalog of longer-range ideas this plan can pull from.
- [`tbd/per-module-fresh-name-allocation.md`](tbd/per-module-fresh-name-allocation.md)
  — the fresh-name scheme that unblocks per-module lowering (the load-bearing
  prerequisite, specified and partially wired).
- ADR 0032 (the salsa foundation + the under-invalidation bar), ADR 0034 (the
  language server as a second consumer, no-second-analyzer invariant), ADR 0035
  (per-module typecheck behind closed typed interfaces).
- [`compilation-performance.md`](compilation-performance.md) — the `ipe run`
  latency budget and the S-strategies (S6, the IR interpreter, interacts with
  the watch loop below).

Governing bar, inherited from ADR 0032 and the principle order (Security >
Correctness > Soundness > Efficiency): **under-invalidation — a stale build
that looks correct — is a correctness violation and outranks every efficiency
gain.** Incrementality trades toward Efficiency but may never weaken
Correctness; every phase below carries a differential incremental==clean gate.

## 1. Scope map — what is incremental today

The graph reaches much further than its founding ADR's title suggests: the
**entire pipeline is memoized at module granularity** in
`src/compiler/db/src/lib.rs`. There is no un-tracked stage left between parse
and emit on the one-shot path (`compile_prepared`, `src/ipe-cli/src/lib.rs`,
demands only tracked queries).

### Inputs (the only write points)

| Input | Granularity | Where |
| --- | --- | --- |
| `SourceFile` (module path + text + trust origin) | per module | `lib.rs:157` |
| `SourceRoot` (the in-scope file set) | per program | `lib.rs:175` |
| `BuildConfig` (target, SQL driver, FFI bundle, wasm, production) | per field | `lib.rs:1105` |

`sync_source_root` (`lib.rs:1443`) is the driver boundary: it reconciles the
inputs with byte-equal no-op suppression (`set_text_if_changed`, `lib.rs:187`),
so a save that changes nothing invalidates nothing.

### Tracked queries (all `#[salsa::tracked]`)

| Tier | Queries | Granularity |
| --- | --- | --- |
| Front end | `parse` (`lib.rs:208`), `imports` (`:225`), `resolve_imports` (`:268`), `canonicalize` (`:323`), `module_interface` (`:395`), `identifier_words` (`:1412`) | per module |
| Link | `topo_order` (`:555`), `linked_program` (`:604`), `kernel_types` (`:653`) | whole program |
| Typecheck | `typecheck` (`:694`) whole-program; `infer_module_scoped` (`:883`), `typed_interface` (`:952`), `typecheck_module` (`:985`) | per module (ADR 0035) |
| Lower | `lower_program` (`:1034`) | **whole program** |
| Emit | `emit_project` (`:1163`), `program_rust_file_ids` (`:1251`), `emit_spine_file` (`:1276`), `emit_rust_file` (`:1314`), `emit_manifest` (`:1357`) | per Rust file |

### Recompute boundaries

- **File → module**: `parse`/`canonicalize` re-run only for the edited
  `SourceFile`. `module_interface` is the canon-tier firewall — it is
  **span-free by construction** (`lib.rs:388-393`: every field keyed by
  `Symbol`, exported alias bodies are span-free `TypeAnnotation`s), so a
  formatting/comment edit backdates and importers never re-canonicalize. A
  previously recorded "span-erase the interface" follow-up is therefore a
  no-op: there is no span-shift over-invalidation to erase.
- **Module → program**: `typed_interface` is the typed-tier firewall.
  `typecheck_module` serves the scoped solve when provably faithful, else falls
  back fail-closed to the whole-program `typecheck` projection (the Boundary
  Scheme Promotion constraint — residual variables are shared program-wide, so
  the module is the finest unit at which a *closed* interface exists).
- **The remaining coarse floor**: `linked_program`, `typecheck`,
  `lower_program` are one-per-program. They memoize (a no-op re-save executes
  nothing) but any reachable semantic edit re-runs them **in full**. Emit is
  already per-file below that floor: an unrelated `emit_rust_file` re-executes
  against a byte-identical IR slice, produces an identical `String`, backdates,
  and the on-disk write skips — but it still had to wait for the whole-program
  lower.
- **Deliberately un-memoized**: the wasm security gates
  `check_client_reachability` / `check_wasm_client` re-run every invocation.
  Wrapping a security gate in a memoization seam adds a stale-cache hazard on
  the highest-priority axis for near-zero gain; they stay direct calls.

### Consumers

- **`ipe watch`** (`src/ipe-cli/src/watch.rs`): ONE warm `IpeDatabase` per
  session; the coalesce thread settles filesystem batches, `sync_source_root`
  runs between batches (never while a cloned worker handle is alive — salsa's
  cancellation pattern), each compile worker gets a cloned handle and demands
  `compile_prepared`; at most one `cargo build` child, killed when superseded.
- **The language server** (ADR 0034): one warm DB, mutates inputs on document
  change, reads `typecheck_module` — never a second analyzer.
- **`ipe build`** (cold, `src/ipe-cli/src/cache.rs`): every invocation starts a
  fresh DB; `compute_project_key` (`cache.rs:228`, length-prefixed content hash
  of entry, target, driver, every module's path + origin + text) fronts an
  on-disk `EmittedProject` cache under a **version epoch** directory
  (`derive_epoch`, `cache.rs:375`: hash of the running `ipe` binary's bytes +
  `rustc -vV`) — stale entries are unreachable by construction, never
  detected. The cache stores only pure `String`s; IR is not persistable because
  embedded `ipe_intern::Symbol`s are process-local indices (`cache.rs:20-27`).

## 2. Remaining phases — design

### The gaps, ranked by latency payoff

1. **The lowering floor** (largest): every semantic edit pays a whole-program
   `typecheck` + `lower_program` before any emit query can early-cut.
2. **Cross-process cold start**: the disk cache key is whole-project, so a
   one-module edit misses the entire entry and rebuilds the world cold.
3. **FFI as a hidden coarse input**: installed-crate interfaces ride on
   `BuildConfig.ffi` as one opaque blob; a one-crate change invalidates
   emission wholesale.
4. **Reload payoff**: warmth exists but every settled batch still ends in a
   full `cargo build` + process restart.

### Invalidation granularity: the module, firewalled by interfaces

The unit of invalidation stays the **module** (`SourceFile`), never the
declaration. Per-decl granularity is out of scope: Boundary Scheme Promotion
shares residual variables program-wide, so a decl-level solve cannot reproduce
reverse-edge information flow (and mutually recursive decls would demand
SCC-computed units — recorded in the horizons catalog, not scheduled).
Backdating at the interface firewalls (`module_interface`, `typed_interface`,
and the per-module IR value below) is what turns "re-execute" into
"re-validate then early-cut" for every module an edit did not semantically
touch.

### Per-module lowering (the pivot)

Two-step, specified in `tbd/per-module-fresh-name-allocation.md`:

**Step 1 — deterministic per-module fresh names.** `ipe_lower::lower`
(`src/compiler/lower/src/lib.rs`) mints fresh-symbol pools of two kinds. The
position-indexed pools (`eta_`/`cap_`) are already per-module-deterministic
(the string at position *i* depends on `(prefix, i, interned identifiers)`,
never pool size) and their per-module sizing is wired. The monotonic-cursor
pools (`arg_`, `anyp_`, `destr_thunk_`, `ncons_`, `nstrlit_`) number sites by
a whole-program cursor; the scheme replaces each name with
`prefix ++ (module_base_offset(home) + local_index(site))`, where the offset
table (prefix sums of per-module site counts, a small
`BTreeMap<home, [usize; 5]>`) is the only whole-program input — a pure
function of the module set's shape, never of any module's body text. On the
whole-program path this reproduces the current cursor exactly (byte-identity
by prefix-sum construction), which is why it ships only together with the
query that consumes it.

**Step 2 — `lower_module` + re-parented emit.** New tracked query
`lower_module(root, entry, module)` keyed per `SourceFile`, depending on that
module's `typecheck_module`/`canonicalize` plus the offset table (itself a
tracked query). `lower_program` becomes a thin assembler over `lower_module`
outputs (value-equal assembly still backdates emit). `emit_rust_file`
re-parents onto `lower_module(that file's home)` instead of the coarse
`lower_program` — an unrelated edit no longer re-executes it at all.

Invalidation behaviour: a body-only edit that preserves fresh-name site counts
leaves every other module's offset — and therefore its `lower_module` memo —
standing. An edit that changes a module's site count shifts the offsets of all
*later* modules, correctly invalidating exactly their memos. No stale hole.

### Per-file cross-process disk cache

Extend `cache.rs` with a second tier **beneath** the whole-project entry
(which stays as the "nothing changed" fast path): a per-Rust-file entry keyed
on `hash(module text ⊕ resolved transitive dep-interface hashes ⊕ emit-relevant
config)`, under the same version epoch. Emitted files are pure `String`s — no
`Symbol`, no interner dependency — so they relocate across processes with the
same trick the whole-project entry already relies on. The key is by
construction a **superset** of the in-memory firewall's dependency set: it
changes exactly when the salsa memo would have re-executed, keeping the two
tiers coherent. A cold `ipe build` after a one-module edit reads N−1 files
from cache and re-emits one.

Persisting the salsa graph itself stays **rejected** (not deferred): memoized
values embed process-local `Symbol`s, and a sound relocation pass plus a
cache-coherence proof is the highest-risk option against the correctness bar,
for the narrowest consumer.

### FFI package interfaces as a salsa input

`ffi_package_interface(PackageId)`: a per-installed-crate salsa input that
`kernel_types` and FFI-importing canon union in, replacing the opaque
`BuildConfig.ffi` blob for invalidation purposes. A one-crate change then
invalidates only its importers. The interface data crosses a trust boundary
(an installed crate is attacker-influenced), so it must arrive through a
typed, parse-don't-validate driver — never raw text a query re-parses — and
the change requires a security-soundness review before merge. Payoff is
marginal until multi-crate FFI is common; scheduled accordingly.

## 3. `ipe watch` payoff and target latency

Today a settled batch on a warm DB costs: re-parse + re-canon of the edited
module (cheap) → **whole-program typecheck + lower** (the dominant compiler
cost, proportional to program size) → per-file emit early-cuts → `cargo build`
of the touched compilation unit(s) → process restart.

| After | Compiler-side settled-edit cost | End-to-end |
| --- | --- | --- |
| today | O(program) — the lower floor | cargo (1 unit) + link + restart |
| per-module lower | **O(edited module)** — everything else backdates | unchanged |
| per-file disk cache | same, and cold `ipe build` gets the same profile | unchanged |
| diff-driven reload | same | restart only when an interface changed |

Target: compiler-side recompute for a body-only edit in **single-digit
milliseconds independent of project size** (memo-hit assertions make this
structural, not aspirational — the unrelated work provably does not run).
End-to-end latency is then dominated by `cargo build` + link (~1 s with mold
per `compilation-performance.md`), which is exactly the boundary the S-track
attacks (S1/S2/S3 shrink the crate graph; S8 the linker).

**Interaction with the S6 IR interpreter.** S6 executes `ipe_ir` directly for
`ipe run` (no `Rust.` FFI, dev only, differential-oracle-gated). Per-module
lowering makes warm per-module IR a first-class memoized value — precisely
what an interpreter-backed watch loop would consume: a settled body edit could
re-run under the interpreter straight off the warm DB, **skipping cargo
entirely** in dev, with the AOT path untouched. This document does not design
that loop; it guarantees the IR-freshness substrate S6's watch integration
needs, and notes that the same fail-closed rule applies (FFI programs fall
back to the AOT watch path).

**Hot reload** stays defined as **state-preserving restart** (the TEA answer:
compile a fully verified binary, migrate the serialized model, kill the old
process). Native code patching is rejected on principle (dynamic code loading
+ `unsafe` boundaries — the exact hole the no-`eval`/strict-CSP invariant
keeps shut). The near-term reload win is narrower: when a settled batch
changes no module's typed interface, the supervisor knows the restart is
signature-stable before cargo runs; any interface change falls back to full
restart, fail-closed.

## 4. Correctness — the incremental==clean guard

Incrementality must be observationally invisible: for any edit sequence, the
warm output must equal a cold build of the final state. The guard already
exists and every phase extends it:

- **`clean_vs_incremental_parity.rs`** (`src/ipe-cli/tests/`): one warm DB
  reused across an adversarial edit sequence (body-only edit, export widening,
  export type flip, red edit, probe-edit → revert that skews warm symbol
  numbering) vs a cold DB built from the final state — emitted project must be
  **byte-identical**. Every phase that touches lowering or emission re-runs
  this over the full golden corpus before landing. This is the SEAL guard; it
  fails loud on any fresh-name or interning drift.
- **Stale-cache regression tests (the load-bearing direction).** For each
  firewall, a test that a *semantic* dep change (exported scheme moved, alias
  body changed, FFI interface changed, fresh-name site count changed) DOES
  invalidate importers. Under-invalidation is the correctness failure mode, so
  these outrank the memo-hit tests.
- **Memo-hit assertions** via `IpeDatabase::with_event_callback`
  (`lib.rs:131`): each phase asserts the *absence* of `WillExecute` for
  queries an edit must not disturb (the `phase2_incrementality.rs` pattern) —
  deterministic proof that work was skipped, preferred over wall-clock gates.
- **Structural guards that stay**: security gates un-memoized; disk cache
  serves only pure `String`s under the version epoch (stale = unreachable);
  `BuildConfig` field granularity keeps emit-only toggles out of the lower
  tier.

## 5. Scope and non-goals

**Ships first** (biggest latency win, bounded risk): per-module fresh names +
`lower_module` + re-parented emit. **Then**: the per-file disk cache (worthless
before per-module lower exists). **Then, opportunistically**: FFI input (when
multi-crate FFI is common), signature-stability restart gating, and the
horizon items already tagged near-term (Merkle project keys, SEAL-verdict
memoization, `--explain` rebuild provenance).

**Non-goals** (decisions, not omissions): salsa graph persistence (rejected —
symbol relocation risk); per-declaration typecheck/lower (BSP reverse-edge
constraint); constraint-solution reuse inside the solver (soundness stakes too
high for marginal win once solves are module-sized); native code hotpatching
(rejected on principle); interpreter watch integration (S6's design, not
this one — this plan only guarantees its IR substrate).

## 6. Implementation plan — test-first, independently landable

Each phase: failing test first → minimal change → gate. The standing gate for
every phase: `clean_vs_incremental_parity.rs` green over the full golden
corpus, every `emits_byte_identical_main_rs`/seal golden unchanged,
`cargo build --workspace` + `clippy -D warnings` + `fmt --check` clean.

**Phase A — offset-table query (fresh-name substrate).**
- *Failing test*: a `ipe_db` unit test asserting a tracked
  `fresh_name_offsets(root, entry)` query returns, for a 3-module fixture, the
  prefix-sum table matching hand-counted per-module site counts; and a
  body-only edit that preserves site counts does NOT re-execute it for later
  modules' consumption (event-callback assertion).
- *Change*: implement per-module site counting in `ipe_lower` (reusing
  `count_destructure_param_sites`'s walk) + the tracked query. No consumer yet
  changes behaviour.
- *Gate*: standing gate (numeric no-op on the whole-program path — byte
  identity is the proof the counting matches the cursor).

**Phase B — two-level cursor inside whole-program lower.**
- *Failing test*: a lower unit test pinning that every monotonic-cursor name
  equals `prefix ++ (base + local)` for a fixture with ≥2 modules and sites in
  each — written against the current cursor output, so it fails until the
  two-level scheme reproduces it.
- *Change*: thread `module_base_offset + local_index` through the five
  monotonic-cursor pools in `Lowerer::run`, still on the whole-program path.
- *Gate*: standing gate — this is the phase byte-identity exists to police.

**Phase C — `lower_module` + assembler `lower_program`.**
- *Failing test*: in `ipe_db`, a body-only edit to module X must NOT
  re-execute `lower_module` for module Y (event-callback `WillExecute`
  absence); an edit changing X's site count MUST re-execute `lower_module` for
  every module after X (the stale-cache direction).
- *Change*: add the tracked `lower_module(root, entry, module)`; rebuild
  `lower_program` as a value-equal assembler over it.
- *Gate*: standing gate + both new tests.

**Phase D — re-parent `emit_rust_file`.**
- *Failing test*: a body edit to an unrelated module must produce NO
  `WillExecute` for the other module's `emit_rust_file` (today it re-executes
  and backdates; after re-parenting it never runs).
- *Change*: `emit_rust_file` depends on `lower_module(that home)` (plus the
  spine inputs it genuinely needs), not on coarse `lower_program`.
- *Gate*: standing gate + the no-execute assertion + watch integration tests
  (`watch_integration.rs`, `watch_cancellation.rs`) green.

**Phase E — per-file disk cache tier.**
- *Failing tests*: (1) cold `ipe build` after a one-module edit re-emits
  exactly one file and serves N−1 from cache (observable via cache-dir
  inspection); (2) **a dep-interface change must MISS the importer's entry** —
  the primary stale-cache regression; (3) an emit-config change must miss
  every entry.
- *Change*: per-file entries in `cache.rs` keyed
  `hash(module text ⊕ dep-interface hashes ⊕ emit config)` under the existing
  epoch; whole-project tier unchanged above it.
- *Gate*: standing gate + all three tests; key derivation centralized in one
  function, property-tested "distinct inputs ⇒ distinct keys".

**Phase F — signature-stability restart gate in `ipe watch`.**
- *Failing test*: a watch orchestrator test where an interface-changing edit
  MUST take the full-restart path and a body-only edit MAY take the fast path;
  plus the invariant test that no reload path ever executes supplied strings
  (the no-`eval` posture).
- *Change*: the supervisor consults typed-interface change status from the
  warm DB before choosing restart strategy; fail-closed to full restart.
- *Gate*: standing gate + `watch_sigterm.rs`/cancellation suites green;
  security review of the reload decision boundary.

Phases A–B are pure substrate (byte-identical, land independently); C is the
pivot; D delivers the watch latency win; E extends it cross-process; F is the
product-facing payoff. The FFI input phase mirrors E's shape (typed input +
one-crate-invalidates-importers-only test + security review) and slots in
whenever multi-crate FFI demand arrives.

## 7. Risks and cost

| Risk | Where | Mitigation |
| --- | --- | --- |
| **Fresh-name drift** — per-module numbering diverges from the cold cursor; byte-different output is a SEAL violation | Phases B–C | scheme keyed only on stable per-module facts; two-level cursor lands *inside* the whole-program path first (Phase B) where byte-identity polices it before any per-module query exists |
| **Under-invalidation at a new firewall** — a `lower_module` dependency or per-file cache key omitting a real input serves a stale artifact that type-checks | Phases C–E | per-file key is a superset of the in-memory dependency set by construction; every firewall gets a semantic-change-must-invalidate test; key derivation centralized + property-tested |
| **Cross-process symbol identity** — any persisted artifact embedding `ipe_intern::Symbol` is unsound in a fresh process | Phase E | only pure-`String` files are ever persisted; graph persistence rejected outright |
| **Salsa API churn** — tracked-struct/accumulator surface moves between releases | all | the graph confines salsa to `ipe_db`; consumers see plain functions; version pinned at the workspace, upgrades are one-crate events |
| **Memory footprint of retained memos** — per-module IR values retained per revision in a long-lived watch/LSP session | Phases C–D | per-module values replace (not add to) the whole-program value's footprint for edits; salsa's LRU/evict knobs available if session growth observed — measure before tuning |
| **Emit-cache coherence** — the disk tier disagreeing with the in-memory graph | Phase E | shared key inputs (interface hashes) + the version epoch make disagreement unreachable, not detected; the epoch already folds in the `ipe` binary and `rustc` |
| **Reload safety** — a fast-restart path becoming a dynamic-code hole | Phase F | reload = verified-binary restart only; interface change ⇒ full restart, fail-closed; no-`eval`/strict-CSP invariant is a hard gate |
| **Cost** | — | Phases A–B are days-scale, mechanical-with-a-proof; C–D the structural core (the offset table + query re-parenting) with the parity suite as a safety net; E bounded (extends an existing, disciplined cache); F small but review-gated |

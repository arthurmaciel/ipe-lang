# Implementation Plan — Incremental compilation via salsa (Tier-3 #3)

> **Source spec (authoritative, do not redesign):**
> `docs/architecture/incremental-compilation-and-watch.md`. This plan turns the
> locked Q1–Q4 decisions + hazard ledger (H1–H24) into a bite-sized, ordered,
> test-first task list. Every decision here is downstream of that spec; where the
> spec locked a choice (Option B persistence, Position A interpreter, DEFER
> call-graph firewall) this plan implements the locked choice and does not reopen
> it.

## Goal

Make the pure compiler DAG
(`sky_parse → sky_canon → sky_types → sky_lower → sky_ir → sky_backend_rust`) an
incremental salsa query graph cut durably at `sky_ir`, so that (a) `ipe watch`
recomputes only the dirty subgraph on a file save, (b) the same engine backs the
LSP (Tier-3 #4), and (c) **every incremental result is byte-identical to a clean
from-scratch build** — the primary soundness guard against stale-cache
under-invalidation.

## Global Constraints

1. **PRINCIPLES order (hard, from the spec):**
   security > correctness > soundness > efficiency > completeness > readability.
   Under-invalidation (a stale build that looks correct) is a **correctness**
   violation and outranks every efficiency gain. When a granularity optimisation
   and soundness conflict, ship the coarse-but-sound query and file the
   optimisation as a separately-audited follow-up (this is exactly the spec's
   LOCKED "DEFER call-graph-shape firewall" posture).

2. **Two fundamental rules, applied to the incremental engine itself:**
   - **PARSE, DON'T VALIDATE** — external data (source bytes, `sky.toml`,
     build-affecting env, FFI interface, toolchain identity) is parsed **once**
     at the driver boundary into typed salsa **inputs**. A salsa query that reads
     `std::fs`/`std::env`/clock on its query path is the bug (INV-1). Values that
     encode resolution/restore results are closed enums
     (`Resolved | Unresolved | Ambiguous`; `RestoredModel` only constructible from
     a schema-tag-matched + decoded blob) so "resolved to nothing" ≠ "resolved to
     two things" and a stale/mismatched artifact is **unrepresentable**, not
     caught-if-it-throws.
   - **MAKE INVALID STATES UNREPRESENTABLE** — cache addresses are the full
     provenance tuple (H1); `emit_manifest` is authoritative and orphans are
     deleted (H7); `LastGoodBinary`/`VerifiedFfiInterface`/`WatchedPath` are
     newtypes only constructible through a verifying constructor.

3. **Additional non-negotiables carried from CLAUDE.md / repo:** no eval hole
   (INV-2: `data-sky-eval` stays forbidden, CSP no-`unsafe-eval` preserved at
   every reload level); last-good binary stays alive on any red build (INV-3);
   every rebuild+cargo cycle is timeout-bounded (CLAUDE.md §3); the workspace
   `deny(unwrap_used, panic, indexing_slicing, unreachable)` lints stay green on
   all new crates; emit only through the typed escaped-literal IR emitter, never
   string-concat source-derived data into `.rs` (H10).

## Repo coupling notes (read before starting)

- **Mid-rename `sky_*` → `ipe_*` (#59).** This plan uses the **current** crate
  names (`sky_parse`, `sky_canon`, `sky_types`, `sky_lower`, `sky_ir`,
  `sky_backend_rust`, `ipe`, `sky_intern`). If the rename lands first, s/sky_/ipe_/
  mechanically; nothing here depends on the prefix. The new crate introduced
  below is named `sky_db` under current convention (→ `ipe_db`).
- **Whole-program link is the structural crux.** `ipe::build_project`
  (crates/ipe/src/lib.rs:246) parses+canonicalises each module dep-first, then
  **`sky_canon::link::link(...)` merges them into ONE module**, and runs
  `sky_types::infer` / `sky_lower::lower` / `RustBackend::emit` on the *linked
  single module*. Today typecheck/lower/emit are **whole-program, not per-module**.
  The spec's per-module `typecheck(ModuleId)` / `lower(ModuleId)` queries require
  decoupling inference and lowering from the linked whole. This is the riskiest
  refactor (see Task 12/13); it is staged behind the parity gate so it can never
  ship a divergence.
- **Interner is `&mut`-threaded and not `Send`/`Sync`.** Every pass takes
  `&mut sky_intern::Interner` (`intern(&mut self, ...) -> DResult<Symbol>`,
  crates/sky_intern/src/lib.rs:50). Salsa queries are pure over an immutable `&db`.
  Reconciling this is the interning story (Task 3) and is a prerequisite for every
  tracked query.
- **cargo is not invoked today.** `ipe::build`/`build_project` stop at writing
  emitted files; only an `IPE_E2E`-gated test runs cargo (lib.rs:1116). The
  emit→cargo bridge + integrated `ipe build` cargo step are net-new (Task 15/16).

---

## Phase A — Salsa foundation + the interning story

### Task 1 — Add salsa; stand up the `sky_db` crate + empty database

**Goal.** A compiling `sky_db` crate exposing a `#[salsa::db]` database with zero
queries yet, wired into the workspace.

**Test first.** `crates/sky_db/tests/db_smoke.rs`: construct the database, assert
it implements `salsa::Database`, drop it. Fails to compile until the crate exists.

**Do.**
- Add `salsa` to `[workspace.dependencies]`. **Pin the exact modern 0.x version**
  (the `#[salsa::db]` supertrait + `salsa::Storage` + tracked-function generation
  — the post-"jar" model; verify the precise input/tracked/interned macro surface
  against the pinned version's docs at implementation time, salsa's API churns).
- New crate `crates/sky_db` with `SkyDatabase { storage: salsa::Storage<Self> }`,
  `#[salsa::db] impl salsa::Database for SkyDatabase`.
- Add to workspace members; apply the workspace clippy lint table.

**Verify.** `cargo test -p sky_db db_smoke`; `cargo clippy -p sky_db` clean.
**Done when.** Empty db constructs and the smoke test is green.

### Task 2 — Define the input set (typed, parse-don't-validate boundary)

**Goal.** Every external datum the spec's Q1b input table lists exists as a
`#[salsa::input]`, with the durability rule encoded. No query reads the world.

**Test first.** `inputs_roundtrip.rs`: `set_source_text` then read back equal;
mutating one input and reading another returns the *same* `salsa::Id` /
unchanged value (proves inputs are independent).

**Do.** Model the spec's input table exactly:
- `source_text(FileId) -> Arc<str>` (low durability), `file_set() -> Arc<BTreeSet<FileId>>` (low).
- `project_config() -> ProjectConfig` (high) — a **typed** parse of `sky.toml`
  (reuse `ipe::project::parse_manifest`), NOT a raw string.
- `codegen_flags() -> CodegenFlags` (high) — `IPE_DCE`, `IPE_SOLVER_BUDGET`,
  budget factor, env-prefix parsed to typed fields (closes the hidden-env hole, H2).
- `ffi_package_interface(PackageId) -> Arc<VerifiedFfiInterface>` (high) — **reserve
  the seam only** (FFI is PARKED per spec); a stub `VerifiedFfiInterface` whose
  only constructor takes a provenance hash. Do not build introspection.
- `compiler_revision() -> ContentHash` (high, read once at process start).
- `toolchain_fingerprint() -> ToolchainId` (high; refuse-don't-guess handling is
  Task 20, not here).
- `FileId` / `ModuleId` / `PackageId` / `RustFileId` are `#[salsa::interned]` keys.

**Soundness gate for this task.** Grep the DAG crates on the query path for
`std::fs`, `std::env`, `SystemTime`/`Instant` — there must be **zero** on any
`#[salsa::tracked]` path (INV-1, H2). Add a CI grep test asserting this.

**Verify.** `cargo test -p sky_db inputs_roundtrip`; the INV-1 grep test green.
**Done when.** All inputs settable/readable; the no-hidden-input grep passes.

### Task 3 — The interning story: salsa ⇄ `sky_intern` coexistence (PREREQUISITE)

**Goal.** Passes stop threading `&mut Interner`; a single interner is owned by the
database and shared immutably by every query. This unblocks all tracked queries.

> This is the load-bearing enabler. Every later query depends on it. Do NOT skip
> to Task 4 without this green.

**Test first.** `intern_shared.rs`: two tracked queries (a trivial `parse` stub +
a trivial `imports` stub) run against the same db and resolve the **same** symbol
to the same string; a query never needs `&mut`.

**Do — pick and implement ONE, spec-neutral (the spec leaves mechanism open):**
- **Option 3a (recommended): DB-owned append-only interner behind interior
  mutability.** Keep `sky_intern::Interner`'s string↔Symbol identity, but store it
  in the db as `Arc<Mutex<Interner>>` (or `boxcar`/append-only + `RwLock`). Expose
  `db.intern(&str) -> Symbol` / `db.resolve(Symbol) -> Arc<str>`. Interning is
  *append-only and deterministic given input order*, so it does not break salsa's
  purity **provided symbol assignment is a pure function of the demanded query
  set** — verify order-independence (Task 3 test + parity gate). Symbols are dense
  `u32`; `resolve` returns owned `Arc<str>` so no borrow of the lock escapes.
- **Option 3b: migrate to `#[salsa::interned]` for identifiers.** Replace `Symbol`
  with a salsa interned key. Larger blast radius (touches every crate that names
  `Symbol`), but is the "most salsa-native" answer and removes the ad-hoc interner
  entirely. File as the post-v1 cleanup if 3a ships first.

**Soundness note.** The risk in 3a is a hidden dependency on *insertion order*: if
symbol values depend on which query ran first, two demand orders could yield
different `Symbol` numbers and thus different emitted bytes → a false parity-gate
failure that is actually a real nondeterminism bug. The parity gate (Task 18) is
the guard; additionally add a test that runs the same project under two different
query-demand orders and asserts byte-identical emit.

**Verify.** `cargo test -p sky_db intern_shared`; demand-order determinism test green.
**Done when.** No pass signature requires `&mut Interner` on the salsa path.

---

## Phase B — Per-pass tracked queries (granularity is the key design decision)

> **Granularity discipline (applies to every task in this phase).** Each pass
> becomes a `#[salsa::tracked]` query keyed at the granularity in the spec's
> derived-query table. Each task ships **its own "no-op edit to module B does not
> recompute module A's query" test** using salsa's event log: install a
> `salsa_event` callback that records `WillExecute { database_key }`; edit B
> (byte-changing), demand A's query, assert A's key is **absent** from the executed
> set. This event-count test is the per-task granularity proof and is
> non-negotiable — a task without it is not done.

### Task 4 — `parse(FileId)` tracked query

**Goal.** `parse` keyed on `source_text(self)` only. Edit in A never re-parses B.
**Test first.** Two files A, B; warm the db; `set_source_text(B, ...)`; demand
`parse(A)`; assert `parse(A)` not in the executed-key set. Also: byte-equal
re-save of A is a no-op at the input boundary (no `parse(A)` execution).
**Do.** Wrap `sky_parse::parse_module(&db.source_text(f), db.interner())`. Return
`Arc<Module>` (or a salsa-tracked struct). Diagnostics returned as data, not `?`
out of the query (a query must be total — see error-accumulation note below).
**Error handling.** Parse failures are represented as a typed
`Result<Arc<Module>, Diagnostic>` **value** of the query (or salsa accumulator),
never a panic. Downstream queries pattern-match; a red parse yields a red
downstream, no stale reuse.
**Verify.** `cargo test -p sky_db parse_granularity`.

### Task 5 — `imports(FileId)` + `program_modules()` + `file_set` wiring

**Goal.** `imports(self)` derives from `parse(self)`; `program_modules()` derives
from `file_set()`. Makes the "ALL modules" quantifier an explicit `file_set()`
dependency (H6 — an added module can't be silently excluded).
**Test first.** Adding a file bumps `file_set()` and re-executes `program_modules()`
but NOT `parse` of unrelated unchanged files.
**Do.** `imports` reuses `project::extract_imports_from_source` logic but over the
parsed AST (typed, not string-scan) so it is a pure function of `parse(self)`.

### Task 6 — `resolve_imports(ModuleId)` — the module-resolution edge (under-invalidation gate)

**Goal.** Map each import *name* → `Resolved(ModuleId) | Unresolved | Ambiguous`
as a pure function of `imports(self)` + `file_set()`. This is the query that makes
add/delete/rename/shadow of a module re-canonicalise its importers.
**Test first (four cases, each a distinct test):**
- **Add** a file satisfying a previously-`Unresolved` import → importer's
  `resolve_imports` flips to `Resolved` and re-executes.
- **Delete** an imported module → flips to `Unresolved`; importer re-typechecks red.
- **Rename** / **shadow** → resolution retargets; every importer re-canons.
- **Ambiguous** (two files claim one module id) → `Ambiguous` value; `canonicalize`
  turns it into a hard error, never a silent pick.
**Do.** Closed enum result (MAKE-INVALID-STATES-UNREPRESENTABLE). Because it reads
`file_set()`, adding/removing any file re-validates every `resolve_imports`.
**Why it matters.** `parse`/`module_interface` key only on file *contents*, never
the *set* of files; without this edge the set-vs-contents bug class (GAP-1) is
open. This is the highest-value soundness task in Phase B.

### Task 7 — `module_interface(ModuleId)` — the PRIMARY firewall (completeness release gate)

**Goal.** A summary of module A derived from `parse(A)` that changes **only** when a
cross-module-observable of A changes, so importers early-cut on A-body-only edits.
**Test first.**
- Body-only edit to A (change a private fn body, no signature change) → importers'
  `canonicalize`/`typecheck` do **not** re-execute (firewall holds).
- **The completeness counter-example (release gate):** flip A's export from
  `Html msg` to `String`. Importer names resolve identically, but the interface
  hash MUST change and the importer MUST re-typecheck+re-emit. A test asserts this.
**Do.** Interface = **sound over-approximation of every type-directed-lowering
observable**: exported types, constructor arities, **full resolved value
signatures**, fixity, re-exports, parametric-record-alias shapes (H3). Rule:
**when in doubt, include it in the hash.** This obligation is foregrounded because
ipê does type-directed lowering — a name-level interface silently under-invalidates.
**Done when.** Both firewall (early-cut) and completeness (flip-type) tests green.

### Task 8 — `canonicalize(ModuleId)` per-module tracked query

**Goal.** `canonicalize(self)` from `parse(self)` + `resolve_imports(self)` +
`module_interface(deps)` where `deps` = the resolved set from `resolve_imports`.
**Test first.** Importer does NOT re-canon on a dep *body* change (via firewall)
but DOES re-canon when `resolve_imports` changes (add/delete/rename/shadow).
**Do.** Adapt `sky_canon::canonicalise_module` to consume `module_interface(dep)`
summaries in place of the `dep_exports: BTreeMap` it threads today
(lib.rs:322-324). The current dep-first `BTreeMap<path, ModuleExports>` becomes the
salsa-tracked `module_interface` set — same data, now incrementally keyed.

### Task 9 — `kernel_types()` tracked query

**Goal.** `kernel_types()` = static kernel table ∪ `ffi_package_interface(*)`.
Early-cuts on unchanged FFI packages.
**Test first.** Changing one `ffi_package_interface` input re-executes
`kernel_types()` but changing unrelated `source_text` does not.
**Do.** Union the static `sky_kernels` table with the (currently empty, PARKED)
per-package interface inputs. Minimum forward contract from the spec: adding FFI
later flips inputs on with **no query-graph redesign**.

### Task 10 — `typecheck(ModuleId)` per-module tracked query

**Goal.** HM + exhaustiveness + region types **per module**, from
`canonicalize(self)`, `resolve_imports(self)`, `module_interface(deps)`,
`kernel_types()`.
**Test first.** No-op edit to B does not re-execute `typecheck(A)` when A does not
import B; a newly-satisfied/broken import DOES re-typecheck the importer (carries
the resolution edge).
**Do — THIS IS THE STRUCTURAL CRUX (see Task 12).** Today `sky_types::infer` runs
on the **linked whole-program**, not per module. This task requires inference to
run against a single module + its dep interfaces. Stage it: land it first as a
*coarse* whole-program query (Task 11) to get the parity gate green, then refine
to true per-module granularity (Task 12) with the event-count test as the proof.

### Task 11 — Coarse whole-program salsa spine (parity-gate scaffold, sound floor)

**Goal.** Before per-module refinement, get the *entire* existing
link→infer→lower→emit pipeline running **inside salsa as coarse queries** so the
clean-vs-incremental parity gate (Task 18) can be stood up early and guard every
subsequent granularity change.
**Test first.** A whole-project golden: emit via the salsa spine == emit via the
legacy `build_project`, byte-for-byte, on all `tests/golden/*` fixtures.
**Do.** One coarse `linked_program()` query wrapping the existing
link+infer+lower; `emit_manifest()` wrapping emit. Correct-but-coarse: any edit
recomputes broadly. This is the sound v1 floor the PRINCIPLES order mandates —
ship coarse-and-correct, then optimise under the gate.
**Done when.** Salsa-spine emit is byte-identical to legacy on every golden.

### Task 12 — Refine `typecheck`/`lower` to true per-module granularity (RISKIEST)

**Goal.** Replace the coarse `linked_program()` with per-module `typecheck(ModuleId)`
+ `lower(ModuleId)` so a body edit to one module does not re-typecheck/re-lower the
world.
**Test first.** (a) The parity gate (Task 18) stays green — per-module output ==
coarse/legacy output byte-for-byte. (b) Event-count: edit module B's body; demand
`emit_manifest`; assert `typecheck(A)` and `lower(A)` did NOT execute for modules A
that don't depend on B.
**Do.** Decouple `sky_types::infer` and `sky_lower::lower` from consuming a single
linked module: infer/lower each module against its `module_interface(deps)` +
`resolve_imports`. **Risk:** inference results could differ from the
link-then-infer path if `module_interface` is not a truly sound over-approximation
— this is precisely why Task 7's completeness gate and Task 18's parity gate exist.
Do not merge this task with a red parity gate.
**Fallback.** If per-module inference proves to change results in a way the
interface can't soundly capture within budget, **keep the coarse whole-program
`typecheck` from Task 11 as the v1 floor** (sound, just less granular) and file
per-module typecheck as an audited follow-up — PRINCIPLES order permits coarse; it
forbids wrong.

### Task 13 — `lower(ModuleId)` per-module IR

**Goal.** Per-module IR mirroring the legacy `.ipe/lowered/` layout, from
`typecheck(self)`.
**Test first.** Body edit to B re-executes `lower(B)` only; `lower(A)` untouched;
parity gate green.
**Do.** As Task 12's lower half; split out only if Task 12 lands typecheck first.

---

## Phase C — Whole-program metadata + emit (the durable `sky_ir` cut + bridge)

### Task 14 — `program_metadata()` + `program_ir_module(ModuleId)` + `emit_rust_file`

**Goal.** Whole-program DCE/mono metadata over the **full lowered-IR set**, then
post-DCE/mono per-module IR that early-cuts on byte-identical metadata, then
per-file emit text.
**Test first.**
- Dead-fn-promoted-to-live: edit B so a previously-dead fn becomes reachable →
  `program_metadata()` re-executes and the promoted fn appears in emit (H6 — proves
  metadata is NOT firewalled behind interfaces).
- Body edit whose reachability is unchanged → `program_metadata()` output is
  byte-identical → `program_ir_module`/`emit_rust_file` early-cut; only the edited
  module's `.rs` changes.
**Do.**
- `program_metadata()` depends on `program_modules()` + `lower(ALL of
  program_modules())` and **re-runs every build** (spec LOCKED: DEFER the
  call-graph-shape firewall; conservative floor is sound-by-construction). The
  explicit `program_modules()` dep is load-bearing (added module can't be excluded
  from mono/DCE).
- `program_ir_module(self)` = `lower(self)` + `program_metadata()`; early-cuts on
  identical metadata.
- `emit_rust_file(RustFileId)` = `program_ir_module(owner)` + `program_metadata()`;
  a body edit changes only that file's text.
- Fields sorted by `_fieldIndex`, sorted mono table, stable ordering everywhere —
  **byte-determinism is both a soundness property and what stops cargo spurious
  rebuilds** (H8). Emit only through the typed escaped-literal emitter (H10).
**Note.** `sky_ir` is the durable cut-point; the future interpreter tier consumes
`program_ir_module` directly (Position A, spec-locked). Keep the query's output
backend-agnostic.

### Task 15 — `emit_manifest()` + the emit→cargo bridge (deterministic / content-gated / prune)

**Goal.** The on-disk emitted project is a **pure function of `emit_manifest()`**,
never an accretion.
**Test first.**
- **Prune test (H7):** delete a Ipê module; rebuild; assert its emitted `.rs` is
  **removed** from disk (orphan deletion), not lingering.
- **Content-gated write (H8):** a comment-only edit that yields byte-identical emit
  writes **zero** files (mtime not bumped → no cargo rebuild).
- **Atomic write:** interrupted write leaves last consistent state (tmp+rename).
**Do.** `emit_manifest() -> Map<PathBuf,(ContentHash,Arc<str>)>` = the complete
intended project (`emit_rust_file(ALL)` + `program_modules()` + `project_config()`).
Reconciler: compare bytes to disk, write only if different (atomic tmp+rename),
**delete anything under the emit root not in the manifest** (INV-5). **Transactional:**
reconcile only if ALL Ipê-side stages succeeded; on a Ipê-side failure leave disk
at last consistent state and the running binary untouched (INV-3).

### Task 16 — Integrated `ipe build` cargo step (net-new orchestration)

**Goal.** A real `ipe build` that runs emit → reconcile → **timeout-bounded cargo
build** and distinguishes "lowering succeeded" from "cargo succeeded" (H9).
**Test first.** Reuse/promote the `IPE_E2E` cargo test (lib.rs:1116) into a
first-class, timeout-bounded `ipe build` integration test on `tests/golden/basics`:
emit → cargo build → run → assert prints `42`. Assert the "cargo built" signal is
distinct from the "lowering succeeded" signal.
**Do.** Add the cargo invocation (`CGO`-equivalent path detection is Go-only, N/A
here) with a hard timeout ceiling (CLAUDE.md §3), against a warm shared target dir
+ sccache. Print "cold build (first run)" vs warm distinctly; surface ENOSPC as a
first-class failure mode (CLAUDE.md §6), not a mis-attributed codegen regression.
`ipe watch` composes THIS step — it does not own a divergent private driver.

### Task 17 — `project_config` field-granular projection firewall

**Goal.** Editing `[log].level` must not invalidate codegen; editing `entry` must.
**Test first.** Change `[log].level` in `sky.toml` → `emit_rust_file` queries early
-cut (no re-emit). Change `entry` → codegen re-runs.
**Do.** Interpose thin per-field derived queries between the single
`project_config()` input and consumers: `config_entry() = project_config().entry`,
`config_log_level() = project_config().log.level`, one projection per
build-relevant field. Consumers depend on the **specific field query**, never on
`project_config()` directly. Salsa's value-equality back-dating gives
field-granularity with a single input and no bespoke diffing.

---

## Phase D — The correctness gate (primary soundness guard)

### Task 18 — Clean-vs-incremental parity gate (NON-NEGOTIABLE)

> This is the primary guard against every under-invalidation hazard (H1–H7, H13,
> H14). An incremental result that diverges from a clean build is a soundness hole.
> This gate must be green before Tasks 12/13 (granularity refinement) and before
> `ipe watch` (Phase E) are considered done.

**Goal.** For any edit sequence, `emit_manifest()` from an **incrementally-updated**
db == `emit_manifest()` from a **cold db built from the final source state**,
byte-for-byte across all files.

**Test first / this task IS the test.**
`crates/sky_db/tests/clean_vs_incremental_parity.rs`:
1. Take each multi-module fixture under `tests/golden/*` (+ a purpose-built
   multi-module fixture exercising import add/delete/rename/shadow, dead→live
   promotion, and a `Html msg`→`String` export flip).
2. For a scripted edit sequence: (a) apply edits incrementally to a warm db,
   snapshot `emit_manifest()`; (b) build a **fresh** db from the final source set,
   snapshot `emit_manifest()`; (c) assert the two maps are byte-identical (same
   keys, same content hashes, same bytes).
3. Include the adversarial edits that specifically stress each firewall:
   body-only edit, signature flip, module delete, module add that satisfies a
   dangling import, module rename that shadows, `[log].level` toggle,
   dead-fn→live promotion.

**Do.** Wire it into CI as a required check. A divergence **fails the build** and
files a task (CLAUDE.md "spotted = filed"). This gate also catches interner
demand-order nondeterminism (Task 3) and metadata under-firewalling (Task 14).
**Done when.** Green on every fixture + every adversarial edit; CI-required.

### Task 19 — Persisted lowered-IR cache (spec-LOCKED Option B) with completeness gate

**Goal.** Persist per-module lowered IR to `.ipe/lowered/`, content-addressed over
the **whole-project key surface**, behind a **version-epoch directory prefix**.
**Test first.**
- Cold start rehydrates from `.ipe/lowered/` and produces byte-identical emit to a
  no-cache cold build (parity, extended to disk).
- **Deletion/rename does not resurrect (GAP-1 on disk):** build project with module
  X; delete X; cold-start from cache; assert X does not reappear — because the
  address includes the `file_set()`-derived module identity + resolved import
  targets + emitted `mod` list.
- **`ipe add` dependency change invalidates:** change app-crate `Cargo.toml`
  `[dependencies]` without touching any `.ipe`; assert the cached crate is not
  reused (address includes the `Cargo.toml` content hash).
- **Compiler upgrade wipes wholesale:** bump `compiler_revision`; assert the whole
  prior epoch directory is abandoned (H14), never per-entry-trusted across versions.
- Corrupt/truncated entry → discarded → recompute (advisory entries, total).
**Do.** Address = artifact bytes + module identity + resolved import targets +
emitted `mod` list + app-crate `Cargo.toml` hash + version epoch. Entries advisory:
hash miss → recompute, corrupt → discard. This is the on-disk analog of the
`module_interface` completeness obligation.

### Task 20 — Toolchain fingerprint: refuse-don't-guess (GAP-2)

**Goal.** A mid-session `rustup update` / directory override cannot leave a
high-durability emit memo (or the FFI cache) validated against the old toolchain.
**Test first.** Simulate a fingerprint change between revisions; assert the rebuild
**hard-refuses** with `toolchain changed (was A, now B) — restart 'ipe watch'` and
keeps the last-good binary alive; assert no query keyed on the old fingerprint is
silently reused.
**Do.** Re-derive `rustc -vV` + rustup-state at the **start of every revision** in
the driver (milliseconds, never on a salsa query path — preserves INV-1). Differ →
hard-refuse + keep last-good (INV-3). Chosen over set-input-and-recompute because a
toolchain swap invalidates *everything*; a restart is the honest, simplest sound
state. `compiler_revision()` is read once at process start (the running watcher IS
that binary).

---

## Phase E — `ipe watch` (powered by the incremental engine)

### Task 21 — Confined watcher: `WatchedPath` newtype + bounded intake (INV-4, H18)

**Goal.** Observe only a typed, canonicalised, project-root-confined path set.
**Test first.** A symlink resolving outside the project root is **refused**
(`WatchedPath` un-constructible); an excluded dir event (`sky-out/`, `.ipe/`,
`target/`, `.git/`) is dropped at the source; event queue is bounded.
**Do.** Allowlist: `sky.toml`, entry dir recursive `.ipe` walk, `tests/`,
`~/.cache/ipe/ffi/rust/*.ipei` + `kernel.json` (read-only). **Watch directories,
not inodes** (editors tmp-write+rename). Canonicalise every discovered path;
`WatchedPath` only constructible from an in-root canonical path. Bound watched-file
count + total bytes (DoS guard). Watch the FFI interface files so a cross-terminal
`ipe add` is observed (H13) — accepted **only** through the hash-verified
`VerifiedFfiInterface` constructor. **Never** auto-run the FFI inspector on a save
(H19 — introspection is `ipe add`/`install` only).

### Task 22 — Debounce/coalesce + single-flight minimal recompute

**Goal.** Absorb save-storms; recompute only the dirty subgraph; never overlap builds.
**Test first.** A burst of N events within the quiescence window fires **one**
revision; a byte-equal re-save propagates nothing; a change mid-build cancels and
coalesces to the latest state.
**Do.** Quiescence window ~80–120 ms (reset per event) + hard latency cap
~400–500 ms; dedup by canonical path; drop excluded-dir events; bounded queue. On a
settled batch: `set_source_text` for changed files (byte-equal dropped at the input
boundary), `set_project_config` on `sky.toml` change; demand `emit_manifest()`;
reconcile (Task 15); one timeout-bounded cargo build (Task 16). **Single-flight:**
new change mid-build → salsa cancellation (Task 25) → coalesce; never overlapping
cargo builds. Target: warm salsa recompute for a single-body edit well under 100 ms.

### Task 23 — Process state machine: last-good liveness + bounded down-window (H15, H16, H20)

**Goal.** Model the running process so "old killed but new failed to bind" is
unrepresentable; last-good binary stays alive on every red build.
**Test first.**
- Red build (any of parse/canon/type/lower/emit **or** cargo failure) → previously
  -running binary stays alive; diagnostic printed (INV-3, H16).
- Green build with changed binary hash → SIGTERM old → await port release (bounded
  grace → SIGKILL) → spawn new → await `/_sky/readyz`.
- New binary fails readiness → `RespawnLastGood` re-execs the **on-disk last-good
  artifact** and reports the new binary broken (H15).
- Byte-identical binary (comment/test-only change) → no restart, no churn.
- cargo incremental-cache corruption → clean-rebuild emitted crate, last-good stays
  alive until green (H20).
**Do.** Implement the spec's exact state machine
(`RunningGood → RebuildFailed → StopOld → SpawnNew → RunningGood | RespawnLastGood`).
`LastGoodBinary` **only constructible** for a build+process that passed readiness;
it captures **artifact path + content hash** (not a live-process handle) so
recovery survives the old process already being dead. Down-window explicit +
bounded. Every cycle timeout-bounded; child killed when watcher exits; prefer
event-driven monitoring over polling wait-loops (CLAUDE.md §2/§3).

### Task 24 — L0+ session continuity: sqlite dev-store default + schema-gated restore (H21, H22, H24)

**Goal.** A watch-triggered restart lands the user back on their exact Model — the
load-bearing L0+ element — without ever blind-casting a stale blob.
**Test first.**
- Watch defaults Ipe.Live dev session store to `sqlite`; if the app configures
  `memory`, watch warns.
- **Schema-tag reject-before-deserialize (H24):** change the Model type, restart;
  assert the old blob is **rejected on tag mismatch before the deserializer sees the
  bytes** → fresh `init` (not a silent wrong-shape decode).
- **Total on corruption (H22):** truncated same-schema blob → drop session → fresh
  `init`, never a panic.
**Do.** Persisted dev-store blob = length-prefixed `[ ModelSchemaTag header ][
bincode body ]` (replaces Go `gob`; `bincode` is non-self-describing so it cannot
silently fill defaulted fields). `ModelSchemaTag = H(compiler_revision,
structural_hash(Model type))` covering field names, `_fieldIndex` order, and each
field's resolved type recursively. `RestoredModel` only constructible when tag
matches AND blob decodes into the current type; else `init`. **INV-2 unchanged:**
the SSE wire stays a closed set of typed frame kinds (hello/heartbeat/patch/reload);
an unknown/malformed frame is dropped, not interpreted; the `event: reload` emitter
is **absent** (not merely disabled) under the production gate (H23).

### Task 25 — Salsa cancellation (shared by watch single-flight AND the LSP)

**Goal.** A new edit arriving mid-computation cancels the in-flight query walk
(rust-analyzer-style) so watch coalesces and the LSP stays responsive.
**Test first.** Start a long query on thread 1; `set_source_text` on thread 2;
assert thread 1's query unwinds with salsa's `Cancelled` and the db converges to the
latest input state; no stale result is committed.
**Do.** Use salsa's cancellation: a `set_input` on the db signals cancellation to
concurrent readers, which unwind `Cancelled` and retry against the new revision.
Watch's single-flight (Task 22) and the LSP (Task 26) both consume this one
mechanism — cancellation is the shared substrate, not two implementations.

---

## Phase F — LSP handoff seam (Tier-3 #4 depends on this)

### Task 26 — Expose the salsa db as the shared incremental engine for the LSP

**Goal.** Document + expose the seam so Tier-3 #4 (LSP) reuses the **same** salsa
db, queries, and cancellation — watch and LSP share one incremental engine (no
second front-end).
**Test first.** An integration test that drives the db like an LSP would:
`set_source_text` on an open buffer, demand `typecheck(ModuleId)` for diagnostics,
demand `parse`/`resolve_imports` for navigation, cancel on the next keystroke — all
without touching disk (in-memory inputs, editor owns buffer contents).
**Do.** Keep the db `Send`/usable behind the LSP's request loop; the LSP sets
`source_text` from unsaved editor buffers (not files) while `ipe watch` sets it from
disk — same inputs, same queries, different drivers. This task is **seam +
integration test only**; the LSP feature itself is Tier-3 #4. Record the handoff:
salsa is the shared incremental engine for both watch and LSP; the `sky_ir`
cut-point + per-module `typecheck` are exactly the queries the LSP needs for
diagnostics/hover/go-to-def.

---

## Sequencing / dependency graph

```
Task 1 ─▶ Task 2 ─▶ Task 3 (interner PREREQ) ─▶ Phase B (4→5→6→7→8→9→10)
                                                   │
                          Task 11 (coarse spine) ──┤
                                                   ▼
                          Task 18 (PARITY GATE) ◀── stand up EARLY, guards everything
                                                   │
                    Task 12/13 (per-module refine) ┤ (gated by 18)
                                                   ▼
                          Phase C (14→15→16→17)
                                                   ▼
                          Phase D (19→20)  ── uses parity gate
                                                   ▼
                          Phase E (21→22→23→24→25)
                                                   ▼
                          Phase F (26 — LSP seam)
```

**Ship-a-sound-floor rule:** Tasks 1–11 + 14–18 give a correct (coarse) incremental
build guarded by the parity gate. Everything after 12 is granularity/UX
optimisation that the gate protects. If schedule slips, ship the coarse floor;
never ship a granularity optimisation with a red parity gate.

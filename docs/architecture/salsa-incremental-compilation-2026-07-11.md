# Salsa incremental compilation — Phase 1 implementation spec (2026-07-11)

> **Status:** implementation spec for the FIRST landed slice of Tier-3 D.1.
> Downstream of two authoritative docs it does NOT reopen:
> `docs/architecture/incremental-compilation-and-watch.md` (the locked Q1–Q4
> design: query graph, invariants INV-1..5, hazard ledger, Option-B
> persistence, DEFER call-graph firewall) and
> `docs/superpowers/plans/2026-07-03-incremental-salsa.md` (the 26-task plan).
> This doc records what Phase 1 concretely ships against the codebase as of
> HEAD, the salsa version pin, and every deliberate trim/deviation from the
> plan's task list — so the next phase starts from facts, not archaeology.
>
> **Principles order (hard):** security > correctness > soundness > efficiency
> > completeness > readability. Under-invalidation (a stale build that looks
> correct) is a correctness violation and outranks every efficiency gain.

---

## 1. Current stage boundaries (surveyed at HEAD, 2026-07-11)

The workspace is a DAG of stage crates. The driver (`crates/skyc/src/lib.rs`)
runs them one-shot, front to back, on every invocation:

| Stage | Crate | Entry point | Granularity today |
|---|---|---|---|
| Intern | `sky_intern` | `Interner::intern(&mut self, &str)` | `&mut`-threaded through every pass; per-`compile_modules` instance |
| Parse | `sky_parse` | `parse_module(&str, &mut Interner) -> DResult<Module>` | per module |
| Imports (for topo) | `skyc::project` | `extract_imports_from_source(&str)` (string-scan, pre-parse) | per module |
| Canonicalise | `sky_canon` | `canonicalise_module_with_origin(&Module, expected_path, &dep_exports, origin, &mut Interner)` | per module, dep-first, threads `BTreeMap<Vec<Symbol>, ModuleExports>` |
| Link | `sky_canon::link` | `link(entry, Vec<canon::Module>, &Interner)` | **whole-program merge into ONE module** |
| Infer | `sky_types` | over the linked module | whole-program |
| Lower | `sky_lower` | over the linked module | whole-program |
| Emit | `sky_backend_rust` | `RustBackend::emit` | whole-program |

Driver flow (`compile_modules`, `crates/skyc/src/lib.rs`): inject compiled-source
stdlib closure → `topological_order` (imports via string-scan) → per-module
parse+canon loop with one shared `Interner` → link → infer → lower → emit →
write project. Structural facts that shape the salsa port:

- **The whole-program link is the crux** (plan "repo coupling notes"): per-module
  `typecheck`/`lower` queries require decoupling from the linked single module —
  deferred to the plan's Tasks 10–13, NOT touched in Phase 1.
- **The interner is `&mut`-threaded and per-build.** Symbols are dense `u32`s
  whose numeric values depend on interning order. Any salsa query that parses
  must share ONE interner with the rest of the pipeline or symbol identity
  breaks (plan Task 3).
- **`skyc` is one-shot.** There is no long-lived process yet (`ipe watch` is
  plan Phase E); cargo is not invoked by `build` (only the `SKY_E2E`-gated
  test). So Phase 1's job is the *seam*: the salsa database exists, the
  earliest queries run on the production path, and behavior is byte-identical.

## 2. Version pin

**`salsa = "=0.27.2"`** (released 2026-06-25; latest stable on crates.io as of
this spec). Post-"jar" API: `#[salsa::db]` on the database trait/struct/impls,
`#[salsa::input]` / `#[salsa::tracked]` / `#[salsa::interned]` structs and
functions, `salsa::Storage::new(Option<Box<dyn Fn(Event) + Send + Sync>>)` for
the event callback (the memo-hit observability used by the incrementality
tests). Exact-pinned (`=`) because salsa's macro surface churns between 0.x
minors and the plan (Task 1) mandates a deliberate, reviewed bump.

## 3. Phase 1 — what ships now

Phase 1 = plan Tasks **1, 2 (trimmed), 3 (Option 3a), 4, and the imports half
of 5**, wired onto the production `skyc` path.

### 3.1 New crate `crates/sky_db`

- `SkyDatabase { storage: salsa::Storage<Self>, interner: SharedInterner }`,
  `#[salsa::db]`, plus a custom `#[salsa::db] trait Db: salsa::Database`
  exposing the interner to tracked functions.
- `SkyDatabase::with_event_callback(...)` surfaces salsa's `Event` stream
  (`EventKind::WillExecute`) so tests can assert "this query did / did not
  re-execute" — the memo-hit proof mechanism.

### 3.2 Inputs (the parse-don't-validate boundary)

| Input | Shape | Durability | Notes |
|---|---|---|---|
| `SourceFile { module_path: Vec<String>, text: String }` | one per in-scope `.sky` module | low (default) | `text` is the real input; `module_path` is carried for keying/diagnostics |
| `SourceRoot { files: BTreeMap<Vec<String>, SourceFile> }` | the in-scope file set | low (default) | The spec's `file_set()` — the "ALL modules" quantifier as an explicit input |

**Byte-equal re-save is a driver responsibility.** Salsa dirties on every
`set_*` regardless of value, so the boundary no-op lives in the driver helper
`set_text_if_changed(db, file, new_text) -> bool` — compare first, set only on
difference. Covered by a dedicated test.

**Trimmed from plan Task 2** (deliberate, recorded): `project_config()`,
`codegen_flags()`, `ffi_package_interface(PackageId)`, `compiler_revision()`,
`toolchain_fingerprint()` are NOT created in Phase 1. Rationale: they have zero
consumers until lowering/emit enter the graph (plan Phase C) — a settable input
nothing reads is dead surface that can silently rot. The seams stay reserved
exactly as designed in the authoritative spec; adding them is additive and
requires no query-graph redesign. The INV-1 "no `std::fs`/`std::env` on a
tracked path" obligation is enforced now for the queries that exist (see §3.5).

### 3.3 The interning story — plan Task 3, **Option 3a locked**

`SharedInterner(Arc<Mutex<Interner>>)` is owned by the database and shared with
the driver. Properties that make this sound:

- **Append-only.** Symbols are never freed or renumbered, so a memoized
  `Module` from an earlier revision always resolves against the current
  interner (a memo hit cannot dangle).
- **Poison-safe locking.** `lock()` recovers the guard from a poisoned mutex
  (`PoisonError::into_inner`) — sound because append-only interning cannot be
  left in a logically-invalid intermediate state (the same pattern the runtime
  audit blessed for the runtime mutexes). No `unwrap`/`expect`.
- **Deterministic under the one-shot driver.** Symbol numbering depends on
  interning order; the one-shot driver demands queries in the fixed topo order
  with a cold database, so numbering is identical to the pre-salsa code —
  byte-identical emit, proven by the golden suite (§4).
- **Known limitation, deliberately accepted for Phase 1:** on a *warm* database
  (future watch/LSP sessions), a re-parsed module interns new strings at
  different ids than a cold build would assign. Numeric ids must therefore
  never leak into emitted bytes via `BTreeMap<Symbol, _>` iteration order on
  the emit path. This is exactly what the plan's Task 18 clean-vs-incremental
  parity gate exists to prove before any warm-reuse path reaches production;
  until Task 18 is green, **warm reuse stays confined to tests**. The
  demand-order determinism test (§3.6) pins the string-identity half now.

Option 3b (migrate `Symbol` to `#[salsa::interned]`) remains the post-v1
cleanup candidate, unchanged from the plan.

### 3.4 Tracked queries (Phase 1 graph)

| Query | Depends on | Returns |
|---|---|---|
| `parse(db, SourceFile)` | `text(self)` (+ shared interner, append-only) | `Result<Arc<sky_syntax::Module>, Diagnostic>` — errors are **values**, never panics/`?`-escapes (a query is total) |
| `imports(db, SourceFile)` | `text(self)` | `Arc<Vec<Vec<String>>>` via the same string-scan the driver used (`extract_imports_from_source`, moved into `sky_db`, re-exported from `skyc::project`) |

**Why `imports` stays a string-scan (parity choice).** Today the topo sort runs
*before* parse and works even on files whose parse would fail. Deriving
`imports` from `parse` (plan Task 5's eventual shape) changes error ordering /
blame for unparseable modules — an observable behavior change that belongs
behind the parity gate, not in the byte-identical Phase 1. Recorded as a
Phase-2 upgrade.

### 3.5 Wiring behind the one-shot `skyc` entry (byte-identical)

`compile_modules` now: constructs a cold `SkyDatabase` per invocation, creates
`SourceFile` inputs for every module in `sources` + the `SourceRoot` set, and

- routes the topo-sort's imports closure through `sky_db::imports`,
- routes the per-module parse in the canon loop through `sky_db::parse`,
- takes the shared interner from the database (lock-scoped) for the canon /
  link / diagnostic-attribution steps that still need `&mut Interner`.

The interning sequence (parse module₁ → intern expected-path₁ → canon₁ → parse
module₂ → …) is unchanged, the database is cold, and `imports` interns nothing
— so emitted bytes are identical by construction. The golden-oracle SEAL
(140+ `golden_*` byte-diff tests in `crates/skyc/tests/`) is the enforcement.

INV-1 note: both tracked functions read only their salsa inputs (+ the
append-only interner). No `std::fs` / `std::env` / clock on any tracked path;
file reading stays in the driver, which is where `SourceFile` inputs are set.

### 3.6 Incrementality + determinism proof tests (`crates/sky_db/tests/`)

| Test | Asserts |
|---|---|
| `db_smoke` | database constructs, is `salsa::Database`, drops |
| `inputs_roundtrip` | set/read `SourceFile.text`; mutating B leaves A's value and id untouched (inputs independent) |
| `parse_granularity` (the mission's memo-hit proof) | warm the db with `parse(A)`, `parse(B)` (2 `WillExecute`); edit **B only**; re-demand both → exactly 1 new `WillExecute` (B), **zero** for A — the memo hit observed via the salsa event log |
| `byte_equal_resave_noop` | `set_text_if_changed` with identical bytes returns `false` and re-demanding executes nothing |
| `imports_granularity` | same event-log shape for `imports`; editing B never re-scans A |
| `demand_order_determinism` | two cold dbs demanding `parse` in opposite orders resolve every module name to identical **strings** (symbol string-identity is order-independent; numeric ids are not — see §3.3) |

## 4. Coexistence with the golden-oracle SEAL

The SEAL is unchanged and is the gate: every `golden_*` test still byte-diffs
emitted `main.rs` against `tests/golden/*`. Phase 1 adds no output path — salsa
sits strictly upstream of canon on a cold database, so a SEAL divergence would
mean the wiring itself is wrong. `cargo nextest run --workspace` green is the
Phase-1 done-condition; the dedicated clean-vs-incremental parity gate over
*edit sequences* (plan Task 18) becomes mandatory the moment any query result
survives across revisions on a production path (Phase 2+).

## 5. Staged rollout — what remains after Phase 1

| Phase | Plan tasks | Content |
|---|---|---|
| **1 (this spec — DONE)** | 1, 2-trim, 3a, 4, 5-imports | db + inputs + `parse`/`imports` queries on the skyc path, memo-hit proof |
| **2 (DONE — see §7)** | 5 (AST imports), 6, 7, 8 | `resolve_imports` closed-enum edge (add/delete/rename/shadow), `module_interface` firewall + completeness gate, `canonicalize(ModuleId)` tracked |
| 3 | 9, 11, 18 | `kernel_types()`, coarse whole-program spine (`linked_program()` wrapping link→infer→lower→emit), **stand up the clean-vs-incremental parity gate EARLY** |
| 4 | 12, 13 | per-module `typecheck`/`lower` (riskiest; gated by 18; coarse floor is the sanctioned fallback) |
| 5 | 14–17 | `program_metadata()` (conservative, re-runs every build — LOCKED), per-file emit, emit→cargo bridge (content-gated atomic write + manifest prune), config projections |
| 6 | 19, 20 | Option-B persisted lowered-IR cache (whole-project content address, version-epoch), toolchain refuse-don't-guess |
| 7 | 21–25 | `ipe watch` (confined watcher, debounce, last-good state machine, L0+ continuity, cancellation) |
| 8 | 26 | LSP seam — same db, same queries, editor-buffer inputs |

## 6. Phase-1 decisions ledger (rust-analyzer-idiomatic defaults, recorded)

1. **Salsa pin `=0.27.2`** — latest stable; exact pin; bumps are reviewed.
2. **Errors-as-values** in query returns (`Result<Arc<Module>, Diagnostic>`),
   not salsa accumulators — accumulators can come later for *warning* streams;
   the driver's error handling stays structurally identical today.
3. **`Arc<Module>` return** — memo clones are pointer-bumps; `Module` itself
   is `Clone + PartialEq` (contains floats, so no `Eq`).
4. **Interner Option 3a** (db-owned `Arc<Mutex<_>>`) over 3b (salsa-interned
   symbols) — minimal blast radius; 3b filed as post-v1.
5. **Inputs trimmed to consumers that exist** (§3.2) — reserved seams stay
   design-level until their phase.
6. **Cold database per `compile_modules`** — one-shot semantics preserved
   exactly; warm reuse is test-only until the Task-18 parity gate exists.

---

## 7. Phase 2 — implemented (2026-07-11)

Phase 2 = plan Tasks **5 (AST imports), 6, 7, 8** on the production `skyc`
path: the canonicalisation tier is now three tracked queries in `sky_db`, and
the driver's per-module parse+canon loop is a per-module `canonicalize`
demand. The `dep_exports` accumulation map is gone from the driver — the
query graph carries it.

### 7.1 Query graph (added to §3.4's table)

| Query | Depends on | Returns |
|---|---|---|
| `resolve_imports(db, SourceRoot, SourceFile)` | `parse(self)` + `files(root)` | `Result<Arc<Vec<(Vec<String>, ImportResolution)>>, Diagnostic>` — per-import, in declaration order |
| `canonicalize(db, SourceRoot, SourceFile)` | `parse(self)`, `resolve_imports(self)`, `module_interface(dep)` per resolved dep, `origin(self)`, `files(root)` (did-you-mean universe) | `Result<Arc<CanonicalModule>, Diagnostic>` (resolved AST + exports) |
| `module_interface(db, SourceRoot, SourceFile)` | `canonicalize(self)` | `Result<Arc<ModuleExports>, Diagnostic>` |

`ImportResolution` is the closed enum `Resolved(SourceFile) | Unresolved`.
**`Ambiguous` is deliberately unrepresentable**, not merely unconstructed:
`SourceRoot.files` is a `BTreeMap` keyed by module path — the exact invariant
the driver's source map enforces (stdlib injection skips pre-existing keys) —
so two files claiming one module path cannot be expressed at the input
boundary. Recorded as the Phase-2 deviation from plan Task 6's three-variant
sketch; if file discovery ever gains a path where duplicates are possible,
the variant is added *with* the representation change that makes it real.

### 7.2 The firewall is salsa backdating, not a second summarizer

Plan Task 7 sketches `module_interface` "derived from `parse(A)`". Phase 2
instead makes it a **projection of `canonicalize(A)`** (`ModuleExports`, now
`PartialEq`): when a body-only edit re-runs `canonicalize(A)` and the export
surface comes out equal, salsa backdates the interface memo and importers'
`canonicalize` memos validate without re-executing. Rationale (correctness >
efficiency): a second, parse-only export summarizer would be a duplicate
computation of "what does A export" that could silently drift from what
canonicalisation actually injects — the classic under-invalidation seed. One
code path, provably in lockstep, at the cost of re-running the dep's own
canon on its own edits (which is necessary anyway).

Sound over-approximation note (H3): `ModuleExports` carries exported alias
*bodies* including source spans, so an edit that shifts an exported alias's
spans re-canonicalises importers even when nothing semantic changed —
over-invalidation, never staleness. Span-erased interfaces are a filed
follow-up; the Task-7 *type-signature* completeness obligation activates when
typecheck becomes per-module (plan Task 12) — at Phase 2, whole-program
link→infer runs every build, so canon-level exports ARE the complete
cross-module observable.

### 7.3 Byte-identity argument (why the SEAL stays green)

The old loop's interning sequence per module was: memoized parse →
expected-path interning → canon (which interns `Sky`/`Std`/env internals).
`canonicalize` reproduces it exactly: it demands parse / resolutions / dep
interfaces **before** its single interner lock scope (dep interfaces are memo
hits under topo-order demand; `resolve_imports` and the did-you-mean universe
resolve/join strings without interning), then interns the expected path and
runs canon under one guard. Dep-path map keys re-intern strings the dep's own
`canonicalize` already interned — lookups, not appends. Cold database + fixed
topo demand order ⇒ identical append sequence ⇒ identical symbol numbering ⇒
byte-identical emit. Enforced by the 140+ golden byte-diff tests.

### 7.4 Recorded behavioural deltas (error paths only, none golden-visible)

1. **SKY-N0020 did-you-mean universe.** Previously the suggestion list was
   the keys of the driver's *accumulated* dep-exports map — a DFS-order
   prefix of the topo sort (modules that happened to finish first). Now it is
   the full project module set (`SourceRoot` keys, dot-joined,
   lexicographically sorted) — deterministic, complete, and independent of
   traversal order. Strings only: the suggestion path never interns (interning
   not-yet-canonicalised module paths would perturb the symbol numbering the
   SEAL pins). No test pinned the old list; the legacy
   `canonicalise_module_with_origin` entry point keeps the old
   keys-of-the-map behaviour for non-driver callers.
2. **`resolve_imports` is AST-derived** (plan Task 5's shape) because its
   consumer — canonicalisation — iterates the parsed import declarations. The
   pre-parse string-scan `imports` query **stays** in service of the
   topological sort only (§3.4's recorded parity choice); retiring it in
   favour of AST imports changes error ordering for unparseable modules and
   waits for the Task-18 parity gate.
3. **Import cycles**: the driver's topo sort still rejects cycles (SKY-N0021)
   before any `canonicalize` demand. The gate is sound only because the topo
   sort's edge set is a **superset-or-equal of the AST import edges** the
   `canonicalize` demand walk follows. The original pre-parse line scan did
   NOT have that property (it required the literal prefix `"import "`, so
   lexer-legal edges like `import\tB` or `import {- c -} B` were
   scan-invisible; a cycle completed by such an edge bypassed SKY-N0021 and
   reached salsa's dependency-cycle panic on the production path — adversarial
   finding M1). Fixed by replacing the line scan with a token-level scan via
   the real lexer (`sky_parse::scan_import_paths`): the parser consumes the
   same token stream, so every AST edge appears in the scan; over-approximation
   (an `import` token outside the header) is harmless for cycle detection.
   Source that does not lex falls back to the historical line scan for
   ordering only — an unlexable module cannot parse and contributes no AST
   edges. Regression tests: `skyc/tests/adversarial_scan_gap_cycle.rs`,
   `sky_db/tests/adversarial_review.rs`. A *direct* demand on a cyclic graph
   (test/LSP misuse, bypassing the driver's gate) still hits salsa's cycle
   panic — fail-loud, never a stale or silently-fixpointed value.

### 7.5 Phase-2 decisions ledger

1. **`(SourceRoot, SourceFile)` keying** — queries take the root explicitly
   (rust-analyzer style) rather than a global singleton input; salsa interns
   the argument tuple.
2. **Trust origin is an input field.** `SourceFile.origin: ModuleOrigin` is
   set only by the driver from its unforgeable `injected` record; the
   reserved-namespace gate (SKY-N0025) and the stdlib annotation gate key off
   it inside the query. Proven by `stdlib_shadow_stays_rejected`.
3. **Dep interfaces flow by reference.** `canonicalise_module_in_project`
   takes `BTreeMap<Vec<Symbol>, &ModuleExports>` borrowing the interface
   memos — no per-importer deep clone of dep export tables.
4. **Driver clones each canon module out of its memo** (`link` consumes
   `Vec<Module>` by value). One O(AST) clone per module per build — the cost
   of memo-safety until `link` learns to borrow; noted for the Phase-3 coarse
   spine work.
5. **Errors as values, both tiers**: `resolve_imports` propagates the parse
   diagnostic (`Result`, not an empty list — the resolution of an unparseable
   module is *unknown*); `canonicalize` short-circuits on it first, so blame
   attribution in the driver is unchanged.

### 7.6 Phase-2 proof tests (`crates/sky_db/tests/phase2_incrementality.rs`)

| Test | Asserts |
|---|---|
| `module_interface_firewall` (the mission proof) | edit dep A's *private* body; importer B re-demanded → exactly 1 `canonicalize` re-execution (A's), B memo-validated via interface backdating; B's value byte-stable |
| `module_interface_completeness` | widen A's `exposing` list → BOTH A and B re-canonicalise (the firewall must not over-cut) |
| `module_interface_value_stability` | interface value equal across a body edit; unequal across an export change (the property backdating keys on) |
| `canonicalize_granularity` | edit unrelated C → exactly 1 of 3 modules re-canonicalises |
| `resolve_imports_shape` | project import → `Resolved(file)`; kernel import → `Unresolved` |
| `resolve_imports_add_module` | missing dep: red (N0020) → add file to `SourceRoot` → resolution flips `Resolved`, importer re-canons green |
| `resolve_imports_delete_module` | green → remove dep from the set → red (never a stale green) |
| `resolve_imports_rename_module` | rename dep module → importer red; fix the `import` line → green |
| `stdlib_shadow_stays_rejected` | user file at `Std.…` stays SKY-N0025-rejected; same path with driver-vouched `EmbeddedStdlib` origin canonicalises |

---

## 8. Phase 3 — implemented (2026-07-11)

Phase 3 = plan Tasks **18 (the parity gate — stood up FIRST, as mandated),
9, and 11**. Headline result: **the clean-vs-incremental parity gate is
GREEN** — warm-database rebuilds emit byte-identical output to cold builds
across the full golden corpus, closing §3.3's recorded warm-db limitation.

### 8.1 Task 18 — the parity gate, and what it caught

`crates/skyc/tests/clean_vs_incremental_parity.rs`. Both sides drive
`skyc::compile_prepared` — THE production pipeline (see §8.3) — never a
copy:

- **cold side**: fresh db built from the final source state (exactly what a
  one-shot `skyc build` does);
- **warm side**: ONE database reused across a scripted edit sequence,
  inputs reconciled per state via the new `sky_db::sync_source_root`
  driver-boundary helper.

Coverage: every fixture under `tests/golden/*` (4 shards, ~5 s each) runs a
probe-edit → revert sequence (`parity_probe_golden_fixtures_shard{0..3}`)
— the probe appends a never-before-seen top-level identifier to `Main`, so
the warm db re-parses and interns it at a *tail* symbol id where a cold
build interns it mid-parse (the sharpest numbering probe) — plus
`parity_multimodule_adversarial_edits`: body-only edit, export widening,
export type flip (red AND green), module add, module delete, module
rename. Byte-identical file sets + contents, and identical rendered
diagnostics on red states.

**The finding (gate red on first run, as it should be).** The suspected
hazard — symbol *numbering* leaking into emitted bytes — does NOT occur on
this corpus. What DID leak was the lowerer's fresh-name pools:
`Interner::fresh_symbols` skipped any candidate already **interned**, so a
warm rebuild skipped the previous build's own `eta_0…` and minted
`eta_16…` into the emitted Rust (caught on `i121_firstclass_curried`,
`eta_16/eta_17` vs `eta_0/eta_1`) — interner-as-untracked-state inside
lowering. All six pools (`eta_`, `cap_`, `arg_`, `anyp_`, `destr_thunk_`,
`ncons_`) had the defect.

**Root-cause fix (not a workaround).** The collision universe for fresh
names is now a **pure function of the build's source inputs**:

- `sky_parse::scan_identifier_words` — real-lexer scan collecting every
  `Ident` token (full dotted text + segments) plus words inside triple-
  string `{{…}}` interpolation regions: a guaranteed superset of every
  identifier string the parser interns, with no per-AST-node walker that
  could silently under-approximate. Comments and plain string contents
  contribute nothing (matching what the parser interns — this is what
  keeps cold-build bytes unchanged; several goldens name `eta_0` in
  comments).
- `sky_db::identifier_words(file)` — the memoized per-file slice (raw
  word-scan fallback for unlexable text, sound over-approximation).
- `Interner::set_fresh_avoid(set)` — the driver installs the program-wide
  union before the lowering tail; `fresh_symbols` consults the set instead
  of table membership. Unset ⇒ historical whole-table behaviour (unit
  tests, per-build embedders) — equivalent on a cold interner.

Re-minting a pool name on a warm interner returns the same append-only
symbol, so warm and cold builds emit identical names. Proven at the
interner level (`fresh_symbols_avoid_set_*` tests) and end-to-end by the
gate, including the sticky-removal corner (an `eta_0` user identifier
added then removed frees the candidate again — per-build set, never
sticky).

**Residual precision statement.** `sky_types`' `mint_synth_symbol`
(generalized type-var names for untyped-def schemes) still consults
interner membership. The gate is green over the full corpus, so those
names demonstrably do not reach emitted bytes today; if a future change
routes them into emit, the gate is the tripwire, and the same
avoid-set mechanism is the prepared fix.

### 8.2 Task 9 — `kernel_types()`

`sky_types::kernel_type_table(&mut Interner)` materializes every schemed
`StdlibKernel` paired with its inference scheme, read through the SAME
`stdlib_scheme` method inference uses (the test-only minimal builder was
un-gated as `Builder::for_scheme_table`) — one code path, so the memoized
table can never drift from what constraint generation applies.
`sky_db::kernel_types(db, root)` wraps it as a tracked query, keyed on
`SourceRoot` as the forward seam for the parked per-package
`ffi_package_interface` inputs (plan Task 2): when FFI arrives, the query
unions them in with no graph redesign. Proven: memoized, and source edits
never re-derive it (`kernel_types_memoized_and_source_independent`).
Not yet demanded on the production path — infer still derives schemes
internally; the query is the Phase-4 seam for per-module `typecheck`.

### 8.3 Task 11 — the coarse `linked_program()` spine

- `sky_db::topological_order_paths` is now the SINGLE topo algorithm
  (moved from `skyc::project`, which delegates); the memoized
  `topo_order(db, root, entry)` query runs it over the `SourceRoot` keys
  with `imports`-query edges. A cycle returns the SKY-N0021 diagnostic as
  a **value** — and because `linked_program` gates on `topo_order` before
  any `canonicalize` demand, a direct demand on a cyclic graph now yields
  the diagnostic instead of salsa's dependency-cycle panic (strict
  fail-loud improvement over Phase 2's recorded behaviour).
- `linked_program(db, root, entry)` assembles every per-module
  `canonicalize` memo in topo order and links (`LinkedProgram
  { entry_name, module }`). Deliberately coarse: any semantic edit
  re-links the world; byte-equal re-saves and repeat demands execute
  nothing (`linked_program_memoized_coarse_floor`). This is the seam
  Phase 4's per-module `typecheck`/`lower` refines under the now-standing
  parity gate.
- Driver restructure: `compile_modules` = inject → `create_source_root`
  → **`compile_prepared`** (the in-memory production core: topo → blame
  loop → `linked_program` → infer → lower → emit) →
  `write_emitted_project`. `compile_prepared` is public precisely so the
  parity gate drives the real pipeline.

Ordering note (recorded behavioural delta, golden-arbitrated green):
modules **unreachable from the entry** are now appended in sorted
module-path order (`SourceRoot` keys) rather than discovery order. The
DFS-reachable prefix — which determines interning order and linked def
order for every reachable module — is order-independent and unchanged.

### 8.4 Phase-3 decisions ledger

1. **Gate first.** Task 18 was stood up before wiring anything new into
   production, went red on its first run, and the divergence it caught
   (fresh-name pools) was fixed at the root rather than recorded as a gap.
2. **Fresh-name determinism via source-derived avoid set** — chosen over
   (a) a canon-AST symbol walker (a missed arm silently under-approximates
   → capture bug; the token scan cannot under-approximate) and (b) a
   reserved `__sky_`-prefix namespace (would re-bless every golden's
   expected bytes wholesale).
3. **`kernel_types` keyed on `SourceRoot`** — reads nothing from it today
   (documented); the key is the join point where per-project FFI package
   interfaces will enter.
4. **Coarse spine cost**: one extra whole-program module clone per build
   (the linked module leaves its memo by clone). Accepted for Phase 3;
   Phase 4's per-module refinement subsumes it.
5. **Warm-reuse status change**: with the gate green, warm-db reuse
   graduates from "test-only" (§3.3) to *gate-proven on the golden
   corpus*. Production still builds cold-per-invocation; `ipe watch`
   (Phase 7) is what will consume warm reuse, behind this gate in CI.

### 8.5 Phase-3 proof tests

| Test | Asserts |
|---|---|
| `skyc::clean_vs_incremental_parity` (5 tests) | THE gate — see §8.1 |
| `sky_db::phase3_spine topo_order_dep_first_and_memoized` | dep-first order; repeat demand memoized |
| `topo_order_cycle_is_a_value_not_a_panic` | SKY-N0021 as value from both `topo_order` and `linked_program` on a cyclic graph |
| `linked_program_links_all_modules` | whole-program merge carries every module's defs + the entry name |
| `linked_program_memoized_coarse_floor` | repeat demand + byte-equal re-save execute nothing; semantic dep edit re-links (the documented coarse floor) |
| `kernel_types_memoized_and_source_independent` | memoized; source edits never re-derive; query table == direct `kernel_type_table` read |
| `sync_source_root_noop_add_remove` | byte-identical re-sync dirties nothing; module add/remove flows through the file set |
| `sky_intern fresh_symbols_avoid_set_*` (3 tests) | warm re-mint stability, user-identifier dodging, per-build (non-sticky) semantics |

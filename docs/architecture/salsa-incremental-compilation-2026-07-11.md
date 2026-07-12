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
| **4 (implemented — see §9; coarse fallback, NOT per-module)** | 12, 13 | per-module `typecheck`/`lower` (riskiest; gated by 18; coarse floor is the sanctioned fallback) |
| **5 (implemented — see §10; 14+16 shipped, 15 recorded-not-forced, 17 deferred)** | 14–17 | `program_metadata()` (conservative, re-runs every build — LOCKED) — shipped; emit→cargo bridge (content-gated atomic write + manifest prune) — shipped at today's whole-project emit granularity; per-file `emit_rust_file` — genuinely blocked (no `RustFileId` domain exists in the backend today), recorded as a scoped continuation, not forced; config projections — deferred |
| **17 (implemented — see §11)** | 17 | `BuildConfig` salsa input (narrowed to `db_driver`, the one field with a real consumer) + `emit_project(root, entry, config)` — the coarse SEAM over `RustBackend::emit`, replacing Phase 4/5's last remaining plain-function call in `compile_prepared` |
| **6 (implemented — see §12; deliberate EmittedProject-level divergence from literal "lowered IR")** | 19, 20 | On-disk build cache: whole-project content-address key + version-epoch directory (compiler-binary hash + rustc fingerprint) gate — "refuse, don't guess" by construction (stale entries are a different, unreachable address, never a runtime check) |
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

---

## 9. Phase 4 — implemented as the SANCTIONED COARSE FALLBACK, not per-module

Phase 4 = plan Tasks **12, 13**. Headline result: **`typecheck` and
`lower_program` now exist as their own memoized salsa queries**, closing the
gap where `compile_prepared` called `sky_types::infer_attributed` and
`sky_lower::lower` as plain functions on every single build — even a
byte-equal warm rebuild re-ran the whole solver and the whole lowering pass.
That waste is now visible to salsa and skippable. **What Phase 4 does NOT
ship is per-module granularity**: both queries are keyed on `(root, entry)`
and depend on the whole-program [`linked_program`](#83-task-11--the-coarse-linked_program-spine)
merge, so an edit anywhere in the reachable module graph still re-executes
the ENTIRE query, exactly as re-running `infer_attributed`/`lower` would
today. This is the plan's own explicitly sanctioned fallback for "the
riskiest phase" (spec Phase table, this doc's original §5) — recorded here
as a deliberate, load-bearing decision, not an oversight.

### 9.1 Why true per-module `typecheck(ModuleId)` is out of reach today

The survey (reading `sky_types::infer_with_budget_attributed` and
`sky_lower::lower` end to end before writing anything, per this phase's own
mandate) found two independent, compounding sources of whole-program
coupling:

**Inference (`sky_types::constrain` + `solve`).** `Builder::run` builds ONE
[`sky_types::unionfind::UnionFind`] over the ENTIRE linked module — every
def in every reachable module shares the same union-find arena and the same
generated constraint list. The post-solve pipeline then runs, in sequence,
over that single joint constraint set:

1. `solve_attributed` — budget-bounded discharge of every constraint from
   every module at once.
2. **Boundary Scheme Promotion** — generalizes every UNTYPED top-level
   binding at its home module's boundary and discharges every cross-module
   reference against the resulting scheme, fresh per call site. This is the
   ONE place the implementation already partially respects module
   boundaries (scheme generalization per binding, not per program) — but it
   runs as a pass over the whole joint UF state, not as an independently
   invalidatable per-module step, and it explicitly must run before the next
   pass (ordering dependency, see the code comment at
   `sky_types::lib::infer_with_budget_attributed`).
3. `resolve_deferred` — a joint fixpoint over field-access and record-update
   obligations from EVERY module, because a record update in module X can
   pin a field type that a field access in module Y needs (the doc example:
   `{ model | history = snapshots }` in one module enabling `snap.ok` to
   resolve in another).
4. `resolve_route_witness_checks` / `resolve_routed_live_checks` — whole-
   program passes over `Live.app` routing witnesses.

None of these passes is scoped to "the constraints this one module
generated" — they read and write into the same `UnionFind` and the same
`generated: Builder::Output` regardless of which module a constraint's
`home` names. Splitting this into a real per-module query would mean each
module's inference seeds its OWN environment from deps' TYPED interfaces
(inferred schemes, not just the canon-level `ModuleExports`
[`module_interface`] already carries) and discharges its OWN constraints
independently — i.e., building the ML-module-system "compile against
signatures" discipline Boundary Scheme Promotion only halfway implements
today. That is a redesign of `constrain.rs` (7800+ lines) and `solve.rs`,
not a refactor, and every one of the four passes above would need its own
soundness argument for why a per-module scoping doesn't silently
under-constrain a cross-module obligation (a correctness regression, which
this project's principles order ranks above any efficiency gain).

**Lowering (`sky_lower::lower`).** Independently of inference's coupling,
`lower` mints its fresh-symbol pools (`eta_`, `cap_`, `arg_`,
`destr_thunk_`, `ncons_`) sized from WHOLE-PROGRAM facts —
`lower::max_def_arity(m)` and `lower::count_destructure_param_sites(m)` walk
every def in the merged module before lowering begins. A per-module lowering
pass needs those pools either (a) resized per module — which risks exactly
the numbering the golden-oracle SEAL pins, the same bug class Phase 3's
Task-18 gate caught for the fresh-name-avoid-set fix (§8.1) — or (b)
restructured into a composable, incrementally-extensible allocation scheme
that a per-module `lower(ModuleId)` query can grow without renumbering
earlier modules' already-lowered pools. Neither is safe to improvise inside
this phase's budget; both need their own design pass reviewed against the
Task-18 gate before landing.

### 9.2 What shipped instead — the coarse per-program SEAM

`sky_db::typecheck(db, root, entry)` and `sky_db::lower_program(db, root,
entry)` (`crates/sky_db/src/lib.rs`):

- `typecheck` depends on `linked_program(root, entry)`, locks the shared
  interner, and calls `sky_types::infer_attributed` unchanged — same
  computation, same error shape (`(Diagnostic, home)`), now wrapped in a
  memoized salsa node.
- `lower_program` depends on `linked_program(root, entry)` AND
  `typecheck(root, entry)` (a guaranteed memo hit when the driver demands
  `typecheck` first, which it does), locks the interner again, and calls
  `sky_lower::lower` unchanged.
- `compile_prepared` (`crates/skyc/src/lib.rs`) now demands
  `sky_db::typecheck` then `sky_db::lower_program` instead of calling
  `sky_types::infer_attributed` / `sky_lower::lower` directly. The
  `home_to_source` diagnostic-blame map and the `fresh_avoid` set are still
  built in the driver (outside any tracked query, exactly as
  `linked_program`'s consumer-side bookkeeping already was) — no NEW
  interning happens anywhere in this refactor. The interning SEQUENCE
  (parse → canon → link → `Builtins::new` inside `infer_attributed` → the
  fresh-symbol pools inside `lower`) is byte-for-byte the same as before
  Phase 4; only the lock-scope BOUNDARIES moved (one continuous lock became
  three short ones, since the new tracked queries each take their own lock
  internally — the same pattern `canonicalize`/`linked_program` already
  use). This is why the byte-identity SEAL and the Task-18 parity gate stay
  green with zero golden-file changes.
- **Deliberately NOT wired**: `kernel_types(root)` as an explicit dependency
  of `typecheck`. `Builder::run` still derives kernel schemes internally via
  `Builder::stdlib_scheme` (never reads the materialized
  `kernel_type_table`), so adding a `kernel_types(db, root)` demand inside
  `typecheck` would call `Builtins::new` (which interns builtin type-
  constructor names) at a NEW, earlier point in the sequence than today —
  a plausible symbol-numbering perturbation with no compensating benefit
  yet (nothing consumes the materialized table on this path). Recorded as
  a follow-up to attempt ONLY once `Builder::run` is changed to read the
  materialized table instead of re-deriving it — at that point the
  dependency becomes both meaningful (real reuse) and safe to reason about
  in one change.

### 9.3 What this buys, honestly

- **Real, salsa-proven memoization** where there was none: `typecheck_memoized_coarse_floor`
  and `lower_program_memoized_coarse_floor`
  (`crates/sky_db/tests/phase4_seams.rs`) prove a repeat demand and a
  byte-equal re-save execute zero new `WillExecute` events for either query
  — before Phase 4, `infer_attributed`/`lower` re-ran unconditionally on
  every `compile_prepared` call, warm or not.
- **The query-graph seam future work needs** — `typecheck(root, entry)` and
  `lower_program(root, entry)` are now NAMED, independently-observable salsa
  nodes a future per-module redesign can retarget (change their key shape
  to `ModuleId`, loop `compile_prepared` per module) without the driver's
  overall calling convention changing shape, matching `linked_program`'s own
  documented intent ("Phase 4's per-module typecheck/lower refinement
  replaces the CONSUMER SIDE of this query").
- **Explicitly NOT claimed**: any reduction in re-typecheck/re-lower scope
  for a body-only edit to one module in a multi-module program.
  `typecheck_is_program_wide_not_per_module`
  (`crates/sky_db/tests/phase4_seams.rs`) is a REGRESSION-PROOF of the
  coarseness itself — it asserts that editing module C's body (no import
  edge to sibling module A) still re-executes `typecheck` in full. This
  test exists so a future contributor cannot accidentally believe Phase 4
  shipped fine-grained typecheck by reading only the "memoized" tests.

### 9.4 Phase-4 continuation scope (for the next session)

In priority order, matching the two coupling sources found in §9.1:

1. **Design a typed cross-module interface.** Extend `module_interface`
   (or add a sibling query) that carries not just `ModuleExports`'
   canon-level surface but each exported binding's INFERRED scheme —
   requires running a bounded, self-contained inference pass per module
   that only needs deps' schemes (not their bodies) as input, mirroring
   what Boundary Scheme Promotion does for untyped bindings today, widened
   to cover typed bindings and the deferred field-access/record-update/
   routed-Live-check passes. This is the crux; everything else is
   downstream of it.
2. **Prove the four post-solve passes are sound per-module.** For each of
   solve/Boundary-Scheme-Promotion/resolve_deferred/routed-checks, write
   the argument (and a regression test) for why scoping it to one module's
   constraints plus its deps' typed interfaces cannot under-constrain a
   cross-module obligation. Do this BEFORE writing the per-module
   `typecheck` query, not after — a red Task-18 gate after the fact is a
   correctness bug already shipped to a memo, not a design review.
3. **Only then** tackle lowering's fresh-symbol-pool sizing (§9.1,
   independently risky) — a composable per-module allocation scheme,
   proven against Task-18 before touching the production path.
4. **Wire `kernel_types` for real** — change `Builder::run` to consume the
   materialized `kernel_type_table` instead of re-deriving schemes inline,
   THEN add the `typecheck`-depends-on-`kernel_types` edge (§9.2's
   deliberately-skipped step) once the interning-order risk is designed
   away rather than merely avoided.

### 9.5 Phase-4 proof tests

| Test | Asserts |
|---|---|
| `sky_db::phase4_seams typecheck_memoized_coarse_floor` | repeat demand + byte-equal re-save execute nothing; dep body edit re-executes (coarse floor) |
| `typecheck_is_program_wide_not_per_module` | editing an UNRELATED sibling module's body still re-executes the whole seam — the coarseness is a regression-proof, not just an absence of a finer test |
| `lower_program_memoized_coarse_floor` | same shape one layer down; `typecheck` and `lower_program` re-execute in lockstep on a dep edit |
| `lower_program_short_circuits_on_typecheck_error` | a red program never reaches `sky_lower::lower` — `lower_program`'s error is `typecheck`'s own diagnostic, verbatim |
| `skyc::clean_vs_incremental_parity` (5 tests, re-run) | still green — zero golden-file changes, proving the lock-scope restructuring preserved the exact interning sequence |

---

## 10. Phase 5 — `program_metadata()` shipped; per-file emit + emit→cargo
## bridge SPLIT: bridge shipped, per-file emit is a recorded, NOT-forced gap

Phase 5 = plan Tasks **14, 15, 16, 17**. Unlike every prior phase, Task 15
(`emit_rust_file(RustFileId)`) is **not** shipped even in a coarse-fallback
shape — the survey (mandatory before writing anything, same discipline as
Phase 4 §9.1) found that the *precondition* for a coarse `emit_rust_file`
fallback does not exist yet, and forcing one into place this session would
have meant either inventing a backend redesign well outside a single
session's sound-review budget, or shipping a query that is named
`emit_rust_file` but is not actually per-file — exactly the "looks like
progress but isn't" trap this project's non-negotiables forbid. What shipped
instead: Task 14 in full, Task 16 in full (at the granularity that genuinely
exists today), Task 15 recorded as a precisely-scoped gap for the next
session, Task 17 not attempted (budget spent on 14/16 and the Task-15
survey).

### 10.1 Task 14 — `program_metadata()`, shipped as designed

`sky_db::program_metadata(db, root, entry)` (`crates/sky_db/src/metadata.rs`)
is a tracked query depending **directly** on [`lower_program`] (Phase 4's
coarse per-program seam) — never firewalled behind an interface summary.
This gets the design spec's own H6 lock ("Global DCE/mono firewalled behind
interfaces → dead-fn-promoted-to-live not re-emitted") **by construction**:
because `lower_program` is itself the coarse whole-program spine, ANY
semantic edit anywhere already re-executes it, and therefore already
re-executes `program_metadata` too — no special "never firewall this"
mechanism needed, the coarseness inherited from Phase 4 already has the
locked property. What downstream consumers gain from this being a *salsa*
query rather than a plain function call is exactly what the design doc
promises: early-cutting on a byte-identical `ProgramMetadata` output
(`Arc<ProgramMetadata>: PartialEq` backdating) even though the query itself
always re-executes.

**What it computes, and the honestly-recorded scope limit.** Confirmed by
reading `sky_lower::lower` end to end (Phase 4 already established
`Program { modules: vec![module] }` — always exactly one lowered
`sky_ir::Module`; §9.1's finding still holds): `ProgramMetadata` carries

- `reachable_funcs: BTreeSet<FuncId>` — a genuine fixpoint over the
  whole-program call graph (`Expr::Call`'s `Callee::Func` AND
  `Expr::FuncValue`'s first-class references), seeded from the module's
  `entry: Option<FuncId>` (set when `sky_lower::lower` finds a def literally
  named `main`). Proven by three tests: a function nothing reachable calls is
  excluded (`program_metadata_excludes_unreached_function`), a
  transitively-reachable function two calls deep is included
  (`program_metadata_reachability_is_transitive`), and a program with no
  `main` binding falls back to "every function reachable" — the sound,
  never-under-report direction for a set nothing consumes for pruning yet
  (`program_metadata_no_entry_falls_back_to_conservative_reachable_everything`).
- `reachable_types: BTreeSet<(ModPath, Symbol)>` — every enum type
  CONSTRUCTED (`Expr::Ctor`) or PATTERN-MATCHED (`Pat::Ctor`) inside a
  reachable function body. **Deliberately NOT closed over declared
  `EnumDef` variant field types** — a type reachable only via an
  unconstructed/unmatched payload field would be missed. Sound today ONLY
  because `program_metadata` is a forward seam, not yet a dependency of any
  pruning pass (the same status Phase 3's `kernel_types` shipped with — see
  §8.2). A future consumer that actually PRUNES dead code from emission MUST
  close this over `EnumDef` field types first; recorded here so the gap is
  never silently assumed away.

Both walkers (`walk_expr` over all 30 `Expr` variants, `walk_pat` over all 11
`Pat` variants) are EXHAUSTIVE matches — no wildcard arm — so a future IR
variant cannot be silently under-walked; the compiler forces this file to be
revisited when `sky_ir::Expr` or `sky_ir::Pat` grows a case (the same
discipline `CLAUDE.md` §8 requires of the Go/Haskell compiler's AST walkers,
applied here to the Rust IR).

**Wired onto the production path as a forward seam.** `compile_prepared`
(`crates/skyc/src/lib.rs`) now demands `sky_db::program_metadata` right after
`lower_program`, before emission — mirroring `kernel_types`'s Phase-3
"materialized before it has a real consumer" status. Nothing downstream
reads its value (no pruning pass exists), so this demand changes ZERO
emitted bytes; the point is purely to put the query on the same path the
clean-vs-incremental parity gate drives, so a future divergence in this
analysis (a panic, an infinite loop, a wrong diagnostic) cannot go
undetected. Proof: `crates/sky_db/tests/phase5_metadata.rs`, 5 tests (memo
hits, dep-edit re-execution, the reachability computation itself, the
lower-error short-circuit, the no-entry fallback).

### 10.2 Task 15 — `emit_rust_file(RustFileId)`: genuinely blocked, not forced

The design table's shape (`docs/architecture/incremental-compilation-and-
watch.md` row `emit_rust_file(RustFileId)`) requires a `RustFileId` domain —
one Rust file per Sky module, so a body edit to ONE module changes only that
file's text. Reading `sky_backend_rust::project::emit_program` end to end
(the mandatory survey before writing anything) found this domain **does not
exist anywhere in the pipeline today**, for two independent, compounding
reasons — the second is a NEW finding beyond what Phase 4 §9.1 already
recorded for lowering:

1. **Phase 4's finding still applies one layer further down.**
   `sky_lower::lower` always produces exactly ONE `sky_ir::Module`
   (`Program { modules: vec![module] }`) regardless of how many Sky source
   modules were linked — so there is no `program_ir_module(ModuleId)` to key
   `emit_rust_file`'s `owner` dependency on, mirroring the exact
   whole-program IR coupling §9.1 documented for typecheck/lower.
2. **NEW: the backend itself emits ONE `src/main.rs`, not one file per Sky
   module, even given a hypothetical per-module IR.**
   `emit_program` (`crates/sky_backend_rust/src/project.rs:332`) iterates
   `program.modules` and concatenates EVERY module's types then EVERY
   module's funcs into a single growing `String`, written once as
   `files.insert(RelPath::new("src/main.rs")?, out)`. The only OTHER files
   ever produced are two small, program-wide-flag-driven runtime shims
   (`sky_runtime/mod.rs`, `sky_runtime/config.rs`) — neither varies per Sky
   module either. There is currently no `RustFileId` value space to iterate
   at all, coarse or fine.

**Why this is not forced into a fake-coarse fallback (unlike Phase 4).**
Phase 4 had a real, safe fallback available: `typecheck`/`lower_program`
already existed as plain whole-program functions, so wrapping them in a
tracked query cost nothing and genuinely proved memoization. There is no
equivalent safe move here: the only way to give `emit_rust_file` a REAL
per-file domain is to split `emit_program`'s monolithic `main.rs` into
one `.rs` per Sky module — which requires, at minimum, (a) `mod`
declarations and cross-module `pub`/`use` visibility in the emitted Rust
(today every def is a bare top-level item in ONE file, so cross-module name
resolution is free — splitting reintroduces a whole visibility design), (b)
an ownership rule for `EmitCtx::record_structs()` — the deduplicated,
program-wide closed-record-shape table — deciding which Rust file a shared
synthesised struct lives in when two different Sky modules construct the
same shape, (c) relocating the fixed kernel-wrapper prelude / `main()`
entry / TEA-alias block that today anchors the single file, and (d) **every
one of the 140+ `golden_*` byte-diff tests currently pins ONE
`src/main.rs`** — splitting the file boundary is a breaking change to the
golden-oracle SEAL itself, not an additive one, so the parity-gate-first
discipline (§8.4 decision 1: "Gate first... went red on its first run") has
no smaller slice to stand up first. Each of (a)–(d) needs its own design +
review pass; attempting any of them inside this session's remaining budget
would mean either skipping the review this project's principles order
(security > correctness > soundness > efficiency > completeness >
readability) demands, or shipping something that LOOKS like Task 15 but
changes no observable incrementality property — the "unsound-looking-like-
progress" trap the mission brief explicitly forbids.

**Recorded, not shipped.** `emit_rust_file` does not exist in `sky_db`.
`compile_prepared`'s call to `RustBackend::emit` is UNCHANGED from Phase 4 —
still a plain function call, not wrapped in any tracked query — because
wrapping the CURRENT whole-project `emit` in a coarse seam (the `emit_project`
shape a literal reading of "ship the sound floor" might suggest) turned out
to need its OWN new salsa input (`DbDriver`'s selection has no existing
input home — Phase 1 §3.2 explicitly trimmed `project_config()` for having
zero consumers, and Task 17 is exactly where that input belongs) threaded
through EVERY call site of `compile_prepared`, INCLUDING the Task-18 parity
gate itself (`crates/skyc/tests/clean_vs_incremental_parity.rs`) — touching
the gate's call shape for a memoization win with no real per-call-site
value (a fresh salsa input recreated on every warm-side call would defeat
the very memoization being proven) was judged not worth the risk this
session. **Continuation scope for the next session, in priority order:**

1. ~~Land Task 17's `project_config()` (or a narrower `BuildConfig { db_driver
   }` input) FIRST~~ — **DONE, see §11.** Landed as exactly the narrower
   `BuildConfig { db_driver }` shape this item names.
2. ~~THEN wrap `RustBackend::emit` in a coarse `sky_db::emit_project(root,
   entry, config)` tracked query~~ — **DONE, see §11.** Both parity-gate call
   sites (`clean_vs_incremental_parity.rs` AND
   `adversarial_review_parity_probe.rs`, the latter not enumerated here but
   the same shape) now hold a stable `BuildConfig` handle exactly as this
   item specifies.
3. ONLY THEN attempt the real `emit_rust_file(RustFileId)` split — its own
   multi-session design pass (mod/visibility scheme, record-struct ownership
   rule, golden-suite re-baseline strategy), reviewed against the Task-18
   gate before touching the production path, mirroring exactly how Phase 4
   staged lowering's fresh-symbol-pool sizing as future work rather than
   improvising it. **Still not attempted** — unchanged status after §11/§12.

### 10.3 Task 16 — the emit→cargo bridge, shipped at the granularity that
### exists today

`crates/skyc/src/lib.rs`'s `write_emitted_project` is now three functions
implementing the content-gated-write + manifest-driven-prune shape at the
CURRENT emit granularity (whole-project `EmittedProject` + the vendored
runtime tree) — the per-file split blocked in §10.2 is not a precondition
for this: the "manifest" the bridge reconciles against is simply the
complete set of paths a build produces, however many files that is today.

- **`build_emit_manifest`** assembles `BTreeMap<PathBuf, String>` — every
  path this build intends to produce, relative to `out_dir`, mapped to its
  exact text — from three sources, in the SAME precedence
  `write_emitted_project` always used (vendor-then-emit, so the backend's
  trimmed `mod.rs`/`config.rs` win over the fuller source-tree copies): the
  vendored runtime tree (read recursively via the new `collect_dir_text`),
  `Cargo.toml`, and `emitted.files`. Every file this driver ever writes is
  UTF-8 Rust/TOML source, so `String` (not raw bytes) is the honest content
  type here — a non-UTF-8 file under `runtime_dir` surfaces as an
  `CliError::Io`, never a panic (`fs::read_to_string`'s own error path).
- **`reconcile_emitted_project`** writes each manifest entry via
  **`write_if_changed`** (H8): reads the existing file first and skips the
  write entirely when the content already matches, so an unchanged warm
  rebuild touches NO mtimes and therefore never bumps `cargo`'s own
  incremental-build invalidation. The actual write reuses the PRE-EXISTING
  `write_atomic` helper (previously only used by `sky doctor --fix`'s
  patch-application path) rather than a second, parallel atomic-write
  implementation — one tmp-then-rename code path, with its established
  cleanup-on-rename-failure behaviour, now serves both callers.
- **`prune_orphaned_files`** (H7) walks `out_dir/src` AFTER every write and
  deletes any file whose path is not a manifest key — an orphaned/stale
  `.rs` left over from a deleted Sky module, or a file removed from the
  vendored runtime tree upstream, can no longer linger and silently keep
  compiling. Scope is deliberately confined to `out_dir/src`: the walk never
  touches the project root, so `Cargo.lock`, a `target/` build-cache
  directory, or anything else `cargo` itself manages there is structurally
  unreachable from this pass — the manifest only ever claims `src/**` plus
  `Cargo.toml`, so pruning outside `src/` would be pruning against a
  manifest that was never authoritative for that scope in the first place.
  Directories themselves are never removed (only files) — leaving an empty
  directory behind is harmless (`cargo` does not care) and keeps this pass's
  blast radius to exactly "stale file", nothing structural.

The old `copy_dir` (unconditional recursive `fs::copy`, no staleness check,
no prune) is deleted — `collect_dir_text` + the reconciler replace it
end to end. Behavioural delta on a byte-identical rebuild: previously every
vendored + emitted file was rewritten unconditionally (bumping every mtime
even when nothing changed); now nothing is written at all. On a build that
DOES change (any first build, or any edit), output is byte-identical to
before — the golden-oracle SEAL and the Task-18 parity gate do not compare
mtimes, only content, so this is invisible to both and requires no gate
changes.

### 10.4 Task 17 — not attempted this session; landed next session, see §11

Session budget went to the Task 14 implementation + proof, the Task 15
survey (whose negative finding — "the precondition doesn't exist" — took
real investigation to establish soundly rather than assumed), and Task 16's
implementation + review. `project_config()` field-granular projections are
recorded as the Task-15-continuation's first step (§10.2 item 1) rather than
attempted standalone here — landing it without a consumer would repeat the
exact "reserved surface nothing reads is dead surface that can silently rot"
trap Phase 1 §3.2 explicitly named as its reason to trim inputs to have real
consumers.

**Update (next session, §11):** landed as `sky_db::BuildConfig` +
`sky_db::emit_project`, narrowed to the ONE field (`db_driver`) that clears
the real-consumer bar — see §11 for the full shape and for why the
MULTI-field half of "field-granularity" (two config fields independently
firewalled against each other) stays out of scope until a second field
earns its place the same way `db_driver` had to.

### 10.5 Phase-5 decisions ledger

1. **`program_metadata` gets its "never firewalled" property from
   `lower_program`'s existing coarseness, not from a new mechanism** — the
   cheapest possible way to satisfy H6, and the only way that could not
   itself introduce a NEW under-invalidation risk (a bespoke "always dirty"
   flag would be one more thing to keep sound).
2. **`reachable_types` is not closed over `EnumDef` field types** —
   documented gap, sound only while nothing consumes it for pruning (mirrors
   `kernel_types`'s Phase-3 status exactly).
3. **Task 15 is a recorded gap, not a forced fallback** — the first Phase in
   this effort where "ship the sound floor" concluded "there is no floor
   here yet, only design work" rather than "wrap the existing function."
   Distinguishing these two outcomes honestly is itself the discipline this
   project's non-negotiables require.
4. **Task 16 needed zero backend changes** — proof that the emit→cargo
   bridge and the per-file emit split are genuinely INDEPENDENT concerns;
   Task 16 does not become easier or harder once Task 15 eventually lands
   (the manifest shape is agnostic to how many files it contains).
5. **`write_atomic` reuse over a parallel implementation** — one atomic-write
   code path for both `sky doctor --fix` and the emit→cargo bridge; a second
   implementation is a second place for a rename-on-failure bug to hide.

### 10.6 Phase-5 proof tests

| Test | Asserts |
|---|---|
| `sky_db::phase5_metadata program_metadata_memoized_coarse_floor` | repeat demand + byte-equal re-save execute nothing; dep body edit re-executes (coarse floor, same shape as Phase 4) |
| `program_metadata_short_circuits_on_lower_error` | never reaches the structural walk on ill-typed input; surfaces `lower_program`'s own diagnostic verbatim |
| `program_metadata_excludes_unreached_function` | a function nothing reachable calls is absent from `reachable_funcs` — the actual DCE-reachability proof |
| `program_metadata_reachability_is_transitive` | a function reachable only via an intermediate call is included |
| `program_metadata_no_entry_falls_back_to_conservative_reachable_everything` | no `main` binding → every function reachable (never under-reports) |
| `skyc::clean_vs_incremental_parity` (5 tests, re-run) | still green — `program_metadata`'s production-path demand and the emit→cargo bridge rewrite change zero emitted bytes |

---

## 11. Task 17 — `BuildConfig` + `emit_project`, landed with a real consumer

Task 17 = plan Task 17 (`project_config()` field-granular projections),
carried forward from §10.4's "not attempted" status. Landed as
`sky_db::BuildConfig` (one field: `db_driver`) + `sky_db::emit_project` — a
genuine salsa input with a genuine tracked-query consumer, not the
"reserved surface nothing reads" shape Phase 1 §3.2 named as the anti-
pattern to avoid.

### 11.1 Why narrowed to one field, and what that costs honestly

The design doc's `project_config()` (§Q1b) sketches a full parsed-`sky.toml`
shape (`entry`, `codegen_flags`, `[log]` fields, …) with **thin per-field
projection queries** interposed so editing one field's consumer doesn't
retrigger a query that reads a different field. Reading the actual
`salsa-0.27.2` source before writing anything (this phase's own survey
discipline) found that this projection-query layer is **already built into
salsa's `#[salsa::input]` macro** — verified at
`salsa-macro-rules-0.27.2/src/setup_input_struct.rs` +
`salsa-0.27.2/src/input.rs`: each field getter calls
`IngredientImpl::field`, which reports a tracked read keyed on
`(ingredient_index.successor(field_index), id)` — **per field**, not per
struct (`type Revisions = [Revision; N]`, one revision slot per field, set
independently by each field's setter). So a `BuildConfig` with TWO
build-relevant fields would already get "editing field A never invalidates
a query that only reads field B" for free, no hand-rolled `config_entry()`
/ `config_log_level()` projection queries needed — this is a genuine,
load-bearing finding about the salsa version this project is pinned to,
recorded here so a future session does not re-derive the design doc's
projection-query mechanism as new work when salsa already provides it.

What this does NOT change: `BuildConfig` still has exactly ONE field today,
because no second field has a real tracked-query consumer yet — the SAME
bar `db_driver` itself had to clear (Phase 1 §3.2's discipline, applied
again). Adding `entry`/`codegen_flags`/`[log]` fields with no consumer
would be exactly the dead-surface trap this doc keeps naming. So the
MULTI-field half of "field-granularity" (two fields on the SAME struct,
independently firewalled from each other) is honestly unproven today — not
because it wouldn't work (salsa gives it for free, per the finding above),
but because there is nothing yet to prove it WITH. What IS proven, by
`emit_project_config_change_does_not_retrigger_lower`
(`crates/sky_db/tests/phase6_build_config.rs`), is the other half of the
same property: a `BuildConfig`-only edit never retriggers `linked_program`
/ `typecheck` / `lower_program` — config lives on its own input, entirely
separate from `SourceRoot`/`SourceFile`.

### 11.2 `emit_project` — the real consumer

`sky_db::emit_project(db, root, entry, config)` (`crates/sky_db/src/
lib.rs`) is the Phase-4-shaped coarse SEAM over `RustBackend::emit`,
exactly the pattern `typecheck`/`lower_program` established one layer up:
depends on `lower_program(root, entry)` for the IR and
`config.db_driver(db)` for the ONE emit-relevant config field, and replaces
what was, before this task, the LAST remaining plain-function call in
`compile_prepared` (every earlier stage — parse through lower — was already
a tracked query; only the final `RustBackend::new(&interner).emit(&program)`
call had never been wrapped). `compile_prepared`'s signature changed from a
raw `db_driver: DbDriver` parameter to a `config: BuildConfig` handle,
constructed once by the caller.

**The stable-handle trap, closed at the call sites.** Phase 5 §10.2 item 2
already named the hazard: a `BuildConfig` constructed FRESH on every
`compile_prepared` call gets a new salsa `Id` each time, so `emit_project`'s
memo key never matches across calls and the seam's memoization is silently
defeated. `compile_modules` (the one-shot production driver) constructs one
`BuildConfig` per build — fine, it never re-demands. The Task-18 parity
gate's `WarmSession` (`clean_vs_incremental_parity.rs` AND
`adversarial_review_parity_probe.rs`) now holds `config: Option<BuildConfig>`
alongside its existing `root: Option<SourceRoot>`, created once via
`get_or_insert_with` and reused across the whole warm edit sequence — the
SAME lazy-stable-handle shape `root` already used, applied to `config` too.

### 11.3 Task-17 decisions ledger

1. **`BuildConfig` narrowed to `db_driver`** — the one field that clears
   the "has a real tracked-query consumer" bar; a broader `ProjectConfig`
   stays design-level until a second field earns its place (§11.1).
2. **Per-field tracking is salsa-native, not hand-rolled** — verified
   against the pinned `=0.27.2` source; no projection-query boilerplate
   needed for the property the design doc describes.
3. **`config` is a caller-supplied, stable handle, not constructed inside
   `compile_prepared`** — closes the exact warm-sequence memoization trap
   Phase 5 §10.2 recorded in advance.
4. **`db_driver` never reaches `linked_program`/`typecheck`/`lower_program`**
   — those queries take no `BuildConfig` dependency at all; only
   `emit_project` reads it. This is what makes the config-vs-source
   isolation observable (§11.1's proof test), not an incidental side effect.

### 11.4 Task-17 proof tests (`crates/sky_db/tests/phase6_build_config.rs`)

| Test | Asserts |
|---|---|
| `emit_project_memoized_coarse_floor` | repeat demand + byte-equal re-save execute nothing; dep body edit re-executes end to end (same shape as every prior seam) |
| `emit_project_config_change_does_not_retrigger_lower` | a `db_driver`-only edit re-executes `emit_project` but ZERO executions of `linked_program`/`typecheck`/`lower_program` — the mission proof |
| `emit_project_source_edit_retriggers_lower_and_emit` | the other direction: a plain source edit (config untouched) re-executes the whole chain through to `emit_project` |
| `emit_project_short_circuits_on_lower_error` | never reaches `RustBackend::emit` on ill-typed input; surfaces `lower_program`'s own diagnostic verbatim |
| `skyc::clean_vs_incremental_parity` (5 tests, re-run) | still green — the `compile_prepared` signature change (`db_driver` → `config`) and the `emit_project` wiring change zero emitted bytes |
| `skyc::adversarial_review_parity_probe` (4 tests, re-run) | still green |

---

## 12. Phase 6 — on-disk build cache (Tasks 19/20), at `EmittedProject`
## granularity: a deliberate, recorded divergence from literal "lowered IR"

Phase 6 = plan Tasks **19, 20**. Headline result: `skyc build` now survives
ACROSS process invocations for the first time — Phases 1–5 proved every
front-end/back-end stage memoizes correctly WITHIN one process's salsa
database, but every `skyc build` still started that database cold. A
same-project, same-toolchain rebuild with no source changes now skips the
ENTIRE compile pipeline (parse through emit) via a content-addressed,
version-epoch-gated on-disk cache (`crates/skyc/src/cache.rs`).

Same discipline as Phase 4 (§9.1) and Phase 5 (§10.2): survey the design
doc's literal wording BEFORE writing anything, and when the literal shape
hits a genuine soundness blocker, ship the largest SOUND slice and record
the gap honestly rather than force a fake-coarse version of the blocked
shape.

### 12.1 The survey: why literal "persisted lowered IR" is blocked, and
### what ships instead

The design doc's Option-B (`docs/architecture/incremental-compilation-and-
watch.md` §"Cross-session persistence", LOCKED) says: persist per-module
lowered IR to `.ipe/lowered/`. Two compounding findings, read before writing
anything:

1. **Phase 4's finding still applies.** `sky_lower::lower` always produces
   exactly ONE whole-program `sky_ir::Program` (`Program { modules:
   vec![module] }`) — there was never a `ModuleId`-keyed IR to persist "per
   module." This much was already known from Phase 4 §9.1 / Phase 5 §10.2.
2. **NEW: `Symbol` identity does not survive a process boundary.**
   `sky_ir::ir` embeds `sky_intern::Symbol` pervasively — `Var`, `Ctor`,
   record field keys (`BTreeMap<Symbol, IrType>`), `FuncSig` params and
   generics, `EnumDef`/`TypeDef` fields, and more (confirmed by a direct
   grep across `crates/sky_ir/src/ir.rs`: dozens of `Symbol`-carrying
   sites, far beyond the `Ctor`-only surface `sky_db::program_metadata`'s
   walker touches). A `Symbol` is a raw `u32` index into THIS PROCESS's
   `sky_intern::Interner` — meaningless (not merely differently numbered)
   against a fresh process's empty interner. Making a persisted
   `sky_ir::Program` sound requires a relocation pass: serialize every
   embedded `Symbol` as its resolved STRING, and on load, re-intern each
   string into the CURRENT process's interner and rewrite every occurrence
   to the newly-assigned id — an exhaustive walker over every
   `Symbol`-carrying IR site (this project's own non-negotiable #8: "New
   AST nodes require explicit walker arms"), plus full `serde` coverage
   across roughly twenty IR types. That is a genuine, multi-session
   redesign, not a corner cuttable inside this session's budget — the SAME
   "looks like progress but isn't" trap Phase 5 §10.2 named for
   `emit_rust_file(RustFileId)`.

**What ships instead**: `sky_backend::EmittedProject` — the output of
Task 17's `sky_db::emit_project` — is cached. It is pure `String` data
(`RelPath` wraps a `String`; `files: BTreeMap<RelPath, String>`;
`cargo_toml: String`) with **zero** dependency on any interner or `Symbol`,
so it serializes and deserializes losslessly with no cross-process identity
risk whatsoever — no relocation pass needed, because there is nothing left
to relocate by the time `RustBackend::emit` has already resolved every
`Symbol` to its final Rust identifier text.

**Why this is not a lesser win, for `skyc build`'s actual use case.** A
cache hit at the `EmittedProject` level skips parse → canon → link → infer
→ lower → emit ENTIRELY — strictly MORE work skipped than a literal
lowered-IR cache would give (which would still re-run `RustBackend::emit`
on every hit). The cost is real but narrow: this cache cannot serve a
hypothetical future interpreter tier that wants to consume `sky_ir`
directly (design doc §"Why `sky_ir` is the cut-point", motivating Q3) —
that tier is unscheduled and does not exist, so the cost is paid by nobody
today. This substitution is recorded here as a deliberate, reasoned
divergence, not a silent one — if/when literal IR-level caching becomes
necessary (the interpreter tier lands), the relocation-pass design above is
the scoped starting point, and this cache's content-address + version-epoch
machinery (§12.2/§12.3) carries over unchanged — only the CACHED VALUE TYPE
changes, not the addressing scheme.

### 12.2 The content-address key (Task 19)

`cache::compute_project_key` (`crates/skyc/src/cache.rs`) hashes, with
explicit little-endian length-prefixed framing for every variable-length
field (never delimiter-joined — proven by
`key_is_delimiter_collision_safe`, which checks `[["AB"],["C"]]` and
`[["A"],["BC"]]` hash to DIFFERENT keys):

- the entry module path,
- the SQL driver (`sky_backend_rust::DbDriver`),
- every in-scope module's path, trust origin (injected stdlib vs. user
  source), and full source text.

This mirrors the design doc's own cache-key-completeness note for the
persisted cache (ties GAP-1): module IDENTITY (not just content) is in the
key, so an add/delete/rename of a module yields a genuinely different
address, never a stale hit that silently resurrects a deleted module.
Proven by `key_changes_with_module_add_and_remove` and
`key_changes_with_module_origin`. `blame_path` (diagnostic-rendering only)
and the vendored runtime tree are deliberately EXCLUDED from the key —
neither affects `EmittedProject`'s content (a failed compile is never
cached at all; the runtime tree is copied by `write_emitted_project`
independently of the cache, unchanged from Task 16).

### 12.3 The version epoch — "refuse, don't guess" by construction (Task 20)

`cache::derive_epoch` hashes TWO independent probes, both re-derived fresh
at the start of every invocation (driver-boundary only, never inside a
salsa-tracked path — INV-1 holds: no `std::fs`/subprocess spawn on any
tracked query):

- **`compiler_revision()`** — a content hash of the CURRENTLY RUNNING
  `skyc` binary's own bytes (`std::env::current_exe()` + `sha2`), matching
  the design doc's row verbatim: "content hash seeded from the `ipe`
  binary's own build hash." This is the axis that actually matters most in
  THIS repo's dev loop: `[workspace.package] version = "0.0.0"` never
  bumps, so a version-string-only epoch would have silently reused a stale
  cache across every `cargo build`/`cargo install` of `skyc` itself during
  active development — exactly the under-invalidation this project's
  principles order ranks above any efficiency gain. Hashing the actual
  binary bytes closes that gap unconditionally, with no manual version-bump
  discipline required.
- **`toolchain_fingerprint()`** — a hash of `rustc -vV`'s stdout, the
  design doc's `toolchain_fingerprint()` row.

The epoch is used as a DIRECTORY PREFIX
(`<cache_root>/<epoch>/<key>.json`), never compared as a value after a
lookup. This is what makes "refuse, don't guess" structural rather than a
runtime check, mirroring the design doc's own FFI-cache hazard-ledger
entries (H1/H4: "stale entry has a different address → unreachable miss"):
a `cargo build` of `skyc`, or a `rustup update`, moves every subsequent
build to a DIFFERENT directory. There is nothing to "refuse" at lookup
time — old entries are not merely stale, they are unreachable by
construction; the driver never even looks in the old directory. Either
probe failing (no `current_exe`, no `rustc` on `PATH`) disables the cache
entirely for that invocation (`derive_epoch` returns `None`) — never a
guess, never a build failure, matching the design doc's "Entries are
advisory" framing extended one step earlier (probe-unavailable, not just
entry-corrupt, is advisory too).

**Honestly scoped against the design doc's full Task-20 ask.** The design
doc's toolchain-fingerprint row is written for `ipe watch`'s LIVE SESSION
behaviour: hard-refuse a REBUILD mid-session with `toolchain changed (was
A, now B) — restart 'ipe watch'`, keeping the last-good binary alive. That
UX needs a live watch session to refuse INTO — Phase 7 (unscheduled here).
What Task 20 delivers now, for the one-shot `skyc build` driver that exists
today, is the version-epoch gate ITSELF — the sound foundation Phase 7's
watch-specific UX builds on, not a lesser version of it.

### 12.4 Wiring — the driver-boundary-only cache check

`compile_modules` (`crates/skyc/src/lib.rs`) delegates to a new
`compile_modules_observed`, which computes the cache key + epoch BEFORE any
`sky_db::SkyDatabase` is constructed. On a hit, the WHOLE salsa pipeline is
bypassed entirely — no database, no `SourceRoot`, nothing — only
`write_emitted_project` runs, materialising the cached `EmittedProject`
verbatim. On a miss, the pipeline runs exactly as it always has (unchanged
from Task 17's wiring), and a successful result is best-effort stored for
the next invocation. A cache-WRITE failure (permissions, full disk) never
turns a successful compile into a reported build failure — matching
`write_atomic`'s own established failure-isolation discipline, applied to
an entirely optional, advisory side channel.

**Dependency injection over environment mutation, for testability.** The
cache root is an EXPLICIT `Option<&Path>` parameter on
`compile_modules_observed` (`None` disables the cache), resolved from
`SKY_BUILD_CACHE` / `SKY_BUILD_CACHE_DIR` env vars ONLY at the stable
`compile_modules`/`build`/`build_with_sibling_discovery`/`build_project`
entry points (`cache::env_cache_dir`). This was a deliberate design choice,
not an accident of convenience: `std::env::set_var` is `unsafe` as of the
current standard library signature, and this crate is
`#![forbid(unsafe_code)]`; threading the cache root explicitly instead
sidesteps that entirely, avoids any same-process env-mutation race (moot
under `cargo nextest`'s per-test-process isolation, but avoided anyway),
and gives every test in `crates/skyc/src/cache.rs` and the
`on_disk_cache_hit_serves_a_tampered_entry_verbatim` end-to-end proof a
deterministic, parallel-safe handle with no global state at all.

**Default cache location: colocated with `out_dir`.** `<out_dir>/
.skyc-cache` — chosen so the EXISTING "force a clean rebuild" ritual
(`rm -rf <out_dir>`) also resets the cache, with no new mental model or
second directory to remember to clean. `SKY_BUILD_CACHE_DIR` overrides this
for a shared/global cache location; `SKY_BUILD_CACHE=0` disables the cache
outright.

### 12.5 The end-to-end proof — not just determinism

Two identical builds producing identical output does NOT by itself prove a
cache was consulted (a correct, deterministic compiler would produce the
same bytes twice regardless). `on_disk_cache_hit_serves_a_tampered_entry_
verbatim` (`crates/skyc/src/lib.rs`'s test module) closes that gap
directly: compile once (a genuine miss, populates the cache), locate the
single entry the build just wrote, TAMPER with its `cargo_toml` field with
a sentinel no fresh compile of the SAME source could ever produce, then
compile again with the SAME inputs and cache dir. The second build's
`Cargo.toml` carries the sentinel VERBATIM — proof the driver actually
reads and trusts the on-disk entry, not merely that it recomputed the same
answer twice.

### 12.6 Security: `RelPath`'s `Deserialize` cannot be derived

`sky_backend::RelPath`'s whole reason to exist is that `RelPath::new` is
the ONLY constructor, and it rejects any path that could escape the output
directory (absolute, `..`-bearing, Windows drive-letter-rooted) — the
"parse, don't validate" boundary between in-memory emission and the disk
writer. A NAIVE `#[derive(Deserialize)]` on `RelPath` would reconstruct
`RelPath(raw_string)` directly from untrusted bytes, bypassing that
validation entirely: a poisoned/corrupted cache file could smuggle a
`"../../etc/passwd"` key straight into `EmittedProject::files`, and from
there into `fs::write` on the NEXT process's cache-hit path. `RelPath` gets
hand-written `Serialize`/`Deserialize` impls instead — `Deserialize` routes
through `String::deserialize` then `RelPath::new`, so the SAME validation a
fresh in-process emission gets is enforced on every value that ever crosses
the disk boundary. Proven by `emitted_project_deserialize_rejects_a_
poisoned_key` (`sky_backend`) and `try_load_treats_a_poisoned_relpath_
entry_as_a_miss` (`skyc::cache`) — the second test specifically checks the
CACHE LAYER inherits the rejection via `Result::ok()` rather than
accidentally routing around it (e.g. a hypothetical raw-bytes fallback path
would have reintroduced exactly this hole).

### 12.7 Phase-6 decisions ledger

1. **Cache the `EmittedProject`, not the literal `sky_ir::Program`** — a
   deliberate, recorded substitution for the design doc's literal "lowered
   IR" wording, forced by `Symbol`'s process-local identity (§12.1). Not a
   lesser win for `skyc build`'s actual use case (MORE work is skipped, not
   less); the one thing it cannot serve (an unscheduled future interpreter
   tier) is paid for by nobody today.
2. **Version epoch = compiler-binary content hash + rustc fingerprint, both
   re-derived every invocation, both driver-boundary-only** — no reliance
   on the workspace's own (never-bumped, `"0.0.0"`) version string, which
   would have been a real, silent under-invalidation risk in THIS repo's
   fast-iterating dev loop specifically.
3. **Epoch as a directory prefix, not a post-lookup comparison** — makes
   "refuse, don't guess" structural (an unreachable address) rather than a
   runtime check that could itself have a bug. Mirrors the design doc's own
   FFI-cache addressing scheme (H1/H4).
4. **Advisory, best-effort, driver-boundary-only, dependency-injected** —
   every cache failure mode (missing, corrupt, poisoned, unwritable) is a
   plain miss/no-op, never a build failure; the cache root is an explicit
   parameter, never read from env inside the testable core, avoiding both
   `unsafe` `env::set_var` and any same-process test race.
5. **`RelPath` keeps hand-written `Deserialize`** — the ONE place in this
   phase where `#[derive]` would have been actively unsound, not merely
   less precise; documented at the type itself, not just in this ledger.
6. **Content key excludes `blame_path` and the vendored runtime tree** —
   neither affects `EmittedProject`'s bytes; including them would be
   over-invalidation for zero correctness benefit.

### 12.8 Phase-6 proof tests

| Test | Asserts |
|---|---|
| `skyc::cache::tests key_is_deterministic` | same inputs hash to the same key |
| `key_changes_with_source_text` / `key_changes_with_db_driver` / `key_changes_with_entry_path` | each key ingredient is load-bearing |
| `key_changes_with_module_add_and_remove` | module-identity (not just content) is in the key — the design doc's cache-key-completeness note |
| `key_changes_with_module_origin` | the trust-origin axis is in the key |
| `key_is_delimiter_collision_safe` | `[["AB"],["C"]]` vs `[["A"],["BC"]]` hash differently — no delimiter-join ambiguity |
| `store_and_load_round_trip` | write then read reproduces the value; a different epoch or key sees nothing |
| `try_load_treats_corrupt_entry_as_a_miss` | invalid JSON is a miss, never a panic/propagated error |
| `try_load_treats_a_poisoned_relpath_entry_as_a_miss` | a syntactically-valid-but-unsafe `RelPath` entry is discarded whole, not partially trusted |
| `env_cache_dir_respects_disable_and_override` | the env-var resolution the STABLE entry points use |
| `sky_backend relpath_deserialize_rejects_escaping_paths` / `emitted_project_deserialize_rejects_a_poisoned_key` | §12.6's security property, at the type level |
| `skyc::on_disk_cache_hit_serves_a_tampered_entry_verbatim` | the mission proof — a tampered on-disk entry is served VERBATIM, proving the driver reads and trusts the cache rather than merely reproducing a deterministic answer |
| `skyc::cache_dir_none_disables_caching_entirely` | `cache_dir: None` never creates a cache directory and always reports `Miss` |
| `skyc::clean_vs_incremental_parity` / `adversarial_review_parity_probe` (re-run) | still green — the cache sits entirely outside `compile_prepared`; the parity gate's call shape is untouched by Phase 6 |

---

## 13. Phase 6.5 — symbol-relocation persistence: the literal lowered-IR
## cache, closing §12.1's recorded gap for real

Phase 6.5 revisits §12.1's Phase-6 divergence — "cache `EmittedProject`, not
the literal `sky_ir::Program`, because `Symbol` is process-local" — and
closes it completely rather than leaving it recorded. Headline result:
`skyc build` now has a SECOND, earlier on-disk cache tier keyed on
`sky_db::lower_program`'s own inputs (source + entry, deliberately
EXCLUDING `db_driver`), so a cache hit here skips parse → canon → link →
infer → lower entirely — no `sky_db::SkyDatabase` is even constructed — and
only `RustBackend::emit` runs over the recovered `Program`. This is
STRICTLY more coverage than the Task-19 `EmittedProject` tier: a
`db_driver`-only edit (a `sky.toml [database] driver` flip) MISSES the
`EmittedProject` tier (whose key folds in `db_driver`, correctly — it is a
real emit-stage input) but HITS this tier, because `linked_program` /
`typecheck` / `lower_program` never read `db_driver` at all.

### 13.1 The relocation pass — design and where it lives

The blocker was never "can `sky_ir::Program` be serialized" — every field
is either a plain value or a `sky_intern::Symbol`, and `Symbol` is a raw
`u32` index into ONE process's `sky_intern::Interner`. The blocker was:
`Symbol`'s numeric value is process-local, so a naive
`#[derive(Deserialize)]` reconstructing a `Symbol(raw_id)` directly from
disk would silently corrupt every name in a `Program` loaded by a DIFFERENT
process than the one that wrote it (interning order — and therefore id
assignment — depends on parse order, which is not stable across process
invocations).

**Chosen design: ambient-interner, resolve-to-string persistence**
(`crates/sky_intern/src/lib.rs`), NOT the two-pass symbol-table/index
alternative the mission brief also sketched. `sky_intern::Symbol` gets
hand-written `serde::Serialize`/`Deserialize` impls:

- **`Serialize`** resolves `self` against an AMBIENT interner (installed
  via `SerdeInternerGuard::install(Arc<Mutex<Interner>>)`, a thread-local
  RAII guard) and writes the resolved STRING — never the raw id.
- **`Deserialize`** reads the string, validates it through
  `sky_intern::is_valid_symbol_text` (§13.3), and RE-INTERNS it into the
  SAME ambient interner — the relocation pass. The returned `Symbol`'s
  numeric id is whatever the CURRENT process's interner assigns; it is not
  expected to match the writer's id, only the resolved string.

Every OTHER `sky_ir` type (`Program`, `Module`, `EnumDef`, `Func`, `IrType`,
`Expr`, `Pat`, `Arm`, `Callee`, `BinOp`, `ModPath`, `FuncId`, `BoundSet`,
`UiCtor`, `UiPlain` — every type in `crates/sky_ir/src/ir.rs`'s public
surface except `Match`) gets a PLAIN `#[derive(Serialize, Deserialize)]`,
completely unmodified beyond the derive attribute — because `Symbol`
already does the ambient-context resolution, every containing type's
derived impl "just works" without threading any context itself.
`sky_kernels::StdlibKernel` (embedded via `Callee::Kernel`) and
`HtmlEventShape`/`KernelClass` also derive plainly — every variant is a
bare unit tag, no `Symbol` inside.

**Why ambient (thread-local) context, not `DeserializeSeed`.** `serde`'s
proper context-carrying mechanism is `DeserializeSeed`, but it does not
compose through `#[derive(Deserialize)]`: a derived impl always calls
`T::deserialize()` on each field, never a seed, so threading a seed through
`Vec<Symbol>` / `BTreeMap<Symbol, IrType>` / every enum variant nested
three levels deep would require hand-writing `Deserialize` for roughly
twenty IR types instead of one. Ambient thread-local context (installed
immediately before, uninstalled immediately after, via `Drop`) is the
standard workaround real interned-string systems use for exactly this
shape (the same pattern `string_cache`/`lasso`-style interners use for
serde integration) — genuinely call-scoped (never a true global — two
unrelated builds never share state), introduces zero `unsafe` code (the
crate stays `#![forbid(unsafe_code)]`; the mechanism is a `thread_local!`
`RefCell<Vec<Arc<Mutex<Interner>>>>` STACK, so a nested install/drop
composes safely even though nothing in this compiler nests today), and a
missing guard fails as a descriptive `serde` error (never a panic, never a
silently-wrong `Symbol`) — a programmer-error class of bug, not a
soundness hole reachable from untrusted input.

### 13.2 `Match`'s hand-written impl — the one type that isn't a plain derive

`sky_ir::Match`'s fields are PRIVATE by design: the only way to build one
is `Match::new`/`Match::new_flat`, which prove the arm set is structurally
exhaustive at construction time (`Match` is "illegal states unrepresentable
by construction", per `sky_ir`'s own crate doc). A derived `Deserialize`
would reconstruct `Match { scrutinee, arms }` directly from untrusted
bytes, bypassing that proof — the SAME "parse, don't validate" gap
`sky_backend::RelPath`'s hand-written `Deserialize` closes for path
traversal (Phase 6 §12.6), now closed for `Match` too.

`Match`'s `Deserialize` re-validates through `Match::new_flat` — proven
(not merely asserted) to be a superset of what `Match::new` guarantees:
every arm `Match::new` accepts has `Pat::Ctor` as its literal head (`new`'s
own hard requirement), so `is_ctor_headed` holds for every such arm and
`new_flat`'s `all_ctor_headed` branch always accepts it; a `Match` built
via `new_flat` trivially re-validates through the same pure function. So
every legitimately constructed `Match` in the whole compiler round-trips
unchanged, while an EMPTY arm list or an open-literal cover with no
trailing catch-all is rejected exactly as it would be at original
construction time.

**Honestly scoped gap, verified rather than merely asserted.**
`new_flat`'s `all_ctor_headed` branch does not itself re-verify that the
ctor-headed arms cover EVERY variant of the scrutinee's enum — `Match`
carries no external "complete variant set" of its own (that list lives on
the `EnumDef` elsewhere in the `Program`), and `new_flat` deliberately
trusts the upstream Maranget check for that shape. A tampered entry that
drops ONE arm from an otherwise-exhaustive ctor cover (keeping every
remaining arm ctor-headed) is NOT caught at this boundary. Pinned by its
own regression test
(`deserialize_accepts_single_arm_ctor_headed_match_new_flat_does_not_reverify_full_coverage`)
so a future change to `new_flat`'s semantics is forced to reconsider this
boundary rather than silently regress it. The gap is SAFE, not silent: the
resulting `Program` still cannot reach a `cargo build` success —
`RustBackend::emit` renders the missing arm as a genuine Rust
exhaustiveness gap, and `cargo build` rejects it with E0004 (a loud
failure, never wrong output). Closing it fully would need a second,
whole-`Program` pass cross-checking every `Match` against its scrutinee's
`EnumDef` — recorded as a possible future hardening, not attempted because
the current gap already fails safe.

### 13.3 Security: `is_valid_symbol_text` — the deserialize-boundary gate

`sky_intern::Interner::intern` accepts ANY string by design (a pure
append-only table with no opinion about identifier shapes — every REAL
caller only ever passes lexer-scanned or compiler-synthesised text, so
validating there would be pure overhead on the hot in-process path). But a
persisted cache entry is untrusted input from the moment it is read off
disk, and `sky_backend_rust`'s identifier emission (`resolve_ident` /
`emit_ident` / `naming::mangle_reserved`) trusts an interned string
VERBATIM — no sanitisation of its own, confirmed by reading the emit path
end to end before writing anything. A poisoned `Symbol` string like
`"x; std::process::exit(1); //"` could therefore splice arbitrary Rust
source into the next build's `main.rs`, reached the moment `cargo build`
compiles it — a real RCE-shaped risk via a writable build-cache directory,
not a hypothetical.

`sky_intern::is_valid_symbol_text` closes this: every `Symbol` string is
validated BEFORE interning, against the FULL union of legitimate shapes
surveyed across the whole compiler (a dedicated read-only agent pass,
before writing any code) — one or more ASCII identifier segments
(`[A-Za-z_][A-Za-z0-9_]*`, the exact grammar `sky_parse`'s lexer enforces
for `Tok::Ident`) optionally dot-joined for a qualified path, covering
every non-lexer-scanned shape too (`fresh_symbols`' `<prefix><n>` pools,
`sky_types`' single-letter-plus-digit type-variable mint, the handful of
hardcoded dot-embedding qualifier aliases). Rejection is whole-entry, not
partial: `try_load_ir` returns `None` (a plain cache miss) the moment ANY
embedded `Symbol` fails validation — the same "corrupt entry -> discard,
never partially trusted" contract Phase 6 established for `RelPath`.

### 13.4 Wiring — what a hit skips, and where the tier sits

`crates/skyc/src/cache.rs`'s "Phase 6.5" section adds `compute_ir_key`
(deliberately narrower than `compute_project_key` — no `db_driver`
parameter at all, since `lower_program` never reads it),
`try_load_ir`/`store_ir` (installing a `SerdeInternerGuard` around exactly
one `Program` (de)serialize call each), sharing the SAME version-epoch
(`derive_epoch`) the `EmittedProject` tier uses — deliberately: the IR
format is at least as sensitive to a stale compiler binary as the emitted
text is, and reusing one epoch scheme is simpler and strictly sound
(over-invalidating on a toolchain change costs nothing real).

`compile_modules_observed` (`crates/skyc/src/lib.rs`) tries the
`EmittedProject` tier first (unchanged); on a miss, tries the IR tier
BEFORE constructing any `sky_db::SkyDatabase`:

```
EmittedProject-tier hit  → write output                              (CacheOutcome::Hit)
EmittedProject-tier miss, IR-tier hit
    → fresh Interner + SerdeInternerGuard
    → deserialize Program (relocation pass; poisoned entry ⇒ None)
    → RustBackend::new(&interner).with_db_driver(db_driver).emit(&program)
    → on success: warm the EmittedProject tier too, write output      (CacheOutcome::IrHit)
    → on any failure: fall through (advisory, never a build failure)
EmittedProject-tier miss, IR-tier miss
    → full pipeline (SkyDatabase, parse → … → emit)
    → store EmittedProject (unchanged) AND store the lowered Program
      (sky_db::lower_program is a PURE MEMO HIT here — it already ran
      transitively via emit_project's dependency chain — so this costs
      only the relocation-pass serialize)                             (CacheOutcome::Miss)
```

`sky_db::SharedInterner::as_arc(&self) -> &Arc<Mutex<Interner>>` is the one
new public accessor on `sky_db` this required — exposing the handle the
ambient guard needs, alongside the pre-existing `.lock()` for callers that
just want a guard.

### 13.5 Cross-process id-drift correctness — the proof, three layers deep

A same-process round trip cannot distinguish "the relocation pass
correctly re-interns by string" from "the id happened to survive by
coincidence" (a fresh interner given the exact same `intern` call sequence
trivially reproduces the same ids either way). Every layer of this design
therefore has a test that DELIBERATELY diverges the writer's and reader's
interner histories (different noise strings, different counts, different
orders) before comparing — so a raw-id relocation bug WOULD manifest as a
wrong resolved name, not just a coincidentally-matching one:

1. **`sky_intern::serde_persistence_tests::
   serialize_then_deserialize_survives_cross_process_id_drift`** — a single
   `Symbol` ("Increment") interned into a noise-polluted writer interner,
   serialized, then deserialized into a differently-noise-polluted reader
   interner; asserts the raw ids DIFFER (proving drift is real) yet
   `reader_interner.resolve(reader_symbol) == Some("Increment")` (proving
   semantic identity survives regardless).
2. **`sky_ir::ir::serde_persistence_tests::
   program_survives_cross_process_symbol_id_drift`** — the same property
   one layer up, over a whole hand-built `Program` (enum + `Match` +
   record literal — every `Symbol`-carrying IR shape in one value).
   Compares THREE independently-noise-polluted interners (writer / reader /
   a "ground truth" that never touches serialization at all) via
   `sky_ir::pretty::pretty` — a pure, total, name-resolving renderer that
   already existed for the `--emit-ir` developer flag — rather than raw
   `Program == Program` equality, because two independently-relocated
   `Program`s are not expected to share numeric ids, only meaning.
3. **`skyc::cache::tests::ir_cache_hit_survives_cross_process_symbol_id_drift`**
   — the same three-interner/`pretty`-comparison proof at the ON-DISK cache
   boundary (`store_ir` then `try_load_ir` through unrelated interners).
4. **`skyc::tests::on_disk_ir_cache_hit_serves_a_tampered_entry_verbatim`**
   — the END-TO-END mission proof, mirroring Phase 6's own
   `on_disk_cache_hit_serves_a_tampered_entry_verbatim` one tier earlier:
   compile once through the REAL `compile_modules_observed` driver
   (populates both tiers), tamper the on-disk IR entry's literal body
   (`Expr::Int(1)` → `Expr::Int(42)`, a value no fresh compile of the same
   source could ever produce), force an `EmittedProject`-tier miss (a
   `db_driver` flip) so the IR-tier fast path is the one actually
   exercised, and assert the SENTINEL reaches the materialised `main.rs` —
   proof the driver reads, relocates, AND RE-EMITS the on-disk entry,
   never silently recompiling or discarding the tamper.
5. **`skyc::tests::ir_cache_hit_reuses_lowered_program_across_a_db_driver_only_edit`**
   — the coverage proof: a `db_driver`-only edit against a warm cache
   MISSES the `EmittedProject` tier but HITS the IR tier
   (`CacheOutcome::IrHit`), the concrete case this tier exists to cover
   that Task 19's tier structurally cannot.

### 13.6 Phase-6.5 decisions ledger

1. **Ambient (thread-local) interner context over a two-pass symbol
   table/index scheme** — dramatically less code (every `sky_ir` type
   gets a ONE-LINE derive-attribute addition, no shadow types, no
   hand-written conversion functions to keep in sync as `sky_ir` grows),
   equally sound (both approaches ultimately resolve-then-re-intern by
   string), and self-updating (a new `Symbol`-carrying field on an
   existing type, or a wholly new derive-annotated type, needs NO
   persistence-layer change at all — only `Match`, the one type with a
   private-field construction invariant, needed a hand-written impl).
2. **`Match` re-validates through `new_flat`, not a bespoke exhaustiveness
   re-derivation** — reuses the EXISTING structural backstop (one code
   path, provably a superset of what legitimate `Match` values satisfy)
   rather than inventing a second exhaustiveness checker that could drift
   from the first. The resulting gap (single-arm-drop from a ctor-complete
   cover) is recorded honestly with its own pinning regression test rather
   than silently claimed away.
3. **IR-tier key excludes `db_driver` entirely** — not merely "the same key
   minus one field": `compute_ir_key` has no `db_driver` PARAMETER at all,
   making the exclusion a compile-time fact rather than a runtime
   convention that could silently drift back in.
4. **Same version epoch for both tiers** — one hash scheme, one directory
   prefix, reused rather than re-derived; over-invalidating the IR tier on
   an (unlikely, but real) case where only the emit stage's Rust codegen
   changed but the IR format didn't is accepted as strictly sound (costs a
   cache miss, never a wrong answer).
5. **`is_valid_symbol_text` lives in `sky_intern`, checked at deserialize
   time only** — `Interner::intern` itself stays permissive (a pure
   append-only table, zero validation overhead on the hot in-process
   path); the persistence boundary is the ONLY place untrusted text can
   enter the symbol table, so that is the only place the check needs to
   run.
6. **Advisory, best-effort, fail-open on emit failure** — a relocated
   `Program` that (for any reason) fails to re-emit falls through to the
   full pipeline rather than reporting a build failure, matching every
   other cache-tier failure mode in this design (missing, corrupt,
   poisoned, unwritable) established since Phase 6.

### 13.7 Phase-6.5 proof tests

| Test | Asserts |
|---|---|
| `sky_intern::serde_persistence_tests valid_symbol_text_accepts_every_real_shape` / `_rejects_every_poisoned_shape` | the deserialize-boundary grammar, both directions |
| `serialize_then_deserialize_survives_cross_process_id_drift` | §13.5 layer 1 — the `Symbol`-level mission proof |
| `serialize_without_ambient_interner_fails_closed` / `deserialize_without_ambient_interner_fails_closed` | a missing guard is a `serde` error, never a panic |
| `deserialize_rejects_poisoned_symbol_text` / `deserialize_rejects_forged_control_character_payloads` | injection-shaped and control-character payloads rejected, never interned |
| `nested_guards_restore_the_outer_interner_on_drop` | the ambient stack composes safely under nesting |
| `sky_ir::ir::serde_persistence_tests round_trips_within_one_interner` | same-interner round trip preserves both ids and strings |
| `program_survives_cross_process_symbol_id_drift` | §13.5 layer 2 — the whole-`Program` mission proof |
| `deserialize_rejects_unknown_kernel_tag` | a forged `StdlibKernel` tag is rejected, never silently coerced |
| `deserialize_rejects_emptied_tampered_match` | `Match`'s hand-written `Deserialize` actually revalidates |
| `deserialize_accepts_single_arm_ctor_headed_match_new_flat_does_not_reverify_full_coverage` | §13.2's honestly-scoped gap, pinned as a regression test |
| `skyc::cache::tests compute_ir_key_is_deterministic_and_excludes_db_driver` / `compute_ir_key_changes_with_source_text` | the IR-tier key's own ingredients |
| `ir_store_and_load_round_trip_within_one_interner` | the on-disk IR tier's basic round trip |
| `ir_cache_hit_survives_cross_process_symbol_id_drift` | §13.5 layer 3 |
| `ir_try_load_treats_corrupt_entry_as_a_miss` / `ir_try_load_treats_a_poisoned_symbol_entry_as_a_miss` | corrupt JSON and a poisoned `Symbol` string are both plain misses |
| `ir_env_extension_does_not_collide_with_emitted_project_tier` | the two tiers' on-disk filenames cannot alias each other |
| `skyc::tests on_disk_ir_cache_hit_serves_a_tampered_entry_verbatim` | §13.5 layer 4 — the END-TO-END mission proof, through the real driver |
| `ir_cache_hit_reuses_lowered_program_across_a_db_driver_only_edit` | §13.5 layer 5 — the coverage proof (`CacheOutcome::IrHit` on a driver-only edit) |
| `skyc::tests` (full suite, re-run) / `skyc::cache::tests` (full suite, re-run) | still green — zero regressions on every pre-existing Phase 1–6 test |

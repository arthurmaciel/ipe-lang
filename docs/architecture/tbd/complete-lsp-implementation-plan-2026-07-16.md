# The complete Ipê LSP — implementation plan

Status: design + implementation plan (no code in this change). This is the
ONE authoritative plan for the full language server; it consolidates and
supersedes the build-order sections of `ipe-lsp.md` and
`docs/superpowers/plans/2026-07-03-lsp.md` **where they conflict with the
salsa layer that has since landed** (see §2 and the decision ledger §12).
Everything those documents lock that is not superseded here — the G1–G5
guarantees, the hazard ledger L-A…L-N, the `VerifiedEdit` gate, the
type-directed-completion query contract — remains authoritative and is
referenced, not restated in full.

Companion designs integrated as LSP surfaces:
`ipe-lint-tool-design-2026-07-16.md` (lint diagnostics + quick-fixes),
`exhaustive-case-finite-adt-design-2026-07-16.md` (IPE-T0018 + the
expand/add-arms/keep-open code actions),
`salsa-incremental-compilation-2026-07-11.md` (the query layer, esp. §15's
LSP seam) and `incremental-compilation-and-watch.md` (INV-1..5, H-ledger).

Layout (post-rename): compiler crates live at `src/compiler/<name>` as
`ipe_<name>` (`ipe_db` = `src/compiler/db`, `ipe_diagnostics` =
`src/compiler/diagnostics`, …); the driver crate `ipe` lives at
`src/ipe-cli`. The LSP crates live at `src/lsp/{server,features,edits}` as
`ipe_lsp_server` / `ipe_lsp_features` / `ipe_lsp_edits`; the subcommand is
`ipe lsp`.

---

## 1. Mission — why the LSP is a headline feature

The LSP is the language's soundness story made visible at every keystroke:

1. **One analyzer, literally.** The server owns no parser, no resolver, no
   solver, no formatter. Every diagnostic, hover type, completion candidate,
   rename edit, and folding range is computed by the *same* `ipe_db` salsa
   queries `ipe build` and `ipe watch` run. Disagreement with the compiler
   is not "unlikely" — it has no code path.
2. **A build-breaking edit is unrepresentable.** Every synthesized
   `WorkspaceEdit` (code action, quick-fix, rename, auto-import, file
   rename) exists only as the payload of an `Ok(VerifiedEdit)` whose sole
   constructor re-checks the edit's full blast radius through
   parse→canon→typecheck(+exhaustiveness)→fmt. THE SEAL, applied to the
   editor: an action the server offers cannot break the build.
3. **Never stale, by construction.** A `didChange` bumps the salsa
   revision; every in-flight read unwinds via `Cancelled` and returns
   `ContentModified`; the client re-requests against the new state. A CI
   guard asserts every warm response is byte-identical to a cold-database
   response — the LSP inherits the clean-vs-incremental parity gate.
4. **Position-exact, home-attributed diagnostics.** The compiler's
   diagnostics and region types carry their home module
   (`infer_attributed` → `(Diagnostic, home)`;
   `SolvedTypes.regions: BTreeMap<(Vec<Symbol>, Span), Ty>`), so a
   whole-program-linked result maps to the exact file and byte range —
   never a heuristic file guess.
5. **The compiler as kind teacher, in the editor.** Every diagnostic links
   its `explain` page (`codeDescription`); hover carries the inferred type
   plus the ELI10 lead of the relevant explain/kernel doc; quick-fixes
   inherit the compiler's own `Applicability` confidence.

Principles order (hard): security > correctness > soundness > efficiency >
completeness > readability. For the LSP the sharpest edges are
**correctness > efficiency** (a fast answer that disagrees with `ipe`, or
a stale hover, is a defect no latency win excuses) and **completeness
gated by soundness** (a capability we cannot make sound is not advertised
— refuse, don't fake; §11 lists these explicitly).

---

## 2. Substrate survey — what exists today (facts, verified against code)

**The salsa layer is landed and gate-proven.** `src/compiler/db` ships, on
the production `ipe` path:

| Tier | Queries (all tracked, memoized, errors-as-values) |
|---|---|
| Inputs | `SourceFile { module_path, text, origin }`, `SourceRoot { files }`, `BuildConfig`; driver helpers `set_text_if_changed` (byte-equal no-op) + `sync_source_root` (file-set reconciliation) |
| Front | `parse`, `imports` (topo scan), `resolve_imports` (closed enum `Resolved | Unresolved`; `Ambiguous` unrepresentable by input shape), `canonicalize`, `module_interface` (backdating firewall: body-only edits don't re-canon importers), `identifier_words` |
| Spine | `topo_order` (cycle = IPE-N0021 as a value), `linked_program`, `kernel_types`, `typecheck`, `lower_program`, `program_metadata` |
| Emit | `emit_project`, `program_rust_file_ids`, `emit_spine_file`, `emit_rust_file`, `emit_manifest` |

Load-bearing properties the LSP builds on, each already proven by test:

- **Editor-buffer inputs work with zero `ipe_db` changes** — `SourceFile`
  holds `text: String`, not a path; `src/compiler/db/tests/lsp_seam.rs`
  drives diagnostics + navigation from in-memory buffers with no
  filesystem anywhere in the test.
- **`IpeDatabase` is `Send`** (compile-time-asserted in `lsp_seam.rs`) and
  is already cloned into worker threads by `ipe watch` — the
  single-writer + snapshot-reader loop pattern is proven.
- **Cancellation is a database property, not a driver hack** — a direct
  `typecheck` demand on a worker thread unwinds
  `Cancelled::{Local,PendingWrite}` when the main thread sets an input,
  and a fresh demand converges to the edited state (`lsp_seam.rs`).
- **Warm == cold** — the clean-vs-incremental parity gate
  (`src/ipe-cli/tests/clean_vs_incremental_parity.rs`) byte-diffs
  warm-database rebuilds against cold builds across the golden corpus,
  including adversarial edit sequences.
- **`typecheck` is whole-program-coarse.** It is keyed `(root, entry)` and
  depends on `linked_program`; any semantic edit re-executes the whole
  solve (memoized only across byte-equal/no-op revisions). Per-module
  `typecheck(ModuleId)` is a recorded `ipe_types` redesign
  (salsa doc §9.4) that this plan treats as a **latency unlock, not a
  correctness dependency** — handlers consume the query by name, so the
  granularity refinement lands later with zero handler changes.

**Diagnostics infrastructure.** `ipe_diagnostics` has stable `IPE-*`
codes; `explain_page(Code)`/`title(Code)` with a drift test that every
code has a conforming page; `HelpLine::Suggest(Suggestion { span,
replacement, applicability })` with `Applicability
{ MachineApplicable | HasPlaceholders | MaybeIncorrect }`; `ipe fix`
already applies suggestions. `ipe_types::infer_attributed` returns
`(Diagnostic, home)` and `SolvedTypes` exposes `env` (type per binding),
`regions` (type per sub-expression span, home-keyed), `bounds` — hover's
data source exists today; the driver's `home_to_source` map is the exact
(home → file) resolution the LSP reuses.

**What does NOT exist** (each shapes the plan):

- **No `lsp` subcommand, no LSP crates.** `ipe` dispatches
  `build`/`run`/`watch`/`explain`/`fix` only.
- **No formatter.** There is no `fmt` subcommand and no Format crate in
  this workspace (the reference's `sky fmt` is Haskell-side, not ported).
  LSP formatting therefore *waits for the formatter port* — never an
  LSP-side pretty-printer (hazard L-K). This also weakens the
  `VerifiedEdit` "fmt-clean" clause until the port lands: the gate runs
  parse→canon→typecheck now and adds the fmt stage the day the one
  formatter exists (§11 flag F-1).
- **No `expected_type_at` sidecar.** `SolvedTypes.regions` gives inferred
  types; the *expected* type pushed down onto an incomplete hole is
  net-new solver work (the type-directed-completion foundation).
- **No lint crate yet** — `ipe_lint` is a companion design; the LSP wires
  it when it lands (its findings are `Diagnostic`s, so no new LSP
  machinery).

**The reference server** (`../sky/src/Sky/Lsp/{Server,Index,Diag}.hs`,
~4.7 kLoC) implements: incremental sync, publishDiagnostics, hover,
definition/declaration, documentSymbol, completion (prefix + context),
formatting, references, rename + prepareRename, signatureHelp, codeAction
(unused imports, add annotation), semanticTokens/full, inlayHint — over a
whole-project rebuild-on-save index with catch-all walker arms and no
verification gate on edits. §10 states where we match and where we exceed.

---

## 3. Architecture

### 3.1 Crates and loop

```
                        ┌──────────────────────────────────────────────┐
 ipe_parse ─┐           │ ipe_lsp_server  (NEW)                        │
 ipe_canon ─┤           │  main loop (lsp-server, sync, single writer) │
 ipe_types ─┼─ ipe_db ◄─┤  VFS (ropey) + position mapper + watched     │
 ipe_lower ─┤  (salsa)  │  files; capability negotiation; scheduling;  │
 ipe_ir   ──┘   ▲       │  catch_unwind + latency budget + Cancelled   │
                │       └──────────────┬───────────────────────────────┘
            ipe watch                  │ snapshot() reads on worker pool
        (sibling consumer)   ┌─────────┴──────────┐
                             │ ipe_lsp_features   │  pure handlers:
                             │ (NEW)              │  (snapshot, pos) → payload
                             └─────────┬──────────┘
                             ┌─────────┴──────────┐
                             │ ipe_lsp_edits (NEW)│  VerifiedEdit gate, code
                             │                    │  actions, rename, auto-
                             └────────────────────┘  import, TEA scaffolds
```

- **Framework: `lsp-server` + `lsp-types` (pinned).** Synchronous
  single-writer main loop owns the one `IpeDatabase`; reads dispatch to a
  small worker pool over cloned handles. Locked in `ipe-lsp.md` OPEN-3;
  salsa's synchronous `Cancelled`-on-write cancellation composes with a
  sync loop (the reason rust-analyzer declined `tower-lsp`).
- **`ipe_lsp_features` handlers are pure functions** from
  `(analysis snapshot, position) → LSP payload` with no
  `std::fs`/`std::env`/`std::io` capability (INV-1) and no
  parse/resolve/infer logic of their own (G1). Enforcement: dependency
  review + the CI grep for solver/parser calls.
- **`ipe_lsp_edits`** owns the `VerifiedEdit` type and every
  `WorkspaceEdit` producer. It is the only crate that can construct a
  surfaced edit, and it can only do so through the gate.

### 3.2 Decision — build directly on `ipe_db`; retire the `BatchView` backend

`ipe-lsp.md` Q1 and the earlier plan mandate a pre-salsa `BatchView`
backend behind a `ProgramView` trait because the LSP was designed before
the query layer existed and must not block on it. That precondition is
gone: `ipe_db` is on the production path, parity-gate-proven, with the LSP
access pattern (buffer inputs, `Send` database, cancellation on any
demand) already integration-tested. A batch backend today would be a
second driver to keep in lockstep — pure maintenance surface with no
consumer.

**Locked here:** the LSP's one production backend is `ipe_db`. The
`ProgramView` trait **survives with a narrower job**: it remains the seam
handlers are written against, for (a) testability (the feature-test
harness constructs a view from fixture maps), (b) the future per-module
`typecheck` refinement (key-shape change absorbed in one impl), and
(c) keeping handlers structurally incapable of reaching around the query
layer. There is exactly one production impl. The earlier plan's
verify-on-apply/v0 compromises (OPEN-2) dissolve: **verify-on-offer is the
only mode** — an unsound action is never shown, at any phase.

### 3.3 Document sync, VFS, positions

- **Sync kind `Incremental`.** Open buffers in a `ropey` rope; range edits
  O(log n); `didClose` reverts the input to disk bytes. The VFS overlay is
  the LSP↔watch reconciliation point (hazard L-G): both feed the same
  salsa inputs, the LSP's unsaved buffers simply shadow disk while open.
- **One position mapper, property-tested first.** LSP positions are UTF-16
  code units by default; compiler `Span`s are bytes. A single
  `offset` module owns every conversion (hazard L-I), property-tested over
  astral-plane emoji/CJK/combining marks/CRLF (round-trip identity, 1e4+
  cases). **Position-encoding negotiation (LSP 3.17):** the server
  advertises `positionEncoding` `utf-8` preference and accepts `utf-16`;
  when a client (Neovim, Helix) negotiates `utf-8` the conversion becomes
  the identity — one code path less to get wrong, mandatory `utf-16`
  support kept.
- **Watched files.** Register `**/*.ipe` + `sky.toml` via
  `workspace/didChangeWatchedFiles`; on-disk changes to non-open files
  reconcile through `sync_source_root` (module add/delete/rename already
  proven by the resolve-imports tests).

### 3.4 Scheduling, cancellation, resilience

- **Single writer**: only the main loop mutates the database
  (`didOpen`/`didChange`/`didClose`/watched-file events, in receipt
  order). Byte-equal re-sets are input-boundary no-ops
  (`set_text_if_changed`).
- **Reads**: each request runs on a worker with a cloned handle; a write
  cancels in-flight reads (salsa `Cancelled` unwind) → handler returns
  `ContentModified (-32801)`; the client re-requests. `$/cancelRequest`
  cooperatively drops the pull. A stale result cannot be delivered.
- **Debounce**: push diagnostics on ~100 ms quiescence; pull diagnostics
  and hover/completion are demand-driven but cancellable.
- **Resilience (G3), three layers**: workspace `deny(unwrap_used, panic,
  indexing_slicing, unreachable, pedantic, nursery)` on all three crates;
  total handler paths (`Option`/`Result`, positions clamped to buffer
  bounds); handler-boundary `catch_unwind` distinguishing `Cancelled`
  (→ `ContentModified`) from a real panic (→ internal error for that one
  request + a `CompilerBug` log line — never a dead server). Fuzz gate:
  random/truncated/mutated buffers through every handler — no panic,
  bounded time.
- **Latency budget**: ~3 s per request ceiling → friendly refusal, never a
  hang.
- **Parser resilience is a stated precondition, not an assumption**
  (`ipe-lsp.md` G3 layer 1): the no-crash guarantee holds unconditionally
  via layers 2–3; the *quality* of partial results (hover next to a syntax
  error, completion in a broken buffer) tracks `ipe_parse`'s
  error-recovery quality. A fuzz gate on `ipe_parse` (no panic, bounded
  time, best-effort tree) is part of Phase 0; deeper recovery
  (typed error nodes) is an incremental follow-up that improves results
  without changing any handler contract.

---

## 4. The complete capability matrix

Every row names its single source of truth (the query it reads), its
soundness stance, and its phase. "R" = the reference Haskell server has
it; bold = we exceed the reference on that row.

| # | Capability | Source (one analyzer) | Soundness stance | Phase | R |
|---|---|---|---|---|---|
| 1 | Lifecycle: initialize, capability negotiation, shutdown; **positionEncoding utf-8/utf-16** | — | negotiated once, parsed at the boundary | 0 | R |
| 2 | Incremental sync + VFS + watched files | inputs | byte-equal no-op; single writer | 0 | R |
| 3 | **Diagnostics, push** (publishDiagnostics) | `parse` ∪ `canonicalize` ∪ `typecheck` verbatim, home-attributed | compiler values verbatim (G1); exact file+range | 0 | R |
| 4 | **Diagnostics, pull** (textDocument/diagnostic + workspace/diagnostic, resultId/unchanged) | same | resultId == salsa revision fingerprint → `unchanged` is honest, never stale | 2 | — |
| 5 | Hover: inferred type + teacher snippet | `typecheck.regions[(home, span)]`; `kernel_types`; `explain_page` | total; `None` on error nodes | 1 | R |
| 6 | Go-to-definition / declaration | `canonicalize` binding sites + `resolve_imports` | resolved names only; unresolved → null | 1 | R |
| 7 | **Go-to-type-definition** | `typecheck.regions` → the solved `Ty`'s head decl site | nominal heads only (ADT/alias); structural types → null | 2 | — |
| 8 | Document symbols (hierarchical) | `parse` top-level decls | pure over parse tree | 1 | R |
| 9 | **Workspace symbols** (+resolve) | derived symbol-index query (never a hand-kept store, L-D) | index IS a query | 2 | — |
| 10 | Find references | `collect_references` walker (exhaustive match, no wildcard — G4) | same walker rename uses (L-F) | 2 | R |
| 11 | **Document highlight** | `collect_references` scoped to one file, read/write kinds | same walker again | 2 | — |
| 12 | Completion: scope-aware (+resolve) | `scope_at` (locals, module, imports, ctors, record fields) | derived from canon; no private index | 1 | R |
| 13 | **Completion: type-directed** (the DX headline) | `expected_type_at` + `unifiable_candidates` (scratch-arena unification) | expected-`Int` never offers a `String`; arena isolation property-tested | 3 | — |
| 14 | Signature help | `typecheck` callee scheme + active param | total; null outside calls | 2 | R |
| 15 | Semantic tokens **full + delta + range** | exhaustive AST walker (G4: no wildcard arm → new AST variant = compile error) | golden snapshot over the corpus | 1 | R (full only) |
| 16 | Inlay hints (+resolve): inferred `let`/param types | `typecheck.regions` | display of solved types only | 2 | R |
| 17 | **Folding ranges** | `parse` tree (decls, `let…in`, `case` arms, records, triple-strings, comment blocks) | pure over parse | 1 | — |
| 18 | **Selection range** (expand selection) | `parse` AST ancestor chain at pos | pure over parse | 1 | — |
| 19 | **Document links** (`import Foo.Bar` → file) | `resolve_imports` | resolved edges only | 1 | — |
| 20 | **Call hierarchy** (prepare/incoming/outgoing) | `collect_references` + the call edges `program_metadata` already walks | exhaustive IR walkers exist | 4 | — |
| 21 | Code actions (+resolve): compiler-fix surfacing | `Suggestion`/`Applicability` on diagnostics | compiler's own confidence model; `MachineApplicable` → preferred fix | 2 | R (2 fixes) |
| 22 | **Code actions: IPE-T0018 family** — expand catch-all (MachineApplicable), add missing arms from witnesses (HasPlaceholders), keep-open directive | witness machinery from `ipe_types::exhaust` (companion design §7) | message and fix share one witness list — cannot disagree | 4 | — |
| 23 | **Code actions: lint quick-fixes + @allow suppression action** | `ipe_lint` findings (Diagnostic::Lint) when it lands | lint = third consumer of compiler artifacts; fixes gated | 4 | — |
| 24 | **TEA scaffolding**: snippet catalog + program-reading actions (add Msg variant + arm; add subscription; scaffold app; convert to worker) | `canonicalize`/`typecheck` + structured AST insertion | every action through the `VerifiedEdit` gate; snippets golden-tested at their honest bar (L-M) | 5 | — |
| 25 | **Rename** (prepareRename + project-wide) | `collect_references` + `VerifiedEdit` over full blast radius | refuses kernel/FFI/reserved targets; post-edit program typechecks or no edit exists | 4 | R (ungated) |
| 26 | **Auto-import** (completion `additionalTextEdits` + unresolved-name quick-fix) | canonicaliser export index (a query) | ambiguity → disambiguation list, never a silent pick (L-E); gated | 5 | — |
| 27 | **workspace/willRenameFiles** — renaming `Foo.ipe` rewrites `module` header + every importer, atomically | `resolve_imports` reverse edges + `VerifiedEdit` | the same gate; a file rename cannot orphan importers | 5 | — |
| 28 | Formatting (full doc; range later) | THE formatter crate (`ipe fmt` port) | delegated + idempotence-asserted; **gated on the port** (§11 F-1) | 6 | R |
| 29 | Code lens: reference counts (opt-in, default off) | symbol-index query | display-only; §11 F-4 for anything that runs code | 6 | — |
| 30 | `ipe lsp` subcommand + editor onboarding docs | — | — | 0 | R |

**Deliberately not advertised** (refuse, don't fake — §11): `implementation`
and `typeHierarchy` (no sound mapping in an HM language without
interfaces/subtyping; `definition` + `typeDefinition` cover the real user
intent), `onTypeFormatting` (needs the formatter + is layout-fragile),
`linkedEditingRange` (rename already provides the sound version of the
same intent; a heuristic same-name live edit can silently capture).

---

## 5. Soundness architecture — the guarantees, restated as enforcement

Inherited from `ipe-lsp.md` G1–G5 verbatim; this section states only what
each means mechanically in this plan.

**G1 — one type-checker, structurally.** `ipe_lsp_features` depends on
`ipe_db` + `ipe_diagnostics` + the AST/type crates *for types only*; it
contains no call into `parse_module`/`canonicalise_*`/`infer*`. CI check:
a grep test over the crate for solver/parser entry points (mirrors the
INV-1 grep in `ipe_db`). Diagnostics published to the editor are the
compiler's `Diagnostic` values verbatim — code, severity, span, help
lines, suggestion — plus the `codeDescription` link derived from
`explain_page`.

**G2 — `VerifiedEdit`.** In `ipe_lsp_edits`:

```rust
/// A WorkspaceEdit that is proven not to break the build. The ONLY
/// constructor re-checks the edit's full blast radius — every touched
/// file PLUS every importer of any module whose `module_interface`
/// changes — through parse → canonicalize → typecheck (and, once the
/// formatter exists, fmt-idempotence). There is no other way to obtain
/// a surfaced WorkspaceEdit.
pub struct VerifiedEdit(WorkspaceEdit);
```

- The overlay recompute runs on a scratch `SourceRoot` (a second salsa
  input set in the same database — cheap, memo-shared with the live
  root where content is byte-equal).
- Blast radius: body-only edits collapse to the edited module (the
  `module_interface` backdating firewall makes the importer set empty);
  interface-moving edits re-check importers via the `resolve_imports`
  reverse edge. This is exactly the diagnostics-refresh edge, reused.
- **Verify-on-offer, always** (§3.2): an action that fails the gate is
  never surfaced. Compiler-sourced suggestions keep their
  `Applicability` (`MachineApplicable` → preferred quick-fix;
  `HasPlaceholders` → snippet edit; `MaybeIncorrect` → non-preferred) —
  the compiler's confidence model is not re-decided.
- Debug builds panic on a gate failure inside an action generator (a test
  tripwire); release returns `Err` and hides the action.

**G3 — no crash on partial buffers.** §3.4's three layers + the fuzz gate
as a required CI job. The workspace clippy deny-set applies to all three
LSP crates from their first commit.

**G4 — exhaustive walkers.** `sem_tokens`, `collect_references`,
`fold_ranges`, `selection_chain`, `doc_symbols` match AST/IR enums with
**no wildcard arm** (`#![deny(clippy::wildcard_enum_match_arm)]` on the
walker modules + CI grep + golden snapshots over the example corpus). A
new AST variant is a compile error in every walker until it gets an arm.

**G5 — security posture.** stdio JSON-RPC only, no network channel; the
server executes no project code (parse/canon/type are pure; FFI enters
only as a reserved input on the `ipe add` path — an LSP-time cache miss
hard-refuses); structured edits are built from typed AST insertion, never
string concatenation of program-derived data; `ipe_lsp_server` is the
only crate holding I/O capability.

**The never-stale guard (CI-required).** A scripted edit sequence against
a warm LSP database asserts, after each edit, that every
hover/completion/diagnostics payload equals the payload from a **fresh
database** built from the current buffer state — including the
adversarial edits the incremental parity gate uses (body-only, signature
flip, module add/delete/rename/shadow). A divergence fails the build.

**Position accuracy.** Every span→range crossing goes through the one
property-tested mapper; every home→file crossing goes through the
`home_to_source` resolution (exact map lookup, never the fuzzy fallback,
for any diagnostic whose home is non-empty). Regression test: a
multi-module fixture with identical body text in two modules must
attribute each diagnostic to its own file.

---

## 6. Feature notes beyond the matrix (only where design is non-obvious)

**Pull diagnostics (row 4).** LSP 3.17's pull model maps onto salsa
perfectly: the `resultId` is a fingerprint of the salsa revision + file
set; a `textDocument/diagnostic` request against an unchanged revision
answers `unchanged` from the memo without recomputation — the protocol's
staleness contract implemented by the database's own invalidation, not by
a second cache. Push stays on (default for clients that don't pull); both
read the same query.

**Type-directed completion (row 13).** The query contract is locked in
`docs/superpowers/plans/2026-07-03-lsp.md` (queries A/B/C:
`expected_type_at`, `scope_at`, `unifiable_candidates`) and is not
restated; the three load-bearing points: (1) the `ExpectedTypes` sidecar
is net-new `ipe_types` work and must be **additive** — a property test
asserts recording it leaves `SolvedTypes` and every diagnostic unchanged;
(2) speculative unification runs on a scratch arena that is never written
back — an isolation property test snapshots `regions` before/after; (3)
classification is the closed enum `ExactType | Unifiable | InScopeOnly`
with a deterministic total order encoded into `sortText`. When an expected
type exists, non-unifying candidates are dropped — an expected-`Int` slot
never offers a `String`.

**Folding/selection (rows 17–18).** Pure functions over the parse tree —
they ship early precisely because they carry zero soundness risk and round
out the "complete server" feel. Triple-quoted strings fold as one region;
`case` folds per arm; the selection-range chain is the AST ancestor path
(token → pattern/expr → arm → `case` → def → module).

**Call hierarchy (row 20).** Outgoing edges = the call/func-value walk
`program_metadata` already implements over `ipe_ir` (exhaustive over all
`Expr`/`Pat` variants); incoming = the same edge set inverted. Surfaced at
def granularity. This makes the reachability analysis the compiler already
computes visible in the editor — a small feature with outsized "the
compiler is alive" effect.

**IPE-T0018 + lint actions (rows 22–23).** The companion designs already
specify the actions and pin their inputs to the compiler's witness
machinery (message and fix cannot disagree). The LSP work is surfacing
only: findings arrive as `Diagnostic`s on the one channel; fixes arrive as
`Suggestion`s through the one gate. The `@allow` suppression action
inserts the directive with a mandatory-reason tabstop
(`HasPlaceholders` — never auto-applied).

**Rename (row 25).** `prepareRename` refuses: kernels, FFI names, stdlib
bindings (origin `EmbeddedStdlib`), and any target whose new name
collides with reserved-name rewriting in the backend. The edit is built
from the same `collect_references` walker find-refs uses and passes the
gate over the full blast radius. Module rename (row 27) composes the same
machinery: rewrite the `module` header + every `import`/qualifier, gate
the closure, return it from `willRenameFiles` so the client applies edit
and file move atomically.

**Semantic tokens (row 15).** Token legend: namespace/module, type,
enumMember (ctors), function, parameter, variable, property (record
fields), keyword, string, number, comment, operator, decorator (directive
comments); modifiers: declaration, readonly (everything — pure language),
defaultLibrary (stdlib/kernels). Delta encoding via the standard
previous-result diff; range requests slice the full computation (memoized,
so cheap).

---

## 7. Performance posture — honest numbers, structural fixes

The correctness story never depends on speed (cancellation forbids stale
delivery at any latency). The latency story today:

| Operation | Cost today | Why | Unlock |
|---|---|---|---|
| Parse/canon per keystroke-settle | per-module, firewalled | `canonicalize` + `module_interface` backdating | already good |
| Diagnostics after a body edit | whole-program re-solve (memoized across no-ops) | `typecheck(root, entry)` is coarse | per-module `typecheck` (salsa doc §9.4) — a `ipe_types` redesign, tracked separately; zero LSP handler changes when it lands |
| Hover/inlay/sighelp warm | sub-ms | memo read of `typecheck.regions` | — |
| Completion (typed) | first: one scratch-unify pass over K candidates; then memo | pure query per (module, revision) | — |
| VerifiedEdit gate | one overlay re-check of the blast radius | firewall collapses body-only edits to 1 module | same per-module unlock shrinks the interface-moving case |

Interim posture while `typecheck` is coarse: debounce (~100 ms
quiescence) + cancellation keep the editor responsive under continuous
typing (each keystroke cancels the previous solve — p99 is bounded by the
debounce window, not the solve); on a mid-size project the settled-edit
diagnostics latency equals one whole-program solve. That is the same cost
`ipe watch` pays per rebuild today and is acceptable for v1; it is stated
in the docs rather than papered over. The per-module refinement is the
single highest-leverage follow-up and benefits `ipe`/`watch`/LSP alike.

Targets once per-module typecheck lands: warm hover p50 < 5 ms; settled
body-edit diagnostics < 100 ms; completion first request < 100 ms, warm
< 30 ms.

---

## 8. Phased implementation plan

Each phase lands green (workspace clippy deny-set, tests, the cheap gate)
and is independently useful. The feature-test harness (built in Phase 0)
drives every handler as `(fixture map + cursor marker) → payload` against
golden/inline expectations.

### Phase 0 — spine (ships diagnostics)

Crates `ipe_lsp_server` + `ipe_lsp_features`; `lsp-server`/`lsp-types`
pinned; `ipe lsp` dispatch arm in `ipe`.

1. Position mapper FIRST — property-tested UTF-16↔byte (+utf-8
   negotiation), everything depends on it.
2. VFS (ropey) + incremental sync + watched-file reconciliation over
   `sync_source_root`.
3. Main loop: single writer, worker-pool snapshot reads, `catch_unwind` +
   `Cancelled`→`ContentModified` + latency budget; lifecycle +
   capability negotiation.
4. **Push diagnostics**: home-attributed, exact ranges, `codeDescription`
   explain links, debounced; `ipe_parse` fuzz gate stood up.
5. The feature-test harness.

*Gate:* lifecycle + offset property tests + resilience/fuzz tests +
diagnostics fixture tests; INV-1/G1 grep checks in CI.

### Phase 1 — the alive editor (single-read features)

Hover (type + teacher snippet), go-to-def/declaration, document symbols,
folding ranges, selection ranges, document links, semantic tokens
(full+delta+range; exhaustive walkers), scope-aware completion
(`scope_at`).

*Gate:* per-feature fixture tests; semantic-token + folding golden
snapshots over the example corpus; broken-buffer degradation tests
(hover→null, completion→recoverable names, never a panic).

### Phase 2 — whole-program reads + pull model

`collect_references` walker (find references, document highlight);
derived symbol-index query (workspace symbols); go-to-type-definition;
signature help; inlay hints; pull diagnostics with revision-fingerprint
`resultId`s; compiler-`Suggestion` code actions (the existing `ipe fix`
channel surfaced with `Applicability` mapping).

*Gate:* reference-set golden over the corpus (catches a
compiling-but-wrong walker arm); pull-model `unchanged` correctness test
(edit → new resultId; no-op → `unchanged`); never-stale guard stood up
CI-required from this phase on.

### Phase 3 — type-directed completion (the DX headline)

`ipe_types` work first: the `ExpectedTypes` sidecar (additivity
property-tested) → `expected_type_at`; then `unifiable_candidates` with
scratch-arena isolation (property-tested); ranked completion handler
(`sortText` total order, arity-aware snippets, teacher-snippet docs).

*Gate:* expected-`Int`-never-offers-`String` test; arena-isolation
property; ranked-list golden; fallback-to-scope-only when unconstrained.

### Phase 4 — verified edits I: the gate, rename, T0018/lint actions

`ipe_lsp_edits`: the `VerifiedEdit` type + blast-radius closure
(built and tested before any producer); rename + prepareRename; call
hierarchy; the IPE-T0018 action family (expand catch-all / add missing
arms / keep-open) as the exhaustiveness design lands; lint quick-fix
surfacing when `ipe_lint` lands (both degrade gracefully to "not yet
offered" if their compiler-side dependency hasn't merged — the surfacing
code is additive).

*Gate:* type-level test that no public path constructs a `WorkspaceEdit`
outside the gate; rename-across-modules + refused-target tests; a
gate-rejection test (an edit breaking a downstream importer yields `Err`,
no action surfaced).

### Phase 5 — verified edits II: auto-import, TEA scaffolding, file rename

Auto-import (completion-resolve `additionalTextEdits` + unresolved-name
quick-fix; one import-insertion helper; disambiguation list on
ambiguity); the TEA snippet catalog (golden-tested at its two honest
bars) + program-reading TEA actions; `workspace/willRenameFiles`.

*Gate:* auto-import round-trip + ambiguity-list tests; snippet goldens
(self-contained set typechecks standalone; fragment set parse-clean
only); willRename closure test (importers rewritten, gate green).

### Phase 6 — formatting + polish (gated on the formatter port)

The `ipe fmt` port is its own project (tracked separately; the LSP does
not own it). When it lands: `textDocument/formatting` (one full-doc
edit), range formatting, fmt-idempotence assertion, and the fmt stage is
added to the `VerifiedEdit` round-trip (+ its parse/type-preservation
test). Reference-count code lens (opt-in) if confirmed (§12).

*Gate:* format == `ipe fmt` byte-identical; second pass byte-identical;
broken buffer → no edits; gate-with-fmt round-trip tests.

**Sequencing note.** Phases 0–2 are strictly ordered; 3 and 4 are
independent of each other after 2; 5 needs 4's gate; 6 floats on the
formatter port. The `sky_*`→`ipe_*` rename has landed; the crates are
`ipe_*` from their first commit (OPEN-4 resolved).

---

## 9. Testing spine (cross-phase)

- **Harness**: fixture map + `⟨|⟩` cursor marker → handler → payload
  assertion; used by every feature test.
- **Golden snapshots**: semantic tokens, folding, reference sets, ranked
  completion lists — over the example corpus, so a wrong-but-compiling
  walker arm is a diff, not a mystery.
- **Property tests**: offset round-trip; expected-types additivity;
  scratch-arena isolation; fmt preservation (Phase 6).
- **Fuzz gates**: `ipe_parse` (no panic/bounded time on mutated buffers);
  handler-level (random buffers through every feature).
- **Never-stale guard**: warm-vs-fresh-database payload equality across
  adversarial edit scripts; CI-required from Phase 2.
- **Protocol conformance smoke**: drive the real binary over stdio with a
  scripted client (initialize → didOpen → didChange → requests →
  shutdown) per phase's advertised capability set — capabilities never
  advertise what a handler can't serve.

---

## 10. What makes this LSP stand out

Against the reference server: everything it has, plus pull diagnostics,
type-definition, document highlight, workspace symbols, folding,
selection range, document links, call hierarchy, semantic-token
delta/range, type-directed completion, the verified-edit gate on every
action (the reference applies rename/fixes unverified), lint + T0018
action families, auto-import with refuse-don't-guess ambiguity, sound
module rename, and no catch-all walker arms (its `_ -> []` fragility is a
compile error here).

Against the field:

- **rust-analyzer** is the architectural model (salsa, single-writer,
  cancellation) — but even r-a offers assists that don't always compile;
  our `VerifiedEdit` gate is stricter: verify-on-offer over the full
  importer closure, expressed as a type.
- **gopls/tsserver** ship fast approximate engines that can disagree with
  the batch compiler; ours structurally cannot (one query graph for
  build, watch, and IDE).
- **elm-language-server** wraps the compiler binary and re-parses with
  tree-sitter (two analyzers by construction); ours is in-process over
  the compiler's own memoized queries with exact home-attributed spans.
- The **teaching integration** (explain-page links, ELI10 hover
  snippets, `Applicability`-honest quick-fixes) has no counterpart in any
  of the above.

The pitch, in one line: *the first LSP where "the editor suggested it"
implies "it compiles" — proven by types, gates, and CI, not by intent.*

---

## 11. §0 flags — soundness we cannot yet guarantee (honest blockers)

| Flag | Feature | Gap | Disposition |
|---|---|---|---|
| F-1 | Formatting / fmt-clean clause of the gate | **no formatter exists in this workspace** | formatting not advertised until the `ipe fmt` port lands; the gate runs parse→canon→typecheck until then and adds fmt + its preservation test with the port. Never an LSP-side pretty-printer. |
| F-2 | `implementation`, `typeHierarchy`, `linkedEditingRange` | no sound semantic mapping (HM, no interfaces; heuristic linked-edit can capture) | not advertised (refuse, don't fake); revisit if the language grows the concepts |
| F-3 | Type-directed completion | `ExpectedTypes` sidecar is net-new solver work inside `constrain.rs`/`solve.rs` | ships only with the additivity property green (recording it changes nothing else) and the arena-isolation property green |
| F-4 | Any code lens that runs project code ("run main", "run test") | violates G5 (the server executes no project code); even client-executed commands normalize a run-from-editor path we haven't threat-modeled | v1 ships at most the display-only reference-count lens, default off; run lenses need their own security design first |
| F-5 | Partial-buffer result *quality* | `ipe_parse` error recovery is best-effort today | no-crash holds unconditionally (G3 layers 2–3); recovery-quality improvements are additive and tracked on the parser, not the LSP |
| F-6 | Settled-edit diagnostics latency on large projects | `typecheck` is whole-program-coarse | correct today, honest in docs; per-module `typecheck` (salsa doc §9.4) is the tracked unlock — zero handler changes when it lands |
| F-7 | Lint + IPE-T0018 surfacing | companion designs not yet implemented | LSP surfacing code is additive; rows 22–23 activate when their compiler-side dependencies merge |

---

## 12. Decision ledger + decisions needing a user call

Locked by this plan (with rationale):

1. **Single production backend on `ipe_db`; `BatchView` retired** (§3.2).
   The pre-salsa fallback's precondition no longer exists; a second
   driver is drift surface. `ProgramView` survives as the handler seam.
2. **Verify-on-offer everywhere** — the v0 verify-on-apply compromise
   (`ipe-lsp.md` OPEN-2) dissolves with the batch backend.
3. **Three crates** (`ipe_lsp_server` / `ipe_lsp_features` /
   `ipe_lsp_edits`); the edits crate is the only `WorkspaceEdit`
   producer and only through the gate.
4. **Position-encoding negotiation** with utf-8 preference, utf-16
   mandatory.
5. **Pull + push diagnostics both advertised**; `resultId` = salsa
   revision fingerprint.
6. **Capabilities never over-advertise** — each phase's `initialize`
   reply lists exactly what that phase serves.

Needing a user call before implementation:

1. **Confirm `BatchView` retirement** (decision 1) — it supersedes a
   locked choice in `ipe-lsp.md` Q1/OPEN-2 on the grounds that its
   precondition (no salsa layer) is gone.
2. **Formatter sequencing** — the `ipe fmt` port is a prerequisite for
   LSP formatting (F-1) and the gate's fmt clause. Port it as its own
   lane before/during LSP Phases 1–2, or accept a formatting-less v1?
3. **Code lens scope** — reference-count lens opt-in (proposed), or drop
   code lens from v1 entirely?
4. **TEA scaffolding priority** — keep in Phase 5 (proposed) or pull
   ahead of Phase 4's action families as the second headline?
5. **OPEN-1 inherited** — LSP process shares only the read-only
   content-addressed build-cache artifacts, never writes it
   (provisional stance unchanged; confirm against the cache ownership
   rules when Phase 0 starts).
6. **Timing vs the C.1 rename** (OPEN-4 inherited) — confirm the LSP is
   not pulled ahead of C.1 in a way that creates a lasting naming split.

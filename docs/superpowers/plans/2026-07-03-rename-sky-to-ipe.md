# Rename Sky → Ipê / Ipe / ipe (the pre-push total rename)

> Implementation plan (superpowers *writing-plans* grade). Read-only design
> artifact; the tasks below are executed later, in order, by an implementer.
> Source spec (follow, do not redesign): task **#59** + memory
> `pre-push-rename-sky-to-ipe.md`. This is **mandatory Tier-3 item #1** — the
> FINAL step before pushing to `arthurmaciel/ipe-lang`, run only after DONE
> (example sweep green). It is a pure rename: **no behaviour changes, no new
> features**. Every task ends `cargo`-green + tests-green; the whole plan ends
> with the full sweep green and the invariant greps clean.

---

## Goal

Rename OUR language and its toolchain from **Sky** to **Ipê** everywhere,
**case-preservingly** —

| Form found | Replaced by | Applies to |
|---|---|---|
| `Sky` | `Ipe` | code identifiers, type names, module namespaces |
| `sky` | `ipe` | crate names, binary, extension, paths, runtime module |
| `SKY` | `IPE` | env vars, error-code prefixes |
| language NAME in doc **prose** | `Ipê` (capital + caret) | `.md` narrative text only |

**Two hard invariants (must hold at the end):**
1. No lowercase `ipê` (with caret) **anywhere** — the caret form is `Ipê`, prose-only.
2. No lowercase `ipe` (no caret) in doc **prose** — prose says `Ipê`; only quoted
   code identifiers (`ipe` command, `ipe-lang`, `.ipe`, `Ipe.Core`, `ipe_types`) keep code form.

**The critical constraint** — this is NOT a blind global `sed`. The repository
references the **upstream Sky project we ported from**. Those occurrences name a
*different* project and MUST stay `Sky`. The rename is gated against a curated,
guardian-reviewed **exclusion list** (§Exclusion list below). "Divergences from
Ipê" is nonsense; "ported from Ipê" is a lie. Protect them.

---

## Ground-truth inventory (measured at HEAD, 2026-07-03)

| Axis | Measured value |
|---|---|
| Total occurrences `Sky` / `sky` / `SKY` (excl. `target/`, `Cargo.lock`) | **12 196** / **13 733** / **2 981** = **28 910** |
| Files touched (any case) | **1 023** |
| Workspace crates (all `sky_*` / `skyc`) | **12**: `sky_intern`, `sky_diagnostics`, `sky_kernels`, `sky_syntax`, `sky_parse`, `sky_canon`, `sky_types`, `sky_ir`, `sky_lower`, `sky_backend`, `sky_backend_rust`, `skyc` |
| Other workspace members | `runtime` (module `sky_runtime`), `tools/oracle`, `tools/refresh-oracle`, `tools/sky-ffi-inspect-rs`; plus `plugins/sky-compiler` |
| `.ipe` source files | **459** (of which **269** are `Main.ipe`) |
| Runtime type names | `SkyResult` (1166), `SkyMaybe` (399), `SkyTask` (238), `SkyStringify` (133), `SkyError` (132), `SkyCmd` (84), `SkySub` (76), `SkyDict` (28), `SkyRow`, `SkySet`, `SkyEnv`, `SkyCacheHandle`, `SkyFluentSel`, `SkyID*` |
| Runtime module dir | `runtime/src/sky_runtime/` → `runtime/src/ipe_runtime/` |
| Stdlib namespace dir | `crates/skyc/stdlib/Sky/Core/` → `.../Ipe/Core/`; qualifiers `Sky.Core`, `Sky.Live`, `Sky.Http.Server(.Stream/.WebSocket)`, `Sky.Tui`, `Sky.Webview`, `Sky.Test` |
| Error codes | prefix `IPE-` on ~90 distinct codes (`P/N/L/T/I` series + `F44xx` FFI docs); **90-ish `explain/IPE-*.md`** files + `sky_diagnostics/src/code.rs` enum + `diagnostic.rs`/`render.rs` + ~514 test/doc reference lines |
| Env vars | `IPE_*` (documented ~50 in `CLAUDE.md` + internal `IPE_E2E`, `IPE_RUNTIME_DIR`, `IPE_DB_URL`, `IPE_DCE`, `IPE_SOLVER_BUDGET*`, `IPE_UI_LAZY_CAP`, `IPE_TUI_QUIET`, `IPE_LIVE_*`, `IPE_CONSOLE_*`, `IPE_AUTH_*`) |
| CLI binary | `sky` → `ipe`; compiler binary `skyc` → `ipec` |

Everything is git-tracked, so directory/file renames use **`git mv`** to preserve history.

---

## Global constraints (apply to every task)

**PRINCIPLES order (strict tie-breaker, `PRINCIPLES.md`):**
1. Security → 2. Correctness → 3. Soundness → 4. Efficiency → 5. Completeness → 6. Readability.

For a rename the load-bearing principles are **Correctness** (the renamed
program must behave identically — a stray/half rename that breaks a qualifier
lookup silently accepts an ill-typed program) and **Soundness** (a
namespace/qualifier mismatch between `canon` and `constrain` is the
"exit-0-then-cargo-fail" class). Efficiency/Readability are inert here; do not
let a "tidy while I'm in there" edit sneak in — this plan changes **names only**.

**The two fundamental rules:**
- **Parse, don't validate.** The exclusion list is itself parsed once (§Task 1)
  into a machine-checkable allowlist; downstream rename tasks consult that typed
  allowlist rather than re-deciding "is this upstream?" ad hoc per file.
- **Make invalid states unrepresentable.** After the rename, a *residual*
  `Sky/sky/SKY` outside the sanctioned set is a **CI-failing grep** (§Task 8), so
  a half-done rename cannot be represented as "green". Error codes stay a closed
  set: the `code.rs` enum, the `explain/*.md` filenames, and the test-expected
  strings move in one lockstep so a dangling code is a compile/test error.

**Non-negotiables carried in:** never `sky build`/`cargo build` from a stray dir
that clobbers `sky-out/`; every long command `timeout`-bounded; `mem-guard.sh`
running; clean up background tasks; `rg` not `grep`.

---

## Exclusion list (curated — build in Task 1, ~24 files, line-level)

These name the **upstream Sky** project (the Haskell→Go compiler this repo was
ported from) or the upstream Go binary/ecosystem. In them, the token `Sky` /
`sky` referring to *that* project **stays**. Note the nuance: within these files,
tokens that name **our own artifacts** — error codes (`IPE-L0108`), our source
extension (`Main.ipe`), our crate names — **still rename**; only the *upstream
project name* is frozen. This is why the list is **line-level**, never file-level.

| # | File | What stays `Sky` | What still renames in it |
|---|---|---|---|
| 1 | `docs/divergences-from-sky.md` | filename, title, "divergences from Sky", `../sky` paths, Go-reference prose | our error codes `IPE-*`→`IPE-*`, our crate/type names |
| 2 | `docs/divergences-review.md` | upstream "Sky" prose, `../sky` | our codes/identifiers |
| 3 | `docs/divergences-from-sky.md` §6 "Planned future divergences" (absorbed the former ideas-log of departures from Sky) | "departures from Sky" prose, codes discussed as upstream | our error codes when ours |
| 4 | `docs/README-draft-relation-to-elm-and-sky.md` | §"Relationship to Sky", "ported from **Sky**" (L32), "Rust port of Sky" / parity-reference (L145-148) | **also fix**: this draft uses lowercase `ipê` for our language → must become `Ipê` (invariant 1); our identifiers keep code form |
| 5 | `docs/architecture/sky-rust-backend-reference-audit.md` | in-content upstream "Sky" references, `../sky` | **filename renames** → `ipe-rust-backend-reference-audit.md` (the "sky-rust-backend" in the name is OUR backend) |
| 6-18 | `docs/architecture/{repo-layout-and-mirroring, static-compilation, go-oracle-fixture-corpus-plan, examples-sweep-port, windows-ci-support, principled-decisions-audit, sweep-and-parity-plan, ffi-subsystem-design, ffi-sandbox-and-generator-impl-ready, ffi-port-spec, kernel-registry-design, ui-live-tui-webview-spec, tui-windows-ci}.md` | `../sky` sibling-checkout paths, "Go `sky`" binary, upstream-parity prose | our codes/crates/identifiers when named as ours |
| 19 | `docs/superpowers/plans/*.md` (historical: `float-scinotation-verify`, `2026-06-26-...-m0-spine`, `phase4-...`, `examples-sweep-run`, `ci-and-push`, `secret-type`, `m5b-db-followups`, `let-bound-cfg-diagnostic`, `ffi-phase0-inspector`, `registry-phase-{D,E}`, `crate-spec-ssot`, `m5b-http-followups`, `m5a-task-followups`, `css-attr-injection-safe-emit`) | `../sky` paths + "Go reference" — these are frozen historical artifacts | **treat as frozen**: prefer to leave historical plans untouched except where they name a live path the harness reads |
| 20 | `scripts/lib/env.sh` | `../sky` sibling-repo path variable (points at upstream checkout) | any `IPE_*` env var WE own that the script exports |
| 21 | `tools/oracle/src/lib.rs`, `tools/refresh-oracle/src/main.rs` | "Go `sky` version" / the upstream Go binary name captured by the oracle | `MAIN_SKY`/`"Main.ipe"` (OUR extension → `Main.ipe`), crate identifiers |
| 22 | `runtime/src/sky_runtime/{string,stringify}.rs` | any `../sky` provenance comment pointing at upstream | the module path itself renames (Task 2) |
| 23 | `CLAUDE.md` (root) | passages describing the **upstream** Haskell→Go Sky compiler/release flow, `SkyDeploy` (upstream product), "Sky release" | our Rust tooling names, our env vars, our extension — **highest-ambiguity file, curate every line** |
| 24 | `examples/32-sse-relay/{README.md,src/Main.ipe}` | `SkyDeploy` agent-service (upstream product name) | the `.ipe`→`.ipe` extension of the file itself |

**Curation candidates flagged for guardian sign-off (decide during Task 1):**
- `SkyDeploy` — upstream Go ecosystem product. Likely **stays** (proper product
  name of the other project), even though it starts with `Sky`. Confirm.
- Root `CLAUDE.md` — much of it documents the **Go** Sky project's workflow; only
  the parts describing *this* Rust repo's tooling rename. Split line-by-line.
- Remote/URL `arthurmaciel/ipe-lang` is already `ipe`; `anzellai/sky` (upstream
  remote) stays.

**Deliverable of Task 1:** `scripts/rename/upstream-exclusions.txt` — a
`file:line-context` allowlist consumed by the residual-grep in Task 8, so "is
this a sanctioned Sky?" is a lookup, not a judgement call, at verification time.

---

## Tasks (bite-sized, ordered, each independently green)

### Task 1 — Build & freeze the upstream-Sky exclusion list
- Enumerate every file matching upstream markers:
  `rg -l -i 'divergenc|departures-from-sky|reference-audit|\.\./sky|anzellai/sky|ported from|Go reference|SkyDeploy|relation-to-elm-and-sky'`.
- For each, record the specific **lines/contexts** that name upstream Sky vs. name our artifacts.
- Write `scripts/rename/upstream-exclusions.txt` (the typed allowlist) + a one-paragraph rationale header.
- Guardian sign-off on the `SkyDeploy` and root-`CLAUDE.md` ambiguities.
- **Green gate:** no build; the artifact exists and every listed line is justified. This is the parse-once boundary for the whole plan.

### Task 2 — Crate-by-crate rename (ONE crate per sub-task, cargo-green after each)
Order = leaf-to-root dependency order so each rebuild is minimal and self-checking:
`sky_intern → sky_diagnostics → sky_kernels → sky_syntax → sky_parse → sky_canon → sky_types → sky_ir → sky_lower → sky_backend → sky_backend_rust → skyc`,
then `runtime` (module `sky_runtime`→`ipe_runtime`), `tools/oracle`, `tools/refresh-oracle`, `tools/sky-ffi-inspect-rs`, `plugins/sky-compiler`.

For each crate `sky_X` → `ipe_X` (and `skyc`→`ipec`):
1. `git mv crates/sky_X crates/ipe_X` (dir rename, history preserved).
2. In its `Cargo.toml`: `[package] name = "ipe_X"`; rename any `[[bin]]`/`[lib]` names.
3. In **every** `Cargo.toml` across the workspace: rewrite `sky_X = { path = "../sky_X" }` → `ipe_X = { path = "../ipe_X" }` and the top-level `[workspace] members` entry.
4. Rewrite every `use sky_X::` / `sky_X::` path and `extern crate` across all crates.
5. The CLI binary `skyc`→`ipec`: rename the bin target + any `sky-out/skyc` producer/consumer refs in scripts.
6. **Green gate:** `timeout 1200 cargo build -p ipe_X` (then a workspace `cargo build` after the last crate). Because a missed dependency ref fails to compile, the compiler is its own completeness check for this task.

Notes: `sky_runtime` is a *module path* inside the `runtime` crate, not a crate
name — rename `runtime/src/sky_runtime/`→`ipe_runtime/`, its `mod`/`pub use` in
`lib.rs`, and the ~186 `sky_runtime` references (incl. emitted-code preamble that
names the runtime module — this couples to Task 4/5's generated output, verify
goldens there).

### Task 3 — Type / identifier renames (`SkyResult` → `IpeResult`, …) via scoped replace
- Rename the runtime/emitted type names: `SkyResult→IpeResult`, `SkyMaybe→IpeMaybe`,
  `SkyTask→IpeTask`, `SkyError→IpeError`, `SkyCmd→IpeCmd`, `SkySub→IpeSub`,
  `SkyDict→IpeDict`, `SkySet→IpeSet`, `SkyStringify→IpeStringify`, `SkyRow→IpeRow`,
  `SkyEnv→IpeEnv`, `SkyCacheHandle→IpeCacheHandle`, `SkyFluentSel*→IpeFluentSel*`,
  `SkyID*→IpeID*`, `SkyMaybeVisitor→IpeMaybeVisitor`, `SkyCoreErrorError→IpeCoreErrorError`.
- These live in **two** coupled places: (a) `runtime/src/ipe_runtime/*.rs` definitions,
  and (b) the **backend emitter** (`crates/ipe_backend_rust/src/*`) that emits these
  names into generated Rust + the golden `tests/golden/*/main.rs` fixtures.
  Rename emitter + regenerate/rewrite goldens in the same sub-task so emitted ≡ fixture.
- Use word-boundary scoped replace (`\bSky<Name>\b`) to avoid touching substrings.
- **Green gate:** `cargo build` + the golden diff tests + runtime unit tests green.

### Task 4 — Source extension `.ipe` → `.ipe` (+ `Main.ipe`→`Main.ipe`)
- `git mv` all **459** `.ipe` files to `.ipe` (269 `Main.ipe`→`Main.ipe`), across
  `examples/`, `tests/golden/`, `crates/*/stdlib/`, `crates/*/tests/`, test-fixtures.
- Rewrite the extension in the **toolchain that reads it**:
  - loader/driver in `crates/ipec/src/{lib,project,stdlib}.rs`,
  - parser entry (`crates/ipe_parse/src`),
  - `sky watch`/doc paths, `tools/oracle` (`MAIN_SKY = "Main.ipe"`, `sha256(Main.ipe)` comments),
  - sweep harness `scripts/equivalence-checks/examples-sweep.sh` + `scripts/lib/{examples,checks,env}.sh` (`src/Main.ipe`→`src/Main.ipe`, `*.ipe` globs),
  - `sky.toml` `entry = "src/Main.ipe"` fields.
- **Exclusion gate:** in the historical `docs/superpowers/plans/*` (frozen), leave prose `Main.ipe` mentions unless the harness reads that path.
- **Green gate:** `timeout 3600 cargo test` (golden + skyc/ipec integration) green; a spot example builds via the sweep harness.

### Task 5 — Stdlib namespace `Sky.Core`→`Ipe.Core` (+ `Sky.Live/Http.Server/Tui/Webview/Test`) — **RISKIEST**
- `git mv crates/ipec/stdlib/Sky` → `.../Ipe`; rewrite `module Sky.Core.X exposing`
  headers → `module Ipe.Core.X` in every stdlib `.ipe` file, and every
  `import Sky.* ` / qualifier in the **459** user/example/test `.ipe` sources.
- **The lockstep that makes this the riskiest task:** the qualifier string `"Sky"`
  / `"Sky.Core"` / `"Live"` / `"Http.Server"` / `"Tui"` / `"Webview"` appears as a
  **key** in *multiple* tables that MUST move together:
  - `ipe_canon` name-resolution / `qual_vars` / `qual_ctors` + stdlib index,
  - `ipe_types::constrain` kernel-scheme qualifier keys,
  - `ipe_lower` callee-dispatch qualifier match arms,
  - `ipe_kernels` `decl()` qualifier strings,
  - the emitted runtime-module qualifiers.
  If canon renames the qualifier but constrain does not (or vice-versa), an
  ill-typed program **passes `ipec`** and only fails at the downstream `cargo`
  build — the exact `exit-0-then-cargo-fail` soundness hole PRINCIPLES §"invalid
  states unrepresentable" forbids, and the compiler's own `cargo build` will NOT
  catch it. It is caught only by the golden gate + example sweep.
- Rename docs-as-code (`Ipe.Core` in the stdlib reference tables) but only the
  identifier form (`Ipe.Core`), not caret.
- **Green gate:** full `cargo test` **and** a representative slice of the example
  sweep (Live/Tui/Webview/Http.Server) build+run — because the compiler build
  alone cannot prove qualifier lockstep. Add a temporary assertion that the
  qualifier sets in canon/constrain/lower are equal (mirrors the existing
  kernel-registry parity tripwire) before declaring green.

### Task 6 — Env vars + error codes + explain-page renames (closed-set lockstep)
- **Env vars** `IPE_*`→`IPE_*`: `ipe_backend_rust/src/{emit_live,project}.rs`
  (`IPE_LIVE_STORE`, `IPE_DB_URL`), test harness envs (`IPE_E2E`,
  `IPE_RUNTIME_DIR`), runtime readers, all doc references. Keep the same
  suffixes; only the prefix changes.
- **Error codes** `IPE-<L/N/T/P/I/F>NNNN`→`IPE-<…>NNNN` in ONE lockstep move:
  - `git mv crates/ipe_diagnostics/explain/IPE-*.md` → `IPE-*.md` (~90 files),
  - the `code.rs` enum + `Display`/parse, `diagnostic.rs`, `render.rs`,
  - every test-expected code string (~514 lines in `crates/*/tests/*`, golden `Main.ipe`, `unsupported.rs`, `hardening.rs`),
  - doc mentions of our codes (including inside the excluded upstream docs, per §Exclusion nuance — codes are ours).
- The explain-page loader keys files by code → the `.md` filename set and the
  enum must be identical or `ipe explain IPE-Lxxxx` 404s: verify the closed set matches.
- **Green gate:** `cargo test` (diagnostics + golden gate tests assert the new
  codes) green; `ipe explain IPE-L0108` resolves.

### Task 7 — Doc PROSE: language name `Sky`→`Ipê` (excluding upstream + code identifiers)
- In `.md` narrative text, the **language name** becomes `Ipê` (capital + caret).
- **Do NOT** caret-ise code identifiers quoted in docs: `ipe` command, `ipe-lang`,
  `.ipe`, `Ipe.Core`, `ipe_types`, `IpeResult` keep code form.
- **Gate against the Task 1 exclusion list** line-by-line: upstream "Sky" prose
  stays `Sky`; do not write "Ipê" where the sentence means the other project.
- Fix the pre-existing invariant breach in
  `docs/README-draft-relation-to-elm-and-sky.md` (lowercase `ipê` → `Ipê`),
  while preserving its "Relationship to Sky" upstream section.
- Update `templates/`/user-facing docs, `docs/stdlib.md`, skylive/skyui/etc. doc
  set, README "What's in the box".
- **Green gate:** no build; invariant greps (Task 8) run clean over `.md`.

### Task 8 — Post-rename VERIFICATION (greps + full green sweep)
Run, all `timeout`-bounded, in order:
1. **Residual grep** — `rg 'Sky|sky|SKY'` across the repo (excl. `target/`,
   `Cargo.lock`) MINUS `scripts/rename/upstream-exclusions.txt` ⇒ **must be empty**.
   Any hit is either a missed rename or a new sanctioned upstream ref (then it must
   be added to the allowlist with a rationale — never silently).
2. **Invariant 1** — `rg 'ipê'` (lowercase caret) ⇒ **empty everywhere**.
3. **Invariant 2** — `rg -t md '\bipe\b'` in prose contexts (excluding fenced code
   spans / inline-code) ⇒ **empty**; prose must read `Ipê`.
4. **Build/lint/test** — `cargo build`, `cargo clippy` (0 warnings — the
   workspace denies `unwrap/expect/panic/indexing_slicing/…`), `timeout 3600 cargo test`.
5. **Example sweep** — the full ported sweep (build + run + Go-equivalence) green,
   confirming qualifier lockstep (Task 5) and extension/loader (Task 4) end-to-end.
6. `ipe --version` prints a version (not a server); `ipec` builds an example.

---

## Verification matrix (which task each guarantee comes from)

| Guarantee | Proven by |
|---|---|
| Crate graph still links | Task 2 per-crate `cargo build` |
| Emitted ≡ golden type names | Task 3 golden diff |
| Loader reads `.ipe` | Task 4 integration + sweep |
| Qualifier lockstep (no exit-0-cargo-fail) | Task 5 golden + sweep + parity assertion |
| Error-code closed set intact | Task 6 diagnostics tests + `ipe explain` |
| No upstream ref corrupted | Task 1 allowlist + Task 8 residual grep |
| Invariants (caret/lowercase) | Task 8 greps 2-3 |

---

## Coupling flagged (NOT in this plan)

**Flat-namespace redesign** ([[macro-roadmap-post-parity]]): the memory notes the
`Sky.Core`-vs-`Std` split should later collapse into one flat, auto-imported +
DCE'd namespace, and that it "couples to the rename." **Decision: keep it a
separate follow-up plan.** Rationale (PRINCIPLES: Correctness before Efficiency/
Readability): this plan is a *pure, mechanical, invariant-checkable rename* whose
correctness is provable by "the only change is names." Folding a semantic
namespace redesign into it would (a) make the residual-grep verification
impossible to interpret (real semantic diffs mixed with rename diffs), and (b)
turn each cargo-green checkpoint into a moving target. Do the rename first
(`Sky.Core`→`Ipe.Core`, structure unchanged), land it green, *then* flatten as
its own Tier-3 item.

---

## Risk table

| Risk | Task | Severity | Mitigation |
|---|---|---|---|
| **Qualifier rename desync canon↔constrain↔lower** → ill-typed program passes `ipec`, fails `cargo` (exit-0-cargo-fail, soundness class) | **5 (riskiest)** | High | Move all qualifier tables in one sub-task; temporary parity assertion (qualifier sets equal); gate on golden + example sweep, not just compiler build |
| Error-code enum ↔ `explain/*.md` filename ↔ test-string three-way drift → dangling code, `explain` 404 | 6 | High | One lockstep move; closed-set equality check enum≡filenames |
| Blind sed corrupts an upstream-Sky reference ("Divergences from Ipê") | 1/7 | High | Curated line-level allowlist built first; Task 8 residual grep is allowlist-gated |
| Missed `use sky_X::` / path dep | 2 | Medium | Self-caught: `cargo build` fails to link |
| Emitted type name ≠ golden fixture | 3 | Medium | Rename emitter + regenerate goldens together |
| `.ipe` extension missed in one loader path (watch/doc) | 4 | Medium | Enumerate all readers; sweep exercises build path |
| Root `CLAUDE.md` / `SkyDeploy` mis-classified | 1 | Medium | Guardian sign-off; default `SkyDeploy` stays (upstream product) |
| Historical `docs/superpowers/plans/*` over-edited | 1/7 | Low | Treat frozen; edit only harness-read paths |

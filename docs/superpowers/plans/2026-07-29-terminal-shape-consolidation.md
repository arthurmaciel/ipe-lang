# Terminal shape consolidation — implementation plan

> **For agentic workers:** implement task-by-task; build+test after each. The
> spec is `docs/architecture/tbd/shapes-model-design.md`.

**Goal:** Collapse the `Tui` and `Console` shapes into one `Ipe.Tea.Terminal`
shape with two entries — `Terminal.appScreen` (was `Tui.app`, `Model -> Element
Msg` + `onKey`, full screen) and `Terminal.appLines` (was `Console.app`, `Model
-> String` + `onLine`, line stream). Drop `Tui.program`. `Web`/`WebView`/
`Program` are untouched.

**Architecture:** The shape modules are kernel-only (no `.ipe` stdlib source).
The change is a coordinated rename across the compiler's kernel registry, canon
qualifier table, type schemes, lowering arms, per-shape emitters, and their
provenance strings — plus migrating every golden/example/test that names the old
entries. Emitted runtime output is unchanged in behavior; the two existing
emitters are re-pointed, not rewritten.

## Global constraints

- **Display-corruption hazard.** This repo's identifiers `Tui`/`Console`/
  `program` may render corrupted in *some* tool output. Verify every rename by
  **byte-level counts** (`rg -c`, `git grep -c`) and a clean final
  `cargo build`/`nextest`, never by eyeballing a displayed spelling.
- Isolated `CARGO_TARGET_DIR` (a warm lane dir); never pollute in-tree `target/`.
- No self-backgrounding; foreground `timeout`-wrapped builds only.
- Comments say WHAT not HOW; no archaeology labels; self-explaining names.
- No AI-attribution trailer on any commit.

## Old → new map (authoritative)

| Old | New |
|---|---|
| module `Ipe.Tea.Tui` + `Ipe.Tea.Console` | module `Ipe.Tea.Terminal` |
| short qualifier `Tui`, `Console` | short qualifier `Terminal` |
| kernel `TuiApp` | `TerminalAppScreen` |
| kernel `ConsoleApp` | `TerminalAppLines` |
| kernel `TuiProgram` | **removed** |
| entry `Tui.app` | `Terminal.appScreen` |
| entry `Console.app` | `Terminal.appLines` |
| entry `Tui.program` | **removed** (raw-cell escape is the deferred follow-up) |
| `KernelClass::Tui` / `::Console` | `KernelClass::Terminal` (keep one class; both entries share it or keep two emitter arms under it) |

The lowering leaf provenance (`tui_app_ui`, `console_app`, runtime symbol names)
may stay as-is internally where renaming them would churn the runtime crate —
only the **public** module/entry names and the Rust **kernel enum variants**
must change. Keep runtime symbol churn minimal; the goal is the public surface.

---

## Task 1: Canon registry — merge qualifiers

**Files:** `src/compiler/canon/src/env.rs`

- [ ] In `STDLIB_MODULE_QUALIFIERS` (the `&["Ipe","Tea",…]` table, ~line 119/129):
  replace the two rows `(&["Ipe","Tea","Tui"], "Tui")` and
  `(&["Ipe","Tea","Console"], "Console")` with one
  `(&["Ipe","Tea","Terminal"], "Terminal")`.
- [ ] In the member map (~line 1627/1632): replace `("Tui", &["app","program"])`
  and `("Console", &["app"])` with `("Terminal", &["appScreen","appLines"])`.
- [ ] In `QUALIFIER_ALIASES` (~line 1772/1776): replace the `Ipe.Tea.Tui` /
  `Ipe.Tea.Console` alias rows with `("Ipe.Tea.Terminal","Terminal")`.
- [ ] Check for any table-uniqueness `debug_assert`; ensure no dup `Terminal` row.
- [ ] `cargo check -p ipe_canon`.

## Task 2: Kernel registry — rename variants, drop TuiProgram

**Files:** `src/compiler/kernels/src/lib.rs`, `src/compiler/ir/src/pretty.rs`

- [ ] Rename enum variants `TuiApp`→`TerminalAppScreen`, `ConsoleApp`→
  `TerminalAppLines`; **delete** `TuiProgram`.
- [ ] Update their descriptor rows (`d("Tui","app",…)` → `d("Terminal","
  appScreen",…)`; `d("Console","app",…)` → `d("Terminal","appLines",…)`), the
  `KernelClass` they map to (collapse `Tui`/`Console` class usage to
  `Terminal`), and the `pretty.rs` display strings.
- [ ] Grep the whole tree for the three old variant idents and fix every
  reference (types, lower, backend) — the compiler will list them as errors.

## Task 3: Type schemes

**Files:** `src/compiler/types/src/constrain.rs`, `src/compiler/types/src/lib.rs`

- [ ] Re-point the view-type schemes: `TerminalAppScreen` keeps the old `TuiApp`
  scheme (`view : Model -> Element Msg`, `onKey` required). `TerminalAppLines`
  keeps the old `ConsoleApp` scheme (`view : Model -> String`, `onLine`). Remove
  the `TuiProgram` scheme.
- [ ] `cargo check -p ipe_types`.

## Task 4: Lowering

**Files:** `src/compiler/lower/src/lower.rs`

- [ ] Replace the callee match arms `("Tui","app")`, `("Console","app")`,
  `("Tui","program")` with `("Terminal","appScreen") => TerminalAppScreen` and
  `("Terminal","appLines") => TerminalAppLines`. Drop the `program` arm.
- [ ] `cargo check -p ipe_lower`.

## Task 5: Backend emitters

**Files:** `src/compiler/backend/rust/src/emit_tui.rs`,
`emit_console.rs`, `naming.rs`, `project.rs`, `emit.rs` (dispatch)

- [ ] Re-point the emit dispatch: `TerminalAppScreen` → the emit_tui path;
  `TerminalAppLines` → the emit_console path. Remove the `TuiProgram` emit arm.
- [ ] Rename the emitter files to `emit_terminal_screen.rs` /
  `emit_terminal_lines.rs` **only if** cheap; otherwise leave file names and just
  re-point (file names are internal). Prefer minimal churn.
- [ ] `cargo build --release -p ipe`.

## Task 6: Migrate goldens

**Files:** the 15 golden `Main.ipe` under `tests/golden/**` that import
`Ipe.Tea.Tui`/`Ipe.Tea.Console` or call `Tui.app`/`Console.app`/`Tui.program`
(enumerate with `git grep -l`). Golden dirs named `*cli_program*`/`console_app*`
may need renaming to match their tests.

- [ ] Rewrite each `import Ipe.Tea.Tui`→`import Ipe.Tea.Terminal`, `Tui.app`→
  `Terminal.appScreen`, `Console.app`→`Terminal.appLines`.
- [ ] Any `Tui.program` golden: convert to `Terminal.appScreen` if its view is
  `Element`, else drop the fixture and its test (raw-String full-screen is the
  deferred escape-node follow-up — a golden for a removed entry must not linger).
- [ ] Regenerate ONLY if a byte-diff golden legitimately changes (module name is
  kernel-routed, so most `main.rs` outputs are name-agnostic — a diff means a
  provenance string moved; confirm before accepting).
- [ ] `cargo nextest run -p ipe --test golden_*` green.

## Task 7: Migrate first-party examples + e2e tests

**Files:** `examples/tui-counter/src/Main.ipe`,
`examples/console-repl/src/Main.ipe`, `src/ipe-cli/tests/tui_e2e.rs`,
`golden_cli_program_seal.rs` (+ any `console`/`tui` e2e), `examples/README.md`.

- [ ] Rename example dirs `tui-counter`→`terminal-counter`,
  `console-repl`→`terminal-repl` (update `examples/README.md` shape column and
  any manifest/path references), rewrite their `Main.ipe` to the new entries.
- [ ] Update the e2e test sources + assertion strings to the new module/entry.
- [ ] Leave `examples/sky/**` mirrors alone (reference Sky mirror, CI-committed).
- [ ] `IPE_E2E=1 cargo nextest run -p ipe --test tui_e2e …` green (rename tests
  too if their names embed `tui`/`console`).

## Task 8: Docs

**Files:** `docs/shapes/README.md`, `docs/shapes/tui.md`,
`docs/shapes/console.md`, `AGENTS.md` (shape matrix), and a new ADR.

- [ ] Merge `tui.md` + `console.md` → `docs/shapes/terminal.md` (one guide, two
  entries: `appScreen`/`appLines`); delete the two old files.
- [ ] Update `README.md` matrix to four shapes; fix `AGENTS.md` shape matrix.
- [ ] Write `docs/adr/00NN-terminal-shape-consolidation.md` capturing the
  decision + naming rationale (from the spec). Delete
  `docs/architecture/tbd/shapes-model-design.md`.

## Verification (green gate)

- `cargo build --release -p ipe`; `cargo clippy --workspace` clean; `fmt` clean.
- `cargo nextest run --workspace --profile ci` green.
- `IPE_E2E=1` e2e for the two renamed examples build + run.
- `git grep -c 'Tui\.\(app\|program\)\|Console\.app\|Ipe\.Tea\.\(Tui\|Console\)'`
  over `src/` + `tests/` + first-party `examples/` returns **0** (only
  `examples/sky/**` mirrors may still match).

## Risks

- **Display corruption** — verify by counts + clean build, never displayed text.
- **Golden byte-diffs** — a diff means a provenance string moved; confirm it's
  intentional before regenerating (module names are kernel-routed, usually
  name-agnostic in emitted output).
- **`Tui.program` users** — if a non-mirror fixture/example genuinely needs the
  raw-String full-screen view, the escape node isn't built yet; drop the fixture
  and file a follow-up rather than keeping a dead entry.

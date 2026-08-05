# Terminal shape consolidation — implementation plan

> **For agentic workers:** implement task-by-task; build+test after each. The
> spec is `docs/architecture/tbd/shapes-model-design.md`.

**Goal:** Reduce to four shapes, each with a single structured entry, escapes as
nodes:
- Remove `Web.appHtml` and `WebView.appHtml` (raw HTML stays reachable via the
  existing `Ui.html` node).
- Collapse `Tui`+`Console` → `Ipe.Tea.Terminal`: `Terminal.appScreen` (was
  `Tui.app`, `Model -> Element Msg` + `onKey`) and `Terminal.appLines` (was
  `Console.app`, `Model -> String` + `onLine`).
- Drop `Tui.program`; add a new `Ui.cells : List (List Char) -> Element msg` node
  (terminal-only) that restores its raw-cell capability inside `appScreen`.

`Web.app`, `Web.appRouted`, `Web.renderStatic`, `WebView.app`, `Program` keep
their behavior.

## Global constraints

- **Display-corruption hazard.** Identifiers `Tui`/`Console`/`program` may render
  corrupted in *some* tool output. Verify every rename by **byte-level counts**
  (`git grep -c`) and a clean final `cargo build`/`nextest`, never by an eyeballed
  spelling.
- Isolated `CARGO_TARGET_DIR` (warm lane dir); never touch in-tree `target/`.
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
| kernel `WebAppHtml` | **removed** |
| kernel `WebViewAppHtml` | **removed** |
| entry `Tui.app` | `Terminal.appScreen` |
| entry `Console.app` | `Terminal.appLines` |
| entry `Tui.program`, `Web.appHtml`, `WebView.appHtml` | **removed** |
| — | new kernel `UiCells` → node `Ui.cells` |

The lowering leaf provenance (`tui_app_ui`, `console_app`, runtime symbol names)
may stay internally where renaming would churn the runtime crate — only the
**public** module/entry names and the Rust **kernel enum variants** must change.

---

## Task 1: Remove the `appHtml` entries

**Files:** `src/compiler/kernels/src/lib.rs`, `src/compiler/canon/src/env.rs`,
`src/compiler/types/src/constrain.rs`, `src/compiler/lower/src/lower.rs`,
`src/compiler/backend/rust/src/emit_web.rs`/`emit_webview.rs`,
`src/compiler/ir/src/pretty.rs`.

- [ ] Delete kernels `WebAppHtml`, `WebViewAppHtml` and every reference (the
  compiler lists them as errors). Remove `appHtml` from the `Web`/`WebView`
  member lists in `env.rs`. Remove their type schemes, lower arms, emit arms,
  pretty strings.
- [ ] Confirm `Ui.html` (`UiHtml`) is untouched — it is the raw-HTML escape now.
- [ ] `cargo check -p ipe_canon -p ipe_types -p ipe_lower`.

## Task 2: Canon registry — merge Tui/Console qualifiers → Terminal

**Files:** `src/compiler/canon/src/env.rs`

- [ ] In `STDLIB_MODULE_QUALIFIERS`: replace `(&["Ipe","Tea","Tui"], "Tui")` and
  `(&["Ipe","Tea","Console"], "Console")` with one
  `(&["Ipe","Tea","Terminal"], "Terminal")`.
- [ ] In the member map: replace `("Tui", &["app","program"])` and
  `("Console", &["app"])` with `("Terminal", &["appScreen","appLines"])`.
- [ ] In `QUALIFIER_ALIASES`: replace the `Ipe.Tea.Tui` / `Ipe.Tea.Console` rows
  with `("Ipe.Tea.Terminal","Terminal")`.
- [ ] Check for a table-uniqueness `debug_assert`; no dup `Terminal` row.
- [ ] `cargo check -p ipe_canon`.

## Task 3: Kernel registry — rename variants, drop TuiProgram

**Files:** `src/compiler/kernels/src/lib.rs`, `src/compiler/ir/src/pretty.rs`

- [ ] Rename `TuiApp`→`TerminalAppScreen`, `ConsoleApp`→`TerminalAppLines`;
  **delete** `TuiProgram`.
- [ ] Update descriptor rows (`d("Tui","app",…)`→`d("Terminal","appScreen",…)`;
  `d("Console","app",…)`→`d("Terminal","appLines",…)`), the `KernelClass` they
  map to (collapse `Tui`/`Console` class usage to `Terminal`), the ALL-kernels
  list, and `pretty.rs` display strings.
- [ ] Fix every reference the compiler flags (types, lower, backend).

## Task 4: Type schemes

**Files:** `src/compiler/types/src/constrain.rs`, `src/compiler/types/src/lib.rs`

- [ ] `TerminalAppScreen` keeps the old `TuiApp` scheme (`Model -> Element Msg`,
  `onKey`). `TerminalAppLines` keeps the old `ConsoleApp` scheme (`Model ->
  String`, `onLine`). Remove the `TuiProgram` scheme.
- [ ] `cargo check -p ipe_types`.

## Task 5: Lowering

**Files:** `src/compiler/lower/src/lower.rs`

- [ ] Replace arms `("Tui","app")`, `("Console","app")`, `("Tui","program")`
  with `("Terminal","appScreen") => TerminalAppScreen` and
  `("Terminal","appLines") => TerminalAppLines`. Drop the `program` arm.
- [ ] `cargo check -p ipe_lower`.

## Task 6: Backend emitters

**Files:** `src/compiler/backend/rust/src/emit_tui.rs`, `emit_console.rs`,
`naming.rs`, `project.rs`, dispatch in `emit.rs`.

- [ ] Re-point emit dispatch: `TerminalAppScreen`→emit_tui path;
  `TerminalAppLines`→emit_console path. Remove the `TuiProgram` emit arm.
- [ ] Leave emitter file names as-is (internal); re-point only. Minimal churn.
- [ ] `cargo build --release -p ipe`.

## Task 7: Migrate goldens

**Files:** golden `Main.ipe` under `tests/golden/**` importing
`Ipe.Tea.Tui`/`Ipe.Tea.Console` or calling `Tui.app`/`Console.app`/`Tui.program`,
plus any golden using `Web.appHtml`/`WebView.appHtml` (enumerate with `git grep
-l`). Golden dirs named `*cli_program*`/`console_app*` may need renaming to match
their tests.

- [ ] Rewrite: `import Ipe.Tea.Tui`→`import Ipe.Tea.Terminal`, `Tui.app`→
  `Terminal.appScreen`, `Console.app`→`Terminal.appLines`. For `appHtml` goldens:
  convert to `Web.app`/`WebView.app` with the view wrapped in `Ui.html` if it was
  a raw-Html view, else just `.app`.
- [ ] Any `Tui.program` golden: convert to `Terminal.appScreen` if its view is
  `Element`; else drop the fixture + its test (raw-String full-screen is gone;
  `Ui.cells` in Task 10 is its replacement).
- [ ] Regenerate a byte-diff golden ONLY if it legitimately changes (module names
  are kernel-routed → emitted output is usually name-agnostic; a diff means a
  provenance string moved — confirm before accepting).
- [ ] `cargo nextest run -p ipe --test golden_* --profile ci` green.

## Task 8: Migrate first-party examples + e2e tests

**Files:** `examples/tui-counter/`, `examples/console-repl/`,
`src/ipe-cli/tests/tui_e2e.rs`, `golden_cli_program_seal.rs` and any
`console`/`tui` e2e, `examples/README.md`.

- [ ] Rename dirs `tui-counter`→`terminal-counter`, `console-repl`→
  `terminal-repl`; update `examples/README.md` (shape column + descriptions) and
  any manifest/path refs; rewrite each `Main.ipe` to the new entries.
- [ ] Update e2e test sources + assertion strings + test names embedding
  `tui`/`console`.
- [ ] Leave `examples/sky/**` mirrors alone.
- [ ] `IPE_E2E=1 cargo nextest run -p ipe --test tui_e2e … --profile ci` green.

## Task 9: Docs + ADR

**Files:** `docs/shapes/README.md`, `docs/shapes/tui.md`,
`docs/shapes/console.md`, `docs/shapes/web.md`/`webview.md` (drop `appHtml`
mentions), `AGENTS.md`, new `docs/adr/00NN-…`.

- [ ] Merge `tui.md`+`console.md` → `docs/shapes/terminal.md` (one guide, two
  entries); delete the two old files. Update `README.md` matrix (5→4 shapes) and
  the `web.md`/`webview.md` guides to drop `appHtml` and show `Ui.html` as the
  raw-HTML escape. Fix `AGENTS.md` shape matrix.
- [ ] Write `docs/adr/00NN-terminal-shape-consolidation.md` (next free number;
  capture decision + naming rationale from the spec). Delete
  `docs/architecture/tbd/shapes-model-design.md`.

## Task 10: New `Ui.cells` node

**Files:** `src/compiler/kernels/src/lib.rs`, `env.rs`,
`src/compiler/types/src/constrain.rs`, `lower.rs`, the `appScreen`/terminal
emitter, plus the medium-admissibility seal, plus `docs/shapes/terminal.md`.

- [ ] Add kernel `UiCells` → `d("Ui","cells", 1, Ui, "ui_cells_")`; register
  `cells` in the `Ui` member list in `env.rs`; add to the ALL-kernels list.
- [ ] Type scheme: `Ui.cells : List (List Char) -> Element msg`.
- [ ] Lower + emit: paint the char grid into the terminal in the `appScreen`
  emitter (rows joined by newlines, cursor-homed each frame — reuse the existing
  full-screen redraw path).
- [ ] **Admissibility:** `Ui.cells` is terminal-only. Under `Web.app`/
  `WebView.app` it must be rejected with a clear diagnostic (mirror how `Ui.html`
  admissibility is gated for the terminal side, if such a seal exists; if not,
  add a minimal shape-node seal). If adding a new diagnostic code, follow
  the root `AGENTS.md` "Registering a kernel" registration EXACTLY.
- [ ] A golden/e2e: an `appScreen` view using `Ui.cells` builds+runs; a `Web.app`
  view using `Ui.cells` is rejected.
- [ ] Document `Ui.cells` in `docs/shapes/terminal.md` with a runnable example.
- [ ] If this task proves larger than a clean increment, COMMIT Tasks 1–9 green
  first and report `Ui.cells` as remaining — the shape model must land coherent
  regardless.

## Verification (green gate)

- `cargo build --release -p ipe`; `cargo clippy --workspace` clean; `fmt` clean.
- `cargo nextest run --workspace --profile ci` green.
- `IPE_E2E=1` e2e for the two renamed examples build + run.
- `git grep -c 'Tui\.\(app\|program\)\|Console\.app\|\.appHtml\|Ipe\.Tea\.\(Tui\|Console\)'`
  over `src/` + `tests/` + first-party `examples/` returns **0** (only
  `examples/sky/**` mirrors may match).

## Risks

- **Display corruption** — verify by counts + clean build.
- **Golden byte-diffs** — a diff means a provenance string moved; confirm intent.
- **`Ui.cells` admissibility seal** — if no shape-node seal exists yet, keep the
  new one minimal and fail-closed; don't block the consolidation on it.

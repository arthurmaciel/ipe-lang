# RT-TUI findings

7 findings: 0 critical, 1 high, 1 medium, 5 low.

Audited: `src/runtime/rust/src/tui/{layout,app,focus,key,cell,diff,mod}.rs` (prior-audit
paths `runtime/src/ipe_runtime/tui/*` re-homed here). Prior HIGH `fr_total` usize-sum
overflow in `render_grid_tracked` is FIXED (per-track `MAX_CELLS` clamp + saturating
folds, `layout.rs:1809-1832`) — not re-filed. Prior `apply_padding` `total_w` clamp is
in place but only per-dimension (see 002).

## RT-TUI-001 · Unclamped `Ui.fillPortion` weights: portion-sum overflow → `str::repeat` panic / unbounded pad loop
- severity: high
- axis: soundness
- principle: P3 no integer-overflow abort / no panic from a well-typed program; P1 no unbounded resource
- location: `src/runtime/rust/src/tui/layout.rs:2230` (`distribute_row_fill`), `src/runtime/rust/src/tui/layout.rs:2100` (`distribute_col_fill`), sink `layout.rs:403` (`set_width` unclamped `" ".repeat(w - lw)`) and `layout.rs:2127` (row-pad `while` loop)
- reachability: `Ui.fillPortion : Int -> Length` lowers to `Length::Fill(i64)` (`src/runtime/rust/src/ui/helpers.rs:287`, `ui/element.rs:33`) with the raw program `Int`. A well-typed `Ui.row` with ≥3 fill children (or a fixed-height `Ui.column` with ≥2 height-fill children) carrying huge portions reaches both functions on every render.
- problem: `distribute_row_fill` computes `total_portion: i64 = specs…sum()` — a plain sum. `fill_spec` only floors each portion at 1; it never caps it. In debug the sum aborts on overflow; in release it wraps — e.g. portions `[i64::MAX, i64::MAX, 4]` wrap to `total_portion = 2`, then `share = remaining.saturating_mul(p as usize) / 2` ≈ `usize::MAX/2`, and `set_width(target)` executes `" ".repeat(~9.2e18)` → `str::repeat` capacity-overflow panic / OOM abort. `distribute_col_fill` has the identical wrap in its plain `usize` `portion_total` sum (line 2100); its comment (2108-2110) asserts "`portion_total >= portion(i)`, so the quotient stays <= leftover" — an invariant the wrap silently breaks — after which the `while child.block.lines.len() < share` loop pushes ~1e19 blank rows (OOM/hang). This is the same defect class as the fixed `fr_total` finding; the fix clamped `Grid.fr` tracks but not `fillPortion` weights.
- fix direction: clamp each portion at `MAX_CELLS` before summing and saturating-fold the totals (mirror the `fr_total` fix); optionally clamp `set_width`'s `w` at `MAX_CELLS` so no caller can feed `repeat` an unbounded count.
- prior: extends runtime-audit-verdict tui-layout HIGH `fr_total` (fixed for grid tracks only — the sibling class re-files as new)

## RT-TUI-002 · Padding/spacing area product unbounded — per-dimension clamp only, ~10-20 GB allocation reachable
- severity: medium
- axis: soundness
- principle: P1 no unbounded resource; P3 no OOM abort from a well-typed program
- location: `src/runtime/rust/src/tui/layout.rs:906-943` (`apply_padding` top/bottom rows × `total_w`), `layout.rs:755-767` (`vstack` gap rows × `stack_w`), `layout.rs:809-835` (`hstack` gap/filler), `layout.rs:2032-2045` (`apply_self_height` pad rows)
- reachability: `Ui.padding`/`Ui.paddingEach`/`Ui.spacing`/`Ui.height (vh …)` take arbitrary Ipê `Int`s; every render of a node carrying them reaches these loops.
- problem: each dimension is individually clamped at `MAX_CELLS` (100 000), but the allocated AREA is the product. `apply_padding` with `paddingEach { top = 3_000_000, bottom = 3_000_000, left = 1_600_000, right = 1_600_000 }` (px→cells at 80×24) yields `top = bottom = 100 000` rows each holding a `" ".repeat(100 000)` run → ~2×10^10 bytes ≈ 20 GB → OOM abort. `vstack` inter-child gaps have the same shape: `cells_y(spacing)` ≤ 100 000 gap rows × `stack_w` ≤ 100 000 per child pair. A well-typed program falls over; no remote party is needed (local DoS), hence medium.
- fix direction: bound the area, not the axes — e.g. clamp resolved pad/gap rows to a small multiple of the live terminal rows (Go's cap is terminal-proportional), or cap total `Block` cells.
- prior: runtime-audit-verdict tui-layout MEDIUM `apply_padding` `total_w` — PARTIALLY fixed (width clamp + saturating add landed; the rows×width product remains unbounded)

## RT-TUI-003 · Wide-char cursor: char-index vs display-column mismatch → cursor misplaced or invisible
- severity: low
- axis: correctness
- principle: P2 observable behaviour matches the reference (cursor renders over the edited cell)
- location: `src/runtime/rust/src/tui/layout.rs:1274-1286` (`cursor_line_col`, `col += 1` per char) vs `layout.rs:521-556` (`reverse_cell_at`, `acc += UnicodeWidthChar::width(ch)`)
- reachability: any focused text input/textarea whose buffer holds wide chars (CJK/emoji) — typed or model-seeded — on every render.
- problem: the producer counts chars, the consumer matches display columns. Buffer `"漢"` with cursor 1 → `cursor_line_col` yields col 1; `reverse_cell_at` accumulates 0→2, never hits `acc == 1`, and the past-end branch (`acc <= col` = 2 ≤ 1) is false — no cursor cell is reversed at all. With mixed-width text the cursor renders one or more cells off. Visual defect only; no panic.
- fix direction: make `cursor_line_col` accumulate display width (`UnicodeWidthChar`) for `col`, matching the consumer.
- prior: runtime-audit-verdict tui-layout LOW `cursor_line_col` — still present, unchanged

## RT-TUI-004 · `BorderSpec` style is an open `String` silently degrading to solid
- severity: low
- axis: soundness
- principle: make invalid states unrepresentable
- location: `src/runtime/rust/src/tui/layout.rs:195` (`type BorderSpec = (Option<(u8,u8,u8)>, String, (bool,bool,bool,bool))`), matched against a closed set in `border_glyphs` (`layout.rs:1958-1974`)
- reachability: any `Border.style`/raw `border-style` attribute value; constructor sites `walk_attrs` and `render_input`.
- problem: an arbitrary style string is representable and silently renders as solid instead of being parsed to a closed enum at the boundary (parse-don't-validate). Smell — no crash, no wrong output for the sanctioned styles.
- fix direction: a `BorderStyle` enum (`Solid | Dashed | Dotted | Rounded`) parsed once in `walk_attrs`.
- prior: runtime-audit-verdict tui-layout LOW `BorderSpec` — still present

## RT-TUI-005 · `Rendered.hits` index invariant is by-construction only
- severity: low
- axis: soundness
- principle: make invalid states unrepresentable
- location: `src/runtime/rust/src/tui/layout.rs:575-578` (`hits: Vec<(usize, usize, usize, usize, usize)>`), consumer `layout.rs:2408-2415`
- reachability: every render composes hits through `vstack`/`hstack`/`overlay_blocks`/`frame_rendered` without re-validating the focusable index.
- problem: the first tuple field must index `ctx.focusables`; nothing in the type enforces it. The sole consumer uses `.get_mut` (total — a bad index is a silent no-op, not a panic), so this is a smell: a drifted index would silently mis-position a focusable (wrong scroll/hit-test) rather than fail closed.
- fix direction: a named `Hit { focusable: FocusIdx, … }` struct, or fold hits into `Focusable` at push time.
- prior: runtime-audit-verdict tui-layout LOW `Rendered.hits` — still present

## RT-TUI-006 · Shift-Tab heuristic `kind == "other" && value.contains('Z')`
- severity: low
- axis: correctness
- principle: make invalid states unrepresentable (stringly-typed key dispatch)
- location: `src/runtime/rust/src/tui/app.rs:529`; root cause `src/runtime/rust/src/tui/key.rs` (`TuiKey.kind`/`value` open `String`s, no `ShiftTab` variant)
- reachability: every non-mouse key event in the `tui_app_ui` loop.
- problem: real Shift-Tab (`CSI Z`) decodes as `other` with a value containing `'Z'`, but so does e.g. an unrecognised `ESC O Z` (SS3) sequence — those misfire as focus-back navigation. The whole `TuiKey` dispatch is string-matched (`kind == "mouse"`, `== "ctrl"`, …), so an invalid kind is representable and silently a no-op.
- fix direction: decode `CSI Z` to an explicit `shift-tab` kind in `decode_key` (aligned with a `TuiKeyKind` enum).
- prior: runtime-audit-verdict tui-rest LOW Shift-Tab heuristic — still present at the same line

## RT-TUI-007 · `cell::Grid`/`diff` are dead in the paint path while their docs claim the loop consumes them
- severity: low
- axis: completeness
- principle: P5 a claimed capability that no-ops; docs state what IS
- location: `src/runtime/rust/src/tui/diff.rs:3-6` ("The TEA loop turns these into ANSI cursor-moves + styled writes via crossterm"), `src/runtime/rust/src/tui/cell.rs:58-101` (`Grid`), actual paint `src/runtime/rust/src/tui/app.rs:103-108` (`CLEAR_HOME` + full-frame rewrite every event)
- reachability: n/a — the point is non-reachability: no module outside `tui/{cell,diff}.rs` constructs a `Grid` or calls `tui::diff::diff` (the `diff` hits elsewhere are `live`/`dom`/`wasm`'s own diffs).
- problem: the diff-based incremental repaint the module docs describe does not exist; `tui_run`/`tui_app_ui` clear-and-repaint the whole frame per event. `Cell`/`Grid`/`diff` are tested but unused. Harmless today (full repaint is correct, just flicker/bandwidth-heavy), but the doc asserts an unimplemented pipeline, and the prior `Cell::width` open-`u8` invariant concern is moot only because the type has no consumer — the moment the diff path is wired, that invariant gap returns.
- fix direction: either wire the diff path (and close `Cell::width` behind a constructor) or correct the module docs to describe the full-repaint reality.
- prior: subsumes runtime-audit-verdict tui-rest MEDIUM `Cell::width` (downgraded: no consumer ⇒ unreachable) — doc/deadcode aspect new

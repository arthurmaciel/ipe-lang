# T3 — Bound untrusted recursion / allocation

Theme-key: `t3-bound-recursion`. Findings: **CO-FRONT-001**, **RT-UI-001**,
**RT-TUI-001**, **RT-TUI-002**.

## Theme root cause

Four walkers/allocators whose depth or size is a function of *program input*
(source length, Model-derived tree depth, program `Int` weights) run
**native-recursively** or with a **plain unclamped arithmetic fold**, in
crates where the sibling code has *already* recognised and closed exactly this
class:

- `parse/src/parser.rs::MAX_DEPTH` guards operator *nesting* but not chain
  *length* — the flat chain is re-materialised as native recursion one stage
  later in `canon::climb_binops`.
- `html.rs::MAX_HTML_DEPTH` (1024) caps the render/id-stamp descent, and
  `dom/dispatch.rs::walk` was made iterative for the identical "would overflow
  the thread stack and ABORT the whole process" reason — but the two remaining
  walkers in the *same* Live data path (`ui/render.rs::render_element`,
  `dom/diff.rs::diff_node`) recurse uncapped.
- `tui/layout.rs::render_grid_tracked` was fixed to clamp each `Grid.fr` weight
  at `MAX_CELLS` and saturating-fold the total — but the sibling `fillPortion`
  distribution (`fill_spec` → `distribute_row_fill`/`distribute_col_fill`) sums
  raw `i64`/`usize` weights that wrap, and `apply_padding` clamps each *axis* at
  `MAX_CELLS` while allocating their unbounded *product*.

The structural property to establish, per class:

- **P3 no-input-stack-overflow**: every tree/chain walk whose depth scales with
  input is *iterative over an explicit heap work-stack* (unbounded, no thread
  stack) **or** depth-capped and truncated. No walker's native recursion depth
  is a function of input.
- **P3 no-input-overflow / P1 no-unbounded-alloc**: every allocation count
  derived from a program `Int` is bounded by `MAX_CELLS` at *construction of the
  weight*, folded with `saturating_add`, and every `str::repeat`/blank-row loop
  is bounded by a `MAX_CELLS`-clamped count. The bound covers the *area*
  (product), not only each axis.

These are the in-tree precedents the fixes mirror; no new mechanism is invented.

---

## CO-FRONT-001 · `climb_binops` per-operator native recursion → stack overflow

### Root cause
`parse/src/parser.rs::parse_expr` (parser.rs:989-1009) gathers a binary-operator
chain in a **flat** `while` loop into `ops: Vec<(Expr, Located<Symbol>)>`;
nesting depth stays 1, so the `depth > MAX_DEPTH` guard (parser.rs:990) never
trips regardless of chain length. `canon::resolve::canonicalise_binops`
(resolve.rs:2946) hands the flat chain to `climb_binops` (resolve.rs:2983).
For a right-associative operator (`++ :: && || <|`, all `Assoc::Right` in
`op_precedence`, resolve.rs:2920-2928), the recursive-descent core sets
`next_min = prec` (resolve.rs:3007) and **recurses** at resolve.rs:3009 with the
same min-prec, consuming the next equal-precedence operator one native frame
deeper. Native call depth == chain length N; ~300k operators overflow the thread
stack → SIGSEGV/abort, not a coded diagnostic. `ipe` never exits 0, so this is
not a SEAL/cargo-contract breach — it is the P3 "no stack overflow on input"
invariant the parser module doc explicitly advertises, bypassed because the
recursion moved one stage past the guard.

### Design — iterative precedence climb (root-cause), not a length cap
The audit offers two directions (cap chain length in `parse_expr`, or rewrite
`climb_binops` iteratively). **Cap-in-parser is a band-aid**: a long right-assoc
chain is a *legal* Ipê program; rejecting it with `NestingTooDeep` turns a
soundness fix into "reject valid input", the inverse of parse-don't-validate
(which forbids *accepting* invalid states, never *rejecting* valid ones). The
structural fix is to make the climb's stack usage O(1) in chain length, matching
`target_gate::check_expr` (target_gate.rs:47-51, "explicit heap work-stack …
native recursion would risk the thread stack") — the crate's own established
pattern.

Rewrite `climb_binops` as an explicit-stack precedence climb. The recursion is
right-linear (it only ever recurses in the right-operand tail before the final
`combine_binop` fold), so it converts to a loop with a `Vec` of pending
`(left, op, prec, assoc)` frames — the classic shunting-yard operator stack:

```rust
// resolve.rs — replaces the recursive climb_binops body.
// Explicit operator stack; call-stack depth is O(1) in chain length.
// Mirrors target_gate::check_expr's heap-work-stack discipline.
fn climb_binops(
    left0: canon::Expr,
    operands: &mut VecDeque<canon::Expr>,
    ops: &mut VecDeque<(Located<Symbol>, i32, Assoc)>,
    basics: Symbol,
    interner: &mut Interner,
) -> DResult<canon::Expr> {
    // Pending left operands + the operator awaiting their right subtree.
    let mut pending: Vec<(canon::Expr, Located<Symbol>, i32)> = Vec::new();
    let mut left = left0;
    while let Some(&(op, prec, assoc)) = ops.front() {
        // Reduce any pending op that binds at least as tightly as `op`
        // (left/non-assoc: strictly tighter; right-assoc keeps equal on the
        // stack so it nests rightward) before shifting `op`.
        while let Some(&(_, _, top_prec)) = pending.last() {
            let reduce = match assoc {
                Assoc::Left | Assoc::None => top_prec >= prec,
                Assoc::Right => top_prec > prec,
            };
            if !reduce { break; }
            let (l, top_op, _) = pending.pop().expect("checked by last()");
            left = combine_binop(l, top_op, left, basics, interner)?;
        }
        ops.pop_front();
        let next = operands.pop_front().ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_canon::climb_binops",
            detail: "operator without a right operand".to_owned(),
        })?;
        pending.push((left, op, prec));
        left = next;
    }
    // Drain remaining pending ops right-to-left.
    while let Some((l, op, _)) = pending.pop() {
        left = combine_binop(l, op, left, basics, interner)?;
    }
    Ok(left)
}
```

`min_prec` parameter is dropped (the top-level call passed 0). `.expect()` is
forbidden by the clippy deny-set — the real patch uses `if let Some(frame) =
pending.pop()` with a `CompilerBug` on the impossible `None`, matching the
existing `ok_or_else` idiom in the same function (the pseudocode above elides it
for brevity).

**Correctness contract**: output must be **byte-identical** to the current
recursive climb for every chain (this is a port of `Sky.Canonicalise.Expression`
— parity-gated). The reduce predicate above reproduces the recursive semantics:
left/non-assoc restrict the right subtree to strictly-higher precedence
(`next_min = prec + 1` ⇒ reduce when `top_prec >= prec`); right-assoc admits
equal precedence rightward (`next_min = prec` ⇒ reduce only when `top_prec >
prec`, leaving equal-prec ops on the stack to nest right). No divergence-ledger
entry — this is an internal representation change, observably identical.

### Impl plan
1. `src/compiler/canon/src/resolve.rs` — replace the `climb_binops` body with
   the explicit-stack form above; drop the `min_prec` param; update the single
   caller (`canonicalise_binops`, resolve.rs:2976) to
   `climb_binops(left, &mut operands, &mut ops, basics, interner)`. Update the
   doc-comment (resolve.rs:2980-2982) to state the explicit-stack invariant
   ("call-stack depth is O(1) in chain length") — WHAT it guarantees, no
   archaeology.
2. **Parity regression** — `src/ipe-cli/tests/golden_binops.rs` (existing): add
   cases that exercise mixed precedence AND associativity so the reduce-order is
   pinned (`a ++ b ++ c`, `a && b || c && d`, `f <| g <| x`, `a + b * c ++ d`),
   asserting the emitted Rust matches the pre-change golden byte-for-byte. This
   is the guard that the iterative rewrite is observationally identical.
3. **Deep-input soundness regression** — `src/ipe-cli/tests/negative_suite.rs`
   is compile-reject-only; a deep *valid* chain compiles, so it does NOT belong
   there. Add a new integration test (e.g.
   `src/ipe-cli/tests/deep_binop_chain.rs`) that programmatically builds a
   `main = "a" ++ "a" ++ … ++ "a"` source with ~300k operators, runs the
   compiler pipeline, and asserts it returns `Ok`/a normal `Err` (no crash) —
   pre-change this test SIGSEGVs. Run the parse+canon stage only (no cargo
   build) to keep it fast; gate behind the same runtime-locate skip the negative
   suite uses. Cap the operator count at what completes in < a few seconds so
   the timeout-wrapped CI lane stays green.

### Risk / blast radius
- `climb_binops` is on **every** binary-operator expression in every program —
  the parity goldens (step 2) plus the full examples sweep are the gate; any
  re-association bug surfaces as a golden byte-diff or a sweep behavioural break.
- Re-gate: `golden_binops.rs`, the full examples sweep, and the new deep-chain
  test. No SEAL-E2E impact (codegen shape unchanged when the tree is identical).

---

## RT-UI-001 · `render_element` / `diff_node` uncapped recursion → process abort

### Root cause
`ui/render.rs::render_element` → `render_node_as` recurses via
`kids.into_iter().map(render_element)` (render.rs:501) once per nesting level,
and `dom/diff.rs::diff_node` recurses at diff.rs:134 — both with **no depth
cap**, in the same Live/Webview/wasm commit+diff data path where sibling walkers
`html.rs::render_into_ctx`/`assign_ipe_ids_depth` already cap at
`MAX_HTML_DEPTH = 1024` (html.rs:193, 694) and `dom/dispatch.rs::walk` is
iterative. An attacker-length Model list folded into wrapper elements
(`List.foldl (\_ acc -> Ui.el [] acc) base xs`) builds a tree deeper than the
native stack with O(1) app-side frames; the runtime walker aborts the whole
process (uncatchable stack exhaustion) on render/diff.

### Design — depth cap, not iterative (genuine one-liner)
The audit and brief agree this is a **one-liner fix**, not an iterative rewrite:
`render_element`/`diff_node` build/consume owned trees with per-node work
(attribute folding, patch emission) that does not thread cleanly through an
explicit stack the way `dispatch.rs::walk`'s read-only traversal did. Mirror the
established `MAX_HTML_DEPTH` precedent: thread a `depth: usize`, drop (truncate)
descent at the cap. A truncated render/diff is strictly better than a process
abort (html.rs:166-168 states exactly this tradeoff), and no legitimate UI nests
near 1024. Reuse the existing `MAX_HTML_DEPTH` constant (make it
`pub(crate)` in `html.rs`) so all four walkers share ONE ceiling — a second
private constant would drift.

Signatures (private depth-carrying inner fn, public wrapper unchanged — same
shape as `assign_ipe_ids`/`assign_ipe_ids_depth`):

```rust
// ui/render.rs
fn render_element<M: Clone>(elem: Element<M>) -> Html<M> {
    render_element_depth(elem, 0)
}
fn render_element_depth<M: Clone>(elem: Element<M>, depth: usize) -> Html<M> {
    if depth >= crate::html::MAX_HTML_DEPTH {
        return Html::HText(String::new());   // truncate: drop the deep subtree
    }
    // … existing arms; render_node_as threads depth.saturating_add(1) to kids …
}

// dom/diff.rs
fn diff_node<M>(old: &Html<M>, new: &Html<M>, out: &mut Vec<Patch>) {
    diff_node_depth(old, new, out, 0)
}
fn diff_node_depth<M>(old: &Html<M>, new: &Html<M>, out: &mut Vec<Patch>, depth: usize) {
    if depth >= crate::html::MAX_HTML_DEPTH { return; } // stop descending
    // … existing body; the per-position recursion at :134 calls
    //   diff_node_depth(oc, nc, out, depth.saturating_add(1)) …
}
```

`render_node_as` (render.rs:491) and `ui_layout` (render.rs:517, calls
`render_element` at :524) also thread depth: `ui_layout` starts the root at
`render_element_depth(elem, 0)` unchanged via the wrapper; `render_node_as`
takes a `depth` param and passes `depth.saturating_add(1)` into the
`kids.into_iter().map(...)`. `render_nearby_overlays` (siblings, already
sibling-capped per the finding) likewise thread depth.

### Impl plan
1. `src/runtime/rust/src/html.rs` — change `const MAX_HTML_DEPTH` to
   `pub(crate) const MAX_HTML_DEPTH` (html.rs:169). Doc-comment already states
   the rationale; extend it to name render/diff as co-consumers (WHAT, not
   archaeology).
2. `src/runtime/rust/src/ui/render.rs` — add `render_element_depth` +
   `depth` param on `render_node_as`; truncate at the cap. `render_element` and
   `ui_layout` become thin depth-0 entry points.
3. `src/runtime/rust/src/dom/diff.rs` — add `diff_node_depth` + `depth` param;
   return at the cap; thread `depth.saturating_add(1)` at diff.rs:134.
4. **Regression** — in the `#[cfg(test)] mod tests` of `ui/render.rs` and
   `dom/diff.rs`: build an `Element` / `Html` nested ~5000 deep (well past 1024,
   well under the native stack limit so the test itself never overflows) via a
   loop, call `render_element` / `diff_node`, assert it returns without abort and
   that descent stopped at the cap (e.g. the rendered depth ≤ `MAX_HTML_DEPTH`,
   or a sentinel deep leaf is absent from the output). Mirror the existing
   `html.rs` deep-tree cap test if one exists (`MAX_HTML_DEPTH` already has
   coverage there — reuse its construction helper).

### Risk / blast radius
- Runs on every Live commit/update; the cap at 1024 is far above any real UI, so
  no legitimate view changes output. Re-gate:
  `cargo nextest -p sky-runtime-rust --features full` (the `live::*` surface),
  the render/diff unit tests, and the examples sweep (Live examples render).
- Truncation is a *behavioural* change only for pathological trees (which
  currently crash) — no divergence-ledger entry needed beyond the existing
  B14 runtime-hardening note; extend B14's list to mention render/diff depth
  caps for completeness.

---

## RT-TUI-001 · `fillPortion` weight-sum overflow → `str::repeat` OOM/panic

### Root cause
`FillSpec` = `(i64, …)` (layout.rs:263). `fill_spec` (layout.rs:268-279) floors
each portion at 1 (`(*p).max(1)`, layout.rs:270) but **never caps** it — the raw
program `Int` from `Ui.fillPortion` flows through. `distribute_row_fill`
computes `total_portion: i64 = specs…sum()` — a **plain sum** (layout.rs:2230);
`distribute_col_fill` sums `usize` portions plainly (layout.rs:2100). Portions
`[i64::MAX, i64::MAX, 4]` wrap `total_portion` to a tiny value; then
`remaining.saturating_mul(*p as usize) / total_portion` (layout.rs:2255)
produces ≈ `usize::MAX/2`, and `Block::set_width` executes
`" ".repeat(target)` (layout.rs:404) → capacity-overflow panic / OOM. The col
path's `while child.block.lines.len() < share` loop (layout.rs:2127) pushes ~1e19
blank rows. This is the *identical* class the `fr_total` fix already closed for
grid tracks (layout.rs:1809-1832) — clamp-each-weight + saturating-fold — applied
to grid but not to `fillPortion`.

### Design — clamp at the weight boundary + saturating fold (mirror `fr_total`)
Fix at the **construction** point so both consumers inherit the bound (one
place, not two): clamp the portion at `MAX_CELLS` inside `fill_spec` — the parse-
don't-validate boundary where the untyped program `Int` becomes a `FillSpec`.
Then belt-and-braces the two consumers with saturating folds and a `set_width`
clamp so no future caller can reintroduce the wrap.

```rust
// layout.rs — fill_spec: cap the portion at MAX_CELLS at construction.
Length::Fill(p) => Some(((*p).clamp(1, MAX_CELLS as i64), None, None)),

// distribute_row_fill: saturating fold, each portion already ≤ MAX_CELLS.
let total_portion: usize = specs
    .iter()
    .filter_map(|s| s.map(|(p, _, _)| (p.max(0) as usize).min(MAX_CELLS)))
    .fold(0usize, usize::saturating_add)
    .max(1);
// … share = remaining.saturating_mul(p_clamped) / total_portion …  (usize now)

// distribute_col_fill: portion() already .min(MAX_CELLS); portion_total via
//   .fold(0usize, usize::saturating_add).max(1) instead of .sum().

// Block::set_width: clamp the repeat count so no caller can feed an
//   unbounded count (defence in depth — the same ceiling every repeat uses).
let w = w.min(MAX_CELLS);
```

Clamping the portion at `MAX_CELLS` cannot change any legitimate layout: a
terminal is at most `MAX_CELLS` (100k) cells wide, so any portion ≥ that already
claims the whole leftover — the ratio is preserved. `total_portion` becomes
`usize` throughout (the `i64` was only ever a sum of non-negative clamped
weights). The `set_width` clamp is the same guard `apply_padding`/`pad_run`
already apply (layout.rs:919-921).

### Impl plan
1. `src/runtime/rust/src/tui/layout.rs::fill_spec` (layout.rs:270) — clamp the
   `Length::Fill` portion at `MAX_CELLS`.
2. `distribute_row_fill` (layout.rs:2230) — `total_portion` → clamped
   saturating `usize` fold; drop the `as usize`/`as i64` casts now that portions
   are `usize`-bounded. Update the comment to state the invariant (portion ≤
   `MAX_CELLS`, sum saturates).
3. `distribute_col_fill` (layout.rs:2100) — `portion_total` → saturating fold;
   the `portion` closure (layout.rs:2087) gains `.min(MAX_CELLS)`. Correct the
   stale comment at layout.rs:2108-2110 (it asserts an invariant the old wrap
   broke).
4. `Block::set_width` (layout.rs:355) — clamp `w` at `MAX_CELLS` on entry.
5. **Regression** — `tui/layout.rs` `mod tests` (layout.rs:2428): a `Ui.row`
   with three `Ui.fillPortion` children of portions
   `[i64::MAX, i64::MAX, 4]` (or the `Length::Fill(i64::MAX)` specs directly),
   plus a fixed-height `Ui.column` with two `i64::MAX` height-fill children,
   rendered through the layout entry point; assert it returns a bounded `Block`
   (no panic, total cells ≤ a small multiple of the canvas) rather than
   allocating. Pre-change this panics/OOMs.

### Risk / blast radius
- TUI-only, local-author blast radius. Re-gate the `tui/layout.rs` tests and any
  TUI example in the sweep. No parity concern — the Tui surface is not
  byte-gated against Go (layout.rs:283-285 already notes a sanctioned Tui
  divergence); record the clamp under the existing B14 runtime-hardening ledger
  entry alongside RT-UI-001 / RT-TUI-002.

---

## RT-TUI-002 · Padding/spacing *area* product unbounded → ~10-20 GB OOM

### Root cause
`apply_padding` (layout.rs:906-951) clamps each dimension individually at
`MAX_CELLS` — `top`/`bottom` via `cells_y` (≤ 100k), `total_w` `.min(MAX_CELLS)`
(≤ 100k) — but allocates their **product**: `for _ in 0..top { push(vec![
pad_run(total_w)]) }` (layout.rs:926-928) builds up to 100k rows each holding a
`" ".repeat(100k)` = 100 KB run → 100k × 100 KB ≈ 10^10 bytes ≈ 10-20 GB → OOM.
The same shape recurs in `vstack` gap rows × `stack_w` (layout.rs:755-767),
`hstack` gap/filler (layout.rs:809-835), and `apply_self_height` pad rows
(layout.rs:2032-2045). A well-typed program with
`paddingEach { top = 3_000_000, bottom = 3_000_000, left = 1_600_000, right =
1_600_000 }` falls over locally.

### Design — bound the AREA, terminal-proportional (Go's model)
Per-axis clamps are insufficient by construction; the invariant to establish is
that **resolved pad/gap row counts are bounded by a small multiple of the live
terminal rows**, not by the absolute `MAX_CELLS` axis cap. Go caps padding
terminal-proportionally; mirror that. The `Canvas` already carries the logical
row/col count — clamp resolved pad rows to `canvas.rows` (× a small slack factor,
e.g. 4, to allow scroll-region padding without permitting a 100k-row block on a
24-row terminal). A dedicated helper keeps the four sites consistent:

```rust
// layout.rs — one place all pad/gap row counts route through.
// A pad/gap block taller than a few screens serves no display purpose and its
// area (rows × width) is the OOM vector; cap rows terminal-proportionally.
const PAD_ROW_SLACK: usize = 4;
fn clamp_pad_rows(rows: usize, canvas: Canvas) -> usize {
    rows.min(canvas.rows.saturating_mul(PAD_ROW_SLACK).max(PAD_ROW_SLACK))
}
```

Apply at each pad-row producer: `apply_padding` `top`/`bottom`
(layout.rs:907-908), `vstack` gap-row count, `hstack` filler-row count,
`apply_self_height` pad-row count. The per-run width clamp (`total_w.min(
MAX_CELLS)`) stays; the row clamp caps the product at
`(canvas.rows × 4) × MAX_CELLS` ≈ (for a 24-row terminal) 96 × 100k ≈ 10 MB
worst case — bounded and negligible. Alternatively (simpler, coarser) cap total
`Block` cells at a fixed ceiling; the terminal-proportional form is preferred
because it matches Go and never truncates a legitimate on-screen layout.

### Impl plan
1. `src/runtime/rust/src/tui/layout.rs` — add `PAD_ROW_SLACK` +
   `clamp_pad_rows(rows, canvas)`.
2. `apply_padding` (layout.rs:907-908) — `top = clamp_pad_rows(cells_y(...),
   canvas)`, same for `bottom`.
3. `vstack` (layout.rs:755-767), `hstack` (layout.rs:809-835),
   `apply_self_height` (layout.rs:2032-2045) — route each pad/gap ROW count
   through `clamp_pad_rows`.
4. **Regression** — `tui/layout.rs` `mod tests`: a node with
   `paddingEach { top = 3_000_000, bottom = 3_000_000, left = 1_600_000,
   right = 1_600_000 }` on an 80×24 canvas; assert the rendered `Block` line
   count ≤ `canvas.rows * PAD_ROW_SLACK` and total cell count is bounded (no
   multi-GB allocation). Add a `Ui.spacing 3_000_000` column case for the
   `vstack` gap path.

### Risk / blast radius
- TUI-only, local blast radius. Truncating pad rows beyond a few screens changes
  output only for pathological inputs (which currently OOM). Re-gate the
  `tui/layout.rs` tests + TUI sweep examples. Record under B14 with RT-TUI-001.

---

## Consolidated risk / re-gate summary

| Finding | Files | Primary gate | Secondary |
|---|---|---|---|
| CO-FRONT-001 | `canon/src/resolve.rs` | `golden_binops.rs` byte-parity | new deep-chain test; examples sweep |
| RT-UI-001 | `runtime/…/{html,ui/render,dom/diff}.rs` | render/diff unit tests | `-p sky-runtime-rust --features full`; sweep |
| RT-TUI-001 | `runtime/…/tui/layout.rs` | `tui/layout.rs` fill tests | TUI sweep |
| RT-TUI-002 | `runtime/…/tui/layout.rs` | `tui/layout.rs` padding tests | TUI sweep |

Divergence ledger: extend **B14** (`docs/divergences-from-sky.md:231`,
"Runtime-fork behavioral hardening") to enumerate the render/diff depth cap and
the TUI area caps — these are hardening beyond the reference runtime, the exact
class B14 already records. CO-FRONT-001 is internal-representation-only (byte-
identical output) → no ledger entry.

---

## Proposed backlog entries

```json
{"id": "TBD", "priority": "high", "phase": "principles-audit-fix", "task": "CO-FRONT-001: rewrite canon climb_binops as an explicit-stack precedence climb so call-stack depth is O(1) in operator-chain length (no stack overflow on a long right-associative chain)", "notes": "resolve.rs:2983 climb_binops recurses per right-assoc operator; parser MAX_DEPTH guards nesting not chain length. Mirror target_gate::check_expr's heap work-stack. Output MUST be byte-identical (Sky.Canonicalise parity). Tests: extend golden_binops.rs with mixed prec+assoc cases (byte-parity guard); add src/ipe-cli/tests/deep_binop_chain.rs (~300k-op chain compiles without SIGSEGV). NOT a length cap (rejects valid input).", "spec": "docs/audit/2026-07-17-principles-audit/specs/t3-bound-recursion.md", "blocked_by": [], "status": "pending"}
{"id": "TBD", "priority": "medium", "phase": "principles-audit-fix", "task": "RT-UI-001: depth-cap render_element and diff_node at MAX_HTML_DEPTH so an attacker-deep Model-derived Ui tree truncates instead of aborting the process", "notes": "render.rs:501 (render_element/render_node_as) and diff.rs:134 (diff_node) recurse uncapped; sibling html.rs render_into_ctx/assign_ipe_ids_depth already cap at MAX_HTML_DEPTH=1024 and dispatch.rs::walk is iterative. Make MAX_HTML_DEPTH pub(crate); thread depth + truncate. Tests: deep (~5000) Element/Html in render.rs/diff.rs mod tests return without abort, descent stops at cap. Extend divergences B14.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t3-bound-recursion.md", "blocked_by": [], "status": "pending"}
{"id": "TBD", "priority": "high", "phase": "principles-audit-fix", "task": "RT-TUI-001: clamp Ui.fillPortion weights at MAX_CELLS in fill_spec and saturating-fold the portion totals in distribute_row_fill/distribute_col_fill; clamp Block::set_width repeat count", "notes": "FillSpec i64 portion (layout.rs:263/270) uncapped; total_portion plain sum (layout.rs:2230/2100) wraps -> set_width str::repeat(~9e18) OOM/panic and col-fill ~1e19-row loop. Mirror the fr_total fix (layout.rs:1809-1832): clamp-each-weight + saturating fold. Fix at fill_spec construction so both consumers inherit. Test: Ui.row / fixed-height Ui.column with i64::MAX portions renders bounded (no panic). Record under B14.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t3-bound-recursion.md", "blocked_by": [], "status": "pending"}
{"id": "TBD", "priority": "medium", "phase": "principles-audit-fix", "task": "RT-TUI-002: bound TUI padding/spacing AREA terminal-proportionally (clamp_pad_rows) so per-axis-clamped dims cannot allocate their ~10-20 GB product", "notes": "apply_padding (layout.rs:906) clamps top/bottom and total_w each at MAX_CELLS but allocates rows*width; same shape in vstack/hstack/apply_self_height. Add PAD_ROW_SLACK + clamp_pad_rows(rows, canvas) = rows.min(canvas.rows*4); route every pad/gap row count through it. Test: paddingEach {top/bottom=3_000_000, left/right=1_600_000} on 80x24 renders bounded Block (lines <= canvas.rows*PAD_ROW_SLACK). Record under B14.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t3-bound-recursion.md", "blocked_by": [], "status": "pending"}
```

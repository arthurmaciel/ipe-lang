//! The deterministic renderer: lays a [`crate::doc::Doc`] out to bytes matching
//! `rustfmt --edition 2024 --style-edition 2024` for the construct shapes the
//! emitter produces. It is NOT a general rustfmt — only the emitter's shapes.
//!
//! Two layout mechanisms:
//!
//! * [`Doc::Group`] is flat-if-fits-else-break: if the group's flat rendering
//!   fits the remaining width AND it contains no [`Doc::HardLine`], every
//!   [`Doc::Line`] / [`Doc::Softline`] in it lays out flat (a space / nothing);
//!   otherwise every soft break in it becomes a newline plus the current indent.
//!   A [`Doc::HardLine`] always breaks and forces every enclosing group broken —
//!   that keeps a statement block (`{ let x = …; x }`) from ever inlining, while
//!   an inline structure (`(a, b)`, `if c {1} else {2}`, carrying only soft
//!   `Line`s) flattens when it fits.
//!
//! * [`Doc::Chain`] is rustfmt's binary-operator-chain layout, derived
//!   empirically from real rustfmt bytes and proven byte-exact against the
//!   golden corpus (`param_patterns`, `probe_tinytail`, `probe_callbreak`,
//!   `order2`): line-1 packs the maximal left-nested prefix that fits the width;
//!   the first operator that would overflow breaks, and from there EVERY
//!   subsequent operator breaks one-per-line to a single shared indent
//!   (chain-begin-line indent + 4), non-accumulating — with the sole exception
//!   that an operator following a multiline operand glues to that operand's
//!   closing-line column while the chain has not yet broken.

// The renderer's public entry (`render`) is called by the P1 project.rs cutover;
// until then only the P0 tests drive it.
#![allow(dead_code, reason = "consumed by the P1 project.rs native emit path")]

use crate::doc::{ChainOperand, Doc};

/// Rendering configuration. Mirrors the `rustfmt` knobs the golden harness pins.
#[derive(Debug, Clone, Copy)]
pub struct RenderConfig {
    /// The maximum line width (`rustfmt` `max_width`, default 100).
    pub max_width: usize,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self { max_width: 100 }
    }
}

/// The number of columns a chain's broken operators indent past the chain's
/// begin-line indent. `rustfmt` uses one block-indent step (4).
const CHAIN_BREAK_INDENT: usize = 4;

/// Render `doc` to a string starting at column `col` with block indent `indent`.
/// `col` is where the document's first character will be placed (used for the
/// fit test); `indent` is the number of leading spaces a broken line receives.
pub fn render(doc: &Doc, cfg: RenderConfig) -> String {
    let mut out = String::new();
    render_at(doc, cfg, 0, 0, false, &mut out);
    out
}

/// The current column after the last newline in `out`, i.e. how many characters
/// sit on the line so far. This is the live cursor the fit tests measure.
fn current_col(out: &str) -> usize {
    out.rfind('\n').map_or(out.len(), |nl| out.len() - nl - 1)
}

/// The column the next character will land on. Once anything has been written to
/// `out`, the live cursor ([`current_col`]) is authoritative — including after a
/// break reset it to the fresh line's indent. The seed `col` is used only for an
/// empty buffer (the render root, or a `fits` scratch measured from `start_col`),
/// where there is no cursor yet. Taking `max` here would leak a pre-newline
/// column past a break, so we deliberately prefer the live cursor.
fn eff_col(out: &str, col: usize) -> usize {
    if out.is_empty() {
        col
    } else {
        current_col(out)
    }
}

/// Render `doc` into `out`. `indent` is the block indent for newlines within
/// `doc`; `flat` is `true` when the nearest enclosing [`Doc::Group`] chose flat,
/// which turns a soft [`Doc::Line`] into a space and a [`Doc::Softline`] into
/// nothing. A [`Doc::HardLine`] ignores `flat` — it always breaks (and its
/// presence already forced every enclosing group broken, so it is never reached
/// with `flat == true` in practice, but it breaks unconditionally regardless).
/// `col` is the column the first char lands on.
fn render_at(
    doc: &Doc,
    cfg: RenderConfig,
    indent: usize,
    col: usize,
    flat: bool,
    out: &mut String,
) {
    match doc {
        Doc::Text(s) => out.push_str(s),
        Doc::Line => {
            if flat {
                out.push(' ');
            } else {
                out.push('\n');
                push_indent(indent, out);
            }
        }
        Doc::Softline => {
            if !flat {
                out.push('\n');
                push_indent(indent, out);
            }
        }
        Doc::HardLine => {
            out.push('\n');
            push_indent(indent, out);
        }
        Doc::IfBroken(s) => {
            // Renders only when the nearest enclosing group broke (`!flat`) —
            // rustfmt's trailing comma on a broken delimited list. Nothing when
            // flat, so the flat form (and the `fits` measurement of it) carries no
            // trailing comma.
            if !flat {
                out.push_str(s);
            }
        }
        Doc::Concat(docs) => {
            for d in docs {
                let c = eff_col(out, col);
                render_at(d, cfg, indent, c, flat, out);
            }
        }
        Doc::Nest(n, inner) => {
            render_at(inner, cfg, indent + n, col, flat, out);
        }
        Doc::Group(inner) => {
            let start_col = eff_col(out, col);
            // A group flattens only if it fits AND carries no `HardLine` (a
            // statement block's `HardLine`s keep the group broken even when its
            // flat width would fit). Its own soft `Line`s are what flatten or
            // break as a unit — that is the standard Wadler `group`.
            let group_flat =
                flat || (!has_hard_break(inner) && fits(inner, cfg, start_col, indent));
            render_at(inner, cfg, indent, start_col, group_flat, out);
        }
        Doc::BraceBody(body) => {
            let start_col = eff_col(out, col);
            // The body braces only when it does not fit flat here — `rustfmt` re-
            // tests the closure/arm body against the width on its own line, so this
            // decision is independent of any enclosing group. A `HardLine` inside
            // the body (a statement-block body) can never fit, so it always braces.
            let body_flat = flat || (!has_hard_break(body) && fits(body, cfg, start_col, indent));
            if body_flat {
                render_at(body, cfg, indent, start_col, true, out);
            } else {
                out.push('{');
                render_at(
                    &Doc::Nest(4, Box::new(Doc::HardLine)),
                    cfg,
                    indent,
                    start_col,
                    false,
                    out,
                );
                let c = current_col(out);
                render_at(body, cfg, indent + 4, c, false, out);
                out.push('\n');
                push_indent(indent, out);
                out.push('}');
            }
        }
        Doc::MatchArmTail { body, control } => {
            let start_col = eff_col(out, col);
            // The arm body stays inline (with a trailing comma) when it fits on the
            // arm's line. A `HardLine` body (a prelude block arm) never fits, so it
            // always takes the broken path.
            let body_flat = !has_hard_break(body) && fits(body, cfg, start_col, indent);
            if body_flat {
                render_at(body, cfg, indent, start_col, true, out);
                out.push(',');
            } else if *control {
                // A control/paren body that breaks is wrapped in synthesized braces
                // and the trailing comma is dropped — `rustfmt`'s arm-brace form.
                out.push('{');
                render_at(
                    &Doc::Nest(4, Box::new(Doc::HardLine)),
                    cfg,
                    indent,
                    start_col,
                    false,
                    out,
                );
                let c = current_col(out);
                render_at(body, cfg, indent + 4, c, false, out);
                out.push('\n');
                push_indent(indent, out);
                out.push('}');
            } else {
                // A delimited-tail body breaks inside its own brackets; the trailing
                // comma is kept.
                render_at(body, cfg, indent, start_col, false, out);
                out.push(',');
            }
        }
        Doc::Assign {
            prefix,
            rhs,
            trailer,
        } => {
            render_assign(prefix, rhs, *trailer, cfg, indent, col, flat, out);
        }
        Doc::Chain { operands } => {
            render_chain(operands, cfg, indent, col, flat, out);
        }
    }
}

/// Render an assignment with `rustfmt`'s dedicated RHS-break layout axis. See
/// [`Doc::Assign`]. `col` is where the assignment's first character lands;
/// `indent` is the enclosing block indent (broken RHS goes to `indent + 4`).
/// `trailer` is the width reserved after the RHS on its line (the trailing `;`).
#[allow(
    clippy::too_many_arguments,
    reason = "renderer threads col/indent/flat"
)]
fn render_assign(
    prefix: &Doc,
    rhs: &Doc,
    trailer: usize,
    cfg: RenderConfig,
    indent: usize,
    col: usize,
    flat: bool,
    out: &mut String,
) {
    let start_col = eff_col(out, col);
    let prefix_flat_w = flat_width(prefix, cfg, start_col, indent);
    let rhs_flat_w = flat_width(rhs, cfg, 0, indent);

    // FLAT: the whole `prefix rhs;` fits on the current line. An enclosing group
    // that already chose flat forces this too. A hard break in either side rules
    // it out (a statement-block RHS never lays out flat).
    let same_line_end = start_col + prefix_flat_w + rhs_flat_w + trailer;
    let no_hard_break = !has_hard_break(prefix) && !has_hard_break(rhs);
    if flat || (no_hard_break && same_line_end <= cfg.max_width) {
        render_at(prefix, cfg, indent, start_col, true, out);
        let c = current_col(out);
        render_at(rhs, cfg, indent, c, true, out);
        return;
    }

    // The same-line form overflows. Emit the prefix (always single-line — the
    // `let name: TYPE = ` head never breaks), then decide the RHS layout.
    render_at(prefix, cfg, indent, start_col, false, out);

    // RHS-BREAK: dropped onto its own line at one indent step past the block, the
    // RHS's FIRST line (laying out its own internal breaks — e.g. a closure body
    // block) fits the width. `rustfmt` prefers this next-line placement for a
    // value that would otherwise crowd the wide `let name: TYPE = ` prefix. The
    // trailer is charged against the first line only when the RHS does not break
    // internally (a single-line RHS carries the `;` on that one line).
    let rhs_indent = indent + CHAIN_BREAK_INDENT;
    let (rhs_first_line_w, rhs_breaks) = first_line_width(rhs, cfg, rhs_indent);
    let rhs_break_trailer = if rhs_breaks { 0 } else { trailer };
    if no_hard_break && rhs_indent + rhs_first_line_w + rhs_break_trailer <= cfg.max_width {
        // `rustfmt` leaves no trailing space on the `= ` line, so trim it before
        // the newline (the prefix carries the flat-case space after `=`).
        trim_trailing_spaces(out);
        out.push('\n');
        push_indent(rhs_indent, out);
        let c = current_col(out);
        render_at(rhs, cfg, rhs_indent, c, false, out);
        return;
    }

    // DELIMITER-BREAK: even at `indent + 4` the RHS's first line overflows, so
    // `rustfmt` keeps it glued to `= ` and breaks it into its own delimiters at
    // the block indent.
    let c = current_col(out);
    render_at(rhs, cfg, indent, c, false, out);
}

/// The width of `doc` rendered entirely flat from column `start_col` up to its
/// first hard break — the single-line footprint the assignment's fit tests
/// measure. Rendered into a scratch buffer with everything flat; a hard break
/// ends the measured line (a flat statement-block never arises in a fit-tested
/// path, but measuring to the first newline is the correct general rule).
fn flat_width(doc: &Doc, cfg: RenderConfig, start_col: usize, indent: usize) -> usize {
    let mut scratch = String::new();
    render_at(doc, cfg, indent, start_col, true, &mut scratch);
    scratch.split('\n').next().unwrap_or(&scratch).len()
}

/// The column width of `doc`'s first line when rendered from `start_col` letting
/// its own groups decide their internal breaks (non-flat), plus whether it broke
/// onto more than one line. `rustfmt`'s assignment RHS-break test measures this
/// first line: a value whose head fits at the RHS indent goes to the next line
/// even when its body then breaks below (a wide closure body block).
fn first_line_width(doc: &Doc, cfg: RenderConfig, start_col: usize) -> (usize, bool) {
    let mut scratch = String::new();
    push_indent(start_col, &mut scratch);
    render_at(doc, cfg, start_col, start_col, false, &mut scratch);
    let mut lines = scratch.split('\n');
    let first = lines.next().unwrap_or(&scratch);
    let breaks = lines.next().is_some();
    (first.len().saturating_sub(start_col), breaks)
}

/// Whether `doc` contains a [`Doc::HardLine`] that is NOT enclosed in a nested
/// group — an unconditional break that forces its enclosing group to stay broken.
/// A statement block carries a `HardLine` before each statement, so this returns
/// `true` for it; an inline structure carrying only soft `Line` / `Softline`
/// returns `false` and is free to flatten. Nested groups and chains hide their
/// own breaks (they decide their own layout independently).
fn has_hard_break(doc: &Doc) -> bool {
    match doc {
        Doc::HardLine => true,
        // A `BraceBody` decides its own layout independently (like `Group` and
        // `Chain`), so it hides its own breaks from the enclosing group — a
        // closure inside a call does not force the call multiline.
        Doc::Text(_)
        | Doc::Line
        | Doc::Softline
        | Doc::IfBroken(_)
        | Doc::Group(_)
        | Doc::BraceBody(_)
        | Doc::MatchArmTail { .. }
        | Doc::Assign { .. }
        | Doc::Chain { .. } => false,
        Doc::Concat(docs) => docs.iter().any(has_hard_break),
        Doc::Nest(_, inner) => has_hard_break(inner),
    }
}

/// Push `n` spaces of indentation.
fn push_indent(n: usize, out: &mut String) {
    for _ in 0..n {
        out.push(' ');
    }
}

/// Drop trailing spaces from the end of `out`. Used before a break so a line
/// never ends in whitespace (`rustfmt` trims the space after `=` when the RHS
/// moves to the next line).
fn trim_trailing_spaces(out: &mut String) {
    let trimmed = out.trim_end_matches(' ').len();
    out.truncate(trimmed);
}

/// Whether `doc` rendered flat from column `start_col` fits within the width up
/// to its first hard break. Renders into a scratch buffer with the group under
/// test flat; a nested hard break (statement-block) produces a newline that ends
/// the measured line — the width up to that newline is what must fit, matching
/// rustfmt's "does the head fit" test. Callers only fit-test groups with no hard
/// break of their own ([`has_hard_break`] gates that), so in practice the
/// scratch is a single line, but measuring to the first newline is the correct
/// general rule.
fn fits(doc: &Doc, cfg: RenderConfig, start_col: usize, indent: usize) -> bool {
    let mut scratch = String::new();
    render_at(doc, cfg, indent, start_col, true, &mut scratch);
    let first_line = scratch.split('\n').next().unwrap_or(&scratch);
    start_col + first_line.len() <= cfg.max_width
}

/// Render a binop chain with rustfmt's layout.
///
/// `col` is where the chain's first character (its outermost `(`) lands;
/// `indent` is the enclosing block indent. Broken operators go to
/// `chain_begin_line_indent + CHAIN_BREAK_INDENT`, where the begin-line indent is
/// the indentation of the line the chain starts on.
fn render_chain(
    operands: &[ChainOperand],
    cfg: RenderConfig,
    indent: usize,
    col: usize,
    flat: bool,
    out: &mut String,
) {
    // Whole-chain-flat fast path: if the entire flattened chain fits on the
    // current line AND no operand carries a hard break (a statement-block always
    // breaks, forcing the chain broken), emit it inline with no operator breaks.
    let no_hard_break = operands.iter().all(|o| !has_hard_break(&o.doc));
    let whole = Doc::Chain {
        operands: operands.to_vec(),
    };
    if flat || (no_hard_break && fits(&whole, cfg, col, indent)) {
        render_chain_flat(operands, cfg, indent, out);
        return;
    }

    // Broken layout. The shared indent for broken operators is the chain
    // begin-line's indentation plus one block step. The begin-line indent is the
    // column the chain starts at when that column IS the line's indentation
    // (chain sits at the start of a fresh continuation line); otherwise it is the
    // enclosing block indent. rustfmt keys the break indent off the block indent
    // of the statement/line the chain begins on.
    let begin_indent = current_line_indent(out).unwrap_or(indent);
    let shared_indent = begin_indent + CHAIN_BREAK_INDENT;

    // Two distinct line-1 packing regimes, both ended permanently by the first
    // break:
    //   * `flat_prefix` — the initial run of single-line operands, packed
    //     greedily while the operator plus operand fits the width (stair,
    //     tinytail). The first operand that overflows ends it.
    //   * multiline glue — once an operand renders multiline, the NEXT operator
    //     glues to that operand's closing line if it fits; but a single-line
    //     operand after a multiline one does NOT re-open the flat prefix, so the
    //     operator after IT breaks (param_patterns: `+ sum_pair` breaks after the
    //     single-line `ignore_arg`).
    // `broken` latches on the first break: no operator glues afterward.
    let mut broken = false;
    let mut flat_prefix = true;
    let mut prev_multiline = false;
    for (i, operand) in operands.iter().enumerate() {
        if i == 0 {
            let c = eff_col(out, col);
            let before = out.len();
            render_at(&operand.doc, cfg, indent, c, false, out);
            prev_multiline = out[before..].contains('\n');
            flat_prefix = !prev_multiline;
            continue;
        }
        // Non-first operands always carry a leading operator (the builder
        // invariant); an absent one degrades to no operator rather than panicking.
        let op = operand.leading_op.as_deref().unwrap_or("");

        if !broken {
            let cur = current_col(out);
            let can_glue = if flat_prefix {
                glue_fits(&operand.doc, cfg, cur, indent, op)
            } else {
                // Past the flat prefix, an operator glues only immediately after a
                // multiline operand, at that operand's closing-line column.
                prev_multiline && glue_fits(&operand.doc, cfg, cur, indent, op)
            };
            if can_glue {
                out.push(' ');
                out.push_str(op);
                out.push(' ');
                let c = current_col(out);
                let before = out.len();
                render_at(&operand.doc, cfg, indent, c, false, out);
                prev_multiline = out[before..].contains('\n');
                if prev_multiline {
                    flat_prefix = false;
                }
                continue;
            }
            broken = true;
        }
        // Broken: operator on its own line at the shared indent, operand after it.
        out.push('\n');
        push_indent(shared_indent, out);
        out.push_str(op);
        out.push(' ');
        let c = current_col(out);
        render_at(&operand.doc, cfg, shared_indent, c, false, out);
    }
}

/// Whether `op operand` glued onto the current line fits. The operand may render
/// multiline; the fit test measures only whether `op` plus the operand's FIRST
/// line fits at the current column (a multiline operand's later lines are free to
/// break below). A single-line operand must fit entirely.
fn glue_fits(operand: &Doc, cfg: RenderConfig, col: usize, indent: usize, op: &str) -> bool {
    // Column after " op " is appended.
    let after_op = col + 1 + op.len() + 1;
    if after_op > cfg.max_width {
        return false;
    }
    let mut scratch = String::new();
    render_at(operand, cfg, indent, after_op, false, &mut scratch);
    let first_line = scratch.split('\n').next().unwrap_or("");
    after_op + first_line.len() <= cfg.max_width
}

/// The indentation (leading-space count) of the line currently being written in
/// `out`, or `None` if the current line has non-space content already (the chain
/// starts mid-line, e.g. after `let z = `).
fn current_line_indent(out: &str) -> Option<usize> {
    let line_start = out.rfind('\n').map_or(0, |nl| nl + 1);
    let line = &out[line_start..];
    if line.chars().all(|c| c == ' ') {
        Some(line.len())
    } else {
        // Mid-line start: the break indent is keyed off the enclosing block, not
        // this column. Signal by returning None so the caller falls back to the
        // block indent.
        None
    }
}

/// Render every operand of a chain flat (inline), operators separated by spaces.
fn render_chain_flat(
    operands: &[ChainOperand],
    cfg: RenderConfig,
    indent: usize,
    out: &mut String,
) {
    for (i, operand) in operands.iter().enumerate() {
        if i > 0 {
            out.push(' ');
            out.push_str(operand.leading_op.as_deref().unwrap_or(""));
            out.push(' ');
        }
        let c = current_col(out);
        render_at(&operand.doc, cfg, indent, c, true, out);
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::expect_used, reason = "test assertions")]
mod p0_tests {
    //! P0 exit-gate: byte-diff the renderer against real-rustfmt output for the
    //! panel probe set. These fixtures are captured from
    //! `rustfmt --edition 2024 --style-edition 2024`; the renderer is fixed to
    //! match them, never the reverse.

    use super::*;
    use crate::doc::{ChainOperand, Doc};
    use std::borrow::Cow;

    /// A chain operand from a plain text leaf.
    fn op(leading: Option<&'static str>, text: String) -> ChainOperand {
        ChainOperand {
            leading_op: leading.map(Cow::Borrowed),
            doc: Doc::owned(text),
        }
    }

    /// Render a chain doc placed after `prefix` at block `indent`, returning the
    /// whole line(s) so the mid-line start column matches rustfmt's.
    fn render_line(prefix: &str, indent: usize, operands: &[ChainOperand]) -> String {
        let mut out = String::from(prefix);
        let col = current_col(&out);
        render_chain(
            operands,
            RenderConfig::default(),
            indent,
            col,
            false,
            &mut out,
        );
        out
    }

    #[test]
    fn stair_single_line_operands_break_tail_to_shared_indent() {
        // `let z = (((((aaaa(x) + b) + c) + d) + e) + f);` — all single-line
        // operands; line-1 packs the maximal flat-fitting prefix, then `+ e` and
        // `+ f` each break to col 8 (block indent 4 + 4). Captured from rustfmt.
        let operands = vec![
            op(None, "(((((longfunctioncallnamehere_aaaa(x)".into()),
            op(Some("+"), "bbbbbbbbbbb)".into()),
            op(Some("+"), "ccccccccccc)".into()),
            op(Some("+"), "ddddddddddd)".into()),
            op(Some("+"), "eeeeeeeeeee)".into()),
            op(
                Some("+"),
                "ffffffffffffffffffffffffffffffffffffffffffffff)".into(),
            ),
        ];
        let got = render_line("    let z = ", 4, &operands);
        let expected = "    let z = (((((longfunctioncallnamehere_aaaa(x) + bbbbbbbbbbb) + ccccccccccc) + ddddddddddd)\n        + eeeeeeeeeee)\n        + ffffffffffffffffffffffffffffffffffffffffffffff)";
        assert_eq!(
            got, expected,
            "\n--- got ---\n{got}\n--- want ---\n{expected}"
        );
    }

    #[test]
    fn stair2_tiny_tail_still_breaks() {
        // `+ ff` is tiny and fits remaining width, yet STILL breaks to the shared
        // indent: proves post-boundary breaks are unconditional, not width-tested.
        let operands = vec![
            op(
                None,
                "(((((longfunctioncallnamehere_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa(x)".into(),
            ),
            op(Some("+"), "bbbbbbbbbbb)".into()),
            op(Some("+"), "ccccccccccc)".into()),
            op(Some("+"), "ddddddddddd)".into()),
            op(Some("+"), "eeeeeeeeeee)".into()),
            op(Some("+"), "ff)".into()),
        ];
        let got = render_line("    let z = ", 4, &operands);
        let expected = "    let z = (((((longfunctioncallnamehere_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa(x) + bbbbbbbbbbb)\n        + ccccccccccc)\n        + ddddddddddd)\n        + eeeeeeeeeee)\n        + ff)";
        assert_eq!(
            got, expected,
            "\n--- got ---\n{got}\n--- want ---\n{expected}"
        );
    }

    /// A `{ <decl> <tail> }` block Doc with a statement, which ALWAYS breaks (a
    /// block containing any statement is never inlined — spec rule 3). The
    /// statement separators are `HardLine`s, so the block breaks unconditionally
    /// and forces any enclosing group broken, whatever the width.
    fn block(decl: &'static str, tail: &'static str) -> Doc {
        Doc::concat(vec![
            Doc::text("{"),
            Doc::nest(
                4,
                Doc::concat(vec![
                    Doc::HardLine,
                    Doc::text(decl),
                    Doc::HardLine,
                    Doc::text(tail),
                ]),
            ),
            Doc::HardLine,
            Doc::text("}"),
        ])
    }

    /// A `name(a, b, c)` call Doc whose arg list breaks one-per-line when it does
    /// not fit: `name(` + nested Softline-separated args with trailing comma, then
    /// Softline + `)`.
    fn call(name: &'static str, args: &[&'static str]) -> Doc {
        let mut inner = vec![];
        for a in args {
            inner.push(Doc::Softline);
            inner.push(Doc::owned(format!("{a},")));
        }
        Doc::group(Doc::concat(vec![
            Doc::owned(format!("{name}(")),
            Doc::nest(4, Doc::concat(inner)),
            Doc::Softline,
            Doc::text(")"),
        ]))
    }

    #[test]
    fn callbreak_two_operand_glue_after_forced_break_call() {
        // `(bcall(<3 long args, forced break>) + cccccccccccccccc)` — the left
        // operand's arg list breaks; the `+ c` operator GLUES to the `)` closing
        // line because it fits. Captured from rustfmt.
        let operands = vec![
            ChainOperand {
                leading_op: None,
                doc: Doc::concat(vec![
                    Doc::text("("),
                    call(
                        "bcall",
                        &[
                            "argument_number_one_long",
                            "argument_number_two_long",
                            "argument_number_three_verylong",
                        ],
                    ),
                ]),
            },
            op(Some("+"), "cccccccccccccccc)".into()),
        ];
        let got = render_line("    let z = ", 4, &operands);
        let expected = "    let z = (bcall(\n        argument_number_one_long,\n        argument_number_two_long,\n        argument_number_three_verylong,\n    ) + cccccccccccccccc)";
        assert_eq!(
            got, expected,
            "\n--- got ---\n{got}\n--- want ---\n{expected}"
        );
    }

    #[test]
    fn hyp_multiline_operand_drives_next_operator_glue() {
        // `(((mlcall_one({...}) + slshort) + mlcall_two({...})) + tail)`:
        //   op1 glues (operand0 multiline), op2 breaks (operand1 single-line),
        //   op3 breaks (already broken). Captured from rustfmt.
        let operands = vec![
            ChainOperand {
                leading_op: None,
                doc: Doc::concat(vec![
                    Doc::text("(((mlcall_one("),
                    block(
                        "let a = something_long_enough_to_force_break_here_now;",
                        "a",
                    ),
                    Doc::text(")"),
                ]),
            },
            op(Some("+"), "slshort)".into()),
            ChainOperand {
                leading_op: Some(Cow::Borrowed("+")),
                doc: Doc::concat(vec![
                    Doc::text("mlcall_two("),
                    block("let b = another_thing_long_enough_to_break_it;", "b"),
                    Doc::text("))"),
                ]),
            },
            op(Some("+"), "tail_operand_x)".into()),
        ];
        let got = render_line("    let z = ", 4, &operands);
        let expected = "    let z = (((mlcall_one({\n        let a = something_long_enough_to_force_break_here_now;\n        a\n    }) + slshort)\n        + mlcall_two({\n            let b = another_thing_long_enough_to_break_it;\n            b\n        }))\n        + tail_operand_x)";
        assert_eq!(
            got, expected,
            "\n--- got ---\n{got}\n--- want ---\n{expected}"
        );
    }

    #[test]
    fn whole_chain_fits_stays_inline() {
        // A short chain that fits entirely on the line: no operator breaks.
        let operands = vec![
            op(None, "((a".into()),
            op(Some("+"), "b)".into()),
            op(Some("+"), "c)".into()),
        ];
        let got = render_line("    let z = ", 4, &operands);
        assert_eq!(got, "    let z = ((a + b) + c)");
    }

    #[test]
    fn chain_node_is_necessary_a_group_cannot_render_glue_plus_break() {
        // A generic Group is all-flat-or-all-break: it cannot render the mixed
        // layout of `hyp` (op1 glued to a multiline operand's closing line, op2/op3
        // broken to a shared indent). Model the same operands as a Group over
        // Line-separated `op operand` pieces; when broken, EVERY Line breaks, so
        // op1 cannot stay glued. This proves the Chain variant earns its place.
        // The leading operand is padded past the width so the group cannot fit and
        // is forced broken — exercising the "all soft Lines break uniformly" path.
        let group = Doc::group(Doc::concat(vec![
            Doc::owned(format!("(((mlcall_one({}) ", "x".repeat(110))),
            Doc::text("+"),
            Doc::Line,
            Doc::text("slshort) +"),
            Doc::Line,
            Doc::text("mlcall_two({...})) +"),
            Doc::Line,
            Doc::text("tail_operand_x)"),
        ]));
        // The over-wide first operand forces the group broken.
        let mut out = String::from("    let z = ");
        render_at(
            &group,
            RenderConfig::default(),
            4,
            current_col(&out),
            false,
            &mut out,
        );
        // A Group breaks ALL its Lines: the first `+` cannot glue onto the same
        // line as a following operand the way the Chain node does. So this Group
        // rendering differs from the required `hyp` layout — demonstrating the
        // Chain node is not expressible as a plain Group.
        let group_lines: Vec<&str> = out.lines().collect();
        // Every operator sits at the end of its line (glued) in the Chain layout;
        // here they are split by unconditional Line breaks — the Group puts each
        // operand on its own line uniformly, which is NOT the rustfmt chain shape.
        assert!(
            group_lines.len() == 4,
            "a broken Group breaks every Line uniformly ({} lines), unlike the \
             chain's mixed glue+break — proving Chain is necessary",
            group_lines.len()
        );
    }

    #[test]
    fn seal_leaf_sequence_matches_across_all_chain_docs() {
        // SEAL: the whitespace-normalized leaf sequence is layout-invariant. Every
        // paren the emitter emits is carried as a Text leaf and survives rendering.
        // A chain doc's normalized leaves must equal its normalized rendered bytes.
        let operands = vec![
            op(None, "(((((longfunctioncallnamehere_aaaa(x)".into()),
            op(Some("+"), "bbbbbbbbbbb)".into()),
            op(Some("+"), "ccccccccccc)".into()),
            op(Some("+"), "ddddddddddd)".into()),
            op(Some("+"), "eeeeeeeeeee)".into()),
            op(Some("+"), "ff)".into()),
        ];
        let chain = Doc::Chain { operands };
        let rendered = render(&chain, RenderConfig::default());
        assert_eq!(
            crate::doc::whitespace_normalize(&rendered),
            chain.normalized_leaves(),
            "rendered bytes and Doc leaves must normalize to the same token sequence"
        );
    }

    /// A parenthesized arg list `name(a, b, c)`: a `Group` over `name(` + nested
    /// `Softline`-separated args (trailing comma) + `Softline` + `)`. Flat:
    /// `name(a, b, c)`. Broken: one arg per line, trailing comma, closing paren
    /// dedented to the group's start column.
    fn arg_group(name: &str, args: &[&str]) -> Doc {
        let mut inner = vec![Doc::owned(format!("{name}("))];
        let mut nested = vec![];
        for (i, a) in args.iter().enumerate() {
            if i == 0 {
                nested.push(Doc::Softline);
            } else {
                nested.push(Doc::text(","));
                nested.push(Doc::Line);
            }
            nested.push(Doc::owned((*a).to_owned()));
        }
        // Trailing comma appears only when broken; a `Softline`-guarded `,` would
        // vanish flat. Emit it as part of the last arg via a separate group-aware
        // token: here we keep it simple and match rustfmt's "trailing comma when
        // broken" by appending a `,` before the closing `Softline` only in the
        // broken layout. The renderer cannot conditionally add text, so we model
        // the common flat-fits case (no trailing comma) and the broken case is
        // covered by the call-arg builder in emit_doc; this fixture proves the
        // flatten/break decision itself.
        inner.push(Doc::nest(4, Doc::concat(nested)));
        inner.push(Doc::Softline);
        inner.push(Doc::text(")"));
        Doc::group(Doc::concat(inner))
    }

    #[test]
    fn group_with_soft_lines_flattens_when_it_fits() {
        // The whole call fits width 100 from column 0: every soft break lays out
        // flat — `Softline` -> nothing, `Line` -> a single space.
        let doc = arg_group("f", &["a", "b", "c"]);
        assert_eq!(render(&doc, RenderConfig::default()), "f(a, b, c)");
    }

    #[test]
    fn group_with_soft_lines_breaks_when_too_wide() {
        // The same shape, but the args overflow width 100: every soft break in the
        // group breaks uniformly — one arg per line at nest+4, closing paren back
        // at the group's start column.
        let long = "argument_that_is_quite_long_enough_to_matter";
        let doc = arg_group("some_function_name", &[long, long, long]);
        let got = render(&doc, RenderConfig::default());
        let expected = "some_function_name(\n    argument_that_is_quite_long_enough_to_matter,\n    argument_that_is_quite_long_enough_to_matter,\n    argument_that_is_quite_long_enough_to_matter\n)";
        assert_eq!(
            got, expected,
            "\n--- got ---\n{got}\n--- want ---\n{expected}"
        );
    }

    #[test]
    fn group_with_hardline_never_flattens_even_when_it_would_fit() {
        // A tiny group whose flat width easily fits, but carrying a `HardLine`:
        // it must still break (a statement block never inlines).
        let doc = Doc::group(Doc::concat(vec![
            Doc::text("{"),
            Doc::nest(4, Doc::concat(vec![Doc::HardLine, Doc::text("x")])),
            Doc::HardLine,
            Doc::text("}"),
        ]));
        assert_eq!(render(&doc, RenderConfig::default()), "{\n    x\n}");
    }

    #[test]
    fn brace_body_vanishes_when_body_fits_flat() {
        // A `BraceBody` whose body fits the width renders JUST the body — no braces
        // — matching `rustfmt`'s `move |_| rest` brace-strip on a fitting closure.
        let doc = Doc::concat(vec![
            Doc::text("move |_| "),
            Doc::brace_body(Doc::text("short_rest")),
        ]);
        assert_eq!(render(&doc, RenderConfig::default()), "move |_| short_rest");
    }

    #[test]
    fn brace_body_braces_and_breaks_when_body_overflows() {
        // A `BraceBody` whose body overflows the width braces and breaks to a block:
        // `{`, the body on its own line at one indent step, `}` dedented back —
        // `rustfmt`'s block-form closure body.
        let wide = "a".repeat(110);
        let doc = Doc::concat(vec![
            Doc::text("move |_| "),
            Doc::brace_body(Doc::owned(wide.clone())),
        ]);
        let got = render(&doc, RenderConfig::default());
        let expected = format!("move |_| {{\n    {wide}\n}}");
        assert_eq!(
            got, expected,
            "\n--- got ---\n{got}\n--- want ---\n{expected}"
        );
    }

    #[test]
    fn brace_body_with_hardline_body_always_braces_even_when_narrow() {
        // A statement-block body carries a `HardLine`, so it can never fit flat and
        // the `BraceBody` always braces — a block closure body is always braced.
        let body = Doc::concat(vec![Doc::text("let y = 1;"), Doc::HardLine, Doc::text("y")]);
        let doc = Doc::concat(vec![Doc::text("move |_| "), Doc::brace_body(body)]);
        assert_eq!(
            render(&doc, RenderConfig::default()),
            "move |_| {\n    let y = 1;\n    y\n}"
        );
    }

    #[test]
    fn brace_body_leaves_carry_the_braces_for_the_seal() {
        // The braces ARE part of the SEAL leaf sequence (the string emitter always
        // writes them), so the normalized leaves carry `{ body }` whether or not the
        // render drops the braces — the flat render diverges from the leaves on the
        // braces exactly as `IfBroken` diverges on the trailing comma.
        let doc = Doc::brace_body(Doc::text("rest"));
        assert_eq!(doc.normalized_leaves(), "{ rest }");
        // Flat render drops the braces (matches rustfmt), so rendered != leaves here.
        assert_eq!(render(&doc, RenderConfig::default()), "rest");
    }

    /// Render an assignment `doc` as a block statement at block `indent`: seed the
    /// output with the indent, render, and append the trailing `;` the `trailer`
    /// accounted for. Returns the whole statement line(s), matching the column
    /// `rustfmt` captured its golden from.
    fn render_assign_stmt(indent: usize, prefix: &'static str, rhs: Doc) -> String {
        let mut out = String::new();
        push_indent(indent, &mut out);
        let doc = Doc::assign(Doc::text(prefix), rhs, 1);
        render_at(
            &doc,
            RenderConfig::default(),
            indent,
            current_col(&out),
            false,
            &mut out,
        );
        out.push(';');
        out
    }

    #[test]
    fn assign_stays_flat_when_prefix_and_rhs_fit() {
        // `let x: T = rhs;` that fits the width stays on one line: prefix then the
        // flat RHS, no break after `=`.
        let got = render_assign_stmt(4, "let x: i64 = ", Doc::text("Box::new(short)"));
        assert_eq!(got, "    let x: i64 = Box::new(short);");
    }

    #[test]
    fn assign_breaks_rhs_to_own_line_when_prefix_rhs_overflows() {
        // `let __ipe_fn: Box<…> = Box::new(…);` whose one-line form (with the `;`)
        // overflows width 100 but whose flat RHS fits on its own line at col 8
        // (block 4 + step 4): the RHS drops below the `=`, indented +4. Golden
        // captured from `rustfmt --edition 2024 --style-edition 2024`.
        let got = render_assign_stmt(
            4,
            "let __ipe_fn: Box<dyn Fn(i64, i64, i64) -> i64 + Send + 'static> = ",
            Doc::text("Box::new(move |aaaa: i64, bbbb: i64, cccc: i64| -> i64 { aaaa })"),
        );
        let expected = "    let __ipe_fn: Box<dyn Fn(i64, i64, i64) -> i64 + Send + 'static> =\n        Box::new(move |aaaa: i64, bbbb: i64, cccc: i64| -> i64 { aaaa });";
        assert_eq!(
            got, expected,
            "\n--- got ---\n{got}\n--- want ---\n{expected}"
        );
    }

    #[test]
    fn assign_falls_back_to_delimiter_break_when_rhs_first_line_overflows_on_own_line() {
        // When even at col 8 the RHS's FIRST line (its open-delimiter head, laid out
        // broken) overflows the width, RHS-break is unavailable — the flat RHS moved
        // to the next line would not fit either. The assignment keeps the RHS glued
        // to `= ` and breaks it into its own delimiters at the block indent (the
        // fallback). This exercises the third `render_assign` branch: the `= ` line
        // keeps the RHS head, and the RHS lays out non-flat (its `Softline`s break).
        //
        // The name is sized so the broken group's first line — `<name>(` at col 8 —
        // exceeds width 100, ruling out RHS-break.
        let long_name = "a_deliberately_enormous_callee_name_wide_enough_that_its_open_paren_alone_overflows_col_eight_pad";
        let rhs = arg_group(long_name, &["a", "b"]);
        let got = render_assign_stmt(4, "let n: T = ", rhs);
        // The RHS stayed glued to `= ` (no break after `=`) and broke its own args
        // one per line — the delimiter-break fallback, not the RHS-break.
        let first_line = got.lines().next().expect("at least one line");
        assert!(
            first_line.ends_with('('),
            "RHS head should stay glued to `= ` on the first line: {first_line}"
        );
        assert!(
            got.contains("\n        a,\n        b\n"),
            "the RHS should break its args in place at col 8:\n{got}"
        );
    }

    #[test]
    fn assign_leaves_are_break_invisible_matching_a_flat_prefix_rhs() {
        // The break after `= ` is pure whitespace: the normalized leaves of an
        // assignment equal `prefix rhs` with a single space — exactly what the
        // string emitter writes — so the SEAL holds across the break.
        let doc = Doc::assign(Doc::text("let x: T = "), Doc::text("value"), 1);
        assert_eq!(doc.normalized_leaves(), "let x: T = value");
    }

    #[test]
    fn nested_group_refits_independently_of_broken_outer_group() {
        // An outer group forced broken (over-wide leading text) still lets an inner
        // group that fits lay out flat — groups decide their layout independently.
        let inner = arg_group("g", &["p", "q"]);
        let outer = Doc::group(Doc::concat(vec![
            Doc::owned(format!("outer_{}(", "z".repeat(110))),
            Doc::nest(4, Doc::concat(vec![Doc::Softline, inner])),
            Doc::Softline,
            Doc::text(")"),
        ]));
        let got = render(&outer, RenderConfig::default());
        // Outer breaks (its leading token overflows); inner `g(p, q)` fits and
        // stays flat on its own line.
        assert!(
            got.contains("\n    g(p, q)\n"),
            "inner group should flatten inside a broken outer group:\n{got}"
        );
    }
}

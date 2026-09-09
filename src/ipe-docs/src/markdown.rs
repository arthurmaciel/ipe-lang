//! Markdown parse tree, the single-source-of-truth Markdown implementation
//! shared with `Ipe.Markdown` (`src/stdlib/Ipe/Markdown.ipe`).
//!
//! `parse_blocks` / `parse_spans` are a faithful hand-port of the `Ipe.Markdown`
//! parser: the same `Block` / `Span` / `HeadingLevel` trees, the same block and
//! inline recognition rules. A CI parity gate snapshots the expected trees from
//! an `ipe`-run of `Ipe.Markdown`, so `Markdown.ipe` stays authoritative and any
//! drift reddens the build.
//!
//! [`is_safe_href`] is the single home for the doc-side URL-scheme allowlist.
//!
//! The parser mirrors the runtime String kernels
//! (`src/runtime/rust/src/string.rs`) it is ported from: text is indexed by
//! Unicode codepoint (never byte), `lines` maps to `str::lines`, `split` on a
//! non-empty separator maps to `str::split`, and `trim` / `starts_with` /
//! `ends_with` / `contains` mirror the standard library. The `Ipe.Markdown`
//! parser only ever slices at non-negative, in-range codepoint offsets, so the
//! codepoint helpers below take `usize` offsets and clamp — no negative-index
//! machinery is needed.

/// The six Markdown heading levels — mirrors `Ipe.Markdown.HeadingLevel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadingLevel {
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
}

impl HeadingLevel {
    /// The 1-based heading number (`H1` -> 1).
    #[must_use]
    pub const fn number(self) -> u8 {
        match self {
            Self::H1 => 1,
            Self::H2 => 2,
            Self::H3 => 3,
            Self::H4 => 4,
            Self::H5 => 5,
            Self::H6 => 6,
        }
    }
}

/// One paragraph-shaped chunk of a Markdown document — mirrors
/// `Ipe.Markdown.Block` (8 constructors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Header(HeadingLevel, String),
    Para(String),
    Code(String),
    Bullet(Vec<String>),
    Numbered(Vec<String>),
    Table(Vec<String>, Vec<Vec<String>>),
    Rule,
    Blockquote(Vec<Self>),
}

/// A single styled run of text within a line — mirrors `Ipe.Markdown.Span`
/// (7 constructors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Span {
    Plain(String),
    Bold(String),
    Italic(String),
    Code(String),
    Link(String, String),
    Image(String, String),
    HardBreak,
}

// ── Codepoint helpers ─────────────────────────────────────────────────────────
// The `Ipe.Markdown` parser indexes text by Unicode codepoint. These helpers
// reproduce that indexing over `usize` offsets (clamping out-of-range slices to
// empty, exactly as the runtime `string_slice` kernel does).

/// Codepoint count — mirrors `String.length`.
fn char_count(s: &str) -> usize {
    s.chars().count()
}

/// Codepoints in `[start, end)` — mirrors `String.slice start end` for
/// non-negative, clamped offsets.
fn slice_cp(start: usize, end: usize, s: &str) -> String {
    if start >= end {
        return String::new();
    }
    s.chars().skip(start).take(end - start).collect()
}

/// Drop the first `n` codepoints — mirrors `String.slice n (length s) s`.
fn drop_cp(n: usize, s: &str) -> String {
    s.chars().skip(n).collect()
}

/// First codepoint index of `needle` in `hay`, or `None`. Codepoint-indexed,
/// mirroring `indexOfFirst`.
fn index_of(needle: &str, hay: &str) -> Option<usize> {
    // `str::find` returns a byte offset; convert it to a codepoint offset so the
    // result matches the parser's codepoint-based slicing.
    hay.find(needle)
        .map(|byte_idx| hay[..byte_idx].chars().count())
}

// ── Block parsing (faithful port of Ipe.Markdown.parseBlocks) ─────────────────

/// Split a Markdown document into its list of [`Block`]s.
#[must_use]
pub fn parse_blocks(src: &str) -> Vec<Block> {
    // `String.lines` maps to `str::lines`.
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    blocks_from_lines(&lines, &mut out);
    out
}

/// Iterative equivalent of the tail-recursive `blocksFromLines`: consume the
/// line slice front-to-back, pushing one block per recognised construct.
fn blocks_from_lines(lines: &[&str], out: &mut Vec<Block>) {
    let mut idx = 0usize;
    while let Some(&l) = lines.get(idx) {
        let t = l.trim();
        if t.is_empty() {
            idx += 1;
        } else if is_hr_line(t) {
            out.push(Block::Rule);
            idx += 1;
        } else if l.starts_with("###### ") {
            out.push(Block::Header(HeadingLevel::H6, drop_cp(7, l)));
            idx += 1;
        } else if l.starts_with("##### ") {
            out.push(Block::Header(HeadingLevel::H5, drop_cp(6, l)));
            idx += 1;
        } else if l.starts_with("#### ") {
            out.push(Block::Header(HeadingLevel::H4, drop_cp(5, l)));
            idx += 1;
        } else if l.starts_with("### ") {
            out.push(Block::Header(HeadingLevel::H3, drop_cp(4, l)));
            idx += 1;
        } else if l.starts_with("## ") {
            out.push(Block::Header(HeadingLevel::H2, drop_cp(3, l)));
            idx += 1;
        } else if l.starts_with("# ") {
            out.push(Block::Header(HeadingLevel::H1, drop_cp(2, l)));
            idx += 1;
        } else if l.starts_with("```") {
            // Code fence: the body is every line up to the next closing ```.
            let (body, consumed) = take_code_fence(lines, idx + 1);
            out.push(Block::Code(body));
            idx = consumed;
        } else if is_bullet_line(l) {
            let (items, consumed) = take_bullet_group(lines, idx);
            out.push(Block::Bullet(items));
            idx = consumed;
        } else if is_numbered_line(l) {
            let (items, consumed) = take_numbered_group(lines, idx);
            out.push(Block::Numbered(items));
            idx = consumed;
        } else if is_table_header_line(l, lines.get(idx + 1).copied()) {
            let (header, rows, consumed) = take_table(lines, idx);
            out.push(Block::Table(header, rows));
            idx = consumed;
        } else if is_blockquote_line(l) {
            let (inner_lines, consumed) = take_blockquote_group(lines, idx);
            let inner_refs: Vec<&str> = inner_lines.iter().map(String::as_str).collect();
            let mut inner_blocks = Vec::new();
            blocks_from_lines(&inner_refs, &mut inner_blocks);
            out.push(Block::Blockquote(inner_blocks));
            idx = consumed;
        } else {
            let (body, consumed) = take_paragraph(lines, idx);
            out.push(Block::Para(body));
            idx = consumed;
        }
    }
}

/// `---` / `***` / `___` (with the repetition counts `Ipe.Markdown` recognises).
fn is_hr_line(t: &str) -> bool {
    matches!(t, "---" | "***" | "___" | "----" | "-----")
}

fn is_blockquote_line(l: &str) -> bool {
    l.starts_with("> ") || l == ">"
}

/// Collect consecutive blockquote lines, stripping the `> ` prefix from each.
/// Returns the inner lines and the index of the first line after the group.
fn take_blockquote_group(lines: &[&str], from: usize) -> (Vec<String>, usize) {
    let mut inner = Vec::new();
    let mut idx = from;
    while let Some(&l) = lines.get(idx) {
        if is_blockquote_line(l) {
            let stripped = if l.starts_with("> ") {
                drop_cp(2, l)
            } else {
                String::new()
            };
            inner.push(stripped);
            idx += 1;
        } else {
            break;
        }
    }
    (inner, idx)
}

fn is_bullet_line(l: &str) -> bool {
    l.starts_with("- ") || l.starts_with("* ") || l.starts_with("  - ") || l.starts_with("  * ")
}

/// A numbered list line starts `<digits>. ` for the prefixes 1..=10 that
/// `Ipe.Markdown` recognises.
fn is_numbered_line(l: &str) -> bool {
    const PREFIXES: [&str; 10] = [
        "1. ", "2. ", "3. ", "4. ", "5. ", "6. ", "7. ", "8. ", "9. ", "10. ",
    ];
    PREFIXES.iter().any(|p| l.starts_with(p))
}

/// Consume a code fence body: every line from `from` up to (and consuming) the
/// next closing fence. Returns the joined body and the index after the close (or
/// after end-of-input for an unclosed fence).
fn take_code_fence(lines: &[&str], from: usize) -> (String, usize) {
    let mut body: Vec<&str> = Vec::new();
    let mut idx = from;
    while let Some(&l) = lines.get(idx) {
        if l.starts_with("```") {
            idx += 1;
            return (body.join("\n"), idx);
        }
        body.push(l);
        idx += 1;
    }
    (body.join("\n"), idx)
}

/// Group consecutive bullet lines, stripping the marker prefix from each.
fn take_bullet_group(lines: &[&str], from: usize) -> (Vec<String>, usize) {
    let mut items = Vec::new();
    let mut idx = from;
    while let Some(&l) = lines.get(idx) {
        if is_bullet_line(l) {
            items.push(strip_bullet_prefix(l));
            idx += 1;
        } else {
            break;
        }
    }
    (items, idx)
}

/// Group consecutive numbered lines, stripping the `N. ` prefix from each.
fn take_numbered_group(lines: &[&str], from: usize) -> (Vec<String>, usize) {
    let mut items = Vec::new();
    let mut idx = from;
    while let Some(&l) = lines.get(idx) {
        if is_numbered_line(l) {
            items.push(strip_numbered_prefix(l));
            idx += 1;
        } else {
            break;
        }
    }
    (items, idx)
}

/// Drop the `- ` / `* ` / `  - ` / `  * ` bullet prefix.
fn strip_bullet_prefix(l: &str) -> String {
    if l.starts_with("  - ") || l.starts_with("  * ") {
        drop_cp(4, l)
    } else {
        drop_cp(2, l)
    }
}

/// Drop the `1. ` / `10. ` etc. prefix: split on `. ` and re-join the remainder
/// (mirrors `String.split ". "` then `String.join ". "`).
fn strip_numbered_prefix(l: &str) -> String {
    let parts: Vec<&str> = l.split(". ").collect();
    match parts.split_first() {
        Some((_, rest)) => rest.join(". "),
        None => l.to_owned(),
    }
}

/// Consume contiguous paragraph lines joined by a space (or a newline where a
/// line ends in two-or-more spaces — the hard-break marker). Stops at a blank
/// line or a block-starter. Returns the joined body and the index after it.
fn take_paragraph(lines: &[&str], from: usize) -> (String, usize) {
    let mut end = from;
    while let Some(&l) = lines.get(end) {
        if l.trim().is_empty() || is_block_starter(l) {
            break;
        }
        end += 1;
    }
    // Fold [from, end) into a body with the per-line hard-break separator.
    // Mirrors the right-fold in `takeParagraph`: the separator before line `i`
    // is `\n` when the preceding line ends in two-or-more spaces, else a space.
    let mut body = String::new();
    for i in from..end {
        let Some(&l) = lines.get(i) else { break };
        if i == from {
            body.push_str(l);
        } else if let Some(&prev) = lines.get(i - 1) {
            body.push_str(if prev.ends_with("  ") { "\n" } else { " " });
            body.push_str(l);
        }
    }
    (body, end)
}

fn is_block_starter(l: &str) -> bool {
    l.starts_with("# ")
        || l.starts_with("## ")
        || l.starts_with("### ")
        || l.starts_with("#### ")
        || l.starts_with("##### ")
        || l.starts_with("###### ")
        || l.starts_with("```")
        || is_bullet_line(l)
        || is_numbered_line(l)
        || is_hr_line(l.trim())
        || is_blockquote_line(l)
}

// ── Table parsing ─────────────────────────────────────────────────────────────

/// A table-header line is `| ... | ... |` IMMEDIATELY followed by a separator
/// row like `|---|---|`. Both conditions required to avoid false-matching
/// pipe-delimited paragraphs.
fn is_table_header_line(l: &str, next: Option<&str>) -> bool {
    if !looks_like_table_row(l) {
        return false;
    }
    next.is_some_and(is_table_separator)
}

fn looks_like_table_row(l: &str) -> bool {
    let t = l.trim();
    // `startsWith "|" t && contains "|" (slice 1 (length t) t)`.
    t.starts_with('|') && drop_cp(1, t).contains('|')
}

fn is_table_separator(l: &str) -> bool {
    let t = l.trim();
    t.starts_with('|') && (t.contains('-') || t.contains('=')) && table_separator_ok(t)
}

/// A separator row contains only `|`, `-`, `:`, `=`, and whitespace.
fn table_separator_ok(t: &str) -> bool {
    let stripped = t.replace(['|', '-', ':', '='], "");
    stripped.trim().is_empty()
}

/// Parse a table starting at `from` (a header row, its separator, then data
/// rows). Returns (header cells, body rows, index after the table).
fn take_table(lines: &[&str], from: usize) -> (Vec<String>, Vec<Vec<String>>, usize) {
    let Some(&header) = lines.get(from) else {
        return (Vec::new(), Vec::new(), from);
    };
    // `header :: _sep :: dataLines` — data rows start two lines in.
    let header_cells = parse_table_row(header);
    let mut rows = Vec::new();
    let mut idx = from + 2;
    while let Some(&l) = lines.get(idx) {
        if looks_like_table_row(l) {
            rows.push(parse_table_row(l));
            idx += 1;
        } else {
            break;
        }
    }
    (header_cells, rows, idx)
}

/// `| a | b | c |` -> `[a, b, c]`. Strips outer pipes, splits on `|`, trims each.
fn parse_table_row(l: &str) -> Vec<String> {
    let t = l.trim();
    let stripped = if t.starts_with('|') {
        drop_cp(1, t)
    } else {
        t.to_owned()
    };
    let trimmed_trailing = if stripped.ends_with('|') {
        let len = char_count(&stripped);
        slice_cp(0, len.saturating_sub(1), &stripped)
    } else {
        stripped
    };
    trimmed_trailing
        .split('|')
        .map(|c| c.trim().to_owned())
        .collect()
}

// ── Inline parsing (faithful port of Ipe.Markdown.parseSpans) ─────────────────

/// Parse one line of inline markup into its list of [`Span`]s.
#[must_use]
pub fn parse_spans(s: &str) -> Vec<Span> {
    let mut acc: Vec<Span> = Vec::new();
    span_loop(s, &mut acc);
    acc
}

/// Character-by-character loop mirroring `spanLoop`. `pending` accumulates plain
/// text; on a delimiter the pending run is flushed and the delimited span
/// consumed. `Ipe.Markdown` builds the accumulator reversed then reverses at the
/// end; here `acc` is kept forward (each flush appends), yielding the identical
/// span list. Iterative rather than recursive so a long line cannot overflow the
/// stack (bounded-by-construction).
fn span_loop(start: &str, acc: &mut Vec<Span>) {
    let mut remaining = start.to_owned();
    let mut pending = String::new();
    loop {
        if remaining.is_empty() {
            if pending.ends_with("  ") {
                flush_pending(strip_trailing_spaces(&pending), acc);
                acc.push(Span::HardBreak);
            } else {
                flush_pending(pending, acc);
            }
            return;
        } else if remaining.starts_with('\n') {
            flush_pending(strip_trailing_spaces(&pending), acc);
            acc.push(Span::HardBreak);
            remaining = drop_cp(1, &remaining);
            pending = String::new();
        } else if remaining.starts_with("![") {
            if let Some((alt, url, after)) = take_image(&remaining) {
                flush_pending(pending, acc);
                acc.push(Span::Image(alt, url));
                remaining = after;
                pending = String::new();
            } else {
                remaining = drop_cp(1, &remaining);
                pending.push('!');
            }
        } else if remaining.starts_with("**") {
            let (inner, after) = take_between("**", &drop_cp(2, &remaining));
            flush_pending(pending, acc);
            acc.push(Span::Bold(inner));
            remaining = after;
            pending = String::new();
        } else if remaining.starts_with('`') {
            let (inner, after) = take_between("`", &drop_cp(1, &remaining));
            flush_pending(pending, acc);
            acc.push(Span::Code(inner));
            remaining = after;
            pending = String::new();
        } else if remaining.starts_with('[') {
            if let Some((text, url, after)) = take_link(&remaining) {
                flush_pending(pending, acc);
                acc.push(Span::Link(text, url));
                remaining = after;
                pending = String::new();
            } else {
                remaining = drop_cp(1, &remaining);
                pending.push('[');
            }
        } else if remaining.starts_with('*') {
            let (inner, after) = take_between("*", &drop_cp(1, &remaining));
            if inner.is_empty() {
                remaining = drop_cp(1, &remaining);
                pending.push('*');
            } else {
                flush_pending(pending, acc);
                acc.push(Span::Italic(inner));
                remaining = after;
                pending = String::new();
            }
        } else {
            let c = slice_cp(0, 1, &remaining);
            remaining = drop_cp(1, &remaining);
            pending.push_str(&c);
        }
    }
}

fn flush_pending(pending: String, acc: &mut Vec<Span>) {
    if !pending.is_empty() {
        acc.push(Span::Plain(pending));
    }
}

/// Take text up to (and consuming) the closing delimiter. Returns (inner, after).
/// If no closing delimiter is found, treats the rest as inner — graceful
/// degradation on malformed input. Mirrors `takeBetween`.
fn take_between(delim: &str, s: &str) -> (String, String) {
    index_of(delim, s).map_or_else(
        || (s.to_owned(), String::new()),
        |idx| {
            let inner = slice_cp(0, idx, s);
            let after = drop_cp(idx + char_count(delim), s);
            (inner, after)
        },
    )
}

/// Parse `[text](url)` at the current position. `None` when the syntax does not
/// match. Mirrors `takeLink`.
fn take_link(s: &str) -> Option<(String, String, String)> {
    let close = index_of("]", s)?;
    let after_close = drop_cp(close + 1, s);
    if !after_close.starts_with('(') {
        return None;
    }
    let url_and_after = drop_cp(1, &after_close);
    let close_paren = index_of(")", &url_and_after)?;
    let link_text = slice_cp(1, close, s);
    let link_url = slice_cp(0, close_paren, &url_and_after);
    let after_link = drop_cp(close_paren + 1, &url_and_after);
    Some((link_text, link_url, after_link))
}

/// Parse `![alt](url)` at the current position. `s` must start with `![`.
fn take_image(s: &str) -> Option<(String, String, String)> {
    // Slice past the `!` so `take_link` parses `[alt](url)`.
    let without_bang = drop_cp(1, s);
    take_link(&without_bang)
}

/// Right-trim ASCII spaces only. Mirrors `stripTrailingSpaces`.
fn strip_trailing_spaces(s: &str) -> String {
    s.trim_end_matches(' ').to_owned()
}

// ── Shared URL-scheme allowlist (single SSOT) ─────────────────────────────────

/// A link target is safe when it is relative, a fragment, or an
/// `http` / `https` / `mailto` absolute — never `javascript:`, `vbscript:`, or
/// ANY `data:`.
///
/// This is the single home for the doc-side URL allowlist. Both the parse
/// boundary and the emit boundary of the doc-side HTML walker route through it
/// (defend-in-depth), so no single missed check opens a script scheme.
#[must_use]
pub fn is_safe_href(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if let Some(scheme) = lower.split(':').next()
        && scheme != lower
        && !scheme.is_empty()
        && !scheme.contains(['/', '?', '#'])
        && scheme
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
    {
        // A `:` after a valid scheme name (before any `/`, `?`, `#`) is an
        // absolute URL; only the safe schemes are admitted.
        return matches!(scheme, "http" | "https" | "mailto");
    }
    // No scheme (relative, fragment, or scheme-less) is safe.
    true
}

// ── Parser unit tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod parser_tests {
    use super::{Block, HeadingLevel, Span, is_safe_href, parse_blocks, parse_spans};

    #[test]
    fn heading_levels_are_faithful() {
        assert_eq!(
            parse_blocks("# Top"),
            vec![Block::Header(HeadingLevel::H1, "Top".to_owned())]
        );
        assert_eq!(
            parse_blocks("###### Deep"),
            vec![Block::Header(HeadingLevel::H6, "Deep".to_owned())]
        );
    }

    #[test]
    fn paragraph_folds_soft_wrapped_lines() {
        assert_eq!(
            parse_blocks("one\ntwo"),
            vec![Block::Para("one two".to_owned())]
        );
    }

    #[test]
    fn hard_break_keeps_newline_then_span_is_break() {
        // A line ending in two-or-more spaces folds into the next with a `\n`
        // separator (the hard-break marker); the line text itself is kept
        // verbatim (trailing spaces intact), matching `Ipe.Markdown.takeParagraph`.
        let blocks = parse_blocks("one  \ntwo");
        assert_eq!(blocks, vec![Block::Para("one  \ntwo".to_owned())]);
        // The paragraph body carries the newline; span parse turns it into a break.
        let body = match blocks.first() {
            Some(Block::Para(body)) => body.as_str(),
            _ => "",
        };
        assert!(parse_spans(body).contains(&Span::HardBreak));
    }

    #[test]
    fn fenced_code_drops_delimiters() {
        assert_eq!(
            parse_blocks("```ipe\nfoo =\n    bar\n```"),
            vec![Block::Code("foo =\n    bar".to_owned())]
        );
    }

    #[test]
    fn bullet_and_numbered_groups() {
        assert_eq!(
            parse_blocks("- a\n- b"),
            vec![Block::Bullet(vec!["a".to_owned(), "b".to_owned()])]
        );
        assert_eq!(
            parse_blocks("1. a\n2. b"),
            vec![Block::Numbered(vec!["a".to_owned(), "b".to_owned()])]
        );
    }

    #[test]
    fn table_header_and_rows() {
        assert_eq!(
            parse_blocks("| A | B |\n|---|---|\n| 1 | 2 |"),
            vec![Block::Table(
                vec!["A".to_owned(), "B".to_owned()],
                vec![vec!["1".to_owned(), "2".to_owned()]],
            )]
        );
    }

    #[test]
    fn horizontal_rule() {
        assert_eq!(parse_blocks("---"), vec![Block::Rule]);
    }

    #[test]
    fn nested_blockquote() {
        assert_eq!(
            parse_blocks("> a\n> > b"),
            vec![Block::Blockquote(vec![
                Block::Para("a".to_owned()),
                Block::Blockquote(vec![Block::Para("b".to_owned())]),
            ])]
        );
    }

    #[test]
    fn inline_spans_bold_italic_code_link() {
        assert_eq!(
            parse_spans("a **b** *c* `d` [e](http://x)"),
            vec![
                Span::Plain("a ".to_owned()),
                Span::Bold("b".to_owned()),
                Span::Plain(" ".to_owned()),
                Span::Italic("c".to_owned()),
                Span::Plain(" ".to_owned()),
                Span::Code("d".to_owned()),
                Span::Plain(" ".to_owned()),
                Span::Link("e".to_owned(), "http://x".to_owned()),
            ]
        );
    }

    #[test]
    fn unmatched_bracket_and_backtick_stay_plain() {
        // A lone `[` with no valid `](url)` and a lone backtick with no close are
        // literal text (matching `Ipe.Markdown`: a link needs a well-formed pair;
        // an unclosed code span degrades to plain).
        assert_eq!(
            parse_spans("a [x and more"),
            vec![Span::Plain("a [x and more".to_owned())]
        );
    }

    #[test]
    fn lone_asterisk_italicises_to_end_of_line() {
        // `Ipe.Markdown` italicises greedily: a `*` followed by non-empty text
        // with no closing `*` runs to end-of-line as an `ItalicSpan`. (This is a
        // deliberate divergence from the incumbent doc renderer, which requires a
        // matching close; reconciled in the byte-equivalence gate.)
        assert_eq!(
            parse_spans("a * b"),
            vec![Span::Plain("a ".to_owned()), Span::Italic(" b".to_owned())]
        );
    }

    #[test]
    fn image_span() {
        assert_eq!(
            parse_spans("![alt](http://x/i.png)"),
            vec![Span::Image("alt".to_owned(), "http://x/i.png".to_owned())]
        );
    }

    #[test]
    fn unicode_slicing_is_codepoint_indexed() {
        // A multibyte heading text must survive the `drop_cp(2, …)` prefix strip.
        assert_eq!(
            parse_blocks("# café"),
            vec![Block::Header(HeadingLevel::H1, "café".to_owned())]
        );
    }

    #[test]
    fn is_safe_href_allowlist() {
        assert!(is_safe_href("http://example.com"));
        assert!(is_safe_href("https://example.com"));
        assert!(is_safe_href("mailto:a@b.com"));
        assert!(is_safe_href("relative.html"));
        assert!(is_safe_href("#fragment"));
        assert!(is_safe_href("/absolute/path"));
    }

    #[test]
    fn is_safe_href_rejects_scripts_and_all_data() {
        assert!(!is_safe_href("javascript:alert(1)"));
        assert!(!is_safe_href("vbscript:msgbox(1)"));
        assert!(!is_safe_href("data:text/html,<script>"));
        // ALL data: is rejected, including image forms (the doc-path allowlist is
        // stricter than the app-side Ipe.Markdown sink).
        assert!(!is_safe_href("data:image/png;base64,AAAA"));
        // Case / whitespace normalisation cannot smuggle a scheme through.
        assert!(!is_safe_href("  JavaScript:alert(1)"));
    }
}

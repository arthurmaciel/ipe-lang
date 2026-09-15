//! Doc-side `Block`/`Span` → HTML walker — the single security-critical
//! component of the Markdown SSOT path.
//!
//! Escape-by-default is the §1-critical invariant. Every text byte is
//! HTML-escaped, no raw HTML is ever passed through, and every link `href` /
//! image `src` is gated by the shared [`super::is_safe_href`] allowlist at emit
//! (defence-in-depth beside the parse-boundary gate in [`super::parse`]). On a
//! rejected URL the walker emits the visible text as escaped plain text — never
//! a live `href`/`src`.
//!
//! The walk is exhaustive and wildcard-free: an explicit arm per `Block` (8),
//! `Span` (7), and `HeadingLevel` (6) constructor, so a new `Ipe.Markdown`
//! constructor forces a compile error here rather than a silent gap.
//!
//! Blockquote nesting is bounded by construction ([`MAX_BLOCKQUOTE_DEPTH`]) so
//! deeply-nested `>>>>…` input cannot exhaust the stack.
//!
//! Two caller concerns stay OUT of this leaf module so it needs no highlighter
//! or theme dependency: the ATX heading level-shift (a caller offset) and code
//! highlighting (a caller-supplied renderer) — both are passed in via
//! [`WalkOptions`].

use super::parse::parse_spans;
use super::{Block, HeadingLevel, Span, is_safe_href};
use std::fmt::Write as _;

/// Maximum blockquote nesting depth the walker will render.
///
/// Input nested deeper is rendered up to the ceiling and the remainder is
/// dropped, so a pathological `>>>>…` document cannot deep-recurse (bounded by
/// construction, principle 3). The ceiling is far above any real document's
/// nesting.
pub const MAX_BLOCKQUOTE_DEPTH: usize = 32;

/// Caller-supplied rendering concerns kept out of the leaf module.
#[derive(Default)]
pub struct WalkOptions<'a> {
    /// Added to every heading level so a body heading nests under the page
    /// chrome (the incumbent uses `+2`, clamped to `h6`). The parser emits
    /// faithful `H1`..`H6`; the shift is applied here, never in the parser.
    pub heading_offset: u8,
    /// Renders a `CodeBlock` body to `<code>…</code>` inner HTML (the caller
    /// wires the Ipê highlighter). The result MUST already be HTML-safe: the
    /// walker wraps it in `<pre>` without re-escaping. When absent, the walker
    /// falls back to escaped text in a `<code>` element.
    pub code_renderer: Option<&'a dyn Fn(&str) -> String>,
}

/// Render a list of blocks to an HTML fragment.
#[must_use]
pub fn blocks_to_html(blocks: &[Block], opts: &WalkOptions) -> String {
    let mut out = String::new();
    for block in blocks {
        render_block(&mut out, block, opts, 0);
    }
    out
}

/// Render a single line of inline Markdown to an HTML fragment (spans only).
#[must_use]
pub fn inline_to_html(line: &str, opts: &WalkOptions) -> String {
    let mut out = String::new();
    render_spans(&mut out, &parse_spans(line), opts);
    out
}

fn render_block(out: &mut String, block: &Block, opts: &WalkOptions, depth: usize) {
    match block {
        Block::Header(level, text) => {
            let n = heading_number(*level, opts.heading_offset);
            let _ = write!(out, "<h{n}>");
            render_spans(out, &parse_spans(text), opts);
            let _ = writeln!(out, "</h{n}>");
        }
        Block::Para(text) => {
            out.push_str("<p>");
            render_spans(out, &parse_spans(text), opts);
            out.push_str("</p>\n");
        }
        Block::Code(body) => {
            out.push_str("<pre class=\"doc-code\">");
            if let Some(render) = opts.code_renderer {
                // The renderer returns already-safe inner HTML (it escapes).
                out.push_str(&render(body));
            } else {
                out.push_str("<code>");
                out.push_str(&html_escape(body));
                out.push_str("</code>");
            }
            out.push_str("</pre>\n");
        }
        Block::Bullet(items) => {
            out.push_str("<ul class=\"doc-list\">\n");
            for item in items {
                out.push_str("<li>");
                render_spans(out, &parse_spans(item), opts);
                out.push_str("</li>\n");
            }
            out.push_str("</ul>\n");
        }
        Block::Numbered(items) => {
            out.push_str("<ol class=\"doc-list\">\n");
            for item in items {
                out.push_str("<li>");
                render_spans(out, &parse_spans(item), opts);
                out.push_str("</li>\n");
            }
            out.push_str("</ol>\n");
        }
        Block::Table(header, rows) => render_table(out, header, rows, opts),
        Block::Rule => out.push_str("<hr>\n"),
        Block::Blockquote(inner) => {
            if depth >= MAX_BLOCKQUOTE_DEPTH {
                // Bounded by construction: refuse to recurse past the ceiling.
                return;
            }
            out.push_str("<blockquote>\n");
            for b in inner {
                render_block(out, b, opts, depth + 1);
            }
            out.push_str("</blockquote>\n");
        }
    }
}

fn render_table(out: &mut String, header: &[String], rows: &[Vec<String>], opts: &WalkOptions) {
    out.push_str("<table class=\"doc-table\">\n<thead>\n<tr>");
    for cell in header {
        out.push_str("<th>");
        render_spans(out, &parse_spans(cell), opts);
        out.push_str("</th>");
    }
    out.push_str("</tr>\n</thead>\n");
    if !rows.is_empty() {
        out.push_str("<tbody>\n");
        for row in rows {
            out.push_str("<tr>");
            for cell in row {
                out.push_str("<td>");
                render_spans(out, &parse_spans(cell), opts);
                out.push_str("</td>");
            }
            out.push_str("</tr>\n");
        }
        out.push_str("</tbody>\n");
    }
    out.push_str("</table>\n");
}

fn render_spans(out: &mut String, spans: &[Span], opts: &WalkOptions) {
    for span in spans {
        render_span(out, span, opts);
    }
}

fn render_span(out: &mut String, span: &Span, opts: &WalkOptions) {
    match span {
        Span::Plain(text) => out.push_str(&html_escape(text)),
        Span::Bold(text) => {
            out.push_str("<strong>");
            render_spans(out, &parse_spans(text), opts);
            out.push_str("</strong>");
        }
        Span::Italic(text) => {
            out.push_str("<em>");
            render_spans(out, &parse_spans(text), opts);
            out.push_str("</em>");
        }
        Span::Code(text) => {
            out.push_str("<code>");
            out.push_str(&html_escape(text));
            out.push_str("</code>");
        }
        Span::Link(text, url) => {
            // Emit-boundary href gate (defence-in-depth beside the parse gate):
            // an unsafe scheme degrades to the escaped visible text, no anchor.
            if is_safe_href(url) {
                let _ = write!(
                    out,
                    "<a href=\"{}\">{}</a>",
                    html_escape(url),
                    html_escape(text)
                );
            } else {
                out.push_str(&html_escape(text));
            }
        }
        Span::Image(alt, url) => {
            // An image `src` is a fetch sink: same allowlist, fail closed to the
            // escaped alt text rather than an `<img>` with an unsafe source.
            if is_safe_href(url) {
                let _ = write!(
                    out,
                    "<img src=\"{}\" alt=\"{}\">",
                    html_escape(url),
                    html_escape(alt)
                );
            } else {
                out.push_str(&html_escape(alt));
            }
        }
        Span::HardBreak => out.push_str("<br>"),
    }
}

/// The one HTML escaper for the doc-side walker.
///
/// Escapes all five characters an HTML text or attribute context requires. `'`
/// is included (unlike a 4-char escaper) so a value emitted into a
/// single-quoted attribute context cannot break out. No text byte is ever
/// emitted unescaped, and `<`/`>` in body text are escaped rather than passed
/// through (no raw-HTML passthrough).
#[must_use]
pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Apply the caller heading offset to a level, clamped to `h6`. Mirrors the
/// incumbent's `saturating_add(offset).min(6)`.
const fn heading_number(level: HeadingLevel, offset: u8) -> u8 {
    let base: u8 = match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    };
    let shifted = base.saturating_add(offset);
    if shifted > 6 { 6 } else { shifted }
}

#[cfg(test)]
mod tests {
    use super::super::parse::parse_blocks;
    use super::*;

    fn html(src: &str, offset: u8) -> String {
        let opts = WalkOptions {
            heading_offset: offset,
            code_renderer: None,
        };
        blocks_to_html(&parse_blocks(src), &opts)
    }

    fn inline(line: &str) -> String {
        inline_to_html(line, &WalkOptions::default())
    }

    // ── Escape-by-default (the §1-critical line) ────────────────────────────

    #[test]
    fn escapes_all_five_html_specials() {
        assert_eq!(html_escape("a<b>&\"'"), "a&lt;b&gt;&amp;&quot;&#39;");
    }

    #[test]
    fn raw_script_in_body_is_escaped_not_passed_through() {
        let out = html("a <script>alert(1)</script> b", 0);
        assert!(
            !out.contains("<script>"),
            "raw <script> must be escaped: {out}"
        );
        assert!(out.contains("&lt;script&gt;"), "{out}");
    }

    #[test]
    fn raw_img_onerror_is_escaped() {
        let out = html("x <img src=z onerror=alert(1)> y", 0);
        // The literal `<img …>` in body prose is escaped, never a live element.
        assert!(!out.contains("<img src=z"), "{out}");
        assert!(out.contains("&lt;img"), "{out}");
    }

    #[test]
    fn plain_span_escapes_quote_and_apostrophe() {
        assert_eq!(inline("a 'b' \"c\""), "a &#39;b&#39; &quot;c&quot;");
    }

    // ── href/src refusals (defence-in-depth: parse AND emit) ────────────────

    #[test]
    fn safe_link_becomes_anchor() {
        assert_eq!(
            inline("[docs](guide.html)"),
            "<a href=\"guide.html\">docs</a>"
        );
    }

    #[test]
    fn safe_absolute_link_preserved() {
        assert_eq!(
            inline("[a](https://example.com/x)"),
            "<a href=\"https://example.com/x\">a</a>"
        );
    }

    #[test]
    fn javascript_link_rejected_to_plain_text() {
        let out = inline("[x](javascript:alert(1))");
        // The security property is that no live anchor carries the scheme: the
        // rejected URL degrades to escaped plain text (the literal `javascript:`
        // may appear as inert text, but never inside an `href=` attribute).
        assert!(!out.contains("href="), "no anchor for javascript: {out}");
        assert!(
            !out.contains("href=\"javascript"),
            "no javascript scheme in an href: {out}"
        );
        assert!(!out.contains("<a "), "no anchor element at all: {out}");
    }

    #[test]
    fn vbscript_and_data_links_rejected() {
        for src in [
            "[x](vbscript:msgbox(1))",
            "[x](data:text/html,<script>)",
            "[x](data:image/png;base64,AAAA)",
        ] {
            let out = inline(src);
            assert!(!out.contains("href="), "no anchor for {src}: {out}");
        }
    }

    #[test]
    fn unsafe_image_src_rejected_to_alt_text() {
        let out = inline("![alt](javascript:alert(1))");
        assert!(!out.contains("<img"), "no img for unsafe src: {out}");
        assert!(out.contains("alt"), "alt text shown: {out}");
    }

    #[test]
    fn safe_image_renders_img() {
        assert_eq!(
            inline("![logo](logo.png)"),
            "<img src=\"logo.png\" alt=\"logo\">"
        );
    }

    #[test]
    fn href_with_html_special_is_escaped_in_attribute() {
        // A safe relative href carrying a quote must be attribute-escaped.
        let out = inline("[t](a\"b)");
        assert!(out.contains("href=\"a&quot;b\""), "{out}");
    }

    // ── Heading offset lives in the caller ──────────────────────────────────

    #[test]
    fn heading_offset_applied_and_clamped() {
        assert_eq!(html("# Top", 2), "<h3>Top</h3>\n");
        assert_eq!(html("## Sub", 2), "<h4>Sub</h4>\n");
        // Faithful H1 with no offset stays h1 (parser emits true levels).
        assert_eq!(html("# Top", 0), "<h1>Top</h1>\n");
        // Clamp: H6 + 2 saturates at h6, never h8.
        assert_eq!(html("###### Deep", 2), "<h6>Deep</h6>\n");
    }

    // ── Structural forms ────────────────────────────────────────────────────

    #[test]
    fn emphasis_and_code_spans() {
        assert_eq!(
            inline("a **b** *i* `c`"),
            "a <strong>b</strong> <em>i</em> <code>c</code>"
        );
    }

    #[test]
    fn bullet_and_numbered_lists() {
        assert_eq!(
            html("- a\n- b", 0),
            "<ul class=\"doc-list\">\n<li>a</li>\n<li>b</li>\n</ul>\n"
        );
        assert_eq!(
            html("1. a\n2. b", 0),
            "<ol class=\"doc-list\">\n<li>a</li>\n<li>b</li>\n</ol>\n"
        );
    }

    #[test]
    fn table_renders_thead_and_tbody() {
        let out = html("| A | B |\n|---|---|\n| 1 | 2 |", 0);
        assert!(out.contains("<table class=\"doc-table\">"), "{out}");
        assert!(out.contains("<th>A</th>"), "{out}");
        assert!(out.contains("<td>1</td>"), "{out}");
    }

    #[test]
    fn rule_renders_hr() {
        assert_eq!(html("---", 0), "<hr>\n");
    }

    #[test]
    fn blockquote_nests() {
        let out = html("> outer\n> > inner", 0);
        assert!(out.contains("<blockquote>"), "{out}");
        assert!(out.matches("<blockquote>").count() == 2, "nested: {out}");
    }

    #[test]
    fn blockquote_depth_is_bounded() {
        // Genuinely nested blockquotes far past the ceiling: `> > > … x`. The
        // parser nests one level per `> ` prefix; the walker must cap recursion
        // at MAX_BLOCKQUOTE_DEPTH rather than emit an unbounded stack of tags
        // (bounded by construction, principle 3). Reaching the assertion at all
        // proves no stack blow-up / panic.
        let markers = "> ".repeat(MAX_BLOCKQUOTE_DEPTH + 50);
        let src = format!("{markers}x");
        let out = html(&src, 0);
        let opened = out.matches("<blockquote>").count();
        assert!(
            opened <= MAX_BLOCKQUOTE_DEPTH,
            "blockquote nesting must cap at the ceiling, got {opened}"
        );
        assert_eq!(
            opened, MAX_BLOCKQUOTE_DEPTH,
            "deep input should reach the ceiling: {opened}"
        );
    }

    #[test]
    fn code_block_falls_back_to_escaped_code_without_renderer() {
        let out = html("```\n<script>\n```", 0);
        assert!(out.contains("<pre class=\"doc-code\"><code>"), "{out}");
        assert!(out.contains("&lt;script&gt;"), "code body escaped: {out}");
        assert!(!out.contains("<script>"), "{out}");
    }

    #[test]
    fn code_block_uses_caller_renderer_verbatim() {
        let opts = WalkOptions {
            heading_offset: 0,
            code_renderer: Some(&|body: &str| format!("<code class=\"hl\">{body}</code>")),
        };
        let out = blocks_to_html(&parse_blocks("```ipe\nfoo\n```"), &opts);
        assert!(out.contains("<code class=\"hl\">foo</code>"), "{out}");
    }
}

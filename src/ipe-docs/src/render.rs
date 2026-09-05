#![forbid(unsafe_code)]
//! HTML rendering for the static documentation site.
//!
//! Every code snippet is highlighted by feeding it through
//! [`ipe_annotate::annotate_syntax_only`], mapping each [`TokenClass`] to a
//! CSS class.  Definition links come from a token's [`DefKey`]: a
//! `DefKey::Kernel { module, name }` maps to `/symbol/<module>.<name>/`, and
//! `DefKey::TopLevel { module, name }` maps to `/symbol/<module>.<name>/`.
//! No hand-rolled tokenizer or linker is involved.

use std::fmt::Write as _;

use ipe_annotate::{DefKey, TokenClass, annotate_syntax_only};
use ipe_intern::Interner;
use ipe_parse::parse_module;

// ── CSS class mapping ─────────────────────────────────────────────────────────

/// Map a [`TokenClass`] to its CSS class name.
///
/// The names follow the conventional syntax-highlight vocabulary so a
/// stylesheet written against them works without knowing Ipê specifics.
#[must_use]
pub const fn css_class(class: TokenClass) -> &'static str {
    match class {
        TokenClass::Keyword => "kw",
        TokenClass::Type => "ty",
        TokenClass::TypeVar => "tv",
        TokenClass::Function => "fn",
        TokenClass::Kernel => "kn",
        TokenClass::Constructor => "ct",
        TokenClass::Variable => "va",
        TokenClass::Module => "mo",
        TokenClass::Operator => "op",
        TokenClass::StringLit => "st",
        TokenClass::Number => "nm",
        TokenClass::Comment => "cm",
        TokenClass::Punctuation => "pu",
    }
}

// ── DefKey → URL ──────────────────────────────────────────────────────────────

/// Map a [`DefKey`] to the URL of the documentation page for that entity.
///
/// Returned paths are root-relative (`/symbol/…`).  A consumer that needs a
/// different URL scheme (e.g. relative paths or a CDN prefix) wraps this
/// function.
#[must_use]
pub fn def_url(key: &DefKey) -> String {
    match key {
        DefKey::Kernel { module, name } | DefKey::TopLevel { module, name } => {
            format!("/symbol/{module}.{name}/")
        }
        DefKey::Constructor {
            module,
            type_name,
            name,
        } => {
            format!("/symbol/{module}.{type_name}.{name}/")
        }
    }
}

// ── Code snippet highlighter ──────────────────────────────────────────────────

/// Highlight a fenced Ipê snippet, returning an HTML `<code>` block.
///
/// The snippet is parsed with the real lexer via
/// [`annotate_syntax_only`] (no hand-rolled tokenizer).  Each token becomes a
/// `<span class="…">` carrying the appropriate CSS class.  Tokens whose
/// [`DefKey`] resolves to a known page are wrapped in `<a href="…">` links.
/// Text between tokens is HTML-escaped and emitted verbatim.
///
/// When the snippet does not parse (e.g. it is an excerpt, not a full module),
/// the text is HTML-escaped and emitted undecorated inside a `<code>` tag.
#[must_use]
pub fn highlight_snippet(source: &str) -> String {
    // Attempt to parse as a full module; fall back to plain-escaped text when
    // the snippet is not a complete module (most doc-string examples are not).
    let mut interner = Interner::default();
    let Ok(syntax) = parse_module(source, &mut interner) else {
        return format!("<code>{}</code>", html_escape(source));
    };
    let tokens = annotate_syntax_only(&syntax, &interner);
    build_highlighted(source, &tokens)
}

/// Core highlighter: given `source` and its annotated token stream, produce
/// the `<code>…</code>` inner HTML.
///
/// This is the pure, testable entry point used by tests to verify that a
/// specific token class and link appear for a given snippet.
#[must_use]
pub fn build_highlighted(source: &str, tokens: &[ipe_annotate::AnnotatedToken]) -> String {
    let mut out = String::from("<code>");
    let mut cursor: usize = 0;
    let src_bytes = source.as_bytes();

    for tok in tokens {
        let start = tok.byte_start as usize;
        let end = (tok.byte_start + tok.byte_len) as usize;

        // Guard: skip tokens with out-of-range spans (should not occur with a
        // well-formed annotated stream, but we must not panic on bad data).
        if start > src_bytes.len() || end > src_bytes.len() || start > end {
            continue;
        }

        // Emit any text between the previous token and this one.
        if cursor < start {
            let gap = source.get(cursor..start).unwrap_or("");
            out.push_str(&html_escape(gap));
        }

        // Retrieve the source slice for this token.
        let text = source.get(start..end).unwrap_or("");
        let class = css_class(tok.class);

        if let Some(def) = &tok.def {
            let url = def_url(def);
            let escaped = html_escape(text);
            // `write!` on a `String` is infallible; the `Result` is intentionally
            // discarded rather than suppressed with `#[allow]`.
            let _ = write!(out, r#"<a href="{url}" class="{class}">{escaped}</a>"#);
        } else {
            let escaped = html_escape(text);
            let _ = write!(out, r#"<span class="{class}">{escaped}</span>"#);
        }

        cursor = end;
    }

    // Emit any trailing text after the last token.
    if cursor < source.len() {
        let tail = source.get(cursor..).unwrap_or("");
        out.push_str(&html_escape(tail));
    }

    out.push_str("</code>");
    out
}

// ── HTML helpers ──────────────────────────────────────────────────────────────

/// Escape `<`, `>`, `&`, `"` for safe embedding in HTML attribute values or
/// element content.
#[must_use]
pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

// ── Page template ─────────────────────────────────────────────────────────────

/// Wrap `body_html` in a minimal HTML5 document with the shared stylesheet.
///
/// `title` is the document title (HTML-escaped).  `body_html` is trusted
/// pre-rendered HTML and is inserted verbatim.
#[must_use]
pub fn page(title: &str, body_html: &str) -> String {
    let escaped_title = html_escape(title);
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{escaped_title} — Ipê documentation</title>
<link rel="stylesheet" href="/style.css">
</head>
<body>
<nav><a href="/">Ipê docs</a></nav>
<main>
{body_html}
</main>
</body>
</html>
"#
    )
}

/// The shared stylesheet, emitted once to `style.css` in the output tree.
pub const STYLESHEET: &str = r#"/* Ipê documentation — minimal stylesheet */
*, *::before, *::after { box-sizing: border-box; }
:root {
  --bg: #fafafa;
  --fg: #1a1a1a;
  --border: #ddd;
  --pre-bg: #f4f4f4;
  --link: #0055cc;
  --kw: #7c3aed;
  --ty: #0e7490;
  --tv: #6b7280;
  --fn: #1d4ed8;
  --kn: #0369a1;
  --ct: #b45309;
  --va: #374151;
  --mo: #4338ca;
  --op: #374151;
  --st: #166534;
  --nm: #9a3412;
  --cm: #6b7280;
}
body {
  font-family: system-ui, sans-serif;
  background: var(--bg);
  color: var(--fg);
  max-width: 900px;
  margin: 0 auto;
  padding: 1rem 1.5rem 4rem;
  line-height: 1.6;
}
nav { margin-bottom: 1.5rem; font-size: 0.875rem; }
nav a { color: var(--link); text-decoration: none; }
nav a:hover { text-decoration: underline; }
h1 { font-size: 1.75rem; margin: 0 0 0.25rem; }
h2 { font-size: 1.25rem; margin: 2rem 0 0.5rem; border-bottom: 1px solid var(--border); padding-bottom: 0.25rem; }
pre {
  background: var(--pre-bg);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 0.875rem 1rem;
  overflow-x: auto;
  font-size: 0.9rem;
}
code {
  font-family: ui-monospace, "Cascadia Code", "Fira Code", monospace;
  font-size: 0.875rem;
}
p > code, li > code {
  background: var(--pre-bg);
  border: 1px solid var(--border);
  border-radius: 3px;
  padding: 0.1em 0.35em;
}
a { color: var(--link); }
a:hover { text-decoration: underline; }
ul.index-list { list-style: none; padding: 0; columns: 2; }
ul.index-list li { padding: 0.2rem 0; }
.kind-badge {
  display: inline-block;
  font-size: 0.7rem;
  padding: 0.1rem 0.4rem;
  border-radius: 3px;
  margin-left: 0.5rem;
  background: var(--pre-bg);
  border: 1px solid var(--border);
  vertical-align: middle;
}
/* Syntax highlighting */
.kw { color: var(--kw); font-weight: 600; }
.ty { color: var(--ty); }
.tv { color: var(--tv); font-style: italic; }
.fn { color: var(--fn); }
.kn { color: var(--kn); font-weight: 500; }
.ct { color: var(--ct); }
.va { color: var(--va); }
.mo { color: var(--mo); }
.op { color: var(--op); }
.st { color: var(--st); }
.nm { color: var(--nm); }
.cm { color: var(--cm); font-style: italic; }
"#;

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ipe_annotate::{AnnotatedToken, DefKey, TokenClass};

    use super::{build_highlighted, css_class, def_url, html_escape, page};

    // ── css_class ────────────────────────────────────────────────────────────

    #[test]
    fn css_class_all_variants_non_empty() {
        let variants = [
            TokenClass::Keyword,
            TokenClass::Type,
            TokenClass::TypeVar,
            TokenClass::Function,
            TokenClass::Kernel,
            TokenClass::Constructor,
            TokenClass::Variable,
            TokenClass::Module,
            TokenClass::Operator,
            TokenClass::StringLit,
            TokenClass::Number,
            TokenClass::Comment,
            TokenClass::Punctuation,
        ];
        for v in &variants {
            let c = css_class(*v);
            assert!(!c.is_empty(), "css_class({v:?}) must be non-empty");
        }
    }

    #[test]
    fn kernel_maps_to_kn() {
        assert_eq!(css_class(TokenClass::Kernel), "kn");
    }

    // ── def_url ──────────────────────────────────────────────────────────────

    #[test]
    fn def_url_kernel() {
        let key = DefKey::Kernel {
            module: "List".into(),
            name: "map".into(),
        };
        assert_eq!(def_url(&key), "/symbol/List.map/");
    }

    #[test]
    fn def_url_top_level() {
        let key = DefKey::TopLevel {
            module: "Main".into(),
            name: "main".into(),
        };
        assert_eq!(def_url(&key), "/symbol/Main.main/");
    }

    #[test]
    fn def_url_constructor() {
        let key = DefKey::Constructor {
            module: "Ipe.Maybe".into(),
            type_name: "Maybe".into(),
            name: "Just".into(),
        };
        assert_eq!(def_url(&key), "/symbol/Ipe.Maybe.Maybe.Just/");
    }

    // ── html_escape ───────────────────────────────────────────────────────────

    #[test]
    fn html_escape_no_special() {
        assert_eq!(html_escape("hello"), "hello");
    }

    #[test]
    fn html_escape_all_special() {
        assert_eq!(html_escape("<>&\""), "&lt;&gt;&amp;&quot;");
    }

    #[test]
    fn html_escape_empty() {
        assert_eq!(html_escape(""), "");
    }

    // ── build_highlighted ────────────────────────────────────────────────────

    /// The core test mandated by the spec: a snippet's `List.map` token
    /// annotated as Kernel with its `DefKey` renders with class "kn" AND links
    /// to the `List.map` page.
    #[test]
    fn kernel_token_gets_kn_class_and_link() {
        let source = "List.map f xs";
        let tokens = vec![ipe_annotate::AnnotatedToken {
            byte_start: 0,
            byte_len: 8, // "List.map"
            class: TokenClass::Kernel,
            def: Some(DefKey::Kernel {
                module: "List".into(),
                name: "map".into(),
            }),
        }];
        let html = build_highlighted(source, &tokens);
        assert!(
            html.contains("class=\"kn\""),
            "kernel token must carry kn class; got: {html}"
        );
        assert!(
            html.contains("href=\"/symbol/List.map/\""),
            "kernel token must link to /symbol/List.map/; got: {html}"
        );
    }

    /// A name inside a string literal must not be linked (def = None enforced
    /// by annotate; we verify that our renderer never emits a link for
    /// def=None tokens).
    #[test]
    fn no_link_for_def_none_token() {
        let source = "\"List.map\"";
        let tokens = vec![AnnotatedToken {
            byte_start: 0,
            byte_len: 10,
            class: TokenClass::StringLit,
            def: None,
        }];
        let html = build_highlighted(source, &tokens);
        assert!(
            !html.contains("<a "),
            "token with def=None must not produce a link; got: {html}"
        );
        assert!(
            html.contains("class=\"st\""),
            "string token must carry st class; got: {html}"
        );
    }

    #[test]
    fn gap_text_is_escaped() {
        // Two tokens with a gap containing `<` — the gap must be escaped.
        let source = "a<b";
        let tokens = vec![
            AnnotatedToken {
                byte_start: 0,
                byte_len: 1,
                class: TokenClass::Variable,
                def: None,
            },
            AnnotatedToken {
                byte_start: 2,
                byte_len: 1,
                class: TokenClass::Variable,
                def: None,
            },
        ];
        let html = build_highlighted(source, &tokens);
        assert!(
            html.contains("&lt;"),
            "gap `<` must be escaped; got: {html}"
        );
    }

    #[test]
    fn empty_source_produces_empty_code_block() {
        let html = build_highlighted("", &[]);
        assert_eq!(html, "<code></code>");
    }

    #[test]
    fn out_of_range_token_skipped() {
        // A token whose span exceeds source length must be silently skipped,
        // not panic.
        let source = "ab";
        let tokens = vec![AnnotatedToken {
            byte_start: 100,
            byte_len: 5,
            class: TokenClass::Keyword,
            def: None,
        }];
        let html = build_highlighted(source, &tokens);
        // Source text emitted verbatim (gap only), no panic.
        assert!(
            html.contains("ab"),
            "source text still emitted; got: {html}"
        );
    }

    // ── page ─────────────────────────────────────────────────────────────────

    #[test]
    fn page_contains_title_and_nav() {
        let html = page("Ipe.List", "<p>content</p>");
        assert!(html.contains("<title>Ipe.List"), "title present");
        assert!(html.contains("<nav>"), "nav present");
        assert!(html.contains("<p>content</p>"), "body content present");
    }

    #[test]
    fn page_escapes_title() {
        let html = page("A<B>C", "<p/>");
        assert!(
            html.contains("A&lt;B&gt;C"),
            "title special chars escaped; got: {html}"
        );
    }
}

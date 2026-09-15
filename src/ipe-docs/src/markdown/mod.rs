//! Doc-side Markdown parse model — a std-only hand-port of the `Ipe.Markdown`
//! parser (`src/stdlib/Ipe/Markdown.ipe`).
//!
//! `Ipe.Markdown` is the language's Markdown authority: a pure-Ipê parser that
//! turns Markdown source into a `Block` / `Span` tree. The `ipe doc` HTML site
//! cannot run Ipê at doc-time, so the parser is hand-ported here and kept
//! honest against `Ipe.Markdown` by a semantic-parity gate that snapshots the
//! expected tree from an actual `ipe` run.
//!
//! This module is the single home for the URL-scheme allowlist (`is_safe_href`)
//! and the parse tree. It is std-only — no runtime dependency, no indexing /
//! `unwrap` / `panic` — so it is deny-set-clean by construction.
//!
//! The Ipê source operates on Unicode-codepoint indices (`String.slice`,
//! `String.length` count `char`s, not bytes), so this port operates over
//! `&[char]` slices to reproduce that indexing exactly.

pub mod parity;
pub mod parse;
pub mod sexpr;
pub mod walker;

/// The six Markdown heading levels. Mirrors `Ipe.Markdown.HeadingLevel`: a
/// `HeaderBlock` carries exactly one, so levels 0, 7, and negatives are not
/// representable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadingLevel {
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
}

/// A block is one paragraph-shaped chunk of a Markdown document. Mirrors
/// `Ipe.Markdown.Block` (8 constructors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Header(HeadingLevel, String),
    Para(String),
    Code(String),
    Bullet(Vec<String>),
    Numbered(Vec<String>),
    /// Header cells, then body rows.
    Table(Vec<String>, Vec<Vec<String>>),
    Rule,
    Blockquote(Vec<Self>),
}

/// A single styled run of text within a line. Mirrors `Ipe.Markdown.Span`
/// (7 constructors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Span {
    Plain(String),
    Bold(String),
    Italic(String),
    Code(String),
    /// Link text, then URL.
    Link(String, String),
    /// Image alt text, then URL.
    Image(String, String),
    HardBreak,
}

/// The single URL-scheme allowlist SSOT for the doc path.
///
/// A link target is safe when it is scheme-less (relative / fragment) or an
/// `http` / `https` / `mailto` absolute — never `javascript:`, `vbscript:`, or
/// a `data:` scheme. A `:` that follows a valid scheme name (before any `/`,
/// `?`, or `#`) marks an absolute URL; only the safe schemes are admitted, and
/// every unrecognised scheme is refused (fail-closed).
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
        return matches!(scheme, "http" | "https" | "mailto");
    }
    true
}

// ── Codepoint-string helpers ────────────────────────────────────────────────
//
// Faithful ports of the `Ipe.String` kernels the parser uses, over `&[char]`
// so codepoint indices match the Ipê source exactly. Kept private to this
// module: the parser is their only consumer.

/// `String.slice` — codepoint-indexed slice with negative-from-end + clamping.
/// Mirrors `runtime string_slice`: `start`/`end` count codepoints; a negative
/// index counts from the end; both clamp into `[0, len]`; `start > end` yields
/// `""`. Works in `isize` so no lossy `usize`↔`i64` casts arise.
pub(crate) fn slice(start: isize, end: isize, s: &[char]) -> String {
    let total = isize::try_from(s.len()).unwrap_or(isize::MAX);
    let lo = normalise(start, total).max(0);
    let hi = normalise(end, total).min(total);
    if lo > hi {
        return String::new();
    }
    // `lo`/`hi` are within `[0, total]` with `lo <= hi`, so both convert
    // losslessly and the range is in bounds; `.get` keeps it total regardless.
    let (lo, hi) = (
        usize::try_from(lo).unwrap_or(0),
        usize::try_from(hi).unwrap_or(0),
    );
    s.get(lo..hi)
        .map(|r| r.iter().collect())
        .unwrap_or_default()
}

/// Resolve a possibly-negative codepoint index against a length (negative
/// counts from the end). Not clamped to `[0, len]` — the caller clamps per its
/// `min`/`max` need.
const fn normalise(idx: isize, total: isize) -> isize {
    if idx < 0 { idx + total } else { idx }
}

/// A codepoint-slice length as an `isize` index (saturating), for passing to
/// [`slice`] without a lossy `usize as isize` cast.
pub(crate) fn len_i(s: &[char]) -> isize {
    isize::try_from(s.len()).unwrap_or(isize::MAX)
}

/// The first occurrence (codepoint index) of `needle` in `hay`, or `None`.
/// Mirrors the parser's `indexOfFirst` (which returns `-1` on miss).
pub(crate) fn index_of_first(needle: &[char], hay: &[char]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    // `last` cannot underflow: `needle.len() > hay.len()` short-circuits `None`.
    let last = hay.len().checked_sub(needle.len())?;
    (0..=last).find(|&pos| hay.get(pos..pos + needle.len()) == Some(needle))
}

/// Whether `s` begins with `prefix` (codepoint slices).
pub(crate) fn starts_with(prefix: &str, s: &[char]) -> bool {
    let p: Vec<char> = prefix.chars().collect();
    s.get(..p.len()) == Some(p.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_safe_href_allows_safe_schemes_and_relative() {
        assert!(is_safe_href("http://example.com"));
        assert!(is_safe_href("https://example.com"));
        assert!(is_safe_href("mailto:a@b.com"));
        assert!(is_safe_href("guide.html"));
        assert!(is_safe_href("../rel/path"));
        assert!(is_safe_href("#fragment"));
        assert!(is_safe_href("/absolute/path"));
        assert!(is_safe_href(""));
    }

    #[test]
    fn is_safe_href_rejects_script_schemes() {
        assert!(!is_safe_href("javascript:alert(1)"));
        assert!(!is_safe_href("JavaScript:alert(1)"));
        assert!(!is_safe_href("  javascript:alert(1)"));
        assert!(!is_safe_href("vbscript:msgbox(1)"));
        assert!(!is_safe_href("data:text/html,<script>"));
        assert!(!is_safe_href("data:image/png;base64,AAAA"));
        assert!(!is_safe_href("file:///etc/passwd"));
    }

    #[test]
    fn slice_matches_codepoint_semantics() {
        let s: Vec<char> = "café".chars().collect();
        assert_eq!(slice(0, 4, &s), "café");
        assert_eq!(slice(3, 4, &s), "é");
        assert_eq!(
            slice(2, 4, "hello".chars().collect::<Vec<_>>().as_slice()),
            "ll"
        );
        // Negative-from-end and clamp.
        assert_eq!(slice(-1, 4, &s), "é");
        assert_eq!(slice(0, 99, &s), "café");
        assert_eq!(slice(3, 1, &s), "");
    }

    #[test]
    fn index_of_first_codepoint_index() {
        let hay: Vec<char> = "café x".chars().collect();
        assert_eq!(index_of_first(&['x'], &hay), Some(5));
        assert_eq!(index_of_first(&['é'], &hay), Some(3));
        assert_eq!(index_of_first(&['z'], &hay), None);
    }
}

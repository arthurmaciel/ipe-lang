//! Doc-side Markdown parse model — a std-only hand-port of the `Ipe.Markdown`
//! parser (`src/stdlib/Ipe/Markdown.ipe`).
//!
//! `Ipe.Markdown` is the language's Markdown authority: a pure-Ipê parser that
//! turns Markdown source into a `Block` / `Span` tree. The `ipe doc` HTML site
//! cannot run Ipê at doc-time, so the parser is hand-ported here and kept
//! honest against `Ipe.Markdown` by a semantic-parity gate that snapshots the
//! expected tree from an actual `ipe` run.
//!
//! This module is the single home for the URL-scheme allowlist (`SafeHref`)
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

/// Maximum blockquote nesting depth the doc path will build and render.
///
/// The single SSOT ceiling shared by the parser and the walker (principle 3,
/// bounded by construction). The parser refuses to descend into a blockquote
/// tree deeper than this, so a pathological `>>>>…` document (e.g. a 200 KB
/// README of `> ` markers) can neither overflow the parse recursion nor build a
/// tree whose recursive `Drop` overflows the stack. The walker caps its
/// rendering recursion at the same ceiling as a second, independent boundary.
/// Far above any real document's nesting.
pub const MAX_BLOCKQUOTE_DEPTH: usize = 32;

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
    /// Link text, then a proven-safe target.
    Link(String, SafeHref),
    /// Image alt text, then a proven-safe source.
    Image(String, SafeHref),
    HardBreak,
}

/// A URL target that has passed the doc-path scheme allowlist.
///
/// This is the parse-don't-validate boundary for the single injection surface
/// of the Markdown path: an untrusted URL is normalised and vetted ONCE, at
/// construction, and the proof of safety lives in the type. A `Span::Link` /
/// `Span::Image` can only carry a `SafeHref`, so no downstream site can emit an
/// `href` / `src` that skipped the gate — the unsafe scheme has no
/// representation to reach the emitter with.
///
/// The wrapped string is the *normalised* target (the exact bytes safe to place
/// in an attribute after HTML-escaping), never the raw source. Construction is
/// the only way in; there is no public constructor that bypasses [`parse`].
///
/// [`parse`]: SafeHref::parse
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeHref(String);

impl SafeHref {
    /// The scheme allowlist SSOT for the doc path. A target is admitted only
    /// when it is scheme-less (relative / fragment) or carries one of these
    /// absolute schemes — never `javascript:`, `vbscript:`, or a `data:`
    /// scheme.
    const ALLOWED_SCHEMES: [&'static str; 3] = ["http", "https", "mailto"];

    /// Parse an untrusted URL into a proven-safe target, or refuse (`None`).
    ///
    /// Fail-closed by construction: the safe outcome is the only one a caller
    /// can obtain, and absent proof the target is safe the answer is refusal.
    ///
    /// A browser strips ASCII tab (`\t`), line feed (`\n`), and carriage return
    /// (`\r`) from a URL *before* it detects the scheme, so `java&#9;script:`
    /// re-forms `javascript:` in the browser. We strip the same three
    /// characters from the whole URL first, so the scheme we test is the scheme
    /// the browser will act on (no scheme-splitting oracle). Then, over the
    /// cleaned URL:
    ///
    /// * a `:` that precedes any `/`, `?`, or `#` marks an absolute scheme; the
    ///   scheme region MUST be non-empty and contain only `[a-z0-9+-.]` (any
    ///   other byte — control, whitespace, NUL — means it is not a valid scheme
    ///   name, so it is refused, never waved through), and the lowercased
    ///   scheme MUST be in [`ALLOWED_SCHEMES`];
    /// * no such `:` means a scheme-less relative / fragment target, which is
    ///   admitted.
    ///
    /// [`ALLOWED_SCHEMES`]: SafeHref::ALLOWED_SCHEMES
    #[must_use]
    pub fn parse(url: &str) -> Option<Self> {
        // Strip the exact characters a browser removes before scheme detection,
        // then trim surrounding ASCII whitespace. The result is what the
        // browser would act on and what we store.
        let cleaned: String = url
            .chars()
            .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
            .collect();
        let cleaned = cleaned.trim();

        // Find the scheme delimiter: the first `:` that is not preceded by a
        // path/query/fragment separator (those mark a scheme-less target).
        let scheme_end = cleaned.char_indices().find_map(|(i, c)| match c {
            ':' => Some(Some(i)),
            '/' | '?' | '#' => Some(None),
            _ => None,
        });

        match scheme_end {
            // A `:` before any `/`, `?`, `#`: an absolute URL — the scheme must
            // be a valid name AND in the allowlist, else refuse (fail-closed).
            Some(Some(colon)) => {
                let scheme = cleaned.get(..colon)?;
                let valid_name = !scheme.is_empty()
                    && scheme
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
                let lower = scheme.to_ascii_lowercase();
                if valid_name && Self::ALLOWED_SCHEMES.contains(&lower.as_str()) {
                    Some(Self(cleaned.to_owned()))
                } else {
                    None
                }
            }
            // Scheme-less (relative / fragment) or no `:` at all: admitted.
            _ => Some(Self(cleaned.to_owned())),
        }
    }

    /// The normalised, proven-safe target.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
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

    fn accepts(url: &str) -> bool {
        SafeHref::parse(url).is_some()
    }

    #[test]
    fn safe_href_allows_safe_schemes_and_relative() {
        assert!(accepts("http://example.com"));
        assert!(accepts("https://example.com"));
        assert!(accepts("mailto:a@b.com"));
        assert!(accepts("guide.html"));
        assert!(accepts("../rel/path"));
        assert!(accepts("#fragment"));
        assert!(accepts("/absolute/path"));
        assert!(accepts(""));
    }

    #[test]
    fn safe_href_rejects_script_schemes() {
        assert!(!accepts("javascript:alert(1)"));
        assert!(!accepts("JavaScript:alert(1)"));
        assert!(!accepts("  javascript:alert(1)"));
        assert!(!accepts("vbscript:msgbox(1)"));
        assert!(!accepts("data:text/html,<script>"));
        assert!(!accepts("data:image/png;base64,AAAA"));
        assert!(!accepts("file:///etc/passwd"));
    }

    #[test]
    fn safe_href_rejects_scheme_with_interior_control_or_space() {
        // A browser strips tab/LF/CR from a URL before it detects the scheme,
        // so each of these re-forms a live `javascript:` scheme. The fail-open
        // predicate waved these through on the "scheme has a non-scheme char →
        // skip the allowlist → return true" path; the smart constructor strips
        // exactly what the browser strips and then refuses the reconstructed
        // unsafe scheme.
        assert!(!accepts("java\tscript:alert(1)"));
        assert!(!accepts("java\nscript:alert(1)"));
        assert!(!accepts("java\rscript:alert(1)"));
        assert!(!accepts("javascript\t:alert(1)"));
        assert!(!accepts("javascript\n:alert(1)"));
        // A NUL or interior space is NOT stripped by the browser, so the scheme
        // name is simply invalid — refused, never waved through.
        assert!(!accepts("java\0script:alert(1)"));
        assert!(!accepts("java script:alert(1)"));
        // Leading newline/CR/tab before the scheme.
        assert!(!accepts("\njavascript:alert(1)"));
        assert!(!accepts("\r\tjavascript:alert(1)"));
    }

    #[test]
    fn safe_href_stores_the_normalised_target() {
        // Stored value is what the browser would act on: interior tab/LF/CR are
        // gone, surrounding whitespace trimmed. (Uses a safe scheme so the
        // normalisation, not the refusal, is observed.)
        let h = SafeHref::parse("  https://ex\tam\nple.com/x  ").expect("safe");
        assert_eq!(h.as_str(), "https://example.com/x");
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

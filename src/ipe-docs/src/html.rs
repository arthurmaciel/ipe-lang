//! The compiler-side HTML escaper — the one owner every generated page uses.
//!
//! Escapes all five of `& < > " '`, so one function is safe in element text and
//! in a single- or double-quoted attribute value alike: the caller never has to
//! pick the right form for the context. URL attributes additionally pass their
//! own scheme gate (`markdown::SafeHref`); escaping alone does not make a URL
//! safe.

/// Escape `text` for an HTML text or quoted-attribute context.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    escape_into(text, &mut out);
    out
}

/// Append `text` to `out`, escaped as [`escape`] does.
pub fn escape_into(text: &str, out: &mut String) {
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
}

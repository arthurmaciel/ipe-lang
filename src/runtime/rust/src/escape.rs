//! The runtime's one HTML escaper.
//!
//! Every runtime site that writes text into HTML (the `Html` render sink, the
//! `Ipe.Html` escape kernels, the dev-console banner, the debugger overlay)
//! escapes through this module. It is std-only and declared in every runtime
//! module set, so a program that serves HTML without importing `Ipe.Html` (a
//! headless `Ipe.Http.Server` serving the dev banner) still has it.
//!
//! Byte contract (the rendered goldens and e2e expectations depend on it):
//! - text form: `&` `<` `>` `'` become `&amp;` `&lt;` `&gt;` `&#39;`; `"` stays
//!   raw;
//! - attribute form: the text form plus `"` as `&#34;` (never `&quot;`).
//!
//! Both forms escape `'`, so a value is safe in a single- or double-quoted
//! attribute alike. URL-bearing attributes are scheme-checked separately, at
//! the render sink.

/// Append `t`, escaped for HTML text content, to `out`.
///
/// `"` is left raw: it carries no meaning in text content.
pub fn html_text_into(t: &str, out: &mut String) {
    escape_into(t, false, out);
}

/// Append `t`, escaped for a quoted HTML attribute value, to `out`.
///
/// Safe in single- and double-quoted attributes.
pub fn html_attr_into(t: &str, out: &mut String) {
    escape_into(t, true, out);
}

/// Escape `t` for HTML text content.
///
/// The allocating form of [`html_text_into`].
#[must_use]
pub fn html_text(t: &str) -> String {
    let mut out = String::with_capacity(t.len() + 8);
    html_text_into(t, &mut out);
    out
}

/// Escape `t` for a quoted HTML attribute value.
///
/// The allocating form of [`html_attr_into`].
#[must_use]
pub fn html_attr(t: &str) -> String {
    let mut out = String::with_capacity(t.len() + 8);
    html_attr_into(t, &mut out);
    out
}

/// Single-pass escape of `t` into `out`.
///
/// One original-to-output map never re-scans its own output; the
/// metacharacter-free common case appends the input verbatim.
fn escape_into(t: &str, escape_quote: bool, out: &mut String) {
    if !t.contains(['&', '<', '>', '\'', '"']) {
        out.push_str(t);
        return;
    }
    out.reserve(t.len() + 8);
    for c in t.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&#39;"),
            '"' if escape_quote => out.push_str("&#34;"),
            _ => out.push(c),
        }
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::{html_attr, html_attr_into, html_text, html_text_into};

    #[test]
    fn attr_form_escapes_all_five_with_numeric_quote_entities() {
        assert_eq!(html_attr("\"'<>&"), "&#34;&#39;&lt;&gt;&amp;");
    }

    #[test]
    fn text_form_leaves_double_quote_raw() {
        assert_eq!(html_text("\""), "\"");
        assert_eq!(html_text("\"'<>&"), "\"&#39;&lt;&gt;&amp;");
    }

    #[test]
    fn into_forms_append_and_match_allocating_forms() {
        let mut out = String::from("x");
        html_attr_into("a\"b", &mut out);
        html_text_into("c\"d", &mut out);
        assert_eq!(out, "xa&#34;bc\"d");
    }

    #[test]
    fn metacharacter_free_input_is_unchanged() {
        assert_eq!(html_attr("plain text 123"), "plain text 123");
        assert_eq!(html_text(""), "");
    }
}

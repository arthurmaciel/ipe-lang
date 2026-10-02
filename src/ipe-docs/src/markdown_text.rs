//! The Markdown-prose escaper.
//!
//! The one owner every generated Markdown page uses for a table cell or a
//! `[text](dest)` link built from untrusted text.
//!
//! Each function targets one grammar position: a table cell cannot hold a
//! raw `|` or a newline (either ends the row early), a link's text cannot
//! hold a raw `]` or `\` (either ends or escapes past the link early), and a
//! link's destination cannot hold a raw `<`, `>`, space, or control
//! character once wrapped in the angle-bracket destination form.

use std::fmt::Write as _;

/// Escape `s` for a Markdown table cell.
///
/// A `|` would otherwise start a new column, and a newline would otherwise
/// end the row.
#[must_use]
pub fn table_cell(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '|' => out.push_str("\\|"),
            '\n' | '\r' => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

/// Escape `s` for the link-text position of `[text](dest)`.
///
/// A `]` would otherwise close the link text early, and a `\` would
/// otherwise escape the character that follows it.
#[must_use]
pub fn link_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' | '[' | ']' => {
                out.push('\\');
                out.push(ch);
            }
            '\n' | '\r' => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

/// Build the angle-bracket destination form `<dest>` for the link-destination
/// position of `[text](dest)`.
///
/// `<`, `>`, space, and every control character are percent-encoded so none
/// of them can close the bracket or inject a hazard, and the result is
/// always wrapped in `<...>` so a `)` or space elsewhere in `s` cannot end
/// the destination early.
#[must_use]
pub fn link_destination(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('<');
    for ch in s.chars() {
        if ch == '<' || ch == '>' || ch == ' ' || ch.is_control() {
            let mut buf = [0_u8; 4];
            for byte in ch.encode_utf8(&mut buf).as_bytes() {
                let _ = write!(out, "%{byte:02X}");
            }
        } else {
            out.push(ch);
        }
    }
    out.push('>');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_cell_escapes_pipe_and_maps_newline_to_space() {
        assert_eq!(table_cell("a|b\nc"), "a\\|b c");
        assert_eq!(table_cell("no special chars"), "no special chars");
    }

    #[test]
    fn table_cell_maps_carriage_return_to_space() {
        assert_eq!(table_cell("a\rb"), "a b");
    }

    #[test]
    fn link_text_escapes_brackets_and_backslash() {
        assert_eq!(link_text("a]b"), "a\\]b");
        assert_eq!(link_text("a[b"), "a\\[b");
        assert_eq!(link_text("a\\b"), "a\\\\b");
    }

    #[test]
    fn link_text_maps_newline_to_space() {
        assert_eq!(link_text("a\nb"), "a b");
    }

    #[test]
    fn link_destination_wraps_and_encodes_space_and_paren_survives() {
        let out = link_destination("k) x");
        assert_eq!(out, "<k)%20x>");
    }

    #[test]
    fn link_destination_encodes_angle_brackets_and_controls() {
        let out = link_destination("a<b>c\u{7}d");
        assert_eq!(out, "<a%3Cb%3Ec%07d>");
    }
}

//! The escape table shared by every Ipê string and char literal.
//!
//! The lexer reads `\letter` as [`ESCAPES`]' `value`; [`escape_str_body`] and
//! [`escape_char_body`] write a `value` back as `\letter`. One table drives
//! both directions, so a literal printed from a value re-lexes to the value
//! it was printed from.

/// The escapes an Ipê string or char literal resolves, as `(letter, value)`
/// pairs.
pub const ESCAPES: [(char, char); 7] = [
    ('n', '\n'),
    ('t', '\t'),
    ('r', '\r'),
    ('\\', '\\'),
    ('"', '"'),
    ('\'', '\''),
    ('0', '\0'),
];

/// The source text of a string literal's body (no surrounding `"`s): every
/// [`ESCAPES`] value writes back as `\letter`, except a bare `'`, which a
/// string literal never needs to escape.
#[must_use]
pub fn escape_str_body(value: &str) -> String {
    escape_body(value, '\'')
}

/// The source text of a char literal's body (no surrounding `'`s): every
/// [`ESCAPES`] value writes back as `\letter`, except a bare `"`, which a
/// char literal never needs to escape.
#[must_use]
pub fn escape_char_body(value: &str) -> String {
    escape_body(value, '"')
}

/// Shared by [`escape_str_body`] and [`escape_char_body`]: every char writes
/// back as `\letter` except `bare_quote` (the other literal kind's
/// delimiter), which stays bare since this literal kind never needs to
/// escape it.
fn escape_body(value: &str, bare_quote: char) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if c == bare_quote {
            out.push(c);
            continue;
        }
        match ESCAPES.iter().find(|(_, v)| *v == c) {
            Some((letter, _)) => {
                out.push('\\');
                out.push(*letter);
            }
            None => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{ESCAPES, escape_char_body, escape_str_body};

    /// Every table value writes back as its letter, in both a string and a
    /// char body, except each literal kind's own bare delimiter.
    #[test]
    fn every_escape_writes_back_as_its_letter() {
        for (letter, value) in ESCAPES {
            let text = value.to_string();
            if value == '\'' {
                assert_eq!(escape_str_body(&text), text, "bare in a string: {letter}");
            } else {
                assert_eq!(escape_str_body(&text), format!("\\{letter}"));
            }
            if value == '"' {
                assert_eq!(escape_char_body(&text), text, "bare in a char: {letter}");
            } else {
                assert_eq!(escape_char_body(&text), format!("\\{letter}"));
            }
        }
    }

    /// Text outside the table prints unchanged.
    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(escape_str_body("plain é"), "plain é");
        assert_eq!(escape_char_body("é"), "é");
    }
}

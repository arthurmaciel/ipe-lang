//! JSON string escaping for display and log records, and for `<script>` embedding.
//!
//! The compiler-side owner of the JSON display role: every `--json` record the
//! CLI prints and every JSON fragment it embeds in a generated page escapes its
//! strings here. The output is valid JSON that decodes to the input, with every
//! terminal hazard (`Cc ∪ Cf ∪ Zl ∪ Zp`, the set [`crate::terminal`] owns)
//! spelled as a `\u` escape, so a bidi override or a C1 control in source text
//! never reaches a reader raw.

use std::fmt::Write as _;

use crate::terminal::is_denied_format_char;

/// The characters a `<script>` element's data must not hold raw, with their JSON escapes.
///
/// `<` and `>` keep `</script>` and `<!--` out of the element, `&` keeps an
/// entity out of XHTML parsing, and U+2028/U+2029 are line terminators in
/// older JavaScript string grammar.
const SCRIPT_EMBED_ESCAPES: [(char, &str); 5] = [
    ('<', "\\u003c"),
    ('>', "\\u003e"),
    ('&', "\\u0026"),
    ('\u{2028}', "\\u2028"),
    ('\u{2029}', "\\u2029"),
];

/// Whether `c` is a terminal hazard: a control (`Cc`) or a denied format character.
fn is_hazard(c: char) -> bool {
    c.is_control() || is_denied_format_char(c)
}

/// The JSON string body (no surrounding quotes) of `s`, display role.
///
/// `"` and `\` are backslash-escaped, `\n` `\r` `\t` use their short forms,
/// every other hazard is `\uXXXX` in lowercase hex (an astral one as its
/// UTF-16 surrogate pair), and every other character is kept as is.
#[must_use]
pub fn string_body(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    string_body_into(s, &mut out);
    out
}

/// Append the JSON string body of `s` to `out`, as [`string_body`] spells it.
pub fn string_body_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if is_hazard(c) => {
                let mut units = [0_u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
            c => out.push(c),
        }
    }
}

/// Already-encoded JSON text made safe to place inside a `<script>` element.
///
/// Each character of the five-entry script table becomes its `\u` escape. The
/// table's characters occur in encoded JSON only inside string literals, so
/// the result is valid JSON with the same decoded value.
#[must_use]
pub fn script_embed(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        match SCRIPT_EMBED_ESCAPES.iter().find(|(raw, _)| *raw == c) {
            Some((_, escaped)) => out.push_str(escaped),
            None => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::DENIED_FORMAT_CHARS;

    fn decode(body: &str) -> String {
        serde_json::from_str::<String>(&format!("\"{body}\"")).expect("valid JSON string")
    }

    fn neighbour(c: char, step: i64) -> Option<char> {
        let n = i64::from(u32::from(c)).checked_add(step)?;
        char::from_u32(u32::try_from(n).ok()?)
    }

    /// Every edge of the hazard set escapes and every non-hazard neighbour stays raw.
    #[test]
    fn hazard_boundaries_escape() {
        let control_ranges = ['\u{0}'..='\u{1f}', '\u{7f}'..='\u{9f}'];
        for range in DENIED_FORMAT_CHARS.iter().chain(control_ranges.iter()) {
            let (lo, hi) = (*range.start(), *range.end());
            for edge in [lo, hi] {
                let body = string_body(&edge.to_string());
                assert!(body.starts_with('\\'), "{edge:?} stayed raw: {body:?}");
                assert!(!body.contains(edge), "{edge:?} survived: {body:?}");
            }
            for outside in [neighbour(lo, -1), neighbour(hi, 1)].into_iter().flatten() {
                if is_hazard(outside) || matches!(outside, '"' | '\\') {
                    continue;
                }
                assert_eq!(
                    string_body(&outside.to_string()),
                    outside.to_string(),
                    "{outside:?} is not a hazard and must stay raw"
                );
            }
        }
        assert_eq!(string_body("\u{9b}"), "\\u009b");
        assert_eq!(string_body("\u{7f}"), "\\u007f");
        assert_eq!(string_body("\u{202e}"), "\\u202e");
        assert_eq!(string_body("\u{e0041}"), "\\udb40\\udc41");
        assert_eq!(string_body("\u{1b}"), "\\u001b");
        assert_eq!(string_body("a\"b\\c\nd\re\tf"), "a\\\"b\\\\c\\nd\\re\\tf");
        assert_eq!(string_body("plain é ✓"), "plain é ✓");
    }

    /// Over every scalar value, the body holds no raw hazard and decodes to its input.
    #[test]
    fn decodes_to_input() {
        for c in (0..=u32::from(char::MAX)).filter_map(char::from_u32) {
            let input = format!("a{c}b");
            let body = string_body(&input);
            if is_hazard(c) {
                assert!(!body.contains(c), "{c:?} survived: {body:?}");
            }
            assert_eq!(decode(&body), input, "{c:?} did not round-trip");
        }
    }

    /// `script_embed` leaves no table character raw and keeps the decoded value.
    #[test]
    fn script_embed_escapes_the_table() {
        let embedded = script_embed("\"</script>\"");
        assert!(!embedded.contains('<'), "{embedded:?}");
        assert_eq!(embedded, "\"\\u003c/script\\u003e\"");
        for (raw, escaped) in SCRIPT_EMBED_ESCAPES {
            let json = format!("[\"a{raw}b\"]");
            let embedded = script_embed(&json);
            assert!(!embedded.contains(raw), "{raw:?} survived: {embedded:?}");
            assert!(embedded.contains(escaped), "{raw:?} not spelled {escaped}");
            let decoded: Vec<String> = serde_json::from_str(&embedded).expect("valid JSON array");
            assert_eq!(decoded, vec![format!("a{raw}b")]);
        }
    }
}

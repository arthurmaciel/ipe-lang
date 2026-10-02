//! Contract of the compiler-side HTML escaper and its agreement with the runtime.
//!
//! `ipe_docs::html::escape` is the one escaper every generated docs page uses.
//! These tests pin that it leaves none of `& < > " '` raw, that its output
//! decodes back to its input, and that the runtime's text and attribute
//! kernels decode to the same value on every vector.

use ipe_docs::html;
use ipe_docs::render::highlight_snippet;
use ipe_runtime_rust::html::{html_escape_attr_, html_escape_text_};

/// The five characters an HTML text or quoted-attribute context reserves.
const SPECIALS: [char; 5] = ['&', '<', '>', '"', '\''];

/// Every entity either side emits, with the character it decodes to.
const ENTITIES: [(&str, char); 6] = [
    ("&amp;", '&'),
    ("&lt;", '<'),
    ("&gt;", '>'),
    ("&quot;", '"'),
    ("&#34;", '"'),
    ("&#39;", '\''),
];

/// The entities the compiler-side owner emits.
const OWNER_ENTITIES: [&str; 5] = ["&amp;", "&lt;", "&gt;", "&quot;", "&#39;"];

/// Decode the entities in [`ENTITIES`]; `None` on any other `&` sequence.
fn decode(escaped: &str) -> Option<String> {
    let mut out = String::with_capacity(escaped.len());
    let mut rest = escaped;
    while let Some(c) = rest.chars().next() {
        if c == '&' {
            let (tail, decoded) = ENTITIES
                .iter()
                .find_map(|(entity, ch)| rest.strip_prefix(entity).map(|t| (t, *ch)))?;
            out.push(decoded);
            rest = tail;
        } else {
            out.push(c);
            let mut chars = rest.chars();
            chars.next();
            rest = chars.as_str();
        }
    }
    Some(out)
}

/// Whether every `&` in `s` opens one of `entities`.
fn every_amp_opens(s: &str, entities: &[&str]) -> bool {
    s.match_indices('&').all(|(i, _)| {
        s.get(i..)
            .is_some_and(|tail| entities.iter().any(|e| tail.starts_with(e)))
    })
}

/// Each special alone, every ordered pair, all five together, and text that
/// already looks like an entity (it must be escaped again, not passed through).
fn vectors() -> Vec<String> {
    let mut out: Vec<String> = SPECIALS.iter().map(char::to_string).collect();
    for a in SPECIALS {
        for b in SPECIALS {
            out.push(format!("{a}{b}"));
        }
    }
    out.push(SPECIALS.iter().collect());
    out.push("x<y z=\"1\" w='2'>&</y>".to_owned());
    out.push("&amp;&#39;&lt;".to_owned());
    out.push("plain text".to_owned());
    out.push(String::new());
    out
}

#[test]
fn owner_leaves_no_special_raw_and_decodes_to_input() {
    for input in vectors() {
        let escaped = html::escape(&input);
        for raw in ['<', '>', '"', '\''] {
            assert!(
                !escaped.contains(raw),
                "escape({input:?}) = {escaped:?} holds a raw {raw:?}"
            );
        }
        assert!(
            every_amp_opens(&escaped, &OWNER_ENTITIES),
            "escape({input:?}) = {escaped:?} holds a raw `&`"
        );
        assert_eq!(
            decode(&escaped).as_deref(),
            Some(input.as_str()),
            "escape({input:?}) = {escaped:?} does not decode back"
        );
    }
}

#[test]
fn escape_into_appends_the_same_bytes_as_escape() {
    for input in vectors() {
        let mut out = String::from("prefix:");
        html::escape_into(&input, &mut out);
        assert_eq!(out, format!("prefix:{}", html::escape(&input)));
    }
}

#[test]
fn runtime_kernels_decode_equal_to_the_owner() {
    for input in vectors() {
        let owner = decode(&html::escape(&input));
        let attr = html_escape_attr_(input.clone());
        let text = html_escape_text_(input.clone());
        for raw in ['<', '>', '"', '\''] {
            assert!(
                !attr.contains(raw),
                "runtime attr({input:?}) = {attr:?} holds a raw {raw:?}"
            );
        }
        for raw in ['<', '>', '\''] {
            assert!(
                !text.contains(raw),
                "runtime text({input:?}) = {text:?} holds a raw {raw:?}"
            );
        }
        let entities: Vec<&str> = ENTITIES.iter().map(|(e, _)| *e).collect();
        assert!(every_amp_opens(&attr, &entities), "attr {attr:?}: raw `&`");
        assert!(every_amp_opens(&text, &entities), "text {text:?}: raw `&`");
        assert_eq!(decode(&attr), owner, "attr form disagrees on {input:?}");
        assert_eq!(decode(&text), owner, "text form disagrees on {input:?}");
    }
}

#[test]
fn highlighted_snippet_escapes_apostrophe() {
    let out = highlight_snippet("'a'");
    assert!(out.contains("&#39;"), "no `&#39;` in {out:?}");
    assert!(!out.contains('\''), "raw `'` in {out:?}");
}

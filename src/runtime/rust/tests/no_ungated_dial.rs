//! Dial-gate scan: every raw network dial in the runtime's dial-site files
//! must route through its typed SSRF gate.
//!
//! The rule is structural, never proximity-based. A sqlx dial (`connect`,
//! `connect_with`, `connect_lazy`, `connect_lazy_with`, in any method or path
//! form) or `*PoolOptions` opener is admitted only as `VettedPool`'s own
//! associated `connect`, and `db.rs` holds exactly one raw dial, inside the
//! body of `VettedPool::connect`, which takes its options from the driver-typed
//! `DB::gated_connect_options`. An SMTP transport is admitted only when its
//! host argument is `<binding>.dial_host(…)`, a method only `VettedDial` has.
//! No comment, string, or nearby token can exempt a dial or vouch for a gate.
//!
//! Every scan runs over [`production_code`]: [`strip_cfg_test_items`] blanks
//! `#[cfg(test)]` items, then [`mask_non_code`] blanks comments and strings,
//! so a test fixture's own dial or guard can never vouch for — or masquerade
//! as — production code. The stripper is a token-level scanner (not a line
//! heuristic): it tracks string/char/comment state across the whole file, so a
//! brace or a `#[cfg(test)]`-looking line hidden inside a string or comment can
//! never desync the boundary it draws. A boundary drawn too wide silently hides
//! a real ungated dial (a vacuous green), so the helpers carry refusal tests
//! pinning them against every fooling shape below, and every scan test asserts
//! a known production anchor survives — an over-strip turns the anchor
//! assertion red before it could ever turn the dial scan vacuously green.

// ---------------------------------------------------------------------------
// Token-level source classification.
//
// `mask_non_code` walks the WHOLE source once, carrying lexer state across
// line boundaries, and replaces every char that is inside a string literal, a
// char/byte-char literal, or a `//`/`/* */` comment with a space — every
// other char (braces, brackets, identifiers, real code) is left untouched, and
// every `\n` is always preserved so the masked text has exactly the same
// lines, in the same positions, as the original. Downstream code (attribute
// detection, brace-depth counting) runs on this masked text instead of the
// raw source, so a brace or an attribute-shaped line hidden inside a string
// or comment can never be mistaken for real code.
// ---------------------------------------------------------------------------

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

// Detects a raw, byte-raw, or C-raw string prefix (`r#*"`, `br#*"`, `cr#*"`)
// starting at `chars[i]`, guarded so it only fires at a token boundary (not
// mid-identifier, e.g. `foo_r"..."`). Returns the prefix length (through the
// opening `"`) and the hash count the closing `"` must match.
fn raw_string_prefix(chars: &[char], i: usize) -> Option<(usize, usize)> {
    if i > 0 && chars.get(i - 1).is_some_and(|c| is_ident_char(*c)) {
        return None;
    }
    let mut j = i;
    if matches!(chars.get(j), Some(&('b' | 'c'))) {
        j += 1;
    }
    if chars.get(j) != Some(&'r') {
        return None;
    }
    j += 1;
    let mut hashes = 0usize;
    while chars.get(j) == Some(&'#') {
        hashes += 1;
        j += 1;
    }
    if chars.get(j) != Some(&'"') {
        return None;
    }
    Some((j + 1 - i, hashes))
}

// Disambiguates a char/byte-char literal (`'{'`, `'\''`, `'\u{7b}'`) from a
// lifetime (`'a`, `'q>`) at `chars[quote_pos]`. A lifetime has no closing `'`
// right after its one (possibly escaped) content char, so this mirrors the
// same lookahead rustc itself uses. Returns the closing quote's index.
fn char_literal_end(chars: &[char], quote_pos: usize) -> Option<usize> {
    let n = chars.len();
    let mut j = quote_pos + 1;
    if j >= n || chars.get(j) == Some(&'\n') {
        return None;
    }
    if chars.get(j) == Some(&'\\') {
        j += 1;
        let escaped = *chars.get(j)?;
        match escaped {
            'u' if chars.get(j + 1) == Some(&'{') => {
                j += 2;
                while chars.get(j).is_some() && chars.get(j) != Some(&'}') {
                    j += 1;
                }
                chars.get(j)?;
                j += 1;
            }
            'x' => {
                j += 1;
                for _ in 0..2 {
                    if chars.get(j).is_some_and(char::is_ascii_hexdigit) {
                        j += 1;
                    }
                }
            }
            _ => j += 1,
        }
    } else {
        j += 1;
    }
    if chars.get(j) == Some(&'\'') {
        Some(j)
    } else {
        None
    }
}

fn mask_non_code(src: &str) -> String {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum St {
        Normal,
        LineComment,
        BlockComment(u32),
        Str,
        RawStr(usize),
    }

    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<char> = chars.clone();
    let n = chars.len();
    let mut i = 0usize;
    let mut state = St::Normal;
    while i < n {
        let c = chars[i];
        match state {
            St::Normal => {
                if c == '/' && chars.get(i + 1) == Some(&'/') {
                    out[i] = ' ';
                    state = St::LineComment;
                    i += 1;
                } else if c == '/' && chars.get(i + 1) == Some(&'*') {
                    out[i] = ' ';
                    if let Some(o) = out.get_mut(i + 1) {
                        *o = ' ';
                    }
                    state = St::BlockComment(1);
                    i += 2;
                } else if c == '"' {
                    out[i] = ' ';
                    state = St::Str;
                    i += 1;
                } else if let Some((prefix_len, hashes)) = raw_string_prefix(&chars, i) {
                    for o in out.iter_mut().skip(i).take(prefix_len) {
                        *o = ' ';
                    }
                    state = St::RawStr(hashes);
                    i += prefix_len;
                } else if c == '\'' {
                    if let Some(end) = char_literal_end(&chars, i) {
                        for o in out.iter_mut().take(end + 1).skip(i) {
                            *o = ' ';
                        }
                        i = end + 1;
                    } else {
                        // A lifetime — ordinary code, no brace can hide in it.
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            St::LineComment => {
                out[i] = if c == '\n' { '\n' } else { ' ' };
                if c == '\n' {
                    state = St::Normal;
                }
                i += 1;
            }
            St::BlockComment(depth) => {
                if c == '/' && chars.get(i + 1) == Some(&'*') {
                    out[i] = ' ';
                    if let Some(o) = out.get_mut(i + 1) {
                        *o = ' ';
                    }
                    state = St::BlockComment(depth + 1);
                    i += 2;
                } else if c == '*' && chars.get(i + 1) == Some(&'/') {
                    out[i] = ' ';
                    if let Some(o) = out.get_mut(i + 1) {
                        *o = ' ';
                    }
                    state = if depth <= 1 {
                        St::Normal
                    } else {
                        St::BlockComment(depth - 1)
                    };
                    i += 2;
                } else {
                    out[i] = if c == '\n' { '\n' } else { ' ' };
                    i += 1;
                }
            }
            St::Str => {
                if c == '\n' {
                    // A normal (non-raw) string literal may legitimately span
                    // source lines; the state carries across the newline.
                    i += 1;
                } else if c == '\\' && i + 1 < n {
                    out[i] = ' ';
                    if let Some(o) = out.get_mut(i + 1)
                        && chars[i + 1] != '\n'
                    {
                        *o = ' ';
                    }
                    i += 2;
                } else {
                    out[i] = ' ';
                    if c == '"' {
                        state = St::Normal;
                    }
                    i += 1;
                }
            }
            St::RawStr(hashes) => {
                if c == '\n' {
                    i += 1;
                } else if c == '"' && (0..hashes).all(|k| chars.get(i + 1 + k) == Some(&'#')) {
                    out[i] = ' ';
                    for o in out.iter_mut().skip(i + 1).take(hashes) {
                        *o = ' ';
                    }
                    i += 1 + hashes;
                    state = St::Normal;
                } else {
                    out[i] = ' ';
                    i += 1;
                }
            }
        }
    }
    out.into_iter().collect()
}

// ---------------------------------------------------------------------------
// `#[cfg(...)]` attribute classification.
// ---------------------------------------------------------------------------

// True when `word` appears in `haystack` bounded by non-identifier chars (or
// the string edge) on both sides — a substring match alone would also fire on
// `testing` or `attestation`.
fn contains_word(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let is_ident_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = 0;
    while let Some(pos) = haystack.get(start..).and_then(|h| h.find(word)) {
        let idx = start + pos;
        let before_ok = idx == 0 || bytes.get(idx - 1).is_some_and(|b| !is_ident_byte(*b));
        let after = idx + word.len();
        let after_ok = bytes.get(after).is_none_or(|b| !is_ident_byte(*b));
        if before_ok && after_ok {
            return true;
        }
        start = idx + 1;
    }
    false
}

// Extracts the predicate inside `#[cfg(<predicate>)]` from a MASKED line
// (string/comment content already blanked), matching parens so a nested
// combinator like `all(test, feature = "x")` closes correctly and any trailing
// content on the same line (a same-line item, e.g. `#[cfg(test)] fn f() {`)
// is ignored rather than rejected.
fn cfg_predicate(masked_line: &str) -> Option<&str> {
    let rest = masked_line.trim_start().strip_prefix("#[cfg(")?;
    let mut depth = 1i32;
    for (idx, ch) in rest.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return rest.get(idx + 1..)?.starts_with(']').then(|| &rest[..idx]);
                }
            }
            _ => {}
        }
    }
    None
}

// True iff `predicate` can ONLY be satisfied when `test` is active — i.e. the
// item it gates never reaches a production (non-test) build. `any(test, ...)`
// can be true without `test` (ambiguous) and `not(test)` is true exactly when
// `test` is ABSENT (production-only code) — both are refused rather than
// guessed at: fail safe toward scanning MORE, never toward stripping more.
fn cfg_test_gated(predicate: &str) -> bool {
    let p = predicate.trim();
    if p == "test" {
        return true;
    }
    if contains_word(p, "any") || contains_word(p, "not") {
        return false;
    }
    p.strip_prefix("all(")
        .and_then(|s| s.strip_suffix(')'))
        .is_some_and(|inner| contains_word(inner, "test"))
}

/// Blanks every line of every `#[cfg(test)]`-gated item in `src`, keeping the
/// line count and every other line's number identical to `src.lines()`.
///
/// `name` is only used to identify the source in a failure message. Detection
/// and brace-depth tracking both run over [`mask_non_code`]'s output, so a
/// `#[cfg(test)]`-shaped line inside a string or comment is never mistaken for
/// a real attribute and a brace inside one is never mistaken for a real scope
/// boundary. An item's extent runs from its (possibly stacked, possibly
/// same-line) attribute to its opening `{` — tracking nested depth back to
/// zero — or, for a brace-free item such as `#[cfg(test)] use path;`, to its
/// terminating `;`. Reaching EOF before that happens is an unbalanced item and
/// fails loudly, wherever the item sits: an unproven boundary never passes.
fn strip_cfg_test_items(name: &str, src: &str) -> Vec<String> {
    let raw: Vec<&str> = src.lines().collect();
    let masked_src = mask_non_code(src);
    let mask_lines: Vec<&str> = masked_src.lines().collect();
    assert_eq!(
        mask_lines.len(),
        raw.len(),
        "{name}: mask_non_code must preserve line count exactly"
    );
    let mut out: Vec<String> = raw.iter().map(|l| (*l).to_string()).collect();
    let mut i = 0;
    while i < raw.len() {
        let gated = mask_lines
            .get(i)
            .copied()
            .and_then(cfg_predicate)
            .is_some_and(cfg_test_gated);
        if !gated {
            i += 1;
            continue;
        }
        let start = i;
        let mut depth: i32 = 0;
        let mut opened = false;
        let mut j = start;
        let end = loop {
            assert!(
                j < raw.len(),
                "{name}: #[cfg(test)] item starting at line {} never closes before EOF \
                 — strip_cfg_test_items cannot safely bound it",
                start + 1
            );
            let ml = mask_lines[j];
            for ch in ml.chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            let has_code = !ml.trim().is_empty();
            let closed_here = if opened {
                depth <= 0
            } else {
                has_code && ml.trim_end().ends_with(';')
            };
            j += 1;
            if closed_here {
                break j;
            }
        };
        for line in out.iter_mut().take(end).skip(start) {
            line.clear();
        }
        i = end;
    }
    out
}

// ---------------------------------------------------------------------------
// Refusal tests for the helper itself.
// ---------------------------------------------------------------------------

#[test]
fn strip_cfg_test_items_keeps_production_code_after_an_early_test_module() {
    let fixture = "#[cfg(test)]\n\
mod t {\n\
    fn helper() {\n\
        let _ = PgPool::connect(\"unguarded-test-only\");\n\
    }\n\
}\n\
\n\
fn production() {\n\
    PgPool::connect(\"unguarded-production\");\n\
}\n";
    let joined = strip_cfg_test_items("fixture.rs", fixture).join("\n");
    assert!(
        joined.contains("PgPool::connect(\"unguarded-production\")"),
        "production code after an early #[cfg(test)] module must survive stripping:\n{joined}"
    );
    assert!(
        !joined.contains("unguarded-test-only"),
        "code inside the #[cfg(test)] module must be stripped:\n{joined}"
    );
}

#[test]
fn strip_cfg_test_items_handles_attribute_and_item_on_the_same_line() {
    let fixture = "fn before() { PgPool::connect(before_url); }\n\
#[cfg(test)] fn only_in_tests() { let s = \"{ not a real brace }\"; }\n\
fn after() { PgPool::connect(after_url); }\n";
    let joined = strip_cfg_test_items("fixture.rs", fixture).join("\n");
    assert!(joined.contains("PgPool::connect(before_url)"), "{joined}");
    assert!(joined.contains("PgPool::connect(after_url)"), "{joined}");
    assert!(!joined.contains("only_in_tests"), "{joined}");
}

#[test]
fn strip_cfg_test_items_ignores_char_literal_braces_and_quotes() {
    let fixture = "#[cfg(test)]\n\
fn only_in_tests() {\n\
    let a = '{';\n\
    let b = '}';\n\
    let c = '\"';\n\
}\n\
\n\
fn production() {\n\
    PgPool::connect(url);\n\
}\n";
    let joined = strip_cfg_test_items("fixture.rs", fixture).join("\n");
    assert!(
        joined.contains("PgPool::connect(url)"),
        "a char literal's brace must never over-extend the gated boundary:\n{joined}"
    );
    assert!(!joined.contains("only_in_tests"), "{joined}");
}

#[test]
fn strip_cfg_test_items_ignores_block_comment_braces() {
    let fixture = "#[cfg(test)]\n\
fn only_in_tests() {\n\
    /* a comment with a brace { inside it */\n\
    let _ = 1;\n\
}\n\
\n\
fn production() {\n\
    PgPool::connect(url);\n\
}\n";
    let joined = strip_cfg_test_items("fixture.rs", fixture).join("\n");
    assert!(
        joined.contains("PgPool::connect(url)"),
        "a brace inside a block comment must never over-extend the gated boundary:\n{joined}"
    );
    assert!(!joined.contains("only_in_tests"), "{joined}");
}

#[test]
fn strip_cfg_test_items_ignores_a_fake_attribute_inside_a_multiline_string() {
    // The most dangerous shape: a line that reads exactly `#[cfg(test)]`, but
    // it lives inside a STRING in production code — it must never be read as
    // a real attribute that then swallows the genuine dial below it.
    let fixture = "fn production_with_string() {\n\
    let s = \"before\n\
#[cfg(test)]\n\
after\";\n\
    PgPool::connect(url);\n\
}\n";
    let joined = strip_cfg_test_items("fixture.rs", fixture).join("\n");
    assert!(
        joined.contains("PgPool::connect(url)"),
        "a #[cfg(test)]-looking line inside a string literal must never gate real code:\n{joined}"
    );
}

#[test]
fn strip_cfg_test_items_strips_a_cfg_all_test_item() {
    let fixture = "#[cfg(all(test, feature = \"x\"))]\n\
fn only_in_tests() {}\n\
\n\
fn production() {\n\
    PgPool::connect(url);\n\
}\n";
    let joined = strip_cfg_test_items("fixture.rs", fixture).join("\n");
    assert!(joined.contains("PgPool::connect(url)"), "{joined}");
    assert!(!joined.contains("only_in_tests"), "{joined}");
}

#[test]
fn strip_cfg_test_items_does_not_strip_a_cfg_any_test_item() {
    let fixture = "#[cfg(any(test, feature = \"x\"))]\n\
fn maybe_in_production() {\n\
    PgPool::connect(ambiguous_url);\n\
}\n";
    let joined = strip_cfg_test_items("fixture.rs", fixture).join("\n");
    assert!(
        joined.contains("PgPool::connect(ambiguous_url)"),
        "any(test, ...) cannot prove the item is test-only; it must stay scanned:\n{joined}"
    );
}

#[test]
fn strip_cfg_test_items_does_not_strip_a_cfg_not_test_item() {
    let fixture = "#[cfg(not(test))]\n\
fn production_only_outside_tests() {\n\
    PgPool::connect(prod_url);\n\
}\n";
    let joined = strip_cfg_test_items("fixture.rs", fixture).join("\n");
    assert!(
        joined.contains("PgPool::connect(prod_url)"),
        "not(test) is PRODUCTION-only code (true exactly when test is ABSENT) \
         — it must never be stripped:\n{joined}"
    );
}

#[test]
fn strip_cfg_test_items_does_not_confuse_a_feature_named_test_with_cfg_test() {
    let fixture = "#[cfg(feature = \"test\")]\n\
fn production_feature_test() {\n\
    PgPool::connect(feature_url);\n\
}\n";
    let joined = strip_cfg_test_items("fixture.rs", fixture).join("\n");
    assert!(
        joined.contains("PgPool::connect(feature_url)"),
        "a feature literally named \"test\" is not the cfg(test) predicate:\n{joined}"
    );
}

#[test]
#[should_panic(expected = "never closes before EOF")]
fn strip_cfg_test_items_refuses_an_unbalanced_gated_item() {
    let fixture = "#[cfg(test)]\nmod t {\n    fn helper() {\n";
    let _ = strip_cfg_test_items("fixture.rs", fixture);
}

/// A file whose last line is `}` still refuses a gated item that never closes.
#[test]
#[should_panic(expected = "never closes before EOF")]
fn strip_cfg_test_items_refuses_an_unbalanced_last_item_ending_in_a_brace() {
    let fixture = "fn production() {}\n#[cfg(test)]\nmod t {\n    fn helper() {\n    }\n";
    let _ = strip_cfg_test_items("fixture.rs", fixture);
}

#[test]
fn mask_non_code_treats_nested_block_comments_as_non_code() {
    let src = "fn f() { /* outer /* inner */ leaked-brace-here-{ */ }";
    let masked = mask_non_code(src);
    assert_eq!(
        masked.matches('{').count(),
        1,
        "a brace inside a NESTED block comment must stay masked:\n{masked}"
    );
    assert_eq!(
        masked.matches('}').count(),
        1,
        "only the function's own closing brace should survive masking:\n{masked}"
    );
}

#[test]
fn mask_non_code_handles_raw_strings_with_embedded_quotes_and_trailing_backslash() {
    let src = r###"fn f() { let a = r#"a"{"#; let b = r"C:\"; }"###;
    let masked = mask_non_code(src);
    assert_eq!(
        masked.matches('{').count(),
        1,
        "a brace inside a raw string must stay masked:\n{masked}"
    );
    assert_eq!(
        masked.matches('}').count(),
        1,
        "a raw string ending in a backslash must still close at its own quote:\n{masked}"
    );
}

#[test]
fn mask_non_code_handles_c_raw_strings() {
    let src = r###"fn f() { let a = cr#"a"{"#; let b = cr"C:\"; }"###;
    let masked = mask_non_code(src);
    assert_eq!(
        masked.matches('{').count(),
        1,
        "a brace inside a C raw string must stay masked:\n{masked}"
    );
    assert_eq!(
        masked.matches('}').count(),
        1,
        "a C raw string ending in a backslash must still close at its own quote:\n{masked}"
    );
}

#[test]
fn mask_non_code_treats_char_literals_as_non_code_not_lifetimes() {
    let src = "fn f<'q>() { let a = '{'; let b = '}'; let c = '\"'; }";
    let masked = mask_non_code(src);
    assert_eq!(masked.matches('{').count(), 1, "{masked}");
    assert_eq!(masked.matches('}').count(), 1, "{masked}");
    assert!(
        masked.contains("fn f<'q>()"),
        "a lifetime must remain ordinary code, not be read as a char literal:\n{masked}"
    );
}

#[test]
fn mask_non_code_carries_string_state_across_lines() {
    let src = "fn f() {\n    let s = \"line one\nline two { still string\";\n    let x = 1;\n}\n";
    let masked = mask_non_code(src);
    assert_eq!(
        masked.matches('{').count(),
        1,
        "a brace inside a string spanning multiple source lines must stay masked:\n{masked}"
    );
}

// ---------------------------------------------------------------------------
// Token-level dial classification over masked production code.
// ---------------------------------------------------------------------------

/// The production code of `src`, with comments and strings masked.
///
/// `#[cfg(test)]` items are blanked first; every line keeps its number, so a
/// byte offset in the result maps to the same 1-based line of `src`.
fn production_code(name: &str, src: &str) -> String {
    mask_non_code(&strip_cfg_test_items(name, src).join("\n"))
}

/// One identifier token in masked code.
struct Ident<'a> {
    /// Byte offset of its first char.
    at: usize,
    text: &'a str,
}

/// Every identifier token in `code`, in order.
fn idents(code: &str) -> Vec<Ident<'_>> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (at, ch) in code.char_indices() {
        match (is_ident_char(ch), start) {
            (true, None) => start = Some(at),
            (false, Some(s)) => {
                if let Some(text) = code.get(s..at) {
                    out.push(Ident { at: s, text });
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(text) = start.and_then(|s| code.get(s..)) {
        out.push(Ident {
            at: code.len() - text.len(),
            text,
        });
    }
    out
}

/// The 1-based line holding byte offset `at` of `code`.
fn line_of(code: &str, at: usize) -> usize {
    code.get(..at)
        .map_or(0, |before| before.matches('\n').count())
        + 1
}

/// Whether the token before `at` is the `fn` keyword, making the identifier at
/// `at` a definition rather than a use.
fn is_fn_definition(code: &str, at: usize) -> bool {
    code.get(..at)
        .map(str::trim_end)
        .and_then(|before| before.strip_suffix("fn"))
        .is_some_and(|rest| !rest.ends_with(is_ident_char))
}

/// `s` with one trailing balanced `<…>` removed; `s` itself when it does not
/// end in `>`, and `None` when the brackets do not balance.
fn strip_trailing_generics(s: &str) -> Option<&str> {
    if !s.ends_with('>') {
        return Some(s);
    }
    let mut depth = 0i32;
    for (i, ch) in s.char_indices().rev() {
        match ch {
            '>' => depth += 1,
            '<' => {
                depth -= 1;
                if depth == 0 {
                    return s.get(..i).map(str::trim_end);
                }
            }
            _ => {}
        }
    }
    None
}

/// The type segment owning the associated function named at `at`.
///
/// Generics are skipped, so `crate::db::VettedPool::<sqlx::Sqlite>::connect`
/// yields `VettedPool`. A method call (`x.connect`), a bare call, or a
/// qualified `<T as Trait>::` path yields `None`.
fn path_owner(code: &str, at: usize) -> Option<&str> {
    let before = code.get(..at)?.trim_end().strip_suffix("::")?.trim_end();
    let before = strip_trailing_generics(before)?;
    let before = before.strip_suffix("::").unwrap_or(before).trim_end();
    let seg_start = before
        .char_indices()
        .rev()
        .find(|(_, c)| !is_ident_char(*c))
        .map_or(0, |(i, c)| i + c.len_utf8());
    before.get(seg_start..).filter(|seg| !seg.is_empty())
}

/// sqlx functions that open a connection or a pool.
const DIAL_FNS: &[&str] = &[
    "connect",
    "connect_with",
    "connect_lazy",
    "connect_lazy_with",
];

/// The one type whose associated `connect` routes every dial through the gate.
const GATED_OWNER: &str = "VettedPool";

/// What a [`RawDial`] opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RawKind {
    /// A call to one of [`DIAL_FNS`].
    Dial,
    /// A sqlx `*PoolOptions` builder, which opens a pool.
    PoolOptions,
}

/// A raw sqlx dial or pool opener in masked production code.
#[derive(Debug)]
struct RawDial {
    /// Its 1-based line.
    line: usize,
    ident: String,
    kind: RawKind,
}

/// Every raw sqlx dial or pool opener in masked `code`.
///
/// Each mention of a [`DIAL_FNS`] identifier is a dial, in every syntactic
/// form (method call, path call, function reference), unless it is a `fn`
/// definition or an associated function of [`GATED_OWNER`]. Only the path's
/// owning type admits a call; no text near it does.
fn raw_dials(code: &str) -> Vec<RawDial> {
    idents(code)
        .into_iter()
        .filter_map(|id| {
            let kind = if DIAL_FNS.contains(&id.text) {
                RawKind::Dial
            } else if id.text.ends_with("PoolOptions") {
                RawKind::PoolOptions
            } else {
                return None;
            };
            let gated = kind == RawKind::Dial && path_owner(code, id.at) == Some(GATED_OWNER);
            (!gated && !is_fn_definition(code, id.at)).then(|| RawDial {
                line: line_of(code, id.at),
                ident: id.text.to_owned(),
                kind,
            })
        })
        .collect()
}

/// Lines of every raw sqlx dial or pool opener in `src`'s production code.
fn caller_url_raw_dials(name: &str, src: &str) -> Vec<usize> {
    raw_dials(&production_code(name, src))
        .iter()
        .map(|d| d.line)
        .collect()
}

/// The 0-based line range of the body of `VettedPool`'s `connect` in masked
/// `lines`, the one place a raw dial may appear.
fn gate_body(lines: &[&str]) -> Option<(usize, usize)> {
    let impl_at = lines
        .iter()
        .position(|l| l.contains("impl<DB> VettedPool<DB>"))?;
    let fn_start = lines
        .iter()
        .skip(impl_at)
        .position(|l| l.contains("async fn connect("))
        .map(|offset| impl_at + offset)?;
    let mut depth = 0i32;
    let mut opened = false;
    for (i, line) in lines.iter().enumerate().skip(fn_start) {
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if opened && depth <= 0 {
            return Some((fn_start, i));
        }
    }
    None
}

/// Every raw dial in `src`'s production code that bypasses the typed gate.
///
/// Exactly one raw dial is admitted, inside `VettedPool::connect`, and only
/// when that body takes its options from `DB::gated_connect_options`: the
/// driver type, not any text near the call, decides which gate runs. Each
/// violation names its 1-based line.
fn db_dials_bypassing_the_gate(name: &str, src: &str) -> Vec<String> {
    let code = production_code(name, src);
    let lines: Vec<&str> = code.lines().collect();
    let body = gate_body(&lines).map(|(lo, hi)| (lo + 1)..=(hi + 1));
    let dials = raw_dials(&code);
    let mut violations = Vec::new();
    match &body {
        Some(body) => {
            let gated = idents(&code).iter().any(|id| {
                id.text == "gated_connect_options"
                    && body.contains(&line_of(&code, id.at))
                    && path_owner(&code, id.at) == Some("DB")
            });
            if !gated {
                violations.push(format!(
                    "{name}:{}: VettedPool::connect does not take its options from \
                     DB::gated_connect_options",
                    body.start()
                ));
            }
            let admitted = dials
                .iter()
                .filter(|d| d.kind == RawKind::Dial && body.contains(&d.line))
                .count();
            if admitted != 1 {
                violations.push(format!(
                    "{name}:{}: VettedPool::connect holds {admitted} raw dials; exactly one \
                     is admitted",
                    body.start()
                ));
            }
        }
        None => violations.push(format!("{name}: no VettedPool::connect body found")),
    }
    for dial in &dials {
        if !body.as_ref().is_some_and(|b| b.contains(&dial.line)) {
            violations.push(format!(
                "{name}:{}: raw dial `{}` outside VettedPool::connect",
                dial.line, dial.ident
            ));
        }
    }
    violations
}

/// lettre functions that build an SMTP transport dialing their host argument.
const SMTP_TRANSPORT_FNS: &[&str] = &["builder_dangerous", "relay", "starttls_relay", "from_url"];

/// Whether the call whose name ends at byte `end` of `code` passes
/// `<binding>.dial_host(…)` as its first argument.
fn takes_vetted_host(code: &str, end: usize) -> bool {
    let Some(args) = code
        .get(end..)
        .and_then(|rest| rest.trim_start().strip_prefix('('))
        .map(str::trim_start)
    else {
        return false;
    };
    let binding_len = args.find(|c: char| !is_ident_char(c)).unwrap_or(args.len());
    binding_len > 0
        && args
            .get(binding_len..)
            .map(str::trim_start)
            .and_then(|r| r.strip_prefix('.'))
            .map(str::trim_start)
            .and_then(|r| r.strip_prefix("dial_host"))
            .map(str::trim_start)
            .is_some_and(|r| r.starts_with('('))
}

/// Lines of every SMTP transport built from a host that did not pass the gate.
///
/// A transport is gated only when its first argument is `<binding>.dial_host(…)`:
/// `dial_host` exists only on `VettedDial`, whose sole constructor runs the SSRF
/// gate, so the argument's type proves the host was vetted. A guard named only
/// in a comment or string is masked away and proves nothing.
fn ungated_smtp_transports(name: &str, src: &str) -> Vec<usize> {
    let code = production_code(name, src);
    idents(&code)
        .iter()
        .filter(|id| {
            SMTP_TRANSPORT_FNS.contains(&id.text)
                && !is_fn_definition(&code, id.at)
                && !takes_vetted_host(&code, id.at + id.text.len())
        })
        .map(|id| line_of(&code, id.at))
        .collect()
}

// ---------------------------------------------------------------------------
// Refusal tests for the dial scans.
// ---------------------------------------------------------------------------

#[test]
fn raw_dial_scan_admits_only_vetted_pool_calls_and_definitions() {
    let fixture = "async fn open(url: &str) {
    let a = crate::db::VettedPool::<sqlx::Postgres>::connect(url, 4).await;
    let b = VettedPool::<sqlx::Sqlite>::
        connect(url, 4).await;
}
pub async fn connect(url: &str) {}
";
    assert_eq!(
        caller_url_raw_dials("fixture.rs", fixture),
        Vec::<usize>::new()
    );
}

/// A comment or nearby token naming sqlite or a file never exempts a raw dial.
#[test]
fn raw_dial_scan_refuses_a_method_dial_a_comment_calls_local() {
    let fixture = "async fn open(url: &str) {
    // Local sqlite file at :memory:, no host to gate (SqlitePool).
    let _ = options.connect(url).await;
}
";
    assert_eq!(caller_url_raw_dials("fixture.rs", fixture), vec![3]);
}

#[test]
fn raw_dial_scan_refuses_every_path_form() {
    let fixture = "async fn open(url: &str) {
    let _ = PgPool::connect(url).await;
    let _ = sqlx::postgres::PgPool::connect_lazy(url);
    let _ = sqlx::Pool::<Postgres>::connect_with(opts).await;
    let _ = ConnectOptions::connect(&opts).await;
    let _ = <PgConnection as Connection>::connect(url).await;
    let f = SqlitePool::connect;
    let _ = PgPoolOptions::new();
}
";
    assert_eq!(
        caller_url_raw_dials("fixture.rs", fixture),
        vec![2, 3, 4, 5, 6, 7, 8]
    );
}

/// `VettedPool` spelled in a comment or a string does not own the call after it.
#[test]
fn raw_dial_scan_ignores_a_gate_named_in_a_comment_or_string() {
    let fixture = "async fn open(url: &str) {
    let _ = /* VettedPool:: */ PgPool::connect(url).await;
    let s = \"VettedPool::\"; connect(url);
}
";
    assert_eq!(caller_url_raw_dials("fixture.rs", fixture), vec![2, 3]);
}

/// The gate a fixture's `VettedPool::connect` must route through.
const GATED_POOL_FIXTURE: &str = r"
impl<DB> VettedPool<DB> {
    pub async fn connect(url: &str) -> Result<Self, E> {
        let options = DB::gated_connect_options(url).await?;
        let pool = PoolOptions::<DB>::new().connect_with(options).await?;
        Ok(Self(pool))
    }
}
";

#[test]
fn db_gate_scan_admits_the_gated_pool_fixture() {
    assert_eq!(
        db_dials_bypassing_the_gate("fixture.rs", GATED_POOL_FIXTURE),
        Vec::<String>::new()
    );
}

/// A comment naming sqlite or a file above a raw dial does not exempt it:
/// comments are masked, and only the typed gate's body may dial.
#[test]
fn db_gate_scan_refuses_a_dial_a_doc_comment_calls_local() {
    let fixture = format!(
        "{GATED_POOL_FIXTURE}
/// Opens the local sqlite file at :memory: (no host to gate).
async fn sneaky(url: &str) {{
    let _ = options.connect(url).await;
}}
"
    );
    let violations = db_dials_bypassing_the_gate("fixture.rs", &fixture);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations
            .iter()
            .all(|v| v.contains("raw dial `connect` outside")),
        "{violations:?}"
    );
}

#[test]
fn db_gate_scan_refuses_a_path_form_dial_outside_the_gate() {
    let fixture = format!(
        "{GATED_POOL_FIXTURE}
async fn sneaky(url: &str) {{
    let _ = PgPool::connect(url).await;
    let _ = sqlx::Pool::<Sqlite>::connect_lazy(url);
}}
"
    );
    let violations = db_dials_bypassing_the_gate("fixture.rs", &fixture);
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(
        violations
            .iter()
            .all(|v| v.contains("outside VettedPool::connect")),
        "{violations:?}"
    );
}

/// A string literal that spells a dial is not a dial, and a comment inside the
/// gate body that spells the gate call does not stand in for it.
#[test]
fn db_gate_scan_reads_code_not_comments_or_strings() {
    let fixture = r#"
impl<DB> VettedPool<DB> {
    pub async fn connect(url: &str) -> Result<Self, E> {
        // let options = DB::gated_connect_options(url).await?;
        let s = "DB::gated_connect_options(url)";
        let pool = PoolOptions::<DB>::new().connect(url).await?;
        Ok(Self(pool))
    }
}

fn label() -> &'static str {
    "PoolOptions::new().connect(url)"
}
"#;
    let violations = db_dials_bypassing_the_gate("fixture.rs", fixture);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations
            .iter()
            .all(|v| v.contains("does not take its options")),
        "{violations:?}"
    );
}

#[test]
fn db_gate_scan_refuses_a_second_dial_inside_the_gate() {
    let fixture = r"
impl<DB> VettedPool<DB> {
    pub async fn connect(url: &str) -> Result<Self, E> {
        let options = DB::gated_connect_options(url).await?;
        let pool = PoolOptions::<DB>::new().connect_with(options).await?;
        let raw = PgPool::connect(url).await?;
        Ok(Self(pool))
    }
}
";
    let violations = db_dials_bypassing_the_gate("fixture.rs", fixture);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations.iter().all(|v| v.contains("holds 2 raw dials")),
        "{violations:?}"
    );
}

#[test]
fn db_gate_scan_refuses_a_missing_gate() {
    let fixture = "async fn open(url: &str) {\n    let _ = SqlitePool::connect(url).await;\n}\n";
    let violations = db_dials_bypassing_the_gate("fixture.rs", fixture);
    assert_eq!(violations.len(), 2, "{violations:?}");
}

#[test]
fn smtp_scan_admits_a_vetted_host() {
    let fixture = "async fn send() {
    let vetted = VettedDial::for_host(&cfg.host, port).await?;
    let tb = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
        vetted.dial_host(&cfg.host),
    );
}
";
    assert_eq!(
        ungated_smtp_transports("fixture.rs", fixture),
        Vec::<usize>::new()
    );
}

/// A guard named only in a comment or a string never vouches for a transport.
#[test]
fn smtp_scan_refuses_a_guard_in_a_comment_or_string() {
    let fixture = "async fn send() {
    // let vetted = VettedDial::for_host(&cfg.host, port); vetted.dial_host(h)
    let a = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.host);
    let b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(\"vetted.dial_host(h)\");
    let c = AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host);
}
";
    assert_eq!(
        ungated_smtp_transports("fixture.rs", fixture),
        vec![3, 4, 5]
    );
}

// ---------------------------------------------------------------------------
// The dial scans over the runtime's dial sites.
// ---------------------------------------------------------------------------

/// The files that open a pool from a caller-supplied connection URL must do so
/// only through `VettedPool::connect`, never a raw sqlx opener, whose error may
/// echo the URL's credentials and which skips the SSRF gate.
#[test]
fn caller_url_pools_open_only_through_vetted_pool() {
    let sources = [
        (
            "web/store.rs",
            include_str!("../src/web/store.rs"),
            &[
                "pub struct SqliteStore<",
                "VettedPool::<sqlx::Sqlite>::connect(",
            ][..],
        ),
        (
            "external_conn.rs",
            include_str!("../src/external_conn.rs"),
            &[
                "async fn open_external",
                "VettedPool::<sqlx::Postgres>::connect(",
                "VettedPool::<sqlx::Sqlite>::connect(",
            ][..],
        ),
    ];
    for (name, src, anchors) in sources {
        let code = production_code(name, src);
        for anchor in anchors {
            assert!(
                code.contains(anchor),
                "over-strip hid production anchor `{anchor}` in {name}"
            );
        }
        assert_eq!(
            caller_url_raw_dials(name, src),
            Vec::<usize>::new(),
            "raw sqlx dial in {name} — open through VettedPool::connect"
        );
    }
}

#[test]
fn db_pool_connect_is_guarded() {
    let src = include_str!("../src/db.rs");
    let code = production_code("db.rs", src);
    for anchor in [
        "async fn build_pool",
        "VettedPool::<DbDatabase>::connect(",
        "async fn vet_dial_target<",
        "impl GatedDial for sqlx::Postgres",
        "impl GatedDial for sqlx::Sqlite",
    ] {
        assert!(
            code.contains(anchor),
            "over-strip hid the production anchor `{anchor}`"
        );
    }
    assert_eq!(
        db_dials_bypassing_the_gate("db.rs", src),
        Vec::<String>::new()
    );
}

#[test]
fn email_smtp_builder_dangerous_is_guarded() {
    let src = include_str!("../src/email.rs");
    let code = production_code("email.rs", src);
    for anchor in ["async fn send_smtp", "builder_dangerous("] {
        assert!(
            code.contains(anchor),
            "over-strip hid the production anchor `{anchor}` in email.rs"
        );
    }
    assert_eq!(
        ungated_smtp_transports("email.rs", src),
        Vec::<usize>::new(),
        "SMTP transport in email.rs built from an unvetted host — pass VettedDial::dial_host"
    );
}

//! Grep-gate: every raw network-dial call site in the runtime must be
//! accompanied by an SSRF guard (`VettedDial` or `ssrf_apply`).
//!
//! This test scans the four files that are the closed set of network-dial call
//! sites and asserts each `.connect(` / `builder_dangerous(` on a network path
//! has a guard adjacent in the same function.  A newly-added ungated dial fails
//! this test, keeping the egress class closed.
//!
//! Every scan runs over [`strip_cfg_test_items`]'s output, never raw
//! `src.lines()`, so a test fixture's own guard token or dial call can never
//! vouch for — or masquerade as — production code. That helper is a small
//! token-level scanner (not a line heuristic): it tracks string/char/comment
//! state across the whole file, so a brace or a `#[cfg(test)]`-looking line
//! hidden inside a string or comment can never desync the boundary it draws.
//! Getting that boundary too wide silently hides a real ungated dial (a
//! vacuous green), so the helper itself carries refusal tests pinning it
//! against every fooling shape below, and every scan test asserts a known
//! production anchor survives — an over-strip turns the anchor assertion red
//! before it could ever turn the dial-scan itself vacuously green.

const GUARDS: &[&str] = &["VettedDial", "ssrf_apply"];

/// True when any guard marker appears within `window_lines` lines of `target_line`
/// in the line-oriented source view.  Using lines (not bytes) avoids slicing into
/// multi-byte UTF-8 chars in adjacent comments.
fn guarded_near_line(lines: &[String], target_line: usize, window_lines: usize) -> bool {
    let lo = target_line.saturating_sub(window_lines);
    let hi = (target_line + window_lines).min(lines.len());
    lines[lo..hi]
        .iter()
        .any(|l| GUARDS.iter().any(|g| l.contains(g)))
}

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

// Detects a raw or byte-raw string prefix (`r#*"`, `br#*"`) starting at
// `chars[i]`, guarded so it only fires at a token boundary (not mid-identifier,
// e.g. `foo_r"..."`). Returns the prefix length (through the opening `"`) and
// the hash count the closing `"` must match.
fn raw_string_prefix(chars: &[char], i: usize) -> Option<(usize, usize)> {
    if i > 0 && chars.get(i - 1).is_some_and(|c| is_ident_char(*c)) {
        return None;
    }
    let mut j = i;
    if chars.get(j) == Some(&'b') {
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
                    if let Some(o) = out.get_mut(i + 1) {
                        if chars[i + 1] != '\n' {
                            *o = ' ';
                        }
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
/// fails loudly (an unproven boundary must never silently pass), UNLESS it is
/// the file's last item and the file's last non-blank line is `}` — there is
/// no production code left afterward for a wrong boundary to hide.
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
            if j >= raw.len() {
                let last_non_blank_is_close = raw
                    .iter()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .is_some_and(|l| l.trim() == "}");
                assert!(
                    last_non_blank_is_close,
                    "{name}: #[cfg(test)] item starting at line {} never closes before EOF \
                     — strip_cfg_test_items cannot safely bound it",
                    start + 1
                );
                break j;
            }
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
// The dial scans.
// ---------------------------------------------------------------------------

#[test]
fn external_conn_postgres_dial_is_guarded() {
    let src = include_str!("../src/external_conn.rs");
    let lines = strip_cfg_test_items("external_conn.rs", src);
    let joined = lines.join("\n");
    assert!(
        joined.contains("async fn open_external"),
        "over-strip hid open_external"
    );
    assert!(
        joined.contains("VettedPool::<sqlx::Postgres>::connect("),
        "over-strip hid the guarded VettedPool dial"
    );
    for (i, line) in lines.iter().enumerate() {
        if !line.contains(".connect(") {
            continue;
        }
        // SQLite path has no host to gate — skip it.
        let ctx_start = i.saturating_sub(10);
        let ctx: String = lines[ctx_start..=i].join("\n");
        if ctx.contains("Sqlite") || ctx.contains("SqlitePool") {
            continue;
        }
        assert!(
            guarded_near_line(&lines, i, 60),
            "unguarded .connect( at line {} in external_conn.rs — add a VettedDial guard\n{}",
            i + 1,
            line
        );
    }
}

#[test]
fn db_pool_connect_is_guarded() {
    let src = include_str!("../src/db.rs");
    let lines = strip_cfg_test_items("db.rs", src);
    let joined = lines.join("\n");
    assert!(
        joined.contains("async fn build_pool"),
        "over-strip hid build_pool"
    );
    assert!(
        joined.contains("VettedPool::<DbDatabase>::connect("),
        "over-strip hid the guarded VettedPool dial"
    );
    assert!(
        joined.contains("fn vet_dial_target("),
        "over-strip hid the DialTarget SSRF gate"
    );
    for (i, line) in lines.iter().enumerate() {
        if !line.contains(".connect(") {
            continue;
        }
        // SQLite / file / in-memory dials carry no host — exempt.
        let ctx_start = i.saturating_sub(30);
        let ctx: String = lines[ctx_start..=i].join("\n");
        if ctx.contains("sqlite") || ctx.contains("file") || ctx.contains(":memory:") {
            continue;
        }
        assert!(
            guarded_near_line(&lines, i, 60),
            "unguarded .connect( at line {} in db.rs — add a VettedDial guard\n{}",
            i + 1,
            line
        );
    }
}

#[test]
fn email_smtp_builder_dangerous_is_guarded() {
    let src = include_str!("../src/email.rs");
    let lines = strip_cfg_test_items("email.rs", src);
    let joined = lines.join("\n");
    assert!(
        joined.contains("async fn send_smtp"),
        "over-strip hid send_smtp"
    );
    assert!(
        joined.contains("builder_dangerous("),
        "over-strip hid the guarded builder_dangerous( site"
    );
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("builder_dangerous(") {
            continue;
        }
        assert!(
            guarded_near_line(&lines, i, 60),
            "unguarded builder_dangerous( at line {} in email.rs — add a VettedDial guard\n{}",
            i + 1,
            line
        );
    }
}

/// Raw sqlx pool openers: each bypasses `VettedPool::connect` (SSRF gate,
/// connection cap, version floor, credential-free errors).
const RAW_POOL_OPENERS: &[&str] = &[
    "Pool::connect",
    "PoolOptions",
    "connect_with(",
    "connect_lazy",
];

/// The files that open a pool from a caller-supplied connection URL must do so
/// only through `VettedPool::connect`, never a raw sqlx opener whose error may
/// echo the URL's credentials.
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
            ][..],
        ),
    ];
    for (name, src, anchors) in sources {
        let lines = strip_cfg_test_items(name, src);
        let joined = lines.join("\n");
        for anchor in anchors {
            assert!(
                joined.contains(anchor),
                "over-strip hid production anchor `{anchor}` in {name}"
            );
        }
        for (i, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let code = line.replace("VettedPool", "");
            for opener in RAW_POOL_OPENERS {
                assert!(
                    !code.contains(opener),
                    "raw pool opener `{opener}` at {name}:{} — open through VettedPool::connect\n{line}",
                    i + 1
                );
            }
        }
    }
}

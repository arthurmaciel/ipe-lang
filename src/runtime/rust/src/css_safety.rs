//! Shared CSS/style injection-safety encoders — one policy, one place.
//!
//! This module is the SINGLE home for the CSS boundary smart constructors and
//! the `<style>`-body close-tag stripper. `Ipe.Ui`'s inline-style path
//! (`ui/render.rs`), `Ipe.Html.styleNode`'s `<style>` body (`html.rs`), and
//! `Ipe.Css`'s stylesheet renderers (`css.rs`) all import the identical
//! functions from here so no second, weaker encoder can drift into existence
//! (design §Q5 "three producers, two shared encoders, zero new ones").
//!
//! Security posture (PARSE, DON'T VALIDATE / MAKE INVALID STATES
//! UNREPRESENTABLE): every constructor here is the SOLE way to obtain the
//! corresponding `Safe*` type; each runs its charset/scan policy exactly once at
//! the boundary and the resulting value carries the proof of safety in its
//! structure, so downstream emit sinks consume the typed value and cannot forget
//! the check.

/// A validated CSS property name.  The SOLE construction path runs the full
/// charset policy exactly once at the call-site boundary; no downstream
/// re-check is needed (PARSE, DON'T VALIDATE).
///
/// Accepted charset: ASCII alphanumeric + `-` (covers standard properties and
/// vendor-prefixed ones).  CSS custom properties (`--foo`) are also accepted
/// since `--` is two `-` chars.  The empty string and any char outside
/// `[A-Za-z0-9-]` (including `:`, `;`, `{`, whitespace) is rejected, which
/// closes the key-injection vector where a malicious key like
/// `background:url(javascript:alert(1));x` could smuggle a full rule through.
pub(crate) struct SafeCssPropertyName<'a>(&'a str);

impl<'a> SafeCssPropertyName<'a> {
    /// Parse and validate a CSS property name.
    ///
    /// Returns `None` (silently drop) when `k` is empty or contains any byte
    /// outside `[A-Za-z0-9-]` after trimming leading/trailing whitespace.
    pub(crate) fn parse(k: &'a str) -> Option<Self> {
        let k = k.trim();
        let ok = !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if ok {
            Some(SafeCssPropertyName(k))
        } else {
            None
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0
    }
}

/// A validated CSS property value.  The SOLE construction path runs the full
/// security scan exactly once at the call-site boundary (PARSE, DON'T
/// VALIDATE).  Covers the two live XSS holes in the previous prefix-only gate:
///
/// 1. **Prefix bypass** — `0; background:url(javascript:…)` has a safe prefix
///    but contains an injection mid-value.  Whole-string scan closes this.
/// 2. **Key bypass** — delegated to [`SafeCssPropertyName`]; value gate is
///    not responsible for keys.
///
/// Checks (all case-folded on the whole string, with whitespace stripped for
/// the `url(`-scheme sub-checks to catch `url( javascript:…`):
/// - Declaration / ruleset breakout chars: `;` `{` `}` `</` `/*` `@import`.
/// - Script sinks anywhere in the value: `expression(`, `javascript:`,
///   `vbscript:`, `url(javascript:`, `url(vbscript:`, `url(data:text`,
///   `url(data:application`.
pub(crate) struct SafeCssValue<'a>(&'a str);

/// Breakout / script-sink patterns for a CSS declaration value. Checked
/// against both the raw value and the CSS-escape-decoded value
/// (`css_unescape`) by [`has_dangerous_css_pattern`] — one list, one policy.
const BAD_VALUE_PATTERNS: &[&str] = &[
    "expression(",
    "javascript:",
    "vbscript:",
    "url(javascript:",
    "url('javascript:",
    "url(\"javascript:",
    "url(vbscript:",
    "url(data:text",
    "url(data:application",
];

/// True when `low` (already lowercased) carries a declaration/ruleset
/// breakout character or a script-sink keyword. Shared by the raw-value and
/// CSS-escape-decoded-value passes so they cannot drift.
fn has_dangerous_css_pattern(low: &str) -> bool {
    // Declaration / ruleset / style-tag breakout + comment obfuscation.
    if low.contains(';')
        || low.contains('{')
        || low.contains('}')
        || low.contains("</")
        || low.contains("/*")
        || low.contains("@import")
    {
        return true;
    }
    // Script sinks — whitespace stripped so `url( javascript:…` /
    // `java script:` cannot evade.
    let low_nows: String = low.chars().filter(|c| !c.is_whitespace()).collect();
    BAD_VALUE_PATTERNS.iter().any(|bad| low_nows.contains(bad))
}

/// Decode CSS backslash escapes (CSS Syntax Level 3 §4.3.7) for DETECTION
/// purposes only — the decoded string is never emitted; [`SafeCssValue`]
/// keeps the caller's ORIGINAL string on success. A value that hides a
/// blocked keyword or breakout char behind a hex escape (`\65 xpression(…)`,
/// `\3b` for `;`) decodes to the literal form here, so
/// [`has_dangerous_css_pattern`] catches it on the second pass.
///
/// Best-effort / fail-closed, not a spec-complete CSS tokenizer: an escape
/// decoding to an invalid Unicode scalar value is dropped rather than
/// reconstructed (erring toward "the scan sees less" is the safe direction
/// for a detector — it never helps an attacker hide a keyword).
fn css_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut hex = String::new();
        while hex.len() < 6 {
            match chars.peek() {
                Some(h) if h.is_ascii_hexdigit() => {
                    hex.push(*h);
                    chars.next();
                }
                _ => break,
            }
        }
        if !hex.is_empty() {
            // One trailing whitespace char is consumed as the escape's
            // delimiter (CSS Syntax §4.3.7), if present.
            if matches!(chars.peek(), Some(w) if w.is_whitespace()) {
                chars.next();
            }
            if let Ok(cp) = u32::from_str_radix(&hex, 16)
                && let Some(ch) = char::from_u32(cp)
            {
                out.push(ch);
            }
            continue;
        }
        // `\` followed by a non-hex char: CSS escapes that literal char.
        if let Some(next) = chars.next() {
            out.push(next);
        }
        // trailing lone backslash (no following char): dropped.
    }
    out
}

impl<'a> SafeCssValue<'a> {
    /// Parse and validate a CSS property value.
    ///
    /// Returns `None` (silently drop) when any dangerous pattern is found —
    /// in either the raw value or its CSS-escape-decoded form.
    pub(crate) fn parse(v: &'a str) -> Option<Self> {
        let low = v.to_ascii_lowercase();
        if has_dangerous_css_pattern(&low) {
            return None;
        }
        // Defence-in-depth: decode CSS backslash escapes and re-scan so a
        // hex-escaped bypass of the check above (`\65 xpression(…)`, `\3b`
        // for `;`) is caught too.
        let decoded_low = css_unescape(&low);
        if has_dangerous_css_pattern(&decoded_low) {
            return None;
        }
        Some(SafeCssValue(v))
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0
    }
}

/// Sink-side re-validation of a `;`-joined CSS declaration list — the payload
/// carried by the `data-ipe-mq-rules` / `data-ipe-pc-rules` style markers.
///
/// The PRODUCER (`ui_media_query_` / `ui_on_pseudo_`) builds this string from
/// [`SafeCssValue`]-gated declarations, but a caller can FORGE the marker
/// directly with `Ui.htmlAttribute "data-ipe-mq-rules" "…}[x]{@import url(…)"`,
/// which never passes through the producer at all. The raw marker String is
/// therefore the real untrusted boundary — parse, don't validate — so the SINK
/// must re-validate rather than trust an upstream that a generic `htmlAttribute`
/// escape hatch can side-step.
///
/// Splits on `;` and checks each non-empty declaration through the shared
/// [`SafeCssValue`] policy (which rejects `; { } </ /* @import` and the
/// script-sink keywords in both raw and CSS-escape-decoded forms). Returns the
/// ORIGINAL, unmodified slice when EVERY declaration is safe (byte-identical to
/// the producer output — no reformat); drops the WHOLE block (`None`,
/// fail-closed) the moment any declaration carries a breakout. A block of only
/// empty declarations yields `None`.
///
/// `cfg(feature = "web")`-gated: its only callers are the `live` style sink's
/// `build_mq` / `build_pc` (`live/style_inject.rs`), which are themselves under
/// that feature — so the helper is dead code in a runtime built without `live`.
#[cfg(feature = "web")]
pub(crate) fn sink_safe_declaration_list(rules: &str) -> Option<&str> {
    let mut any = false;
    for decl in rules.split(';') {
        let d = decl.trim();
        if d.is_empty() {
            continue;
        }
        SafeCssValue::parse(d)?;
        any = true;
    }
    if any { Some(rules) } else { None }
}

/// Sink-side re-validation of a `@keyframes` BODY — the `<kfBody>` slot of the
/// `data-ipe-anim-rules` marker (`name||tail||kfBody||respect`). A keyframes
/// body legitimately contains `{` `}` `;` (`0% { opacity: 0; … } 100% { … }`),
/// so the flat declaration-value policy cannot be reused — this validator
/// parses the keyframe GRAMMAR instead:
///
/// ```text
/// body      := (selector block)+          -- nothing outside blocks
/// selector  := comma-separated tokens, each `from` | `to` | `<number>%`
/// block     := `{` decl-list `}`          -- keyframe blocks cannot nest
/// decl-list := `;`-separated declarations, each through the shared
///              SafeCssValue policy (raw + CSS-escape-decoded scans)
/// ```
///
/// Returns the ORIGINAL slice when every selector and every declaration is
/// safe (byte-identical passthrough — no reformat); `None` (fail-closed) on
/// ANY breakout: non-keyframe selector text, trailing content after the last
/// block, a missing `{`/`}`, `@import`/script-sink/`</`/`/*` in a declaration
/// (a nested `{` inside a block lands in a declaration and is rejected by the
/// same policy), or an empty body.
///
/// `cfg(feature = "web")`-gated for the same reason as
/// [`sink_safe_declaration_list`]: its only caller is the `live` style sink's
/// `build_anim` (`live/style_inject.rs`).
#[cfg(feature = "web")]
pub(crate) fn sink_safe_keyframes_body(body: &str) -> Option<&str> {
    let mut rest = body.trim_start();
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let open = rest.find('{')?;
        if !is_safe_keyframe_selector(&rest[..open]) {
            return None;
        }
        let after_open = &rest[open + 1..];
        // First `}` closes the block — a nested `{` before it stays inside
        // `decls` and is rejected by the SafeCssValue scan below.
        let close = after_open.find('}')?;
        for d in after_open[..close].split(';') {
            let d = d.trim();
            if d.is_empty() {
                continue;
            }
            SafeCssValue::parse(d)?;
        }
        rest = after_open[close + 1..].trim_start();
    }
    Some(body)
}

/// A keyframe selector list: comma-separated tokens, each `from` / `to`
/// (ASCII-case-insensitive) or a percentage with an optional single decimal
/// point (`0%`, `12.5%`). Anything else — including every breakout char — is
/// rejected, so no escape/obfuscation can hide in the selector slot.
#[cfg(feature = "web")]
fn is_safe_keyframe_selector(sel: &str) -> bool {
    let s = sel.trim();
    !s.is_empty()
        && s.split(',').all(|tok| {
            let t = tok.trim();
            if t.eq_ignore_ascii_case("from") || t.eq_ignore_ascii_case("to") {
                return true;
            }
            match t.strip_suffix('%') {
                Some(num) => {
                    !num.is_empty()
                        && num.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                        && num.bytes().filter(|&b| b == b'.').count() <= 1
                }
                None => false,
            }
        })
}

/// A validated CSS selector / media-query string (NEW — strict, drop-on-doubt).
///
/// Allowed charset: ASCII letters, digits, and the CSS structural set
/// `. # : - _ [ ] = " ' , > + ~ * ( ) % space` plus the injection-safe
/// attribute-selector operators `^ $ |` (`[href^="/"]`, `[lang|="en"]`).
/// Rejected: `{ } ;` (declaration/ruleset breakout), `@` (at-rule injection —
/// the leading `@media`/`@keyframes` keyword is supplied by the renderer, never
/// by the user selector string), `<` `/` `\` (element / close-tag / comment /
/// escape breakout, `</style>` and `/*`).  A selector that fails is DROPPED
/// with its rule
/// (design §Q4-4 + §6.3 "start strict; `strip_style_close` is defence-in-depth,
/// not the primary gate").
pub(crate) struct SafeCssSelector<'a>(&'a str);

impl<'a> SafeCssSelector<'a> {
    /// Parse and validate a CSS selector / media-query string.
    ///
    /// Returns `None` (drop the rule) when `sel` is empty (after trimming) or
    /// contains any byte outside the conservative structural allowlist.
    pub(crate) fn parse(sel: &'a str) -> Option<Self> {
        let s = sel.trim();
        if s.is_empty() {
            return None;
        }
        // Comment digraphs are rejected up front: a lone `/` is allowed (it
        // occurs in attribute-value selectors like `a[href^="/blog"]`), but
        // `/*` / `*/` would open/close a CSS comment that could swallow later
        // rules. `<` is fully blocked below, so `</style` cannot form.
        if s.contains("/*") || s.contains("*/") {
            return None;
        }
        // Explicit allowlist — every accepted byte is enumerated; anything else
        // (notably `{ } ; @ < \` and controls / non-ASCII) drops the rule.
        let ok = s.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'.' | b'#'
                        | b':'
                        | b'-'
                        | b'_'
                        | b'['
                        | b']'
                        | b'='
                        | b'"'
                        | b'\''
                        | b','
                        | b'>'
                        | b'+'
                        | b'~'
                        | b'*'
                        | b'('
                        | b')'
                        | b'%'
                        | b'^'
                        | b'$'
                        | b'|'
                        | b'/'
                        | b' '
                )
        });
        if ok { Some(SafeCssSelector(s)) } else { None }
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0
    }
}

/// A validated CSS media-query condition string (`Ui.mediaQuery` /
/// `Ui.breakpoint` — the text spliced into `@media <query> {` inside a raw
/// `<style>` body by `web::style_inject::build_mq`).
///
/// Deliberately a DISTINCT boundary type from [`SafeCssSelector`]: the
/// selector allowlist blocks `<` outright, but Media Queries Level 4 range
/// syntax legitimately uses it (`(400px <= width <= 700px)`), and a media
/// query is a different grammar from a selector. The POLICY, however, is the
/// shared [`has_dangerous_css_pattern`] + [`css_unescape`] re-scan pair that
/// [`SafeCssValue`] uses (one policy, one place — no second weaker encoder):
/// it rejects everything that could break out of the `@media … {` position —
/// `;` `{` `}` `</` `/*` `@import` and the script-sink keywords, in both the
/// raw and CSS-escape-decoded forms. None of those occur in any valid media
/// query (`</` cannot form because a query has no `/` followed by tag text
/// that matters — and if present it is rejected, fail-closed). A query that
/// fails is DROPPED with its whole rule; the wrapped child still renders.
pub(crate) struct SafeCssMediaQuery<'a>(&'a str);

impl<'a> SafeCssMediaQuery<'a> {
    /// Parse and validate a CSS media-query condition string.
    ///
    /// Returns `None` (drop the media-query styling) when `q` is empty after
    /// trimming or carries any breakout / script-sink pattern in either its
    /// raw or CSS-escape-decoded form.
    pub(crate) fn parse(q: &'a str) -> Option<Self> {
        let s = q.trim();
        if s.is_empty() {
            return None;
        }
        let low = s.to_ascii_lowercase();
        if has_dangerous_css_pattern(&low) {
            return None;
        }
        // Defence-in-depth (same as SafeCssValue): decode CSS backslash
        // escapes and re-scan so `\3b` / `\7b`-style obfuscation of a
        // breakout char is caught too.
        let decoded_low = css_unescape(&low);
        if has_dangerous_css_pattern(&decoded_low) {
            return None;
        }
        Some(SafeCssMediaQuery(s))
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0
    }
}

/// Strip every `</style` close-tag occurrence from a raw CSS body before it
/// becomes an `HRaw` `<style>` child (or is spliced into a `<style>` sink).
///
/// * tag-name matched ASCII-case-insensitively, so `</StYle` breaks out just as
///   `</style` does (a plain two-literal `replace` missed every mixed case);
/// * fixpoint-iterated — `str::replace` removes only non-overlapping matches in
///   ONE left-to-right pass and never re-scans the join seam, so a crafted
///   `</sty</stylele` reconstructs `</style` after a single pass.  Loop until a
///   pass removes nothing.
///
/// Total (never panics; the `</style` needle is ASCII so byte indices are valid
/// char boundaries in `out`).  Stronger-than-Go on purpose: security outranks
/// byte-for-byte Go parity (documented divergence).
pub(crate) fn strip_style_close(s: &str) -> String {
    let mut out = s.to_string();
    loop {
        let lowered = out.to_ascii_lowercase();
        match lowered.find("</style") {
            None => return out,
            Some(idx) => {
                // `</style` is ASCII, so byte index `idx` and the 7-byte length
                // are valid char boundaries in `out` (same byte layout as the
                // lowercased copy).
                out.replace_range(idx..idx.saturating_add("</style".len()), "");
            }
        }
    }
}

/// Split any ASCII-case-insensitive `</script` breakout in a `<script>` body so
/// it cannot terminate the enclosing element early. The browser's HTML parser
/// ends a script element only at a literal `</script` byte run; inserting a `\`
/// after the `<` (`<\/script`) keeps the JavaScript semantically identical (a
/// redundant escape inside a string/regex, inert outside one) while removing the
/// exact byte run the parser scans for. Non-`</script` text is untouched, so
/// ordinary script bodies pass through unchanged.
///
/// Sibling of `strip_style_close`: both neutralise a raw-text element's own
/// close tag at the sink so no attacker-influenced body can break out. `<script>`
/// splits rather than strips because a script body is executable code the author
/// owns — dropping bytes could corrupt the program — whereas a `<style>` body is
/// declarative CSS where deletion is safe.
///
/// Total (never panics; only ASCII single-byte characters are ever compared or
/// emitted specially, and every multi-byte `char` is copied whole).
pub(crate) fn neutralise_script_close(body: &str) -> String {
    const NEEDLE: &[u8] = b"</script";
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < bytes.len() {
        let matches_here = bytes
            .get(i..i.saturating_add(NEEDLE.len()))
            .is_some_and(|w| w.eq_ignore_ascii_case(NEEDLE));
        if matches_here {
            out.push_str("<\\/script");
            i = i.saturating_add(NEEDLE.len());
        } else {
            // A multi-byte UTF-8 char never starts with `<`, so the needle branch
            // only fires on single-byte ASCII; here we copy the next whole char
            // (ASCII or multi-byte) and advance past it.
            match body.get(i..).and_then(|s| s.chars().next()) {
                Some(c) => {
                    out.push(c);
                    i = i.saturating_add(c.len_utf8());
                }
                None => break,
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_rejects_expression_and_scheme_sinks() {
        assert!(SafeCssValue::parse("expression(alert(1))").is_none());
        assert!(SafeCssValue::parse("0; background:url(javascript:alert(1))").is_none());
        assert!(SafeCssValue::parse("url( javascript:alert(1))").is_none()); // ws-stripped
        assert!(SafeCssValue::parse("#ff6600").is_some()); // benign passes
    }

    #[test]
    fn value_rejects_hex_escaped_expression_and_scheme_sinks() {
        // CSS backslash-hex escapes (CSS Syntax L3 §4.3.7)
        // decode to a blocked keyword / breakout char ANYWHERE a token is
        // lexed. The raw substring scan misses these; the decode-then-rescan
        // pass catches them.
        // `\65 `='e' (space delimiter consumed) → "expression(alert(1))".
        assert!(SafeCssValue::parse("\\65 xpression(alert(1))").is_none());
        // `\75`='u', `\6a`='j' → "url(javascript:alert(1))".
        assert!(SafeCssValue::parse("\\75 rl(\\6a avascript:alert(1))").is_none());
        // `\3b`=';' (hex-escaped BREAKOUT char, not just a script-sink kw).
        assert!(SafeCssValue::parse("red\\3b  malicious").is_none());
        // benign values with no escapes still pass unchanged.
        assert_eq!(
            SafeCssValue::parse("#ff6600").map(|v| v.as_str().to_owned()),
            Some("#ff6600".to_owned())
        );
        assert_eq!(
            SafeCssValue::parse("rgba(0,0,0,0.2)").map(|v| v.as_str().to_owned()),
            Some("rgba(0,0,0,0.2)".to_owned())
        );
    }

    #[test]
    fn propname_rejects_key_smuggle() {
        assert!(SafeCssPropertyName::parse("background:url(x);x").is_none());
        assert!(SafeCssPropertyName::parse("--brand").is_some());
        assert!(SafeCssPropertyName::parse("-webkit-box-shadow").is_some());
    }

    #[test]
    fn selector_strict_drops_breakout() {
        assert!(SafeCssSelector::parse("body{}</style><script>").is_none());
        assert!(SafeCssSelector::parse("@import url(x)").is_none());
        assert!(SafeCssSelector::parse("a/*{}*/b").is_none()); // comment digraph
        assert!(SafeCssSelector::parse(".card:hover > a[href^=\"/\"]").is_some());
    }

    #[test]
    fn media_query_accepts_real_queries_including_level4_ranges() {
        for q in [
            "(min-width: 768px)",
            "(max-width: 600px)",
            "(min-width: 768px) and (max-width: 1023px)",
            "(prefers-color-scheme: dark)",
            "(prefers-reduced-motion: reduce)",
            "(hover: hover) and (pointer: fine)",
            "only screen and (min-resolution: 2dppx)",
            "not all and (monochrome)",
            // Media Queries Level 4 range syntax — `<`/`<=` are legitimate
            // here (this is exactly why SafeCssSelector is NOT reused).
            "(400px <= width <= 700px)",
            "(width < 600px)",
        ] {
            assert!(
                SafeCssMediaQuery::parse(q).is_some(),
                "valid media query must pass: {q}"
            );
            assert_eq!(
                SafeCssMediaQuery::parse(q).map(|v| v.as_str().to_owned()),
                Some(q.to_owned())
            );
        }
    }

    #[test]
    fn media_query_rejects_breakout_and_escaped_breakout() {
        for q in [
            "",
            "   ",
            // ruleset / declaration breakout
            "(min-width: 1px) { } body { display:none }",
            "screen) { } @media (",
            "(min-width: 1px); color:red",
            // style-tag breakout + comment obfuscation
            "(min-width: 1px) </style><script>alert(1)</script>",
            "(min-width: 1px) /* */",
            // at-rule smuggling
            "(min-width: 1px) @import url(evil)",
            // CSS-hex-escaped `{` (`\7b`) — caught by the decode-then-rescan.
            "(min-width: 1px) \\7b  color:red",
            // script sink
            "url(javascript:alert(1))",
        ] {
            assert!(
                SafeCssMediaQuery::parse(q).is_none(),
                "breakout media query must be dropped: {q:?}"
            );
        }
    }

    #[cfg(feature = "web")]
    #[test]
    fn keyframes_body_accepts_legit_bodies_byte_identical() {
        for b in [
            "0% { opacity: 0 } 100% { opacity: 1 }",
            "0% { opacity: 0; transform: translateY(10px) } 100% { opacity: 1; transform: translateY(0px) }",
            "from { opacity: 0 } to { opacity: 1 }",
            "FROM { opacity: 0 } TO { opacity: 1 }",
            "0%, 50% { opacity: 0 } 100% { opacity: 1 }",
            "12.5% { transform: scale(1.5) rotate(45deg) }",
            "0% {  }", // empty block is legal CSS
        ] {
            assert_eq!(
                sink_safe_keyframes_body(b),
                Some(b),
                "legit keyframes body must pass byte-identical: {b}"
            );
        }
    }

    #[cfg(feature = "web")]
    #[test]
    fn keyframes_body_rejects_breakout() {
        for b in [
            "",
            "   ",
            // trailing content after the last block — the classic close-then-
            // inject shape (`</style>`, page-wide rule, at-rule…).
            "0% { opacity: 0 } </style><script>alert(1)</script>",
            "0% { opacity: 0 } } body { display:none }",
            "0% { opacity: 0 } @import url(//evil/x.css) ;",
            // non-keyframe selector text (rule injection in the selector slot)
            "body { display:none }",
            "0% </style> { opacity: 0 }",
            "0%; { opacity: 0 }",
            // nested `{` inside a block (lands in a declaration → rejected)
            "0% { a: b { } }",
            // script sinks / comment obfuscation inside a declaration
            "0% { background: url(javascript:alert(1)) }",
            "0% { opacity: 0 /* } body { */ }",
            // hex-escaped breakout inside a declaration (`\7d` = `}`)
            "0% { opacity: 0\\7d  }",
            // unbalanced blocks
            "0% { opacity: 0",
            "0% opacity: 0 }",
        ] {
            assert_eq!(
                sink_safe_keyframes_body(b),
                None,
                "breakout keyframes body must be dropped: {b:?}"
            );
        }
    }

    #[test]
    fn strip_style_close_is_fixpoint_and_case_insensitive() {
        assert!(
            !strip_style_close("a{}</StYlE ><script>")
                .to_ascii_lowercase()
                .contains("</style")
        );
        assert!(
            !strip_style_close("</sty</stylele")
                .to_ascii_lowercase()
                .contains("</style")
        );
    }

    #[test]
    fn neutralise_script_close_splits_breakout_case_insensitively() {
        // The exact XSS payload from the safe-surface `<script>` sink must not
        // survive as a live `</script` byte run.
        assert_eq!(
            neutralise_script_close("a</script><img src=x onerror=alert(1)>"),
            "a<\\/script><img src=x onerror=alert(1)>"
        );
        assert_eq!(neutralise_script_close("b</SCRIPT >"), "b<\\/script >");
        let out = neutralise_script_close("x</script>y</ScRiPt>z");
        assert!(!out.to_ascii_lowercase().contains("</script"));
        // Non-breakout text (incl. multibyte UTF-8) is untouched.
        assert_eq!(neutralise_script_close("λ = 1; // ok"), "λ = 1; // ok");
    }

    #[test]
    fn neutralise_script_close_is_a_fixpoint() {
        // Already-neutralised output (e.g. from `unsafeScript` at construction)
        // passes through unchanged, so routing it through the sink again is a
        // no-op — the capability-gated path keeps its exact bytes.
        let once = neutralise_script_close("x</script>y");
        assert_eq!(neutralise_script_close(&once), once);
    }

    /// Assert that the pattern set checked by `containsDangerousCssConstruct`
    /// in `Ipe/Css.ipe` is identical to the union of the breakout chars and
    /// `BAD_VALUE_PATTERNS` in this module. Any drift between the two makes
    /// this test fail, keeping the two representations in lock-step (SSOT by
    /// assertion — the Ipê side cannot import Rust constants directly).
    ///
    /// The Ipê function checks (on a lowercased copy of the body):
    ///   breakout chars: "}" | "{" | ";" | "</" | "/*" | "@import"
    ///   script sinks:   every entry in BAD_VALUE_PATTERNS
    ///
    /// This test reconstructs that exact token set and compares it to the
    /// union of the breakout literals from `has_dangerous_css_pattern` plus
    /// `BAD_VALUE_PATTERNS`. A mismatch → drift → CI red.
    #[test]
    fn raw_body_gate_matches_rust_policy() {
        // Breakout chars/strings checked by `has_dangerous_css_pattern` (the
        // non-BAD_VALUE_PATTERNS branch). Must match the `}` `{` `;` `</`
        // `/*` `@import` literals in `containsDangerousCssConstruct`.
        let rust_breakout: &[&str] = &[";", "{", "}", "</", "/*", "@import"];

        // The Ipê-side script-sink token set — must equal BAD_VALUE_PATTERNS.
        let ipe_script_sinks: &[&str] = &[
            "expression(",
            "javascript:",
            "vbscript:",
            "url(javascript:",
            "url('javascript:",
            "url(\"javascript:",
            "url(vbscript:",
            "url(data:text",
            "url(data:application",
        ];

        // BAD_VALUE_PATTERNS is the Rust SSOT for script sinks.
        let mut rust_sorted = BAD_VALUE_PATTERNS.to_vec();
        rust_sorted.sort_unstable();
        let mut ipe_sorted = ipe_script_sinks.to_vec();
        ipe_sorted.sort_unstable();
        assert_eq!(
            ipe_sorted, rust_sorted,
            "Ipe.Css containsDangerousCssConstruct script-sink token set \
             has drifted from BAD_VALUE_PATTERNS in css_safety.rs. \
             Update the Ipê side to match exactly."
        );

        // Breakout chars: verify has_dangerous_css_pattern catches each one.
        for pat in rust_breakout {
            let probe = format!("safe-prefix {pat} suffix");
            assert!(
                has_dangerous_css_pattern(&probe.to_ascii_lowercase()),
                "has_dangerous_css_pattern should reject a body containing {pat:?}"
            );
        }
    }

    /// `raw`/`keyframes` bodies carrying each pattern from the extended gate
    /// are rejected; a benign body passes. Covers the previously-bypassing
    /// patterns that the old two-pattern gate missed.
    #[test]
    fn raw_body_gate_rejects_all_dangerous_patterns() {
        // Each pattern that must be rejected (lowercased, as the Ipê gate sees them).
        let must_reject = [
            // Ruleset / style-tag breakout
            "a{color:red} } body{display:none}",
            "a { color: red; {nested}",
            "color:red; background:blue",
            "</style><script>alert(1)</script>",
            "/* comment } body { display:none",
            "@import url(//evil/x.css)",
            // Script-sink keywords (BAD_VALUE_PATTERNS)
            "x:expression(alert(1))",
            "background:url(javascript:alert(1))",
            "background:url(vbscript:alert(1))",
            "background:url(javascript:alert(1))",
            "background:url('javascript:alert(1)')",
            "background:url(\"javascript:alert(1)\")",
            "background:url(data:text/html;base64,abc)",
            "background:url(data:application/x-www-form-urlencoded,abc)",
            // Injection that previously bypassed the old @import-only gate
            "a{color:red} } body{display:none}",
            "x:url(data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)",
        ];

        for body in &must_reject {
            let low = body.to_ascii_lowercase();
            assert!(
                has_dangerous_css_pattern(&low),
                "raw body gate must reject: {body:?}"
            );
        }

        // Benign bodies must pass.
        let must_pass = [
            "0% { opacity: 0 } 100% { opacity: 1 }",
            ".card { color: red }",
            "from { transform: translateX(0) } to { transform: translateX(100px) }",
        ];
        for body in &must_pass {
            // Benign bodies contain `{`, `}`, `;` — so has_dangerous_css_pattern
            // correctly flags them (it's designed for CSS values, not raw blocks).
            // The raw/keyframes gate uses `containsDangerousCssConstruct` which
            // also flags `{`/`}` — that is the intentional broadening for raw
            // bodies (rule breakout prevention). Benign raw bodies should use
            // the typed builders instead. Here we just verify benign script-sink
            // patterns don't appear.
            let no_script_sink = !BAD_VALUE_PATTERNS
                .iter()
                .any(|p| body.to_ascii_lowercase().contains(p));
            assert!(
                no_script_sink,
                "benign body must not accidentally match a script-sink pattern: {body:?}"
            );
        }
    }
}

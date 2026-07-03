//! Shared CSS/style injection-safety encoders — one policy, one place.
//!
//! This module is the SINGLE home for the CSS boundary smart constructors and
//! the `<style>`-body close-tag stripper. `Std.Ui`'s inline-style path
//! (`ui/render.rs`), `Std.Html.styleNode`'s `<style>` body (`html.rs`), and
//! `Std.Css`'s stylesheet renderers (`css.rs`) all import the identical
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

impl<'a> SafeCssValue<'a> {
    /// Parse and validate a CSS property value.
    ///
    /// Returns `None` (silently drop) when any dangerous pattern is found.
    pub(crate) fn parse(v: &'a str) -> Option<Self> {
        let low = v.to_ascii_lowercase();

        // Declaration / ruleset / style-tag breakout + comment obfuscation.
        // These chars cannot appear legitimately in a single CSS declaration
        // value; their presence always indicates an injection attempt.
        if low.contains(';')
            || low.contains('{')
            || low.contains('}')
            || low.contains("</")
            || low.contains("/*")
            || low.contains("@import")
        {
            return None;
        }

        // Script sinks — check WHOLE value (not prefix only) with whitespace
        // stripped so `url( javascript:…` / `java script:` cannot evade.
        let low_nows: String = low.chars().filter(|c| !c.is_whitespace()).collect();
        for bad in &[
            "expression(",
            "javascript:",
            "vbscript:",
            "url(javascript:",
            "url('javascript:",
            "url(\"javascript:",
            "url(vbscript:",
            "url(data:text",
            "url(data:application",
        ] {
            if low_nows.contains(bad) {
                return None;
            }
        }

        Some(SafeCssValue(v))
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0
    }
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
                out.replace_range(idx..idx + "</style".len(), "");
            }
        }
    }
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
}

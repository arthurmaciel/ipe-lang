//! The CSS declaration-value injection-safety policy — the single source of
//! truth for "is this CSS property value free of a breakout / script-sink
//! byte?".
//!
//! This leaf module holds ONLY the value-scan policy (`Css.safeValue`'s gate).
//! It lives here, in the dependency-free kernel crate, because two crates need
//! the identical decision and must never drift:
//!
//! * the **runtime** sanitizer (`css_safety::SafeCssValue::parse`) runs it at
//!   render as the authoritative boundary, and
//! * the **backend** consults it at emit time to decide whether a *direct
//!   literal* CSS value may be hoisted into a view's appearance literal table
//!   for the dev hot-swap loop — a value is hoist-eligible IFF this policy
//!   accepts it, so an un-sanitized value can never be baked as a default.
//!
//! Because the sanitizer keeps the caller's ORIGINAL bytes on success (it is a
//! pass/reject gate, not a transform), a value this policy accepts renders
//! byte-identically whether it took the direct path or the hoisted path, and
//! the runtime sanitizer re-runs on whatever the slot holds — a dev-pushed
//! patch is gated exactly as a compiled value is.
//!
//! Selectors and property *names* are deliberately NOT here: a selector is
//! structure (what a rule targets), not an appearance value, and never hoists.

/// Breakout / script-sink patterns for a CSS declaration value. Checked against
/// both the raw value and its CSS-escape-decoded form (via [`css_value_is_safe`])
/// so a hex-escaped payload cannot slip past — one list, one policy.
const BAD_VALUE_PATTERNS: &[&str] = &[
    "expression(",
    "javascript:",
    "vbscript:",
    // Legacy script-execution properties: Firefox XBL (`-moz-binding:`) and
    // IE HTC (`behavior:`). Neither appears in valid modern CSS; block both as
    // defence-in-depth for contexts that must defend legacy engines.
    "-moz-binding:",
    "behavior:",
    "url(-moz-binding:",
    "url(javascript:",
    "url('javascript:",
    "url(\"javascript:",
    "url(vbscript:",
    "url(data:text",
    "url(data:application",
];

/// True when `low` (already lowercased) carries a declaration/ruleset breakout
/// character or a script-sink keyword. Shared by the raw-value and
/// CSS-escape-decoded-value passes of [`css_value_is_safe`] so they cannot drift.
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
    // Script sinks — whitespace stripped so `url( javascript:…` / `java script:`
    // cannot evade.
    let low_nows: String = low.chars().filter(|c| !c.is_whitespace()).collect();
    BAD_VALUE_PATTERNS.iter().any(|bad| low_nows.contains(bad))
}

/// Decode CSS backslash escapes (CSS Syntax Level 3 §4.3.7) for DETECTION
/// purposes only — the decoded string is never emitted; the caller keeps its
/// ORIGINAL string on success. A value that hides a blocked keyword or breakout
/// char behind a hex escape (`\65 xpression(…)`, `\3b` for `;`) decodes to the
/// literal form here, so [`has_dangerous_css_pattern`] catches it on the second
/// pass.
///
/// Best-effort / fail-closed, not a spec-complete CSS tokenizer: an escape
/// decoding to an invalid Unicode scalar value is dropped rather than
/// reconstructed (erring toward "the scan sees less" is the safe direction for a
/// detector — it never helps an attacker hide a keyword).
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
            // One trailing whitespace char is consumed as the escape's delimiter
            // (CSS Syntax §4.3.7), if present.
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

/// The authoritative `Css.safeValue` decision: `true` iff `v` carries no
/// declaration/ruleset breakout character and no script-sink keyword, in either
/// its raw form or its CSS-escape-decoded form.
///
/// This is a pass/reject gate: the value's bytes are never rewritten, so a
/// `true` result means `v` is safe to emit verbatim. The runtime sanitizer and
/// the backend hoist-eligibility check both call exactly this function.
#[must_use]
pub fn css_value_is_safe(v: &str) -> bool {
    let low = v.to_ascii_lowercase();
    if has_dangerous_css_pattern(&low) {
        return false;
    }
    // Defence-in-depth: decode CSS backslash escapes and re-scan so a hex-escaped
    // bypass of the check above (`\65 xpression(…)`, `\3b` for `;`) is caught too.
    let decoded_low = css_unescape(&low);
    !has_dangerous_css_pattern(&decoded_low)
}

#[cfg(test)]
mod tests {
    use super::css_value_is_safe;

    #[test]
    fn rejects_breakouts_and_script_sinks() {
        assert!(!css_value_is_safe("expression(alert(1))"));
        assert!(!css_value_is_safe("0; background:url(javascript:alert(1))"));
        assert!(!css_value_is_safe("red</style><script>alert(1)</script>"));
        assert!(!css_value_is_safe("url( javascript:alert(1))"));
        // CSS-hex-escaped bypass (`\65 ` -> 'e') caught by the decoded re-scan.
        assert!(!css_value_is_safe("\\65 xpression(alert(1))"));
    }

    #[test]
    fn accepts_benign_values() {
        assert!(css_value_is_safe("#ff6600"));
        assert!(css_value_is_safe("8px"));
        assert!(css_value_is_safe("rgba(0,0,0,0.2)"));
        assert!(css_value_is_safe("1px solid #ccc"));
    }
}

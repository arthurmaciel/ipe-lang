//! `Ipe.Css` leaf security kernels.
//!
//! `Ipe.Css` is compiled **pure Ipê source** (`crates/ipe/stdlib/Std/Css.ipe`):
//! the ADTs (`CssProp` / `CssRule` / `Length` / `Color` / keyword enums), the
//! typed builders, and the render fold all live in Ipê.  The ONLY Rust surface
//! is the four *leaf* security kernels below — thin `pub` shims over the shared,
//! audited `css_safety` policy.  The compiled `Ipe.Css` imports them via
//! `Ipe.CssSafety` and funnels every free-string entry point through them
//! at construction (PARSE, DON'T VALIDATE):
//!
//! * `safe_value`      — `Css.safeValue    : String -> Maybe String`
//! * `safe_prop_name`  — `Css.safePropName : String -> Maybe String`
//! * `safe_selector`   — `Css.safeSelector : String -> Maybe String`
//! * `strip_style_close_kernel` — `Css.stripStyleClose : String -> String`
//!   (the `<style>`-body breakout floor for raw fragments)
//!
//! Policy is single-sourced in `css_safety.rs` (unchanged, audited).  A value /
//! name / selector that fails policy yields `None`, which the Ipê side turns
//! into the explicit `CssDropped` / `CssRuleDropped` state — never a silent
//! partial emit.  There is no ADT ↔ runtime-enum reflection (Design-2 retired):
//! the leaf kernels are primitive-typed, so nothing here can `ipe`-succeed and
//! then `cargo`-fail.

use crate::core::IpeMaybe;
use crate::css_safety::{SafeCssPropertyName, SafeCssSelector, SafeCssValue, strip_style_close};

/// Lift a policy `Option<String>` into the Ipê `Maybe String` runtime
/// representation (`IpeMaybe<String>`), matching every other `String -> Maybe
/// String` kernel (e.g. `uuid_parse`). The backend schemes these three kernels
/// as `String -> Maybe String`, so the emitted call site expects `IpeMaybe`, not
/// `Option`.
#[inline]
fn to_sky_maybe(opt: Option<String>) -> IpeMaybe<String> {
    match opt {
        Some(clean) => IpeMaybe::Just(clean),
        None => IpeMaybe::Nothing,
    }
}

/// `Css.safeValue : String -> Maybe String`.  Parse a CSS declaration value
/// through the audited whole-string scan; `Just(clean)` iff it carries no
/// breakout / script-sink byte, else `Nothing` (the Ipê side drops the
/// declaration).
#[must_use]
pub fn safe_value(v: String) -> IpeMaybe<String> {
    to_sky_maybe(SafeCssValue::parse(&v).map(|s| s.as_str().to_string()))
}

/// `Css.safePropName : String -> Maybe String`.  Parse a CSS property name
/// (charset `[A-Za-z0-9-]`, custom props `--foo` included); `Nothing` closes the
/// key-injection vector (`background:url(x);y`).
#[must_use]
pub fn safe_prop_name(k: String) -> IpeMaybe<String> {
    to_sky_maybe(SafeCssPropertyName::parse(&k).map(|s| s.as_str().to_string()))
}

/// `Css.safeSelector : String -> Maybe String`.  Parse a selector / media-query
/// string through the strict structural allowlist; `Nothing` (drop the rule)
/// closes `{ } ; @ < / \`-based breakout (`@import`-via-selector included).
#[must_use]
pub fn safe_selector(sel: String) -> IpeMaybe<String> {
    to_sky_maybe(SafeCssSelector::parse(&sel).map(|s| s.as_str().to_string()))
}

/// `Css.stripStyleClose : String -> String`.  The breakout floor: strip every
/// (case-insensitive, fixpoint-iterated) `</style` occurrence from a raw CSS
/// body before it can reach a `<style>` sink.  Total (never panics).
#[must_use]
pub fn strip_style_close_kernel(s: String) -> String {
    strip_style_close(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collapse a `IpeMaybe<String>` to `Option<&str>` for terse assertions.
    fn opt(m: &IpeMaybe<String>) -> Option<&str> {
        match m {
            IpeMaybe::Just(s) => Some(s.as_str()),
            IpeMaybe::Nothing => None,
        }
    }

    #[test]
    fn safe_value_drops_injection_keeps_benign() {
        // expression() / mid-value url(javascript:) / breakout chars dropped.
        assert!(matches!(
            safe_value("expression(alert(1))".into()),
            IpeMaybe::Nothing
        ));
        assert!(matches!(
            safe_value("0; background:url(javascript:alert(1))".into()),
            IpeMaybe::Nothing
        ));
        assert!(matches!(
            safe_value("red</style><script>alert(1)</script>".into()),
            IpeMaybe::Nothing
        ));
        // CSS-hex-escaped bypass (`\65 ` → 'e') dropped too.
        assert!(matches!(
            safe_value("\\65 xpression(alert(1))".into()),
            IpeMaybe::Nothing
        ));
        // benign values pass through unchanged.
        assert_eq!(opt(&safe_value("#ff6600".into())), Some("#ff6600"));
        assert_eq!(opt(&safe_value("8px".into())), Some("8px"));
        assert_eq!(
            opt(&safe_value("rgba(0,0,0,0.2)".into())),
            Some("rgba(0,0,0,0.2)")
        );
    }

    #[test]
    fn safe_prop_name_drops_smuggle_keeps_custom_props() {
        assert!(matches!(
            safe_prop_name("background:url(x);y".into()),
            IpeMaybe::Nothing
        ));
        assert_eq!(opt(&safe_prop_name("--brand".into())), Some("--brand"));
        assert_eq!(
            opt(&safe_prop_name("-webkit-box-shadow".into())),
            Some("-webkit-box-shadow")
        );
    }

    #[test]
    fn safe_selector_drops_breakout_keeps_structure() {
        assert!(matches!(
            safe_selector("body{}</style><script>".into()),
            IpeMaybe::Nothing
        ));
        assert!(matches!(
            safe_selector("@import url(x)".into()),
            IpeMaybe::Nothing
        ));
        // media query + structural selectors pass.
        assert_eq!(
            opt(&safe_selector("(max-width: 768px)".into())),
            Some("(max-width: 768px)")
        );
        assert_eq!(
            opt(&safe_selector(".card:hover > a".into())),
            Some(".card:hover > a")
        );
    }

    #[test]
    fn strip_style_close_neutralises_breakout() {
        assert!(
            !strip_style_close_kernel("a{}</StYlE ><script>".into())
                .to_ascii_lowercase()
                .contains("</style")
        );
        // benign body is identity (Go byte-parity for non-attack input).
        assert_eq!(
            strip_style_close_kernel(".card {\n  color: red;\n}\n".into()),
            ".card {\n  color: red;\n}\n"
        );
    }
}

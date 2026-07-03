//! `Std.Css` stylesheet renderers — the gated CSS sinks (#47).
//!
//! Only the two functions that fold user strings into a final CSS string are
//! kernels: `css_stylesheet_` (`List CssRule -> String`) and `css_styles_`
//! (`List CssProp -> String`).  The typed length/colour constructors
//! (`px`/`rem`/`hex`/`rgb`/…) stay pure Sky in `Std/Css.sky` — they stringify
//! bounded numeric fields to digits and cannot express injection, so they never
//! reach this boundary.
//!
//! Security posture (design §Q4): every free-string entry point that reaches a
//! declaration value or a selector is parsed ONCE through the shared
//! `css_safety` encoder; a value/selector that fails policy causes the
//! declaration (or the whole rule) to be DROPPED — an explicit, documented drop,
//! never a silent partial emit.  The assembled body is additionally run through
//! `strip_style_close` (defence in depth) so a `stylesheet` string spliced into
//! a `<style>` is gated twice (belt and braces with `html_style_node_`).

use crate::sky_runtime::css_safety::{
    SafeCssPropertyName, SafeCssSelector, SafeCssValue, strip_style_close,
};

/// A single CSS declaration `key: value` — reflection of `Std.Css`'s `CssProp`
/// ADT at the Rust kernel boundary.
#[derive(Clone, Debug, PartialEq)]
pub enum CssProp {
    /// `CssProp key value` — a `key: value` declaration.
    CssProp(String, String),
}

/// A top-level stylesheet entry — reflection of `Std.Css`'s `CssRule` ADT at the
/// Rust kernel boundary.  Every variant has an explicit render arm (walker-arm
/// invariant — no `_ =>` swallow).
#[derive(Clone, Debug, PartialEq)]
pub enum CssRule {
    /// `selector { props }`.
    CssRule(String, Vec<CssProp>),
    /// `@media query { rules }`.
    CssMedia(String, Vec<CssRule>),
    /// `@keyframes name { frames }` — frames are pre-rendered fragments.
    CssKeyframes(String, Vec<String>),
    /// A pre-rendered fragment emitted verbatim EXCEPT for close-tag stripping
    /// (the deferred `Css.raw` escape hatch; no lower entry ships it — this arm
    /// exists so a value reaching the kernel is still neutralised).
    CssRaw(String),
}

/// Render one declaration, gated.  Key AND value must both parse; otherwise the
/// declaration is DROPPED (explicit, documented — no partial emit, no `_ =>`).
fn render_prop(p: &CssProp) -> Option<String> {
    let CssProp::CssProp(k, v) = p;
    let key = SafeCssPropertyName::parse(k)?;
    let val = SafeCssValue::parse(v)?;
    Some(format!("{}:{}", key.as_str(), val.as_str()))
}

/// Render one stylesheet entry.  Every `CssRule` variant is handled explicitly.
fn render_rule(r: &CssRule) -> String {
    match r {
        // `selector { decl; decl }` — selector gated; each declaration gated,
        // failures dropped. A failing selector drops the whole rule (its
        // declarations included).
        CssRule::CssRule(sel, props) => match SafeCssSelector::parse(sel) {
            None => String::new(),
            Some(sel) => {
                let decls = props
                    .iter()
                    .filter_map(render_prop)
                    .collect::<Vec<_>>()
                    .join(";");
                format!("{}{{{}}}", sel.as_str(), decls)
            }
        },
        // `@media query { rules }` — query gated via the selector grammar; the
        // `@media ` keyword is supplied HERE, never by the user string.
        CssRule::CssMedia(q, rules) => match SafeCssSelector::parse(q) {
            None => String::new(),
            Some(q) => {
                let body = rules.iter().map(render_rule).collect::<String>();
                format!("@media {}{{{}}}", q.as_str(), body)
            }
        },
        // `@keyframes name { frames }` — name gated via the selector grammar;
        // each frame fragment is close-tag stripped (it may carry a raw body).
        CssRule::CssKeyframes(name, frames) => match SafeCssSelector::parse(name) {
            None => String::new(),
            Some(name) => {
                let body = frames
                    .iter()
                    .map(|f| strip_style_close(f))
                    .collect::<String>();
                format!("@keyframes {}{{{}}}", name.as_str(), body)
            }
        },
        // Deferred escape hatch — strip-only. `Css.raw` has no lower entry, so
        // this is only reachable defensively; the body is close-tag neutralised.
        CssRule::CssRaw(s) => strip_style_close(s),
    }
}

// ── Builder kernels — construct the runtime ADT values ────────────────────────
// These mirror the `Std.Ui` precedent (`ui_el_` / `ui_row_` build runtime
// `Element` values): the `Std.Css` builder functions are kernels that construct
// the runtime `CssProp` / `CssRule` enums, so the values reach the gated sink
// kernels (`css_stylesheet_` / `css_styles_`) as typed Rust enums. No user
// string is inspected here — gating happens once at the sink.

/// `Css.property : String -> String -> CssProp`.
pub fn css_property_(key: String, value: String) -> CssProp {
    CssProp::CssProp(key, value)
}

/// `Css.rule : String -> List CssProp -> CssRule`.
pub fn css_rule_(selector: String, props: Vec<CssProp>) -> CssRule {
    CssRule::CssRule(selector, props)
}

/// `Css.media : String -> List CssRule -> CssRule`.
pub fn css_media_(query: String, rules: Vec<CssRule>) -> CssRule {
    CssRule::CssMedia(query, rules)
}

/// `Css.keyframes : String -> List String -> CssRule`.
pub fn css_keyframes_(name: String, frames: Vec<String>) -> CssRule {
    CssRule::CssKeyframes(name, frames)
}

/// `Css.stylesheet : List CssRule -> String`.  Assemble every rule, then run the
/// whole body through `strip_style_close` (defence in depth — a selector or
/// value that slipped a `</style` is caught here regardless).
pub fn css_stylesheet_(rules: Vec<CssRule>) -> String {
    strip_style_close(&rules.iter().map(render_rule).collect::<String>())
}

/// `Css.styles : List CssProp -> String`.  Fold gated declarations into a
/// `;`-joined inline-style string; failing declarations are dropped.
pub fn css_styles_(props: Vec<CssProp>) -> String {
    props
        .iter()
        .filter_map(render_prop)
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_expression_value_dropped() {
        // #7 — `expression()` sink never renders.
        let s = css_styles_(vec![CssProp::CssProp(
            "width".into(),
            "expression(alert(1))".into(),
        )]);
        assert!(s.is_empty());
    }

    #[test]
    fn property_midvalue_scheme_dropped() {
        // #7 — mid-value `url(javascript:)` (prefix-bypass) dropped.
        let s = css_styles_(vec![CssProp::CssProp(
            "background".into(),
            "0; background:url(javascript:alert(1))".into(),
        )]);
        assert!(s.is_empty());
    }

    #[test]
    fn property_close_tag_in_value_dropped_and_stripped() {
        // #6 — `</style><script>` in a declaration value: the `;`/`</`-bearing
        // value is dropped by SafeCssValue AND the assembled body is stripped.
        let sheet = css_stylesheet_(vec![CssRule::CssRule(
            "body".into(),
            vec![CssProp::CssProp(
                "color".into(),
                "red</style><script>alert(1)</script>".into(),
            )],
        )]);
        assert!(!sheet.to_ascii_lowercase().contains("</style"));
        assert!(!sheet.contains("<script>alert(1)"));
        assert!(!sheet.contains("red</style"));
    }

    #[test]
    fn selector_breakout_drops_rule() {
        // #9 — a selector that carries `{ } < /` is dropped with its rule.
        let sheet = css_stylesheet_(vec![CssRule::CssRule(
            "body{}</style><script>".into(),
            vec![CssProp::CssProp("color".into(), "red".into())],
        )]);
        assert!(sheet.trim().is_empty() || !sheet.to_ascii_lowercase().contains("<script"));
    }

    #[test]
    fn media_query_breakout_neutralised() {
        // A raw media query carrying a `</style><script>` breakout is dropped
        // (the `<`/`/` bytes fail the selector grammar), so no rule renders.
        let sheet = css_stylesheet_(vec![CssRule::CssMedia(
            "(min-width: 1px) </style><script>alert(1)</script>".into(),
            vec![CssRule::CssRule(
                ".card".into(),
                vec![CssProp::CssProp("color".into(), "red".into())],
            )],
        )]);
        assert!(!sheet.to_ascii_lowercase().contains("</style"));
        assert!(!sheet.to_ascii_lowercase().contains("<script"));
    }

    #[test]
    fn benign_stylesheet_go_parity_shape() {
        // Benign input renders the expected byte-shape.
        let sheet = css_stylesheet_(vec![CssRule::CssRule(
            ".card".into(),
            vec![CssProp::CssProp("color".into(), "#ff6600".into())],
        )]);
        assert!(sheet.contains(".card") && sheet.contains("#ff6600"));
    }

    #[test]
    fn benign_media_and_styles_render() {
        let mq = css_stylesheet_(vec![CssRule::CssMedia(
            "(min-width: 768px)".into(),
            vec![CssRule::CssRule(
                ".card".into(),
                vec![CssProp::CssProp("display".into(), "flex".into())],
            )],
        )]);
        assert!(mq.contains("@media (min-width: 768px)"));
        assert!(mq.contains(".card{display:flex}"));

        let inline = css_styles_(vec![
            CssProp::CssProp("color".into(), "#fff".into()),
            CssProp::CssProp("padding".into(), "8px".into()),
        ]);
        assert_eq!(inline, "color:#fff;padding:8px");
    }
}

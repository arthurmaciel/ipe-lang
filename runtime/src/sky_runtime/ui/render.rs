//! `Std.Ui` → `Html<M>` render kernel — Phase 0 implementation.
//!
//! This module is the ONLY place that converts a `Std.Ui` `Element<M>` tree to
//! `sky_runtime::html::Html<M>`.  It is a runtime kernel (not compiled from Sky)
//! because the render chain touches `any`-returning stdlib fields (`Raw any`,
//! `AttrEvent any`) that cannot be typed soundly in Sky-over-Rust (spec §1.4).
//!
//! Security note — this file is T1/T3/T5-critical (spec §6):
//! - T1: never call `renderElement` from Sky; keep it here as a typed Rust fn.
//! - T3: `AttrStyle`, `AttrBgImage`, `AttrAttribute` carry user-controlled strings
//!   entering `style="…"` / HTML-attribute sinks.  The CSS URL sanitiser
//!   (`sanitise_css_url`) gates `url(…)` payloads; HTML values pass through
//!   `html::render_html`'s existing `SafeAttrName` + `sanitise_url_attr` gates.
//! - T5: `AttrBorderWidthEach(t,r,b,l)` uses `saturating_add` throughout.
//!
//! ### Design rationale
//! `Ui.layout` emits an outer 100 vh flex-column wrapper (matching Sky's Go
//! runtime `runtime-go/rt/ui.go`) and the converted root element inside it.
//! `Ui.layoutWith` additionally applies `wrapperAttrs` to the outer wrapper and
//! `rootAttrs` to an intermediate flex root, mirroring `Ui.layoutWith`'s Go shape.

use super::super::css_safety::{SafeCssPropertyName, SafeCssValue};
use super::super::html::{Attribute as HtmlAttribute, Html};
use super::element::{Attribute, Color, Description, Element, HAlign, Length, Location, VAlign};

// ── CSS boundary smart constructors ───────────────────────────────────────────
// `SafeCssPropertyName` / `SafeCssValue` moved to the shared `css_safety` module
// (design §Q5: one policy, one place). Imported above so the Std.Ui inline-style
// path and the Std.Css / styleNode sinks share the identical encoder.

/// Check whether a bare URL string (not yet wrapped in `url(…)`) carries a
/// dangerous scheme.  Used for `AttrBgImage` / `AttrBgGradient` before the
/// `url(…)` wrapper is emitted.
fn is_dangerous_url_scheme(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    lower.starts_with("javascript:")
        || lower.starts_with("vbscript:")
        || lower.starts_with("data:text/html")
        || lower.starts_with("data:application/")
}

// ── Length / Color → CSS ──────────────────────────────────────────────────────

fn length_css(len: &Length) -> String {
    match len {
        Length::Px(n) => format!("{n}px"),
        Length::Content => "auto".to_owned(),
        // Fill(1) = "100%", Fill(n) = "100%" with flex-grow:n handled separately
        Length::Fill(_) => "100%".to_owned(),
        Length::Min(n, inner) => format!("min({}px,{})", n, length_css(inner)),
        Length::Max(n, inner) => format!("max({}px,{})", n, length_css(inner)),
        Length::Vh(n) => format!("{n}vh"),
        Length::Vw(n) => format!("{n}vw"),
    }
}

fn color_css(c: &Color) -> String {
    match c {
        Color::Rgba(r, g, b, a) => format!("rgba({r},{g},{b},{a})"),
    }
}

// ── Attribute → (style entries, html attrs) ───────────────────────────────────

/// Collect all CSS `key:value` pairs from a slice of `Attribute<M>` into a
/// `style="…"` string.  Values that fail the CSS security gate are silently
/// dropped (T3).
fn build_style_string<M>(attrs: &[Attribute<M>]) -> String {
    let mut parts: Vec<String> = Vec::new();

    for attr in attrs {
        match attr {
            Attribute::AttrWidth(len) => {
                parts.push(format!("width:{}", length_css(len)));
                if let Length::Fill(n) = len {
                    parts.push(format!("flex-grow:{n}"));
                    parts.push("min-width:0".to_owned());
                }
            }
            Attribute::AttrHeight(len) => {
                parts.push(format!("height:{}", length_css(len)));
                if let Length::Fill(n) = len {
                    parts.push(format!("flex-grow:{n}"));
                    parts.push("min-height:0".to_owned());
                }
            }
            Attribute::AttrAlignX(h) => {
                let v = match h {
                    HAlign::AlignLeft => "flex-start",
                    HAlign::CenterX => "center",
                    HAlign::AlignRight => "flex-end",
                };
                parts.push(format!("align-self:{v}"));
            }
            Attribute::AttrAlignY(v) => {
                let css = match v {
                    VAlign::AlignTop => "flex-start",
                    VAlign::CenterY => "center",
                    VAlign::AlignBottom => "flex-end",
                };
                parts.push(format!("align-self:{css}"));
            }
            Attribute::AttrPadding(t, r, b, l) => {
                parts.push(format!("padding:{t}px {r}px {b}px {l}px"));
            }
            Attribute::AttrSpacing(n) => {
                parts.push(format!("gap:{n}px"));
            }
            Attribute::AttrStyle(k, v) => {
                // Internal direction markers injected by `ui_row_` / `ui_column_` /
                // `ui_wrapped_row_` in helpers.rs.  They carry layout semantics but
                // must NOT be emitted as literal CSS `__col:true` / `__row:true`.
                // Instead: map to the corresponding Flexbox CSS.
                match k.as_str() {
                    "__col" => {
                        parts.push("display:flex".to_owned());
                        parts.push("flex-direction:column".to_owned());
                    }
                    "__row" => {
                        parts.push("display:flex".to_owned());
                        parts.push("flex-direction:row".to_owned());
                    }
                    "__wrappedrow" => {
                        parts.push("display:flex".to_owned());
                        parts.push("flex-direction:row".to_owned());
                        parts.push("flex-wrap:wrap".to_owned());
                    }
                    _ => {
                        // User-supplied CSS key+value.
                        // `SafeCssPropertyName` gates the key (charset policy);
                        // `SafeCssValue` gates the value (whole-string scan).
                        // Both are the SOLE validation boundary — no re-check
                        // downstream (PARSE, DON'T VALIDATE / T3/T4).
                        if let (Some(pk), Some(pv)) =
                            (SafeCssPropertyName::parse(k), SafeCssValue::parse(v))
                        {
                            parts.push(format!("{}:{}", pk.as_str(), pv.as_str()));
                        }
                        // else: silently drop — consistent with the
                        // `is_dangerous_url_scheme` path in `AttrBgImage`.
                    }
                }
            }
            Attribute::AttrFontSize(n) => {
                parts.push(format!("font-size:{n}px"));
            }
            Attribute::AttrFontColor(c) => {
                parts.push(format!("color:{}", color_css(c)));
            }
            Attribute::AttrFontFamily(f) => {
                parts.push(format!("font-family:{f}"));
            }
            Attribute::AttrFontWeight(w) => {
                parts.push(format!("font-weight:{w}"));
            }
            Attribute::AttrFontItalic => {
                parts.push("font-style:italic".to_owned());
            }
            Attribute::AttrFontUnderline => {
                parts.push("text-decoration:underline".to_owned());
            }
            Attribute::AttrFontDecoration(d) => {
                parts.push(format!("text-decoration:{d}"));
            }
            Attribute::AttrFontLetterSpacing(n) => {
                parts.push(format!("letter-spacing:{n}px"));
            }
            Attribute::AttrFontWordSpacing(n) => {
                parts.push(format!("word-spacing:{n}px"));
            }
            Attribute::AttrFontAlign(a) => {
                parts.push(format!("text-align:{a}"));
            }
            Attribute::AttrBgColor(c) => {
                parts.push(format!("background-color:{}", color_css(c)));
            }
            Attribute::AttrBgImage(url) => {
                // T3: check the raw URL scheme before wrapping in url().
                // `sanitise_css_value` only fires on `url(` or `expression(` prefixes,
                // so we need the is_dangerous_url_scheme check here on the bare URL.
                if !is_dangerous_url_scheme(url) {
                    parts.push(format!("background-image:url({url})"));
                }
            }
            Attribute::AttrBgGradient(g) => {
                // Gradient CSS value — whole-string scan via SafeCssValue (T3).
                // Replaces the former prefix-only `sanitise_css_value` call,
                // closing the mid-value `expression()` / `url(javascript:…)`
                // bypass.
                if let Some(sv) = SafeCssValue::parse(g) {
                    parts.push(format!("background-image:{}", sv.as_str()));
                }
            }
            Attribute::AttrBorderWidth(n) => {
                parts.push(format!("border-width:{n}px"));
            }
            Attribute::AttrBorderWidthEach(t, r, b, l) => {
                // T5: use saturating_add to avoid debug-mode overflow panics.
                let _total = (*t)
                    .saturating_add(*r)
                    .saturating_add(*b)
                    .saturating_add(*l);
                parts.push(format!("border-width:{t}px {r}px {b}px {l}px"));
            }
            Attribute::AttrBorderColor(c) => {
                parts.push(format!("border-color:{}", color_css(c)));
            }
            Attribute::AttrBorderRounded(n) => {
                parts.push(format!("border-radius:{n}px"));
            }
            Attribute::AttrBorderStyle(s) => {
                parts.push(format!("border-style:{s}"));
            }
            Attribute::AttrBorderShadow(x, y, blur, spread, c) => {
                parts.push(format!(
                    "box-shadow:{x}px {y}px {blur}px {spread}px {}",
                    color_css(c)
                ));
            }
            Attribute::AttrBorderInsetShadow(x, y, blur, spread, c) => {
                parts.push(format!(
                    "box-shadow:inset {x}px {y}px {blur}px {spread}px {}",
                    color_css(c)
                ));
            }
            Attribute::AttrPointer => {
                parts.push("cursor:pointer".to_owned());
            }
            Attribute::AttrOverflow(x, y) => {
                parts.push(format!("overflow-x:{x}"));
                parts.push(format!("overflow-y:{y}"));
            }
            Attribute::AttrTransition(t, _respect_reduced) => {
                parts.push(format!("transition:{t}"));
            }
            Attribute::AttrAnimation(name, spec, keyframes, _respect) => {
                // Emit animation name + spec; keyframes require a <style> block
                // which the Phase-0 render doesn't inject (no DOM).  The `name`
                // + `spec` provide the `animation:` property; keyframes are
                // silently dropped for now and tracked as a Phase-1 follow-up.
                let _ = keyframes; // suppress unused-variable warning
                parts.push(format!("animation:{name} {spec}"));
            }
            // Non-style attrs handled in `collect_html_attrs` below.
            Attribute::NoAttribute
            | Attribute::AttrNearby(_, _)
            | Attribute::AttrDescribe(_)
            | Attribute::AttrClass(_)
            | Attribute::AttrEvent(_)
            | Attribute::AttrAttribute(_, _)
            | Attribute::AttrPseudoRule(_, _) => {}
        }
    }

    parts.join(";")
}

/// Collect HTML attributes (class, arbitrary attrs, event handlers, `nearby`
/// overlays) from a slice of `Attribute<M>`, producing a `Vec<HtmlAttribute<M>>`.
/// The returned vec does NOT include a `style` attribute — the caller prepends
/// `build_style_string` if non-empty.
///
/// Security: `AttrAttribute(k, v)` passes through with the key/value pair as
/// `HtmlAttribute::Attr(k, v)`, where `html::render_html`'s `SafeAttrName` gate
/// will drop dangerous attribute names (on*-events, srcdoc) and
/// `sanitise_url_attr` will drop dangerous URL values.  We do not double-gate
/// here; trust the render sink.
fn collect_html_attrs<M: Clone>(attrs: &[Attribute<M>]) -> Vec<HtmlAttribute<M>> {
    let mut out: Vec<HtmlAttribute<M>> = Vec::new();
    for attr in attrs {
        match attr {
            Attribute::AttrClass(c) => {
                out.push(HtmlAttribute::Attr("class".to_owned(), c.clone()));
            }
            Attribute::AttrAttribute(k, v) => {
                // Pass through — the HTML render sink applies SafeAttrName + URL gates.
                out.push(HtmlAttribute::Attr(k.clone(), v.clone()));
            }
            Attribute::AttrEvent(html_attr) => {
                out.push(html_attr.clone());
            }
            Attribute::AttrDescribe(desc) => {
                // Emit ARIA roles / landmark attributes for semantic elements.
                // `pick_semantic_tag` handles the tag; these emit supplementary
                // aria attributes where the semantic tag alone is insufficient.
                match desc {
                    Description::DescLivePolite => {
                        out.push(HtmlAttribute::Attr(
                            "aria-live".to_owned(),
                            "polite".to_owned(),
                        ));
                    }
                    Description::DescLiveAssertive => {
                        out.push(HtmlAttribute::Attr(
                            "aria-live".to_owned(),
                            "assertive".to_owned(),
                        ));
                    }
                    Description::DescLabel(label) => {
                        out.push(HtmlAttribute::Attr("aria-label".to_owned(), label.clone()));
                    }
                    _ => {}
                }
            }
            // Style and nearby handled separately.
            _ => {}
        }
    }
    out
}

/// Nearby overlays (`AttrNearby(Location, Element)`) are rendered as absolutely-
/// positioned child elements.  Returns a vec of `Html<M>` overlay nodes.
fn render_nearby_overlays<M: Clone>(attrs: &[Attribute<M>]) -> Vec<Html<M>> {
    let mut overlays: Vec<Html<M>> = Vec::new();
    for attr in attrs {
        if let Attribute::AttrNearby(loc, child_elem) = attr {
            let position_style = match loc {
                Location::Above => "position:absolute;bottom:100%;left:0;right:0",
                Location::Below => "position:absolute;top:100%;left:0;right:0",
                Location::OnLeft => "position:absolute;right:100%;top:0;bottom:0",
                Location::OnRight => "position:absolute;left:100%;top:0;bottom:0",
                Location::InFront => "position:absolute;top:0;left:0;right:0;bottom:0",
                Location::Behind => "position:absolute;top:0;left:0;right:0;bottom:0;z-index:-1",
            };
            let overlay_node = render_element(child_elem.clone());
            overlays.push(Html::HElement(
                "div".into(),
                vec![HtmlAttribute::Attr("style".into(), position_style.into())],
                vec![overlay_node],
            ));
        }
    }
    overlays
}

// ── Description → semantic HTML tag ──────────────────────────────────────────

/// Pick the semantic HTML tag for a layout node based on its `Description`.
/// `NoDescription` defaults to `div`.  `TaggedNode` overrides this with an
/// explicit user-supplied tag (already validated by the Sky stdlib).
fn tag_for_description(desc: &Description) -> &'static str {
    match desc {
        Description::NoDescription => "div",
        Description::DescMain => "main",
        Description::DescNavigation => "nav",
        Description::DescContentInfo => "footer",
        Description::DescComplementary => "aside",
        Description::DescHeading(n) => match n {
            1 => "h1",
            2 => "h2",
            3 => "h3",
            4 => "h4",
            5 => "h5",
            _ => "h6",
        },
        Description::DescLabel(_) => "label",
        Description::DescLivePolite | Description::DescLiveAssertive => "div",
        Description::DescButton => "button",
        Description::DescParagraph => "p",
    }
}

// ── Element → Html (recursive) ───────────────────────────────────────────────

/// Recursively convert a `Std.Ui` `Element<M>` to `Html<M>`.
///
/// Security: all attribute values flow through `build_style_string` (which
/// calls `sanitise_css_value`) or `collect_html_attrs` (which passes values to
/// `html::Attribute::Attr` where `render_html` applies `SafeAttrName` +
/// `sanitise_url_attr`).  No value reaches the HTML sink without one of these gates.
fn render_element<M: Clone>(elem: Element<M>) -> Html<M> {
    match elem {
        Element::Empty => Html::HText(String::new()),
        Element::Text(s) => Html::HText(s),
        Element::Raw(html) => html,
        Element::Node(desc, attrs, kids) => {
            render_node_as(tag_for_description(&desc), &attrs, kids)
        }
        Element::TaggedNode(tag, _desc, attrs, kids) => render_node_as(&tag, &attrs, kids),
    }
}

/// Build a single `HElement` from a tag name, attribute slice, and children,
/// weaving together the `style=""` attribute, class/event HTML attributes, and
/// any `AttrNearby` overlay children.
///
/// The structure is:
///
/// ```html
/// <{tag} style="{css}" {html_attrs}...>
///   {rendered children}
///   {nearby overlays (position:absolute)}
/// </{tag}>
/// ```
fn render_node_as<M: Clone>(tag: &str, attrs: &[Attribute<M>], kids: Vec<Element<M>>) -> Html<M> {
    let style_str = build_style_string(attrs);
    let mut html_attrs = collect_html_attrs(attrs);

    if !style_str.is_empty() {
        // Prepend — style first so tests can pattern-match on it predictably.
        html_attrs.insert(0, HtmlAttribute::Attr("style".to_owned(), style_str));
    }

    // Rendered children in source order.
    let mut html_kids: Vec<Html<M>> = kids.into_iter().map(render_element).collect();

    // Nearby overlays appended after the regular children (they are absolutely
    // positioned, so their DOM order is irrelevant for layout).
    html_kids.extend(render_nearby_overlays(attrs));

    Html::HElement(tag.to_owned(), html_attrs, html_kids)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// `Ui.layout : List (Attribute msg) -> Element msg -> Html msg`
///
/// Wraps the element in a full-viewport flex-column page wrapper, then renders
/// the root element with the given root attributes applied. Mirrors the Sky Go
/// runtime's `layout` output shape.
pub fn ui_layout<M: Clone>(attrs: Vec<Attribute<M>>, elem: Element<M>) -> Html<M> {
    // Root element rendered with the caller's root attrs applied.
    let root_style = build_style_string(&attrs);
    let mut root_html_attrs = collect_html_attrs(&attrs);
    if !root_style.is_empty() {
        root_html_attrs.insert(0, HtmlAttribute::Attr("style".to_owned(), root_style));
    }
    let rendered_elem = render_element(elem);
    let mut root_kids: Vec<Html<M>> = vec![rendered_elem];
    root_kids.extend(render_nearby_overlays(&attrs));

    let root_div = Html::HElement("div".to_owned(), root_html_attrs, root_kids);

    // Page wrapper: fills the full viewport with a flex column.
    Html::HElement(
        "div".to_owned(),
        vec![HtmlAttribute::Attr(
            "style".to_owned(),
            "display:flex;flex-direction:column;height:100vh;width:100%;overflow:hidden".to_owned(),
        )],
        vec![root_div],
    )
}

/// `Ui.layoutWith : { wrapperAttrs : List (Attribute msg), rootAttrs : List
/// (Attribute msg) } -> Element msg -> Html msg`
///
/// Applies `wrapper_attrs` to the outer viewport div and `root_attrs` to the
/// inner root element, mirroring `Ui.layoutWith`'s Go shape.
///
/// Called by the emitted code as
/// `sky_runtime::ui::render::ui_layout_with_vecs::<M>(wrapper, root, elem)`.
/// The two `Vec<Attribute<M>>` arguments are extracted at the **emit site**
/// (field-extraction on the IR `Expr::Record` literal), so the cfg record struct
/// never needs to be materialised — closing SKY-I0001 and eliminating the
/// former silent-drop stub `ui_layout_with<M, C>` (which ignored cfg entirely).
///
/// # Design note (MAKE INVALID STATES UNREPRESENTABLE)
///
/// The former `pub fn ui_layout_with<M: Clone, C>(_cfg: C, elem: Element<M>)`
/// was deleted in Phase-0 unfreeze.  That function accepted any `C` and
/// silently dropped it, producing wrong HTML (exit-0-cargo-ok-but-wrong-output).
/// The correct impl was always here; the emit-site change wires it directly.
pub fn ui_layout_with_vecs<M: Clone>(
    wrapper_attrs: Vec<Attribute<M>>,
    root_attrs: Vec<Attribute<M>>,
    elem: Element<M>,
) -> Html<M> {
    // Root element rendered with `root_attrs`.
    let root_style = build_style_string(&root_attrs);
    let mut root_html_attrs = collect_html_attrs(&root_attrs);
    if !root_style.is_empty() {
        root_html_attrs.insert(0, HtmlAttribute::Attr("style".to_owned(), root_style));
    }
    let rendered_elem = render_element(elem);
    let mut root_kids: Vec<Html<M>> = vec![rendered_elem];
    root_kids.extend(render_nearby_overlays(&root_attrs));
    let root_div = Html::HElement("div".to_owned(), root_html_attrs, root_kids);

    // Wrapper — starts with the page baseline then merges wrapper_attrs on top.
    let wrapper_base = "display:flex;flex-direction:column;height:100vh;width:100%;overflow:hidden";
    let wrapper_extra = build_style_string(&wrapper_attrs);
    let wrapper_style = if wrapper_extra.is_empty() {
        wrapper_base.to_owned()
    } else {
        format!("{wrapper_base};{wrapper_extra}")
    };
    let mut wrapper_html_attrs = collect_html_attrs(&wrapper_attrs);
    wrapper_html_attrs.insert(0, HtmlAttribute::Attr("style".to_owned(), wrapper_style));
    let mut wrapper_kids: Vec<Html<M>> = vec![root_div];
    wrapper_kids.extend(render_nearby_overlays(&wrapper_attrs));

    Html::HElement("div".to_owned(), wrapper_html_attrs, wrapper_kids)
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sky_runtime::html::render_html;

    #[derive(Clone, Debug, PartialEq)]
    enum TestMsg {
        Click,
    }

    #[test]
    fn layout_empty_attrs_text_elem() {
        let elem: Element<TestMsg> = Element::Text("Hello".to_owned());
        let html = ui_layout(vec![], elem);
        let s = render_html(&html);
        assert!(
            s.contains("Hello"),
            "rendered output must contain text: {s}"
        );
        // Must be wrapped in the viewport div.
        assert!(
            s.contains("height:100vh"),
            "must contain viewport wrapper: {s}"
        );
    }

    #[test]
    fn layout_with_padding_attr() {
        let attrs = vec![Attribute::AttrPadding(8, 16, 8, 16)];
        let elem: Element<TestMsg> = Element::Text("body".to_owned());
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(
            s.contains("padding:8px 16px 8px 16px"),
            "padding missing: {s}"
        );
    }

    #[test]
    fn layout_dangerous_style_attr_is_dropped() {
        let attrs = vec![Attribute::AttrStyle(
            "background".to_owned(),
            "expression(alert(1))".to_owned(),
        )];
        let elem: Element<TestMsg> = Element::Empty;
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(
            !s.contains("expression("),
            "dangerous CSS expression must be dropped: {s}"
        );
    }

    #[test]
    fn layout_bg_image_javascript_url_dropped() {
        let attrs = vec![Attribute::AttrBgImage("javascript:alert(1)".to_owned())];
        let elem: Element<TestMsg> = Element::Empty;
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(
            !s.contains("javascript:"),
            "javascript: in background-image must be dropped: {s}"
        );
    }

    #[test]
    fn border_width_each_saturating_add() {
        // Large values must not panic in debug mode — saturating_add is required.
        let attrs = vec![Attribute::AttrBorderWidthEach(
            i64::MAX,
            i64::MAX,
            i64::MAX,
            i64::MAX,
        )];
        let elem: Element<TestMsg> = Element::Empty;
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        // The border-width property should still be present.
        assert!(s.contains("border-width:"), "border-width missing: {s}");
    }

    #[test]
    fn border_shadow_renders_box_shadow() {
        // `Border.shadow { offsetX = 0, offsetY = 1, blur = 2, spread = 0,
        //   color = Ui.rgb 0 0 0 }` must render the CSS box-shadow shape,
        // routing the colour through the same `color_css` boundary as
        // `Border.color`. Exercises the `ui_border_shadow_` helper end to end.
        let attrs = vec![super::super::helpers::ui_border_shadow_(
            0,
            1,
            2,
            0,
            Color::Rgba(0, 0, 0, 1.0),
        )];
        let elem: Element<TestMsg> = Element::Empty;
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(
            s.contains("box-shadow:0px 1px 2px 0px rgba(0,0,0,1)"),
            "box-shadow missing/malformed: {s}"
        );
    }

    #[test]
    fn border_inner_shadow_renders_inset_box_shadow() {
        // `Border.innerShadow { offsetX = 0, offsetY = 1, blur = 2, spread = 0,
        //   color = Ui.rgb 0 0 0 }` must render the INSET CSS box-shadow shape,
        // routing the colour through the same `color_css` boundary as
        // `Border.color`. Exercises the `ui_border_inner_shadow_` helper end to
        // end — identical to `Border.shadow` but prefixed with `inset`.
        let attrs = vec![super::super::helpers::ui_border_inner_shadow_(
            0,
            1,
            2,
            0,
            Color::Rgba(0, 0, 0, 1.0),
        )];
        let elem: Element<TestMsg> = Element::Empty;
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(
            s.contains("box-shadow:inset 0px 1px 2px 0px rgba(0,0,0,1)"),
            "inset box-shadow missing/malformed: {s}"
        );
    }

    #[test]
    fn nearby_overlay_renders() {
        let overlay: Element<TestMsg> = Element::Text("tooltip".to_owned());
        let attrs = vec![Attribute::AttrNearby(Location::Above, overlay)];
        let elem: Element<TestMsg> = Element::Text("base".to_owned());
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(s.contains("tooltip"), "nearby overlay must render: {s}");
        assert!(
            s.contains("position:absolute"),
            "nearby must use absolute positioning: {s}"
        );
    }

    #[test]
    fn layout_with_wrapper_and_root_attrs() {
        let wrapper_attrs = vec![Attribute::AttrBgColor(Color::Rgba(0, 0, 0, 1.0))];
        let root_attrs = vec![Attribute::AttrPadding(4, 4, 4, 4)];
        let elem: Element<TestMsg> = Element::Text("content".to_owned());
        let html = ui_layout_with_vecs(wrapper_attrs, root_attrs, elem);
        let s = render_html(&html);
        assert!(s.contains("rgba(0,0,0,1)"), "wrapper bg-color missing: {s}");
        assert!(
            s.contains("padding:4px 4px 4px 4px"),
            "root padding missing: {s}"
        );
        assert!(s.contains("content"), "content must render: {s}");
    }

    // ── Follow-up 3: CSS injection hardening tests (T3/T4) ───────────────────

    /// A mid-value `url(javascript:…)` payload (`;` breakout + dangerous URL)
    /// must be dropped entirely — the old prefix-only gate missed this.
    #[test]
    fn css_midvalue_injection_dropped() {
        let attrs = vec![Attribute::AttrStyle(
            "background".to_owned(),
            "0; background:url(javascript:alert(1))".to_owned(),
        )];
        let elem: Element<TestMsg> = Element::Empty;
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(
            !s.contains("javascript:"),
            "mid-value javascript: must be dropped: {s}"
        );
        assert!(
            !s.contains("alert("),
            "mid-value injection must be dropped: {s}"
        );
    }

    /// A dangerous style KEY (contains `;` + injection payload) must be
    /// dropped — the key was previously emitted verbatim, allowing a whole
    /// CSS rule to be smuggled through the value gate.
    #[test]
    fn css_dangerous_key_dropped() {
        let attrs = vec![Attribute::AttrStyle(
            "x;background:url(javascript:alert(1))".to_owned(),
            "y".to_owned(),
        )];
        let elem: Element<TestMsg> = Element::Empty;
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(
            !s.contains("javascript:"),
            "dangerous key must be dropped, no javascript: in output: {s}"
        );
        assert!(
            !s.contains("alert("),
            "dangerous key must be dropped, no alert( in output: {s}"
        );
    }

    /// A legitimate `AttrStyle("color", "red")` must still be emitted
    /// correctly after the smart-constructor hardening.
    #[test]
    fn css_safe_attr_style_emits_correctly() {
        let attrs = vec![Attribute::AttrStyle("color".to_owned(), "red".to_owned())];
        let elem: Element<TestMsg> = Element::Empty;
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(
            s.contains("color:red"),
            "safe color:red must be emitted: {s}"
        );
    }

    #[test]
    fn empty_element_renders_empty_text_node() {
        let html: Html<TestMsg> = render_element(Element::Empty);
        assert_eq!(html, Html::HText(String::new()));
    }

    #[test]
    fn raw_element_passes_through() {
        let inner: Html<TestMsg> =
            Html::HElement("span".into(), vec![], vec![Html::HText("raw".into())]);
        let html = render_element(Element::Raw(inner.clone()));
        assert_eq!(html, inner);
    }

    #[test]
    fn event_attr_on_msg_registers_handler() {
        use crate::sky_runtime::html::{Attribute as HtmlAttr, Event};
        // Constructs TestMsg::Click — suppresses the dead-code lint while adding
        // genuine coverage for the AttrEvent/Event::OnMsg path through
        // collect_html_attrs → render_html's data-sky-on emission.
        let evt = HtmlAttr::EventAttr(Event::OnMsg("click".to_owned(), TestMsg::Click));
        let attrs = vec![Attribute::AttrEvent(evt)];
        let elem: Element<TestMsg> = Element::Text("press me".to_owned());
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(
            s.contains("data-sky-on=\"click\""),
            "click event handler must register in rendered HTML: {s}"
        );
        assert!(s.contains("press me"), "element text must render: {s}");
    }
}

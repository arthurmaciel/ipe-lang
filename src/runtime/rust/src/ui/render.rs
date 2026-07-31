//! `Ipe.Ui` → `Html<M>` render kernel.
//!
//! This module is the ONLY place that converts a `Ipe.Ui` `Element<M>` tree to
//! `ipe_runtime::html::Html<M>`.  It is a runtime kernel (not compiled from Ipê)
//! because the render chain touches `any`-returning stdlib fields (`Raw any`,
//! `AttrEvent any`) that cannot be typed soundly in Ipê-over-Rust (spec §1.4).
//!
//! Security note — this file is T1/T3/T5-critical (spec §6):
//! - T1: never call `renderElement` from Ipê; keep it here as a typed Rust fn.
//! - T3: `AttrStyle`, `AttrBgImage`, `AttrAttribute` carry user-controlled strings
//!   entering `style="…"` / HTML-attribute sinks.  The CSS URL sanitiser
//!   (`sanitise_css_url`) gates `url(…)` payloads; HTML values pass through
//!   `html::render_html`'s existing `SafeAttrName` + `sanitise_url_attr` gates.
//! - T5: `AttrBorderWidthEach(t,r,b,l)` uses `saturating_add` throughout.
//!
//! ### Design rationale
//! `Ui.layout` emits an outer 100 vh flex-column wrapper (matching Ipê's Go
//! runtime `runtime-go/rt/ui.go`) and the converted root element inside it.
//! `Ui.layoutWith` additionally applies `wrapperAttrs` to the outer wrapper and
//! `rootAttrs` to an intermediate flex root, mirroring `Ui.layoutWith`'s Go shape.

use super::super::css_safety::{SafeCssPropertyName, SafeCssValue};
use super::super::html::{Attribute as HtmlAttribute, Html};
use super::element::{Attribute, Color, Description, Element, HAlign, Length, Location, VAlign};

// ── CSS boundary smart constructors ───────────────────────────────────────────
// `SafeCssPropertyName` / `SafeCssValue` moved to the shared `css_safety` module
// (design §Q5: one policy, one place). Imported above so the Ipe.Ui inline-style
// path and the Ipe.Css / styleNode sinks share the identical encoder.

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
///
/// `pub(crate)` so `ui::helpers::ui_on_pseudo_` can reuse the identical
/// style-collection logic to build a pseudo-class rules-string (mirrors the
/// `../ipe` reference's `onPseudo pc attrs = AttrPseudoRule pc
/// (mediaQueryRulesCss attrs)`, which folds attrs through the SAME collector
/// used for the main `style=""` attribute — one collector, two call sites).
#[allow(clippy::too_many_lines)]
pub(crate) fn build_style_string<M>(attrs: &[Attribute<M>]) -> String {
    // Single shared buffer with a running `;` separator — replaces the former
    // per-declaration `Vec<String>` + `join(";")` (efficiency-audit §6 medium:
    // one String per CSS declaration + a join copy). Byte-identical output:
    // the first declaration is unprefixed and every later one prepends `;`,
    // exactly what `join(";")` produced. CSS security gates
    // (`SafeCssPropertyName`/`SafeCssValue`, dangerous-URL, saturating_add)
    // are untouched.
    use std::fmt::Write as _;
    let mut parts = String::new();
    macro_rules! decl {
        ($($arg:tt)*) => {{
            if !parts.is_empty() {
                parts.push(';');
            }
            let _ = write!(parts, $($arg)*);
        }};
    }

    for attr in attrs {
        match attr {
            Attribute::AttrWidth(len) => {
                decl!("width:{}", length_css(len));
                if let Length::Fill(n) = len {
                    decl!("flex-grow:{n}");
                    decl!("min-width:0");
                }
            }
            Attribute::AttrHeight(len) => {
                decl!("height:{}", length_css(len));
                if let Length::Fill(n) = len {
                    decl!("flex-grow:{n}");
                    decl!("min-height:0");
                }
            }
            Attribute::AttrAlignX(h) => {
                let v = match h {
                    HAlign::AlignLeft => "flex-start",
                    HAlign::CenterX => "center",
                    HAlign::AlignRight => "flex-end",
                };
                decl!("align-self:{v}");
            }
            Attribute::AttrAlignY(v) => {
                let css = match v {
                    VAlign::AlignTop => "flex-start",
                    VAlign::CenterY => "center",
                    VAlign::AlignBottom => "flex-end",
                };
                decl!("align-self:{css}");
            }
            Attribute::AttrPadding(t, r, b, l) => {
                decl!("padding:{t}px {r}px {b}px {l}px");
            }
            Attribute::AttrSpacing(n) => {
                decl!("gap:{n}px");
            }
            Attribute::AttrStyle(k, v) => {
                // Internal direction markers injected by `ui_row_` / `ui_column_` /
                // `ui_wrapped_row_` in helpers.rs.  They carry layout semantics but
                // must NOT be emitted as literal CSS `__col:true` / `__row:true`.
                // Instead: map to the corresponding Flexbox CSS.
                match k.as_str() {
                    "__col" => {
                        decl!("display:flex");
                        decl!("flex-direction:column");
                    }
                    "__row" => {
                        decl!("display:flex");
                        decl!("flex-direction:row");
                    }
                    "__wrappedrow" => {
                        decl!("display:flex");
                        decl!("flex-direction:row");
                        decl!("flex-wrap:wrap");
                    }
                    "__grid" => {
                        decl!("display:grid");
                    }
                    "__paragraph" => {
                        // A `Ui.paragraph` `<p>`: its element children flow as
                        // inline runs, so the block itself needs no flex/grid —
                        // but an explicit `display:block` keeps it a block box
                        // even when nested inside another inline-block context.
                        decl!("display:block");
                    }
                    "__inline" => {
                        // Injected by `render_node_as` onto a `Ui.el` child of a
                        // paragraph so its run flows inline with the surrounding
                        // text instead of breaking onto its own line.
                        decl!("display:inline-block");
                        decl!("vertical-align:baseline");
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
                            decl!("{}:{}", pk.as_str(), pv.as_str());
                        }
                        // else: silently drop — consistent with the
                        // `is_dangerous_url_scheme` path in `AttrBgImage`.
                    }
                }
            }
            Attribute::AttrFontSize(n) => {
                decl!("font-size:{n}px");
            }
            Attribute::AttrFontColor(c) => {
                decl!("color:{}", color_css(c));
            }
            Attribute::AttrFontFamily(f) => {
                // Value-as-data gate (UI CSS-escaping hardening): a raw Ipê
                // `String` reaching a CSS sink must pass the shared
                // `SafeCssValue` breakout scan (`;{}@import`, script sinks) —
                // drop on failure, same posture as `AttrStyle` /
                // `AttrBgGradient` below. Legit font stacks (commas, quotes,
                // spaces) pass untouched. See
                // `docs/adr/0005-ui-html-render-invariants.md §2`.
                if let Some(v) = SafeCssValue::parse(f) {
                    decl!("font-family:{}", v.as_str());
                }
            }
            Attribute::AttrFontWeight(w) => {
                decl!("font-weight:{w}");
            }
            Attribute::AttrFontItalic => {
                decl!("font-style:italic");
            }
            Attribute::AttrFontUnderline => {
                decl!("text-decoration:underline");
            }
            Attribute::AttrFontDecoration(d) => {
                if let Some(v) = SafeCssValue::parse(d) {
                    decl!("text-decoration:{}", v.as_str());
                }
            }
            Attribute::AttrFontLetterSpacing(n) => {
                decl!("letter-spacing:{n}px");
            }
            Attribute::AttrFontWordSpacing(n) => {
                decl!("word-spacing:{n}px");
            }
            Attribute::AttrFontAlign(a) => {
                if let Some(v) = SafeCssValue::parse(a) {
                    decl!("text-align:{}", v.as_str());
                }
            }
            Attribute::AttrBgColor(c) => {
                decl!("background-color:{}", color_css(c));
            }
            Attribute::AttrBgImage(url) => {
                // T3: check the raw URL scheme before wrapping in url().
                // `sanitise_css_value` only fires on `url(` or `expression(` prefixes,
                // so we need the is_dangerous_url_scheme check here on the bare URL.
                if !is_dangerous_url_scheme(url) {
                    // BG-1 (spec §4.3): gate the COMPOSED `url({url})` value
                    // so a `)` closing `url(` early followed by `}`/`;`/
                    // `@import` is rejected by the shared breakout scan.
                    // Known, documented limitation: an inline base64 data
                    // URI (`url(data:image/png;base64,…)`) contains `;` and
                    // is dropped — Background.image takes a path/URL;
                    // data-URI backgrounds are unsupported through Ipe.Ui
                    // (BG-2 quoting is the upgrade if ever needed).
                    let composed = format!("url({url})");
                    if let Some(v) = SafeCssValue::parse(&composed) {
                        decl!("background-image:{}", v.as_str());
                    }
                }
            }
            Attribute::AttrBgGradient(g) => {
                // Gradient CSS value — whole-string scan via SafeCssValue (T3).
                // Replaces the former prefix-only `sanitise_css_value` call,
                // closing the mid-value `expression()` / `url(javascript:…)`
                // bypass.
                if let Some(sv) = SafeCssValue::parse(g) {
                    decl!("background-image:{}", sv.as_str());
                }
            }
            Attribute::AttrBorderWidth(n) => {
                decl!("border-width:{n}px");
            }
            Attribute::AttrBorderWidthEach(t, r, b, l) => {
                // T5: use saturating_add to avoid debug-mode overflow panics.
                let _total = (*t)
                    .saturating_add(*r)
                    .saturating_add(*b)
                    .saturating_add(*l);
                decl!("border-width:{t}px {r}px {b}px {l}px");
            }
            Attribute::AttrBorderColor(c) => {
                decl!("border-color:{}", color_css(c));
            }
            Attribute::AttrBorderRounded(n) => {
                decl!("border-radius:{n}px");
            }
            Attribute::AttrBorderStyle(s) => {
                if let Some(v) = SafeCssValue::parse(s) {
                    decl!("border-style:{}", v.as_str());
                }
            }
            Attribute::AttrBorderShadow(x, y, blur, spread, c) => {
                decl!(
                    "box-shadow:{x}px {y}px {blur}px {spread}px {}",
                    color_css(c)
                );
            }
            Attribute::AttrBorderInsetShadow(x, y, blur, spread, c) => {
                decl!(
                    "box-shadow:inset {x}px {y}px {blur}px {spread}px {}",
                    color_css(c)
                );
            }
            Attribute::AttrPointer => {
                decl!("cursor:pointer");
            }
            Attribute::AttrOverflow(x, y) => {
                // Per-component gating: one bad axis drops alone, the other
                // legit axis still renders.
                if let Some(v) = SafeCssValue::parse(x) {
                    decl!("overflow-x:{}", v.as_str());
                }
                if let Some(v) = SafeCssValue::parse(y) {
                    decl!("overflow-y:{}", v.as_str());
                }
            }
            Attribute::AttrTransition(t, _respect_reduced) => {
                if let Some(v) = SafeCssValue::parse(t) {
                    decl!("transition:{}", v.as_str());
                }
            }
            Attribute::AttrGridTracks(cols, rows) => {
                // Fixed property names; user-supplied values go through SafeCssValue.
                if !cols.is_empty()
                    && let Some(pv) = SafeCssValue::parse(cols)
                {
                    decl!("grid-template-columns:{}", pv.as_str());
                }
                if !rows.is_empty()
                    && let Some(pv) = SafeCssValue::parse(rows)
                {
                    decl!("grid-template-rows:{}", pv.as_str());
                }
            }
            Attribute::AttrAnimation(name, spec, keyframes, _respect) => {
                // Emit animation name + spec; keyframes require a <style> block
                // which this render doesn't inject (no DOM).  The `name` +
                // `spec` provide the `animation:` property; keyframes are
                // dropped here.
                let _ = keyframes; // suppress unused-variable warning
                // Gate the composed `name spec` shorthand as ONE value.
                let shorthand = format!("{name} {spec}");
                if let Some(v) = SafeCssValue::parse(&shorthand) {
                    decl!("animation:{}", v.as_str());
                }
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

    parts
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
    // `AttrPseudoRule` entries (from `Ui.onPseudo` and the `hoverColor` /
    // `focusColor` / `activeColor` / `disabledColor` sub-module helpers that
    // build on it) are collected into ONE `data-ipe-pc-rules` marker attr,
    // wire-format `"<tag>|<css>||<tag2>|<css2>"` — consumed by
    // `ipe_runtime::live::style_inject::build_pc` (called post-`assign_ipe_ids`
    // from the Ipe.Web / Ipe.WebView render pipelines), which expands it into
    // a ipe-id-scoped `<style>` block. Multiple entries with the SAME tag are
    // NOT merged (each keeps its own `tag|css` segment) — matches the `../ipe`
    // reference's `injectPseudoClassStyles` wire contract.
    let mut pseudo_rules: Vec<String> = Vec::new();
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
            Attribute::AttrPseudoRule(pc, css) if !css.is_empty() => {
                pseudo_rules.push(format!("{}|{css}", pc.wire_tag()));
            }
            // Style and nearby handled separately.
            _ => {}
        }
    }
    if !pseudo_rules.is_empty() {
        out.push(HtmlAttribute::Attr(
            "data-ipe-pc-rules".to_owned(),
            pseudo_rules.join("||"),
        ));
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
/// explicit user-supplied tag (already validated by the Ipê stdlib).
fn tag_for_description(desc: &Description) -> &'static str {
    match desc {
        Description::NoDescription
        | Description::DescLivePolite
        | Description::DescLiveAssertive => "div",
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
        Description::DescButton => "button",
        Description::DescParagraph => "p",
    }
}

// ── Element → Html (recursive) ───────────────────────────────────────────────

/// Depth-0 entry point. All callers outside this module use this wrapper.
fn render_element<M: Clone>(elem: Element<M>) -> Html<M> {
    render_element_depth(elem, 0)
}

/// Recursively convert a `Ipe.Ui` `Element<M>` to `Html<M>`.
///
/// Security: all attribute values flow through `build_style_string` (which
/// calls `sanitise_css_value`) or `collect_html_attrs` (which passes values to
/// `html::Attribute::Attr` where `render_html` applies `SafeAttrName` +
/// `sanitise_url_attr`).  No value reaches the HTML sink without one of these gates.
///
/// Bounded descent: at `MAX_HTML_DEPTH` the subtree is dropped (empty text
/// node) rather than recursed into — a truncated render is strictly better than
/// overflowing the thread stack. Same ceiling as `html.rs::render_into_ctx` and
/// `html.rs::assign_ipe_ids_depth`.
fn render_element_depth<M: Clone>(elem: Element<M>, depth: usize) -> Html<M> {
    if depth >= crate::html::MAX_HTML_DEPTH {
        return Html::HText(String::new());
    }
    match elem {
        Element::Empty => Html::HText(String::new()),
        Element::Text(s) => Html::HText(s),
        Element::Raw(html) => html,
        // `Ui.cells` is terminal-only and is rejected before reaching the Web
        // backend (see the shape admissibility gate). This total fallback keeps
        // the match exhaustive: degrade the grid to its text rows.
        Element::Cells(grid) => {
            let text = grid
                .into_iter()
                .map(|row| row.into_iter().collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            Html::HText(text)
        }
        Element::Node(desc, attrs, kids) => {
            render_node_as(tag_for_description(&desc), &attrs, kids, depth)
        }
        Element::TaggedNode(tag, _desc, attrs, kids) => render_node_as(&tag, &attrs, kids, depth),
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
/// True when a node carries the `__paragraph` marker (`Ui.paragraph`), meaning
/// its element children must flow inline rather than as block boxes.
fn has_paragraph_marker<M>(attrs: &[Attribute<M>]) -> bool {
    attrs
        .iter()
        .any(|a| matches!(a, Attribute::AttrStyle(k, _) if k == "__paragraph"))
}

/// Render one child of a `Ui.paragraph`. A bare `Ui.el` child
/// (`Element::Node(NoDescription, …)`) becomes an inline `<span>` carrying the
/// `__inline` marker, so its styled run (e.g. `Font.bold`) flows inline with the
/// surrounding text and produces valid `<p>`-nestable markup. Every other child
/// (text, `Ui.link`/`Ui.el`-with-explicit-tag, raw HTML) renders unchanged.
fn render_paragraph_child<M: Clone>(child: Element<M>, depth: usize) -> Html<M> {
    match child {
        Element::Node(Description::NoDescription, mut attrs, kids) => {
            attrs.insert(
                0,
                Attribute::AttrStyle("__inline".to_owned(), "true".to_owned()),
            );
            render_node_as("span", &attrs, kids, depth)
        }
        other => render_element_depth(other, depth),
    }
}

fn render_node_as<M: Clone>(
    tag: &str,
    attrs: &[Attribute<M>],
    kids: Vec<Element<M>>,
    depth: usize,
) -> Html<M> {
    let style_str = build_style_string(attrs);
    let mut html_attrs = collect_html_attrs(attrs);

    if !style_str.is_empty() {
        // Prepend — style first so tests can pattern-match on it predictably.
        html_attrs.insert(0, HtmlAttribute::Attr("style".to_owned(), style_str));
    }

    // A `Ui.paragraph` node's element children must flow inline: a bare
    // `Ui.el` lowers to `Element::Node(NoDescription, …)` (a block `<div>`),
    // which both breaks onto its own line and — as a `<div>` inside a `<p>` —
    // is invalid HTML5 that a browser auto-closes the `<p>` around. Inside a
    // paragraph, render each such child as an inline `<span>`.
    let inside_paragraph = has_paragraph_marker(attrs);

    // Rendered children in source order, each one level deeper.
    let child_depth = depth.saturating_add(1);
    let mut html_kids: Vec<Html<M>> = kids
        .into_iter()
        .map(|k| {
            if inside_paragraph {
                render_paragraph_child(k, child_depth)
            } else {
                render_element_depth(k, child_depth)
            }
        })
        .collect();

    // Nearby overlays appended after the regular children (they are absolutely
    // positioned, so their DOM order is irrelevant for layout).
    html_kids.extend(render_nearby_overlays(attrs));

    Html::HElement(tag.to_owned(), html_attrs, html_kids)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// `Ui.layout : List (Attribute msg) -> Element msg -> Html msg`
///
/// Wraps the element in a full-viewport flex-column page wrapper, then renders
/// the root element with the given root attributes applied. Mirrors the Ipê Go
/// runtime's `layout` output shape.
#[must_use]
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
/// `ipe_runtime::ui::render::ui_layout_with_vecs::<M>(wrapper, root, elem)`.
/// The two `Vec<Attribute<M>>` arguments are extracted at the **emit site**
/// (field-extraction on the IR `Expr::Record` literal), so the cfg record struct
/// never needs to be materialised — closing IPE-I0001.
///
/// # Design note (MAKE INVALID STATES UNREPRESENTABLE)
///
/// There is deliberately no `ui_layout_with<M: Clone, C>(_cfg: C, elem)` shape:
/// a fn accepting any `C` and silently dropping it would produce wrong HTML
/// (exit-0-cargo-ok-but-wrong-output). The cfg's `wrapper_attrs`/`root_attrs`
/// are extracted at the emit site and passed here explicitly.
#[must_use]
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
    use crate::html::render_html;

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
    fn border_glow_renders_box_shadow() {
        // `Border.glow 4 (Ui.rgb 0 0 0)` is a convenience box-shadow with `(0, 0)`
        // offset and `0` spread — only blur + colour vary. It must render the CSS
        // `box-shadow: 0px 0px <blur>px 0px <colour>` shape via the generic
        // `AttrStyle` boundary, routing the colour through the same conversion as
        // `Border.color`. Exercises the `ui_border_glow_` helper end to end.
        let attrs = vec![super::super::helpers::ui_border_glow_(
            4,
            Color::Rgba(0, 0, 0, 1.0),
        )];
        let elem: Element<TestMsg> = Element::Empty;
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(
            s.contains("box-shadow:0px 0px 4px 0px rgba(0,0,0,1)"),
            "box-shadow (glow) missing/malformed: {s}"
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
    /// dropped — an unchecked key emitted verbatim would let a whole CSS rule
    /// be smuggled through the value gate.
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
        use crate::html::{Attribute as HtmlAttr, Event};
        // Constructs TestMsg::Click — suppresses the dead-code lint while adding
        // genuine coverage for the AttrEvent/Event::OnMsg path through
        // collect_html_attrs → render_html's data-ipe-on emission.
        let evt = HtmlAttr::EventAttr(Event::OnMsg("click".to_owned(), TestMsg::Click));
        let attrs = vec![Attribute::AttrEvent(evt)];
        let elem: Element<TestMsg> = Element::Text("press me".to_owned());
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(
            s.contains("data-ipe-on=\"click\""),
            "click event handler must register in rendered HTML: {s}"
        );
        assert!(s.contains("press me"), "element text must render: {s}");
    }

    // ── kernel-wiring regressions ───────────────────────────────

    #[test]
    fn ui_padding_each_renders_four_distinct_sides() {
        // `Ui.paddingEach { top = 1, right = 2, bottom = 3, left = 4 }` — each
        // side distinct proves the record fields are NOT swapped/aliased.
        let attrs = vec![super::super::helpers::ui_padding_each_::<TestMsg>(
            1, 2, 3, 4,
        )];
        let elem: Element<TestMsg> = Element::Empty;
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(
            s.contains("padding:1px 2px 3px 4px"),
            "paddingEach must render top/right/bottom/left in order: {s}"
        );
    }

    #[test]
    fn ui_clip_x_y_render_single_axis_clip_not_hidden() {
        // Go/Ipê reference: clipX = AttrOverflow "clip" "visible" (NOT "hidden"),
        // clipY = AttrOverflow "visible" "clip". Distinct from `Ui.clip` (which
        // uses "hidden" on both axes) — this is the exact semantics bug the
        // design doc's clip family warns about.
        let attrs_x = vec![super::super::helpers::ui_clip_x_::<TestMsg>()];
        let html_x = ui_layout(attrs_x, Element::Empty);
        let sx = render_html(&html_x);
        assert!(
            sx.contains("overflow-x:clip") && sx.contains("overflow-y:visible"),
            "clipX must be overflow-x:clip;overflow-y:visible (not hidden): {sx}"
        );

        let attrs_y = vec![super::super::helpers::ui_clip_y_::<TestMsg>()];
        let html_y = ui_layout(attrs_y, Element::Empty);
        let sy = render_html(&html_y);
        assert!(
            sy.contains("overflow-x:visible") && sy.contains("overflow-y:clip"),
            "clipY must be overflow-x:visible;overflow-y:clip: {sy}"
        );
    }

    #[test]
    fn ui_scrollbar_x_y_render_off_axis_hidden_not_visible() {
        // Go/Ipê reference: scrollbarX = AttrOverflow "auto" "hidden" (off-axis
        // hidden, NOT visible — a visible off-axis gets promoted to `auto` by
        // CSS, producing an unwanted second scrollbar).
        let attrs_x = vec![super::super::helpers::ui_scrollbar_x_::<TestMsg>()];
        let html_x = ui_layout(attrs_x, Element::Empty);
        let sx = render_html(&html_x);
        assert!(
            sx.contains("overflow-x:auto") && sx.contains("overflow-y:hidden"),
            "scrollbarX must be overflow-x:auto;overflow-y:hidden: {sx}"
        );

        let attrs_y = vec![super::super::helpers::ui_scrollbar_y_::<TestMsg>()];
        let html_y = ui_layout(attrs_y, Element::Empty);
        let sy = render_html(&html_y);
        assert!(
            sy.contains("overflow-x:hidden") && sy.contains("overflow-y:auto"),
            "scrollbarY must be overflow-x:hidden;overflow-y:auto: {sy}"
        );
    }

    #[test]
    fn ui_image_renders_img_src_alt_void() {
        let elem = super::super::helpers::ui_image_::<TestMsg>(
            vec![],
            "https://example.com/x.png".to_owned(),
            "a cat".to_owned(),
        );
        let html = ui_layout(vec![], elem);
        let s = render_html(&html);
        assert!(s.contains("<img"), "must render <img>: {s}");
        assert!(
            s.contains("src=\"https://example.com/x.png\""),
            "src attr missing: {s}"
        );
        assert!(s.contains("alt=\"a cat\""), "alt attr missing: {s}");
        assert!(
            !s.contains("</img>"),
            "img is a void element — no closing tag: {s}"
        );
    }

    #[test]
    fn background_linear_gradient_renders_css_gradient() {
        let attrs = vec![super::super::helpers::ui_background_linear_gradient_::<
            TestMsg,
        >(
            90.0,
            vec![
                (0.0, Color::Rgba(255, 0, 0, 1.0)),
                (100.0, Color::Rgba(0, 0, 255, 1.0)),
            ],
        )];
        let html = ui_layout(attrs, Element::Empty);
        let s = render_html(&html);
        assert!(
            s.contains(
                "background-image:linear-gradient(90deg, rgba(255,0,0,1) 0%, rgba(0,0,255,1) 100%)"
            ),
            "linear-gradient CSS malformed: {s}"
        );
    }

    #[test]
    fn ui_on_pseudo_emits_data_ipe_pc_rules_marker() {
        // `Ui.onPseudo Ui.hover [Background.color red]` must attach a
        // `data-ipe-pc-rules="h|background-color:rgba(255,0,0,1)"` marker —
        // the wire format `ipe_runtime::live::style_inject::build_pc` decodes
        // into a ipe-id-scoped `<style>` block post-`assign_ipe_ids`.
        let inner = vec![Attribute::AttrBgColor(Color::Rgba(255, 0, 0, 1.0))];
        let pseudo_attr =
            super::super::helpers::ui_on_pseudo_(super::super::helpers::ui_hover_(), inner);
        let attrs = vec![pseudo_attr];
        let elem: Element<TestMsg> = Element::Text("hi".to_owned());
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(
            s.contains("data-ipe-pc-rules=\"h|background-color:rgba(255,0,0,1)\""),
            "onPseudo(hover) marker missing/malformed: {s}"
        );
    }

    #[test]
    fn ui_media_query_emits_wrapper_with_mq_markers() {
        // `Ui.mediaQuery "(min-width: 768px)" [Background.color …] child` must
        // wrap the child in a <div> carrying data-ipe-mq-q (the verbatim
        // query) + data-ipe-mq-rules (the attrs folded through
        // build_style_string) — the wire pair
        // `ipe_runtime::live::style_inject::build_mq` decodes into a
        // ipe-id-scoped <style> block post-`assign_ipe_ids`.
        let elem = super::super::helpers::ui_media_query_::<TestMsg>(
            "(min-width: 768px)".to_owned(),
            vec![Attribute::AttrBgColor(Color::Rgba(18, 18, 24, 1.0))],
            Element::Text("responsive".to_owned()),
        );
        let html = ui_layout(vec![], elem);
        let s = render_html(&html);
        assert!(
            s.contains("data-ipe-mq-q=\"(min-width: 768px)\""),
            "mediaQuery query marker missing/malformed: {s}"
        );
        assert!(
            s.contains("data-ipe-mq-rules=\"background-color:rgba(18,18,24,1)\""),
            "mediaQuery rules marker missing/malformed: {s}"
        );
        assert!(s.contains("responsive"), "child must render: {s}");
    }

    #[test]
    fn ui_breakpoint_delegates_to_media_query_markers() {
        // `Ui.breakpoint Ui.mobile [...] child` is upstream-defined as
        // `mediaQuery (breakpointToQuery bp) ...`; with Breakpoint = String
        // in this port the delegation must emit the same marker pair, not a
        // passthrough that drops the query.
        let elem = super::super::helpers::ui_breakpoint_::<TestMsg>(
            super::super::helpers::ui_mobile_(),
            vec![Attribute::AttrBgColor(Color::Rgba(1, 2, 3, 1.0))],
            Element::Text("m".to_owned()),
        );
        let html = ui_layout(vec![], elem);
        let s = render_html(&html);
        assert!(
            s.contains("data-ipe-mq-q=\"(max-width: 767px)\""),
            "breakpoint must emit the Ui.mobile media-query marker: {s}"
        );
        assert!(
            s.contains("data-ipe-mq-rules=\"background-color:rgba(1,2,3,1)\""),
            "breakpoint rules marker missing: {s}"
        );
    }

    /// SECURITY: a breakout media-query string must be neutralised at the
    /// producer — the `SafeCssMediaQuery` gate drops BOTH markers (fail-closed:
    /// no styling), while the wrapper + child still render.
    #[test]
    fn ui_media_query_breakout_query_drops_markers_fail_closed() {
        let elem = super::super::helpers::ui_media_query_::<TestMsg>(
            "(min-width: 1px) { } </style><script>alert(1)</script> @import url(evil)".to_owned(),
            vec![Attribute::AttrBgColor(Color::Rgba(18, 18, 24, 1.0))],
            Element::Text("safe".to_owned()),
        );
        let html = ui_layout(vec![], elem);
        let s = render_html(&html);
        assert!(
            !s.contains("data-ipe-mq-q") && !s.contains("data-ipe-mq-rules"),
            "breakout query must drop BOTH markers: {s}"
        );
        assert!(!s.contains("<script"), "script must never render: {s}");
        assert!(s.contains("safe"), "child must still render: {s}");
    }

    // ── UI CSS-escaping hardening (value-as-data attrs; spec §6.1) ─────────

    /// Every previously-ungated raw-string arm must DROP a value carrying the
    /// rule-breakout set (`}` / `;` / `@import`) — the page-wide-injection
    /// primitive once `Ui.onPseudo` routes the collector output into a
    /// `<style>` block (Repro A of the spec).
    #[test]
    fn value_as_data_attrs_drop_rule_breakout_payloads() {
        let cases: Vec<Attribute<TestMsg>> = vec![
            Attribute::AttrFontFamily("serif } body { display:none".to_owned()),
            Attribute::AttrFontFamily("x;color:red".to_owned()),
            Attribute::AttrFontDecoration("underline } .x{}".to_owned()),
            Attribute::AttrFontAlign("center;position:fixed".to_owned()),
            Attribute::AttrBorderStyle("solid } .x{color:red".to_owned()),
            Attribute::AttrTransition("all 1s } body{}".to_owned(), true),
            Attribute::AttrAnimation(
                "a } body {".to_owned(),
                "300ms".to_owned(),
                String::new(),
                true,
            ),
            Attribute::AttrBgImage("x) } @import url(evil)".to_owned()),
        ];
        for attr in cases {
            let css = build_style_string(std::slice::from_ref(&attr));
            assert!(
                !css.contains('}') && !css.contains("display:none") && !css.contains("@import"),
                "breakout payload must be dropped, got {css:?} for {attr:?}"
            );
        }
    }

    /// Per-component gating: one poisoned overflow axis drops alone; the
    /// sibling legit axis still renders.
    #[test]
    fn overflow_gates_each_axis_independently() {
        let attr: Attribute<TestMsg> =
            Attribute::AttrOverflow("auto }".to_owned(), "hidden".to_owned());
        let css = build_style_string(std::slice::from_ref(&attr));
        assert!(
            !css.contains("overflow-x"),
            "poisoned axis must drop: {css}"
        );
        assert!(
            css.contains("overflow-y:hidden"),
            "legit axis must stay: {css}"
        );
    }

    /// Legitimate values must render byte-for-byte — the gate's charset is
    /// permissive for everything a real single-declaration value contains
    /// (commas, quotes, spaces); zero legitimate loss.
    #[test]
    fn value_as_data_attrs_keep_legitimate_values() {
        let cases: Vec<(Attribute<TestMsg>, &str)> = vec![
            (
                Attribute::AttrFontFamily("\"Helvetica Neue\", Georgia, serif".to_owned()),
                "font-family:\"Helvetica Neue\", Georgia, serif",
            ),
            (
                Attribute::AttrTransition("all 200ms ease-in-out".to_owned(), true),
                "transition:all 200ms ease-in-out",
            ),
            (
                Attribute::AttrFontAlign("center".to_owned()),
                "text-align:center",
            ),
            (
                Attribute::AttrBorderStyle("dashed".to_owned()),
                "border-style:dashed",
            ),
            (
                Attribute::AttrFontDecoration("underline".to_owned()),
                "text-decoration:underline",
            ),
            (
                Attribute::AttrAnimation(
                    "fadeIn".to_owned(),
                    "300ms ease".to_owned(),
                    String::new(),
                    true,
                ),
                "animation:fadeIn 300ms ease",
            ),
            (
                Attribute::AttrOverflow("auto".to_owned(), "scroll".to_owned()),
                "overflow-x:auto;overflow-y:scroll",
            ),
            (
                Attribute::AttrBgImage("hero.png".to_owned()),
                "background-image:url(hero.png)",
            ),
        ];
        for (attr, want) in cases {
            let css = build_style_string(std::slice::from_ref(&attr));
            assert_eq!(css, want, "legit value must render verbatim for {attr:?}");
        }
    }

    #[test]
    fn multiple_pseudo_rules_merge_into_one_marker() {
        // #113 spec §1.4: two pseudo-class sugars on ONE element must merge
        // into a single `data-ipe-pc-rules` marker with `||`-joined entries.
        // NB: `Border.focusColor` maps to `PseudoClass::FocusVisible` (wire
        // tag "v"), not `Focus` ("f") — see `ui_border_focus_color_`.
        let attrs: Vec<Attribute<TestMsg>> = vec![
            super::super::helpers::ui_bg_hover_color_(Color::Rgba(255, 0, 0, 1.0)),
            super::super::helpers::ui_border_focus_color_(Color::Rgba(0, 0, 255, 1.0)),
        ];
        let elem: Element<TestMsg> = Element::Text("hi".to_owned());
        let html = ui_layout(attrs, elem);
        let s = render_html(&html);
        assert!(s.contains("h|background-color:rgba(255,0,0,1)"), "{s}");
        assert!(s.contains("||"), "entries must be || joined: {s}");
        assert!(s.contains("v|border-color:rgba(0,0,255,1)"), "{s}");
        assert_eq!(
            s.matches("data-ipe-pc-rules").count(),
            1,
            "exactly ONE merged marker attr, not one per rule: {s}"
        );
    }

    #[test]
    fn ui_on_pseudo_all_five_constants_produce_distinct_wire_tags() {
        // hover→h, focus→f, focusVisible→v, active→a, disabled→d — MUST match
        // `ipe_runtime::live::style_inject::pseudo_selector_for_tag` and the
        // `../ipe` reference's `pseudoClassTag`.
        let cases: [(super::super::element::PseudoClass, &str); 5] = [
            (super::super::helpers::ui_hover_(), "h"),
            (super::super::helpers::ui_focus_(), "f"),
            (super::super::helpers::ui_focus_visible_(), "v"),
            (super::super::helpers::ui_active_(), "a"),
            (super::super::helpers::ui_disabled_(), "d"),
        ];
        for (pc, tag) in cases {
            let attr: Attribute<TestMsg> =
                super::super::helpers::ui_on_pseudo_(pc, vec![Attribute::AttrPointer]);
            let s = build_style_string_for_test(&attr);
            assert!(
                s.starts_with(&format!("{tag}|")),
                "pseudo-class {pc:?} must wire-tag as {tag:?}: {s}"
            );
        }
    }

    /// Test-only accessor: extract the `(tag, css)` payload of an
    /// `AttrPseudoRule` as `"tag|css"` for assertions above.
    fn build_style_string_for_test<M>(attr: &Attribute<M>) -> String {
        match attr {
            Attribute::AttrPseudoRule(pc, css) => format!("{}|{css}", pc.wire_tag()),
            _ => String::new(),
        }
    }

    #[test]
    fn ui_on_file_registers_ipe_file_wire_event() {
        use crate::html::{Attribute as HtmlAttr, Event};
        let attr =
            super::super::helpers::ui_on_file_::<TestMsg>(std::sync::Arc::new(|_s| TestMsg::Click));
        match attr {
            Attribute::AttrEvent(HtmlAttr::EventAttr(Event::OnString(name, _))) => {
                assert_eq!(name, "ipe-file", "onFile must wire as event name ipe-file");
            }
            other => {
                panic!("expected AttrEvent(EventAttr(OnString(\"ipe-file\", _))), got {other:?}")
            }
        }
    }

    #[test]
    fn paragraph_el_child_renders_inline_span_not_block_div() {
        // Mirrors `Ipe.Markdown.renderInline "a **bold** word"`:
        //   Ui.paragraph [] [ Ui.text "a "
        //                    , Ui.el [ Font.bold ] (Ui.text "bold")
        //                    , Ui.text " word" ]
        // The bold `Ui.el` lowers to Element::Node(NoDescription, [FontWeight
        // 700], [Text "bold"]). On the web backend it MUST render as an inline
        // <span>, not a block <div>, so the bold run flows inline and the markup
        // stays valid inside <p>.
        let para: Element<TestMsg> = super::super::helpers::ui_paragraph_(
            vec![],
            vec![
                Element::Text("a ".to_owned()),
                super::super::helpers::ui_el_(
                    vec![Attribute::AttrFontWeight(700)],
                    Element::Text("bold".to_owned()),
                ),
                Element::Text(" word".to_owned()),
            ],
        );
        let html = render_element(para);
        let s = render_html(&html);
        assert!(
            s.contains("<span style=\"display:inline-block;vertical-align:baseline;font-weight:700\">bold</span>"),
            "bold el child must render as an inline <span>: {s}"
        );
        assert!(
            !s.contains("<div"),
            "no block <div> may appear inside the paragraph: {s}"
        );
        assert!(
            s.starts_with("<p"),
            "the paragraph itself must render as <p>: {s}"
        );
    }

    // RT-UI-001: depth cap — render_element must return (not abort/stack-overflow)
    // when given a tree deeper than MAX_HTML_DEPTH = 1024. We build a chain at
    // depth 1200 and run the render in a 48 MB thread (debug-mode frame sizes are
    // ~10–20× release-mode sizes, so the cap depth × frame needs a larger stack
    // than the 8 MB default to reach). The key property: render returns instead of
    // recursing forever; the cap silently truncates at depth 1024.
    #[test]
    fn render_element_depth_cap_does_not_overflow() {
        // Build Node([], [Node([], [Node([], [... Text("leaf") ...])])]) 1200 deep.
        const DEPTH: usize = 1_200;
        let is_valid = std::thread::Builder::new()
            .stack_size(48 * 1024 * 1024) // 48 MB — enough for debug-mode frames
            .spawn(|| {
                let mut elem: Element<TestMsg> = Element::Text("leaf".to_owned());
                for _ in 0..DEPTH {
                    elem = Element::Node(
                        super::super::element::Description::NoDescription,
                        vec![],
                        vec![elem],
                    );
                }
                // This call must return, not recurse forever. The depth cap at 1024
                // truncates the remaining 176 levels and returns an empty text node.
                let html = render_element(elem);
                let valid = matches!(
                    &html,
                    crate::html::Html::HElement(_, _, _) | crate::html::Html::HText(_)
                );
                // Leak to avoid recursive drop overflow at this depth.
                std::mem::forget(html);
                valid
            })
            .expect("spawn thread")
            .join()
            .expect("thread did not panic");
        assert!(is_valid, "render_element must return a valid Html variant");
    }
}

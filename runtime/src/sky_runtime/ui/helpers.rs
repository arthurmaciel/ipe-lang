//! Helper functions backing the Std.Ui kernel dispatch in the Rust code-gen.
//!
//! Each function corresponds to one `KernelFn` variant wired in `sky_lower` +
//! `sky_backend_rust`. The signatures mirror `Std/Ui.sky` exactly so that the
//! emitter can call them without any wrapping or unwrapping.
//!
//! Naming convention: every public function carries a trailing underscore to
//! match the `naming.rs` convention for kernel helpers (e.g. `ui_column_`)
//! and to avoid shadowing the runtime's own `element` type names.

use super::element::{Attribute, Color, Description, Element, HAlign, Length, Location, PseudoClass, VAlign};
use crate::sky_runtime::core::SkyMaybe;
use crate::sky_runtime::html::Html;

/// Inline colour → CSS string.  Mirrors `render::color_css`; kept private so
/// helpers.rs stays self-contained without a dependency on render.rs.
#[inline]
fn color_to_css(c: &Color) -> String {
    match c {
        Color::Rgba(r, g, b, a) => format!("rgba({r},{g},{b},{a})"),
    }
}

// ── Element builders ──────────────────────────────────────────────────────────

/// `Ui.none : Element msg`
pub fn ui_none_<M>() -> Element<M> {
    Element::Empty
}

/// `Ui.text : String -> Element msg`
pub fn ui_text_<M>(s: String) -> Element<M> {
    Element::Text(s)
}

/// `Ui.html : Html msg -> Element msg`
pub fn ui_html_<M: Clone>(h: Html<M>) -> Element<M> {
    Element::Raw(h)
}

/// `Ui.el : List (Attribute msg) -> Element msg -> Element msg`
pub fn ui_el_<M: Clone>(attrs: Vec<Attribute<M>>, ch: Element<M>) -> Element<M> {
    Element::Node(Description::NoDescription, attrs, vec![ch])
}

/// `Ui.row : List (Attribute msg) -> List (Element msg) -> Element msg`
///
/// Prepends the `__row` row-direction marker matching `rowMarker` in
/// `Std/Ui.sky`.
pub fn ui_row_<M: Clone>(attrs: Vec<Attribute<M>>, children: Vec<Element<M>>) -> Element<M> {
    let mut full = Vec::with_capacity(attrs.len() + 1);
    full.push(Attribute::AttrStyle("__row".to_owned(), "true".to_owned()));
    full.extend(attrs);
    Element::Node(Description::NoDescription, full, children)
}

/// `Ui.column : List (Attribute msg) -> List (Element msg) -> Element msg`
///
/// Prepends the `__col` column-direction marker matching `colMarker` in
/// `Std/Ui.sky`.
pub fn ui_column_<M: Clone>(attrs: Vec<Attribute<M>>, children: Vec<Element<M>>) -> Element<M> {
    let mut full = Vec::with_capacity(attrs.len() + 1);
    full.push(Attribute::AttrStyle("__col".to_owned(), "true".to_owned()));
    full.extend(attrs);
    Element::Node(Description::NoDescription, full, children)
}

/// `Ui.wrappedRow : List (Attribute msg) -> List (Element msg) -> Element msg`
pub fn ui_wrapped_row_<M: Clone>(
    attrs: Vec<Attribute<M>>,
    children: Vec<Element<M>>,
) -> Element<M> {
    let mut full = Vec::with_capacity(attrs.len() + 1);
    full.push(Attribute::AttrStyle(
        "__wrappedrow".to_owned(),
        "true".to_owned(),
    ));
    full.extend(attrs);
    Element::Node(Description::NoDescription, full, children)
}

/// `Ui.grid : List (Attribute msg) -> List (Element msg) -> Element msg`
pub fn ui_grid_<M: Clone>(attrs: Vec<Attribute<M>>, children: Vec<Element<M>>) -> Element<M> {
    let mut full = Vec::with_capacity(attrs.len() + 1);
    full.push(Attribute::AttrStyle("__grid".to_owned(), "true".to_owned()));
    full.extend(attrs);
    Element::Node(Description::NoDescription, full, children)
}

/// `Ui.button : List (Attribute msg) -> { onPress : Maybe msg, label : Element msg } -> Element msg`
///
/// When `onPress` is `Just msg`, adds an `onclick` event attribute + `cursor: pointer`.
/// When `onPress` is `Nothing`, adds `disabled="true"` so the button is visually and
/// semantically disabled.
pub fn ui_button_<M: Clone>(
    attrs: Vec<Attribute<M>>,
    on_press: SkyMaybe<M>,
    label: Element<M>,
) -> Element<M> {
    use crate::sky_runtime::html::{Attribute as HtmlAttribute, Event};
    let mut full = Vec::with_capacity(attrs.len() + 2);
    match on_press {
        SkyMaybe::Just(msg) => {
            full.push(Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnMsg(
                "click".into(),
                msg,
            ))));
            full.push(Attribute::AttrPointer);
        }
        SkyMaybe::Nothing => {
            full.push(Attribute::AttrAttribute("disabled".into(), "true".into()));
        }
    }
    full.extend(attrs);
    Element::TaggedNode(
        "button".into(),
        Description::NoDescription,
        full,
        vec![label],
    )
}

/// `Ui.link : List (Attribute msg) -> { url : String, label : Element msg } -> Element msg`
/// Renders as `<a href=url>label</a>`.
pub fn ui_link_<M: Clone>(
    attrs: Vec<Attribute<M>>,
    url: String,
    label: Element<M>,
) -> Element<M> {
    let mut full = Vec::with_capacity(attrs.len() + 1);
    full.push(Attribute::AttrAttribute("href".into(), url));
    full.extend(attrs);
    Element::TaggedNode(
        "a".into(),
        Description::NoDescription,
        full,
        vec![label],
    )
}

// ── Attribute builders ────────────────────────────────────────────────────────

/// `Ui.spacing : Int -> Attribute msg`
pub fn ui_spacing_<M>(n: i64) -> Attribute<M> {
    Attribute::AttrSpacing(n)
}

/// `Ui.padding : Int -> Attribute msg`  (uniform padding on all four sides)
pub fn ui_padding_<M>(n: i64) -> Attribute<M> {
    // AttrPadding(top, right, bottom, left)
    Attribute::AttrPadding(n, n, n, n)
}

/// `Ui.paddingXY : Int -> Int -> Attribute msg`
///
/// `x` = left/right padding, `y` = top/bottom padding.
pub fn ui_padding_xy_<M>(x: i64, y: i64) -> Attribute<M> {
    Attribute::AttrPadding(y, x, y, x)
}

/// `Ui.width : Length -> Attribute msg`
pub fn ui_width_<M>(l: Length) -> Attribute<M> {
    Attribute::AttrWidth(l)
}

/// `Ui.height : Length -> Attribute msg`
pub fn ui_height_<M>(l: Length) -> Attribute<M> {
    Attribute::AttrHeight(l)
}

/// `Ui.centerX : Attribute msg`
pub fn ui_center_x_<M>() -> Attribute<M> {
    Attribute::AttrAlignX(HAlign::CenterX)
}

/// `Ui.centerY : Attribute msg`
pub fn ui_center_y_<M>() -> Attribute<M> {
    Attribute::AttrAlignY(VAlign::CenterY)
}

/// `Ui.alignLeft : Attribute msg`
pub fn ui_align_left_<M>() -> Attribute<M> {
    Attribute::AttrAlignX(HAlign::AlignLeft)
}

/// `Ui.alignRight : Attribute msg`
pub fn ui_align_right_<M>() -> Attribute<M> {
    Attribute::AttrAlignX(HAlign::AlignRight)
}

/// `Ui.alignTop : Attribute msg`
pub fn ui_align_top_<M>() -> Attribute<M> {
    Attribute::AttrAlignY(VAlign::AlignTop)
}

/// `Ui.alignBottom : Attribute msg`
pub fn ui_align_bottom_<M>() -> Attribute<M> {
    Attribute::AttrAlignY(VAlign::AlignBottom)
}

/// `Ui.pointer : Attribute msg`
pub fn ui_pointer_<M>() -> Attribute<M> {
    Attribute::AttrPointer
}

/// `Ui.clip / clipX / clipY : Attribute msg`
pub fn ui_clip_<M>() -> Attribute<M> {
    Attribute::AttrOverflow("hidden".to_owned(), "hidden".to_owned())
}

/// `Ui.scrollbars / scrollbarX / scrollbarY : Attribute msg`
pub fn ui_scrollbars_<M>() -> Attribute<M> {
    Attribute::AttrOverflow("auto".to_owned(), "auto".to_owned())
}

/// `Ui.gridColumns : Int -> Attribute msg`
pub fn ui_grid_columns_<M>(n: i64) -> Attribute<M> {
    Attribute::AttrStyle("--sky-grid-columns".to_owned(), n.to_string())
}

// ── Length builders ───────────────────────────────────────────────────────────

/// `Ui.px : Int -> Length`
pub fn ui_px_(n: i64) -> Length {
    Length::Px(n)
}

/// `Ui.fill : Length`  (fill portion = 1)
pub fn ui_fill_() -> Length {
    Length::Fill(1)
}

/// `Ui.content : Length`
pub fn ui_content_() -> Length {
    Length::Content
}

/// `Ui.shrink : Length`  (alias for `Ui.content` in Sky)
pub fn ui_shrink_() -> Length {
    Length::Content
}

/// `Ui.fillPortion : Int -> Length`
pub fn ui_fill_portion_(n: i64) -> Length {
    Length::Fill(n)
}

/// `Ui.vh : Int -> Length`
pub fn ui_vh_(n: i64) -> Length {
    Length::Vh(n)
}

/// `Ui.vw : Int -> Length`
pub fn ui_vw_(n: i64) -> Length {
    Length::Vw(n)
}

/// `Ui.minimum : Int -> Length -> Length`
pub fn ui_minimum_(n: i64, l: Length) -> Length {
    Length::Min(n, Box::new(l))
}

/// `Ui.maximum : Int -> Length -> Length`
pub fn ui_maximum_(n: i64, l: Length) -> Length {
    Length::Max(n, Box::new(l))
}

// ── Color builders ────────────────────────────────────────────────────────────

/// `Ui.rgb : Int -> Int -> Int -> Color`  (alpha = 1.0)
pub fn ui_rgb_(r: i64, g: i64, b: i64) -> Color {
    Color::Rgba(r, g, b, 1.0)
}

/// `Ui.rgba : Int -> Int -> Int -> Float -> Color`
pub fn ui_rgba_(r: i64, g: i64, b: i64, a: f64) -> Color {
    Color::Rgba(r, g, b, a)
}

/// `Ui.white : Color`
pub fn ui_white_() -> Color {
    Color::Rgba(255, 255, 255, 1.0)
}

/// `Ui.black : Color`
pub fn ui_black_() -> Color {
    Color::Rgba(0, 0, 0, 1.0)
}

/// `Ui.transparent : Color`
pub fn ui_transparent_() -> Color {
    Color::Rgba(0, 0, 0, 0.0)
}

/// `Ui.colorCss : Color -> String` — convert a `Color` to its CSS string.
pub fn ui_color_css_(c: Color) -> String {
    color_to_css(&c)
}

// ── Background sub-module ─────────────────────────────────────────────────────

/// `Background.color : Color -> Attribute msg`
pub fn ui_background_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrBgColor(c)
}

/// `Background.image : String -> Attribute msg`
pub fn ui_background_image_<M>(s: String) -> Attribute<M> {
    Attribute::AttrBgImage(s)
}

// ── Border sub-module ─────────────────────────────────────────────────────────

/// `Border.width : Int -> Attribute msg`
pub fn ui_border_width_<M>(n: i64) -> Attribute<M> {
    Attribute::AttrBorderWidth(n)
}

/// `Border.rounded : Int -> Attribute msg`
pub fn ui_border_rounded_<M>(n: i64) -> Attribute<M> {
    Attribute::AttrBorderRounded(n)
}

/// `Border.color : Color -> Attribute msg`
pub fn ui_border_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrBorderColor(c)
}

/// `Border.widthEach : { top : Int, right : Int, bottom : Int, left : Int } -> Attribute msg`
pub fn ui_border_width_each_<M>(top: i64, right: i64, bottom: i64, left: i64) -> Attribute<M> {
    Attribute::AttrBorderWidthEach(top, right, bottom, left)
}

/// `Border.shadow : { offsetX : Int, offsetY : Int, blur : Int, spread : Int, color : Color } -> Attribute msg`
///
/// Mirrors the reference (`Std.Ui.Border.shadow` → `borderShadow`) which renders
/// the CSS `box-shadow: <ox>px <oy>px <blur>px <spread>px <colour>;` shape. Uses
/// the dedicated `AttrBorderShadow` runtime variant (rendered in `render.rs`) so
/// the colour flows through the same `color_css` boundary as `Border.color`.
pub fn ui_border_shadow_<M>(
    horiz: i64,
    vert: i64,
    blur: i64,
    spread: i64,
    c: Color,
) -> Attribute<M> {
    Attribute::AttrBorderShadow(horiz, vert, blur, spread, c)
}

/// `Border.glow : Int -> Color -> Attribute msg`
///
/// Convenience wrapper over `box-shadow`: a shadow with `(0, 0)` offset and `0`
/// spread, so the user supplies only a blur radius and a colour. Emits the CSS
/// `box-shadow: 0px 0px <blur>px 0px <colour>` via the generic `AttrStyle`
/// boundary (the colour flows through the same `color_to_css` conversion as
/// `Border.color` / `Border.shadow`).
pub fn ui_border_glow_<M>(blur: i64, c: Color) -> Attribute<M> {
    Attribute::AttrStyle(
        "box-shadow".into(),
        format!("0px 0px {blur}px 0px {}", color_to_css(&c)),
    )
}

/// `Border.innerShadow : { offsetX : Int, offsetY : Int, blur : Int, spread : Int, color : Color } -> Attribute msg`
///
/// Same record shape as [`ui_border_shadow_`] but INSET: renders the CSS
/// `box-shadow: inset <ox>px <oy>px <blur>px <spread>px <colour>;`. Uses the
/// dedicated `AttrBorderInsetShadow` runtime variant (rendered in `render.rs`)
/// so the colour flows through the same `color_css` boundary as `Border.color`.
pub fn ui_border_inner_shadow_<M>(
    horiz: i64,
    vert: i64,
    blur: i64,
    spread: i64,
    c: Color,
) -> Attribute<M> {
    Attribute::AttrBorderInsetShadow(horiz, vert, blur, spread, c)
}

// ── Font sub-module ───────────────────────────────────────────────────────────

/// `Font.size : Int -> Attribute msg`
pub fn ui_font_size_<M>(n: i64) -> Attribute<M> {
    Attribute::AttrFontSize(n)
}

/// `Font.color : Color -> Attribute msg`
pub fn ui_font_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrFontColor(c)
}

/// `Font.family : List String -> Attribute msg`
///
/// Passes the font-family CSS value string through as-is.
pub fn ui_font_family_<M>(family: String) -> Attribute<M> {
    Attribute::AttrFontFamily(family)
}

/// `Font.bold : Attribute msg`
pub fn ui_font_bold_<M>() -> Attribute<M> {
    Attribute::AttrFontWeight(700)
}

/// `Font.italic : Attribute msg`
pub fn ui_font_italic_<M>() -> Attribute<M> {
    Attribute::AttrFontItalic
}

// ── Html element builders ─────────────────────────────────────────────────────
// These mirror `Std.Html`'s pure-Sky constructors (`HText`, `HRaw`,
// `HElement`) without the `Sky.Ffi` dependency that blocks compiling
// `Std/Html.sky` from source in Sky-Rust.

/// `Html.text : String -> Html msg`
pub fn html_text_node_<M>(s: String) -> Html<M> {
    Html::HText(s)
}

/// `Html.raw : String -> Html msg`
pub fn html_raw_node_<M>(s: String) -> Html<M> {
    Html::HRaw(s)
}

/// `Html.styleNode : List (Attribute msg) -> String -> Html msg`
///
/// SECURITY (F7): `styleNode` is arity-2 `(attrs, css:String)` — NOT the arity-3
/// `html_node_` it was previously mis-lowered to. It bakes the injection fix into
/// construction (PARSE, DON'T VALIDATE): the CSS body is close-tag-neutralised
/// exactly once, HERE, so the `HRaw` it produces is already safe. The `<style>`
/// render sink (`html::render_into_ctx`) strips again — defence in depth — so a
/// `</style><script>` breakout in a `Std.Css` value cannot reach the DOM.
pub fn html_style_node_<M>(
    attrs: Vec<crate::sky_runtime::html::Attribute<M>>,
    css: String,
) -> Html<M> {
    Html::HElement(
        "style".to_owned(),
        attrs,
        vec![Html::HRaw(
            crate::sky_runtime::css_safety::strip_style_close(&css),
        )],
    )
}

/// `Html.node : String -> List (Attribute msg) -> List (Html msg) -> Html msg`
pub fn html_node_<M>(
    tag: String,
    attrs: Vec<crate::sky_runtime::html::Attribute<M>>,
    children: Vec<Html<M>>,
) -> Html<M> {
    Html::HElement(tag, attrs, children)
}

/// `Html.div (and header) : List (Attribute msg) -> List (Html msg) -> Html msg`
pub fn html_div_<M>(
    attrs: Vec<crate::sky_runtime::html::Attribute<M>>,
    children: Vec<Html<M>>,
) -> Html<M> {
    Html::HElement("div".to_owned(), attrs, children)
}

/// `Html.span : List (Attribute msg) -> List (Html msg) -> Html msg`
pub fn html_span_<M>(
    attrs: Vec<crate::sky_runtime::html::Attribute<M>>,
    children: Vec<Html<M>>,
) -> Html<M> {
    Html::HElement("span".to_owned(), attrs, children)
}

/// `Html.a (and link) : List (Attribute msg) -> List (Html msg) -> Html msg`
pub fn html_a_<M>(
    attrs: Vec<crate::sky_runtime::html::Attribute<M>>,
    children: Vec<Html<M>>,
) -> Html<M> {
    Html::HElement("a".to_owned(), attrs, children)
}

/// `Html.button : List (Attribute msg) -> List (Html msg) -> Html msg`
pub fn html_button_<M>(
    attrs: Vec<crate::sky_runtime::html::Attribute<M>>,
    children: Vec<Html<M>>,
) -> Html<M> {
    Html::HElement("button".to_owned(), attrs, children)
}

/// `Html.p (and other block elements) : List (Attribute msg) -> List (Html msg) -> Html msg`
///
/// NOTE (Phase 0): `h1`/`h2`/.../`body`/`footer`/`nav`/`section`/… all map
/// here because they share the 2-arg `(attrs, children)` signature.  A future
/// refactor will split them into per-tag kernel variants or use `html_node_`
/// with an injected tag-name arg.  In Phase 0 only `p` is the primary tag —
/// the other tag names are not yet exercised by any test.
pub fn html_p_<M>(
    attrs: Vec<crate::sky_runtime::html::Attribute<M>>,
    children: Vec<Html<M>>,
) -> Html<M> {
    Html::HElement("p".to_owned(), attrs, children)
}

/// `Html.input : List (Attribute msg) -> Html msg`  (void element, no children)
pub fn html_input_<M>(attrs: Vec<crate::sky_runtime::html::Attribute<M>>) -> Html<M> {
    Html::HElement("input".to_owned(), attrs, vec![])
}

/// `Html.img (and other void elements) : List (Attribute msg) -> Html msg`
pub fn html_img_<M>(attrs: Vec<crate::sky_runtime::html::Attribute<M>>) -> Html<M> {
    Html::HElement("img".to_owned(), attrs, vec![])
}

// ── Phase-1a: Event-attribute builders ───────────────────────────────────────
//
// These back the `UiOnClick`, `UiOnFocus`, … KernelFn variants.  They return
// `element::Attribute<M>` (same as all other Ui attribute builders) with the
// `AttrEvent` variant wrapping an `html::Attribute::EventAttr(Event::…)`.
//
// The two `Attribute` types are:
//   • `html::Attribute<M>` — raw HTML attribute (event, class, data-*, …)
//   • `element::Attribute<M>` — typed Std.Ui attribute; `AttrEvent` carries
//     an `html::Attribute` for event dispatch.
//
// Plain-message events (`OnMsg`) take the typed message value directly.
// String-carrying events (`OnString`) take an `Arc<dyn Fn(String)->M+…>`
// so the runtime can call the function from a send-safe dispatcher.
// Callers emit: `Arc::new(move |_x| (f)(_x))`.

use crate::sky_runtime::html::{Attribute as HtmlAttribute, Event};

/// `Ui.onClick : msg -> Attribute msg`
pub fn ui_on_click_<M>(msg: M) -> Attribute<M> {
    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnMsg("click".into(), msg)))
}

/// `Ui.onFocus : msg -> Attribute msg`
pub fn ui_on_focus_<M>(msg: M) -> Attribute<M> {
    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnMsg("focus".into(), msg)))
}

/// `Ui.onBlur : msg -> Attribute msg`
pub fn ui_on_blur_<M>(msg: M) -> Attribute<M> {
    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnMsg("blur".into(), msg)))
}

/// `Ui.onMouseOver : msg -> Attribute msg`
pub fn ui_on_mouse_over_<M>(msg: M) -> Attribute<M> {
    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnMsg(
        "mouseover".into(),
        msg,
    )))
}

/// `Ui.onMouseOut : msg -> Attribute msg`
pub fn ui_on_mouse_out_<M>(msg: M) -> Attribute<M> {
    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnMsg(
        "mouseout".into(),
        msg,
    )))
}

/// `Ui.onInput : (String -> msg) -> Attribute msg`
///
/// The callback is Arc-wrapped so the runtime can dispatch it from a
/// send-safe context.  Callers emit `std::sync::Arc::new(move |_x| (f)(_x))`
/// where `f` is the emitted Sky function expression (T6 trap).
pub fn ui_on_input_<M>(f: std::sync::Arc<dyn Fn(String) -> M + Send + Sync>) -> Attribute<M> {
    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnString("input".into(), f)))
}

/// `Ui.onChange : (String -> msg) -> Attribute msg`
pub fn ui_on_change_<M>(f: std::sync::Arc<dyn Fn(String) -> M + Send + Sync>) -> Attribute<M> {
    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnString(
        "change".into(),
        f,
    )))
}

/// `Ui.onKeyDown : (String -> msg) -> Attribute msg`
pub fn ui_on_key_down_<M>(f: std::sync::Arc<dyn Fn(String) -> M + Send + Sync>) -> Attribute<M> {
    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnString(
        "keydown".into(),
        f,
    )))
}

/// `Ui.onKeyUp : (String -> msg) -> Attribute msg`
pub fn ui_on_key_up_<M>(f: std::sync::Arc<dyn Fn(String) -> M + Send + Sync>) -> Attribute<M> {
    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnString("keyup".into(), f)))
}

/// `Event.onBool : (Bool -> msg) -> Attribute msg`
///
/// Wires a boolean-carrying event (typically `change` on a checkbox) so that
/// the Sky callback receives the DOM `checked` value as a Rust `bool`.
/// The `f` argument is arc-wrapped at the call site by the emitter (T6 trap).
pub fn ui_on_bool_<M>(f: std::sync::Arc<dyn Fn(bool) -> M + Send + Sync>) -> Attribute<M> {
    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnBool("change".into(), f)))
}

// ── #76 Tier 1: extended Std.Ui / Font / Background / Border builders ────────

// Ui namespace — aspect-ratio

/// `Ui.square : Attribute msg` — aspect-ratio 1 / 1.
pub fn ui_square_<M>() -> Attribute<M> {
    Attribute::AttrStyle("aspect-ratio".into(), "1 / 1".into())
}

/// `Ui.widescreen : Attribute msg` — aspect-ratio 16 / 9.
pub fn ui_widescreen_<M>() -> Attribute<M> {
    Attribute::AttrStyle("aspect-ratio".into(), "16 / 9".into())
}

/// `Ui.cinemascope : Attribute msg` — aspect-ratio 2.35 / 1 (anamorphic wide).
pub fn ui_cinemascope_<M>() -> Attribute<M> {
    Attribute::AttrStyle("aspect-ratio".into(), "2.35 / 1".into())
}

/// `Ui.name : String -> Attribute msg` — HTML name= attribute (radio groups, form fields).
pub fn ui_name_<M>(value: String) -> Attribute<M> {
    Attribute::AttrAttribute("name".into(), value)
}

/// `Ui.style : String -> String -> Attribute msg` — raw inline CSS property + value.
pub fn ui_style_<M>(property: String, value: String) -> Attribute<M> {
    Attribute::AttrStyle(property, value)
}

/// `Ui.transitionRaw : String -> Bool -> Attribute msg` — the CSS `transition`
/// shorthand (built by `Std.Ui.Transition.buildShorthand`) plus a
/// respect-`prefers-reduced-motion` flag. `respect = True` (via
/// `Transition.attribute`) auto-gates the rule behind
/// `@media (prefers-reduced-motion: no-preference)` in the live style-injection
/// pass; `False` (via `attributeUnsafe`) fires unconditionally.
pub fn ui_transition_raw_<M>(shorthand: String, respect: bool) -> Attribute<M> {
    Attribute::AttrTransition(shorthand, respect)
}

/// `Ui.gridTracksRaw : String -> String -> Attribute msg` — CSS grid-template-columns
/// (first arg) and grid-template-rows (second arg).  Pass `""` for either axis to skip it.
pub fn ui_grid_tracks_raw_<M>(cols: String, rows: String) -> Attribute<M> {
    Attribute::AttrGridTracks(cols, rows)
}

/// `Ui.aspectRatio : Float -> Attribute msg`
pub fn ui_aspect_ratio_<M>(r: f64) -> Attribute<M> {
    Attribute::AttrStyle("aspect-ratio".into(), format!("{r}"))
}

/// `Ui.aspectRatioWH : Int -> Int -> Attribute msg`
pub fn ui_aspect_ratio_wh_<M>(w: i64, h: i64) -> Attribute<M> {
    Attribute::AttrStyle("aspect-ratio".into(), format!("{w} / {h}"))
}

/// `Ui.htmlAttribute : String -> String -> Attribute msg`
pub fn ui_html_attribute_<M>(key: String, value: String) -> Attribute<M> {
    Attribute::AttrAttribute(key, value)
}

// Background pseudo-class colour attrs

/// `Background.hoverColor : Color -> Attribute msg`
pub fn ui_bg_hover_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrPseudoRule(
        PseudoClass::Hover,
        format!("background-color:{}", color_to_css(&c)),
    )
}

/// `Background.focusColor : Color -> Attribute msg`
pub fn ui_bg_focus_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrPseudoRule(
        PseudoClass::FocusVisible,
        format!("background-color:{}", color_to_css(&c)),
    )
}

/// `Background.activeColor : Color -> Attribute msg`
pub fn ui_bg_active_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrPseudoRule(
        PseudoClass::Active,
        format!("background-color:{}", color_to_css(&c)),
    )
}

/// `Background.disabledColor : Color -> Attribute msg`
pub fn ui_bg_disabled_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrPseudoRule(
        PseudoClass::Disabled,
        format!("background-color:{}", color_to_css(&c)),
    )
}

// Border namespace — style keywords (nullary)

/// `Border.solid : Attribute msg`
pub fn ui_border_solid_<M>() -> Attribute<M> {
    Attribute::AttrBorderStyle("solid".into())
}

/// `Border.dashed : Attribute msg`
pub fn ui_border_dashed_<M>() -> Attribute<M> {
    Attribute::AttrBorderStyle("dashed".into())
}

/// `Border.dotted : Attribute msg`
pub fn ui_border_dotted_<M>() -> Attribute<M> {
    Attribute::AttrBorderStyle("dotted".into())
}

// Border pseudo-class attrs

/// `Border.hoverColor : Color -> Attribute msg`
pub fn ui_border_hover_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrPseudoRule(
        PseudoClass::Hover,
        format!("border-color:{}", color_to_css(&c)),
    )
}

/// `Border.focusColor : Color -> Attribute msg`
pub fn ui_border_focus_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrPseudoRule(
        PseudoClass::FocusVisible,
        format!("border-color:{}", color_to_css(&c)),
    )
}

/// `Border.activeColor : Color -> Attribute msg`
pub fn ui_border_active_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrPseudoRule(
        PseudoClass::Active,
        format!("border-color:{}", color_to_css(&c)),
    )
}

/// `Border.hoverWidth : Int -> Attribute msg`
pub fn ui_border_hover_width_<M>(n: i64) -> Attribute<M> {
    Attribute::AttrPseudoRule(PseudoClass::Hover, format!("border-width:{n}px"))
}

/// `Border.hoverRounded : Int -> Attribute msg`
pub fn ui_border_hover_rounded_<M>(n: i64) -> Attribute<M> {
    Attribute::AttrPseudoRule(PseudoClass::Hover, format!("border-radius:{n}px"))
}

// Font namespace — weight

/// `Font.weight : Int -> Attribute msg`
pub fn ui_font_weight_<M>(n: i64) -> Attribute<M> {
    Attribute::AttrFontWeight(n)
}

/// `Font.semiBold : Attribute msg` (weight 600)
pub fn ui_font_semi_bold_<M>() -> Attribute<M> {
    Attribute::AttrFontWeight(600)
}

/// `Font.regular : Attribute msg` (weight 400)
pub fn ui_font_regular_<M>() -> Attribute<M> {
    Attribute::AttrFontWeight(400)
}

/// `Font.light : Attribute msg` (weight 300)
pub fn ui_font_light_<M>() -> Attribute<M> {
    Attribute::AttrFontWeight(300)
}

/// `Font.extraBold : Attribute msg` (weight 800)
pub fn ui_font_extra_bold_<M>() -> Attribute<M> {
    Attribute::AttrFontWeight(800)
}

/// `Font.black : Attribute msg` (weight 900)
pub fn ui_font_black_<M>() -> Attribute<M> {
    Attribute::AttrFontWeight(900)
}

// Font namespace — decoration

/// `Font.underline : Attribute msg`
pub fn ui_font_underline_<M>() -> Attribute<M> {
    Attribute::AttrFontUnderline
}

/// `Font.noDecoration : Attribute msg`
pub fn ui_font_no_decoration_<M>() -> Attribute<M> {
    Attribute::AttrFontDecoration("none".into())
}

/// `Font.lineThrough : Attribute msg`
pub fn ui_font_line_through_<M>() -> Attribute<M> {
    Attribute::AttrFontDecoration("line-through".into())
}

// Font namespace — spacing (Float → Attr)

/// `Font.letterSpacing : Float -> Attribute msg`
pub fn ui_font_letter_spacing_<M>(v: f64) -> Attribute<M> {
    Attribute::AttrFontLetterSpacing(v)
}

/// `Font.wordSpacing : Float -> Attribute msg`
pub fn ui_font_word_spacing_<M>(v: f64) -> Attribute<M> {
    Attribute::AttrFontWordSpacing(v)
}

// Font namespace — text alignment (nullary)

/// `Font.alignLeft : Attribute msg`
pub fn ui_font_align_left_<M>() -> Attribute<M> {
    Attribute::AttrFontAlign("left".into())
}

/// `Font.alignRight : Attribute msg`
pub fn ui_font_align_right_<M>() -> Attribute<M> {
    Attribute::AttrFontAlign("right".into())
}

/// `Font.alignCenter : Attribute msg`
pub fn ui_font_align_center_<M>() -> Attribute<M> {
    Attribute::AttrFontAlign("center".into())
}

/// `Font.center : Attribute msg`
pub fn ui_font_center_<M>() -> Attribute<M> {
    Attribute::AttrFontAlign("center".into())
}

/// `Font.justify : Attribute msg`
pub fn ui_font_justify_<M>() -> Attribute<M> {
    Attribute::AttrFontAlign("justify".into())
}

// Font namespace — String constants (NOT Attribute; used as members of List String
// passed to Font.family)

/// `Font.sansSerif : String`
pub fn ui_font_sans_serif_() -> String {
    "sans-serif".into()
}

/// `Font.serif : String`
pub fn ui_font_serif_() -> String {
    "serif".into()
}

/// `Font.monospace : String`
pub fn ui_font_monospace_() -> String {
    "monospace".into()
}

// Font namespace — pseudo-class colour attrs

/// `Font.hoverColor : Color -> Attribute msg`
pub fn ui_font_hover_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrPseudoRule(PseudoClass::Hover, format!("color:{}", color_to_css(&c)))
}

/// `Font.focusColor : Color -> Attribute msg`
pub fn ui_font_focus_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrPseudoRule(
        PseudoClass::FocusVisible,
        format!("color:{}", color_to_css(&c)),
    )
}

/// `Font.activeColor : Color -> Attribute msg`
pub fn ui_font_active_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrPseudoRule(PseudoClass::Active, format!("color:{}", color_to_css(&c)))
}

/// `Font.disabledColor : Color -> Attribute msg`
pub fn ui_font_disabled_color_<M>(c: Color) -> Attribute<M> {
    Attribute::AttrPseudoRule(
        PseudoClass::Disabled,
        format!("color:{}", color_to_css(&c)),
    )
}

/// `Font.hoverSize : Int -> Attribute msg`
pub fn ui_font_hover_size_<M>(n: i64) -> Attribute<M> {
    Attribute::AttrPseudoRule(PseudoClass::Hover, format!("font-size:{n}px"))
}

// ── Std.Ui.Region (#117) ────────────────────────────────────────────────────

/// `Region.mainContent : Attribute msg`
pub fn ui_region_main_content_<M>() -> Attribute<M> {
    Attribute::AttrDescribe(Description::DescMain)
}

/// `Region.navigation : Attribute msg`
pub fn ui_region_navigation_<M>() -> Attribute<M> {
    Attribute::AttrDescribe(Description::DescNavigation)
}

/// `Region.footer : Attribute msg`
pub fn ui_region_footer_<M>() -> Attribute<M> {
    Attribute::AttrDescribe(Description::DescContentInfo)
}

/// `Region.aside : Attribute msg`
pub fn ui_region_aside_<M>() -> Attribute<M> {
    Attribute::AttrDescribe(Description::DescComplementary)
}

/// `Region.heading : Int -> Attribute msg`
pub fn ui_region_heading_<M>(n: i64) -> Attribute<M> {
    Attribute::AttrDescribe(Description::DescHeading(n))
}

/// `Region.label : String -> Attribute msg`
pub fn ui_region_label_<M>(s: String) -> Attribute<M> {
    Attribute::AttrDescribe(Description::DescLabel(s))
}

/// `Region.announce : Attribute msg`
pub fn ui_region_announce_<M>() -> Attribute<M> {
    Attribute::AttrDescribe(Description::DescLivePolite)
}

/// `Region.announceUrgently : Attribute msg`
pub fn ui_region_announce_urgently_<M>() -> Attribute<M> {
    Attribute::AttrDescribe(Description::DescLiveAssertive)
}

// ── Ui.input + Ui.describe + desc* constructors ────────────────────────────

/// `Ui.input : List (Attribute msg) -> Element msg`
///
/// Creates a real `<input>` void HTML element:
/// `TaggedNode "input" NoDescription attrs []`
pub fn ui_input_<M>(attrs: Vec<Attribute<M>>) -> Element<M> {
    Element::TaggedNode("input".into(), Description::NoDescription, attrs, vec![])
}

/// `Ui.describe : Description -> Attribute msg`
pub fn ui_describe_<M>(d: Description) -> Attribute<M> {
    Attribute::AttrDescribe(d)
}

/// `Ui.descMain : Description`
pub fn ui_desc_main_() -> Description {
    Description::DescMain
}

/// `Ui.descNavigation : Description`
pub fn ui_desc_navigation_() -> Description {
    Description::DescNavigation
}

/// `Ui.descContentInfo : Description`
pub fn ui_desc_content_info_() -> Description {
    Description::DescContentInfo
}

/// `Ui.descComplementary : Description`
pub fn ui_desc_complementary_() -> Description {
    Description::DescComplementary
}

/// `Ui.descLivePolite : Description`
pub fn ui_desc_live_polite_() -> Description {
    Description::DescLivePolite
}

/// `Ui.descLiveAssertive : Description`
pub fn ui_desc_live_assertive_() -> Description {
    Description::DescLiveAssertive
}

/// `Ui.descHeading : Int -> Description`
pub fn ui_desc_heading_(n: i64) -> Description {
    Description::DescHeading(n)
}

/// `Ui.descLabel : String -> Description`
pub fn ui_desc_label_(s: String) -> Description {
    Description::DescLabel(s)
}

/// `Ui.paragraph : List (Attribute msg) -> List (Element msg) -> Element msg`
///
/// Mirrors `paragraph` in `Std/Ui.sky`: a `<p>`-tagged node carrying
/// `DescParagraph` plus the `__paragraph` marker (matching `paragraphMarker`),
/// so text children wrap as inline flow.
pub fn ui_paragraph_<M: Clone>(
    attrs: Vec<Attribute<M>>,
    children: Vec<Element<M>>,
) -> Element<M> {
    let mut full = Vec::with_capacity(attrs.len() + 1);
    full.push(Attribute::AttrStyle(
        "__paragraph".to_owned(),
        "true".to_owned(),
    ));
    full.extend(attrs);
    Element::TaggedNode("p".to_owned(), Description::DescParagraph, full, children)
}

/// `Ui.textColumn : List (Attribute msg) -> List (Element msg) -> Element msg`
///
/// Mirrors `textColumn` in `Std/Ui.sky`: a `<section>`-tagged block container
/// with the `__textcolumn` marker (matching `textColumnMarker`), keeping each
/// paragraph child on its own line with normal text flow.
pub fn ui_text_column_<M: Clone>(
    attrs: Vec<Attribute<M>>,
    children: Vec<Element<M>>,
) -> Element<M> {
    let mut full = Vec::with_capacity(attrs.len() + 1);
    full.push(Attribute::AttrStyle(
        "__textcolumn".to_owned(),
        "true".to_owned(),
    ));
    full.extend(attrs);
    Element::TaggedNode(
        "section".to_owned(),
        Description::NoDescription,
        full,
        children,
    )
}

// ── Form ─────────────────────────────────────────────────────────────────────

/// `Ui.form : List (Attribute msg) -> List (Element msg) -> Element msg`
pub fn ui_form_<M: Clone>(
    attrs: Vec<Attribute<M>>,
    children: Vec<Element<M>>,
) -> Element<M> {
    Element::TaggedNode(
        "form".to_owned(),
        Description::NoDescription,
        attrs,
        children,
    )
}

/// `Ui.onSubmit : (a -> msg) -> Attribute msg`
///
/// Builds `Event::OnForm` directly: the handler `f`'s argument type `T` is
/// recovered by ordinary Rust generic inference from `f`'s own monomorphized
/// signature at the codegen call site (never type-erased at runtime). The
/// Sky.Live dispatch layer (`HandlerIndex::resolve_form`) decodes the wire
/// `FormData` into `T` via a re-encoded x-www-form-urlencoded round trip
/// (`live::form::decode_form_or_warn` — type-directed per-field coercion, NOT
/// a JSON path), matching the Go backend's `json.Unmarshal` semantics at the
/// record-shape level (case-insensitive field-name match, missing field ⇒
/// zero value). `F: Fn(T) -> M + Send + Sync + 'static` is always satisfied
/// by emitted Sky function types (they are `'static` enum constructors or
/// pure closures with no borrows) — a strictly narrower requirement than the
/// `A: Any` bound this replaces (#109/#156).
#[cfg(feature = "live")]
pub fn ui_on_submit_<M, T, F>(f: F) -> Attribute<M>
where
    T: serde::de::DeserializeOwned,
    F: Fn(T) -> M + Send + Sync + 'static,
{
    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnForm(
        "submit".into(),
        std::sync::Arc::new(move |fd: crate::sky_runtime::html::FormData| {
            crate::sky_runtime::live::form::decode_form_or_warn::<T>(fd).map(&f)
        }),
    )))
}

/// Non-`live` builds (Sky.Tui without the HTTP form wire): `Ui.onSubmit` was
/// already inert everywhere before this fix, so this degrades to a
/// structural no-op — not a regression, Tui has no form-submit wire concept.
#[cfg(not(feature = "live"))]
pub fn ui_on_submit_<M, T, F: Fn(T) -> M>(_f: F) -> Attribute<M> {
    Attribute::NoAttribute
}

// ── Nearby attribute builders ────────────────────────────────────────────────

/// `Ui.above : Element msg -> Attribute msg`
pub fn ui_above_<M: Clone>(elem: Element<M>) -> Attribute<M> {
    Attribute::AttrNearby(Location::Above, elem)
}

/// `Ui.below : Element msg -> Attribute msg`
pub fn ui_below_<M: Clone>(elem: Element<M>) -> Attribute<M> {
    Attribute::AttrNearby(Location::Below, elem)
}

/// `Ui.onLeft : Element msg -> Attribute msg`
pub fn ui_on_left_<M: Clone>(elem: Element<M>) -> Attribute<M> {
    Attribute::AttrNearby(Location::OnLeft, elem)
}

/// `Ui.onRight : Element msg -> Attribute msg`
pub fn ui_on_right_<M: Clone>(elem: Element<M>) -> Attribute<M> {
    Attribute::AttrNearby(Location::OnRight, elem)
}

/// `Ui.inFront : Element msg -> Attribute msg`
pub fn ui_in_front_<M: Clone>(elem: Element<M>) -> Attribute<M> {
    Attribute::AttrNearby(Location::InFront, elem)
}

/// `Ui.behind : Element msg -> Attribute msg`
pub fn ui_behind_<M: Clone>(elem: Element<M>) -> Attribute<M> {
    Attribute::AttrNearby(Location::Behind, elem)
}

// ── #154: Breakpoint constants + wrapper ──────────────────────────────────────
//
// These functions support `Ui.breakpoint` and the named `Breakpoint` constants
// (`Ui.mobile`, `Ui.tablet`, …).
//
// **Sanctioned divergence from Sky Go** (see docs/divergences-from-sky.md
// §B-Breakpoint): in the Go runtime `Breakpoint` is an opaque struct that
// carries the CSS media-query string and is consumed by the renderer to inject
// a `<style data-sky-mq=…>` child.  In this Rust Phase-0 port we model
// `Breakpoint` as a plain `String` (the raw CSS query) and the
// `ui_breakpoint_` wrapper is a no-op passthrough — the element is returned
// unchanged.  Rendering of media-query scoped styles is tracked as a Phase-1
// item.

/// `Ui.mobile : String` — CSS media query `(max-width: 767px)`.
pub fn ui_mobile_<M>() -> String {
    "(max-width: 767px)".to_owned()
}

/// `Ui.tablet : String` — CSS media query `(min-width: 768px) and (max-width: 1023px)`.
pub fn ui_tablet_<M>() -> String {
    "(min-width: 768px) and (max-width: 1023px)".to_owned()
}

/// `Ui.desktop : String` — CSS media query `(min-width: 1024px)`.
pub fn ui_desktop_<M>() -> String {
    "(min-width: 1024px)".to_owned()
}

/// `Ui.darkMode : String` — CSS media query `(prefers-color-scheme: dark)`.
pub fn ui_dark_mode_<M>() -> String {
    "(prefers-color-scheme: dark)".to_owned()
}

/// `Ui.lightMode : String` — CSS media query `(prefers-color-scheme: light)`.
pub fn ui_light_mode_<M>() -> String {
    "(prefers-color-scheme: light)".to_owned()
}

/// `Ui.reducedMotion : String` — CSS media query `(prefers-reduced-motion: reduce)`.
pub fn ui_reduced_motion_<M>() -> String {
    "(prefers-reduced-motion: reduce)".to_owned()
}

/// `Ui.breakpoint : String -> List (Attribute msg) -> Element msg -> Element msg`
///
/// Phase-0 passthrough: the `_query` and `_attrs` arguments are intentionally
/// ignored and the element is returned unchanged.  Breakpoint-scoped CSS
/// injection is a Phase-1 renderer feature.
pub fn ui_breakpoint_<M>(
    _query: String,
    _attrs: Vec<Attribute<M>>,
    el: Element<M>,
) -> Element<M> {
    el
}

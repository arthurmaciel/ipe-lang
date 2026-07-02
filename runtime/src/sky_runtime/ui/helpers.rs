//! Helper functions backing the Std.Ui kernel dispatch in the Rust code-gen.
//!
//! Each function corresponds to one `KernelFn` variant wired in `sky_lower` +
//! `sky_backend_rust`. The signatures mirror `Std/Ui.sky` exactly so that the
//! emitter can call them without any wrapping or unwrapping.
//!
//! Naming convention: every public function carries a trailing underscore to
//! match the `naming.rs` convention for kernel helpers (e.g. `ui_column_`)
//! and to avoid shadowing the runtime's own `element` type names.

use super::element::{Attribute, Color, Description, Element, HAlign, Length, VAlign};
use crate::sky_runtime::html::Html;

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
/// Joins the family list with `", "` — CSS `font-family` value format.
pub fn ui_font_family_<M>(families: Vec<String>) -> Attribute<M> {
    Attribute::AttrFontFamily(families.join(", "))
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

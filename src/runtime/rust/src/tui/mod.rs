//! Ipe.Terminal — terminal (ANSI cell) backend for the Rust target.
//!
//! TEA-shaped (`Tui.app cfg`): `view : Model -> Cells msg` lowered
//! to ANSI cells via the structured `Element<Msg>` tree inside `CellsView<M>`.

pub mod app;
pub mod cell;
pub mod diff; // accessed qualified (tui::diff::diff) — `diff` collides with live's
pub mod focus; // input registry + focusable model + key editing
pub mod key;
pub mod layout; // structured Element → ANSI cells
pub use app::{tui_app, tui_app_ui};
pub use cell::*;

use crate::ui::element::{Attribute, Color, Element, HAlign};

/// A cell-native view attribute: the ONLY attributes a terminal `Screen` view
/// can carry.  Distinct from the DOM `ui::Attribute` — DOM-only affordances
/// (`onClick`, `scrollbars`, `inFront`, …) have no `TuiAttr` variant, so they
/// are unnameable in a terminal view rather than silently dropped at render.
///
/// Each variant translates to the honorable `ui::Attribute` the cell renderer
/// already interprets (`translate`), so the layout engine is unchanged.
#[derive(Clone, Debug, PartialEq)]
pub enum TuiAttr<M> {
    /// Gap between children, in whole terminal cells.
    Spacing(i64),
    /// Inner padding on all four sides, in whole terminal cells.
    Padding(i64),
    /// Horizontal alignment of the node's content.
    Align(HAlign),
    /// Bold text.
    Bold,
    /// Underlined text.
    Underline,
    /// Dim (faint) text.
    Dim,
    /// Reverse video (swap foreground and background).
    Reverse,
    /// Foreground (text) colour, from the terminal palette.
    FgColor(Color),
    /// Background colour, from the terminal palette.
    BgColor(Color),
    /// Uninhabited-in-practice marker carrying the message type so `TuiAttr`
    /// stays parametric in `M` even though no current variant holds an `M`.
    _Msg(core::marker::PhantomData<M>),
}

impl<M> TuiAttr<M> {
    /// Lower a cell-native attribute to the honorable `ui::Attribute` the cell
    /// layout engine reads.  `bold` maps to a font weight the renderer treats
    /// as bold (`>= 600`).
    fn translate(self) -> Option<Attribute<M>> {
        match self {
            TuiAttr::Spacing(n) => Some(Attribute::AttrSpacing(n)),
            TuiAttr::Padding(n) => Some(Attribute::AttrPadding(n, n, n, n)),
            TuiAttr::Align(h) => Some(Attribute::AttrAlignX(h)),
            TuiAttr::Bold => Some(Attribute::AttrFontWeight(700)),
            TuiAttr::Underline => Some(Attribute::AttrFontUnderline),
            TuiAttr::Dim => Some(Attribute::AttrFontDecoration("dim".to_owned())),
            TuiAttr::Reverse => Some(Attribute::AttrFontDecoration("reverse".to_owned())),
            TuiAttr::FgColor(c) => Some(Attribute::AttrFontColor(c)),
            TuiAttr::BgColor(c) => Some(Attribute::AttrBgColor(c)),
            TuiAttr::_Msg(_) => None,
        }
    }
}

/// Lower a list of cell-native attributes to the honorable `ui::Attribute` set.
fn translate_attrs<M>(attrs: Vec<TuiAttr<M>>) -> Vec<Attribute<M>> {
    attrs.into_iter().filter_map(TuiAttr::translate).collect()
}

/// Newtype wrapper: the Tui-only view type `Screen msg`.
///
/// Emitted code produces a `CellsView<M>` from every `Ipe.Tea.Tui.Ui.*`
/// builder call; `tui_app_ui` unwraps the inner `Element<M>` and passes it
/// to the ANSI-cell renderer.  The wrapper is the compile-time gate that
/// prevents Web-only `Element`-producing constructs from appearing inside a
/// `view : Model -> Screen Msg` function.
#[derive(Clone, Debug, PartialEq)]
pub struct CellsView<M>(pub Element<M>);

impl<M> CellsView<M> {
    /// Wrap an existing `Element` as a `Cells` view.
    pub fn new(inner: Element<M>) -> Self {
        Self(inner)
    }

    /// Consume the wrapper and return the inner `Element`.
    pub fn into_element(self) -> Element<M> {
        self.0
    }
}

// ── Ipe.Ui.Cells builder functions ───────────────────────────────────────────
// Each delegates to the corresponding `ui_*_` helper and wraps the result in
// `CellsView`.  The wrapper is the only compile-time evidence that a value
// came from a `Cells`-producing builder, making Web-only builders (which return
// bare `Element<M>`) an immediate type error inside a `view : M -> Cells Msg`
// function.

/// `Ipe.Ui.Cells.none : Cells msg`
#[must_use]
pub fn cells_none_<M>() -> CellsView<M> {
    CellsView::new(Element::Empty)
}

/// `Ipe.Ui.Cells.text : String -> Cells msg`
#[must_use]
pub fn cells_text_<M>(s: String) -> CellsView<M> {
    CellsView::new(Element::Text(s))
}

/// `Ipe.Ui.Cells.cells : List (List Char) -> Cells msg`
#[must_use]
pub fn cells_cells_<M>(grid: Vec<Vec<char>>) -> CellsView<M> {
    CellsView::new(Element::Cells(grid))
}

/// `Ipe.Tea.Tui.Ui.el : List (Attribute msg) -> Screen msg -> Screen msg`
#[must_use]
pub fn cells_el_<M: Clone>(attrs: Vec<TuiAttr<M>>, child: CellsView<M>) -> CellsView<M> {
    use crate::ui::element::Description;
    CellsView::new(Element::Node(
        Description::NoDescription,
        translate_attrs(attrs),
        vec![child.into_element()],
    ))
}

/// `Ipe.Tea.Tui.Ui.row : List (Attribute msg) -> List (Screen msg) -> Screen msg`
#[must_use]
pub fn cells_row_<M: Clone>(attrs: Vec<TuiAttr<M>>, children: Vec<CellsView<M>>) -> CellsView<M> {
    use crate::ui::element::Description;
    let mut full = Vec::with_capacity(attrs.len() + 1);
    full.push(Attribute::AttrStyle("__row".to_owned(), "true".to_owned()));
    full.extend(translate_attrs(attrs));
    let elems: Vec<Element<M>> = children.into_iter().map(CellsView::into_element).collect();
    CellsView::new(Element::Node(Description::NoDescription, full, elems))
}

/// `Ipe.Tea.Tui.Ui.column : List (Attribute msg) -> List (Screen msg) -> Screen msg`
#[must_use]
pub fn cells_column_<M: Clone>(
    attrs: Vec<TuiAttr<M>>,
    children: Vec<CellsView<M>>,
) -> CellsView<M> {
    use crate::ui::element::Description;
    let mut full = Vec::with_capacity(attrs.len() + 1);
    full.push(Attribute::AttrStyle("__col".to_owned(), "true".to_owned()));
    full.extend(translate_attrs(attrs));
    let elems: Vec<Element<M>> = children.into_iter().map(CellsView::into_element).collect();
    CellsView::new(Element::Node(Description::NoDescription, full, elems))
}

// ── Cell-native attribute builders (`Ipe.Tea.Tui.Ui.Attribute`) ───────────────
// Each returns a `TuiAttr<M>` — the terminal-honorable attribute surface.

/// `Ipe.Tea.Tui.Ui.spacing : Int -> Attribute msg` — gap between children, cells.
#[must_use]
pub fn tui_spacing_<M>(n: i64) -> TuiAttr<M> {
    TuiAttr::Spacing(n)
}

/// `Ipe.Tea.Tui.Ui.padding : Int -> Attribute msg` — inner padding, cells.
#[must_use]
pub fn tui_padding_<M>(n: i64) -> TuiAttr<M> {
    TuiAttr::Padding(n)
}

/// `Ipe.Tea.Tui.Ui.alignLeft : Attribute msg`
#[must_use]
pub fn tui_align_left_<M>() -> TuiAttr<M> {
    TuiAttr::Align(HAlign::AlignLeft)
}

/// `Ipe.Tea.Tui.Ui.alignRight : Attribute msg`
#[must_use]
pub fn tui_align_right_<M>() -> TuiAttr<M> {
    TuiAttr::Align(HAlign::AlignRight)
}

/// `Ipe.Tea.Tui.Ui.center : Attribute msg`
#[must_use]
pub fn tui_center_<M>() -> TuiAttr<M> {
    TuiAttr::Align(HAlign::CenterX)
}

/// `Ipe.Tea.Tui.Ui.bold : Attribute msg`
#[must_use]
pub fn tui_bold_<M>() -> TuiAttr<M> {
    TuiAttr::Bold
}

/// `Ipe.Tea.Tui.Ui.underline : Attribute msg`
#[must_use]
pub fn tui_underline_<M>() -> TuiAttr<M> {
    TuiAttr::Underline
}

/// `Ipe.Tea.Tui.Ui.dim : Attribute msg` — faint text.
#[must_use]
pub fn tui_dim_<M>() -> TuiAttr<M> {
    TuiAttr::Dim
}

/// `Ipe.Tea.Tui.Ui.reverse : Attribute msg` — reverse video.
#[must_use]
pub fn tui_reverse_<M>() -> TuiAttr<M> {
    TuiAttr::Reverse
}

/// `Ipe.Tea.Tui.Ui.color : Color -> Attribute msg` — foreground text colour.
#[must_use]
pub fn tui_color_<M>(c: Color) -> TuiAttr<M> {
    TuiAttr::FgColor(c)
}

/// `Ipe.Tea.Tui.Ui.bg : Color -> Attribute msg` — background colour.
#[must_use]
pub fn tui_bg_<M>(c: Color) -> TuiAttr<M> {
    TuiAttr::BgColor(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dim_and_reverse_translate_to_decoration_attributes() {
        assert!(matches!(
            TuiAttr::<()>::Dim.translate(),
            Some(Attribute::AttrFontDecoration(s)) if s == "dim"
        ));
        assert!(matches!(
            TuiAttr::<()>::Reverse.translate(),
            Some(Attribute::AttrFontDecoration(s)) if s == "reverse"
        ));
    }
}

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

use crate::ui::element::{Attribute, Element};

/// Newtype wrapper: the Tui-only view type `Cells msg`.
///
/// Emitted code produces a `CellsView<M>` from every `Ipe.Ui.Cells.*`
/// builder call; `tui_app_ui` unwraps the inner `Element<M>` and passes it
/// to the ANSI-cell renderer.  The wrapper is the compile-time gate that
/// prevents Web-only `Element`-producing constructs from appearing inside a
/// `view : Model -> Cells Msg` function.
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

/// `Ipe.Ui.Cells.el : List (Attribute msg) -> Cells msg -> Cells msg`
#[must_use]
pub fn cells_el_<M: Clone>(attrs: Vec<Attribute<M>>, child: CellsView<M>) -> CellsView<M> {
    use crate::ui::element::Description;
    CellsView::new(Element::Node(
        Description::NoDescription,
        attrs,
        vec![child.into_element()],
    ))
}

/// `Ipe.Ui.Cells.row : List (Attribute msg) -> List (Cells msg) -> Cells msg`
#[must_use]
pub fn cells_row_<M: Clone>(attrs: Vec<Attribute<M>>, children: Vec<CellsView<M>>) -> CellsView<M> {
    use crate::ui::element::Description;
    let mut full = Vec::with_capacity(attrs.len() + 1);
    full.push(Attribute::AttrStyle("__row".to_owned(), "true".to_owned()));
    full.extend(attrs);
    let elems: Vec<Element<M>> = children.into_iter().map(CellsView::into_element).collect();
    CellsView::new(Element::Node(Description::NoDescription, full, elems))
}

/// `Ipe.Ui.Cells.column : List (Attribute msg) -> List (Cells msg) -> Cells msg`
#[must_use]
pub fn cells_column_<M: Clone>(
    attrs: Vec<Attribute<M>>,
    children: Vec<CellsView<M>>,
) -> CellsView<M> {
    use crate::ui::element::Description;
    let mut full = Vec::with_capacity(attrs.len() + 1);
    full.push(Attribute::AttrStyle("__col".to_owned(), "true".to_owned()));
    full.extend(attrs);
    let elems: Vec<Element<M>> = children.into_iter().map(CellsView::into_element).collect();
    CellsView::new(Element::Node(Description::NoDescription, full, elems))
}

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

/// The first-class terminal colour palette (`Terminal.Color`): the sixteen
/// named ANSI colours plus `default` (the terminal's own colour), plus a
/// truecolour path. A closed sum — an invalid colour has no representation.
///
/// Named colours render as ANSI SGR palette codes (portable across terminals);
/// `Rgb` / `Rgba` render as 24-bit truecolour on terminals that support it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TermColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    /// The terminal's own default foreground / background colour.
    Default,
    /// A 24-bit truecolour (red, green, blue), 0-255 each.
    Rgb(u8, u8, u8),
}

impl TermColor {
    /// The ANSI SGR code for this colour as a *foreground*: named palette codes
    /// 30-37 / 90-97, `default` 39. Returns `None` for the truecolour path
    /// (rendered separately as a `38;2;r;g;b` sequence).
    #[must_use]
    pub const fn fg_code(self) -> Option<u8> {
        Some(match self {
            TermColor::Black => 30,
            TermColor::Red => 31,
            TermColor::Green => 32,
            TermColor::Yellow => 33,
            TermColor::Blue => 34,
            TermColor::Magenta => 35,
            TermColor::Cyan => 36,
            TermColor::White => 37,
            TermColor::BrightBlack => 90,
            TermColor::BrightRed => 91,
            TermColor::BrightGreen => 92,
            TermColor::BrightYellow => 93,
            TermColor::BrightBlue => 94,
            TermColor::BrightMagenta => 95,
            TermColor::BrightCyan => 96,
            TermColor::BrightWhite => 97,
            TermColor::Default => 39,
            TermColor::Rgb(..) => return None,
        })
    }

    /// The ANSI SGR code for this colour as a *background*: named palette codes
    /// 40-47 / 100-107, `default` 49. Returns `None` for the truecolour path.
    #[must_use]
    pub const fn bg_code(self) -> Option<u8> {
        // Background codes are the foreground code offset by 10.
        match self.fg_code() {
            Some(fg) => Some(fg + 10),
            None => None,
        }
    }
}

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
    FgColor(TermColor),
    /// Background colour, from the terminal palette.
    BgColor(TermColor),
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
            TuiAttr::FgColor(c) => Some(term_fg_attr(c)),
            TuiAttr::BgColor(c) => Some(term_bg_attr(c)),
            TuiAttr::_Msg(_) => None,
        }
    }
}

/// Lower a list of cell-native attributes to the honorable `ui::Attribute` set.
fn translate_attrs<M>(attrs: Vec<TuiAttr<M>>) -> Vec<Attribute<M>> {
    attrs.into_iter().filter_map(TuiAttr::translate).collect()
}

/// Lower a palette foreground colour to a renderer attribute. A named colour is
/// carried as a decoration string (`"fg:31"`) so the terminal `sgr` path emits
/// the portable SGR palette code; a truecolour takes the 24-bit `AttrFontColor`
/// path the renderer already interprets.
fn term_fg_attr<M>(c: TermColor) -> Attribute<M> {
    match c {
        TermColor::Rgb(r, g, b) => {
            Attribute::AttrFontColor(Color::Rgba(i64::from(r), i64::from(g), i64::from(b), 1.0))
        }
        named => Attribute::AttrFontDecoration(format!("fg:{}", named.fg_code().unwrap_or(39))),
    }
}

/// Lower a palette background colour to a renderer attribute (see `term_fg_attr`).
fn term_bg_attr<M>(c: TermColor) -> Attribute<M> {
    match c {
        TermColor::Rgb(r, g, b) => {
            Attribute::AttrBgColor(Color::Rgba(i64::from(r), i64::from(g), i64::from(b), 1.0))
        }
        named => Attribute::AttrFontDecoration(format!("bg:{}", named.bg_code().unwrap_or(49))),
    }
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

/// `Ipe.Tea.Tui.Ui.color : Terminal.Color -> Attribute msg` — foreground colour.
#[must_use]
pub fn tui_color_<M>(c: TermColor) -> TuiAttr<M> {
    TuiAttr::FgColor(c)
}

/// `Ipe.Tea.Tui.Ui.bg : Terminal.Color -> Attribute msg` — background colour.
#[must_use]
pub fn tui_bg_<M>(c: TermColor) -> TuiAttr<M> {
    TuiAttr::BgColor(c)
}

// ── Ipe.Tea.Terminal.Color palette constructors ──────────────────────────────

/// `Terminal.Color.black : Color`
#[must_use]
pub const fn term_color_black_() -> TermColor {
    TermColor::Black
}
/// `Terminal.Color.red : Color`
#[must_use]
pub const fn term_color_red_() -> TermColor {
    TermColor::Red
}
/// `Terminal.Color.green : Color`
#[must_use]
pub const fn term_color_green_() -> TermColor {
    TermColor::Green
}
/// `Terminal.Color.yellow : Color`
#[must_use]
pub const fn term_color_yellow_() -> TermColor {
    TermColor::Yellow
}
/// `Terminal.Color.blue : Color`
#[must_use]
pub const fn term_color_blue_() -> TermColor {
    TermColor::Blue
}
/// `Terminal.Color.magenta : Color`
#[must_use]
pub const fn term_color_magenta_() -> TermColor {
    TermColor::Magenta
}
/// `Terminal.Color.cyan : Color`
#[must_use]
pub const fn term_color_cyan_() -> TermColor {
    TermColor::Cyan
}
/// `Terminal.Color.white : Color`
#[must_use]
pub const fn term_color_white_() -> TermColor {
    TermColor::White
}
/// `Terminal.Color.brightBlack : Color`
#[must_use]
pub const fn term_color_bright_black_() -> TermColor {
    TermColor::BrightBlack
}
/// `Terminal.Color.brightRed : Color`
#[must_use]
pub const fn term_color_bright_red_() -> TermColor {
    TermColor::BrightRed
}
/// `Terminal.Color.brightGreen : Color`
#[must_use]
pub const fn term_color_bright_green_() -> TermColor {
    TermColor::BrightGreen
}
/// `Terminal.Color.brightYellow : Color`
#[must_use]
pub const fn term_color_bright_yellow_() -> TermColor {
    TermColor::BrightYellow
}
/// `Terminal.Color.brightBlue : Color`
#[must_use]
pub const fn term_color_bright_blue_() -> TermColor {
    TermColor::BrightBlue
}
/// `Terminal.Color.brightMagenta : Color`
#[must_use]
pub const fn term_color_bright_magenta_() -> TermColor {
    TermColor::BrightMagenta
}
/// `Terminal.Color.brightCyan : Color`
#[must_use]
pub const fn term_color_bright_cyan_() -> TermColor {
    TermColor::BrightCyan
}
/// `Terminal.Color.brightWhite : Color`
#[must_use]
pub const fn term_color_bright_white_() -> TermColor {
    TermColor::BrightWhite
}
/// `Terminal.Color.default : Color`
#[must_use]
pub const fn term_color_default_() -> TermColor {
    TermColor::Default
}
/// `Terminal.Color.rgb : Int -> Int -> Int -> Color` — a 24-bit truecolour.
/// Channels are clamped to 0-255.
#[must_use]
pub fn term_color_rgb_(r: i64, g: i64, b: i64) -> TermColor {
    TermColor::Rgb(clamp_channel(r), clamp_channel(g), clamp_channel(b))
}
/// `Terminal.Color.rgba : Int -> Int -> Int -> Float -> Color`. The alpha is
/// accepted for surface parity with `Ui.rgba`; a terminal cell has no alpha, so
/// the colour is applied opaque.
#[must_use]
pub fn term_color_rgba_(r: i64, g: i64, b: i64, _a: f64) -> TermColor {
    TermColor::Rgb(clamp_channel(r), clamp_channel(g), clamp_channel(b))
}

/// Clamp an `Int` colour channel into the representable `0..=255` byte range.
fn clamp_channel(v: i64) -> u8 {
    v.clamp(0, 255) as u8
}

// ── Ipe.Tea.Cli.Ui line-oriented view surface ────────────────────────────────

/// A line-native view attribute: the ONLY styles a `Lines` view can carry.
/// Distinct from both the DOM `ui::Attribute` and the cell-native `TuiAttr` —
/// 2D geometry (`spacing`, `padding`, alignment) has no `CliAttr` variant, so
/// it is unnameable in a `Lines` view rather than silently dropped.
#[derive(Clone, Debug, PartialEq)]
pub enum CliAttr<M> {
    /// Bold text.
    Bold,
    /// Underlined text.
    Underline,
    /// Dim (faint) text.
    Dim,
    /// Reverse video (swap foreground and background).
    Reverse,
    /// Foreground (text) colour, from the terminal palette.
    FgColor(TermColor),
    /// Background colour, from the terminal palette.
    BgColor(TermColor),
    /// Marker carrying the message type so `CliAttr` stays parametric in `M`.
    _Msg(core::marker::PhantomData<M>),
}

impl<M> CliAttr<M> {
    /// Lower a line-native attribute to the honorable `ui::Attribute` the cell
    /// layout engine reads. A `Lines` view is rendered by the same styled-run
    /// engine as `Screen`, restricted to one column of lines.
    fn translate(self) -> Option<Attribute<M>> {
        match self {
            CliAttr::Bold => Some(Attribute::AttrFontWeight(700)),
            CliAttr::Underline => Some(Attribute::AttrFontUnderline),
            CliAttr::Dim => Some(Attribute::AttrFontDecoration("dim".to_owned())),
            CliAttr::Reverse => Some(Attribute::AttrFontDecoration("reverse".to_owned())),
            CliAttr::FgColor(c) => Some(term_fg_attr(c)),
            CliAttr::BgColor(c) => Some(term_bg_attr(c)),
            CliAttr::_Msg(_) => None,
        }
    }
}

fn translate_cli_attrs<M>(attrs: Vec<CliAttr<M>>) -> Vec<Attribute<M>> {
    attrs.into_iter().filter_map(CliAttr::translate).collect()
}

/// Newtype wrapper: the Cli-only line-oriented view type `Lines msg`.
///
/// A `Lines` view is a vertical stack of styled lines. It reuses the cell
/// layout engine (as a single column) so styling renders identically to a
/// `Screen`, but its builder surface admits only line-scoped attributes.
#[derive(Clone, Debug, PartialEq)]
pub struct LinesView<M>(pub Element<M>);

impl<M> LinesView<M> {
    /// Wrap an existing `Element` as a `Lines` view.
    pub fn new(inner: Element<M>) -> Self {
        Self(inner)
    }

    /// Consume the wrapper and return the inner `Element`.
    pub fn into_element(self) -> Element<M> {
        self.0
    }
}

/// `Ipe.Tea.Cli.Ui.none : Lines msg`
#[must_use]
pub fn cli_none_<M>() -> LinesView<M> {
    LinesView::new(Element::Empty)
}

/// `Ipe.Tea.Cli.Ui.text : String -> Lines msg` — one unstyled line.
#[must_use]
pub fn cli_text_<M>(s: String) -> LinesView<M> {
    LinesView::new(Element::Text(s))
}

/// `Ipe.Tea.Cli.Ui.line : List (Attribute msg) -> String -> Lines msg`
#[must_use]
pub fn cli_line_<M: Clone>(attrs: Vec<CliAttr<M>>, s: String) -> LinesView<M> {
    use crate::ui::element::Description;
    LinesView::new(Element::Node(
        Description::NoDescription,
        translate_cli_attrs(attrs),
        vec![Element::Text(s)],
    ))
}

/// `Ipe.Tea.Cli.Ui.lines : List (Lines msg) -> Lines msg` — stack vertically.
#[must_use]
pub fn cli_lines_<M: Clone>(children: Vec<LinesView<M>>) -> LinesView<M> {
    use crate::ui::element::Description;
    let attrs = vec![Attribute::AttrStyle("__col".to_owned(), "true".to_owned())];
    let elems: Vec<Element<M>> = children.into_iter().map(LinesView::into_element).collect();
    LinesView::new(Element::Node(Description::NoDescription, attrs, elems))
}

/// `Ipe.Tea.Cli.Ui.bold : Attribute msg`
#[must_use]
pub fn cli_bold_<M>() -> CliAttr<M> {
    CliAttr::Bold
}
/// `Ipe.Tea.Cli.Ui.underline : Attribute msg`
#[must_use]
pub fn cli_underline_<M>() -> CliAttr<M> {
    CliAttr::Underline
}
/// `Ipe.Tea.Cli.Ui.dim : Attribute msg` — faint text.
#[must_use]
pub fn cli_dim_<M>() -> CliAttr<M> {
    CliAttr::Dim
}
/// `Ipe.Tea.Cli.Ui.reverse : Attribute msg` — reverse video.
#[must_use]
pub fn cli_reverse_<M>() -> CliAttr<M> {
    CliAttr::Reverse
}
/// `Ipe.Tea.Cli.Ui.color : Terminal.Color -> Attribute msg` — foreground colour.
#[must_use]
pub fn cli_color_<M>(c: TermColor) -> CliAttr<M> {
    CliAttr::FgColor(c)
}
/// `Ipe.Tea.Cli.Ui.bg : Terminal.Color -> Attribute msg` — background colour.
#[must_use]
pub fn cli_bg_<M>(c: TermColor) -> CliAttr<M> {
    CliAttr::BgColor(c)
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

    #[test]
    fn term_color_named_fg_and_bg_codes() {
        assert_eq!(TermColor::Red.fg_code(), Some(31));
        assert_eq!(TermColor::Red.bg_code(), Some(41));
        assert_eq!(TermColor::BrightRed.fg_code(), Some(91));
        assert_eq!(TermColor::BrightRed.bg_code(), Some(101));
        assert_eq!(TermColor::Default.fg_code(), Some(39));
        assert_eq!(TermColor::Default.bg_code(), Some(49));
        assert_eq!(TermColor::Black.fg_code(), Some(30));
        assert_eq!(TermColor::BrightWhite.fg_code(), Some(97));
    }

    #[test]
    fn term_color_rgb_has_no_named_code() {
        assert_eq!(TermColor::Rgb(1, 2, 3).fg_code(), None);
        assert_eq!(TermColor::Rgb(1, 2, 3).bg_code(), None);
    }

    #[test]
    fn named_palette_fg_translates_to_code_decoration() {
        assert!(matches!(
            term_fg_attr::<()>(TermColor::Red),
            Attribute::AttrFontDecoration(s) if s == "fg:31"
        ));
        assert!(matches!(
            term_bg_attr::<()>(TermColor::Blue),
            Attribute::AttrFontDecoration(s) if s == "bg:44"
        ));
    }

    #[test]
    fn truecolor_translates_to_font_color_attribute() {
        assert!(matches!(
            term_fg_attr::<()>(TermColor::Rgb(10, 20, 30)),
            Attribute::AttrFontColor(Color::Rgba(10, 20, 30, _))
        ));
    }

    #[test]
    fn cli_attr_line_styles_translate() {
        assert!(matches!(
            CliAttr::<()>::Bold.translate(),
            Some(Attribute::AttrFontWeight(700))
        ));
        assert!(matches!(
            CliAttr::<()>::FgColor(TermColor::Green).translate(),
            Some(Attribute::AttrFontDecoration(s)) if s == "fg:32"
        ));
    }

    #[test]
    fn rgb_channels_clamp_into_byte_range() {
        assert_eq!(term_color_rgb_(-5, 300, 128), TermColor::Rgb(0, 255, 128));
    }
}

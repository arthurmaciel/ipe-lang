//! Shared `Ipe.Ui` element tree — the general UI abstraction.
//!
//! These types mirror `ipe-stdlib/Ipe/Ui.ipe`'s ADTs **variant-for-variant and
//! field-for-field**. They live in the runtime (not generated per-project) so
//! that every backend — Ipe.Web (→ HTML), Ipe.Tui (→ ANSI cells), Ipe.WebView
//! (→ native webview) — renders the SAME structured `Element` tree to its own
//! target, exactly as the Go backend does (`runtime-go/rt/tui_ui.go` walks the
//! structured Element ADT directly; it never round-trips through CSS).
//!
//! The Rust codegen maps the Ipê `Ipe.Ui.*` types onto these via
//! `runtimeOpaqueTypes` (the same `{M}` mechanism that makes `Html` a shared
//! type), so `Ipe.Ui.column` etc. construct `ipe_runtime::ui::Element` and the
//! pure-Ipê render chain (`renderElement` → `Html`) pattern-matches them.
//!
//! INVARIANT (load-bearing): the variant names + field order MUST stay identical
//! to `Ipe.Ui.ipe:39-190`. The opaque alias hides any drift from the Rust
//! compiler, so a mismatch mis-renders at runtime rather than failing to build —
//! the byte-identical-HTML regression on the Web backend is the safety net.

use super::super::html::{Attribute as HtmlAttribute, Html};

/// `Ipe.Ui.Color` = `Rgba Int Int Int Float` (R/G/B 0-255 ints, alpha 0..1).
#[derive(Clone, Debug, PartialEq)]
pub enum Color {
    Rgba(i64, i64, i64, f64),
}

impl Color {
    /// Render this colour to its CSS value string. The single renderer for the
    /// `Ipe.Ui.Color` domain: the inline-style path, the stylesheet path, the
    /// `Ui.colorCss` kernel, and the gradient/shadow/pseudo builders all route
    /// here, so a colour formats identically wherever it lands.
    #[must_use]
    pub(crate) fn css(&self) -> String {
        match self {
            Self::Rgba(r, g, b, a) => format!("rgba({r},{g},{b},{a})"),
        }
    }
}

/// `Ipe.Ui.Length`. `Min`/`Max` are self-recursive → `Box` (E0072 otherwise).
#[derive(Clone, Debug, PartialEq)]
pub enum Length {
    Px(i64),
    Content,
    Fill(i64),
    Min(i64, Box<Length>),
    Max(i64, Box<Length>),
    Vh(i64),
    Vw(i64),
}

impl Length {
    /// Render this length to its CSS value string. The single renderer for the
    /// `Ipe.Ui.Length` domain, shared by the inline-style and stylesheet paths.
    ///
    /// `Fill(n)` renders `100%`; the flex sizing (`flex-grow:n`, `flex-basis:0`)
    /// that divides free space is emitted at the width/height attribute arms,
    /// not here.
    #[must_use]
    pub(crate) fn css(&self) -> String {
        match self {
            Self::Px(n) => format!("{n}px"),
            Self::Content => "auto".to_owned(),
            Self::Fill(_) => "100%".to_owned(),
            Self::Min(n, inner) => format!("min({}px,{})", n, inner.css()),
            Self::Max(n, inner) => format!("max({}px,{})", n, inner.css()),
            Self::Vh(n) => format!("{n}vh"),
            Self::Vw(n) => format!("{n}vw"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HAlign {
    AlignLeft,
    CenterX,
    AlignRight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VAlign {
    AlignTop,
    CenterY,
    AlignBottom,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Location {
    Above,
    Below,
    OnRight,
    OnLeft,
    InFront,
    Behind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PseudoClass {
    Hover,
    Focus,
    FocusVisible,
    Active,
    Disabled,
}

impl PseudoClass {
    /// Stable wire tag consumed by
    /// `ipe_runtime::web::style_inject::pseudo_selector_for_tag` when
    /// decoding the `data-ipe-pc-rules` marker attribute. MUST stay in
    /// lock-step with that function and with `pseudoClassTag` in
    /// `../ipe`'s `Ipe.Ui.ipe` (the shared wire-format contract).
    #[must_use]
    pub const fn wire_tag(self) -> &'static str {
        match self {
            Self::Hover => "h",
            Self::Focus => "f",
            Self::FocusVisible => "v",
            Self::Active => "a",
            Self::Disabled => "d",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Description {
    NoDescription,
    DescMain,
    DescNavigation,
    DescContentInfo,
    DescComplementary,
    DescHeading(i64),
    DescLabel(String),
    DescLivePolite,
    DescLiveAssertive,
    DescButton,
    DescParagraph,
}

/// `Ipe.Ui.LayoutContext` — the flex direction a parent imposes on its children.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayoutContext {
    AsRow,
    AsColumn,
    AsEl,
    AsParagraph,
    AsTextColumn,
}

/// `Ipe.Ui.Attribute msg` — the typed layout/style/event attributes. Variant
/// order matches `Ipe.Ui.ipe:55-123` EXACTLY. `AttrEvent any` carries the
/// `Ipe.Html.Attributes.Attribute` (the codegen's existing any-carrier mapping);
/// `AttrNearby` is self-referential through `Element<M>`.
#[derive(Clone, Debug, PartialEq)]
pub enum Attribute<M> {
    NoAttribute,
    AttrWidth(Length),
    AttrHeight(Length),
    AttrAlignX(HAlign),
    AttrAlignY(VAlign),
    AttrNearby(Location, Element<M>),
    AttrPadding(i64, i64, i64, i64),
    AttrSpacing(i64),
    AttrStyle(String, String),
    AttrDescribe(Description),
    AttrClass(String),
    AttrEvent(HtmlAttribute<M>),
    /// `Ui.htmlAttribute name value` escape hatch — an arbitrary, possibly
    /// attacker-derived attribute name + value. The TUI renderer (the only
    /// current sink) emits ANSI cells, so there is no markup-injection surface.
    /// SECURITY CONTRACT for the Ipe.Ui→HTML lowering: an `AttrAttribute`
    /// MUST be lowered to a `html::Attribute::Attr` and emitted through
    /// `html::render` (whose `render_into_ctx` gates every name via
    /// `SafeAttrName` and every URL value via `sanitise_url_attr`). A bespoke
    /// renderer that bypasses that path would reintroduce the `onerror=` /
    /// `href="javascript:"` XSS class — do not write one.
    AttrAttribute(String, String),
    AttrFontSize(i64),
    AttrFontColor(Color),
    AttrFontFamily(String),
    AttrFontWeight(i64),
    AttrFontItalic,
    AttrFontUnderline,
    AttrFontDecoration(String),
    AttrFontLetterSpacing(f64),
    AttrFontWordSpacing(f64),
    AttrFontAlign(String),
    AttrBgColor(Color),
    AttrBgImage(String),
    AttrBgGradient(String),
    AttrBorderWidth(i64),
    AttrBorderWidthEach(i64, i64, i64, i64),
    AttrBorderColor(Color),
    AttrBorderRounded(i64),
    AttrBorderStyle(String),
    AttrBorderShadow(i64, i64, i64, i64, Color),
    AttrBorderInsetShadow(i64, i64, i64, i64, Color),
    AttrPointer,
    AttrOverflow(String, String),
    AttrPseudoRule(PseudoClass, String),
    AttrTransition(String, bool),
    AttrGridTracks(String, String),
    AttrAnimation(String, String, String, bool),
}

/// `Ipe.Ui.Element msg` — the layout tree. Variant order matches
/// `Ipe.Ui.ipe:39-53`. `Raw any` carries a `Ipe.Html` node (the codegen's
/// any-carrier mapping) so user code can drop native HTML into the tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Element<M> {
    Empty,
    Text(String),
    Node(Description, Vec<Attribute<M>>, Vec<Element<M>>),
    TaggedNode(String, Description, Vec<Attribute<M>>, Vec<Element<M>>),
    Raw(Html<M>),
    /// `Ui.cells`: a raw terminal cell grid (rows of characters), painted
    /// verbatim by the terminal backend and embeddable as an island inside an
    /// otherwise-structured `Ipe.Ui` view under `Terminal.appScreen`.
    Cells(Vec<Vec<char>>),
}

// ─── IpeStringify for the Ipe.Ui runtime types ──────────────────────────────
// errorToString / Ipe.Test.debugShow can reach these when a generated Ipe.Ui
// type (e.g. an Input config record) or an app Model carries them as a field:
// the codegen-emitted `ipe_show` recurses into EVERY field, so each runtime type
// a generated type can hold must impl the trait or the generated impl fails to
// compile (E0599). These UI values have no Go `%v` analogue worth matching (and
// no example stringifies one), so a stable type-tag placeholder is the total,
// correct rendering — never panics, never recurses into the `M` payload.
impl crate::stringify::IpeStringify for Color {
    fn ipe_show(&self) -> String {
        "<color>".to_string()
    }
}
impl crate::stringify::IpeStringify for Length {
    fn ipe_show(&self) -> String {
        "<length>".to_string()
    }
}
impl crate::stringify::IpeStringify for HAlign {
    fn ipe_show(&self) -> String {
        "<halign>".to_string()
    }
}
impl crate::stringify::IpeStringify for VAlign {
    fn ipe_show(&self) -> String {
        "<valign>".to_string()
    }
}
impl crate::stringify::IpeStringify for Location {
    fn ipe_show(&self) -> String {
        "<location>".to_string()
    }
}
impl crate::stringify::IpeStringify for PseudoClass {
    fn ipe_show(&self) -> String {
        "<pseudo-class>".to_string()
    }
}
impl crate::stringify::IpeStringify for Description {
    fn ipe_show(&self) -> String {
        "<description>".to_string()
    }
}
impl crate::stringify::IpeStringify for LayoutContext {
    fn ipe_show(&self) -> String {
        "<layout-context>".to_string()
    }
}
impl<M> crate::stringify::IpeStringify for Attribute<M> {
    fn ipe_show(&self) -> String {
        "<ui-attribute>".to_string()
    }
}
impl<M> crate::stringify::IpeStringify for Element<M> {
    fn ipe_show(&self) -> String {
        "<element>".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SSOT: the inline-style path, the `Ui.colorCss` kernel, and the direct
    // `Color::css` renderer must format one colour byte-identically. A second
    // renderer for the same value would break exactly this assertion.
    #[test]
    fn colour_renders_identically_across_paths() {
        let c = Color::Rgba(18, 52, 86, 0.5);
        let direct = c.css();
        let kernel = super::super::helpers::ui_color_css_(c.clone());

        enum Msg {}
        let style =
            super::super::render::build_style_string(&[Attribute::<Msg>::AttrBgColor(c.clone())]);

        assert_eq!(direct, "rgba(18,52,86,0.5)");
        assert_eq!(kernel, direct);
        assert_eq!(style, format!("background-color:{direct}"));
    }

    // SSOT: a length formats identically through the inline-style path and the
    // direct `Length::css` renderer, including the recursive `Min`/`Max` arms.
    #[test]
    fn length_renders_identically_across_paths() {
        let len = Length::Max(320, Box::new(Length::Vh(80)));
        let direct = len.css();

        enum Msg {}
        let style =
            super::super::render::build_style_string(&[Attribute::<Msg>::AttrWidth(len.clone())]);

        assert_eq!(direct, "max(320px,80vh)");
        assert_eq!(style, format!("width:{direct}"));
    }
}

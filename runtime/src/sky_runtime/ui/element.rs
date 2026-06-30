//! Shared `Std.Ui` element tree — the general UI abstraction.
//!
//! These types mirror `sky-stdlib/Std/Ui.sky`'s ADTs **variant-for-variant and
//! field-for-field**. They live in the runtime (not generated per-project) so
//! that every backend — Sky.Live (→ HTML), Sky.Tui (→ ANSI cells), Sky.Webview
//! (→ native webview) — renders the SAME structured `Element` tree to its own
//! target, exactly as the Go backend does (`runtime-go/rt/tui_ui.go` walks the
//! structured Element ADT directly; it never round-trips through CSS).
//!
//! The Rust codegen maps the Sky `Std.Ui.*` types onto these via
//! `runtimeOpaqueTypes` (the same `{M}` mechanism that makes `Html` a shared
//! type), so `Std.Ui.column` etc. construct `sky_runtime::ui::Element` and the
//! pure-Sky render chain (`renderElement` → `Html`) pattern-matches them.
//!
//! INVARIANT (load-bearing): the variant names + field order MUST stay identical
//! to `Std.Ui.sky:39-190`. The opaque alias hides any drift from the Rust
//! compiler, so a mismatch mis-renders at runtime rather than failing to build —
//! the byte-identical-HTML regression on the Live backend is the safety net.

use super::super::html::{Attribute as HtmlAttribute, Html};

/// `Std.Ui.Color` = `Rgba Int Int Int Float` (R/G/B 0-255 ints, alpha 0..1).
#[derive(Clone, Debug, PartialEq)]
pub enum Color {
    Rgba(i64, i64, i64, f64),
}

/// `Std.Ui.Length`. `Min`/`Max` are self-recursive → `Box` (E0072 otherwise).
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

/// `Std.Ui.LayoutContext` — the flex direction a parent imposes on its children.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayoutContext {
    AsRow,
    AsColumn,
    AsEl,
    AsParagraph,
    AsTextColumn,
}

/// `Std.Ui.Attribute msg` — the typed layout/style/event attributes. Variant
/// order matches `Std.Ui.sky:55-123` EXACTLY. `AttrEvent any` carries the
/// `Std.Html.Attributes.Attribute` (the codegen's existing any-carrier mapping);
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
    AttrAnimation(String, String, String, bool),
}

/// `Std.Ui.Element msg` — the layout tree. Variant order matches
/// `Std.Ui.sky:39-53`. `Raw any` carries a `Std.Html` node (the codegen's
/// any-carrier mapping) so user code can drop native HTML into the tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Element<M> {
    Empty,
    Text(String),
    Node(Description, Vec<Attribute<M>>, Vec<Element<M>>),
    TaggedNode(String, Description, Vec<Attribute<M>>, Vec<Element<M>>),
    Raw(Html<M>),
}

// ─── SkyStringify for the Std.Ui runtime types ──────────────────────────────
// errorToString / Sky.Test.debugShow can reach these when a generated Std.Ui
// type (e.g. an Input config record) or an app Model carries them as a field:
// the codegen-emitted `sky_show` recurses into EVERY field, so each runtime type
// a generated type can hold must impl the trait or the generated impl fails to
// compile (E0599). These UI values have no Go `%v` analogue worth matching (and
// no example stringifies one), so a stable type-tag placeholder is the total,
// correct rendering — never panics, never recurses into the `M` payload.
impl crate::sky_runtime::stringify::SkyStringify for Color {
    fn sky_show(&self) -> String {
        "<color>".to_string()
    }
}
impl crate::sky_runtime::stringify::SkyStringify for Length {
    fn sky_show(&self) -> String {
        "<length>".to_string()
    }
}
impl crate::sky_runtime::stringify::SkyStringify for HAlign {
    fn sky_show(&self) -> String {
        "<halign>".to_string()
    }
}
impl crate::sky_runtime::stringify::SkyStringify for VAlign {
    fn sky_show(&self) -> String {
        "<valign>".to_string()
    }
}
impl crate::sky_runtime::stringify::SkyStringify for Location {
    fn sky_show(&self) -> String {
        "<location>".to_string()
    }
}
impl crate::sky_runtime::stringify::SkyStringify for PseudoClass {
    fn sky_show(&self) -> String {
        "<pseudo-class>".to_string()
    }
}
impl crate::sky_runtime::stringify::SkyStringify for Description {
    fn sky_show(&self) -> String {
        "<description>".to_string()
    }
}
impl crate::sky_runtime::stringify::SkyStringify for LayoutContext {
    fn sky_show(&self) -> String {
        "<layout-context>".to_string()
    }
}
impl<M> crate::sky_runtime::stringify::SkyStringify for Attribute<M> {
    fn sky_show(&self) -> String {
        "<ui-attribute>".to_string()
    }
}
impl<M> crate::sky_runtime::stringify::SkyStringify for Element<M> {
    fn sky_show(&self) -> String {
        "<element>".to_string()
    }
}

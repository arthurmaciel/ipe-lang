//! Inert static-`Ipe.Ui`-subtree template + its materializer.
//!
//! The `Ipe.Ui` analogue of [`crate::web::template`]: a [`UiTemplate`] is a
//! fully-static `Ipe.Ui` `Element` subtree reduced to data — the structural
//! element variants (`Node` / `TaggedNode` / `Text` / `Empty`) and the inert,
//! non-logic attribute variants only. [`materialize_ui_template`] rebuilds an
//! [`Element`] tree from a [`UiTemplate`] through the SAME `Element` / `Attribute`
//! constructors the normal render path builds, so a materialized template feeds
//! the identical `render_element` chain and renders byte-identically to the
//! original compiled subtree — dev == prod by construction.
//!
//! Inert by construction (make-invalid-states-unrepresentable): the type has NO
//! variant for an event handler (`AttrEvent`), for raw embedded HTML (`Raw`),
//! for a nearby overlay (`AttrNearby`, which nests a whole `Element` sub-view),
//! or for a raw terminal cell grid (`Cells`). A `UiTemplate` therefore cannot
//! carry logic, a `Msg`, or un-escaped markup — its only payloads are `String`s
//! and numbers that the render path style-encodes, name-gates, or escapes
//! exactly as it does a compiled literal. There is no code path, including
//! deserialization, by which a `UiTemplate` yields a handler or unescaped HTML.

use super::element::{Attribute, Color, Description, Element, HAlign, Length, PseudoClass, VAlign};

/// The maximum template nesting depth accepted on decode and descended on
/// materialize. Shares the render/diff ceiling ([`crate::html::MAX_HTML_DEPTH`])
/// as a single source of truth, exactly as [`crate::web::template`] does: a
/// template can never describe a tree deeper than the renderer will walk, so
/// materialize and render agree on the bound.
pub const MAX_UI_TEMPLATE_DEPTH: usize = crate::html::MAX_HTML_DEPTH;

/// `Ipe.Ui.Color` reduced to inert data. Mirrors [`Color`] field-for-field —
/// the single `Rgba` shape — so materialize rebuilds the exact `Color`.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UiColor {
    pub r: i64,
    pub g: i64,
    pub b: i64,
    pub a: f64,
}

impl UiColor {
    fn from_color(c: &Color) -> Self {
        match c {
            Color::Rgba(r, g, b, a) => Self {
                r: *r,
                g: *g,
                b: *b,
                a: *a,
            },
        }
    }

    fn to_color(&self) -> Color {
        Color::Rgba(self.r, self.g, self.b, self.a)
    }
}

/// `Ipe.Ui.Length` reduced to inert data. Mirrors [`Length`] variant-for-variant
/// (including the self-recursive `Min` / `Max`), so materialize rebuilds the
/// exact `Length` the render path formats.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UiLength {
    Px(i64),
    Content,
    Fill(i64),
    Min(i64, Box<UiLength>),
    Max(i64, Box<UiLength>),
    Vh(i64),
    Vw(i64),
}

impl UiLength {
    fn from_length(l: &Length) -> Self {
        match l {
            Length::Px(n) => Self::Px(*n),
            Length::Content => Self::Content,
            Length::Fill(n) => Self::Fill(*n),
            Length::Min(n, inner) => Self::Min(*n, Box::new(Self::from_length(inner))),
            Length::Max(n, inner) => Self::Max(*n, Box::new(Self::from_length(inner))),
            Length::Vh(n) => Self::Vh(*n),
            Length::Vw(n) => Self::Vw(*n),
        }
    }

    fn to_length(&self) -> Length {
        match self {
            Self::Px(n) => Length::Px(*n),
            Self::Content => Length::Content,
            Self::Fill(n) => Length::Fill(*n),
            Self::Min(n, inner) => Length::Min(*n, Box::new(inner.to_length())),
            Self::Max(n, inner) => Length::Max(*n, Box::new(inner.to_length())),
            Self::Vh(n) => Length::Vh(*n),
            Self::Vw(n) => Length::Vw(*n),
        }
    }
}

/// `Ipe.Ui.HAlign` reduced to inert data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UiHAlign {
    AlignLeft,
    CenterX,
    AlignRight,
}

/// `Ipe.Ui.VAlign` reduced to inert data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UiVAlign {
    AlignTop,
    CenterY,
    AlignBottom,
}

/// `Ipe.Ui.PseudoClass` reduced to inert data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UiPseudoClass {
    Hover,
    Focus,
    FocusVisible,
    Active,
    Disabled,
}

impl UiPseudoClass {
    fn from_pc(pc: PseudoClass) -> Self {
        match pc {
            PseudoClass::Hover => Self::Hover,
            PseudoClass::Focus => Self::Focus,
            PseudoClass::FocusVisible => Self::FocusVisible,
            PseudoClass::Active => Self::Active,
            PseudoClass::Disabled => Self::Disabled,
        }
    }

    fn to_pc(self) -> PseudoClass {
        match self {
            Self::Hover => PseudoClass::Hover,
            Self::Focus => PseudoClass::Focus,
            Self::FocusVisible => PseudoClass::FocusVisible,
            Self::Active => PseudoClass::Active,
            Self::Disabled => PseudoClass::Disabled,
        }
    }
}

/// `Ipe.Ui.Description` (the ARIA role) reduced to inert data. Mirrors
/// [`Description`] variant-for-variant.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UiDescription {
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

impl UiDescription {
    fn from_desc(d: &Description) -> Self {
        match d {
            Description::NoDescription => Self::NoDescription,
            Description::DescMain => Self::DescMain,
            Description::DescNavigation => Self::DescNavigation,
            Description::DescContentInfo => Self::DescContentInfo,
            Description::DescComplementary => Self::DescComplementary,
            Description::DescHeading(n) => Self::DescHeading(*n),
            Description::DescLabel(s) => Self::DescLabel(s.clone()),
            Description::DescLivePolite => Self::DescLivePolite,
            Description::DescLiveAssertive => Self::DescLiveAssertive,
            Description::DescButton => Self::DescButton,
            Description::DescParagraph => Self::DescParagraph,
        }
    }

    fn to_desc(&self) -> Description {
        match self {
            Self::NoDescription => Description::NoDescription,
            Self::DescMain => Description::DescMain,
            Self::DescNavigation => Description::DescNavigation,
            Self::DescContentInfo => Description::DescContentInfo,
            Self::DescComplementary => Description::DescComplementary,
            Self::DescHeading(n) => Description::DescHeading(*n),
            Self::DescLabel(s) => Description::DescLabel(s.clone()),
            Self::DescLivePolite => Description::DescLivePolite,
            Self::DescLiveAssertive => Description::DescLiveAssertive,
            Self::DescButton => Description::DescButton,
            Self::DescParagraph => Description::DescParagraph,
        }
    }
}

/// An inert, static `Ipe.Ui` attribute — the non-logic subset of
/// [`Attribute`]. Each variant mirrors an `Attribute` variant that carries ONLY
/// inert style/layout data (strings, numbers, and the reduced enums above).
///
/// Deliberately absent (the security guarantee, enforced by the type):
/// - `AttrEvent` — an event handler is logic, never inert data;
/// - `AttrNearby` — nests a whole `Element` sub-view (an overlay), out of the
///   flat static-attribute scope of a template;
/// - `AttrExplain` — a debug-only outline toggle, excluded to keep the inert
///   set to render-affecting appearance data.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UiTemplateAttr {
    NoAttribute,
    Width(UiLength),
    Height(UiLength),
    AlignX(UiHAlign),
    AlignY(UiVAlign),
    Padding(i64, i64, i64, i64),
    Spacing(i64),
    Style(String, String),
    Describe(UiDescription),
    Class(String),
    Attribute(String, String),
    FontSize(i64),
    FontColor(UiColor),
    FontFamily(String),
    FontWeight(i64),
    FontItalic,
    FontUnderline,
    FontDecoration(String),
    FontLetterSpacing(f64),
    FontWordSpacing(f64),
    FontAlign(String),
    BgColor(UiColor),
    BgImage(String),
    BgGradient(String),
    BorderWidth(i64),
    BorderWidthEach(i64, i64, i64, i64),
    BorderColor(UiColor),
    BorderRounded(i64),
    BorderStyle(String),
    BorderShadow(i64, i64, i64, i64, UiColor),
    BorderInsetShadow(i64, i64, i64, i64, UiColor),
    Pointer,
    Overflow(String, String),
    PseudoRule(UiPseudoClass, String),
    Transition(String, bool),
    GridTracks(String, String),
    Animation(String, String, String, bool),
}

impl UiTemplateAttr {
    /// Reduce an inert [`Attribute`] to a [`UiTemplateAttr`], or `None` when the
    /// attribute carries logic (`AttrEvent`), a nested sub-view (`AttrNearby`),
    /// or is the debug-only `AttrExplain`. Fail-closed: a refused attribute
    /// keeps the whole subtree compiled rather than dropping the attribute.
    fn from_attr<M>(attr: &Attribute<M>) -> Option<Self> {
        Some(match attr {
            Attribute::NoAttribute => Self::NoAttribute,
            Attribute::AttrWidth(l) => Self::Width(UiLength::from_length(l)),
            Attribute::AttrHeight(l) => Self::Height(UiLength::from_length(l)),
            Attribute::AttrAlignX(h) => Self::AlignX(match h {
                HAlign::AlignLeft => UiHAlign::AlignLeft,
                HAlign::CenterX => UiHAlign::CenterX,
                HAlign::AlignRight => UiHAlign::AlignRight,
            }),
            Attribute::AttrAlignY(v) => Self::AlignY(match v {
                VAlign::AlignTop => UiVAlign::AlignTop,
                VAlign::CenterY => UiVAlign::CenterY,
                VAlign::AlignBottom => UiVAlign::AlignBottom,
            }),
            Attribute::AttrPadding(t, r, b, l) => Self::Padding(*t, *r, *b, *l),
            Attribute::AttrSpacing(n) => Self::Spacing(*n),
            Attribute::AttrStyle(k, v) => Self::Style(k.clone(), v.clone()),
            Attribute::AttrDescribe(d) => Self::Describe(UiDescription::from_desc(d)),
            Attribute::AttrClass(c) => Self::Class(c.clone()),
            Attribute::AttrAttribute(k, v) => Self::Attribute(k.clone(), v.clone()),
            Attribute::AttrFontSize(n) => Self::FontSize(*n),
            Attribute::AttrFontColor(c) => Self::FontColor(UiColor::from_color(c)),
            Attribute::AttrFontFamily(f) => Self::FontFamily(f.clone()),
            Attribute::AttrFontWeight(n) => Self::FontWeight(*n),
            Attribute::AttrFontItalic => Self::FontItalic,
            Attribute::AttrFontUnderline => Self::FontUnderline,
            Attribute::AttrFontDecoration(s) => Self::FontDecoration(s.clone()),
            Attribute::AttrFontLetterSpacing(v) => Self::FontLetterSpacing(*v),
            Attribute::AttrFontWordSpacing(v) => Self::FontWordSpacing(*v),
            Attribute::AttrFontAlign(s) => Self::FontAlign(s.clone()),
            Attribute::AttrBgColor(c) => Self::BgColor(UiColor::from_color(c)),
            Attribute::AttrBgImage(s) => Self::BgImage(s.clone()),
            Attribute::AttrBgGradient(s) => Self::BgGradient(s.clone()),
            Attribute::AttrBorderWidth(n) => Self::BorderWidth(*n),
            Attribute::AttrBorderWidthEach(t, r, b, l) => Self::BorderWidthEach(*t, *r, *b, *l),
            Attribute::AttrBorderColor(c) => Self::BorderColor(UiColor::from_color(c)),
            Attribute::AttrBorderRounded(n) => Self::BorderRounded(*n),
            Attribute::AttrBorderStyle(s) => Self::BorderStyle(s.clone()),
            Attribute::AttrBorderShadow(a, b, c, d, col) => {
                Self::BorderShadow(*a, *b, *c, *d, UiColor::from_color(col))
            }
            Attribute::AttrBorderInsetShadow(a, b, c, d, col) => {
                Self::BorderInsetShadow(*a, *b, *c, *d, UiColor::from_color(col))
            }
            Attribute::AttrPointer => Self::Pointer,
            Attribute::AttrOverflow(x, y) => Self::Overflow(x.clone(), y.clone()),
            Attribute::AttrPseudoRule(pc, rule) => {
                Self::PseudoRule(UiPseudoClass::from_pc(*pc), rule.clone())
            }
            Attribute::AttrTransition(s, respect) => Self::Transition(s.clone(), *respect),
            Attribute::AttrGridTracks(c, r) => Self::GridTracks(c.clone(), r.clone()),
            Attribute::AttrAnimation(n, tail, body, respect) => {
                Self::Animation(n.clone(), tail.clone(), body.clone(), *respect)
            }
            // Logic (a handler), a nested sub-view overlay, or the debug-only
            // outline toggle — not inert static attribute data. Refuse: keep the
            // subtree compiled.
            Attribute::AttrEvent(_) | Attribute::AttrNearby(..) | Attribute::AttrExplain => {
                return None;
            }
        })
    }

    /// Rebuild the exact [`Attribute`] this inert form was reduced from, through
    /// the same variant the normal builders produce, so the render path formats
    /// it byte-identically. `M` is free — a `UiTemplateAttr` carries no `Msg`.
    fn to_attr<M>(&self) -> Attribute<M> {
        match self {
            Self::NoAttribute => Attribute::NoAttribute,
            Self::Width(l) => Attribute::AttrWidth(l.to_length()),
            Self::Height(l) => Attribute::AttrHeight(l.to_length()),
            Self::AlignX(h) => Attribute::AttrAlignX(match h {
                UiHAlign::AlignLeft => HAlign::AlignLeft,
                UiHAlign::CenterX => HAlign::CenterX,
                UiHAlign::AlignRight => HAlign::AlignRight,
            }),
            Self::AlignY(v) => Attribute::AttrAlignY(match v {
                UiVAlign::AlignTop => VAlign::AlignTop,
                UiVAlign::CenterY => VAlign::CenterY,
                UiVAlign::AlignBottom => VAlign::AlignBottom,
            }),
            Self::Padding(t, r, b, l) => Attribute::AttrPadding(*t, *r, *b, *l),
            Self::Spacing(n) => Attribute::AttrSpacing(*n),
            Self::Style(k, v) => Attribute::AttrStyle(k.clone(), v.clone()),
            Self::Describe(d) => Attribute::AttrDescribe(d.to_desc()),
            Self::Class(c) => Attribute::AttrClass(c.clone()),
            Self::Attribute(k, v) => Attribute::AttrAttribute(k.clone(), v.clone()),
            Self::FontSize(n) => Attribute::AttrFontSize(*n),
            Self::FontColor(c) => Attribute::AttrFontColor(c.to_color()),
            Self::FontFamily(f) => Attribute::AttrFontFamily(f.clone()),
            Self::FontWeight(n) => Attribute::AttrFontWeight(*n),
            Self::FontItalic => Attribute::AttrFontItalic,
            Self::FontUnderline => Attribute::AttrFontUnderline,
            Self::FontDecoration(s) => Attribute::AttrFontDecoration(s.clone()),
            Self::FontLetterSpacing(v) => Attribute::AttrFontLetterSpacing(*v),
            Self::FontWordSpacing(v) => Attribute::AttrFontWordSpacing(*v),
            Self::FontAlign(s) => Attribute::AttrFontAlign(s.clone()),
            Self::BgColor(c) => Attribute::AttrBgColor(c.to_color()),
            Self::BgImage(s) => Attribute::AttrBgImage(s.clone()),
            Self::BgGradient(s) => Attribute::AttrBgGradient(s.clone()),
            Self::BorderWidth(n) => Attribute::AttrBorderWidth(*n),
            Self::BorderWidthEach(t, r, b, l) => Attribute::AttrBorderWidthEach(*t, *r, *b, *l),
            Self::BorderColor(c) => Attribute::AttrBorderColor(c.to_color()),
            Self::BorderRounded(n) => Attribute::AttrBorderRounded(*n),
            Self::BorderStyle(s) => Attribute::AttrBorderStyle(s.clone()),
            Self::BorderShadow(a, b, c, d, col) => {
                Attribute::AttrBorderShadow(*a, *b, *c, *d, col.to_color())
            }
            Self::BorderInsetShadow(a, b, c, d, col) => {
                Attribute::AttrBorderInsetShadow(*a, *b, *c, *d, col.to_color())
            }
            Self::Pointer => Attribute::AttrPointer,
            Self::Overflow(x, y) => Attribute::AttrOverflow(x.clone(), y.clone()),
            Self::PseudoRule(pc, rule) => Attribute::AttrPseudoRule(pc.to_pc(), rule.clone()),
            Self::Transition(s, respect) => Attribute::AttrTransition(s.clone(), *respect),
            Self::GridTracks(c, r) => Attribute::AttrGridTracks(c.clone(), r.clone()),
            Self::Animation(n, tail, body, respect) => {
                Attribute::AttrAnimation(n.clone(), tail.clone(), body.clone(), *respect)
            }
        }
    }
}

/// An inert, fully-static `Ipe.Ui` subtree.
///
/// The variants are the ONLY shapes a static `Ipe.Ui` subtree takes: an empty
/// node, a static text node, a role-described node, or an HTML-tagged node —
/// each with inert attributes and static children. There is deliberately no
/// `Raw` variant (embedded `Html`), no `Cells` variant (a raw terminal grid),
/// and no attribute able to carry a handler — that absence is the security
/// guarantee, enforced by the type rather than a runtime check, mirroring the
/// runtime [`crate::web::template::Template`].
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UiTemplate {
    /// `Ui.none` — the empty element.
    Empty,
    /// A static text node. Rendered HTML-escaped, exactly like `Element::Text`.
    Text(String),
    /// A role-described container: `Element::Node`.
    Node {
        desc: UiDescription,
        attrs: Vec<UiTemplateAttr>,
        children: Vec<UiTemplate>,
    },
    /// An HTML-tagged container: `Element::TaggedNode`.
    TaggedNode {
        tag: String,
        desc: UiDescription,
        attrs: Vec<UiTemplateAttr>,
        children: Vec<UiTemplate>,
    },
}

impl Drop for UiTemplate {
    /// Dismantle the tree iteratively so dropping a deeply nested template can
    /// never overflow the stack. A `UiTemplate` can be decoded from untrusted
    /// wire input, so it may nest arbitrarily deep; the derived recursive drop
    /// would abort the process on such a tree. Draining each node's children
    /// onto an explicit stack keeps the destructor bounded by the heap, not the
    /// native call stack.
    fn drop(&mut self) {
        let mut pending: Vec<UiTemplate> = match self {
            UiTemplate::Node { children, .. } | UiTemplate::TaggedNode { children, .. } => {
                std::mem::take(children)
            }
            UiTemplate::Empty | UiTemplate::Text(_) => return,
        };
        while let Some(mut node) = pending.pop() {
            match &mut node {
                UiTemplate::Node { children, .. } | UiTemplate::TaggedNode { children, .. } => {
                    pending.append(&mut std::mem::take(children));
                }
                UiTemplate::Empty | UiTemplate::Text(_) => {}
            }
            // `node` (now child-free) drops here without recursion.
        }
    }
}

/// A malformed or out-of-bounds template, surfaced as a typed error rather than
/// a panic. A patched template arrives from the dev overlay transport as
/// untrusted input, so an over-deep tree is turned back here (bounded by
/// construction) instead of being allowed to exhaust the stack at materialize.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiTemplateError {
    /// The template nests deeper than [`MAX_UI_TEMPLATE_DEPTH`].
    TooDeep,
}

impl std::fmt::Display for UiTemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UiTemplateError::TooDeep => {
                write!(f, "ui template nests deeper than {MAX_UI_TEMPLATE_DEPTH}")
            }
        }
    }
}

impl std::error::Error for UiTemplateError {}

impl UiTemplate {
    /// Validate a decoded template's shape: reject a tree deeper than the render
    /// ceiling before it is materialized. Total and allocation-free — walks the
    /// existing tree without recursion, so an adversarial decode cannot overflow
    /// the stack in the check itself.
    ///
    /// Call this on any template that crossed an untrusted boundary (the dev
    /// overlay transport) before handing it to [`materialize_ui_template`].
    ///
    /// # Errors
    /// Returns [`UiTemplateError::TooDeep`] when the tree nests deeper than
    /// [`MAX_UI_TEMPLATE_DEPTH`].
    pub fn check_bounds(&self) -> Result<(), UiTemplateError> {
        // Explicit stack, not recursion: the depth check must not itself be
        // bounded by the native call stack it is meant to protect.
        let mut stack: Vec<(&UiTemplate, usize)> = vec![(self, 0)];
        while let Some((node, depth)) = stack.pop() {
            if depth >= MAX_UI_TEMPLATE_DEPTH {
                return Err(UiTemplateError::TooDeep);
            }
            if let UiTemplate::Node { children, .. } | UiTemplate::TaggedNode { children, .. } =
                node
            {
                for child in children {
                    stack.push((child, depth.saturating_add(1)));
                }
            }
        }
        Ok(())
    }
}

/// Rebuild an [`Element`] tree from a [`UiTemplate`], using the same `Element`
/// and `Attribute` constructors the normal builders emit, so the result feeds
/// the identical `render_element` chain and renders byte-identically to the
/// original compiled subtree.
///
/// Bounded by construction: descent stops at [`MAX_UI_TEMPLATE_DEPTH`] (the
/// render ceiling), so a deep template can never overflow the stack. A subtree
/// at the cap materializes to an empty element — the same "stop, don't recurse
/// further" posture the renderer takes at its own depth cap — never a panic.
///
/// The produced tree is inert by construction: no attribute is an `AttrEvent`,
/// and no node is `Raw`/`Cells` — no input can make this emit a handler or raw
/// markup.
#[must_use]
pub fn materialize_ui_template<M>(template: &UiTemplate) -> Element<M> {
    materialize_ui_at(template, 0)
}

fn materialize_ui_at<M>(template: &UiTemplate, depth: usize) -> Element<M> {
    if depth >= MAX_UI_TEMPLATE_DEPTH {
        // Same bounded-descent posture as the renderer at its cap: stop
        // descending. An empty element is inert and well-formed.
        return Element::Empty;
    }
    match template {
        UiTemplate::Empty => Element::Empty,
        UiTemplate::Text(s) => Element::Text(s.clone()),
        UiTemplate::Node {
            desc,
            attrs,
            children,
        } => Element::Node(
            desc.to_desc(),
            attrs.iter().map(UiTemplateAttr::to_attr).collect(),
            children
                .iter()
                .map(|c| materialize_ui_at(c, depth.saturating_add(1)))
                .collect(),
        ),
        UiTemplate::TaggedNode {
            tag,
            desc,
            attrs,
            children,
        } => Element::TaggedNode(
            tag.clone(),
            desc.to_desc(),
            attrs.iter().map(UiTemplateAttr::to_attr).collect(),
            children
                .iter()
                .map(|c| materialize_ui_at(c, depth.saturating_add(1)))
                .collect(),
        ),
    }
}

/// Decode a serialized [`UiTemplate`] and materialize it, through the dev
/// overlay transport (a JSON string). The string front door to
/// [`materialize_ui_template`]: the emitted `view` reads its per-view slot
/// (`__ipe_lit.get(N)`) and hands the baked-default-or-patched JSON here, so
/// prod (baked default) and dev (patched slot) run the SAME materialize path —
/// dev == prod by construction.
///
/// Fail-closed on hostile input, never a panic (the slot value crosses the
/// untrusted dev overlay boundary):
/// - a decode failure returns the inert empty element (`Element::Empty`);
/// - an over-deep decoded template ([`UiTemplate::check_bounds`]) returns the
///   same inert empty element, so a decode cannot exhaust the stack at
///   materialize.
///
/// Inert by construction: the [`UiTemplate`] type has no handler and no raw
/// variant, so no JSON — however adversarial — decodes into logic or unescaped
/// markup.
#[cfg(feature = "json")]
#[must_use]
pub fn materialize_ui_template_str<M>(json: &str) -> Element<M> {
    let Ok(template) = serde_json::from_str::<UiTemplate>(json) else {
        return Element::Empty;
    };
    if template.check_bounds().is_err() {
        return Element::Empty;
    }
    materialize_ui_template(&template)
}

/// Build a [`UiTemplate`] from a static [`Element`] subtree — the inverse of
/// [`materialize_ui_template`]. Fail-closed (parse, don't validate): any node
/// that is NOT provably static returns `None`, so a template is only ever built
/// from a subtree that materialize can reproduce byte-identically.
///
/// A node is non-static, and so refuses, when it is:
/// - `Element::Raw` (embedded `Html`, possibly un-escaped — never representable
///   in a `UiTemplate`);
/// - `Element::Cells` (a raw terminal grid, outside the structured scope);
/// - a node carrying an `AttrEvent` (a handler — logic), an `AttrNearby` (a
///   nested sub-view overlay), or an `AttrExplain` (debug outline);
/// - nested deeper than [`MAX_UI_TEMPLATE_DEPTH`].
///
/// Returns `None` in each case rather than silently dropping the offending part,
/// so the caller treats a non-templatable subtree as "keep it compiled", never
/// "template a lie".
#[must_use]
pub fn ui_template_of<M>(elem: &Element<M>) -> Option<UiTemplate> {
    ui_template_of_at(elem, 0)
}

fn ui_template_of_at<M>(elem: &Element<M>, depth: usize) -> Option<UiTemplate> {
    if depth >= MAX_UI_TEMPLATE_DEPTH {
        return None;
    }
    match elem {
        Element::Empty => Some(UiTemplate::Empty),
        Element::Text(s) => Some(UiTemplate::Text(s.clone())),
        // Embedded raw HTML and a raw terminal grid have no inert `Ipe.Ui`
        // representation — refuse rather than smuggle them.
        Element::Raw(_) | Element::Cells(_) => None,
        Element::Node(desc, attrs, children) => {
            let attrs = static_ui_attrs(attrs)?;
            let children = static_ui_children(children, depth)?;
            Some(UiTemplate::Node {
                desc: UiDescription::from_desc(desc),
                attrs,
                children,
            })
        }
        Element::TaggedNode(tag, desc, attrs, children) => {
            let attrs = static_ui_attrs(attrs)?;
            let children = static_ui_children(children, depth)?;
            Some(UiTemplate::TaggedNode {
                tag: tag.clone(),
                desc: UiDescription::from_desc(desc),
                attrs,
                children,
            })
        }
    }
}

fn static_ui_attrs<M>(attrs: &[Attribute<M>]) -> Option<Vec<UiTemplateAttr>> {
    let mut out = Vec::with_capacity(attrs.len());
    for a in attrs {
        out.push(UiTemplateAttr::from_attr(a)?);
    }
    Some(out)
}

fn static_ui_children<M>(children: &[Element<M>], depth: usize) -> Option<Vec<UiTemplate>> {
    let mut out = Vec::with_capacity(children.len());
    for c in children {
        out.push(ui_template_of_at(c, depth.saturating_add(1))?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_UI_TEMPLATE_DEPTH, UiTemplate, UiTemplateError, materialize_ui_template, ui_template_of,
    };
    use crate::ui::element::{Attribute, Color, Description, Element, Length};
    use crate::ui::render::ui_layout;

    // Render an `Element` the way the app does — through the public `ui_layout`
    // entry — to compare rendered bytes. `ui_layout(vec![], elem)` wraps the
    // element in the standard viewport shell and runs the SAME `render_element`
    // chain a live view uses.
    fn render(elem: Element<()>) -> String {
        crate::html::render_html(&ui_layout(vec![], elem))
    }

    // The dev == prod soundness proof: round-tripping a static `Ipe.Ui` subtree
    // through a `UiTemplate` and its materializer produces an `Element` that
    // renders byte-identically to rendering the original. `ui_template_of` +
    // `materialize_ui_template` compose to the identity on the rendered bytes.
    fn assert_round_trip_byte_identical(subtree: &Element<()>) {
        let template = ui_template_of(subtree).expect("static subtree must be templatable");
        let materialized: Element<()> = materialize_ui_template(&template);
        // The rebuilt `Element` is value-equal to the original (a strictly
        // stronger property than byte-identity — the exact variants are
        // reconstructed), so the render must match.
        assert_eq!(
            &materialized, subtree,
            "materialize must reconstruct the exact Element"
        );
        assert_eq!(
            render(materialized),
            render(subtree.clone()),
            "materialized ui template must render byte-identically to the original subtree"
        );
    }

    // ── acceptance: a provably-static subtree templates ──────────────────────

    #[test]
    fn round_trip_text_node() {
        assert_round_trip_byte_identical(&Element::Text("Hello".to_string()));
    }

    #[test]
    fn round_trip_empty_node() {
        assert_round_trip_byte_identical(&Element::Empty);
    }

    #[test]
    fn round_trip_row_with_inert_attrs_and_children() {
        // A `Ui.row [spacing 8, width fill] [text "a", text "b"]` shape: the
        // `__row` marker style is an inert `AttrStyle`, spacing/width are inert.
        let subtree: Element<()> = Element::Node(
            Description::NoDescription,
            vec![
                Attribute::AttrStyle("__row".to_string(), "true".to_string()),
                Attribute::AttrSpacing(8),
                Attribute::AttrWidth(Length::Fill(1)),
            ],
            vec![
                Element::Text("a".to_string()),
                Element::Text("b".to_string()),
            ],
        );
        assert_round_trip_byte_identical(&subtree);
    }

    #[test]
    fn round_trip_tagged_node_with_font_and_border() {
        let subtree: Element<()> = Element::TaggedNode(
            "section".to_string(),
            Description::DescMain,
            vec![
                Attribute::AttrPadding(4, 8, 4, 8),
                Attribute::AttrFontSize(16),
                Attribute::AttrFontColor(Color::Rgba(10, 20, 30, 1.0)),
                Attribute::AttrBorderWidth(2),
                Attribute::AttrBorderColor(Color::Rgba(0, 0, 0, 0.5)),
                Attribute::AttrClass("card".to_string()),
            ],
            vec![Element::Text("Body".to_string())],
        );
        assert_round_trip_byte_identical(&subtree);
    }

    #[test]
    fn round_trip_nested_column_of_rows() {
        let subtree: Element<()> = Element::Node(
            Description::NoDescription,
            vec![Attribute::AttrStyle(
                "__col".to_string(),
                "true".to_string(),
            )],
            vec![
                Element::Node(
                    Description::NoDescription,
                    vec![Attribute::AttrStyle(
                        "__row".to_string(),
                        "true".to_string(),
                    )],
                    vec![Element::Text("one".to_string())],
                ),
                Element::Node(
                    Description::NoDescription,
                    vec![Attribute::AttrStyle(
                        "__row".to_string(),
                        "true".to_string(),
                    )],
                    vec![Element::Text("two".to_string())],
                ),
            ],
        );
        assert_round_trip_byte_identical(&subtree);
    }

    #[test]
    fn round_trip_escaped_text_stays_escaped_no_xss() {
        let raw = r#"<script>alert("x & 'y'")</script>"#;
        let subtree: Element<()> = Element::Node(
            Description::NoDescription,
            vec![],
            vec![Element::Text(raw.to_string())],
        );
        assert_round_trip_byte_identical(&subtree);
        let template = ui_template_of(&subtree).expect("templatable");
        let rendered = render(materialize_ui_template::<()>(&template));
        assert!(
            !rendered.contains("<script>"),
            "escaped text must not yield a raw <script> tag: {rendered}"
        );
        assert!(
            rendered.contains("&lt;script&gt;"),
            "special chars must be entity-escaped: {rendered}"
        );
    }

    #[test]
    fn round_trip_special_chars_in_attribute_value() {
        let subtree: Element<()> = Element::TaggedNode(
            "a".to_string(),
            Description::NoDescription,
            vec![Attribute::AttrAttribute(
                "title".to_string(),
                r#"a "quote" & <tag>"#.to_string(),
            )],
            vec![Element::Text("link".to_string())],
        );
        assert_round_trip_byte_identical(&subtree);
    }

    // ── inert-by-construction refusals ──────────────────────────────────────

    #[test]
    fn raw_embedded_html_is_refused() {
        let subtree: Element<()> =
            Element::Raw(crate::html::Html::HRaw("<b>trusted?</b>".to_string()));
        assert_eq!(ui_template_of(&subtree), None);
    }

    #[test]
    fn cells_grid_is_refused() {
        let subtree: Element<()> = Element::Cells(vec![vec!['a', 'b']]);
        assert_eq!(ui_template_of(&subtree), None);
    }

    #[test]
    fn event_handler_attribute_is_refused() {
        let subtree: Element<i32> = Element::TaggedNode(
            "button".to_string(),
            Description::NoDescription,
            vec![Attribute::AttrEvent(crate::html::Attribute::EventAttr(
                crate::html::Event::OnMsg("click".to_string(), 1),
            ))],
            vec![Element::Text("+".to_string())],
        );
        assert_eq!(ui_template_of(&subtree), None);
    }

    #[test]
    fn nearby_overlay_attribute_is_refused() {
        let subtree: Element<()> = Element::Node(
            Description::NoDescription,
            vec![Attribute::AttrNearby(
                crate::ui::element::Location::Above,
                Element::Text("tooltip".to_string()),
            )],
            vec![],
        );
        assert_eq!(ui_template_of(&subtree), None);
    }

    #[test]
    fn explain_debug_attribute_is_refused() {
        let subtree: Element<()> = Element::Node(
            Description::NoDescription,
            vec![Attribute::AttrExplain],
            vec![],
        );
        assert_eq!(ui_template_of(&subtree), None);
    }

    #[test]
    fn handler_nested_in_child_refuses_whole_subtree() {
        // A static wrapper around a handler-bearing child must refuse whole —
        // never template the wrapper and drop the child's logic.
        let subtree: Element<i32> = Element::Node(
            Description::NoDescription,
            vec![],
            vec![Element::TaggedNode(
                "button".to_string(),
                Description::NoDescription,
                vec![Attribute::AttrEvent(crate::html::Attribute::EventAttr(
                    crate::html::Event::OnMsg("click".to_string(), 7),
                ))],
                vec![],
            )],
        );
        assert_eq!(ui_template_of(&subtree), None);
    }

    // ── bounded-by-construction decode ──────────────────────────────────────

    #[test]
    fn over_deep_template_fails_bounds_check() {
        let mut node = UiTemplate::Text("x".to_string());
        for _ in 0..=MAX_UI_TEMPLATE_DEPTH {
            node = UiTemplate::Node {
                desc: super::UiDescription::NoDescription,
                attrs: vec![],
                children: vec![node],
            };
        }
        assert_eq!(node.check_bounds(), Err(UiTemplateError::TooDeep));
    }

    #[test]
    fn legal_depth_passes_bounds_check() {
        let mut node = UiTemplate::Text("x".to_string());
        for _ in 0..16 {
            node = UiTemplate::Node {
                desc: super::UiDescription::NoDescription,
                attrs: vec![],
                children: vec![node],
            };
        }
        assert_eq!(node.check_bounds(), Ok(()));
    }

    // Iteratively measure an `Element` tree's nesting depth (no recursion, so the
    // measurement itself cannot overflow on a maximally deep tree).
    fn element_depth(root: &Element<()>) -> usize {
        let mut max = 0usize;
        let mut stack = vec![(root, 1usize)];
        while let Some((node, depth)) = stack.pop() {
            max = max.max(depth);
            if let Element::Node(_, _, kids) | Element::TaggedNode(_, _, _, kids) = node {
                for k in kids {
                    stack.push((k, depth.saturating_add(1)));
                }
            }
        }
        max
    }

    #[test]
    fn materialize_caps_descent_at_the_ceiling() {
        // Materialize is bounded by construction: given a template far deeper
        // than the cap, descent stops at `MAX_UI_TEMPLATE_DEPTH`. Run on a
        // large-stack thread because building a ceiling-deep tree uses the
        // native stack up to the same bound.
        let handle = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                let mut node = UiTemplate::Text("x".to_string());
                for _ in 0..(MAX_UI_TEMPLATE_DEPTH + 500) {
                    node = UiTemplate::Node {
                        desc: super::UiDescription::NoDescription,
                        attrs: vec![],
                        children: vec![node],
                    };
                }
                let elem: Element<()> = materialize_ui_template(&node);
                element_depth(&elem)
            })
            .expect("spawn measuring thread");
        let depth = handle.join().expect("measuring thread must not panic");
        assert!(
            depth <= MAX_UI_TEMPLATE_DEPTH + 1,
            "materialize must cap descent at the ceiling, got depth {depth}"
        );
        assert!(
            depth >= MAX_UI_TEMPLATE_DEPTH,
            "the deep input should materialize right up to the ceiling, got {depth}"
        );
    }

    // ── serde round-trip + the string front door (dev overlay transport) ─────

    #[cfg(feature = "json")]
    #[test]
    fn serde_round_trip_preserves_template() {
        let subtree: Element<()> = Element::TaggedNode(
            "section".to_string(),
            Description::DescMain,
            vec![
                Attribute::AttrPadding(4, 8, 4, 8),
                Attribute::AttrFontColor(Color::Rgba(1, 2, 3, 0.25)),
            ],
            vec![Element::Text("hi".to_string())],
        );
        let template = ui_template_of(&subtree).expect("templatable");
        let json = serde_json::to_string(&template).expect("serialize");
        let decoded: UiTemplate = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, template);
    }

    // dev == prod at the runtime level: the baked-default JSON string (what prod
    // holds AND what the emitted `view` reads via `__ipe_lit.get(N)`)
    // materializes to an `Element` that renders byte-identically to rendering
    // the original static subtree directly.
    #[cfg(feature = "json")]
    #[test]
    fn str_materialize_matches_direct_render() {
        use super::materialize_ui_template_str;
        let subtree: Element<()> = Element::Node(
            Description::NoDescription,
            vec![Attribute::AttrStyle(
                "__col".to_string(),
                "true".to_string(),
            )],
            vec![
                Element::Text("Title".to_string()),
                Element::Text("Body".to_string()),
            ],
        );
        let template = ui_template_of(&subtree).expect("templatable");
        let json = serde_json::to_string(&template).expect("serialize");
        let via_str: Element<()> = materialize_ui_template_str(&json);
        assert_eq!(
            render(via_str),
            render(subtree),
            "materialize_ui_template_str over the baked default must render byte-identically"
        );
    }

    #[cfg(feature = "json")]
    #[test]
    fn str_materialize_reflects_a_structural_edit() {
        use super::materialize_ui_template_str;
        let after: Element<()> = Element::Node(
            Description::NoDescription,
            vec![],
            vec![
                Element::Text("one".to_string()),
                Element::Text("two".to_string()),
            ],
        );
        let before: Element<()> = Element::Node(
            Description::NoDescription,
            vec![],
            vec![Element::Text("one".to_string())],
        );
        let json_after =
            serde_json::to_string(&ui_template_of(&after).expect("templatable")).unwrap();
        let materialized: Element<()> = materialize_ui_template_str(&json_after);
        assert_eq!(render(materialized), render(after));
        assert_ne!(
            render(materialize_ui_template_str::<()>(&json_after)),
            render(before)
        );
    }

    #[cfg(feature = "json")]
    #[test]
    fn str_materialize_malformed_json_is_inert_empty() {
        use super::materialize_ui_template_str;
        let out: Element<()> = materialize_ui_template_str("this is not json");
        assert_eq!(out, Element::Empty);
        // A payload naming a handler/raw variant simply fails to decode — there
        // is no inert-data path to logic or raw markup.
        let bogus: Element<()> =
            materialize_ui_template_str(r#"{"Raw":"<script>evil()</script>"}"#);
        assert_eq!(out, bogus);
    }

    #[cfg(feature = "json")]
    #[test]
    fn str_materialize_keeps_text_escaped() {
        use super::materialize_ui_template_str;
        let json = r#"{"Text":"<script>alert(1)</script>"}"#;
        let out: Element<()> = materialize_ui_template_str(json);
        let rendered = render(out);
        assert!(
            !rendered.contains("<script>"),
            "must stay escaped: {rendered}"
        );
        assert!(rendered.contains("&lt;script&gt;"));
    }

    // dev == prod at the compiler/runtime seam: the EXACT JSON the backend
    // serializer (`ipe_backend_rust::emit_ui_template::CompileUiTemplate::to_json`)
    // bakes must decode into a `UiTemplate` equal to the tree it described, so
    // the emitted baked default materializes byte-identically to the direct
    // inline emit. This literal is pinned identically on the backend side
    // (`json_tagged_node_full_shape`); a drift on either side fails one of the
    // two pins.
    #[cfg(feature = "json")]
    #[test]
    fn backend_baked_json_decodes_to_the_described_tree() {
        let baked = concat!(
            r#"{"TaggedNode":{"tag":"section","desc":{"DescHeading":2},"attrs":["#,
            r#"{"Width":{"Max":[320,{"Vh":80}]}},{"Padding":[1,2,3,4]},{"Style":["k","v"]},"#,
            r#"{"AlignX":"CenterX"},"Pointer",{"FontAlign":"center"}],"#,
            r#""children":[{"Text":"hi"},"Empty"]}}"#,
        );
        let decoded: UiTemplate = serde_json::from_str(baked).expect("backend JSON decodes");
        let expected = UiTemplate::TaggedNode {
            tag: "section".to_string(),
            desc: super::UiDescription::DescHeading(2),
            attrs: vec![
                super::UiTemplateAttr::Width(super::UiLength::Max(
                    320,
                    Box::new(super::UiLength::Vh(80)),
                )),
                super::UiTemplateAttr::Padding(1, 2, 3, 4),
                super::UiTemplateAttr::Style("k".to_string(), "v".to_string()),
                super::UiTemplateAttr::AlignX(super::UiHAlign::CenterX),
                super::UiTemplateAttr::Pointer,
                super::UiTemplateAttr::FontAlign("center".to_string()),
            ],
            children: vec![UiTemplate::Text("hi".to_string()), UiTemplate::Empty],
        };
        assert_eq!(
            decoded, expected,
            "backend baked JSON must decode to the exact tree"
        );
        // And it re-serializes to the identical bytes — the backend spelling IS
        // serde_json's spelling.
        assert_eq!(serde_json::to_string(&expected).unwrap(), baked);
    }
}

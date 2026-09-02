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
use crate::html::{Attribute as HtmlAttribute, Event};

/// A per-render handler resolution map: the concrete `Msg`s a templatized
/// subtree's model-dependent event handlers evaluate to at THIS render, in the
/// stable emit order the compiler assigns each hole.
///
/// This is the server-side half of the handler-id HOLE (issue #1668). A model-
/// dependent event (`onClick (Select model.id)`) blocks its subtree from
/// templatizing as pure inert data, because the concrete `Msg` depends on the
/// model — it is logic, not appearance. Instead the template carries an opaque
/// [`UiTemplateAttr::HandlerHole`] placeholder (an event name + a small integer
/// hole id), and the compiled `view` builds this map fresh every render by
/// evaluating each captured `Msg` against the current model. Materialize then
/// resolves each hole against this map. The model-dependent part therefore lives
/// ONLY here, in the compiled per-render map — it is never serialized into the
/// inert template, never sent to the client, never a closure on the wire.
///
/// # Trust boundary (fail-closed)
///
/// The hole id is a compile-time-stable index the SERVER assigns; the client
/// never sees it and never sends it (the browser addresses handlers by DOM
/// `ipe-id`, resolved by the separate live [`crate::dispatch::HandlerIndex`]).
/// [`Self::resolve`] is nonetheless fail-closed by construction: an out-of-range
/// id (a stale template decoded against a shorter map after an edit, or any
/// forged index) resolves to `None` — the event attribute is simply not
/// reconstructed. There is no code path by which an unresolved hole yields an
/// attacker-chosen `Msg`, a `Msg` from a different render, or a panic: the map
/// is indexed, never trusted, and a miss drops the handler.
#[derive(Clone, Debug, Default)]
pub struct UiHandlerMap<M> {
    /// The captured `Msg`s in hole-id order. Index i is hole id i.
    msgs: Vec<M>,
}

impl<M: Clone> UiHandlerMap<M> {
    /// An empty map — every hole resolves to `None` (fail-closed). This is what
    /// the prod render and the map-less materialize path use when no handler
    /// captures are supplied, so a hole never fabricates a `Msg`.
    #[must_use]
    pub fn new() -> Self {
        Self { msgs: Vec::new() }
    }

    /// Build a map from the per-render captured `Msg`s, in hole-id order. The
    /// compiled `view` calls this with each model-dependent handler's evaluated
    /// `Msg` — position i is hole id i.
    #[must_use]
    pub fn from_msgs(msgs: Vec<M>) -> Self {
        Self { msgs }
    }

    /// Resolve a hole id to its captured `Msg`, or `None` when the id is out of
    /// range. Fail-closed: never panics, never returns a `Msg` for a different
    /// id, never fabricates one — a miss is a clean drop of the handler.
    #[must_use]
    pub fn resolve(&self, handler_id: u32) -> Option<M> {
        self.msgs.get(handler_id as usize).cloned()
    }
}

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
    /// A model-dependent event handler reduced to an opaque HOLE (issue #1668):
    /// only the DOM event name and a compile-time-stable hole id, NEVER the
    /// `Msg` or a closure. The concrete `Msg` is resolved per render from a
    /// [`UiHandlerMap`] the compiled `view` supplies — the model-dependent logic
    /// lives only in that server-side map, never in this inert datum. A hole
    /// carries no logic and no payload beyond two placeholders, so it cannot
    /// smuggle a handler or a `Msg` across the (untrusted) template transport.
    HandlerHole {
        event: String,
        handler_id: u32,
    },
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

    /// Reduce an inert [`Attribute`] to a [`UiTemplateAttr`], OR — for a clean
    /// model-capture `AttrEvent` (`onClick msg`) — to a [`Self::HandlerHole`]
    /// (issue #1668), pushing the captured `Msg` onto `captures` and using its
    /// index as the hole id. Returns `None` (refuses the whole subtree) for a
    /// nested-sub-view overlay, the debug outline, or an event shape that is NOT
    /// a plain model-capture — `OnString` / `OnBool` / `OnForm` / `OnWidget` all
    /// need runtime-argument-dependent resolution (a value the client sends, a
    /// form payload, a seal decode), so they are not a pure per-render `Msg`
    /// capture and stay compiled. Parse-don't-validate: only the provably-clean
    /// `OnMsg` capture becomes a hole; everything else refuses.
    fn from_attr_holed<M: Clone>(attr: &Attribute<M>, captures: &mut Vec<M>) -> Option<Self> {
        match attr {
            Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnMsg(event, msg))) => {
                let handler_id = u32::try_from(captures.len())
                    .ok()
                    .filter(|_| captures.len() < u32::MAX as usize)?;
                captures.push(msg.clone());
                Some(Self::HandlerHole {
                    event: event.clone(),
                    handler_id,
                })
            }
            // A non-`OnMsg` event, a nested overlay, or the debug outline is not
            // a clean per-render capture → refuse, keep the subtree compiled.
            Attribute::AttrEvent(_) | Attribute::AttrNearby(..) | Attribute::AttrExplain => None,
            // Every other (inert) attribute reduces exactly as the pure path.
            _ => Self::from_attr(attr),
        }
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
            // A handler hole with NO resolution map cannot reconstruct a live
            // handler (it carries no `Msg`), so it drops to `NoAttribute` —
            // fail-closed by construction. The map-aware [`Self::to_attr_with_handlers`]
            // is the path that resolves a hole against a per-render map.
            Self::HandlerHole { .. } => Attribute::NoAttribute,
        }
    }

    /// Rebuild the [`Attribute`], resolving a [`Self::HandlerHole`] against the
    /// per-render `handlers` map. Every non-hole variant is identical to
    /// [`Self::to_attr`]; a hole becomes a live `AttrEvent(EventAttr(OnMsg(..)))`
    /// bound to the map-resolved `Msg`, or `NoAttribute` when the hole id does
    /// not resolve (fail-closed — never a fabricated or cross-render `Msg`).
    fn to_attr_with_handlers<M: Clone>(&self, handlers: &UiHandlerMap<M>) -> Attribute<M> {
        match self {
            Self::HandlerHole { event, handler_id } => match handlers.resolve(*handler_id) {
                Some(msg) => {
                    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnMsg(event.clone(), msg)))
                }
                // Unknown / out-of-range hole id → no handler. The element still
                // renders (its structure is intact); it simply carries no event
                // marker for this hole. No Msg is invented.
                None => Attribute::NoAttribute,
            },
            // Every inert variant is `M`-free — reuse the map-less rebuild.
            other => other.to_attr(),
        }
    }
}

/// An inert, mostly-static `Ipe.Ui` subtree, optionally carrying numbered
/// **holes** where a `Model`-derived value is spliced in at render.
///
/// The static variants are the shapes a static `Ipe.Ui` subtree takes: an empty
/// node, a static text node, a role-described node, or an HTML-tagged node —
/// each with inert attributes and static children. There is deliberately no
/// `Raw` variant (embedded `Html`), no `Cells` variant (a raw terminal grid),
/// and no attribute able to carry a handler — that absence is the security
/// guarantee, enforced by the type rather than a runtime check, mirroring the
/// runtime [`crate::web::template::Template`].
///
/// A **hole** is an inert index (a `usize`), never logic: the model-derived
/// value it stands for is computed by the compiled `view` and passed in a
/// per-render slice, so the template datum itself carries no `Msg`, no handler,
/// and no un-escaped markup — a hole cannot smuggle logic, exactly like the
/// static variants. An out-of-range or unfilled hole materializes to the inert
/// empty element (fail-closed), never a panic. Two hole shapes:
/// - [`UiTemplate::Hole`] — one element in a single position (a value leaf or a
///   control-flow branch result);
/// - [`UiTemplate::ChildrenHole`] — a run of elements spliced into a children
///   list (a `List.map` comprehension).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UiTemplate {
    /// `Ui.none` — the empty element.
    Empty,
    /// A static text node. Rendered HTML-escaped, exactly like `Element::Text`.
    Text(String),
    /// A single-element hole: the compiled `view` supplies one `Element` for this
    /// index in the per-render fill slice. Stands for a `Model`-derived value leaf
    /// or a control-flow (`if` / `case`) result.
    Hole(usize),
    /// A children hole: the compiled `view` supplies a run of `Element`s (a
    /// `Vec`) for this index, spliced in place among a node's static children.
    /// Stands for a `List.map` comprehension.
    ChildrenHole(usize),
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
            UiTemplate::Empty
            | UiTemplate::Text(_)
            | UiTemplate::Hole(_)
            | UiTemplate::ChildrenHole(_) => return,
        };
        while let Some(mut node) = pending.pop() {
            match &mut node {
                UiTemplate::Node { children, .. } | UiTemplate::TaggedNode { children, .. } => {
                    pending.append(&mut std::mem::take(children));
                }
                UiTemplate::Empty
                | UiTemplate::Text(_)
                | UiTemplate::Hole(_)
                | UiTemplate::ChildrenHole(_) => {}
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
    // A fully-static template has no holes: an empty fill set is correct, and any
    // stray hole (there should be none) fails closed to the inert empty element.
    materialize_ui_at(template, 0, &mut HoleFills::empty())
}

/// Rebuild an [`Element`] tree from a hole-bearing [`UiTemplate`], splicing each
/// hole with its per-render fill. `element_holes[n]` fills a [`UiTemplate::Hole`]
/// with index `n`; `children_holes[n]` fills a [`UiTemplate::ChildrenHole`] with
/// index `n`. Each fill is consumed at most once (a hole index appears at most
/// once in a template); a missing or out-of-range fill materializes to the inert
/// empty element (fail-closed), never a panic.
///
/// The static structure comes from the (inert) template; only the fills carry
/// `Model`-derived content, and they are ordinary compiled `Element`s the caller
/// already built — so dev == prod: the same fills feed a baked-default template
/// (prod) and a patched-structure template (dev), and only the static skeleton
/// hot-swaps.
#[must_use]
pub fn materialize_ui_template_with_holes<M>(
    template: &UiTemplate,
    element_holes: Vec<Element<M>>,
    children_holes: Vec<Vec<Element<M>>>,
) -> Element<M> {
    let mut fills = HoleFills {
        elements: element_holes.into_iter().map(Some).collect(),
        children: children_holes.into_iter().map(Some).collect(),
    };
    materialize_ui_at(template, 0, &mut fills)
}

/// Per-render hole fills, each taken at most once. `None` marks a slot already
/// consumed (or never provided) so a duplicate/out-of-range reference fails
/// closed to the empty element rather than reusing or panicking.
struct HoleFills<M> {
    elements: Vec<Option<Element<M>>>,
    children: Vec<Option<Vec<Element<M>>>>,
}

impl<M> HoleFills<M> {
    fn empty() -> Self {
        Self {
            elements: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Take the single-element fill at `idx`, or the inert empty element when the
    /// index is out of range or already consumed. Fail-closed by construction.
    fn take_element(&mut self, idx: usize) -> Element<M> {
        self.elements
            .get_mut(idx)
            .and_then(Option::take)
            .unwrap_or(Element::Empty)
    }

    /// Take the children-run fill at `idx`, or an empty run when the index is out
    /// of range or already consumed.
    fn take_children(&mut self, idx: usize) -> Vec<Element<M>> {
        self.children
            .get_mut(idx)
            .and_then(Option::take)
            .unwrap_or_default()
    }
}

fn materialize_ui_at<M>(
    template: &UiTemplate,
    depth: usize,
    fills: &mut HoleFills<M>,
) -> Element<M> {
    if depth >= MAX_UI_TEMPLATE_DEPTH {
        // Same bounded-descent posture as the renderer at its cap: stop
        // descending. An empty element is inert and well-formed.
        return Element::Empty;
    }
    match template {
        UiTemplate::Empty => Element::Empty,
        UiTemplate::Text(s) => Element::Text(s.clone()),
        UiTemplate::Hole(idx) => fills.take_element(*idx),
        // A `ChildrenHole` only carries meaning inside a node's children list,
        // where [`materialize_children`] splices its run in place. Reaching it as
        // a standalone element (a malformed decode) is inert: an empty element.
        UiTemplate::ChildrenHole(_) => Element::Empty,
        UiTemplate::Node {
            desc,
            attrs,
            children,
        } => Element::Node(
            desc.to_desc(),
            attrs.iter().map(UiTemplateAttr::to_attr).collect(),
            materialize_children(children, depth, fills),
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
            materialize_children(children, depth, fills),
        ),
    }
}

/// Materialize a node's children, splicing each [`UiTemplate::ChildrenHole`] run
/// in place (a `List.map` comprehension expands to zero or more siblings) and
/// materializing every other child as a single element.
fn materialize_children<M>(
    children: &[UiTemplate],
    depth: usize,
    fills: &mut HoleFills<M>,
) -> Vec<Element<M>> {
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        if let UiTemplate::ChildrenHole(idx) = child {
            out.extend(fills.take_children(*idx));
        } else {
            out.push(materialize_ui_at(child, depth.saturating_add(1), fills));
        }
    }
    out
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

/// Rebuild an [`Element`] tree from a [`UiTemplate`], resolving each handler-id
/// HOLE (issue #1668) against the per-render `handlers` map. Identical to
/// [`materialize_ui_template`] on every inert node/attribute; a
/// [`UiTemplateAttr::HandlerHole`] becomes a live `AttrEvent` bound to the
/// map-resolved `Msg`, or drops (fail-closed) when its hole id does not resolve.
///
/// The reconstructed handler is a real `Event::OnMsg`, so the materialized tree
/// feeds `assign_ipe_ids` + `build_index` exactly like a compiled subtree: the
/// browser addresses it by DOM `ipe-id` and the live [`crate::dispatch::HandlerIndex`]
/// resolves it server-side, unchanged. The hole map is consulted ONLY here, at
/// materialize, and only with the SERVER-assigned hole id — the untrusted client
/// never supplies it.
#[must_use]
pub fn materialize_ui_template_with_handlers<M: Clone>(
    template: &UiTemplate,
    handlers: &UiHandlerMap<M>,
) -> Element<M> {
    materialize_ui_at_with_handlers(template, handlers, 0)
}

fn materialize_ui_at_with_handlers<M: Clone>(
    template: &UiTemplate,
    handlers: &UiHandlerMap<M>,
    depth: usize,
) -> Element<M> {
    if depth >= MAX_UI_TEMPLATE_DEPTH {
        return Element::Empty;
    }
    match template {
        UiTemplate::Empty => Element::Empty,
        UiTemplate::Text(s) => Element::Text(s.clone()),
        // The handler front door carries no value/children fills (its holes are the
        // handler-id ATTR holes, resolved from `handlers`), so a value hole and a
        // standalone children hole are inert here — the empty element, fail-closed,
        // never a panic. A children hole inside a children list splices nothing (see
        // `materialize_children_with_handlers`).
        UiTemplate::Hole(_) | UiTemplate::ChildrenHole(_) => Element::Empty,
        UiTemplate::Node {
            desc,
            attrs,
            children,
        } => Element::Node(
            desc.to_desc(),
            attrs
                .iter()
                .map(|a| a.to_attr_with_handlers(handlers))
                .collect(),
            materialize_children_with_handlers(children, handlers, depth),
        ),
        UiTemplate::TaggedNode {
            tag,
            desc,
            attrs,
            children,
        } => Element::TaggedNode(
            tag.clone(),
            desc.to_desc(),
            attrs
                .iter()
                .map(|a| a.to_attr_with_handlers(handlers))
                .collect(),
            materialize_children_with_handlers(children, handlers, depth),
        ),
    }
}

/// Materialize a node's children on the handler-resolving path. A
/// [`UiTemplate::ChildrenHole`] carries no fill here (the handler front door has
/// no fills), so its run is empty — it splices nothing rather than a stray inert
/// element; every other child materializes through
/// [`materialize_ui_at_with_handlers`].
fn materialize_children_with_handlers<M: Clone>(
    children: &[UiTemplate],
    handlers: &UiHandlerMap<M>,
    depth: usize,
) -> Vec<Element<M>> {
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        if matches!(child, UiTemplate::ChildrenHole(_)) {
            continue;
        }
        out.push(materialize_ui_at_with_handlers(
            child,
            handlers,
            depth.saturating_add(1),
        ));
    }
    out
}

/// Decode a serialized [`UiTemplate`] and materialize it, resolving handler-id
/// HOLES against the per-render `handlers` map — the handler-bearing counterpart
/// of [`materialize_ui_template_str`]. The emitted `view` reads its per-view slot
/// (`__ipe_lit.get(N)`) for the JSON and passes the freshly-built map (each
/// model-dependent handler's `Msg` evaluated against the current model), so prod
/// (baked default) and dev (patched slot) run the SAME materialize path — dev ==
/// prod by construction.
///
/// Fail-closed on hostile input, never a panic (the slot value crosses the
/// untrusted dev overlay boundary):
/// - a decode failure returns the inert empty element (`Element::Empty`);
/// - an over-deep decoded template returns the same inert empty element;
/// - a hole whose id does not resolve against `handlers` drops its handler (no
///   fabricated `Msg`, no cross-render `Msg`).
#[cfg(feature = "json")]
#[must_use]
pub fn materialize_ui_template_str_with_handlers<M: Clone>(
    json: &str,
    handlers: &UiHandlerMap<M>,
) -> Element<M> {
    let Ok(template) = serde_json::from_str::<UiTemplate>(json) else {
        return Element::Empty;
    };
    if template.check_bounds().is_err() {
        return Element::Empty;
    }
    materialize_ui_template_with_handlers(&template, handlers)
}

/// The hole-bearing string front door: decode the per-view slot JSON and
/// materialize it, splicing the compiled hole fills. The `Ipe.Ui` analogue of
/// [`materialize_ui_template_str`] for a mostly-static view with `Model`-derived
/// holes — the emitted `view` reads its slot (`__ipe_lit.get(N)`) and hands the
/// baked-default-or-patched template JSON here together with the compiled fills it
/// built for each hole, so prod (baked default) and dev (patched slot) run the
/// SAME materialize path over the SAME fills — dev == prod by construction; only
/// the static skeleton hot-swaps, the fills stay compiled.
///
/// Fail-closed on hostile input, never a panic (the slot value crosses the
/// untrusted dev overlay boundary): a decode failure or over-deep template
/// returns the inert empty element, and any hole the patched template references
/// beyond the supplied fills materializes to the empty element rather than
/// panicking.
#[cfg(feature = "json")]
#[must_use]
pub fn materialize_ui_template_str_with_holes<M>(
    json: &str,
    element_holes: Vec<Element<M>>,
    children_holes: Vec<Vec<Element<M>>>,
) -> Element<M> {
    let Ok(template) = serde_json::from_str::<UiTemplate>(json) else {
        return Element::Empty;
    };
    if template.check_bounds().is_err() {
        return Element::Empty;
    }
    materialize_ui_template_with_holes(&template, element_holes, children_holes)
}

/// Decode a serialized [`UiTemplate`] and materialize it, resolving both
/// numbered value/children **holes** (spliced from `element_holes` /
/// `children_holes`) and model-dependent handler-id **holes** (resolved from
/// `handlers`) in a single pass — the combined front door for a `Ui` subtree
/// that carries both kinds in the same tree.
///
/// Each hole kind is resolved independently, exactly as its single-kind
/// counterpart would:
/// - [`UiTemplate::Hole(n)`] → `element_holes[n]`, consumed once, inert empty
///   on a miss;
/// - [`UiTemplate::ChildrenHole(n)`] → `children_holes[n]`, spliced in place;
/// - [`UiTemplateAttr::HandlerHole { handler_id }`] → `handlers.resolve(id)`,
///   producing a live `AttrEvent`, or `NoAttribute` on a miss (fail-closed).
///
/// Fail-closed on hostile input in every dimension (never panics):
/// - decode failure → `Element::Empty`;
/// - over-deep template → `Element::Empty`;
/// - out-of-range or already-consumed hole index → `Element::Empty` / empty run;
/// - unresolved handler hole id → `NoAttribute` (event silently absent).
#[cfg(feature = "json")]
#[must_use]
pub fn materialize_ui_template_str_with_holes_and_handlers<M: Clone>(
    json: &str,
    element_holes: Vec<Element<M>>,
    children_holes: Vec<Vec<Element<M>>>,
    handlers: &UiHandlerMap<M>,
) -> Element<M> {
    let Ok(template) = serde_json::from_str::<UiTemplate>(json) else {
        return Element::Empty;
    };
    if template.check_bounds().is_err() {
        return Element::Empty;
    }
    let mut fills = HoleFills {
        elements: element_holes.into_iter().map(Some).collect(),
        children: children_holes.into_iter().map(Some).collect(),
    };
    materialize_ui_at_combined(&template, handlers, 0, &mut fills)
}

#[cfg(feature = "json")]
fn materialize_ui_at_combined<M: Clone>(
    template: &UiTemplate,
    handlers: &UiHandlerMap<M>,
    depth: usize,
    fills: &mut HoleFills<M>,
) -> Element<M> {
    if depth >= MAX_UI_TEMPLATE_DEPTH {
        return Element::Empty;
    }
    match template {
        UiTemplate::Empty => Element::Empty,
        UiTemplate::Text(s) => Element::Text(s.clone()),
        UiTemplate::Hole(idx) => fills.take_element(*idx),
        UiTemplate::ChildrenHole(_) => Element::Empty,
        UiTemplate::Node {
            desc,
            attrs,
            children,
        } => Element::Node(
            desc.to_desc(),
            attrs
                .iter()
                .map(|a| a.to_attr_with_handlers(handlers))
                .collect(),
            materialize_children_combined(children, handlers, depth, fills),
        ),
        UiTemplate::TaggedNode {
            tag,
            desc,
            attrs,
            children,
        } => Element::TaggedNode(
            tag.clone(),
            desc.to_desc(),
            attrs
                .iter()
                .map(|a| a.to_attr_with_handlers(handlers))
                .collect(),
            materialize_children_combined(children, handlers, depth, fills),
        ),
    }
}

#[cfg(feature = "json")]
fn materialize_children_combined<M: Clone>(
    children: &[UiTemplate],
    handlers: &UiHandlerMap<M>,
    depth: usize,
    fills: &mut HoleFills<M>,
) -> Vec<Element<M>> {
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        if let UiTemplate::ChildrenHole(idx) = child {
            out.extend(fills.take_children(*idx));
        } else {
            out.push(materialize_ui_at_combined(
                child,
                handlers,
                depth.saturating_add(1),
                fills,
            ));
        }
    }
    out
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

/// Build a [`UiTemplate`] from a static [`Element`] subtree, templatizing a
/// model-dependent `onClick msg`-shaped event handler as a [`UiTemplateAttr::HandlerHole`]
/// (issue #1668) instead of refusing it. Returns the template AND the captured
/// `Msg`s in hole-id order, so the caller pairs the inert template with a
/// [`UiHandlerMap::from_msgs`] resolving each hole back to its `Msg`.
///
/// This is the handler-bearing counterpart of [`ui_template_of`]: it accepts the
/// SAME provably-static structure, and additionally a clean `Event::OnMsg`
/// capture (an event whose only payload is a per-render `Msg`). Every other
/// non-static shape still refuses (returns `None`) exactly as [`ui_template_of`]
/// does — a raw HTML node, a terminal grid, a nested overlay, the debug outline,
/// an `OnString`/`OnBool`/`OnForm`/`OnWidget` handler (runtime-arg-dependent, not
/// a pure capture), or an over-deep tree.
#[must_use]
pub fn ui_template_of_holed<M: Clone>(elem: &Element<M>) -> Option<(UiTemplate, Vec<M>)> {
    let mut captures = Vec::new();
    let template = ui_template_of_holed_at(elem, &mut captures, 0)?;
    Some((template, captures))
}

fn ui_template_of_holed_at<M: Clone>(
    elem: &Element<M>,
    captures: &mut Vec<M>,
    depth: usize,
) -> Option<UiTemplate> {
    if depth >= MAX_UI_TEMPLATE_DEPTH {
        return None;
    }
    match elem {
        Element::Empty => Some(UiTemplate::Empty),
        Element::Text(s) => Some(UiTemplate::Text(s.clone())),
        Element::Raw(_) | Element::Cells(_) => None,
        Element::Node(desc, attrs, children) => {
            let attrs = static_ui_attrs_holed(attrs, captures)?;
            let children = static_ui_children_holed(children, captures, depth)?;
            Some(UiTemplate::Node {
                desc: UiDescription::from_desc(desc),
                attrs,
                children,
            })
        }
        Element::TaggedNode(tag, desc, attrs, children) => {
            let attrs = static_ui_attrs_holed(attrs, captures)?;
            let children = static_ui_children_holed(children, captures, depth)?;
            Some(UiTemplate::TaggedNode {
                tag: tag.clone(),
                desc: UiDescription::from_desc(desc),
                attrs,
                children,
            })
        }
    }
}

fn static_ui_attrs_holed<M: Clone>(
    attrs: &[Attribute<M>],
    captures: &mut Vec<M>,
) -> Option<Vec<UiTemplateAttr>> {
    let mut out = Vec::with_capacity(attrs.len());
    for a in attrs {
        out.push(UiTemplateAttr::from_attr_holed(a, captures)?);
    }
    Some(out)
}

fn static_ui_children_holed<M: Clone>(
    children: &[Element<M>],
    captures: &mut Vec<M>,
    depth: usize,
) -> Option<Vec<UiTemplate>> {
    let mut out = Vec::with_capacity(children.len());
    for c in children {
        out.push(ui_template_of_holed_at(
            c,
            captures,
            depth.saturating_add(1),
        )?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_UI_TEMPLATE_DEPTH, UiHandlerMap, UiTemplate, UiTemplateAttr, UiTemplateError,
        materialize_ui_template, materialize_ui_template_with_handlers, ui_template_of,
        ui_template_of_holed,
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
            r#"{"AlignX":"CenterX"},"Pointer",{"FontAlign":"center"},"#,
            r#"{"FontColor":{"r":10,"g":20,"b":30,"a":1.0}},"#,
            r#"{"BgColor":{"r":1,"g":2,"b":3,"a":0.25}},{"FontLetterSpacing":1.5}],"#,
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
                super::UiTemplateAttr::FontColor(super::UiColor {
                    r: 10,
                    g: 20,
                    b: 30,
                    a: 1.0,
                }),
                super::UiTemplateAttr::BgColor(super::UiColor {
                    r: 1,
                    g: 2,
                    b: 3,
                    a: 0.25,
                }),
                super::UiTemplateAttr::FontLetterSpacing(1.5),
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

    // ── handler-id HOLE (issue #1668) ────────────────────────────────────────
    //
    // A model-dependent event (`onClick (Select id)`) reduces to an opaque HOLE
    // in the inert template; the concrete `Msg` is resolved per render from a
    // `UiHandlerMap`. The model-dependent logic lives ONLY in that server-side
    // map — never serialized, never sent to the client, never a wire closure.
    //
    // Nested in its own module so its local `enum Msg` and `crate::html` imports
    // never collide with the value/list-hole suite below.
    mod handler_holes {
        use super::*;
        use crate::html::{Attribute as HtmlAttribute, Event};

        #[derive(Clone, Debug, PartialEq)]
        enum Msg {
            Select(i64),
            Save,
        }

        // A button carrying a model-dependent `onClick` — the shape that BLOCKS the
        // pure `ui_template_of` (it refuses the whole subtree) but templatizes as a
        // hole through `ui_template_of_holed`.
        fn button_onclick(msg: Msg) -> Element<Msg> {
            Element::TaggedNode(
                "button".to_string(),
                Description::NoDescription,
                vec![Attribute::AttrEvent(HtmlAttribute::EventAttr(
                    Event::OnMsg("click".to_string(), msg),
                ))],
                vec![Element::Text("pick".to_string())],
            )
        }

        // A subtree with a model-dependent handler now templatizes: the pure path
        // refuses it, the holed path accepts it and records the captured Msg.
        #[test]
        fn model_dependent_handler_templatizes_as_a_hole() {
            let subtree = button_onclick(Msg::Select(7));
            // Pure path refuses (it has no hole mechanism).
            assert_eq!(ui_template_of(&subtree), None);
            // Holed path accepts — one hole, one captured Msg.
            let (template, captures) =
                ui_template_of_holed(&subtree).expect("holed path templatizes the handler");
            assert_eq!(captures, vec![Msg::Select(7)]);
            let UiTemplate::TaggedNode { attrs, .. } = &template else {
                panic!("expected a TaggedNode, got {template:?}");
            };
            assert_eq!(
                attrs.as_slice(),
                &[UiTemplateAttr::HandlerHole {
                    event: "click".to_string(),
                    handler_id: 0,
                }],
                "the handler reduced to a bare event-name + hole-id placeholder — no Msg in the datum"
            );
        }

        // The hole resolves back to the captured Msg through the per-render map, so
        // the materialized element carries a LIVE handler that fires the right Msg.
        #[test]
        fn hole_resolves_to_the_captured_msg() {
            let subtree = button_onclick(Msg::Select(42));
            let (template, captures) = ui_template_of_holed(&subtree).expect("templatizes");
            let handlers = UiHandlerMap::from_msgs(captures);
            let materialized: Element<Msg> =
                materialize_ui_template_with_handlers(&template, &handlers);
            assert_eq!(
                materialized, subtree,
                "materialize with the per-render map reconstructs the exact handler-bearing Element"
            );
        }

        // dev == prod byte-identity: the materialized hole renders the SAME DOM
        // event markers (`data-ipe-on`, `ipe-click`, `data-ipe-hid` after id stamp)
        // as the original compiled handler — the wire contract the browser POSTs is
        // unchanged.
        #[test]
        fn hole_renders_byte_identical_event_markers() {
            // Use a `()` Msg so both sides render through the same path; the marker
            // depends only on the event NAME, never the Msg.
            let original: Element<()> = Element::TaggedNode(
                "button".to_string(),
                Description::NoDescription,
                vec![Attribute::AttrEvent(HtmlAttribute::EventAttr(
                    Event::OnMsg("click".to_string(), ()),
                ))],
                vec![Element::Text("go".to_string())],
            );
            let (template, captures) = ui_template_of_holed(&original).expect("templatizes");
            let handlers = UiHandlerMap::from_msgs(captures);
            let materialized: Element<()> =
                materialize_ui_template_with_handlers(&template, &handlers);
            assert_eq!(
                render(materialized),
                render(original),
                "the hole must render byte-identical event markers to the compiled handler"
            );
        }

        // Fail-closed: an out-of-range hole id (a stale template resolved against a
        // shorter map — e.g. after an edit removed a handler) drops the handler.
        // NO Msg is fabricated, no cross-id Msg leaks, no panic.
        #[test]
        fn out_of_range_hole_id_fails_closed_to_no_handler() {
            let template = UiTemplate::TaggedNode {
                tag: "button".to_string(),
                desc: super::super::UiDescription::NoDescription,
                attrs: vec![UiTemplateAttr::HandlerHole {
                    event: "click".to_string(),
                    handler_id: 5, // no such capture
                }],
                children: vec![],
            };
            // A map with only id 0 populated — id 5 is out of range.
            let handlers = UiHandlerMap::from_msgs(vec![Msg::Save]);
            let materialized: Element<Msg> =
                materialize_ui_template_with_handlers(&template, &handlers);
            let Element::TaggedNode(_, _, attrs, _) = &materialized else {
                panic!("expected a TaggedNode, got {materialized:?}");
            };
            assert_eq!(
                attrs.as_slice(),
                &[Attribute::NoAttribute],
                "an unresolved hole drops to NoAttribute — never a fabricated or cross-id Msg"
            );
        }

        // Fail-closed: the map-less materialize path (no handler captures at all)
        // drops every hole. This is the prod/no-capture posture — a hole never
        // invents a Msg without a map.
        #[test]
        fn mapless_materialize_drops_the_hole() {
            let subtree = button_onclick(Msg::Select(1));
            let (template, _captures) = ui_template_of_holed(&subtree).expect("templatizes");
            // Materialize WITHOUT the handler map (empty map) — the hole drops.
            let empty: UiHandlerMap<Msg> = UiHandlerMap::new();
            let materialized: Element<Msg> =
                materialize_ui_template_with_handlers(&template, &empty);
            let Element::TaggedNode(_, _, attrs, _) = &materialized else {
                panic!("expected a TaggedNode, got {materialized:?}");
            };
            assert_eq!(attrs.as_slice(), &[Attribute::NoAttribute]);
        }

        // Cross-render / cross-session isolation: the SAME inert template resolved
        // against two DIFFERENT per-render maps yields two DIFFERENT handlers — the
        // Msg comes only from the render's own map, never leaks across renders. A
        // forged id (out of both maps' range) resolves to nothing in both.
        #[test]
        fn hole_resolution_is_scoped_to_its_own_render_map() {
            let template = UiTemplate::TaggedNode {
                tag: "button".to_string(),
                desc: super::super::UiDescription::NoDescription,
                attrs: vec![UiTemplateAttr::HandlerHole {
                    event: "click".to_string(),
                    handler_id: 0,
                }],
                children: vec![],
            };
            let render_a = UiHandlerMap::from_msgs(vec![Msg::Select(1)]);
            let render_b = UiHandlerMap::from_msgs(vec![Msg::Select(2)]);
            let a: Element<Msg> = materialize_ui_template_with_handlers(&template, &render_a);
            let b: Element<Msg> = materialize_ui_template_with_handlers(&template, &render_b);
            // Compare the RESOLVED Msg directly (Element/Event PartialEq deliberately
            // ignores the Msg payload — two OnMsg("click", _) compare equal for diff
            // purposes — so the distinction must be read off the handler's Msg).
            assert_eq!(resolved_click_msg(&a), Some(Msg::Select(1)));
            assert_eq!(resolved_click_msg(&b), Some(Msg::Select(2)));
            assert_ne!(
                resolved_click_msg(&a),
                resolved_click_msg(&b),
                "each render's map resolves the hole to ITS own Msg"
            );
            // Neither map holds a handler for a forged higher id.
            let forged = UiTemplate::TaggedNode {
                tag: "button".to_string(),
                desc: super::super::UiDescription::NoDescription,
                attrs: vec![UiTemplateAttr::HandlerHole {
                    event: "click".to_string(),
                    handler_id: 99,
                }],
                children: vec![],
            };
            let fa: Element<Msg> = materialize_ui_template_with_handlers(&forged, &render_a);
            let fb: Element<Msg> = materialize_ui_template_with_handlers(&forged, &render_b);
            for e in [fa, fb] {
                let Element::TaggedNode(_, _, attrs, _) = &e else {
                    panic!("expected a TaggedNode");
                };
                assert_eq!(attrs.as_slice(), &[Attribute::NoAttribute]);
            }
        }

        // A non-`OnMsg` event (a runtime-arg-dependent handler) is NOT a clean
        // per-render capture, so even the holed path refuses the whole subtree — it
        // stays compiled rather than templatizing a handler whose resolution needs a
        // client-supplied value / form payload / seal decode.
        #[test]
        fn non_onmsg_handlers_refuse_even_holed() {
            let on_string: Element<Msg> = Element::TaggedNode(
                "input".to_string(),
                Description::NoDescription,
                vec![Attribute::AttrEvent(HtmlAttribute::EventAttr(
                    Event::OnString(
                        "input".to_string(),
                        std::sync::Arc::new(|s| Msg::Select(s.len() as i64)),
                    ),
                ))],
                vec![],
            );
            assert_eq!(ui_template_of_holed(&on_string), None);

            let on_form: Element<Msg> = Element::TaggedNode(
                "form".to_string(),
                Description::NoDescription,
                vec![Attribute::AttrEvent(HtmlAttribute::EventAttr(
                    Event::OnForm(
                        "submit".to_string(),
                        std::sync::Arc::new(|_fd| Some(Msg::Save)),
                    ),
                ))],
                vec![],
            );
            assert_eq!(ui_template_of_holed(&on_form), None);
        }

        // The whole point of issue #1668: a subtree bearing a model-dependent handler
        // hot-swaps its STRUCTURE (the inert template changes) while the handler
        // still fires the right Msg (resolved from the per-render map). Here the
        // structure gains a child between renders; both the old and new template
        // resolve the hole to the same captured Msg.
        #[test]
        fn structure_hot_swaps_while_handler_still_fires() {
            // Before: button with one text child, model-dependent onClick.
            let before: Element<Msg> = Element::TaggedNode(
                "button".to_string(),
                Description::NoDescription,
                vec![Attribute::AttrEvent(HtmlAttribute::EventAttr(
                    Event::OnMsg("click".to_string(), Msg::Select(3)),
                ))],
                vec![Element::Text("one".to_string())],
            );
            // After: the SAME handler, an added static child (a structural edit).
            let after: Element<Msg> = Element::TaggedNode(
                "button".to_string(),
                Description::NoDescription,
                vec![Attribute::AttrEvent(HtmlAttribute::EventAttr(
                    Event::OnMsg("click".to_string(), Msg::Select(3)),
                ))],
                vec![
                    Element::Text("one".to_string()),
                    Element::Text("two".to_string()),
                ],
            );
            let (t_before, c_before) = ui_template_of_holed(&before).expect("templatizes");
            let (t_after, c_after) = ui_template_of_holed(&after).expect("templatizes");
            // The templates differ (structure hot-swapped) …
            assert_ne!(t_before, t_after);
            // … but each resolves the hole to the SAME model-captured Msg.
            let m_before: Element<Msg> = materialize_ui_template_with_handlers(
                &t_before,
                &UiHandlerMap::from_msgs(c_before),
            );
            let m_after: Element<Msg> =
                materialize_ui_template_with_handlers(&t_after, &UiHandlerMap::from_msgs(c_after));
            assert_eq!(m_before, before);
            assert_eq!(m_after, after);
        }

        // Two model-dependent handlers in one subtree get distinct, order-stable hole
        // ids (0, 1), so each resolves to its own captured Msg.
        #[test]
        fn multiple_holes_get_distinct_ordered_ids() {
            let subtree: Element<Msg> = Element::Node(
                Description::NoDescription,
                vec![],
                vec![button_onclick(Msg::Select(10)), button_onclick(Msg::Save)],
            );
            let (_template, captures) = ui_template_of_holed(&subtree).expect("templatizes");
            assert_eq!(captures, vec![Msg::Select(10), Msg::Save]);
        }

        // Read the concrete `Msg` off a materialized element's `click` handler, or
        // `None` when it carries no such handler. Needed because `Element`/`Event`
        // equality ignores the Msg payload, so the resolved Msg must be inspected
        // directly to prove per-render resolution.
        fn resolved_click_msg(elem: &Element<Msg>) -> Option<Msg> {
            let attrs = match elem {
                Element::Node(_, attrs, _) | Element::TaggedNode(_, _, attrs, _) => attrs,
                _ => return None,
            };
            attrs.iter().find_map(|a| match a {
                Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnMsg(name, msg)))
                    if name == "click" =>
                {
                    Some(msg.clone())
                }
                _ => None,
            })
        }
    }

    // ── holes: value / control-flow (single-element) + list (children) ────────
    //
    // Nested in its own module so its `UiTemplate as T` / `UiTemplateAttr`
    // re-imports never collide with the parent suite or the handler-hole suite.
    mod value_holes {
        use super::super::{
            UiDescription, UiTemplate as T, UiTemplateAttr, materialize_ui_template_with_holes,
        };
        use super::*;

        // A value hole: a mostly-static node whose single child is a `Hole(0)` filled
        // by a `Model`-derived text leaf. Materialize splices the fill in place; the
        // surrounding structure comes from the (inert) template.
        #[test]
        fn value_hole_splices_the_element_fill() {
            let template = T::Node {
                desc: UiDescription::NoDescription,
                attrs: vec![UiTemplateAttr::Spacing(8)],
                children: vec![T::Text("count: ".to_string()), T::Hole(0)],
            };
            let got: Element<()> = materialize_ui_template_with_holes(
                &template,
                vec![Element::Text("42".to_string())],
                vec![],
            );
            assert_eq!(
                got,
                Element::Node(
                    Description::NoDescription,
                    vec![Attribute::AttrSpacing(8)],
                    vec![
                        Element::Text("count: ".to_string()),
                        Element::Text("42".to_string()),
                    ],
                )
            );
        }

        // A control-flow hole is materially the same shape as a value hole — the fill
        // is whatever `Element` the compiled `if`/`case` produced. Editing the static
        // wrapper structure (here: adding a sibling) hot-swaps; the fill is unchanged.
        #[test]
        fn control_flow_hole_fill_is_opaque_element() {
            let template = T::Node {
                desc: UiDescription::NoDescription,
                attrs: vec![],
                children: vec![T::Hole(0), T::Text("footer".to_string())],
            };
            // The compiled branch chose a tagged node this render.
            let branch: Element<()> = Element::TaggedNode(
                "strong".to_string(),
                Description::NoDescription,
                vec![],
                vec![Element::Text("on".to_string())],
            );
            let got: Element<()> =
                materialize_ui_template_with_holes(&template, vec![branch.clone()], vec![]);
            assert_eq!(
                got,
                Element::Node(
                    Description::NoDescription,
                    vec![],
                    vec![branch, Element::Text("footer".to_string())],
                )
            );
        }

        // A children hole: a `List.map` comprehension expands to a RUN of siblings
        // spliced among the node's static children, in order.
        #[test]
        fn children_hole_splices_the_run_in_place() {
            let template = T::Node {
                desc: UiDescription::NoDescription,
                attrs: vec![],
                children: vec![
                    T::Text("head".to_string()),
                    T::ChildrenHole(0),
                    T::Text("tail".to_string()),
                ],
            };
            let items: Vec<Element<()>> = vec![
                Element::Text("a".to_string()),
                Element::Text("b".to_string()),
                Element::Text("c".to_string()),
            ];
            let got: Element<()> =
                materialize_ui_template_with_holes(&template, vec![], vec![items]);
            assert_eq!(
                got,
                Element::Node(
                    Description::NoDescription,
                    vec![],
                    vec![
                        Element::Text("head".to_string()),
                        Element::Text("a".to_string()),
                        Element::Text("b".to_string()),
                        Element::Text("c".to_string()),
                        Element::Text("tail".to_string()),
                    ],
                )
            );
        }

        // Mixed: both an element hole and a children hole, each indexed within its own
        // kind (the compiler numbers them per-kind).
        #[test]
        fn element_and_children_holes_index_independently() {
            let template = T::Node {
                desc: UiDescription::NoDescription,
                attrs: vec![],
                children: vec![T::Hole(0), T::ChildrenHole(0), T::Hole(1)],
            };
            let got: Element<()> = materialize_ui_template_with_holes(
                &template,
                vec![
                    Element::Text("first".to_string()),
                    Element::Text("last".to_string()),
                ],
                vec![vec![Element::Text("mid".to_string())]],
            );
            assert_eq!(
                got,
                Element::Node(
                    Description::NoDescription,
                    vec![],
                    vec![
                        Element::Text("first".to_string()),
                        Element::Text("mid".to_string()),
                        Element::Text("last".to_string()),
                    ],
                )
            );
        }

        // Fail-closed: a hole index past the supplied fills materializes to the inert
        // empty element, never a panic (the patched template is untrusted).
        #[test]
        fn out_of_range_hole_is_inert_empty() {
            let template = T::Node {
                desc: UiDescription::NoDescription,
                attrs: vec![],
                children: vec![T::Hole(5), T::ChildrenHole(9)],
            };
            let got: Element<()> = materialize_ui_template_with_holes(&template, vec![], vec![]);
            assert_eq!(
                got,
                Element::Node(Description::NoDescription, vec![], vec![Element::Empty]),
                "an out-of-range element hole is empty and a missing children hole adds nothing"
            );
        }

        // A `ChildrenHole` reached as a standalone element (a malformed decode, not in
        // a children list) is inert — the empty element, never a panic.
        #[test]
        fn standalone_children_hole_is_inert_empty() {
            let got: Element<()> =
                materialize_ui_template_with_holes(&T::ChildrenHole(0), vec![], vec![vec![]]);
            assert_eq!(got, Element::Empty);
        }

        // Combined: a subtree carrying both a value hole (Hole) and a handler hole
        // (HandlerHole) materializes correctly through the combined fn — value fills
        // are spliced and the handler resolves to the captured Msg.
        #[cfg(feature = "json")]
        #[test]
        fn combined_value_hole_and_handler_hole_materialize_together() {
            use super::super::{UiHandlerMap, materialize_ui_template_str_with_holes_and_handlers};
            use crate::html::{Attribute as HtmlAttribute, Event};

            #[derive(Clone, Debug, PartialEq)]
            enum Msg {
                Submit,
            }

            // Template: a node with an onClick handler hole (id 0) and one element
            // child that is a value hole (Hole 0) — both kinds in the same subtree.
            let template = T::Node {
                desc: UiDescription::NoDescription,
                attrs: vec![UiTemplateAttr::HandlerHole {
                    event: "click".to_string(),
                    handler_id: 0,
                }],
                children: vec![T::Hole(0)],
            };
            let json = serde_json::to_string(&template).unwrap();
            let fill: Element<Msg> = Element::Text("label text".to_string());
            let handlers = UiHandlerMap::from_msgs(vec![Msg::Submit]);

            let got: Element<Msg> = materialize_ui_template_str_with_holes_and_handlers(
                &json,
                vec![fill.clone()],
                vec![],
                &handlers,
            );
            assert_eq!(
                got,
                Element::Node(
                    Description::NoDescription,
                    vec![Attribute::AttrEvent(HtmlAttribute::EventAttr(
                        Event::OnMsg("click".to_string(), Msg::Submit),
                    ))],
                    vec![fill],
                ),
                "combined materializer must wire the value fill AND the handler in one pass"
            );
        }

        // dev == prod for a hole-bearing template: the SAME fills over a baked-default
        // template (prod) and a structurally-edited template (dev) render each other's
        // static skeleton, with the fills unchanged — the structural edit hot-swaps.
        #[cfg(feature = "json")]
        #[test]
        fn holes_str_reflects_structural_edit_with_same_fills() {
            use super::super::materialize_ui_template_str_with_holes;
            // before: [Hole(0)] ; after: ["x", Hole(0)] — a static sibling added.
            let before = T::Node {
                desc: UiDescription::NoDescription,
                attrs: vec![],
                children: vec![T::Hole(0)],
            };
            let after = T::Node {
                desc: UiDescription::NoDescription,
                attrs: vec![],
                children: vec![T::Text("x".to_string()), T::Hole(0)],
            };
            let fill = || vec![Element::Text("v".to_string())];
            let json_before = serde_json::to_string(&before).unwrap();
            let json_after = serde_json::to_string(&after).unwrap();
            let out_before: Element<()> =
                materialize_ui_template_str_with_holes(&json_before, fill(), vec![]);
            let out_after: Element<()> =
                materialize_ui_template_str_with_holes(&json_after, fill(), vec![]);
            let before_render = render(out_before);
            assert_eq!(
                before_render,
                render(materialize_ui_template_with_holes::<()>(
                    &before,
                    fill(),
                    vec![]
                ))
            );
            assert_ne!(
                before_render,
                render(out_after),
                "the edit must change render"
            );
        }
    }
}

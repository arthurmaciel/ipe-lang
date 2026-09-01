//! Static-`Ipe.Ui`-subtree partition: recognise a provably-static `Ipe.Ui`
//! `view` subtree in the IR and reduce it to an inert serialized template the
//! runtime materializes at render — the `Ipe.Ui` analogue of
//! [`crate::emit_template`].
//!
//! A subtree is TEMPLATABLE iff it is built entirely from the literal `Ipe.Ui`
//! element node kernels (`UiNode` / `UiTaggedNode` / `UiText` / `UiNone`) over
//! literal arguments, an inert (non-logic, non-`Color`, non-`Float`) attribute
//! set, and a static role `Description`. Any `Model` read ([`Expr::Var`] /
//! [`Expr::Access`]), control flow ([`Expr::If`] / [`Expr::Match`]), event
//! handler, embedded raw HTML (`Ui.html`), record-config builder
//! (`Ui.button` / `Ui.link` / `Ui.image`), or non-literal argument anywhere in
//! the subtree fails the match, so an unprovable subtree stays compiled — the
//! recompile path, conservative by construction.
//!
//! ## Conservative attribute scope
//!
//! The accepted attribute set is deliberately narrower than the runtime
//! [`ipe_runtime::ui::template::UiTemplate`] supports: only integer / string /
//! marker attributes and integer-valued `Length`s templatize. `Color`- and
//! `Float`-bearing attributes (`Font.color`, `Background.color`, shadows, the
//! `rgba` alpha, letter/word spacing, aspect ratio) refuse and stay compiled —
//! reproducing `serde_json`'s exact float spelling in the baked JSON without a
//! float serializer would risk a baked default that fails to decode
//! byte-identically. Refusing is always safe: a refused attribute recompiles.
//! The runtime datum carries the full set, so widening the compiler accept set
//! needs no runtime change.
//!
//! ## Inert by construction
//!
//! A [`CompileUiTemplate`] carries only tag / attribute / text `String`s and
//! `i64`s — it has no handler and no raw-markup variant, mirroring the runtime
//! `UiTemplate`. Its JSON ([`CompileUiTemplate::to_json`]) is byte-identical to
//! the runtime `UiTemplate`'s serde form (pinned by a test), so the emitted
//! baked default decodes back into exactly the tree it described and
//! materializes byte-identically to the direct inline emit — dev == prod.

use crate::emit_template::write_json_string;
use ipe_ir::{Callee, Expr, KernelFn};

/// The render/decode nesting ceiling, mirrored from the runtime
/// (`ipe_runtime::ui::template::MAX_UI_TEMPLATE_DEPTH`). Kept as a local constant
/// (the backend does not depend on the runtime crate outside tests); it equals
/// [`crate::emit_template::MAX_TEMPLATE_DEPTH`], the same HTML render depth cap.
pub const MAX_UI_TEMPLATE_DEPTH: usize = crate::emit_template::MAX_TEMPLATE_DEPTH;

/// A role `Description` reduced to inert data — the producible subset of the
/// runtime `ipe_runtime::ui::template::UiDescription`. The runtime's `DescButton`
/// has no templatable producing kernel (only `Ui.button`, a refused record
/// config, carries it), so it is deliberately absent here; every variant present
/// serializes to the same JSON the runtime decodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileUiDesc {
    NoDescription,
    DescMain,
    DescNavigation,
    DescContentInfo,
    DescComplementary,
    DescHeading(i64),
    DescLabel(String),
    DescLivePolite,
    DescLiveAssertive,
    DescParagraph,
}

impl CompileUiDesc {
    fn write_json(&self, out: &mut String) {
        match self {
            Self::NoDescription => out.push_str("\"NoDescription\""),
            Self::DescMain => out.push_str("\"DescMain\""),
            Self::DescNavigation => out.push_str("\"DescNavigation\""),
            Self::DescContentInfo => out.push_str("\"DescContentInfo\""),
            Self::DescComplementary => out.push_str("\"DescComplementary\""),
            Self::DescHeading(n) => {
                out.push_str("{\"DescHeading\":");
                push_i64(*n, out);
                out.push('}');
            }
            Self::DescLabel(s) => {
                out.push_str("{\"DescLabel\":");
                write_json_string(s, out);
                out.push('}');
            }
            Self::DescLivePolite => out.push_str("\"DescLivePolite\""),
            Self::DescLiveAssertive => out.push_str("\"DescLiveAssertive\""),
            Self::DescParagraph => out.push_str("\"DescParagraph\""),
        }
    }
}

/// A `Length` reduced to inert data — integer-valued shapes only. Mirrors the
/// runtime `UiLength`; the `Min` / `Max` recursive shapes carry an inner length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileUiLength {
    Px(i64),
    Content,
    Fill(i64),
    Min(i64, Box<Self>),
    Max(i64, Box<Self>),
    Vh(i64),
    Vw(i64),
}

impl CompileUiLength {
    fn write_json(&self, out: &mut String) {
        match self {
            Self::Px(n) => tagged_i64("Px", *n, out),
            Self::Content => out.push_str("\"Content\""),
            Self::Fill(n) => tagged_i64("Fill", *n, out),
            Self::Vh(n) => tagged_i64("Vh", *n, out),
            Self::Vw(n) => tagged_i64("Vw", *n, out),
            Self::Min(n, inner) => tagged_i64_len("Min", *n, inner, out),
            Self::Max(n, inner) => tagged_i64_len("Max", *n, inner, out),
        }
    }
}

/// An inert, static `Ipe.Ui` attribute — the integer / string / marker subset of
/// the runtime `UiTemplateAttr`. Each variant serializes to the same JSON the
/// runtime `UiTemplateAttr` decodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileUiAttr {
    Width(CompileUiLength),
    Height(CompileUiLength),
    AlignX(&'static str),
    AlignY(&'static str),
    Padding(i64, i64, i64, i64),
    Spacing(i64),
    Style(String, String),
    Describe(CompileUiDesc),
    Attribute(String, String),
    FontSize(i64),
    FontFamily(String),
    FontWeight(i64),
    FontItalic,
    FontUnderline,
    FontDecoration(&'static str),
    FontAlign(&'static str),
    BorderWidth(i64),
    BorderRounded(i64),
    BorderStyle(&'static str),
    Pointer,
    Overflow(&'static str, &'static str),
}

impl CompileUiAttr {
    fn write_json(&self, out: &mut String) {
        match self {
            Self::Width(l) => {
                out.push_str("{\"Width\":");
                l.write_json(out);
                out.push('}');
            }
            Self::Height(l) => {
                out.push_str("{\"Height\":");
                l.write_json(out);
                out.push('}');
            }
            Self::AlignX(v) => tagged_enum_str("AlignX", v, out),
            Self::AlignY(v) => tagged_enum_str("AlignY", v, out),
            Self::Padding(t, r, b, l) => {
                out.push_str("{\"Padding\":[");
                push_i64(*t, out);
                out.push(',');
                push_i64(*r, out);
                out.push(',');
                push_i64(*b, out);
                out.push(',');
                push_i64(*l, out);
                out.push_str("]}");
            }
            Self::Spacing(n) => tagged_i64("Spacing", *n, out),
            Self::Style(k, v) => tagged_two_strings("Style", k, v, out),
            Self::Describe(d) => {
                out.push_str("{\"Describe\":");
                d.write_json(out);
                out.push('}');
            }
            Self::Attribute(k, v) => tagged_two_strings("Attribute", k, v, out),
            Self::FontSize(n) => tagged_i64("FontSize", *n, out),
            Self::FontFamily(s) => tagged_string("FontFamily", s, out),
            Self::FontWeight(n) => tagged_i64("FontWeight", *n, out),
            Self::FontItalic => out.push_str("\"FontItalic\""),
            Self::FontUnderline => out.push_str("\"FontUnderline\""),
            Self::FontDecoration(v) => tagged_enum_static_str("FontDecoration", v, out),
            Self::FontAlign(v) => tagged_enum_static_str("FontAlign", v, out),
            Self::BorderWidth(n) => tagged_i64("BorderWidth", *n, out),
            Self::BorderRounded(n) => tagged_i64("BorderRounded", *n, out),
            Self::BorderStyle(v) => tagged_enum_static_str("BorderStyle", v, out),
            Self::Pointer => out.push_str("\"Pointer\""),
            Self::Overflow(x, y) => {
                out.push_str("{\"Overflow\":[");
                write_json_string(x, out);
                out.push(',');
                write_json_string(y, out);
                out.push_str("]}");
            }
        }
    }
}

/// An inert, fully-static `Ipe.Ui` subtree reduced to data. Mirrors the runtime
/// `UiTemplate`: there is deliberately no `Raw`, no `Cells`, and no
/// handler-bearing attribute variant — that absence is the security guarantee,
/// enforced by the type (make-invalid-states-unrepresentable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileUiTemplate {
    Empty,
    Text(String),
    Node {
        desc: CompileUiDesc,
        attrs: Vec<CompileUiAttr>,
        children: Vec<Self>,
    },
    TaggedNode {
        tag: String,
        desc: CompileUiDesc,
        attrs: Vec<CompileUiAttr>,
        children: Vec<Self>,
    },
}

impl CompileUiTemplate {
    /// Serialize to the JSON the runtime `UiTemplate` decodes — the externally
    /// tagged enum representation `serde_json` emits by default. Byte-identical
    /// to `serde_json::to_string(&UiTemplate)` (pinned by a backend test).
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    fn write_json(&self, out: &mut String) {
        match self {
            Self::Empty => out.push_str("\"Empty\""),
            Self::Text(s) => tagged_string("Text", s, out),
            Self::Node {
                desc,
                attrs,
                children,
            } => {
                out.push_str("{\"Node\":{\"desc\":");
                desc.write_json(out);
                out.push_str(",\"attrs\":[");
                write_attrs(attrs, out);
                out.push_str("],\"children\":[");
                write_children(children, out);
                out.push_str("]}}");
            }
            Self::TaggedNode {
                tag,
                desc,
                attrs,
                children,
            } => {
                out.push_str("{\"TaggedNode\":{\"tag\":");
                write_json_string(tag, out);
                out.push_str(",\"desc\":");
                desc.write_json(out);
                out.push_str(",\"attrs\":[");
                write_attrs(attrs, out);
                out.push_str("],\"children\":[");
                write_children(children, out);
                out.push_str("]}}");
            }
        }
    }
}

fn write_attrs(attrs: &[CompileUiAttr], out: &mut String) {
    for (i, a) in attrs.iter().enumerate() {
        if i != 0 {
            out.push(',');
        }
        a.write_json(out);
    }
}

fn write_children(children: &[CompileUiTemplate], out: &mut String) {
    for (i, c) in children.iter().enumerate() {
        if i != 0 {
            out.push(',');
        }
        c.write_json(out);
    }
}

/// `serde_json` renders an `i64` as its plain decimal — total, no allocation.
fn push_i64(n: i64, out: &mut String) {
    use std::fmt::Write as _;
    let _ = write!(out, "{n}");
}

fn tagged_i64(tag: &str, n: i64, out: &mut String) {
    out.push_str("{\"");
    out.push_str(tag);
    out.push_str("\":");
    push_i64(n, out);
    out.push('}');
}

fn tagged_i64_len(tag: &str, n: i64, inner: &CompileUiLength, out: &mut String) {
    out.push_str("{\"");
    out.push_str(tag);
    out.push_str("\":[");
    push_i64(n, out);
    out.push(',');
    inner.write_json(out);
    out.push_str("]}");
}

fn tagged_string(tag: &str, s: &str, out: &mut String) {
    out.push_str("{\"");
    out.push_str(tag);
    out.push_str("\":");
    write_json_string(s, out);
    out.push('}');
}

fn tagged_two_strings(tag: &str, a: &str, b: &str, out: &mut String) {
    out.push_str("{\"");
    out.push_str(tag);
    out.push_str("\":[");
    write_json_string(a, out);
    out.push(',');
    write_json_string(b, out);
    out.push_str("]}");
}

/// A single-newtype attr carrying a `String` variant tag (`AlignX(HAlign)` etc.)
/// — the payload is the runtime enum variant name as a plain JSON string.
fn tagged_enum_str(tag: &str, variant: &str, out: &mut String) {
    out.push_str("{\"");
    out.push_str(tag);
    out.push_str("\":\"");
    out.push_str(variant);
    out.push_str("\"}");
}

/// A single-newtype attr carrying a raw `String` payload built from a `&'static
/// str` (`FontDecoration(String)` etc.) — the payload is an ordinary JSON string
/// and must be escaped like any other.
fn tagged_enum_static_str(tag: &str, s: &str, out: &mut String) {
    tagged_string(tag, s, out);
}

/// Reduce a static `Ipe.Ui` `view` subtree to a [`CompileUiTemplate`], or `None`
/// when the subtree is not provably static — the caller then keeps it compiled.
pub fn ui_template_of_expr(expr: &Expr) -> Option<CompileUiTemplate> {
    ui_template_of_expr_at(expr, 0)
}

fn ui_template_of_expr_at(expr: &Expr, depth: usize) -> Option<CompileUiTemplate> {
    if depth >= MAX_UI_TEMPLATE_DEPTH {
        return None;
    }
    let Expr::Call { callee, args, .. } = expr else {
        return None;
    };
    let Callee::Kernel(k) = callee else {
        return None;
    };
    // Exhaustive over the `Ipe.Ui` element-node kernels: the four static node
    // builders reduce; every other element-producing kernel (`UiHtml` embeds raw
    // `Html`, `UiCells` a raw grid, `UiButton`/`UiLink`/`UiImage` carry a record
    // config with a possible handler, the nearby `UiAbove`/… nest a sub-view,
    // `UiWidget` a live handler) refuses and stays compiled. A non-element kernel
    // (an attribute or value builder in element position) is a type error
    // upstream and also refuses here.
    match k {
        KernelFn::UiNone => match args.as_slice() {
            [] => Some(CompileUiTemplate::Empty),
            _ => None,
        },
        KernelFn::UiText => match args.as_slice() {
            [Expr::Str(s)] => Some(CompileUiTemplate::Text(s.clone())),
            _ => None,
        },
        // `ui_node_(desc, attrs, children)`.
        KernelFn::UiNode => match args.as_slice() {
            [desc, attrs, children] => Some(CompileUiTemplate::Node {
                desc: static_desc(desc)?,
                attrs: static_attrs(attrs)?,
                children: static_children(children, depth)?,
            }),
            _ => None,
        },
        // `ui_tagged_node_(tag, desc, attrs, children)`.
        KernelFn::UiTaggedNode => match args.as_slice() {
            [Expr::Str(tag), desc, attrs, children] => Some(CompileUiTemplate::TaggedNode {
                tag: tag.clone(),
                desc: static_desc(desc)?,
                attrs: static_attrs(attrs)?,
                children: static_children(children, depth)?,
            }),
            _ => None,
        },
        // Every other kernel — a raw / record-config / nearby / widget element,
        // an attribute or value builder, or a non-UI call — is not a static
        // `Ipe.Ui` element node. Refuse, keep compiled.
        _ => None,
    }
}

/// Reduce a literal list of child `Ipe.Ui` nodes to templates, or `None`.
fn static_children(children: &Expr, depth: usize) -> Option<Vec<CompileUiTemplate>> {
    let Expr::List { items, .. } = children else {
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(ui_template_of_expr_at(item, depth.saturating_add(1))?);
    }
    Some(out)
}

/// Reduce a literal list of `Ipe.Ui` attributes to inert data, or `None` when
/// any element is not an accepted static attribute.
fn static_attrs(attrs: &Expr) -> Option<Vec<CompileUiAttr>> {
    let Expr::List { items, .. } = attrs else {
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(static_attr(item)?);
    }
    Some(out)
}

/// Reduce one `Ipe.Ui` attribute expression to inert data, or `None`.
///
/// Fail-closed allowlist: only the integer / string / marker attribute kernels
/// below templatize. Every other attribute kernel — an event handler, a `Color`-
/// or `Float`-bearing attribute (deferred), a pseudo-rule, a nearby overlay, the
/// debug outline — returns `None`, so the subtree stays compiled rather than
/// mis-templated. A kernel absent from this allowlist defaults to refuse, which
/// is always safe (it merely recompiles).
fn static_attr(attr: &Expr) -> Option<CompileUiAttr> {
    let Expr::Call { callee, args, .. } = attr else {
        return None;
    };
    let Callee::Kernel(k) = callee else {
        return None;
    };
    match (k, args.as_slice()) {
        (KernelFn::UiSpacing, [Expr::Int(n)]) => Some(CompileUiAttr::Spacing(*n)),
        (KernelFn::UiPadding, [Expr::Int(n)]) => Some(CompileUiAttr::Padding(*n, *n, *n, *n)),
        (KernelFn::UiPaddingXY, [Expr::Int(x), Expr::Int(y)]) => {
            Some(CompileUiAttr::Padding(*y, *x, *y, *x))
        }
        (KernelFn::UiWidth, [len]) => Some(CompileUiAttr::Width(static_length(len)?)),
        (KernelFn::UiHeight, [len]) => Some(CompileUiAttr::Height(static_length(len)?)),
        (KernelFn::UiCenterX, []) => Some(CompileUiAttr::AlignX("CenterX")),
        (KernelFn::UiAlignLeft, []) => Some(CompileUiAttr::AlignX("AlignLeft")),
        (KernelFn::UiAlignRight, []) => Some(CompileUiAttr::AlignX("AlignRight")),
        (KernelFn::UiCenterY, []) => Some(CompileUiAttr::AlignY("CenterY")),
        (KernelFn::UiAlignTop, []) => Some(CompileUiAttr::AlignY("AlignTop")),
        (KernelFn::UiAlignBottom, []) => Some(CompileUiAttr::AlignY("AlignBottom")),
        (KernelFn::UiPointer, []) => Some(CompileUiAttr::Pointer),
        (KernelFn::UiClip, []) => Some(CompileUiAttr::Overflow("hidden", "hidden")),
        (KernelFn::UiClipX, []) => Some(CompileUiAttr::Overflow("clip", "visible")),
        (KernelFn::UiClipY, []) => Some(CompileUiAttr::Overflow("visible", "clip")),
        (KernelFn::UiScrollbars, []) => Some(CompileUiAttr::Overflow("auto", "auto")),
        (KernelFn::UiScrollbarX, []) => Some(CompileUiAttr::Overflow("auto", "hidden")),
        (KernelFn::UiScrollbarY, []) => Some(CompileUiAttr::Overflow("hidden", "auto")),
        (KernelFn::UiStyle, [Expr::Str(p), Expr::Str(v)]) => {
            Some(CompileUiAttr::Style(p.clone(), v.clone()))
        }
        (KernelFn::UiGridColumns, [Expr::Int(n)]) => Some(CompileUiAttr::Style(
            "--ipe-grid-columns".to_string(),
            n.to_string(),
        )),
        (KernelFn::UiName, [Expr::Str(v)]) => {
            Some(CompileUiAttr::Attribute("name".to_string(), v.clone()))
        }
        (KernelFn::UiHtmlAttribute, [Expr::Str(key), Expr::Str(value)]) => {
            Some(CompileUiAttr::Attribute(key.clone(), value.clone()))
        }
        (KernelFn::FontSize, [Expr::Int(n)]) => Some(CompileUiAttr::FontSize(*n)),
        (KernelFn::FontFamily, [Expr::Str(f)]) => Some(CompileUiAttr::FontFamily(f.clone())),
        (KernelFn::FontWeight, [Expr::Int(n)]) => Some(CompileUiAttr::FontWeight(*n)),
        (KernelFn::FontBold, []) => Some(CompileUiAttr::FontWeight(700)),
        (KernelFn::FontSemiBold, []) => Some(CompileUiAttr::FontWeight(600)),
        (KernelFn::FontRegular, []) => Some(CompileUiAttr::FontWeight(400)),
        (KernelFn::FontLight, []) => Some(CompileUiAttr::FontWeight(300)),
        (KernelFn::FontExtraBold, []) => Some(CompileUiAttr::FontWeight(800)),
        (KernelFn::FontBlack, []) => Some(CompileUiAttr::FontWeight(900)),
        (KernelFn::FontItalic, []) => Some(CompileUiAttr::FontItalic),
        (KernelFn::FontUnderline, []) => Some(CompileUiAttr::FontUnderline),
        (KernelFn::FontNoDecoration, []) => Some(CompileUiAttr::FontDecoration("none")),
        (KernelFn::FontLineThrough, []) => Some(CompileUiAttr::FontDecoration("line-through")),
        (KernelFn::FontAlignLeft, []) => Some(CompileUiAttr::FontAlign("left")),
        (KernelFn::FontAlignRight, []) => Some(CompileUiAttr::FontAlign("right")),
        (KernelFn::FontAlignCenter | KernelFn::FontCenter, []) => {
            Some(CompileUiAttr::FontAlign("center"))
        }
        (KernelFn::FontJustify, []) => Some(CompileUiAttr::FontAlign("justify")),
        (KernelFn::BorderWidth, [Expr::Int(n)]) => Some(CompileUiAttr::BorderWidth(*n)),
        (KernelFn::BorderRounded, [Expr::Int(n)]) => Some(CompileUiAttr::BorderRounded(*n)),
        (KernelFn::BorderSolid, []) => Some(CompileUiAttr::BorderStyle("solid")),
        (KernelFn::BorderDashed, []) => Some(CompileUiAttr::BorderStyle("dashed")),
        (KernelFn::BorderDotted, []) => Some(CompileUiAttr::BorderStyle("dotted")),
        (KernelFn::UiDescribe, [desc]) => Some(CompileUiAttr::Describe(static_desc(desc)?)),
        (KernelFn::RegionMainContent, []) => Some(CompileUiAttr::Describe(CompileUiDesc::DescMain)),
        (KernelFn::RegionNavigation, []) => {
            Some(CompileUiAttr::Describe(CompileUiDesc::DescNavigation))
        }
        (KernelFn::RegionFooter, []) => {
            Some(CompileUiAttr::Describe(CompileUiDesc::DescContentInfo))
        }
        (KernelFn::RegionAside, []) => {
            Some(CompileUiAttr::Describe(CompileUiDesc::DescComplementary))
        }
        (KernelFn::RegionHeading, [Expr::Int(n)]) => {
            Some(CompileUiAttr::Describe(CompileUiDesc::DescHeading(*n)))
        }
        (KernelFn::RegionLabel, [Expr::Str(s)]) => {
            Some(CompileUiAttr::Describe(CompileUiDesc::DescLabel(s.clone())))
        }
        (KernelFn::RegionAnnounce, []) => {
            Some(CompileUiAttr::Describe(CompileUiDesc::DescLivePolite))
        }
        (KernelFn::RegionAnnounceUrgently, []) => {
            Some(CompileUiAttr::Describe(CompileUiDesc::DescLiveAssertive))
        }
        // Every other attribute kernel — a handler, a `Color`/`Float`-bearing
        // attribute (deferred), a pseudo-rule, a nearby overlay, the debug
        // outline, or an unrecognised one — is not an accepted inert attribute.
        // Refuse: keep the subtree compiled.
        _ => None,
    }
}

/// Reduce a `Length`-producing kernel call over literal arguments to inert data,
/// or `None`. Only integer-valued `Length`s templatize; a `Model`-derived length
/// refuses.
fn static_length(expr: &Expr) -> Option<CompileUiLength> {
    let Expr::Call { callee, args, .. } = expr else {
        return None;
    };
    let Callee::Kernel(k) = callee else {
        return None;
    };
    match (k, args.as_slice()) {
        (KernelFn::UiPx, [Expr::Int(n)]) => Some(CompileUiLength::Px(*n)),
        (KernelFn::UiFill, []) => Some(CompileUiLength::Fill(1)),
        (KernelFn::UiFillPortion, [Expr::Int(n)]) => Some(CompileUiLength::Fill(*n)),
        (KernelFn::UiContent | KernelFn::UiShrink, []) => Some(CompileUiLength::Content),
        (KernelFn::UiVh, [Expr::Int(n)]) => Some(CompileUiLength::Vh(*n)),
        (KernelFn::UiVw, [Expr::Int(n)]) => Some(CompileUiLength::Vw(*n)),
        (KernelFn::UiMinimum, [Expr::Int(n), inner]) => {
            Some(CompileUiLength::Min(*n, Box::new(static_length(inner)?)))
        }
        (KernelFn::UiMaximum, [Expr::Int(n), inner]) => {
            Some(CompileUiLength::Max(*n, Box::new(static_length(inner)?)))
        }
        _ => None,
    }
}

/// Reduce a `Description`-producing kernel call to inert data, or `None`.
fn static_desc(expr: &Expr) -> Option<CompileUiDesc> {
    let Expr::Call { callee, args, .. } = expr else {
        return None;
    };
    let Callee::Kernel(k) = callee else {
        return None;
    };
    match (k, args.as_slice()) {
        (KernelFn::UiDescNone, []) => Some(CompileUiDesc::NoDescription),
        (KernelFn::UiDescParagraph, []) => Some(CompileUiDesc::DescParagraph),
        (KernelFn::UiDescMain, []) => Some(CompileUiDesc::DescMain),
        (KernelFn::UiDescNavigation, []) => Some(CompileUiDesc::DescNavigation),
        (KernelFn::UiDescContentInfo, []) => Some(CompileUiDesc::DescContentInfo),
        (KernelFn::UiDescComplementary, []) => Some(CompileUiDesc::DescComplementary),
        (KernelFn::UiDescLivePolite, []) => Some(CompileUiDesc::DescLivePolite),
        (KernelFn::UiDescLiveAssertive, []) => Some(CompileUiDesc::DescLiveAssertive),
        (KernelFn::UiDescHeading, [Expr::Int(n)]) => Some(CompileUiDesc::DescHeading(*n)),
        (KernelFn::UiDescLabel, [Expr::Str(s)]) => Some(CompileUiDesc::DescLabel(s.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CompileUiAttr, CompileUiDesc, CompileUiLength, CompileUiTemplate, ui_template_of_expr,
    };
    use ipe_ir::{CallPin, Callee, Expr, IrType, KernelFn, OnFormKind, UiCtor};

    fn kcall(k: KernelFn, args: Vec<Expr>) -> Expr {
        Expr::Call {
            callee: Callee::Kernel(k),
            args,
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }
    }

    fn attr_list(items: Vec<Expr>) -> Expr {
        Expr::List {
            elem: IrType::Ui {
                ctor: UiCtor::HtmlAttribute,
                msg: Box::new(IrType::Int),
            },
            items,
        }
    }

    fn child_list(items: Vec<Expr>) -> Expr {
        Expr::List {
            elem: IrType::Ui {
                ctor: UiCtor::Element,
                msg: Box::new(IrType::Int),
            },
            items,
        }
    }

    fn desc_none() -> Expr {
        kcall(KernelFn::UiDescNone, vec![])
    }

    fn text(s: &str) -> Expr {
        kcall(KernelFn::UiText, vec![Expr::Str(s.to_string())])
    }

    // ── acceptance ───────────────────────────────────────────────────────────

    #[test]
    fn text_node_templates() {
        assert_eq!(
            ui_template_of_expr(&text("hi")),
            Some(CompileUiTemplate::Text("hi".to_string()))
        );
    }

    #[test]
    fn none_templates_to_empty() {
        assert_eq!(
            ui_template_of_expr(&kcall(KernelFn::UiNone, vec![])),
            Some(CompileUiTemplate::Empty)
        );
    }

    #[test]
    fn node_with_inert_attrs_and_children_templates() {
        // A `Ui.row` shape lowered to `ui_node_(descNone, [__row marker via
        // style; spacing 8; width (px 16)], [text "a"])`.
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![
                    kcall(
                        KernelFn::UiStyle,
                        vec![
                            Expr::Str("__row".to_string()),
                            Expr::Str("true".to_string()),
                        ],
                    ),
                    kcall(KernelFn::UiSpacing, vec![Expr::Int(8)]),
                    kcall(
                        KernelFn::UiWidth,
                        vec![kcall(KernelFn::UiPx, vec![Expr::Int(16)])],
                    ),
                ]),
                child_list(vec![text("a")]),
            ],
        );
        let got = ui_template_of_expr(&node).expect("templatable");
        assert_eq!(
            got,
            CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![
                    CompileUiAttr::Style("__row".to_string(), "true".to_string()),
                    CompileUiAttr::Spacing(8),
                    CompileUiAttr::Width(CompileUiLength::Px(16)),
                ],
                children: vec![CompileUiTemplate::Text("a".to_string())],
            }
        );
    }

    #[test]
    fn tagged_node_templates() {
        let node = kcall(
            KernelFn::UiTaggedNode,
            vec![
                Expr::Str("section".to_string()),
                kcall(KernelFn::UiDescMain, vec![]),
                attr_list(vec![kcall(KernelFn::FontSize, vec![Expr::Int(16)])]),
                child_list(vec![text("Body")]),
            ],
        );
        let got = ui_template_of_expr(&node).expect("templatable");
        assert_eq!(
            got,
            CompileUiTemplate::TaggedNode {
                tag: "section".to_string(),
                desc: CompileUiDesc::DescMain,
                attrs: vec![CompileUiAttr::FontSize(16)],
                children: vec![CompileUiTemplate::Text("Body".to_string())],
            }
        );
    }

    #[test]
    fn nested_min_max_length_templates() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![kcall(
                    KernelFn::UiWidth,
                    vec![kcall(
                        KernelFn::UiMaximum,
                        vec![Expr::Int(320), kcall(KernelFn::UiVh, vec![Expr::Int(80)])],
                    )],
                )]),
                child_list(vec![]),
            ],
        );
        let got = ui_template_of_expr(&node).expect("templatable");
        assert_eq!(
            got,
            CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![CompileUiAttr::Width(CompileUiLength::Max(
                    320,
                    Box::new(CompileUiLength::Vh(80))
                ))],
                children: vec![],
            }
        );
    }

    // ── refusal: not provably static ⇒ keep compiled (None) ──────────────────

    #[test]
    fn model_read_child_refuses() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![]),
                child_list(vec![Expr::Var(ipe_intern::Symbol::from_raw(1))]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node), None);
    }

    #[test]
    fn embedded_raw_html_child_refuses() {
        // `Ui.html (...)` embeds a `Html` node — not a static `Ipe.Ui` node.
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![]),
                child_list(vec![kcall(
                    KernelFn::UiHtml,
                    vec![Expr::Var(ipe_intern::Symbol::from_raw(2))],
                )]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node), None);
    }

    #[test]
    fn event_handler_attribute_refuses() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![kcall(
                    KernelFn::UiOnClick,
                    vec![Expr::Var(ipe_intern::Symbol::from_raw(3))],
                )]),
                child_list(vec![]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node), None);
    }

    #[test]
    fn color_bearing_attribute_refuses() {
        // `Font.color (rgb …)` is inert but `Color`/`Float`-bearing — outside the
        // integer/string/marker accept set, so it refuses and keeps the subtree
        // compiled.
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![kcall(
                    KernelFn::FontColor,
                    vec![kcall(
                        KernelFn::UiRgb,
                        vec![Expr::Int(1), Expr::Int(2), Expr::Int(3)],
                    )],
                )]),
                child_list(vec![]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node), None);
    }

    #[test]
    fn non_literal_attr_value_refuses() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![kcall(
                    KernelFn::UiStyle,
                    vec![
                        Expr::Str("color".to_string()),
                        Expr::Var(ipe_intern::Symbol::from_raw(4)),
                    ],
                )]),
                child_list(vec![]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node), None);
    }

    #[test]
    fn non_literal_tag_refuses() {
        let node = kcall(
            KernelFn::UiTaggedNode,
            vec![
                Expr::Var(ipe_intern::Symbol::from_raw(5)),
                desc_none(),
                attr_list(vec![]),
                child_list(vec![]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node), None);
    }

    #[test]
    fn button_record_config_refuses() {
        // `Ui.button` carries an `onPress` handler record — never a static node.
        assert_eq!(
            ui_template_of_expr(&kcall(KernelFn::UiButton, vec![])),
            None
        );
    }

    // ── JSON shape: byte-identical to the runtime `UiTemplate` serde form ─────
    //
    // Pinned literals (verified against `serde_json::to_string(&UiTemplate)` —
    // the runtime crate's `str_materialize_matches_direct_render` proves the
    // render-equivalence half). A drift in the serializer is caught here without
    // pulling the runtime `ui` crate into the backend's dev build.

    #[test]
    fn json_text_node_shape() {
        assert_eq!(
            CompileUiTemplate::Text("hi".to_string()).to_json(),
            r#"{"Text":"hi"}"#
        );
    }

    #[test]
    fn json_empty_node_shape() {
        assert_eq!(CompileUiTemplate::Empty.to_json(), r#""Empty""#);
    }

    #[test]
    fn json_tagged_node_full_shape() {
        let t = CompileUiTemplate::TaggedNode {
            tag: "section".to_string(),
            desc: CompileUiDesc::DescHeading(2),
            attrs: vec![
                CompileUiAttr::Width(CompileUiLength::Max(320, Box::new(CompileUiLength::Vh(80)))),
                CompileUiAttr::Padding(1, 2, 3, 4),
                CompileUiAttr::Style("k".to_string(), "v".to_string()),
                CompileUiAttr::AlignX("CenterX"),
                CompileUiAttr::Pointer,
                CompileUiAttr::FontAlign("center"),
            ],
            children: vec![
                CompileUiTemplate::Text("hi".to_string()),
                CompileUiTemplate::Empty,
            ],
        };
        let expected = concat!(
            r#"{"TaggedNode":{"tag":"section","desc":{"DescHeading":2},"attrs":["#,
            r#"{"Width":{"Max":[320,{"Vh":80}]}},{"Padding":[1,2,3,4]},{"Style":["k","v"]},"#,
            r#"{"AlignX":"CenterX"},"Pointer",{"FontAlign":"center"}],"#,
            r#""children":[{"Text":"hi"},"Empty"]}}"#,
        );
        assert_eq!(t.to_json(), expected);
    }

    #[test]
    fn json_node_shape() {
        let t = CompileUiTemplate::Node {
            desc: CompileUiDesc::NoDescription,
            attrs: vec![CompileUiAttr::Spacing(8)],
            children: vec![],
        };
        assert_eq!(
            t.to_json(),
            r#"{"Node":{"desc":"NoDescription","attrs":[{"Spacing":8}],"children":[]}}"#
        );
    }
}

//! Static-`Ipe.Ui`-subtree partition: recognise a provably-static `Ipe.Ui`
//! `view` subtree in the IR and reduce it to an inert serialized template the
//! runtime materializes at render — the `Ipe.Ui` analogue of
//! [`crate::emit_template`].
//!
//! A subtree is TEMPLATABLE (pure mode, [`ui_template_of_expr`]) iff it is built
//! entirely from the literal `Ipe.Ui` element node kernels (`UiNode` /
//! `UiTaggedNode` / `UiText` / `UiNone`) over literal arguments, an inert
//! attribute set, and a static role `Description`. Any `Model` read
//! ([`Expr::Var`] / [`Expr::Access`]), control flow ([`Expr::If`] /
//! [`Expr::Match`]), event handler, embedded raw HTML (`Ui.html`), record-config
//! builder (`Ui.button` / `Ui.link` / `Ui.image`), or non-literal argument
//! anywhere fails the match, so an unprovable subtree stays compiled — the
//! recompile path, conservative by construction.
//!
//! ## Holes ([`ui_template_of_expr_holes`])
//!
//! The hole-bearing partition admits a mostly-static subtree with `Model`-derived
//! **holes**, each replaced by a numbered marker plus a compiled fill:
//! - a `Model`-derived value leaf (`Ui.text (…model…)`) or control-flow
//!   ([`Expr::If`] / [`Expr::Match`]) in element position → a single
//!   [`CompileUiTemplate::Hole`] + an [`HoleKind::Element`] fill;
//! - a `List.map` comprehension in children-list position →
//!   [`CompileUiTemplate::ChildrenHole`] + an [`HoleKind::Children`] fill.
//!
//! Everything else stays as conservative as pure mode: a handler, raw markup, a
//! record-config builder, or a non-literal attribute still refuses the whole
//! subtree — a hole never covers a handler (that is the deferred, guardian-gated
//! increment). Hole indices are numbered per-KIND so the emit's two ordered fill
//! vecs land each fill where its marker names it.
//!
//! ## Conservative attribute scope
//!
//! The accepted attribute set covers the integer / string / marker attributes and
//! integer-valued `Length`s, plus the single-`Color` and single-`Float`
//! attributes: `Font.color` / `Background.color` / `Border.color` (each over a
//! literal `Ui.rgb` / `Ui.rgba` / `Ui.white` / `Ui.black` / `Ui.transparent`
//! color), and `Font.letterSpacing` / `Font.wordSpacing` (over a `Float`
//! literal). The float spelling in the baked JSON is produced by `ryu`
//! ([`push_f64`]) — the exact formatter `serde_json`'s `write_f64` uses — so it is
//! byte-identical to the runtime's serde form, single source of truth with no
//! hand-rolled float formatter to drift.
//!
//! Still refused (always safe — a refused attribute recompiles): the shadow
//! record attributes (`Border.shadow` / `Border.innerShadow`, whose
//! `{offsetX, offsetY, blur, spread, color}` record arg is a distinct reduction
//! shape), aspect ratio, gradients, a `Model`-derived color/float, and any
//! non-literal argument. The runtime datum carries the full set, so widening the
//! compiler accept set further needs no runtime change.
//!
//! ## Inert by construction
//!
//! A [`CompileUiTemplate`] carries only tag / attribute / text `String`s, `i64`s,
//! `f64` style values, and hole INDICES (`usize`) — it has no handler and no
//! raw-markup variant, mirroring the runtime `UiTemplate`. A hole is an inert
//! index, never logic: the `Model`-derived value it stands for is compiled
//! separately and passed to the materializer, so no template datum — however
//! patched — can smuggle a `Msg`, a handler, or unescaped markup. Its JSON
//! ([`CompileUiTemplate::to_json`]) is byte-identical to
//! the runtime `UiTemplate`'s serde form (pinned by a test), so the emitted
//! baked default decodes back into exactly the tree it described and
//! materializes byte-identically to the direct inline emit — dev == prod.

use std::collections::BTreeMap;

use crate::emit_template::write_json_string;
use ipe_intern::Symbol;
use ipe_ir::{Callee, Expr, FuncId, KernelFn};

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

/// A `Color` reduced to inert data — the single `Rgba` shape. Mirrors the runtime
/// `ipe_runtime::ui::template::UiColor` field-for-field. The alpha channel is a
/// `Float`; it serializes through `serde_json` ([`push_f64`]) so the baked JSON is
/// byte-identical to the runtime's `#[derive(Serialize)]` form (single source of
/// truth for the float spelling — no hand-rolled float formatter to drift).
#[derive(Clone, Debug, PartialEq)]
pub struct CompileUiColor {
    pub r: i64,
    pub g: i64,
    pub b: i64,
    pub a: f64,
}

impl CompileUiColor {
    fn write_json(&self, out: &mut String) {
        out.push_str("{\"r\":");
        push_i64(self.r, out);
        out.push_str(",\"g\":");
        push_i64(self.g, out);
        out.push_str(",\"b\":");
        push_i64(self.b, out);
        out.push_str(",\"a\":");
        push_f64(self.a, out);
        out.push('}');
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

/// An inert, static `Ipe.Ui` attribute — the integer / string / marker / color /
/// float subset of the runtime `UiTemplateAttr`. Each variant serializes to the
/// same JSON the runtime `UiTemplateAttr` decodes.
///
/// `Eq` is deliberately absent: the color and letter/word-spacing variants carry a
/// `Float`, which is not `Eq`. `PartialEq` (structural, including the float bits)
/// is enough for the byte-identity pins and the classifier's value diff.
#[derive(Clone, Debug, PartialEq)]
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
    FontColor(CompileUiColor),
    FontLetterSpacing(f64),
    FontWordSpacing(f64),
    BgColor(CompileUiColor),
    BorderWidth(i64),
    BorderRounded(i64),
    BorderStyle(&'static str),
    BorderColor(CompileUiColor),
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
            Self::FontColor(c) => tagged_color("FontColor", c, out),
            Self::FontLetterSpacing(v) => tagged_f64("FontLetterSpacing", *v, out),
            Self::FontWordSpacing(v) => tagged_f64("FontWordSpacing", *v, out),
            Self::BgColor(c) => tagged_color("BgColor", c, out),
            Self::BorderWidth(n) => tagged_i64("BorderWidth", *n, out),
            Self::BorderRounded(n) => tagged_i64("BorderRounded", *n, out),
            Self::BorderStyle(v) => tagged_enum_static_str("BorderStyle", v, out),
            Self::BorderColor(c) => tagged_color("BorderColor", c, out),
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
///
/// `Eq` is absent (an attribute may carry a `Float` alpha, which is not `Eq`),
/// matching the runtime `UiTemplate`; `PartialEq` is enough for the pins.
#[derive(Clone, Debug, PartialEq)]
pub enum CompileUiTemplate {
    Empty,
    Text(String),
    /// A single-element hole (index into the per-render element-fill slice): a
    /// `Model`-derived value leaf or a control-flow (`if` / `case`) result. Inert
    /// — carries only an index; the fill is compiled separately.
    Hole(usize),
    /// A children hole (index into the per-render children-fill slice): a
    /// `List.map` comprehension expanding to a run of sibling elements.
    ChildrenHole(usize),
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
            Self::Hole(n) => tagged_usize("Hole", *n, out),
            Self::ChildrenHole(n) => tagged_usize("ChildrenHole", *n, out),
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

/// Append `f`'s JSON spelling exactly as the runtime's `#[derive(Serialize)]`
/// would — `serde_json`'s `write_f64` formats an `f64` with
/// `ryu::Buffer::format_finite`, so using `ryu` here yields byte-identical output
/// (single source of truth; no hand-rolled float formatter to drift from
/// `serde_json`'s shortest-round-trip form).
///
/// A non-finite `f64` (`NaN` / `±∞`) has no JSON number form and `format_finite`
/// is documented only for finite inputs. The partition reaches this only with a
/// finite literal (the classifier refuses any non-literal), and a source `Float`
/// literal is always finite — so the non-finite arm is unreachable in practice;
/// it emits `0.0` (a finite, decodable value) rather than an undecodable token, so
/// even a future non-finite path fails closed to a valid template.
fn push_f64(f: f64, out: &mut String) {
    if f.is_finite() {
        let mut buf = ryu::Buffer::new();
        out.push_str(buf.format_finite(f));
    } else {
        out.push_str("0.0");
    }
}

fn tagged_f64(tag: &str, f: f64, out: &mut String) {
    out.push_str("{\"");
    out.push_str(tag);
    out.push_str("\":");
    push_f64(f, out);
    out.push('}');
}

/// `{"<tag>":{"r":R,"g":G,"b":B,"a":A}}` — the runtime `UiColor` struct form.
fn tagged_color(tag: &str, c: &CompileUiColor, out: &mut String) {
    out.push_str("{\"");
    out.push_str(tag);
    out.push_str("\":");
    c.write_json(out);
    out.push('}');
}

fn tagged_i64(tag: &str, n: i64, out: &mut String) {
    out.push_str("{\"");
    out.push_str(tag);
    out.push_str("\":");
    push_i64(n, out);
    out.push('}');
}

/// A single-newtype variant carrying a `usize` (a hole index). `serde_json`
/// renders a `usize` as its plain decimal — the same spelling the runtime
/// `UiTemplate::Hole(usize)` serde form decodes.
fn tagged_usize(tag: &str, n: usize, out: &mut String) {
    use std::fmt::Write as _;
    out.push_str("{\"");
    out.push_str(tag);
    out.push_str("\":");
    let _ = write!(out, "{n}");
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

/// A structural `Ipe.Ui` wrapper function's parameter symbols and lowered body.
///
/// Qualifies when the body is a single `UiNode` / `UiTaggedNode` / `UiText` /
/// `UiNone` kernel call over params and static descriptions — pure, no Model
/// read, no handler, no control flow, no capability. `el` / `row` / `column` /
/// `wrappedRow` / `grid` / `paragraph` / `textColumn` / `form` / `input` all
/// qualify. The params list is positional and matches the function signature;
/// `substitute_wrapper` replaces each param with the call-site argument.
pub type WrapperBody = (Vec<Symbol>, Expr);

/// One recognized hole and the `Model`-derived expression that fills it, in
/// index order. The emit compiles each `expr` in the surrounding element/child
/// position and passes the results to the runtime materializer, which splices
/// them at the matching `Hole` / `ChildrenHole` marker.
#[derive(Clone, Debug, PartialEq)]
pub struct HoleFill {
    /// The kind of runtime hole this fill supplies.
    pub kind: HoleKind,
    /// The original `Model`-derived expression, emitted by the caller in the
    /// hole's position (an `Element<M>` for a single hole, a `Vec<Element<M>>`
    /// for a children hole).
    pub expr: Expr,
}

/// Which runtime fill slice a [`HoleFill`] feeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoleKind {
    /// One `Element<M>` in a single position (a value leaf or a control-flow
    /// result) — a [`CompileUiTemplate::Hole`].
    Element,
    /// A run of `Element<M>` spliced among a node's children (a `List.map`
    /// comprehension) — a [`CompileUiTemplate::ChildrenHole`].
    Children,
}

/// The result of a hole-bearing partition: the (inert) template skeleton with
/// numbered hole markers, plus the compiled fills in index order.
#[derive(Clone, Debug, PartialEq)]
pub struct HolePartition {
    pub template: CompileUiTemplate,
    pub holes: Vec<HoleFill>,
}

/// The hole accumulator threaded through the partition recursion. `None` = pure
/// mode: holes are disallowed, so a non-static sub-expression refuses (`None`),
/// exactly the shipped behaviour. `Some` = hole mode: a `Model`-derived leaf,
/// control-flow, or `List.map` in a hole-legal position is recorded and replaced
/// by a numbered marker instead of refusing.
type Holes<'a> = Option<&'a mut Vec<HoleFill>>;

/// Reduce a static `Ipe.Ui` `view` subtree to a [`CompileUiTemplate`] with NO
/// holes admitted (pure mode), or `None` when it is not provably fully static — a
/// `Model` read, control flow, or non-literal anywhere refuses. This is the
/// shipped, fully-static path; [`ui_template_of_expr_holes`] is the superset that
/// also admits `Model`-derived holes.
///
/// `wrappers` maps each recognized `Ipe.Ui` structural-wrapper [`FuncId`] to its
/// parameter list and lowered body. When `None`, wrapper resolution is skipped
/// (pure kernel path). A `Callee::Func` whose id is absent from `wrappers` falls
/// through to recompile — conservative, never a mis-hoist.
///
/// Test-only: the production emit uses [`ui_template_of_expr_holes`] and treats an
/// empty hole set as the pure case, so this convenience entry is exercised only by
/// the partition's own unit tests.
#[cfg(test)]
pub fn ui_template_of_expr(
    expr: &Expr,
    wrappers: Option<&BTreeMap<FuncId, WrapperBody>>,
) -> Option<CompileUiTemplate> {
    ui_template_of_expr_at(expr, wrappers, 0, &mut None)
}

/// Reduce a mostly-static `Ipe.Ui` `view` subtree to a template skeleton plus its
/// hole fills, or `None` when the subtree is not templatable even with holes.
///
/// A hole is admitted only in a hole-legal position and only for a shape the emit
/// can compile back byte-identically:
/// - a `Model`-derived value leaf or control-flow (`if` / `case`) in element
///   position → a single [`CompileUiTemplate::Hole`];
/// - a `List.map` comprehension in children-list position →
///   [`CompileUiTemplate::ChildrenHole`].
///
/// Everything else stays exactly as conservative as the pure path: a handler, raw
/// markup, a non-literal attribute, or an unrecognised callee still refuses the
/// whole subtree. A subtree with no holes yields an empty `holes` vec and the same
/// template the pure path would.
pub fn ui_template_of_expr_holes(
    expr: &Expr,
    wrappers: Option<&BTreeMap<FuncId, WrapperBody>>,
) -> Option<HolePartition> {
    let mut holes = Vec::new();
    let template = ui_template_of_expr_at(expr, wrappers, 0, &mut Some(&mut holes))?;
    Some(HolePartition { template, holes })
}

fn ui_template_of_expr_at(
    expr: &Expr,
    wrappers: Option<&BTreeMap<FuncId, WrapperBody>>,
    depth: usize,
    holes: &mut Holes,
) -> Option<CompileUiTemplate> {
    if depth >= MAX_UI_TEMPLATE_DEPTH {
        return None;
    }
    // A wrapper body may be lowered with a `let tmp = <expr> in <call>` for
    // intermediate computations (e.g. the cons prepend `style "k" "v" :: attrs`).
    // β-reduce the let: substitute `Var(name)` in `body` with `value`, then
    // recurse. Since wrapper bodies are one-level, the chain terminates quickly.
    if let Expr::Let { name, value, body } = expr {
        let reduced = subst_expr(body, &[*name], &[*value.clone()]);
        return ui_template_of_expr_at(&reduced, wrappers, depth, holes);
    }
    // In hole mode, an element-position control-flow (`if` / `case`) over the
    // Model is a single element hole: the whole conditional is compiled and its
    // result spliced. In pure mode `holes` is `None`, so this is skipped and the
    // non-Call expression refuses below — the shipped behaviour.
    if matches!(expr, Expr::If { .. } | Expr::Match(_)) {
        return push_element_hole(expr, holes);
    }
    let Expr::Call { callee, args, .. } = expr else {
        return None;
    };

    // Structural wrapper: a `Callee::Func` whose id resolves to a qualifying
    // `Ipe.Ui` layout builder. Inline the wrapper body by substituting the
    // call-site arguments for the parameters, then recurse on the result. An id
    // absent from `wrappers` falls through to the recompile return below.
    // A wrapper call whose arguments are non-literal causes `static_attrs` /
    // `static_children` / `static_desc` to refuse inside the recursion, keeping
    // the overall result `None` — conservative by construction.
    if let Callee::Func(id) = callee {
        if let Some((params, body)) = wrappers.and_then(|m| m.get(id)) {
            let inlined = substitute_wrapper(body, params, args);
            return ui_template_of_expr_at(&inlined, wrappers, depth, holes);
        }
        // An unrecognised `Callee::Func` (not a known structural wrapper) stays
        // compiled — never a mis-hoist.
        return None;
    }

    let Callee::Kernel(k) = callee else {
        // `Callee::Ffi` — not a static element node.
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
            // `Ui.text <Model-derived string>` — in hole mode the whole text
            // node is a single element hole (the compiled `Ui.text (…)` result is
            // spliced); in pure mode `push_element_hole` refuses (`holes` None).
            [_] => push_element_hole(expr, holes),
            _ => None,
        },
        // `ui_node_(desc, attrs, children)`.
        KernelFn::UiNode => match args.as_slice() {
            [desc, attrs, children] => Some(CompileUiTemplate::Node {
                desc: static_desc(desc)?,
                attrs: static_attrs(attrs)?,
                children: static_children(children, wrappers, depth, holes)?,
            }),
            _ => None,
        },
        // `ui_tagged_node_(tag, desc, attrs, children)`.
        KernelFn::UiTaggedNode => match args.as_slice() {
            [Expr::Str(tag), desc, attrs, children] => Some(CompileUiTemplate::TaggedNode {
                tag: tag.clone(),
                desc: static_desc(desc)?,
                attrs: static_attrs(attrs)?,
                children: static_children(children, wrappers, depth, holes)?,
            }),
            _ => None,
        },
        // Every other element-producing kernel that ISN'T a handler / raw / config
        // shape — refuse in pure mode, but in hole mode a `Model`-derived element
        // producer is a single element hole IF it is a safe, inert element value
        // (no handler variant reachable). Conservative: only the explicitly
        // hole-eligible producers below are admitted; everything else refuses.
        _ => None,
    }
}

/// Record `expr` as an element hole and return its numbered marker, or `None` in
/// pure mode (`holes` is `None`) — where a `Model`-derived element simply refuses,
/// exactly the shipped behaviour.
///
/// The marker index is per-KIND (the count of element holes already recorded),
/// because the runtime materializer indexes the element-fill slice and the
/// children-fill slice separately. So the emit that splits the combined hole list
/// back into two ordered vecs lands each fill at the index its marker names.
///
/// An element hole is only admitted when `expr` is provably a safe, inert
/// `Element`-producing shape the emit can compile: an `if` / `case`, or a
/// `Ui.text` over a non-literal. A handler-bearing or raw-markup producer is NOT
/// admitted here (those never reach this helper — the kernel match refuses them
/// before it), so a hole can never smuggle logic or unescaped markup.
fn push_element_hole(expr: &Expr, holes: &mut Holes) -> Option<CompileUiTemplate> {
    let acc = holes.as_mut()?;
    let idx = acc.iter().filter(|h| h.kind == HoleKind::Element).count();
    acc.push(HoleFill {
        kind: HoleKind::Element,
        expr: expr.clone(),
    });
    Some(CompileUiTemplate::Hole(idx))
}

/// Record `expr` as a children hole (a `List.map` run) and return its per-kind
/// numbered marker, or `None` in pure mode.
fn push_children_hole(expr: &Expr, holes: &mut Holes) -> Option<CompileUiTemplate> {
    let acc = holes.as_mut()?;
    let idx = acc.iter().filter(|h| h.kind == HoleKind::Children).count();
    acc.push(HoleFill {
        kind: HoleKind::Children,
        expr: expr.clone(),
    });
    Some(CompileUiTemplate::ChildrenHole(idx))
}

/// Substitute call-site `args` for `params` in `body` — a minimal positional
/// substitution for the structural-wrapper inline path. Each param symbol that
/// appears as an `Expr::Var` or `Expr::CloneVar` in `body` is replaced by the
/// corresponding argument expression. The lowerer may introduce `Let`
/// bindings for intermediate expressions (e.g. the cons prepend in a wrapper
/// body); `subst_expr` descends into those so all parameter references are
/// replaced regardless of nesting depth.
fn substitute_wrapper(body: &Expr, params: &[Symbol], args: &[Expr]) -> Expr {
    subst_expr(body, params, args)
}

fn subst_expr(expr: &Expr, params: &[Symbol], args: &[Expr]) -> Expr {
    match expr {
        Expr::Var(sym) | Expr::CloneVar(sym) => {
            // Replace a parameter reference with its call-site argument.
            // A parameter with no corresponding argument is an arity mismatch —
            // leave as-is so the downstream static_* fns refuse (returning
            // None), which is the conservative outcome.
            params.iter().position(|p| p == sym).map_or_else(
                || expr.clone(),
                |pos| args.get(pos).cloned().unwrap_or_else(|| expr.clone()),
            )
        }

        // Structural descent: a wrapper body may contain kernel calls, list
        // literals, Cons prepend, and Let bindings (for intermediate exprs).
        Expr::Call {
            callee,
            args: call_args,
            pin,
            on_form,
        } => Expr::Call {
            callee: callee.clone(),
            args: call_args
                .iter()
                .map(|a| subst_expr(a, params, args))
                .collect(),
            pin: *pin,
            on_form: *on_form,
        },

        Expr::List { elem, items } => Expr::List {
            elem: elem.clone(),
            items: items.iter().map(|i| subst_expr(i, params, args)).collect(),
        },

        Expr::Cons { head, tail } => Expr::Cons {
            head: Box::new(subst_expr(head, params, args)),
            tail: Box::new(subst_expr(tail, params, args)),
        },

        // The lowerer may bind an intermediate expression in a `Let`. Recurse
        // into both the bound value and the body so that any `Expr::Var(param)`
        // inside either arm is replaced. The let-bound name (`Let::name`) is a
        // local binder — it shadows any outer param with the same symbol, but
        // wrapper bodies are guaranteed not to shadow their own params (they are
        // a single return expression over the param list), so no shadowing check
        // is needed here.
        Expr::Let { name, value, body } => Expr::Let {
            name: *name,
            value: Box::new(subst_expr(value, params, args)),
            body: Box::new(subst_expr(body, params, args)),
        },

        // Leaves and shapes not present in a structural wrapper body pass through.
        _ => expr.clone(),
    }
}

/// Reduce a child `Ipe.Ui` node list to templates (plus any holes), or `None`.
///
/// Two shapes reach here:
/// - a literal `Expr::List` of child elements — each item is recursed (a
///   `Model`-derived value leaf or control-flow item becomes an element hole in
///   hole mode; a `List.map` item becomes a children hole);
/// - the whole children expr being a single `List.map` comprehension (the common
///   `column [] (List.map itemView model.items)` shape) — one children hole for
///   the entire run.
///
/// In pure mode (`holes` is `None`) both hole paths refuse, leaving the shipped
/// static-only behaviour: only a literal list of provably-static children reduces.
fn static_children(
    children: &Expr,
    wrappers: Option<&BTreeMap<FuncId, WrapperBody>>,
    depth: usize,
    holes: &mut Holes,
) -> Option<Vec<CompileUiTemplate>> {
    // The whole children list is a `List.map` comprehension → one children hole.
    if is_list_map_call(children) {
        return Some(vec![push_children_hole(children, holes)?]);
    }
    let Expr::List { items, .. } = children else {
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        // A `List.map` appearing as a list item still splices a run of children.
        if is_list_map_call(item) {
            out.push(push_children_hole(item, holes)?);
        } else {
            out.push(ui_template_of_expr_at(
                item,
                wrappers,
                depth.saturating_add(1),
                holes,
            )?);
        }
    }
    Some(out)
}

/// Is `expr` a `List.map f xs` call? The comprehension shape a children hole
/// splices. Only the direct `List.map` kernel qualifies — a user helper aliasing
/// it (a `Callee::Func`) stays conservative and refuses (recompiles).
const fn is_list_map_call(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Call {
            callee: Callee::Kernel(KernelFn::ListMap),
            ..
        }
    )
}

/// Reduce a literal list of `Ipe.Ui` attributes to inert data, or `None` when
/// any element is not an accepted static attribute.
///
/// Accepts both `Expr::List` (the direct-kernel shape) and a `Expr::Cons` chain
/// (the shape a structural wrapper produces after substitution — e.g. the
/// `style "__row" "true" :: attrs` prepend in `Ui.row`'s lowered body).
fn static_attrs(attrs: &Expr) -> Option<Vec<CompileUiAttr>> {
    let mut out = Vec::new();
    collect_static_attrs(attrs, &mut out)?;
    Some(out)
}

/// Append the static attrs in `expr` to `out`. Handles both a literal
/// `Expr::List` and a right-spine `Expr::Cons { head, tail }` chain whose
/// eventual tail is a `List`.
fn collect_static_attrs(expr: &Expr, out: &mut Vec<CompileUiAttr>) -> Option<()> {
    match expr {
        Expr::List { items, .. } => {
            for item in items {
                out.push(static_attr(item)?);
            }
            Some(())
        }
        // A `Cons` head is one prepended attribute; the tail is recursed.
        Expr::Cons { head, tail } => {
            out.push(static_attr(head)?);
            collect_static_attrs(tail, out)
        }
        // Any other shape (a Var, a non-list call) is not a provably-static
        // attribute list — refuse.
        _ => None,
    }
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
        // ── single-`Color` attributes over a literal color ────────────────
        (KernelFn::FontColor, [c]) => Some(CompileUiAttr::FontColor(static_color(c)?)),
        (KernelFn::BackgroundColor, [c]) => Some(CompileUiAttr::BgColor(static_color(c)?)),
        (KernelFn::BorderColor, [c]) => Some(CompileUiAttr::BorderColor(static_color(c)?)),
        // ── single-`Float` attributes over a literal ──────────────────────
        (KernelFn::FontLetterSpacing, [Expr::Float(v)]) => {
            Some(CompileUiAttr::FontLetterSpacing(*v))
        }
        (KernelFn::FontWordSpacing, [Expr::Float(v)]) => Some(CompileUiAttr::FontWordSpacing(*v)),
        // Every other attribute kernel — a handler, a shadow record, an
        // aspect-ratio / gradient, a `Model`-derived color/float, a pseudo-rule, a
        // nearby overlay, the debug outline, or an unrecognised one — is not an
        // accepted inert attribute. Refuse: keep the subtree compiled.
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

/// Reduce a `Color`-producing kernel call over literal arguments to inert data,
/// or `None`. Only the literal color builders templatize; a `Model`-derived color
/// (a `Var` / `Access`) or a non-literal channel refuses and keeps the subtree
/// compiled. The alpha of `Ui.rgb` / the named colors matches the runtime helper
/// bodies (`ui_rgb_` → `a = 1.0`, `ui_transparent_` → `a = 0.0`).
fn static_color(expr: &Expr) -> Option<CompileUiColor> {
    let Expr::Call { callee, args, .. } = expr else {
        return None;
    };
    let Callee::Kernel(k) = callee else {
        return None;
    };
    match (k, args.as_slice()) {
        (KernelFn::UiRgb, [Expr::Int(red), Expr::Int(green), Expr::Int(blue)]) => {
            Some(CompileUiColor {
                r: *red,
                g: *green,
                b: *blue,
                a: 1.0,
            })
        }
        (
            KernelFn::UiRgba,
            [
                Expr::Int(red),
                Expr::Int(green),
                Expr::Int(blue),
                Expr::Float(alpha),
            ],
        ) => Some(CompileUiColor {
            r: *red,
            g: *green,
            b: *blue,
            a: *alpha,
        }),
        (KernelFn::UiWhite, []) => Some(CompileUiColor {
            r: 255,
            g: 255,
            b: 255,
            a: 1.0,
        }),
        (KernelFn::UiBlack, []) => Some(CompileUiColor {
            r: 0,
            g: 0,
            b: 0,
            a: 1.0,
        }),
        (KernelFn::UiTransparent, []) => Some(CompileUiColor {
            r: 0,
            g: 0,
            b: 0,
            a: 0.0,
        }),
        // A `Model`-derived color, `Ui.colorCss` (a string form the runtime
        // `Color` variant does not carry), or any non-literal channel refuses.
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
    use std::collections::BTreeMap;

    use super::{
        CompileUiAttr, CompileUiColor, CompileUiDesc, CompileUiLength, CompileUiTemplate,
        WrapperBody, ui_template_of_expr,
    };
    use ipe_intern::Symbol;
    use ipe_ir::{CallPin, Callee, Expr, FuncId, IrType, KernelFn, OnFormKind, UiCtor};

    fn kcall(k: KernelFn, args: Vec<Expr>) -> Expr {
        Expr::Call {
            callee: Callee::Kernel(k),
            args,
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }
    }

    impl CompileUiAttr {
        /// Serialize a single attribute to its JSON — a test-only view of the
        /// private `write_json`, used by the byte-shape pins.
        fn attr_json(&self) -> String {
            let mut out = String::new();
            self.write_json(&mut out);
            out
        }
    }

    fn fcall(id: u32, args: Vec<Expr>) -> Expr {
        Expr::Call {
            callee: Callee::Func(FuncId::from_raw(id)),
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

    fn sym(n: u32) -> Symbol {
        Symbol::from_raw(n)
    }

    /// Build a wrapper table with one entry: func id `id` with params `params`
    /// and body `body`.
    fn one_wrapper(id: u32, params: Vec<Symbol>, body: Expr) -> BTreeMap<FuncId, WrapperBody> {
        let mut m = BTreeMap::new();
        m.insert(FuncId::from_raw(id), (params, body));
        m
    }

    // ── acceptance: direct kernel calls ──────────────────────────────────────

    #[test]
    fn text_node_templates() {
        assert_eq!(
            ui_template_of_expr(&text("hi"), None),
            Some(CompileUiTemplate::Text("hi".to_string()))
        );
    }

    #[test]
    fn none_templates_to_empty() {
        assert_eq!(
            ui_template_of_expr(&kcall(KernelFn::UiNone, vec![]), None),
            Some(CompileUiTemplate::Empty)
        );
    }

    #[test]
    fn node_with_inert_attrs_and_children_templates() {
        // A `Ui.row`-equivalent shape lowered to
        // `ui_node_(descNone, [__row marker; spacing 8; width (px 16)], [text "a"])`.
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
        let got = ui_template_of_expr(&node, None).expect("templatable");
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
        let got = ui_template_of_expr(&node, None).expect("templatable");
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
        let got = ui_template_of_expr(&node, None).expect("templatable");
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

    // ── acceptance: structural wrapper resolution ─────────────────────────────

    /// `el attrs child = node descNone attrs [child]`
    /// — inlines to `Node { desc: NoDescription, attrs: [spacing 8], children: [text "hi"] }`.
    #[test]
    fn wrapper_el_inlines_to_node() {
        // params: (attrs_sym=10, child_sym=11)
        // body:   ui_node_(descNone, Var(10), List [Var(11)])
        let attrs_sym = sym(10);
        let child_sym = sym(11);
        let body = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                Expr::Var(attrs_sym),
                Expr::List {
                    elem: IrType::Ui {
                        ctor: UiCtor::Element,
                        msg: Box::new(IrType::Int),
                    },
                    items: vec![Expr::Var(child_sym)],
                },
            ],
        );
        let wrappers = one_wrapper(1, vec![attrs_sym, child_sym], body);

        // call: el [spacing 8] (text "hi")
        let call = fcall(
            1,
            vec![
                attr_list(vec![kcall(KernelFn::UiSpacing, vec![Expr::Int(8)])]),
                text("hi"),
            ],
        );
        let got = ui_template_of_expr(&call, Some(&wrappers)).expect("el templatizes");
        assert_eq!(
            got,
            CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![CompileUiAttr::Spacing(8)],
                children: vec![CompileUiTemplate::Text("hi".to_string())],
            }
        );
    }

    /// `row attrs children = node descNone (style "__row" "true" :: attrs) children`
    /// — the Cons prepend of the marker attr is folded into the accepted attr list.
    #[test]
    fn wrapper_row_cons_prepend_templatizes() {
        // params: (attrs_sym=20, children_sym=21)
        // body:   ui_node_(descNone, Cons(style "__row" "true", Var(20)), Var(21))
        let attrs_sym = sym(20);
        let children_sym = sym(21);
        let marker = kcall(
            KernelFn::UiStyle,
            vec![
                Expr::Str("__row".to_string()),
                Expr::Str("true".to_string()),
            ],
        );
        let body = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                Expr::Cons {
                    head: Box::new(marker),
                    tail: Box::new(Expr::Var(attrs_sym)),
                },
                Expr::Var(children_sym),
            ],
        );
        let wrappers = one_wrapper(2, vec![attrs_sym, children_sym], body);

        // call: row [spacing 4] [text "a", text "b"]
        let call = fcall(
            2,
            vec![
                attr_list(vec![kcall(KernelFn::UiSpacing, vec![Expr::Int(4)])]),
                child_list(vec![text("a"), text("b")]),
            ],
        );
        let got = ui_template_of_expr(&call, Some(&wrappers)).expect("row templatizes");
        assert_eq!(
            got,
            CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![
                    CompileUiAttr::Style("__row".to_string(), "true".to_string()),
                    CompileUiAttr::Spacing(4),
                ],
                children: vec![
                    CompileUiTemplate::Text("a".to_string()),
                    CompileUiTemplate::Text("b".to_string()),
                ],
            }
        );
    }

    /// `column attrs children = node descNone (style "__col" "true" :: attrs) children`
    #[test]
    fn wrapper_column_cons_prepend_templatizes() {
        let attrs_sym = sym(30);
        let children_sym = sym(31);
        let marker = kcall(
            KernelFn::UiStyle,
            vec![
                Expr::Str("__col".to_string()),
                Expr::Str("true".to_string()),
            ],
        );
        let body = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                Expr::Cons {
                    head: Box::new(marker),
                    tail: Box::new(Expr::Var(attrs_sym)),
                },
                Expr::Var(children_sym),
            ],
        );
        let wrappers = one_wrapper(3, vec![attrs_sym, children_sym], body);

        let call = fcall(3, vec![attr_list(vec![]), child_list(vec![text("x")])]);
        let got = ui_template_of_expr(&call, Some(&wrappers)).expect("column templatizes");
        assert_eq!(
            got,
            CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![CompileUiAttr::Style(
                    "__col".to_string(),
                    "true".to_string()
                )],
                children: vec![CompileUiTemplate::Text("x".to_string())],
            }
        );
    }

    /// `text` is a kernel alias — the kernel path already handles `Ui.text`
    /// directly; the wrapper table is not consulted for a kernel callee.
    #[test]
    fn text_kernel_templatizes_without_wrapper_table() {
        assert_eq!(
            ui_template_of_expr(&text("hello"), None),
            Some(CompileUiTemplate::Text("hello".to_string()))
        );
    }

    // ── refusal: wrapper with non-literal / model-dependent args ─────────────

    /// A wrapper call whose attrs contain a `Model`-read (`Expr::Var`) refuses.
    #[test]
    fn wrapper_with_model_dependent_arg_refuses() {
        let attrs_sym = sym(40);
        let child_sym = sym(41);
        let body = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                Expr::Var(attrs_sym),
                Expr::List {
                    elem: IrType::Ui {
                        ctor: UiCtor::Element,
                        msg: Box::new(IrType::Int),
                    },
                    items: vec![Expr::Var(child_sym)],
                },
            ],
        );
        let wrappers = one_wrapper(4, vec![attrs_sym, child_sym], body);

        // Attrs list is a Var (Model-dependent) — must refuse.
        let call = fcall(
            4,
            vec![
                Expr::Var(sym(99)), // model-dependent attrs
                text("hi"),
            ],
        );
        assert_eq!(ui_template_of_expr(&call, Some(&wrappers)), None);
    }

    /// A wrapper call with a handler attribute refuses.
    #[test]
    fn wrapper_with_event_handler_refuses() {
        let attrs_sym = sym(50);
        let child_sym = sym(51);
        let body = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                Expr::Var(attrs_sym),
                Expr::List {
                    elem: IrType::Ui {
                        ctor: UiCtor::Element,
                        msg: Box::new(IrType::Int),
                    },
                    items: vec![Expr::Var(child_sym)],
                },
            ],
        );
        let wrappers = one_wrapper(5, vec![attrs_sym, child_sym], body);

        let call = fcall(
            5,
            vec![
                attr_list(vec![kcall(KernelFn::UiOnClick, vec![Expr::Var(sym(99))])]),
                text("hi"),
            ],
        );
        assert_eq!(ui_template_of_expr(&call, Some(&wrappers)), None);
    }

    /// An unrecognised `Callee::Func` (not in the wrapper table) refuses.
    #[test]
    fn unrecognised_func_callee_refuses() {
        let wrappers: BTreeMap<FuncId, WrapperBody> = BTreeMap::new();
        let call = fcall(99, vec![attr_list(vec![]), child_list(vec![])]);
        assert_eq!(ui_template_of_expr(&call, Some(&wrappers)), None);
    }

    /// A `Callee::Func` with `wrappers = None` refuses (no table — conservative).
    #[test]
    fn func_callee_without_wrapper_table_refuses() {
        let call = fcall(1, vec![attr_list(vec![]), child_list(vec![])]);
        assert_eq!(ui_template_of_expr(&call, None), None);
    }

    // ── refusal: not provably static ⇒ keep compiled (None) ──────────────────

    #[test]
    fn model_read_child_refuses() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![]),
                child_list(vec![Expr::Var(sym(1))]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node, None), None);
    }

    #[test]
    fn embedded_raw_html_child_refuses() {
        // `Ui.html (...)` embeds a `Html` node — not a static `Ipe.Ui` node.
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![]),
                child_list(vec![kcall(KernelFn::UiHtml, vec![Expr::Var(sym(2))])]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node, None), None);
    }

    #[test]
    fn event_handler_attribute_refuses() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![kcall(KernelFn::UiOnClick, vec![Expr::Var(sym(3))])]),
                child_list(vec![]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node, None), None);
    }

    #[test]
    fn literal_color_bearing_attribute_templatizes() {
        // `Font.color (rgb 1 2 3)` is inert and over a literal color, so it
        // templatizes with the alpha defaulted to `1.0` (matching `ui_rgb_`). A
        // Model-derived color still refuses (`model_derived_color_refuses`).
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
        assert_eq!(
            ui_template_of_expr(&node, None),
            Some(CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![CompileUiAttr::FontColor(CompileUiColor {
                    r: 1,
                    g: 2,
                    b: 3,
                    a: 1.0,
                })],
                children: vec![],
            })
        );
    }

    #[test]
    fn non_literal_attr_value_refuses() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![kcall(
                    KernelFn::UiStyle,
                    vec![Expr::Str("color".to_string()), Expr::Var(sym(4))],
                )]),
                child_list(vec![]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node, None), None);
    }

    #[test]
    fn non_literal_tag_refuses() {
        let node = kcall(
            KernelFn::UiTaggedNode,
            vec![
                Expr::Var(sym(5)),
                desc_none(),
                attr_list(vec![]),
                child_list(vec![]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node, None), None);
    }

    #[test]
    fn button_record_config_refuses() {
        // `Ui.button` carries an `onPress` handler record — never a static node.
        assert_eq!(
            ui_template_of_expr(&kcall(KernelFn::UiButton, vec![]), None),
            None
        );
    }

    // ── acceptance: single-Color and single-Float attributes ─────────────────

    fn rgb(r: i64, g: i64, b: i64) -> Expr {
        kcall(
            KernelFn::UiRgb,
            vec![Expr::Int(r), Expr::Int(g), Expr::Int(b)],
        )
    }

    fn rgba(r: i64, g: i64, b: i64, a: f64) -> Expr {
        kcall(
            KernelFn::UiRgba,
            vec![Expr::Int(r), Expr::Int(g), Expr::Int(b), Expr::Float(a)],
        )
    }

    #[test]
    fn font_color_rgb_templatizes() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![kcall(KernelFn::FontColor, vec![rgb(10, 20, 30)])]),
                child_list(vec![]),
            ],
        );
        let got = ui_template_of_expr(&node, None).expect("font color templatizes");
        assert_eq!(
            got,
            CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![CompileUiAttr::FontColor(super::CompileUiColor {
                    r: 10,
                    g: 20,
                    b: 30,
                    a: 1.0,
                })],
                children: vec![],
            }
        );
    }

    #[test]
    fn bg_and_border_color_rgba_templatize() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![
                    kcall(KernelFn::BackgroundColor, vec![rgba(1, 2, 3, 0.25)]),
                    kcall(
                        KernelFn::BorderColor,
                        vec![kcall(KernelFn::UiBlack, vec![])],
                    ),
                ]),
                child_list(vec![]),
            ],
        );
        let got = ui_template_of_expr(&node, None).expect("bg/border color templatize");
        assert_eq!(
            got,
            CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![
                    CompileUiAttr::BgColor(super::CompileUiColor {
                        r: 1,
                        g: 2,
                        b: 3,
                        a: 0.25,
                    }),
                    CompileUiAttr::BorderColor(super::CompileUiColor {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 1.0,
                    }),
                ],
                children: vec![],
            }
        );
    }

    #[test]
    fn letter_and_word_spacing_float_templatize() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![
                    kcall(KernelFn::FontLetterSpacing, vec![Expr::Float(1.5)]),
                    kcall(KernelFn::FontWordSpacing, vec![Expr::Float(0.5)]),
                ]),
                child_list(vec![]),
            ],
        );
        let got = ui_template_of_expr(&node, None).expect("spacing templatizes");
        assert_eq!(
            got,
            CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![
                    CompileUiAttr::FontLetterSpacing(1.5),
                    CompileUiAttr::FontWordSpacing(0.5),
                ],
                children: vec![],
            }
        );
    }

    // ── refusal: Model-derived / non-literal color, colorCss, deferred shadow ─

    #[test]
    fn model_derived_color_refuses() {
        // `Font.color model.accent` — the color arg is a Model read, not a literal.
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![kcall(KernelFn::FontColor, vec![Expr::Var(sym(7))])]),
                child_list(vec![]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node, None), None);
    }

    #[test]
    fn color_css_string_form_refuses() {
        // `Ui.colorCss "red"` is a string color the runtime `Color::Rgba` variant
        // does not carry — refuse, keep compiled.
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![kcall(
                    KernelFn::FontColor,
                    vec![kcall(
                        KernelFn::UiColorCss,
                        vec![Expr::Str("red".to_string())],
                    )],
                )]),
                child_list(vec![]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node, None), None);
    }

    #[test]
    fn model_derived_letter_spacing_refuses() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![kcall(
                    KernelFn::FontLetterSpacing,
                    vec![Expr::Var(sym(8))],
                )]),
                child_list(vec![]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node, None), None);
    }

    // ── holes: value / control-flow / list ───────────────────────────────────

    use super::{HoleFill, HoleKind, ui_template_of_expr_holes};

    /// A `Ui.text <non-literal>` — a value leaf that becomes an element hole.
    fn model_text(binder: u32) -> Expr {
        kcall(KernelFn::UiText, vec![Expr::Var(sym(binder))])
    }

    // A value hole: `column [] [text "count: ", text model.count]` — the static
    // text stays in the template, the model-derived text is `Hole(0)` + a fill.
    #[test]
    fn value_hole_partitions_and_collects_fill() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![]),
                child_list(vec![text("count: "), model_text(50)]),
            ],
        );
        let part = ui_template_of_expr_holes(&node, None).expect("value-hole subtree templatizes");
        assert_eq!(
            part.template,
            CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![],
                children: vec![
                    CompileUiTemplate::Text("count: ".to_string()),
                    CompileUiTemplate::Hole(0),
                ],
            }
        );
        assert_eq!(
            part.holes,
            vec![HoleFill {
                kind: HoleKind::Element,
                expr: model_text(50),
            }]
        );
    }

    // A control-flow hole: an `if` in element position is one element hole.
    #[test]
    fn control_flow_hole_partitions_the_if() {
        let cond = kcall(KernelFn::UiText, vec![Expr::Str("ignored".to_string())]);
        let iff = Expr::If {
            cond: Box::new(Expr::Var(sym(60))),
            then_: Box::new(text("on")),
            else_: Box::new(text("off")),
        };
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![]),
                child_list(vec![iff.clone(), cond]),
            ],
        );
        let part = ui_template_of_expr_holes(&node, None).expect("if-hole templatizes");
        assert_eq!(
            part.template,
            CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![],
                children: vec![
                    CompileUiTemplate::Hole(0),
                    CompileUiTemplate::Text("ignored".to_string()),
                ],
            }
        );
        assert_eq!(
            part.holes,
            vec![HoleFill {
                kind: HoleKind::Element,
                expr: iff,
            }]
        );
    }

    // A list hole: the whole children arg is `List.map itemView model.items`.
    #[test]
    fn list_map_children_is_one_children_hole() {
        let listmap = kcall(
            KernelFn::ListMap,
            vec![Expr::Var(sym(70)), Expr::Var(sym(71))],
        );
        let node = kcall(
            KernelFn::UiNode,
            vec![desc_none(), attr_list(vec![]), listmap.clone()],
        );
        let part = ui_template_of_expr_holes(&node, None).expect("list-map templatizes");
        assert_eq!(
            part.template,
            CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![],
                children: vec![CompileUiTemplate::ChildrenHole(0)],
            }
        );
        assert_eq!(
            part.holes,
            vec![HoleFill {
                kind: HoleKind::Children,
                expr: listmap,
            }]
        );
    }

    // Per-kind indexing: two element holes and one children hole, interleaved,
    // number 0/1 (elements) and 0 (children) within their own slice.
    #[test]
    fn holes_number_per_kind() {
        let listmap = kcall(
            KernelFn::ListMap,
            vec![Expr::Var(sym(80)), Expr::Var(sym(81))],
        );
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![]),
                child_list(vec![model_text(82), listmap, model_text(83)]),
            ],
        );
        let part = ui_template_of_expr_holes(&node, None).expect("mixed holes templatize");
        assert_eq!(
            part.template,
            CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![],
                children: vec![
                    CompileUiTemplate::Hole(0),
                    CompileUiTemplate::ChildrenHole(0),
                    CompileUiTemplate::Hole(1),
                ],
            }
        );
        assert_eq!(part.holes.len(), 3);
    }

    // Pure mode still refuses a model-derived leaf (no holes admitted) — the
    // shipped conservative behaviour is unchanged for the no-hole entry.
    #[test]
    fn pure_mode_refuses_model_leaf() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![]),
                child_list(vec![model_text(90)]),
            ],
        );
        assert_eq!(ui_template_of_expr(&node, None), None);
    }

    // A handler-bearing subtree still refuses WHOLE even in hole mode — a hole
    // never covers a handler (that is the deferred, guardian-gated increment).
    #[test]
    fn hole_mode_still_refuses_handler_attr() {
        let node = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                attr_list(vec![kcall(KernelFn::UiOnClick, vec![Expr::Var(sym(91))])]),
                child_list(vec![model_text(92)]),
            ],
        );
        assert_eq!(ui_template_of_expr_holes(&node, None), None);
    }

    // JSON shape for the hole markers — pinned against the runtime serde form.
    #[test]
    fn json_hole_marker_shapes() {
        assert_eq!(CompileUiTemplate::Hole(3).to_json(), r#"{"Hole":3}"#);
        assert_eq!(
            CompileUiTemplate::ChildrenHole(0).to_json(),
            r#"{"ChildrenHole":0}"#
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
                CompileUiAttr::FontColor(CompileUiColor {
                    r: 10,
                    g: 20,
                    b: 30,
                    a: 1.0,
                }),
                CompileUiAttr::BgColor(CompileUiColor {
                    r: 1,
                    g: 2,
                    b: 3,
                    a: 0.25,
                }),
                CompileUiAttr::FontLetterSpacing(1.5),
            ],
            children: vec![
                CompileUiTemplate::Text("hi".to_string()),
                CompileUiTemplate::Empty,
            ],
        };
        let expected = concat!(
            r#"{"TaggedNode":{"tag":"section","desc":{"DescHeading":2},"attrs":["#,
            r#"{"Width":{"Max":[320,{"Vh":80}]}},{"Padding":[1,2,3,4]},{"Style":["k","v"]},"#,
            r#"{"AlignX":"CenterX"},"Pointer",{"FontAlign":"center"},"#,
            r#"{"FontColor":{"r":10,"g":20,"b":30,"a":1.0}},"#,
            r#"{"BgColor":{"r":1,"g":2,"b":3,"a":0.25}},{"FontLetterSpacing":1.5}],"#,
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

    // Byte-shape pin for the single-Color attributes: the `UiColor` struct form
    // `{"r":R,"g":G,"b":B,"a":A}` with the alpha in `ryu`'s shortest form
    // (`1.0` / `0.25`). Verified byte-identical to the runtime serde form by the
    // runtime pin `backend_baked_json_decodes_to_the_described_tree` (extended to
    // carry a color), so a drift on either side fails one of the two pins.
    #[test]
    fn json_color_attr_shape() {
        assert_eq!(
            CompileUiAttr::FontColor(CompileUiColor {
                r: 10,
                g: 20,
                b: 30,
                a: 1.0,
            })
            .attr_json(),
            r#"{"FontColor":{"r":10,"g":20,"b":30,"a":1.0}}"#
        );
        assert_eq!(
            CompileUiAttr::BgColor(CompileUiColor {
                r: 1,
                g: 2,
                b: 3,
                a: 0.25,
            })
            .attr_json(),
            r#"{"BgColor":{"r":1,"g":2,"b":3,"a":0.25}}"#
        );
    }

    // Byte-shape pin for the single-Float attributes: the alpha/spacing float in
    // `ryu` shortest form. `ryu` always renders a decimal point (`1.5`, `0.5`),
    // matching `serde_json`'s `write_f64`.
    #[test]
    fn json_float_attr_shape() {
        assert_eq!(
            CompileUiAttr::FontLetterSpacing(1.5).attr_json(),
            r#"{"FontLetterSpacing":1.5}"#
        );
        assert_eq!(
            CompileUiAttr::FontWordSpacing(0.5).attr_json(),
            r#"{"FontWordSpacing":0.5}"#
        );
    }

    // `ryu`'s float spelling matches `serde_json`'s exactly (serde_json's
    // `write_f64` calls `ryu::Buffer::format_finite`). This pins the equivalence
    // directly for the alpha values a template can carry, so the single-source-of-
    // truth claim is a standing check, not a comment.
    #[test]
    fn ryu_float_matches_serde_json() {
        for a in [1.0f64, 0.0, 0.5, 0.25, 0.18, 0.08, 0.9, 1.5, 100.0, 0.1] {
            let mut baked = String::new();
            super::push_f64(a, &mut baked);
            assert_eq!(
                baked,
                serde_json::to_string(&a).expect("serialize f64"),
                "ryu float spelling must equal serde_json for {a}"
            );
        }
    }

    /// `column [padding 8] [row [htmlAttr "class" "marker"] [text "uno"], text "two"]`
    /// — two wrappers, one nested as child of the other, both must templatize.
    /// Mirrors the exact fixture used in the wrapper SEAL test after the
    /// structural (add-child) edit.
    #[test]
    fn wrapper_column_containing_row_and_text_templatizes() {
        // row: id=2, params=(attrs_sym=20, children_sym=21)
        // body: node descNone (style "__row" "true" :: Var(20)) Var(21)
        let row_attrs_sym = sym(20);
        let row_children_sym = sym(21);
        let row_marker = kcall(
            KernelFn::UiStyle,
            vec![
                Expr::Str("__row".to_string()),
                Expr::Str("true".to_string()),
            ],
        );
        let row_body = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                Expr::Cons {
                    head: Box::new(row_marker),
                    tail: Box::new(Expr::Var(row_attrs_sym)),
                },
                Expr::Var(row_children_sym),
            ],
        );

        // column: id=3, params=(attrs_sym=30, children_sym=31)
        // body: node descNone (style "__col" "true" :: Var(30)) Var(31)
        let col_attrs_sym = sym(30);
        let col_children_sym = sym(31);
        let col_marker = kcall(
            KernelFn::UiStyle,
            vec![
                Expr::Str("__col".to_string()),
                Expr::Str("true".to_string()),
            ],
        );
        let col_body = kcall(
            KernelFn::UiNode,
            vec![
                desc_none(),
                Expr::Cons {
                    head: Box::new(col_marker),
                    tail: Box::new(Expr::Var(col_attrs_sym)),
                },
                Expr::Var(col_children_sym),
            ],
        );

        let mut wrappers: BTreeMap<FuncId, WrapperBody> = BTreeMap::new();
        wrappers.insert(
            FuncId::from_raw(2),
            (vec![row_attrs_sym, row_children_sym], row_body),
        );
        wrappers.insert(
            FuncId::from_raw(3),
            (vec![col_attrs_sym, col_children_sym], col_body),
        );

        // call: column [padding 8] [row [htmlAttr "class" "marker"] [text "uno"], text "two"]
        let row_call = fcall(
            2,
            vec![
                attr_list(vec![kcall(
                    KernelFn::UiHtmlAttribute,
                    vec![
                        Expr::Str("class".to_string()),
                        Expr::Str("marker".to_string()),
                    ],
                )]),
                child_list(vec![text("uno")]),
            ],
        );
        let col_call = fcall(
            3,
            vec![
                attr_list(vec![kcall(KernelFn::UiPadding, vec![Expr::Int(8)])]),
                child_list(vec![row_call, text("two")]),
            ],
        );

        let got = ui_template_of_expr(&col_call, Some(&wrappers))
            .expect("column(row(...), text) must templatize");
        assert_eq!(
            got,
            CompileUiTemplate::Node {
                desc: CompileUiDesc::NoDescription,
                attrs: vec![
                    CompileUiAttr::Style("__col".to_string(), "true".to_string()),
                    CompileUiAttr::Padding(8, 8, 8, 8),
                ],
                children: vec![
                    CompileUiTemplate::Node {
                        desc: CompileUiDesc::NoDescription,
                        attrs: vec![
                            CompileUiAttr::Style("__row".to_string(), "true".to_string()),
                            CompileUiAttr::Attribute("class".to_string(), "marker".to_string()),
                        ],
                        children: vec![CompileUiTemplate::Text("uno".to_string())],
                    },
                    CompileUiTemplate::Text("two".to_string()),
                ],
            }
        );
    }
}

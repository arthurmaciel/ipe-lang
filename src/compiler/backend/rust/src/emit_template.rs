//! Static-subtree partition: recognise a provably-static `Ipe.Html` `view`
//! subtree in the IR and reduce it to an inert serialized template the runtime
//! materializes at render.
//!
//! A subtree is TEMPLATABLE iff it is built entirely from literal `Ipe.Html`
//! element / text / attribute kernels over string literals — no `Model` read
//! ([`Expr::Var`] / [`Expr::Access`]), no control flow ([`Expr::If`] /
//! [`Expr::Match`]), no handler (an event-attribute kernel), and no raw markup
//! ([`ipe_ir::KernelFn::HtmlRawNode`] and the trusted-markup node kernels).
//! [`template_of_expr`] returns `Some` only for such a subtree and `None` for
//! everything else, so an unprovable subtree stays compiled (the recompile
//! path) — conservative by construction, exactly the appearance-vs-logic split.
//!
//! Fail-closed over node/attribute kernels: only the templatable `Ipe.Html`
//! element / text / string-attribute builders reduce to a template; EVERY other
//! kernel — raw / trusted markup, value builders, `Ui.*` nodes, handlers,
//! effects — refuses and keeps the subtree compiled. A new kernel is therefore
//! never templated by default; it must be added to the accept set deliberately.
//!
//! ## Why a shape match, not the purity analysis
//!
//! [`crate::const_fold`] proves a pure VALUE-builder pipeline constant; a static
//! subtree is a stricter, simpler property — a syntactically-literal element
//! tree. A `Var` / `Access` / `If` / `Match` / foreign call anywhere in the
//! subtree fails this match, which IS "no Model read, no control flow, no
//! handler". The exhaustive shape match enforces it directly; `const_fold` stays
//! the mechanism for the leaf VALUES, composable but not needed here.
//!
//! ## Inert by construction
//!
//! A [`CompileTemplate`] carries only tag / attribute-key / attribute-value /
//! text `String`s — it has no raw-markup and no handler variant, mirroring the
//! runtime `Template`. Its JSON serialization ([`CompileTemplate::to_json`]) is
//! byte-identical to the runtime `Template`'s serde form (pinned by a test), so
//! the emitted baked default decodes back into exactly the tree it described and
//! materializes byte-identically to the direct inline emit — dev == prod.

use ipe_ir::{Callee, Expr, KernelFn};

/// The render/decode nesting ceiling, mirrored from the runtime
/// (`ipe_runtime::web::template::MAX_TEMPLATE_DEPTH`, itself the HTML render
/// depth cap `MAX_HTML_DEPTH`). A subtree deeper than this is NOT templated
/// (returns `None`), so the emitter never bakes a template the runtime
/// materializer would refuse. Kept as a local constant rather than a runtime
/// import because the backend does not depend on the runtime crate outside
/// tests; a drift test pins the two together. Staying at-or-below the runtime
/// cap is the soundness requirement — equal keeps them exactly in step.
pub const MAX_TEMPLATE_DEPTH: usize = 1024;

/// A static attribute reduced to an inert key/value string pair. Mirrors the
/// runtime `TemplateAttr` — strings only, never a handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileTemplateAttr {
    key: String,
    value: String,
}

/// An inert, fully-static `Ipe.Html` subtree reduced to data. The two variants
/// are the ONLY shapes a static subtree takes; there is deliberately no
/// raw-markup and no handler variant — that absence is the security guarantee,
/// enforced by the type (make-invalid-states-unrepresentable), exactly as the
/// runtime `Template`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileTemplate {
    Element {
        tag: String,
        attrs: Vec<CompileTemplateAttr>,
        children: Vec<Self>,
    },
    Text(String),
}

impl CompileTemplate {
    /// Serialize to the JSON the runtime `Template` decodes — an externally
    /// tagged enum matching `serde_json`'s default representation:
    /// `{"Element":{"tag":…,"attrs":[{"key":…,"value":…}],"children":[…]}}` and
    /// `{"Text":…}`. Deterministic (fixed field order, deterministic string
    /// escaping) so the emit is stable across runs — a requirement for the
    /// golden suite and the classifier's byte-diff. Byte-identical to
    /// `serde_json::to_string(&Template)` (pinned by a backend test).
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    fn write_json(&self, out: &mut String) {
        match self {
            Self::Text(s) => {
                out.push_str("{\"Text\":");
                write_json_string(s, out);
                out.push('}');
            }
            Self::Element {
                tag,
                attrs,
                children,
            } => {
                out.push_str("{\"Element\":{\"tag\":");
                write_json_string(tag, out);
                out.push_str(",\"attrs\":[");
                for (i, a) in attrs.iter().enumerate() {
                    if i != 0 {
                        out.push(',');
                    }
                    out.push_str("{\"key\":");
                    write_json_string(&a.key, out);
                    out.push_str(",\"value\":");
                    write_json_string(&a.value, out);
                    out.push('}');
                }
                out.push_str("],\"children\":[");
                for (i, c) in children.iter().enumerate() {
                    if i != 0 {
                        out.push(',');
                    }
                    c.write_json(out);
                }
                out.push_str("]}}");
            }
        }
    }
}

/// Append `s` as a JSON string literal (surrounding quotes + escaping) matching
/// `serde_json`'s encoding: the two mandatory JSON escapes (`"` and `\`), the
/// short escapes for the C0 control characters `serde_json` spells short
/// (`\n \r \t \u{08} \u{0c}`), and `\u00XX` for every other control character.
/// All other characters (including non-ASCII) pass through verbatim —
/// `serde_json` does not escape non-ASCII by default. Total: never panics.
fn write_json_string(s: &str, out: &mut String) {
    use std::fmt::Write as _;
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                // Remaining C0 controls: `serde_json` emits `\u00XX` lowercase-hex.
                // `write!` to a `String` is infallible; the result is discarded.
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Reduce a static `Ipe.Html` `view` subtree to a [`CompileTemplate`], or `None`
/// when the subtree is not provably static (any `Model` read, control flow,
/// handler, raw markup, non-literal argument, or over-deep nesting) — the
/// caller then keeps it compiled (recompile path).
pub fn template_of_expr(expr: &Expr) -> Option<CompileTemplate> {
    template_of_expr_at(expr, 0)
}

fn template_of_expr_at(expr: &Expr, depth: usize) -> Option<CompileTemplate> {
    if depth >= MAX_TEMPLATE_DEPTH {
        return None;
    }
    let Expr::Call { callee, args, .. } = expr else {
        // A non-call node in element position (a `Var`, `Access`, `If`, `Match`,
        // literal, user-function call reference, …) is never a static
        // `Ipe.Html` node — refuse, keep compiled.
        return None;
    };
    let Callee::Kernel(k) = callee else {
        // A user-function or FFI call is not a static node kernel.
        return None;
    };
    // Matched by NODE kernel. Only the four templatable node builders below reduce
    // to a template; EVERY other kernel — including the raw / trusted-markup node
    // kernels (`HtmlRawNode` / `HtmlStyleNode` / `HtmlScriptNode` / `HtmlDoctype`,
    // which carry un-escaped markup and must NEVER become an inert template, exactly
    // as the runtime `template_of` refuses `HRaw`), every value builder, every
    // `Ui.*` layout node, and every handler-bearing or effectful call — refuses via
    // the final arm and stays compiled. Fail-closed: an unrecognised node kernel is
    // never templated.
    match k {
        // `Html.text s` — a static text node when `s` is a literal.
        KernelFn::HtmlTextNode => match args.as_slice() {
            [Expr::Str(s)] => Some(CompileTemplate::Text(s.clone())),
            _ => None,
        },
        // `Html.node tag attrs children` — a static element when the tag is a
        // literal, every attribute is a static string attribute, and every child
        // is itself templatable.
        KernelFn::HtmlNode => match args.as_slice() {
            [Expr::Str(tag), attrs, children] => {
                let attrs = static_attrs(attrs)?;
                let children = static_children(children, depth)?;
                Some(CompileTemplate::Element {
                    tag: tag.clone(),
                    attrs,
                    children,
                })
            }
            _ => None,
        },
        // `Html.voidNode tag attrs` — the runtime lowers it to
        // `html_node_(tag, attrs, [])`, so it is a static element with no
        // children when the tag is a literal and the attributes are static.
        KernelFn::HtmlVoidNode => match args.as_slice() {
            [Expr::Str(tag), attrs] => {
                let attrs = static_attrs(attrs)?;
                Some(CompileTemplate::Element {
                    tag: tag.clone(),
                    attrs,
                    children: Vec::new(),
                })
            }
            _ => None,
        },
        // `Html.titleNode s` — the runtime wraps a literal string as
        // `HElement "title" [] [HText s]`; mirror that exactly.
        KernelFn::HtmlTitleNode => match args.as_slice() {
            [Expr::Str(s)] => Some(CompileTemplate::Element {
                tag: "title".to_string(),
                attrs: Vec::new(),
                children: vec![CompileTemplate::Text(s.clone())],
            }),
            _ => None,
        },
        // Raw / trusted-markup, value builders, `Ui.*` nodes, handlers, effects —
        // not a static `Ipe.Html` element/text node. Refuse, keep compiled.
        _ => None,
    }
}

/// Reduce a literal list of `Ipe.Html` attributes to inert key/value pairs, or
/// `None` when any element is not a static string attribute (a bool/absent attr,
/// an event handler, or a non-literal). The list itself must be a literal
/// `Expr::List` — a `List.map` / variable / concatenation is not statically
/// known, so it refuses.
fn static_attrs(attrs: &Expr) -> Option<Vec<CompileTemplateAttr>> {
    let Expr::List { items, .. } = attrs else {
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(static_attr(item)?);
    }
    Some(out)
}

/// Reduce one attribute expression to an inert key/value pair, or `None`.
/// EXHAUSTIVE over the `Ipe.Html` attribute-producing kernels — a new one forces
/// a decision here rather than a silent refusal.
fn static_attr(attr: &Expr) -> Option<CompileTemplateAttr> {
    let Expr::Call { callee, args, .. } = attr else {
        return None;
    };
    let Callee::Kernel(k) = callee else {
        return None;
    };
    // A bool attribute (`HtmlBoolAttribute`), an absent attribute (`HtmlNoAttr`),
    // every event-handler attribute, and any other attribute kernel are outside the
    // static string-attr scope of a template (mirrors the runtime `template_of`
    // refusing `BoolAttr` / `NoAttr` / `EventAttr`) — they refuse via the final arm
    // and keep the subtree compiled. Only the plain string attribute reduces.
    match k {
        // `Html.attribute key value` — a static string attribute.
        KernelFn::HtmlAttribute | KernelFn::UiHtmlAttribute => match args.as_slice() {
            [Expr::Str(key), Expr::Str(value)] => Some(CompileTemplateAttr {
                key: key.clone(),
                value: value.clone(),
            }),
            _ => None,
        },
        // A bool / absent / event / any-other attribute kernel is not a static
        // string attribute — keep the subtree compiled.
        _ => None,
    }
}

/// Reduce a literal list of child `Ipe.Html` nodes to templates, or `None` when
/// the list is non-literal or any child is not templatable.
fn static_children(children: &Expr, depth: usize) -> Option<Vec<CompileTemplate>> {
    let Expr::List { items, .. } = children else {
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(template_of_expr_at(item, depth.saturating_add(1))?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{
        CompileTemplate, CompileTemplateAttr, MAX_TEMPLATE_DEPTH, template_of_expr,
        write_json_string,
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

    fn text(s: &str) -> Expr {
        kcall(KernelFn::HtmlTextNode, vec![Expr::Str(s.to_string())])
    }

    fn attr(k: &str, v: &str) -> Expr {
        kcall(
            KernelFn::HtmlAttribute,
            vec![Expr::Str(k.to_string()), Expr::Str(v.to_string())],
        )
    }

    // ── acceptance: a provably-static subtree templates ──────────────────────

    #[test]
    fn static_text_node_templates() {
        assert_eq!(
            template_of_expr(&text("hi")),
            Some(CompileTemplate::Text("hi".to_string()))
        );
    }

    #[test]
    fn static_element_with_attrs_and_children_templates() {
        let node = kcall(
            KernelFn::HtmlNode,
            vec![
                Expr::Str("div".to_string()),
                attr_list(vec![attr("class", "card"), attr("id", "main")]),
                child_list(vec![text("Hello")]),
            ],
        );
        let got = template_of_expr(&node).expect("templatable");
        assert_eq!(
            got,
            CompileTemplate::Element {
                tag: "div".to_string(),
                attrs: vec![
                    CompileTemplateAttr {
                        key: "class".to_string(),
                        value: "card".to_string()
                    },
                    CompileTemplateAttr {
                        key: "id".to_string(),
                        value: "main".to_string()
                    },
                ],
                children: vec![CompileTemplate::Text("Hello".to_string())],
            }
        );
    }

    #[test]
    fn void_node_templates_with_empty_children() {
        let node = kcall(
            KernelFn::HtmlVoidNode,
            vec![
                Expr::Str("hr".to_string()),
                attr_list(vec![attr("class", "sep")]),
            ],
        );
        let got = template_of_expr(&node).expect("templatable");
        assert_eq!(
            got,
            CompileTemplate::Element {
                tag: "hr".to_string(),
                attrs: vec![CompileTemplateAttr {
                    key: "class".to_string(),
                    value: "sep".to_string()
                }],
                children: vec![],
            }
        );
    }

    #[test]
    fn title_node_templates_as_title_element() {
        let got = template_of_expr(&kcall(
            KernelFn::HtmlTitleNode,
            vec![Expr::Str("Page".to_string())],
        ))
        .expect("templatable");
        assert_eq!(
            got,
            CompileTemplate::Element {
                tag: "title".to_string(),
                attrs: vec![],
                children: vec![CompileTemplate::Text("Page".to_string())],
            }
        );
    }

    #[test]
    fn deeply_nested_static_tree_templates() {
        let mut node = text("deep");
        for _ in 0..16 {
            node = kcall(
                KernelFn::HtmlNode,
                vec![
                    Expr::Str("div".to_string()),
                    attr_list(vec![]),
                    child_list(vec![node]),
                ],
            );
        }
        assert!(template_of_expr(&node).is_some());
    }

    // ── refusal: not provably static ⇒ keep compiled (None) ──────────────────

    #[test]
    fn model_read_child_refuses() {
        // A `Var` (a bound name — e.g. a model field flowed in) in child
        // position is not static.
        let node = kcall(
            KernelFn::HtmlNode,
            vec![
                Expr::Str("div".to_string()),
                attr_list(vec![]),
                child_list(vec![Expr::Var(ipe_intern::Symbol::from_raw(1))]),
            ],
        );
        assert_eq!(template_of_expr(&node), None);
    }

    #[test]
    fn raw_markup_node_refuses() {
        let node = kcall(
            KernelFn::HtmlRawNode,
            vec![Expr::Str("<b>x</b>".to_string())],
        );
        assert_eq!(template_of_expr(&node), None);
    }

    #[test]
    fn style_and_script_nodes_refuse() {
        assert_eq!(
            template_of_expr(&kcall(
                KernelFn::HtmlScriptNode,
                vec![Expr::Str("evil()".to_string())]
            )),
            None
        );
    }

    #[test]
    fn bool_attribute_refuses() {
        let node = kcall(
            KernelFn::HtmlNode,
            vec![
                Expr::Str("input".to_string()),
                attr_list(vec![kcall(
                    KernelFn::HtmlBoolAttribute,
                    vec![Expr::Str("disabled".to_string()), Expr::Bool(true)],
                )]),
                child_list(vec![]),
            ],
        );
        assert_eq!(template_of_expr(&node), None);
    }

    #[test]
    fn no_attr_refuses() {
        let node = kcall(
            KernelFn::HtmlNode,
            vec![
                Expr::Str("div".to_string()),
                attr_list(vec![kcall(KernelFn::HtmlNoAttr, vec![])]),
                child_list(vec![]),
            ],
        );
        assert_eq!(template_of_expr(&node), None);
    }

    #[test]
    fn non_literal_attr_value_refuses() {
        // An attribute whose value is a `Var` (Model-dependent) is not static.
        let node = kcall(
            KernelFn::HtmlNode,
            vec![
                Expr::Str("div".to_string()),
                attr_list(vec![kcall(
                    KernelFn::HtmlAttribute,
                    vec![
                        Expr::Str("class".to_string()),
                        Expr::Var(ipe_intern::Symbol::from_raw(2)),
                    ],
                )]),
                child_list(vec![]),
            ],
        );
        assert_eq!(template_of_expr(&node), None);
    }

    #[test]
    fn non_literal_attrs_list_refuses() {
        // `attrs` is a `Var` (e.g. computed list) — not a literal list.
        let node = kcall(
            KernelFn::HtmlNode,
            vec![
                Expr::Str("div".to_string()),
                Expr::Var(ipe_intern::Symbol::from_raw(3)),
                child_list(vec![]),
            ],
        );
        assert_eq!(template_of_expr(&node), None);
    }

    #[test]
    fn non_literal_tag_refuses() {
        let node = kcall(
            KernelFn::HtmlNode,
            vec![
                Expr::Var(ipe_intern::Symbol::from_raw(4)),
                attr_list(vec![]),
                child_list(vec![]),
            ],
        );
        assert_eq!(template_of_expr(&node), None);
    }

    #[test]
    fn non_html_call_refuses() {
        // A `Ui.*` node (not `Ipe.Html`) is out of scope for this slice.
        let node = kcall(KernelFn::UiNode, vec![]);
        assert_eq!(template_of_expr(&node), None);
    }

    #[test]
    fn over_deep_subtree_refuses() {
        // Build and walk a chain past the ceiling on a large-stack thread: the
        // refuse-path descends to the cap before returning `None`, so the walk
        // itself uses the native stack up to that bound.
        let refused = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                let mut node = text("x");
                for _ in 0..=MAX_TEMPLATE_DEPTH {
                    node = kcall(
                        KernelFn::HtmlNode,
                        vec![
                            Expr::Str("div".to_string()),
                            attr_list(vec![]),
                            child_list(vec![node]),
                        ],
                    );
                }
                template_of_expr(&node).is_none()
            })
            .expect("spawn")
            .join()
            .expect("thread must not panic");
        assert!(refused, "an over-deep subtree must not template");
    }

    // ── JSON escaping ────────────────────────────────────────────────────────

    #[test]
    fn json_string_escapes_specials() {
        let mut out = String::new();
        write_json_string("a\"b\\c\nd\te", &mut out);
        assert_eq!(out, "\"a\\\"b\\\\c\\nd\\te\"");
    }

    #[test]
    fn json_string_escapes_control_chars() {
        let mut out = String::new();
        write_json_string("\u{01}", &mut out);
        assert_eq!(out, "\"\\u0001\"");
    }

    #[test]
    fn json_string_passes_non_ascii_verbatim() {
        let mut out = String::new();
        write_json_string("café ☕", &mut out);
        assert_eq!(out, "\"café ☕\"");
    }

    // ── dev == prod at the compiler/runtime seam: the emitted JSON is exactly
    //    the runtime `Template`'s serde shape ───────────────────────────────

    // The compile-time serializer must emit the externally-tagged JSON the
    // runtime `Template` (`serde_json::from_str::<Template>`) decodes, or the
    // baked default would fail to decode and the view would render an empty
    // node. Pinned as a literal so a drift in the serializer is caught here
    // without pulling the heavy `web` runtime into the backend's dev build; the
    // render-equivalence half is proven in the runtime crate's own
    // `str_materialize_matches_direct_render`. The expected bytes match
    // serde_json's default externally-tagged encoding of the equivalent
    // `Template::Element{…}` / `Template::Text(…)` / `TemplateAttr{key,value}`.
    #[test]
    fn compile_json_is_runtime_template_serde_shape() {
        let compile = CompileTemplate::Element {
            tag: "section".to_string(),
            attrs: vec![CompileTemplateAttr {
                key: "id".to_string(),
                value: r#"a "quote" & <tag>"#.to_string(),
            }],
            children: vec![
                CompileTemplate::Element {
                    tag: "h1".to_string(),
                    attrs: vec![],
                    children: vec![CompileTemplate::Text("Título ☕".to_string())],
                },
                CompileTemplate::Text("<script>".to_string()),
            ],
        };
        // serde_json escapes `"` and `\` only among the printable ASCII; `<`,
        // `>`, `&`, and non-ASCII pass through verbatim.
        let expected = concat!(
            r#"{"Element":{"tag":"section","attrs":[{"key":"id","value":"a \"quote\" & <tag>"}],"#,
            r#""children":[{"Element":{"tag":"h1","attrs":[],"children":[{"Text":"Título ☕"}]}},"#,
            r#"{"Text":"<script>"}]}}"#,
        );
        assert_eq!(compile.to_json(), expected);
    }

    // A single text node's JSON is the minimal externally-tagged form.
    #[test]
    fn compile_json_text_node_shape() {
        assert_eq!(
            CompileTemplate::Text("hi".to_string()).to_json(),
            r#"{"Text":"hi"}"#
        );
    }
}

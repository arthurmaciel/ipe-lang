//! Std.Ui style-marker injection — Rust port of Go's `applyStyleInjections`
//! (live.go:872-1110).
//!
//! The shared `Std.Ui` stdlib emits `data-sky-{mq,pc,tr,anim}-*` marker
//! *attributes* on elements for `Ui.breakpoint` / `Ui.mediaQuery`,
//! `Background.hoverColor` / `Ui.onPseudo`, `Transition.attribute`, and
//! `Animation.attribute`. The Go backend consumes them into sky-id-scoped
//! `<style>` blocks; without this pass the Rust backend rendered the markers
//! inert and produced zero CSS, so hover / breakpoint / media-query /
//! transition / animation were entirely dead.
//!
//! Pre-condition: `assign_sky_ids` has stamped a `sky-id` attr on every
//! element. Post-condition: every marker attr is stripped (even on no-match, so
//! an empty marker never leaks as an inert `data-*`) and a `<style>` child is
//! prepended (or sibling-hoisted after a void element, #409).
//!
//! SECURITY: the `<style>` body is emitted as an `HRaw` node (CSS cannot be
//! entity-escaped without breaking it), so the only thing standing between a
//! marker value and a tag breakout is the close-tag strip applied to EVERY CSS
//! fragment below — it is load-bearing, mirrors Go byte-for-byte (the same
//! 7-char both-case strip), and must never be dropped. The selector uses the
//! element's own already-sanitised sky-id (assign_sky_ids), never a user attr,
//! so the selector cannot be broken out of either.
//!
//! Idempotent: a second run finds the markers already stripped and is a no-op
//! (matches Go's idempotency contract), a belt-and-braces against a missed
//! call site.

use crate::sky_runtime::html::{Attribute, Html, is_void};

/// Every style marker attr this module consumes, across all four passes. Used to
/// strip a void TREE-ROOT's markers (see `apply_style_injections`). MUST stay in
/// sync with the per-pass marker lists below.
const ALL_MARKERS: &[&str] = &[
    "data-sky-mq-q",
    "data-sky-mq-rules",
    "data-sky-pc-rules",
    "data-sky-tr-rules",
    "data-sky-tr-respect",
    "data-sky-anim-rules",
];

/// Run every style-marker pass over the tree in Go's fixed order. Call
/// immediately after `assign_sky_ids`, on the SAME tree that becomes both the
/// render output AND the diff baseline (Go applies it before render + before
/// storing the tree, so the diff compares two already-injected trees and never
/// sees a marker-attr-vs-style-child asymmetry → no spurious replace).
/// Hard recursion-depth bound for the style-injection tree walk — same rationale
/// and value as html's MAX_HTML_DEPTH: a deeply-nested (attacker-influenced) tree
/// would overflow the thread stack. Past the cap we stop descending (deeper nodes
/// keep their markers) rather than abort the process.
const MAX_STYLE_DEPTH: usize = 1024;

pub fn apply_style_injections<M>(node: &mut Html<M>) {
    inject_pass(
        node,
        &["data-sky-mq-q", "data-sky-mq-rules"],
        "data-sky-mq",
        &|id, a| build_mq(id, a),
        0,
    );
    inject_pass(
        node,
        &["data-sky-pc-rules"],
        "data-sky-pc",
        &|id, a| build_pc(id, a),
        0,
    );
    inject_pass(
        node,
        &["data-sky-tr-rules", "data-sky-tr-respect"],
        "data-sky-tr",
        &|id, a| build_tr(id, a),
        0,
    );
    inject_pass(
        node,
        &["data-sky-anim-rules"],
        "data-sky-anim",
        &|id, a| build_anim(id, a),
        0,
    );
    // A void element at the TREE ROOT is never self-handled (inject_pass skips
    // void self-build, since a void tag can take no child <style>) and has no
    // parent to hoist a sibling <style> after it (#409). Its markers would
    // therefore survive every pass and leak as inert data-* attrs, breaking the
    // post-condition. Strip them here. The CSS is necessarily dropped — a void
    // root has nowhere to carry a <style> node. (A void node WITH a parent is
    // unaffected: the parent's loop still finds its markers intact and hoists.)
    if let Html::HElement(t, attrs, _) = node {
        if is_void(t) {
            strip_markers(attrs, ALL_MARKERS);
        }
    }
}

/// One style-injection pass over a subtree: self-handle (non-void prepends a
/// style child), then walk children splicing a sibling `<style>` after any void
/// child whose marker survived (the void child's own self-handler bails).
fn inject_pass<M>(
    node: &mut Html<M>,
    markers: &[&str],
    style_attr: &str,
    build: &impl Fn(&str, &[Attribute<M>]) -> String,
    depth: usize,
) {
    // Stack-overflow guard: stop descending a pathologically deep tree (deeper
    // nodes keep their markers — a truncated injection beats a process abort).
    if depth >= MAX_STYLE_DEPTH {
        return;
    }
    let (tag, attrs, kids) = match node {
        Html::HElement(t, a, k) => (t, a, k),
        _ => return,
    };
    // Non-void self: build + prepend the style child (build_style_node strips
    // the markers regardless of outcome).
    if !is_void(tag) {
        if let Some(style) = build_style_node(attrs, markers, style_attr, build) {
            kids.insert(0, style);
        }
    }
    // Walk children, recursing into each and hoisting a sibling style block
    // after any void child that still carries a marker.
    let mut out: Vec<Html<M>> = Vec::with_capacity(kids.len());
    for mut child in std::mem::take(kids) {
        inject_pass(&mut child, markers, style_attr, build, depth + 1);
        let hoist = match &mut child {
            Html::HElement(ct, ca, _) if is_void(ct) => {
                build_style_node(ca, markers, style_attr, build)
            }
            _ => None,
        };
        out.push(child);
        if let Some(h) = hoist {
            out.push(h);
        }
    }
    *kids = out;
}

/// Build the `<style>` node for an element's markers and strip those markers
/// from its attrs. Returns `None` (markers still stripped) when there's no
/// sky-id, no non-empty marker, or the built CSS is empty.
fn build_style_node<M>(
    attrs: &mut Vec<Attribute<M>>,
    markers: &[&str],
    style_attr: &str,
    build: &impl Fn(&str, &[Attribute<M>]) -> String,
) -> Option<Html<M>> {
    let sky_id = match attr_get(attrs, "sky-id") {
        Some(s) => s,
        None => {
            strip_markers(attrs, markers);
            return None;
        }
    };
    let has_any = markers
        .iter()
        .any(|m| attr_get(attrs, m).is_some_and(|v| !v.is_empty()));
    if !has_any {
        strip_markers(attrs, markers);
        return None;
    }
    let css = build(&sky_id, attrs); // reads markers BEFORE they're stripped
    strip_markers(attrs, markers);
    if css.is_empty() {
        return None;
    }
    Some(Html::HElement(
        "style".to_string(),
        vec![Attribute::Attr(style_attr.to_string(), sky_id)],
        vec![Html::HRaw(css)],
    ))
}

/// Read an attribute's value by key (owned clone — values are short and this
/// keeps the helper lifetime-free, which also dodges a false-positive in the
/// runtime indexing-precheck on `&'a [T]`).
fn attr_get<M>(attrs: &[Attribute<M>], key: &str) -> Option<String> {
    attrs.iter().find_map(|a| match a {
        Attribute::Attr(k, v) if k == key => Some(v.clone()),
        _ => None,
    })
}

fn strip_markers<M>(attrs: &mut Vec<Attribute<M>>, markers: &[&str]) {
    attrs.retain(|a| !matches!(a, Attribute::Attr(k, _) if markers.contains(&k.as_str())));
}

// `strip_style_close` moved to the shared `css_safety` module (design §Q5: one
// policy, one place). Imported below so the Std.Ui pseudo-class / media-query
// `<style>` path and the Std.Css / styleNode `<style>` sink share the identical
// close-tag stripper.
use super::super::css_safety::strip_style_close;

fn build_mq<M>(sky_id: &str, attrs: &[Attribute<M>]) -> String {
    let query = attr_get(attrs, "data-sky-mq-q").unwrap_or_default();
    let rules = attr_get(attrs, "data-sky-mq-rules").unwrap_or_default();
    if query.is_empty() || rules.is_empty() {
        return String::new();
    }
    let selector = format!("[sky-id=\"{sky_id}\"]");
    let safe_rules = strip_style_close(&rules);
    let safe_query = strip_style_close(&query);
    format!("@media {safe_query} {{ {selector} {{ {safe_rules} }} }}")
}

fn build_pc<M>(sky_id: &str, attrs: &[Attribute<M>]) -> String {
    let encoded = attr_get(attrs, "data-sky-pc-rules").unwrap_or_default();
    if encoded.is_empty() {
        return String::new();
    }
    let selector = format!("[sky-id=\"{sky_id}\"]");
    let mut out = String::new();
    for entry in encoded.split("||") {
        let (tag, css) = match entry.split_once('|') {
            Some(x) => x,
            None => continue,
        };
        if css.is_empty() {
            continue;
        }
        let (pseudo, hover_gated) = match pseudo_selector_for_tag(tag) {
            Some(x) => x,
            None => continue,
        };
        let safe_css = strip_style_close(css);
        if hover_gated {
            out.push_str(&format!(
                "@media (hover: hover) {{ {selector}{pseudo} {{ {safe_css} }} }} "
            ));
        } else {
            out.push_str(&format!("{selector}{pseudo} {{ {safe_css} }} "));
        }
    }
    out.trim().to_string()
}

/// Wire-format pseudo-class tag → (selector, hover-gated). Keep in lock-step
/// with `pseudoClassTag` in Std.Ui.sky / Go's `pseudoSelectorForTag`.
fn pseudo_selector_for_tag(tag: &str) -> Option<(&'static str, bool)> {
    match tag {
        "h" => Some((":hover", true)),
        "f" => Some((":focus", false)),
        "v" => Some((":focus-visible", false)),
        "a" => Some((":active", false)),
        "d" => Some((":disabled", false)),
        _ => None,
    }
}

fn build_tr<M>(sky_id: &str, attrs: &[Attribute<M>]) -> String {
    let rules = attr_get(attrs, "data-sky-tr-rules").unwrap_or_default();
    if rules.is_empty() {
        return String::new();
    }
    let respect = attr_get(attrs, "data-sky-tr-respect")
        .unwrap_or_default()
        .as_str()
        != "0";
    let safe_rules = strip_style_close(&rules);
    let selector = format!("[sky-id=\"{sky_id}\"]");
    if respect {
        format!(
            "@media (prefers-reduced-motion: no-preference) {{ {selector} {{ transition: {safe_rules}; }} }}"
        )
    } else {
        format!("{selector} {{ transition: {safe_rules}; }}")
    }
}

fn build_anim<M>(sky_id: &str, attrs: &[Attribute<M>]) -> String {
    let encoded = attr_get(attrs, "data-sky-anim-rules").unwrap_or_default();
    if encoded.is_empty() {
        return String::new();
    }
    let ident = sky_id_to_css_ident(sky_id);
    let selector = format!("[sky-id=\"{sky_id}\"]");
    let mut keyframes = String::new();
    let mut gated: Vec<String> = vec![];
    let mut ungated: Vec<String> = vec![];

    for entry in encoded.split("@@") {
        let mut it = entry.splitn(4, "||");
        let (name, tail, body, respect) = match (it.next(), it.next(), it.next(), it.next()) {
            (Some(n), Some(t), Some(b), Some(r)) => (n, t, b, r),
            _ => continue,
        };
        if name.is_empty() || body.is_empty() {
            continue;
        }
        let safe_body = strip_style_close(body);
        let safe_tail = strip_style_close(tail);
        let safe_name = sanitise_animation_name(name);
        if safe_name.is_empty() {
            continue;
        }
        let effective = format!("{safe_name}__{ident}");
        keyframes.push_str(&format!("@keyframes {effective} {{ {safe_body} }} "));
        let r = format!("{effective} {safe_tail}");
        if respect == "0" {
            ungated.push(r);
        } else {
            gated.push(r);
        }
    }

    if keyframes.is_empty() {
        return String::new();
    }
    let mut sb = keyframes;
    if !gated.is_empty() {
        sb.push_str(&format!(
            "@media (prefers-reduced-motion: no-preference) {{ {selector} {{ animation: {}; }} }} ",
            gated.join(", ")
        ));
    }
    if !ungated.is_empty() {
        sb.push_str(&format!(
            "{selector} {{ animation: {}; }} ",
            ungated.join(", ")
        ));
    }
    sb.trim().to_string()
}

/// sky-id (`r.0.2#div`) → CSS-safe ident suffix (`r_0_2_div`) for @keyframes
/// names. Structural separators map to `_`; anything else outside the CSS-ident
/// charset is dropped (Go's `skyIDToCSSIdent`).
fn sky_id_to_css_ident(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => Some(c),
            '.' | '#' => Some('_'),
            _ => None,
        })
        .collect()
}

/// Strip chars that would break a CSS `@keyframes` ident (non-ident → `_`); a
/// leading digit is illegal so prefix `_` (Go's `sanitiseAnimationName`).
fn sanitise_animation_name(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let mut out: String = s
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => c,
            _ => '_',
        })
        .collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr(k: &str, v: &str) -> Attribute<()> {
        Attribute::Attr(k.to_string(), v.to_string())
    }

    fn count_styles(n: &Html<()>) -> usize {
        match n {
            Html::HElement(t, _, kids) => {
                (if t == "style" { 1 } else { 0 }) + kids.iter().map(count_styles).sum::<usize>()
            }
            _ => 0,
        }
    }

    // SECURITY regression: the close-tag strip must be TOTAL — no spelling of
    // `</style` survives, including mixed case and post-removal reconstruction.
    #[test]
    fn strip_style_close_is_total() {
        // plain
        assert!(
            !strip_style_close("a</style>b")
                .to_ascii_lowercase()
                .contains("</style")
        );
        // mixed case (HTML end-tags are ASCII-case-insensitive)
        assert!(
            !strip_style_close("a</StYle>b")
                .to_ascii_lowercase()
                .contains("</style")
        );
        assert!(
            !strip_style_close("a</STYLE>b")
                .to_ascii_lowercase()
                .contains("</style")
        );
        // reconstruction across the join seam after a single removal
        assert!(
            !strip_style_close("</sty</stylele>")
                .to_ascii_lowercase()
                .contains("</style")
        );
        assert!(
            !strip_style_close("</st</STYLEyle>")
                .to_ascii_lowercase()
                .contains("</style")
        );
        // benign content is untouched
        assert_eq!(strip_style_close("color: red;"), "color: red;");
    }

    // SECURITY regression: a `</style><script>` payload in any CSS fragment must
    // be neutralised by the close-tag strip so it cannot break out of the raw
    // <style> block (stored-XSS). One test per build fn that takes raw-ish CSS.
    #[test]
    fn pc_strips_style_close_breakout() {
        let attrs = vec![
            attr("sky-id", "r_0_button"),
            attr(
                "data-sky-pc-rules",
                "h|color: red } </style><script>alert(1)</script>",
            ),
        ];
        let css = build_pc("r_0_button", &attrs);
        assert!(!css.contains("</style"), "breakout not stripped: {css}");
        assert!(
            css.contains("@media (hover: hover)") && css.contains(":hover"),
            "{css}"
        );
    }

    #[test]
    fn mq_strips_style_close_in_query_and_rules() {
        let attrs = vec![
            attr("data-sky-mq-q", "(max-width: 600px) </style>"),
            attr(
                "data-sky-mq-rules",
                "color: blue </style><script>x</script>",
            ),
        ];
        let css = build_mq("r0", &attrs);
        assert!(!css.contains("</style"), "{css}");
        assert!(css.contains("@media"), "{css}");
    }

    // Snapshot port of ../sky fixture `70-style-injection`: the exact raw
    // media-query breakout probe must be neutralised — no `</style>` survives and
    // no `</style><script` breakout sequence forms (the injected `<script>` is
    // trapped inert inside the `<style>` block).
    #[test]
    fn fixture70_mediaquery_breakout_probe_neutralised() {
        let attrs = vec![
            attr(
                "data-sky-mq-q",
                "(min-width: 1px) </style><script>alert(1)</script>",
            ),
            attr("data-sky-mq-rules", "background-color:rgb(1,2,3)"),
        ];
        let css = build_mq("r_2_el", &attrs);
        assert!(!css.to_ascii_lowercase().contains("</style"), "{css}");
        assert!(
            !css.to_ascii_lowercase().contains("</style><script"),
            "{css}"
        );
        assert!(css.contains("@media"), "{css}");
    }

    #[test]
    fn anim_strips_breakout_and_sanitises_name() {
        let attrs = vec![attr(
            "data-sky-anim-rules",
            "9bad name!||300ms ease||0% { opacity: 0 } </style>||1",
        )];
        let css = build_anim("r.0#div", &attrs);
        assert!(!css.contains("</style"), "{css}");
        // leading-digit prefixed + non-ident chars → `_`, sky-id ident suffix.
        assert!(css.contains("@keyframes _9bad_name___r_0_div"), "{css}");
    }

    #[test]
    fn apply_prepends_style_child_and_strips_marker() {
        let mut tree: Html<()> = Html::HElement(
            "button".to_string(),
            vec![
                attr("sky-id", "r"),
                attr("data-sky-pc-rules", "h|color: red"),
            ],
            vec![Html::HText("x".to_string())],
        );
        apply_style_injections(&mut tree);
        match &tree {
            Html::HElement(_, attrs, kids) => {
                assert!(
                    !attrs
                        .iter()
                        .any(|a| matches!(a, Attribute::Attr(k, _) if k == "data-sky-pc-rules")),
                    "marker must be stripped"
                );
                assert!(
                    matches!(kids.first(), Some(Html::HElement(t, _, _)) if t == "style"),
                    "style child must be prepended"
                );
            }
            _ => panic!("expected element"),
        }
    }

    #[test]
    fn void_element_hoists_style_to_sibling() {
        // An <input> (void) carrying a marker can't take a child <style>; it must
        // be hoisted to a sibling slot right after the input (#409).
        let mut tree: Html<()> = Html::HElement(
            "div".to_string(),
            vec![attr("sky-id", "r")],
            vec![Html::HElement(
                "input".to_string(),
                vec![
                    attr("sky-id", "r_0_input"),
                    attr("data-sky-pc-rules", "f|outline: none"),
                ],
                vec![],
            )],
        );
        apply_style_injections(&mut tree);
        if let Html::HElement(_, _, kids) = &tree {
            assert_eq!(kids.len(), 2, "input + hoisted style sibling");
            assert!(matches!(&kids[0], Html::HElement(t, _, _) if t == "input"));
            assert!(matches!(&kids[1], Html::HElement(t, _, _) if t == "style"));
        } else {
            panic!("expected element");
        }
    }

    #[test]
    fn void_root_strips_its_markers() {
        // A void element at the tree root has no parent to hoist a sibling style
        // and is never self-handled; its markers must still be stripped so they
        // don't leak as inert data-* attrs (post-condition).
        let mut tree: Html<()> = Html::HElement(
            "input".to_string(),
            vec![
                attr("sky-id", "r"),
                attr("data-sky-pc-rules", "f|outline: none"),
            ],
            vec![],
        );
        apply_style_injections(&mut tree);
        if let Html::HElement(_, attrs, _) = &tree {
            assert!(
                !attrs
                    .iter()
                    .any(|a| matches!(a, Attribute::Attr(k, _) if k == "data-sky-pc-rules")),
                "void-root marker must be stripped"
            );
        } else {
            panic!("expected element");
        }
    }

    /// #113 spec §1.4 end-to-end: the FULL pipeline from a Rust
    /// `Attribute::AttrPseudoRule` (as `Background.hoverColor` constructs it)
    /// through `ui_layout` → `assign_sky_ids` → `apply_style_injections` →
    /// `render_html` must produce a sky-id-scoped `<style>` rule and leave NO
    /// `data-sky-pc-rules` marker in the final HTML. Closes the "two halves
    /// each tested, whole pipeline never composed" gap flagged by the
    /// independent review of the 2026-07-10 Std.Ui/Html kernel batch.
    #[test]
    fn end_to_end_ui_hover_color_renders_scoped_style_and_leaves_no_marker() {
        use crate::sky_runtime::html::{assign_sky_ids, render_html};
        use crate::sky_runtime::ui::element::{Color, Element};
        use crate::sky_runtime::ui::helpers::ui_bg_hover_color_;
        use crate::sky_runtime::ui::render::ui_layout;

        let attrs = vec![ui_bg_hover_color_::<()>(Color::Rgba(0, 92, 215, 1.0))];
        let elem: Element<()> = Element::Text("hover me".to_owned());
        let mut html = ui_layout(attrs, elem);
        assign_sky_ids(&mut html, "r");
        apply_style_injections(&mut html);
        let s = render_html(&html);
        assert!(
            !s.contains("data-sky-pc-rules"),
            "marker must be consumed, never leak into final HTML: {s}"
        );
        assert!(s.contains("<style"), "scoped <style> block must render: {s}");
        assert!(
            s.contains(":hover") && s.contains("background-color:rgba(0,92,215,1)"),
            "hover rule with the exact CSS must render: {s}"
        );
        assert!(
            s.contains("@media (hover: hover)"),
            "hover rules must be wrapped in the touch-device guard: {s}"
        );
    }

    #[test]
    fn idempotent_second_run_adds_no_duplicate_style() {
        let mut tree: Html<()> = Html::HElement(
            "div".to_string(),
            vec![
                attr("sky-id", "r"),
                attr("data-sky-tr-rules", "color 200ms"),
            ],
            vec![],
        );
        apply_style_injections(&mut tree);
        let first = count_styles(&tree);
        apply_style_injections(&mut tree);
        assert_eq!(first, count_styles(&tree), "second run must be a no-op");
    }
}

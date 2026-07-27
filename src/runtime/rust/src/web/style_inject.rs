//! Ipe.Ui style-marker injection — Rust port of Go's `applyStyleInjections`
//! (live.go:872-1110).
//!
//! The shared `Ipe.Ui` stdlib emits `data-ipe-{mq,pc,tr,anim}-*` marker
//! *attributes* on elements for `Ui.breakpoint` / `Ui.mediaQuery`,
//! `Background.hoverColor` / `Ui.onPseudo`, `Transition.attribute`, and
//! `Animation.attribute`. The Go backend consumes them into ipe-id-scoped
//! `<style>` blocks; without this pass the Rust backend rendered the markers
//! inert and produced zero CSS, so hover / breakpoint / media-query /
//! transition / animation were entirely dead.
//!
//! Pre-condition: `assign_ipe_ids` has stamped a `ipe-id` attr on every
//! element. Post-condition: every marker attr is stripped (even on no-match, so
//! an empty marker never leaks as an inert `data-*`) and a `<style>` child is
//! prepended (or sibling-hoisted after a void element).
//!
//! SECURITY: the `<style>` body is emitted as an `HRaw` node (CSS cannot be
//! entity-escaped without breaking it). The marker attrs are raw Strings a
//! caller can FORGE via `Ui.htmlAttribute` (bypassing every producer-side
//! `SafeCss*` gate), so the PRIMARY gate lives at THIS sink: every builder
//! below re-validates its marker payload through the shared `css_safety`
//! policy (`SafeCssMediaQuery` / `sink_safe_declaration_list` / `SafeCssValue`
//! / `sink_safe_keyframes_body`) and drops the block fail-closed on any
//! breakout. The close-tag strip applied to EVERY CSS fragment stays as
//! belt-and-braces and must never be dropped. The selector uses the element's
//! own already-sanitised ipe-id (assign_ipe_ids), never a user attr, so the
//! selector cannot be broken out of either.
//!
//! Idempotent: a second run finds the markers already stripped and is a no-op
//! (matches Go's idempotency contract), a belt-and-braces against a missed
//! call site.

use crate::html::{Attribute, Html, is_void};

/// Every style marker attr this module consumes, across all four passes. Used to
/// strip a void TREE-ROOT's markers (see `apply_style_injections`). MUST stay in
/// sync with the per-pass marker lists below.
const ALL_MARKERS: &[&str] = &[
    "data-ipe-mq-q",
    "data-ipe-mq-rules",
    "data-ipe-pc-rules",
    "data-ipe-tr-rules",
    "data-ipe-tr-respect",
    "data-ipe-anim-rules",
];

/// Run every style-marker pass over the tree in Go's fixed order. Call
/// immediately after `assign_ipe_ids`, on the SAME tree that becomes both the
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
        &["data-ipe-mq-q", "data-ipe-mq-rules"],
        "data-ipe-mq",
        &|id, a| build_mq(id, a),
        0,
    );
    inject_pass(
        node,
        &["data-ipe-pc-rules"],
        "data-ipe-pc",
        &|id, a| build_pc(id, a),
        0,
    );
    inject_pass(
        node,
        &["data-ipe-tr-rules", "data-ipe-tr-respect"],
        "data-ipe-tr",
        &|id, a| build_tr(id, a),
        0,
    );
    inject_pass(
        node,
        &["data-ipe-anim-rules"],
        "data-ipe-anim",
        &|id, a| build_anim(id, a),
        0,
    );
    // A void element at the TREE ROOT is never self-handled (inject_pass skips
    // void self-build, since a void tag can take no child <style>) and has no
    // parent to hoist a sibling <style> after it. Its markers would
    // therefore survive every pass and leak as inert data-* attrs, breaking the
    // post-condition. Strip them here. The CSS is necessarily dropped — a void
    // root has nowhere to carry a <style> node. (A void node WITH a parent is
    // unaffected: the parent's loop still finds its markers intact and hoists.)
    if let Html::HElement(t, attrs, _) = node
        && is_void(t)
    {
        strip_markers(attrs, ALL_MARKERS);
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
    if !is_void(tag)
        && let Some(style) = build_style_node(attrs, markers, style_attr, build)
    {
        kids.insert(0, style);
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
/// ipe-id, no non-empty marker, or the built CSS is empty.
fn build_style_node<M>(
    attrs: &mut Vec<Attribute<M>>,
    markers: &[&str],
    style_attr: &str,
    build: &impl Fn(&str, &[Attribute<M>]) -> String,
) -> Option<Html<M>> {
    let ipe_id = match attr_get(attrs, "ipe-id") {
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
    let css = build(&ipe_id, attrs); // reads markers BEFORE they're stripped
    strip_markers(attrs, markers);
    if css.is_empty() {
        return None;
    }
    Some(Html::HElement(
        "style".to_string(),
        vec![Attribute::Attr(style_attr.to_string(), ipe_id)],
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
// policy, one place). Imported below so the Ipe.Ui pseudo-class / media-query
// `<style>` path and the Ipe.Css / styleNode `<style>` sink share the identical
// close-tag stripper.
use super::super::css_safety::{
    SafeCssMediaQuery, SafeCssValue, sink_safe_declaration_list, sink_safe_keyframes_body,
    strip_style_close,
};

fn build_mq<M>(ipe_id: &str, attrs: &[Attribute<M>]) -> String {
    let query = attr_get(attrs, "data-ipe-mq-q").unwrap_or_default();
    let rules = attr_get(attrs, "data-ipe-mq-rules").unwrap_or_default();
    if query.is_empty() || rules.is_empty() {
        return String::new();
    }
    // Sink-side re-validation. The markers are raw String attributes a caller
    // can FORGE via `Ui.htmlAttribute`, bypassing the producer's
    // `SafeCssMediaQuery` / `SafeCssValue` gates — so validate at THIS boundary
    // and drop the whole block fail-closed on any breakout. `strip_style_close`
    // stays as sink-final belt-and-braces below.
    let (safe_query, safe_rules) = match (
        SafeCssMediaQuery::parse(&query),
        sink_safe_declaration_list(&rules),
    ) {
        (Some(q), Some(r)) => (q.as_str().to_owned(), strip_style_close(r)),
        _ => return String::new(),
    };
    let selector = format!("[ipe-id=\"{ipe_id}\"]");
    format!("@media {safe_query} {{ {selector} {{ {safe_rules} }} }}")
}

fn build_pc<M>(ipe_id: &str, attrs: &[Attribute<M>]) -> String {
    let encoded = attr_get(attrs, "data-ipe-pc-rules").unwrap_or_default();
    if encoded.is_empty() {
        return String::new();
    }
    let selector = format!("[ipe-id=\"{ipe_id}\"]");
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
        // Sink-side re-validation (forgeable marker — see `build_mq`). A
        // declaration that fails drops THIS pseudo-rule only; sibling rules and
        // the element still render.
        let safe_css = match sink_safe_declaration_list(css) {
            Some(c) => strip_style_close(c),
            None => continue,
        };
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
/// with `pseudoClassTag` in Ipe.Ui.ipe / Go's `pseudoSelectorForTag`.
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

fn build_tr<M>(ipe_id: &str, attrs: &[Attribute<M>]) -> String {
    let rules = attr_get(attrs, "data-ipe-tr-rules").unwrap_or_default();
    if rules.is_empty() {
        return String::new();
    }
    let respect = attr_get(attrs, "data-ipe-tr-respect")
        .unwrap_or_default()
        .as_str()
        != "0";
    // Sink-side re-validation (forgeable marker — see `build_mq`). The
    // transition value is a single declaration value; a breakout drops the
    // whole block fail-closed.
    let safe_rules = match SafeCssValue::parse(&rules) {
        Some(v) => strip_style_close(v.as_str()),
        None => return String::new(),
    };
    let selector = format!("[ipe-id=\"{ipe_id}\"]");
    if respect {
        format!(
            "@media (prefers-reduced-motion: no-preference) {{ {selector} {{ transition: {safe_rules}; }} }}"
        )
    } else {
        format!("{selector} {{ transition: {safe_rules}; }}")
    }
}

fn build_anim<M>(ipe_id: &str, attrs: &[Attribute<M>]) -> String {
    let encoded = attr_get(attrs, "data-ipe-anim-rules").unwrap_or_default();
    if encoded.is_empty() {
        return String::new();
    }
    let ident = ipe_id_to_css_ident(ipe_id);
    let selector = format!("[ipe-id=\"{ipe_id}\"]");
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
        // Sink-side re-validation (forgeable marker — see `build_mq`). The
        // keyframes BODY legitimately contains `{ } ;`, so it goes through the
        // keyframe-grammar validator instead of the flat declaration policy;
        // the shorthand TAIL is a single declaration value. An entry that
        // fails either gate drops fail-closed (`continue` — sibling animations
        // and the element still render, same per-entry posture as `build_pc`).
        // `strip_style_close` stays as sink-final belt-and-braces.
        let safe_body = match sink_safe_keyframes_body(body) {
            Some(b) => strip_style_close(b),
            None => continue,
        };
        let safe_tail = match SafeCssValue::parse(tail) {
            Some(t) => strip_style_close(t.as_str()),
            None => continue,
        };
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

/// ipe-id (`r.0.2#div`) → CSS-safe ident suffix (`r_0_2_div`) for @keyframes
/// names. Structural separators map to `_`; anything else outside the CSS-ident
/// charset is dropped (Go's `ipeIDToCSSIdent`).
fn ipe_id_to_css_ident(s: &str) -> String {
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
            attr("ipe-id", "r_0_button"),
            attr(
                "data-ipe-pc-rules",
                "h|color: red } </style><script>alert(1)</script>",
            ),
        ];
        let css = build_pc("r_0_button", &attrs);
        // Sink hardening: the forged rule carries a ruleset
        // breakout (`}`) + `</style>`; `sink_safe_declaration_list` drops the
        // whole pseudo-rule FAIL-CLOSED (stronger than the old strip-only
        // contract that let the sanitised `:hover` block through).
        assert!(!css.contains("</style"), "breakout not stripped: {css}");
        assert!(
            !css.to_ascii_lowercase().contains("<script"),
            "script leaked: {css}"
        );
        assert_eq!(
            css, "",
            "forged breakout pseudo-rule must drop fail-closed: {css}"
        );
    }

    #[test]
    fn mq_strips_style_close_in_query_and_rules() {
        let attrs = vec![
            attr("data-ipe-mq-q", "(max-width: 600px) </style>"),
            attr(
                "data-ipe-mq-rules",
                "color: blue </style><script>x</script>",
            ),
        ];
        let css = build_mq("r0", &attrs);
        // Sink hardening: both markers carry a `</style>`
        // breakout, so `SafeCssMediaQuery` (query) and `sink_safe_declaration_list`
        // (rules) each reject and `build_mq` drops the whole @media block
        // fail-closed — stronger than the old strip-then-still-emit contract.
        assert!(!css.contains("</style"), "{css}");
        assert_eq!(
            css, "",
            "forged breakout @media block must drop fail-closed: {css}"
        );
    }

    // Snapshot port of ../ipe fixture `70-style-injection`: the exact raw
    // media-query breakout probe must be neutralised — no `</style>` survives and
    // no `</style><script` breakout sequence forms (the injected `<script>` is
    // trapped inert inside the `<style>` block).
    #[test]
    fn fixture70_mediaquery_breakout_probe_neutralised() {
        let attrs = vec![
            attr(
                "data-ipe-mq-q",
                "(min-width: 1px) </style><script>alert(1)</script>",
            ),
            attr("data-ipe-mq-rules", "background-color:rgb(1,2,3)"),
        ];
        let css = build_mq("r_2_el", &attrs);
        assert!(!css.to_ascii_lowercase().contains("</style"), "{css}");
        assert!(
            !css.to_ascii_lowercase().contains("</style><script"),
            "{css}"
        );
        // Sink hardening: the query carries a `</style><script>`
        // breakout → `SafeCssMediaQuery` rejects it → `build_mq` drops the whole
        // @media block fail-closed (even though the rules half is legit).
        assert_eq!(
            css, "",
            "forged breakout query must drop the whole @media block: {css}"
        );
    }

    #[test]
    fn anim_strips_breakout_and_sanitises_name() {
        let attrs = vec![attr(
            "data-ipe-anim-rules",
            "9bad name!||300ms ease||0% { opacity: 0 } </style>||1",
        )];
        let css = build_anim("r.0#div", &attrs);
        assert!(!css.contains("</style"), "{css}");
        // Sink hardening: the body carries trailing content
        // after the last keyframe block (`</style>`), so
        // `sink_safe_keyframes_body` rejects it and the whole entry drops
        // FAIL-CLOSED (stronger than the old strip-then-still-emit contract).
        assert_eq!(
            css, "",
            "forged breakout keyframes body must drop the entry: {css}"
        );
    }

    #[test]
    fn anim_sanitises_name_on_legit_entry() {
        // Name sanitisation (leading digit prefixed, non-ident chars → `_`,
        // ipe-id-derived ident suffix) still applies on the legit path.
        let attrs = vec![attr(
            "data-ipe-anim-rules",
            "9bad name!||300ms ease||0% { opacity: 0 } 100% { opacity: 1 }||1",
        )];
        let css = build_anim("r.0#div", &attrs);
        assert!(css.contains("@keyframes _9bad_name___r_0_div"), "{css}");
        assert!(
            css.contains("@media (prefers-reduced-motion: no-preference)"),
            "respect=1 must keep the reduced-motion gate: {css}"
        );
    }

    #[test]
    fn build_anim_sink_keeps_legit_entries_byte_identical() {
        let attrs = vec![attr(
            "data-ipe-anim-rules",
            "fadeIn||300ms ease-out 0ms 1 none||0% { opacity: 0; transform: translateY(10px) } 100% { opacity: 1; transform: translateY(0px) }||0",
        )];
        let css = build_anim("r.0", &attrs);
        assert!(
            css.contains(
                "@keyframes fadeIn__r_0 { 0% { opacity: 0; transform: translateY(10px) } \
                 100% { opacity: 1; transform: translateY(0px) } }"
            ),
            "legit keyframes must survive byte-identical: {css}"
        );
        assert!(
            css.contains("animation: fadeIn__r_0 300ms ease-out 0ms 1 none;"),
            "legit shorthand tail must survive: {css}"
        );
        assert!(
            !css.contains("prefers-reduced-motion"),
            "respect=0 must skip the media gate: {css}"
        );
    }

    #[test]
    fn build_anim_sink_drops_forged_body_breakout() {
        // Forged body: valid first block, then a close-and-inject page-wide
        // rule + at-rule — exactly what `strip_style_close` alone missed.
        let attrs = vec![attr(
            "data-ipe-anim-rules",
            "x||300ms||0% { opacity: 0 } } body { display:none } @import url(//evil/x.css) {||1",
        )];
        assert_eq!(
            build_anim("r.0", &attrs),
            "",
            "forged keyframes-body breakout must drop the entry fail-closed"
        );
    }

    #[test]
    fn build_anim_sink_drops_forged_tail_breakout() {
        let attrs = vec![attr(
            "data-ipe-anim-rules",
            "x||300ms; } [y] { color:red||0% { opacity: 0 }||1",
        )];
        assert_eq!(
            build_anim("r.0", &attrs),
            "",
            "forged shorthand-tail breakout must drop the entry fail-closed"
        );
    }

    #[test]
    fn build_anim_sink_drops_only_the_forged_entry() {
        // Per-entry fail-closed posture (same as build_pc): the forged entry
        // drops, the legit sibling animation still renders.
        let attrs = vec![attr(
            "data-ipe-anim-rules",
            "evil||300ms||0% { opacity: 0 } </style><script>alert(1)</script>||1\
             @@good||200ms linear||0% { opacity: 0 } 100% { opacity: 1 }||1",
        )];
        let css = build_anim("r.0", &attrs);
        assert!(!css.contains("evil__"), "forged entry must drop: {css}");
        assert!(
            !css.to_ascii_lowercase().contains("<script"),
            "script must never render: {css}"
        );
        assert!(
            css.contains("@keyframes good__r_0"),
            "legit sibling entry must survive: {css}"
        );
    }

    #[test]
    fn apply_prepends_style_child_and_strips_marker() {
        let mut tree: Html<()> = Html::HElement(
            "button".to_string(),
            vec![
                attr("ipe-id", "r"),
                attr("data-ipe-pc-rules", "h|color: red"),
            ],
            vec![Html::HText("x".to_string())],
        );
        apply_style_injections(&mut tree);
        match &tree {
            Html::HElement(_, attrs, kids) => {
                assert!(
                    !attrs
                        .iter()
                        .any(|a| matches!(a, Attribute::Attr(k, _) if k == "data-ipe-pc-rules")),
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
        // be hoisted to a sibling slot right after the input.
        let mut tree: Html<()> = Html::HElement(
            "div".to_string(),
            vec![attr("ipe-id", "r")],
            vec![Html::HElement(
                "input".to_string(),
                vec![
                    attr("ipe-id", "r_0_input"),
                    attr("data-ipe-pc-rules", "f|outline: none"),
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
                attr("ipe-id", "r"),
                attr("data-ipe-pc-rules", "f|outline: none"),
            ],
            vec![],
        );
        apply_style_injections(&mut tree);
        if let Html::HElement(_, attrs, _) = &tree {
            assert!(
                !attrs
                    .iter()
                    .any(|a| matches!(a, Attribute::Attr(k, _) if k == "data-ipe-pc-rules")),
                "void-root marker must be stripped"
            );
        } else {
            panic!("expected element");
        }
    }

    /// #113 spec §1.4 end-to-end: the FULL pipeline from a Rust
    /// `Attribute::AttrPseudoRule` (as `Background.hoverColor` constructs it)
    /// through `ui_layout` → `assign_ipe_ids` → `apply_style_injections` →
    /// `render_html` must produce a ipe-id-scoped `<style>` rule and leave NO
    /// `data-ipe-pc-rules` marker in the final HTML. Composes the whole
    /// pipeline, not just each half in isolation.
    #[test]
    fn end_to_end_ui_hover_color_renders_scoped_style_and_leaves_no_marker() {
        use crate::html::{assign_ipe_ids, render_html};
        use crate::ui::element::{Color, Element};
        use crate::ui::helpers::ui_bg_hover_color_;
        use crate::ui::render::ui_layout;

        let attrs = vec![ui_bg_hover_color_::<()>(Color::Rgba(0, 92, 215, 1.0))];
        let elem: Element<()> = Element::Text("hover me".to_owned());
        let mut html = ui_layout(attrs, elem);
        assign_ipe_ids(&mut html, "r");
        apply_style_injections(&mut html);
        let s = render_html(&html);
        assert!(
            !s.contains("data-ipe-pc-rules"),
            "marker must be consumed, never leak into final HTML: {s}"
        );
        assert!(
            s.contains("<style"),
            "scoped <style> block must render: {s}"
        );
        assert!(
            s.contains(":hover") && s.contains("background-color:rgba(0,92,215,1)"),
            "hover rule with the exact CSS must render: {s}"
        );
        assert!(
            s.contains("@media (hover: hover)"),
            "hover rules must be wrapped in the touch-device guard: {s}"
        );
    }

    /// UI CSS-escaping hardening (spec §6.2), the direct regression for
    /// Repro A: an adversarial `Font.family` value routed through
    /// `Ui.onPseudo` → `build_style_string` → `collect_html_attrs` →
    /// `apply_style_injections` must not survive into the emitted `<style>`
    /// block as a page-wide rule. The `}` breakout is dropped upstream in
    /// `build_style_string`, so no `body{`/`display:none` reaches the sink,
    /// while the scoped `:hover` rule + `@media (hover: hover)` guard stay.
    #[test]
    fn onpseudo_font_family_breakout_is_neutralised_end_to_end() {
        use crate::html::{assign_ipe_ids, render_html};
        use crate::ui::element::Element;
        use crate::ui::helpers::{ui_hover_, ui_on_pseudo_};
        use crate::ui::render::ui_layout;

        let evil = "s } body { display:none } .x:hover {".to_owned();
        let pseudo = ui_on_pseudo_::<()>(
            ui_hover_(),
            vec![crate::ui::element::Attribute::AttrFontFamily(evil)],
        );
        let mut html = ui_layout(vec![pseudo], Element::Text("hi".to_owned()));
        assign_ipe_ids(&mut html, "r");
        apply_style_injections(&mut html);
        let out = render_html(&html);
        assert!(
            !out.contains("body {") && !out.contains("body{") && !out.contains("display:none"),
            "page-wide selector must not survive the collector gate: {out}"
        );
    }

    /// Repro B: an `@import` breakout through `Background.image` inside
    /// `Ui.onPseudo` must not reach the emitted `<style>` block.
    #[test]
    fn onpseudo_bg_image_import_breakout_is_neutralised_end_to_end() {
        use crate::html::{assign_ipe_ids, render_html};
        use crate::ui::element::Element;
        use crate::ui::helpers::{ui_hover_, ui_on_pseudo_};
        use crate::ui::render::ui_layout;

        let evil = "x) } @import url(\"https://evil.example/x.css\") ; .y:hover { background:url(x"
            .to_owned();
        let pseudo = ui_on_pseudo_::<()>(
            ui_hover_(),
            vec![crate::ui::element::Attribute::AttrBgImage(evil)],
        );
        let mut html = ui_layout(vec![pseudo], Element::Text("hi".to_owned()));
        assign_ipe_ids(&mut html, "r");
        apply_style_injections(&mut html);
        let out = render_html(&html);
        assert!(
            !out.contains("@import"),
            "remote @import must not survive the collector gate: {out}"
        );
    }

    /// `Ui.mediaQuery` exact-output pin at the injector: the marker pair on a
    /// ipe-id-stamped element must expand to EXACTLY the upstream shape
    /// `<style data-ipe-mq="<sid>">@media <q> { [ipe-id="<sid>"] { <rules> } }</style>`
    /// prepended as the first child, with both markers stripped.
    #[test]
    fn media_query_markers_expand_to_exact_scoped_style_block() {
        use crate::html::render_html;

        let mut tree: Html<()> = Html::HElement(
            "div".to_string(),
            vec![
                attr("ipe-id", "mq0"),
                attr("data-ipe-mq-q", "(min-width: 768px)"),
                attr("data-ipe-mq-rules", "background-color:rgba(18,18,24,1)"),
            ],
            vec![Html::HText("responsive".to_string())],
        );
        apply_style_injections(&mut tree);
        let s = render_html(&tree);
        assert!(
            s.contains(
                "<style data-ipe-mq=\"mq0\">@media (min-width: 768px) { \
                 [ipe-id=\"mq0\"] { background-color:rgba(18,18,24,1) } }</style>"
            ),
            "exact <style data-ipe-mq=…> block must render: {s}"
        );
        assert!(
            !s.contains("data-ipe-mq-q") && !s.contains("data-ipe-mq-rules"),
            "markers must be consumed, never leak into final HTML: {s}"
        );
    }

    /// `Ui.mediaQuery` FULL-pipeline end-to-end (producer → consumer): the
    /// runtime helper's wrapper (`ui_media_query_`) through `ui_layout` →
    /// `assign_ipe_ids` → `apply_style_injections` → `render_html` must
    /// produce a ipe-id-scoped `@media` `<style>` block, leave no marker, and
    /// keep the rule keyed to the wrapper's own ipe-id so two media queries
    /// on one page cannot cross-contaminate.
    #[test]
    fn end_to_end_ui_media_query_renders_scoped_style_and_leaves_no_marker() {
        use crate::html::{assign_ipe_ids, render_html};
        use crate::ui::element::{Attribute, Color, Element};
        use crate::ui::helpers::ui_media_query_;
        use crate::ui::render::ui_layout;

        let elem = ui_media_query_::<()>(
            "(min-width: 768px)".to_owned(),
            vec![Attribute::AttrBgColor(Color::Rgba(18, 18, 24, 1.0))],
            Element::Text("responsive".to_owned()),
        );
        let mut html = ui_layout(vec![], elem);
        assign_ipe_ids(&mut html, "r");
        apply_style_injections(&mut html);
        let s = render_html(&html);
        assert!(
            !s.contains("data-ipe-mq-q") && !s.contains("data-ipe-mq-rules"),
            "markers must be consumed, never leak into final HTML: {s}"
        );
        assert!(
            s.contains("<style data-ipe-mq=\""),
            "scoped <style data-ipe-mq=…> block must render: {s}"
        );
        assert!(
            s.contains("@media (min-width: 768px) { [ipe-id=\""),
            "@media rule must be ipe-id-scoped: {s}"
        );
        assert!(
            s.contains("background-color:rgba(18,18,24,1) } }"),
            "collector rules must land inside the scoped block: {s}"
        );
    }

    /// SECURITY end-to-end: a breakout media-query string through the real
    /// producer must be neutralised — the `SafeCssMediaQuery` gate drops the
    /// markers at construction, so the pipeline emits NO `@media`, NO
    /// `</style` breakout, and NO `<script>`, while the child still renders.
    #[test]
    fn end_to_end_ui_media_query_breakout_is_neutralised() {
        use crate::html::{assign_ipe_ids, render_html};
        use crate::ui::element::{Attribute, Color, Element};
        use crate::ui::helpers::ui_media_query_;
        use crate::ui::render::ui_layout;

        let elem = ui_media_query_::<()>(
            "(min-width: 1px) </style><script>alert(1)</script> { } @import url(evil)".to_owned(),
            vec![Attribute::AttrBgColor(Color::Rgba(1, 2, 3, 1.0))],
            Element::Text("still here".to_owned()),
        );
        let mut html = ui_layout(vec![], elem);
        assign_ipe_ids(&mut html, "r");
        apply_style_injections(&mut html);
        let s = render_html(&html);
        let low = s.to_ascii_lowercase();
        assert!(
            !low.contains("</style><script"),
            "breakout must not form: {s}"
        );
        assert!(!low.contains("<script"), "script must never render: {s}");
        assert!(
            !low.contains("@media"),
            "gated query must emit no rule: {s}"
        );
        assert!(!low.contains("@import"), "at-rule must not survive: {s}");
        assert!(s.contains("still here"), "child must still render: {s}");
    }

    // ── Sink-side forgery gates ───────────────
    // The style markers are raw String attributes a caller can FORGE via
    // `Ui.htmlAttribute "data-ipe-mq-rules" "…"`, bypassing the producer's
    // SafeCssValue / SafeCssMediaQuery gates entirely. The sink builders must
    // re-validate (parse, don't validate at the real boundary) and drop the
    // block fail-closed. `strip_style_close` alone did NOT catch `{ } ; @import`.

    #[test]
    fn build_mq_sink_drops_forged_rules_breakout() {
        // Forged rules containing a ruleset breakout + at-rule injection.
        let attrs = vec![
            attr("data-ipe-mq-q", "screen"),
            attr(
                "data-ipe-mq-rules",
                "color:red } [ipe-id=\"x\"] { } @import url(//evil/x.css)",
            ),
        ];
        assert_eq!(
            build_mq("r.0", &attrs),
            "",
            "forged breakout rules must drop the whole @media block"
        );
    }

    #[test]
    fn build_mq_sink_drops_forged_query_breakout() {
        let attrs = vec![
            attr("data-ipe-mq-q", "screen { } [x] { }"),
            attr("data-ipe-mq-rules", "color:red"),
        ];
        assert_eq!(
            build_mq("r.0", &attrs),
            "",
            "forged breakout query must drop the whole @media block"
        );
    }

    #[test]
    fn build_mq_sink_keeps_legit_markers() {
        let attrs = vec![
            attr("data-ipe-mq-q", "(min-width: 768px)"),
            attr("data-ipe-mq-rules", "background-color:rgba(18,18,24,1)"),
        ];
        let out = build_mq("r.0", &attrs);
        assert!(
            out.contains("@media (min-width: 768px) { [ipe-id=\"r.0\"]"),
            "legit query + selector must survive: {out}"
        );
        assert!(
            out.contains("background-color:rgba(18,18,24,1)"),
            "legit rules must survive byte-identical: {out}"
        );
    }

    #[test]
    fn build_tr_sink_drops_forged_breakout() {
        let attrs = vec![attr(
            "data-ipe-tr-rules",
            "x } [y] { color:red } @import url(evil)",
        )];
        assert_eq!(
            build_tr("r.0", &attrs),
            "",
            "forged transition breakout must drop the block"
        );
    }

    #[test]
    fn build_pc_sink_drops_forged_breakout() {
        let attrs = vec![attr(
            "data-ipe-pc-rules",
            "h|color:red } [x] { } @import url(evil)",
        )];
        assert_eq!(
            build_pc("r.0", &attrs),
            "",
            "forged pseudo-class breakout must drop that rule"
        );
    }

    #[test]
    fn idempotent_second_run_adds_no_duplicate_style() {
        let mut tree: Html<()> = Html::HElement(
            "div".to_string(),
            vec![
                attr("ipe-id", "r"),
                attr("data-ipe-tr-rules", "color 200ms"),
            ],
            vec![],
        );
        apply_style_injections(&mut tree);
        let first = count_styles(&tree);
        apply_style_injections(&mut tree);
        assert_eq!(first, count_styles(&tree), "second run must be a no-op");
    }
}

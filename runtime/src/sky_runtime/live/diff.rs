use crate::sky_runtime::live::html::*;
use serde::Serialize;
use std::collections::HashMap;

/// A single DOM patch emitted by `diff`. Field names mirror the Go `Patch` struct
/// (JSON: `id`, `text`, `html`, `attrs`, `remove`) — see `runtime-go/rt/live.go`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Patch {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    /// Attribute delta: present key with non-empty value → set; empty value → remove.
    /// Go convention: `""` means remove; `BoolAttr(k,true)` encodes as `{k:k}`.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub attrs: HashMap<String, String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub remove: bool,
}

impl Patch {
    fn for_id(id: &str) -> Self {
        Patch {
            id: id.into(),
            text: None,
            html: None,
            attrs: HashMap::new(),
            remove: false,
        }
    }
}

/// Structural diff between two `Html` trees that have already had `assign_sky_ids`
/// applied. Returns the minimal list of `Patch` operations needed to update the
/// DOM from `old` to `new`. Faithful port of Go `diffNodes`
/// (`runtime-go/rt/live.go`):
/// - Matched-tag element pair: diff attributes + events, then children.
/// - Tag/kind mismatch, child-count change, or any mixed-child text change:
///   whole-subtree `html` replace at the parent.
/// - Sole text-child change: `SetText` via `p.text` (fast path).
/// - Event handlers toggled on/off: `sky-<event>` attr set/remove + `data-sky-hid`.
/// - Keyed identity is carried by `assign_sky_ids` (the `:{key}` segment) so a
///   reordered keyed item keeps its sky-id and only its moved attrs patch.
pub fn diff<M>(old: &Html<M>, new: &Html<M>) -> Vec<Patch> {
    let mut out = vec![];
    diff_node(old, new, &mut out);
    out
}

// ─── internal helpers ─────────────────────────────────────────────────────────

fn sky_id<M>(n: &Html<M>) -> Option<&str> {
    if let Html::HElement(_, attrs, _) = n {
        for a in attrs {
            if let Attribute::Attr(k, v) = a {
                if k == "sky-id" {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Emit a whole-subtree innerHTML replace at `id` (Go: `Patch{ID, HTML}`).
fn push_html_replace<M>(id: &str, new_kids: &[Html<M>], out: &mut Vec<Patch>) {
    if id.is_empty() {
        return;
    }
    let mut p = Patch::for_id(id);
    p.html = Some(render_children(new_kids));
    out.push(p);
}

fn diff_node<M>(old: &Html<M>, new: &Html<M>, out: &mut Vec<Patch>) {
    let (ot, oa, ok, _nt, na, nk) = match (old, new) {
        (Html::HElement(ot, oa, ok), Html::HElement(nt, na, nk)) if ot == nt => {
            (ot, oa, ok, nt, na, nk)
        }
        // Tag/kind mismatch is handled by the parent (mixed-child / count branch).
        // A top-level mismatch has no parent to address, so nothing to emit.
        _ => return,
    };
    let _ = ot;
    // Patch id targets the element currently in the DOM — the OLD tree's id
    // (Go parity: `old.SkyID`).
    let id = sky_id(old).unwrap_or("").to_string();

    // Attribute + event delta.
    let mut p = Patch::for_id(&id);
    diff_attrs(oa, na, &mut p);
    if !id.is_empty() && !p.attrs.is_empty() {
        out.push(p);
    }

    // Sole text-child fast path (common for buttons / spans).
    if ok.len() == 1 && nk.len() == 1 {
        if let (Some(Html::HText(o)), Some(Html::HText(n))) = (ok.first(), nk.first()) {
            if o != n && !id.is_empty() {
                let mut tp = Patch::for_id(&id);
                tp.text = Some(n.clone());
                out.push(tp);
            }
            return;
        }
    }

    // Child-count change → replace the whole subtree.
    if ok.len() != nk.len() {
        push_html_replace(&id, nk, out);
        return;
    }

    // Per-position structural diff.
    for (oc, nc) in ok.iter().zip(nk.iter()) {
        match (oc, nc) {
            (Html::HText(o), Html::HText(n)) => {
                // Mixed-child text change → replace the whole subtree at the parent
                // (Go parity: single-text is the fast path above; anything else is a
                // parent html-replace).
                if o != n {
                    push_html_replace(&id, nk, out);
                    return;
                }
            }
            // Raw-vs-raw: Go recurses (a no-op) — changed raw content is not
            // patched. Match that quirk rather than emitting a spurious replace.
            (Html::HRaw(_), Html::HRaw(_)) => {}
            (Html::HElement(t1, _, _), Html::HElement(t2, _, _)) if t1 == t2 => {
                diff_node(oc, nc, out);
            }
            // Tag / kind mismatch → replace the subtree at the parent.
            _ => {
                push_html_replace(&id, nk, out);
                return;
            }
        }
    }
}

/// Compute the attribute + event delta between `old` and `new`.
/// Keys changed or added → new value. Keys removed → empty string (Go convention).
/// `sky-id` is excluded (never patched as an attribute).
fn diff_attrs<M>(old: &[Attribute<M>], new: &[Attribute<M>], p: &mut Patch) {
    let collect = |xs: &[Attribute<M>]| -> HashMap<String, String> {
        let mut m = HashMap::new();
        for a in xs {
            match a {
                Attribute::Attr(k, v) if k != "sky-id" => {
                    m.insert(k.clone(), v.clone());
                }
                Attribute::BoolAttr(k, true) => {
                    // Go parity: live.go line 190 `vn.Attrs[k] = k`.
                    // Key-as-value encodes a present boolean attr; "" is the remove sentinel only.
                    m.insert(k.clone(), k.clone());
                }
                _ => {}
            }
        }
        m
    };
    let (om, nm) = (collect(old), collect(new));
    for (k, v) in &nm {
        if om.get(k) != Some(v) {
            p.attrs.insert(k.clone(), v.clone());
        }
    }
    for k in om.keys() {
        if !nm.contains_key(k) {
            // Signal removal with empty string (Go convention).
            p.attrs.insert(k.clone(), String::new());
        }
    }
    diff_events(old, new, p);
}

/// Event-handler delta. Mirrors Go `diffNodes`' Events block: an element gaining
/// a handler emits `sky-<event>` = `<event>` (the value the client posts back as
/// `msg`, matching `render_html`) plus a fresh `data-sky-hid`; an element losing a
/// handler emits `sky-<event>` = `""` (remove), and clears `data-sky-hid` once the
/// last handler is gone. Without this, toggling a handler leaves a stale listener
/// marker and the user's gesture is silently dropped.
fn diff_events<M>(old: &[Attribute<M>], new: &[Attribute<M>], p: &mut Patch) {
    let names = |xs: &[Attribute<M>]| -> Vec<String> {
        xs.iter()
            .filter_map(|a| match a {
                Attribute::EventAttr(e) => Some(e.name().to_string()),
                _ => None,
            })
            .collect()
    };
    // Wire-marker key per event name — MUST match render_html's emission
    // (html.rs): file/image meta-events are already `sky-`-prefixed and the
    // client reads them as `data-sky-ev-<name>`; plain DOM events use
    // `sky-<name>`. Render and diff disagreeing here = a dead listener or a
    // spurious patch.
    let ev_key = |ev: &str| -> String {
        if ev.starts_with("sky-") {
            format!("data-sky-ev-{ev}")
        } else {
            format!("sky-{ev}")
        }
    };
    let (on, nn) = (names(old), names(new));
    let id = p.id.clone();
    for ev in &nn {
        if !on.contains(ev) {
            p.attrs.insert(ev_key(ev), ev.clone());
            p.attrs.insert("data-sky-hid".into(), id.clone());
        }
    }
    for ev in &on {
        if !nn.contains(ev) {
            p.attrs.insert(ev_key(ev), String::new());
            if nn.is_empty() {
                p.attrs.insert("data-sky-hid".into(), String::new());
            }
        }
    }
}

fn render_children<M>(kids: &[Html<M>]) -> String {
    let mut s = String::new();
    for c in kids {
        s.push_str(&render_html(c));
    }
    s
}

// ─── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(h: &mut Html<()>) {
        assign_sky_ids(h, "r");
    }

    #[test]
    fn diff_text_change() {
        let mut a: Html<()> = Html::HElement("p".into(), vec![], vec![Html::HText("1".into())]);
        let mut b: Html<()> = Html::HElement("p".into(), vec![], vec![Html::HText("2".into())]);
        ids(&mut a);
        ids(&mut b);
        let p = diff(&a, &b);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].id, "r");
        assert_eq!(p[0].text.as_deref(), Some("2"));
    }

    #[test]
    fn diff_attr_set_and_remove() {
        let mut a: Html<()> = Html::HElement(
            "div".into(),
            vec![Attribute::Attr("class".into(), "x".into())],
            vec![],
        );
        let mut b: Html<()> = Html::HElement(
            "div".into(),
            vec![
                Attribute::Attr("class".into(), "y".into()),
                Attribute::Attr("title".into(), "t".into()),
            ],
            vec![],
        );
        ids(&mut a);
        ids(&mut b);
        let p = diff(&a, &b);
        assert_eq!(p.len(), 1);
        let attrs = &p[0].attrs;
        assert_eq!(attrs.get("class").map(String::as_str), Some("y"));
        assert_eq!(attrs.get("title").map(String::as_str), Some("t"));
    }

    #[test]
    fn diff_identical_is_empty() {
        let mut a: Html<()> = Html::HElement("p".into(), vec![], vec![Html::HText("1".into())]);
        let mut b = a.clone();
        ids(&mut a);
        ids(&mut b);
        assert!(diff(&a, &b).is_empty());
    }

    #[test]
    fn diff_bool_attr_add_uses_key_as_value() {
        let mut a: Html<()> = Html::HElement("button".into(), vec![], vec![]);
        let mut b: Html<()> = Html::HElement(
            "button".into(),
            vec![Attribute::BoolAttr("disabled".into(), true)],
            vec![],
        );
        ids(&mut a);
        ids(&mut b);
        let p = diff(&a, &b);
        assert_eq!(p.len(), 1);
        // Go parity: present BoolAttr encodes as {k: k}, NOT {k: ""}.
        assert_eq!(
            p[0].attrs.get("disabled").map(String::as_str),
            Some("disabled")
        );
    }

    #[test]
    fn diff_event_added_emits_marker_and_hid() {
        // <button> gains an onClick: client needs sky-click + data-sky-hid to bind.
        let mut a: Html<()> = Html::HElement("button".into(), vec![], vec![]);
        let mut b: Html<()> = Html::HElement(
            "button".into(),
            vec![Attribute::EventAttr(Event::OnMsg("click".into(), ()))],
            vec![],
        );
        ids(&mut a);
        ids(&mut b);
        let p = diff(&a, &b);
        assert_eq!(p.len(), 1);
        assert_eq!(
            p[0].attrs.get("sky-click").map(String::as_str),
            Some("click")
        );
        assert_eq!(
            p[0].attrs.get("data-sky-hid").map(String::as_str),
            Some("r")
        );
    }

    #[test]
    fn diff_event_removed_clears_marker_and_hid() {
        let mut a: Html<()> = Html::HElement(
            "button".into(),
            vec![Attribute::EventAttr(Event::OnMsg("click".into(), ()))],
            vec![],
        );
        let mut b: Html<()> = Html::HElement("button".into(), vec![], vec![]);
        ids(&mut a);
        ids(&mut b);
        let p = diff(&a, &b);
        assert_eq!(p.len(), 1);
        // Removal sentinel: empty string for both the marker and the (now-stale) hid.
        assert_eq!(p[0].attrs.get("sky-click").map(String::as_str), Some(""));
        assert_eq!(p[0].attrs.get("data-sky-hid").map(String::as_str), Some(""));
    }

    #[test]
    fn diff_mixed_child_text_change_replaces_parent() {
        // Parent with [<span>, text]; the text child changes → Go emits a parent
        // html-replace (the sole-text fast path doesn't apply to mixed children).
        let mk = |t: &str| -> Html<()> {
            Html::HElement(
                "div".into(),
                vec![],
                vec![
                    Html::HElement("span".into(), vec![], vec![]),
                    Html::HText(t.into()),
                ],
            )
        };
        let mut a = mk("x");
        let mut b = mk("y");
        ids(&mut a);
        ids(&mut b);
        let p = diff(&a, &b);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].id, "r");
        assert!(p[0].html.is_some(), "expected html replace, got {:?}", p[0]);
        assert!(p[0].text.is_none());
    }

    #[test]
    fn diff_child_count_change_replaces_parent() {
        let mut a: Html<()> = Html::HElement(
            "ul".into(),
            vec![],
            vec![Html::HElement("li".into(), vec![], vec![])],
        );
        let mut b: Html<()> = Html::HElement(
            "ul".into(),
            vec![],
            vec![
                Html::HElement("li".into(), vec![], vec![]),
                Html::HElement("li".into(), vec![], vec![]),
            ],
        );
        ids(&mut a);
        ids(&mut b);
        let p = diff(&a, &b);
        assert_eq!(p.len(), 1);
        assert!(p[0].html.is_some());
    }

    #[test]
    fn diff_bool_attr_remove_uses_empty_string() {
        let mut a: Html<()> = Html::HElement(
            "button".into(),
            vec![Attribute::BoolAttr("disabled".into(), true)],
            vec![],
        );
        let mut b: Html<()> = Html::HElement("button".into(), vec![], vec![]);
        ids(&mut a);
        ids(&mut b);
        let p = diff(&a, &b);
        assert_eq!(p.len(), 1);
        // Removal sentinel: empty string (Go convention).
        assert_eq!(p[0].attrs.get("disabled").map(String::as_str), Some(""));
    }
}

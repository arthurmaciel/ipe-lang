use crate::sky_runtime::live::html::*;
use std::collections::HashMap;

/// Per-session handler index: maps `sky-id` → (`event-name` → the cloneable
/// `Event` that owns the handler closure, via `Arc`).
///
/// Built once per `view` commit via [`build_index`]; thrown away and rebuilt
/// on every update cycle (the view function is the single source of truth).
///
/// The two-level shape lets [`Self::resolve`] / [`Self::resolve_form`] look up
/// by `&str` (`HashMap<String, _>: get(&str)` via `Borrow<str>`) without
/// heap-allocating throwaway key Strings on the wire-event hot path.
pub struct HandlerIndex<M> {
    map: HashMap<String, HashMap<String, Event<M>>>,
}

impl<M: Clone> HandlerIndex<M> {
    /// Resolve a wire event from the browser.
    ///
    /// - `OnMsg`   — returns the message directly (args ignored).
    /// - `OnString`— calls the closure with `args[0]` (or `""` if absent).
    /// - `OnBool`  — calls the closure with `args[0] == "true"` (or `false`).
    /// - `OnForm`  — dispatched via [`Self::resolve_form`]; returns `None` here.
    ///
    /// Returns `None` when the sky-id is unknown or the event name doesn't
    /// match any registered handler.
    pub fn resolve(&self, sky_id: &str, event: &str, args: &[String]) -> Option<M> {
        match self.map.get(sky_id)?.get(event)? {
            Event::OnMsg(_, m) => Some(m.clone()),
            Event::OnString(_, f) => Some(f(args.first().cloned().unwrap_or_default())),
            Event::OnBool(_, f) => Some(f(args.first().map(|s| s == "true").unwrap_or(false))),
            Event::OnForm(_, _) => None, // dispatched via resolve_form
            Event::OnRaw(_, _) => None,  // heterogeneous payload — not dispatchable
        }
    }

    /// Resolve a form-submit event. Distinct from [`Self::resolve`] because
    /// the `FormData` map arrives via the form-submission wire path, not the
    /// positional `args` slice.
    pub fn resolve_form(&self, sky_id: &str, event: &str, fd: FormData) -> Option<M> {
        match self.map.get(sky_id)?.get(event)? {
            Event::OnForm(_, f) => f(fd), // f already returns Option<M> (None on decode failure)
            _ => None,
        }
    }
}

/// Build a [`HandlerIndex`] by walking `root` and collecting every
/// `Attribute::Event` keyed by its element's `sky-id` + event name.
///
/// Precondition: `assign_sky_ids` must have been called on `root` first.
/// Elements without a `sky-id` attribute (shouldn't happen after assignment)
/// are indexed under the empty-string key, which is harmless — no browser
/// event will carry an empty sky-id.
pub fn build_index<M: Clone>(root: &Html<M>) -> HandlerIndex<M> {
    let mut map = HashMap::new();
    walk(root, &mut map);
    HandlerIndex { map }
}

/// Walk the view tree iteratively with an explicit heap work-stack.
///
/// Native recursion here would overflow the (uncatchable) thread stack on a
/// deeply nested view — e.g. a comment/thread tree whose nesting depth scales
/// with attacker-influenced data. An explicit `Vec` work-list keeps
/// index-building O(nodes) in heap memory, bounded only by allocation.
fn walk<M: Clone>(root: &Html<M>, map: &mut HashMap<String, HashMap<String, Event<M>>>) {
    let mut stack: Vec<&Html<M>> = vec![root];
    while let Some(n) = stack.pop() {
        if let Html::HElement(_, attrs, kids) = n {
            let id = attrs
                .iter()
                .find_map(|a| match a {
                    Attribute::Attr(k, v) if k == "sky-id" => Some(v.clone()),
                    _ => None,
                })
                .unwrap_or_default();

            for a in attrs {
                if let Attribute::EventAttr(e) = a {
                    map.entry(id.clone())
                        .or_default()
                        .insert(e.name().to_string(), e.clone());
                }
            }

            for c in kids {
                stack.push(c);
            }
        }
    }
}

// ─── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    enum Msg {
        Inc,
        Typed(String),
    }

    fn tree() -> Html<Msg> {
        let mut t = Html::HElement(
            "div".into(),
            vec![],
            vec![
                Html::HElement(
                    "button".into(),
                    vec![Attribute::EventAttr(Event::OnMsg("click".into(), Msg::Inc))],
                    vec![],
                ),
                Html::HElement(
                    "input".into(),
                    vec![Attribute::EventAttr(Event::OnString(
                        "input".into(),
                        std::sync::Arc::new(Msg::Typed),
                    ))],
                    vec![],
                ),
            ],
        );
        assign_sky_ids(&mut t, "r");
        t
    }

    #[test]
    fn resolves_onmsg_and_onstring() {
        let idx = build_index(&tree());
        assert_eq!(idx.resolve("r_0_button", "click", &[]), Some(Msg::Inc));
        assert_eq!(
            idx.resolve("r_1_input", "input", &["hi".into()]),
            Some(Msg::Typed("hi".into()))
        );
        assert_eq!(idx.resolve("r_0_button", "input", &[]), None); // wrong event
        assert_eq!(idx.resolve("nope", "click", &[]), None); // unknown id
    }

    #[test]
    fn resolves_onbool() {
        let mut t = Html::HElement(
            "input".into(),
            vec![Attribute::EventAttr(Event::OnBool(
                "change".into(),
                std::sync::Arc::new(|b| {
                    if b {
                        Msg::Inc
                    } else {
                        Msg::Typed("off".into())
                    }
                }),
            ))],
            vec![],
        );
        assign_sky_ids(&mut t, "r");
        let idx = build_index(&t);
        assert_eq!(idx.resolve("r", "change", &["true".into()]), Some(Msg::Inc));
        assert_eq!(
            idx.resolve("r", "change", &["false".into()]),
            Some(Msg::Typed("off".into()))
        );
    }

    #[test]
    fn resolves_onform() {
        let mut t = Html::HElement(
            "form".into(),
            vec![Attribute::EventAttr(Event::OnForm(
                "submit".into(),
                std::sync::Arc::new(|fd: FormData| {
                    Some(Msg::Typed(fd.get("name").cloned().unwrap_or_default()))
                }),
            ))],
            vec![],
        );
        assign_sky_ids(&mut t, "r");
        let idx = build_index(&t);

        // resolve() returns None for OnForm; resolve_form() dispatches it.
        assert_eq!(idx.resolve("r", "submit", &[]), None);

        let mut fd = FormData::new();
        fd.insert("name".into(), "alice".into());
        assert_eq!(
            idx.resolve_form("r", "submit", fd),
            Some(Msg::Typed("alice".into()))
        );
    }

    #[test]
    fn onstring_empty_args_gives_default() {
        let mut t = Html::HElement(
            "input".into(),
            vec![Attribute::EventAttr(Event::OnString(
                "input".into(),
                std::sync::Arc::new(Msg::Typed),
            ))],
            vec![],
        );
        assign_sky_ids(&mut t, "r");
        let idx = build_index(&t);
        // No args → closure receives ""
        assert_eq!(
            idx.resolve("r", "input", &[]),
            Some(Msg::Typed(String::new()))
        );
    }
}

use std::collections::HashMap;

use super::css_safety;

/// Form data delivered to an `OnForm` handler (lower-cased keys; see dispatch).
pub type FormData = HashMap<String, String>;

#[derive(Clone, Debug, PartialEq)]
pub enum Html<M> {
    /// Normal element node — matches Sky's `HElement tag attrs children`.
    HElement(String, Vec<Attribute<M>>, Vec<Html<M>>),
    /// Text node (HTML-escaped on render) — matches Sky's `HText s`.
    HText(String),
    /// Raw, un-escaped HTML — trusted pre-rendered content only; caller sanitises.
    /// Matches Sky's `HRaw s`.
    HRaw(String),
}

/// Variant names mirror the Sky stdlib `Std.Html.Attributes.Attribute` ADT
/// (`Attr | BoolAttr | EventAttr (Event msg) | NoAttr`) so the Rust codegen's
/// bridge (`StdHtmlAttributesAttribute<msg> = sky_runtime::Attribute<msg>`)
/// constructs the right variants by name.
#[derive(Clone)]
pub enum Attribute<M> {
    Attr(String, String),
    BoolAttr(String, bool),
    EventAttr(Event<M>),
    /// Sentinel for a conditionally-absent attribute; skipped during render.
    NoAttr,
}

/// Variant names mirror the Sky stdlib `Std.Html.Attributes.Event` ADT
/// (`OnMsg | OnString | OnBool | OnRaw String any`). `OnString`/`OnBool` carry
/// `Arc<dyn Fn(..) -> msg>` (not bare fn pointers) so the handler can be a
/// CAPTURING closure — exactly as the Go backend allows. A faithful Sky.Live
/// app's `onChange = \s -> toMsg (parse s default)` captures locals; a bare
/// fn-pointer field rejected that. Bare ctors / non-capturing fns coerce into
/// `Arc::new` fine; capturing closures box into the trait object. This follows
/// the `OnForm` precedent (already `Arc<dyn Fn>`). `OnRaw` is the
/// heterogeneous-payload escape hatch (`on` / `onSubmit`); its payload is
/// type-erased — not dispatchable, but kept so the bridge compiles. The
/// `submit` wire path resolves via `OnForm` instead (constructed server-side,
/// never from Sky stdlib).
#[derive(Clone)]
pub enum Event<M> {
    OnMsg(String, M),
    OnString(String, std::sync::Arc<dyn Fn(String) -> M + Send + Sync>),
    OnBool(String, std::sync::Arc<dyn Fn(bool) -> M + Send + Sync>),
    OnRaw(String, std::sync::Arc<dyn std::any::Any + Send + Sync>),
    /// Server-constructed form handler (not produced by the Sky stdlib bridge).
    /// Returns `Option<M>`: a malformed/incomplete form (decode failure) yields
    /// `None` so the live loop dispatches no Msg (see `decode_form`).
    OnForm(
        String,
        std::sync::Arc<dyn Fn(FormData) -> Option<M> + Send + Sync>,
    ),
}

impl<M: PartialEq> PartialEq for Attribute<M> {
    fn eq(&self, o: &Self) -> bool {
        use Attribute::*;
        match (self, o) {
            (Attr(a, b), Attr(c, d)) => a == c && b == d,
            (BoolAttr(a, b), BoolAttr(c, d)) => a == c && b == d,
            (EventAttr(a), EventAttr(b)) => a == b,
            (NoAttr, NoAttr) => true,
            _ => false,
        }
    }
}

impl<M> std::fmt::Debug for Attribute<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Attribute::Attr(k, v) => write!(f, "Attr({k:?},{v:?})"),
            Attribute::BoolAttr(k, v) => write!(f, "BoolAttr({k:?},{v})"),
            Attribute::EventAttr(e) => write!(f, "{e:?}"),
            Attribute::NoAttr => write!(f, "NoAttr"),
        }
    }
}

// Structural equality only: (variant kind, event name). The message payload /
// closure is deliberately ignored. Sky.Live's diff is structure-only — it just
// decides whether a DOM node carries a listener of a given event name. The
// actual handler lives server-side in the per-session handler_index, which is
// REBUILT from the fresh view on every commit, so the diff never needs to
// compare handlers. Comparing payloads here would cause spurious re-renders
// (e.g. OnMsg("click", Inc) vs OnMsg("click", Dec) are equal for diff purposes).
impl<M> PartialEq for Event<M> {
    fn eq(&self, o: &Self) -> bool {
        self.kind_name() == o.kind_name()
    }
}

impl<M> Event<M> {
    pub fn name(&self) -> &str {
        match self {
            Event::OnMsg(n, _)
            | Event::OnString(n, _)
            | Event::OnBool(n, _)
            | Event::OnRaw(n, _)
            | Event::OnForm(n, _) => n,
        }
    }

    fn kind_name(&self) -> (u8, &str) {
        match self {
            Event::OnMsg(n, _) => (0, n),
            Event::OnString(n, _) => (1, n),
            Event::OnBool(n, _) => (2, n),
            Event::OnRaw(n, _) => (4, n),
            Event::OnForm(n, _) => (3, n),
        }
    }

    fn kind_tag(&self) -> &'static str {
        match self {
            Event::OnMsg(..) => "OnMsg",
            Event::OnString(..) => "OnString",
            Event::OnBool(..) => "OnBool",
            Event::OnRaw(..) => "OnRaw",
            Event::OnForm(..) => "OnForm",
        }
    }
}

impl<M> std::fmt::Debug for Event<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Event::{}({})", self.kind_tag(), self.name())
    }
}

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// True for HTML void elements (no children, self-closing). Exposed for the
/// style-injection pass, which must hoist a `<style>` to a sibling slot after a
/// void element because `render_into` emits no children for void tags (#409).
/// Its sole consumer is `live/style_inject.rs`, so it is `live`-gated (webview
/// enables `live` too); a non-live build that gained a caller would fail loud.
#[cfg(feature = "live")]
pub(crate) fn is_void(tag: &str) -> bool {
    VOID.contains(&tag)
}

/// Render an `Html` tree to an HTML string. Text is HTML-escaped; Raw is
/// emitted verbatim; void elements self-close with no children; event
/// handlers emit a `data-sky-on="<space-separated event names>"` marker
/// attribute that the browser client reads to bind listeners. Mirrors Go
/// `renderVNode`.
pub fn render_html<M>(node: &Html<M>) -> String {
    let mut s = String::new();
    render_into(node, &mut s);
    s
}

/// Maximum Html nesting depth the renderer and the sky-id stamper descend.
/// The Html tree is produced by the Sky `view` from Model, and Model commonly
/// holds attacker-influenced data (nested comments / replies / a worst-case
/// string folded into a chain of wrapper elements). Recursing once per nesting
/// level with no cap would overflow the thread stack and ABORT the whole
/// process — a panic the no-runtime-error thesis forbids. At the cap we stop
/// descending (deeper nodes are not emitted / not stamped) rather than recurse
/// further: a truncated render is strictly better than a process abort, and no
/// legitimate UI nests anywhere near this deep.
const MAX_HTML_DEPTH: usize = 1024;

fn render_into<M>(node: &Html<M>, s: &mut String) {
    render_into_ctx(node, s, None, false, 0)
}

// `select_value`: when this node renders as a direct child of a `<select>` that
// carries a value, the chosen value is threaded here so the matching `<option>`
// flips `selected` (Go renderVNode, live.go:432-453). `raw_text`: set when the
// parent is `<script>`/`<style>` so HText children emit verbatim — see SECURITY.
fn render_into_ctx<M>(
    node: &Html<M>,
    s: &mut String,
    select_value: Option<&str>,
    raw_text: bool,
    depth: usize,
) {
    // Bounded descent: a tree deeper than the cap is dropped here rather than
    // recursed into (see MAX_HTML_DEPTH). The parent has already emitted its
    // open tag and will emit its close tag, so the output stays well-formed.
    if depth >= MAX_HTML_DEPTH {
        return;
    }
    match node {
        // SECURITY: verbatim (un-escaped) text is reachable ONLY here, with
        // raw_text=true, which is set ONLY when the parent tag is the literal
        // "script"/"style" (see the child loop below). Std.Ui never produces a
        // script/style ELEMENT — its styling flows through data-sky-* markers
        // consumed server-side — so the "Std.Ui HTML-escapes everything" contract
        // is NOT weakened. This path is the documented Std.Html raw escape hatch
        // (`node "script" [] [text code]`): the author owns sanitisation of any
        // interpolated value, exactly as on the Go backend (live.go:421-438,
        // which likewise does NOT strip `</script>` here — matching it keeps the
        // byte-for-byte equivalence gate green).
        Html::HText(t) => {
            if raw_text {
                s.push_str(t);
            } else {
                s.push_str(&escape_text(t));
            }
        }
        Html::HRaw(r) => s.push_str(r),
        Html::HElement(tag, attrs, kids) => {
            // Html.doctype wraps children in a pseudo-element; emit a literal
            // `<!DOCTYPE html>` then the children directly (Go live.go:303-312).
            // Handled BEFORE the name gate because `!doctype-wrapper` contains a
            // `!`. The doctype string is a fixed literal and the wrapper's own
            // tag/attrs are never interpolated → not an injection vector; the
            // children keep full name/attr/text gating via render_into_ctx.
            if tag == "!doctype-wrapper" {
                s.push_str("<!DOCTYPE html>");
                for c in kids {
                    render_into_ctx(c, s, None, false, depth.saturating_add(1));
                }
                return;
            }
            // Injection guard: an unsafe tag name (spaces / `>` / `<` …) would
            // break out of the start tag. Drop the whole element — including its
            // subtree — rather than emit an attacker-controlled tag.
            if !is_safe_html_name(tag) {
                return;
            }
            s.push('<');
            s.push_str(tag);
            // Collect regular + bool attrs into (key, value) pairs, then sort
            // by key — Go's renderVNode emits from a map under sort.Strings, so
            // matching byte-for-byte requires the same alphabetical order. A
            // BoolAttr renders as `k="true"` (Go stores it as the string "true"
            // in n.Attrs), NOT a bare `k`.
            let mut pairs: Vec<(&str, String)> = vec![];
            let mut events: Vec<&str> = vec![];
            let mut sky_id: Option<&str> = None;
            // Multi-valued attribute merge — mirrors Go `HtmlToVNode`
            // (live.go ~L161-185). `class` is HTML's space-separated and
            // `style` HTML's semicolon-separated multi-valued attribute: an
            // element carrying BOTH a Std.Ui-computed inline `style` (from
            // padding/background/border attrs) AND a user
            // `Ui.htmlAttribute "style" "z-index: 5"` must emit ONE merged
            // `style="…computed…; z-index: 5"`, not two `style="…"`
            // attributes (the browser keeps only the first → the user's
            // declarations are silently dropped). The Rust attribute list
            // never went through Go's map accumulation, so we fold the merge
            // here at collection time. First-seen value keeps its position
            // (Std.Ui composes the computed style first); later values append
            // so a user style declared last can override. Every other key is
            // last-wins (two `href`/`value` ⇒ override, matching Go's map set).
            for a in attrs {
                match a {
                    Attribute::Attr(k, v) => {
                        let k = k.as_str();
                        if k == "sky-id" {
                            sky_id = Some(v);
                        }
                        match pairs.iter_mut().find(|(pk, _)| *pk == k) {
                            Some((_, existing)) if !existing.is_empty() && k == "class" => {
                                existing.push(' ');
                                existing.push_str(v);
                            }
                            Some((_, existing)) if !existing.is_empty() && k == "style" => {
                                // `; ` separator unless `existing` already ends
                                // with `;` (then a single space) — avoids `;;`.
                                if existing.ends_with(';') {
                                    existing.push(' ');
                                } else {
                                    existing.push_str("; ");
                                }
                                existing.push_str(v);
                            }
                            Some((_, existing)) => {
                                // Last-wins for every other key (and for an
                                // empty existing style/class — nothing to join).
                                existing.clear();
                                existing.push_str(v);
                            }
                            None => pairs.push((k, v.clone())),
                        }
                    }
                    Attribute::BoolAttr(k, true) => {
                        let k = k.as_str();
                        match pairs.iter_mut().find(|(pk, _)| *pk == k) {
                            Some((_, existing)) => {
                                existing.clear();
                                existing.push_str("true");
                            }
                            None => pairs.push((k, "true".to_string())),
                        }
                    }
                    Attribute::BoolAttr(_, false) | Attribute::NoAttr => {}
                    Attribute::EventAttr(e) => events.push(e.name()),
                }
            }
            // <textarea> and <select> have NO `value` attribute in the HTML spec
            // — a textarea's displayed value is its TEXT CONTENT; a select's is the
            // `selected` <option>. Emitting `<textarea value="…">` renders EMPTY in
            // every browser (and a server re-render would wipe the user's text). So
            // strip the `value` attr here for both, and (textarea only) splice it
            // back as escaped text content after the open tag when there are no
            // explicit children. Mirrors Go `renderVNode` (live.go).
            let mut textarea_value: Option<String> = None;
            if tag == "textarea" || tag == "select" {
                if let Some(pos) = pairs.iter().position(|(k, _)| *k == "value") {
                    let (_, v) = pairs.remove(pos);
                    textarea_value = Some(v);
                }
            }
            // <option selected> flip: when rendered as a direct child of a
            // <select> with a value, set `selected` on the value-matching option
            // and drop any stale `selected` otherwise (Go renderVNode, copy-
            // don't-mutate). We touch only the local `pairs`, never the caller's
            // tree (it is the diff baseline — mutating it would corrupt the next
            // diff). Added before the sort so byte order matches Go's map+sort.
            if tag == "option" {
                if let Some(sv) = select_value {
                    pairs.retain(|(k, _)| *k != "selected");
                    if pairs.iter().any(|(k, v)| *k == "value" && v == sv) {
                        pairs.push(("selected", "selected".to_string()));
                    }
                }
            }
            pairs.sort_by(|a, b| a.0.cmp(b.0));
            for (k, v) in &pairs {
                // Attr KEY is emitted unescaped; an unsafe key (`x onload=…`)
                // injects a new attribute, and a script-bearing key (`onerror`,
                // `srcdoc`) executes regardless of value-escaping. `SafeAttrName`
                // enforces both policies in one place; a key that fails is dropped.
                let safe_key = match SafeAttrName::parse(k) {
                    Some(n) => n,
                    None => continue,
                };
                s.push(' ');
                s.push_str(safe_key.as_str());
                s.push_str("=\"");
                s.push_str(&escape_attr(sanitise_url_attr(k, v)));
                s.push('"');
            }
            // Browser-client wire markers (live/client.js): the delegated
            // binder scans for `[sky-<event>]`, reads `data-sky-hid` for the
            // sky-id, and posts the `sky-<event>` value as `msg`. We make that
            // value the EVENT NAME so the server can tell click from submit
            // (the client doesn't send the event type otherwise) — the handler
            // resolves by (sky-id, event). `data-sky-on` is kept for parity
            // with Go's render.
            // Event names are emitted unescaped as both the `data-sky-on` value
            // and the `sky-<ev>` attribute key — an unsafe name injects markup.
            // Drop any that aren't valid HTML names.
            events.retain(|e| is_safe_html_name(e));
            if !events.is_empty() {
                s.push_str(" data-sky-on=\"");
                s.push_str(&events.join(" "));
                s.push('"');
                if let Some(id) = sky_id {
                    s.push_str(" data-sky-hid=\"");
                    s.push_str(&escape_attr(id));
                    s.push('"');
                }
                for ev in &events {
                    // File/image meta-events arrive already `sky-`-prefixed
                    // (`sky-image`/`sky-file`); the client's upload driver reads
                    // them via the `data-sky-ev-<name>` HTML5 data-attribute
                    // convention, while plain DOM events keep `sky-<name>` (Go
                    // live.go:395-405). Emitting `sky-sky-image` made the driver
                    // lookup miss, so uploads never fired. `ev` is already
                    // name-gated (events.retain above), so both keys are safe.
                    let key = if ev.starts_with("sky-") {
                        format!("data-sky-ev-{ev}")
                    } else {
                        format!("sky-{ev}")
                    };
                    s.push(' ');
                    s.push_str(&key);
                    s.push_str("=\"");
                    s.push_str(ev);
                    s.push('"');
                }
            }
            if VOID.contains(&tag.as_str()) {
                s.push_str(" />");
                return;
            }
            s.push('>');
            // Textarea value-as-content (Go parity): write the captured value as
            // escaped text content. Explicit children take precedence (a user who
            // wrote `textarea [] [ text "hi" ]` keeps that), matching Go's
            // `isTextarea && value != "" && len(children) == 0` guard.
            if tag == "textarea" {
                if let Some(v) = &textarea_value {
                    if !v.is_empty() && kids.is_empty() {
                        s.push_str(&escape_text(v));
                    }
                }
            }
            // <script>/<style> emit text children verbatim (rawBody); a
            // <select> threads its value to option children for the `selected`
            // flip. Both reset for deeper descendants (Go parity).
            let raw_body = tag == "script" || tag == "style";
            let child_select_value = if tag == "select" {
                textarea_value.as_deref().filter(|v| !v.is_empty())
            } else {
                None
            };
            if tag == "style" {
                // SECURITY (F7): every `<style>` body — from `Std.Html.styleNode`,
                // a hand-built `Html.node "style" [] [Html.raw css]`, or a
                // `Std.Css` stylesheet string — is close-tag-neutralised at THIS
                // sink before it reaches the DOM. `styleNode` also pre-strips at
                // construction (`html_style_node_`), so the body is gated twice
                // (belt and braces). Rendered into a scratch buffer with
                // raw_text=true (CSS is not HTML-decoded), then `strip_style_close`
                // removes any `</style` breakout. Asymmetry with `<script>` is
                // deliberate: `<style>` bodies are attacker-reachable via
                // `Std.Css` values, whereas `<script>` is the documented,
                // author-owned Go-parity raw escape hatch (see the HText comment
                // above) and is NOT stripped.
                let mut body = String::new();
                for c in kids {
                    render_into_ctx(c, &mut body, None, true, depth.saturating_add(1));
                }
                s.push_str(&css_safety::strip_style_close(&body));
            } else {
                for c in kids {
                    render_into_ctx(c, s, child_select_value, raw_body, depth.saturating_add(1));
                }
            }
            s.push_str("</");
            s.push_str(tag);
            s.push('>');
        }
    }
}

fn escape_text(t: &str) -> String {
    // `&` first (so the inserted `&xxx;` entities aren't re-escaped), then the
    // remaining metacharacters. The single quote `'` is escaped too — Go's
    // html.EscapeString covers the full `& ' < > "` set, and a missed `'` is an
    // attribute-breakout XSS hole when the result lands in a single-quoted attr.
    t.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&#39;")
}

fn escape_attr(t: &str) -> String {
    // `"` → `&#34;` (NOT `&quot;`) to match Go's html.EscapeString byte-for-byte
    // (GOROOT/src/html/escape.go uses the numeric entity). Both are valid HTML;
    // the numeric form is what the Go renderer emits, so equiv tests byte-compare.
    escape_text(t).replace('"', "&#34;")
}

/// True when `name` is safe to emit UNESCAPED as a tag name, attribute key, or
/// event name. Tag/attr/event names are NEVER escaped (an escaped `<` in a tag
/// position is meaningless), so a name carrying a structural metacharacter is a
/// direct injection: a tag `"div><script>…"` or an attr key `"x onmouseover=…"`
/// would break out of the element. Sky `Html.node` / `Html.attribute` take the
/// name as a `String`, so it can be attacker-derived. Accept only the characters
/// that appear in real HTML names — letters, digits, and `-_:.` — and reject
/// everything else (whitespace, `<>"'=/\``, control bytes, non-ASCII). An invalid
/// name causes the element / attribute / event marker to be DROPPED rather than
/// emitted, closing the XSS hole with no escaping ambiguity.
fn is_safe_html_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'))
}

/// Attribute NAMES that execute script (or embed a scripting context) regardless
/// of how the VALUE is escaped — entity-escaping the value is useless when the
/// value IS script (event handlers like `onerror`/`onclick`) or markup
/// (`srcdoc`). `Html.attribute` takes the name as a String that can be
/// attacker-derived, and `is_safe_html_name` permits these (all valid HTML-name
/// chars), so this denylist is the gate that stops them. Case-insensitive; no
/// legitimate HTML element or non-event attribute name begins with `on`.
fn is_dangerous_attr_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("on") || lower == "srcdoc"
}

/// A validated HTML **attribute** name (parse-don't-validate). The ONLY way to
/// build one runs the full attribute-name policy — `is_safe_html_name` (charset:
/// no structural-breakout bytes) AND `!is_dangerous_attr_name` (no script-bearing
/// names) — exactly once. Every emit sink consumes a `SafeAttrName` instead of a
/// raw `&str`, so a sink CANNOT forget a check: the divergence that left
/// `htmlAttrToString` exposed (guardian, 2026-06-27) becomes unrepresentable.
/// NB: element tag names and event-marker names are NOT attribute names — they
/// keep the plain `is_safe_html_name` gate.
struct SafeAttrName<'a>(&'a str);

impl<'a> SafeAttrName<'a> {
    fn parse(name: &'a str) -> Option<Self> {
        if is_safe_html_name(name) && !is_dangerous_attr_name(name) {
            Some(SafeAttrName(name))
        } else {
            None
        }
    }

    fn as_str(&self) -> &str {
        self.0
    }
}

/// URL-bearing attributes whose VALUE is fetched/navigated as a URL — a
/// `javascript:` / `vbscript:` / non-image `data:` value here is a script-execution
/// (XSS) vector that HTML entity-escaping does NOT neutralise. Case-insensitive.
fn is_url_attr(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "href"
            | "src"
            | "xlink:href"
            | "formaction"
            | "action"
            | "poster"
            | "cite"
            | "background"
            | "manifest"
            | "longdesc"
            // `<object data="…">` / `<embed src>` navigate the value as a document;
            // a `data:text/html,<script>` or `javascript:` value is script-exec.
            // Not media (is_media_url_attr("data")=false) so data: is blocked here.
            // (Exact name "data" only — `data-*` custom attrs do not match.)
            | "data"
    )
}

/// Media URL attributes where an inert *raster* `data:image/...` value is
/// legitimately useful (and cannot script). `data:` is allowed ONLY here, and
/// only for the raster types in `is_inert_data_image`. All other URL attributes
/// are navigational/document-context, where `data:` is a navigable document
/// (incl. scriptable SVG) and is blocked outright.
fn is_media_url_attr(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "src" | "poster" | "background"
    )
}

/// The URL scheme = the run before the first `:` that precedes any `/`, `?` or
/// `#`, with leading C0 controls/space and intra-scheme controls/whitespace
/// stripped + lowercased (mirrors the browser URL parser, so `java\tscript:` and
/// `\x01javascript:` are caught). `None` = relative / no-scheme URL (safe). Runs
/// BEFORE HTML entity-escaping, so the value is the raw scheme the browser sees.
fn url_scheme(value: &str) -> Option<String> {
    let mut scheme = String::new();
    for ch in value.chars() {
        match ch {
            ':' => return Some(scheme),
            '/' | '?' | '#' => return None,
            c if (c as u32) <= 0x20 => continue,
            c => scheme.push(c.to_ascii_lowercase()),
        }
    }
    None
}

/// True only for an inert *raster* `data:image/...` value. SVG (`image/svg+xml`)
/// is deliberately EXCLUDED — an SVG document can execute script — as is every
/// non-image / text/html data payload.
fn is_inert_data_image(value: &str) -> bool {
    let rest = match value.find(':').and_then(|i| value.get(i + 1..)) {
        Some(r) => r,
        None => return false,
    };
    let rest = rest.trim_start().to_ascii_lowercase();
    match rest.strip_prefix("image/") {
        Some(after) => {
            let subtype: String = after
                .chars()
                .take_while(|c| *c != ';' && *c != ',')
                .collect();
            matches!(
                subtype.as_str(),
                "png"
                    | "jpeg"
                    | "jpg"
                    | "gif"
                    | "webp"
                    | "bmp"
                    | "avif"
                    | "x-icon"
                    | "vnd.microsoft.icon"
            )
        }
        None => false,
    }
}

/// True if `value` is a dangerous URL for attribute `name`. `javascript:` /
/// `vbscript:` are always dangerous. `data:` is dangerous EXCEPT an inert raster
/// `data:image/...` on a media attribute (src/poster/background) — on a
/// navigational attribute (href/action/cite/…) every `data:` is blocked, since a
/// `data:image/svg+xml,<svg onload=…>` navigated there executes script.
fn is_dangerous_url(name: &str, value: &str) -> bool {
    let scheme = match url_scheme(value) {
        Some(s) => s,
        None => return false,
    };
    if matches!(scheme.as_str(), "javascript" | "vbscript") {
        return true;
    }
    if scheme == "data" {
        return !(is_media_url_attr(name) && is_inert_data_image(value));
    }
    false
}

/// For a URL-bearing attribute, neutralise a dangerous-scheme value to empty
/// (an inert URL) before it is escaped + emitted; pass any other value through.
/// Closes the `href="javascript:…"` / `src="data:text/html,…"` /
/// `href="data:image/svg+xml,…"` XSS class at the render sink (so every producer
/// — Std.Html, Std.Ui link/image — is covered).
fn sanitise_url_attr<'a>(name: &str, value: &'a str) -> &'a str {
    if is_url_attr(name) && is_dangerous_url(name, value) {
        ""
    } else {
        value
    }
}

/// Sanitise an attribute `(name, value)` pair destined for a Sky.Live SSE
/// patch that the browser applies via `setAttribute`. Returns the safe pair to
/// set, or `None` to DROP the attribute entirely.
///
/// SSE patches bypass `render_into_ctx` (the first-paint XSS gate), so this is
/// the gate for the patch path — it applies the SAME policy by construction:
/// the name must pass [`SafeAttrName`] (rejects `on*` handlers, `srcdoc`, and
/// structural-breakout charsets) and a URL-bearing value is scheme-checked via
/// [`sanitise_url_attr`]. No HTML-entity escaping is applied: `setAttribute`
/// sets a raw DOM attribute value and does NOT re-parse it as HTML, so escaping
/// would corrupt legitimate values (and is unnecessary for safety). A name that
/// fails policy is dropped, which also covers the removal sentinel (an empty
/// value on a dangerous name must never reach `setAttribute`).
///
/// `live`-gated: its sole consumer is `live/diff.rs` (the SSE patch builder).
#[cfg(feature = "live")]
pub(crate) fn safe_patch_attr<'a>(name: &'a str, value: &'a str) -> Option<(&'a str, &'a str)> {
    let SafeAttrName(safe_name) = SafeAttrName::parse(name)?;
    Some((safe_name, sanitise_url_attr(name, value)))
}

/// Stamp every HElement (not HText/HRaw) with a stable `sky-id` attribute derived
/// from its path. Idempotent: an existing sky-id is overwritten with the same
/// value. HText/HRaw nodes are unaddressable (Go parity).
///
/// Each non-root segment is `{path}_{idx}_{tag}[:{key}]` — the embedded tag means
/// two structurally different subtrees never share an id at the same positional
/// depth, and the optional `:{key}` disambiguator (from an explicit `sky-key`
/// attribute, or implicit from `name` on form-bearing tags) lets keyed list items
/// and named form fields keep identity across reorder. Mirrors Go `assignSkyIDs`
/// / `skyIDKey` (`runtime-go/rt/live.go`).
pub fn assign_sky_ids<M>(node: &mut Html<M>, path: &str) {
    assign_sky_ids_depth(node, path, 0)
}

// Same bounded-descent rationale as render_into_ctx (see MAX_HTML_DEPTH): the
// stamper recurses once per nesting level, so an attacker-influenced deep tree
// would overflow the stack. Stop descending at the cap. Kept in step with the
// renderer's cap so a node the renderer drops is also left unstamped.
fn assign_sky_ids_depth<M>(node: &mut Html<M>, path: &str, depth: usize) {
    if depth >= MAX_HTML_DEPTH {
        return;
    }
    if let Html::HElement(_tag, attrs, kids) = node {
        set_attr(attrs, "sky-id", path);
        let mut idx = 0usize;
        for child in kids.iter_mut() {
            if let Html::HElement(ctag, cattrs, _) = child {
                let mut seg = format!("{path}_{idx}_{ctag}");
                if let Some(key) = sky_id_key(ctag, cattrs) {
                    seg.push(':');
                    seg.push_str(&key);
                }
                idx += 1;
                assign_sky_ids_depth(child, &seg, depth.saturating_add(1));
            }
        }
    }
}

/// Stable disambiguator for an element, or `None`. Priority: an explicit
/// `sky-key` attribute (set by `Html.keyed`), then `name` on form-bearing tags.
/// Any matched value is sanitised so it can't corrupt the sky-id grammar.
/// Mirrors Go `skyIDKey`.
fn sky_id_key<M>(tag: &str, attrs: &[Attribute<M>]) -> Option<String> {
    if let Some(k) = attr_value(attrs, "sky-key") {
        if !k.is_empty() {
            return Some(sanitise_sky_id_key(k));
        }
    }
    if matches!(
        tag,
        "input" | "textarea" | "select" | "form" | "button" | "fieldset"
    ) {
        if let Some(k) = attr_value(attrs, "name") {
            if !k.is_empty() {
                return Some(sanitise_sky_id_key(k));
            }
        }
    }
    None
}

fn attr_value<'a, M>(attrs: &'a [Attribute<M>], key: &str) -> Option<&'a str> {
    attrs.iter().find_map(|a| match a {
        Attribute::Attr(k, v) if k == key => Some(v.as_str()),
        _ => None,
    })
}

/// Replace anything outside `[A-Za-z0-9_-]` with `_`. Prevents the key from
/// breaking sky-id parsing, CSS selector escaping, or HTML attribute quoting.
/// Mirrors Go `sanitiseSkyIDKey`.
fn sanitise_sky_id_key(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn set_attr<M>(attrs: &mut Vec<Attribute<M>>, key: &str, val: &str) {
    for a in attrs.iter_mut() {
        if let Attribute::Attr(k, v) = a {
            if k == key {
                *v = val.to_string();
                return;
            }
        }
    }
    attrs.push(Attribute::Attr(key.to_string(), val.to_string()));
}

// --- Std.Html kernel wrappers (`Ffi.callPure "htmlXxx"`) ---
// These match the kernel names used in sky-stdlib Std.Html.sky — the Sky-side
// helpers (render, escapeHtml, escapeAttr, attrToString) route here on the Rust
// backend. The codegen converts "htmlRender" → `html_render_()`, etc. Kept in
// this standalone module (not under live/) so a non-Live Std.Html / Std.Ui app
// renders via Html.toString without pulling the Sky.Live server machinery.

/// `Ffi.callPure "htmlRender"` — render an Html tree to an HTML string.
pub fn html_render_<M>(node: Html<M>) -> String {
    render_html(&node)
}

/// `Ffi.callPure "htmlEscapeText"` — HTML-escape a string for text content.
/// Routes through the same escaper as render so the escape set (`& ' < > "`,
/// matching Go's html.EscapeString for the text subset) can never drift.
pub fn html_escape_text_(s: String) -> String {
    escape_text(&s)
}

/// `Ffi.callPure "htmlEscapeAttr"` — escape a string for use in a quoted
/// attribute. Shares render's attr escaper, so a value placed in a single- or
/// double-quoted attribute is escaped identically (no attribute-breakout hole).
pub fn html_escape_attr_(s: String) -> String {
    escape_attr(&s)
}

/// `Ffi.callPure "htmlAttrToString"` — serialise a single Attribute to its key="value" form.
pub fn html_attr_to_string_<M>(attr: Attribute<M>) -> String {
    // Gate the attribute KEY through `SafeAttrName` — the SAME single policy the
    // render_into sink uses (charset + no script-bearing names). The key is
    // emitted UNESCAPED, so a hostile key such as `x onload=alert(1)` or a live
    // `onerror`/`srcdoc` (reachable via Std.Html.Attributes.attribute →
    // Std.Html.attrToString) would inject markup or execute script that
    // value-escaping cannot stop. An unvetted name drops the whole attribute.
    // Event-marker names are not attribute names → keep `is_safe_html_name`.
    match attr {
        Attribute::Attr(k, v) if SafeAttrName::parse(&k).is_some() => {
            let v = sanitise_url_attr(&k, &v).to_string();
            format!("{}=\"{}\"", k, html_escape_attr_(v))
        }
        Attribute::BoolAttr(k, true) if SafeAttrName::parse(&k).is_some() => k,
        Attribute::EventAttr(e) if is_safe_html_name(e.name()) => {
            format!("data-sky-on=\"{}\"", e.name())
        }
        // unsafe key / event name, false bool attr, or NoAttr → emit nothing
        Attribute::Attr(..)
        | Attribute::BoolAttr(..)
        | Attribute::EventAttr(..)
        | Attribute::NoAttr => String::new(),
    }
}

// ─── SkyStringify for the Html runtime types ────────────────────────────────
// Same rationale as the Std.Ui impls in ui/element.rs: a codegen-emitted
// `sky_show` recurses into every field, so an Html/Attribute/Event a generated
// type can hold must impl the trait (else E0599). No Go `%v` analogue worth
// matching; a stable type-tag placeholder is total and never recurses into `M`.
impl<M> crate::sky_runtime::stringify::SkyStringify for Html<M> {
    fn sky_show(&self) -> String {
        "<html>".to_string()
    }
}
impl<M> crate::sky_runtime::stringify::SkyStringify for Attribute<M> {
    fn sky_show(&self) -> String {
        "<html-attribute>".to_string()
    }
}
impl<M> crate::sky_runtime::stringify::SkyStringify for Event<M> {
    fn sky_show(&self) -> String {
        "<event>".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone, Debug, PartialEq)]
    enum Msg {
        Inc,
    }

    #[test]
    fn render_escapes_and_emits_attrs_events_void() {
        let t: Html<()> = Html::HElement(
            "div".into(),
            vec![Attribute::Attr("class".into(), "x".into())],
            vec![
                Html::HElement(
                    "input".into(),
                    vec![
                        Attribute::Attr("value".into(), "a<b".into()),
                        Attribute::BoolAttr("disabled".into(), true),
                    ],
                    vec![],
                ),
                Html::HText("1 < 2".into()),
                Html::HRaw("<b>ok</b>".into()),
            ],
        );
        let mut t = t;
        assign_sky_ids(&mut t, "r");
        let s = render_html(&t);
        assert!(s.contains(r#"<div class="x" sky-id="r">"#), "{s}");
        // Attrs are sorted alphabetically; BoolAttr renders as `k="true"`; void
        // elements self-close — all to match Go renderVNode.
        assert!(
            s.contains(r#"<input disabled="true" sky-id="r_0_input" value="a&lt;b" />"#),
            "{s}"
        );
        assert!(s.contains("1 &lt; 2"));
        assert!(s.contains("<b>ok</b>"));
        assert!(s.contains("</div>"));
    }

    // SECURITY regression: event-handler + srcdoc attribute NAMES execute script
    // regardless of value-escaping. `Html.attribute` names can be attacker-derived,
    // so the render sink must DROP them (is_safe_html_name alone permits them).
    #[test]
    fn render_drops_event_handler_and_srcdoc_attrs() {
        let t: Html<()> = Html::HElement(
            "img".into(),
            vec![
                Attribute::Attr("onerror".into(), "alert(1)".into()),
                Attribute::Attr("OnClick".into(), "x".into()),
                Attribute::Attr("srcdoc".into(), "<script>1</script>".into()),
                Attribute::Attr("alt".into(), "ok".into()),
            ],
            vec![],
        );
        let mut t = t;
        assign_sky_ids(&mut t, "r");
        let s = render_html(&t);
        let low = s.to_ascii_lowercase();
        assert!(!low.contains("onerror"), "{s}");
        assert!(!low.contains("onclick"), "{s}");
        assert!(!low.contains("srcdoc"), "{s}");
        assert!(s.contains(r#"alt="ok""#), "{s}");
        assert!(is_dangerous_attr_name("onmouseover") && is_dangerous_attr_name("SRCDOC"));
        // Conservative by design: any name starting with `on` is dropped (no
        // legitimate non-event HTML attribute begins with `on`; custom data goes
        // through `data-*`).
        assert!(is_dangerous_attr_name("onfoo"));
        assert!(!is_dangerous_attr_name("class") && !is_dangerous_attr_name("href"));
        // SafeAttrName is the single policy both sinks share.
        assert!(SafeAttrName::parse("class").is_some());
        assert!(SafeAttrName::parse("onload").is_none() && SafeAttrName::parse("srcdoc").is_none());
    }

    // SECURITY regression (guardian 2026-06-27): the sibling sink
    // `Std.Html.attrToString` must drop the SAME script-bearing names as
    // render_into — both now route through SafeAttrName.
    #[test]
    fn attr_to_string_drops_event_handler_and_srcdoc() {
        let onload: String =
            html_attr_to_string_(Attribute::<()>::Attr("onload".into(), "alert(1)".into()));
        let srcdoc: String = html_attr_to_string_(Attribute::<()>::Attr(
            "srcdoc".into(),
            "<script>1</script>".into(),
        ));
        let onbool: String =
            html_attr_to_string_(Attribute::<()>::BoolAttr("onclick".into(), true));
        let ok: String = html_attr_to_string_(Attribute::<()>::Attr("alt".into(), "x".into()));
        assert_eq!(onload, "");
        assert_eq!(srcdoc, "");
        assert_eq!(onbool, "");
        assert_eq!(ok, r#"alt="x""#);
    }

    #[test]
    fn style_attrs_merge_into_one_attribute() {
        // Std.Ui composes a computed inline `style` from layout attrs
        // (padding/background/border) AND a user `Ui.htmlAttribute "style" V`
        // adds another. The renderer MUST merge both into ONE `style="…"` —
        // emitting two `style="…"` attrs makes the browser keep only the first,
        // silently dropping the user's declarations. Go parity (live.go ~L176).
        let computed = "padding: 8px; background-color: rgb(255, 102, 0)";
        let t: Html<()> = Html::HElement(
            "div".into(),
            vec![
                Attribute::Attr("style".into(), computed.into()),
                Attribute::Attr("style".into(), "z-index: 5".into()),
            ],
            vec![],
        );
        let s = render_html(&t);
        // Exactly ONE style attribute …
        assert_eq!(
            s.matches("style=\"").count(),
            1,
            "expected a single style attr: {s}"
        );
        // … containing BOTH the computed declarations AND the user's z-index,
        // joined `; ` (computed has no trailing `;`), computed first/user last.
        assert!(
            s.contains(r#"style="padding: 8px; background-color: rgb(255, 102, 0); z-index: 5""#),
            "merged style must keep both declarations: {s}"
        );
    }

    #[test]
    fn style_merge_handles_trailing_semicolon_and_user_only() {
        // Trailing `;` on the computed style ⇒ single-space join (no `;;`).
        let t: Html<()> = Html::HElement(
            "div".into(),
            vec![
                Attribute::Attr("style".into(), "color: red;".into()),
                Attribute::Attr("style".into(), "z-index: 5".into()),
            ],
            vec![],
        );
        let s = render_html(&t);
        assert!(s.contains(r#"style="color: red; z-index: 5""#), "{s}");
        assert!(!s.contains(";;"), "no double semicolon: {s}");

        // Only a user style (no computed) ⇒ emitted verbatim, single attr.
        let only: Html<()> = Html::HElement(
            "div".into(),
            vec![Attribute::Attr("style".into(), "z-index: 5".into())],
            vec![],
        );
        let s2 = render_html(&only);
        assert_eq!(s2.matches("style=\"").count(), 1, "{s2}");
        assert!(s2.contains(r#"style="z-index: 5""#), "{s2}");

        // `class` merges space-separated (HTML multi-valued parity).
        let cls: Html<()> = Html::HElement(
            "div".into(),
            vec![
                Attribute::Attr("class".into(), "a".into()),
                Attribute::Attr("class".into(), "b".into()),
            ],
            vec![],
        );
        let s3 = render_html(&cls);
        assert_eq!(s3.matches("class=\"").count(), 1, "{s3}");
        assert!(s3.contains(r#"class="a b""#), "{s3}");
    }

    #[test]
    fn textarea_value_renders_as_content_not_attr() {
        // <textarea value="…"> renders EMPTY in browsers — the value must become
        // the text content. Go parity (live.go renderVNode). Std.Ui.Input.multiline
        // sets a `value` attr; the renderer must move it into the body.
        let t: Html<()> = Html::HElement(
            "textarea".into(),
            vec![Attribute::Attr("value".into(), "fill the column".into())],
            vec![],
        );
        let s = render_html(&t);
        assert!(
            !s.contains("value=\""),
            "value attr must be stripped from textarea: {s}"
        );
        assert!(
            s.contains(">fill the column</textarea>"),
            "value must be content: {s}"
        );
    }

    #[test]
    fn textarea_value_is_escaped_and_select_strips_value() {
        // textarea content is HTML-escaped (XSS); explicit children win over the
        // attr-derived value; <select> strips a redundant `value` attr (no content).
        let ta: Html<()> = Html::HElement(
            "textarea".into(),
            vec![Attribute::Attr("value".into(), "a<b'c".into())],
            vec![],
        );
        assert!(
            render_html(&ta).contains(">a&lt;b&#39;c</textarea>"),
            "{}",
            render_html(&ta)
        );

        let ta_kids: Html<()> = Html::HElement(
            "textarea".into(),
            vec![Attribute::Attr("value".into(), "ignored".into())],
            vec![Html::HText("explicit".into())],
        );
        let s = render_html(&ta_kids);
        assert!(
            s.contains(">explicit</textarea>") && !s.contains("ignored"),
            "{s}"
        );

        let sel: Html<()> = Html::HElement(
            "select".into(),
            vec![Attribute::Attr("value".into(), "x".into())],
            vec![],
        );
        assert!(
            !render_html(&sel).contains("value="),
            "select strips value: {}",
            render_html(&sel)
        );
    }

    #[test]
    fn single_quote_is_escaped_everywhere() {
        // A `'` in attr/text/kernel output must become `&#39;` so a value placed
        // in a single-quoted attribute can't break out (XSS) and the escape set
        // matches Go's html.EscapeString.
        assert_eq!(escape_text("it's <b>"), "it&#39;s &lt;b&gt;");
        assert_eq!(escape_attr("a'\"b"), "a&#39;&#34;b");
        assert_eq!(html_escape_text_("x'y".to_string()), "x&#39;y");
        assert_eq!(html_escape_attr_("x'y".to_string()), "x&#39;y");
        // Round-trips through render on a real attribute value.
        let mut t: Html<()> = Html::HElement(
            "a".into(),
            vec![Attribute::Attr("href".into(), "/x?q='z".into())],
            vec![],
        );
        assign_sky_ids(&mut t, "r");
        let s = render_html(&t);
        assert!(s.contains("href=\"/x?q=&#39;z\""), "{s}");
    }

    #[test]
    fn render_emits_data_event_attr() {
        let t: Html<()> = Html::HElement(
            "button".into(),
            vec![Attribute::EventAttr(Event::OnMsg("click".into(), ()))],
            vec![],
        );
        let mut t = t;
        assign_sky_ids(&mut t, "r");
        let s = render_html(&t);
        assert!(s.contains(r#"data-sky-on="click""#), "{s}");
    }

    #[test]
    fn sky_ids_are_stable_and_pathed() {
        let mut t: Html<()> = Html::HElement(
            "div".into(),
            vec![],
            vec![
                Html::HElement("span".into(), vec![], vec![Html::HText("a".into())]),
                Html::HElement("span".into(), vec![], vec![]),
            ],
        );
        assign_sky_ids(&mut t, "r");
        let ids = collect_ids(&t);
        assert_eq!(ids, vec!["r", "r_0_span", "r_1_span"]);
        let mut t2 = t.clone();
        assign_sky_ids(&mut t2, "r");
        assert_eq!(collect_ids(&t2), ids);
    }

    fn collect_ids<M>(n: &Html<M>) -> Vec<String> {
        let mut out = vec![];
        fn go<M>(n: &Html<M>, out: &mut Vec<String>) {
            if let Html::HElement(_, attrs, kids) = n {
                for a in attrs {
                    if let Attribute::Attr(k, v) = a {
                        if k == "sky-id" {
                            out.push(v.clone());
                        }
                    }
                }
                for c in kids {
                    go(c, out);
                }
            }
        }
        go(n, &mut out);
        out
    }

    #[test]
    fn keyed_items_keep_id_across_reorder() {
        // Two keyed <li> swapped: each keeps its `:{key}` id so the diff can
        // target the moved element instead of replacing the whole list.
        let li = |k: &str| -> Html<()> {
            Html::HElement(
                "li".into(),
                vec![Attribute::Attr("sky-key".into(), k.into())],
                vec![Html::HText(k.into())],
            )
        };
        let mut a: Html<()> = Html::HElement("ul".into(), vec![], vec![li("alpha"), li("beta")]);
        let mut b: Html<()> = Html::HElement("ul".into(), vec![], vec![li("beta"), li("alpha")]);
        assign_sky_ids(&mut a, "r");
        assign_sky_ids(&mut b, "r");
        let ids_a = collect_ids(&a);
        let ids_b = collect_ids(&b);
        // alpha keeps the same id in both renders even though its position moved.
        assert!(ids_a.contains(&"r_0_li:alpha".to_string()), "{ids_a:?}");
        assert!(ids_b.contains(&"r_1_li:alpha".to_string()), "{ids_b:?}");
        // The key disambiguator is present, sanitised.
        assert!(
            ids_a
                .iter()
                .all(|s| s.contains(":alpha") || s.contains(":beta") || s == "r")
        );
    }

    #[test]
    fn name_on_form_tag_becomes_implicit_key() {
        let mut t: Html<()> = Html::HElement(
            "form".into(),
            vec![],
            vec![Html::HElement(
                "input".into(),
                vec![Attribute::Attr("name".into(), "email".into())],
                vec![],
            )],
        );
        assign_sky_ids(&mut t, "r");
        assert!(collect_ids(&t).contains(&"r_0_input:email".to_string()));
    }

    #[test]
    fn sky_key_value_is_sanitised() {
        assert_eq!(sanitise_sky_id_key("a/b c.d"), "a_b_c_d");
        assert_eq!(sanitise_sky_id_key("keep-_OK9"), "keep-_OK9");
    }

    #[test]
    fn html_tree_constructs() {
        let t: Html<Msg> = Html::HElement(
            "button".into(),
            vec![Attribute::EventAttr(Event::OnMsg("click".into(), Msg::Inc))],
            vec![Html::HText("+".into())],
        );
        match t {
            Html::HElement(tag, attrs, kids) => {
                assert_eq!(tag, "button");
                assert_eq!(attrs.len(), 1);
                assert_eq!(kids.len(), 1);
            }
            _ => panic!("expected element"),
        }

        // Clone + PartialEq round-trip on an attribute holding a closure.
        let attr: Attribute<Msg> = Attribute::EventAttr(Event::OnMsg("click".into(), Msg::Inc));
        assert_eq!(attr, attr.clone());
        // Debug prints the variant name + event name, not a numeric discriminant.
        let dbg = format!("{:?}", attr);
        assert!(
            dbg.contains("OnMsg") && dbg.contains("click"),
            "debug was: {dbg}"
        );
    }

    // ── F7: `<style>`-body injection regressions (design §Q6 #6/#8/#9) ─────────
    use crate::sky_runtime::ui::helpers::html_style_node_;

    /// Count occurrences of `</style` (case-insensitive) in `out`. A safe
    /// `<style>` element has EXACTLY ONE — its own closing tag. Any additional
    /// occurrence means the CSS body carried a `</style` that would close the
    /// raw-text element early and let following `<script>` parse as markup.
    fn style_close_count(out: &str) -> usize {
        out.to_ascii_lowercase().matches("</style").count()
    }

    #[test]
    fn style_node_strips_close_tag_breakout() {
        // #6 — a `</style><script>` in the CSS body cannot break out. After
        // stripping, the only `</style` left is the element's own closing tag,
        // so the injected `<script>` is trapped INSIDE the raw-text <style>
        // element (inert — the HTML parser does not parse elements inside
        // <style>), never parsed as a live script element.
        let node = html_style_node_::<()>(
            vec![],
            "body{color:red}</style><script>alert(1)</script>".into(),
        );
        let out = render_html(&node);
        assert_eq!(style_close_count(&out), 1, "early </style break-out: {out}");
        assert!(
            !out.to_ascii_lowercase().contains("</style><script"),
            "{out}"
        );
    }

    #[test]
    fn hand_built_style_element_is_also_stripped() {
        // Defence-in-depth — a hand-built `<style>` (NOT via styleNode) with an
        // HRaw body is stripped at the render sink too.
        let node: Html<()> = Html::HElement(
            "style".into(),
            vec![],
            vec![Html::HRaw("x{}</StYlE ><script>".into())],
        );
        let out = render_html(&node);
        assert_eq!(style_close_count(&out), 1, "{out}");
        assert!(
            !out.to_ascii_lowercase().contains("</style><script"),
            "{out}"
        );
    }

    #[test]
    fn style_node_defeats_reconstruction_trick() {
        // #8 — the `</sty</stylele` reconstruction trick is defeated (fixpoint):
        // no residual `</style` survives in the body, only the closing tag.
        let node = html_style_node_::<()>(vec![], "a{}</sty</stylele>".into());
        let out = render_html(&node);
        assert_eq!(style_close_count(&out), 1, "{out}");
    }

    #[test]
    fn script_element_body_stays_verbatim() {
        // Asymmetry contract: `<script>` is the documented author-owned raw
        // escape hatch (Go parity) and is NOT stripped — only `<style>` is.
        let node: Html<()> = Html::HElement(
            "script".into(),
            vec![],
            vec![Html::HRaw("if (1 < 2) { x(); }".into())],
        );
        assert!(render_html(&node).contains("if (1 < 2)"));
    }

    // ── Snapshot port of ../sky fixture `69-html-render-parity` (Go parity) ────
    #[test]
    fn fixture69_render_parity() {
        // <select value="b"> flips `selected` onto the matching <option>, NOT the
        // first; <script> text child emits verbatim; ordinary text stays
        // entity-escaped; Html.doctype → literal <!DOCTYPE html>.
        let tree: Html<()> = Html::HElement(
            "!doctype-wrapper".into(),
            vec![],
            vec![Html::HElement(
                "div".into(),
                vec![],
                vec![
                    Html::HElement(
                        "select".into(),
                        vec![Attribute::Attr("value".into(), "b".into())],
                        vec![
                            Html::HElement(
                                "option".into(),
                                vec![Attribute::Attr("value".into(), "a".into())],
                                vec![Html::HText("A".into())],
                            ),
                            Html::HElement(
                                "option".into(),
                                vec![Attribute::Attr("value".into(), "b".into())],
                                vec![Html::HText("B".into())],
                            ),
                        ],
                    ),
                    Html::HElement(
                        "script".into(),
                        vec![],
                        vec![Html::HText("if (1 < 2) { x = '&'; }".into())],
                    ),
                    Html::HElement("div".into(), vec![], vec![Html::HText("<b>raw</b>".into())]),
                ],
            )],
        );
        let out = render_html(&tree);
        assert!(out.starts_with("<!DOCTYPE html>"), "{out}");
        // selected flips onto the `b` option, not `a`.
        assert!(
            out.contains(r#"<option selected="selected" value="b">"#),
            "{out}"
        );
        assert!(!out.contains(r#"value="a" selected"#), "{out}");
        // <script> body verbatim (inline JS not entity-escaped).
        assert!(out.contains("if (1 < 2) { x = '&'; }"), "{out}");
        // ordinary text still entity-escaped.
        assert!(out.contains("&lt;b&gt;raw&lt;/b&gt;"), "{out}");
    }

    // ── #46: attribute-builder injection regressions (design §Q6 #1–5, #10) ────

    #[test]
    fn attr_value_quote_breakout_is_escaped() {
        // #1 — a `"` in an attribute value is entity-escaped, no live handler.
        let n: Html<()> = Html::HElement(
            "div".into(),
            vec![Attribute::Attr(
                "title".into(),
                "x\" onmouseover=\"alert(1)".into(),
            )],
            vec![],
        );
        let out = render_html(&n);
        assert!(out.contains("onmouseover=&#34;"), "{out}"); // escaped, inert
        assert!(!out.contains("onmouseover=\""), "{out}"); // no live handler
    }

    #[test]
    fn javascript_href_is_neutralised() {
        // #2 — a `javascript:` scheme on href is neutralised to empty.
        let n: Html<()> = Html::HElement(
            "a".into(),
            vec![Attribute::Attr("href".into(), "javascript:alert(1)".into())],
            vec![],
        );
        let out = render_html(&n);
        assert!(out.contains("href=\"\""), "{out}");
        assert!(!out.to_ascii_lowercase().contains("javascript:"), "{out}");
    }

    #[test]
    fn data_uri_src_policy() {
        // #3 — scriptable `data:text/html` neutralised; inert raster passes.
        let bad: Html<()> = Html::HElement(
            "img".into(),
            vec![Attribute::Attr(
                "src".into(),
                "data:text/html,<script>1</script>".into(),
            )],
            vec![],
        );
        assert!(render_html(&bad).contains("src=\"\""));
        let ok: Html<()> = Html::HElement(
            "img".into(),
            vec![Attribute::Attr(
                "src".into(),
                "data:image/png;base64,iVBOR".into(),
            )],
            vec![],
        );
        assert!(render_html(&ok).contains("data:image/png;base64,iVBOR"));
    }

    #[test]
    fn generic_attribute_drops_event_and_srcdoc_names() {
        // #4 — the generic `attribute k v` setter cannot register `on*`/`srcdoc`.
        for k in ["onclick", "OnClick", "onfoo", "srcdoc"] {
            let n: Html<()> = Html::HElement(
                "div".into(),
                vec![Attribute::Attr(k.into(), "x".into())],
                vec![],
            );
            let out = render_html(&n).to_ascii_lowercase();
            assert!(
                !out.contains(&format!("{}=", k.to_ascii_lowercase())),
                "{out}"
            );
        }
    }

    #[test]
    fn unsafe_tag_name_drops_element() {
        // #5 — a start-tag breakout in the tag name drops the whole element.
        let n: Html<()> = Html::HElement("div><script>".into(), vec![], vec![]);
        assert!(!render_html(&n).contains("<script"));
    }

    #[cfg(feature = "live")]
    #[test]
    fn sse_patch_shares_the_policy() {
        // #10 — SSE patch attributes route through the same name+scheme gate.
        assert!(safe_patch_attr("onclick", "x").is_none());
        let (_, v) = safe_patch_attr("href", "javascript:alert(1)").unwrap();
        assert!(!v.to_ascii_lowercase().contains("javascript:"));
    }
}

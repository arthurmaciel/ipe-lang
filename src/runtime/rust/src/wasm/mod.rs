//! Browser-WASM TEA sink — mount, patch-apply, delegated events, scheduler.
//!
//! The same data path as the Ipe.Live SSE wire, with the transport swapped
//! for in-process calls: `view` → `Html<M>` (`ui/render.rs`) →
//! `dom::diff` → `Vec<Patch>` → typed `web-sys` DOM mutation. One diff
//! algorithm, two consumers (the SSE client and this sink), so behaviour
//! stays byte-parity with the server-driven path.
//!
//! Mount applies the sanitiser-gated `render_html` output through ONE
//! renderer (the same string the SSE first paint serves), so the DOM the
//! diff subsequently patches is byte-identical to what `render_html`
//! produces — a hand-rolled node builder would be a second render
//! implementation that could drift from the first. `Patch.html` subtree
//! replaces flow through the same gate.
//!
//! Casting discipline: every JS→web-sys crossing uses the CHECKED
//! `dyn_into`/`dyn_ref` (the DOM is mutable by extensions and devtools; an
//! unchecked cast on a foreign node is UB in the glue). A failed cast routes
//! to the classified console-error path, never a trap.
//!
//! Not yet supported (fail loud, never silent): `Cmd.publish` (in-tab
//! broker), `Sub.every`/`Sub` sources (timer bridge), routed Live apps.
//! Each logs a classified `console.error` when reached.

#![allow(clippy::type_complexity)] // TEA fn-quadruples are inherent here

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::core::{IpeResult, IpeTask};
use crate::dom::{HandlerIndex, LiveReq, build_index, diff::Patch, diff::diff};
use crate::html::{FormData, Html, assign_sky_ids, render_html};
use crate::tea::{IpeCmd, IpeSub};

/// Root sky-id path — matches the Live/Webview renderers so a future
/// SSR-adopt path sees identical ids.
const ROOT_SKY_ID: &str = "r";

/// Classified fatal-diagnostic prefix (mirrors the native panic classifier's
/// taxonomy labels; the instance may be unusable after one of these).
fn console_fatal(class: &str, detail: &str) {
    web_sys::console::error_1(&JsValue::from_str(&format!("[ipe-wasm:{class}] {detail}")));
}

fn console_warn(detail: &str) {
    web_sys::console::warn_1(&JsValue::from_str(&format!("[ipe-wasm] {detail}")));
}

/// Install `console_error_panic_hook` so a residual trap (e.g. stack
/// exhaustion from a non-TCO fold — the documented reachable residual)
/// dies with a classified `console.error`, never a silent white screen.
pub fn install_panic_hook() {
    console_error_panic_hook::set_once();
}

/// Drive the program's entry task on the browser microtask queue.
/// The `Err` arm logs a classified diagnostic (there is no process to exit).
pub fn run_start<E: std::fmt::Debug + 'static>(task: IpeTask<E, ()>) {
    wasm_bindgen_futures::spawn_local(async move {
        if let IpeResult::Err(e) = task.await {
            console_fatal("EntryFailed", &format!("{e:?}"));
        }
    });
}

/// `Live.app` compiled for the browser: mount into `document.body`, then run
/// the update→diff→patch loop locally. Session stores do not exist client-side;
/// `init` receives a `LiveReq` synthesised from `location` + `document.cookie`.
pub fn wasm_app<E, Model, Msg, FInit, FUpdate, FView, FSubs>(
    init: FInit,
    update: FUpdate,
    view: FView,
    subscriptions: FSubs,
) -> IpeTask<E, ()>
where
    E: From<String> + 'static,
    Model: Clone + 'static,
    Msg: Clone + 'static,
    FInit: Fn(LiveReq) -> (Model, IpeCmd<Msg>) + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, IpeCmd<Msg>) + 'static,
    FView: Fn(Model) -> Html<Msg> + 'static,
    FSubs: Fn(Model) -> IpeSub<Msg> + 'static,
{
    Box::pin(async move {
        match mount_app(init, update, view, subscriptions) {
            Ok(()) => IpeResult::Ok(()),
            Err(e) => {
                console_fatal("MountFailed", &e);
                IpeResult::Err(E::from(e))
            }
        }
    })
}

/// The retained application state driving one mounted TEA app.
struct App<Model, Msg> {
    model: RefCell<Model>,
    tree: RefCell<Html<Msg>>,
    index: RefCell<HandlerIndex<Msg>>,
    queue: RefCell<VecDeque<Msg>>,
    frame_scheduled: Cell<bool>,
    update: Box<dyn Fn(Msg, Model) -> (Model, IpeCmd<Msg>)>,
    view: Box<dyn Fn(Model) -> Html<Msg>>,
    #[allow(dead_code)] // wired to the timer bridge when Sub substitutes land
    subscriptions: Box<dyn Fn(Model) -> IpeSub<Msg>>,
}

fn mount_app<Model, Msg, FInit, FUpdate, FView, FSubs>(
    init: FInit,
    update: FUpdate,
    view: FView,
    subscriptions: FSubs,
) -> Result<(), String>
where
    Model: Clone + 'static,
    Msg: Clone + 'static,
    FInit: Fn(LiveReq) -> (Model, IpeCmd<Msg>) + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, IpeCmd<Msg>) + 'static,
    FView: Fn(Model) -> Html<Msg> + 'static,
    FSubs: Fn(Model) -> IpeSub<Msg> + 'static,
{
    let document = document()?;
    let body: web_sys::HtmlElement = document.body().ok_or("document has no <body>")?;

    let (model, cmd0) = init(synthesize_req(&document)?);

    let mut tree = view(model.clone());
    assign_sky_ids(&mut tree, ROOT_SKY_ID);
    // One renderer: the DOM starts as exactly the bytes `render_html`
    // produces, so every later `diff` runs against known ground truth.
    body.set_inner_html(&render_html(&tree));

    let index = build_index(&tree);
    let app = Rc::new(App {
        model: RefCell::new(model),
        tree: RefCell::new(tree),
        index: RefCell::new(index),
        queue: RefCell::new(VecDeque::new()),
        frame_scheduled: Cell::new(false),
        update: Box::new(update),
        view: Box::new(view),
        subscriptions: Box::new(subscriptions),
    });

    attach_delegated_listeners(&body, &app)?;
    run_cmd(&app, cmd0);
    // Sub bridge lands with the timer substitutes; warn once if the app
    // declares real subscriptions so silence is never mistaken for support.
    {
        let model = app.model.borrow().clone();
        if !matches!((app.subscriptions)(model), IpeSub::None) {
            console_warn("subscriptions are not yet supported on the wasm client");
        }
    }
    Ok(())
}

fn document() -> Result<web_sys::Document, String> {
    web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| "no window/document (not a browser context?)".to_owned())
}

/// Rebuild the row-poly `req` record from the browser environment.
fn synthesize_req(document: &web_sys::Document) -> Result<LiveReq, String> {
    let window = web_sys::window().ok_or("no window")?;
    let loc = window.location();
    let path = loc.pathname().unwrap_or_default();
    let query = loc.search().unwrap_or_default();
    let raw_query = query.strip_prefix('?').unwrap_or(&query).to_owned();

    let mut cookies: crate::dict::IpeDict<String> = crate::dict::IpeDict::new();
    // `cookie()` lives on `HtmlDocument`; checked cast (an XML document has none).
    if let Some(html_doc) = document.dyn_ref::<web_sys::HtmlDocument>()
        && let Ok(cookie_str) = html_doc.cookie()
    {
        for pair in cookie_str.split(';') {
            let pair = pair.trim();
            if let Some((k, v)) = pair.split_once('=') {
                cookies.entry(k.to_owned()).or_insert_with(|| v.to_owned());
            }
        }
    }

    Ok(LiveReq {
        path,
        query: raw_query,
        method: "GET".to_owned(),
        params: crate::dict::IpeDict::new(),
        headers: crate::dict::IpeDict::new(),
        cookies,
    })
}

/// The delegated event names — one listener per name on `<body>`, so nodes
/// inserted by later patches need no re-wiring and no per-node `Closure`
/// lifecycle exists to leak.
const DELEGATED_EVENTS: &[&str] = &[
    "click",
    "input",
    "change",
    "submit",
    "keydown",
    "keyup",
    "mouseover",
    "mouseout",
];

fn attach_delegated_listeners<Model, Msg>(
    body: &web_sys::HtmlElement,
    app: &Rc<App<Model, Msg>>,
) -> Result<(), String>
where
    Model: Clone + 'static,
    Msg: Clone + 'static,
{
    for name in DELEGATED_EVENTS {
        let app = Rc::clone(app);
        let event_name: &'static str = name;
        let closure = Closure::<dyn Fn(web_sys::Event)>::new(move |ev: web_sys::Event| {
            on_dom_event(&app, event_name, &ev);
        });
        body.add_event_listener_with_callback(name, closure.as_ref().unchecked_ref())
            .map_err(|e| format!("addEventListener({name}) failed: {e:?}"))?;
        // Delegated root listeners live for the whole app lifetime.
        closure.forget();
    }
    Ok(())
}

/// Decode + dispatch one delegated DOM event: climb from the target to the
/// nearest ancestor whose sky-id has a handler for this event name.
fn on_dom_event<Model, Msg>(app: &Rc<App<Model, Msg>>, name: &'static str, ev: &web_sys::Event)
where
    Model: Clone + 'static,
    Msg: Clone + 'static,
{
    let target: Option<web_sys::Element> = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok());
    let args = decode_event_args(name, ev, target.as_ref());

    let mut cur = target;
    while let Some(el) = cur {
        if let Some(ipe_id) = el.get_attribute("sky-id") {
            let resolved = if name == "submit" {
                ev.prevent_default();
                let fd = collect_form_data(&el);
                app.index.borrow().resolve_form(&ipe_id, name, fd)
            } else {
                app.index.borrow().resolve(&ipe_id, name, &args)
            };
            if let Some(msg) = resolved {
                enqueue(app, msg);
                return;
            }
        }
        cur = el.parent_element();
    }
}

/// Wire-event args per shape (mirrors the SSE wire): checkbox/radio →
/// checked Bool, other input-likes → value String, key events → key.
fn decode_event_args(
    name: &str,
    ev: &web_sys::Event,
    target: Option<&web_sys::Element>,
) -> Vec<String> {
    match name {
        "input" | "change" => target.map_or_else(Vec::new, |el| {
            if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
                let ty = input.type_();
                if ty == "checkbox" || ty == "radio" {
                    vec![input.checked().to_string()]
                } else {
                    vec![input.value()]
                }
            } else if let Some(ta) = el.dyn_ref::<web_sys::HtmlTextAreaElement>() {
                vec![ta.value()]
            } else if let Some(sel) = el.dyn_ref::<web_sys::HtmlSelectElement>() {
                vec![sel.value()]
            } else {
                Vec::new()
            }
        }),
        "keydown" | "keyup" => ev
            .dyn_ref::<web_sys::KeyboardEvent>()
            .map_or_else(Vec::new, |k| vec![k.key()]),
        _ => Vec::new(),
    }
}

/// Collect `{name: value}` form data from a `<form>` element (the shape
/// `dom::form::decode_form` narrows into the typed record).
fn collect_form_data(el: &web_sys::Element) -> FormData {
    let mut out = FormData::new();
    let Some(form) = el.dyn_ref::<web_sys::HtmlFormElement>() else {
        return out;
    };
    let Ok(fd) = web_sys::FormData::new_with_form(form) else {
        return out;
    };
    let entries = fd.entries();
    loop {
        let Ok(next) = entries.next() else { break };
        if next.done() {
            break;
        }
        let Ok(pair) = next.value().dyn_into::<js_sys::Array>() else {
            continue;
        };
        let key = pair.get(0).as_string().unwrap_or_default();
        let val = pair.get(1).as_string().unwrap_or_default();
        // First-value-wins on duplicate names (matches the server decode).
        out.entry(key).or_insert(val);
    }
    out
}

/// Queue a message and coalesce processing into one rAF tick — one
/// update+view+diff+patch per animation frame (the browser analogue of the
/// SSE wire's seq-ordered batching).
fn enqueue<Model, Msg>(app: &Rc<App<Model, Msg>>, msg: Msg)
where
    Model: Clone + 'static,
    Msg: Clone + 'static,
{
    app.queue.borrow_mut().push_back(msg);
    if app.frame_scheduled.get() {
        return;
    }
    app.frame_scheduled.set(true);
    let app2 = Rc::clone(app);
    let cb = Closure::once_into_js(move || {
        app2.frame_scheduled.set(false);
        flush(&app2);
    });
    let scheduled = web_sys::window()
        .map(|w| w.request_animation_frame(cb.unchecked_ref()).is_ok())
        .unwrap_or(false);
    if !scheduled {
        // No rAF (e.g. hidden document edge) — run on the microtask queue.
        let app3 = Rc::clone(app);
        wasm_bindgen_futures::spawn_local(async move {
            app3.frame_scheduled.set(false);
            flush(&app3);
        });
    }
}

/// Drain the queue: fold every pending Msg through `update`, then run ONE
/// view+diff+patch cycle and rebuild the handler index.
fn flush<Model, Msg>(app: &Rc<App<Model, Msg>>)
where
    Model: Clone + 'static,
    Msg: Clone + 'static,
{
    let msgs: Vec<Msg> = app.queue.borrow_mut().drain(..).collect();
    if msgs.is_empty() {
        return;
    }
    let mut model = app.model.borrow().clone();
    let mut cmds: Vec<IpeCmd<Msg>> = Vec::new();
    for msg in msgs {
        let (next, cmd) = (app.update)(msg, model);
        model = next;
        cmds.push(cmd);
    }
    *app.model.borrow_mut() = model.clone();

    let mut new_tree = (app.view)(model);
    assign_sky_ids(&mut new_tree, ROOT_SKY_ID);
    let patches = diff(&app.tree.borrow(), &new_tree);
    apply_patches(&patches);
    *app.index.borrow_mut() = build_index(&new_tree);
    *app.tree.borrow_mut() = new_tree;

    for cmd in cmds {
        run_cmd(app, cmd);
    }
}

/// Fire a Cmd: None/Batch recurse; Perform runs on the microtask queue and
/// feeds its Msg back through the scheduler; Publish is not yet bridged.
fn run_cmd<Model, Msg>(app: &Rc<App<Model, Msg>>, cmd: IpeCmd<Msg>)
where
    Model: Clone + 'static,
    Msg: Clone + 'static,
{
    match cmd {
        IpeCmd::None => {}
        IpeCmd::Batch(items) => {
            for c in items {
                run_cmd(app, c);
            }
        }
        IpeCmd::Perform(thunk) => {
            let app = Rc::clone(app);
            wasm_bindgen_futures::spawn_local(async move {
                let msg = thunk().await;
                enqueue(&app, msg);
            });
        }
        IpeCmd::Publish(_) => {
            console_fatal(
                "UnsupportedEffect",
                "Cmd.publish is not yet bridged on the wasm client",
            );
        }
    }
}

/// Apply the same `Vec<Patch>` the SSE wire serialises, via typed web-sys
/// calls. Target lookup is by the `sky-id` attribute, exactly as `client.js`.
fn apply_patches(patches: &[Patch]) {
    let Ok(document) = document() else {
        console_fatal("PatchTargetLost", "document vanished during patch apply");
        return;
    };
    for p in patches {
        let selector = format!("[sky-id=\"{}\"]", p.id.replace('"', "\\\""));
        let Ok(Some(el)) = document.query_selector(&selector) else {
            // A stale id can appear when a parent html-replace in this same
            // batch already rewrote the subtree — benign, matching client.js.
            continue;
        };
        if p.remove {
            el.remove();
            continue;
        }
        if let Some(text) = &p.text {
            el.set_text_content(Some(text));
        }
        if let Some(html) = &p.html {
            // Sanitiser-gated: `Patch.html` is produced exclusively by
            // `render_html` over the new subtree.
            el.set_inner_html(html);
        }
        for (k, v) in &p.attrs {
            if v.is_empty() {
                let _ = el.remove_attribute(k);
            } else {
                let _ = el.set_attribute(k, v);
                sync_dom_property(&el, k, v);
            }
        }
    }
}

/// Attributes whose DOM *property* does not reflect from the attribute —
/// mirror `client.js`'s value/checked/selected/disabled sync.
fn sync_dom_property(el: &web_sys::Element, key: &str, val: &str) {
    let truthy = !val.is_empty() && val != "false";
    match key {
        "value" => {
            if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
                input.set_value(val);
            } else if let Some(ta) = el.dyn_ref::<web_sys::HtmlTextAreaElement>() {
                ta.set_value(val);
            } else if let Some(sel) = el.dyn_ref::<web_sys::HtmlSelectElement>() {
                sel.set_value(val);
            }
        }
        "checked" => {
            if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
                input.set_checked(truthy);
            }
        }
        "disabled" => {
            if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
                input.set_disabled(truthy);
            }
        }
        _ => {}
    }
}

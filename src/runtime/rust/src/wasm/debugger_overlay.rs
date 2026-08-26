//! Client-WASM debugger overlay — message list + scrubber panel.
//!
//! Mounts a fixed-position panel into the page via CHECKED `web-sys` calls.
//! The panel is NOT part of the app's own view tree and does not perturb the
//! app DOM or ipe-id space: it is appended directly to `<body>` by overlay
//! code, never by `view`/diff/patch.
//!
//! ## Live vs scrubbed modes
//!
//! - **Live** (scrubber at the rightmost position): new messages append to the
//!   log and the message list updates; the app renders the live model as usual.
//! - **Scrubbed** (scrubber at any earlier position): incoming messages still
//!   record into the ring buffer (history is never corrupted), but the app view
//!   is re-rendered at the reconstructed model for the selected step and frozen
//!   there. Moving the scrubber back to the end resumes live mode and re-renders
//!   the current live model.
//!
//! ## No re-fire guarantee
//!
//! Scrubbing calls `RecordBuffer::reconstruct`, which is a pure re-fold over the
//! retained message log — every `Cmd` produced by `update` during reconstruction
//! is discarded. The overlay never enqueues or runs any `Cmd`.
//!
//! ## Secret redaction
//!
//! Message labels in the list pass through `IpeStringify::ipe_show`, which is
//! hand-implemented on `Secret` to always emit the fixed redacted placeholder.
//! A `Secret`-bearing `Msg` cannot implement `serde::Serialize`, so export is
//! compile-time unavailable for such types, but live display still works — every
//! rendered label is redacted at the `IpeStringify` layer, never in the clear.
//!
//! ## Zero residue without `--debugger`
//!
//! Every item in this file is `#[cfg(feature = "debugger")]`-gated. A build
//! without the feature includes none of this code.

#![cfg(feature = "debugger")]

use std::cell::Cell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

// ── Constants ────────────────────────────────────────────────────────────────

/// Attribute written on the overlay root so nothing in the app's ipe-id
/// selector can accidentally match it.
const OVERLAY_ATTR: &str = "data-ipe-debugger";

/// Inline style applied to the overlay container — fixed bottom-right, above
/// everything, small neutral chrome. Self-contained so no stylesheet is needed.
const OVERLAY_STYLE: &str = concat!(
    "position:fixed;bottom:0;right:0;width:320px;max-height:40vh;",
    "background:#1e1e1e;color:#d4d4d4;font:12px/1.4 monospace;",
    "border-top-left-radius:6px;overflow:hidden;",
    "box-shadow:0 -2px 8px rgba(0,0,0,.4);z-index:2147483647;",
    "display:flex;flex-direction:column;"
);

/// Style for the scrubber row.
const SCRUBBER_STYLE: &str =
    "display:flex;align-items:center;gap:6px;padding:4px 6px;background:#252526;flex-shrink:0;";

/// Style for the message list scrollable area.
const LIST_STYLE: &str = "overflow-y:auto;flex:1;padding:4px 0;";

/// Style for a single message row — normal (live) state.
const ROW_STYLE_LIVE: &str =
    "padding:1px 8px;cursor:pointer;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;";

/// Style for the selected (scrubbed) message row.
const ROW_STYLE_SELECTED: &str = "padding:1px 8px;cursor:pointer;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;background:#264f78;";

/// Maximum label length shown per message (keeps the panel readable).
const MAX_LABEL_LEN: usize = 80;

// ── Overlay state ─────────────────────────────────────────────────────────────

/// The state kept alongside the `App` for the overlay panel.
///
/// `scrub_step` is `None` in live mode; `Some(n)` when the user has selected
/// step `n` (0-indexed in the ring buffer's retained window). The driver checks
/// this to decide whether to re-render at the reconstructed model or let the
/// normal flush path render the live model.
pub(super) struct OverlayState {
    /// The overlay root element (fixed panel in `<body>`).
    root: web_sys::HtmlElement,
    /// The `<input type=range>` scrubber.
    scrubber: web_sys::HtmlInputElement,
    /// The `<div>` that holds the message rows.
    list_el: web_sys::HtmlElement,
    /// `None` = live; `Some(n)` = viewing retained step n.
    pub(super) scrub_step: Cell<Option<usize>>,
}

impl OverlayState {
    /// The current scrub position, if any.
    pub(super) fn scrub_step(&self) -> Option<usize> {
        self.scrub_step.get()
    }
}

// ── Mount ────────────────────────────────────────────────────────────────────

/// Create and mount the overlay panel into `<body>`.
///
/// Returns `None` (with a console warning) on any DOM failure rather than
/// propagating errors into the app mount path.
pub(super) fn mount_overlay() -> Option<Rc<OverlayState>> {
    let result = try_mount_overlay();
    match result {
        Ok(state) => Some(Rc::new(state)),
        Err(msg) => {
            super::console_warn(&format!("debugger-overlay mount failed: {msg}"));
            None
        }
    }
}

fn try_mount_overlay() -> Result<OverlayState, String> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let body: web_sys::HtmlElement = document.body().ok_or("no body")?;

    // ── Root container ────────────────────────────────────────────────────
    let root: web_sys::HtmlElement = document
        .create_element("div")
        .map_err(|e| format!("createElement(div): {e:?}"))?
        .dyn_into::<web_sys::HtmlElement>()
        .map_err(|_| "root cast to HtmlElement failed")?;

    root.set_attribute(OVERLAY_ATTR, "1")
        .map_err(|e| format!("setAttribute: {e:?}"))?;
    root.set_attribute("style", OVERLAY_STYLE)
        .map_err(|e| format!("setAttribute style: {e:?}"))?;

    // ── Scrubber row ──────────────────────────────────────────────────────
    let scrubber_row: web_sys::HtmlElement = document
        .create_element("div")
        .map_err(|e| format!("createElement scrubber-row: {e:?}"))?
        .dyn_into::<web_sys::HtmlElement>()
        .map_err(|_| "scrubber-row cast failed")?;
    scrubber_row
        .set_attribute("style", SCRUBBER_STYLE)
        .map_err(|e| format!("setAttribute scrubber-row style: {e:?}"))?;

    // Label
    let label: web_sys::HtmlElement = document
        .create_element("span")
        .map_err(|e| format!("createElement span: {e:?}"))?
        .dyn_into::<web_sys::HtmlElement>()
        .map_err(|_| "label cast failed")?;
    label.set_text_content(Some("▶ Debugger"));
    label
        .set_attribute("style", "flex-shrink:0;opacity:.7;")
        .map_err(|e| format!("setAttribute label style: {e:?}"))?;

    // Range input
    let scrubber: web_sys::HtmlInputElement = document
        .create_element("input")
        .map_err(|e| format!("createElement input: {e:?}"))?
        .dyn_into::<web_sys::HtmlInputElement>()
        .map_err(|_| "input cast to HtmlInputElement failed")?;
    scrubber.set_type("range");
    scrubber.set_min("0");
    scrubber.set_max("0");
    scrubber.set_value("0");
    scrubber
        .set_attribute("style", "flex:1;min-width:0;")
        .map_err(|e| format!("setAttribute scrubber style: {e:?}"))?;

    scrubber_row
        .append_child(&label)
        .map_err(|e| format!("appendLabel: {e:?}"))?;
    scrubber_row
        .append_child(&scrubber)
        .map_err(|e| format!("appendScrubber: {e:?}"))?;

    // ── Message list ──────────────────────────────────────────────────────
    let list_el: web_sys::HtmlElement = document
        .create_element("div")
        .map_err(|e| format!("createElement list: {e:?}"))?
        .dyn_into::<web_sys::HtmlElement>()
        .map_err(|_| "list cast failed")?;
    list_el
        .set_attribute("style", LIST_STYLE)
        .map_err(|e| format!("setAttribute list style: {e:?}"))?;

    root.append_child(&scrubber_row)
        .map_err(|e| format!("appendScrubberRow: {e:?}"))?;
    root.append_child(&list_el)
        .map_err(|e| format!("appendList: {e:?}"))?;

    body.append_child(&root)
        .map_err(|e| format!("appendRoot: {e:?}"))?;

    Ok(OverlayState {
        root,
        scrubber,
        list_el,
        scrub_step: Cell::new(None),
    })
}

// ── Render ────────────────────────────────────────────────────────────────────

/// Rebuild the overlay panel contents from the current ring-buffer snapshot.
///
/// `labels` — pre-rendered string labels for retained steps (oldest first).
/// `len`    — total number of retained steps (equals `labels` count).
/// `selected` — which step index is currently selected (`None` = live = last).
///
/// This never touches the app DOM — it only mutates elements owned by the
/// overlay (`list_el`, `scrubber`).
pub(super) fn render_overlay(
    state: &OverlayState,
    labels: impl Iterator<Item = String>,
    len: usize,
    selected: Option<usize>,
) {
    let Some(document) = (|| web_sys::window()?.document())() else {
        return;
    };

    // ── Update scrubber range ─────────────────────────────────────────────
    // max = len (step past the last recorded step = live mode).
    // value = selected.unwrap_or(len) — live is at the end.
    let max_val = len.to_string();
    let cur_val = selected.unwrap_or(len).to_string();
    state.scrubber.set_max(&max_val);
    state.scrubber.set_value(&cur_val);

    // ── Rebuild message list ──────────────────────────────────────────────
    state.list_el.set_inner_html(""); // clear; bounded by ring-buffer cap

    let selected_step = selected.unwrap_or(len); // len means "past end" = live
    for (idx, label_raw) in labels.enumerate() {
        let label = truncate(label_raw, MAX_LABEL_LEN);
        let style = if idx == selected_step && selected.is_some() {
            ROW_STYLE_SELECTED
        } else {
            ROW_STYLE_LIVE
        };

        let row: web_sys::HtmlElement = match document
            .create_element("div")
            .ok()
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        {
            Some(el) => el,
            None => continue,
        };
        let _ = row.set_attribute("style", style);
        let _ = row.set_attribute("data-ipe-dbg-step", &idx.to_string());
        row.set_text_content(Some(&label));

        let _ = state.list_el.append_child(&row);
    }

    // Auto-scroll to bottom when in live mode.
    if selected.is_none() {
        state.list_el.set_scroll_top(state.list_el.scroll_height());
    }
}

/// Truncate a string to at most `max` characters, appending `…` if cut.
fn truncate(mut s: String, max: usize) -> String {
    if s.chars().count() <= max {
        return s;
    }
    // Truncate at char boundary.
    let cut: usize = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
    s.truncate(cut);
    s.push('…');
    s
}

// ── Event wiring ──────────────────────────────────────────────────────────────

/// Attach the scrubber `input` listener and the list `click` listener.
///
/// When the user moves the scrubber:
/// - If the new value equals `len` (rightmost), resume live mode.
/// - Otherwise, set `scrub_step = Some(n)` and call `on_scrub(n)`.
///
/// When the user clicks a message row:
/// - Read `data-ipe-dbg-step`, set the scrubber, call `on_scrub`.
///
/// The `on_scrub` callback is provided by the driver (`wasm/mod.rs`) and
/// triggers the reconstruct→re-render path WITHOUT running any `Cmd`.
pub(super) fn attach_overlay_listeners<F>(state: &Rc<OverlayState>, on_scrub: F)
where
    F: Fn(Option<usize>) + 'static,
{
    let on_scrub = Rc::new(on_scrub);

    // ── Scrubber input ────────────────────────────────────────────────────
    {
        let state2 = Rc::clone(state);
        let cb = Rc::clone(&on_scrub);
        let closure = Closure::<dyn Fn(web_sys::Event)>::new(move |ev: web_sys::Event| {
            let Some(input) = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
            else {
                return;
            };
            let val: usize = input.value().parse().unwrap_or(0);
            let max: usize = input.max().parse().unwrap_or(0);
            // `val == max` means "live" (past the last step).
            let step = if val >= max { None } else { Some(val) };
            state2.scrub_step.set(step);
            cb(step);
        });
        let _ = state
            .scrubber
            .add_event_listener_with_callback("input", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    // ── List click ────────────────────────────────────────────────────────
    {
        let state2 = Rc::clone(state);
        let cb = Rc::clone(&on_scrub);
        let closure = Closure::<dyn Fn(web_sys::Event)>::new(move |ev: web_sys::Event| {
            // Walk up from the click target to find a row with data-ipe-dbg-step.
            let mut cur: Option<web_sys::Element> = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok());
            while let Some(el) = cur {
                if let Some(attr) = el.get_attribute("data-ipe-dbg-step") {
                    if let Ok(n) = attr.parse::<usize>() {
                        // Set the scrubber value to match.
                        state2.scrubber.set_value(&n.to_string());
                        state2.scrub_step.set(Some(n));
                        cb(Some(n));
                        return;
                    }
                }
                cur = el.parent_element();
            }
        });
        let _ = state
            .list_el
            .add_event_listener_with_callback("click", closure.as_ref().unchecked_ref());
        closure.forget();
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncate_short_string_unchanged() {
        let s = "hello".to_owned();
        assert_eq!(truncate(s.clone(), 10), s);
    }

    #[test]
    fn truncate_long_string_appends_ellipsis() {
        let s = "abcdefghij".to_owned(); // 10 chars
        let t = truncate(s, 5);
        assert!(t.ends_with('…'), "must end with ellipsis");
        assert!(t.starts_with("abcde"));
    }

    #[test]
    fn truncate_exact_boundary_unchanged() {
        let s = "hello".to_owned();
        assert_eq!(truncate(s, 5), "hello");
    }
}

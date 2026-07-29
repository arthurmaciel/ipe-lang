//! Ipe.Tui — the `tui_app` TEA loop.
//!
//! Mirrors `console_app` (tea.rs) exactly — same `CliEvent` channel, `SubManager`
//! (so `Sub.every` → Tick works) and `cli_run_cmd` (so `Cmd.perform` works) — but
//! reads RAW key bytes (raw mode + `decode_key`) instead of stdin lines, and
//! paints into the alternate screen. A Ipe.Tui app quits by calling `System.exit`
//! from `update` (the `Quit` Msg) or by stdin EOF.
//!
//! No panic vectors: a `TuiGuard` restores the TTY (cooked mode, cursor, main
//! screen) on Drop — normal exit AND panic unwind — so no path leaves the
//! terminal wedged. Raw-mode failure returns `Err`; `TERM=dumb` is refused.

use super::super::core::{IpeResult, IpeTask, ok_res};
use super::super::tea::{CliEvent, IpeCmd, IpeSub, SubManager, cli_run_cmd};
use super::super::ui::Element;
use super::focus::{
    Focusable, InputRegistry, clamp_focus, edit_input, ensure_focus_visible, extract_click_msg,
    extract_input_msg, extract_msg_named, hit_test, parse_mouse,
};
use super::key::{TuiKey, decode_key};
use super::layout::render_with_focus;
use std::io::{Read, Write};

const ALT_SCREEN_ON: &str = "\x1b[?1049h";
const ALT_SCREEN_OFF: &str = "\x1b[?1049l";
const CLEAR_HOME: &str = "\x1b[2J\x1b[H";
const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";
// Click tracking (1000) + SGR extended coords (1006) — wheel reports as buttons
// 64/65, clicks as button 0. Only the Element backend (`tui_app_ui`) enables it.
const MOUSE_ON: &str = "\x1b[?1000;1006h";
const MOUSE_OFF: &str = "\x1b[?1000;1006l";

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether a TUI session is currently active (terminal in raw mode + alt screen).
/// Gates `tui_teardown` so the restore runs exactly once, from whichever path
/// fires first (Drop on a clean break / panic unwind, OR the System.exit hook).
static TUI_RESTORE_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Whether mouse reporting was enabled (so the teardown emits MOUSE_OFF).
static TUI_MOUSE: AtomicBool = AtomicBool::new(false);

/// Idempotent terminal restore: mouse off → cursor shown → main screen → cooked
/// mode. Runs once (AtomicBool gate). Called from EITHER the `TuiGuard` Drop
/// (clean break / panic unwind) OR the `System.exit` hook — the latter is the
/// load-bearing path: `std::process::exit` bypasses Drop, so without the hook a
/// `Cmd.perform (System.exit n)` quit would leave the TTY in raw mode + the
/// alternate screen (needing `reset`). Mirrors Go's `tuiTeardown`. Never panics.
fn tui_teardown() {
    if !TUI_RESTORE_ACTIVE.swap(false, Ordering::SeqCst) {
        return; // already restored, or never entered
    }
    let mut out = std::io::stdout();
    if TUI_MOUSE.load(Ordering::SeqCst) {
        let _ = out.write_all(MOUSE_OFF.as_bytes());
    }
    let _ = out.write_all(SHOW_CURSOR.as_bytes());
    let _ = out.write_all(ALT_SCREEN_OFF.as_bytes());
    let _ = out.flush();
    let _ = crossterm::terminal::disable_raw_mode();
}

/// RAII terminal-state guard — restores cooked mode + cursor + main screen (and
/// mouse reporting, when enabled) on Drop (normal exit or panic unwind) AND via
/// the registered `System.exit` hook (process::exit bypasses Drop). Best-effort,
/// never panics.
struct TuiGuard;

impl TuiGuard {
    /// String-view driver (`tui_app`, the raw-cell path) — no mouse reporting.
    fn enter() -> Result<Self, String> {
        Self::enter_with(false)
    }
    /// Element-view driver (`tui_app_ui` / `Terminal.appScreen`) — enables mouse reporting
    /// for focus-click + wheel scroll.
    fn enter_mouse() -> Result<Self, String> {
        Self::enter_with(true)
    }
    fn enter_with(mouse: bool) -> Result<Self, String> {
        crossterm::terminal::enable_raw_mode().map_err(|e| format!("Tui: enable raw mode: {e}"))?;
        TUI_MOUSE.store(mouse, Ordering::SeqCst);
        TUI_RESTORE_ACTIVE.store(true, Ordering::SeqCst);
        // Register the teardown so a `System.exit` quit restores the terminal even
        // though process::exit skips Drop. Idempotent with the Drop path below.
        crate::system::register_exit_hook(tui_teardown);
        let mut out = std::io::stdout();
        let _ = out.write_all(ALT_SCREEN_ON.as_bytes());
        let _ = out.write_all(HIDE_CURSOR.as_bytes());
        if mouse {
            let _ = out.write_all(MOUSE_ON.as_bytes());
        }
        let _ = out.flush();
        Ok(TuiGuard)
    }
}

impl Drop for TuiGuard {
    fn drop(&mut self) {
        tui_teardown();
    }
}

fn paint(frame: &str) {
    let mut out = std::io::stdout();
    let _ = out.write_all(CLEAR_HOME.as_bytes());
    let _ = out.write_all(frame.as_bytes());
    let _ = out.flush();
}

/// Fire `onBlur` for the previously-focused element + `onFocus` for the newly-
/// focused one (when those events are bound), enqueuing each Msg on the event
/// channel so it flows through the same `update` sequence as everything else.
/// Mirrors Go's `tuiDispatchFocusChange`. A send failure (receiver gone) is
/// ignored — the loop is tearing down anyway. No-op when focus didn't move.
fn dispatch_focus_change<Msg: Clone + Send + 'static>(
    focusables: &[Focusable<Msg>],
    old_idx: usize,
    new_idx: usize,
    tx: &tokio::sync::mpsc::UnboundedSender<CliEvent<Msg>>,
) {
    if old_idx == new_idx {
        return;
    }
    if let Some(msg) = focusables
        .get(old_idx)
        .and_then(|f| extract_msg_named(&f.events, "blur"))
    {
        let _ = tx.send(CliEvent::Msg(msg));
    }
    if let Some(msg) = focusables
        .get(new_idx)
        .and_then(|f| extract_msg_named(&f.events, "focus"))
    {
        let _ = tx.send(CliEvent::Msg(msg));
    }
}

/// Current terminal size in cells; `(80, 24)` if it can't be queried (e.g. the
/// stream isn't a TTY). Re-queried each paint so the Element renderer reflows on
/// resize.
fn term_size() -> (usize, usize) {
    // Fall back to 80×24 when the size can't be determined OR is reported as 0 in
    // either dimension (a pty with no winsize set / a non-interactive pipe reports
    // (0, 0); crossterm passes that through). Clamping a 0 to 1 — as the old code
    // did — rendered a 1×1 canvas, i.e. an (almost) blank frame, diverging from Go
    // (which defaults to 80×24). Only a genuine non-zero size is honoured.
    match crossterm::terminal::size() {
        Ok((w, h)) if w > 0 && h > 0 => (w as usize, h as usize),
        _ => (80, 24),
    }
}

/// Longest key sequence `decode_key` recognises (`ESC [ 1 ; <mod> <final>` = 6
/// bytes); padded to 8 so any tail at least this long is decoded, never carried.
const MAX_KEY_SEQ: usize = 8;

/// Bytes a UTF-8 lead byte announces (1 for ASCII / continuation / invalid lead).
fn utf8_seq_len(lead: u8) -> usize {
    match lead {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

/// Whether `tail` — the bytes left at the end of a COMPLETELY FILLED read — might
/// be the truncated prefix of a longer key sequence and so should be carried into
/// the next read rather than decoded now. ESC sequences are variable-length; a
/// multibyte UTF-8 char needs all its continuation bytes. Carrying a tail that is
/// in fact already complete is harmless — it is decoded on the next read with the
/// following bytes prepended (one-read latency, never a drop or mis-decode).
fn tail_maybe_truncated(tail: &[u8]) -> bool {
    match tail.first() {
        Some(&0x1b) => tail.len() < MAX_KEY_SEQ,
        Some(&b) if b >= 0x80 => tail.len() < utf8_seq_len(b),
        _ => false,
    }
}

/// Blocking raw-key reader: decodes stdin bytes into `CliEvent::Key(kind, value)`
/// events via `decode_key`, then a final `Eof`. Reassembles escape / UTF-8 key
/// sequences that straddle the fixed 64-byte read boundary by carrying the
/// unconsumed tail into the next read — a fixed-buffer decode would otherwise
/// mis-decode or corrupt a split sequence (e.g. a multibyte char inside a paste
/// larger than 64 bytes). `map_kind` turns the decoded `TuiKey` into the wire
/// `(kind, value)` pair (`tui_app_ui` folds the ctrl modifier into the kind for
/// the input editor's word-jumps; `tui_app` passes it through). Runs on its own
/// blocking thread so `on_key` stays off it.
fn read_keys_loop<Msg, FMap>(tx: &tokio::sync::mpsc::UnboundedSender<CliEvent<Msg>>, map_kind: FMap)
where
    FMap: Fn(TuiKey) -> (String, String),
{
    let mut stdin = std::io::stdin();
    let mut buf = [0u8; 64];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut data = std::mem::take(&mut carry);
        data.extend_from_slice(buf.get(..n).unwrap_or(&[]));
        // A completely filled read signals more bytes are likely queued, so a
        // trailing partial sequence should wait for them; a short read is taken
        // as the full input (so a solo Escape stays responsive).
        let filled = n == buf.len();
        let mut i = 0;
        while i < data.len() {
            let rest = data.get(i..).unwrap_or(&[]);
            if filled && tail_maybe_truncated(rest) {
                break;
            }
            let (k, consumed) = decode_key(rest);
            if consumed == 0 {
                break;
            }
            i += consumed;
            let (kind, value) = map_kind(k);
            if tx.send(CliEvent::Key(kind, value)).is_err() {
                return;
            }
        }
        carry = data.get(i..).map(|tail| tail.to_vec()).unwrap_or_default();
    }
    // Drain any carried tail before EOF (decode greedily — no more bytes coming).
    let mut i = 0;
    while i < carry.len() {
        let (k, consumed) = decode_key(carry.get(i..).unwrap_or(&[]));
        if consumed == 0 {
            break;
        }
        i += consumed;
        let (kind, value) = map_kind(k);
        if tx.send(CliEvent::Key(kind, value)).is_err() {
            return;
        }
    }
    let _ = tx.send(CliEvent::Eof);
}

/// Shared terminal TEA driver. `render_frame` turns the current model into the
/// ANSI frame painted each tick — `tui_app` passes the user's `String` view
/// straight through; `tui_app_ui` renders the `Ipe.Ui` Element tree via
/// `render_element`. Everything else (raw-key reader, `SubManager` ticks,
/// `cli_run_cmd`, RAII teardown) is identical and lives here once.
#[allow(clippy::type_complexity)]
fn tui_run<Model, Msg, E, FInit, FUpdate, FSubs, FOnKey, FRender>(
    init: FInit,
    update: FUpdate,
    subscriptions: FSubs,
    on_key: FOnKey,
    render_frame: FRender,
) -> IpeTask<E, ()>
where
    E: Send + From<String> + 'static,
    Model: Clone + Send + 'static,
    Msg: Clone + Send + 'static,
    FInit: Fn(()) -> (Model, IpeCmd<Msg>) + Send + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, IpeCmd<Msg>) + Send + 'static,
    FSubs: Fn(Model) -> IpeSub<Msg> + Send + 'static,
    FOnKey: Fn(String, String) -> Msg + Send + 'static,
    FRender: Fn(&Model) -> String + Send + 'static,
{
    Box::pin(async move {
        if crate::system::read_env_var("TERM").as_deref() == Ok("dumb") {
            return IpeResult::Err(
                "Tui: TERM=dumb is not an interactive terminal"
                    .to_string()
                    .into(),
            );
        }
        let _guard = match TuiGuard::enter() {
            Ok(g) => g,
            Err(e) => return IpeResult::Err(e.into()),
        };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<CliEvent<Msg>>();

        // Blocking raw-key reader → Key(kind, value) events, then Eof. on_key is
        // applied in the main task (keeps it off the blocking thread).
        let key_tx = tx.clone();
        std::thread::spawn(move || {
            read_keys_loop(&key_tx, |k| (k.kind, k.value));
        });

        let (mut model, cmd0) = init(());
        cli_run_cmd(cmd0, &tx);
        let mut submgr = SubManager::new(tx.clone());
        submgr.update(subscriptions(model.clone()));
        paint(&render_frame(&model));

        while let Some(ev) = rx.recv().await {
            let msg = match ev {
                CliEvent::Key(kind, value) => on_key(kind, value),
                // Tui fires Cmds untracked, so a Perform result also arrives as a
                // plain Msg; PerformDone is matched for exhaustiveness.
                CliEvent::Msg(m) | CliEvent::PerformDone(m) => m,
                CliEvent::Line(_) => continue, // Tui has no line input
                CliEvent::Eof => break,
            };
            let (next, cmd) = update(msg, model);
            model = next;
            cli_run_cmd(cmd, &tx);
            submgr.update(subscriptions(model.clone()));
            paint(&render_frame(&model));
        }
        submgr.stop_all();
        ok_res(())
    })
}

/// `tui_app` — terminal TEA driver for a `view : Model -> String` (the raw
/// frame is painted verbatim), the vehicle for the `Ui.cells` raw-cell escape.
/// `on_key` receives the decoded key's
/// `(kind, value)` and yields a `Msg` (the codegen wraps the user's
/// `onKey : KeyEvent -> Msg` so the `{ kind, value }` record is built there).
#[allow(clippy::type_complexity)]
pub fn tui_app<Model, Msg, E, FInit, FUpdate, FView, FSubs, FOnKey>(
    init: FInit,
    update: FUpdate,
    view: FView,
    subscriptions: FSubs,
    on_key: FOnKey,
) -> IpeTask<E, ()>
where
    E: Send + From<String> + 'static,
    Model: Clone + Send + 'static,
    Msg: Clone + Send + 'static,
    FInit: Fn(()) -> (Model, IpeCmd<Msg>) + Send + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, IpeCmd<Msg>) + Send + 'static,
    FView: Fn(Model) -> String + Send + 'static,
    FSubs: Fn(Model) -> IpeSub<Msg> + Send + 'static,
    FOnKey: Fn(String, String) -> Msg + Send + 'static,
{
    tui_run(init, update, subscriptions, on_key, move |m: &Model| {
        view(m.clone())
    })
}

/// Render the Element view (twice: discover focusables, then scroll-correct + the
/// focus highlight) and paint it. Returns the focusables so the loop can dispatch
/// their input/click Msgs. Mirrors Go's `renderElementFrameScroll` double pass.
fn render_and_paint<Model, Msg, FView>(
    view: &FView,
    model: &Model,
    inputs: &mut InputRegistry,
    focus_idx: &mut usize,
    scroll_y: &mut usize,
) -> Vec<Focusable<Msg>>
where
    Model: Clone,
    Msg: Clone,
    FView: Fn(Model) -> Element<Msg>,
{
    let (cols, rows) = term_size();
    let (_f1, fs1, content_h) = render_with_focus(
        &view(model.clone()),
        cols,
        rows,
        *focus_idx,
        inputs,
        *scroll_y,
    );
    *focus_idx = clamp_focus(*focus_idx, fs1.len());
    *scroll_y = ensure_focus_visible(&fs1, *focus_idx, *scroll_y, rows, content_h);
    let (frame, fs2, _) = render_with_focus(
        &view(model.clone()),
        cols,
        rows,
        *focus_idx,
        inputs,
        *scroll_y,
    );
    paint(&frame);
    fs2
}

/// `Terminal.appScreen` — terminal TEA driver for a `view : Model -> Element msg`. The
/// `Ipe.Ui` Element is the SAME structured tree Ipe.Web renders to HTML; here it
/// is laid out to ANSI cells by walking the typed attributes (`tui::layout`), and
/// `Ipe.Ui.Input.*` widgets become focusables: Tab / Shift-Tab (and Up/Down on a
/// non-input focus) cycle focus; typing edits the focused text input (dispatching
/// its `onInput`); Enter/Space activates a button or toggles a checkbox/radio;
/// the view auto-scrolls to keep the focused element on screen. Ctrl-keys and any
/// unhandled key fall through to the user's `onKey`. Mouse, multiline cursor
/// movement, and word-jumps are a follow-on.
#[allow(clippy::type_complexity, clippy::too_many_lines, unused_assignments)]
pub fn tui_app_ui<Model, Msg, E, FInit, FUpdate, FView, FSubs, FOnKey>(
    init: FInit,
    update: FUpdate,
    view: FView,
    subscriptions: FSubs,
    on_key: FOnKey,
) -> IpeTask<E, ()>
where
    E: Send + From<String> + 'static,
    Model: Clone + Send + 'static,
    Msg: Clone + Send + 'static,
    FInit: Fn(()) -> (Model, IpeCmd<Msg>) + Send + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, IpeCmd<Msg>) + Send + 'static,
    FView: Fn(Model) -> Element<Msg> + Send + 'static,
    FSubs: Fn(Model) -> IpeSub<Msg> + Send + 'static,
    FOnKey: Fn(String, String) -> Msg + Send + 'static,
{
    Box::pin(async move {
        if crate::system::read_env_var("TERM").as_deref() == Ok("dumb") {
            return IpeResult::Err(
                "Tui: TERM=dumb is not an interactive terminal"
                    .to_string()
                    .into(),
            );
        }
        let _guard = match TuiGuard::enter_mouse() {
            Ok(g) => g,
            Err(e) => return IpeResult::Err(e.into()),
        };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<CliEvent<Msg>>();
        let key_tx = tx.clone();
        std::thread::spawn(move || {
            read_keys_loop(&key_tx, |k| {
                // The (kind, value) channel is flat, so fold the ctrl modifier on
                // Left/Right into the kind (`ctrlleft`/`ctrlright`) for the input
                // editor's word-jumps.
                let kind = if k.ctrl && (k.kind == "left" || k.kind == "right") {
                    format!("ctrl{}", k.kind)
                } else {
                    k.kind
                };
                (kind, k.value)
            });
        });

        let (mut model, cmd0) = init(());
        cli_run_cmd(cmd0, &tx);
        let mut submgr = SubManager::new(tx.clone());
        submgr.update(subscriptions(model.clone()));

        let mut inputs = InputRegistry::new();
        let mut focus_idx = 0usize;
        let mut scroll_y = 0usize;
        let mut focusables: Vec<Focusable<Msg>> =
            render_and_paint(&view, &model, &mut inputs, &mut focus_idx, &mut scroll_y);

        while let Some(ev) = rx.recv().await {
            let mut produced: Option<Msg> = None;
            match ev {
                CliEvent::Msg(m) | CliEvent::PerformDone(m) => produced = Some(m),
                CliEvent::Eof => break,
                CliEvent::Line(_) => continue,
                CliEvent::Key(kind, value) => {
                    // Mouse: wheel scrolls the viewport; a left-press focuses the
                    // hit element and activates it (if not an input).
                    if kind == "mouse" {
                        if let Some((btn, mcol, mrow, press)) = parse_mouse(&value) {
                            if press && (btn == 64 || btn == 65) {
                                let (cols, rows) = term_size();
                                let (_f, _fs, content_h) = render_with_focus(
                                    &view(model.clone()),
                                    cols,
                                    rows,
                                    focus_idx,
                                    &mut inputs,
                                    scroll_y,
                                );
                                let max_scroll = content_h.saturating_sub(rows);
                                scroll_y = if btn == 64 {
                                    scroll_y.saturating_sub(3)
                                } else {
                                    (scroll_y + 3).min(max_scroll)
                                };
                                let (frame, fs, _) = render_with_focus(
                                    &view(model.clone()),
                                    cols,
                                    rows,
                                    focus_idx,
                                    &mut inputs,
                                    scroll_y,
                                );
                                paint(&frame);
                                focusables = fs;
                                continue;
                            }
                            if press && btn == 0 {
                                if let Some(hit) = hit_test(
                                    &focusables,
                                    mcol.saturating_sub(1),
                                    mrow.saturating_sub(1),
                                    scroll_y,
                                ) {
                                    let old_focus = focus_idx;
                                    focus_idx = hit;
                                    // onBlur (old) + onFocus (new) on a click focus
                                    // change — same as Tab nav (Go fires it on both).
                                    dispatch_focus_change(&focusables, old_focus, hit, &tx);
                                    let is_input =
                                        focusables.get(hit).map(|f| f.is_input).unwrap_or(false);
                                    if !is_input {
                                        produced = focusables
                                            .get(hit)
                                            .and_then(|f| extract_click_msg(&f.events));
                                    }
                                    if produced.is_none() {
                                        focusables = render_and_paint(
                                            &view,
                                            &model,
                                            &mut inputs,
                                            &mut focus_idx,
                                            &mut scroll_y,
                                        );
                                        continue;
                                    }
                                    // else fall through to dispatch `produced`.
                                } else {
                                    continue;
                                }
                            } else {
                                continue;
                            }
                        } else {
                            continue;
                        }
                        // A left-press that produced a click Msg skips the key
                        // logic below (the `else`) and dispatches `produced`.
                    } else {
                        let n = focusables.len();
                        let focused_input = focusables
                            .get(focus_idx)
                            .map(|f| f.is_input)
                            .unwrap_or(false);
                        let is_shift_tab = kind == "other" && value.contains('Z');
                        let nav_fwd = kind == "tab" || (kind == "down" && !focused_input);
                        let nav_back = is_shift_tab || (kind == "up" && !focused_input);
                        // A focused <textarea>'s Enter inserts a newline (multiline
                        // edit), not a submit: remap to a char-insert so the generic
                        // edit path below handles it uniformly.
                        let is_textarea = focusables
                            .get(focus_idx)
                            .map(|f| f.input_type == "textarea")
                            .unwrap_or(false);
                        let (kind, value) = if focused_input && is_textarea && kind == "enter" {
                            ("char".to_string(), "\n".to_string())
                        } else {
                            (kind, value)
                        };

                        if (nav_fwd || nav_back) && n > 0 {
                            let old_focus = focus_idx;
                            focus_idx = if nav_back {
                                (focus_idx + n - 1) % n
                            } else {
                                (focus_idx + 1) % n
                            };
                            focusables = render_and_paint(
                                &view,
                                &model,
                                &mut inputs,
                                &mut focus_idx,
                                &mut scroll_y,
                            );
                            // onBlur (old) + onFocus (new) — Go's tuiDispatchFocusChange.
                            dispatch_focus_change(&focusables, old_focus, focus_idx, &tx);
                            continue;
                        }

                        if kind == "ctrl" {
                            produced = Some(on_key(kind, value));
                        } else if focused_input {
                            let is_cbr = focusables
                                .get(focus_idx)
                                .map(|f| f.is_checkbox_or_radio())
                                .unwrap_or(false);
                            if is_cbr && (kind == "space" || kind == "enter") {
                                produced = focusables
                                    .get(focus_idx)
                                    .and_then(|f| extract_click_msg(&f.events));
                                if produced.is_none() {
                                    continue;
                                }
                            } else if kind == "enter" {
                                let buf = inputs.get(focus_idx).buffer.clone();
                                produced = focusables.get(focus_idx).and_then(|f| {
                                    extract_input_msg(&f.events, "change", &buf)
                                        .or_else(|| extract_input_msg(&f.events, "input", &buf))
                                });
                                if produced.is_none() {
                                    continue;
                                }
                            } else {
                                let changed = edit_input(inputs.get(focus_idx), &kind, &value);
                                if changed {
                                    let buf = inputs.get(focus_idx).buffer.clone();
                                    inputs.get(focus_idx).last_value = buf.clone();
                                    produced = focusables
                                        .get(focus_idx)
                                        .and_then(|f| extract_input_msg(&f.events, "input", &buf));
                                    if produced.is_none() {
                                        // local echo (no onInput handler) — repaint only.
                                        focusables = render_and_paint(
                                            &view,
                                            &model,
                                            &mut inputs,
                                            &mut focus_idx,
                                            &mut scroll_y,
                                        );
                                        continue;
                                    }
                                } else {
                                    // cursor move / unhandled edit key — repaint the cursor.
                                    focusables = render_and_paint(
                                        &view,
                                        &model,
                                        &mut inputs,
                                        &mut focus_idx,
                                        &mut scroll_y,
                                    );
                                    continue;
                                }
                            }
                        } else if (kind == "enter" || kind == "space") && focus_idx < n {
                            produced = focusables
                                .get(focus_idx)
                                .and_then(|f| extract_click_msg(&f.events));
                            if produced.is_none() {
                                continue;
                            }
                        } else {
                            produced = Some(on_key(kind, value));
                        }
                    } // end key-logic else (non-mouse)
                }
            }

            if let Some(msg) = produced {
                let (next, cmd) = update(msg, model);
                model = next;
                cli_run_cmd(cmd, &tx);
                submgr.update(subscriptions(model.clone()));
                focusables =
                    render_and_paint(&view, &model, &mut inputs, &mut focus_idx, &mut scroll_y);
            }
        }
        submgr.stop_all();
        ok_res(())
    })
}

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
#[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
use super::super::debugger::tui::TuiDebugger;
use super::super::stringify::IpeStringify;
use super::super::tea::{CliEvent, IpeCmd, IpeSub, SubManager, cli_run_cmd};
use super::CellsView;
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
/// alternate screen (needing `reset`). Implements `tuiTeardown`. Never panics.
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
    /// Element-view driver (`tui_app_ui` / `Tui.tea`) — enables mouse reporting
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
    // Clear and frame content are concatenated into one buffer so the terminal
    // emulator receives a single write: the prior content disappears and the new
    // frame appears in one step, never leaving a visible blank between them.
    // Two separate write_all calls (clear then frame) would each flush to the tty
    // independently, letting the emulator render the blank clear before the content
    // arrives — the structural cause of the cursor-move flicker.
    let mut buf = String::with_capacity(CLEAR_HOME.len() + frame.len());
    buf.push_str(CLEAR_HOME);
    buf.push_str(frame);
    let mut out = std::io::stdout();
    let _ = out.write_all(buf.as_bytes());
    let _ = out.flush();
}

/// Fire `onBlur` for the previously-focused element + `onFocus` for the newly-
/// focused one (when those events are bound), enqueuing each Msg on the event
/// channel so it flows through the same `update` sequence as everything else.
/// Implements `tuiDispatchFocusChange`. A send failure (receiver gone) is
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
    // did — rendered a 1×1 canvas, i.e. an (almost) blank frame, diverging
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
    Msg: Clone + Send + IpeStringify + 'static,
    FInit: Fn(()) -> (Model, IpeCmd<Msg>) + Send + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, IpeCmd<Msg>) + Send + Sync + 'static,
    FView: Fn(Model) -> String + Send + 'static,
    FSubs: Fn(Model) -> IpeSub<Msg> + Send + 'static,
    FOnKey: Fn(String, String) -> Msg + Send + 'static,
{
    // Wrap update in an Arc so it can be shared between the live-pass call
    // site and the debugger's reconstruct closure without requiring Clone.
    // The Arc is a single allocation per session; it is transparent to the
    // non-debugger build path (Arc<F>: Fn(...) when F: Fn(...)).
    let update = std::sync::Arc::new(update);
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

        let key_tx = tx.clone();
        std::thread::spawn(move || {
            read_keys_loop(&key_tx, |k| {
                // Under the debugger, fold a Ctrl modifier on Left/Right into the
                // kind (`ctrlleft`/`ctrlright`) so the history step keys are
                // distinguishable on the flat (kind, value) channel.
                #[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
                let kind = if k.ctrl && (k.kind == "left" || k.kind == "right") {
                    format!("ctrl{}", k.kind)
                } else {
                    k.kind
                };
                #[cfg(not(all(feature = "debugger", not(target_arch = "wasm32"))))]
                let kind = k.kind;
                (kind, k.value)
            });
        });

        let (mut model, cmd0) = init(());
        cli_run_cmd(cmd0, &tx);
        let mut submgr = SubManager::new(tx.clone());
        submgr.update(subscriptions(model.clone()));

        #[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
        let mut dbg = {
            let upd = std::sync::Arc::clone(&update);
            TuiDebugger::new(model.clone(), move |msg, mdl| upd(msg, mdl))
        };

        let render_frame = move |m: &Model| view(m.clone());

        // Initial paint.
        #[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
        {
            let mut frame = render_frame(&model);
            frame.push_str("\r\n");
            frame.push_str(&dbg.status_line());
            paint(&frame);
        }
        #[cfg(not(all(feature = "debugger", not(target_arch = "wasm32"))))]
        paint(&render_frame(&model));

        while let Some(ev) = rx.recv().await {
            let msg = match ev {
                CliEvent::Key(kind, value) => {
                    #[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
                    {
                        // Ctrl-T: toggle time-travel mode.
                        if kind == crate::debugger::tui::TOGGLE_KIND
                            && value == crate::debugger::tui::TOGGLE_VALUE
                        {
                            let display_model = dbg.toggle().unwrap_or_else(|| model.clone());
                            let mut frame = render_frame(&display_model);
                            frame.push_str("\r\n");
                            frame.push_str(&dbg.status_line());
                            paint(&frame);
                            continue;
                        }
                        // Ctrl-Left / Ctrl-Right: step in time-travel mode.
                        if dbg.is_scrubbing() {
                            if kind == crate::debugger::tui::STEP_BACK_KIND {
                                if let Some(past) = dbg.step_back() {
                                    let mut frame = render_frame(&past);
                                    frame.push_str("\r\n");
                                    frame.push_str(&dbg.status_line());
                                    paint(&frame);
                                }
                                continue;
                            }
                            if kind == crate::debugger::tui::STEP_FWD_KIND {
                                if let Some(past) = dbg.step_fwd() {
                                    let mut frame = render_frame(&past);
                                    frame.push_str("\r\n");
                                    frame.push_str(&dbg.status_line());
                                    paint(&frame);
                                }
                                continue;
                            }
                        }
                    }
                    on_key(kind, value)
                }
                CliEvent::Msg(m) | CliEvent::PerformDone(m) => m,
                CliEvent::Line(_) => continue,
                CliEvent::Eof => break,
            };

            #[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
            let (next, cmd) = update(msg.clone(), model);
            #[cfg(not(all(feature = "debugger", not(target_arch = "wasm32"))))]
            let (next, cmd) = update(msg, model);

            model = next;

            #[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
            dbg.record(msg, model.clone());

            cli_run_cmd(cmd, &tx);
            submgr.update(subscriptions(model.clone()));

            #[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
            {
                // In time-travel mode: stay frozen on the pinned past step.
                // Toggle Ctrl-T to return to the live head.
                let display_model = dbg.current_reconstructed().unwrap_or_else(|| model.clone());
                let mut frame = render_frame(&display_model);
                frame.push_str("\r\n");
                frame.push_str(&dbg.status_line());
                paint(&frame);
            }
            #[cfg(not(all(feature = "debugger", not(target_arch = "wasm32"))))]
            paint(&render_frame(&model));
        }
        submgr.stop_all();
        ok_res(())
    })
}

/// Render the Element view (twice: discover focusables, then scroll-correct + the
/// focus highlight) and paint it. Returns the focusables so the loop can dispatch
/// their input/click Msgs. Implements `renderElementFrameScroll` double pass.
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
    FView: Fn(Model) -> CellsView<Msg>,
{
    let (cols, rows) = term_size();
    let (_f1, fs1, content_h) = render_with_focus(
        &view(model.clone()).into_element(),
        cols,
        rows,
        *focus_idx,
        inputs,
        *scroll_y,
    );
    *focus_idx = clamp_focus(*focus_idx, fs1.len());
    *scroll_y = ensure_focus_visible(&fs1, *focus_idx, *scroll_y, rows, content_h);
    let (frame, fs2, _) = render_with_focus(
        &view(model.clone()).into_element(),
        cols,
        rows,
        *focus_idx,
        inputs,
        *scroll_y,
    );
    paint(&frame);
    fs2
}

/// Lay out `model` with the debugger status line appended, WITHOUT painting —
/// the I/O-free core of a debugger frame. Owns the `CellsView -> Element`
/// conversion so no time-travel render site can drift from the layout input
/// contract (`render_with_focus` takes `&Element`). Returns the annotated frame
/// string and the new focusables.
#[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
fn render_debug_annotated<Model, Msg, FView>(
    view: &FView,
    model: Model,
    dbg: &TuiDebugger<Msg, Model>,
    inputs: &mut InputRegistry,
    focus_idx: usize,
    scroll_y: usize,
) -> (String, Vec<Focusable<Msg>>)
where
    Model: Clone,
    Msg: Clone + IpeStringify,
    FView: Fn(Model) -> CellsView<Msg>,
{
    let (cols, rows) = term_size();
    let (frame, fs, _) = render_with_focus(
        &view(model).into_element(),
        cols,
        rows,
        focus_idx,
        inputs,
        scroll_y,
    );
    let mut annotated = frame;
    annotated.push_str("\r\n");
    annotated.push_str(&dbg.status_line());
    (annotated, fs)
}

/// The reply an [`apply_control_frame`] call yields, paired with the repaint it
/// requests. Keeping the repaint out of the seam (the seam is I/O-free) lets a
/// unit test observe both without touching a real terminal, and lets the live
/// loop own the single `paint` call. `frame` is `None` when the recomputed
/// surface is byte-identical to the last painted one — the minimal-repaint
/// guard: an appearance patch that changes nothing (or a redundant scrub step)
/// costs zero writes, never a full-screen redraw.
///
/// [`apply_control_frame`]: TuiSurface::apply_control_frame
#[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
pub struct ApplyOutcome<Msg> {
    /// The child→parent reply frame (`Ack` or `ModelSnapshot`).
    pub reply: crate::control::ControlFrame,
    /// The annotated frame to paint, or `None` when nothing changed.
    pub repaint: Option<String>,
    /// The focusables of the freshly-rendered surface (unchanged when `repaint`
    /// is `None`, so the caller keeps its current set in that case).
    pub focusables: Option<Vec<Focusable<Msg>>>,
}

/// The mutable render surface of a running tui app — the input registry, the
/// focus cursor, the scroll offset, and the last painted frame. It owns the ONE
/// [`apply_control_frame`](Self::apply_control_frame) seam that realizes an
/// incoming [`ControlFrame`](crate::control::ControlFrame) onto the surface, so
/// the keyboard-driven scrub and the (later) wire-driven control both drive the
/// identical path — a second apply path is a divergence waiting to happen (the
/// `CellsView`-vs-`Element` drift that #2762 fixed).
#[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
pub struct TuiSurface {
    inputs: InputRegistry,
    focus_idx: usize,
    scroll_y: usize,
    /// The frame most recently handed out for painting — the diff baseline that
    /// makes a no-op apply cost zero writes.
    last_frame: Option<String>,
}

#[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
impl TuiSurface {
    fn new(inputs: InputRegistry, focus_idx: usize, scroll_y: usize) -> Self {
        Self {
            inputs,
            focus_idx,
            scroll_y,
            last_frame: None,
        }
    }

    /// The current focus cursor (test + caller observability).
    #[must_use]
    pub fn focus_idx(&self) -> usize {
        self.focus_idx
    }

    /// The current scroll offset (test + caller observability).
    #[must_use]
    pub fn scroll_y(&self) -> usize {
        self.scroll_y
    }

    /// The last frame handed out for painting, if any.
    #[must_use]
    pub fn last_frame(&self) -> Option<&str> {
        self.last_frame.as_deref()
    }

    /// Turn a freshly-rendered annotated frame into a repaint request: `Some`
    /// only when it differs from the last painted frame, and record it as the
    /// new baseline in that case. This is the minimal-repaint guard.
    fn diff_repaint(&mut self, annotated: String) -> Option<String> {
        if self.last_frame.as_deref() == Some(annotated.as_str()) {
            return None;
        }
        self.last_frame = Some(annotated.clone());
        Some(annotated)
    }

    /// Realize an incoming control frame onto the surface — the ONE apply seam.
    ///
    /// Two parent→child arms, one shared repaint tail:
    ///
    /// - [`ControlFrame::HotAppearance`] — register the appearance patch in the
    ///   dev overlay (where that mechanism is compiled in), then recompute the
    ///   surface from the CURRENT `model` (never through `update` — an
    ///   appearance-only edit changes literals, not state) and diff-repaint.
    ///   Focus and scroll are preserved.
    /// - [`ControlFrame::Debug`] — drive the recorder: `StepTo`/`Back`/`Forward`
    ///   move the scrub cursor and reconstruct the model at that step;
    ///   `InspectModel` reconstructs read-only and replies with a
    ///   [`ModelSnapshot`](crate::control::ControlFrame::ModelSnapshot); `Reset`
    ///   and `LiveTail` return to the live head. Every reconstruct re-folds
    ///   `update` over the retained messages and re-fires no `Cmd`, so a scrub
    ///   never perturbs the live model (determinism, principle 2).
    ///
    /// `model` is the live head model; the seam borrows it read-only and never
    /// mutates it. The reply is an [`Ack`](crate::control::ControlFrame::Ack) for
    /// every arm except `InspectModel`, which replies with the snapshot.
    pub fn apply_control_frame<Model, Msg, FView>(
        &mut self,
        frame: crate::control::ControlFrame,
        view: &FView,
        model: &Model,
        dbg: &mut TuiDebugger<Msg, Model>,
    ) -> ApplyOutcome<Msg>
    where
        Model: Clone,
        Msg: Clone + IpeStringify,
        FView: Fn(Model) -> CellsView<Msg>,
    {
        use crate::control::{ControlFrame, DebugCmd};
        match frame {
            ControlFrame::HotAppearance(patch) => {
                // Register the appearance overlay only where the mechanism is
                // compiled in: the `LiteralTable` overlay lives in the `web-core`
                // module, and a tui view routes its literals through it only when
                // the emit shape uses web. Absent that, the recompute below still
                // runs the contract (recompute-from-current-model + repaint) and
                // is byte-identical — visually inert until a per-tui literal table
                // lands, never a full rebuild.
                #[cfg(feature = "web-core")]
                crate::web::literal_table::register_dev_patch(&patch.defaults, patch.patch.clone());
                #[cfg(not(feature = "web-core"))]
                let _ = &patch; // no overlay mechanism in this build; recompute still runs

                // Recompute the surface from the CURRENT model — NOT through
                // `update`. The scrub cursor is untouched, so a hot-swap while
                // time-travelling repaints the pinned step, not the live head.
                let display = dbg.current_reconstructed().unwrap_or_else(|| model.clone());
                let (annotated, fs) = render_debug_annotated(
                    view,
                    display,
                    dbg,
                    &mut self.inputs,
                    self.focus_idx,
                    self.scroll_y,
                );
                let repaint = self.diff_repaint(annotated);
                ApplyOutcome {
                    reply: ControlFrame::Ack {
                        ok: true,
                        detail: "hot-appearance applied".to_owned(),
                    },
                    focusables: repaint.as_ref().map(|_| fs),
                    repaint,
                }
            }
            ControlFrame::Debug(cmd) => self.apply_debug(cmd, view, model, dbg),
            // `Ack` / `ModelSnapshot` are child→parent REPLIES, never a command
            // the child applies. Receiving one is a malformed control exchange:
            // fail closed with a rejecting `Ack` and no repaint (an exhaustive
            // match — a new parent→child variant must be handled here, never
            // silently swallowed).
            ControlFrame::Ack { .. } | ControlFrame::ModelSnapshot { .. } => ApplyOutcome {
                reply: ControlFrame::Ack {
                    ok: false,
                    detail: "not a parent-to-child command".to_owned(),
                },
                repaint: None,
                focusables: None,
            },
        }
    }

    /// The `Debug` arm of the seam (split out for readability). Drives the
    /// recorder's scrub cursor / inspection and shares the diff-repaint tail.
    fn apply_debug<Model, Msg, FView>(
        &mut self,
        cmd: crate::control::DebugCmd,
        view: &FView,
        model: &Model,
        dbg: &mut TuiDebugger<Msg, Model>,
    ) -> ApplyOutcome<Msg>
    where
        Model: Clone,
        Msg: Clone + IpeStringify,
        FView: Fn(Model) -> CellsView<Msg>,
    {
        use crate::control::{ControlFrame, DebugCmd};

        // `InspectModel` is read-only — it never moves the cursor and it renders
        // its own reply frame rather than repainting the live surface.
        if let DebugCmd::InspectModel(n) = cmd {
            // An empty history has no step to reconstruct — reply with the live
            // head at the requested index rather than fail.
            let (step, mdl) = match dbg.reconstruct_at(n) {
                Some(pair) => pair,
                None => (n, model.clone()),
            };
            // Render the model at step `n` to its frame — the tui rendering of
            // "the model at step n" (the surface has no `IpeStringify` bound on
            // `Model`, so its view frame is the faithful, bound-free snapshot).
            let (rendered, _fs) = render_debug_annotated(
                view,
                mdl,
                dbg,
                &mut self.inputs,
                self.focus_idx,
                self.scroll_y,
            );
            return ApplyOutcome {
                reply: ControlFrame::ModelSnapshot { step, rendered },
                repaint: None,
                focusables: None,
            };
        }

        // The cursor-moving / mode arms all reconstruct a model to display and
        // share the repaint tail below.
        let (display, detail) = match cmd {
            DebugCmd::StepTo(n) => (dbg.step_to(n), "scrub: step-to"),
            DebugCmd::Back => (dbg.step_back(), "scrub: back"),
            DebugCmd::Forward => (dbg.step_fwd(), "scrub: forward"),
            DebugCmd::Reset | DebugCmd::LiveTail => {
                // Return to the live head: leave scrub mode and repaint the live
                // model. (`Reset`'s recorder-fork semantics need the caller's
                // `init` model and a live-driver reset, which the wire cannot
                // carry; the in-process apply resolves both to "resume live".)
                dbg.live_tail();
                (None, "live-tail")
            }
            // `InspectModel` handled above.
            DebugCmd::InspectModel(_) => (None, "inspect"),
        };
        // A cursor arm that reconstructed a step displays it; otherwise (live
        // arms, or an empty history) display the live head.
        let display = display.unwrap_or_else(|| model.clone());
        let (annotated, fs) = render_debug_annotated(
            view,
            display,
            dbg,
            &mut self.inputs,
            self.focus_idx,
            self.scroll_y,
        );
        let repaint = self.diff_repaint(annotated);
        ApplyOutcome {
            reply: ControlFrame::Ack {
                ok: true,
                detail: detail.to_owned(),
            },
            focusables: repaint.as_ref().map(|_| fs),
            repaint,
        }
    }
}

/// Drive a keyboard-derived debugger command through the ONE apply seam, then
/// paint whatever repaint it requested. The keyboard scrub sites and the (later)
/// wire handler both funnel through [`TuiSurface::apply_control_frame`], so there
/// is exactly one apply path — never a keyboard path and a wire path that can
/// drift. Returns the freshly-rendered focusables, or the caller's current set
/// when the frame was unchanged.
///
/// The transient `TuiSurface` starts with an empty diff baseline, so a keyboard
/// step always paints — matching the pre-seam per-keypress repaint exactly; the
/// diff guard's dedup pays off on the wire path, where redundant frames recur.
#[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
#[allow(clippy::too_many_arguments)] // threads the loop's render surface + seam inputs
fn keyboard_scrub<Model, Msg, FView>(
    cmd: crate::control::DebugCmd,
    view: &FView,
    model: &Model,
    dbg: &mut TuiDebugger<Msg, Model>,
    inputs: &mut InputRegistry,
    focus_idx: usize,
    scroll_y: usize,
    current: Vec<Focusable<Msg>>,
) -> Vec<Focusable<Msg>>
where
    Model: Clone,
    Msg: Clone + IpeStringify,
    FView: Fn(Model) -> CellsView<Msg>,
{
    let mut surface = TuiSurface::new(std::mem::take(inputs), focus_idx, scroll_y);
    let outcome =
        surface.apply_control_frame(crate::control::ControlFrame::Debug(cmd), view, model, dbg);
    // Restore the (possibly edited) input registry to the loop's owner.
    *inputs = std::mem::take(&mut surface.inputs);
    if let Some(frame) = outcome.repaint {
        paint(&frame);
    }
    outcome.focusables.unwrap_or(current)
}

/// `Tui.tea` — terminal TEA driver for a `view : Model -> Cells msg`.
/// The `Cells msg` value wraps the same structured `Element` tree that `Ipe.Web`
/// renders; here it is laid out to ANSI cells by walking the typed attributes
/// (`tui::layout`), and `Ipe.Ui.Input.*` widgets become focusables. Tab /
/// Shift-Tab cycle focus; typing edits the focused text input (dispatching its
/// `onInput`); Enter/Space activates a button or toggles a checkbox/radio; the
/// view auto-scrolls to keep the focused element on screen. Ctrl-keys and any
/// unhandled key fall through to the user's `onKey`.
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
    Msg: Clone + Send + IpeStringify + 'static,
    FInit: Fn(()) -> (Model, IpeCmd<Msg>) + Send + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, IpeCmd<Msg>) + Send + Sync + 'static,
    FView: Fn(Model) -> CellsView<Msg> + Send + 'static,
    FSubs: Fn(Model) -> IpeSub<Msg> + Send + 'static,
    FOnKey: Fn(String, String) -> Msg + Send + 'static,
{
    // Wrap update in Arc — same rationale as tui_app.
    let update = std::sync::Arc::new(update);
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

        #[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
        let mut dbg = {
            let upd = std::sync::Arc::clone(&update);
            TuiDebugger::new(model.clone(), move |msg, mdl| upd(msg, mdl))
        };

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
                    // Debugger key intercept — must come before any app key handling.
                    #[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
                    {
                        // Ctrl-T: toggle time-travel mode — routed through the ONE
                        // apply seam. Entering pins the head step (`StepTo` of the
                        // last index, clamped); leaving resumes the live tail.
                        if kind == crate::debugger::tui::TOGGLE_KIND
                            && value == crate::debugger::tui::TOGGLE_VALUE
                        {
                            let cmd = if dbg.is_scrubbing() {
                                crate::control::DebugCmd::LiveTail
                            } else {
                                crate::control::DebugCmd::StepTo(usize::MAX)
                            };
                            focusables = keyboard_scrub(
                                cmd,
                                &view,
                                &model,
                                &mut dbg,
                                &mut inputs,
                                focus_idx,
                                scroll_y,
                                focusables,
                            );
                            continue;
                        }
                        // Ctrl-Left / Ctrl-Right: step in time-travel mode — same
                        // seam as Ctrl-T and (later) the wire, so a step can never
                        // diverge from a wire-driven `Back`/`Forward`.
                        if dbg.is_scrubbing() {
                            let cmd = if kind == crate::debugger::tui::STEP_BACK_KIND {
                                Some(crate::control::DebugCmd::Back)
                            } else if kind == crate::debugger::tui::STEP_FWD_KIND {
                                Some(crate::control::DebugCmd::Forward)
                            } else {
                                None
                            };
                            if let Some(cmd) = cmd {
                                focusables = keyboard_scrub(
                                    cmd,
                                    &view,
                                    &model,
                                    &mut dbg,
                                    &mut inputs,
                                    focus_idx,
                                    scroll_y,
                                    focusables,
                                );
                                continue;
                            }
                        }
                    }
                    // Mouse: wheel scrolls the viewport; a left-press focuses the
                    // hit element and activates it (if not an input).
                    if kind == "mouse" {
                        if let Some((btn, mcol, mrow, press)) = parse_mouse(&value) {
                            if press && (btn == 64 || btn == 65) {
                                let (cols, rows) = term_size();
                                let (_f, _fs, content_h) = render_with_focus(
                                    &view(model.clone()).into_element(),
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
                                    &view(model.clone()).into_element(),
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
                                    // change — same as Tab nav .
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
                            // onBlur (old) + onFocus (new) —  tuiDispatchFocusChange.
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
                #[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
                let (next, cmd) = update(msg.clone(), model);
                #[cfg(not(all(feature = "debugger", not(target_arch = "wasm32"))))]
                let (next, cmd) = update(msg, model);

                model = next;

                #[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
                dbg.record(msg, model.clone());

                cli_run_cmd(cmd, &tx);
                submgr.update(subscriptions(model.clone()));

                #[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]
                {
                    // In time-travel mode: freeze on the pinned past step.
                    // Toggle Ctrl-T to return to the live head.
                    let display_model =
                        dbg.current_reconstructed().unwrap_or_else(|| model.clone());
                    let (annotated, fs) = render_debug_annotated(
                        &view,
                        display_model,
                        &dbg,
                        &mut inputs,
                        focus_idx,
                        scroll_y,
                    );
                    paint(&annotated);
                    focusables = fs;
                }
                #[cfg(not(all(feature = "debugger", not(target_arch = "wasm32"))))]
                {
                    focusables =
                        render_and_paint(&view, &model, &mut inputs, &mut focus_idx, &mut scroll_y);
                }
            }
        }
        submgr.stop_all();
        ok_res(())
    })
}

// ── The apply-seam tests ────────────────────────────────────────────────────
//
// Every test CONSTRUCTS `ControlFrame`s directly — no transport, no socket — so
// it pins the in-process apply/scrub/inspect behavior and the single-seam
// invariant independently of any wire.
#[cfg(all(test, feature = "debugger", not(target_arch = "wasm32")))]
mod apply_seam_tests {
    use super::*;
    use crate::control::{ControlFrame, DebugCmd};
    use crate::tea::IpeCmd;

    #[derive(Clone, Debug, PartialEq)]
    enum TMsg {
        Add(i64),
    }

    impl IpeStringify for TMsg {
        fn ipe_show(&self) -> String {
            match self {
                TMsg::Add(n) => format!("Add({n})"),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq)]
    struct TModel {
        count: i64,
    }

    fn t_update(msg: TMsg, model: TModel) -> (TModel, IpeCmd<TMsg>) {
        let TMsg::Add(n) = msg;
        (
            TModel {
                count: model.count + n,
            },
            IpeCmd::None,
        )
    }

    // The view renders the model's count as text, so distinct models produce
    // distinct frames — the property the diff-repaint and reconstruct tests rely
    // on.
    fn t_view(model: TModel) -> CellsView<TMsg> {
        super::super::cells_text_(format!("count={}", model.count))
    }

    // A debugger seeded with N recorded steps folded from `t_update`, plus the
    // live head model. Step `i` records the msg and the model AFTER applying it.
    fn seeded(msgs: &[i64]) -> (TuiDebugger<TMsg, TModel>, TModel) {
        let mut dbg = TuiDebugger::new(TModel { count: 0 }, t_update);
        let mut live = TModel { count: 0 };
        for &n in msgs {
            let (next, _) = t_update(TMsg::Add(n), live.clone());
            dbg.record(TMsg::Add(n), next.clone());
            live = next;
        }
        (dbg, live)
    }

    // (a) A HotAppearance frame recomputes from the CURRENT model and repaints via
    // the diff guard — an inert patch (no literal-table mechanism in a tui build)
    // reproduces the identical frame, so the SECOND apply requests NO repaint (not
    // a full-screen redraw) — and focus/scroll are preserved across both.
    #[test]
    fn hot_appearance_diff_repaints_and_preserves_input_state() {
        let (mut dbg, live) = seeded(&[10, 5]);
        let mut surface = TuiSurface::new(InputRegistry::new(), 3, 7);

        let patch = crate::control::AppearancePatch::default();
        let first = surface.apply_control_frame(
            ControlFrame::HotAppearance(patch.clone()),
            &t_view,
            &live,
            &mut dbg,
        );
        assert!(
            matches!(first.reply, ControlFrame::Ack { ok: true, .. }),
            "hot-appearance replies Ack ok"
        );
        assert!(
            first.repaint.is_some(),
            "the first apply establishes the frame (a repaint)"
        );

        // Second identical apply: the recomputed frame equals the baseline, so the
        // diff guard requests NO paint — minimal repaint, never a full redraw.
        let second = surface.apply_control_frame(
            ControlFrame::HotAppearance(patch),
            &t_view,
            &live,
            &mut dbg,
        );
        assert!(
            second.repaint.is_none(),
            "an unchanged surface repaints nothing (no full-screen redraw)"
        );
        // Input state preserved across the appearance apply.
        assert_eq!(surface.focus_idx(), 3, "focus preserved");
        assert_eq!(surface.scroll_y(), 7, "scroll preserved");
    }

    // (b) StepTo / Back / Forward move the scrub cursor and reconstruct the model
    // at that step; the reconstructed model equals the live model at step n.
    #[test]
    fn debug_scrub_reconstructs_the_step_model() {
        let (mut dbg, live) = seeded(&[10, 5, 3]); // steps: 10, 15, 18
        let mut surface = TuiSurface::new(InputRegistry::new(), 0, 0);

        // StepTo(0) → model after the first msg (count = 10).
        let out = surface.apply_control_frame(
            ControlFrame::Debug(DebugCmd::StepTo(0)),
            &t_view,
            &live,
            &mut dbg,
        );
        assert!(matches!(out.reply, ControlFrame::Ack { ok: true, .. }));
        assert!(out.repaint.is_some(), "a scrub to a new step repaints");
        // reconstruct(0) == the model at step 0.
        assert_eq!(
            dbg.current_reconstructed(),
            Some(TModel { count: 10 }),
            "StepTo(0) reconstructs the step-0 model"
        );

        // Forward → step 1 (count = 15).
        let _ = surface.apply_control_frame(
            ControlFrame::Debug(DebugCmd::Forward),
            &t_view,
            &live,
            &mut dbg,
        );
        assert_eq!(dbg.current_reconstructed(), Some(TModel { count: 15 }));

        // Back → step 0 (count = 10).
        let _ = surface.apply_control_frame(
            ControlFrame::Debug(DebugCmd::Back),
            &t_view,
            &live,
            &mut dbg,
        );
        assert_eq!(dbg.current_reconstructed(), Some(TModel { count: 10 }));
    }

    // (c) InspectModel(n) returns the snapshot of the model at step n WITHOUT
    // moving the cursor; LiveTail restores live mode.
    #[test]
    fn inspect_model_snapshots_and_live_tail_restores_live() {
        let (mut dbg, live) = seeded(&[10, 5, 3]);
        let mut surface = TuiSurface::new(InputRegistry::new(), 0, 0);

        // Enter scrub at step 0 first, so we can prove Inspect does not move it.
        let _ = surface.apply_control_frame(
            ControlFrame::Debug(DebugCmd::StepTo(0)),
            &t_view,
            &live,
            &mut dbg,
        );
        assert!(dbg.is_scrubbing());

        let out = surface.apply_control_frame(
            ControlFrame::Debug(DebugCmd::InspectModel(1)),
            &t_view,
            &live,
            &mut dbg,
        );
        let ControlFrame::ModelSnapshot { step, rendered } = out.reply else {
            assert!(false, "InspectModel must reply ModelSnapshot");
            return;
        };
        assert_eq!(step, 1, "the snapshot reflects the requested step");
        assert!(
            rendered.contains("count=15"),
            "the snapshot renders the step-1 model (count=15); got: {rendered:?}"
        );
        assert!(
            out.repaint.is_none(),
            "an inspection is read-only — it does not repaint the live surface"
        );
        // Cursor unmoved by the inspection.
        assert_eq!(
            dbg.current_reconstructed(),
            Some(TModel { count: 10 }),
            "InspectModel must not move the scrub cursor"
        );

        // LiveTail leaves scrub mode.
        let out = surface.apply_control_frame(
            ControlFrame::Debug(DebugCmd::LiveTail),
            &t_view,
            &live,
            &mut dbg,
        );
        assert!(matches!(out.reply, ControlFrame::Ack { ok: true, .. }));
        assert!(!dbg.is_scrubbing(), "LiveTail restores live mode");
    }

    // (d) The keyboard path and the frame path drive the IDENTICAL seam: a
    // Ctrl-Left keypress (folded to `Back`) and a directly-constructed
    // `Debug(Back)` frame produce the same rendered frame and the same cursor.
    #[test]
    fn keyboard_path_equals_frame_path() {
        // Frame path: seed, step to the tail, then Back one step.
        let (mut dbg_f, live_f) = seeded(&[10, 5, 3]);
        let mut surface_f = TuiSurface::new(InputRegistry::new(), 0, 0);
        let _ = surface_f.apply_control_frame(
            ControlFrame::Debug(DebugCmd::StepTo(2)),
            &t_view,
            &live_f,
            &mut dbg_f,
        );
        let frame_out = surface_f.apply_control_frame(
            ControlFrame::Debug(DebugCmd::Back),
            &t_view,
            &live_f,
            &mut dbg_f,
        );

        // Keyboard path: same seeded state and cursor, driven through
        // `keyboard_scrub` (the shared keyboard entry point).
        let (mut dbg_k, live_k) = seeded(&[10, 5, 3]);
        let mut inputs = InputRegistry::new();
        let _ = dbg_k.step_to(2);
        let kb_focusables = keyboard_scrub(
            DebugCmd::Back,
            &t_view,
            &live_k,
            &mut dbg_k,
            &mut inputs,
            0,
            0,
            Vec::new(),
        );

        // Same reconstructed cursor after the Back step.
        assert_eq!(
            dbg_f.current_reconstructed(),
            dbg_k.current_reconstructed(),
            "keyboard and frame paths land on the same scrub step"
        );
        // Same rendered frame (the seam is the sole frame producer).
        let frame_frame = frame_out.repaint.expect("the frame path repaints");
        let display_k = dbg_k
            .current_reconstructed()
            .unwrap_or_else(|| live_k.clone());
        let (kb_frame, _) = render_debug_annotated(&t_view, display_k, &dbg_k, &mut inputs, 0, 0);
        assert_eq!(
            frame_frame, kb_frame,
            "keyboard and frame paths render the identical frame"
        );
        assert_eq!(
            kb_focusables.len(),
            frame_out.focusables.map(|f| f.len()).unwrap_or(0),
            "both paths yield the same focusable set"
        );
    }
}

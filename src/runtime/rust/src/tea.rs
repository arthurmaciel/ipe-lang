//! Ipê TEA runtime core — Cmd/Sub + the Ipe.Console line-oriented loop.
//!
//! Cmd/Sub are generic over the message type M (NOT `any`): the intermediate
//! value `a` in `Cmd.perform` is erased inside a boxed M-producing future, but M
//! stays concrete. Step 1 (this file) ships the types, the simple kernels, and a
//! blocking Cli.app loop (stdin -> onLine -> update -> view). Sub.every
//! tickers + async Cmd.perform delivery land in steps 2-3 (a subManager + an
//! mpsc msg channel + tokio::select over stdin and the channel).

use super::*;
use std::future::Future;
use std::pin::Pin;

/// Ipê `Cmd msg`. Perform carries a boxed thunk producing the message (the
/// task's success/error type is erased inside; M is concrete).
pub enum IpeCmd<M> {
    None,
    Batch(Vec<IpeCmd<M>>),
    Perform(PerformThunk<M>),
    /// pub/sub broadcast. The thunk receives the publishing session's sid (the
    /// origin), injected by the Web dispatch loop, and returns the subscriber
    /// count. Not generic over the payload type T — T is captured inside the
    /// thunk (the same erasure-free pattern as `Perform`'s boxed future).
    Publish(Box<dyn FnOnce(&str) -> i64 + Send>),
}

/// The boxed message-producing thunk inside [`IpeCmd::Perform`]. Same
/// cfg-split rationale as `IpeTask` (`core.rs`): wasm futures touch the DOM
/// and are `!Send`; the native bound backs `tokio::spawn`.
#[cfg(not(target_arch = "wasm32"))]
pub type PerformThunk<M> = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = M> + Send>> + Send>;
#[cfg(target_arch = "wasm32")]
pub type PerformThunk<M> = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = M>>>>;

/// A custom subscription event source: given an `emit` callback, spawn a task
/// that pushes messages into the loop, returning its JoinHandle (aborted on
/// re-subscribe). Keeps IpeSub decoupled from source-specific runtimes (e.g. the
/// WebSocket client builds one of these for `onMessage`).
#[cfg(not(target_arch = "wasm32"))]
pub type SubSpawn<M> =
    Box<dyn FnOnce(std::sync::Arc<dyn Fn(M) + Send + Sync>) -> tokio::task::JoinHandle<()> + Send>;
/// wasm: single-threaded, no tokio — a source registers its emit callback and
/// returns a teardown closure the scheduler runs on re-subscribe/unmount (the
/// wasm analogue of aborting the native `JoinHandle`). The M4 pub/sub broker
/// (`wasm::pubsub::sub_subscribe_topic`) is the first constructor of this
/// type on wasm; every source MUST return a real unregister thunk (never a
/// no-op) so the scheduler's stop-all-then-respawn cycle (mirroring native's
/// `SubManager::update`) cannot accumulate duplicate listeners.
#[cfg(target_arch = "wasm32")]
pub type SubSpawn<M> = Box<dyn FnOnce(std::rc::Rc<dyn Fn(M)>) -> Box<dyn FnOnce()>>;

/// Ipê `Sub msg`.
pub enum IpeSub<M> {
    None,
    Batch(Vec<IpeSub<M>>),
    Every { ms: i64, msg: M },
    Source(SubSpawn<M>),
}

// ─── Cmd kernels ──────────────────────────────────────────────────────────

pub fn cmd_none<M>() -> IpeCmd<M> {
    IpeCmd::None
}
pub fn cmd_batch<M>(list: Vec<IpeCmd<M>>) -> IpeCmd<M> {
    IpeCmd::Batch(list)
}

/// Cmd.perform : Task err a -> (Result err a -> msg) -> Cmd msg.
/// Composes the task and the toMsg decoder (which receives the IpeResult) into a
/// single message-producing thunk fired by the run loop.
#[cfg(not(target_arch = "wasm32"))]
pub fn cmd_perform<E, A, M, F>(task: IpeTask<E, A>, to_msg: F) -> IpeCmd<M>
where
    E: Send + 'static,
    A: Send + 'static,
    M: Send + 'static,
    F: FnOnce(IpeResult<E, A>) -> M + Send + 'static,
{
    IpeCmd::Perform(Box::new(move || {
        Box::pin(async move { to_msg(task.await) })
    }))
}

/// wasm: same composition, minus the `Send` bounds (single-threaded browser
/// event loop; the thunk is driven by `spawn_local`).
#[cfg(target_arch = "wasm32")]
pub fn cmd_perform<E, A, M, F>(task: IpeTask<E, A>, to_msg: F) -> IpeCmd<M>
where
    E: 'static,
    A: 'static,
    M: 'static,
    F: FnOnce(IpeResult<E, A>) -> M + 'static,
{
    IpeCmd::Perform(Box::new(move || {
        Box::pin(async move { to_msg(task.await) })
    }))
}

/// Cmd.map : (a -> msg) -> Cmd a -> Cmd msg — retag every message a command
/// would produce. Rebuilds the command tree, composing `f` over each leaf's
/// payload: `Perform`'s produced value is fed through `f`; `Batch` maps its
/// children; `None` passes through. `Publish` carries no `M`-typed payload (its
/// thunk yields the subscriber count `i64`; `M` is phantom there), so it is
/// re-tagged by identity — the one leaf where `f` is not applied.
///
/// `f` is shared (`Arc`) because a `Batch` fans it across children and a
/// `Perform` thunk captures it to run later; the composition stays lazy — no
/// message is produced until the run loop fires the thunk.
#[cfg(not(target_arch = "wasm32"))]
pub fn cmd_map<A, M, F>(cmd: IpeCmd<A>, f: F) -> IpeCmd<M>
where
    A: Send + 'static,
    M: Send + 'static,
    F: Fn(A) -> M + Send + Sync + 'static,
{
    cmd_map_arc(cmd, std::sync::Arc::new(f))
}

#[cfg(not(target_arch = "wasm32"))]
fn cmd_map_arc<A, M>(cmd: IpeCmd<A>, f: std::sync::Arc<dyn Fn(A) -> M + Send + Sync>) -> IpeCmd<M>
where
    A: Send + 'static,
    M: Send + 'static,
{
    match cmd {
        IpeCmd::None => IpeCmd::None,
        IpeCmd::Batch(items) => IpeCmd::Batch(
            items
                .into_iter()
                .map(|c| cmd_map_arc(c, f.clone()))
                .collect(),
        ),
        IpeCmd::Perform(thunk) => IpeCmd::Perform(Box::new(move || {
            Box::pin(async move {
                let a = thunk().await;
                f(a)
            })
        })),
        IpeCmd::Publish(thunk) => IpeCmd::Publish(thunk),
    }
}

/// wasm: same tree rebuild, `Rc`-shared `f`, no `Send`/`Sync` bounds
/// (single-threaded browser event loop). `Publish` re-tags by identity, as on
/// native.
#[cfg(target_arch = "wasm32")]
pub fn cmd_map<A, M, F>(cmd: IpeCmd<A>, f: F) -> IpeCmd<M>
where
    A: 'static,
    M: 'static,
    F: Fn(A) -> M + 'static,
{
    cmd_map_rc(cmd, std::rc::Rc::new(f))
}

#[cfg(target_arch = "wasm32")]
fn cmd_map_rc<A, M>(cmd: IpeCmd<A>, f: std::rc::Rc<dyn Fn(A) -> M>) -> IpeCmd<M>
where
    A: 'static,
    M: 'static,
{
    match cmd {
        IpeCmd::None => IpeCmd::None,
        IpeCmd::Batch(items) => IpeCmd::Batch(
            items
                .into_iter()
                .map(|c| cmd_map_rc(c, f.clone()))
                .collect(),
        ),
        IpeCmd::Perform(thunk) => IpeCmd::Perform(Box::new(move || {
            Box::pin(async move {
                let a = thunk().await;
                f(a)
            })
        })),
        IpeCmd::Publish(thunk) => IpeCmd::Publish(thunk),
    }
}

// ─── Sub kernels ──────────────────────────────────────────────────────────

pub fn sub_none<M>() -> IpeSub<M> {
    IpeSub::None
}
pub fn sub_batch<M>(list: Vec<IpeSub<M>>) -> IpeSub<M> {
    IpeSub::Batch(list)
}

/// Sub.every : Int -> msg -> Sub msg — dispatch `msg` every `ms` milliseconds.
pub fn sub_every<M>(ms: i64, msg: M) -> IpeSub<M> {
    IpeSub::Every { ms, msg }
}

/// Time.every : Int -> msg -> Sub msg — alias of `Sub.every` (matches
/// `Time_every`, which delegates to `Sub_every`). The `Time_every` kernel name
/// lowers to this.
pub fn time_every<M>(ms: i64, msg: M) -> IpeSub<M> {
    sub_every(ms, msg)
}

/// Sub.map : (a -> msg) -> Sub a -> Sub msg — retag every message a
/// subscription would deliver. Rebuilds the subscription tree: `Every`'s stored
/// `msg` is retagged eagerly (`f msg`); `Batch` maps its children; `None`
/// passes through; a `Source` is rewrapped so the emit callback it receives
/// first pushes each `a` through `f` before handing the resulting `msg` to the
/// scheduler's real emit — the source stays oblivious to the retagging and its
/// teardown handle is preserved unchanged.
#[cfg(not(target_arch = "wasm32"))]
pub fn sub_map<A, M, F>(sub: IpeSub<A>, f: F) -> IpeSub<M>
where
    A: Send + 'static,
    M: Send + 'static,
    F: Fn(A) -> M + Send + Sync + 'static,
{
    sub_map_arc(sub, std::sync::Arc::new(f))
}

#[cfg(not(target_arch = "wasm32"))]
fn sub_map_arc<A, M>(sub: IpeSub<A>, f: std::sync::Arc<dyn Fn(A) -> M + Send + Sync>) -> IpeSub<M>
where
    A: Send + 'static,
    M: Send + 'static,
{
    match sub {
        IpeSub::None => IpeSub::None,
        IpeSub::Batch(items) => IpeSub::Batch(
            items
                .into_iter()
                .map(|s| sub_map_arc(s, f.clone()))
                .collect(),
        ),
        IpeSub::Every { ms, msg } => IpeSub::Every { ms, msg: f(msg) },
        IpeSub::Source(spawn) => IpeSub::Source(Box::new(
            move |emit_outer: std::sync::Arc<dyn Fn(M) + Send + Sync>| {
                let emit_inner: std::sync::Arc<dyn Fn(A) + Send + Sync> =
                    std::sync::Arc::new(move |a| emit_outer(f(a)));
                spawn(emit_inner)
            },
        )),
    }
}

/// wasm: same tree rebuild, `Rc`-shared `f`, no `Send`/`Sync` bounds. The
/// `Source` rewrap preserves the source's teardown thunk unchanged.
#[cfg(target_arch = "wasm32")]
pub fn sub_map<A, M, F>(sub: IpeSub<A>, f: F) -> IpeSub<M>
where
    A: 'static,
    M: 'static,
    F: Fn(A) -> M + 'static,
{
    sub_map_rc(sub, std::rc::Rc::new(f))
}

#[cfg(target_arch = "wasm32")]
fn sub_map_rc<A, M>(sub: IpeSub<A>, f: std::rc::Rc<dyn Fn(A) -> M>) -> IpeSub<M>
where
    A: 'static,
    M: 'static,
{
    match sub {
        IpeSub::None => IpeSub::None,
        IpeSub::Batch(items) => IpeSub::Batch(
            items
                .into_iter()
                .map(|s| sub_map_rc(s, f.clone()))
                .collect(),
        ),
        IpeSub::Every { ms, msg } => IpeSub::Every { ms, msg: f(msg) },
        IpeSub::Source(spawn) => {
            IpeSub::Source(Box::new(move |emit_outer: std::rc::Rc<dyn Fn(M)>| {
                let emit_inner: std::rc::Rc<dyn Fn(A)> =
                    std::rc::Rc::new(move |a| emit_outer(f(a)));
                spawn(emit_inner)
            }))
        }
    }
}

// `Ipe.Http.Stream.chunks` → `Sub_subscribeStream` lives in `http_stream.rs`
// now (alongside the stream registry it drains + the bridged `ChunkEvent` enum).
// It returns a `IpeSub::Source` driven by this module's SubManager.

// ─── TEA event loop plumbing (Sub.every tickers + Cmd firing) ───────────────

/// Internal loop event: a raw stdin line (Cli), a decoded key as (kind, value)
/// (Tui — Strings keep this free of the feature-gated TuiKey type), a ticker or
/// subscription Msg, a resolved `Cmd.perform`/`Task.attempt` result, or EOF.
/// Shared by `console_app` and `tui_app` so both reuse `SubManager` (Tick) +
/// `cli_run_cmd`.
///
/// `PerformDone` and `Msg` carry the same payload but are kept distinct so the
/// Cli loop can tell a one-shot effect's result apart from an unbounded ticker
/// or subscription emission: only `PerformDone` counts toward the outstanding
/// one-shot effects that must be delivered before EOF may terminate the loop. A
/// ticker `Msg` can arrive forever and so must never keep the loop alive.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) enum CliEvent<M> {
    Line(String),
    // Constructed only by the `tui` raw-key reader; console_app matches it
    // defensively (keys are ignored under Cli). In a non-tui build the variant
    // is never constructed but must remain in the shared enum for that arm.
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    Key(String, String),
    Msg(M),
    PerformDone(M),
    Eof,
}

/// Tracks the goroutine-equivalent ticker tasks spawned for the active
/// `Sub.every` subscriptions. `update` stops all + respawns from the new Sub
/// (one program, one model, re-evaluated each tick).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct SubManager<M> {
    tx: tokio::sync::mpsc::UnboundedSender<CliEvent<M>>,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl<M: Clone + Send + 'static> SubManager<M> {
    pub(crate) fn new(tx: tokio::sync::mpsc::UnboundedSender<CliEvent<M>>) -> Self {
        SubManager {
            tx,
            handles: Vec::new(),
        }
    }
    pub(crate) fn stop_all(&mut self) {
        for h in self.handles.drain(..) {
            h.abort();
        }
    }
    pub(crate) fn update(&mut self, sub: IpeSub<M>) {
        self.stop_all();
        self.spawn(sub);
    }
    fn spawn(&mut self, sub: IpeSub<M>) {
        match sub {
            IpeSub::None => {}
            IpeSub::Batch(items) => {
                for it in items {
                    self.spawn(it);
                }
            }
            IpeSub::Every { ms, msg } => {
                if ms <= 0 {
                    return;
                }
                let tx = self.tx.clone();
                let dur = std::time::Duration::from_millis(ms as u64);
                // First tick after `ms` (sleep-loop, matching  time.After).
                let h = tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(dur).await;
                        if tx.send(CliEvent::Msg(msg.clone())).is_err() {
                            break;
                        }
                    }
                });
                self.handles.push(h);
            }
            IpeSub::Source(spawn) => {
                // Hand the source an emit callback that funnels Msgs into the loop.
                let tx = self.tx.clone();
                let emit: std::sync::Arc<dyn Fn(M) + Send + Sync> = std::sync::Arc::new(move |m| {
                    let _ = tx.send(CliEvent::Msg(m));
                });
                self.handles.push(spawn(emit));
            }
        }
    }
}

/// Fire a Cmd: None/Batch recurse; Perform spawns the composed task→toMsg thunk
/// and pushes the resulting Msg back into the loop channel. The Tui driver
/// (which exits on a quit key, not stdin EOF) does not track outstanding
/// effects, so it fires without a counter.
// Only the Tui driver fires Cmds untracked; the Cli loop uses the tracked
// variant directly. Gate this wrapper on the same feature as its sole caller so
// a Tui-less feature combo does not see it as dead code.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
pub(crate) fn cli_run_cmd<M: Send + 'static>(
    cmd: IpeCmd<M>,
    tx: &tokio::sync::mpsc::UnboundedSender<CliEvent<M>>,
) {
    cli_run_cmd_tracked(cmd, tx, None);
}

/// As `cli_run_cmd`, but when `outstanding` is `Some`, each spawned `Perform`
/// increments it before spawning and its result is delivered as a
/// `CliEvent::PerformDone` (the Cli loop decrements the counter on dequeue).
/// This lets the Cli loop keep running past stdin EOF until every one-shot
/// effect an `init`/`update` issued has delivered its Msg — without letting
/// unbounded ticker/subscription `Msg`s (which never touch the counter) keep
/// the loop alive.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn cli_run_cmd_tracked<M: Send + 'static>(
    cmd: IpeCmd<M>,
    tx: &tokio::sync::mpsc::UnboundedSender<CliEvent<M>>,
    outstanding: Option<&std::sync::Arc<std::sync::atomic::AtomicUsize>>,
) {
    match cmd {
        IpeCmd::None => {}
        IpeCmd::Batch(items) => {
            for c in items {
                cli_run_cmd_tracked(c, tx, outstanding);
            }
        }
        IpeCmd::Perform(thunk) => {
            let tx = tx.clone();
            // A tracked Perform is counted as outstanding at spawn and delivers a
            // `PerformDone`; an untracked one (Tui) delivers a plain `Msg`.
            let counter = outstanding.cloned();
            if let Some(c) = &counter {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            // Fire-and-forget: a panic inside the composed task→toMsg thunk aborts
            // only this task and is intentionally swallowed — that is the
            // Task-boundary recover contract (an effectful task that faults must
            // not crash the TEA loop). On a fault the JoinHandle is dropped and
            // no event is sent; the Cli loop's counter would then never be
            // decremented for this effect, so the spawned task decrements the
            // counter on the fault path (drop guard) to preserve the EOF
            // invariant. Structured-warn observability on this path is a known
            // follow-up (would require awaiting the JoinHandle's JoinError).
            tokio::spawn(async move {
                // Decrement on any exit from this task (normal or panic-unwind)
                // so a faulting effect can never wedge the EOF-drain invariant.
                struct OutstandingGuard(Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>);
                impl Drop for OutstandingGuard {
                    fn drop(&mut self) {
                        if let Some(c) = &self.0 {
                            c.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                }
                match counter {
                    Some(c) => {
                        // Counter decremented by the loop on `PerformDone` dequeue
                        // (below) for the delivered case; the guard covers only the
                        // panic-unwind case where no event reaches the loop.
                        let guard = OutstandingGuard(Some(c));
                        let msg = thunk().await;
                        std::mem::forget(guard); // delivered → loop owns the decrement
                        // A send failure means the loop already exited (rx
                        // dropped); the leaked count is then unobservable, so it
                        // is intentionally not decremented here.
                        let _ = tx.send(CliEvent::PerformDone(msg));
                    }
                    None => {
                        let msg = thunk().await;
                        let _ = tx.send(CliEvent::Msg(msg));
                    }
                }
            });
        }
        IpeCmd::Publish(thunk) => {
            // No Web session in a Cli program; publish with an empty origin
            // (no subscriber's owner_sid matches "" → echo-default no-op).
            let _ = thunk("");
        }
    }
}

// ─── Ipe.Terminal — line-oriented TEA loop ─────────────────────────────────────

/// Cli.app { init, update, view, subscriptions, onLine } : Task Error ().
///
/// init -> fire cmd -> subs -> view; then fold each event (stdin line via
/// onLine, ticker/Cmd.perform Msg) through update -> re-fire cmd -> re-subs ->
/// view, until stdin EOF. Stdin is read on a blocking task; tickers + perform
/// results merge into the same single-threaded update sequence via one channel.
#[cfg(not(target_arch = "wasm32"))]
pub fn console_app<Model, Msg, E, FInit, FUpdate, FView, FSubs, FOnLine>(
    init: FInit,
    update: FUpdate,
    view: FView,
    subscriptions: FSubs,
    on_line: FOnLine,
) -> IpeTask<E, ()>
where
    E: Send + 'static,
    Model: Clone + Send + 'static,
    Msg: Clone + Send + 'static,
    FInit: Fn(()) -> (Model, IpeCmd<Msg>) + Send + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, IpeCmd<Msg>) + Send + 'static,
    FView: Fn(Model) -> String + Send + 'static,
    FSubs: Fn(Model) -> IpeSub<Msg> + Send + 'static,
    FOnLine: Fn(String) -> Msg + Send + 'static,
{
    Box::pin(async move {
        use std::io::Write;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<CliEvent<Msg>>();

        // Blocking stdin reader → raw Line events, then Eof. onLine is applied in
        // the main task (keeps it off the blocking thread / out of Send bounds).
        //
        // KNOWN LEAK (intentional, bounded): this detached thread is never joined
        // or signalled — if the returned future is dropped/cancelled the thread
        // stays parked on `lines()` until the next stdin line (or process exit).
        // Benign for a one-shot Cli `main` (the process is exiting anyway); a
        // shutdown flag wouldn't help since the read blocks until the next line
        // regardless. Do NOT compose `console_app` under a cancelling parent or
        // invoke it twice in one process without first accounting for this.
        let line_tx = tx.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(l) => {
                        if line_tx.send(CliEvent::Line(l)).is_err() {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = line_tx.send(CliEvent::Eof);
        });

        // Count of one-shot `Perform` effects that were issued but whose Msg has
        // not yet been folded through `update`. EOF must not terminate the loop
        // while this is non-zero, or an init/update-issued effect's result would
        // be silently dropped on empty/early-closing stdin.
        let outstanding = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut eof_seen = false;

        let (mut model, cmd0) = init(());
        cli_run_cmd_tracked(cmd0, &tx, Some(&outstanding));
        let mut submgr = SubManager::new(tx.clone());
        submgr.update(subscriptions(model.clone()));
        // Inline render (a closure borrowing `view` would make the future non-Send).
        // Fallible writes (NOT print!/println!, which panic on a broken pipe).
        //
        // A render must NOT force a trailing "\n": that would diverge from the
        // the spec. ``'s `cliPrintView` is explicit that
        // it "writes the result to stdout WITHOUT a trailing newline (the
        // user's prompt formatting decides whether to add one)" — runtime only ever
        // appends ONE newline, at `fmt.Println()` after the event loop exits
        // (same as here). This is intentional REPL-prompt design:
        // `examples/shapes/terminal/simple-counter`'s `view` returns
        // `"count=" ++ ... ++ "  (+, -, r, q) > "` with NO trailing newline so
        // the cursor stays on the prompt line for the user's input. An app that
        // wants each render on its own line supplies its own trailing "\n"
        // in its `view` string — see `tests/golden/console_app_view_separator`
        // for a fixture that deliberately does NOT do this and therefore glues
        // renders together.
        //
        // The initial render is skipped when `init` issued an outstanding
        // effect: that effect's Msg folds through `update` and renders the
        // settled model below, so an eager render here would paint the
        // pre-effect model and duplicate the frame. An effect-free `init`
        // (`Cmd.none`) has nothing to settle, so its initial model renders now
        // (the `lines: 0` frame the separator fixture pins).
        if outstanding.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            let _ = std::io::stdout().write_all(view(model.clone()).as_bytes());
            let _ = std::io::stdout().flush();
        }

        while let Some(ev) = rx.recv().await {
            let msg = match ev {
                CliEvent::Line(l) => on_line(l),
                CliEvent::Key(_, _) => continue, // Cli has no keys
                CliEvent::Msg(m) => m,
                CliEvent::PerformDone(m) => {
                    // A one-shot effect delivered its result: this effect is no
                    // longer outstanding. If EOF already arrived and this was the
                    // last outstanding effect, fold it and then let EOF terminate.
                    outstanding.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    m
                }
                CliEvent::Eof => {
                    // EOF terminates only once every outstanding one-shot effect
                    // has delivered. If effects are still in flight, remember EOF
                    // and keep folding their results; the check after each fold
                    // (below) breaks once the count reaches zero.
                    if outstanding.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                        break;
                    }
                    eof_seen = true;
                    continue;
                }
            };
            let (next, cmd) = update(msg, model);
            model = next;
            cli_run_cmd_tracked(cmd, &tx, Some(&outstanding));
            submgr.update(subscriptions(model.clone()));
            let _ = std::io::stdout().write_all(view(model.clone()).as_bytes());
            let _ = std::io::stdout().flush();
            // After folding an effect's result, if EOF was already seen and no
            // effects remain outstanding, terminate as EOF would have.
            if eof_seen && outstanding.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                break;
            }
        }
        submgr.stop_all();
        let _ = std::io::stdout().write_all(b"\n");
        ok_res(())
    })
}

// ─── Shape opaque app-leaf types ──────────────────────────────────────────
//
// Each entry builder (`Web.app`, `Tui.app`, `Cli.app`) returns one of these
// opaque handles instead of
// `IpeTask<E, ()>`. The handle wraps the underlying task and exposes a single
// `run_blocking` method consumed by the emitted `fn main()`. This erases the
// msg/model type parameters from the program's `main` type signature while
// keeping the emit path concrete (no `dyn`).

/// A concrete-erased builder that, given a mount base-path prefix, produces the
/// embedded web app's fully-layered axum `Router` (the same router the
/// standalone `serve_web` binds). Boxed so `WebApp` stays non-generic — the box
/// is over the *builder*, NOT the app's handlers: `init`/`update`/`view`/`subs`
/// are concrete monomorphised closures captured inside, so the mounted app is
/// erased-free at the handler level (§9: no `dyn` over the app / handlers).
#[cfg(all(not(target_arch = "wasm32"), feature = "web"))]
pub type MountBuilder = Box<
    dyn FnOnce(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = axum::Router> + Send>>
        + Send,
>;

/// Opaque app handle returned by `Web.app` / `Web.appRouted` / `Web.appWith`
/// (standalone) or `Web.embed` (mountable). The `WebApp(...)` tuple form is the
/// leaf-constructor the backend's shape-app entry switch detects; the inner
/// [`WebAppKind`] selects the run mode.
#[cfg(not(target_arch = "wasm32"))]
pub struct WebApp(pub WebAppKind);

/// The two run modes a `WebApp` leaf can carry.
///
/// * `Standalone` — from `Web.app`: a fully-built server task that binds its
///   own listener. `run_blocking` drives it.
/// * `Mountable` — from `Web.embed`: carries BOTH a standalone `serve` task (so
///   a top-level `main = Web.embed { … }` still runs on its own port) AND a
///   `router` builder that `Server.mountApp` nests under a prefix on the shared
///   server port (one listener).
#[cfg(not(target_arch = "wasm32"))]
pub enum WebAppKind {
    Standalone(IpeTask<crate::error::IpeError, ()>),
    #[cfg(feature = "web")]
    Mountable {
        serve: IpeTask<crate::error::IpeError, ()>,
        router: MountBuilder,
    },
}

#[cfg(not(target_arch = "wasm32"))]
impl WebApp {
    /// Blocking entry: drives the underlying task to completion on a
    /// fresh tokio runtime. Returns the task's `IpeResult`. A mountable handle
    /// used top-level runs its standalone `serve` task (binds its own port).
    pub fn run_blocking(self) -> crate::IpeResult<crate::error::IpeError, ()> {
        match self.0 {
            WebAppKind::Standalone(task) => crate::task::block_on(task),
            #[cfg(feature = "web")]
            WebAppKind::Mountable { serve, .. } => crate::task::block_on(serve),
        }
    }

    /// Take the mount router-builder, if this is an embedded (mountable) handle.
    /// `Server.mountApp` calls this; a `Web.app` (standalone) handle yields
    /// `None`, which the mount path turns into a fail-closed diagnostic route
    /// (unreachable for well-typed source: `mountApp` only accepts `Web.embed`
    /// / `Web.app` handles, and `Web.app` handles are still mountable-capable
    /// only via `embed`).
    #[cfg(feature = "web")]
    pub fn into_mount_builder(self) -> Option<MountBuilder> {
        match self.0 {
            WebAppKind::Mountable { router, .. } => Some(router),
            WebAppKind::Standalone(_) => None,
        }
    }
}

/// Opaque app handle for the webview-native host of a `Web.app` (a `web desktop`
/// delivery). Backed by a boxed `IpeTask<IpeError, ()>`; run via `run_blocking` on
/// the current thread (tao/Cocoa mandates the process main thread on macOS).
#[cfg(not(target_arch = "wasm32"))]
pub struct WebViewApp(pub IpeTask<crate::error::IpeError, ()>);

#[cfg(not(target_arch = "wasm32"))]
impl WebViewApp {
    /// Blocking entry on the CURRENT thread (required by tao/Cocoa on macOS).
    pub fn run_blocking(self) -> crate::IpeResult<crate::error::IpeError, ()> {
        crate::task::block_on_current_thread(self.0)
    }
}

/// Opaque app handle returned by `Tui.app`.
/// Backed by a boxed `IpeTask<IpeError, ()>`; run via `run_blocking`.
#[cfg(not(target_arch = "wasm32"))]
pub struct TuiApp(pub IpeTask<crate::error::IpeError, ()>);

#[cfg(not(target_arch = "wasm32"))]
impl TuiApp {
    /// Blocking entry: drives the underlying task to completion.
    pub fn run_blocking(self) -> crate::IpeResult<crate::error::IpeError, ()> {
        crate::task::block_on(self.0)
    }
}

/// Opaque app handle returned by `Cli.app`.
/// Backed by a boxed `IpeTask<IpeError, ()>`; run via `run_blocking`.
#[cfg(not(target_arch = "wasm32"))]
pub struct CliApp(pub IpeTask<crate::error::IpeError, ()>);

#[cfg(not(target_arch = "wasm32"))]
impl CliApp {
    /// Blocking entry: drives the underlying task to completion.
    pub fn run_blocking(self) -> crate::IpeResult<crate::error::IpeError, ()> {
        crate::task::block_on(self.0)
    }
}

// ─── Cmd.map / Sub.map unit tests ──────────────────────────────────────────

#[cfg(all(test, not(target_arch = "wasm32")))]
mod map_tests {
    use super::*;

    // Two distinct message types so the retag is observable at the type level:
    // `sub_map`/`cmd_map` carry `Child` into `Parent`.
    #[derive(Clone, Debug, PartialEq)]
    enum Child {
        Tick(i64),
    }
    #[derive(Clone, Debug, PartialEq)]
    enum Parent {
        FromChild(Child),
    }

    fn wrap(c: Child) -> Parent {
        Parent::FromChild(c)
    }

    #[test]
    fn sub_map_every_retags_stored_msg() {
        let mapped = sub_map(sub_every(50, Child::Tick(7)), wrap);
        match mapped {
            IpeSub::Every { ms, msg } => {
                assert_eq!(ms, 50);
                assert_eq!(msg, Parent::FromChild(Child::Tick(7)));
            }
            _ => panic!("expected Every"),
        }
    }

    #[test]
    fn sub_map_batch_and_none_recurse() {
        let mapped = sub_map(
            sub_batch(vec![sub_every(1, Child::Tick(1)), sub_none()]),
            wrap,
        );
        match mapped {
            IpeSub::Batch(items) => {
                assert_eq!(items.len(), 2);
                assert!(matches!(items[0], IpeSub::Every { ms: 1, .. }));
                assert!(matches!(items[1], IpeSub::None));
            }
            _ => panic!("expected Batch"),
        }
    }

    #[test]
    fn sub_map_source_retags_emitted_value() {
        // A source that emits one Child::Tick(9); after mapping, the emit
        // callback must receive Parent::FromChild(Child::Tick(9)).
        let src: IpeSub<Child> = IpeSub::Source(Box::new(
            |emit: std::sync::Arc<dyn Fn(Child) + Send + Sync>| {
                tokio::spawn(async move {
                    emit(Child::Tick(9));
                })
            },
        ));
        let mapped = sub_map(src, wrap);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let got = rt.block_on(async move {
            let (tx, rx) = std::sync::mpsc::channel::<Parent>();
            let emit: std::sync::Arc<dyn Fn(Parent) + Send + Sync> =
                std::sync::Arc::new(move |m| {
                    let _ = tx.send(m);
                });
            let IpeSub::Source(spawn) = mapped else {
                panic!("expected Source");
            };
            let handle = spawn(emit);
            let _ = handle.await;
            rx.recv().expect("one message")
        });
        assert_eq!(got, Parent::FromChild(Child::Tick(9)));
    }

    #[test]
    fn cmd_map_perform_retags_produced_msg() {
        let cmd: IpeCmd<Child> = IpeCmd::Perform(Box::new(|| Box::pin(async { Child::Tick(4) })));
        let mapped = cmd_map(cmd, wrap);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let got = rt.block_on(async move {
            let IpeCmd::Perform(thunk) = mapped else {
                panic!("expected Perform");
            };
            thunk().await
        });
        assert_eq!(got, Parent::FromChild(Child::Tick(4)));
    }

    #[test]
    fn cmd_map_batch_and_none_recurse() {
        let cmd: IpeCmd<Child> = cmd_batch(vec![
            IpeCmd::Perform(Box::new(|| Box::pin(async { Child::Tick(2) }))),
            cmd_none(),
        ]);
        let mapped = cmd_map(cmd, wrap);
        match mapped {
            IpeCmd::Batch(items) => {
                assert_eq!(items.len(), 2);
                assert!(matches!(items[0], IpeCmd::Perform(_)));
                assert!(matches!(items[1], IpeCmd::None));
            }
            _ => panic!("expected Batch"),
        }
    }

    #[test]
    fn cmd_map_publish_retags_by_identity() {
        // Publish yields the subscriber count (i64), not an M — mapping keeps
        // the thunk intact and only changes the phantom M in the type.
        let cmd: IpeCmd<Child> = IpeCmd::Publish(Box::new(|_origin| 3));
        let mapped: IpeCmd<Parent> = cmd_map(cmd, wrap);
        match mapped {
            IpeCmd::Publish(thunk) => assert_eq!(thunk("sid"), 3),
            _ => panic!("expected Publish"),
        }
    }
}

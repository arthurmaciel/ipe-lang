//! Ipê TEA runtime core — Cmd/Sub + the Ipe.Cli line-oriented loop.
//!
//! Cmd/Sub are generic over the message type M (NOT `any`): the intermediate
//! value `a` in `Cmd.perform` is erased inside a boxed M-producing future, but M
//! stays concrete. Step 1 (this file) ships the types, the simple kernels, and a
//! blocking Cli.program loop (stdin -> onLine -> update -> view). Sub.every
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
    Perform(Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = M> + Send>> + Send>),
    /// pub/sub broadcast. The thunk receives the publishing session's sid (the
    /// origin), injected by the Live dispatch loop, and returns the subscriber
    /// count. Not generic over the payload type T — T is captured inside the
    /// thunk (the same erasure-free pattern as `Perform`'s boxed future).
    Publish(Box<dyn FnOnce(&str) -> i64 + Send>),
}

/// A custom subscription event source: given an `emit` callback, spawn a task
/// that pushes messages into the loop, returning its JoinHandle (aborted on
/// re-subscribe). Keeps IpeSub decoupled from source-specific runtimes (e.g. the
/// WebSocket client builds one of these for `onMessage`).
pub type SubSpawn<M> =
    Box<dyn FnOnce(std::sync::Arc<dyn Fn(M) + Send + Sync>) -> tokio::task::JoinHandle<()> + Send>;

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

/// Time.every : Int -> msg -> Sub msg — alias of `Sub.every` (matches Go's
/// `Time_every`, which delegates to `Sub_every`). The `Time_every` kernel name
/// lowers to this.
pub fn time_every<M>(ms: i64, msg: M) -> IpeSub<M> {
    sub_every(ms, msg)
}

// `Ipe.Http.Stream.chunks` → `Sub_subscribeStream` lives in `http_stream.rs`
// now (alongside the stream registry it drains + the bridged `ChunkEvent` enum).
// It returns a `IpeSub::Source` driven by this module's SubManager.

// ─── TEA event loop plumbing (Sub.every tickers + Cmd firing) ───────────────

/// Internal loop event: a raw stdin line (Cli), a decoded key as (kind, value)
/// (Tui — Strings keep this free of the feature-gated TuiKey type), an
/// already-built Msg (from a ticker or a Cmd.perform result), or EOF. Shared by
/// `cli_program` and `tui_app` so both reuse `SubManager` (Tick) + `cli_run_cmd`.
pub(crate) enum CliEvent<M> {
    Line(String),
    // Constructed only by the `tui` raw-key reader; cli_program matches it
    // defensively (keys are ignored under Cli). In a non-tui build the variant
    // is never constructed but must remain in the shared enum for that arm.
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    Key(String, String),
    Msg(M),
    Eof,
}

/// Tracks the goroutine-equivalent ticker tasks spawned for the active
/// `Sub.every` subscriptions. `update` stops all + respawns from the new Sub
/// (mirrors tea_subs.go: one program, one model, re-evaluated each tick).
pub(crate) struct SubManager<M> {
    tx: tokio::sync::mpsc::UnboundedSender<CliEvent<M>>,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

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
                // First tick after `ms` (sleep-loop, matching Go's time.After).
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
/// and pushes the resulting Msg back into the loop channel.
pub(crate) fn cli_run_cmd<M: Send + 'static>(
    cmd: IpeCmd<M>,
    tx: &tokio::sync::mpsc::UnboundedSender<CliEvent<M>>,
) {
    match cmd {
        IpeCmd::None => {}
        IpeCmd::Batch(items) => {
            for c in items {
                cli_run_cmd(c, tx);
            }
        }
        IpeCmd::Perform(thunk) => {
            let tx = tx.clone();
            // Fire-and-forget: a panic inside the composed task→toMsg thunk aborts
            // only this task and is intentionally swallowed — that is the
            // Task-boundary recover contract (an effectful task that faults must
            // not crash the TEA loop). The fault therefore produces no Msg (the
            // send never runs); structured-warn observability on this path is a
            // known follow-up (would require awaiting the JoinHandle's JoinError).
            tokio::spawn(async move {
                let msg = thunk().await;
                let _ = tx.send(CliEvent::Msg(msg));
            });
        }
        IpeCmd::Publish(thunk) => {
            // No Live session in a Cli program; publish with an empty origin
            // (no subscriber's owner_sid matches "" → echo-default no-op).
            let _ = thunk("");
        }
    }
}

// ─── Ipe.Cli — line-oriented TEA loop ──────────────────────────────────────

/// Cli.program { init, update, view, subscriptions, onLine } : Task Error ().
///
/// init -> fire cmd -> subs -> view; then fold each event (stdin line via
/// onLine, ticker/Cmd.perform Msg) through update -> re-fire cmd -> re-subs ->
/// view, until stdin EOF. Stdin is read on a blocking task; tickers + perform
/// results merge into the same single-threaded update sequence via one channel.
pub fn cli_program<Model, Msg, E, FInit, FUpdate, FView, FSubs, FOnLine>(
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
        // regardless. Do NOT compose `cli_program` under a cancelling parent or
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

        let (mut model, cmd0) = init(());
        cli_run_cmd(cmd0, &tx);
        let mut submgr = SubManager::new(tx.clone());
        submgr.update(subscriptions(model.clone()));
        // Inline render (a closure borrowing `view` would make the future non-Send).
        // Fallible writes (NOT print!/println!, which panic on a broken pipe).
        //
        // A render must NOT force a trailing "\n": that would diverge from the
        // Go oracle. `runtime-go/rt/cli.go`'s `cliPrintView` is explicit that
        // it "writes the result to stdout WITHOUT a trailing newline (the
        // user's prompt formatting decides whether to add one)" — Go only ever
        // appends ONE newline, at `fmt.Println()` after the event loop exits
        // (same as here). This is intentional REPL-prompt design:
        // `examples/20-cli-counter`'s `view` returns
        // `"count=" ++ ... ++ "  (+, -, r, q) > "` with NO trailing newline so
        // the cursor stays on the prompt line for the user's input. An app that
        // wants each render on its own line supplies its own trailing "\n"
        // in its `view` string, exactly as the Go contract requires — see
        // `tests/golden/cli_program_view_separator` for a fixture that
        // deliberately does NOT do this and therefore glues renders
        // together, matching Go byte-for-byte.
        let _ = std::io::stdout().write_all(view(model.clone()).as_bytes());
        let _ = std::io::stdout().flush();

        while let Some(ev) = rx.recv().await {
            let msg = match ev {
                CliEvent::Line(l) => on_line(l),
                CliEvent::Key(_, _) => continue, // Cli has no keys
                CliEvent::Msg(m) => m,
                CliEvent::Eof => break,
            };
            let (next, cmd) = update(msg, model);
            model = next;
            cli_run_cmd(cmd, &tx);
            submgr.update(subscriptions(model.clone()));
            let _ = std::io::stdout().write_all(view(model.clone()).as_bytes());
            let _ = std::io::stdout().flush();
        }
        submgr.stop_all();
        let _ = std::io::stdout().write_all(b"\n");
        ok_res(())
    })
}

//! `ipe watch`.
//!
//! The salsa-aware orchestrator that wires [`ipe_watch`]'s salsa-agnostic
//! primitives (confined watcher, debounce, process supervisor) to THIS
//! crate's warm compile pipeline. This is the first real CONSUMER of
//! incremental compilation's speed benefit — a naive "re-run `ipe build`
//! from scratch on every save" watch mode would defeat the entire point of
//! the salsa port.
//!
//! Decision record: `docs/adr/0032-salsa-incremental-compilation-phase1.md`.
//!
//! ## Architecture
//!
//! Three threads plus the orchestrator (this module's `run`):
//!
//! 1. **notify watcher thread** (owned by the `notify::Watcher` handle) —
//!    pushes every IN-SCOPE raw path change onto an `mpsc` channel. The
//!    [`ipe_watch::WatchScope::is_relevant`] filter runs INSIDE the event
//!    callback, so an excluded-dir storm never reaches the channel at all.
//!    The callback ALSO rejects `EventKind::Access` (open/read)
//!    and `EventKind::Other` before that filter even runs — load-bearing,
//!    not an optimisation: `resolve_project_sources` opens every in-scope
//!    file on every rebuild, and without this exclusion that OPEN is
//!    itself an observable event that would queue another rebuild forever.
//!    One Access sub-variant, `Close(Write)`, is deliberately EXEMPTED from
//!    that rejection — it is the write-completion proof a debounce window
//!    alone cannot substitute for; see the callback's own doc comment for
//!    why.
//! 2. **coalesce thread** — [`ipe_watch::coalesce_loop`] turns that raw
//!    stream into settled batches (the debounce half).
//! 3. **compile worker thread** (spawned fresh per rebuild cycle) — holds a
//!    CLONED [`ipe_db::IpeDatabase`] handle and runs [`compile_prepared`]
//!    inside [`salsa::Cancelled::catch`]. The orchestrator thread never runs
//!    a salsa query itself; it only mutates inputs (which is what makes a
//!    superseding edit cancel the in-flight worker — see the cancellation
//!    section below).
//!
//! The orchestrator drains one unified `mpsc::Receiver<OrchestratorEvent>`
//! that both the coalesce thread and every worker/cargo-wait thread feed —
//! a single blocking `recv()` with **no busy-polling**, and no risk of two
//! event sources racing on separate wakeups.
//!
//! ## Cancellation, and why it needs no extra machinery
//!
//! Salsa's `#[salsa::input]` setters require `&mut IpeDatabase` (routed
//! through `zalsa_mut()`), which — per `salsa::Storage`'s own documented
//! contract (verified against the pinned `salsa=0.27.2` source,
//! `src/storage.rs`'s `cancel_others`) — sets a cancellation flag and BLOCKS
//! until every other `Storage` handle (a `.clone()` of the database, exactly
//! what a compile worker holds) has been dropped. A query running on a
//! cancelled snapshot unwinds via `panic::resume_unwind(Cancelled)` the next
//! time it checks (every tracked-function boundary — the `WillCheckCancellation`
//! event salsa's own test suite pins). So:
//!
//! - the orchestrator NEVER calls `sync_source_root` while ALSO holding a
//!   worker's clone alive on its own thread — it hands the clone to the
//!   worker THREAD and keeps only the original `db_main` on the orchestrator
//!   thread;
//! - when a new settled batch arrives mid-build, `sync_source_root(&mut
//!   db_main, …)` is called immediately — this call blocks (invisibly, from
//!   the orchestrator's point of view) until the worker thread's query
//!   unwinds and drops its cloned `Storage`, then returns; the worker thread
//!   observes `Cancelled` via `salsa::Cancelled::catch`, reports it, and
//!   exits;
//! - the orchestrator then spawns a FRESH worker against the just-synced
//!   (latest) state.
//!
//! This is exactly rust-analyzer's own cancellation pattern, and it needs
//! zero new synchronisation primitives beyond what salsa already provides.
//!
//! `cargo build` cancellation (never overlapping cargo builds) uses the
//! portable equivalent for a plain OS process: the
//! orchestrator holds the `Child` handle directly and calls `.kill()` on a
//! superseding batch; a dedicated per-build "waiter" thread blocks on
//! `.wait()` and reports completion (or, if killed, a status the
//! orchestrator recognises as "superseded, not a real failure") through the
//! SAME unified event channel, tagged with a generation counter so a stale
//! completion from an already-superseded cycle is silently ignored rather
//! than raced against the new one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use ipe_intern::Interner;

use crate::project;
use crate::{CliError, write_emitted_project};

/// A lifecycle notification from a running watch session.
///
/// Delivered synchronously (before the orchestrator moves on) through
/// [`WatchOptions::on_event`]. This is the seam integration tests use to
/// observe internal state transitions (exactly-once coalescing, red-build
/// preservation, cancellation) without racing wall-clock timing or
/// capturing stderr — and, incidentally, a reusable hook for a future
/// structured/`--json` watch log. The CLI path leaves `on_event` unset
/// (`None`), so it costs nothing there.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// A settled batch triggered a NEW rebuild cycle. Exactly one of these
    /// fires per coalesced batch, never per raw filesystem event.
    RebuildStarted { generation: u64 },
    /// `generation`'s compile finished with a compiler diagnostic — the
    /// running process (if any) is untouched (INV-3).
    CompileFailed { generation: u64 },
    /// `generation`'s compile was cancelled by a superseding edit
    /// — never reported to the end user, but observable here for tests.
    CompileCancelled { generation: u64 },
    /// `generation`'s `cargo build` failed — the running process (if any)
    /// is untouched (INV-3).
    CargoFailed { generation: u64 },
    /// `generation`'s `cargo build` was killed because a newer batch
    /// superseded it ("never overlapping cargo builds").
    CargoKilled { generation: u64 },
    /// `generation` reached [`ipe_watch::SupervisorState::apply_green`] and
    /// this was the outcome.
    Restarted {
        generation: u64,
        outcome: RestartOutcomeKind,
    },
}

/// A test/observability-friendly mirror of [`ipe_watch::RestartOutcome`].
///
/// That type intentionally isn't `Clone` — it can carry a live [`Child`]
/// via [`ipe_watch::SupervisorState`] elsewhere in this module — so
/// [`WatchEvent`] carries this small `Clone`-able summary instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartOutcomeKind {
    Spawned,
    UnchangedBinary,
    Restarted,
    RespawnedLastGood,
    NothingRunning,
}

/// Configuration for one `ipe watch` session. Every field has a sound
/// default via [`WatchOptions::new`]; the CLI layer (`run_watch` in
/// `lib.rs`) is the only place that overrides them from flags.
pub struct WatchOptions {
    pub entry: PathBuf,
    pub out_dir: PathBuf,
    pub runtime_dir: PathBuf,
    /// The port injected as `IPE_LIVE_PORT` for the spawned child and probed
    /// for `/_ipe/readyz` when the emitted project is detected as a
    /// Ipe.Web app. Harmless (ignored) for every other app shape.
    pub port: u16,
    pub debounce: ipe_watch::DebounceConfig,
    pub restart_timeouts: ipe_watch::RestartTimeouts,
    /// The resolved `cargo` executable each rebuild spawns. The CLI layer
    /// resolves it once (fail-closed) before the loop starts and stores the
    /// path here; the default is the bare name `cargo`, deferring to `PATH`
    /// resolution for callers that do not pre-resolve.
    pub cargo_path: PathBuf,
    /// Optional lifecycle observer — see [`WatchEvent`]. `None` on the CLI
    /// path.
    pub on_event: Option<Arc<dyn Fn(WatchEvent) + Send + Sync>>,
}

impl WatchOptions {
    #[must_use]
    pub fn new(entry: PathBuf, out_dir: PathBuf, runtime_dir: PathBuf) -> Self {
        Self {
            entry,
            out_dir,
            runtime_dir,
            port: 8000,
            debounce: ipe_watch::DebounceConfig::default(),
            restart_timeouts: ipe_watch::RestartTimeouts::default(),
            cargo_path: PathBuf::from("cargo"),
            on_event: None,
        }
    }
}

fn emit(opts: &WatchOptions, event: WatchEvent) {
    if let Some(cb) = &opts.on_event {
        cb(event);
    }
}

/// One resolved project snapshot — the ingredients [`compile_modules`]
/// (the one-shot driver) would consume, but returned to the CALLER instead
/// of immediately compiled, so `ipe watch` can re-resolve on every settled
/// batch and feed the result through a WARM, reused database via
/// `ipe_db::sync_source_root` rather than constructing a fresh one per
/// build. Deliberately mirrors `run_build`'s own manifest-resolution
/// dispatch (`crates/ipe/src/lib.rs`) without touching that code — the
/// one-shot entry points stay exactly as tested by the golden suite; this
/// is an independent, read-only duplicate of the RESOLUTION step only (no
/// compiler stage runs here).
pub(crate) struct ResolvedProject {
    pub(crate) sources: BTreeMap<Vec<String>, (PathBuf, String)>,
    pub(crate) discovered: Vec<project::DiscoveredModule>,
    pub(crate) entry_path: Vec<String>,
    pub(crate) blame_path: PathBuf,
    pub(crate) db_driver: ipe_backend_rust::DbDriver,
    /// The `[wasm] publicEnv` allowlist (empty for the no-manifest / sibling-
    /// discovery path — there is no manifest to declare one).
    pub(crate) wasm_public_env: Vec<String>,
    /// The sanitized Cargo package name for the emitted crate (from `package.ipe`
    /// name via [`ipe_backend_rust::sanitize_cargo_name`]). Empty string
    /// when no manifest is present (sibling-discovery path uses `"ipe-app"`).
    pub(crate) cargo_name: String,
}

/// Resolve `entry` (a `.ipe` file or a project directory) into a fresh
/// [`ResolvedProject`] by re-reading every relevant file from disk. Mirrors
/// `run_build`'s dispatch: directory → `package.ipe` inside it; `.ipe` → walk
/// up for a manifest, else sibling discovery.
///
/// `entry_text_override`, when given, shadows the entry `.ipe` file's disk
/// bytes in the no-manifest branch — the LSP hands the unsaved editor
/// buffer here so module-path discovery follows what the author sees, not
/// stale disk state. `ipe watch` always passes `None` (disk is its truth).
///
/// # Errors
/// [`CliError::Io`] on any filesystem failure; [`CliError::Pipeline`] if the
/// entry file itself fails to parse (needed only to learn its declared
/// module path in the no-manifest case).
pub(crate) fn resolve_project_sources(
    entry: &Path,
    entry_text_override: Option<&str>,
) -> Result<ResolvedProject, CliError> {
    let manifest_path = if entry.is_dir() {
        match project::manifest_in_dir(entry) {
            Some(manifest) => Some(manifest),
            None if project::migration_pending(entry) => {
                return Err(CliError::Usage(project::MIGRATE_CONFIG_HINT));
            }
            None => {
                return Err(CliError::Usage(
                    "directory supplied but no package.ipe found inside it",
                ));
            }
        }
    } else {
        crate::find_manifest_for_ipe_file(entry)
    };

    if let Some(manifest_path) = manifest_path {
        let manifest = project::parse_manifest(&manifest_path)?;
        let discovered = project::discover_modules(&manifest.src_root)?;
        let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
        for m in &discovered {
            let src = crate::io_bounded::read_to_string_capped(
                &m.path,
                crate::io_bounded::SOURCE_READ_CAP,
            )?;
            sources.insert(m.module_path.clone(), (m.path.clone(), src));
        }
        let cargo_name = ipe_backend_rust::sanitize_cargo_name(&manifest.name);
        return Ok(ResolvedProject {
            sources,
            discovered,
            entry_path: vec!["Main".to_owned()],
            blame_path: manifest_path,
            db_driver: manifest.driver,
            wasm_public_env: manifest.wasm.public_env,
            cargo_name,
        });
    }

    // No manifest: sibling discovery, mirroring `build_with_sibling_discovery`.
    let source = match entry_text_override {
        Some(text) => text.to_owned(),
        None => {
            crate::io_bounded::read_to_string_capped(entry, crate::io_bounded::SOURCE_READ_CAP)?
        }
    };
    let mut name_interner = Interner::new();
    let parsed = ipe_parse::parse_module(&source, &mut name_interner).map_err(|diag| {
        CliError::Pipeline {
            file: entry.to_path_buf(),
            src: source.clone(),
            diag: Box::new(diag),
        }
    })?;
    let entry_module_path: Vec<String> = parsed
        .name
        .value
        .iter()
        .map(|s| name_interner.resolve(*s).unwrap_or_default().to_owned())
        .collect();
    let src_root = entry
        .parent()
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| Path::new("."));
    let mut discovered = project::discover_modules(src_root)?;
    if !discovered
        .iter()
        .any(|m| m.module_path == entry_module_path)
    {
        discovered.push(project::DiscoveredModule {
            path: entry.to_path_buf(),
            module_path: entry_module_path.clone(),
        });
    }
    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    for m in &discovered {
        if m.module_path == entry_module_path {
            sources.insert(
                entry_module_path.clone(),
                (entry.to_path_buf(), source.clone()),
            );
        } else {
            let src = crate::io_bounded::read_to_string_capped(
                &m.path,
                crate::io_bounded::SOURCE_READ_CAP,
            )?;
            sources.insert(m.module_path.clone(), (m.path.clone(), src));
        }
    }
    Ok(ResolvedProject {
        sources,
        discovered,
        entry_path: entry_module_path,
        blame_path: entry.to_path_buf(),
        db_driver: ipe_backend_rust::DbDriver::Sqlite,
        wasm_public_env: Vec::new(),
        cargo_name: String::new(),
    })
}

/// The project root + entry directory a [`ipe_watch::WatchScope`] confines
/// itself to, derived from one resolved snapshot.
fn scope_roots(resolved: &ResolvedProject, entry: &Path) -> (PathBuf, PathBuf) {
    // The manifest's directory when a `package.ipe` is the blame path;
    // otherwise the blame path IS the entry file, so its parent is the source
    // root — matching `build_with_sibling_discovery`.
    if resolved.blame_path.file_name().and_then(|n| n.to_str())
        == Some(crate::package_manifest::PACKAGE_IPE)
    {
        let root = resolved
            .blame_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        (root.clone(), root)
    } else {
        let dir = entry
            .parent()
            .filter(|p| p.is_dir())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        (dir.clone(), dir)
    }
}

/// One event on the orchestrator's unified channel. Carries a `generation`
/// on every variant whose completion could race a superseding cycle, so a
/// STALE completion (from a cycle the orchestrator has already moved past)
/// is recognisable and silently dropped rather than raced against the
/// current one.
enum OrchestratorEvent {
    /// A settled batch of filesystem changes — the coalescer's output.
    /// Carries no paths: `ipe watch` re-resolves the WHOLE project on every
    /// cycle (cheap — a directory walk over a bounded file set) and lets
    /// `sync_source_root`'s own byte-equal no-op boundary do the real
    /// dirty-vs-clean filtering, so there is no need to thread individual
    /// changed paths through the recompute step.
    FsBatch,
    /// The compile worker for `generation` finished (successfully, with a
    /// compiler diagnostic, or cancelled).
    CompileDone {
        generation: u64,
        outcome: CompileOutcome,
    },
    /// The `cargo build` for `generation` finished (successfully, with a
    /// build failure, or was killed because it was superseded).
    CargoDone {
        generation: u64,
        outcome: CargoOutcome,
    },
    /// An external caller requested a clean shutdown (see [`WatchHandle`]).
    /// Used by tests and any future embedder that needs to stop a watch
    /// session programmatically rather than only on Ctrl-C.
    Shutdown,
}

/// Upper bound on how long [`WatchHandle::stop`] (and, transitively,
/// [`WatchHandle`]'s `Drop` safety net) will block waiting for the
/// orchestrator thread to confirm it has actually finished tearing down
/// (child process killed/reaped, watcher + coalesce threads joined) —
/// never an unbounded wait (every long-running command is timeout-bounded).
/// Generously above the realistic worst case: a
/// `graceful_stop` of [`ipe_watch::RestartTimeouts::default`]'s 3 s, plus
/// slack for the cargo-kill waiter's poll loop, the compile worker's salsa
/// unwind, and the coalesce thread's join — all of which are themselves
/// individually bounded and normally complete in well under a second once
/// shutdown starts.
const SHUTDOWN_WAIT_BUDGET: Duration = Duration::from_secs(20);

/// Delay before retrying a cycle whose `resolve_project_sources` call
/// failed. A transient resolve failure (mid-save partial write, a file
/// momentarily unreadable during an editor's atomic rename) has no
/// guarantee of a follow-up filesystem event to retry it, so the cycle
/// schedules its own retry rather than losing the save until the next
/// unrelated edit.
const RESOLVE_RETRY_DELAY: Duration = Duration::from_millis(250);

/// Schedule one follow-up [`OrchestratorEvent::FsBatch`] after
/// [`RESOLVE_RETRY_DELAY`] — the recovery path for a `resolve_project_sources`
/// failure. Without this, a transient failure has no other route back into
/// the orchestrator's event loop: the triggering save is lost until an
/// unrelated future filesystem event happens to arrive.
fn schedule_resolve_retry(evt_tx: &mpsc::Sender<OrchestratorEvent>) {
    let retry_tx = evt_tx.clone();
    thread::spawn(move || {
        thread::sleep(RESOLVE_RETRY_DELAY);
        let _ = retry_tx.send(OrchestratorEvent::FsBatch);
    });
}

/// A handle to a running [`spawn`]ed watch session.
///
/// Lets the caller request a clean shutdown from another thread — the seam
/// integration tests use to stop `ipe watch` deterministically instead of
/// relying on a process signal.
///
/// `Drop` is a genuine safety net, not merely `stop()` called for you: an
/// embedder that lets a `WatchHandle` fall out of scope WITHOUT calling
/// `stop()` first — a bug in the embedder's own code, a panic unwinding
/// through a scope that holds one, an early `return`/`?` — must never leak
/// the supervised child process (a whole spawned `ipe-app` server binding a
/// real port) as an orphan. `stop()` and `Drop::drop` therefore share one
/// synchronous, bounded implementation: signal the orchestrator, then block
/// (up to [`SHUTDOWN_WAIT_BUDGET`]) until it confirms teardown is done —
/// not merely requested.
pub struct WatchHandle {
    stop_tx: mpsc::Sender<()>,
    /// Signalled by the orchestrator thread's own wrapper (see [`spawn`])
    /// once `run_inner` has returned — i.e. AFTER `SupervisorState::shutdown`
    /// has killed/reaped the supervised child and every helper thread has
    /// been joined. `Mutex<Option<..>>` rather than a bare `Receiver` so
    /// `stop()` can take `&self` (matching its pre-existing public
    /// signature) while still being able to drain the receiver exactly
    /// once; a second `stop()`/`Drop` call after the first successful wait
    /// finds `None` and is a harmless no-op, matching `stop()`'s existing
    /// idempotency contract.
    done_rx: Mutex<Option<mpsc::Receiver<()>>>,
}

impl WatchHandle {
    /// Request a clean shutdown and BLOCK (bounded by
    /// [`SHUTDOWN_WAIT_BUDGET`]) until the orchestrator thread confirms it
    /// has actually finished — not merely until the request was sent.
    /// Idempotent: a second call, or a call after `Drop` already waited
    /// (impossible through the public API, since `Drop` consumes `self`,
    /// but relevant to `Drop`'s own internal reuse of this method) is a
    /// harmless no-op.
    pub fn stop(&self) {
        let _ = self.stop_tx.send(());
        self.wait_for_shutdown();
    }

    /// Block until the orchestrator's done-signal arrives, or
    /// [`SHUTDOWN_WAIT_BUDGET`] elapses — whichever comes first. Draining
    /// (`Option::take`) the receiver makes this safe to call more than
    /// once: every call after the first successful wait (or a prior
    /// `Drop`) is a no-op.
    fn wait_for_shutdown(&self) {
        let mut guard = self
            .done_rx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(rx) = guard.take() {
            let _ = rx.recv_timeout(SHUTDOWN_WAIT_BUDGET);
        }
    }
}

impl Drop for WatchHandle {
    /// The safety net described on the type itself: guarantees the
    /// supervised child process is torn down even when the embedder never
    /// called `stop()` — including when a panic unwinds through a scope
    /// holding a `WatchHandle`. Rust always runs `Drop` during ordinary
    /// unwinding (this is not the `abort` panic strategy), so this fires on
    /// both the "forgot to call `stop()`" and the "panicked while holding
    /// one" cases the review flagged. `stop_tx.send` and
    /// `recv_timeout` are both plain, synchronous, non-blocking-by-default
    /// calls (no `.await`, nothing that needs an async runtime), so this is
    /// sound to run directly from `drop`.
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
        self.wait_for_shutdown();
    }
}

/// Spawn `ipe watch` on its own thread, returning a join handle plus a
/// [`WatchHandle`] the caller can use to stop it.
///
/// Identical behaviour to [`run`], with one addition: an external `stop()`
/// call is delivered
/// through the SAME unified event channel `run`'s own loop already drains
/// (as an [`OrchestratorEvent::Shutdown`]), so shutdown ordering is
/// serialised with every other event exactly like a real Ctrl-C would be —
/// no separate code path to keep in sync.
#[must_use]
pub fn spawn(opts: WatchOptions) -> (thread::JoinHandle<Result<(), CliError>>, WatchHandle) {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    // `done_tx` is moved into the spawned thread's own closure (never handed
    // to `run_inner` itself) and is unconditionally dropped the instant that
    // closure returns — on EVERY exit path (the clean `Ok(())` after full
    // shutdown, or an early setup-failure `Err`), not just the happy path.
    // `WatchHandle::wait_for_shutdown` therefore unblocks promptly even if
    // `run_inner` fails before ever reaching its main loop.
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let handle = thread::spawn(move || {
        let result = run_inner(&opts, Some(stop_rx));
        let _ = done_tx.send(());
        result
    });
    (
        handle,
        WatchHandle {
            stop_tx,
            done_rx: Mutex::new(Some(done_rx)),
        },
    )
}

enum CompileOutcome {
    Green(Arc<ipe_backend::EmittedProject>),
    Red(String),
    /// Cancelled via salsa (a newer generation superseded it) — never
    /// reported to the user; the newer cycle already took over.
    Cancelled,
}

enum CargoOutcome {
    Green(PathBuf),
    Red(String),
    Killed,
}

/// Run `ipe watch` until the process receives a shutdown signal (Ctrl-C) or
/// every event source disconnects.
///
/// Never returns an `Err` for a build failure — INV-3 means a red build is
/// a LOGGED event, not a fatal one; this only returns `Err` for a genuine
/// setup failure (scope refused, watcher couldn't start).
///
/// # Errors
/// [`CliError`] if the confined scope cannot be built, or the filesystem
/// watcher cannot be started.
pub fn run(opts: &WatchOptions) -> Result<(), CliError> {
    run_inner(opts, None)
}

/// The full implementation behind both [`run`] (CLI-facing, no external stop
/// channel) and [`spawn`] (embedder/test-facing, stoppable via
/// [`WatchHandle`]).
///
/// # Errors
/// [`CliError`] if the confined scope cannot be built, or the filesystem
/// watcher cannot be started.
#[allow(clippy::too_many_lines)]
fn run_inner(
    opts: &WatchOptions,
    external_stop: Option<mpsc::Receiver<()>>,
) -> Result<(), CliError> {
    let initial = resolve_project_sources(&opts.entry, None)?;
    let (root_dir, entry_dir) = scope_roots(&initial, &opts.entry);

    let scope = ipe_watch::WatchScope::build(&root_dir, &entry_dir)
        .map_err(|e| CliError::UsageOwned(e.to_string()))?;
    eprintln!(
        "{}",
        crate::style::gutter(&format!(
            "[ipe watch] watching {} ({} source files)",
            scope.root().display(),
            scope.file_count()
        ))
    );

    let (raw_tx, raw_rx) = mpsc::channel::<PathBuf>();
    let mut watcher = {
        let scope = scope.clone();
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            // Reject non-mutating ACCESS events (open/read/execute) and the
            // watch-internal `Other` kind — EXCEPT `Access(Close(Write))`,
            // which is deliberately let through (see below). This is
            // load-bearing, not an optimisation: the orchestrator's own
            // `resolve_project_sources` OPENS every in-scope `.ipe` file
            // (and reads the watched directory) on EVERY rebuild cycle.
            // Some backends (Linux inotify, by default) report that
            // open/read as an `Access(Open)`/`Access(Close(Read))` event —
            // without this filter, the watcher would observe its own read,
            // queue another rebuild, read again to service it, and so on
            // forever (a self-triggering rebuild storm this was caught by,
            // not merely guarded against speculatively). `resolve_project_
            // sources` only ever opens `.ipe` files for READING, so it can
            // never itself produce the one Access variant this filter
            // exempts (`Close(Write)` — see below); the exemption cannot
            // reopen the self-trigger hole.
            //
            // `Close(Write)` exemption (coalescing race):
            // `std::fs::write` — and most editors' "atomic-ish"
            // save path — is `open(O_TRUNC) → write() → close()`, which is
            // NOT one atomic filesystem operation. `open(O_TRUNC)` alone
            // can fire `Modify(Data)` (truncating IS a data change) the
            // instant the file is EMPTIED, strictly BEFORE the writer's own
            // `write()` call actually lands the new bytes. Under enough
            // scheduling pressure on the WRITING process/thread (verified:
            // 8-way CPU saturation + a concurrent real `cargo build`
            // reproduces this reliably; a quiet system does not), the gap
            // between that `open(O_TRUNC)` and the following `write()` can
            // exceed the debounce quiescence window — the coalescer, having
            // received only the truncate's `Modify(Data)` and nothing more
            // within its window, correctly (by ITS OWN local contract)
            // considers the batch settled and fires a rebuild that reads a
            // GENUINELY EMPTY file on disk (`IPE-P0020` "malformed module
            // header"), followed shortly by a second, correct rebuild once
            // the writer finally catches up and closes the file.
            // `Close(Write)` is the one signal that is only ever
            // emitted once a write-mode file handle is actually `close()`d
            // — which, by the writing process's own fd lifecycle, can only
            // happen AFTER its `write()` call returns. Letting that specific
            // event back through means the coalescer's quiescence timer
            // resets on it exactly like any other mutating event, so a
            // truncate-then-write sequence that spans MORE than the nominal
            // debounce window still gets folded into the SAME batch — closed
            // by construction (keyed off the syscall that is a
            // write-completion PROOF), not by widening a timing margin and
            // hoping it's wide enough. `Create`/`Modify`/`Remove`/`Any` (the
            // "imprecise backend" catch-all) all pass through
            // unconditionally, unaffected by this exemption.
            let is_write_close = matches!(
                event.kind,
                notify::EventKind::Access(notify::event::AccessKind::Close(
                    notify::event::AccessMode::Write
                ))
            );
            if !is_write_close
                && (event.kind.is_access() || matches!(event.kind, notify::EventKind::Other))
            {
                return;
            }
            for path in event.paths {
                if scope.is_relevant(&path) {
                    let _ = raw_tx.send(path);
                }
            }
        })
        .map_err(|e| CliError::UsageOwned(format!("watch: cannot start filesystem watcher: {e}")))?
    };
    for w in scope.roots_to_watch() {
        notify::Watcher::watch(&mut watcher, w.as_path(), notify::RecursiveMode::Recursive)
            .map_err(|e| {
                CliError::UsageOwned(format!(
                    "watch: cannot watch {}: {e}",
                    w.as_path().display()
                ))
            })?;
    }

    warn_if_memory_store();

    let (batch_tx, batch_rx) = mpsc::channel::<ipe_watch::Batch>();
    let debounce_cfg = opts.debounce;
    let coalesce_handle =
        thread::spawn(move || ipe_watch::coalesce_loop(&raw_rx, &batch_tx, debounce_cfg));

    let (evt_tx, evt_rx) = mpsc::channel::<OrchestratorEvent>();
    {
        let evt_tx = evt_tx.clone();
        thread::spawn(move || {
            for _batch in batch_rx {
                if evt_tx.send(OrchestratorEvent::FsBatch).is_err() {
                    return;
                }
            }
        });
    }
    // SIGTERM → orderly shutdown, for the CLI `run()` path ONLY (`external_stop`
    // is `None` exactly there). A supervisor's `kill -TERM <ipe-pid>` (systemd's
    // default, PID-only — not the foreground process group Ctrl-C signals) would
    // otherwise hard-kill this process before ANY teardown code runs, orphaning
    // the supervised child on its port forever. The forwarder is a third instance
    // of the existing "send into the unified event channel" pattern; the
    // `Shutdown => break` arm below then runs the full, already-tested teardown.
    //
    // NEVER installed for `spawn()` (`external_stop` is `Some`): `spawn()` runs
    // on a same-process background thread inside an EMBEDDING HOST — installing a
    // process-wide SIGTERM handler there would silently and permanently change
    // the HOST's signal disposition (signal-hook does not restore the previous
    // disposition once its action is gone). `spawn()` keeps relying exclusively
    // on `WatchHandle`'s stop channel + `Drop` safety net.
    if external_stop.is_none() {
        #[cfg(unix)]
        {
            let evt_tx = evt_tx.clone();
            // Errors are logged, never fatal — a platform where signal
            // registration fails degrades to the pre-existing behaviour (no
            // PID-only-SIGTERM handling), never a hard failure of `ipe watch`.
            if let Err(e) = ipe_watch::install_sigterm_forwarder(move || {
                let _ = evt_tx.send(OrchestratorEvent::Shutdown);
            }) {
                eprintln!(
                    "{}",
                    crate::style::gutter(&format!(
                        "[ipe watch] warning: could not install SIGTERM handler: {e}"
                    ))
                );
            }
        }
    }
    if let Some(stop_rx) = external_stop {
        let evt_tx = evt_tx.clone();
        thread::spawn(move || {
            if stop_rx.recv().is_ok() {
                let _ = evt_tx.send(OrchestratorEvent::Shutdown);
            }
        });
    }

    // Resolve the runtime crate root once, fail-closed, before the event loop
    // starts. The path-dependency emit needs the CRATE ROOT (the directory
    // holding the runtime `Cargo.toml`), not the source sub-tree. This
    // mirrors how `ipe build` resolves it via `runtime_embed::resolve()`.
    let runtime_dep_root = crate::runtime_embed::resolve()?.root().to_path_buf();

    let mut db_main = ipe_db::IpeDatabase::new();
    let mut source_root: Option<ipe_db::SourceRoot> = None;
    let mut config: Option<ipe_db::BuildConfig> = None;
    let mut supervisor = ipe_watch::SupervisorState::fresh();
    let mut generation: u64 = 0;
    let mut compile_worker: Option<thread::JoinHandle<()>> = None;
    let mut cargo_child: Option<Arc<std::sync::Mutex<Child>>> = None;
    // Set at `CompileDone` (Green), consumed at `CargoDone` (Green) — the
    // readiness strategy is a property of the SOURCE (does it call
    // `Web.app`?), decided once per generation right after emit, not
    // re-derived from the built executable (which carries no such marker).
    let mut current_is_web = false;

    // Kick off the first build immediately — don't wait for a file event.
    if evt_tx.send(OrchestratorEvent::FsBatch).is_err() {
        return Ok(());
    }

    while let Ok(event) = evt_rx.recv() {
        match event {
            OrchestratorEvent::FsBatch => {
                generation += 1;
                let this_gen = generation;
                emit(
                    opts,
                    WatchEvent::RebuildStarted {
                        generation: this_gen,
                    },
                );

                // Single-flight: a superseding batch kills any in-flight
                // cargo build immediately ("never overlapping cargo
                // builds"). Only a signal is sent here (`kill`, never
                // a blocking `wait`) — the build's own waiter thread (see
                // `spawn_cargo_build`) observes the exit via its own poll
                // and reports `CargoOutcome::Killed`, so this arm never
                // blocks the orchestrator.
                if let Some(child) = cargo_child.take()
                    && let Ok(mut child) = child.lock()
                {
                    let _ = child.kill();
                }

                let resolved = match resolve_project_sources(&opts.entry, None) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("{}", crate::style::gutter(&format!("[ipe watch] {e}")));
                        // This cycle's `generation` bump and cargo-kill
                        // already happened above, so without a scheduled
                        // retry the save that triggered this cycle is lost
                        // until an unrelated future edit.
                        schedule_resolve_retry(&evt_tx);
                        continue;
                    }
                };

                let mut sources = resolved.sources;
                let mut discovered = resolved.discovered;
                let injected = project::inject_compiled_std_closure(&mut sources, &mut discovered);

                // Load the FFI catalog and inject installed-crate interface
                // modules — the same seam `run_build` uses (CO-INCR-005).
                // An error logs and skips the cycle (same policy as a
                // resolution failure above) rather than tearing down the
                // whole watch session.
                let ffi_prep = match crate::ffi::prepare_ffi(&mut sources, &resolved.blame_path) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!(
                            "{}",
                            crate::style::gutter(&format!("[ipe watch] FFI catalog error: {e}"))
                        );
                        continue;
                    }
                };

                let desired: BTreeMap<Vec<String>, (String, ipe_db::ModuleOrigin)> = sources
                    .iter()
                    .map(|(p, (_, text))| {
                        let origin = if injected.contains(p) {
                            ipe_db::ModuleOrigin::EmbeddedStdlib
                        } else if ffi_prep.injected.contains(p) {
                            ipe_db::ModuleOrigin::FfiInterface
                        } else {
                            ipe_db::ModuleOrigin::User
                        };
                        (p.clone(), (text.clone(), origin))
                    })
                    .collect();

                // This call BLOCKS until any in-flight compile
                // worker's cancelled query unwinds and drops its database
                // clone — see the module doc's cancellation walkthrough.
                let root = if let Some(root) = source_root {
                    ipe_db::sync_source_root(&mut db_main, root, &desired);
                    root
                } else {
                    let root = crate::create_source_root(
                        &db_main,
                        &sources,
                        &injected,
                        &ffi_prep.injected,
                    );
                    source_root = Some(root);
                    root
                };

                let cfg = if let Some(cfg) = config {
                    if cfg.db_driver(&db_main) != resolved.db_driver {
                        use salsa::Setter as _;
                        cfg.set_db_driver(&mut db_main).to(resolved.db_driver);
                    }
                    if cfg.wasm_public_env(&db_main) != &resolved.wasm_public_env {
                        use salsa::Setter as _;
                        cfg.set_wasm_public_env(&mut db_main)
                            .to(resolved.wasm_public_env.clone());
                    }
                    cfg
                } else {
                    // Pass ffi_prep.emit so the backend can write FFI
                    // bindings (src/ffi.rs) into the emitted project — the
                    // same slot `run_build` fills via BuildConfig (CO-INCR-005).
                    let cfg = ipe_db::BuildConfig::new(
                        &db_main,
                        resolved.db_driver,
                        ffi_prep.emit,
                        ipe_ir::Target::Native,
                        resolved.wasm_public_env.clone(),
                        false,
                        // `ipe watch` is a development loop — Debug.* is allowed.
                        false,
                        // Dependency-model emit: the project links the runtime as a
                        // path dependency (what `ipe build` uses by default), so
                        // no runtime source is vendored into `src/ipe_runtime/`.
                        // `runtime_dep_root` is the verified crate root (holds
                        // `Cargo.toml`), resolved once before the loop via
                        // `runtime_embed::resolve()`, matching `ipe build`'s path.
                        Some(ipe_backend_rust::RuntimeDep {
                            root: runtime_dep_root.clone(),
                        }),
                        // `ipe watch` does not expose `--debugger`; never record.
                        false,
                        resolved.cargo_name.clone(),
                    );
                    config = Some(cfg);
                    cfg
                };

                // The prior worker (if any) has already unwound by the time
                // `sync_source_root` returned above; join to reclaim the
                // thread handle (fast — it already finished).
                if let Some(h) = compile_worker.take() {
                    let _ = h.join();
                }

                let db_worker = db_main.clone();
                let entry_path = resolved.entry_path.clone();
                let blame_path = resolved.blame_path.clone();
                let evt_tx = evt_tx.clone();
                compile_worker = Some(thread::spawn(move || {
                    let outcome =
                        match salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
                            crate::compile_prepared(
                                &db_worker,
                                root,
                                &sources,
                                &entry_path,
                                &blame_path,
                                cfg,
                            )
                        })) {
                            Ok(Ok(emitted)) => CompileOutcome::Green(Arc::new(emitted)),
                            Ok(Err(e)) => CompileOutcome::Red(e.to_string()),
                            Err(_cancelled) => CompileOutcome::Cancelled,
                        };
                    let _ = evt_tx.send(OrchestratorEvent::CompileDone {
                        generation: this_gen,
                        outcome,
                    });
                }));
            }

            OrchestratorEvent::CompileDone {
                generation: g,
                outcome,
            } => {
                if g != generation {
                    continue; // stale — a newer cycle already superseded this one.
                }
                match outcome {
                    CompileOutcome::Cancelled => {
                        emit(opts, WatchEvent::CompileCancelled { generation: g });
                    }
                    CompileOutcome::Red(msg) => {
                        eprintln!(
                            "{}",
                            crate::style::gutter(&format!(
                                "[ipe watch] build failed (last-good binary stays up):\n{msg}"
                            ))
                        );
                        emit(opts, WatchEvent::CompileFailed { generation: g });
                    }
                    CompileOutcome::Green(emitted) => {
                        // Watch is always a dynamic dev build — no static plan,
                        // no tree-shaking (the full runtime tree keeps rebuilds
                        // incremental across a session's changing reach set).
                        if let Err(e) = write_emitted_project(
                            &emitted,
                            &opts.out_dir,
                            &opts.runtime_dir,
                            None,
                            false,
                        ) {
                            eprintln!(
                                "{}",
                                crate::style::gutter(&format!(
                                    "[ipe watch] failed to write emitted project: {e}"
                                ))
                            );
                            continue;
                        }
                        current_is_web = is_ipe_web_project(&emitted);
                        // Design doc "First-run vs warm-run UX": the cold
                        // (first) build pays the full dependency-compile
                        // cost and can take minutes; every subsequent
                        // rebuild is warm (seconds). Distinguishing them
                        // here means "watch is slow" is never misattributed
                        // to the salsa layer, which already ran in
                        // milliseconds by the time this line prints.
                        if generation == 1 {
                            eprintln!(
                                "{}",
                                crate::style::gutter(
                                    "[ipe watch] cold build (first run) — this can take a while…"
                                )
                            );
                        } else {
                            eprintln!("{}", crate::style::gutter("[ipe watch] rebuilding…"));
                        }
                        match spawn_cargo_build(
                            &opts.cargo_path,
                            &opts.out_dir,
                            generation,
                            evt_tx.clone(),
                        ) {
                            Ok(child) => cargo_child = Some(child),
                            Err(e) => eprintln!(
                                "{}",
                                crate::style::gutter(&format!(
                                    "[ipe watch] cannot start cargo build: {e}"
                                ))
                            ),
                        }
                    }
                }
            }

            OrchestratorEvent::CargoDone {
                generation: g,
                outcome,
            } => {
                if g != generation {
                    continue;
                }
                cargo_child = None;
                match outcome {
                    CargoOutcome::Killed => {
                        emit(opts, WatchEvent::CargoKilled { generation: g });
                    }
                    CargoOutcome::Red(msg) => {
                        eprintln!(
                            "{}",
                            crate::style::gutter(&format!(
                                "[ipe watch] cargo build failed (last-good binary stays up):\n{msg}"
                            ))
                        );
                        emit(opts, WatchEvent::CargoFailed { generation: g });
                    }
                    CargoOutcome::Green(exe_path) => {
                        let readiness = if current_is_web {
                            ipe_watch::ReadinessCheck::HttpReadyz { port: opts.port }
                        } else {
                            ipe_watch::ReadinessCheck::AliveGrace {
                                grace: Duration::from_millis(300),
                            }
                        };
                        let env = child_env(opts.port, &opts.out_dir);
                        let outcome = supervisor.apply_green(
                            &exe_path,
                            move |path| spawn_command(path, &env),
                            readiness,
                            opts.restart_timeouts,
                        );
                        report_restart_outcome(&outcome);
                        emit(
                            opts,
                            WatchEvent::Restarted {
                                generation: g,
                                outcome: restart_outcome_kind(&outcome),
                            },
                        );
                    }
                }
            }

            OrchestratorEvent::Shutdown => break,
        }
    }

    // Explicitly drop the notify watcher — and with it, its owned `raw_tx`
    // clone — BEFORE waiting on `coalesce_handle` below. `raw_tx` is moved
    // (never `.clone()`d — see `mpsc::channel` above) into `watcher`'s own
    // event callback, so `watcher` is the SOLE owner of a live sender.
    // `ipe_watch::coalesce_loop`'s blocking `raw_rx.recv()` only returns
    // once every `raw_tx` sender has been dropped; leaving `watcher` to die
    // via its ordinary end-of-function scope drop (i.e. AFTER
    // `coalesce_handle.join()` below) means that `join()` blocks FOREVER —
    // notify's own internal OS-watch thread stays parked (`epoll_wait`),
    // `raw_rx` never observes a disconnect, and the whole shutdown wedges.
    // Confirmed live via `/proc/<pid>/task/*/wchan` before this fix: the
    // notify thread parked in `ep_poll` while every sibling thread sat in
    // `futex_wait_queue_me`, and both `watch_rebuild_on_save_swaps_…` and
    // `watch_coalesces_a_rapid_double_save_…` hung past their nextest
    // SIGTERM ceiling as a direct result.
    drop(watcher);

    if let Some(child) = cargo_child.take()
        && let Ok(mut child) = child.lock()
    {
        let _ = child.kill();
    }
    supervisor.shutdown(opts.restart_timeouts);
    if let Some(h) = compile_worker {
        let _ = h.join();
    }
    let _ = coalesce_handle.join();
    Ok(())
}

/// Detection heuristic for readiness strategy: the backend's Ipe.Web entry
/// point emission always contains the literal `ipe_runtime::web::web_app`
/// call (`crates/ipe_backend_rust/src/emit_web.rs`) — deterministic,
/// compiler-controlled text, not user input, so a substring check is sound
/// here (unlike parsing arbitrary user text). Ipe.Web apps get the
/// precise `/_ipe/readyz` probe; every other shape (Ipe.Http.Server has no
/// readiness endpoint yet, and its listen port is a Ipê-source-level
/// argument this driver cannot statically know) falls back to
/// `AliveGrace` — matching the design doc's own readiness bifurcation
/// ("`/_ipe/readyz` for Ipe.Web; alive + optional health for CLI").
fn is_ipe_web_project(emitted: &ipe_backend::EmittedProject) -> bool {
    emitted
        .files
        .iter()
        .find(|(rel, _)| rel.as_str() == "src/main.rs")
        .is_some_and(|(_, text)| text.contains("ipe_runtime::web::web_app"))
}

/// Build the child process's environment.
///
/// Sets both `IPE_LIVE_PORT` and `IPE_SERVER_PORT` to the SAME configured
/// port — harmless (ignored) for whichever app shape didn't ask for it, and
/// what lets a `Ipe.Http.Server` fixture that reads `IPE_SERVER_PORT` (the
/// convention this repo's own `server_e2e.rs` test suite already
/// establishes) be driven by `--port` exactly like a Ipe.Web app is.
///
/// Also provides the watch-scoped half of session continuity —
/// default the dev session store to `sqlite` (persisted under `out_dir`,
/// confined to the emit tree's parent so the emit→cargo bridge's
/// `src/`-only prune pass never touches it) unless the caller's OWN
/// environment already configures `IPE_LIVE_STORE`, in which case that
/// choice is respected verbatim (and warned about when it is exactly
/// `memory` — see `warn_if_memory_store`, called once at watch startup).
fn child_env(port: u16, out_dir: &Path) -> Vec<(String, String)> {
    let mut env = vec![
        ("IPE_LIVE_PORT".to_owned(), port.to_string()),
        ("IPE_SERVER_PORT".to_owned(), port.to_string()),
    ];
    if std::env::var("IPE_LIVE_STORE").is_err() {
        env.push(("IPE_LIVE_STORE".to_owned(), "sqlite".to_owned()));
        let db_path = out_dir
            .parent()
            .unwrap_or(out_dir)
            .join(".ipe-watch-sessions.db");
        env.push((
            "IPE_LIVE_STORE_PATH".to_owned(),
            db_path.to_string_lossy().into_owned(),
        ));
    }
    env
}

fn spawn_command(exe_path: &Path, env: &[(String, String)]) -> Command {
    let mut cmd = Command::new(exe_path);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd
}

/// The `memory`-store warning: called once, at watch startup, so it is
/// printed exactly once per session rather than on every rebuild.
///
/// # Errors
/// Never — this only ever prints; it returns nothing failable, kept as a
/// plain function (not `Result`) so call sites don't need to thread an
/// unused error channel.
fn warn_if_memory_store() {
    if std::env::var("IPE_LIVE_STORE").as_deref() == Ok("memory") {
        eprintln!(
            "{}",
            crate::style::gutter(
                "[ipe watch] warning: IPE_LIVE_STORE=memory is set — session state will NOT \
                 survive a watch-triggered restart. Unset it (watch defaults to sqlite) or set \
                 IPE_LIVE_STORE=sqlite explicitly to keep your session across rebuilds."
            )
        );
    }
}

fn report_restart_outcome(outcome: &ipe_watch::RestartOutcome) {
    match outcome {
        ipe_watch::RestartOutcome::Spawned => {
            eprintln!("{}", crate::style::gutter("[ipe watch] app is up"));
        }
        ipe_watch::RestartOutcome::UnchangedBinary => {}
        ipe_watch::RestartOutcome::Restarted => {
            eprintln!("{}", crate::style::gutter("[ipe watch] app restarted"));
        }
        ipe_watch::RestartOutcome::RespawnedLastGood { broken } => eprintln!(
            "{}",
            crate::style::gutter(&format!(
                "[ipe watch] new binary failed its readiness probe ({}); kept the previous \
                 last-good binary running instead",
                broken.display()
            ))
        ),
        ipe_watch::RestartOutcome::NothingRunning {
            broken,
            last_good_error,
        } => {
            eprintln!(
                "{}",
                crate::style::gutter(&format!(
                    "[ipe watch] new binary failed its readiness probe ({}); no previous \
                     last-good binary could be brought up{}",
                    broken.display(),
                    last_good_error
                        .as_ref()
                        .map_or_else(String::new, |e| format!(" ({e})"))
                ))
            );
        }
    }
}

/// Project a [`ipe_watch::RestartOutcome`] down to the `Clone`-able
/// [`RestartOutcomeKind`] tests observe via [`WatchEvent::Restarted`].
const fn restart_outcome_kind(outcome: &ipe_watch::RestartOutcome) -> RestartOutcomeKind {
    match outcome {
        ipe_watch::RestartOutcome::Spawned => RestartOutcomeKind::Spawned,
        ipe_watch::RestartOutcome::UnchangedBinary => RestartOutcomeKind::UnchangedBinary,
        ipe_watch::RestartOutcome::Restarted => RestartOutcomeKind::Restarted,
        ipe_watch::RestartOutcome::RespawnedLastGood { .. } => {
            RestartOutcomeKind::RespawnedLastGood
        }
        ipe_watch::RestartOutcome::NothingRunning { .. } => RestartOutcomeKind::NothingRunning,
    }
}

/// Spawn `cargo build --message-format=json` in `out_dir` and a companion
/// waiter thread that reports completion (or "killed — superseded")
/// through `evt_tx`, tagged with `generation`. Returns a shared handle the
/// orchestrator can `.kill()` from a DIFFERENT thread while the waiter is
/// concurrently polling it — see the module doc's cancellation section.
///
/// The returned handle is wrapped in `Arc<Mutex<..>>` rather than handed
/// out as a bare `Child` because BOTH the orchestrator (kill-on-supersede)
/// and this function's own waiter thread (exit detection) need independent
/// access; the waiter deliberately uses `try_wait` in a short poll loop
/// rather than a blocking `wait()` so it never holds the lock across a
/// call that could block for the build's entire duration — holding the
/// lock there would make the orchestrator's `.kill()` block on the very
/// wait it's trying to interrupt (a real deadlock this design avoids by
/// construction, not by convention).
///
/// # Errors
/// An I/O error if the `cargo` process itself cannot be spawned.
fn spawn_cargo_build(
    cargo_path: &Path,
    out_dir: &Path,
    generation: u64,
    evt_tx: mpsc::Sender<OrchestratorEvent>,
) -> std::io::Result<Arc<std::sync::Mutex<Child>>> {
    let mut cmd = Command::new(cargo_path);
    cmd.arg("build")
        .arg("--message-format=json")
        .current_dir(out_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn()?;

    // Take the pipes now, before the `Child` moves behind the shared lock —
    // reading them on their own threads (rather than after exit) avoids the
    // classic `Command`-pipe deadlock where a full OS pipe buffer blocks the
    // child before it can exit.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let shared = Arc::new(std::sync::Mutex::new(child));
    let shared_for_waiter = Arc::clone(&shared);

    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || read_all(stderr));

    thread::spawn(move || {
        let status = loop {
            let polled = shared_for_waiter
                .lock()
                .ok()
                .and_then(|mut c| c.try_wait().ok().flatten());
            if let Some(status) = polled {
                break Some(status);
            }
            thread::sleep(Duration::from_millis(30));
        };
        let out_buf = stdout_reader.join().unwrap_or_default();
        let err_buf = stderr_reader.join().unwrap_or_default();
        let outcome = match status {
            None => CargoOutcome::Red("cargo build: could not observe exit status".to_owned()),
            Some(status) if status.success() => find_executable_path(&out_buf).map_or_else(
                || {
                    CargoOutcome::Red(
                        "cargo build succeeded but produced no executable artifact".to_owned(),
                    )
                },
                CargoOutcome::Green,
            ),
            Some(status) => {
                if is_killed_status(status) {
                    CargoOutcome::Killed
                } else {
                    CargoOutcome::Red(err_buf)
                }
            }
        };
        let _ = evt_tx.send(OrchestratorEvent::CargoDone {
            generation,
            outcome,
        });
    });

    Ok(shared)
}

/// Drain an optional pipe to a `String`, best-effort (a read failure yields
/// whatever was read so far — never a panic, never lost build output on a
/// transient short read).
fn read_all(pipe: Option<impl std::io::Read>) -> String {
    let mut buf = String::new();
    if let Some(mut s) = pipe {
        let _ = s.read_to_string(&mut buf);
    }
    buf
}

/// Whether a non-success exit status looks like "killed by us" (a signal
/// termination on unix, matching `Child::kill`'s SIGKILL) rather than a
/// genuine compile error — used to route a superseded build to
/// `CargoOutcome::Killed` (silently dropped) instead of
/// `CargoOutcome::Red` (reported as a failure INV-3 must preserve
/// last-good against).
fn is_killed_status(status: std::process::ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        status.signal().is_some()
    }
    #[cfg(not(unix))]
    {
        !status.success()
    }
}

/// Parse `cargo build --message-format=json`'s stdout for the produced
/// `executable` artifact path. Mirrors `oracle::build_rust_binary`'s own
/// parsing (that crate stays a dev-dependency only — see `Cargo.toml`'s own
/// note — so this is a small, independent re-implementation rather than a
/// production dependency on a test-utility crate).
fn find_executable_path(cargo_json_stdout: &str) -> Option<PathBuf> {
    for line in cargo_json_stdout.lines() {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        if value.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-artifact")
            && let Some(exe) = value.get("executable").and_then(serde_json::Value::as_str)
        {
            return Some(PathBuf::from(exe));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{Duration, OrchestratorEvent, RESOLVE_RETRY_DELAY, mpsc, schedule_resolve_retry};

    /// CO-INCR-008: a `resolve_project_sources` failure must not lose the
    /// rebuild cycle silently. `schedule_resolve_retry` is the orchestrator's
    /// ONLY route back into its event loop after such a failure (no
    /// filesystem event is guaranteed to follow), so this pins that a
    /// `FsBatch` retry actually lands on the channel.
    #[test]
    fn schedule_resolve_retry_sends_a_follow_up_fs_batch() {
        let (evt_tx, evt_rx) = mpsc::channel::<OrchestratorEvent>();
        schedule_resolve_retry(&evt_tx);
        let event = evt_rx
            .recv_timeout(RESOLVE_RETRY_DELAY * 4)
            .expect("a retry FsBatch must arrive — the save must not be lost");
        assert!(
            matches!(event, OrchestratorEvent::FsBatch),
            "the retry must be a FsBatch, not some other orchestrator event"
        );
    }

    /// The retry is delayed, not immediate — an instant re-send would defeat
    /// the point of a "short delay" retry (hammering a still-broken
    /// filesystem state) and would also mask a resolve failure that
    /// self-heals within one debounce window.
    #[test]
    fn schedule_resolve_retry_waits_before_sending() {
        let (evt_tx, evt_rx) = mpsc::channel::<OrchestratorEvent>();
        schedule_resolve_retry(&evt_tx);
        assert!(
            evt_rx.recv_timeout(Duration::from_millis(10)).is_err(),
            "the retry must not fire immediately"
        );
        evt_rx
            .recv_timeout(RESOLVE_RETRY_DELAY * 4)
            .expect("the retry must still arrive after the short delay");
    }
}

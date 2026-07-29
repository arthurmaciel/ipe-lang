#![forbid(unsafe_code)]
//! `ipe_watch` — the confined filesystem watcher + process supervisor that
//! power `ipe watch`.
//!
//! This crate is deliberately salsa-agnostic: it knows nothing about
//! `ipe_db`, `IpeDatabase`, or the compile pipeline. It provides three
//! independently-testable primitives that `crates/ipe/src/watch.rs` (the
//! salsa-aware orchestrator) wires together:
//!
//! - [`scope`] — the typed, project-root-confined watch allowlist
//!   (`WatchedPath`, `WatchScope`), foreclosing symlink escape (H18) and
//!   bounding watched-file count (`DoS` guard).
//! - [`coalesce`] — the debounce half: turns a storm of raw
//!   filesystem events into settled batches via a quiescence window bounded
//!   by a hard latency cap.
//! - [`process`] — the typed `SupervisorState` state machine
//!   (`NotRunning` / `Running`) plus readiness-gated restart, implementing
//!   INV-3 ("a failing rebuild never kills the running binary") and H15/H16
//!   (`RespawnLastGood` recovery from the on-disk artifact when a fresh
//!   binary fails its readiness probe).
//! - [`signal`] (unix) — a safe "run this closure on SIGTERM" listener the
//!   orchestrator's `run()` path (never `spawn()` — an in-process embedder's
//!   SIGTERM disposition must not be touched) forwards into its shutdown
//!   channel.
//!
//! Decision record: `docs/adr/0032-salsa-incremental-compilation-phase1.md`.

pub mod coalesce;
pub mod process;
pub mod scope;
pub mod signal;

pub use coalesce::{Batch, DebounceConfig, coalesce_loop};
pub use process::{
    LastGoodBinary, ReadinessCheck, RestartOutcome, RestartTimeouts, SupervisorState,
};
pub use scope::{MAX_WATCHED_FILES, ScopeError, WatchScope, WatchedPath};
#[cfg(unix)]
pub use signal::install_sigterm_forwarder;

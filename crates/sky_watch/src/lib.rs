#![forbid(unsafe_code)]
//! `sky_watch` — the confined filesystem watcher + process supervisor that
//! power `ipe watch` (incremental-compilation plan Phase E / Tasks 21-23).
//!
//! This crate is deliberately salsa-agnostic: it knows nothing about
//! `sky_db`, `SkyDatabase`, or the compile pipeline. It provides three
//! independently-testable primitives that `crates/skyc/src/watch.rs` (the
//! salsa-aware orchestrator — Tasks 22's recompute half, 24, and 25) wires
//! together:
//!
//! - [`scope`] — Task 21: the typed, project-root-confined watch allowlist
//!   (`WatchedPath`, `WatchScope`), foreclosing symlink escape (H18) and
//!   bounding watched-file count (`DoS` guard).
//! - [`coalesce`] — Task 22 (debounce half): turns a storm of raw
//!   filesystem events into settled batches via a quiescence window bounded
//!   by a hard latency cap.
//! - [`process`] — Task 23: the typed `SupervisorState` state machine
//!   (`NotRunning` / `Running`) plus readiness-gated restart, implementing
//!   INV-3 ("a failing rebuild never kills the running binary") and H15/H16
//!   (`RespawnLastGood` recovery from the on-disk artifact when a fresh
//!   binary fails its readiness probe).
//!
//! Authoritative design: `docs/architecture/incremental-compilation-and-watch.md`
//! §Q2. Phase 7 addendum:
//! `docs/architecture/salsa-incremental-compilation-2026-07-11.md`.

pub mod coalesce;
pub mod process;
pub mod scope;

pub use coalesce::{Batch, DebounceConfig, coalesce_loop};
pub use process::{
    LastGoodBinary, ReadinessCheck, RestartOutcome, RestartTimeouts, SupervisorState,
};
pub use scope::{MAX_WATCHED_FILES, ScopeError, WatchScope, WatchedPath};

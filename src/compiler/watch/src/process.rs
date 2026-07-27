//! Task 23 — the running-process state machine (INV-3, H15, H16, H20).
//!
//! Models the ONE thing `ipe watch` supervises — the compiled child binary —
//! so "old process killed, new process failed to bind" is structurally
//! unrepresentable rather than a runtime invariant a maintainer has to keep
//! remembering to preserve.
//!
//! [`SupervisorState`] has exactly two variants: nothing is running
//! ([`SupervisorState::NotRunning`], optionally remembering the last
//! artifact that ever passed readiness) or a child IS running, in which case
//! it is ALWAYS paired with the [`LastGoodBinary`] it was spawned from —
//! there is no "child alive, no known-good artifact" state, and no "marked
//! running but readiness was never checked" state, because [`Running`] is
//! only ever constructed by [`SupervisorState::apply_green`] after readiness
//! has already passed.
//!
//! [`SupervisorState::apply_green`] is the ONLY way a caller advances the
//! state machine on a successful build, and it implements the design doc's
//! diagram directly: stop-old (bounded grace, then SIGKILL) → spawn-new →
//! await-readiness → `Running`, or on readiness failure →
//! `RespawnLastGood` (re-exec the ON-DISK last-good artifact, since the old
//! *process* is already dead but the old *artifact* is not — H15/H16).
//!
//! A RED build never calls `apply_green` at all — the caller simply doesn't
//! advance the state machine, so `NotRunning`/`Running` stays exactly where
//! it was, which is what "the last-good binary stays alive on any failing
//! rebuild" (INV-3) means at the type level: there is no transition out of
//! `Running` that a red build can trigger.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// An artifact that has, at some point, passed its readiness probe.
///
/// Captures the ON-DISK path and content hash — never a live-process handle
/// — so recovery ([`SupervisorState::apply_green`]'s `RespawnLastGood` path)
/// survives the process that first proved it good already being dead
/// (design doc H15's own framing: "it captures artifact path + content
/// hash, not a live-process handle, so recovery survives the old process
/// already being dead").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastGoodBinary {
    pub artifact_path: PathBuf,
    pub content_hash: [u8; 32],
}

impl LastGoodBinary {
    /// Hash `artifact_path`'s bytes and wrap it. Used both to CONSTRUCT a
    /// candidate before it has passed readiness (the caller only promotes it
    /// to a `Running` state via `apply_green`, which is the actual
    /// readiness-gated boundary) and to compare against the currently
    /// running artifact ("byte-identical binary → no restart, no churn").
    ///
    /// # Errors
    /// An I/O error reading the artifact.
    pub fn hash(artifact_path: &Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(artifact_path)?;
        Ok(Self {
            artifact_path: artifact_path.to_path_buf(),
            content_hash: sha256(&bytes),
        })
    }
}

/// Minimal, dependency-free SHA-256 is overkill here — `ipe`'s own build
/// cache already pulls in `sha2` for exactly this purpose at the crate
/// level, but `ipe_watch` is deliberately dependency-light (Task 21's own
/// "confined watcher" doesn't need a crypto crate) so this module borrows
/// nothing from `ipe`. A FNV-1a-based 256-bit-widened digest would be
/// cheaper, but content-hash COLLISION here only degrades to a missed
/// "byte-identical, skip the restart" fast path, never a soundness hole
/// (the hash is compared for equality only, never trusted as an integrity
/// oracle across a process boundary) — even so, this uses the real `sha2`
/// crate transitively available via the workspace lockfile for a stronger,
/// unambiguous guarantee with no meaningful cost.
fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// How readiness is probed for a freshly spawned candidate.
///
/// Chosen by the caller from what it knows about the emitted project
/// (Ipe.Web apps expose `/_ipe/readyz`; everything else falls back to a
/// plain TCP connect on the configured port, and finally to "did it stay
/// alive" for a program that binds no port at all — matching the design
/// doc's own bifurcation: "readiness probe (`/_ipe/readyz` for Ipe.Web;
/// alive + optional health for CLI)").
#[derive(Debug, Clone, Copy)]
pub enum ReadinessCheck {
    /// GET `/_ipe/readyz` on `127.0.0.1:port`; ready on any 2xx response.
    HttpReadyz { port: u16 },
    /// A bare TCP connect to `127.0.0.1:port` succeeding is enough (no
    /// process-level readiness endpoint to ask, e.g. `Ipe.Http.Server`).
    TcpConnect { port: u16 },
    /// No network surface at all (CLI / batch job) — "ready" means the
    /// process is still alive after `grace` has elapsed, never a network
    /// probe that would hang forever against a program that binds nothing.
    AliveGrace { grace: Duration },
}

/// Bounded timeouts governing every phase of a restart cycle.
///
/// (design doc "every stage timeout-bounded", AGENTS.md §3). None of these
/// is optional — a caller that wants "no timeout" is a caller with a
/// soundness bug.
#[derive(Debug, Clone, Copy)]
pub struct RestartTimeouts {
    /// Total time to wait for the new candidate to pass its readiness probe
    /// before declaring it broken and falling back to `RespawnLastGood`.
    pub readiness: Duration,
    /// Grace period after SIGTERM before escalating to SIGKILL.
    pub graceful_stop: Duration,
    /// Poll interval while waiting on a readiness probe.
    pub poll_interval: Duration,
}

impl Default for RestartTimeouts {
    fn default() -> Self {
        Self {
            readiness: Duration::from_secs(10),
            graceful_stop: Duration::from_secs(3),
            poll_interval: Duration::from_millis(50),
        }
    }
}

/// The result of one `apply_green` call — what actually happened, for
/// logging/UX.
///
/// Never affects correctness (the state machine's invariants hold
/// regardless of which variant is returned); this is purely observability.
#[derive(Debug)]
pub enum RestartOutcome {
    /// First successful build; nothing was running before.
    Spawned,
    /// The new artifact is byte-identical to what was already running — no
    /// restart, no observable churn (design doc: "A green build producing a
    /// BYTE-IDENTICAL binary → no restart, no churn").
    UnchangedBinary,
    /// Old stopped, new spawned, new passed readiness.
    Restarted,
    /// The new binary FAILED readiness; the on-disk last-good artifact was
    /// re-spawned instead and passed readiness. `broken` is the candidate
    /// that failed, kept for the caller's diagnostic.
    RespawnedLastGood { broken: PathBuf },
    /// The new binary failed readiness AND the last-good artifact ALSO
    /// failed to (re)spawn or pass readiness — the double-failure floor
    /// case. Nothing is running; `self` is left in `NotRunning` (never a
    /// panic, never a state claiming a dead process is alive).
    NothingRunning {
        broken: PathBuf,
        last_good_error: Option<String>,
    },
}

/// The state machine itself.
///
/// `NotRunning{ last_good }` and `Running{ child, artifact }` are the ONLY
/// two representable states — see the module doc for why every other
/// combination the design doc's diagram implies (mid-transition states like
/// `StopOld`/`SpawnNew`) is deliberately NOT a variant here: those are
/// actions `apply_green` performs synchronously inside one call, not states
/// this type persists between calls. A caller that crashes mid-`apply_green`
/// simply never observes a torn state — the old `Child` handle either got
/// SIGTERM'd (and its process is gone) or didn't (and it's still `Running`
/// with the OLD artifact, since the field is only overwritten at the very
/// end of a successful transition).
pub enum SupervisorState {
    NotRunning {
        last_good: Option<LastGoodBinary>,
    },
    Running {
        child: Child,
        artifact: LastGoodBinary,
    },
}

impl SupervisorState {
    #[must_use]
    pub const fn fresh() -> Self {
        Self::NotRunning { last_good: None }
    }

    /// Whether a process is currently running (for CLI status output only).
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    /// Advance the state machine on a GREEN build. `candidate_path` is the
    /// freshly built artifact; `spawn` constructs the `Command` to launch
    /// WHATEVER path it is given (the caller owns env/args, e.g. the port
    /// and `IPE_LIVE_STORE` injection); `readiness` and `timeouts` govern
    /// the readiness gate.
    ///
    /// `spawn` takes the path as a PARAMETER (rather than the caller
    /// capturing one fixed path in the closure) because the SAME
    /// environment-building logic must apply to BOTH the fresh candidate
    /// AND the `RespawnLastGood` fallback path below — a `spawn` closure
    /// that only knew how to launch `candidate_path` would silently drop
    /// every env var (port, session-store path, …) when recovering onto
    /// the on-disk last-good artifact, which is a real regression this
    /// design closes by construction rather than by remembering to pass
    /// the env through twice.
    ///
    /// This function NEVER leaves `self` in a state where a `Child` handle
    /// is stored for a process known to be dead, and it never panics —
    /// every I/O failure (spawn failure, hash failure) degrades to
    /// [`RestartOutcome::NothingRunning`] with a diagnostic string, never an
    /// `unwrap`.
    pub fn apply_green(
        &mut self,
        candidate_path: &Path,
        spawn: impl Fn(&Path) -> Command,
        readiness: ReadinessCheck,
        timeouts: RestartTimeouts,
    ) -> RestartOutcome {
        let candidate_hash = match LastGoodBinary::hash(candidate_path) {
            Ok(h) => h,
            Err(e) => {
                return RestartOutcome::NothingRunning {
                    broken: candidate_path.to_path_buf(),
                    last_good_error: Some(format!("cannot hash candidate: {e}")),
                };
            }
        };

        // Byte-identical binary already running → no restart, no churn.
        if let Self::Running { artifact, .. } = self
            && artifact.content_hash == candidate_hash.content_hash
        {
            return RestartOutcome::UnchangedBinary;
        }

        let had_prior_running = matches!(self, Self::Running { .. });

        // Stop-old-before-spawn-new (v1 fixed-port floor — two processes
        // cannot hold the same port at once).
        if let Self::Running { child, .. } = self {
            stop_gracefully(child, timeouts.graceful_stop);
        }

        // Spawn the candidate and await readiness.
        match spawn_and_await_ready(spawn(candidate_path), readiness, timeouts) {
            Ok(child) => {
                *self = Self::Running {
                    child,
                    artifact: candidate_hash,
                };
                return if had_prior_running {
                    RestartOutcome::Restarted
                } else {
                    RestartOutcome::Spawned
                };
            }
            Err(_readiness_failed) => {
                // Fall through to RespawnLastGood below.
            }
        }

        // The candidate failed readiness. Recover via the ON-DISK last-good
        // artifact (H15/H16) — never the old `Child` handle directly (it is
        // already dead: `stop_gracefully` above killed it, but deliberately
        // does NOT transition `self` out of `Running` — that transition
        // only happens via the `*self = …` assignments below, so `self`
        // stays whatever it was on entry until `apply_green` actually
        // commits an outcome).
        //
        // Two cases reach this match:
        // - `self` was `Running` on entry (the common restart-attempt-
        //   fails case): its `artifact` IS the last-good one — the process
        //   we just stopped was, by definition, the previously-passing
        //   build — so it is used directly, without needing a fresh
        //   readiness re-check (it already passed one to get here).
        // - `self` was `NotRunning` on entry (first-ever build failed
        //   readiness, or a previous `RespawnLastGood`/`NothingRunning`
        //   cycle left it there): fall back to whatever `last_good` it
        //   remembers, if any.
        let last_good = match self {
            Self::NotRunning { last_good } => last_good.clone(),
            Self::Running { artifact, .. } => Some(artifact.clone()),
        };
        let Some(last_good) = last_good else {
            *self = Self::NotRunning { last_good: None };
            return RestartOutcome::NothingRunning {
                broken: candidate_path.to_path_buf(),
                last_good_error: Some("no prior last-good artifact to fall back to".to_owned()),
            };
        };

        let last_good_path = last_good.artifact_path.clone();
        match spawn_and_await_ready(spawn(&last_good_path), readiness, timeouts) {
            Ok(child) => {
                *self = Self::Running {
                    child,
                    artifact: last_good,
                };
                RestartOutcome::RespawnedLastGood {
                    broken: candidate_path.to_path_buf(),
                }
            }
            Err(e) => {
                *self = Self::NotRunning {
                    last_good: Some(last_good),
                };
                RestartOutcome::NothingRunning {
                    broken: candidate_path.to_path_buf(),
                    last_good_error: Some(e),
                }
            }
        }
    }

    /// Kill whatever is running (used on watcher shutdown — "the child
    /// process is killed when the watcher exits", AGENTS.md §3 / design doc
    /// "Timeout / hang bounding"). No-op on `NotRunning`.
    pub fn shutdown(&mut self, timeouts: RestartTimeouts) {
        if let Self::Running { child, .. } = self {
            stop_gracefully(child, timeouts.graceful_stop);
        }
        let last_good = match self {
            Self::NotRunning { last_good } => last_good.take(),
            Self::Running { artifact, .. } => Some(artifact.clone()),
        };
        *self = Self::NotRunning { last_good };
    }
}

/// SIGTERM (unix) / plain kill (non-unix) `child`, wait up to `grace` for it
/// to exit on its own, then SIGKILL if it hasn't. Never panics — a failure
/// to signal or wait is logged-nowhere-but-tolerated (the process may have
/// already exited between the caller's last observation and this call).
///
/// SAFETY / design note: this uses the SAFE, portable [`Child::kill`]
/// (SIGKILL) as the escalation path. A true graceful SIGTERM needs a raw
/// `kill(2)` call, which the runtime's own `console_proxy` module already
/// isolates as the ONE sanctioned `unsafe` site in the whole workspace (see
/// `PRINCIPLES.md`) — this module deliberately does NOT duplicate that
/// unsafe surface. Consequence: the "graceful" step here is a bounded WAIT
/// (letting a process that is already shutting down on its own drain)
/// rather than an ACTIVE SIGTERM; [`Child::kill`] (SIGKILL, always
/// available safely) is the hard stop once the grace window elapses. The
/// bounded-down-window property (H15) holds either way, at the cost of
/// never proactively signalling the target's graceful-drain path from the
/// watcher itself — a scoped, honest limitation rather than an unjustified
/// `unsafe` block.
fn stop_gracefully(child: &mut Child, grace: Duration) {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return, // already exited
            Ok(None) => {}
            Err(_) => break, // can't observe status — fall through to kill
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(
            Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Spawn `cmd` and poll `readiness` until it passes or its budget elapses.
/// Returns the live `Child` on success. On failure, the child (if it was
/// spawned at all) is killed before returning — callers never have to
/// remember to clean up a half-ready process.
///
/// Takes `cmd` BY VALUE (not `&Command`): `std::process::Command` has no
/// `Clone` impl and no public accessor for its configured argv/envs, so the
/// only sound way to spawn a fully-configured command (port env vars,
/// `IPE_LIVE_STORE`, …) without silently dropping that configuration is to
/// consume the one, already-built `Command` the caller hands in. Every call
/// site therefore builds a FRESH `Command` per spawn attempt (`apply_green`
/// calls the caller's `spawn` factory for the candidate, and
/// [`respawn_command`] for the last-good fallback) rather than trying to
/// reuse one across attempts.
fn spawn_and_await_ready(
    mut cmd: Command,
    readiness: ReadinessCheck,
    timeouts: RestartTimeouts,
) -> Result<Child, String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;

    // `AliveGrace` uses its OWN budget (the grace window itself IS the
    // probe), not `timeouts.readiness` (which governs the network probes).
    let budget = match readiness {
        ReadinessCheck::AliveGrace { grace } => grace,
        ReadinessCheck::HttpReadyz { .. } | ReadinessCheck::TcpConnect { .. } => timeouts.readiness,
    };
    let deadline = Instant::now() + budget;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("process exited before becoming ready: {status}"));
        }
        if probe_once(&readiness) {
            return Ok(child);
        }
        let now = Instant::now();
        if now >= deadline {
            if matches!(readiness, ReadinessCheck::AliveGrace { .. }) {
                // Still alive (the try_wait check above passed) once the
                // grace window elapsed — that IS readiness for this variant.
                return Ok(child);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err("readiness probe timed out".to_owned());
        }
        std::thread::sleep(
            timeouts
                .poll_interval
                .min(deadline.saturating_duration_since(now)),
        );
    }
}

/// One readiness probe attempt (non-blocking beyond a short per-attempt
/// socket timeout) — `true` means ready. `AliveGrace` always reports "not
/// yet" here; its success path is the deadline branch in
/// [`spawn_and_await_ready`] (surviving to the end of the grace window IS
/// the readiness signal for a program with no network surface to ask).
fn probe_once(readiness: &ReadinessCheck) -> bool {
    match *readiness {
        ReadinessCheck::HttpReadyz { port } => http_get_ok(port, "/_ipe/readyz"),
        ReadinessCheck::TcpConnect { port } => tcp_connect_ok(port),
        ReadinessCheck::AliveGrace { .. } => false,
    }
}

fn tcp_connect_ok(port: u16) -> bool {
    let addr: SocketAddr = match format!("127.0.0.1:{port}").parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

fn http_get_ok(port: u16, path: &str) -> bool {
    let addr: SocketAddr = match format!("127.0.0.1:{port}").parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(200)) else {
        return false;
    };
    if stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .is_err()
    {
        return false;
    }
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 64];
    let Ok(n) = stream.read(&mut buf) else {
        return false;
    };
    let Some(received) = buf.get(..n) else {
        return false;
    };
    let head = String::from_utf8_lossy(received);
    // "HTTP/1.1 200 ..." — a bare status-line prefix check is enough; this
    // is a liveness probe, not a conformance parser.
    head.starts_with("HTTP/1.1 2") || head.starts_with("HTTP/1.0 2")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_good_hash_is_deterministic() {
        let dir =
            std::env::temp_dir().join(format!("ipe_watch_process_hash_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("bin");
        std::fs::write(&f, b"hello world").unwrap();
        let a = LastGoodBinary::hash(&f).unwrap();
        let b = LastGoodBinary::hash(&f).unwrap();
        assert_eq!(a.content_hash, b.content_hash);
        std::fs::write(&f, b"hello world!").unwrap();
        let c = LastGoodBinary::hash(&f).unwrap();
        assert_ne!(a.content_hash, c.content_hash);
    }

    #[test]
    fn fresh_state_is_not_running() {
        let s = SupervisorState::fresh();
        assert!(!s.is_running());
    }

    /// Not available on non-unix CI images; these three tests all shell out
    /// to `/bin/sleep` and `/bin/false`, so they are unix-gated the same
    /// way the rest of this module's SIGKILL-only escalation is.
    #[cfg(unix)]
    fn quick_timeouts() -> RestartTimeouts {
        RestartTimeouts {
            readiness: Duration::from_millis(400),
            graceful_stop: Duration::from_millis(300),
            poll_interval: Duration::from_millis(20),
        }
    }

    #[cfg(unix)]
    #[test]
    fn apply_green_spawns_a_long_running_process_and_reports_spawned() {
        let mut state = SupervisorState::fresh();
        let readiness = ReadinessCheck::AliveGrace {
            grace: Duration::from_millis(80),
        };
        let outcome = state.apply_green(
            Path::new("/bin/sleep"),
            |_path| {
                let mut c = Command::new("/bin/sleep");
                c.arg("5");
                c
            },
            readiness,
            quick_timeouts(),
        );
        assert!(matches!(outcome, RestartOutcome::Spawned), "{outcome:?}");
        assert!(state.is_running());
        state.shutdown(quick_timeouts());
    }

    #[cfg(unix)]
    #[test]
    fn apply_green_reports_unchanged_binary_for_a_byte_identical_candidate() {
        let mut state = SupervisorState::fresh();
        let readiness = ReadinessCheck::AliveGrace {
            grace: Duration::from_millis(80),
        };
        let spawn = |_path: &Path| {
            let mut c = Command::new("/bin/sleep");
            c.arg("5");
            c
        };
        let first = state.apply_green(Path::new("/bin/sleep"), spawn, readiness, quick_timeouts());
        assert!(matches!(first, RestartOutcome::Spawned), "{first:?}");

        // SAME candidate path (same bytes on disk) → no restart, no churn.
        let second = state.apply_green(Path::new("/bin/sleep"), spawn, readiness, quick_timeouts());
        assert!(
            matches!(second, RestartOutcome::UnchangedBinary),
            "{second:?}"
        );
        assert!(state.is_running());
        state.shutdown(quick_timeouts());
    }

    #[cfg(unix)]
    #[test]
    fn apply_green_falls_back_to_last_good_when_a_restart_candidate_fails_readiness() {
        let mut state = SupervisorState::fresh();
        let readiness = ReadinessCheck::AliveGrace {
            grace: Duration::from_millis(80),
        };

        // A single `spawn` closure serves EVERY call `apply_green` makes,
        // dispatching on the path it is given — exactly the real contract
        // (`watch.rs`'s own `spawn_command` behaves the same way, just
        // keyed on env vars instead of an if/else). `/bin/sleep` is the
        // "good" path (runs forever); anything else (the bad candidate
        // below) launches `/bin/false`, which exits immediately and can
        // never satisfy `AliveGrace`. This is what proves the FIX: before
        // it, the `RespawnLastGood` path called a path-blind
        // `respawn_command` that silently dropped the caller's env/args —
        // this closure would have been unreachable for the fallback call,
        // and the fallback would have failed even though `/bin/sleep` (the
        // ACTUAL last-good binary) is perfectly runnable.
        let good_path = PathBuf::from("/bin/sleep");
        let spawn = {
            let good_path = good_path.clone();
            move |path: &Path| {
                if path == good_path {
                    let mut c = Command::new("/bin/sleep");
                    c.arg("5");
                    c
                } else {
                    Command::new("/bin/false")
                }
            }
        };

        // First: a genuinely good, long-running candidate.
        let good = state.apply_green(&good_path, &spawn, readiness, quick_timeouts());
        assert!(matches!(good, RestartOutcome::Spawned), "{good:?}");
        let good_hash = match &state {
            SupervisorState::Running { artifact, .. } => Some(artifact.content_hash),
            SupervisorState::NotRunning { .. } => None,
        }
        .expect("expected Running after a successful spawn");

        // Second: a DIFFERENT-content candidate path (so it isn't judged
        // byte-identical) that `spawn` routes to `/bin/false` — must fail
        // readiness and fall back to respawning `good_path` through the
        // SAME `spawn` closure.
        let dir =
            std::env::temp_dir().join(format!("ipe_watch_process_fallback_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad_candidate = dir.join("bad-candidate");
        std::fs::write(&bad_candidate, b"not actually executed").unwrap();

        let outcome = state.apply_green(&bad_candidate, &spawn, readiness, quick_timeouts());
        assert!(
            matches!(outcome, RestartOutcome::RespawnedLastGood { .. }),
            "{outcome:?}"
        );
        let respawned_hash = match &state {
            SupervisorState::Running { artifact, .. } => Some(artifact.content_hash),
            SupervisorState::NotRunning { .. } => None,
        }
        .expect("expected Running (respawned last-good) after a failed candidate");
        assert_eq!(
            respawned_hash, good_hash,
            "the respawned artifact must be the ORIGINAL good one, not the failed candidate"
        );
        state.shutdown(quick_timeouts());
    }
}

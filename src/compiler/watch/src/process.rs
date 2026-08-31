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
    /// Intended for deterministic tests: spin-polls `try_wait` for a fixed
    /// number of iterations with no wall-clock deadline, then declares the
    /// process ready if it has not exited.  Tests pair this with a spawn
    /// closure that causes readiness failure via a spawn error (nonexistent
    /// binary) rather than relying on process-exit timing, eliminating the
    /// race between process reaping and readiness polling.
    #[cfg(test)]
    AliveImmediate,
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

impl RestartTimeouts {
    /// The timeouts a live-reload dev watch uses. `ipe watch` is a DEV-only loop,
    /// so the stop is aggressive: no drain grace. The old server holds the port
    /// the rebuilt one must bind, and every millisecond draining it is latency on
    /// the critical path from save to reloaded app; a mid-flight dev request is
    /// worth nothing next to reload speed. A zero grace means SIGTERM followed
    /// immediately by SIGKILL — the port is freed at once. The readiness budget
    /// stays generous so a slow first serve is not misjudged as broken, and the
    /// readiness poll is tightened so "up" is detected the instant it binds.
    #[must_use]
    pub fn for_dev_watch() -> Self {
        Self {
            graceful_stop: Duration::from_millis(0),
            poll_interval: Duration::from_millis(15),
            ..Self::default()
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
    /// and `IPE_WEB_STORE` injection); `readiness` and `timeouts` govern
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

    /// Advance the state machine on a GREEN build in the BLUE-GREEN dev path:
    /// the new binary is spawned on its OWN internal port, BEHIND the
    /// persistent front proxy, so the old binary keeps serving (on its own
    /// internal port) until the new one is ready. Only once the new binary
    /// passes readiness is `cut_over` called (the caller flips the proxy's
    /// upstream), and only THEN is the old binary drained — so the user-facing
    /// port never stops answering and no client connection is dropped by a
    /// port handoff.
    ///
    /// This is the deliberate inverse of [`apply_green`]'s stop-old-before-
    /// spawn-new ordering (`apply_green` shares ONE fixed port, so the two
    /// binaries cannot coexist; here they bind DIFFERENT internal ports and
    /// overlap on purpose).
    ///
    /// `spawn` builds the launch `Command` for a given `(path, internal_port)`
    /// — the caller owns the env (it injects `IPE_WEB_PORT=internal_port`, the
    /// session-store path, …). `cut_over` is invoked with the ready internal
    /// port EXACTLY ONCE, between "new binary ready" and "old binary drained".
    ///
    /// On readiness failure the new binary is discarded and the OLD one is
    /// left running untouched (INV-3 at the type level — same guarantee a red
    /// build gives): `self` stays `Running` with the old child, `cut_over` is
    /// never called, and the proxy keeps pointing at the still-live old
    /// upstream. Never panics; every I/O failure degrades to a
    /// [`RestartOutcome`] variant, never an `unwrap`.
    ///
    /// [`apply_green`]: SupervisorState::apply_green
    pub fn apply_green_behind_proxy(
        &mut self,
        candidate_path: &Path,
        internal_port: u16,
        spawn: impl Fn(&Path, u16) -> Command,
        readiness: ReadinessCheck,
        timeouts: RestartTimeouts,
        cut_over: impl FnOnce(u16),
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

        // Byte-identical binary already running → no restart, no churn (the
        // proxy already points at it).
        if let Self::Running { artifact, .. } = self
            && artifact.content_hash == candidate_hash.content_hash
        {
            return RestartOutcome::UnchangedBinary;
        }

        let had_prior_running = matches!(self, Self::Running { .. });

        // Spawn the NEW binary on its own internal port and await readiness —
        // WITHOUT stopping the old one first (they hold different ports).
        match spawn_and_await_ready(spawn(candidate_path, internal_port), readiness, timeouts) {
            Ok(new_child) => {
                // The new binary is ready. Flip the proxy's upstream to it —
                // this is the zero-drop cutover: the user-facing port has been
                // held by the proxy the whole time.
                cut_over(internal_port);
                // Drain the OLD binary: the proxy now routes to the new one,
                // so nothing reaches the old port.
                if let Self::Running { child, .. } = self {
                    stop_gracefully(child, timeouts.graceful_stop);
                }
                *self = Self::Running {
                    child: new_child,
                    artifact: candidate_hash,
                };
                if had_prior_running {
                    RestartOutcome::Restarted
                } else {
                    RestartOutcome::Spawned
                }
            }
            Err(_readiness_failed) => {
                // The new binary never became ready. Leave the OLD one exactly
                // as it was (still `Running`, still routed to by the proxy):
                // no cutover, no drain. This is the INV-3 guarantee — a build
                // that produces a non-ready binary never disturbs the live
                // one. When nothing was running before (first build failed
                // readiness), report the double-failure floor.
                if had_prior_running {
                    RestartOutcome::RespawnedLastGood {
                        broken: candidate_path.to_path_buf(),
                    }
                } else {
                    *self = Self::NotRunning { last_good: None };
                    RestartOutcome::NothingRunning {
                        broken: candidate_path.to_path_buf(),
                        last_good_error: Some(
                            "new binary failed readiness and nothing was running to keep"
                                .to_owned(),
                        ),
                    }
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
/// `IPE_WEB_STORE`, …) without silently dropping that configuration is to
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

    // `AliveImmediate` (test-only): confirm the process is alive with a
    // tight `try_wait` loop, then declare it ready.  Tests that use this
    // variant ensure readiness failure via a spawn error (nonexistent
    // binary → `cmd.spawn()` fails above, never reaching here) rather
    // than relying on process-exit timing.  The loop guards against the
    // narrow window where a process that exits immediately is not yet
    // surfaced by the first `try_wait`, giving the OS scheduler room to
    // surface the exit before we declare readiness.
    #[cfg(test)]
    if matches!(readiness, ReadinessCheck::AliveImmediate) {
        const ALIVE_POLLS: usize = 64;
        for _ in 0..ALIVE_POLLS {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(format!("process exited before becoming ready: {status}"));
                }
                Ok(None) => {} // still alive — keep polling
                Err(e) => {
                    return Err(format!("try_wait error: {e}"));
                }
            }
        }
        return Ok(child);
    }

    // `AliveGrace` uses its OWN budget (the grace window itself IS the
    // probe), not `timeouts.readiness` (which governs the network probes).
    let budget = match readiness {
        ReadinessCheck::AliveGrace { grace } => grace,
        ReadinessCheck::HttpReadyz { .. } | ReadinessCheck::TcpConnect { .. } => timeouts.readiness,
        // Handled by the early `AliveImmediate` return above; a safe budget here
        // keeps the match total without an abrupt-failure construct.
        #[cfg(test)]
        ReadinessCheck::AliveImmediate => timeouts.readiness,
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
            // For alive-based variants the deadline IS the readiness signal:
            // if the process is still alive at this point (the `try_wait`
            // above passed) it is considered ready.
            if is_alive_based_readiness(&readiness) {
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

/// True for readiness variants whose success condition is "process still alive
/// at the deadline" rather than a positive network probe. The deadline IS the
/// signal for these variants; `probe_once` always returns `false` for them.
const fn is_alive_based_readiness(readiness: &ReadinessCheck) -> bool {
    match readiness {
        ReadinessCheck::AliveGrace { .. } => true,
        ReadinessCheck::HttpReadyz { .. } | ReadinessCheck::TcpConnect { .. } => false,
        #[cfg(test)]
        ReadinessCheck::AliveImmediate => true,
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
        #[cfg(test)]
        ReadinessCheck::AliveImmediate => false,
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
        let outcome = state.apply_green(
            Path::new("/bin/sleep"),
            |_path| {
                let mut c = Command::new("/bin/sleep");
                c.arg("5");
                c
            },
            ReadinessCheck::AliveImmediate,
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
        let spawn = |_path: &Path| {
            let mut c = Command::new("/bin/sleep");
            c.arg("5");
            c
        };
        let first = state.apply_green(
            Path::new("/bin/sleep"),
            spawn,
            ReadinessCheck::AliveImmediate,
            quick_timeouts(),
        );
        assert!(matches!(first, RestartOutcome::Spawned), "{first:?}");

        // SAME candidate path (same bytes on disk) → no restart, no churn.
        let second = state.apply_green(
            Path::new("/bin/sleep"),
            spawn,
            ReadinessCheck::AliveImmediate,
            quick_timeouts(),
        );
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

        // The spawn closure dispatches on path — exactly the real contract
        // (`watch.rs`'s `spawn_command` works the same way, keyed on env vars
        // rather than an if/else).  The `RespawnLastGood` path calls the SAME
        // closure with `good_path` to re-exec the last-good artifact; that is
        // what this test exercises.
        //
        // The bad-candidate command deliberately points to a nonexistent
        // binary so that `spawn()` itself fails (an I/O error, not a
        // process-exit race).  A spawn failure is caught inside
        // `spawn_and_await_ready` before any process is created, making
        // the readiness failure 100% deterministic regardless of OS load
        // or scheduler timing.  The last-good respawn uses `/bin/sleep 5`,
        // which stays alive and passes `AliveImmediate`'s spin-poll.
        let good_path = PathBuf::from("/bin/sleep");
        let spawn = {
            let good_path = good_path.clone();
            move |path: &Path| {
                if path == good_path {
                    let mut c = Command::new("/bin/sleep");
                    c.arg("5");
                    c
                } else {
                    // Nonexistent binary → `spawn()` returns Err immediately,
                    // no process is ever created, readiness fails without any
                    // timing dependency.
                    Command::new("/nonexistent/__ipe_watch_test_bad_candidate__")
                }
            }
        };

        // Step 1: establish the last-good state with a successfully running process.
        let good = state.apply_green(
            &good_path,
            &spawn,
            ReadinessCheck::AliveImmediate,
            quick_timeouts(),
        );
        assert!(matches!(good, RestartOutcome::Spawned), "{good:?}");
        let good_hash = match &state {
            SupervisorState::Running { artifact, .. } => Some(artifact.content_hash),
            SupervisorState::NotRunning { .. } => None,
        }
        .expect("expected Running after a successful spawn");

        // Step 2: present a DIFFERENT-content candidate (a distinct on-disk
        // file so it is not judged byte-identical) that the spawn closure
        // routes to a nonexistent binary.  The spawn fails immediately
        // (before any process is created), which is the same `Err` path
        // inside `spawn_and_await_ready` as a process-exit readiness
        // failure — fully deterministic, no OS timing involved.  The state
        // machine then respawns the last-good artifact (`/bin/sleep 5`)
        // which stays alive and passes readiness.
        let dir =
            std::env::temp_dir().join(format!("ipe_watch_process_fallback_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad_candidate = dir.join("bad-candidate");
        std::fs::write(&bad_candidate, b"not actually executed").unwrap();

        let outcome = state.apply_green(
            &bad_candidate,
            &spawn,
            ReadinessCheck::AliveImmediate,
            quick_timeouts(),
        );
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

    /// The blue-green path cuts over ONLY after the new binary is ready — the
    /// `cut_over` callback must fire exactly once, with the new internal port,
    /// and the state must end `Running` on the new binary.
    #[cfg(unix)]
    #[test]
    fn apply_green_behind_proxy_cuts_over_after_readiness() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU16, Ordering};

        let mut state = SupervisorState::fresh();
        let cut_to = Arc::new(AtomicU16::new(0));
        let cut_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let spawn = |_path: &Path, _port: u16| {
            let mut c = Command::new("/bin/sleep");
            c.arg("5");
            c
        };

        let internal = 41999;
        let cut_to_cb = Arc::clone(&cut_to);
        let cut_calls_cb = Arc::clone(&cut_calls);
        let outcome = state.apply_green_behind_proxy(
            Path::new("/bin/sleep"),
            internal,
            spawn,
            ReadinessCheck::AliveImmediate,
            quick_timeouts(),
            move |port| {
                cut_to_cb.store(port, Ordering::SeqCst);
                cut_calls_cb.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert!(matches!(outcome, RestartOutcome::Spawned), "{outcome:?}");
        assert!(state.is_running());
        assert_eq!(
            cut_calls.load(Ordering::SeqCst),
            1,
            "cut_over must fire exactly once on a ready cutover"
        );
        assert_eq!(
            cut_to.load(Ordering::SeqCst),
            internal,
            "cut_over must be called with the new binary's internal port"
        );
        state.shutdown(quick_timeouts());
    }

    /// INV-3 for the blue-green path: when the new binary fails readiness the
    /// OLD one is left untouched and the proxy is NEVER cut over — no dropped
    /// connection, no torn state.
    #[cfg(unix)]
    #[test]
    fn apply_green_behind_proxy_keeps_old_and_never_cuts_over_on_readiness_failure() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mut state = SupervisorState::fresh();
        let good_path = PathBuf::from("/bin/sleep");
        let cut_calls = Arc::new(AtomicUsize::new(0));

        // First, establish a running old binary via the same blue-green path.
        let spawn_good = |_path: &Path, _port: u16| {
            let mut c = Command::new("/bin/sleep");
            c.arg("5");
            c
        };
        let cc = Arc::clone(&cut_calls);
        let first = state.apply_green_behind_proxy(
            &good_path,
            40001,
            spawn_good,
            ReadinessCheck::AliveImmediate,
            quick_timeouts(),
            move |_| {
                cc.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert!(matches!(first, RestartOutcome::Spawned), "{first:?}");
        let old_hash = match &state {
            SupervisorState::Running { artifact, .. } => Some(artifact.content_hash),
            SupervisorState::NotRunning { .. } => None,
        }
        .expect("expected Running after the initial blue-green spawn");
        assert_eq!(cut_calls.load(Ordering::SeqCst), 1);

        // Now a candidate whose spawn fails immediately (nonexistent binary) —
        // readiness can never pass.
        let dir = std::env::temp_dir().join(format!("ipe_watch_bg_fail_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad");
        std::fs::write(&bad, b"distinct-bytes").unwrap();
        let spawn_bad =
            |_path: &Path, _port: u16| Command::new("/nonexistent/__ipe_watch_bg_bad__");
        let cc2 = Arc::clone(&cut_calls);
        let outcome = state.apply_green_behind_proxy(
            &bad,
            40002,
            spawn_bad,
            ReadinessCheck::AliveImmediate,
            quick_timeouts(),
            move |_| {
                cc2.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert!(
            matches!(outcome, RestartOutcome::RespawnedLastGood { .. }),
            "a failed candidate reports the old binary is kept: {outcome:?}"
        );
        // The OLD binary is still running, unchanged, and NO extra cutover
        // fired (still exactly the one from the initial spawn).
        let now_hash = match &state {
            SupervisorState::Running { artifact, .. } => Some(artifact.content_hash),
            SupervisorState::NotRunning { .. } => None,
        }
        .expect("the old binary must stay Running after a failed candidate");
        assert_eq!(now_hash, old_hash, "the old binary must be untouched");
        assert_eq!(
            cut_calls.load(Ordering::SeqCst),
            1,
            "a failed candidate must NEVER cut the proxy over"
        );
        state.shutdown(quick_timeouts());
    }
}

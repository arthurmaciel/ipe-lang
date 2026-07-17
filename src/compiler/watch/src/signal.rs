//! A SAFE SIGTERM listener.
//!
//! The salsa-agnostic primitive `ipe watch`'s orchestrator wires to its own
//! shutdown channel so a supervisor's `kill -TERM <pid>` (systemd's default
//! stop signal, delivered to the PID only, not the process group) runs the
//! full orderly teardown instead of a hard kernel kill that orphans the
//! supervised child.
//!
//! Unix-only: SIGTERM has no equivalent OS-level concept on Windows (Ctrl-C
//! there is a distinct console-event API this module deliberately does not
//! attempt to unify with — out of scope, matching this crate's existing
//! unix-gated SIGKILL escalation in `process.rs`).
//!
//! No `unsafe`, compatible with this crate's `#![forbid(unsafe_code)]`:
//! `signal-hook`'s public surface (`Signals::new` + the `forever()`
//! iterator) is ordinary safe Rust from the caller's side; the `sigaction(2)`
//! FFI it needs lives inside `signal-hook-registry`, a dependency crate this
//! crate's own forbid attribute never reaches or needs to reach.

#[cfg(unix)]
use std::thread::JoinHandle;

/// Spawn a dedicated OS thread that blocks for SIGTERM and, on receipt,
/// invokes `on_sigterm` exactly once, then exits.
///
/// Exactly one signal is enough — the caller's closure drives its own full
/// teardown. A second SIGTERM after this thread returns is NOT guaranteed to
/// terminate the process: `signal-hook-registry` does not restore the
/// pre-registration (default) disposition once the last action for a signal
/// is gone, so a second SIGTERM is silently absorbed from that point on, not
/// delivered as a kernel kill. The documented, tested escape hatch for a
/// hung teardown is therefore SIGKILL, never a second SIGTERM (see the
/// double-SIGTERM proof test in `crates/skyc/tests/watch_sigterm.rs`).
///
/// # Errors
/// If the OS refuses to register the handler (e.g. an already-exhausted
/// signal-handler slot — vanishingly rare in practice).
#[cfg(unix)]
pub fn install_sigterm_forwarder<F>(on_sigterm: F) -> std::io::Result<JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    let mut signals = signal_hook::iterator::Signals::new([signal_hook::consts::SIGTERM])?;
    Ok(std::thread::spawn(move || {
        if signals.forever().next().is_some() {
            on_sigterm();
        }
    }))
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use super::install_sigterm_forwarder;

    /// Serialise the two tests below: they share ONE process-wide SIGTERM
    /// disposition, so the positive test's real signal must never race the
    /// negative control's "no signal arrives" window when the test harness
    /// runs them as threads of one process (plain `cargo test`; `nextest`'s
    /// process-per-test isolation makes this a no-op there).
    static SIGNAL_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn poll_until(flag: &AtomicBool, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if flag.load(Ordering::Relaxed) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        flag.load(Ordering::Relaxed)
    }

    #[test]
    fn sigterm_forwarder_invokes_callback_on_sigterm() {
        static FIRED: AtomicBool = AtomicBool::new(false);
        let _guard = SIGNAL_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        install_sigterm_forwarder(|| FIRED.store(true, Ordering::Relaxed))
            .expect("SIGTERM registration must succeed");
        let status = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(std::process::id().to_string())
            .status()
            .expect("kill(1) must spawn");
        assert!(status.success(), "kill -TERM must deliver");
        assert!(
            poll_until(&FIRED, Duration::from_secs(2)),
            "the forwarder must invoke its callback once SIGTERM arrives"
        );
    }

    #[test]
    fn sigterm_forwarder_never_fires_without_a_signal() {
        static FIRED: AtomicBool = AtomicBool::new(false);
        let _guard = SIGNAL_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        install_sigterm_forwarder(|| FIRED.store(true, Ordering::Relaxed))
            .expect("SIGTERM registration must succeed");
        assert!(
            !poll_until(&FIRED, Duration::from_millis(500)),
            "the forwarder must NOT invoke its callback when no signal is sent"
        );
    }
}

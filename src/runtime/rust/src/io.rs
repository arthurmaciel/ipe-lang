// Ipe.Io — line-oriented stdio. All effectful, so IpeTask-returning.
use super::{IpeResult, IpeTask, ok_res, str_err};

use std::io::Write;

/// `Io.readLine : () -> Task Error String`. Reads one line from stdin with the
/// trailing newline stripped. EOF yields an empty string (Ok), matching the
/// "no more input" convention rather than erroring.
///
/// AUD-09: capped at 1 MiB via a `Take`-wrapped reader. Unbounded, a caller
/// piping input with no newline (or a misbehaving/adversarial source) could
/// grow `line` without limit (an OOM / `DoS` vector). Over the cap,
/// `read_line` stops at the byte limit (a truncated line, no newline found)
/// rather than allocating without bound — the same truncate-not-OOM
/// trade-off `File.readFileLimit` already makes.
const IO_READ_LINE_CAP_BYTES: u64 = 1024 * 1024;

#[must_use]
pub fn io_read_line<E: Send + From<String> + 'static>(_: ()) -> IpeTask<E, String> {
    Box::pin(async move {
        let mut line = String::new();
        let stdin = std::io::stdin();
        let limited = std::io::Read::take(stdin.lock(), IO_READ_LINE_CAP_BYTES);
        let mut reader = std::io::BufReader::new(limited);
        match std::io::BufRead::read_line(&mut reader, &mut line) {
            Ok(_) => {
                let trimmed = line.trim_end_matches(['\n', '\r']).to_string();
                ok_res(trimmed)
            }
            Err(e) => IpeResult::Err(str_err(&format!("{e}"))),
        }
    })
}

/// Restores the saved terminal attributes when dropped, so echo is turned back
/// on even if the read errors or the thread unwinds. This is the fail-safe: the
/// terminal is never left with echo disabled once the guard leaves scope.
#[cfg(all(unix, feature = "secret"))]
struct EchoGuard<F: std::os::fd::AsFd> {
    fd: F,
    prior: rustix::termios::Termios,
}

#[cfg(all(unix, feature = "secret"))]
impl<F: std::os::fd::AsFd> Drop for EchoGuard<F> {
    fn drop(&mut self) {
        // Best-effort restore; a failure here cannot itself be surfaced from
        // `drop`, and there is no safer state to fall back to than "re-apply the
        // attributes we captured before we changed them".
        let _ = rustix::termios::tcsetattr(
            &self.fd,
            rustix::termios::OptionalActions::Flush,
            &self.prior,
        );
    }
}

/// Disable terminal echo on `fd`, returning a guard that restores the prior mode on drop.
///
/// `None` when `fd` is not a tty (nothing to toggle — the caller then reads with
/// echo unchanged, i.e. a plain line read).
#[cfg(all(unix, feature = "secret"))]
fn suppress_echo<F: std::os::fd::AsFd>(fd: F) -> Option<EchoGuard<F>> {
    // Not a terminal (piped/redirected stdin): there is no echo state to change,
    // so report "no guard" and let the caller fall back to a normal read.
    if !rustix::termios::isatty(&fd) {
        return None;
    }
    let prior = rustix::termios::tcgetattr(&fd).ok()?;
    let mut raw = prior.clone();
    raw.local_modes.remove(rustix::termios::LocalModes::ECHO);
    rustix::termios::tcsetattr(&fd, rustix::termios::OptionalActions::Flush, &raw).ok()?;
    Some(EchoGuard { fd, prior })
}

/// `Io.readSecret : String -> Task Error Secret`. Writes `prompt` to stdout,
/// then reads one line from stdin with terminal echo suppressed (a password
/// read) and strips the trailing newline. The prior terminal mode is always
/// restored on return — success, error, or unwind — via an RAII guard.
///
/// The read line is sealed into an opaque [`crate::secret::Secret`] before it is
/// handed back — never returned as a bare `String`. The plaintext is reachable
/// only through the scoped `Secret.use` / `Secret.reveal` API, so a freshly-read
/// secret cannot flow into a log line, an error message, or a serialized payload
/// by accident. Gated behind the `secret` feature: a program that reads a secret
/// necessarily holds a `Secret`-typed value, which turns the feature on.
///
/// On a non-tty stdin (piped/redirected) there is no echo state to toggle, so
/// this degrades to a plain line read. On non-Unix targets, where no echo
/// toggle is wired, it also reads with echo unchanged. Capped at the same
/// 1 MiB `IO_READ_LINE_CAP_BYTES` limit as `readLine` (truncate, never OOM).
#[cfg(feature = "secret")]
#[must_use]
pub fn io_read_secret<E: Send + From<String> + 'static>(
    prompt: String,
) -> IpeTask<E, crate::secret::Secret> {
    Box::pin(async move {
        {
            let mut out = std::io::stdout();
            let _ = out.write_all(prompt.as_bytes());
            let _ = out.flush();
        }

        #[cfg(unix)]
        let _echo_guard = suppress_echo(std::io::stdin());

        let mut line = String::new();
        let stdin = std::io::stdin();
        let limited = std::io::Read::take(stdin.lock(), IO_READ_LINE_CAP_BYTES);
        let mut reader = std::io::BufReader::new(limited);
        let result = match std::io::BufRead::read_line(&mut reader, &mut line) {
            Ok(_) => {
                // Seal the plaintext into `Secret` at the read boundary: the bare
                // `String` never escapes this function, so the sealed value's
                // redaction / no-`Debug` / zeroize-on-`Drop` protections cover it
                // from the moment it is read.
                let trimmed = line.trim_end_matches(['\n', '\r']).to_string();
                ok_res(crate::secret::secret_from_string(trimmed))
            }
            Err(e) => IpeResult::Err(str_err(&format!("{e}"))),
        };
        // Echo was suppressed, so the user's Enter produced no visible newline;
        // emit one so following output starts on a fresh line. Skipped on a
        // non-tty (no guard) to keep piped output byte-clean.
        #[cfg(unix)]
        if _echo_guard.is_some() {
            let mut out = std::io::stdout();
            let _ = out.write_all(b"\n");
            let _ = out.flush();
        }
        result
    })
}

/// `Io.writeStdout : String -> Task Error ()`. Writes verbatim (no newline).
#[must_use]
pub fn io_write_stdout<E: Send + From<String> + 'static>(s: String) -> IpeTask<E, ()> {
    Box::pin(async move {
        let r = (|| {
            let mut out = std::io::stdout();
            out.write_all(s.as_bytes())?;
            out.flush()
        })();
        match r {
            Ok(()) => ok_res(()),
            Err(e) => IpeResult::Err(str_err(&format!("{e}"))),
        }
    })
}

/// `Io.writeStderr : String -> Task Error ()`. Writes verbatim (no newline).
#[must_use]
pub fn io_write_stderr<E: Send + From<String> + 'static>(s: String) -> IpeTask<E, ()> {
    Box::pin(async move {
        let r = (|| {
            let mut err = std::io::stderr();
            err.write_all(s.as_bytes())?;
            err.flush()
        })();
        match r {
            Ok(()) => ok_res(()),
            Err(e) => IpeResult::Err(str_err(&format!("{e}"))),
        }
    })
}

/// `Io.println : String -> Task Error ()`. Writes the message followed by a
/// single `\n` to stdout, then flushes.
#[must_use]
pub fn io_println<E: Send + From<String> + 'static>(msg: String) -> IpeTask<E, ()> {
    Box::pin(async move {
        let r = (|| {
            let mut out = std::io::stdout();
            out.write_all(msg.as_bytes())?;
            out.write_all(b"\n")?;
            out.flush()
        })();
        match r {
            Ok(()) => ok_res(()),
            Err(e) => IpeResult::Err(str_err(&format!("{e}"))),
        }
    })
}

/// `Io.eprintln : String -> Task Error ()`. Writes the message followed by a
/// single `\n` to stderr, then flushes.
#[must_use]
pub fn io_eprintln<E: Send + From<String> + 'static>(msg: String) -> IpeTask<E, ()> {
    Box::pin(async move {
        let r = (|| {
            let mut err = std::io::stderr();
            err.write_all(msg.as_bytes())?;
            err.write_all(b"\n")?;
            err.flush()
        })();
        match r {
            Ok(()) => ok_res(()),
            Err(e) => IpeResult::Err(str_err(&format!("{e}"))),
        }
    })
}

// The echo guard exists only when the `secret` feature is on (it is used solely
// by `io_read_secret`, which returns a `secret`-gated `Secret`), so its tests
// are gated the same way.
#[cfg(all(test, unix, feature = "secret"))]
mod echo_guard_tests {
    use super::suppress_echo;
    use std::os::fd::OwnedFd;

    /// Read the current `ECHO` bit of a tty fd.
    fn echo_on(fd: &OwnedFd) -> bool {
        rustix::termios::tcgetattr(fd)
            .is_ok_and(|t| t.local_modes.contains(rustix::termios::LocalModes::ECHO))
    }

    /// A real pty pair; both ends close when dropped.
    struct Pty {
        _master: OwnedFd,
        replica: OwnedFd,
    }

    fn open_pty() -> Option<Pty> {
        use rustix::pty::OpenptFlags;
        let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).ok()?;
        rustix::pty::grantpt(&master).ok()?;
        rustix::pty::unlockpt(&master).ok()?;
        let name = rustix::pty::ptsname(&master, Vec::new()).ok()?;
        let replica = rustix::fs::open(
            name.as_c_str(),
            rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NOCTTY,
            rustix::fs::Mode::empty(),
        )
        .ok()?;
        Some(Pty {
            _master: master,
            replica,
        })
    }

    // On a real tty, `suppress_echo` turns ECHO off for the guard's lifetime and
    // restores the prior mode (ECHO on) the moment the guard is dropped.
    #[test]
    fn suppresses_then_restores_echo_on_a_tty() {
        let pty = open_pty();
        assert!(pty.is_some(), "pty allocation failed");
        let Some(pty) = pty else { return };
        // Ensure the starting state has ECHO on.
        let start = rustix::termios::tcgetattr(&pty.replica);
        assert!(start.is_ok(), "tcgetattr failed");
        let Ok(mut t) = start else { return };
        t.local_modes.insert(rustix::termios::LocalModes::ECHO);
        assert!(
            rustix::termios::tcsetattr(&pty.replica, rustix::termios::OptionalActions::Now, &t)
                .is_ok()
        );
        assert!(echo_on(&pty.replica), "precondition: ECHO on");

        {
            let guard = suppress_echo(&pty.replica);
            assert!(guard.is_some(), "a tty must yield an echo guard");
            assert!(
                !echo_on(&pty.replica),
                "ECHO must be off while the guard lives"
            );
        } // guard drops here

        assert!(
            echo_on(&pty.replica),
            "ECHO must be restored after the guard drops"
        );
    }

    // A non-tty fd (a pipe) has no echo state to toggle: `suppress_echo` returns
    // `None`, so the caller falls back to a plain read — never panics.
    #[test]
    fn non_tty_yields_no_guard() {
        let fds = rustix::pipe::pipe();
        assert!(fds.is_ok(), "pipe failed");
        let Ok((read_fd, _write_fd)) = fds else {
            return;
        };
        assert!(
            suppress_echo(&read_fd).is_none(),
            "a pipe is not a tty; no echo guard"
        );
    }
}

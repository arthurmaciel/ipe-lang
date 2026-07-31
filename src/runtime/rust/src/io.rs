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
#[cfg(unix)]
struct EchoGuard {
    fd: std::os::unix::io::RawFd,
    prior: libc::termios,
}

#[cfg(unix)]
impl Drop for EchoGuard {
    fn drop(&mut self) {
        // Best-effort restore; a failure here cannot itself be surfaced from
        // `drop`, and there is no safer state to fall back to than "re-apply the
        // attributes we captured before we changed them".
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSAFLUSH, &self.prior);
        }
    }
}

/// Disable terminal echo on `fd`, returning a guard that restores the prior mode
/// on drop. `None` when `fd` is not a tty (nothing to toggle — the caller then
/// reads with echo unchanged, i.e. a plain line read).
#[cfg(unix)]
fn suppress_echo(fd: std::os::unix::io::RawFd) -> Option<EchoGuard> {
    // Not a terminal (piped/redirected stdin): there is no echo state to change,
    // so report "no guard" and let the caller fall back to a normal read.
    if unsafe { libc::isatty(fd) } != 1 {
        return None;
    }
    let mut prior: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut prior) } != 0 {
        return None;
    }
    let mut raw = prior;
    raw.c_lflag &= !libc::ECHO;
    if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &raw) } != 0 {
        return None;
    }
    Some(EchoGuard { fd, prior })
}

/// `Io.readSecret : String -> Task Error String`. Writes `prompt` to stdout,
/// then reads one line from stdin with terminal echo suppressed (a password
/// read) and strips the trailing newline. The prior terminal mode is always
/// restored on return — success, error, or unwind — via an RAII guard.
///
/// On a non-tty stdin (piped/redirected) there is no echo state to toggle, so
/// this degrades to a plain line read. On non-Unix targets, where no echo
/// toggle is wired, it also reads with echo unchanged. Capped at the same
/// 1 MiB `IO_READ_LINE_CAP_BYTES` limit as `readLine` (truncate, never OOM).
#[must_use]
pub fn io_read_secret<E: Send + From<String> + 'static>(prompt: String) -> IpeTask<E, String> {
    Box::pin(async move {
        {
            let mut out = std::io::stdout();
            let _ = out.write_all(prompt.as_bytes());
            let _ = out.flush();
        }

        #[cfg(unix)]
        let _echo_guard = {
            use std::os::unix::io::AsRawFd;
            suppress_echo(std::io::stdin().as_raw_fd())
        };

        let mut line = String::new();
        let stdin = std::io::stdin();
        let limited = std::io::Read::take(stdin.lock(), IO_READ_LINE_CAP_BYTES);
        let mut reader = std::io::BufReader::new(limited);
        let result = match std::io::BufRead::read_line(&mut reader, &mut line) {
            Ok(_) => {
                let trimmed = line.trim_end_matches(['\n', '\r']).to_string();
                ok_res(trimmed)
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

#[cfg(all(test, unix))]
mod echo_guard_tests {
    use super::suppress_echo;

    /// Read the current `ECHO` bit of a tty fd.
    fn echo_on(fd: std::os::unix::io::RawFd) -> bool {
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::tcgetattr(fd, &mut t) },
            0,
            "tcgetattr failed"
        );
        (t.c_lflag & libc::ECHO) != 0
    }

    /// A real pty pair whose fds are closed on drop.
    struct Pty {
        master: std::os::unix::io::RawFd,
        slave: std::os::unix::io::RawFd,
    }

    impl Drop for Pty {
        fn drop(&mut self) {
            unsafe {
                libc::close(self.master);
                libc::close(self.slave);
            }
        }
    }

    fn open_pty() -> Pty {
        let mut master = 0;
        let mut slave = 0;
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        assert_eq!(rc, 0, "openpty failed");
        Pty { master, slave }
    }

    // On a real tty, `suppress_echo` turns ECHO off for the guard's lifetime and
    // restores the prior mode (ECHO on) the moment the guard is dropped.
    #[test]
    fn suppresses_then_restores_echo_on_a_tty() {
        let pty = open_pty();
        // Ensure the starting state has ECHO on.
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::tcgetattr(pty.slave, &mut t) }, 0);
        t.c_lflag |= libc::ECHO;
        assert_eq!(unsafe { libc::tcsetattr(pty.slave, libc::TCSANOW, &t) }, 0);
        assert!(echo_on(pty.slave), "precondition: ECHO on");

        {
            let guard = suppress_echo(pty.slave);
            assert!(guard.is_some(), "a tty must yield an echo guard");
            assert!(
                !echo_on(pty.slave),
                "ECHO must be off while the guard lives"
            );
        } // guard drops here

        assert!(
            echo_on(pty.slave),
            "ECHO must be restored after the guard drops"
        );
    }

    // A non-tty fd (a pipe) has no echo state to toggle: `suppress_echo` returns
    // `None`, so the caller falls back to a plain read — never panics.
    #[test]
    fn non_tty_yields_no_guard() {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe failed");
        let read_fd = fds[0];
        assert!(
            suppress_echo(read_fd).is_none(),
            "a pipe is not a tty; no echo guard"
        );
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }
}

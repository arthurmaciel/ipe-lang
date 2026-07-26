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

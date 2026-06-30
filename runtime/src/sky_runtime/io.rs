// Sky.Core.Io — line-oriented stdio. All effectful, so SkyTask-returning.
use super::*;

use std::io::Write;

/// `Io.readLine : () -> Task Error String`. Reads one line from stdin with the
/// trailing newline stripped. EOF yields an empty string (Ok), matching the
/// "no more input" convention rather than erroring.
pub fn io_read_line<E: Send + From<String> + 'static>(_: ()) -> SkyTask<E, String> {
    Box::pin(async move {
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(_) => {
                let trimmed = line.trim_end_matches(['\n', '\r']).to_string();
                ok_res(trimmed)
            }
            Err(e) => SkyResult::Err(str_err(&format!("{}", e))),
        }
    })
}

/// `Io.writeStdout : String -> Task Error ()`. Writes verbatim (no newline).
pub fn io_write_stdout<E: Send + From<String> + 'static>(s: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        let r = (|| {
            let mut out = std::io::stdout();
            out.write_all(s.as_bytes())?;
            out.flush()
        })();
        match r {
            Ok(()) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&format!("{}", e))),
        }
    })
}

/// `Io.writeStderr : String -> Task Error ()`. Writes verbatim (no newline).
pub fn io_write_stderr<E: Send + From<String> + 'static>(s: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        let r = (|| {
            let mut err = std::io::stderr();
            err.write_all(s.as_bytes())?;
            err.flush()
        })();
        match r {
            Ok(()) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&format!("{}", e))),
        }
    })
}

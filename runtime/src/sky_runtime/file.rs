// File kernel stubs — generic over E.
use super::*;

/// `Sky.Core.File.readFile : String -> Task Error String`. Reads the whole file,
/// but bounded by a hard ceiling so an attacker-controlled path pointing at an
/// unbounded source (`/dev/zero`, a named pipe, a multi-GiB file) cannot OOM the
/// process — `read_to_string` on `/dev/zero` never returns. The ceiling defaults
/// to 512 MiB and is overridable via `SKY_FILE_READ_MAX` (bytes). For a smaller
/// explicit cap use `File.readFileLimit`; reading past the ceiling is an `Err`,
/// never a silent truncation.
fn file_read_ceiling() -> u64 {
    std::env::var("SKY_FILE_READ_MAX")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(512 * 1024 * 1024)
}

pub fn file_read_file<E: Send + From<String> + 'static>(path: String) -> SkyTask<E, String> {
    Box::pin(async move {
        let cap = file_read_ceiling();
        let run = || -> Result<String, String> {
            use std::io::Read;
            let f = std::fs::File::open(&path).map_err(|e| format!("{}", e))?;
            // take(cap + 1): if the source yields more than `cap` bytes we still
            // stop at a bounded read and report an error rather than OOM.
            let mut buf = String::new();
            let read = f
                .take(cap.saturating_add(1))
                .read_to_string(&mut buf)
                .map_err(|e| format!("{}", e))?;
            if read as u64 > cap {
                return Err(format!(
                    "file exceeds read ceiling of {} bytes (raise SKY_FILE_READ_MAX or use File.readFileLimit): {}",
                    cap, path
                ));
            }
            Ok(buf)
        };
        match run() {
            Ok(s) => ok_res(s),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

pub fn file_write_file<E: Send + From<String> + 'static>(
    path: String,
    content: String,
) -> SkyTask<E, ()> {
    Box::pin(async move {
        match std::fs::write(&path, &content) {
            Ok(_) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&format!("{}", e))),
        }
    })
}

pub fn file_exists<E: Send + 'static>(path: String) -> SkyTask<E, bool> {
    Box::pin(async move { ok_res(std::path::Path::new(&path).exists()) })
}

/// Alias of `file_remove` (the `remove` contract). Kept as a public name for
/// ABI stability; delegates so the two never drift.
pub fn file_delete<E: Send + From<String> + 'static>(path: String) -> SkyTask<E, ()> {
    file_remove(path)
}

/// `Sky.Core.File.mkdirAll : String -> Task Error ()` — create the directory
/// and every missing parent (mkdir -p). Already-exists is `Ok` (matching
/// `std::fs::create_dir_all`); a real I/O failure is `Err`.
pub fn file_mkdir_all<E: Send + From<String> + 'static>(path: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        match std::fs::create_dir_all(&path) {
            Ok(_) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&format!("{}", e))),
        }
    })
}

// ─── Read variants ─────────────────────────────────────────────────────────

/// `Sky.Core.File.readFileLimit : String -> Int -> Task Error String`
/// Read at most `limit` bytes. Returns `Err` when the file is larger than
/// `limit` (to avoid OOM on unbounded inputs) or when the content is not
/// valid UTF-8 (use `readFileBytes` for binary data in that case).
/// A non-positive limit falls back to the same 10 MiB default Go uses.
pub fn file_read_file_limit<E: Send + From<String> + 'static>(
    path: String,
    limit: i64,
) -> SkyTask<E, String> {
    use std::io::Read as _;
    let cap: u64 = if limit > 0 {
        limit as u64
    } else {
        10 * 1024 * 1024
    };
    Box::pin(async move {
        let result: Result<String, String> = (|| {
            let f = std::fs::File::open(&path).map_err(|e| format!("{}", e))?;
            let meta = f.metadata().map_err(|e| format!("{}", e))?;
            if meta.len() > cap {
                return Err(format!(
                    "file exceeds {}-byte limit (actual: {})",
                    cap,
                    meta.len()
                ));
            }
            let mut buf = String::new();
            f.take(cap)
                .read_to_string(&mut buf)
                .map_err(|e| format!("{}", e))?;
            Ok(buf)
        })();
        match result {
            Ok(s) => ok_res(s),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

/// `Sky.Core.File.readFileBytes : String -> Task Error (List Int)`
/// Read the file as raw bytes, returned as `Vec<i64>` (Sky `List Int`,
/// values 0..=255). Uses the same 10 MiB default cap as Go. For text
/// content with guaranteed UTF-8, prefer `readFile` / `readFileLimit`.
pub fn file_read_file_bytes<E: Send + From<String> + 'static>(
    path: String,
) -> SkyTask<E, Vec<i64>> {
    const DEFAULT_CAP: u64 = 10 * 1024 * 1024;
    Box::pin(async move {
        use std::io::Read as _;
        let result: Result<Vec<i64>, String> = (|| {
            let f = std::fs::File::open(&path).map_err(|e| format!("{}", e))?;
            let mut buf = Vec::new();
            f.take(DEFAULT_CAP)
                .read_to_end(&mut buf)
                .map_err(|e| format!("{}", e))?;
            Ok(from_u8_slice(&buf))
        })();
        match result {
            Ok(v) => ok_res(v),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

// ─── Write variants ────────────────────────────────────────────────────────

/// `Sky.Core.File.append : String -> String -> Task Error ()`
/// Append `content` to the end of the file at `path`, creating it if absent.
/// Mirrors Go's `os.OpenFile(…, O_APPEND|O_CREATE|O_WRONLY, 0644)`.
pub fn file_append<E: Send + From<String> + 'static>(
    path: String,
    content: String,
) -> SkyTask<E, ()> {
    Box::pin(async move {
        use std::io::Write as _;
        let result = (|| {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&path)
                .map_err(|e| format!("{}", e))?;
            f.write_all(content.as_bytes())
                .map_err(|e| format!("{}", e))
        })();
        match result {
            Ok(_) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

// ─── Removal ───────────────────────────────────────────────────────────────

/// `Sky.Core.File.remove : String -> Task Error ()`
/// Remove the file at `path`. Returns `Err` on any I/O failure (including
/// "not found"). Mirrors Go's `os.Remove`.
pub fn file_remove<E: Send + From<String> + 'static>(path: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        match std::fs::remove_file(&path) {
            Ok(_) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&format!("{}", e))),
        }
    })
}

// ─── Directory queries ─────────────────────────────────────────────────────

/// `Sky.Core.File.readDir : String -> Task Error (List String)`
/// Return the names (not full paths) of all entries in the directory at
/// `path`, in filesystem order. Mirrors Go's `os.ReadDir` → `e.Name()`.
pub fn file_read_dir<E: Send + From<String> + 'static>(path: String) -> SkyTask<E, Vec<String>> {
    Box::pin(async move {
        // Propagate per-entry read errors instead of silently dropping them
        // (`rd.flatten()` would discard `Err` items mid-walk, omitting entries
        // a transient stat/readdir failure touched — Go's `os.ReadDir` surfaces
        // such an error rather than returning a truncated list).
        let result: Result<Vec<String>, String> = (|| {
            let rd = std::fs::read_dir(&path).map_err(|e| format!("{}", e))?;
            let mut names: Vec<String> = Vec::new();
            for entry in rd {
                let entry = entry.map_err(|e| format!("{}", e))?;
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
            Ok(names)
        })();
        match result {
            Ok(names) => ok_res(names),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

/// `Sky.Core.File.isDir : String -> Task Error Bool`
/// Returns `Ok(true)` when `path` exists and is a directory, `Ok(false)` when
/// it exists and is not a directory, and `Ok(false)` (not `Err`) when the path
/// does not exist — matching Go's shape (`os.Stat` error → `false`).
pub fn file_is_dir<E: Send + 'static>(path: String) -> SkyTask<E, bool> {
    Box::pin(async move {
        let is_dir = std::fs::metadata(&path)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        ok_res(is_dir)
    })
}

// ─── Temp paths ────────────────────────────────────────────────────────────

/// `Sky.Core.File.tempFile : String -> Task Error String`
/// Create a uniquely-named empty file in the system temp directory, using
/// `prefix` as the filename prefix. Returns the absolute path.
/// The caller is responsible for removing the file when done.
///
/// Implementation: retry loop with a monotonic-time + process-ID suffix until
/// exclusive creation succeeds (`O_CREAT|O_EXCL` semantics via
/// `OpenOptions::create_new`). No `tempfile` crate needed (pure `std`).
pub fn file_temp_file<E: Send + From<String> + 'static>(prefix: String) -> SkyTask<E, String> {
    Box::pin(async move {
        match make_temp_path(&prefix, false) {
            Ok(p) => ok_res(p),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

/// `Sky.Core.File.tempDir : String -> Task Error String`
/// Create a uniquely-named directory in the system temp directory, using
/// `prefix` as the directory name prefix. Returns the absolute path.
/// The caller is responsible for removing the directory when done.
pub fn file_temp_dir<E: Send + From<String> + 'static>(prefix: String) -> SkyTask<E, String> {
    Box::pin(async move {
        match make_temp_path(&prefix, true) {
            Ok(p) => ok_res(p),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

/// Shared helper: create a uniquely-named file (`is_dir=false`) or directory
/// (`is_dir=true`) in the system temp directory, returning its absolute path.
///
/// Uses a monotonic-time nanos + process-ID suffix and retries up to 32 times
/// to get an exclusive slot (the same approach libc tempfile() uses).  No
/// external crate needed.
fn make_temp_path(prefix: &str, is_dir: bool) -> Result<String, String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Sanitise the caller-controlled prefix: keep only filename-safe chars so it
    // cannot contain a path separator ('/'/'\\' — would escape temp_dir) or be
    // absolute. Without this, prefix="../../etc/" or "/tmp/evil" is a
    // write-arbitrary-path primitive (Path::join honours absolute/.. components).
    let prefix: String = prefix
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let prefix = prefix.as_str();
    let base = std::env::temp_dir();
    let pid = std::process::id();
    // Retry loop: collision is extremely rare but theoretically possible.
    for attempt in 0u32..32 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(attempt);
        let name = format!("{}{}{:08x}{:04x}", prefix, pid, nanos, attempt);
        let path = base.join(&name);
        if is_dir {
            // Owner-only (0700) on Unix — a temp dir created with the default
            // umask can be world-readable/traversable, exposing whatever the
            // caller writes into it (Go uses 0700 for MkdirTemp).
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(_) => return Ok(path.to_string_lossy().into_owned()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("{}", e)),
            }
        } else {
            // Owner-only (0600) on Unix — same rationale; Go's CreateTemp is 0600.
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            match opts.open(&path) {
                Ok(_) => return Ok(path.to_string_lossy().into_owned()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("{}", e)),
            }
        }
    }
    Err("could not create a unique temporary path after 32 attempts".to_string())
}

// ─── Copy / rename ─────────────────────────────────────────────────────────

/// `Sky.Core.File.copy : String -> String -> Task Error ()`
/// Copy the file at `src` to `dst`, creating or overwriting `dst`.
/// Mirrors Go's `io.Copy(out, in)` pattern.
pub fn file_copy<E: Send + From<String> + 'static>(src: String, dst: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        match std::fs::copy(&src, &dst)
            .map(|_| ())
            .map_err(|e| format!("{}", e))
        {
            Ok(_) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

/// `Sky.Core.File.rename : String -> String -> Task Error ()`
/// Rename (move) the file or directory at `src` to `dst`.
/// Mirrors Go's `os.Rename`.
pub fn file_rename<E: Send + From<String> + 'static>(src: String, dst: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        match std::fs::rename(&src, &dst) {
            Ok(_) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&format!("{}", e))),
        }
    })
}

#[cfg(test)]
mod read_ceiling_tests {
    use super::*;

    fn block<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    // SECURITY/DoS regression: readFile must refuse a source larger than the
    // ceiling instead of allocating it unbounded.
    #[test]
    fn read_file_rejects_over_ceiling() {
        let p = std::env::temp_dir().join(format!("sky_rc_over_{}.txt", std::process::id()));
        std::fs::write(&p, vec![b'x'; 8192]).unwrap();
        std::env::set_var("SKY_FILE_READ_MAX", "1024");
        let res: SkyResult<String, String> =
            block(file_read_file(p.to_string_lossy().into_owned()));
        std::env::remove_var("SKY_FILE_READ_MAX");
        let _ = std::fs::remove_file(&p);
        assert!(
            matches!(res, SkyResult::Err(_)),
            "8 KiB read under a 1 KiB ceiling must Err"
        );
    }

    #[test]
    fn read_file_under_ceiling_ok() {
        let p = std::env::temp_dir().join(format!("sky_rc_ok_{}.txt", std::process::id()));
        std::fs::write(&p, b"hello").unwrap();
        let res: SkyResult<String, String> =
            block(file_read_file(p.to_string_lossy().into_owned()));
        let _ = std::fs::remove_file(&p);
        match res {
            SkyResult::Ok(s) => assert_eq!(s, "hello"),
            SkyResult::Err(e) => panic!("unexpected Err: {e}"),
        }
    }
}

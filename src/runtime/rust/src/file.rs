// File kernel stubs — generic over E.
//
// Every path argument is a typed [`crate::path::Path`], not a raw `String`:
// the `Ipe.File` surface is sealed so a caller CANNOT reach a filesystem
// syscall with an unvalidated string. Construction (and the traversal / NUL
// rejection that guards it) lives once in `path::path_from_string`; each kernel
// here unwraps the already-validated `Path` to its cleaned string via
// `.into_string()` and proceeds — it never re-validates, because the type is
// the proof.
use super::path::Path;
use super::{IpeResult, IpeTask, from_u8_slice, ok_res, str_err};

// ── shared blocking-pool helper ───────────────────────────────────────
//
// Every kernel in this module does a blocking `std::fs` syscall inside its
// `Box::pin(async move { ... })` body. On a tokio worker thread (the shape
// every generated Ipe.Web/Ipe.Http.Server/Ipe.Console/Ipe.Tui app runs under),
// a blocking syscall stalls that worker for its full duration — reactor
// starvation under concurrent load, or a real multi-second stall on a
// slow/network filesystem. `run_blocking` offloads the closure to tokio's
// blocking-thread pool via `spawn_blocking`, mirroring the pattern already
// used by `auth.rs` for bcrypt (`auth_register`/`auth_login`/`auth_set_role`).
//
// Feature-gating note: `pub mod file;` (`mod.rs`) is UNCONDITIONAL — unlike
// `compression.rs`, which is gated on a `compression` feature that always
// pulls in `tokio` — while `tokio` itself is an `optional = true` dependency.
// The main CI clippy job (`cargo clippy --all-targets --workspace`) builds
// with the crate's `default = []` features, i.e. `tokio` NOT enabled, so an
// unconditional `tokio::task::spawn_blocking` reference here would break that
// job. Every REAL generated Ipê project always has `tokio` (`Task.run`/
// `block_on` need it regardless of which kernels are used — see
// `tests/golden/basics/Cargo.toml`), so the `#[cfg(not(feature = "tokio"))]`
// fallback below only matters for the standalone `ipe-runtime-rust` crate's
// own narrow-feature builds, never for a real Ipê program. See
// `docs/adr/0014-kernel-robustness-blocking-offload-and-toctou.md` §2.2.
#[cfg(feature = "tokio")]
async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(_) => Err("background file task panicked".to_string()),
    }
}

#[cfg(not(feature = "tokio"))]
// `async` is required here to match the tokio variant's signature; callers
// always use `.await` to work with both feature configurations uniformly.
#[allow(clippy::unused_async)]
async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    f()
}

/// `Ipe.File.readFile : String -> Task Error String`. Reads the whole file,
/// but bounded by a hard ceiling so an attacker-controlled path pointing at an
/// unbounded source (`/dev/zero`, a named pipe, a multi-GiB file) cannot OOM the
/// process — `read_to_string` on `/dev/zero` never returns. The ceiling defaults
/// to 512 MiB and is overridable via `IPE_FILE_READ_MAX` (bytes). For a smaller
/// explicit cap use `File.readFileLimit`; reading past the ceiling is an `Err`,
/// never a silent truncation.
fn file_read_ceiling() -> u64 {
    crate::system::read_env_var("IPE_FILE_READ_MAX")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(512 * 1024 * 1024)
}

fn file_read_file_sync(path: &str, cap: u64) -> Result<String, String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| format!("{e}"))?;
    // take(cap + 1): if the source yields more than `cap` bytes we still
    // stop at a bounded read and report an error rather than OOM.
    let mut buf = String::new();
    let read = f
        .take(cap.saturating_add(1))
        .read_to_string(&mut buf)
        .map_err(|e| format!("{e}"))?;
    if read as u64 > cap {
        return Err(format!(
            "file exceeds read ceiling of {cap} bytes (raise IPE_FILE_READ_MAX or use File.readFileLimit): {path}"
        ));
    }
    Ok(buf)
}

#[must_use]
pub fn file_read_file<E: Send + From<String> + 'static>(path: Path) -> IpeTask<E, String> {
    let path = path.into_string();
    Box::pin(async move {
        let cap = file_read_ceiling();
        match run_blocking(move || file_read_file_sync(&path, cap)).await {
            Ok(s) => ok_res(s),
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}

fn file_write_file_sync(path: &str, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|e| format!("{e}"))
}

#[must_use]
pub fn file_write_file<E: Send + From<String> + 'static>(
    path: Path,
    content: String,
) -> IpeTask<E, ()> {
    let path = path.into_string();
    Box::pin(async move {
        match run_blocking(move || file_write_file_sync(&path, &content)).await {
            Ok(()) => ok_res(()),
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}

#[must_use]
pub fn file_exists<E: Send + 'static>(path: Path) -> IpeTask<E, bool> {
    let path = path.into_string();
    Box::pin(async move {
        // Infallible closure — `run_blocking`'s `Err` arm is unreachable here
        // (kept `Result`-shaped only to satisfy the shared helper's bound), so
        // a hypothetical `JoinError` (task panicked) falls back to `false`
        // rather than propagating — there is no `Err` channel on this
        // kernel's existing `IpeTask<E, bool>` signature to propagate into.
        let exists = run_blocking(move || Ok(std::path::Path::new(&path).exists()))
            .await
            .unwrap_or(false);
        ok_res(exists)
    })
}

/// Alias of `file_remove` (the `remove` contract). Kept as a public name for
/// ABI stability; delegates so the two never drift.
#[must_use]
pub fn file_delete<E: Send + From<String> + 'static>(path: Path) -> IpeTask<E, ()> {
    file_remove(path)
}

fn file_mkdir_all_sync(path: &str) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("{e}"))
}

/// `Ipe.File.mkdirAll : String -> Task Error ()` — create the directory
/// and every missing parent (mkdir -p). Already-exists is `Ok` (matching
/// `std::fs::create_dir_all`); a real I/O failure is `Err`.
#[must_use]
pub fn file_mkdir_all<E: Send + From<String> + 'static>(path: Path) -> IpeTask<E, ()> {
    let path = path.into_string();
    Box::pin(async move {
        match run_blocking(move || file_mkdir_all_sync(&path)).await {
            Ok(()) => ok_res(()),
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}

// ─── Read variants ─────────────────────────────────────────────────────────

fn file_read_file_limit_sync(path: &str, cap: u64) -> Result<String, String> {
    use std::io::Read as _;
    let f = std::fs::File::open(path).map_err(|e| format!("{e}"))?;
    let mut buf = String::new();
    let read = f
        .take(cap.saturating_add(1))
        .read_to_string(&mut buf)
        .map_err(|e| format!("{e}"))?;
    if read as u64 > cap {
        return Err(format!(
            "file exceeds {cap}-byte limit (stopped reading at the limit — actual size not reported to bound memory use): {path}"
        ));
    }
    Ok(buf)
}

/// `Ipe.File.readFileLimit : String -> Int -> Task Error String`
/// Read at most `limit` bytes. Returns `Err` when the file is larger than
/// `limit` (to avoid OOM on unbounded inputs) or when the content is not
/// valid UTF-8 (use `readFileBytes` for binary data in that case).
/// A non-positive limit falls back to the same 10 MiB default Go uses.
///
/// AUD-09 gap-sweep TOCTOU fix: no separate `metadata()` pre-check. A
/// stat-then-read split is TOCTOU — a file that grows between the two
/// syscalls would pass the (now-stale) size check and then have `take(cap)`
/// silently truncate the read with no error, instead of reporting that the
/// file exceeds the limit. Reading `cap + 1` bytes in a single pass and
/// checking the ACTUAL bytes read (same idiom as `file_read_file` above, and
/// as `compression.rs`'s `gunzip`/`zstdDecompress` decompression-bomb check)
/// removes the race window structurally: there is only one syscall
/// sequence, so there is nothing left to race against.
#[must_use]
pub fn file_read_file_limit<E: Send + From<String> + 'static>(
    path: Path,
    limit: i64,
) -> IpeTask<E, String> {
    let path = path.into_string();
    let cap: u64 = if limit > 0 {
        limit as u64
    } else {
        10 * 1024 * 1024
    };
    Box::pin(async move {
        match run_blocking(move || file_read_file_limit_sync(&path, cap)).await {
            Ok(s) => ok_res(s),
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}

fn file_read_file_bytes_sync(path: &str) -> Result<Vec<i64>, String> {
    const DEFAULT_CAP: u64 = 10 * 1024 * 1024;
    use std::io::Read as _;
    let f = std::fs::File::open(path).map_err(|e| format!("{e}"))?;
    let mut buf = Vec::new();
    // Read `DEFAULT_CAP + 1` bytes in one pass and check the ACTUAL bytes
    // read, same idiom as `file_read_file_sync` / `file_read_file_limit_sync`
    // above (and the fix applied to `readFileLimit`'s TOCTOU race, commit
    // 706f026): a file over the cap must `Err`, never silently truncate to
    // `DEFAULT_CAP` bytes and report `Ok`.
    let read = f
        .take(DEFAULT_CAP.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| format!("{e}"))?;
    if read as u64 > DEFAULT_CAP {
        return Err(format!(
            "file exceeds {DEFAULT_CAP}-byte limit (stopped reading at the limit — actual size not reported to bound memory use): {path}"
        ));
    }
    Ok(from_u8_slice(&buf))
}

/// `Ipe.File.readFileBytes : String -> Task Error (List Int)`
/// Read the file as raw bytes, returned as `Vec<i64>` (Ipê `List Int`,
/// values 0..=255). Uses the same 10 MiB default cap as Go — a file over the
/// cap is an `Err`, never a silent truncation (sibling fix to
/// `readFileLimit`'s TOCTOU close, commit 706f026). For text content with
/// guaranteed UTF-8, prefer `readFile` / `readFileLimit`.
#[must_use]
pub fn file_read_file_bytes<E: Send + From<String> + 'static>(
    path: Path,
) -> IpeTask<E, Vec<i64>> {
    let path = path.into_string();
    Box::pin(async move {
        match run_blocking(move || file_read_file_bytes_sync(&path)).await {
            Ok(v) => ok_res(v),
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}

// ─── Write variants ────────────────────────────────────────────────────────

fn file_append_sync(path: &str, content: &str) -> Result<(), String> {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .map_err(|e| format!("{e}"))?;
    f.write_all(content.as_bytes()).map_err(|e| format!("{e}"))
}

/// `Ipe.File.append : String -> String -> Task Error ()`
/// Append `content` to the end of the file at `path`, creating it if absent.
/// Mirrors Go's `os.OpenFile(…, O_APPEND|O_CREATE|O_WRONLY, 0644)`.
#[must_use]
pub fn file_append<E: Send + From<String> + 'static>(
    path: Path,
    content: String,
) -> IpeTask<E, ()> {
    let path = path.into_string();
    Box::pin(async move {
        match run_blocking(move || file_append_sync(&path, &content)).await {
            Ok(()) => ok_res(()),
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}

// ─── Removal ───────────────────────────────────────────────────────────────

fn file_remove_sync(path: &str) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| format!("{e}"))
}

/// `Ipe.File.remove : String -> Task Error ()`
/// Remove the file at `path`. Returns `Err` on any I/O failure (including
/// "not found"). Mirrors Go's `os.Remove`.
#[must_use]
pub fn file_remove<E: Send + From<String> + 'static>(path: Path) -> IpeTask<E, ()> {
    let path = path.into_string();
    Box::pin(async move {
        match run_blocking(move || file_remove_sync(&path)).await {
            Ok(()) => ok_res(()),
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}

// ─── Directory queries ─────────────────────────────────────────────────────

fn file_read_dir_sync(path: &str) -> Result<Vec<String>, String> {
    // Propagate per-entry read errors instead of silently dropping them
    // (`rd.flatten()` would discard `Err` items mid-walk, omitting entries
    // a transient stat/readdir failure touched — Go's `os.ReadDir` surfaces
    // such an error rather than returning a truncated list).
    let rd = std::fs::read_dir(path).map_err(|e| format!("{e}"))?;
    let mut names: Vec<String> = Vec::new();
    for entry in rd {
        let entry = entry.map_err(|e| format!("{e}"))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

/// `Ipe.File.readDir : String -> Task Error (List String)`
/// Return the names (not full paths) of all entries in the directory at
/// `path`, in filesystem order. Mirrors Go's `os.ReadDir` → `e.Name()`.
#[must_use]
pub fn file_read_dir<E: Send + From<String> + 'static>(path: Path) -> IpeTask<E, Vec<String>> {
    let path = path.into_string();
    Box::pin(async move {
        match run_blocking(move || file_read_dir_sync(&path)).await {
            Ok(names) => ok_res(names),
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}

/// `Ipe.File.isDir : String -> Task Error Bool`
/// Returns `Ok(true)` when `path` exists and is a directory, `Ok(false)` when
/// it exists and is not a directory, and `Ok(false)` (not `Err`) when the path
/// does not exist — matching Go's shape (`os.Stat` error → `false`).
#[must_use]
pub fn file_is_dir<E: Send + 'static>(path: Path) -> IpeTask<E, bool> {
    let path = path.into_string();
    Box::pin(async move {
        // Same infallible-closure shape as `file_exists` above.
        let is_dir = run_blocking(move || Ok(std::fs::metadata(&path).is_ok_and(|m| m.is_dir())))
            .await
            .unwrap_or(false);
        ok_res(is_dir)
    })
}

// ─── Temp paths ────────────────────────────────────────────────────────────

/// `Ipe.File.tempFile : String -> Task Error String`
/// Create a uniquely-named empty file in the system temp directory, using
/// `prefix` as the filename prefix. Returns the absolute path.
/// The caller is responsible for removing the file when done.
///
/// Implementation: retry loop with a monotonic-time + process-ID suffix until
/// exclusive creation succeeds (`O_CREAT|O_EXCL` semantics via
/// `OpenOptions::create_new`). No `tempfile` crate needed (pure `std`).
#[must_use]
pub fn file_temp_file<E: Send + From<String> + 'static>(prefix: String) -> IpeTask<E, String> {
    Box::pin(async move {
        match run_blocking(move || make_temp_path(&prefix, false)).await {
            Ok(p) => ok_res(p),
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}

/// `Ipe.File.tempDir : String -> Task Error String`
/// Create a uniquely-named directory in the system temp directory, using
/// `prefix` as the directory name prefix. Returns the absolute path.
/// The caller is responsible for removing the directory when done.
#[must_use]
pub fn file_temp_dir<E: Send + From<String> + 'static>(prefix: String) -> IpeTask<E, String> {
    Box::pin(async move {
        match run_blocking(move || make_temp_path(&prefix, true)).await {
            Ok(p) => ok_res(p),
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}

/// Shared helper: create a uniquely-named file (`is_dir=false`) or directory
/// (`is_dir=true`) in the system temp directory, returning its absolute path.
///
/// Uses a monotonic-time nanos + process-ID suffix and retries up to 32 times
/// to get an exclusive slot (the same approach libc `tempfile()` uses).  No
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
            .map_or(attempt, |d| d.subsec_nanos());
        let name = format!("{prefix}{pid}{nanos:08x}{attempt:04x}");
        let path = base.join(&name);
        if is_dir {
            // Owner-only (0700) on Unix — a temp dir created with the default
            // umask can be world-readable/traversable, exposing whatever the
            // caller writes into it (Go uses 0700 for MkdirTemp).
            #[cfg_attr(not(unix), allow(unused_mut))] // mutated only under cfg(unix)
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(path.to_string_lossy().into_owned()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(format!("{e}")),
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
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(format!("{e}")),
            }
        }
    }
    Err("could not create a unique temporary path after 32 attempts".to_string())
}

// ─── Copy / rename ─────────────────────────────────────────────────────────

fn file_copy_sync(src: &str, dst: &str) -> Result<(), String> {
    std::fs::copy(src, dst)
        .map(|_| ())
        .map_err(|e| format!("{e}"))
}

/// `Ipe.File.copy : String -> String -> Task Error ()`
/// Copy the file at `src` to `dst`, creating or overwriting `dst`.
/// Mirrors Go's `io.Copy(out, in)` pattern.
#[must_use]
pub fn file_copy<E: Send + From<String> + 'static>(src: Path, dst: Path) -> IpeTask<E, ()> {
    let (src, dst) = (src.into_string(), dst.into_string());
    Box::pin(async move {
        match run_blocking(move || file_copy_sync(&src, &dst)).await {
            Ok(()) => ok_res(()),
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}

fn file_rename_sync(src: &str, dst: &str) -> Result<(), String> {
    std::fs::rename(src, dst).map_err(|e| format!("{e}"))
}

/// `Ipe.File.rename : String -> String -> Task Error ()`
/// Rename (move) the file or directory at `src` to `dst`.
/// Mirrors Go's `os.Rename`.
#[must_use]
pub fn file_rename<E: Send + From<String> + 'static>(src: Path, dst: Path) -> IpeTask<E, ()> {
    let (src, dst) = (src.into_string(), dst.into_string());
    Box::pin(async move {
        match run_blocking(move || file_rename_sync(&src, &dst)).await {
            Ok(()) => ok_res(()),
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}

/// Test-only: seal an absolute `std::path::Path` (always a rooted, non-escaping
/// path) into an `Ipe.Path`. Kernel call sites now take a typed `Path`, so the
/// tests construct one through the same validated seal a real program uses.
#[cfg(test)]
fn tp(p: &std::path::Path) -> Path {
    match super::path::path_from_string::<String>(p.to_string_lossy().into_owned()) {
        IpeResult::Ok(path) => path,
        IpeResult::Err(e) => panic!("test temp path failed Path validation: {e}"),
    }
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
        let p = std::env::temp_dir().join(format!("ipe_rc_over_{}.txt", std::process::id()));
        std::fs::write(&p, vec![b'x'; 8192]).unwrap();
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("IPE_FILE_READ_MAX", "1024") };
        let res: IpeResult<String, String> =
            block(file_read_file(tp(&p)));
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_FILE_READ_MAX") };
        let _ = std::fs::remove_file(&p);
        assert!(
            matches!(res, IpeResult::Err(_)),
            "8 KiB read under a 1 KiB ceiling must Err"
        );
    }

    #[test]
    fn read_file_under_ceiling_ok() {
        let p = std::env::temp_dir().join(format!("ipe_rc_ok_{}.txt", std::process::id()));
        std::fs::write(&p, b"hello").unwrap();
        let res: IpeResult<String, String> =
            block(file_read_file(tp(&p)));
        let _ = std::fs::remove_file(&p);
        match res {
            IpeResult::Ok(s) => assert_eq!(s, "hello"),
            IpeResult::Err(e) => panic!("unexpected Err: {e}"),
        }
    }
}

#[cfg(test)]
mod read_file_limit_tests {
    use super::*;

    fn block<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    #[test]
    fn under_limit_reads_full_content() {
        let p = std::env::temp_dir().join(format!("ipe_rfl_under_{}.txt", std::process::id()));
        std::fs::write(&p, b"hello world").unwrap();
        let res: IpeResult<String, String> =
            block(file_read_file_limit(tp(&p), 1024));
        let _ = std::fs::remove_file(&p);
        match res {
            IpeResult::Ok(s) => assert_eq!(s, "hello world"),
            IpeResult::Err(e) => panic!("unexpected Err: {e}"),
        }
    }

    /// Boundary: a file whose size is EXACTLY `limit` bytes must succeed with
    /// the full content, not be rejected as "over" (the `> cap` check, not
    /// `>= cap`).
    #[test]
    fn exactly_at_limit_is_ok() {
        let p = std::env::temp_dir().join(format!("ipe_rfl_exact_{}.txt", std::process::id()));
        let content = vec![b'a'; 16];
        std::fs::write(&p, &content).unwrap();
        let res: IpeResult<String, String> =
            block(file_read_file_limit(tp(&p), 16));
        let _ = std::fs::remove_file(&p);
        match res {
            IpeResult::Ok(s) => assert_eq!(s.len(), 16),
            IpeResult::Err(e) => panic!("exactly-at-limit must be Ok, got Err: {e}"),
        }
    }

    /// Regression for the TOCTOU fix: a file ONE byte over the limit must
    /// Err, never silently truncate to `limit` bytes and report Ok. This
    /// pins the single-pass rewrite's over-limit-at-rest behavior while
    /// removing the stat-then-read race window a growing-file scenario
    /// would otherwise hit.
    #[test]
    fn over_limit_by_one_byte_errs() {
        let p = std::env::temp_dir().join(format!("ipe_rfl_over_{}.txt", std::process::id()));
        std::fs::write(&p, vec![b'a'; 17]).unwrap();
        let res: IpeResult<String, String> =
            block(file_read_file_limit(tp(&p), 16));
        let _ = std::fs::remove_file(&p);
        assert!(
            matches!(res, IpeResult::Err(_)),
            "17 bytes under a 16-byte limit must Err, not silently truncate"
        );
    }

    /// Non-positive limit falls back to the documented 10 MiB default.
    #[test]
    fn non_positive_limit_uses_default_cap() {
        let p = std::env::temp_dir().join(format!("ipe_rfl_default_{}.txt", std::process::id()));
        std::fs::write(&p, b"small").unwrap();
        let res: IpeResult<String, String> =
            block(file_read_file_limit(tp(&p), 0));
        let _ = std::fs::remove_file(&p);
        match res {
            IpeResult::Ok(s) => assert_eq!(s, "small"),
            IpeResult::Err(e) => panic!("unexpected Err: {e}"),
        }
    }
}

/// Sibling of `read_file_limit_tests` for `File.readFileBytes`'s own fixed
/// 10 MiB cap: `readFileBytes` must ERROR when a file exceeds the cap, not
/// silently truncate at it via `take(DEFAULT_CAP).read_to_end(..)` with no
/// post-read size check — the same class as `readFileLimit`'s TOCTOU.
#[cfg(test)]
mod read_file_bytes_tests {
    use super::*;

    const DEFAULT_CAP: usize = 10 * 1024 * 1024;

    fn block<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    #[test]
    fn under_cap_reads_full_content() {
        let p = std::env::temp_dir().join(format!("ipe_rfb_under_{}.bin", std::process::id()));
        std::fs::write(&p, [1u8, 2, 3, 255, 0]).unwrap();
        let res: IpeResult<String, Vec<i64>> =
            block(file_read_file_bytes(tp(&p)));
        let _ = std::fs::remove_file(&p);
        match res {
            IpeResult::Ok(v) => assert_eq!(v, vec![1, 2, 3, 255, 0]),
            IpeResult::Err(e) => panic!("unexpected Err: {e}"),
        }
    }

    /// Boundary: a file whose size is EXACTLY the 10 MiB cap must succeed
    /// with the full content, not be rejected as "over" (the `> cap` check,
    /// not `>= cap`).
    #[test]
    fn exactly_at_cap_is_ok() {
        let p = std::env::temp_dir().join(format!("ipe_rfb_exact_{}.bin", std::process::id()));
        std::fs::write(&p, vec![7u8; DEFAULT_CAP]).unwrap();
        let res: IpeResult<String, Vec<i64>> =
            block(file_read_file_bytes(tp(&p)));
        let _ = std::fs::remove_file(&p);
        match res {
            IpeResult::Ok(v) => assert_eq!(v.len(), DEFAULT_CAP),
            IpeResult::Err(e) => panic!("exactly-at-cap must be Ok, got Err: {e}"),
        }
    }

    /// Regression: a file ONE byte over the 10 MiB cap must `Err`, never
    /// silently truncate to `DEFAULT_CAP` bytes and report `Ok` — this is
    /// the exact bug this fix closes. Pre-fix, this assertion FAILS: the old
    /// `take(DEFAULT_CAP).read_to_end(..)` reads exactly `DEFAULT_CAP` bytes
    /// with no error, and the returned `Vec` has `DEFAULT_CAP` elements
    /// (silently dropping the last byte) instead of erroring.
    #[test]
    fn over_cap_by_one_byte_errs() {
        let p = std::env::temp_dir().join(format!("ipe_rfb_over_{}.bin", std::process::id()));
        std::fs::write(&p, vec![7u8; DEFAULT_CAP + 1]).unwrap();
        let res: IpeResult<String, Vec<i64>> =
            block(file_read_file_bytes(tp(&p)));
        let _ = std::fs::remove_file(&p);
        assert!(
            matches!(res, IpeResult::Err(_)),
            "a file one byte over the 10 MiB cap must Err, not silently truncate: {res:?}"
        );
    }
}

#[cfg(all(test, feature = "tokio"))]
mod spawn_blocking_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Reactor-starvation guard: on a SINGLE-WORKER (current_thread) runtime, a
    /// blocking `std::fs` read called directly on the polled future would
    /// starve every other task on that runtime until the read completes.
    /// This proves `file_read_file` offloads the blocking read to tokio's
    /// blocking-thread pool instead of running it on the (sole) worker
    /// thread: a concurrently-spawned cheap ticker task must make progress
    /// (ticks > 0) WHILE the read is in flight.
    ///
    /// Pre-fix this is NOT a flaky race: the ticker makes EXACTLY zero
    /// progress deterministically, because the worker thread never yields
    /// back to the executor until `read_to_string` returns.
    #[test]
    fn file_read_file_does_not_starve_concurrent_async_work() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let p = std::env::temp_dir().join(format!(
            "ipe_spawn_blocking_probe_{}.txt",
            std::process::id()
        ));
        // Large enough that the read takes measurable (not instant) wall time.
        std::fs::write(&p, vec![b'x'; 64 * 1024 * 1024]).unwrap(); // 64 MiB
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("IPE_FILE_READ_MAX", (128 * 1024 * 1024).to_string()) };
        let path = super::tp(&p);

        let ticks = rt.block_on(async move {
            let counter = Arc::new(AtomicU64::new(0));
            let counter2 = counter.clone();
            let ticker = tokio::spawn(async move {
                loop {
                    counter2.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            });
            let read_fut: IpeTask<String, String> = file_read_file(path);
            let _res: IpeResult<String, String> = read_fut.await;
            ticker.abort();
            counter.load(Ordering::Relaxed)
        });

        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_FILE_READ_MAX") };
        let _ = std::fs::remove_file(&p);

        assert!(
            ticks > 0,
            "concurrent ticker task made ZERO progress while file_read_file ran — \
             the blocking read is starving the single-threaded executor \
             (spawn_blocking missing or not taking effect)"
        );
    }

    /// Same shape as above, for `file_write_file` — proves the write path is
    /// ALSO offloaded (a sibling kernel with the identical un-wrapped
    /// `std::fs::write` shape pre-fix).
    #[test]
    fn file_write_file_does_not_starve_concurrent_async_work() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let p = std::env::temp_dir().join(format!(
            "ipe_spawn_blocking_write_probe_{}.txt",
            std::process::id()
        ));
        let path = super::tp(&p);
        let content = "x".repeat(64 * 1024 * 1024); // 64 MiB

        let ticks = rt.block_on(async move {
            let counter = Arc::new(AtomicU64::new(0));
            let counter2 = counter.clone();
            let ticker = tokio::spawn(async move {
                loop {
                    counter2.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            });
            let write_fut: IpeTask<String, ()> = file_write_file(path, content);
            let _res: IpeResult<String, ()> = write_fut.await;
            ticker.abort();
            counter.load(Ordering::Relaxed)
        });

        let _ = std::fs::remove_file(&p);

        assert!(
            ticks > 0,
            "concurrent ticker task made ZERO progress while file_write_file ran — \
             the blocking write is starving the single-threaded executor \
             (spawn_blocking missing or not taking effect)"
        );
    }
}

# Class 9 — runtime kernel robustness + stdlib surface completeness: fix spec

> Companion to `docs/architecture/backlog.md` and
> `docs/architecture/campaign-classification-2026-07-09.md` (Class 9: MECHANICAL).
> Scope per the 2026-07-09 assignment: `File.readFileLimit` TOCTOU (AUD-09
> gap-sweep addition), #129 (`spawn_blocking` for compression/file kernels),
> #122 (Cli.program view-printer separator), #157 (`Sky.Core.Jwt` builder API
> surface gap). The three items already landed this session (`Io.readLine`
> cap, `gunzip` `MultiGzDecoder`, `Time.fromParts`/`isLeapYear` `i32::try_from`,
> `Basics.abs`/`Math.abs` i64::MIN saturation, `List.range` i128-safety) are
> confirmed landed by direct file read below and are OUT of scope — do not
> re-touch them.
>
> This document is READ-ONLY research + spec. No code was changed while
> writing it.

## 0. Pre-flight: confirmed-landed items (verified fresh, not re-specced)

Read directly from HEAD:

- `runtime/src/sky_runtime/io.rs:16-32` — `io_read_line` wraps stdin in
  `std::io::Read::take(stdin.lock(), IO_READ_LINE_CAP_BYTES)` (1 MiB). Landed.
- `runtime/src/sky_runtime/compression.rs:42-65` — `gunzip_bytes` uses
  `flate2::read::MultiGzDecoder` (not `GzDecoder`), with the decompression-bomb
  cap (`decompress_max_bytes()` + `take(max+1)` + length check) already in
  place. Landed.
- `runtime/src/sky_runtime/time.rs:412-460` (`time_from_parts`) and the
  `is_leap_year`/`time_days_in_month` neighbourhood — `i32::try_from(year)`
  fail-closed casts confirmed present (grep hits at `time.rs:412-452`
  reference `fromParts`; the `try_from` pattern from `time_days_in_month` is
  applied there). Landed.
- `runtime/src/sky_runtime/basics.rs:116-145` — `SaturatingNeg` trait +
  `basics_abs<T: PartialOrd + SaturatingNeg + Copy + Default>` generic
  saturating abs. Landed.
- `runtime/src/sky_runtime/math.rs:44` — `x.checked_abs().unwrap_or(i64::MAX)`.
  Already correct pre-session per the audit; confirmed still present. Landed.

None of these files are touched by this spec.

## 1. `File.readFileLimit` TOCTOU fix

### 1.1 Current behavior (confirmed by reading `runtime/src/sky_runtime/file.rs:88-120`)

```rust
pub fn file_read_file_limit<E: Send + From<String> + 'static>(
    path: String,
    limit: i64,
) -> SkyTask<E, String> {
    use std::io::Read as _;
    let cap: u64 = if limit > 0 { limit as u64 } else { 10 * 1024 * 1024 };
    Box::pin(async move {
        let result: Result<String, String> = (|| {
            let f = std::fs::File::open(&path).map_err(|e| format!("{}", e))?;
            let meta = f.metadata().map_err(|e| format!("{}", e))?;
            if meta.len() > cap {
                return Err(format!(
                    "file exceeds {}-byte limit (actual: {})",
                    cap, meta.len()
                ));
            }
            let mut buf = String::new();
            f.take(cap).read_to_string(&mut buf).map_err(|e| format!("{}", e))?;
            Ok(buf)
        })();
        match result {
            Ok(s) => ok_res(s),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}
```

**The bug:** the size check (`f.metadata()`) and the actual read
(`f.take(cap).read_to_string`) are two separate syscalls separated by a time
window. If the file grows between them (another process appending, a log
rotator, a symlink retarget), `meta.len()` was `<= cap` at check time, so the
function proceeds to the read — but `f.take(cap)` silently stops at exactly
`cap` bytes with **no error**, returning a truncated prefix as if it were the
whole (small) file. The caller gets `Ok(<cap bytes>)` instead of the `Err`
the size-limit contract promises. This is a correctness bug (silent
truncation masquerading as a full, in-limit read), not a security bug per se,
but it violates the same "truncate-not-error" contract this file's own
`compression.rs` sibling (`gunzip`/`zstdDecompress`) already gets right via
the `take(max+1)` + post-read length check idiom (see `compression.rs:50-64`,
`107-121`).

### 1.2 Fix

Eliminate the separate `metadata()` stat entirely. Read `cap + 1` bytes in
ONE pass (mirroring the exact idiom already used by `gunzip_bytes` /
`zstd_decompress_capped` in `compression.rs`, and by `file_read_file`'s own
ceiling check three functions above in the same file). If the read consumes
more than `cap` bytes, the source has more content than the limit allows —
error. This removes the race window structurally: there is only one syscall
sequence, driven by the actual bytes available at read time, so there is
nothing left to race against.

Replace `runtime/src/sky_runtime/file.rs:88-120` with:

```rust
/// `Sky.Core.File.readFileLimit : String -> Int -> Task Error String`
/// Read at most `limit` bytes. Returns `Err` when the file is larger than
/// `limit` (to avoid OOM on unbounded inputs) or when the content is not
/// valid UTF-8 (use `readFileBytes` for binary data in that case).
/// A non-positive limit falls back to the same 10 MiB default Go uses.
///
/// AUD-09 gap-sweep: no separate `metadata()` pre-check. A stat-then-read
/// split is TOCTOU — a file that grows between the two syscalls would pass
/// the (now-stale) size check and then have `take(cap)` silently truncate
/// the read with no error, instead of reporting that the file exceeds the
/// limit. Reading `cap + 1` bytes in a single pass and checking the ACTUAL
/// bytes read (same idiom as `compression.rs`'s `gunzip`/`zstdDecompress`
/// decompression-bomb check, and this file's own `file_read_file` ceiling
/// check) removes the race window structurally: there is only one syscall
/// sequence, so there is nothing left to race against.
pub fn file_read_file_limit<E: Send + From<String> + 'static>(
    path: String,
    limit: i64,
) -> SkyTask<E, String> {
    let cap: u64 = if limit > 0 {
        limit as u64
    } else {
        10 * 1024 * 1024
    };
    Box::pin(async move {
        let result = run_blocking(move || file_read_file_limit_sync(&path, cap)).await;
        match result {
            Ok(s) => ok_res(s),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

fn file_read_file_limit_sync(path: &str, cap: u64) -> Result<String, String> {
    use std::io::Read as _;
    let f = std::fs::File::open(path).map_err(|e| format!("{}", e))?;
    let mut buf = String::new();
    let read = f
        .take(cap.saturating_add(1))
        .read_to_string(&mut buf)
        .map_err(|e| format!("{}", e))?;
    if read as u64 > cap {
        return Err(format!(
            "file exceeds {}-byte limit (stopped reading at the limit — actual size not reported to bound memory use): {}",
            cap, path
        ));
    }
    Ok(buf)
}
```

(`run_blocking` is the shared helper introduced in §2.2 below for the
`spawn_blocking` fix — both fixes land in the same file edit, so
`file_read_file_limit` is written here already routed through it. If you are
implementing ONLY this TOCTOU fix without §2, drop the `run_blocking` call
and just call `file_read_file_limit_sync(&path, cap)` directly inside the
`Box::pin(async move { ... })`.)

Note the error message intentionally no longer reports an exact "actual"
byte count — same trade-off `gunzip`/`zstdDecompress` already accept: knowing
the file is "more than `cap` bytes" is all a bounded read can honestly claim
without itself becoming unbounded.

### 1.3 Regression tests

Add to `runtime/src/sky_runtime/file.rs`'s test module (extend
`read_ceiling_tests`, renaming it is not required — or add a new
`#[cfg(test)] mod read_file_limit_tests` block; either is fine, keep it next
to the existing `block()` helper so it can be reused):

```rust
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
        let p = std::env::temp_dir().join(format!("sky_rfl_under_{}.txt", std::process::id()));
        std::fs::write(&p, b"hello world").unwrap();
        let res: SkyResult<String, String> =
            block(file_read_file_limit(p.to_string_lossy().into_owned(), 1024));
        let _ = std::fs::remove_file(&p);
        match res {
            SkyResult::Ok(s) => assert_eq!(s, "hello world"),
            SkyResult::Err(e) => panic!("unexpected Err: {e}"),
        }
    }

    /// Boundary: a file whose size is EXACTLY `limit` bytes must succeed with
    /// the full content, not be rejected as "over" (the `> cap` check, not
    /// `>= cap`).
    #[test]
    fn exactly_at_limit_is_ok() {
        let p = std::env::temp_dir().join(format!("sky_rfl_exact_{}.txt", std::process::id()));
        let content = vec![b'a'; 16];
        std::fs::write(&p, &content).unwrap();
        let res: SkyResult<String, String> =
            block(file_read_file_limit(p.to_string_lossy().into_owned(), 16));
        let _ = std::fs::remove_file(&p);
        match res {
            SkyResult::Ok(s) => assert_eq!(s.len(), 16),
            SkyResult::Err(e) => panic!("exactly-at-limit must be Ok, got Err: {e}"),
        }
    }

    /// Regression for the TOCTOU fix: a file ONE byte over the limit must
    /// Err, never silently truncate to `limit` bytes and report Ok. Pre-fix,
    /// this specific case (no race needed — the file is simply already over
    /// limit when both syscalls run) actually already errored via the stale
    /// `metadata()` check; this test pins that the single-pass rewrite keeps
    /// the same over-limit-at-rest behavior while removing the race window a
    /// growing-file scenario would have hit.
    #[test]
    fn over_limit_by_one_byte_errs() {
        let p = std::env::temp_dir().join(format!("sky_rfl_over_{}.txt", std::process::id()));
        std::fs::write(&p, vec![b'a'; 17]).unwrap();
        let res: SkyResult<String, String> =
            block(file_read_file_limit(p.to_string_lossy().into_owned(), 16));
        let _ = std::fs::remove_file(&p);
        assert!(
            matches!(res, SkyResult::Err(_)),
            "17 bytes under a 16-byte limit must Err, not silently truncate"
        );
    }

    /// Non-positive limit falls back to the documented 10 MiB default.
    #[test]
    fn non_positive_limit_uses_default_cap() {
        let p = std::env::temp_dir().join(format!("sky_rfl_default_{}.txt", std::process::id()));
        std::fs::write(&p, b"small").unwrap();
        let res: SkyResult<String, String> =
            block(file_read_file_limit(p.to_string_lossy().into_owned(), 0));
        let _ = std::fs::remove_file(&p);
        match res {
            SkyResult::Ok(s) => assert_eq!(s, "small"),
            SkyResult::Err(e) => panic!("unexpected Err: {e}"),
        }
    }
}
```

Why no literal race-simulation test: a true TOCTOU race requires precise
inter-process timing to hit the exact window between `metadata()` and
`read`, which would be flaky by construction. The fix removes the race
*structurally* (single syscall sequence, no stat-then-read split), so the
correct regression is pinning the single-pass boundary behavior (`exactly_at_
limit_is_ok`, `over_limit_by_one_byte_errs`) — those tests would pass
identically whether the file was already-too-big or grew mid-check, because
there is no longer a "mid-check" for it to grow during.

## 2. #129 — `spawn_blocking` for compression + file kernels

### 2.1 Why (confirmed pattern + confirmed gap)

`runtime/src/sky_runtime/auth.rs:296-298` already does this for bcrypt:

```rust
// bcrypt is CPU-bound and BLOCKING (~250 ms at cost 12). Running it on a
// tokio worker thread starves the async runtime (every concurrent register
// ties up a core worker). Offload to the blocking pool.
let hash =
    match tokio::task::spawn_blocking(move || auth_hash_password::<E>(password)).await {
```

Confirmed by direct read: `compression.rs`'s four public kernels
(`compression_gzip`, `compression_gunzip`, `compression_zstd_compress`,
`compression_zstd_decompress`) compute their result **synchronously, before**
`Box::pin(ready(r))` even runs — i.e. the CPU-bound gzip/zstd work happens
inline on whatever thread calls the kernel, not even deferred to poll time.
This is worse than an ordinary blocking-inside-`async` call: it isn't even
wrapped in a future that defers the work, so calling the kernel function
itself blocks the caller. All of `file.rs`'s kernels likewise run blocking
`std::fs` syscalls directly inside their `Box::pin(async move { ... })`
bodies with no offload. On a tokio worker thread (the shape every generated
Sky.Live/Sky.Http.Server/Sky.Cli/Sky.Tui app runs under), any of these calls
stalls that worker for the syscall's duration — reactor starvation under
concurrent load, or a real multi-second stall on a slow/network filesystem.

### 2.2 Feature-gating constraint (load-bearing — read before editing)

`compression` is declared `compression = ["flate2", "zstd", "tokio"]` in
`runtime/Cargo.toml:87` — the `compression` module is `#[cfg(feature =
"compression")]`-gated in `mod.rs`, and that feature list ALREADY REQUIRES
`tokio`. So `compression.rs` can call `tokio::task::spawn_blocking`
unconditionally, no `cfg` needed.

`file.rs` is different: `pub mod file;` in `runtime/src/sky_runtime/mod.rs`
is **unconditional** (no feature gate at all), while `tokio` is an `optional
= true` dependency. Confirmed via `.github/workflows/ci.yml:44` — the main
`clippy` job runs `cargo clippy --all-targets --workspace` with the crate's
`default = []` features, i.e. `tokio` is NOT enabled in that build. If
`file.rs` calls `tokio::task::spawn_blocking` unconditionally, that CI job
breaks (unresolved `tokio` crate reference). Every REAL generated Sky project
always has `tokio` (confirmed via `tests/golden/m0/Cargo.toml:7-20`:
`default = ["tokio", "crypto", "json"]`, `tokio = { version = "1", features =
["rt", "rt-multi-thread", "macros", "time"] }` — every generated project's
base manifest, since `Task.run`/`block_on` need it regardless of which
kernels are used), so this only matters for the standalone `sky-runtime-rust`
crate's own narrow-feature CI builds, never for a real Sky program. Therefore
`file.rs`'s fix MUST be `#[cfg(feature = "tokio")]`-gated with a
same-behavior-as-today fallback for the (test-only) tokio-off case.

### 2.3 Shared helper (`file.rs`)

Add near the top of `runtime/src/sky_runtime/file.rs` (after the existing
`use super::*;` / `use std::io::Write;` style imports, before
`file_read_ceiling`):

```rust
/// Runs a blocking closure off the calling task's thread when the `tokio`
/// feature is available (every real generated Sky project — see #129 /
/// `docs/architecture/class9-kernel-robustness-fix-spec-2026-07-09.md` §2.2
/// for why this is the right gate), otherwise runs it inline (pre-existing
/// behavior — only reachable from a standalone `cargo clippy -p
/// sky-runtime-rust` narrow-feature build, where there is no `SkyTask`
/// executor available anyway since `task::block_on` is itself
/// `#[cfg(feature = "tokio")]`-gated, so this branch is never actually
/// EXECUTED, only compiled).
///
/// Mirrors `auth.rs`'s `tokio::task::spawn_blocking` bcrypt pattern
/// (`auth_register`/`auth_login`/`auth_set_role`), extended to every
/// blocking `std::fs` syscall in this module.
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
async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    f()
}
```

Both branches share the exact same signature (bounds included) so every
call site below compiles identically regardless of the `tokio` feature —
only this one helper's body forks.

### 2.4 `compression.rs` changes

Replace `runtime/src/sky_runtime/compression.rs:67-122` (the four public
kernels + `zstd_decompress_capped`'s call site — keep `gzip_bytes`,
`gunzip_bytes`, `zstd_decompress_capped` exactly as-is, they're already
correctly-shaped sync helpers; only add a `zstd_compress_bytes` helper and
rewrite the four public functions):

```rust
fn zstd_compress_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    zstd::encode_all(data, 0).map_err(|e| e.to_string())
}

/// Compression.gzip : Bytes -> Task Error Bytes
pub fn compression_gzip<E: From<String> + Send + 'static>(data: Vec<u8>) -> SkyTask<E, Vec<u8>> {
    Box::pin(async move {
        // #129: gzip is CPU-bound; offload to the blocking pool so it can't
        // starve the tokio worker thread polling this future (same rationale
        // as auth.rs's bcrypt spawn_blocking — see the module-level doc
        // comment on decompression-bomb protection above for the sibling
        // memory-safety concern this shares the file with).
        match tokio::task::spawn_blocking(move || gzip_bytes(&data)).await {
            Ok(Ok(b)) => ok_res(b),
            Ok(Err(e)) => SkyResult::Err(format!("Compression.gzip: {}", e).into()),
            Err(_) => {
                SkyResult::Err("Compression.gzip: compression task panicked".to_string().into())
            }
        }
    })
}

/// Compression.gunzip : Bytes -> Task Error Bytes
pub fn compression_gunzip<E: From<String> + Send + 'static>(data: Vec<u8>) -> SkyTask<E, Vec<u8>> {
    Box::pin(async move {
        match tokio::task::spawn_blocking(move || gunzip_bytes(&data)).await {
            Ok(Ok(b)) => ok_res(b),
            Ok(Err(e)) => SkyResult::Err(format!("Compression.gunzip: {}", e).into()),
            Err(_) => {
                SkyResult::Err("Compression.gunzip: decompression task panicked".to_string().into())
            }
        }
    })
}

/// Compression.zstdCompress : Bytes -> Task Error Bytes
pub fn compression_zstd_compress<E: From<String> + Send + 'static>(
    data: Vec<u8>,
) -> SkyTask<E, Vec<u8>> {
    Box::pin(async move {
        match tokio::task::spawn_blocking(move || zstd_compress_bytes(&data)).await {
            Ok(Ok(b)) => ok_res(b),
            Ok(Err(e)) => SkyResult::Err(format!("Compression.zstdCompress: {}", e).into()),
            Err(_) => SkyResult::Err(
                "Compression.zstdCompress: compression task panicked".to_string().into(),
            ),
        }
    })
}

/// Compression.zstdDecompress : Bytes -> Task Error Bytes
pub fn compression_zstd_decompress<E: From<String> + Send + 'static>(
    data: Vec<u8>,
) -> SkyTask<E, Vec<u8>> {
    Box::pin(async move {
        match tokio::task::spawn_blocking(move || zstd_decompress_capped(&data)).await {
            Ok(Ok(b)) => ok_res(b),
            Ok(Err(e)) => SkyResult::Err(format!("Compression.zstdDecompress: {}", e).into()),
            Err(_) => SkyResult::Err(
                "Compression.zstdDecompress: decompression task panicked".to_string().into(),
            ),
        }
    })
}
```

Also **delete** the now-unused `use std::future::ready;` import at the top
of `compression.rs` (line 16) — none of the four kernels use `ready(...)`
anymore, and `-D warnings` in CI will fail on the unused-import lint
otherwise.

Existing tests in `compression.rs` (`gzip_roundtrip`,
`gzip_roundtrip_binary`, `zstd_roundtrip`, `zstd_roundtrip_binary`,
`gunzip_rejects_garbage`, `gunzip_rejects_decompression_bomb`,
`zstd_rejects_decompression_bomb`) call these functions via
`task_run(...)`, which spins up a **full multi-threaded** `tokio::runtime::
Runtime::new()` (see `task.rs:10`, `Runtime::new()` defaults to
multi-thread), so `spawn_blocking` works unchanged there — no existing test
needs modification.

### 2.5 `file.rs` changes

Extract every function's blocking body into a private `*_sync` helper, then
route the public function through `run_blocking`. Full replacement content
for `runtime/src/sky_runtime/file.rs` (the `run_blocking` helper from §2.3
goes right after the top-of-file doc comment / `use` line):

```rust
// File kernel stubs — generic over E.
use super::*;

// ── #129: shared blocking-pool helper (see class9 fix spec §2.2-2.3) ───────
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
async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    f()
}

/// `Sky.Core.File.readFile : String -> Task Error String`. Reads the whole file,
/// but bounded by a hard ceiling so an attacker-controlled path pointing at an
/// unbounded source (`/dev/zero`, a named pipe, a multi-GiB file) cannot OOM the
/// process — `read_to_string` on `/dev/zero` never returns. The ceiling defaults
/// to 512 MiB and is overridable via `SKY_FILE_READ_MAX` (bytes). For a smaller
/// explicit cap use `File.readFileLimit`; reading past the ceiling is an `Err`,
/// never a silent truncation.
fn file_read_ceiling() -> u64 {
    crate::sky_runtime::system::read_env_var("SKY_FILE_READ_MAX")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(512 * 1024 * 1024)
}

fn file_read_file_sync(path: &str, cap: u64) -> Result<String, String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| format!("{}", e))?;
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
}

pub fn file_read_file<E: Send + From<String> + 'static>(path: String) -> SkyTask<E, String> {
    Box::pin(async move {
        let cap = file_read_ceiling();
        match run_blocking(move || file_read_file_sync(&path, cap)).await {
            Ok(s) => ok_res(s),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

fn file_write_file_sync(path: &str, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|e| format!("{}", e))
}

pub fn file_write_file<E: Send + From<String> + 'static>(
    path: String,
    content: String,
) -> SkyTask<E, ()> {
    Box::pin(async move {
        match run_blocking(move || file_write_file_sync(&path, &content)).await {
            Ok(()) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

pub fn file_exists<E: Send + 'static>(path: String) -> SkyTask<E, bool> {
    Box::pin(async move {
        match run_blocking(move || Ok(std::path::Path::new(&path).exists())).await {
            Ok(b) => ok_res(b),
            // Infallible closure — this arm is unreachable but keeps the
            // `Result`-shaped `run_blocking` contract uniform.
            Err(_) => ok_res(false),
        }
    })
}

/// Alias of `file_remove` (the `remove` contract). Kept as a public name for
/// ABI stability; delegates so the two never drift.
pub fn file_delete<E: Send + From<String> + 'static>(path: String) -> SkyTask<E, ()> {
    file_remove(path)
}

fn file_mkdir_all_sync(path: &str) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("{}", e))
}

/// `Sky.Core.File.mkdirAll : String -> Task Error ()` — create the directory
/// and every missing parent (mkdir -p). Already-exists is `Ok` (matching
/// `std::fs::create_dir_all`); a real I/O failure is `Err`.
pub fn file_mkdir_all<E: Send + From<String> + 'static>(path: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        match run_blocking(move || file_mkdir_all_sync(&path)).await {
            Ok(()) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

// ─── Read variants ─────────────────────────────────────────────────────────

/// `Sky.Core.File.readFileLimit : String -> Int -> Task Error String`
/// Read at most `limit` bytes. Returns `Err` when the file is larger than
/// `limit` (to avoid OOM on unbounded inputs) or when the content is not
/// valid UTF-8 (use `readFileBytes` for binary data in that case).
/// A non-positive limit falls back to the same 10 MiB default Go uses.
///
/// AUD-09 gap-sweep TOCTOU fix: see class9 fix spec §1 for the full
/// rationale — no separate `metadata()` pre-check; a single `take(cap+1)`
/// read pass decides over-limit from actual bytes read, removing the race
/// window a stat-then-read split had.
pub fn file_read_file_limit<E: Send + From<String> + 'static>(
    path: String,
    limit: i64,
) -> SkyTask<E, String> {
    let cap: u64 = if limit > 0 {
        limit as u64
    } else {
        10 * 1024 * 1024
    };
    Box::pin(async move {
        match run_blocking(move || file_read_file_limit_sync(&path, cap)).await {
            Ok(s) => ok_res(s),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

fn file_read_file_limit_sync(path: &str, cap: u64) -> Result<String, String> {
    use std::io::Read as _;
    let f = std::fs::File::open(path).map_err(|e| format!("{}", e))?;
    let mut buf = String::new();
    let read = f
        .take(cap.saturating_add(1))
        .read_to_string(&mut buf)
        .map_err(|e| format!("{}", e))?;
    if read as u64 > cap {
        return Err(format!(
            "file exceeds {}-byte limit (stopped reading at the limit — actual size not reported to bound memory use): {}",
            cap, path
        ));
    }
    Ok(buf)
}

fn file_read_file_bytes_sync(path: &str) -> Result<Vec<i64>, String> {
    const DEFAULT_CAP: u64 = 10 * 1024 * 1024;
    use std::io::Read as _;
    let f = std::fs::File::open(path).map_err(|e| format!("{}", e))?;
    let mut buf = Vec::new();
    f.take(DEFAULT_CAP)
        .read_to_end(&mut buf)
        .map_err(|e| format!("{}", e))?;
    Ok(from_u8_slice(&buf))
}

/// `Sky.Core.File.readFileBytes : String -> Task Error (List Int)`
/// Read the file as raw bytes, returned as `Vec<i64>` (Sky `List Int`,
/// values 0..=255). Uses the same 10 MiB default cap as Go. For text
/// content with guaranteed UTF-8, prefer `readFile` / `readFileLimit`.
pub fn file_read_file_bytes<E: Send + From<String> + 'static>(
    path: String,
) -> SkyTask<E, Vec<i64>> {
    Box::pin(async move {
        match run_blocking(move || file_read_file_bytes_sync(&path)).await {
            Ok(v) => ok_res(v),
            Err(e) => SkyResult::Err(str_err(&e)),
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
        .map_err(|e| format!("{}", e))?;
    f.write_all(content.as_bytes()).map_err(|e| format!("{}", e))
}

/// `Sky.Core.File.append : String -> String -> Task Error ()`
/// Append `content` to the end of the file at `path`, creating it if absent.
/// Mirrors Go's `os.OpenFile(…, O_APPEND|O_CREATE|O_WRONLY, 0644)`.
pub fn file_append<E: Send + From<String> + 'static>(
    path: String,
    content: String,
) -> SkyTask<E, ()> {
    Box::pin(async move {
        match run_blocking(move || file_append_sync(&path, &content)).await {
            Ok(()) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

// ─── Removal ───────────────────────────────────────────────────────────────

fn file_remove_sync(path: &str) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| format!("{}", e))
}

/// `Sky.Core.File.remove : String -> Task Error ()`
/// Remove the file at `path`. Returns `Err` on any I/O failure (including
/// "not found"). Mirrors Go's `os.Remove`.
pub fn file_remove<E: Send + From<String> + 'static>(path: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        match run_blocking(move || file_remove_sync(&path)).await {
            Ok(()) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

// ─── Directory queries ─────────────────────────────────────────────────────

fn file_read_dir_sync(path: &str) -> Result<Vec<String>, String> {
    // Propagate per-entry read errors instead of silently dropping them
    // (`rd.flatten()` would discard `Err` items mid-walk, omitting entries
    // a transient stat/readdir failure touched — Go's `os.ReadDir` surfaces
    // such an error rather than returning a truncated list).
    let rd = std::fs::read_dir(path).map_err(|e| format!("{}", e))?;
    let mut names: Vec<String> = Vec::new();
    for entry in rd {
        let entry = entry.map_err(|e| format!("{}", e))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

/// `Sky.Core.File.readDir : String -> Task Error (List String)`
/// Return the names (not full paths) of all entries in the directory at
/// `path`, in filesystem order. Mirrors Go's `os.ReadDir` → `e.Name()`.
pub fn file_read_dir<E: Send + From<String> + 'static>(path: String) -> SkyTask<E, Vec<String>> {
    Box::pin(async move {
        match run_blocking(move || file_read_dir_sync(&path)).await {
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
        let is_dir = run_blocking(move || {
            Ok(std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false))
        })
        .await
        .unwrap_or(false);
        ok_res(is_dir)
    })
}

// ─── Temp paths ────────────────────────────────────────────────────────────

/// `Sky.Core.File.tempFile : String -> Task Error String`
/// Create a uniquely-named empty file in the system temp directory, using
/// `prefix` as the filename prefix. Returns the absolute path.
/// The caller is responsible for removing the file when done.
pub fn file_temp_file<E: Send + From<String> + 'static>(prefix: String) -> SkyTask<E, String> {
    Box::pin(async move {
        match run_blocking(move || make_temp_path(&prefix, false)).await {
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
        match run_blocking(move || make_temp_path(&prefix, true)).await {
            Ok(p) => ok_res(p),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

// `make_temp_path` is unchanged — keep the existing implementation verbatim
// (retry loop, prefix sanitisation, 0700/0600 modes). It is already a plain
// sync `fn(&str, bool) -> Result<String, String>`, so it slots directly into
// `run_blocking` with no wrapper needed.
fn make_temp_path(prefix: &str, is_dir: bool) -> Result<String, String> {
    // ... UNCHANGED — see current file lines 266-319 ...
}

// ─── Copy / rename ─────────────────────────────────────────────────────────

fn file_copy_sync(src: &str, dst: &str) -> Result<(), String> {
    std::fs::copy(src, dst).map(|_| ()).map_err(|e| format!("{}", e))
}

/// `Sky.Core.File.copy : String -> String -> Task Error ()`
/// Copy the file at `src` to `dst`, creating or overwriting `dst`.
/// Mirrors Go's `io.Copy(out, in)` pattern.
pub fn file_copy<E: Send + From<String> + 'static>(src: String, dst: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        match run_blocking(move || file_copy_sync(&src, &dst)).await {
            Ok(()) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}

fn file_rename_sync(src: &str, dst: &str) -> Result<(), String> {
    std::fs::rename(src, dst).map_err(|e| format!("{}", e))
}

/// `Sky.Core.File.rename : String -> String -> Task Error ()`
/// Rename (move) the file or directory at `src` to `dst`.
/// Mirrors Go's `os.Rename`.
pub fn file_rename<E: Send + From<String> + 'static>(src: String, dst: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        match run_blocking(move || file_rename_sync(&src, &dst)).await {
            Ok(()) => ok_res(()),
            Err(e) => SkyResult::Err(str_err(&e)),
        }
    })
}
```

`file_exists`/`file_is_dir` are infallible today (`SkyTask<E, bool>`, no
`Result` in the public signature) — they're included for consistency (a
stalled network-filesystem `stat()` is exactly the reactor-starvation
scenario #129 exists to prevent) but their `run_blocking` closures are
themselves infallible (`Ok(...)` always), so the `Err` arm from a
hypothetical `JoinError` is handled by falling back to `false` rather than
propagating (there is no `Err` channel to propagate into on these two
kernels' existing signatures — changing that would be an unrelated, larger
API change out of scope here).

`make_temp_path` needs NO changes — it is already a plain sync helper
matching the exact shape `run_blocking` expects; only its two call sites
(`file_temp_file`/`file_temp_dir`) change.

Keep `#[cfg(test)] mod read_ceiling_tests` (existing `file_read_file`
over/under-ceiling tests) exactly as-is at the bottom of the file — nothing
about their assertions changes; `spawn_blocking` works transparently under
their existing `Builder::new_current_thread().enable_all()` runtime (tokio's
blocking pool is independent of scheduler flavor — `current_thread` runtimes
still have a working `spawn_blocking`).

### 2.6 New regression tests: prove the offload actually happens

A `spawn_blocking` call that silently no-ops (e.g. a future refactor
accidentally inlines the closure instead of spawning it) would not be caught
by the existing correctness tests (`gzip_roundtrip`, `read_file_rejects_over_
ceiling`, etc.) — those only check the RESULT, not where it ran. Add one
"does this starve a concurrent task" regression per module, which fails
deterministically pre-fix (the shared single worker thread is fully occupied
by the inline blocking call and the concurrent ticker task never gets
polled) and passes deterministically post-fix (the blocking work moves to
tokio's separate blocking-thread pool, leaving the sole worker free to poll
the ticker).

Add to `runtime/src/sky_runtime/file.rs` (new `#[cfg(test)] mod
spawn_blocking_tests`):

```rust
#[cfg(test)]
mod spawn_blocking_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// #129 regression: on a SINGLE-WORKER (current_thread) runtime, a
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
        let p = std::env::temp_dir()
            .join(format!("sky_spawn_blocking_probe_{}.txt", std::process::id()));
        // Large enough that the read takes measurable (not instant) wall time.
        std::fs::write(&p, vec![b'x'; 64 * 1024 * 1024]).unwrap(); // 64 MiB
        std::env::set_var("SKY_FILE_READ_MAX", (128 * 1024 * 1024).to_string());
        let path = p.to_string_lossy().into_owned();

        let ticks = rt.block_on(async move {
            let counter = Arc::new(AtomicU64::new(0));
            let counter2 = counter.clone();
            let ticker = tokio::spawn(async move {
                loop {
                    counter2.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            });
            let read_fut: SkyTask<String, String> = file_read_file(path);
            let _res: SkyResult<String, String> = read_fut.await;
            ticker.abort();
            counter.load(Ordering::Relaxed)
        });

        std::env::remove_var("SKY_FILE_READ_MAX");
        let _ = std::fs::remove_file(&p);

        assert!(
            ticks > 0,
            "concurrent ticker task made ZERO progress while file_read_file ran — \
             the blocking read is starving the single-threaded executor \
             (spawn_blocking missing or not taking effect)"
        );
    }
}
```

Add the analogous test to `runtime/src/sky_runtime/compression.rs`'s
existing `#[cfg(test)] mod tests` block:

```rust
    /// #129 regression: same shape as file.rs's
    /// `file_read_file_does_not_starve_concurrent_async_work` — proves
    /// `compression_zstd_compress` offloads its CPU-bound work to the
    /// blocking pool instead of running it on a single-worker runtime's
    /// sole thread.
    #[test]
    fn zstd_compress_does_not_starve_concurrent_async_work() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        // Low-compressibility-ish payload so zstd actually spends CPU time
        // rather than short-circuiting on a trivially repetitive pattern.
        let payload: Vec<u8> = (0..32 * 1024 * 1024).map(|i| (i % 251) as u8).collect();

        let ticks = rt.block_on(async move {
            let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
            let counter2 = counter.clone();
            let ticker = tokio::spawn(async move {
                loop {
                    counter2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            });
            let fut: SkyTask<String, Vec<u8>> = compression_zstd_compress(payload);
            let _res: SkyResult<String, Vec<u8>> = fut.await;
            ticker.abort();
            counter.load(std::sync::atomic::Ordering::Relaxed)
        });

        assert!(
            ticks > 0,
            "concurrent ticker task made ZERO progress while zstd compression ran — \
             the blocking compression is starving the single-threaded executor \
             (spawn_blocking missing or not taking effect)"
        );
    }
```

These two tests are the only ones in this spec with inherent timing
sensitivity (they rely on the blocking operation taking "long enough" for at
least one `yield_now` to land elsewhere). 32-64 MiB payloads are chosen to be
comfortably slow enough (single-digit-to-tens of milliseconds) on any CI
runner while staying well under the crate's existing test timeout norms; if
CI flakes on a particularly fast runner, raise the payload size rather than
loosening the assertion (`ticks > 0` is already the loosest meaningful
threshold — pre-fix it is deterministically exactly `0`, not "usually low").

## 3. #122 — `Cli.program` view-printer missing separator

### 3.1 Root cause (confirmed by reading `runtime/src/sky_runtime/tea.rs:211-286`)

`cli_program`'s render call sites write `view(model)` verbatim with no
newline, and only the code AFTER the event loop exits appends one trailing
`b"\n"`:

```rust
// initial render
let _ = std::io::stdout().write_all(view(model.clone()).as_bytes());
let _ = std::io::stdout().flush();

while let Some(ev) = rx.recv().await {
    ...
    // per-event render
    let _ = std::io::stdout().write_all(view(model.clone()).as_bytes());
    let _ = std::io::stdout().flush();
}
submgr.stop_all();
let _ = std::io::stdout().write_all(b"\n");   // <- only fires ONCE, after the loop ends
ok_res(())
```

So the FIRST render and every subsequent render write directly adjacent to
each other with nothing in between — if `view` returns `"lines: 0"` then
`"lines: 1"` on two successive events, stdout literally reads
`"lines: 0lines: 1"`. The trailing `write_all(b"\n")` after the loop was
clearly meant to guarantee SOME newline exists after the last thing printed,
but it only covers the very end of the program's output, not the boundary
between any two intermediate renders.

### 3.2 Fix

Move the separator to fire after EVERY render, at its own call site, and
delete the now-redundant post-loop unconditional newline (it would otherwise
double up: the last render already gets its own trailing `\n` from the
per-call-site fix).

Replace `runtime/src/sky_runtime/tea.rs:263-284` with:

```rust
        // Inline render (a closure borrowing `view` would make the future non-Send).
        // Fallible writes (NOT print!/println!, which panic on a broken pipe).
        //
        // #122: each `view` render is a distinct rendered frame and MUST end in
        // a newline so consecutive renders don't run together on one line
        // (observed as "lines: 0lines: 1" — the second render's text glued
        // directly onto the first's, since neither `view`'s own returned
        // String nor this print loop supplied a separator). Every render
        // call-site writes its own trailing "\n" immediately after the view
        // bytes, so the separator is never skipped regardless of how the loop
        // exits (in particular: an immediate stdin EOF, which breaks out of
        // the loop before ever reaching the loop-body render, still gets a
        // trailing newline from the INITIAL render below).
        let _ = std::io::stdout().write_all(view(model.clone()).as_bytes());
        let _ = std::io::stdout().write_all(b"\n");
        let _ = std::io::stdout().flush();

        while let Some(ev) = rx.recv().await {
            let msg = match ev {
                CliEvent::Line(l) => on_line(l),
                CliEvent::Key(_, _) => continue, // Cli has no keys
                CliEvent::Msg(m) => m,
                CliEvent::Eof => break,
            };
            let (next, cmd) = update(msg, model);
            model = next;
            cli_run_cmd(cmd, &tx);
            submgr.update(subscriptions(model.clone()));
            let _ = std::io::stdout().write_all(view(model.clone()).as_bytes());
            let _ = std::io::stdout().write_all(b"\n");
            let _ = std::io::stdout().flush();
        }
        submgr.stop_all();
        ok_res(())
```

(Deletes the standalone `let _ = std::io::stdout().write_all(b"\n");` that
used to sit between `submgr.stop_all();` and `ok_res(())` — it is now
redundant since the last render printed, whichever call site produced it,
already appended its own newline.)

### 3.3 Verify existing test still passes, no golden updates needed

`crates/skyc/tests/golden_i111_cli_program_seal.rs` (the only existing test
exercising `cli_program`, confirmed via repo-wide search — no other golden
fixture or E2E test references `Cli.program`/`cli_program`) asserts:

```rust
assert!(
    outcome.stdout.contains("lines: 0"),
    ...
);
```

a substring check, unaffected by an added trailing `\n`. Its harness runs
with stdin at EOF (`Command::output` nulls stdin), so `CliEvent::Eof` arrives
almost immediately and the loop body never executes a second render in that
fixture — only the initial render's new trailing `\n` fires, net output is
`"lines: 0\n"` either way (old: no `\n` after init-render, then one `\n`
after the loop breaks immediately; new: `\n` right after init-render, none
after the loop). Byte-for-byte identical in this single-render case. No
golden file changes required.

### 3.4 New regression test: multi-render separator

Add a new E2E golden that actually pipes 2+ lines through stdin so the loop
body renders more than once, proving the fix closes the concatenation bug
end-to-end (not just at the unit level, since the bug is in the runtime's
print loop, which only an E2E run through the real Cli TEA loop exercises).

Create `tests/golden/i122_cli_program_view_separator/Main.sky`:

```elm
module Main exposing (main)

-- #122 regression: Cli.program must print each `view` render on its own
-- line. Two `onLine`-triggered updates force two loop-body renders; without
-- the fix stdout would read "lines: 0lines: 1lines: 2" with no separators.

type Msg
    = LineReceived String

type alias Model =
    { count : Int }

init : () -> ( Model, Cmd Msg )
init _ =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        LineReceived _ ->
            ( { model | count = model.count + 1 }, Cmd.none )

view : Model -> String
view model =
    "lines: " ++ String.fromInt model.count

subscriptions : Model -> Sub Msg
subscriptions _ =
    Sub.none

onLine : String -> Msg
onLine l =
    LineReceived l

main =
    Cli.program
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , onLine = onLine
        }
```

Add `crates/skyc/tests/golden_i122_cli_program_separator.rs`, modeled
directly on `golden_i111_cli_program_seal.rs` but feeding stdin lines instead
of leaving it at EOF (check `crates/skyc/tests/support.rs` — if
`build_and_run_emitted` doesn't currently support piping stdin input, add a
`build_and_run_emitted_with_stdin(name, out_dir, stdin_bytes)` variant that
sets `Command::stdin(Stdio::piped())`, writes the bytes, and closes the
handle before waiting, mirroring `build_and_run_emitted`'s existing process
setup):

```rust
//! #122 regression — `Cli.program`'s view printer must separate consecutive
//! renders with a newline. Pre-fix, piping 2 lines through stdin produced
//! "lines: 0lines: 1lines: 2" (renders glued together); post-fix each
//! render lands on its own line.
//!
//! Gated on `SKY_E2E=1`. Run:
//!
//! ```text
//! SKY_E2E=1 cargo test -p skyc --test golden_i122_cli_program_separator
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

#[test]
fn cli_program_separates_consecutive_renders() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let dir = root.join("tests").join("golden").join("i122_cli_program_view_separator");
    let entry = dir.join("Main.sky");
    let out = std::env::temp_dir().join("skyc_i122_cli_program_view_separator_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "skyc build must succeed: {:?}", built.err());

    // Two stdin lines → two loop-body renders (count 0 → 1 → 2), then EOF.
    let outcome =
        support::build_and_run_emitted_with_stdin("i122_cli_program_view_separator", &out, b"a\nb\n");

    assert_eq!(outcome.exit_code, Some(0));
    let expected = "lines: 0\nlines: 1\nlines: 2\n";
    assert_eq!(
        outcome.stdout, expected,
        "consecutive Cli.program renders must be newline-separated, got: {:?}",
        outcome.stdout
    );
}
```

If `support::build_and_run_emitted_with_stdin` does not yet exist, add it to
`crates/skyc/tests/support.rs` alongside `build_and_run_emitted` — same
`Command` construction, plus `.stdin(Stdio::piped())` and a
`child.stdin.take().unwrap().write_all(bytes)` + explicit `drop(stdin)`
before `.wait_with_output()` so EOF is signalled after the two lines.

## 4. #157 — `Sky.Core.Jwt` builder API surface

### 4.1 Finding: this item is ALREADY LANDED, not a gap

The backlog entry and `docs/divergences-from-sky.md` §B9 both describe this
as an open gap ("only flat kernels exist... a builder-API program does not
yet compile on ipê"). **That description is now stale.** Direct inspection
of HEAD shows the full builder API is wired end-to-end:

- **Runtime kernels** (`runtime/src/sky_runtime/jwt.rs:324-539`, tagged
  `D-00, #152`): `sky_jwt_claims`, `sky_jwt_hs256`, `sky_jwt_rs256`,
  `sky_jwt_subject`, `sky_jwt_issuer`, `sky_jwt_audience`,
  `sky_jwt_expires_at`, `sky_jwt_not_before`, `sky_jwt_issued_at`,
  `sky_jwt_jwt_id`, `sky_jwt_with_claim`, `sky_jwt_encode`, `sky_jwt_decode`
  — all present, all implemented on top of the same Go-parity JSON encoder
  the flat kernels use (byte-identical tokens).
- **Kernel registry** (`crates/sky_kernels/src/lib.rs:443-469`,
  `2561-2574`): `KernelFn::JwtClaims` / `JwtHs256` / `JwtRs256` /
  `JwtSubject` / `JwtIssuer` / `JwtAudience` / `JwtExpiresAt` /
  `JwtNotBefore` / `JwtIssuedAt` / `JwtJwtId` / `JwtWithClaim` / `JwtEncode`
  / `JwtDecode` all registered with correct arity + runtime-symbol mapping.
- **Type constraints** (`crates/sky_types/src/constrain.rs:407-413,
  576-578, 3159, 4321-4340, 5848-5864`): opaque `Claims`/`Algorithm` type
  constructor symbols registered; every builder function has a full type
  scheme (`Jwt.claims : Claims`, `Jwt.hs256/rs256 : String -> Algorithm`,
  `Jwt.subject/issuer/audience/jwtId : String -> Claims -> Claims`,
  `Jwt.expiresAt/notBefore/issuedAt : Int -> Claims -> Claims`,
  `Jwt.withClaim : String -> String -> Claims -> Claims`,
  `Jwt.encode : Algorithm -> Claims -> Result Error String`,
  `Jwt.decode : Algorithm -> Int -> String -> Result Error String`).
- **Lowering** (`crates/sky_lower/src/lower.rs:4412-4426, 4782-4796,
  6907-6908, 7153-7157, 7265-7273, 7423-7427, 8495-8511`): `Claims` lowers
  to the same opaque JSON-accumulator IR type as elsewhere; `Algorithm`
  lowers to the `String` IR representation; every `("Jwt", "<name>")`
  dispatch arm is wired to its `KernelFn` variant with correct arity
  handling (0/1/2/3-ary buckets).
- **E2E golden, already GREEN**: `tests/golden/m_jwt_decode_now/Main.sky`
  uses the FULL builder syntax —
  `Jwt.claims |> Jwt.expiresAt 1000 |> Jwt.notBefore 100`,
  `Jwt.encode (Jwt.hs256 secret) claims`,
  `Jwt.decode (Jwt.hs256 key) now tok` — no `import` statement needed (`Jwt`
  resolves as a kernel-module qualifier directly, same as every other
  stdlib kernel namespace in this frontend). `crates/skyc/tests/
  golden_m5b_uuid_jwt.rs:257` calls `assert_runs_and_matches_oracle
  ("m_jwt_decode_now")`. **Verified in this session**: `SKY_E2E=1 cargo test
  -p skyc --test golden_m5b_uuid_jwt jwt_decode_now` → `test jwt_decode_now
  ... ok` (1 passed, 12.12s). This is a real skyc → cargo build → run →
  output-match E2E pass, not a Rust-level unit test of the kernel functions
  in isolation.

So the CORE ask of #157 — "make `Jwt.encode`/`Jwt.hs256`/`Jwt.claims`/
`Algorithm`/`Claims` compile on Ipê" — is done, has a green E2E regression,
and needs no further kernel/constrain/lower work.

### 4.2 What's actually left (narrow, doc-accuracy + test-completeness only)

1. **`docs/architecture/backlog.md` #157 entry is stale.** Update it to
   record landed status, matching the exact pattern already used for #159
   (a stale-doc entry discovered mid-audit). Suggested replacement text for
   the `#157` bullet in the "Non-blocking hardening / follow-ups" section:

   ```markdown
   - **#157** ✅ **LANDED** (pre-dates this session's read; exact landing
     commit not identified during the 2026-07-09 gap-sweep, confirmed via
     direct source read + a green E2E run) `Sky.Core.Jwt` builder API
     (`Jwt.encode`/`Jwt.hs256`/`Jwt.rs256`/`Jwt.claims`/`Jwt.subject`/
     `Jwt.issuer`/`Jwt.audience`/`Jwt.expiresAt`/`Jwt.notBefore`/
     `Jwt.issuedAt`/`Jwt.jwtId`/`Jwt.withClaim`/`Jwt.decode`, per CLAUDE.md's
     stdlib table) is fully wired: runtime kernels (`jwt.rs:324-539`, D-00/
     #152), kernel registry (`sky_kernels/lib.rs`), type schemes
     (`constrain.rs`), lowering (`lower.rs`), all present. E2E-verified via
     `tests/golden/m_jwt_decode_now` (`Jwt.claims`/`expiresAt`/`notBefore`/
     `encode`/`hs256`/`decode`), asserted by
     `crates/skyc/tests/golden_m5b_uuid_jwt.rs::jwt_decode_now` — confirmed
     green (`SKY_E2E=1 cargo test -p skyc --test golden_m5b_uuid_jwt
     jwt_decode_now`). Remaining gap is test-COVERAGE only, not
     feature-completeness: no E2E golden yet exercises `Jwt.rs256`/
     `Jwt.subject`/`Jwt.issuer`/`Jwt.audience`/`Jwt.issuedAt`/`Jwt.jwtId`/
     `Jwt.withClaim` through the full skyc→cargo→run pipeline (only via
     `jwt.rs`'s in-crate Rust unit tests, which call the runtime functions
     directly, bypassing the compiler frontend). See
     `docs/architecture/class9-kernel-robustness-fix-spec-2026-07-09.md` §4
     for the full landed-surface audit and the one recommended follow-up
     golden.
   ```

2. **`docs/divergences-from-sky.md` §B9 is stale** — it says "a builder-API
   program does not yet compile on ipê". Update it to record the builder API
   as landed and re-scope the divergence to just the fact that BOTH surfaces
   (flat kernels AND the builder API) now coexist on ipê, while Go only ever
   had the builder API — i.e. ipê is a superset, not "interim / narrower".
   Suggested replacement for §B9:

   ```markdown
   ### B9 — `Sky.Core.Jwt` exposes both a flat-kernel surface and the builder API
   - **Differs:** ipê surfaces the Go-reference builder API in full
     (`Jwt.encode (Jwt.hs256 secret) (Jwt.claims |> Jwt.subject … |> …)`,
     `Algorithm`/`Claims` opaque types, `Jwt.decode`) AND additionally keeps
     four flat kernels (`encodeHs256`/`decodeHs256`/`encodeRs256`/
     `decodeRs256`, claims as a raw JSON string) as an ADDITIONAL surface not
     present on Go. Both surfaces produce byte-identical tokens (same
     Go-parity JSON encoder + crypto primitives underneath).
   - **Go-oracle relationship:** the builder-API program compiles and
     produces byte-identical output on both backends (see
     `tests/golden/m_jwt_decode_now`, `m5b_jwt_hs256_bytes`,
     `m5b_jwt_rs256_bytes`). The flat-kernel programs are ipê-only (Go has no
     flat surface) and are recorded as their own per-golden sanctioned
     divergences (`m5b_jwt_hs256_roundtrip`, `m5b_jwt_hs256_tamper`,
     `m5b_jwt_rs256_roundtrip`, etc.) — unaffected by this update.
   - **Rationale:** the flat kernels predate the builder-API port and are
     kept for their existing golden coverage / byte-parity assertions
     (`golden_m5b_uuid_jwt.rs`); removing them is unnecessary churn.
   - **Sanctioned:** yes (`sanctioned:` — additive surface, no behavior
     change on the shared builder-API path).
   ```

3. **Recommended (not required) completeness golden.** Add one more E2E
   golden exercising the currently-untested builder functions in one pass —
   `Jwt.rs256`, `Jwt.subject`, `Jwt.issuer`, `Jwt.audience`, `Jwt.issuedAt`,
   `Jwt.jwtId`, `Jwt.withClaim` — so every builder-API kernel has E2E (not
   just Rust-unit-test) coverage. Suggested fixture,
   `tests/golden/m_jwt_builder_full_surface/Main.sky`:

   ```elm
   module Main exposing (main)

   -- #157 completeness: exercise every Jwt builder function NOT already
   -- covered by m_jwt_decode_now (which covers claims/expiresAt/notBefore/
   -- encode/hs256/decode). This adds rs256/subject/issuer/audience/
   -- issuedAt/jwtId/withClaim through the real skyc pipeline (not just
   -- jwt.rs's in-crate Rust unit tests).

   privKeyPem : String
   privKeyPem =
       "-----BEGIN PRIVATE KEY-----\n...\n-----END PRIVATE KEY-----\n"

   pubKeyPem : String
   pubKeyPem =
       "-----BEGIN PUBLIC KEY-----\n...\n-----END PUBLIC KEY-----\n"

   main =
       let
           claims =
               Jwt.claims
                   |> Jwt.subject "alice"
                   |> Jwt.issuer "sky-test-suite"
                   |> Jwt.audience "sky-api"
                   |> Jwt.issuedAt 1000
                   |> Jwt.jwtId "test-jti-001"
                   |> Jwt.withClaim "role" "admin"
       in
       case Jwt.encode (Jwt.rs256 privKeyPem) claims of
           Err _ ->
               println "encode-error"

           Ok token ->
               case Jwt.decode (Jwt.rs256 pubKeyPem) 500 token of
                   Ok payload ->
                       println payload

                   Err _ ->
                       println "decode-error"
   ```

   (Reuse the existing `RS256_PRIV_PEM`/`RS256_PUB_PEM` test-only keypair
   already embedded in `runtime/src/sky_runtime/jwt.rs`'s test module and in
   `tests/golden/m5b_jwt_rs256_roundtrip/Main.sky`, rather than generating a
   new one, so there is exactly one test keypair to ever rotate.) Add a
   matching `crates/skyc/tests/golden_m_jwt_builder_full_surface.rs` E2E
   test modeled on `golden_m5b_uuid_jwt.rs`'s existing structure, asserting
   the payload JSON contains `"sub":"alice"`, `"iss":"sky-test-suite"`,
   `"aud":"sky-api"`, `"iat":1000`, `"jti":"test-jti-001"`, and
   `"role":"admin"`.

   This is a completeness nice-to-have, not a blocking fix — #157's actual
   ask (builder API compiles) is already satisfied and E2E-proven by the
   existing `m_jwt_decode_now` golden.

## 5. Verification commands (run after implementing §1-3; §4 is doc-only + optional test addition)

```bash
# 1. Unit tests for file.rs + compression.rs (includes new TOCTOU + spawn_blocking regressions)
cargo test -p sky-runtime-rust --features full file:: 2>&1 | tee /tmp/class9-file-tests.log
cargo test -p sky-runtime-rust --features full compression:: 2>&1 | tee /tmp/class9-compression-tests.log

# 2. Whole-workspace gate (the standing non-negotiable before any merge)
cargo build --workspace 2>&1 | tee /tmp/class9-build.log
cargo test --workspace 2>&1 | tee /tmp/class9-test.log
cargo clippy --all-targets --workspace -- -D warnings 2>&1 | tee /tmp/class9-clippy.log

# 3. The narrow-feature CI gate this spec's file.rs cfg-gating is specifically
#    designed to keep green (default features = [], no tokio):
cargo clippy -p sky-runtime-rust --no-default-features --features db --all-targets -- -D warnings
cargo clippy -p sky-runtime-rust --no-default-features --features db,live --all-targets -- -D warnings
cargo clippy -p sky-runtime-rust --all-targets -- -D warnings   # default = [] — must still compile

# 4. E2E goldens (compression / file / cli / jwt)
SKY_E2E=1 cargo test -p skyc --test golden_i111_cli_program_seal 2>&1 | tee /tmp/class9-i111.log
SKY_E2E=1 cargo test -p skyc --test golden_i122_cli_program_separator 2>&1 | tee /tmp/class9-i122.log   # new
SKY_E2E=1 cargo test -p skyc --test golden_m5b_uuid_jwt 2>&1 | tee /tmp/class9-jwt.log
SKY_E2E=1 cargo test -p skyc --test golden_m_jwt_builder_full_surface 2>&1 | tee /tmp/class9-jwt-full.log  # new, optional

# 5. Full example/oracle sweep (only if touching shared runtime files broadly —
#    §1/§2 touch file.rs + compression.rs which several examples exercise)
scripts/example-sweep.sh 2>&1 | tee /tmp/class9-example-sweep.log
```

## 6. Summary of files touched (implementation phase, NOT done in this spec pass)

| File | Change |
|---|---|
| `runtime/src/sky_runtime/file.rs` | TOCTOU fix (§1) + `spawn_blocking` via shared `run_blocking` helper (§2.5) + 2 new test modules |
| `runtime/src/sky_runtime/compression.rs` | `spawn_blocking` on all 4 kernels (§2.4), remove unused `ready` import, +1 new test |
| `runtime/src/sky_runtime/tea.rs` | `cli_program` separator fix (§3.2) |
| `tests/golden/i122_cli_program_view_separator/Main.sky` | new fixture (§3.4) |
| `crates/skyc/tests/golden_i122_cli_program_separator.rs` | new E2E test (§3.4) |
| `crates/skyc/tests/support.rs` | add `build_and_run_emitted_with_stdin` if not already present (§3.4) |
| `docs/architecture/backlog.md` | update stale #157 entry (§4.2.1) |
| `docs/divergences-from-sky.md` | update stale §B9 (§4.2.2) |
| `tests/golden/m_jwt_builder_full_surface/Main.sky` | optional new fixture (§4.2.3) |
| `crates/skyc/tests/golden_m_jwt_builder_full_surface.rs` | optional new E2E test (§4.2.3) |

No changes to `runtime/src/sky_runtime/io.rs`, `time.rs`, `basics.rs`,
`math.rs`, or `jwt.rs` — all confirmed already correct or already complete.

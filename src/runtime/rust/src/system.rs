// System helpers — some generic over E (when returning IpeTask).
use super::{IpeMaybe, IpeResult, IpeTask, ok_res, str_err};

// `std::env::set_var`/`remove_var` are documented as NOT thread-safe: a mutator
// can reallocate the C `environ` block while another thread READS it
// (`std::env::var` walks `environ`), which is a data race / use-after-free by the
// std + POSIX contract — not just a mutator↔mutator hazard. Both are reachable
// from Ipê purely through env-Task composition under `Task.parallel`
// (`System.setenv`/`unsetenv`/`loadEnv` are mutators; `System.getenv*` are
// readers). Serialise BOTH sides behind one process-global RwLock: mutators take
// the write lock (exclusive), readers take the read lock (shared with each other,
// excluded against any mutator). This closes the reader↔mutator race for every
// Ipê-originated access. (A non-Ipê dependency reading `environ` without this lock
// is outside our reach — but every Ipê path is now serialised.)
static ENV_LOCK: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// Read an environment variable under the shared env read lock (excluded against
/// any concurrent mutator so the `environ` walk can't race a realloc). `pub(crate)`
/// so every non-test process-env read in the crate routes through this one lock —
/// that's what makes the reader↔mutator serialisation true by construction.
pub(crate) fn read_env_var(key: &str) -> Result<String, std::env::VarError> {
    let _guard = ENV_LOCK
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::env::var(key)
}

/// Read a renamed environment variable with a deprecated-alias fallback.
/// Checks `new_key` first; if absent, tries `old_key`. A set-but-empty
/// `new_key` wins (returns `Ok("")`) and does not fall through to the alias.
/// Both reads share the same lock acquisition, so the check-then-fallback is
/// atomic with respect to other Ipê-originated env mutations.
///
/// Use this for every env var that was renamed: the new name takes full effect
/// while the old name keeps working as a deprecated alias. Callers that need
/// to distinguish "unset" from "set to empty" should treat `VarError::NotFound`
/// on the return as "neither set".
pub(crate) fn read_env_var_renamed(
    new_key: &str,
    old_key: &str,
) -> Result<String, std::env::VarError> {
    let _guard = ENV_LOCK
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match std::env::var(new_key) {
        Ok(v) => Ok(v),
        Err(_) => std::env::var(old_key),
    }
}

/// Read an environment variable as an `OsString` under the shared env read lock —
/// the `var_os` companion of `read_env_var` (same poison handling, same race
/// guarantee). `None` when unset or — unlike `read_env_var` — when the value is
/// not valid Unicode. Gated to the feature whose module actually reads `var_os`
/// (`tui` — the `NO_COLOR` probe); widen the gate when another feature gains a
/// `var_os` reader, so it never sits as dead code under `-D warnings`.
#[cfg(feature = "tui")]
pub(crate) fn read_env_var_os(key: &str) -> Option<std::ffi::OsString> {
    let _guard = ENV_LOCK.read().unwrap_or_else(|p| p.into_inner());
    std::env::var_os(key)
}

/// Set an environment variable under the exclusive env write lock.
pub(crate) fn locked_set_var(key: &str, val: &str) {
    // std::env::set_var PANICS on an empty key, a key containing '=' or NUL, or a
    // value containing NUL. Skip such a key/value (no-op) rather than panic.
    if key.is_empty() || key.contains('=') || key.contains('\0') || val.contains('\0') {
        return;
    }
    let _guard = ENV_LOCK
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // SAFETY: `set_var` is `unsafe` in Rust 2024 because a concurrent reader
    // walking `environ` can race the mutation. The exclusive `ENV_LOCK` write
    // guard held here excludes every Ipê-originated reader (all route through
    // `read_env_var`/`read_env_var_os`, which take the shared read lock), so no
    // such reader can run during this write.
    unsafe { std::env::set_var(key, val) };
}

/// Set an environment variable ONLY if it is currently absent, performing the
/// presence check and the set atomically under a SINGLE write-lock acquisition.
/// This avoids the TOCTOU window a separate `read_env_var_os` + `locked_set_var`
/// pair would open (a concurrent mutator could set the key between the two lock
/// acquisitions). Same invalid-key/value guard as `locked_set_var` — never panics.
pub(crate) fn locked_set_var_if_absent(key: &str, val: &str) {
    if key.is_empty() || key.contains('=') || key.contains('\0') || val.contains('\0') {
        return;
    }
    let _guard = ENV_LOCK
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if std::env::var_os(key).is_none() {
        // SAFETY: held under the exclusive `ENV_LOCK` write guard, which excludes
        // every Ipê-originated reader (all take the shared read lock) — see
        // `locked_set_var`. The presence check and set share this one acquisition.
        unsafe { std::env::set_var(key, val) };
    }
}

/// Remove an environment variable under the exclusive env write lock.
pub(crate) fn locked_remove_var(key: &str) {
    // std::env::remove_var panics on the same invalid keys as set_var — guard it.
    if key.is_empty() || key.contains('=') || key.contains('\0') {
        return;
    }
    let _guard = ENV_LOCK
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // SAFETY: held under the exclusive `ENV_LOCK` write guard, which excludes
    // every Ipê-originated reader (all take the shared read lock) — see
    // `locked_set_var`.
    unsafe { std::env::remove_var(key) };
}

#[must_use]
pub fn system_args<E: Send + 'static>(_: ()) -> IpeTask<E, Vec<String>> {
    Box::pin(async move { ok_res(std::env::args().skip(1).collect()) })
}

// ── shared blocking-pool helper ───────────────────────────────────────
//
// `process_run` calls `std::process::Command::output()`, which BLOCKS the
// calling thread until the child process exits — an arbitrarily long wait
// (the whole point of `Process.run` is running a caller-chosen subprocess).
// On a tokio worker thread that stalls every other task scheduled on it for
// the subprocess's full runtime — reactor starvation, same class as the
// bcrypt/gzip/zstd/file cases. `system` (this module) is UNCONDITIONALLY
// compiled (not gated behind any feature — see the module-level comment
// above `pub mod system;` in `mod.rs`), while `tokio` is an `optional = true`
// dependency, so `tokio` is not guaranteed present here. Same
// `#[cfg(feature = "tokio")]` / fallback split `file.rs` uses for its own
// `run_blocking` helper (real generated Ipê projects always have `tokio` —
// see `docs/adr/0014-kernel-robustness-blocking-offload-and-toctou.md`
// §2.2 — so the fallback only matters for this crate's own narrow-feature
// standalone builds).
// tokio is native-only (declared under the `cfg(not(target_arch = "wasm32"))`
// dependency table), so the `spawn_blocking` offload compiles only there. On
// wasm32 the synchronous fallback runs even when `feature = "tokio"` is set —
// the browser has no blocking-thread pool to offload to.
#[cfg(all(feature = "tokio", not(target_arch = "wasm32")))]
async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(_) => Err("background process task panicked".to_string()),
    }
}

#[cfg(any(not(feature = "tokio"), target_arch = "wasm32"))]
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

/// The default combined-output capture ceiling (16 MiB), overridable via
/// `IPE_PROCESS_OUTPUT_MAX` (bytes). A subprocess is a caller-chosen program
/// that may write without bound (or be attacker-influenced); an uncapped
/// `Command::output()` buffers ALL of it in memory and can OOM the host. Reading
/// past the ceiling is an `Err`, never a silent truncation of a returned success
/// value.
fn process_output_ceiling() -> u64 {
    read_env_var("IPE_PROCESS_OUTPUT_MAX")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(16 * 1024 * 1024)
}

/// The captured result of a subprocess: its combined stdout+stderr (bounded)
/// and whether it exited successfully. `status` is the display form of the exit
/// status so the sync helper needs no `std::process` types in its signature.
struct ProcessCapture {
    combined: Vec<u8>,
    success: bool,
    status: String,
}

/// RAII owner of a spawned child: guarantees the child is reaped on EVERY exit
/// path (early `?` return, panic, or normal completion). `std::process::Child`'s
/// `Drop` does NOT kill or reap, so without this a read error or a bail would
/// leak a running, unreaped child (a zombie in a long-lived server). `wait()`
/// takes ownership so the destructor becomes a no-op once the caller has reaped
/// the child itself on the success path.
struct ChildGuard(Option<std::process::Child>);

impl ChildGuard {
    fn get_mut(&mut self) -> Option<&mut std::process::Child> {
        self.0.as_mut()
    }

    /// Reap the child ourselves, taking it out of the guard so `Drop` does
    /// nothing. Returns the exit status.
    fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        match self.0.take() {
            Some(mut c) => c.wait(),
            None => Err(std::io::Error::other("child already reaped")),
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            // The child is still owned here => it was NOT reaped on a normal
            // path (an error/bail/panic left it running). Kill then reap so no
            // subprocess is left running and no zombie accumulates.
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Read up to `limit` bytes from `reader` on a dedicated thread. Draining each
/// pipe on its OWN thread avoids the sequential-drain deadlock: a child that
/// fills one pipe's kernel buffer while blocking on the other cannot wedge the
/// capture, because both pipes are drained concurrently. The `take(limit)` bound
/// caps peak per-stream allocation regardless of how much the child writes.
/// `limit` is a per-call value (`cap + 1`) passed by ownership, so concurrent
/// `process_run` calls never share or clobber it.
fn spawn_capture_thread<R>(
    reader: Option<R>,
    limit: u64,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>>
where
    R: std::io::Read + Send + 'static,
{
    std::thread::spawn(move || {
        use std::io::Read as _;
        let mut buf = Vec::new();
        if let Some(reader) = reader {
            reader.take(limit).read_to_end(&mut buf)?;
        }
        Ok::<_, std::io::Error>(buf)
    })
}

/// Spawn `cmd args` with NO shell (direct argv), capturing combined
/// stdout+stderr under `cap`. stdout and stderr are drained on SEPARATE threads
/// (no sequential-drain pipe deadlock), each bounded by `take(cap + 1)`; the
/// combined result over `cap` is an `Err`, never an unbounded allocation.
/// `stdin` is closed (`Stdio::null`) so a child reading stdin gets EOF and
/// cannot block the capture. The child is reaped on every exit path via
/// [`ChildGuard`].
fn process_run_sync(cmd: &str, args: &[String], cap: u64) -> Result<ProcessCapture, String> {
    use std::process::{Command, Stdio};

    let child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{cmd}: {e}"))?;
    let mut guard = ChildGuard(Some(child));

    // Per-stream read bound; passed by value to each capture thread (no shared
    // global, so concurrent `process_run` calls cannot clobber one another).
    let limit = cap.saturating_add(1);

    // Take the pipe handles so each thread owns its reader; a `?` before the
    // joins still reaps the child via `guard`'s `Drop`.
    let (stdout, stderr) = {
        let c = guard
            .get_mut()
            .ok_or_else(|| format!("{cmd}: child unexpectedly reaped"))?;
        (c.stdout.take(), c.stderr.take())
    };
    let out_handle = spawn_capture_thread(stdout, limit);
    let err_handle = spawn_capture_thread(stderr, limit);

    // A thread panic (e.g. OOM in the reader) surfaces as an `Err`, never a
    // propagated panic; `guard` still reaps the child on the `?` return.
    let mut combined = out_handle
        .join()
        .map_err(|_| format!("{cmd}: stdout capture thread panicked"))?
        .map_err(|e| format!("{cmd}: {e}"))?;
    let stderr_bytes = err_handle
        .join()
        .map_err(|_| format!("{cmd}: stderr capture thread panicked"))?
        .map_err(|e| format!("{cmd}: {e}"))?;
    // Combined = stdout then stderr (callers usually treat it as `2>&1`).
    combined.extend_from_slice(&stderr_bytes);

    if combined.len() as u64 > cap {
        // `guard`'s `Drop` kills + reaps the still-running child on this bail.
        return Err(format!(
            "{cmd}: output exceeds the {cap}-byte capture ceiling \
             (raise IPE_PROCESS_OUTPUT_MAX)"
        ));
    }

    let status = guard.wait().map_err(|e| format!("{cmd}: {e}"))?;
    Ok(ProcessCapture {
        combined,
        success: status.success(),
        status: format!("{status}"),
    })
}

/// `Ipe.Process.run : String -> List String -> Task Error String` — run a
/// subprocess with NO shell (the arguments are a direct `argv` vector, never
/// passed to `sh -c`, so a caller-controlled argument can never be reinterpreted
/// as shell syntax — no command injection). Returns the child's combined
/// stdout+stderr on a clean exit; a non-zero exit or a spawn failure is `Err`
/// carrying the captured output + the status. Total — every failure maps to
/// `Err`, never a panic.
///
/// SECURITY: `Process.run` is a server-only capability (`subprocess`):
/// default-denied under `--target wasm`, and a program that reaches it is tagged
/// with the `subprocess` capability so a sandbox can isolate it. Captured output
/// is bounded (`process_output_ceiling`) so an unbounded-output child cannot OOM
/// the host. Sandboxing which programs may be spawned is the calling
/// application's responsibility.
///
/// The blocking spawn+wait is offloaded via `run_blocking` (see the module-level
/// doc comment above) so a long-running subprocess can't stall the tokio worker
/// thread polling this future.
#[must_use]
pub fn process_run<E: Send + From<String> + 'static>(
    cmd: String,
    args: Vec<String>,
) -> IpeTask<E, String> {
    process_run_with_cap(cmd, args, process_output_ceiling())
}

/// `process_run` with the capture ceiling supplied explicitly rather than read
/// from the environment. `process_run` reads `process_output_ceiling()` once and
/// forwards it here; tests exercise a specific ceiling by passing it directly,
/// so no test mutates the process-global environment (which would race a
/// concurrent subprocess call reading the same var).
#[must_use]
fn process_run_with_cap<E: Send + From<String> + 'static>(
    cmd: String,
    args: Vec<String>,
    cap: u64,
) -> IpeTask<E, String> {
    Box::pin(async move {
        // `process_run_sync` folds `cmd` into every `Err` string, so the outer
        // `Err` arm (a `run_blocking` `JoinError`, i.e. the blocking task
        // panicked) doesn't need `cmd` — it's moved into the closure.
        match run_blocking(move || process_run_sync(&cmd, &args, cap)).await {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.combined).into_owned();
                if out.success {
                    ok_res(text)
                } else {
                    // Cap the captured output folded into the Err string: large /
                    // binary subprocess output bloats the error and may embed
                    // secrets the process printed. Truncate to a bounded prefix
                    // (on a char boundary) before prepending the status.
                    const MAX_ERR_OUTPUT: usize = 4096;
                    let snippet: String = if text.len() > MAX_ERR_OUTPUT {
                        let mut end = MAX_ERR_OUTPUT;
                        while end > 0 && !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        // Total accessor — `end` is a char boundary <= len, so
                        // `get` yields Some; the fallback keeps it slice-free and
                        // clippy::indexing_slicing-clean for non-test runtime code.
                        format!("{}… (output truncated)", text.get(..end).unwrap_or(&text))
                    } else {
                        text
                    };
                    IpeResult::Err(str_err(&format!("{}: {}", snippet, out.status)))
                }
            }
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}
/// The structured result of a `runWith` spawn: independent exit code, stdout, and
/// stderr captures. Exposed as a `pub struct` so the emitter can access its fields
/// directly (the same pattern `email::EmailMessage` and `cache::CacheCfg` use).
///
/// Field names match the Ipê record keys verbatim (`exitCode` / `stdout` /
/// `stderr`); `#[allow(non_snake_case)]` suppresses the style lint for `exitCode`.
#[allow(non_snake_case)]
pub struct ProcessRunOutput {
    pub exitCode: i64,
    pub stdout: String,
    pub stderr: String,
}

/// Spawn `cmd args` under optional `cwd` and env overrides, capturing stdout and
/// stderr INDEPENDENTLY on SEPARATE threads (no sequential-drain pipe-deadlock),
/// each bounded by `take(cap + 1)`. Returns the exit code alongside the two
/// streams. The child is reaped on every exit path via `ChildGuard`. An env pair
/// whose key is empty, contains `=` or NUL, or whose value contains NUL is
/// silently skipped — the same guard `locked_set_var` applies.
fn process_run_with_sync(
    cmd: &str,
    args: &[String],
    cwd: Option<&std::path::Path>,
    env_overrides: &[(String, String)],
    cap: u64,
) -> Result<ProcessRunOutput, String> {
    use std::process::{Command, Stdio};

    let mut builder = Command::new(cmd);
    builder
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(dir) = cwd {
        builder.current_dir(dir);
    }

    for (k, v) in env_overrides {
        // Guard against invalid keys/values — same policy as `locked_set_var`.
        if k.is_empty() || k.contains('=') || k.contains('\0') || v.contains('\0') {
            continue;
        }
        builder.env(k, v);
    }

    let child = builder.spawn().map_err(|e| format!("{cmd}: {e}"))?;
    let mut guard = ChildGuard(Some(child));

    let limit = cap.saturating_add(1);

    let (stdout_pipe, stderr_pipe) = {
        let c = guard
            .get_mut()
            .ok_or_else(|| format!("{cmd}: child unexpectedly reaped"))?;
        (c.stdout.take(), c.stderr.take())
    };
    let out_handle = spawn_capture_thread(stdout_pipe, limit);
    let err_handle = spawn_capture_thread(stderr_pipe, limit);

    let stdout_bytes = out_handle
        .join()
        .map_err(|_| format!("{cmd}: stdout capture thread panicked"))?
        .map_err(|e| format!("{cmd}: {e}"))?;
    let stderr_bytes = err_handle
        .join()
        .map_err(|_| format!("{cmd}: stderr capture thread panicked"))?
        .map_err(|e| format!("{cmd}: {e}"))?;

    if stdout_bytes.len() as u64 > cap {
        return Err(format!(
            "{cmd}: stdout exceeds the {cap}-byte capture ceiling \
             (raise IPE_PROCESS_OUTPUT_MAX)"
        ));
    }
    if stderr_bytes.len() as u64 > cap {
        return Err(format!(
            "{cmd}: stderr exceeds the {cap}-byte capture ceiling \
             (raise IPE_PROCESS_OUTPUT_MAX)"
        ));
    }

    let status = guard.wait().map_err(|e| format!("{cmd}: {e}"))?;

    #[allow(non_snake_case)]
    Ok(ProcessRunOutput {
        exitCode: i64::from(status.code().unwrap_or(-1)),
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
    })
}

/// `Ipe.Process.runWith` — spawn a child process with per-child cwd and env
/// overrides, capturing exit code, stdout, and stderr independently.
///
/// A non-zero exit is a NORMAL result carried in `exitCode`; only a spawn
/// failure fails the Task. Both streams are bounded by `IPE_PROCESS_OUTPUT_MAX`
/// (default 16 MiB) and drained concurrently (no pipe-deadlock). The blocking
/// spawn+wait is offloaded via `run_blocking` so a long-running child cannot
/// stall the tokio worker thread.
///
/// SECURITY: same `subprocess` capability gate as `Process.run`. Per-child
/// `cwd` and env overrides do NOT escape the jail/sandbox roots — the child
/// inherits its confined environment from the parent, and the overrides are
/// applied ON TOP of that already-confined environment.
#[must_use]
pub fn process_run_with<E: Send + From<String> + 'static>(
    cfg: ProcessRunWithCfg,
) -> IpeTask<E, ProcessRunOutput> {
    process_run_with_impl(cfg, process_output_ceiling())
}

/// `ProcessRunWithCfg` — the Ipê record `{ command, args, cwd, env }` lowered
/// to a plain Rust struct. Owned values let the closure move into `run_blocking`
/// without a lifetime on the borrow. The emitter constructs this directly
/// (same pattern as `EmailMessage` / `CacheCfg`).
///
/// Field names match the Ipê record keys verbatim (`exitCode` etc.); the
/// non_snake_case allow is per-field.
pub struct ProcessRunWithCfg {
    pub command: String,
    pub args: Vec<String>,
    /// `Nothing` → inherit the parent cwd; `Just(p)` → set the child's cwd.
    pub cwd: IpeMaybe<String>,
    /// Per-child env overrides as `(key, value)` pairs.
    pub env: Vec<(String, String)>,
}

#[must_use]
fn process_run_with_impl<E: Send + From<String> + 'static>(
    cfg: ProcessRunWithCfg,
    cap: u64,
) -> IpeTask<E, ProcessRunOutput> {
    Box::pin(async move {
        let result = run_blocking(move || {
            let cwd_path: Option<std::path::PathBuf> = match &cfg.cwd {
                IpeMaybe::Just(p) => Some(std::path::PathBuf::from(p)),
                IpeMaybe::Nothing => None,
            };
            process_run_with_sync(&cfg.command, &cfg.args, cwd_path.as_deref(), &cfg.env, cap)
        })
        .await;
        match result {
            Ok(out) => ok_res(out),
            Err(e) => IpeResult::Err(str_err(&e)),
        }
    })
}

/// Process-exit cleanup hook. `std::process::exit` (what `System.exit` lowers to)
/// bypasses Drop, so an RAII guard's destructor never runs on that path. A backend
/// driver that puts the terminal/process into a state needing restoration (the
/// Ipe.Tui driver: raw mode + alternate screen + hidden cursor + mouse reporting)
/// registers its idempotent teardown here; `system_exit` runs it BEFORE
/// `process::exit`. Mirrors Go's `System_exit` → `tuiTeardown()` → `os.Exit`.
/// A plain `fn()` keeps the boundary clean — `system` (always compiled) never
/// references the feature-gated `tui`/crossterm; the TUI provides the function.
static EXIT_HOOK: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// Register the process-exit cleanup (idempotent target; set once per process —
/// there is a single backend driver). Subsequent registrations are ignored.
pub fn register_exit_hook(f: fn()) {
    let _ = EXIT_HOOK.set(f);
}

/// Run the registered exit hook, if any. Called by `system_exit`; also safe to
/// call from a backend driver's own normal-exit path (the hook is idempotent).
pub fn run_exit_hook() {
    if let Some(f) = EXIT_HOOK.get() {
        f();
    }
}

pub fn system_exit(code: i64) -> ! {
    // Restore any driver-owned terminal/process state BEFORE exiting — Drop does
    // NOT run on std::process::exit, so without this a Ipe.Tui `System.exit` quit
    // would leave the TTY in raw mode + the alternate screen (needing `reset`).
    run_exit_hook();
    // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — this IS the `System.exit` kernel: the Ipê program requested process termination with `code` [ledger #boundary]
    std::process::exit(code as i32)
}

/// `Ipe.System.getenv key : String -> Task Error String` — the env var as a
/// Task, or `Err` when unset. Returning a `IpeTask` (not a bare `String`) is
/// required for parity: `getenv` is Task-typed in the stdlib, so a bare `String`
/// fails to type-check in any `Task.andThen`/`Task.run` position. Returning `Err`
/// on unset (rather than `Ok("")`) mirrors Go's `System_getenv` `ErrNotFound`
/// short-circuit so a chained Task fails identically on both backends. The
/// string-based error follows `system_cwd`'s convention — the generic `E` bound
/// can only build `From<String>`, so the kind is coarser than Go's typed
/// `NotFound` (shared limitation with `system_cwd`). NOTE: `getenvOr` stays a bare
/// `String` (the default plugs the missing case at the call site).
#[must_use]
pub fn system_getenv<E: Send + From<String> + 'static>(key: String) -> IpeTask<E, String> {
    Box::pin(async move {
        if let Ok(v) = read_env_var(&key) {
            ok_res(v)
        } else {
            let msg = format!("environment variable {key:?} is not set");
            IpeResult::Err(str_err(&msg))
        }
    })
}
/// `Ipe.System.getenvOr key default` — the env var, or `default` when unset.
#[must_use]
pub fn system_getenv_or(key: String, default: String) -> String {
    read_env_var(&key).unwrap_or(default)
}

/// `System.getenvInt key : String -> Task Error Int`. Unset → `Err` (Go's
/// `ErrNotFound`); set-but-not-an-int → `Err` (Go's `ErrFfi`). The string-based error
/// follows the generic-`E` convention (coarser than Go's typed kinds; shared with
/// `getenv`/`cwd`).
#[must_use]
pub fn system_getenv_int<E: Send + From<String> + 'static>(key: String) -> IpeTask<E, i64> {
    Box::pin(async move {
        let r: Result<i64, String> = match read_env_var(&key) {
            Err(_) => Err(format!("environment variable {key:?} is not set")),
            Ok(v) => v
                .trim()
                .parse::<i64>()
                // Do NOT echo the env var VALUE into the Ipê-propagated error
                // string: env vars are a primary secret store and this message
                // flows out via Task Error → Error.toString → operator logs /
                // user surface. Mirror system_getenv (key only).
                .map_err(|_| format!("env {key}: not a valid int")),
        };
        match r {
            Ok(n) => ok_res(n),
            Err(m) => IpeResult::Err(str_err(&m)),
        }
    })
}

/// `System.getenvBool key : String -> Task Error Bool`. Matches Go's truthy/falsy
/// table: `true/yes/1/on/y/t` → true; `false/no/0/off/n/f`/empty → false; unset →
/// `Err` (`NotFound`); anything else → `Err` (not-a-bool).
#[must_use]
pub fn system_getenv_bool<E: Send + From<String> + 'static>(key: String) -> IpeTask<E, bool> {
    Box::pin(async move {
        let r: Result<bool, String> = match read_env_var(&key) {
            Err(_) => Err(format!("environment variable {key:?} is not set")),
            Ok(v) => match v.trim().to_lowercase().as_str() {
                "true" | "yes" | "1" | "on" | "y" | "t" => Ok(true),
                "false" | "no" | "0" | "off" | "n" | "f" | "" => Ok(false),
                // Key only — never echo the env var VALUE (secret-store leak).
                _ => Err(format!("env {key}: not a valid bool")),
            },
        };
        match r {
            Ok(b) => ok_res(b),
            Err(m) => IpeResult::Err(str_err(&m)),
        }
    })
}

/// `System.getArg n : Int -> Task Error (Maybe String)`. Indexes the FULL arg
/// vector to match Go's `System_getArg` (`os.Args[n]` — index 0 is the program
/// name, UNLIKE `System.args` which skips it); out-of-range / negative →
/// `Ok Nothing`. Never `Err` (mirrors Go).
#[must_use]
pub fn system_get_arg<E: Send + 'static>(n: i64) -> IpeTask<E, IpeMaybe<String>> {
    Box::pin(async move {
        let out = if n < 0 {
            IpeMaybe::Nothing
        } else {
            match std::env::args().nth(n as usize) {
                Some(a) => IpeMaybe::Just(a),
                None => IpeMaybe::Nothing,
            }
        };
        ok_res(out)
    })
}

#[must_use]
pub fn system_setenv<E: Send + 'static>(key: String, val: String) -> IpeTask<E, ()> {
    Box::pin(async move {
        locked_set_var(&key, &val);
        ok_res(())
    })
}

#[must_use]
pub fn system_unsetenv<E: Send + 'static>(key: String) -> IpeTask<E, ()> {
    Box::pin(async move {
        locked_remove_var(&key);
        ok_res(())
    })
}

/// `System.cwd : () -> Task Error String`.
#[must_use]
pub fn system_cwd<E: Send + From<String> + 'static>(_: ()) -> IpeTask<E, String> {
    Box::pin(async move {
        match std::env::current_dir() {
            Ok(p) => ok_res(p.to_string_lossy().into_owned()),
            Err(e) => IpeResult::Err(str_err(&format!("{e}"))),
        }
    })
}

/// `System.getcwd : () -> Task Error String` — backward-compat alias for `cwd`.
/// Go: `func System_getcwd(unit any) any { return System_cwd(unit) }`.
#[must_use]
pub fn system_getcwd<E: Send + From<String> + 'static>(unit: ()) -> IpeTask<E, String> {
    system_cwd(unit)
}

/// Blocking half of `system_load_env`: read + parse `.env` in the CWD and set
/// each var. Never fails — a missing/unreadable `.env` is silently a no-op,
/// matching the Ipê-facing contract (`loadEnv` never returns `Err`).
fn system_load_env_sync() {
    if let Ok(contents) = std::fs::read_to_string(".env") {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = v.trim().trim_matches('"').trim_matches('\'');
                // Atomic check-and-set under one write lock — avoids the
                // TOCTOU window a separate read + set would open against a
                // concurrent mutator.
                locked_set_var_if_absent(k, v);
            }
        }
    }
}

/// `System.loadEnv : () -> Task Error ()`. Parses a `.env` file in the CWD
/// (KEY=VALUE per line, `#` comments, optional surrounding quotes) and sets
/// each var WITHOUT overriding one already present in the process environment
/// (process env wins, matching Ipê's precedence). A missing `.env` is a no-op
/// success.
///
/// `std::fs::read_to_string(".env")` is a blocking syscall, so it routes
/// through the `run_blocking` helper this module defines (above, for
/// `process_run`) rather than running inline inside the `async move` body —
/// the same offload `file.rs`/`compression.rs`/`csv.rs`/`config_decode.rs`
/// use. Real-world impact is low (`.env` is small and read once at startup),
/// but on a slow/network filesystem an inline read would stall the tokio
/// worker thread polling this future.
#[must_use]
pub fn system_load_env<E: Send + 'static>(_: ()) -> IpeTask<E, ()> {
    Box::pin(async move {
        // `run_blocking`'s `Err` arm (the blocking task panicked) is folded
        // back into `Ok(())` here — `loadEnv` never surfaces an `Err` for a
        // missing/unreadable `.env`, and a panicked blocking task shouldn't
        // change that contract either.
        let _: Result<(), String> = run_blocking(|| {
            system_load_env_sync();
            Ok(())
        })
        .await;
        ok_res(())
    })
}

#[cfg(test)]
mod exit_hook_tests {
    use super::{register_exit_hook, run_exit_hook};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn bump() {
        CALLS.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn exit_hook_runs_and_is_safe_without_registration() {
        // No hook registered yet → run_exit_hook must be a safe no-op (the common
        // CLI / server / non-TUI case — System.exit must not require a hook).
        run_exit_hook();
        // Register one and confirm it runs (the Ipe.Tui driver registers its
        // terminal-restore here so a System.exit quit doesn't bypass cleanup).
        register_exit_hook(bump);
        run_exit_hook();
        assert!(
            CALLS.load(Ordering::SeqCst) >= 1,
            "registered exit hook must run"
        );
    }
}

#[cfg(test)]
mod process_run_tests {
    use super::*;

    fn block<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    /// Functional correctness (independent of whether `run_blocking` takes the
    /// real `spawn_blocking` path or the no-tokio-feature fallback — both
    /// paths must return the same result).
    #[test]
    fn success_returns_combined_output() {
        let res: IpeResult<String, String> = block(process_run::<String>(
            "echo".to_string(),
            vec!["hello".to_string()],
        ));
        match res {
            IpeResult::Ok(s) => assert!(s.contains("hello"), "unexpected output: {s:?}"),
            IpeResult::Err(e) => panic!("unexpected Err: {e}"),
        }
    }

    #[test]
    fn nonexistent_binary_errs() {
        let res: IpeResult<String, String> = block(process_run::<String>(
            "ipe-does-not-exist-binary-xyz".to_string(),
            vec![],
        ));
        assert!(matches!(res, IpeResult::Err(_)));
    }

    #[test]
    fn nonzero_exit_errs() {
        let res: IpeResult<String, String> =
            block(process_run::<String>("false".to_string(), vec![]));
        assert!(matches!(res, IpeResult::Err(_)));
    }

    /// No-shell proof: an argument containing shell metacharacters is passed
    /// literally as an argv element, never evaluated by `sh -c`. `printf %s`
    /// echoes it verbatim; a shell would have run the `; touch <marker>` clause
    /// (creating the file) and would NOT echo the clause back verbatim.
    #[test]
    fn args_are_literal_no_shell_interpretation() {
        let marker = std::env::temp_dir().join(format!("ipe_noshell_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let payload = format!("; touch {} ; echo pwned", marker.display());
        let res: IpeResult<String, String> = block(process_run::<String>(
            "printf".to_string(),
            vec!["%s".to_string(), payload.clone()],
        ));
        let marker_created = marker.exists();
        let _ = std::fs::remove_file(&marker);
        match res {
            IpeResult::Ok(s) => {
                // The whole payload is echoed back verbatim (one argv element),
                // proving no `sh -c` split it on `;`.
                assert_eq!(s, payload, "argv must be passed literally (no shell)");
            }
            IpeResult::Err(e) => panic!("unexpected Err: {e}"),
        }
        assert!(
            !marker_created,
            "the `; touch` clause ran — argv was evaluated by a shell (injection)"
        );
    }

    /// Deadlock regression: a child that writes a LOT to BOTH stdout and stderr
    /// (each well past a 64 KiB pipe buffer) must complete, not wedge. The
    /// sequential stdout-then-stderr drain would deadlock here — the child
    /// blocks on a full stderr pipe while we drain stdout, and vice versa. The
    /// concurrent per-stream capture threads make this terminate.
    #[test]
    fn large_stdout_and_stderr_does_not_deadlock() {
        // `sh` is the program under test (invoked as an argv vector, not via
        // this kernel's own shell — there is none): it writes ~512 KiB to each
        // stream, far exceeding the ~64 KiB kernel pipe buffer.
        let script = "yes ABCDEFGH | head -c 524288; yes abcdefgh | head -c 524288 >&2";
        let res: IpeResult<String, String> = block(process_run::<String>(
            "sh".to_string(),
            vec!["-c".to_string(), script.to_string()],
        ));
        match res {
            IpeResult::Ok(s) => assert_eq!(
                s.len(),
                524288 * 2,
                "combined output must be both streams in full"
            ),
            IpeResult::Err(e) => panic!("large dual-stream output must not deadlock/err: {e}"),
        }
    }

    /// DoS guard: a subprocess whose combined output exceeds the capture
    /// ceiling must `Err`, never buffer it all and OOM the host, and never
    /// silently truncate a returned success value.
    #[test]
    fn output_over_ceiling_errs() {
        // The ceiling is passed explicitly (not via a process-global env var),
        // so this runs safely in parallel with any other subprocess test.
        let res: IpeResult<String, String> = block(process_run_with_cap::<String>(
            "printf".to_string(),
            vec!["%s".to_string(), "x".repeat(64)],
            8,
        ));
        assert!(
            matches!(res, IpeResult::Err(_)),
            "64 bytes of output under an 8-byte ceiling must Err, not OOM/truncate"
        );
    }
}

#[cfg(test)]
mod process_run_with_tests {
    use super::*;

    fn block<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    fn cfg(command: &str, args: &[&str]) -> ProcessRunWithCfg {
        ProcessRunWithCfg {
            command: command.to_owned(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: IpeMaybe::Nothing,
            env: Vec::new(),
        }
    }

    /// Non-zero exit is carried in `exitCode` — the Task succeeds.
    #[test]
    fn nonzero_exit_is_normal_result_not_task_failure() {
        let res: IpeResult<String, ProcessRunOutput> =
            block(process_run_with::<String>(cfg("false", &[])));
        match res {
            IpeResult::Ok(out) => {
                assert_ne!(out.exitCode, 0, "false must exit non-zero");
            }
            IpeResult::Err(e) => panic!("spawn failure not expected: {e}"),
        }
    }

    /// Successful command: exit 0, stdout captured.
    #[test]
    fn success_captures_stdout_and_exit_zero() {
        let res: IpeResult<String, ProcessRunOutput> =
            block(process_run_with::<String>(cfg("echo", &["hello"])));
        match res {
            IpeResult::Ok(out) => {
                assert_eq!(out.exitCode, 0);
                assert!(
                    out.stdout.contains("hello"),
                    "expected stdout: {:?}",
                    out.stdout
                );
                assert!(out.stderr.is_empty(), "unexpected stderr: {:?}", out.stderr);
            }
            IpeResult::Err(e) => panic!("unexpected error: {e}"),
        }
    }

    /// stderr is captured separately from stdout.
    #[test]
    fn stderr_captured_separately() {
        let res: IpeResult<String, ProcessRunOutput> = block(process_run_with::<String>(cfg(
            "sh",
            &["-c", "echo err >&2"],
        )));
        match res {
            IpeResult::Ok(out) => {
                assert!(out.stdout.is_empty(), "unexpected stdout: {:?}", out.stdout);
                assert!(
                    out.stderr.contains("err"),
                    "expected stderr: {:?}",
                    out.stderr
                );
            }
            IpeResult::Err(e) => panic!("unexpected error: {e}"),
        }
    }

    /// Spawn failure (non-existent binary) → Task.fail.
    #[test]
    fn nonexistent_binary_fails_task() {
        let res: IpeResult<String, ProcessRunOutput> = block(process_run_with::<String>(cfg(
            "ipe-does-not-exist-xyz",
            &[],
        )));
        assert!(matches!(res, IpeResult::Err(_)));
    }

    /// cwd override is honoured: `pwd` must echo the target directory.
    #[test]
    fn cwd_override_is_honoured() {
        let tmp = std::env::temp_dir();
        let tmp_str = tmp.to_string_lossy().into_owned();
        let mut c = cfg("sh", &["-c", "pwd"]);
        c.cwd = IpeMaybe::Just(tmp_str.clone());
        let res: IpeResult<String, ProcessRunOutput> = block(process_run_with::<String>(c));
        match res {
            IpeResult::Ok(out) => {
                let canonical_tmp = std::fs::canonicalize(&tmp)
                    .unwrap_or(tmp.clone())
                    .to_string_lossy()
                    .into_owned();
                let got = out.stdout.trim().to_owned();
                assert!(
                    got == canonical_tmp || got == tmp_str,
                    "pwd must report the overridden cwd; got {got:?}, expected {canonical_tmp:?}"
                );
            }
            IpeResult::Err(e) => panic!("unexpected error: {e}"),
        }
    }

    /// env override is passed to the child; parent env is also inherited.
    #[test]
    fn env_override_is_passed_to_child() {
        let marker = format!("ipe_run_with_probe_{}", std::process::id());
        let mut c = cfg("sh", &["-c", "echo $IPE_RUN_WITH_TEST_VAR"]);
        c.env = vec![("IPE_RUN_WITH_TEST_VAR".to_owned(), marker.clone())];
        let res: IpeResult<String, ProcessRunOutput> = block(process_run_with::<String>(c));
        match res {
            IpeResult::Ok(out) => {
                assert!(
                    out.stdout.contains(&marker),
                    "env override must be visible to child; got {:?}",
                    out.stdout
                );
            }
            IpeResult::Err(e) => panic!("unexpected error: {e}"),
        }
    }

    /// The ceiling applies per-stream; a stream that exceeds it fails the Task.
    #[test]
    fn per_stream_ceiling_is_enforced() {
        let c = ProcessRunWithCfg {
            command: "printf".to_owned(),
            args: vec!["%s".to_owned(), "x".repeat(64)],
            cwd: IpeMaybe::Nothing,
            env: Vec::new(),
        };
        // Swap command for a ceiling test via the internal cap-threaded helper.
        let _ = c; // used below via process_run_with_impl directly
        let res: IpeResult<String, ProcessRunOutput> = block(process_run_with_impl::<String>(
            ProcessRunWithCfg {
                command: "printf".to_owned(),
                args: vec!["%s".to_owned(), "x".repeat(64)],
                cwd: IpeMaybe::Nothing,
                env: Vec::new(),
            },
            8,
        ));
        assert!(
            matches!(res, IpeResult::Err(_)),
            "64-byte output under an 8-byte ceiling must fail"
        );
    }

    /// No-shell guard: a shell-metacharacter argument is passed verbatim.
    #[test]
    fn args_passed_literally_no_shell() {
        let payload = "; echo pwned".to_owned();
        let res: IpeResult<String, ProcessRunOutput> =
            block(process_run_with::<String>(cfg("printf", &["%s", &payload])));
        match res {
            IpeResult::Ok(out) => {
                assert_eq!(
                    out.stdout, payload,
                    "argv must be literal, not shell-interpreted"
                );
            }
            IpeResult::Err(e) => panic!("unexpected error: {e}"),
        }
    }
}

#[cfg(all(test, feature = "tokio"))]
mod process_run_spawn_blocking_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Reactor-starvation guard: `Command::output()` blocks the calling thread until
    /// the child process exits. On a SINGLE-WORKER (current_thread) runtime,
    /// running that wait inline (no `spawn_blocking`) would starve every
    /// other task scheduled on that runtime for the subprocess's whole
    /// lifetime. This proves `process_run` offloads the wait to tokio's
    /// blocking-thread pool: a concurrently-spawned cheap ticker task must
    /// make progress (ticks > 0) WHILE the subprocess is running.
    ///
    /// Uses `sleep 1` as a cheap, portable way to force the subprocess to run
    /// long enough for at least one `yield_now` to land elsewhere. Pre-fix
    /// this is NOT a flaky race: the ticker makes EXACTLY zero progress
    /// deterministically, because the worker thread never yields back to the
    /// executor until `Command::output()` returns.
    #[test]
    fn process_run_does_not_starve_concurrent_async_work() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let ticks = rt.block_on(async move {
            let counter = Arc::new(AtomicU64::new(0));
            let counter2 = counter.clone();
            let ticker = tokio::spawn(async move {
                loop {
                    counter2.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            });
            let run_fut: IpeTask<String, String> =
                process_run("sleep".to_string(), vec!["1".to_string()]);
            let _res: IpeResult<String, String> = run_fut.await;
            ticker.abort();
            counter.load(Ordering::Relaxed)
        });

        assert!(
            ticks > 0,
            "concurrent ticker task made ZERO progress while process_run ran — \
             the blocking subprocess wait is starving the single-threaded executor \
             (spawn_blocking missing or not taking effect)"
        );
    }
}

#[cfg(all(test, feature = "tokio"))]
mod system_load_env_spawn_blocking_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Reactor-starvation guard: `system_load_env` reads `.env` via
    /// `std::fs::read_to_string`, a blocking syscall. It must route through the
    /// shared `run_blocking` helper (defined above in this file, already used
    /// by `process_run`) rather than run inline inside the `async move` body —
    /// the same offload `file.rs` / `compression.rs` / `csv.rs` /
    /// `config_decode.rs` use. This proves `system_load_env` offloads the read
    /// to tokio's blocking-thread pool: a concurrently-
    /// spawned cheap ticker task must make progress (ticks > 0) WHILE the
    /// read is in flight.
    ///
    /// Uses a large `.env` (64 MiB of comment padding, same idiom as
    /// `file.rs`'s `spawn_blocking_tests`) so the read takes measurable wall
    /// time. Pre-fix this is NOT a flaky race — the ticker makes EXACTLY
    /// zero progress deterministically, because the worker thread never
    /// yields back to the executor until `read_to_string` returns.
    ///
    /// `set_current_dir` mutates process-global state; safe here only
    /// because this crate's tests run one-process-per-test under `cargo
    /// nextest` (the codebase's existing convention for tests that mutate
    /// global process state — e.g. this same file's `exit_hook_tests` /
    /// `console.rs`'s `ingest_token_gate` mutate env vars directly for the
    /// same reason).
    #[test]
    fn system_load_env_does_not_starve_concurrent_async_work() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ipe_load_env_spawn_blocking_probe_{}_{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        // One huge comment line (skipped by the parser) + one real var —
        // large enough that the read takes measurable (not instant) wall
        // time, same idiom as `file.rs`'s spawn_blocking probe.
        let mut contents = String::from("# ");
        contents.push_str(&"x".repeat(64 * 1024 * 1024));
        contents.push('\n');
        contents.push_str("IPE_LOAD_ENV_PROBE_VAR=probe_value\n");
        std::fs::write(&env_path, contents).unwrap();

        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        locked_remove_var("IPE_LOAD_ENV_PROBE_VAR");

        let ticks = rt.block_on(async move {
            let counter = Arc::new(AtomicU64::new(0));
            let counter2 = counter.clone();
            let ticker = tokio::spawn(async move {
                loop {
                    counter2.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            });
            let load_fut: IpeTask<String, ()> = system_load_env(());
            let _res: IpeResult<String, ()> = load_fut.await;
            ticker.abort();
            counter.load(Ordering::Relaxed)
        });

        // Functional sanity: the real var was actually picked up.
        let loaded = read_env_var("IPE_LOAD_ENV_PROBE_VAR");

        std::env::set_current_dir(&orig_cwd).unwrap();
        locked_remove_var("IPE_LOAD_ENV_PROBE_VAR");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            loaded.as_deref(),
            Ok("probe_value"),
            "system_load_env did not set the var from the probe .env file"
        );
        assert!(
            ticks > 0,
            "concurrent ticker task made ZERO progress while system_load_env ran — \
             the blocking .env read is starving the single-threaded executor \
             (spawn_blocking missing or not taking effect)"
        );
    }
}

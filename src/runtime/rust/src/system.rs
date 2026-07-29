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
#[cfg(feature = "tokio")]
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

fn process_run_sync(cmd: &str, args: &[String]) -> Result<std::process::Output, String> {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("{cmd}: {e}"))
}

/// `Ipe.Process.run : String -> List String -> Task Error String` — run a
/// subprocess, returning its combined stdout+stderr. Mirrors Go's `Process_run`
/// (`exec.Command` + `CombinedOutput`): a non-zero exit or a spawn failure is
/// `Err` carrying the captured output + the error; a clean exit is `Ok(output)`.
/// Total — every failure maps to `Err`, never a panic.
///
/// SECURITY: `Process.run` is an intentional Ipê stdlib effect (Task-tier,
/// parity with the Go backend) — no more permissive than Go's. Sandboxing
/// untrusted Ipê source (e.g. blocking the `Process.` module) is the calling
/// application's responsibility, exactly as on Go.
///
/// The actual `Command::output()` wait is offloaded via `run_blocking`
/// (see the module-level doc comment above) so a long-running subprocess
/// can't stall the tokio worker thread polling this future.
#[must_use]
pub fn process_run<E: Send + From<String> + 'static>(
    cmd: String,
    args: Vec<String>,
) -> IpeTask<E, String> {
    Box::pin(async move {
        // `process_run_sync` already folds `cmd` into its `Err` string (spawn
        // failure), so the outer `Err` arm (a `run_blocking` `JoinError`,
        // i.e. the blocking task panicked) doesn't need `cmd` — it's moved
        // into the closure below.
        match run_blocking(move || process_run_sync(&cmd, &args)).await {
            Ok(out) => {
                // Go's CombinedOutput: stdout then stderr (callers usually `2>&1`).
                let mut combined = out.stdout;
                combined.extend_from_slice(&out.stderr);
                let text = String::from_utf8_lossy(&combined).into_owned();
                if out.status.success() {
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

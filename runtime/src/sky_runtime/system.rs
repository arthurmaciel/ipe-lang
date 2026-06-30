// System helpers — some generic over E (when returning SkyTask).
use super::*;

// `std::env::set_var`/`remove_var` are documented as NOT thread-safe: a mutator
// can reallocate the C `environ` block while another thread READS it
// (`std::env::var` walks `environ`), which is a data race / use-after-free by the
// std + POSIX contract — not just a mutator↔mutator hazard. Both are reachable
// from Sky purely through env-Task composition under `Task.parallel`
// (`System.setenv`/`unsetenv`/`loadEnv` are mutators; `System.getenv*` are
// readers). Serialise BOTH sides behind one process-global RwLock: mutators take
// the write lock (exclusive), readers take the read lock (shared with each other,
// excluded against any mutator). This closes the reader↔mutator race for every
// Sky-originated access. (A non-Sky dependency reading `environ` without this lock
// is outside our reach — but every Sky path is now serialised.)
static ENV_LOCK: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// Read an environment variable under the shared env read lock (excluded against
/// any concurrent mutator so the `environ` walk can't race a realloc).
fn read_env_var(key: &str) -> Result<String, std::env::VarError> {
    let _guard = ENV_LOCK.read().unwrap_or_else(|p| p.into_inner());
    std::env::var(key)
}

/// Set an environment variable under the exclusive env write lock.
fn locked_set_var(key: &str, val: &str) {
    // std::env::set_var PANICS on an empty key, a key containing '=' or NUL, or a
    // value containing NUL. Skip such a key/value (no-op) rather than panic.
    if key.is_empty() || key.contains('=') || key.contains('\0') || val.contains('\0') {
        return;
    }
    let _guard = ENV_LOCK.write().unwrap_or_else(|p| p.into_inner());
    std::env::set_var(key, val);
}

/// Remove an environment variable under the exclusive env write lock.
fn locked_remove_var(key: &str) {
    // std::env::remove_var panics on the same invalid keys as set_var — guard it.
    if key.is_empty() || key.contains('=') || key.contains('\0') {
        return;
    }
    let _guard = ENV_LOCK.write().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var(key);
}

pub fn system_args<E: Send + 'static>(_: ()) -> SkyTask<E, Vec<String>> {
    Box::pin(async move { ok_res(std::env::args().skip(1).collect()) })
}

/// `Sky.Core.Process.run : String -> List String -> Task Error String` — run a
/// subprocess, returning its combined stdout+stderr. Mirrors Go's `Process_run`
/// (`exec.Command` + `CombinedOutput`): a non-zero exit or a spawn failure is
/// `Err` carrying the captured output + the error; a clean exit is `Ok(output)`.
/// Total — every failure maps to `Err`, never a panic.
///
/// SECURITY: `Process.run` is an intentional Sky stdlib effect (Task-tier,
/// parity with the Go backend) — no more permissive than Go's. Sandboxing
/// untrusted Sky source (e.g. blocking the `Process.` module) is the calling
/// application's responsibility, exactly as on Go.
pub fn process_run<E: Send + From<String> + 'static>(
    cmd: String,
    args: Vec<String>,
) -> SkyTask<E, String> {
    Box::pin(async move {
        match std::process::Command::new(&cmd).args(&args).output() {
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
                    SkyResult::Err(str_err(&format!("{}: {}", snippet, out.status)))
                }
            }
            Err(e) => SkyResult::Err(str_err(&format!("{}: {}", cmd, e))),
        }
    })
}
/// Process-exit cleanup hook. `std::process::exit` (what `System.exit` lowers to)
/// bypasses Drop, so an RAII guard's destructor never runs on that path. A backend
/// driver that puts the terminal/process into a state needing restoration (the
/// Sky.Tui driver: raw mode + alternate screen + hidden cursor + mouse reporting)
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
    // NOT run on std::process::exit, so without this a Sky.Tui `System.exit` quit
    // would leave the TTY in raw mode + the alternate screen (needing `reset`).
    run_exit_hook();
    std::process::exit(code as i32)
}

/// `Sky.Core.System.getenv key : String -> Task Error String` — the env var as a
/// Task, or `Err` when unset. Returning a `SkyTask` (not a bare `String`) is
/// required for parity: `getenv` is Task-typed in the stdlib, so a bare `String`
/// fails to type-check in any `Task.andThen`/`Task.run` position. Returning `Err`
/// on unset (rather than `Ok("")`) mirrors Go's `System_getenv` ErrNotFound
/// short-circuit so a chained Task fails identically on both backends. The
/// string-based error follows `system_cwd`'s convention — the generic `E` bound
/// can only build `From<String>`, so the kind is coarser than Go's typed
/// NotFound (shared limitation with `system_cwd`). NOTE: `getenvOr` stays a bare
/// `String` (the default plugs the missing case at the call site).
pub fn system_getenv<E: Send + From<String> + 'static>(key: String) -> SkyTask<E, String> {
    Box::pin(async move {
        match read_env_var(&key) {
            Ok(v) => ok_res(v),
            Err(_) => {
                let msg = format!("environment variable {:?} is not set", key);
                SkyResult::Err(str_err(&msg))
            }
        }
    })
}
/// `Sky.Core.System.getenvOr key default` — the env var, or `default` when unset.
pub fn system_getenv_or(key: String, default: String) -> String {
    read_env_var(&key).unwrap_or(default)
}

/// `System.getenvInt key : String -> Task Error Int`. Unset → `Err` (Go's
/// ErrNotFound); set-but-not-an-int → `Err` (Go's ErrFfi). The string-based error
/// follows the generic-`E` convention (coarser than Go's typed kinds; shared with
/// `getenv`/`cwd`).
pub fn system_getenv_int<E: Send + From<String> + 'static>(key: String) -> SkyTask<E, i64> {
    Box::pin(async move {
        let r: Result<i64, String> = match read_env_var(&key) {
            Err(_) => Err(format!("environment variable {:?} is not set", key)),
            Ok(v) => v
                .trim()
                .parse::<i64>()
                // Do NOT echo the env var VALUE into the Sky-propagated error
                // string: env vars are a primary secret store and this message
                // flows out via Task Error → Error.toString → operator logs /
                // user surface. Mirror system_getenv (key only).
                .map_err(|_| format!("env {}: not a valid int", key)),
        };
        match r {
            Ok(n) => ok_res(n),
            Err(m) => SkyResult::Err(str_err(&m)),
        }
    })
}

/// `System.getenvBool key : String -> Task Error Bool`. Matches Go's truthy/falsy
/// table: `true/yes/1/on/y/t` → true; `false/no/0/off/n/f`/empty → false; unset →
/// `Err` (NotFound); anything else → `Err` (not-a-bool).
pub fn system_getenv_bool<E: Send + From<String> + 'static>(key: String) -> SkyTask<E, bool> {
    Box::pin(async move {
        let r: Result<bool, String> = match read_env_var(&key) {
            Err(_) => Err(format!("environment variable {:?} is not set", key)),
            Ok(v) => match v.trim().to_lowercase().as_str() {
                "true" | "yes" | "1" | "on" | "y" | "t" => Ok(true),
                "false" | "no" | "0" | "off" | "n" | "f" | "" => Ok(false),
                // Key only — never echo the env var VALUE (secret-store leak).
                _ => Err(format!("env {}: not a valid bool", key)),
            },
        };
        match r {
            Ok(b) => ok_res(b),
            Err(m) => SkyResult::Err(str_err(&m)),
        }
    })
}

/// `System.getArg n : Int -> Task Error (Maybe String)`. Indexes the FULL arg
/// vector to match Go's `System_getArg` (`os.Args[n]` — index 0 is the program
/// name, UNLIKE `System.args` which skips it); out-of-range / negative →
/// `Ok Nothing`. Never `Err` (mirrors Go).
pub fn system_get_arg<E: Send + 'static>(n: i64) -> SkyTask<E, SkyMaybe<String>> {
    Box::pin(async move {
        let out = if n < 0 {
            SkyMaybe::Nothing
        } else {
            match std::env::args().nth(n as usize) {
                Some(a) => SkyMaybe::Just(a),
                None => SkyMaybe::Nothing,
            }
        };
        ok_res(out)
    })
}

pub fn system_setenv<E: Send + 'static>(key: String, val: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        locked_set_var(&key, &val);
        ok_res(())
    })
}

pub fn system_unsetenv<E: Send + 'static>(key: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        locked_remove_var(&key);
        ok_res(())
    })
}

/// `System.cwd : () -> Task Error String`.
pub fn system_cwd<E: Send + From<String> + 'static>(_: ()) -> SkyTask<E, String> {
    Box::pin(async move {
        match std::env::current_dir() {
            Ok(p) => ok_res(p.to_string_lossy().into_owned()),
            Err(e) => SkyResult::Err(str_err(&format!("{}", e))),
        }
    })
}

/// `System.getcwd : () -> Task Error String` — backward-compat alias for `cwd`.
/// Go: `func System_getcwd(unit any) any { return System_cwd(unit) }`.
pub fn system_getcwd<E: Send + From<String> + 'static>(unit: ()) -> SkyTask<E, String> {
    system_cwd(unit)
}

/// `System.loadEnv : () -> Task Error ()`. Parses a `.env` file in the CWD
/// (KEY=VALUE per line, `#` comments, optional surrounding quotes) and sets
/// each var WITHOUT overriding one already present in the process environment
/// (process env wins, matching Sky's precedence). A missing `.env` is a no-op
/// success.
pub fn system_load_env<E: Send + 'static>(_: ()) -> SkyTask<E, ()> {
    Box::pin(async move {
        if let Ok(contents) = std::fs::read_to_string(".env") {
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    let k = k.trim();
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    if read_env_var(k).is_err() {
                        locked_set_var(k, v);
                    }
                }
            }
        }
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
        // Register one and confirm it runs (the Sky.Tui driver registers its
        // terminal-restore here so a System.exit quit doesn't bypass cleanup).
        register_exit_hook(bump);
        run_exit_hook();
        assert!(
            CALLS.load(Ordering::SeqCst) >= 1,
            "registered exit hook must run"
        );
    }
}

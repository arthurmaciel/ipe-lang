//! Build + run plumbing for the golden E2E gate.
//!
//! Provides the core routines the golden test harness and E2E tests share:
//! build the emitted Rust project, locate the produced binary, and run it.
//! There is no Go-oracle format here — the expected output is captured once
//! (as `tests/golden/<name>/expected.txt`) and compared directly by the test.
//!
//! Two entry points:
//!   * [`build_and_run_rust`] — build + run; returns stdout + exit code.
//!   * [`build_rust_binary`]  — build only; returns the binary path.

#![forbid(unsafe_code)]

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The expected-output file name inside a golden directory.
pub const EXPECTED_FILE: &str = "expected.txt";
/// The Ipê entry point inside every golden directory.
pub const MAIN_IPE: &str = "Main.ipe";

/// Fail-fast ceiling for the emitted-crate `cargo build`, in seconds.
///
/// A hung or lock-contended build would otherwise spin to the outer nextest
/// per-test cap (300s under `--profile ci`) and read as a whole-shard straggler.
/// Every emitted app links against a warm shared dependency target, so even the
/// heaviest axum/tokio SEAL crate finishes well inside this window; a genuine
/// wedge fails here in minutes with a clear message instead. Overridable via
/// `IPE_E2E_BUILD_TIMEOUT_SECS` for a cold, deps-not-yet-warm environment.
const DEFAULT_EMITTED_BUILD_TIMEOUT_SECS: u64 = 240;

/// Resolve the emitted-build fail-fast ceiling from the environment, falling
/// back to [`DEFAULT_EMITTED_BUILD_TIMEOUT_SECS`].
///
/// A non-empty, parseable positive value wins; anything else (absent, empty,
/// non-numeric, zero) uses the default — an unreadable override must never
/// silently disable the cap.
fn emitted_build_timeout() -> Duration {
    let secs = std::env::var("IPE_E2E_BUILD_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(DEFAULT_EMITTED_BUILD_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Stable token stored in a portable golden `Cargo.toml` instead of the
/// machine-specific `ipe-runtime-rust` crate path.
///
/// The real emit writes a live, resolvable absolute path so `cargo build`
/// works in any environment; only the golden fixture stores this placeholder
/// so the byte-compare is machine-independent. The comparison and bless paths
/// both normalise the emitted path to this value before touching the golden.
pub const RUNTIME_PATH_PLACEHOLDER: &str = "__IPE_RUNTIME_PATH__";

/// Replace the `ipe-runtime-rust` dependency's `path = "<abs>"` value in a
/// `Cargo.toml` text with [`RUNTIME_PATH_PLACEHOLDER`], leaving every other
/// byte untouched.
///
/// Only the one `ipe_runtime = { … package = "ipe-runtime-rust" … path = "…"
/// … }` dependency line carries a machine-specific value; the rewrite is
/// scoped to `path = "…"` on that line, so a manifest with no such line (e.g.
/// the vendored / wasm shape) passes through unchanged, and any real manifest
/// drift still surfaces as a diff.
#[must_use]
pub fn normalize_runtime_dep_path(manifest: &str) -> String {
    manifest
        .lines()
        .map(|line| {
            if line.contains("package = \"ipe-runtime-rust\"")
                && let Some(start) = line.find("path = \"")
            {
                let val_start = start + "path = \"".len();
                if let Some(rel_end) = line[val_start..].find('"') {
                    let end = val_start + rel_end;
                    return format!(
                        "{}{}{}",
                        &line[..val_start],
                        RUNTIME_PATH_PLACEHOLDER,
                        &line[end..]
                    );
                }
            }
            line.to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + if manifest.ends_with('\n') { "\n" } else { "" }
}

/// Captured stdout + exit code from running a built program.
#[derive(Clone, Debug)]
pub struct RunResult {
    /// The program's standard output, decoded lossily from UTF-8.
    pub stdout: String,
    /// The process exit code (`None` if killed by a signal).
    pub exit_code: Option<i32>,
}

/// Turn an arbitrary golden name into a cargo-package-safe suffix.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Rewrite the emitted `Cargo.toml` so its package — and hence its binary — is
/// unique to this golden, letting every golden's binary coexist in the one
/// shared cargo target. Returns the unique package name.
///
/// The emitted package name is `ipe-app` for a single-file build but the slug
/// from `package.ipe` for a project build, so the rewrite targets the first
/// `name = "..."` line inside the `[package]` section (whatever its value)
/// rather than a fixed anchor string.
fn rewrite_package_name(emitted_dir: &Path, golden_name: &str) -> Result<String, String> {
    let manifest = emitted_dir.join("Cargo.toml");
    let original = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
    let unique = format!("ipe-app-e2e-{}", sanitize(golden_name));
    let rewritten = replace_package_name(&original, &unique).ok_or_else(|| {
        format!(
            "emitted manifest {} has no `name = \"...\"` line in its `[package]` section",
            manifest.display()
        )
    })?;
    std::fs::write(&manifest, rewritten)
        .map_err(|e| format!("cannot write {}: {e}", manifest.display()))?;
    Ok(unique)
}

/// Replace the package name in a `Cargo.toml` text with `unique`, returning the
/// rewritten text (or `None` if no `[package]` `name = "..."` line is present).
///
/// The `[package]` table is the first section of every emitted manifest, so the
/// first `name = "..."` line at or after a `[package]` header is the package
/// name; a `name = ` under `[dependencies]`/`[features]` never precedes it.
fn replace_package_name(manifest: &str, unique: &str) -> Option<String> {
    let mut in_package = false;
    let mut done = false;
    let out = manifest
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('[') {
                in_package = trimmed.starts_with("[package]");
            } else if in_package
                && !done
                && trimmed.starts_with("name")
                && let Some((lhs, _)) = line.split_once('=')
                && lhs.trim() == "name"
            {
                done = true;
                return format!("name = \"{unique}\"");
            }
            line.to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + if manifest.ends_with('\n') { "\n" } else { "" };
    done.then_some(out)
}

/// Parse `cargo build --message-format=json` stdout for the produced binary.
fn find_executable(json_stdout: &str, unique_pkg: &str) -> Option<String> {
    let mut found: Option<String> = None;
    for line in json_stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        let Some(exe) = value.get("executable").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let pkg_id = value
            .get("package_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if pkg_id.contains(unique_pkg) {
            found = Some(exe.to_owned());
        }
    }
    found
}

/// Decide the emitted-project cargo target, decoupling it from the ambient
/// `CARGO_TARGET_DIR` set by the outer build lane.
///
/// `shared` is the raw `IPE_ORACLE_SHARED_TARGET` value. Returns `Some(path)`
/// only when it is a non-empty absolute path — anything else (absent, relative,
/// whitespace) returns `None` (inherit the ambient env = isolate). This
/// fail-safe prevents a runtime-editing lane that vendors a different
/// `ipe_runtime` from accidentally reusing a stale shared target and producing
/// a false green.
fn resolve_emitted_target(shared: Option<&str>) -> Option<String> {
    let trimmed = shared?.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !Path::new(trimmed).is_absolute() {
        eprintln!(
            "e2e-support: ignoring IPE_ORACLE_SHARED_TARGET={trimmed:?} \
             (not an absolute path); isolating emitted build in ambient CARGO_TARGET_DIR"
        );
        return None;
    }
    Some(trimmed.to_owned())
}

/// The `CARGO_TARGET_DIR` a child `ipe`/`cargo` process should inherit so its
/// emitted build links against the warm shared dependency target.
///
/// A test spawning the `ipe` subprocess (`ipe run|build|watch`) forwards this on
/// the child's environment. The resolution order:
///   * `IPE_ORACLE_SHARED_TARGET`, when it is an absolute path — CI's e2e/seal
///     jobs export ONLY this variable, and production `ipe` never reads it, so
///     the harness must translate it into the child's `CARGO_TARGET_DIR` or the
///     child cold-builds the full tokio/axum/runtime tree.
///   * else the ambient `CARGO_TARGET_DIR`, when a local run set one — the child
///     inherits it untouched, so agent-lane target isolation is preserved.
///   * else `None` (nothing to forward; cargo's default per-crate target).
///
/// Returning `None` when neither is set keeps a bare local run hermetic and
/// unchanged; a non-absolute `IPE_ORACLE_SHARED_TARGET` fails safe exactly as
/// [`resolve_emitted_target`] does (isolate rather than reuse a stale target).
#[must_use]
pub fn child_shared_target(
    shared: Option<&str>,
    ambient_cargo_target: Option<&str>,
) -> Option<String> {
    resolve_emitted_target(shared).or_else(|| {
        ambient_cargo_target
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

/// Resolve [`child_shared_target`] from the current process environment.
///
/// Convenience wrapper reading `IPE_ORACLE_SHARED_TARGET` and `CARGO_TARGET_DIR`
/// from the ambient environment for the common call site that forwards the warm
/// target onto a spawned `ipe`/`cargo` child.
#[must_use]
pub fn child_shared_target_from_env() -> Option<String> {
    let shared = std::env::var("IPE_ORACLE_SHARED_TARGET").ok();
    let ambient = std::env::var("CARGO_TARGET_DIR").ok();
    child_shared_target(shared.as_deref(), ambient.as_deref())
}

/// Build the emitted project at `emitted_dir` for `golden_name`, returning the
/// located binary path. The shared core of [`build_and_run_rust`] and
/// [`build_rust_binary`], so both drive `cargo build` identically.
///
/// The emitted build's cargo target is chosen by [`resolve_emitted_target`] from
/// `IPE_ORACLE_SHARED_TARGET`: when the harness opts in with an absolute path the
/// build is pinned to that shared target (runtime deps compiled once, reused);
/// otherwise the ambient env is inherited untouched (isolate — the fail-safe
/// default).
///
/// The compiler wrapper (sccache) is disabled for this build. Each emitted crate
/// lives in a per-golden scratch directory the golden removes on its next run, so
/// pinning rustc to a cwd-sensitive shared sccache server is unsound under
/// parallelism: one golden's scratch teardown unlinks the very cwd the shared
/// sccache server inherited, after which every sibling compile fails
/// `sccache rustc -vV` with "couldn't determine current working directory".
/// Running the emitted builds without the wrapper removes that shared, racy
/// resource; the shared cargo target already caches the heavy runtime dep tree.
///
/// An EMPTY `CARGO_BUILD_RUSTC_WRAPPER` (not `env_remove`) is required: the
/// wrapper is commonly configured in `~/.cargo/config.toml`'s `[build]
/// rustc-wrapper`, which `env_remove` cannot override — only an empty env var,
/// which takes precedence over the config value, actually disables it.
fn build_emitted_binary(golden_name: &str, emitted_dir: &Path) -> Result<String, String> {
    let unique_pkg = rewrite_package_name(emitted_dir, golden_name)?;

    let shared = std::env::var("IPE_ORACLE_SHARED_TARGET").ok();
    let mut cmd = Command::new("cargo");
    // `--offline` prevents live crates.io index updates, so resolution uses
    // only the already-fetched registry cache — the same versions the workspace
    // build resolved. Without this flag cargo re-queries the index on every run
    // and may resolve newer transitive versions whose APIs break the emitted
    // crate (a hermeticity gap, not a codegen SEAL breach).
    cmd.arg("build")
        .arg("--offline")
        .arg("--message-format=json")
        .current_dir(emitted_dir)
        .env("CARGO_BUILD_RUSTC_WRAPPER", "")
        .env("RUSTC_WRAPPER", "");
    if let Some(p) = resolve_emitted_target(shared.as_deref()) {
        cmd.env("CARGO_TARGET_DIR", p);
    }
    let build = run_bounded_build(cmd, golden_name, emitted_build_timeout())?;
    if !build.status.success() {
        return Err(format!(
            "{golden_name}: emitted project must build\n--- cargo stderr ---\n{}",
            String::from_utf8_lossy(&build.stderr)
        ));
    }

    let json_stdout = String::from_utf8_lossy(&build.stdout);
    find_executable(&json_stdout, &unique_pkg).ok_or_else(|| {
        format!("{golden_name}: no `executable` artifact for package `{unique_pkg}` in cargo JSON")
    })
}

/// The captured result of a bounded `cargo build`: its exit status and the
/// stdout / stderr streams drained from the child.
#[derive(Debug)]
struct BuildCapture {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Spawn `cmd`, draining stdout/stderr on reader threads, and wait for it to
/// finish within `timeout`. On timeout the child is killed and an `Err` is
/// returned naming the ceiling, so a wedged or lock-contended emitted build
/// fails fast here instead of spinning to the outer nextest per-test cap.
///
/// The streams are drained by dedicated threads because cargo's
/// `--message-format=json` stdout can exceed the OS pipe buffer; polling
/// `try_wait` while the child blocks on a full pipe would otherwise deadlock.
fn run_bounded_build(
    mut cmd: Command,
    golden_name: &str,
    timeout: Duration,
) -> Result<BuildCapture, String> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{golden_name}: failed to spawn `cargo build`: {e}"))?;

    let stdout_reader = child.stdout.take().map(drain_stream);
    let stderr_reader = child.stderr.take().map(drain_stream);

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "{golden_name}: emitted `cargo build` exceeded {}s and was killed \
                         (fail-fast cap; raise IPE_E2E_BUILD_TIMEOUT_SECS for a cold environment)",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{golden_name}: waiting on `cargo build` failed: {e}"
                ));
            }
        }
    };

    let stdout = stdout_reader.map(join_stream).unwrap_or_default();
    let stderr = stderr_reader.map(join_stream).unwrap_or_default();
    Ok(BuildCapture {
        status,
        stdout,
        stderr,
    })
}

/// Spawn a thread that reads a child stream to EOF, returning its join handle.
fn drain_stream<R: Read + Send + 'static>(mut stream: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf);
        buf
    })
}

/// Join a drain thread, returning the bytes it read (empty if the thread panicked).
fn join_stream(handle: std::thread::JoinHandle<Vec<u8>>) -> Vec<u8> {
    handle.join().unwrap_or_default()
}

/// Build the emitted Rust project at `emitted_dir` and run the resulting binary,
/// returning its stdout + exit code.
///
/// # Errors
/// Returns a message if the manifest cannot be retargeted, `cargo build` fails
/// (the message carries cargo's stderr), the binary cannot be located in the
/// JSON output, or the binary cannot be executed.
pub fn build_and_run_rust(golden_name: &str, emitted_dir: &Path) -> Result<RunResult, String> {
    let exe = build_emitted_binary(golden_name, emitted_dir)?;

    let run = Command::new(&exe)
        .output()
        .map_err(|e| format!("{golden_name}: emitted binary `{exe}` must run: {e}"))?;
    Ok(RunResult {
        stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
        exit_code: run.status.code(),
    })
}

/// Build the emitted Rust project at `emitted_dir` and return the path of the
/// resulting binary WITHOUT running it.
///
/// Used by E2E tests that need to control binary launch (custom env vars,
/// pre-started fixture servers, stack-limit wrappers, …).
///
/// # Errors
/// Returns a message if the manifest cannot be retargeted, `cargo build` fails
/// (carrying cargo's stderr), or the binary cannot be located in the JSON
/// output.
pub fn build_rust_binary(golden_name: &str, emitted_dir: &Path) -> Result<String, String> {
    build_emitted_binary(golden_name, emitted_dir)
}

/// Read the expected output for a golden from its `expected.txt` file.
///
/// Returns `Ok(text)` on success, `Err` when the file is missing or unreadable
/// — a hard failure (never a skip) so a golden without an expected file cannot
/// pass silently. `expected.txt` is the self-regression anchor: captured from
/// ipe's own output and only changed when behaviour intentionally changes.
///
/// # Errors
/// Returns a human-readable message when the file is absent or cannot be read.
pub fn read_expected(golden_dir: &Path) -> Result<String, String> {
    let path = golden_dir.join(EXPECTED_FILE);
    std::fs::read_to_string(&path)
        .map_err(|e| format!("missing or unreadable {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_EMITTED_BUILD_TIMEOUT_SECS, emitted_build_timeout, replace_package_name,
        resolve_emitted_target, run_bounded_build,
    };
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn bounded_build_kills_a_hung_process_before_the_cap() {
        // A process that sleeps far longer than the tight cap stands in for a
        // wedged `cargo build`. The bounded wait must kill it and return the
        // fail-fast error well before the sleep would finish — proving the cap
        // fires, not the outer nextest terminate.
        let mut cmd = Command::new("sleep");
        cmd.arg("120");
        let started = Instant::now();
        let result = run_bounded_build(cmd, "hung_build_probe", Duration::from_millis(300));
        let elapsed = started.elapsed();

        let err = result.expect_err("a 120s sleep under a 300ms cap must be killed");
        assert!(
            err.contains("exceeded") && err.contains("fail-fast cap"),
            "the timeout error must name the fail-fast cap, got: {err}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "the cap must fire promptly (killed the child), took {elapsed:?}"
        );
    }

    #[test]
    fn bounded_build_returns_a_fast_process_output() {
        // A process that finishes inside the cap must return its captured
        // output normally — the cap only bites a genuine hang.
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("printf hello; printf oops 1>&2");
        let capture = run_bounded_build(cmd, "fast_build_probe", Duration::from_secs(30))
            .expect("a fast process must return its output");
        assert!(capture.status.success());
        assert_eq!(capture.stdout, b"hello");
        assert_eq!(capture.stderr, b"oops");
    }

    #[test]
    fn build_timeout_defaults_when_env_absent_or_invalid() {
        // The helper reads a process-global env var; assert only the default
        // path (env unset in the test harness) so the test needs no env mutation.
        // A parse guard in the helper covers empty/zero/non-numeric overrides.
        assert_eq!(
            emitted_build_timeout(),
            Duration::from_secs(DEFAULT_EMITTED_BUILD_TIMEOUT_SECS)
        );
    }

    #[test]
    fn rewrites_single_file_ipe_app_name() {
        let manifest = "[package]\nname = \"ipe-app\"\nversion = \"0.1.0\"\n\n[dependencies]\n";
        let out = replace_package_name(manifest, "uniq").expect("has a package name");
        assert!(out.contains("name = \"uniq\""));
        assert!(!out.contains("name = \"ipe-app\""));
    }

    #[test]
    fn rewrites_project_named_crate() {
        let manifest = "[package]\nname = \"connroseal\"\nedition = \"2024\"\n\n[dependencies]\n";
        let out = replace_package_name(manifest, "uniq").expect("has a package name");
        assert!(out.contains("name = \"uniq\""));
        assert!(!out.contains("name = \"connroseal\""));
    }

    #[test]
    fn leaves_non_package_name_lines_untouched() {
        // A `name = ` under a later section must not be mistaken for the package name.
        let manifest =
            "[package]\nname = \"app\"\n\n[[bin]]\nname = \"other\"\npath = \"src/main.rs\"\n";
        let out = replace_package_name(manifest, "uniq").expect("has a package name");
        assert!(out.contains("name = \"uniq\""));
        assert!(out.contains("name = \"other\""));
    }

    #[test]
    fn no_package_name_yields_none() {
        assert!(replace_package_name("[dependencies]\nfoo = \"1\"\n", "uniq").is_none());
    }

    // These lock the fail-safe semantics of `resolve_emitted_target` without
    // touching the ambient env — the function signature takes an `Option<&str>`
    // so tests pass the value directly.

    #[test]
    fn shared_absolute_path_overrides_ambient() {
        assert_eq!(
            resolve_emitted_target(Some("/home/x/.cache/ipe-lang-target")),
            Some("/home/x/.cache/ipe-lang-target".to_owned())
        );
    }

    #[test]
    fn unset_inherits_ambient_isolate() {
        assert_eq!(resolve_emitted_target(None), None);
    }

    #[test]
    fn empty_or_whitespace_fails_safe() {
        assert_eq!(resolve_emitted_target(Some("")), None);
        assert_eq!(resolve_emitted_target(Some("   ")), None);
    }

    #[test]
    fn relative_path_fails_safe() {
        assert_eq!(resolve_emitted_target(Some("relative/target")), None);
        assert_eq!(resolve_emitted_target(Some("./target")), None);
    }

    // `child_shared_target` layers the ambient-CARGO_TARGET_DIR fallback over the
    // same fail-safe resolution, so an `ipe`-subprocess site can forward the warm
    // target whether CI exports IPE_ORACLE_SHARED_TARGET or a local lane exports
    // CARGO_TARGET_DIR.
    use super::child_shared_target;

    #[test]
    fn child_prefers_absolute_shared_over_ambient() {
        assert_eq!(
            child_shared_target(Some("/warm/shared"), Some("/lane/target")),
            Some("/warm/shared".to_owned())
        );
    }

    #[test]
    fn child_falls_back_to_ambient_when_shared_absent() {
        assert_eq!(
            child_shared_target(None, Some("/lane/target")),
            Some("/lane/target".to_owned())
        );
    }

    #[test]
    fn child_falls_back_to_ambient_when_shared_non_absolute() {
        // A non-absolute shared value fails safe; a valid ambient value still wins.
        assert_eq!(
            child_shared_target(Some("relative/target"), Some("/lane/target")),
            Some("/lane/target".to_owned())
        );
    }

    #[test]
    fn child_is_none_when_neither_set() {
        assert_eq!(child_shared_target(None, None), None);
        assert_eq!(child_shared_target(Some("  "), Some("")), None);
    }

    /// The load-bearing soundness guarantee of the warm-deps/cold-app dep cache:
    /// a shared cargo target reuses only DEPENDENCY artifacts, never masking a
    /// broken app crate. Two crates build into ONE shared `CARGO_TARGET_DIR`;
    /// the first is well-typed and warms the target, the second carries a
    /// deliberate type error. cargo fingerprints each crate on its own source
    /// hash, so the second must FAIL to compile even though the target is warm —
    /// proving the SEAL cannot be greened by cache reuse.
    #[test]
    fn shared_target_never_masks_a_broken_crate() {
        use std::process::Command;

        let root = std::env::temp_dir().join(format!(
            "e2e_support_soundness_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let shared_target = root.join("shared-target");

        // Minimal, dependency-free crates so the proof is fast and hermetic —
        // the fingerprint mechanism under test is cargo's, independent of the
        // dependency set. Each gets a UNIQUE package name (the same rule the
        // real harness applies), so both coexist in the one shared target.
        let write_crate = |name: &str, main_rs: &str| -> std::path::PathBuf {
            let dir = root.join(name);
            std::fs::create_dir_all(dir.join("src")).expect("create crate dir");
            std::fs::write(
                dir.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n"
                ),
            )
            .expect("write Cargo.toml");
            std::fs::write(dir.join("src").join("main.rs"), main_rs).expect("write main.rs");
            dir
        };

        let build_into_shared = |dir: &std::path::Path| -> std::process::Output {
            Command::new("cargo")
                .arg("build")
                .current_dir(dir)
                .env("CARGO_TARGET_DIR", &shared_target)
                .env("CARGO_BUILD_RUSTC_WRAPPER", "")
                .env("RUSTC_WRAPPER", "")
                .output()
                .expect("cargo must spawn")
        };

        // 1. Well-typed crate → warms the shared target and succeeds.
        let good = write_crate("e2e_soundness_good", "fn main() { println!(\"ok\"); }");
        let good_out = build_into_shared(&good);
        assert!(
            good_out.status.success(),
            "the well-typed crate must build into the shared target\n{}",
            String::from_utf8_lossy(&good_out.stderr)
        );

        // 2. Broken crate (E0308) → the warm target MUST NOT mask it.
        let bad = write_crate(
            "e2e_soundness_bad",
            "fn main() { let _x: u32 = \"not a number\"; }",
        );
        let bad_out = build_into_shared(&bad);
        assert!(
            !bad_out.status.success(),
            "SOUNDNESS BREACH: a crate with a deliberate type error built \
             successfully into the warm shared target — the dep cache masked a \
             broken app. The SEAL is only sound if this build FAILS."
        );
        assert!(
            String::from_utf8_lossy(&bad_out.stderr).contains("E0308"),
            "the broken crate must fail with the injected type mismatch (E0308)\n{}",
            String::from_utf8_lossy(&bad_out.stderr)
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}

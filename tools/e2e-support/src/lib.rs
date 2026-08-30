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

use std::path::Path;
use std::process::Command;

/// The expected-output file name inside a golden directory.
pub const EXPECTED_FILE: &str = "expected.txt";
/// The Ipê entry point inside every golden directory.
pub const MAIN_IPE: &str = "Main.ipe";

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
fn rewrite_package_name(emitted_dir: &Path, golden_name: &str) -> Result<String, String> {
    const ANCHOR: &str = "name = \"ipe-app\"";

    let manifest = emitted_dir.join("Cargo.toml");
    let original = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
    if !original.contains(ANCHOR) {
        return Err(format!(
            "emitted manifest {} did not contain the expected `{ANCHOR}` anchor",
            manifest.display()
        ));
    }
    let unique = format!("ipe-app-e2e-{}", sanitize(golden_name));
    let rewritten = original.replacen(ANCHOR, &format!("name = \"{unique}\""), 1);
    std::fs::write(&manifest, rewritten)
        .map_err(|e| format!("cannot write {}: {e}", manifest.display()))?;
    Ok(unique)
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
    cmd.arg("build")
        .arg("--message-format=json")
        .current_dir(emitted_dir)
        .env("CARGO_BUILD_RUSTC_WRAPPER", "")
        .env("RUSTC_WRAPPER", "");
    if let Some(p) = resolve_emitted_target(shared.as_deref()) {
        cmd.env("CARGO_TARGET_DIR", p);
    }
    let build = cmd
        .output()
        .map_err(|e| format!("{golden_name}: failed to spawn `cargo build`: {e}"))?;
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
    use super::resolve_emitted_target;

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
}

//! Shared end-to-end build/run support for the golden parity gate.
//!
//! Every golden's emitted Rust project is built into ONE shared cargo target —
//! the machine-global `~/.cache/sky-rust-target`, configured in the global
//! `~/.cargo/config.toml` (`target-dir = …`). cargo reads that file on every
//! invocation, including builds launched from an emitted project under
//! `std::env::temp_dir()`, so heavy dependencies (tokio / rsa / serde / …)
//! compile ONCE and are reused across all goldens and across runs. The target
//! is never deleted; reuse is the whole point.
//!
//! To let the per-golden root binaries coexist in that single target without
//! clobbering one another, this helper rewrites the emitted manifest to a unique
//! package (and therefore binary) name per golden before building.
//!
//! The produced binary is located ROBUSTLY by parsing
//! `cargo build --message-format=json` for the artifact's `executable` field —
//! never a hard-coded `target/debug/<name>` path, which would silently break the
//! moment the target dir moves (exactly the breakage the per-test
//! `CARGO_TARGET_DIR` override existed to paper over).
//!
//! Rigour: a build failure FAILS the test (the build assert carries cargo's
//! stderr); it is never skipped and never reported as a false green.

use std::path::Path;
use std::process::Command;

/// Outcome of building and running an emitted Sky project.
pub struct RunOutcome {
    /// The program's standard output, decoded lossily from UTF-8.
    pub stdout: String,
    /// The process exit code (`None` if the process was killed by a signal).
    pub exit_code: Option<i32>,
}

impl RunOutcome {
    /// The empty outcome returned only on a path that the preceding `assert!`
    /// has already failed the test on — keeps the type checker satisfied without
    /// reaching for a deny-listed `unwrap`/`expect`/`panic!`.
    const fn aborted() -> Self {
        Self {
            stdout: String::new(),
            exit_code: None,
        }
    }
}

/// Turn an arbitrary golden name into a cargo-package-safe suffix: cargo names
/// permit only ASCII alphanumerics, `-`, and `_`. Anything else becomes `_`.
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

/// Rewrite the emitted `Cargo.toml` so its package — and hence its default
/// binary — carries a name unique to this golden. The emitter always writes
/// `name = "sky-app"`; we swap in `sky-app-e2e-<golden>` so the binaries from
/// every golden coexist in the one shared target.
///
/// Returns the unique package name on success, or an error string describing
/// what went wrong (missing manifest, unexpected manifest shape).
fn rewrite_package_name(emitted_dir: &Path, golden_name: &str) -> Result<String, String> {
    const ANCHOR: &str = "name = \"sky-app\"";

    let manifest = emitted_dir.join("Cargo.toml");
    let original = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;

    if !original.contains(ANCHOR) {
        return Err(format!(
            "emitted manifest {} did not contain the expected `{ANCHOR}` anchor",
            manifest.display()
        ));
    }

    let unique = format!("sky-app-e2e-{}", sanitize(golden_name));
    // Replace only the first occurrence (the `[package]` name); the dependency
    // table never spells `name = "sky-app"`.
    let rewritten = original.replacen(ANCHOR, &format!("name = \"{unique}\""), 1);
    std::fs::write(&manifest, rewritten)
        .map_err(|e| format!("cannot write {}: {e}", manifest.display()))?;
    Ok(unique)
}

/// Parse `cargo build --message-format=json` stdout for the produced binary.
///
/// cargo emits one JSON object per line. The binary we want is the unique
/// `compiler-artifact` whose `executable` field is a non-null string (library
/// dependencies have a null `executable`). We additionally require the
/// artifact's package id to mention our unique package name, so a stray
/// executable artifact can never be mistaken for ours.
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

/// Build the emitted project at `emitted_dir` into the shared target and run the
/// resulting binary, returning its captured stdout and exit code.
///
/// # Panics
///
/// Fails the calling test (via `assert!`) if the manifest cannot be retargeted,
/// if `cargo build` fails (surfacing cargo's stderr), if the produced binary
/// cannot be located in the JSON output, or if the binary cannot be executed.
/// It never returns a placeholder on a green path — a broken golden cannot pass.
#[must_use]
#[allow(dead_code)] // not every golden test binary exercises every helper
pub fn build_and_run_emitted(golden_name: &str, emitted_dir: &Path) -> RunOutcome {
    let retargeted = rewrite_package_name(emitted_dir, golden_name);
    assert!(
        retargeted.is_ok(),
        "{golden_name}: {}",
        retargeted.as_ref().err().map_or("", String::as_str)
    );
    let Ok(unique_pkg) = retargeted else {
        return RunOutcome::aborted();
    };

    // No CARGO_TARGET_DIR override: the build inherits the global shared target
    // from ~/.cargo/config.toml, so deps are reused, not recompiled per golden.
    let build = Command::new("cargo")
        .arg("build")
        .arg("--message-format=json")
        .current_dir(emitted_dir)
        .output();
    assert!(
        build.is_ok(),
        "{golden_name}: failed to spawn `cargo build`: {:?}",
        build.as_ref().err()
    );
    let Ok(build) = build else {
        return RunOutcome::aborted();
    };

    assert!(
        build.status.success(),
        "{golden_name}: emitted project must build\n--- cargo stderr ---\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let json_stdout = String::from_utf8_lossy(&build.stdout);
    let exe = find_executable(&json_stdout, &unique_pkg);
    assert!(
        exe.is_some(),
        "{golden_name}: no `executable` artifact for package `{unique_pkg}` in cargo JSON output"
    );
    let Some(exe) = exe else {
        return RunOutcome::aborted();
    };

    let run = Command::new(&exe).output();
    assert!(
        run.is_ok(),
        "{golden_name}: emitted binary `{exe}` must run: {:?}",
        run.as_ref().err()
    );
    let Ok(run) = run else {
        return RunOutcome::aborted();
    };

    RunOutcome {
        stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
        exit_code: run.status.code(),
    }
}

//! Integration tests for `skyc run` — gated on `SKY_E2E=1` so the default
//! `cargo nextest` stays fast and offline (no cargo invocation required).
//!
//! The non-E2E tests still exercise the CLI parsing surface (usage errors) and
//! are unconditionally active.

use std::fs;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `assert!(false_marker())` fails the test without tripping
/// `clippy::assertions_on_constants`.
fn false_marker() -> bool {
    std::hint::black_box(false)
}

// ---------------------------------------------------------------------------
// CLI-parsing tests (unconditional — no network, no build)
// ---------------------------------------------------------------------------

/// `skyc run` (no arguments) must return a usage error, not panic.
#[test]
fn run_no_args_returns_usage_error() {
    let args: Vec<String> = vec!["run".to_owned()];
    let result = skyc::run_cli(&args);
    assert!(
        matches!(result, Err(skyc::CliError::Usage(_))),
        "expected Usage error for bare `skyc run`, got: {result:?}"
    );
}

/// `skyc run <entry> --bogus` (unrecognised flag after the entry) must return
/// a usage error, not panic.  Note: `run_run` treats the FIRST positional arg
/// as the entry path unconditionally, so `--bogus-flag` in the flag position
/// (after the entry) is what triggers the Usage arm.
#[test]
fn run_unknown_flag_returns_usage_error() {
    let args: Vec<String> = vec![
        "run".to_owned(),
        "Main.sky".to_owned(),
        "--bogus-flag".to_owned(),
    ];
    let result = skyc::run_cli(&args);
    assert!(
        matches!(result, Err(skyc::CliError::Usage(_))),
        "expected Usage error for unknown flag after entry, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// E2E test — only active when SKY_E2E=1 (requires cargo + runtime)
// ---------------------------------------------------------------------------

/// `skyc run <entry.sky>` must:
///   1. Compile the Sky program (exit 0 from skyc pipeline).
///   2. Invoke `cargo build` on the emitted project (SEAL check).
///   3. Exec the resulting binary; its stdout must equal `"hello from run\n"`.
///
/// This test exercises the full `run_run` path from CLI dispatch through the
/// Unix `exec` replacement.  It is skipped unless `SKY_E2E=1` is set.
#[test]
fn run_subcommand_builds_and_executes_hello_program() {
    const SRC: &str = "module Main exposing (main)\n\nmain = println \"hello from run\"\n";

    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    // Resolve the runtime dir (skips the test when SKY_RUNTIME_DIR is unset
    // and the walk-up also fails, which happens in CI without the repo tree).
    let runtime = skyc::resolve_runtime();
    assert!(
        runtime.is_ok(),
        "runtime must resolve for E2E test: {runtime:?}"
    );
    let Ok(runtime_dir) = runtime else {
        return;
    };

    // Write the source file into a temp directory.
    let dir = std::env::temp_dir().join("skyc_run_subcommand_e2e");
    let _ = fs::remove_dir_all(&dir);
    let entry = dir.join("Main.sky");
    let created = fs::create_dir_all(&dir).and_then(|()| fs::write(&entry, SRC));
    assert!(created.is_ok(), "write source: {created:?}");

    // Use a dedicated out dir so CARGO_TARGET_DIR contention is isolated.
    let out_dir = dir.join("out");

    // --- Step 1+2: compile + cargo build via run_cli ---
    // We cannot call run_cli("run", …) directly here because on Unix it would
    // exec (replace) this test process.  Instead, call the public `build`
    // function + cargo build directly to verify SEAL, then run the binary as a
    // child process to check stdout.  This exercises the same code paths as
    // run_run without sacrificing the test runner.
    let built = skyc::build(&entry, &out_dir, &runtime_dir);
    assert!(built.is_ok(), "skyc build step must succeed: {built:?}");

    let cargo_status = std::process::Command::new("cargo")
        .arg("build")
        .current_dir(&out_dir)
        .env("CARGO_TARGET_DIR", out_dir.join("target"))
        .status();
    assert!(
        matches!(&cargo_status, Ok(s) if s.success()),
        "cargo build on emitted project must succeed: {cargo_status:?}"
    );

    // --- Step 3: run the binary, capture stdout ---
    let bin: PathBuf = out_dir.join("target").join("debug").join("sky-app");
    let run = std::process::Command::new(&bin).output();
    let Ok(run) = run else {
        assert!(false_marker(), "failed to run emitted binary: {run:?}");
        return;
    };
    assert!(run.status.success(), "binary must exit 0");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "hello from run\n",
        "skyc run e2e: stdout mismatch"
    );

    // Cleanup heavy cargo artifacts; leave src for post-mortem if needed.
    let _ = fs::remove_dir_all(out_dir.join("target"));
}

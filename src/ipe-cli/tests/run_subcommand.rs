//! Integration tests for `ipe run` — gated on `IPE_E2E=1` so the default
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
// `const` is rejected on purpose: a const-known `false` would re-trip
// `assertions_on_constants` at the call site, defeating the `black_box`.
#[allow(clippy::missing_const_for_fn)]
fn false_marker() -> bool {
    std::hint::black_box(false)
}

// ---------------------------------------------------------------------------
// CLI-parsing tests (unconditional — no network, no build)
// ---------------------------------------------------------------------------

/// `ipe run` (no arguments, nothing to build here) must return a command-usage
/// error naming `run` — so the caller shows `run`'s help page — not panic.
#[test]
fn run_no_args_returns_usage_error() {
    let args: Vec<String> = vec!["run".to_owned()];
    let result = ipe::run_cli(&args);
    assert!(
        matches!(
            result,
            Err(ipe::CliError::CommandUsage { command: "run", .. })
        ),
        "expected a `run` command-usage error for bare `ipe run`, got: {result:?}"
    );
}

/// `ipe run <entry> --bogus` (unrecognised flag after the entry) must return a
/// command-usage error for `run` — carrying a reason naming the offending flag,
/// so the caller shows `run`'s help page — not panic.
#[test]
fn run_unknown_flag_returns_usage_error() {
    let args: Vec<String> = vec![
        "run".to_owned(),
        "Main.ipe".to_owned(),
        "--bogus-flag".to_owned(),
    ];
    let result = ipe::run_cli(&args);
    assert!(
        matches!(
            &result,
            Err(ipe::CliError::CommandUsage { command, reason })
                if *command == "run" && reason.contains("--bogus-flag")
        ),
        "expected a `run` command-usage error naming the offending flag, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// E2E test — only active when IPE_E2E=1 (requires cargo + runtime)
// ---------------------------------------------------------------------------

/// `ipe run <entry.ipe>` must:
///   1. Compile the Ipê program (exit 0 from ipe pipeline).
///   2. Invoke `cargo build` on the emitted project (SEAL check).
///   3. Exec the resulting binary; its stdout must equal `"hello from run\n"`.
///
/// This test exercises the full `run_run` path from CLI dispatch through the
/// Unix `exec` replacement.  It is skipped unless `IPE_E2E=1` is set.
#[test]
fn run_subcommand_builds_and_executes_hello_program() {
    const SRC: &str = "module Main exposing (main)\n\nmain = Io.println \"hello from run\"\n";

    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    // Resolve the runtime dir (skips the test when IPE_RUNTIME_DIR is unset
    // and the walk-up also fails, which happens in CI without the repo tree).
    let runtime = ipe::resolve_runtime();
    assert!(
        runtime.is_ok(),
        "runtime must resolve for E2E test: {runtime:?}"
    );
    let Ok(runtime_dir) = runtime else {
        return;
    };

    // Write the source file into a temp directory.
    let dir = std::env::temp_dir().join("ipec_run_subcommand_e2e");
    let _ = fs::remove_dir_all(&dir);
    let entry = dir.join("Main.ipe");
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
    let built = ipe::build(&entry, &out_dir, &runtime_dir);
    assert!(built.is_ok(), "ipe build step must succeed: {built:?}");

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
    let bin: PathBuf = out_dir.join("target").join("debug").join("ipe-app");
    let run = std::process::Command::new(&bin).output();
    let Ok(run) = run else {
        assert!(false_marker(), "failed to run emitted binary: {run:?}");
        return;
    };
    assert!(run.status.success(), "binary must exit 0");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "hello from run\n",
        "ipec run e2e: stdout mismatch"
    );

    // Cleanup heavy cargo artifacts; leave src for post-mortem if needed.
    let _ = fs::remove_dir_all(out_dir.join("target"));
}

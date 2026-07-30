//! `ipe verify` — the one-command project gate.
//!
//! Runs the project's checks in order — format, type-check, build — stopping at
//! the first failure. Each stage composes the same code path its standalone
//! command uses, so these tests assert the *composition and reporting*: an
//! unformatted project stops at the format stage, a type-erroring but
//! well-formatted project passes format then stops at the type-check stage, and
//! a clean project clears every stage (the full pass, which builds, is gated on
//! `IPE_E2E=1` so the default `cargo nextest` stays fast and offline).

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

/// Absolute path to a fixture under this crate's `tests/fixtures/verify`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/verify")
        .join(name)
}

/// Run the built `ipe` binary and capture `(success, stdout, stderr)`.
fn run_ipe(args: &[&str]) -> Result<(bool, String, String), Box<dyn Error>> {
    let out = Command::new(env!("CARGO_BIN_EXE_ipe"))
        .args(args)
        .output()?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// An unformatted project stops at the very first stage: `verify` reports the
/// format failure and exits non-zero, and never reaches the type-check stage.
#[test]
fn unformatted_project_stops_at_the_format_stage() -> TestResult {
    let (ok, stdout, stderr) = run_ipe(&["verify", &fixture("unformatted.ipe").to_string_lossy()])?;
    assert!(!ok, "an unformatted project must exit non-zero");
    assert!(
        stdout.contains("format failed"),
        "the format stage must be reported as failed, got stdout:\n{stdout}"
    );
    assert!(
        stderr.contains("the format stage failed") && stderr.contains("not formatted"),
        "the format stage's own report must be shown, got stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("type-check"),
        "verify must stop before the type-check stage, got stdout:\n{stdout}"
    );
    Ok(())
}

/// A well-formatted but type-erroring project passes the format stage, then
/// stops at the type-check stage, exiting non-zero with the type diagnostic —
/// and never reaches the build stage.
#[test]
fn type_error_project_stops_at_the_type_check_stage() -> TestResult {
    let (ok, stdout, stderr) = run_ipe(&["verify", &fixture("type_error.ipe").to_string_lossy()])?;
    assert!(!ok, "a type-erroring project must exit non-zero");
    assert!(
        stdout.contains("format passed") && stdout.contains("type-check failed"),
        "format must pass and type-check must fail, got stdout:\n{stdout}"
    );
    assert!(
        stderr.contains("the type-check stage failed")
            && stderr.contains("IPE-T0001")
            && stderr.contains("type mismatch"),
        "the type diagnostic must be shown, got stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("stage 3/3"),
        "verify must stop before the build stage, got stdout:\n{stdout}"
    );
    Ok(())
}

/// A stage failure is a gate result, not a misuse of `verify`: the failing
/// stage's report is shown alone, never the `verify --help` page a raw usage
/// error would trigger.
#[test]
fn stage_failure_does_not_print_the_verify_help_page() -> TestResult {
    let (_, _, stderr) = run_ipe(&["verify", &fixture("unformatted.ipe").to_string_lossy()])?;
    assert!(
        !stderr.contains("ipe verify [<path>]"),
        "a stage failure must not print the verify help synopsis, got stderr:\n{stderr}"
    );
    Ok(())
}

/// An unknown flag IS a misuse of `verify`: it exits non-zero and shows the
/// `verify` help page (the uniform "misuse shows help" behaviour).
#[test]
fn unknown_flag_is_misuse_and_shows_help() {
    let args: Vec<String> = vec!["verify".to_owned(), "--bogus".to_owned()];
    let result = ipe::run_cli(&args);
    assert!(
        matches!(
            &result,
            Err(ipe::CliError::CommandUsage { command, reason })
                if *command == "verify" && reason.contains("--bogus")
        ),
        "expected a `verify` command-usage error naming the offending flag, got: {result:?}"
    );
}

/// A clean project clears every stage and exits 0 with the all-passed summary.
/// Gated on `IPE_E2E=1` because the build stage invokes `cargo` and needs the
/// Ipê runtime — kept out of the default fast, offline test run.
#[test]
fn clean_project_passes_every_stage() -> TestResult {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("skipping: set IPE_E2E=1 to run the building verify E2E");
        return Ok(());
    }
    // Copy the clean entry into a fresh directory named `Main.ipe` so the build
    // stage's default entry conventions apply cleanly.
    let dir = std::env::temp_dir().join(format!("ipe_verify_clean_{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let src = dir.join("Main.ipe");
    std::fs::copy(fixture("clean.ipe"), &src)?;

    let (ok, stdout, stderr) = run_ipe(&["verify", &src.to_string_lossy()])?;
    assert!(
        ok,
        "a clean project must pass every stage, got stderr:\n{stderr}"
    );
    assert!(
        stdout.contains("format passed")
            && stdout.contains("type-check passed")
            && stdout.contains("build passed")
            && stdout.contains("all 3 stages passed"),
        "every stage must be reported as passed, got stdout:\n{stdout}"
    );
    Ok(())
}

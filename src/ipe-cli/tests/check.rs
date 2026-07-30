//! `ipe check` — type-check a program with no build, no run, no emit.
//!
//! Exit 0 with a terse `ok` when the program type-checks; non-zero with the
//! rendered diagnostic on any parse/canon/type error. A program importing a
//! compiled-source stdlib module (`Ipe.Test`) resolves through the same
//! injection-aware source graph the build path uses.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

/// Absolute path to a fixture under this crate's `tests/fixtures/check`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/check")
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

#[test]
fn well_typed_program_exits_zero_with_ok() -> TestResult {
    let (ok, stdout, _) = run_ipe(&["check", &fixture("well_typed.ipe").to_string_lossy()])?;
    assert!(ok, "a well-typed program must exit 0");
    assert_eq!(stdout.trim(), "ok");
    Ok(())
}

#[test]
fn type_error_program_exits_nonzero_with_the_diagnostic() -> TestResult {
    let (ok, _, stderr) = run_ipe(&["check", &fixture("type_error.ipe").to_string_lossy()])?;
    assert!(!ok, "a type-error program must exit non-zero");
    assert!(
        stderr.contains("IPE-T0001") && stderr.contains("type mismatch"),
        "the rendered type diagnostic must be shown, got:\n{stderr}"
    );
    Ok(())
}

/// A program importing `Ipe.Test` — a compiled-source stdlib module that
/// declares its own `Test` type — must resolve through injection and type-check,
/// exactly as `ipe build` would. A bare single-module path fails name
/// resolution here (IPE-N0004) because the module's source is never injected.
#[test]
fn program_using_ipe_test_resolves_and_type_checks() -> TestResult {
    let (ok, stdout, stderr) =
        run_ipe(&["check", &fixture("uses_ipe_test.ipe").to_string_lossy()])?;
    assert!(
        ok,
        "an Ipe.Test-using program must type-check, got stderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "ok");
    Ok(())
}

/// `check` type-checks and stops: no emitted project is written next to the
/// entry (a build would create `out/`). The entry is copied into a fresh,
/// otherwise-empty directory so any emission would be unmistakable.
#[test]
fn check_writes_no_emitted_project() -> TestResult {
    let dir = std::env::temp_dir().join(format!("ipe_check_no_emit_{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let src = dir.join("Main.ipe");
    std::fs::copy(fixture("well_typed.ipe"), &src)?;

    let (ok, _, _) = run_ipe(&["check", &src.to_string_lossy()])?;
    let out_present = dir.join("out").exists();
    let siblings: Vec<_> = std::fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.path())
        .collect();
    std::fs::remove_dir_all(&dir)?;

    assert!(ok, "the well-typed program must check");
    assert!(
        !out_present,
        "check must not emit an out/ directory, dir held: {siblings:?}"
    );
    Ok(())
}

#[test]
fn check_help_page_names_the_command() -> TestResult {
    let (ok, stdout, _) = run_ipe(&["check", "--help"])?;
    assert!(ok, "--help exits 0");
    assert!(
        stdout.contains("check") && stdout.contains("Type-check"),
        "help page names the command, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn check_rejects_an_unexpected_option() -> TestResult {
    let (ok, _, stderr) = run_ipe(&["check", "--json"])?;
    assert!(!ok, "an unknown flag is misuse");
    assert!(
        stderr.contains("unexpected option"),
        "the misuse reason must name the option, got:\n{stderr}"
    );
    Ok(())
}

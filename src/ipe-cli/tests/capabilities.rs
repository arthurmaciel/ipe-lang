//! `ipe capabilities` — the read-only capability report and the
//! declared-set verification primitive.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

use ipe::verify_capabilities;
use ipe_ir::Capability;

type TestResult = Result<(), Box<dyn Error>>;

/// Absolute path to a fixture under this crate's `tests/fixtures/capabilities`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/capabilities")
        .join(name)
}

/// Run the built `ipe` binary and return its captured `(status_success, stdout)`.
fn run_ipe(args: &[&str]) -> Result<(bool, String), Box<dyn Error>> {
    let out = Command::new(env!("CARGO_BIN_EXE_ipe"))
        .args(args)
        .output()?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

#[test]
fn reports_network_for_an_http_program() -> TestResult {
    let (ok, stdout) = run_ipe(&[
        "capabilities",
        "--plain",
        &fixture("uses_http.ipe").to_string_lossy(),
    ])?;
    assert!(ok, "capabilities must exit 0");
    assert_eq!(stdout.trim(), "network");
    Ok(())
}

#[test]
fn reports_none_for_a_pure_program() -> TestResult {
    let (ok, stdout) = run_ipe(&[
        "capabilities",
        "--plain",
        &fixture("pure_string.ipe").to_string_lossy(),
    ])?;
    assert!(ok, "capabilities must exit 0");
    // Under `--plain`, a pure program emits zero records (empty) — the "no
    // capabilities" wording lives only in the human default and `--json`'s `[]`.
    assert!(
        stdout.trim().is_empty(),
        "a pure program has no --plain capability lines, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn capabilities_help_page_lists_the_command() -> TestResult {
    let (ok, stdout) = run_ipe(&["capabilities", "--help"])?;
    assert!(ok, "--help exits 0");
    assert!(
        stdout.contains("capabilities"),
        "help page names the command, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn verify_accepts_the_exact_declared_set() {
    let declared = BTreeSet::from([Capability::Network]);
    let r = verify_capabilities(&fixture("uses_http.ipe"), &declared);
    assert!(r.is_ok(), "an exact declaration verifies: {r:?}");
}

#[test]
fn verify_rejects_underdeclared() {
    // The program uses `network` but declares nothing.
    let declared = BTreeSet::new();
    let r = verify_capabilities(&fixture("uses_http.ipe"), &declared);
    assert!(r.is_err(), "an empty declaration must be rejected");
}

#[test]
fn verify_rejects_overdeclared() {
    // The pure program uses nothing but declares `filesystem`.
    let declared = BTreeSet::from([Capability::Filesystem]);
    let r = verify_capabilities(&fixture("pure_string.ipe"), &declared);
    assert!(r.is_err(), "an over-declaration must be rejected");
}

/// Acceptance over a real `examples/*` app with known effects: the
/// `02-go-stdlib` program makes an HTTP call (`Http.get`) and reads the clock
/// (`Time.now` / `Time.timeString`), and uses no other effect — so its inferred
/// capability set is exactly `{network, clock}`. Any drift here is a
/// mis-classified tag, caught against a real program rather than a fixture.
#[test]
fn acceptance_go_stdlib_example_infers_network_and_clock() -> TestResult {
    let example = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/sky/ipe/02-go-stdlib/src/Main.ipe");
    let (ok, stdout) = run_ipe(&["capabilities", "--plain", &example.to_string_lossy()])?;
    assert!(ok, "capabilities must exit 0 on the example");
    let reported: BTreeSet<&str> = stdout.split_whitespace().collect();
    assert_eq!(
        reported,
        BTreeSet::from(["network", "clock"]),
        "unexpected capability set for 02-go-stdlib, got:\n{stdout}"
    );

    // The library verifier agrees with the reported set exactly.
    let declared = BTreeSet::from([Capability::Network, Capability::Clock]);
    let r = verify_capabilities(&example, &declared);
    assert!(r.is_ok(), "the exact inferred set must verify: {r:?}");
    Ok(())
}

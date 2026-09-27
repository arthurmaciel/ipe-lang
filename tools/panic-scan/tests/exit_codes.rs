//! Binary-level refusals: the exit code the CI gate keys on.
//!
//! `scan_str` returning `Err` is not enough — the guarantee is that the
//! *process* fails closed. A file the scanner cannot lex is unaudited, so the
//! run must exit non-zero rather than silently pass: "cannot analyze" must
//! never read as "no panics". These pin that boundary against regression.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_panic-scan"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

/// An un-lexable file (unterminated string literal) must fail the scan closed,
/// never pass green. This is the fail-open the gate exists to prevent.
#[test]
fn unlexable_file_fails_closed() {
    let status = bin()
        .arg(fixture("unlexable.rstxt"))
        .status()
        .expect("run panic-scan");
    assert!(
        !status.success(),
        "un-lexable file must fail the scan closed, got success"
    );
    assert_eq!(
        status.code(),
        Some(2),
        "un-lexable file must exit 2 (hard error), not 0/1"
    );
}

/// A clean, lexable file with no banned construct exits 0 — proving the gate is
/// not merely always-failing.
#[test]
fn clean_file_passes() {
    let status = bin()
        .arg(fixture("negatives.rs"))
        .status()
        .expect("run panic-scan");
    assert!(
        status.success(),
        "clean file must pass, got exit {:?}",
        status.code()
    );
}

/// A lexable file that DOES contain banned constructs exits 1 — the ordinary
/// "found panics" failure, distinct from the exit-2 hard error above.
#[test]
fn file_with_hits_exits_one() {
    let status = bin()
        .arg(fixture("positives.rs"))
        .status()
        .expect("run panic-scan");
    assert_eq!(
        status.code(),
        Some(1),
        "file with banned constructs must exit 1"
    );
}

/// Run the binary from the `test_paths` fixture root.
///
/// Every path it sees is then relative to that root, so no ancestor directory
/// can read as a test marker.
#[allow(clippy::expect_used)] // a test that cannot spawn the binary has nothing to assert
fn scan_test_paths(args: &[&str]) -> std::process::Output {
    bin()
        .current_dir(fixture("test_paths"))
        .args(args)
        .output()
        .expect("run panic-scan")
}

/// A `pub mod tests;` without `#[cfg(test)]` puts its `unwrap()` in production.
///
/// Skipping its `tests/` directory must be refused, naming the declaring file.
#[test]
fn ungated_tests_directory_fails_closed() {
    let out = scan_test_paths(&["ungated/src/tests/mod.rs"]);
    assert_eq!(out.status.code(), Some(2), "ungated tests/ must exit 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ungated/src/lib.rs"),
        "the refusal must name the declaring file, got {stderr:?}"
    );
}

/// An ungated `mod tests;` backed by a `tests.rs` file is refused the same way.
#[test]
fn ungated_tests_module_file_fails_closed() {
    let out = scan_test_paths(&["ungated_file/src/tests.rs"]);
    assert_eq!(out.status.code(), Some(2), "ungated tests.rs must exit 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ungated_file/src/lib.rs"),
        "the refusal must name the declaring file, got {stderr:?}"
    );
}

/// A `tests/` directory no module declares has no proof it is test-only.
#[test]
fn undeclared_tests_directory_fails_closed() {
    let out = scan_test_paths(&["undeclared/src/tests/mod.rs"]);
    assert_eq!(out.status.code(), Some(2), "undeclared tests/ must exit 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("undeclared/src/tests"),
        "the refusal must name the test module, got {stderr:?}"
    );
}

/// The walk refuses an ungated test module it meets inside the tree.
#[test]
fn walk_fails_closed_on_an_ungated_tests_directory() {
    let out = scan_test_paths(&["--walk", "ungated/src"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "walk over ungated tests/ must exit 2"
    );
}

/// Gated `tests/` and `tests.rs` modules are skipped despite their `unwrap()`.
#[test]
fn gated_test_modules_are_skipped() {
    let out = scan_test_paths(&[
        "gated/src/lib.rs",
        "gated/src/unit.rs",
        "gated/src/tests/mod.rs",
        "gated/src/unit/tests.rs",
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "gated test modules must be skipped, got stdout {:?} stderr {:?}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A production file whose name merely contains `tests` is still scanned.
#[test]
fn a_name_containing_tests_is_still_scanned() {
    let out = scan_test_paths(&["gated/src/contests.rs"]);
    assert_eq!(out.status.code(), Some(1), "contests.rs must be scanned");
}

/// The walk scans production files and skips gated test modules.
#[test]
fn walk_scans_production_and_skips_gated_tests() {
    let out = scan_test_paths(&["--walk", "gated/src"]);
    assert_eq!(out.status.code(), Some(1), "contests.rs hit must exit 1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.is_empty()
            && stdout
                .lines()
                .all(|line| line.starts_with("gated/src/contests.rs:")),
        "only the contests.rs hit may be reported, got {stdout:?}"
    );
}

/// A walk that finds nothing to audit must not read as a pass.
#[test]
fn walk_of_a_missing_or_empty_tree_fails_closed() {
    for args in [&["--walk", "no-such-dir"][..], &["--walk"][..]] {
        let out = scan_test_paths(args);
        assert_eq!(out.status.code(), Some(2), "{args:?} must exit 2");
    }
}

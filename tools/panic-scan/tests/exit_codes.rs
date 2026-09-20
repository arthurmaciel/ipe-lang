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

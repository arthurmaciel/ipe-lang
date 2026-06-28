//! Milestone-3b-1 fail-closed gates: each tuple-pattern shape the lowerer does
//! not model yet must surface a clean, span-carrying SKY-L0115 — never a panic,
//! an internal compiler bug, or silent (refutable) cargo-failing Rust.
//!
//! * a `case` on a tuple with MORE THAN ONE arm (needs product/literal-pattern
//!   exhaustiveness) → SKY-L0115,
//! * a single tuple `case` arm with a REFUTABLE element (a constructor) →
//!   SKY-L0115.
//!
//! Each is driven through the full `skyc` pipeline and asserted to produce its
//! exact code, locking the gap so it can never regress into a worse failure mode.

use std::path::{Path, PathBuf};

use skyc::CliError;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Build the named golden fixture and assert it surfaces exactly `expected` as a
/// pipeline diagnostic. A build that succeeds, or fails with any other error,
/// makes `got` differ from `Some(expected)` and fails with a descriptive message
/// — never a panic. A skip occurs only when the runtime cannot be resolved.
fn assert_gate(fixture: &str, out_suffix: &str, expected: sky_diagnostics::Code) {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join(fixture)
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(out_suffix);
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };
    let built = skyc::build(&entry, &out, &runtime);
    let got = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        got,
        Some(expected),
        "fixture {fixture}: expected {expected:?}, got build result {built:?}"
    );
}

#[test]
fn multi_arm_tuple_case_is_sky_l0115() {
    assert_gate(
        "m3b1_gate_multiarm",
        "m3b1_gate_multiarm_emit",
        sky_diagnostics::SKY_L0115,
    );
}

#[test]
fn refutable_tuple_element_is_sky_l0115() {
    assert_gate(
        "m3b1_gate_refutable",
        "m3b1_gate_refutable_emit",
        sky_diagnostics::SKY_L0115,
    );
}

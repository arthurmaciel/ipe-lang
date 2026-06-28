//! Milestone-3b-2 fail-closed gates — each unsupported shape must surface a
//! clean, span-carrying diagnostic, never a panic / internal compiler bug /
//! cargo-failing Rust:
//!
//! * a RECORD sub-pattern nested in a constructor payload (`Box { x }`) →
//!   SKY-L0112 (the nested-record-carrier gap), and
//! * a non-exhaustive NESTED `case` (`Just (Just a) -> … ; Nothing -> …`,
//!   missing `Just Nothing`) → SKY-T0010, caught BEFORE emit so rustc never
//!   sees a non-exhaustive `match` (the soundness floor).
//!
//! (The sibling SKY-L0116 — two arms for the same constructor — is locked by
//! `golden_m3a_gates::duplicate_constructor_arms_is_sky_l0116`.)

use std::path::{Path, PathBuf};

use skyc::CliError;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Build the named golden fixture and assert it surfaces exactly `expected` as a
/// pipeline diagnostic — never a panic. A skip occurs only when the runtime
/// cannot be resolved.
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
fn record_pattern_in_ctor_payload_is_sky_l0112() {
    assert_gate(
        "m3b2_gate_record_payload",
        "m3b2_gate_record_payload_emit",
        sky_diagnostics::SKY_L0112,
    );
}

#[test]
fn non_exhaustive_nested_case_is_sky_t0010() {
    assert_gate(
        "m3b2_gate_nonexhaustive",
        "m3b2_gate_nonexhaustive_emit",
        sky_diagnostics::SKY_T0010,
    );
}

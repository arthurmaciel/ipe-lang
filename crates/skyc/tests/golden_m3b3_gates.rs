//! Milestone-3b-3 exhaustiveness / redundancy gates for literal-pattern `case`s.
//!
//! * Int / Char / String are OPEN types: literal arms with no wildcard / variable
//!   catch-all do not cover every value, so the `case` is non-exhaustive →
//!   SKY-T0010 (the soundness floor — a non-exhaustive Sky `case` MUST be caught
//!   before emit, never deferred to a rustc E0004 / `unreachable!()` fallback).
//! * A literal arm AFTER a wildcard catch-all matches no value left open → the
//!   redundant-branch warning SKY-T0011.
//!
//! Each is driven through the full `skyc` pipeline and asserted to produce its
//! exact code. The Go reference accepts a redundant branch silently; the Rust
//! backend is deliberately stricter (it reports SKY-T0011) — a documented,
//! never-silently-wrong divergence.

use std::path::{Path, PathBuf};

use skyc::CliError;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

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
fn non_exhaustive_int_case_is_sky_t0010() {
    assert_gate(
        "m3b3_gate_nonexhaustive",
        "m3b3_gate_nonexhaustive_emit",
        sky_diagnostics::SKY_T0010,
    );
}

#[test]
fn redundant_branch_after_catch_all_is_sky_t0011() {
    assert_gate(
        "m3b3_gate_redundant",
        "m3b3_gate_redundant_emit",
        sky_diagnostics::SKY_T0011,
    );
}

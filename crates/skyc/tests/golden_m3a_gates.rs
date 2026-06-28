//! Milestone-3a fail-closed gates: each shape M3a does not model must surface a
//! clean, span-carrying diagnostic — never a panic, an internal compiler bug, or
//! silent cargo-failing Rust.
//!
//! * a constructor pattern binding the wrong number of payload fields →
//!   SKY-T0013 (a type error),
//! * two `case` arms head-matching the same constructor with a refutable nested
//!   payload (`Som (Som x)` then `Som Non`) → SKY-L0116 (nested constructor
//!   discrimination; M3b-2 lowers one arm per top-level constructor),
//! * a partially-applied payload constructor used as a function value
//!   (`Node Leaf 1`) → SKY-L0113 (the constructor-as-function lowering gap).
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
fn ctor_pattern_arity_is_sky_t0013() {
    assert_gate(
        "m3a_gate_arity",
        "m3a_gate_arity_emit",
        sky_diagnostics::SKY_T0013,
    );
}

#[test]
fn duplicate_constructor_arms_is_sky_l0116() {
    // `Som (Som x) -> …` then `Som Non -> …`: two arms for `Som`. The nested
    // payload pattern itself now lowers (M3b-2); the second `Som` arm is the
    // gated shape (nested constructor discrimination).
    assert_gate(
        "m3a_gate_nested",
        "m3a_gate_nested_emit",
        sky_diagnostics::SKY_L0116,
    );
}

#[test]
fn partial_ctor_application_is_sky_l0113() {
    assert_gate(
        "m3a_gate_partial",
        "m3a_gate_partial_emit",
        sky_diagnostics::SKY_L0113,
    );
}

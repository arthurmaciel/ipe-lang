//! Fail-closed gate: with the same-top-constructor restriction
//! lifted, exhaustiveness over the nested shape is still proven BEFORE emit.
//!
//! A NESTED non-exhaustive `case` (`Som (Som x) -> … ; Non -> …`, missing
//! `Som Non`) must surface SKY-T0010 from the usefulness checker — never reach
//! the backend, so rustc never sees a non-exhaustive `match` (the soundness
//! floor). This locks the property that removing the SKY-L0116 gate did NOT
//! remove the exhaustiveness guarantee: the Maranget check is the gate.

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
fn non_exhaustive_nested_same_ctor_case_is_sky_t0010() {
    assert_gate(
        "gate_nonexhaustive_nested_same_ctor",
        "m3b4_gate_nonexhaustive_emit",
        sky_diagnostics::SKY_T0010,
    );
}

/// Floor sentinel: EVERY top constructor is covered (`Som`/`Non`) and inside
/// `Som` both inner constructors are covered (`Som …`/`Non`) — the only gap is
/// the DEEPER literal column (`Som (Som 0)` matched, `Som (Som n)` for `n /= 0`
/// uncovered, since `Int` is OPEN). A shallow top-constructor guard would wave
/// this through; only the deep Maranget usefulness check catches it. Asserting
/// SKY-T0010 here pins that the soundness floor rests SOLELY on that check now
/// that the same-top-constructor restriction is lifted.
#[test]
fn all_top_ctors_covered_with_nested_literal_gap_is_sky_t0010() {
    assert_gate(
        "floor_sentinel",
        "m3b4_floor_sentinel_emit",
        sky_diagnostics::SKY_T0010,
    );
}

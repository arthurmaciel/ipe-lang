//! Milestone-2d-1 super-type soundness gate.
//!
//! A generic function whose body constrains a type variable to a Sky super-type
//! (`Number` via `+ - *`, `Comparable` via `< > <= >=`) generalises to a Rust
//! generic carrying the matching trait bound. Using such a function at a type
//! that does not satisfy the bound — here `double` (which needs `Number`)
//! instantiated at `Bool` — must be rejected at type-checking time (SKY-T0014),
//! never left to fail when `cargo` compiles the emitted Rust. This is the
//! soundness floor for bounded generics: skyc accepting a program it cannot
//! lower to compiling Rust is forbidden.
//!
//! The Go reference rejects the same program too (its `Number` constraint is not
//! satisfied by `Bool`); the codes differ but both fail closed.

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
fn number_generic_at_bool_is_sky_t0014() {
    assert_gate(
        "m2d1_gate_unsatisfied",
        "m2d1_gate_unsatisfied_emit",
        sky_diagnostics::SKY_T0014,
    );
}

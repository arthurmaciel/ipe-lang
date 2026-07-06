//! i148 regression lock — `Input.slider` must type-check and lower without
//! `SKY-N0005: Input has no member slider`.
//!
//! Before this fix, `InputSlider` had a `StdlibKernel` variant and a type
//! scheme but was missing from `lower.rs`'s arity table (`callee_arity`) and
//! from the "lower as app-cfg-record" dispatch arm.  The lowerer's exhaustive
//! match rejected it with a compile-time error; user code triggered `SKY-N0005`
//! at resolution time (the name was unregistered in `STDLIB_MODULE_QUALIFIERS`
//! in older builds) or a `CompilerBug` panic at lowering.
//!
//! This test asserts that `skyc::build` succeeds on the fixture — no pipeline
//! diagnostic, no panic.  It does NOT run cargo or the emitted binary (no
//! `SKY_E2E` required).

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Compile `tests/golden/i148_input_slider/Main.sky` and assert it succeeds.
/// Skips silently when the runtime cannot be resolved (CI without runtime dir).
#[test]
fn input_slider_typechecks_and_lowers() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("i148_input_slider")
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("i148_input_slider_emit");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };
    let result = skyc::build(&entry, &out, &runtime);
    assert!(
        result.is_ok(),
        "Input.slider must compile without diagnostic; got: {:?}",
        result.err()
    );
}

//! #193 D3 seal: `count_fn_value_uses` Match arms must stay SUM (not MAX).
//!
//! A `NonClone` fn-typed local passed as an argument (consuming position) in
//! each of two case arms sums to 2 total consuming uses → `reject_fn_value_reuse`
//! must fire with `SKY-L0127`.
//!
//! If `count_fn_value_uses`'s Match arm were accidentally changed to MAX (as
//! the `CloneOk` counter `count_var_uses` was), `max(1,1)=1` → gate silently
//! accepts → the `NonClone` fn-value would be emitted as a double-move in the
//! pattern the runtime cannot execute (only one arm runs, but the compiler does
//! not know which at codegen time and cannot guarantee soundness without the
//! gate).
//!
//! This golden asserts the compiler REJECTS the program with `SKY-L0127`,
//! pinning that the fn-value gate kept SUM after the #193 `CloneOk` counter
//! change.
//!
//! Run:
//! ```text
//! cargo test -p skyc --test golden_i193_nonclone_fn_once_per_arm
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// The compiler must reject a fn-typed local passed as an argument in each of
/// two match arms with `SKY-L0127` (`FunctionValueReuse`).
///
/// If the gate silently accepted (MAX regression), `build` would return `Ok`
/// here and the following `assert_eq` would fail, loudly surfacing the D3
/// regression.
#[test]
fn i193_nonclone_fn_once_per_arm_rejected() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("i193_nonclone_fn_once_per_arm")
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("i193_nonclone_fn_once_per_arm_out");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP i193_nonclone_fn_once_per_arm: runtime not available");
        return;
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);

    let code = match &built {
        Err(skyc::CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        code,
        Some(sky_diagnostics::SKY_L0127),
        "a NonClone fn-typed local passed as argument in each of two match arms \
         must be rejected with SKY-L0127 (FunctionValueReuse); the fn-value gate \
         must keep SUM (not MAX) — got: {built:?}"
    );
}

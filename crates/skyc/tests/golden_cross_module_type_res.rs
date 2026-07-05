//! Regression gate: skyc must exit 0 on multi-module projects.
//!
//! Historical context: `Ui/Charts.sky` in example 17 originally used `(Html Msg)`
//! in annotations without `import State exposing (..)`.  The Rust compiler ICE'd
//! with SKY-I0001 because `ir_type_from_canon` had a unique-match heuristic that
//! searched `enum_variants` by name when `home = []` and no builtin matched.  The
//! heuristic ICE'd when zero or multiple matches existed.
//!
//! **#138 total-resolution fix** (replaces the heuristic): unknown unqualified
//! type names now fail closed at canon time with SKY-N0002 (`TypeNotFound`).
//! The heuristic in `ir_type_from_canon` and `ir_type_from_ty` is removed.
//! Examples 16 and 17 were updated to add the missing `import State exposing (..)`
//! so they compile cleanly without the heuristic.  See `golden_i138_total_resolution`
//! for the error-path regression gate.
//!
//! These tests only check that `skyc::build` succeeds (exit 0); they do NOT
//! build or run the emitted Rust project and do NOT require `SKY_E2E`.
//!
//! Run:
//! ```text
//! cargo test -p skyc --test golden_cross_module_type_res
//! ```

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Compile an example's entry point with `skyc::build` and assert exit 0.
/// Skips silently when the runtime directory is not present (e.g. a minimal
/// checkout without the embedded stdlib), so the test never fails in CI
/// environments that only have the crates but not the full repo layout.
fn assert_skyc_exit0(label: &str, entry_rel: &str) {
    let root = repo_root();
    let entry = root.join(entry_rel);
    if !entry.exists() {
        eprintln!("SKIP {label}: entry not found at {}", entry.display());
        return;
    }
    let out =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{label}_skyc_out"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP {label}: runtime not available");
        return;
    };
    // Multi-module examples require sibling-discovery so imports like
    // `import State exposing (..)` resolve to adjacent `.sky` files.
    let result = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        result.is_ok(),
        "skyc must compile {label} cleanly (exit 0): {:?}",
        result.err()
    );
}

/// Example 17 (skymon) — `Ui/Charts.sky` uses `(Html Msg)` in annotations
/// without `import State`.  Regression for SKY-I0001 in `ir_type_from_canon`.
#[test]
fn ex17_skymon_exits_zero() {
    assert_skyc_exit0("ex17_skymon", "examples/17-skymon/src/Main.sky");
}

/// Example 10 (live-component) — also triggered the constructor-as-callback
/// `ir_type_from_canon` path.
#[test]
fn ex10_live_component_exits_zero() {
    assert_skyc_exit0("ex10_live_component", "examples/10-live-component/src/Main.sky");
}

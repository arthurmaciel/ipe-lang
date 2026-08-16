//! Positive boundary test for the `RetryPolicy` exact-shape guard.
//!
//! A USER record with the EXACT five `RetryPolicy` field names
//! (`baseMs`, `jitter`, `kind`, `maxAttempts`, `shouldRetry`) and a function-
//! typed `shouldRetry : Int -> Bool` COMPILES AND RUNS correctly.  This is NOT
//! a bypass of IPE-L0107: the fn-value-reuse `Arc<dyn Fn>` carrier handles the
//! function field (same as any other record-fn field); the generic DERIVE carrier
//! is never involved, so L0107 does not apply.
//!
//! The test documents the intended boundary alongside the existing negative tests
//! (`retry_policy_nearmiss_still_rejects` / `retry_policy_shape_nearmiss_rejects`)
//! and replaces any over-stated comment that implied the exact-shape user record
//! was rejected.
//!
//! The emitted crate must contain exactly ONE struct for the five-field set
//! (the user's `Int -> Bool` predicate struct), not a second dead `_2`
//! `Arc<dyn Fn(IpeError) -> bool>` duplicate from the concretisation.  The
//! `non_camel_case_types` warning the duplicate drew is proof of the defect;
//! its absence here is the structural fix.

use std::path::{Path, PathBuf};

use crate::support::repo_root;

fn fixture_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("retry_policy_exact_shape_user_record")
        .join("Main.ipe")
}

/// The frontend must accept the exact-5-field user record with `shouldRetry :
/// Int -> Bool` — no IPE-L0107, no ICE.  This is the positive boundary: the
/// user record is sound (fn-value-reuse carrier, not derive carrier).
#[test]
fn retry_policy_exact_shape_user_record_emits() {
    let root = repo_root();
    let entry = fixture_entry(&root);
    let out = std::env::temp_dir().join("ipec_i979_exact_shape_emit");
    let _ = std::fs::remove_dir_all(&out);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "a user record with the exact five `RetryPolicy` field names and \
         `shouldRetry : Int -> Bool` must be accepted and emitted — the \
         fn-value-reuse carrier handles the function field, L0107 does not \
         apply; got: {built:?}"
    );
}

/// Under `IPE_E2E=1`: build and run the emitted crate.
/// `makePolicy 3 True` produces `{{ baseMs=100, jitter=True, kind=1,
///  maxAttempts=3, shouldRetry=\n -> n < 3 }}`.
/// `applyPolicy p 1` → `"retry"` (1 < 3 = true).
/// `applyPolicy p 5` → `"done"` (5 < 3 = false).
/// `p.maxAttempts` → `"3"`.
/// Expected output: `"retry,done,3"`.
#[test]
fn retry_policy_exact_shape_user_record_builds_and_runs() {
    let root = repo_root();
    let entry = fixture_entry(&root);
    let out = std::env::temp_dir().join("ipec_i979_exact_shape_e2e");
    let _ = std::fs::remove_dir_all(&out);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "retry_policy_exact_shape_user_record: must be accepted; got: {built:?}"
    );

    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let outcome =
        crate::support::build_and_run_emitted("retry_policy_exact_shape_user_record", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "emitted crate must build and exit 0; stdout:\n{}",
        outcome.stdout
    );
    assert_eq!(
        outcome.stdout.trim(),
        "retry,done,3",
        "wrong runtime output — `applyPolicy p 1` = retry, `applyPolicy p 5` = done, \
         `p.maxAttempts` = 3"
    );
}

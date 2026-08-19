//! Regression: a record literal with a function-typed field, constructed only
//! inside an untyped let binding with no typed function signature exposing the
//! record shape, previously triggered IPE-I0001 ("this is a bug in Ipe, please
//! report it") at struct synthesis time.
//!
//! Root cause: `collect_records_in_ty` skipped records whose lowered IR
//! contained a function-type field (`ir_contains_fun` gate), so the struct was
//! never registered in the synthesis table. When no typed function's
//! `func.ret` or `func.params` exposed the same shape — the only other path to
//! registration in the backend — `record_struct_by_key` found no entry and
//! raised IPE-I0001.
//!
//! Fix: `collect_records_in_ty` now registers ALL concrete record shapes,
//! including those with function-typed fields. The `Arc<dyn Fn>` carrier stores
//! such fields on a `Clone`-able slot; the backend synthesises a sound struct
//! with a hand-written `impl Clone`. No existing valid program is affected:
//! programs that expose the record shape in a typed signature were already
//! emitting correctly (the backend collected the struct from `func.ret`), and
//! the near-miss value-callee gate (`reject_value_callee_fn_into_carrier`) that
//! still gives IPE-L0107 for a function threaded through an unresolvable callee
//! into a record field is unaffected.

use std::path::{Path, PathBuf};

use ipe::CliError;

use crate::support::repo_root;

fn fixture(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("i1139_fn_field_record_no_signature")
        .join("Main.ipe")
}

/// The frontend must accept and emit the fixture without ICE (IPE-I0001) or
/// any spurious lower diagnostic. A record literal with a function field in an
/// untyped let binding is valid; the struct must be synthesised from the
/// region-map registration, not only from function signatures.
#[test]
fn fn_field_record_no_signature_emits() {
    let root = repo_root();
    let out = std::env::temp_dir().join("ipec_i1139_emit");
    let _ = std::fs::remove_dir_all(&out);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&fixture(&root), &out, &runtime);
    assert!(
        built.is_ok(),
        "a record literal with a function field in an untyped let binding must \
         compile without ICE (IPE-I0001); the struct must be registered from \
         the region map; got: {built:?}"
    );
}

/// Under `IPE_E2E=1`: the emitted crate must `cargo build` and print the
/// expected output. The fixture accesses both a scalar field (`maxAttempts`)
/// and calls a function field (`shouldRetry`), exercising both emitted struct
/// operations for a function-field record without a typed signature.
#[test]
fn fn_field_record_no_signature_builds_and_runs() {
    let root = repo_root();
    let out = std::env::temp_dir().join("ipec_i1139_e2e");
    let _ = std::fs::remove_dir_all(&out);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&fixture(&root), &out, &runtime);
    assert!(
        built.is_ok(),
        "fn_field_record_no_signature: must be accepted; got: {built:?}"
    );

    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let outcome = crate::support::build_and_run_emitted("i1139_fn_field_record_no_signature", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "emitted crate must build and exit 0; stdout:\n{}",
        outcome.stdout
    );
    // `policy.maxAttempts` = 3, `policy.shouldRetry policy.baseMs` = 100 + 1 = 101.
    assert_eq!(
        outcome.stdout.trim(),
        "3,101",
        "wrong runtime output — maxAttempts=3, shouldRetry(100)=101"
    );
}

/// The existing near-miss gate must be unaffected: a user record with a lone
/// `shouldRetry` field threaded through a value callee still gives IPE-L0107,
/// not a compiler acceptance or ICE. This guards against the registration
/// change weakening the value-callee fn-into-carrier gate.
#[test]
fn value_callee_fn_into_carrier_still_rejects_nearmiss() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("retry_policy_shape_nearmiss")
        .join("Main.ipe");
    let out = std::env::temp_dir().join("ipec_i1139_nearmiss_guard");
    let _ = std::fs::remove_dir_all(&out);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    let got = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        got,
        Some(ipe_diagnostics::IPE_L0107),
        "the near-miss value-callee gate must still give IPE-L0107 after the \
         struct-registration fix — that gate is about a function threaded \
         through an unresolvable callee into a carrier, not about direct \
         function-field record literals; got: {built:?}"
    );
}

/// The exact-5-field user record (the boundary case from the `RetryPolicy`
/// exemption guard) must still compile unaffected. This was already working
/// before the fix; the new registration path is additive and must not break it.
#[test]
fn exact_shape_user_record_unaffected() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("retry_policy_exact_shape_user_record")
        .join("Main.ipe");
    let out = std::env::temp_dir().join("ipec_i1139_exact_shape_guard");
    let _ = std::fs::remove_dir_all(&out);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "the exact-5-field user record with `shouldRetry : Int -> Bool` must \
         still compile — it was already handled correctly via the typed function \
         signature path, and the new region-map registration must not conflict; \
         got: {built:?}"
    );
}

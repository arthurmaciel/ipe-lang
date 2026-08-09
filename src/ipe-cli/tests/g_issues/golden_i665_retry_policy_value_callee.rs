//! Regression: `Task.retryOn` / `Task.withRetryOn` used in a pipe were
//! type-checked (ipe exit 0) but then rejected by `ipe build` with IPE-L0107
//! (storing a function value in a record field).
//!
//! `Task.retryOn : (e -> Bool) -> RetryPolicy e -> RetryPolicy e` returns a
//! `RetryPolicy e` — an anonymous record carrying a `shouldRetry` predicate
//! FUNCTION. A pipe (`Task.linearBackoff 5 1 |> Task.retryOn (\_ -> False)`)
//! lowers `retryOn` through the value-callee (`Expr::Apply`) arm, whose
//! fn-in-derive-carrier gate saw a fn-typed parameter plus a fn-carrying result
//! record and rejected the application. That gate lacked the `RetryPolicy`
//! exemption its sibling gates already carry: the `RetryPolicy` struct is
//! kernel-managed (`emit_task_retry_call` emits `shouldRetry` as a
//! `Box<dyn Fn>` field and skips the `Clone`/`PartialEq` derives), so it is a
//! sound end-to-end path, not a generic derive carrier. The gate now exempts
//! exactly this shape; every other function-in-record result still fails
//! closed.
//!
//! This golden is the end-to-end SEAL lock: the emitted crate must `cargo
//! build` and print the retried Task's value `x`.

use std::path::{Path, PathBuf};

use crate::support::repo_root;

fn fixture_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("retry_policy_value_callee")
        .join("Main.ipe")
}

/// Build the fixture; return whether the frontend accepted + emitted it. `None`
/// when the runtime resolver is unavailable in this environment (mirrors the
/// resolve-skip convention every other golden in this suite uses).
fn built(root: &Path, out: &Path) -> Option<Result<(), ipe::CliError>> {
    let entry = fixture_entry(root);
    let _ = std::fs::remove_dir_all(out);
    let runtime = ipe::resolve_runtime().ok()?;
    Some(ipe::build(&entry, out, &runtime))
}

/// Emit assertion (default gate): the frontend must accept the piped
/// `retryOn` program and emit its crate — the exit-0-then-cargo-fail this
/// regression closed was invisible here (the failure was at emit/`ipe build`,
/// not at type-check), so an accept alone is not enough; see the SEAL test.
#[test]
fn retry_policy_value_callee_emits() {
    let root = repo_root();
    let out = std::env::temp_dir().join("ipec_retry_policy_value_callee_emit");
    let Some(built) = built(&root, &out) else {
        return; // resolver unavailable — skip, matches the other goldens
    };
    assert!(
        built.is_ok(),
        "retry_policy_value_callee: piped `Task.retryOn` must be accepted + \
         emitted (the value-callee fn-carrier gate exempts the `RetryPolicy` \
         `shouldRetry` record), got: {built:?}"
    );
}

/// The load-bearing SEAL proof: under `IPE_E2E=1`, actually `cargo build` the
/// emitted crate and run it. The `RetryPolicy` struct's `shouldRetry:
/// Box<dyn Fn>` field must emit soundly; the crate builds and prints `x` (the
/// value of the succeeding, non-retried Task).
#[test]
fn retry_policy_value_callee_builds_and_runs() {
    let root = repo_root();
    let out = std::env::temp_dir().join("ipec_retry_policy_value_callee_e2e");
    let Some(built) = built(&root, &out) else {
        return;
    };
    assert!(
        built.is_ok(),
        "retry_policy_value_callee: must be accepted, got: {built:?}"
    );

    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    let outcome = crate::support::build_and_run_emitted("retry_policy_value_callee", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "retry_policy_value_callee: emitted crate must build and exit 0 (the \
         `RetryPolicy` `shouldRetry` fn field emits as a `Box<dyn Fn>` on a \
         kernel-managed struct); stdout:\n{}",
        outcome.stdout
    );
    assert_eq!(
        outcome.stdout.trim(),
        "x",
        "wrong runtime output — the succeeding Task carries the value `x`"
    );
}

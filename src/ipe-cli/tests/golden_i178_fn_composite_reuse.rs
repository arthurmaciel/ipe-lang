//! Fn-value-reuse COMPOSITE promotion — relaxing the composite side of the
//! fail-closed IPE-L0127 / IPE-L0107 gates for a reused record-of-functions.
//!
//! A closed anon record whose fields include function values, reused as a value
//! (each field read is a non-call use of the whole record), is promoted by
//! flipping its function slots from the `Box<dyn Fn>` carrier to the `Clone`
//! `Arc<dyn Fn>` carrier ([`ipe_ir::IrType::SharedFun`]). The whole struct then
//! derives a hand-written `Clone`, so the N-1 last-use clone rewrite makes it
//! move-safe. The promotion fires ONLY under a whole-value containment +
//! whole-clonable precondition; every shape outside it stays fail-closed.
//!
//! | Fixture | Shape | Outcome |
//! |---|---|---|
//! | `fn_record_reuse_promoted` | record of two functions + data, reused | promoted; builds + prints `len: 8` |
//! | `fn_record_reuse_escapes` | that record RETURNED from a function | fail-closed IPE-L0107 (not contained) |
//! | `fn_record_reuse_mixed` | fn field + `Task` field, reused | fail-closed IPE-L0127 (not whole-clonable) |
//!
//! A `Send`-not-`Sync` capture control is covered by the carrier-transparency
//! argument, not a distinct fixture: the emitted `Box`/`Arc` carrier already
//! bounds every captured free variable `Send + Sync + 'static`, so a
//! `Send`-not-`Sync` capture is rejected by rustc on the closure upstream of the
//! carrier choice, identically before and after the `Box`->`Arc` swap.

use std::path::{Path, PathBuf};

mod support;

use support::repo_root;

fn entry_of(root: &Path, name: &str) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join(name)
        .join("Main.ipe")
}

fn assert_byte_identical(name: &str) {
    let root = repo_root();
    let entry = entry_of(&root, name);
    let golden = root.join("tests").join("golden").join(name).join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_emit"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    support::assert_emitted_project_matches_golden_dir(&out, support::golden_dir_of(&golden));
}

fn assert_e2e_prints(name: &str, want_stdout: &str) {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    let root = repo_root();
    let entry = entry_of(&root, name);
    let out = std::env::temp_dir().join(format!("ipec_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let outcome = support::build_and_run_emitted(name, &out);
    assert_eq!(
        outcome.stdout, want_stdout,
        "promoted record prints its value"
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0 (THE SEAL)");
}

/// Build the named fixture and assert it fails closed with exactly `expected` —
/// a typed `ipe`-time diagnostic, never a panic and never an accept that would
/// defer to a raw rustc error.
fn assert_fail_closed(name: &str, expected: ipe_diagnostics::Code) {
    let root = repo_root();
    let entry = entry_of(&root, name);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_gate"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    let got = match &built {
        Err(ipe::CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        got,
        Some(expected),
        "fixture {name}: expected {expected:?}, got build result {built:?}"
    );
}

#[test]
fn record_of_functions_reuse_emits_byte_identical_main_rs() {
    assert_byte_identical("fn_record_reuse_promoted");
}

#[test]
fn record_of_functions_reuse_end_to_end() {
    assert_e2e_prints("fn_record_reuse_promoted", "len: 8\n");
}

#[test]
fn escaping_record_of_functions_stays_fail_closed() {
    // The record is returned from `mk`, so it is not contained; the escaping
    // literal is rejected as a first-class-function record field (IPE-L0107).
    assert_fail_closed("fn_record_reuse_escapes", ipe_diagnostics::IPE_L0107);
}

#[test]
fn mixed_composite_reuse_stays_fail_closed() {
    // The `Task` field keeps the record non-`Clone` even after the fn-slot flip,
    // so reuse cannot be made move-safe: fail closed (IPE-L0127).
    assert_fail_closed("fn_record_reuse_mixed", ipe_diagnostics::IPE_L0127);
}

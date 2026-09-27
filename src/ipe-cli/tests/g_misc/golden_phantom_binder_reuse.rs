//! Phantom-typed binder reuse gate: a `let` binder whose type keeps a variable
//! the solver left unconstrained, read twice.
//!
//! `n = Nothing` leaves the `Maybe` element phantom; `e = Result.mapError
//! String.length (Err "boom")` leaves the `Ok` payload phantom. The ownership
//! disciplines classify each through a defaulted stand-in that never reaches
//! emission. `ipe` must emit `main.rs` byte-identical to each checked-in
//! golden, and (behind `IPE_E2E=1`) each emitted project must build and print
//! its `expected.txt`.
//!
//! The producer-pin fixtures cover a phantom-born value in each position a
//! consumer cannot fix its type from: `Nothing` as a direct argument, a bare
//! `Err` scrutinee with a free `Ok` type, a nullary constructor of a
//! parameterised user type, and an empty list read twice through a generic
//! function. Each must be accepted by `ipe` and (behind `IPE_E2E=1`) build and
//! print its `expected.txt`.
//!
//! The mixed-position fixture sends one free variable to a `Maybe` element and
//! a `Result` error slot, and binds a `Result a a`: every slot takes the one
//! phantom default, so the emitted project must build.
use std::path::{Path, PathBuf};

use crate::support::repo_root;

const NOTHING_GOLDEN: &str = "phantom_nothing_binder_reuse";
const MAP_ERROR_GOLDEN: &str = "phantom_map_error_binder_reuse";
const NOTHING_ARG_FIXTURE: &str = "phantom_nothing_direct_arg";
const ERR_FREE_OK_FIXTURE: &str = "phantom_err_free_ok";
const USER_ENUM_NULLARY_FIXTURE: &str = "phantom_user_enum_nullary";
const EMPTY_LIST_REUSE_FIXTURE: &str = "phantom_empty_list_generic_reuse";
const MIXED_POSITION_FIXTURE: &str = "phantom_mixed_position";

fn golden_dir(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name)
}

/// Emit `name`'s `Main.ipe` and byte-compare it against the golden dir.
fn assert_emits_golden(name: &str) {
    let root = repo_root();
    let dir = golden_dir(&root, name);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_emit"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = ipe::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    let built = ipe::build(&dir.join("Main.ipe"), &out, &runtime);
    assert!(built.is_ok(), "{name}: build failed: {:?}", built.err());

    crate::support::assert_emitted_project_matches_golden_dir(&out, &dir);
}

/// `ipe` accepts `name`'s `Main.ipe` and emits a project.
fn assert_accepts(name: &str) {
    let root = repo_root();
    let dir = golden_dir(&root, name);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_accept"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = ipe::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    let built = ipe::build(&dir.join("Main.ipe"), &out, &runtime);
    assert!(built.is_ok(), "{name}: build failed: {:?}", built.err());
    assert!(
        out.join("src").join("main.rs").is_file(),
        "{name}: no main.rs emitted"
    );
}

/// Build and run `name`'s emitted project and check its stdout, behind `IPE_E2E=1`.
fn assert_runs_golden(name: &str) {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let dir = golden_dir(&root, name);
    let out = crate::support::scratch_root().join(format!("ipec_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = ipe::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = ipe::build(&dir.join("Main.ipe"), &out, &runtime);
    assert!(built.is_ok(), "{name}: build failed: {:?}", built.err());

    let outcome = crate::support::build_and_run_emitted(name, &out);
    crate::support::assert_go_parity(name, &dir, &outcome.stdout);
    assert_eq!(outcome.exit_code, Some(0), "{name}: exit 0");
}

#[test]
fn nothing_binder_emits_byte_identical_main_rs() {
    assert_emits_golden(NOTHING_GOLDEN);
}

#[test]
fn nothing_binder_end_to_end_prints_none_twice() {
    assert_runs_golden(NOTHING_GOLDEN);
}

#[test]
fn map_error_binder_emits_byte_identical_main_rs() {
    assert_emits_golden(MAP_ERROR_GOLDEN);
}

#[test]
fn map_error_binder_end_to_end_prints_err_twice() {
    assert_runs_golden(MAP_ERROR_GOLDEN);
}

#[test]
fn nothing_direct_arg_is_accepted() {
    assert_accepts(NOTHING_ARG_FIXTURE);
}

#[test]
fn nothing_direct_arg_end_to_end_prints_none() {
    assert_runs_golden(NOTHING_ARG_FIXTURE);
}

#[test]
fn err_free_ok_scrutinee_is_accepted() {
    assert_accepts(ERR_FREE_OK_FIXTURE);
}

#[test]
fn err_free_ok_scrutinee_end_to_end_prints_err() {
    assert_runs_golden(ERR_FREE_OK_FIXTURE);
}

#[test]
fn user_enum_nullary_arg_is_accepted() {
    assert_accepts(USER_ENUM_NULLARY_FIXTURE);
}

#[test]
fn user_enum_nullary_arg_end_to_end_prints_leaf() {
    assert_runs_golden(USER_ENUM_NULLARY_FIXTURE);
}

#[test]
fn empty_list_generic_reuse_is_accepted() {
    assert_accepts(EMPTY_LIST_REUSE_FIXTURE);
}

#[test]
fn empty_list_generic_reuse_end_to_end_prints_zero() {
    assert_runs_golden(EMPTY_LIST_REUSE_FIXTURE);
}

#[test]
fn mixed_position_phantom_is_accepted() {
    assert_accepts(MIXED_POSITION_FIXTURE);
}

#[test]
fn mixed_position_phantom_end_to_end_prints_one_carrier() {
    assert_runs_golden(MIXED_POSITION_FIXTURE);
}

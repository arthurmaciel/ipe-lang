//! `Store.upsert` guard: the conflict target comes only from the declared key.
//!
//! * `db_store_upsert_guard` — a store with no primary key and no unique
//!   column, with several unique columns and no primary key, keyed on a
//!   `serial` column, or with a refused key declaration is a typed `Err` before
//!   any SQL; a composite-key upsert updates the conflicting row in place, a
//!   `defaultNow` column never takes the client's value, and a unique-only store
//!   upserts on its unique column. Emit gate always; run + `expected.txt` under
//!   `IPE_E2E=1`.

use std::path::{Path, PathBuf};

use crate::support::repo_root;

fn fixture_dir(root: &Path, golden: &str) -> PathBuf {
    root.join("tests").join("golden").join(golden)
}

/// Emit gate + THE SEAL for `Store.upsert`.
///
/// The frontend accepts every upsert call, and under `IPE_E2E=1` the emitted
/// crate builds and prints one `:ok` line per refusal/acceptance check.
#[test]
fn db_store_upsert_guard() {
    const GOLDEN: &str = "db_store_upsert_guard";
    let root = repo_root();
    let dir = fixture_dir(&root, GOLDEN);
    let out = std::env::temp_dir().join("ipec_db_store_upsert_guard");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&dir.join("Main.ipe"), &out, &runtime);
    assert!(built.is_ok(), "{GOLDEN} must be accepted, got: {built:?}");

    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    let outcome = crate::support::build_and_run_emitted(GOLDEN, &out);
    crate::support::assert_go_parity(GOLDEN, &dir, &outcome.stdout);
    assert_eq!(outcome.exit_code, Some(0), "{GOLDEN}: exit 0");
}

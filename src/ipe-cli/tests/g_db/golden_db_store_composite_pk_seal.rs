//! Primary-key declaration guard for `Ipe.Db.Store` — the one-key-per-table
//! and at-least-two-columns-per-composite-key invariants, and the accessor tie
//! of `Store.compositePrimaryKey2` / `Store.compositePrimaryKey3`.
//!
//! * `db_store_composite_pk_guard` — every refused declaration (a second key in
//!   any order/form, a one-column or empty composite key) is a typed `Err` from
//!   `createSql` / `migrations` / the by-key operations; each composite form
//!   records its columns in declared order; a single-column key's DDL is
//!   unchanged. Emit gate always; run + `expected.txt` under `IPE_E2E=1`.
//! * `db_store_composite_pk_unknown_field_rejected` — a composite-key accessor
//!   naming a field the row does not declare is an ipe-time TYPE error.

use std::path::{Path, PathBuf};

use ipe::CliError;

use crate::support::repo_root;

fn fixture_dir(root: &Path, golden: &str) -> PathBuf {
    root.join("tests").join("golden").join(golden)
}

/// Emit gate + THE SEAL: the frontend accepts every primary-key builder form
/// (the accessor intercepts for `compositePrimaryKey2` / `compositePrimaryKey3`
/// included), and under `IPE_E2E=1` the emitted crate builds and prints one
/// `:ok` line per refusal/acceptance check.
#[test]
fn db_store_composite_pk_guard() {
    const GOLDEN: &str = "db_store_composite_pk_guard";
    let root = repo_root();
    let dir = fixture_dir(&root, GOLDEN);
    let out = std::env::temp_dir().join("ipec_db_store_composite_pk_guard");
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

/// A composite-key accessor naming a field absent from the row type MUST be
/// rejected at ipe time as a type error — the accessor tie is what keeps an
/// unchecked column name out of the key.
#[test]
fn db_store_composite_pk_unknown_field_rejected() {
    const GOLDEN: &str = "db_store_composite_pk_unknown_field_rejected";
    let root = repo_root();
    let entry = fixture_dir(&root, GOLDEN).join("Main.ipe");
    let out = std::env::temp_dir().join("ipec_db_store_composite_pk_unknown_field_rejected");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    let code = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code().as_str().to_owned()),
        _ => None,
    };
    assert!(
        code.as_deref().is_some_and(|c| c.starts_with("IPE-T")),
        "`Store.compositePrimaryKey2 .userId .tenantId` on a row without \
         `tenantId` MUST be rejected with a type error (IPE-T…); got: {built:?}"
    );
}

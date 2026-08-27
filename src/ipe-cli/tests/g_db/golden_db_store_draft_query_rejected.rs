//! Deny-by-default: querying an UNCLASSIFIED `Draft` must be an ipe-time type
//! error — never accepted.
//!
//! `Store.fromCodec` returns a `Draft a` (a table whose schema is known but
//! whose access intent is not declared). The read and write operations
//! (`all` / `get` / `insert` / …) accept only a classified `Store a`, reachable
//! solely through `Store.public` / `Store.secured`. Passing a `Draft` straight
//! to `Store.all` — the "forgot to classify this table" mistake — is a
//! `TYPE MISMATCH` (IPE-T0001): the unqueryable-by-construction guarantee that
//! makes the unsecured-by-accident state unrepresentable.

use std::path::{Path, PathBuf};

use crate::support::repo_root;

fn fixture_entry(root: &Path, golden: &str) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join(golden)
        .join("Main.ipe")
}

/// A read (`Store.all`) applied to an unclassified `Draft` MUST be rejected at
/// ipe time — the only path to a queryable `Store a` is `public` / `secured`.
#[test]
fn draft_query_without_classification_is_rejected() {
    const GOLDEN: &str = "db_store_draft_query_rejected";
    let root = repo_root();
    let entry = fixture_entry(&root, GOLDEN);
    let out = std::env::temp_dir().join("ipec_db_store_draft_query_rejected");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return; // resolver unavailable — skip
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_err(),
        "querying an unclassified `Draft` with `Store.all` MUST be rejected — a \
         table is unqueryable until classified with `Store.public` or \
         `Store.secured` (deny-by-default)"
    );
}

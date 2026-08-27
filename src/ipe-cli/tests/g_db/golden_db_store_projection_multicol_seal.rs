//! End-to-end SEAL for the typed multi-column projection over a two-store inner
//! join (`Store.join` / `Store.select` / `Store.selectToList`). Two
//! `Codec.auto`-derived stores join on `books.author_id = authors.id`; `select`
//! projects TWO columns as a flat tuple of references
//! (`( book.title, author.name )`), so the join lowers to ONE parameterized
//! statement that selects exactly those two columns
//! (`SELECT a0.title AS p0, a1.name AS p1 FROM …`, column pushdown) over the
//! two-table `FROM`. Each projected row comes back as a `Row` keyed by the output
//! names `p0` / `p1`, decoded caller-side by position; every projected identifier
//! is re-validated at the runtime boundary, and no filter value is interpolated.
//!
//! The decode is concrete/monomorphized: the projection value carries only an
//! ordered `(alias, column)` list (a plain `Vec<(String, String)>`), never a
//! boxed decoder — the caller reads each `p<index>` cell with its own typed
//! `projRead*`. So the multi-column path introduces no `Box<dyn Fn …>` decoder.
//!
//! THE SEAL: `ipe` accepting the projected program (exit 0) must imply a
//! buildable emitted crate. Under `IPE_E2E=1` the emitted crate must
//! `cargo build` (no live database or network is needed — the projection is
//! assembled, never executed).

use std::path::{Path, PathBuf};

use crate::support::repo_root;

const GOLDEN: &str = "db_store_projection_multicol_seal";

fn fixture_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join(GOLDEN)
        .join("Main.ipe")
}

/// Emit gate: the frontend must accept the multi-column `Store.select` /
/// `Store.selectToList` program (the tuple projection lowers to the ordered
/// `(alias, column)` list, and the `Db.findProjection` kernel schemes, lowers,
/// and emits).
#[test]
fn db_store_projection_multicol_emits() {
    let root = repo_root();
    let entry = fixture_entry(&root);
    let out = std::env::temp_dir().join("ipec_db_store_projection_multicol_seal_emit");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return; // resolver unavailable — skip
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "a `Store.select` multi-column projection run through `selectToList` \
         must be accepted and emitted, got: {built:?}"
    );
}

/// THE SEAL: under `IPE_E2E=1`, actually `cargo build` the emitted crate — the
/// multi-column `SELECT a0.title AS p0, a1.name AS p1` projection over the
/// two-table `FROM` and the positional `p0` / `p1` row decode must all compile.
/// The emitted projection value carries a plain `(alias, column)` list and no
/// boxed decoder, so no `dyn`-decoder obligation ever reaches the emitter.
#[test]
fn db_store_projection_multicol_seal_builds() {
    let root = repo_root();
    let entry = fixture_entry(&root);
    let out = std::env::temp_dir().join("ipec_db_store_projection_multicol_seal_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "{GOLDEN} must be accepted, got: {built:?}");

    crate::support::assert_seal_builds(GOLDEN, &out);

    // Witness the concrete/monomorphized decode: the emitted `main`-module source
    // constructs the exact ordered two-column projection list, and neither the
    // projection value nor the `select_to_list` path stores a boxed decoder for
    // it. (Framework machinery elsewhere — codecs, `Task` — may use `dyn Fn`; the
    // claim is scoped to the projection's own decode.)
    let main_mod = out.join("src").join("ipe_mods").join("ipe_mod_main.rs");
    let src = std::fs::read_to_string(&main_mod).expect("emitted main module must exist");
    assert!(
        src.contains("\"title\".to_string()") && src.contains("\"name\".to_string()"),
        "the emitted projection must name both projected columns as plain data"
    );
    // The projection is built as a Vec of (alias, column) string pairs — a data
    // list, not a decoder closure.
    assert!(
        src.contains("user_ipe_db_store_select_named"),
        "the projection must lower to the `selectNamed` data constructor"
    );
    assert!(
        !src.contains("Box<dyn Fn") && !src.contains("dyn Fn"),
        "the multi-column projection's `main`-module emit must carry no boxed \
         decoder — the decode is concrete/monomorphized and caller-side"
    );
}

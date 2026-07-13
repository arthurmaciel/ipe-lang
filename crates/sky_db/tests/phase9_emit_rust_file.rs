#![forbid(unsafe_code)]
//! Milestone D proofs (spec:
//! `docs/architecture/phase5-emit-rust-file-design-2026-07-12.md` §4 —
//! Tasks 14/15/16).
//!
//! Milestone D wires the per-`RustFileId` salsa query graph on top of the
//! coarse `lower_program` floor. It changes NO emitted bytes — it makes the
//! COMPILER's own re-derivation of each Rust file's content salsa-tracked and
//! memoized, so an unaffected module's text is not re-rendered when an
//! unrelated module is edited (§3.4).
//!
//! Task 14 (this section): [`sky_db::RustFileId`] is a genuine
//! `#[salsa::interned]` key — distinct from Task 1's non-interned
//! `sky_backend_rust::RustFileId` (§4.1). The rest of the file (Tasks 15/16)
//! proves the tracked queries built on it.

use sky_db::{Db as _, RustFileId, SkyDatabase};
use sky_ir::ModPath;

/// Intern a `ModPath` through the database's shared interner — the same table
/// every salsa query resolves symbols against, so a `ModPath` built here is
/// comparable to one the real lowering path produces.
///
/// `expect` is sanctioned here: this is a test helper, and interning a short
/// ASCII segment cannot fail on a fresh interner (`clippy.toml`'s
/// `allow-expect-in-tests` covers `#[test]` fns but not a free helper, so the
/// allow is explicit).
#[allow(clippy::expect_used)]
fn mod_path(db: &SkyDatabase, segs: &[&str]) -> ModPath {
    let mut interner = db.interner().lock();
    ModPath(
        segs.iter()
            .map(|s| interner.intern(s).expect("intern must succeed for a test segment"))
            .collect(),
    )
}

#[test]
fn rust_file_id_interns_distinct_homes_distinctly() {
    let db = SkyDatabase::new();
    let lib = mod_path(&db, &["Lib"]);
    let main = mod_path(&db, &["Main"]);

    let id_lib = RustFileId::new(&db, lib.clone());
    let id_main = RustFileId::new(&db, main);

    assert_ne!(
        id_lib, id_main,
        "distinct homes must intern to distinct RustFileId keys"
    );

    // Interning the SAME home twice returns the SAME salsa id (interning
    // dedups by value — the standard salsa-interned-key smoke test).
    let id_lib_again = RustFileId::new(&db, lib);
    assert_eq!(
        id_lib, id_lib_again,
        "interning the same home twice must return the same salsa id"
    );

    // The stored field round-trips.
    assert_eq!(id_lib.home(&db), mod_path(&db, &["Lib"]));
}

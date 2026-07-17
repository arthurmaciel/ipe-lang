#![forbid(unsafe_code)]
//! Smoke: the database constructs, is a `salsa::Database`, and drops cleanly.

use ipe_db::SkyDatabase;

const fn assert_is_salsa_db<D: salsa::Database>(_db: &D) {}

#[test]
fn db_smoke() {
    let db = SkyDatabase::new();
    assert_is_salsa_db(&db);
    let cloned = db.clone();
    drop(db);
    drop(cloned);
}

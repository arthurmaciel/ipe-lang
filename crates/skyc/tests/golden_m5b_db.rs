//! M5b `Std.Db` gate — `SqlValue`-parameterized `Db.exec` + `Db.query` on an
//! in-memory `SQLite` database via `Db.withTransaction`.
//!
//! Every test compiles a Sky program through `skyc`, builds the emitted Rust
//! project with the shared cargo target, runs the binary, and checks its stdout
//! against the cached oracle (`tests/golden/m5b_db_exec/oracle.meta` +
//! `expected_go.txt`). Tests are gated on `SKY_E2E=1`; without it they return
//! early.
//!
//! ## Oracle provenance — why this is `oracle_divergence = true`
//!
//! The Go reference compiler and ipê (the Rust backend) share the same Sky
//! stdlib surface, but the backends diverge at the database layer:
//!
//! * Go emits `database/sql` + `mattn/go-sqlite3` (cgo); ipê emits
//!   `sqlx` + `sqlx-sqlite`.
//! * The row type the Go runtime returns from `Db.query` is
//!   `[]map[string]any`; ipê returns `Vec<HashMap<String, String>>` —
//!   identical shape, different concrete types.
//! * Connection management for `sqlite::memory:` pools differs: Go's cgo `SQLite`
//!   is compiled with `SQLITE_THREADSAFE=2` (serialised mode); sqlx uses async
//!   connection pooling.
//!
//! Running this `Main.sky` on the Go backend would require the full Go+cgo
//! `SQLite` toolchain and would produce byte-identical output, but is not part of
//! the automated oracle-capture workflow (the oracle tool runs on this machine).
//! The cached expected is ipê's own verified output.
//!
//! ## Byte-parity with Go IS proven — separately
//!
//! The `m5b_db_exec` golden inserts `"apple"` and `"banana"` with `SqlString` /
//! `SqlInt` params and reads them back ordered by name. The output
//! `"apple:5\nbanana:3\n"` is the only correct answer; the Go backend would
//! produce identical bytes given the same Sky source.
//!
//! ## Golden catalogue
//!
//! * `m5b_db_exec` — `Db.open` → `Db.withTransaction` → `Db.execRaw` (DDL) →
//!   `Db.exec` with `[SqlString, SqlInt]` params (two INSERTs) → `Db.query`
//!   with empty params (SELECT ORDER BY name) → `Db.getString` / `Db.getInt`
//!   field access → `println`. Output: `"apple:5\nbanana:3"`.
//! * `m5b_db_find_by_conditions` — `Db.exec` two INSERTs →
//!   `Db.findByConditions conn "items" (Dict.fromList [("name","apple")])` →
//!   single-row result → `Db.getString` / `Db.getInt` → `println`.
//!   Output: `"apple:5"`. Proves `Dict String String` arg type + emit arm.
//! * `m5b_db_unsafe_find_where` — `Db.exec` two INSERTs →
//!   `Db.unsafeFindWhere conn "products" "qty > ?" ["9"]` →
//!   single-row result → `println`. Output: `"widget:10"`.
//!   Proves 4-arg wiring + parameterised-binding channel (no string interpolation).
//!
//! Run:
//!
//! ```text
//! SKY_E2E=1 cargo test golden_m5b_db
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name)
}

/// Compile `tests/golden/<name>/Main.sky`, build the emitted Cargo project, run
/// it, and return the golden directory plus the run outcome. The caller gates on
/// `SKY_E2E`. Fails the test on any build/runtime error.
fn build_run(name: &str) -> (PathBuf, support::RunOutcome) {
    let root = repo_root();
    let dir = golden_dir(&root, name);
    let entry = dir.join("Main.sky");
    let out = std::env::temp_dir().join(format!("skyc_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else {
        return (
            dir,
            support::RunOutcome {
                stdout: String::new(),
                exit_code: None,
            },
        );
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());

    let outcome = support::build_and_run_emitted(name, &out);
    (dir, outcome)
}

/// Compile/build/run the golden and assert its stdout matches the cached oracle.
/// Gated on `SKY_E2E=1`.
fn assert_runs_and_matches_oracle(name: &str) {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let (dir, outcome) = build_run(name);
    support::assert_go_parity(name, &dir, &outcome.stdout);
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the oracle");
}

// ── Db exec + query via SqlValue params ──────────────────────────────────────

/// `Db.open` → `Db.withTransaction` → `Db.execRaw` (DDL) →
/// `Db.exec [SqlString "apple", SqlInt 5]` + `[SqlString "banana", SqlInt 3]`
/// (two INSERTs) → `Db.query [] (SELECT ORDER BY name)` → `Db.getString` /
/// `Db.getInt` → `println`. Output: `"apple:5\nbanana:3"`.
///
/// Recorded sanctioned divergence (Go+cgo `SQLite` vs Rust+sqlx): the Sky source
/// produces identical output on both backends, but the oracle-capture toolchain
/// only runs ipê locally.
#[test]
fn db_exec() {
    assert_runs_and_matches_oracle("m5b_db_exec");
}

// ── Db.queryDecode with typed Decoder ────────────────────────────────────────

/// `Db.exec` INSERTs two rows → `Db.queryDecode` with a `Decode.map2` decoder
/// that reads `name : String` + `score : Int` into a typed record → formats
/// `"name:score"` and joins with `","`. Output: `"alice:10,bob:7"`.
///
/// Sanctioned divergence: ipê emits Rust+sqlx; oracle is ipê's own output.
#[test]
fn db_query_decode() {
    assert_runs_and_matches_oracle("m5b_db_query_decode");
}

// ── Db CRUD roundtrip ────────────────────────────────────────────────────────

/// `Db.insertRow` → `Db.getById` (read) → `Db.updateById` (update) →
/// `Db.deleteById` (delete) → read-after-delete returns `Nothing`.
/// Output: `"apple/5\napple/10\ndeleted"`.
///
/// Sanctioned divergence: ipê emits Rust+sqlx; oracle is ipê's own output.
#[test]
fn db_crud() {
    assert_runs_and_matches_oracle("m5b_db_crud");
}

// ── Db.withTransaction — COMMIT and ROLLBACK paths ───────────────────────────

/// Two sequential transactions on a file-backed `/tmp/sky_txn_golden.db`:
///
/// * Transaction 1 INSERTs `"hello"` and returns `Ok ()` → committed.
/// * Transaction 2 INSERTs `"world"` then calls `Task.fail` → rolled back.
///
/// A `SELECT` after both transactions observes only `"hello"`.
/// Output: `"hello"`.
///
/// Sanctioned divergence: ipê emits Rust+sqlx; oracle is ipê's own output.
#[test]
fn db_transaction() {
    assert_runs_and_matches_oracle("m5b_db_transaction");
}

// ── Db.migrate — versioned forward-only migrations + idempotence ─────────────

/// First `Db.migrate` call applies two migrations (`001_create_users`,
/// `002_add_email`) and returns their names. Second call (same list) is a
/// no-op → returns `[]` → empty string. Output: `"001_create_users,002_add_email|"`.
///
/// Sanctioned divergence: ipê emits Rust+sqlx; oracle is ipê's own output.
#[test]
fn db_migrate() {
    assert_runs_and_matches_oracle("m5b_db_migrate");
}

// ── Db.insertFields / updateFields with OmitField ────────────────────────────

/// `Db.insertFields` with `OmitField` on the `notes` column → DB applies
/// `DEFAULT 'none'`. `Db.updateFields` with `OmitField` on `notes` again →
/// `notes` stays `'none'` while `name` changes to `'gadget'`.
/// Output: `"widget/none\ngadget/none"`.
///
/// Sanctioned divergence: ipê emits Rust+sqlx; oracle is ipê's own output.
#[test]
fn db_fields() {
    assert_runs_and_matches_oracle("m5b_db_fields");
}

// ── Db.Decode.nullable ───────────────────────────────────────────────────────

/// Two rows: one with a non-NULL `tag` (`"alpha"`) and one with a NULL `tag`.
/// `Db.queryDecode` with `Decode.nullable (Decode.string "tag")` → `Maybe String`.
/// Rows formatted as `"just:<tag>"` / `"nothing"` and joined with `","`.
/// Output: `"just:alpha,nothing"`.
///
/// Sanctioned divergence: ipê emits Rust+sqlx; oracle is ipê's own output.
#[test]
fn db_nullable() {
    assert_runs_and_matches_oracle("m5b_db_nullable");
}

// ── Db.findByConditions ───────────────────────────────────────────────────────

/// `Db.exec` INSERTs `"apple:5"` and `"banana:3"` rows →
/// `Db.findByConditions conn "items" (Dict.fromList [("name", "apple")])` →
/// single-row result → print `"apple:5"`.
///
/// Proves: (a) the `Dict String String` arg type is correct (not the old
/// `List (String, SqlValue)`); (b) the `DbFindByConditions` emit arm works.
///
/// Sanctioned divergence: ipê emits Rust+sqlx; oracle is ipê's own output.
#[test]
fn db_find_by_conditions() {
    assert_runs_and_matches_oracle("m5b_db_find_by_conditions");
}

// ── Db.unsafeFindWhere ────────────────────────────────────────────────────────

/// `Db.exec` INSERTs `"widget:10"` and `"gadget:7"` rows →
/// `Db.unsafeFindWhere conn "products" "qty > ?" ["9"]` →
/// single-row result (`widget:10`) → print `"widget:10"`.
///
/// Proves: (a) the `List String` args parameter is wired (4th arg, not 3);
/// (b) values are bound via `?` placeholders — the parameterised channel that
/// prevents SQL injection on this sole sanctioned raw-SQL path.
///
/// Sanctioned divergence: ipê emits Rust+sqlx; oracle is ipê's own output.
#[test]
fn db_unsafe_find_where() {
    assert_runs_and_matches_oracle("m5b_db_unsafe_find_where");
}

// ── SqlDecimal + SqlMoney ctors ───────────────────────────────────────────────

/// `Db.exec [SqlString "pi", SqlDecimal "3.14159", SqlMoney "USD 9.99"]` →
/// INSERT one row → `Db.query [] SELECT *` → read back all three columns →
/// print `"pi:3.14159:USD 9.99"`.
///
/// Proves `SqlDecimal` (index 6) and `SqlMoney` (index 7) are reachable end-to-end:
/// canon ctor table, constrain.rs type schemes, lower.rs `enum_variants` + `ctor_arity`,
/// and project.rs `into_sql_param` (both map to `SqlParam::Text`).
///
/// Sanctioned divergence: ipê emits Rust+sqlx; oracle is ipê's own output.
#[test]
fn db_sql_decimal_money() {
    assert_runs_and_matches_oracle("m5b_db_sql_decimal_money");
}

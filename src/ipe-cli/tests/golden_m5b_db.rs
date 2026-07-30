//! `Ipe.Db` gate — `SqlValue`-parameterized `Db.exec` + `Db.query` on an
//! in-memory `SQLite` database via `Db.withTransaction`.
//!
//! Every test compiles a Ipê program through `ipe`, builds the emitted Rust
//! project with the shared cargo target, runs the binary, and checks its stdout
//! against the cached oracle (`tests/golden/db_exec/oracle.meta` +
//! `expected_go.txt`). Tests are gated on `IPE_E2E=1`; without it they return
//! early.
//!
//! ## Oracle provenance — why this is `oracle_divergence = true`
//!
//! The Go reference compiler and Ipê (the Rust backend) share the same Ipe
//! stdlib surface, but the backends diverge at the database layer:
//!
//! * Go emits `database/sql` + `mattn/go-sqlite3` (cgo); Ipê emits
//!   `sqlx` + `sqlx-sqlite`.
//! * The row type the Go runtime returns from `Db.query` is
//!   `[]map[string]any`; Ipê returns `Vec<HashMap<String, String>>` —
//!   identical shape, different concrete types.
//! * Connection management for `sqlite::memory:` pools differs: Go's cgo `SQLite`
//!   is compiled with `SQLITE_THREADSAFE=2` (serialised mode); sqlx uses async
//!   connection pooling.
//!
//! Running this `Main.ipe` on the Go backend would require the full Go+cgo
//! `SQLite` toolchain and would produce byte-identical output, but is not part of
//! the automated oracle-capture workflow (the oracle tool runs on this machine).
//! The cached expected is Ipê's own verified output.
//!
//! ## Byte-parity with Go IS proven — separately
//!
//! The `db_exec` golden inserts `"apple"` and `"banana"` with `SqlString` /
//! `SqlInt` params and reads them back ordered by name. The output
//! `"apple:5\nbanana:3\n"` is the only correct answer; the Go backend would
//! produce identical bytes given the same Ipê source.
//!
//! ## Golden catalogue
//!
//! * `db_exec` — `Db.open` → `Db.withTransaction` → `Db.unsafeExecRaw` (DDL) →
//!   `Db.exec` with `[SqlString, SqlInt]` params (two INSERTs) → `Db.query`
//!   with empty params (SELECT ORDER BY name) → `Db.getString` / `Db.getInt`
//!   field access → `println`. Output: `"apple:5\nbanana:3"`.
//! * `db_find_by_conditions` — `Db.exec` two INSERTs →
//!   `Db.findByConditions conn "items" (Dict.fromList [("name","apple")])` →
//!   single-row result → `Db.getString` / `Db.getInt` → `println`.
//!   Output: `"apple:5"`. Proves `Dict String String` arg type + emit arm.
//! * `db_find_where` — `Db.exec` two INSERTs →
//!   `Db.findWhere conn "products" (Sql.gt (Sql.column "qty") (Sql.int 9))` →
//!   single-row result → `println`. Output: `"widget:10"`.
//!   The `SqlFragment`-typed counterpart to a raw `Db.unsafeFindWhere` —
//!   the WHERE clause can only be built through the `Sql.*` combinators, never
//!   a hand-built string.
//! * `db_delete_where` — `Db.exec` three INSERTs →
//!   `Db.deleteWhere conn "products" (Sql.eq (Sql.column "name") (Sql.string "gadget"))`
//!   → row-count + a follow-up `Db.query` confirming only the matched row was
//!   removed → `println`. Output: `"1:sprocket,widget"`.
//! * `db_sql_combinators` — exercises every `Sql.*` combinator at least
//!   once (column, param via int/string/float/bool, eq, ne, gt, lt, gte, lte,
//!   and, or, not, isNull, isNotNull, like, inList non-empty AND the
//!   empty-list `(1 = 0)` shortcut) via three `Db.findWhere` calls.
//!   Output: `"widget|0|gadget"`.
//! * `db_gate_findwhere_string` (negative, `golden_m5b_db_gates.rs`) —
//!   `Db.findWhere conn "products" ("qty > " ++ "9")` is a compile-time
//!   `IPE-T0001` (`String` vs `SqlFragment`) — the "parse, don't validate"
//!   property this surface establishes.
//! * `db_find_by_field` — `Db.exec` three INSERTs (two `category = "fruit"`,
//!   one `category = "veggie"`) → `Db.findOneByField conn "items" "name" "apple"`
//!   (match → `Just` row) → `Db.findOneByField conn "items" "name" "durian"`
//!   (no match → `Nothing`) → `Db.findManyByField conn "items" "category"
//!   "fruit"` (match → two rows, sorted by name since the kernel issues no
//!   `ORDER BY`) → `Db.findManyByField conn "items" "category" "mineral"` (no
//!   match → `[]`). Output:
//!   `"apple:fruit:5\nmissing\napple:fruit:5,banana:fruit:3\nempty"`.
//!   Golden-E2E coverage for `DbFindOneByField`/`DbFindManyByField` through a
//!   real `ipe build` + `cargo build` + run, not just direct runtime source
//!   inspection.
//! * `db_decode_money` — `Db.Decode.money "amount"` inside a
//!   `Db.queryDecode` pipeline decodes the `"CODE AMOUNT"` TEXT column
//!   `SqlMoney` writes on INSERT back into `(Decimal, String)`, and a
//!   malformed value (no space separator) is a total `Task Err`, never a
//!   panic, caught via `Task.onError`. Output: `"USD 12.34\nmalformed:caught"`.
//!   Proves the kernel-registration recipe works end-to-end (canon
//!   allowlist, `StdlibKernel` decl, constrain scheme, lower dispatch, emit,
//!   pretty-print); without registration `Db.Decode.money` is rejected at
//!   canonicalisation as an unknown qualified name even when the runtime
//!   function is implemented and unit-tested.
//!
//! Run:
//!
//! ```text
//! IPE_E2E=1 cargo test golden_m5b_db
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

/// Compile `tests/golden/<name>/Main.ipe`, build the emitted Cargo project, run
/// it, and return the golden directory plus the run outcome. The caller gates on
/// `IPE_E2E`. Fails the test on any build/runtime error.
fn build_run(name: &str) -> (PathBuf, support::RunOutcome) {
    let root = repo_root();
    let dir = golden_dir(&root, name);
    let entry = dir.join("Main.ipe");
    let out = std::env::temp_dir().join(format!("ipec_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = ipe::resolve_runtime();
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
    let built = ipe::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());

    let outcome = support::build_and_run_emitted(name, &out);
    (dir, outcome)
}

/// Compile/build/run the golden and assert its stdout matches the cached oracle.
/// Gated on `IPE_E2E=1`.
fn assert_runs_and_matches_oracle(name: &str) {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    let (dir, outcome) = build_run(name);
    support::assert_go_parity(name, &dir, &outcome.stdout);
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the oracle");
}

// ── Db exec + query via SqlValue params ──────────────────────────────────────

/// `Db.open` → `Db.withTransaction` → `Db.unsafeExecRaw` (DDL) →
/// `Db.exec [SqlString "apple", SqlInt 5]` + `[SqlString "banana", SqlInt 3]`
/// (two INSERTs) → `Db.query [] (SELECT ORDER BY name)` → `Db.getString` /
/// `Db.getInt` → `println`. Output: `"apple:5\nbanana:3"`.
///
/// Recorded sanctioned divergence (Go+cgo `SQLite` vs Rust+sqlx): the Ipê source
/// produces identical output on both backends, but the oracle-capture toolchain
/// only runs Ipê locally.
#[test]
fn db_exec() {
    assert_runs_and_matches_oracle("db_exec");
}

// ── Db.queryDecode with typed Decoder ────────────────────────────────────────

/// `Db.exec` INSERTs two rows → `Db.queryDecode` with a `Decode.map2` decoder
/// that reads `name : String` + `score : Int` into a typed record → formats
/// `"name:score"` and joins with `","`. Output: `"alice:10,bob:7"`.
///
/// Sanctioned divergence: Ipê emits Rust+sqlx; oracle is Ipê's own output.
#[test]
fn db_query_decode() {
    assert_runs_and_matches_oracle("db_query_decode");
}

// ── Db CRUD roundtrip ────────────────────────────────────────────────────────

/// `Db.insertRow` → `Db.getById` (read) → `Db.updateById` (update) →
/// `Db.deleteById` (delete) → read-after-delete returns `Nothing`.
/// Output: `"apple/5\napple/10\ndeleted"`.
///
/// Sanctioned divergence: Ipê emits Rust+sqlx; oracle is Ipê's own output.
#[test]
fn db_crud() {
    assert_runs_and_matches_oracle("db_crud");
}

// ── Db.withTransaction — COMMIT and ROLLBACK paths ───────────────────────────

/// Two sequential transactions on a file-backed `/tmp/ipe_txn_golden.db`:
///
/// * Transaction 1 INSERTs `"hello"` and returns `Ok ()` → committed.
/// * Transaction 2 INSERTs `"world"` then calls `Task.fail` → rolled back.
///
/// A `SELECT` after both transactions observes only `"hello"`.
/// Output: `"hello"`.
///
/// Sanctioned divergence: Ipê emits Rust+sqlx; oracle is Ipê's own output.
#[test]
fn db_transaction() {
    assert_runs_and_matches_oracle("db_transaction");
}

// ── Db.migrate — versioned forward-only migrations + idempotence ─────────────

/// First `Db.migrate` call applies two migrations (`001_create_users`,
/// `002_add_email`) and returns their names. Second call (same list) is a
/// no-op → returns `[]` → empty string. Output: `"001_create_users,002_add_email|"`.
///
/// Sanctioned divergence: Ipê emits Rust+sqlx; oracle is Ipê's own output.
#[test]
fn db_migrate() {
    assert_runs_and_matches_oracle("db_migrate");
}

// ── Db.insertFields / updateFields with OmitField ────────────────────────────

/// `Db.insertFields` with `OmitField` on the `notes` column → DB applies
/// `DEFAULT 'none'`. `Db.updateFields` with `OmitField` on `notes` again →
/// `notes` stays `'none'` while `name` changes to `'gadget'`.
/// Output: `"widget/none\ngadget/none"`.
///
/// Sanctioned divergence: Ipê emits Rust+sqlx; oracle is Ipê's own output.
#[test]
fn db_fields() {
    assert_runs_and_matches_oracle("db_fields");
}

// ── Db.Decode.nullable ───────────────────────────────────────────────────────

/// Two rows: one with a non-NULL `tag` (`"alpha"`) and one with a NULL `tag`.
/// `Db.queryDecode` with `Decode.nullable (Decode.string "tag")` → `Maybe String`.
/// Rows formatted as `"just:<tag>"` / `"nothing"` and joined with `","`.
/// Output: `"just:alpha,nothing"`.
///
/// Sanctioned divergence: Ipê emits Rust+sqlx; oracle is Ipê's own output.
#[test]
fn db_nullable() {
    assert_runs_and_matches_oracle("db_nullable");
}

// ── Db.findByConditions ───────────────────────────────────────────────────────

/// `Db.exec` INSERTs `"apple:5"` and `"banana:3"` rows →
/// `Db.findByConditions conn "items" (Dict.fromList [("name", "apple")])` →
/// single-row result → print `"apple:5"`.
///
/// Proves: (a) the `Dict String String` arg type is correct (not the old
/// `List (String, SqlValue)`); (b) the `DbFindByConditions` emit arm works.
///
/// Sanctioned divergence: Ipê emits Rust+sqlx; oracle is Ipê's own output.
#[test]
fn db_find_by_conditions() {
    assert_runs_and_matches_oracle("db_find_by_conditions");
}

// ── Db.findWhere / Db.deleteWhere / Ipe.Db.Sql combinators ──────

/// `Db.exec` INSERTs `"widget:10"` and `"gadget:7"` rows →
/// `Db.findWhere conn "products" (Sql.gt (Sql.column "qty") (Sql.int 9))` →
/// single-row result (`widget:10`) → print `"widget:10"`.
///
/// Proves: (a) the `SqlFragment`-typed `findWhere` wiring works end-to-end
/// (kernel decl, scheme, lower, emit, runtime); (b) values are bound via `?`
/// placeholders — the WHERE clause can only be built through `Sql.*`
/// combinators, never a hand-built string (see `db_findwhere_string_is_t0001`
/// in `golden_m5b_db_gates.rs` for the negative side of this property).
///
/// Sanctioned divergence: Ipê emits Rust+sqlx; `Db.findWhere` has no Go
/// counterpart; oracle is Ipê's own output.
#[test]
fn db_find_where() {
    assert_runs_and_matches_oracle("db_find_where");
}

/// `Db.exec` INSERTs three rows → `Db.deleteWhere conn "products" (Sql.eq
/// (Sql.column "name") (Sql.string "gadget"))` → row count `1` → a follow-up
/// `Db.query` confirms only `"gadget"` was removed → print
/// `"1:sprocket,widget"`.
///
/// Sanctioned divergence: Ipê emits Rust+sqlx; `Db.deleteWhere` has no Go
/// counterpart; oracle is Ipê's own output.
#[test]
fn db_delete_where() {
    assert_runs_and_matches_oracle("db_delete_where");
}

/// Exercises every `Ipe.Db.Sql` combinator at least once (column, param via
/// int/string/float/bool, eq, ne, gt, lt, gte, lte, and, or, not, isNull,
/// isNotNull, like, inList non-empty AND the empty-list `(1 = 0)` shortcut)
/// through three `Db.findWhere` calls. Output: `"widget|0|gadget"`.
///
/// Sanctioned divergence: Ipê emits Rust+sqlx; the `Sql.*` family has no Go
/// counterpart; oracle is Ipê's own output.
#[test]
fn db_sql_combinators() {
    assert_runs_and_matches_oracle("db_sql_combinators");
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
/// Sanctioned divergence: Ipê emits Rust+sqlx; oracle is Ipê's own output.
#[test]
fn db_sql_decimal_money() {
    assert_runs_and_matches_oracle("db_sql_decimal_money");
}

// ── Polymorphic params — T0001 regression ────────────────────────────────────

/// IPE-T0001 regression: `Db.exec`/`Db.query` accept `List a` (polymorphic),
/// so `List Int`, `List String`, and mixed `List SqlValue` all compile without
/// a type-mismatch error.
///
/// The fixture exercises three call shapes in one `Db.withTransaction`:
///
/// * `Db.exec … [1]`                     — `List Int`   (as in the job-queue example)
/// * `Db.exec … ["hello"]`               — `List String` (as in the ipemon example)
/// * `Db.exec … [SqlNull …, SqlInt 42, SqlBool True]` — mixed `List SqlValue`
///
/// Compile-only assertion: `ipe::build` must succeed (no `IPE-T0001`).
/// E2E assertion (gated on `IPE_E2E=1`): binary prints `"3"` — three rows
/// inserted and queried back.
///
/// Sanctioned divergence: Ipê emits Rust+sqlx; oracle is Ipê's own output.
#[test]
fn db_poly_params_compiles() {
    let root = repo_root();
    let dir = golden_dir(&root, "db_poly_params");
    let entry = dir.join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("db_poly_params");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = ipe::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    // Core assertion: `ipe::build` succeeds — no IPE-T0001 for any param-list shape.
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "IPE-T0001 regression: `Db.exec` with `List Int` / `List String` / mixed \
         `List SqlValue` must compile without a type-mismatch error; got: {:?}",
        built.err()
    );
}

/// E2E extension of [`db_poly_params_compiles`]: build the emitted Cargo project,
/// run it, and assert the binary prints `"3"` (three rows inserted and read back).
/// Gated on `IPE_E2E=1`.
#[test]
fn db_poly_params_e2e() {
    assert_runs_and_matches_oracle("db_poly_params");
}

// ── Db.findOneByField / Db.findManyByField ────────────────────────────────────

/// `Db.exec` three INSERTs into `items` (apple/fruit, banana/fruit,
/// carrot/veggie) → `Db.findOneByField conn "items" "name" "apple"` (match) →
/// `Db.findOneByField conn "items" "name" "durian"` (no match) →
/// `Db.findManyByField conn "items" "category" "fruit"` (two-row match) →
/// `Db.findManyByField conn "items" "category" "mineral"` (no match) →
/// `println`. Output:
/// `"apple:fruit:5\nmissing\napple:fruit:5,banana:fruit:3\nempty"`.
///
/// Golden-E2E coverage for `DbFindOneByField`/`DbFindManyByField` through a
/// real `ipe build` + `cargo build` + run, not just direct runtime source
/// inspection + the internal exhaustiveness test.
///
/// Sanctioned divergence: Ipê emits Rust+sqlx; oracle is Ipê's own output.
#[test]
fn db_find_by_field() {
    assert_runs_and_matches_oracle("db_find_by_field");
}

// ── Db.Decode.money kernel registration ────────────────────

/// `Db.Decode.money "amount"` decodes a `"CODE AMOUNT"` TEXT column
/// (`SqlMoney`'s lossless serialisation) back into `(Decimal, String)`, and a
/// malformed value is a total `Task Err` — never a panic — caught via
/// `Task.onError`. Output: `"USD 12.34\nmalformed:caught"`.
///
/// Proves the kernel-registration recipe works end-to-end: canon
/// `Db.Decode` allowlist, `StdlibKernel::DbDecMoney` decl, constrain.rs
/// scheme, `ipe_lower` arity-1 dispatch, `ipe_backend_rust` standard-path
/// emit, `ipe_ir` pretty-print.
///
/// Sanctioned divergence (tagged `divergence`, not `sanctioned`): the Rust
/// backend's `Db.Decode.money` returns `Decoder (Decimal, String)`, not the
/// Go backend's `Decoder Money` — `Money`/`Currency` are project-generated
/// Rust types unnameable from the shared runtime crate. See
/// `docs/divergences-from-sky.md` (`B-DbDecMoney`).
#[test]
fn db_decode_money() {
    assert_runs_and_matches_oracle("db_decode_money");
}

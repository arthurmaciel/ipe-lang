// DB kernel functions — generic over E and over backend.
// Uses DbPool, DbRow, ipe_db_url, db_last_insert_id, db_format_sql from
// config.rs (generated at build time per ipe.toml [database] driver).
use super::json::{Decoder, JsonVal, decode_and_map, decode_err_str, decode_field, decode_ok};
use super::*;
use sqlx::{Column, Row, TypeInfo};
use std::collections::HashMap;

pub type Db = DbPool;

/// Build a Ipê-visible `Error` from a sqlx error WITHOUT leaking row/column
/// VALUES. The `Display` of a driver error (PostgreSQL/MySQL especially) embeds
/// the offending value in a constraint-violation message — e.g.
/// `... Key (email)=(victim@example.com) already exists` — so funnelling the raw
/// `format!("{}", e)` into the returned `Error` leaks private row data the moment
/// an app surfaces or logs it (PRINCIPLES #1). For a database-level error we
/// therefore build a STRUCTURAL message from the safe-to-expose fields only:
/// the SQLSTATE code (a correlation id operators can trace) and the constraint
/// NAME (a schema identifier, not row data) — never the value. Non-database
/// errors (pool acquisition, connect, decode, IO) carry no row values, so their
/// `Display` is kept for diagnosability. Total — no unwrap/index/panic.
fn ipe_err<E: From<String> + Send>(e: &sqlx::Error) -> E {
    if let Some(dbe) = e.as_database_error() {
        let mut msg = String::from("db: database error");
        if let Some(code) = dbe.code() {
            // SQLSTATE / driver code — structural, value-free.
            msg.push_str(&format!(" [{}]", code));
        }
        if let Some(constraint) = dbe.constraint() {
            // Constraint NAME is a schema identifier (e.g. `users_email_key`),
            // not the offending value — safe to expose and useful for the caller.
            msg.push_str(&format!(" (constraint {})", constraint));
        }
        return str_err(&msg);
    }
    // Non-database errors generally carry no row VALUES, but `ColumnDecode` /
    // `Decode` can embed `source` text that may include a column value — keep
    // those structural (index / variant only). Io / Tls / Protocol /
    // PoolTimedOut / RowNotFound carry no row data, so their `Display` is kept.
    match e {
        sqlx::Error::ColumnDecode { index, .. } => {
            str_err(&format!("db: column decode error at index {index}"))
        }
        sqlx::Error::Decode(_) => str_err("db: decode error"),
        other => str_err(&format!("{other}")),
    }
}

// ─── Transaction connection routing (task-local) ──────────────────────────────
//
// `withTransaction` must run BEGIN, the entire body, and COMMIT/ROLLBACK on ONE
// physical connection. A bare `pool.execute(BEGIN)` routes each statement to an
// arbitrary free connection, so on a multi-connection pool (Postgres/MySQL
// default, or `IPE_DB_MAX_CONNECTIONS > 1` on sqlite) the body's writes can
// autocommit on a different connection that has no open transaction — a rollback
// then silently fails to undo them.
//
// Fix: `db_with_transaction` acquires ONE `PoolConnection` from the pool, stores
// it (behind a `tokio::sync::Mutex` for shared, serialised access) in a
// `tokio::task_local!`, and runs the body inside `TXN_CONN.scope(..)`. Every
// body-reachable DB op routes its query through `exec_*` / `fetch_*` helpers
// below, which lock the task-local connection when one is present, else fall back
// to the pool. Because the body runs on the SAME tokio task (and any spawned
// child task does NOT inherit the task-local — by design, child tasks get the
// pool and must not share the txn connection), every statement lands on the held
// connection and the transaction is real on any pool size.
//
// A `tokio::task_local!` (NOT `thread_local!`) is mandatory: tokio's work-
// stealing scheduler moves a task across worker threads at every `.await`, so a
// thread-local would lose the connection mid-body.

/// The concrete sqlx database backend for this build (sqlite / postgres / mysql),
/// derived from the configured `DbRow` so the helpers stay driver-agnostic.
type DbDatabase = <DbRow as sqlx::Row>::Database;

/// A dedicated sqlx `Transaction`, shared across the body via `Arc<Mutex<..>>`
/// so re-entrant body ops serialise on it (sqlx connections are `&mut`-exclusive).
/// Using a `Transaction` (not a bare `PoolConnection` + raw `BEGIN`) is
/// load-bearing for CANCELLATION SAFETY: its `Drop` rolls back, so a body future
/// dropped mid-transaction (timeout / `select!` / task abort) can never return an
/// OPEN transaction to the pool for the next checkout to inherit.
type TxnConn = std::sync::Arc<tokio::sync::Mutex<sqlx::Transaction<'static, DbDatabase>>>;

// A stable identity for a `Db` (pool) value. `Db` stays a bare `sqlx::Pool`
// alias (no newtype, no change to any of the 70+ existing call sites) — but
// `Pool::connect_options()` hands back a clone of an `Arc` that the pool
// allocated ONCE at build time and every `Pool::clone()` shares. Two clones of
// the SAME pool therefore return `Arc`s that are `ptr_eq`; two DIFFERENT pools
// (even ones connected to the same URL via two separate `.connect()` calls
// that didn't go through `connect_cached`) get distinct allocations. This
// gives genuine pool identity with zero blast radius on the public `Db` type.
type PoolIdentity =
    std::sync::Arc<<<DbDatabase as sqlx::Database>::Connection as sqlx::Connection>::Options>;

fn pool_identity(pool: &Db) -> PoolIdentity {
    pool.connect_options()
}

tokio::task_local! {
    /// Present (Some) for the dynamic extent of a `withTransaction` body — holds
    /// the identity of the pool the transaction was opened on, plus the
    /// dedicated connection BEGIN/COMMIT/ROLLBACK ran on. The identity is
    /// load-bearing: routing (below) and the nesting gate in
    /// `db_with_transaction` both consult it so that a DB op or a nested
    /// `withTransaction` call against a DIFFERENT `Db` handle never gets
    /// silently executed against this transaction's connection (AUD-03).
    static TXN_CONN: Option<(PoolIdentity, TxnConn)>;
}

/// Read the active transaction connection for the current task, but ONLY when
/// it was opened on the SAME pool as `pool` — a transaction active for a
/// different `Db` handle must never receive this pool's operations. Total:
/// returns `None` outside a `withTransaction` scope, or when the active
/// transaction belongs to a different pool (both cases fall through to
/// running directly against `pool`, exactly like "no transaction active").
fn current_txn_conn_for(pool: &Db) -> Option<TxnConn> {
    let active = TXN_CONN.try_with(|c| c.clone()).ok().flatten()?;
    let (owner, conn) = active;
    if std::sync::Arc::ptr_eq(&owner, &pool_identity(pool)) {
        Some(conn)
    } else {
        None
    }
}

// The query type produced by `sqlx::query(&sql)` for the configured backend.
type DbQuery<'q> =
    sqlx::query::Query<'q, DbDatabase, <DbDatabase as sqlx::Database>::Arguments<'q>>;

/// Run a built query for its side effects, on the active transaction connection
/// when one is present (so the statement shares the transaction), else on the
/// pool. Returns the driver query result.
async fn exec_routed<'q>(
    pool: &Db,
    query: DbQuery<'q>,
) -> Result<<DbDatabase as sqlx::Database>::QueryResult, sqlx::Error> {
    match current_txn_conn_for(pool) {
        Some(conn) => {
            let mut guard = conn.lock().await;
            query.execute(&mut **guard).await
        }
        None => query.execute(pool).await,
    }
}

/// `fetch_all` routed through the active transaction connection when present.
async fn fetch_all_routed<'q>(pool: &Db, query: DbQuery<'q>) -> Result<Vec<DbRow>, sqlx::Error> {
    match current_txn_conn_for(pool) {
        Some(conn) => {
            let mut guard = conn.lock().await;
            query.fetch_all(&mut **guard).await
        }
        None => query.fetch_all(pool).await,
    }
}

/// `fetch_optional` routed through the active transaction connection when present.
async fn fetch_optional_routed<'q>(
    pool: &Db,
    query: DbQuery<'q>,
) -> Result<Option<DbRow>, sqlx::Error> {
    match current_txn_conn_for(pool) {
        Some(conn) => {
            let mut guard = conn.lock().await;
            query.fetch_optional(&mut **guard).await
        }
        None => query.fetch_optional(pool).await,
    }
}

/// `fetch_one` routed through the active transaction connection when present.
async fn fetch_one_routed<'q>(pool: &Db, query: DbQuery<'q>) -> Result<DbRow, sqlx::Error> {
    match current_txn_conn_for(pool) {
        Some(conn) => {
            let mut guard = conn.lock().await;
            query.fetch_one(&mut **guard).await
        }
        None => query.fetch_one(pool).await,
    }
}

/// True when column `i`'s runtime type is a genuine boolean, so the `bool`
/// reader must run before the integer reader.
///
/// The decision is keyed on the driver-reported storage type, NOT on a
/// speculative `try_get::<bool>`. That probe is the bug's generative cause: on
/// SQLite a `bool` decode succeeds for EVERY integer cell (any non-zero → true),
/// so a bool-first probe silently stole `qty = 7` and rendered it `"true"`.
/// Postgres `BOOL` and SQLite `BOOLEAN` report a boolean type name; a SQLite
/// INTEGER cell reports `INTEGER` (its runtime storage class) even when it was
/// bound from a Rust `bool` — which matches the Go oracle, whose driver returns
/// `int64` for those cells.
fn column_is_boolean(row: &DbRow, i: usize) -> bool {
    row.columns()
        .get(i)
        .map(sqlx::Column::type_info)
        .map(|ti| {
            let name = ti.name().to_ascii_uppercase();
            name == "BOOL" || name == "BOOLEAN"
        })
        .unwrap_or(false)
}

/// Decode column `i` into a `String` for the untyped `row_to_map` path.
///
/// A boolean-typed column reads via `bool` first; every other column reads
/// numeric-first (i64 → f64) so a SQLite INTEGER is never stolen by the bool
/// reader. `Ok(None)` at any arm = SQL NULL → `""`. The final fallback is
/// `String::new()` — the untyped path has no typed consumer to distinguish NULL
/// from empty (documented at call site).
fn column_to_string(row: &DbRow, i: usize) -> String {
    if column_is_boolean(row, i)
        && let Ok(opt) = row.try_get::<Option<bool>, _>(i)
    {
        return opt.map_or_else(String::new, |b| b.to_string());
    }
    // NULL at any arm → ""; continue to next probe only on decode error.
    if let Ok(opt) = row.try_get::<Option<i64>, _>(i) {
        return opt.map_or_else(String::new, |n| n.to_string());
    }
    if let Ok(opt) = row.try_get::<Option<f64>, _>(i) {
        return opt.map_or_else(String::new, |f| f.to_string());
    }
    if let Ok(opt) = row.try_get::<Option<String>, _>(i) {
        return opt.unwrap_or_default();
    }
    // BYTEA / BLOB: encode as lowercase hex so the value survives round-trip
    // through `db_decode_bytes` (which hex-decodes back to `Vec<u8>`).
    if let Ok(Some(bytes)) = row.try_get::<Option<Vec<u8>>, _>(i) {
        return hex::encode(bytes);
    }
    String::new()
}

/// Decode column `i` into a `JsonVal` for the typed-decoder path.
///
/// A boolean-typed column reads via `bool` first; every other column reads
/// numeric-first (i64 → f64) so a SQLite INTEGER is never stolen by the bool
/// reader (`db_decode_bool` still accepts numeric `0`/`1`, so a SQLite bool
/// round-trips through its INTEGER storage without loss). `Ok(None)` at any arm
/// = SQL NULL → `JsonVal::Null`. The final arm returns `Err` for driver types
/// none of the probes cover; callers surface it as a decode error rather than a
/// phantom Null.
fn column_to_json(row: &DbRow, i: usize) -> Result<JsonVal, sqlx::Error> {
    if column_is_boolean(row, i)
        && let Ok(opt) = row.try_get::<Option<bool>, _>(i)
    {
        return Ok(opt.map_or(JsonVal::Null, JsonVal::Bool));
    }
    if let Ok(opt) = row.try_get::<Option<i64>, _>(i) {
        return Ok(opt.map_or(JsonVal::Null, |n| {
            JsonVal::Number(serde_json::Number::from(n))
        }));
    }
    if let Ok(opt) = row.try_get::<Option<f64>, _>(i) {
        return Ok(opt.map_or(JsonVal::Null, |f| {
            serde_json::Number::from_f64(f).map_or(JsonVal::Null, JsonVal::Number)
        }));
    }
    if let Ok(opt) = row.try_get::<Option<String>, _>(i) {
        return Ok(opt.map_or(JsonVal::Null, JsonVal::String));
    }
    // BYTEA / BLOB: hex-encode for a lossless, driver-neutral text form
    // that pairs with `db_decode_bytes`.
    if let Ok(opt) = row.try_get::<Option<Vec<u8>>, _>(i) {
        return Ok(opt.map_or(JsonVal::Null, |b| JsonVal::String(hex::encode(b))));
    }
    // Driver type not covered by any probe — return a decode error so the
    // caller can surface it instead of silently returning Null.
    Err(sqlx::Error::ColumnDecode {
        index: i.to_string(),
        source: "unsupported column type (not bool/i64/f64/String/bytes)".into(),
    })
}

// needless_range_loop (accepted, cosmetic): the loop indexes by position to pair
// column name[i] with value[i] across two parallel slices — an iterator can't
// thread both. Not a soundness concern.
#[allow(clippy::needless_range_loop)]
fn row_to_map(row: &DbRow) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let cols = row.columns();
    for (i, col) in cols.iter().enumerate() {
        let name = col.name().to_string();
        map.insert(name, column_to_string(row, i));
    }
    map
}

/// NULL-preserving row → `JsonVal` bridge for the typed-decoder path.
///
/// `row_to_map` (the untyped `db_query` path) collapses SQL NULL → `String::new()`,
/// making NULL and empty-string indistinguishable. `db_query_decode` and
/// `db_get_by_id_decode` MUST use this function instead so `db_decode_nullable`
/// can correctly distinguish NULL from an empty value.
///
/// Probe order per column: bool → i64 → f64 → String → bytes-hex.
/// An unreadable driver type (none of the five probes) surfaces as
/// `Err(ColumnDecode)` — the caller converts via `ipe_err`, giving a
/// structural error message rather than a phantom Null.
#[allow(clippy::needless_range_loop)]
fn row_to_json(row: &DbRow) -> Result<JsonVal, sqlx::Error> {
    let cols = row.columns();
    let mut map = serde_json::Map::with_capacity(cols.len());
    for (i, col) in cols.iter().enumerate() {
        let name = col.name().to_string();
        let val = column_to_json(row, i)?;
        map.insert(name, val);
    }
    Ok(JsonVal::Object(map))
}

// ─── DB-specific typed decoder primitives ─────────────────────────────────────
//
// Each primitive wraps `decode_field` (reads a named column from the JsonVal
// object produced by `row_to_json`) and adds domain-specific value parsing.
// ALL are TOTAL: missing column, NULL, or parse failure → `IpeResult::Err` via
// `decode_err_str`, NEVER `.unwrap()` / `.expect()` / `panic!`.
//
// The shared `Decoder<E,T>` type (json.rs:7) is reused here — DbDec decoders
// and JsonDec decoders are the same Rust type. Correctness is ensured by the
// runner functions (`db_query_decode`, `db_get_by_id_decode`) which always feed
// a `row_to_json`-produced `JsonVal::Object` to the decoder, never a raw JSON
// document. Cross-application (JsonDec decoder run against a DB row or vice
// versa) is still well-formed (the types match); it just may produce parse
// errors on format mismatches, which is the expected behaviour.

/// `DbDec.string col` — read column `col` as a String.
/// Fails with Err when the column is missing OR its value is NULL.
pub fn db_decode_string<E: From<String> + 'static>(col: String) -> Decoder<E, String> {
    decode_field(
        col.clone(),
        Decoder::new(
            Box::new(move |v| match v {
                JsonVal::String(s) => decode_ok(s.clone()),
                JsonVal::Null => {
                    decode_err_str(format!("column {}: expected String, got NULL", col))
                }
                _ => decode_err_str(format!(
                    "column {}: expected String, got {:?}",
                    col,
                    v.to_string()
                )),
            }),
            vec![],
        ),
    )
}

/// `DbDec.int col` — read column `col` as an Int (i64).
/// Accepts: JSON Number, or a String representation of an integer or decimal
/// (e.g. "42", "3.0" → 3). NULL → Err. Parse failure → Err.
/// Matches Go's DbDec_int truthy table (int/int64/float64/string forms).
pub fn db_decode_int<E: From<String> + 'static>(col: String) -> Decoder<E, i64> {
    decode_field(
        col.clone(),
        Decoder::new(
            Box::new(move |v| match v {
                JsonVal::Number(n) => match n.as_i64() {
                    Some(i) => decode_ok(i),
                    None => match n.as_f64() {
                        Some(f) => decode_ok(f as i64),
                        None => decode_err_str(format!(
                            "column {}: expected Int, number out of range",
                            col
                        )),
                    },
                },
                JsonVal::String(s) => {
                    // Accept "42" or "3.0" (decimal truncation like Go).
                    if let Ok(i) = s.parse::<i64>() {
                        return decode_ok(i);
                    }
                    if let Ok(f) = s.parse::<f64>() {
                        return decode_ok(f as i64);
                    }
                    decode_err_str(format!("column {}: expected Int, got {:?}", col, s))
                }
                JsonVal::Null => decode_err_str(format!("column {}: expected Int, got NULL", col)),
                _ => decode_err_str(format!("column {}: expected Int, got unexpected type", col)),
            }),
            vec![],
        ),
    )
}

/// `DbDec.float col` — read column `col` as a Float (f64).
/// Matches Go's DbDec_float truthy table (float64/int/int64/string forms).
pub fn db_decode_float<E: From<String> + 'static>(col: String) -> Decoder<E, f64> {
    decode_field(
        col.clone(),
        Decoder::new(
            Box::new(move |v| match v {
                JsonVal::Number(n) => match n.as_f64() {
                    Some(f) => decode_ok(f),
                    None => decode_err_str(format!(
                        "column {}: expected Float, number unrepresentable as f64",
                        col
                    )),
                },
                JsonVal::String(s) => match s.parse::<f64>() {
                    Ok(f) => decode_ok(f),
                    Err(_) => {
                        decode_err_str(format!("column {}: expected Float, got {:?}", col, s))
                    }
                },
                JsonVal::Null => {
                    decode_err_str(format!("column {}: expected Float, got NULL", col))
                }
                _ => decode_err_str(format!(
                    "column {}: expected Float, got unexpected type",
                    col
                )),
            }),
            vec![],
        ),
    )
}

/// `DbDec.bool col` — read column `col` as a Bool.
/// Truthy table (matches Go DbDec_bool):
///   true  ← "true" | "TRUE" | "True" | "t" | "T" | "1" | JSON true  | int 1  | int64 1
///   false ← "false"| "FALSE"| "False"| "f" | "F" | "0" | JSON false | int 0  | int64 0
/// NULL or unrecognised string → Err.
pub fn db_decode_bool<E: From<String> + 'static>(col: String) -> Decoder<E, bool> {
    decode_field(
        col.clone(),
        Decoder::new(
            Box::new(move |v| match v {
                JsonVal::Bool(b) => decode_ok(*b),
                JsonVal::Number(n) => match n.as_i64() {
                    Some(i) => decode_ok(i != 0),
                    None => decode_err_str(format!(
                        "column {}: expected Bool, numeric value unrepresentable",
                        col
                    )),
                },
                JsonVal::String(s) => match s.as_str() {
                    "true" | "TRUE" | "True" | "t" | "T" | "1" => decode_ok(true),
                    "false" | "FALSE" | "False" | "f" | "F" | "0" => decode_ok(false),
                    _ => decode_err_str(format!("column {}: expected Bool, got {:?}", col, s)),
                },
                JsonVal::Null => decode_err_str(format!("column {}: expected Bool, got NULL", col)),
                _ => decode_err_str(format!(
                    "column {}: expected Bool, got unexpected type",
                    col
                )),
            }),
            vec![],
        ),
    )
}

/// `DbDec.money col` — read column `col` as a `(Decimal, String)` pair
/// representing `(amount, currency_code)`.
///
/// The DB column stores a TEXT value in `"ISO_CODE AMOUNT"` format
/// (e.g. `"USD 1234.56"`, `"BTC 0.00012"`), written by `SqlMoney` on the
/// bind side.
///
/// ### Type representation
///
/// The Ipê `Money` ADT is `type Money = Money Decimal Currency` — a generated
/// user-space type (`StdMoneyMoney::Money(StdDecimalDecimal, StdMoneyCurrency)`)
/// that differs per project. The Rust runtime has no single `Money` type to
/// return from a generic `Decoder<E, T>`.  The return type is therefore
/// `(Decimal, String)` — a structural pair that a codegen-emitted wrapper can
/// destructure into the project's concrete `StdMoneyMoney::Money(amount, currency)`.
///
/// The Kernel.hs routing entry **cannot** be wired directly to
/// `db_decode_money` without a codegen-level wrapper that constructs
/// `StdMoneyMoney` from the `(Decimal, String)`.
///
/// Totality: missing column, NULL, bad format, unparseable amount → `Err`.
pub fn db_decode_money<E: From<String> + 'static>(col: String) -> Decoder<E, (Decimal, String)> {
    decode_field(
        col.clone(),
        Decoder::new(
            Box::new(move |v| {
                let s = match v {
                    JsonVal::String(s) => s.clone(),
                    JsonVal::Null => {
                        return decode_err_str(format!(
                            "column {}: expected Money 'CODE AMOUNT', got NULL",
                            col
                        ));
                    }
                    _ => {
                        return decode_err_str(format!(
                            "column {}: expected Money 'CODE AMOUNT' string",
                            col
                        ));
                    }
                };
                // Split on the first space separating the currency code from the amount.
                // `split_once` is total — no raw slicing / index arithmetic on `s` (which
                // would be `indexing_slicing` + an underflow risk on `s.len() - 1`).
                match s.split_once(' ') {
                    Some((code, amount_str)) if !code.is_empty() && !amount_str.is_empty() => {
                        use rust_decimal::Decimal as RD;
                        use std::str::FromStr;
                        match RD::from_str(amount_str) {
                            Ok(d) => decode_ok((Decimal(d), code.to_string())),
                            Err(e) => decode_err_str(format!(
                                "column {}: Money amount parse error for {:?}: {}",
                                col, amount_str, e
                            )),
                        }
                    }
                    _ => decode_err_str(format!(
                        "column {}: expected Money 'CODE AMOUNT', got {:?}",
                        col, s
                    )),
                }
            }),
            vec![],
        ),
    )
}

/// `DbDec.bytes col` — read column `col` as raw bytes (`Vec<u8>`).
///
/// The DB column stores hex-encoded bytes written by `SqlBytes` on the bind
/// side (via `column_to_json`'s hex encoding). Hex-decodes the string value
/// back to `Vec<u8>`, closing the `SqlBytes` write-without-read asymmetry.
///
/// Totality: missing column, NULL, or non-hex string → `Err`.
pub fn db_decode_bytes<E: From<String> + 'static>(col: String) -> Decoder<E, Vec<u8>> {
    decode_field(
        col.clone(),
        Decoder::new(
            Box::new(move |v| match v {
                JsonVal::String(s) => match hex::decode(s) {
                    Ok(b) => decode_ok(b),
                    Err(e) => decode_err_str(format!(
                        "column {}: expected hex-encoded bytes, got {:?}: {}",
                        col, s, e
                    )),
                },
                JsonVal::Null => {
                    decode_err_str(format!("column {}: expected bytes, got NULL", col))
                }
                _ => decode_err_str(format!(
                    "column {}: expected hex-encoded bytes, got {:?}",
                    col,
                    v.to_string()
                )),
            }),
            vec![],
        ),
    )
}

/// `DbDec.nullable inner` — ONE-arg form matching Ipê's
/// `nullable : Decoder a -> Decoder (Maybe a)`.
///
/// Uses `inner.fields` (the `Decoder` struct's `{run, fields}` metadata) to
/// determine which columns the inner decoder reads.
/// This is the Rust equivalent of Go's `DbDec_nullable` which gates on
/// `inner.cols`.
///
/// NULL-gate logic (matches Go):
/// - If `inner.fields` is non-empty: check each named field in the row
///   `JsonVal::Object`. If ANY field is `JsonVal::Null` or absent →
///   `Ok(Nothing)`. Only when all fields are present + non-null do we
///   delegate to `inner.run`.
/// - If `inner.fields` is empty (e.g. a `succeed`/`fail` decoder with no
///   column binding): check the current value directly — `JsonVal::Null`
///   → `Ok(Nothing)`, else delegate.
///
/// Totality: every path returns a `IpeResult`; no panic/unwrap.
pub fn db_decode_nullable<E: From<String> + 'static, T: Send + 'static>(
    inner: Decoder<E, T>,
) -> Decoder<E, IpeMaybe<T>> {
    let gate_fields = inner.fields.clone();
    // Clone for use in the Decoder::new second arg (moved into closure above).
    let fields_for_struct = gate_fields.clone();
    Decoder::new(
        Box::new(move |v| {
            if gate_fields.is_empty() {
                // Leaf decoder with no named fields — gate on the current value itself.
                if v == &JsonVal::Null {
                    return decode_ok(IpeMaybe::Nothing);
                }
            } else {
                // Gate on every field the inner decoder reads.
                for col in &gate_fields {
                    match v.get(col.as_str()) {
                        None | Some(JsonVal::Null) => return decode_ok(IpeMaybe::Nothing),
                        Some(_) => {}
                    }
                }
            }
            // All gate fields are present + non-null (or no gate fields and value
            // is not Null): delegate to inner. Inner Err = structural mismatch.
            match (inner.run)(v) {
                IpeResult::Ok(t) => decode_ok(IpeMaybe::Just(t)),
                IpeResult::Err(e) => IpeResult::Err(e),
            }
        }),
        fields_for_struct,
    )
}

/// `DbDec.required col fieldDec ctorDec` — pipeline step for a required column.
///
/// Ipê signature: `required : String -> Decoder a -> Decoder (a -> b) -> Decoder b`
///
/// Implemented APPLICATIVELY as `decode_and_map(decode_field(col, fieldDec), ctorDec)`.
/// This avoids any FnOnce/Clone wall: `decode_field` reads the named column from the row
/// and returns `IpeResult<E, A>`; `ctorDec` returns `IpeResult<E, Box<dyn FnOnce(A)->B>>`;
/// `decode_and_map` calls the FnOnce once per decoder invocation, which is sound because
/// the decoder is called once per row (not twice for the same row).
///
/// The `col` parameter is accepted for API parity with Ipê's signature but is
/// documentation-only here — `fieldDec` already names its column via `decode_field`.
///
/// Totality: missing column or decode error → Err propagated; no panic/unwrap.
/// Matches Go's `DbDec_required` which delegates to `DbDec_andMap(fieldDec, ctorDec)`.
pub fn db_decode_required<E: From<String> + 'static, A: 'static + Send, B: 'static + Send>(
    _col: String,
    field_dec: Decoder<E, A>,
    ctor_dec: Decoder<E, Box<dyn FnOnce(A) -> B + Send>>,
) -> Decoder<E, B> {
    decode_and_map(field_dec, ctor_dec)
}

/// `DbDec.optional col fieldDec fallback ctorDec` — pipeline step for an optional column.
///
/// Ipê signature: `optional : String -> Decoder a -> a -> Decoder (a -> b) -> Decoder b`
///
/// Like `required` but a missing or NULL column yields `fallback` instead of failing.
/// Implemented applicatively: wrap `fieldDec` so that:
/// - Column absent or `JsonVal::Null` → `Ok(fallback.clone())`
/// - Column present + non-null → `fieldDec` decode result (Err on type mismatch)
///
/// Then `decode_and_map` applies the ctor.
///
/// Totality: NULL/absent → Ok(fallback); present but bad type → Err; ctor Err → Err.
/// Matches Go's `DbDec_optional`.
pub fn db_decode_optional<
    E: From<String> + 'static,
    A: Clone + 'static + Send,
    B: 'static + Send,
>(
    col: String,
    field_dec: Decoder<E, A>,
    fallback: A,
    ctor_dec: Decoder<E, Box<dyn FnOnce(A) -> B + Send>>,
) -> Decoder<E, B> {
    // Build a nullable-aware wrapper: absent/NULL col → Ok(fallback), else decode.
    // `field_dec` is a db_decode_* primitive created with decode_field(col, inner),
    // so it expects the FULL row `JsonVal::Object` (not the extracted field value).
    // We gate on the column presence/NULL status, then pass the full row to field_dec.run.
    let fallback_run = fallback.clone();
    let dec_fields = field_dec.fields.clone();
    let nullable_field = Decoder::new(
        Box::new(move |v| match v.get(&col) {
            None | Some(JsonVal::Null) => decode_ok(fallback_run.clone()),
            Some(_) => (field_dec.run)(v), // pass full row — field_dec peels the column name
        }),
        dec_fields,
    );
    decode_and_map(nullable_field, ctor_dec)
}

// ─── Connection-lifecycle hardening ───────────────────────────────────────────
//
// `Db` (sqlx `Pool`) is an `Arc`-backed handle DESIGNED to be cloned and shared
// process-wide. The Ipê compiler lowers an idiomatic top-level
// `dbConn = Task.run (Db.connect ())` binding as a per-call function, so a user
// who references it per request/session re-enters `db_connect` on every request.
// sqlx's `Pool::connect` is EAGER (real I/O per call), so without a cache that
// pattern (a) churns connections and, on Postgres/MySQL, (b) blows straight
// through the server's `max_connections` cap — a resource-exhaustion / DoS vector
// driven purely by unpredictable user code. The runtime MUST absorb that
// (runtime-rust/AGENTS.md: consistent, secure, sound, efficient under any
// well-typed Ipê program). So `Db.connect <url>` resolves to ONE bounded,
// shared pool per URL — independent of how often the user calls it.

/// Process-global pool registry keyed by connection URL.
fn pool_cache() -> &'static std::sync::Mutex<HashMap<String, Db>> {
    static C: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Db>>> =
        std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// `:memory:` SQLite URLs (bare, or with a scheme prefix like `sqlite://` or
/// `sqlite:`, optionally further wrapped in a `file:` sub-scheme per
/// SQLite's own documented idiom — `sqlite.org/inmemorydb.html`) and
/// URI-mode `mode=memory` must NOT be pooled unless `cache=shared` is
/// present: each connection to a private in-memory database is a DISTINCT
/// database, so sharing a pool would silently merge what callers expect to
/// be isolated DBs (soundness — verified empirically against sqlx 0.8.6:
/// two independently-built pools to `"file::memory:"` do NOT see each
/// other's rows, but two pools to `"file::memory:?cache=shared"` DO).
/// Matching on the exact SQLite special-string / query parameter — not a
/// raw substring match on "memory" anywhere in the URL — so a legitimate
/// file path like `sqlite://data/memory_bank.db` is correctly treated as
/// cacheable.
fn url_is_cacheable(url: &str) -> bool {
    // Strip a `sqlite:` / `sqlite://` scheme prefix if present, then compare
    // the remainder (path + query) — mirrors how sqlx/libsqlite3 parse the
    // connection string.
    let rest = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .unwrap_or(url);

    // Split off the query string (everything after the first `?`) so
    // `mode=memory` can be checked independently of the path.
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let has_cache_shared = query.split('&').any(|kv| kv.starts_with("cache=shared"));

    // SQLite's own documented idiom wraps the special `:memory:` name in a
    // `file:` sub-scheme (e.g. `sqlite3_open("file::memory:?cache=shared",
    // &db)`). Strip that sub-scheme too before comparing — otherwise
    // `"file::memory:"` falls through both checks below and is misclassified
    // as a plain (cacheable) file path, silently merging distinct private
    // in-memory databases behind one pooled connection.
    let file_wrapped = path.starts_with("file:");
    let path = path.strip_prefix("file:").unwrap_or(path);

    if path == ":memory:" {
        // A BARE `:memory:` (no `file:` sub-scheme) is not parsed as a URI
        // by SQLite at all — `cache=shared` has no effect on it and it is
        // unconditionally a private, per-connection database. Only the
        // `file:`-wrapped URI form honours `cache=shared`.
        return file_wrapped && has_cache_shared;
    }
    if query.split('&').any(|kv| kv == "mode=memory") && !has_cache_shared {
        return false;
    }
    true
}

/// Upper bound on pooled connections per database. Bounded by default so that
/// arbitrary user code calling `Db.connect` can NEVER exhaust the database
/// server's connection limit; raise via `IPE_DB_MAX_CONNECTIONS` for workloads
/// that genuinely need more headroom.
fn max_pool_connections() -> u32 {
    crate::system::read_env_var("IPE_DB_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(16)
}

/// Upper bound on DISTINCT cached pools (one per URL). Without this, code that
/// connects to many distinct URLs accumulates live pools forever (memory +
/// connection-handle DoS). At the cap, a new URL is served by a freshly-built,
/// UNCACHED pool — still fully functional, just rebuilt per connect for that URL.
/// Env IPE_DB_MAX_POOLS; default 32 (far above the typical 1–2 DBs per app).
fn max_db_pools() -> usize {
    crate::system::read_env_var("IPE_DB_MAX_POOLS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(32)
}

/// Build one configured pool. SQLite (file, not `:memory:`) gets WAL — concurrent
/// readers alongside a single writer — plus a `busy_timeout` so lock contention
/// WAITS (sound) instead of erroring with `SQLITE_BUSY`. Without WAL a shared pool
/// serialises every statement on the rollback-journal lock (the contention that a
/// naive cache-only change regressed). The PRAGMAs are a no-op for other drivers
/// (guarded by the url scheme).
async fn build_pool<E: Send + From<String> + 'static>(url: &str) -> IpeResult<E, Db> {
    let pool: Db = match sqlx::pool::PoolOptions::new()
        .max_connections(max_pool_connections())
        .connect(url)
        .await
    {
        Ok(p) => p,
        Err(e) => return IpeResult::Err(ipe_err(&e)),
    };
    if url.contains("sqlite") && url_is_cacheable(url) {
        let _ = sqlx::query("PRAGMA journal_mode=WAL;").execute(&pool).await;
        let _ = sqlx::query("PRAGMA busy_timeout=5000;")
            .execute(&pool)
            .await;
    }
    ok_res(pool)
}

/// Connect to `url`, returning a clone of the cached pool on a hit. On a miss the
/// pool is built with NO lock held (never block other tasks on connect I/O); a
/// concurrent miss that built a redundant pool loses the `entry` race and its
/// extra pool drops (closes) — steady state keeps exactly one pool per URL.
async fn connect_cached<E: Send + From<String> + 'static>(url: String) -> IpeResult<E, Db> {
    if url_is_cacheable(&url) {
        let g = pool_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = g.get(&url) {
            return ok_res(p.clone());
        }
    }
    match build_pool::<E>(&url).await {
        IpeResult::Ok(pool) => {
            if url_is_cacheable(&url) {
                let mut g = pool_cache().lock().unwrap_or_else(|e| e.into_inner());
                // Another task may have inserted during the lock-free build → reuse it.
                if let Some(existing) = g.get(&url) {
                    return ok_res(existing.clone());
                }
                // Bound the cache: at cap, return the freshly-built pool UNCACHED
                // (functional; just not memoised) rather than growing without limit.
                if g.len() >= max_db_pools() {
                    return ok_res(pool);
                }
                ok_res(g.entry(url).or_insert(pool).clone())
            } else {
                ok_res(pool)
            }
        }
        IpeResult::Err(e) => IpeResult::Err(e),
    }
}

pub fn db_connect<E: Send + From<String> + 'static>(_unit: ()) -> IpeTask<E, Db> {
    Box::pin(connect_cached(ipe_db_url()))
}

/// `Db.open : String -> String -> Task Error Db` (driver, path). The compiled
/// `DbPool` type is already fixed by the ipe.toml driver, so `driver` is
/// informational; we connect using `path`. For sqlite a bare file path needs a
/// `sqlite://…?mode=rwc` URL (create-if-missing); other drivers pass `path`
/// through as the connection string. (Was wrongly `(_unit: ())` → ignored both
/// args → E0061 at every `Db.open "sqlite" "x.db"` call site.)
pub fn db_open<E: Send + From<String> + 'static>(driver: String, path: String) -> IpeTask<E, Db> {
    let url = if driver == "sqlite" && !path.contains(':') {
        format!("sqlite://{}?mode=rwc", path)
    } else {
        path
    };
    Box::pin(connect_cached(url))
}

pub fn db_open_with_path<E: Send + From<String> + 'static>(path: String) -> IpeTask<E, Db> {
    Box::pin(connect_cached(path))
}

pub fn db_exec_raw<E: Send + From<String> + 'static>(conn: Db, sql: String) -> IpeTask<E, i64> {
    Box::pin(async move {
        // `execRaw : Db -> String -> Task Error Int` — Int is the rows-affected
        // count (Go parity: res.RowsAffected()). `as i64` matches the existing
        // insert/update/delete sites + Go's int64() truncation (rows-affected can
        // never realistically exceed i64::MAX).
        match exec_routed(&conn, sqlx::query(&sql)).await {
            Ok(res) => ok_res(res.rows_affected() as i64),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

pub fn db_exec<E: Send + From<String> + 'static>(
    conn: Db,
    sql: String,
    params: Vec<String>,
) -> IpeTask<E, i64> {
    Box::pin(async move {
        // Same path as the structured kernels: `db_format_sql` adapts `?`
        // placeholders per backend, then bind positionally. sqlx owns the
        // escaping; a placeholder/param count mismatch surfaces as Err.
        // `exec : ... -> Task Error Int` returns rows-affected (Go parity).
        let final_sql = db_format_sql(sql);
        let mut q = sqlx::query(&final_sql);
        for p in params {
            q = q.bind(p);
        }
        match exec_routed(&conn, q).await {
            Ok(res) => ok_res(res.rows_affected() as i64),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

pub fn db_query<E: Send + From<String> + 'static>(
    conn: Db,
    sql: String,
    params: Vec<String>,
) -> IpeTask<E, Vec<HashMap<String, String>>> {
    Box::pin(async move {
        let final_sql = db_format_sql(sql);
        let mut q = sqlx::query(&final_sql);
        for p in params {
            q = q.bind(p);
        }
        match fetch_all_routed(&conn, q).await {
            Ok(rows) => ok_res(rows.iter().map(row_to_map).collect()),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

// ─── Typed-parameter exec/query (Go's v0.16.26 `List SqlValue`) ────────────────
//
// `Db.exec`/`Db.query` are `Db -> String -> List a -> Task ...`. With `a = String`
// the params route through `db_exec`/`db_query` above (Vec<String>). With
// `a = SqlValue` (mixed-type params: String + Int + Bool + Float + Decimal + Time
// + Money + typed NULL), codegen detects the `List SqlValue` element type, lowers
// each element to the runtime-nameable `SqlParam`, and routes HERE. The String
// path is untouched (zero regression); these are a parallel, typed binding path.
//
// Identical to `db_exec`/`db_query` except each param binds via `bind_sql_param`
// (the total SqlParam→query binder used by insertFields/updateFields) instead of
// `q.bind(String)`. Same `exec_routed`/`fetch_all_routed` (task-local
// transaction-aware), same `db_format_sql` placeholder adaptation, same positional
// binding — values are NEVER interpolated (sqlx owns escaping); the SQL string is
// app-authored, exactly as in the String path and as in Go.

pub fn db_exec_params<E: Send + From<String> + 'static>(
    conn: Db,
    sql: String,
    params: Vec<SqlParam>,
) -> IpeTask<E, i64> {
    Box::pin(async move {
        let final_sql = db_format_sql(sql);
        let mut q = sqlx::query(&final_sql);
        for p in params {
            q = bind_sql_param(q, p);
        }
        // Rows-affected (Go parity), same as db_exec.
        match exec_routed(&conn, q).await {
            Ok(res) => ok_res(res.rows_affected() as i64),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

pub fn db_query_params<E: Send + From<String> + 'static>(
    conn: Db,
    sql: String,
    params: Vec<SqlParam>,
) -> IpeTask<E, Vec<HashMap<String, String>>> {
    Box::pin(async move {
        let final_sql = db_format_sql(sql);
        let mut q = sqlx::query(&final_sql);
        for p in params {
            q = bind_sql_param(q, p);
        }
        match fetch_all_routed(&conn, q).await {
            Ok(rows) => ok_res(rows.iter().map(row_to_map).collect()),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

/// A value a Ipê `Db.get*` accessor can read string-keyed fields from.
///
/// Ipê's `getString : String -> row -> String` is polymorphic in `row`; the
/// row can be a query result (`Dict String String`), a pub/sub `Dict` payload,
/// or the typed `WebReq` an `init` handler receives. `IpeRow` is the seam that
/// lets the Rust accessors stay generic and monomorphise per row type — no
/// `dyn Any`, no panic (an absent field reads as `""`).
pub trait IpeRow {
    fn ipe_get(&self, field: &str) -> String;
}

// `IpeDict<String>` is a transparent alias for `HashMap<String, String>`, so this
// is the impl for every Dict-shaped row (query rows + pub/sub Dict payloads).
// Named via the alias for intent; a genuine newtype is tracked as a future task.
impl IpeRow for IpeDict<String> {
    fn ipe_get(&self, field: &str) -> String {
        self.get(field).cloned().unwrap_or_default()
    }
}

// The typed request an `init` handler receives. `Db.getString "path" req` reads
// the named field; `params`/`headers`/`cookies` are searched for any other key.
//
// INVARIANT: `db` must build WITHOUT `live` — a DB-only server / CLI app does not
// pull in Ipe.Web. `super::WebReq` is a `web`-only type, so this impl (the ONLY
// `live` dependency in this module) stays behind `#[cfg(feature = "web")]`. Do not
// reference `live`-only items from `db`-gated code without the same gate. Enforced
// by CI job `runtime-feature-combos` (.github/workflows/ci.yml), which builds
// `--no-default-features --features db` (no web) under `-D warnings`.
#[cfg(feature = "web")]
impl IpeRow for super::WebReq {
    fn ipe_get(&self, field: &str) -> String {
        match field {
            "path" => self.path.clone(),
            "query" => self.query.clone(),
            "method" => self.method.clone(),
            _ => self
                .params
                .get(field)
                .or_else(|| self.headers.get(field))
                .or_else(|| self.cookies.get(field))
                .cloned()
                .unwrap_or_default(),
        }
    }
}

pub fn db_get_field<R: IpeRow>(field: String, row: &R) -> String {
    row.ipe_get(&field)
}

pub fn db_get_string<R: IpeRow>(field: String, row: &R) -> String {
    row.ipe_get(&field)
}

pub fn db_get_int<R: IpeRow>(field: String, row: &R) -> i64 {
    // Align with db_decode_int / Go: accept "42" or a decimal string like
    // "3.0" (truncate to 3) before defaulting to 0.
    let s = row.ipe_get(&field);
    if let Ok(i) = s.parse::<i64>() {
        return i;
    }
    if let Ok(f) = s.parse::<f64>() {
        return f as i64;
    }
    0
}

/// Lowercase sha256-hex of a migration's SQL text. This value is stored in the
/// `_ipe_migrations` ledger and is a CROSS-BACKEND DB CONTRACT: the Go backend
/// records `fmt.Sprintf("%x", sha256.Sum256([]byte(stmt)))` (db_auth.go), so a
/// database created/advanced by one backend must hash byte-identically under the
/// other. Hence sha256, lowercase hex, over the exact statement bytes — never a
/// different/cheaper hash. `{:x}` on a `Sha256` digest is lowercase hex.
fn migrate_checksum(sql: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(sql.as_bytes());
    format!("{:x}", h.finalize())
}

/// `migrate : Db -> List (String, String) -> Task Error (List String)` — apply
/// forward-only schema migrations, recording each in the `_ipe_migrations`
/// ledger so re-runs are idempotent. Go parity: `Db_migrateApply`'s library
/// (Task-return) path in `runtime-go/rt/db_auth.go`.
///
/// Per migration `(name, sql)`:
/// - checksum = sha256-hex(sql).
/// - already in the ledger: checksum match → SKIP (already up to date);
///   checksum DIFFERS → ERROR (the migration's SQL was edited after it was
///   applied — "drift"; the developer must restore the text or ship a new
///   compensating migration).
/// - not yet applied: run the SQL AND record `(name, checksum, applied_at)` in
///   ONE transaction (via the single-connection `db_with_transaction`), so a
///   failure rolls back only that migration and a re-run resumes from it.
///
/// Trust model (matches Go): the migration SQL is compile-time app source the
/// developer ships — it is run verbatim via `db_exec_raw` (arbitrary DDL is the
/// point). Only the ledger bookkeeping crosses into bound-parameter territory
/// (the INSERT binds name/checksum/applied_at — never string-interpolated).
///
/// Single-deployer assumption (matches Go): not concurrency-safe by design. The
/// `name TEXT PRIMARY KEY` ledger column is the backstop — a racing double-apply
/// loses the INSERT to a PK violation inside its own tx, which rolls back, so
/// there is no partial-corruption window.
///
/// DB-ops mode (parity with Go's `Db_migrateApply`): when the `IPE_DB_OP` env var
/// is set — the CLI `ipe db status` / `ipe db migrate --backend rust` sets it — the
/// task PRINTS a human report and `process::exit`s instead of returning, so the
/// surrounding app never starts serving:
///
/// - `status`: print applied / pending / drifted, exit 0 (1 if drift)
/// - `migrate`: apply pending, print summary, exit 0 (1 on error, to stderr)
/// - unset: normal Task behaviour (apply, return Ok/Err) — UNCHANGED
///
/// `process::exit` is reachable ONLY under the CLI-set env op (never from a normal
/// well-typed Ipê `Db.migrate` call), and it is a deliberate CLI termination, not a
/// panic — the no-runtime-panic thesis is about faults, not intentional exits.
pub fn db_migrate_apply<E: Send + From<String> + 'static>(
    db: Db,
    migrations: Vec<(String, String)>,
) -> IpeTask<E, Vec<String>> {
    Box::pin(async move {
        // CLI op mode (empty when unset → library Task-return path, unchanged).
        let op: String = crate::system::read_env_var("IPE_DB_OP")
            .map(|v| v.trim().to_ascii_lowercase())
            .unwrap_or_default();
        // In `migrate` op mode an infra error prints context to stderr + exits 1;
        // otherwise it is returned as a Task Err. Mirrors Go's `fail`.
        macro_rules! db_op_fail {
            ($ctx:expr_2021, $err:expr_2021) => {{
                if op == "migrate" {
                    eprintln!("db: {} failed", $ctx);
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                    // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — `ipe db migrate` CLI-op boundary: a migration infra failure exits the process (library path returns a Task Err instead) [ledger #boundary]
                    std::process::exit(1);
                }
                return IpeResult::Err($err);
            }};
        }

        // 1. Ensure the ledger exists. `IF NOT EXISTS` → idempotent.
        if let IpeResult::Err(e) = db_exec_raw::<E>(
            db.clone(),
            "CREATE TABLE IF NOT EXISTS _ipe_migrations (name TEXT PRIMARY KEY, \
             checksum TEXT NOT NULL, applied_at TEXT NOT NULL)"
                .to_string(),
        )
        .await
        {
            db_op_fail!("create _ipe_migrations", e);
        }

        // 2. Snapshot already-applied migrations: name -> (checksum, applied_at).
        //    Read OUTSIDE any transaction (the per-migration txns come below);
        //    single-deployer so no TOCTOU concern. No interpolation in the SELECT.
        let rows: Vec<HashMap<String, String>> = match db_query::<E>(
            db.clone(),
            "SELECT name, checksum, applied_at FROM _ipe_migrations".to_string(),
            Vec::new(),
        )
        .await
        {
            IpeResult::Ok(r) => r,
            IpeResult::Err(e) => db_op_fail!("read _ipe_migrations", e),
        };
        let mut applied: HashMap<String, (String, String)> = HashMap::new();
        for row in &rows {
            // Total: a row missing a column is skipped rather than panicking
            // (applied_at defaults to empty — only used for the status report).
            if let (Some(name), Some(sum)) = (row.get("name"), row.get("checksum")) {
                let at = row.get("applied_at").cloned().unwrap_or_default();
                applied.insert(name.clone(), (sum.clone(), at));
            }
        }

        // 2b. `status` op mode — read-only report from `applied` × `migrations`,
        //     then exit. Mirrors Go's `dbPrintMigrationStatus`.
        if op == "status" {
            let (mut applied_n, mut pending_n, mut drift_n) = (0usize, 0usize, 0usize);
            // (mark, name, detail) per declared migration.
            let mut lines: Vec<(&'static str, &str, String)> = Vec::with_capacity(migrations.len());
            for (name, sql) in &migrations {
                let sum = migrate_checksum(sql);
                match applied.get(name) {
                    Some((csum, at)) if csum != &sum => {
                        drift_n += 1;
                        lines.push(("✗", name, format!("DRIFT — SQL changed since applied {at}")));
                    }
                    Some((_, at)) => {
                        applied_n += 1;
                        lines.push(("✓", name, format!("applied {at}")));
                    }
                    None => {
                        pending_n += 1;
                        lines.push(("•", name, "pending".to_string()));
                    }
                }
            }
            print!(
                "db: {} migration(s) — {applied_n} applied, {pending_n} pending",
                migrations.len()
            );
            if drift_n > 0 {
                print!(", {drift_n} DRIFTED");
            }
            print!("\n\n");
            let width = lines.iter().map(|(_, n, _)| n.len()).max().unwrap_or(0);
            for (mark, name, detail) in &lines {
                println!("  {mark}  {name:<width$}  {detail}");
            }
            if lines.is_empty() {
                println!("  (no migrations declared)");
            }
            let _ = std::io::Write::flush(&mut std::io::stdout());
            if drift_n > 0 {
                eprintln!(
                    "\ndb: drift detected — an applied migration's SQL was edited. \
                     Restore its original text, or ship a new compensating migration."
                );
                // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — `ipe db migrate` status-op boundary: drift detected, exit non-zero [ledger #boundary]
                std::process::exit(1);
            }
            // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — `ipe db migrate` status-op boundary: clean status, exit zero [ledger #boundary]
            std::process::exit(0);
        }

        // 3. Apply pending migrations in declaration order.
        let mut out: Vec<String> = Vec::new();
        for (name, sql) in migrations {
            let sum = migrate_checksum(&sql);
            if let Some((prev, _)) = applied.get(&name) {
                if prev != &sum {
                    // Drift: error embeds only the app-authored NAME, never the
                    // SQL body (which may carry seed-data literals) nor the hash.
                    if op == "migrate" {
                        eprintln!(
                            "db: migration '{name}' changed after it was applied — checksum mismatch"
                        );
                        let _ = std::io::Write::flush(&mut std::io::stderr());
                        // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — `ipe db migrate` CLI-op boundary: applied migration changed (checksum mismatch), exit non-zero [ledger #boundary]
                        std::process::exit(1);
                    }
                    return IpeResult::Err(
                        format!(
                            "db.migrate: migration '{name}' changed after it was \
                             applied — checksum mismatch"
                        )
                        .into(),
                    );
                }
                continue; // already up to date
            }

            // Each migration in its OWN transaction (single held connection via
            // db_with_transaction's task-local routing): the migration SQL + the
            // ledger INSERT commit together or roll back together.
            let stmt = sql.clone();
            let rec_name = name.clone();
            let rec_sum = sum.clone();
            let applied_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            // db_with_transaction takes `FnOnce` (called exactly once), but
            // clones inside ensure the outer loop can re-bind on each iteration.
            // db_exec/db_exec_raw now return rows-affected (i64), so the tx body's
            // tail yields i64 — bind/turbofish accordingly; the count is unused
            // (migrate cares about success, not row counts).
            let outcome: IpeResult<E, i64> = db_with_transaction::<E, i64>(db.clone(), move |c| {
                let stmt = stmt.clone();
                let rec_name = rec_name.clone();
                let rec_sum = rec_sum.clone();
                let applied_at = applied_at.clone();
                Box::pin(async move {
                    if let IpeResult::Err(e) = db_exec_raw::<E>(c.clone(), stmt).await {
                        return IpeResult::Err(e);
                    }
                    // Ledger INSERT uses BOUND params — no interpolation.
                    db_exec::<E>(
                        c.clone(),
                        "INSERT INTO _ipe_migrations (name, checksum, applied_at) \
                             VALUES (?, ?, ?)"
                            .to_string(),
                        vec![rec_name, rec_sum, applied_at],
                    )
                    .await
                })
            })
            .await;

            match outcome {
                IpeResult::Ok(_) => out.push(name),
                IpeResult::Err(e) => db_op_fail!(format!("apply migration '{name}'"), e),
            }
        }

        // 4. `migrate` op mode — print the summary, then exit. Mirrors Go.
        if op == "migrate" {
            if out.is_empty() {
                println!("db: schema already up to date — 0 migrations applied");
            } else {
                println!("db: applied {} migration(s): {}", out.len(), out.join(", "));
            }
            let _ = std::io::Write::flush(&mut std::io::stdout());
            // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — `ipe db migrate` CLI-op boundary: migrations applied, exit zero [ledger #boundary]
            std::process::exit(0);
        }
        IpeResult::Ok(out)
    })
}

// ─── Additional Ipe.Db kernels ────────────────────────────────────────

/// `close : Db -> Task Error ()` — sqlx::Pool drops on its own; this is
/// a graceful explicit close (any in-flight queries finish, then the
/// pool is closed).
pub fn db_close<E: Send + From<String> + 'static>(db: Db) -> IpeTask<E, ()> {
    Box::pin(async move {
        db.close().await;
        ok_res(())
    })
}

/// `getBool : String -> Dict String String -> Bool` — parses common
/// truthy values (`"1"`, `"true"`, `"TRUE"`, `"t"`, `"T"`).
pub fn db_get_bool<R: IpeRow>(field: String, row: &R) -> bool {
    matches!(
        row.ipe_get(&field).as_str(),
        "1" | "true" | "TRUE" | "t" | "T"
    )
}

/// A validated SQL identifier (table/column name) — parse-don't-validate. The
/// only constructor runs the `[A-Za-z0-9_]`, non-empty policy, so a value of
/// this type is always safe to interpolate. No `""` sentinel to re-check; an
/// unvalidated name is unrepresentable past the boundary.
struct SqlIdent(String);
impl SqlIdent {
    fn parse(name: &str) -> Option<SqlIdent> {
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            Some(SqlIdent(name.to_string()))
        } else {
            None
        }
    }
    fn as_str(&self) -> &str {
        &self.0
    }
}

/// Extract an `Int` id from a `RETURNING id` row. `Err` (never a fabricated
/// `0`) when the `id` column isn't `i64`- or `i32`-decodable — a non-integer
/// primary key (`TEXT`/`UUID`/composite) or a table whose PK column isn't
/// named `id`. Before this helper existed, `db_insert_row` silently returned
/// `0` on a decode miss — indistinguishable from a genuine `id = 0` row, and
/// any caller that used the returned id to look the row back up would
/// silently operate on the wrong row (or no row at all).
#[cfg(feature = "db")]
fn extract_returning_id(r: &DbRow) -> Result<i64, String> {
    r.try_get::<i64, _>("id")
        .or_else(|_| r.try_get::<i32, _>("id").map(i64::from))
        .map_err(|_| {
            "inserted row's id column is not an integer (non-integer or composite \
             primary key) — cannot report an Int id; use Db.insertFieldsReturning \
             with a typed decoder instead"
                .to_string()
        })
}

/// `insertRow : Db -> String -> Dict String String -> Task Error Int` —
/// returns the inserted row's id (lastInsertRowid for sqlite).
pub fn db_insert_row<E: Send + From<String> + 'static>(
    conn: Db,
    table: String,
    row: HashMap<String, String>,
) -> IpeTask<E, i64> {
    Box::pin(async move {
        let qtable = match SqlIdent::parse(&table) {
            Some(t) => t,
            None => {
                return IpeResult::Err(
                    format!("db.insertRow: invalid table name {:?}", table).into(),
                );
            }
        };
        if row.is_empty() {
            return IpeResult::Err("db.insertRow: empty row".to_string().into());
        }
        let mut keys: Vec<&String> = row.keys().collect();
        keys.sort(); // deterministic column order
        let col_idents: Vec<SqlIdent> = match keys
            .iter()
            .map(|k| SqlIdent::parse(k))
            .collect::<Option<Vec<_>>>()
        {
            Some(v) => v,
            None => return IpeResult::Err("db.insertRow: invalid column name".to_string().into()),
        };
        let col_names: Vec<&str> = col_idents.iter().map(SqlIdent::as_str).collect();
        let placeholders = vec!["?"; col_names.len()].join(", ");
        let base = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            qtable.as_str(),
            col_names.join(", "),
            placeholders
        );
        if DB_USES_RETURNING_ID {
            // Postgres has no LastInsertId — append `RETURNING id` and read the
            // generated key (matches the Go backend's pgx path). `id` is
            // BIGSERIAL (i64) by db_auto_id_column, but a user table may use
            // SERIAL (i32); try both, and surface a clear Err — never a
            // fabricated `0` — when the id column isn't integer-decodable at
            // all (non-integer/composite primary key).
            let sql = db_format_sql(format!("{} RETURNING id", base));
            let mut q = sqlx::query(&sql);
            for k in &keys {
                q = q.bind(row.get(*k).cloned().unwrap_or_default());
            }
            match fetch_one_routed(&conn, q).await {
                Ok(r) => match extract_returning_id(&r) {
                    Ok(id) => ok_res(id),
                    Err(msg) => IpeResult::Err(format!("db.insertRow: {msg}").into()),
                },
                Err(e) => IpeResult::Err(ipe_err(&e)),
            }
        } else {
            let sql = db_format_sql(base);
            let mut q = sqlx::query(&sql);
            for k in &keys {
                q = q.bind(row.get(*k).cloned().unwrap_or_default());
            }
            match exec_routed(&conn, q).await {
                Ok(res) => ok_res(db_last_insert_id(&res)),
                Err(e) => IpeResult::Err(ipe_err(&e)),
            }
        }
    })
}

/// `getById : Db -> String -> String -> Task Error (Maybe (Dict String String))`.
pub fn db_get_by_id<E: Send + From<String> + 'static>(
    conn: Db,
    table: String,
    id: String,
) -> IpeTask<E, IpeMaybe<HashMap<String, String>>> {
    Box::pin(async move {
        let qtable = match SqlIdent::parse(&table) {
            Some(t) => t,
            None => {
                return IpeResult::Err(
                    format!("db.getById: invalid table name {:?}", table).into(),
                );
            }
        };
        let sql = db_format_sql(format!(
            "SELECT * FROM {} WHERE id = ? LIMIT 1",
            qtable.as_str()
        ));
        match fetch_optional_routed(&conn, sqlx::query(&sql).bind(id)).await {
            Ok(Some(r)) => ok_res(IpeMaybe::Just(row_to_map(&r))),
            Ok(None) => ok_res(IpeMaybe::Nothing),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

/// `updateById : Db -> String -> String -> Dict String String -> Task Error Int` —
/// returns the affected row count.
pub fn db_update_by_id<E: Send + From<String> + 'static>(
    conn: Db,
    table: String,
    id: String,
    row: HashMap<String, String>,
) -> IpeTask<E, i64> {
    Box::pin(async move {
        let qtable = match SqlIdent::parse(&table) {
            Some(t) => t,
            None => {
                return IpeResult::Err(
                    format!("db.updateById: invalid table name {:?}", table).into(),
                );
            }
        };
        if row.is_empty() {
            return ok_res(0);
        }
        let mut keys: Vec<&String> = row.keys().collect();
        keys.sort();
        let col_idents: Vec<SqlIdent> = match keys
            .iter()
            .map(|k| SqlIdent::parse(k))
            .collect::<Option<Vec<_>>>()
        {
            Some(v) => v,
            None => return IpeResult::Err("db.updateById: invalid column name".to_string().into()),
        };
        let col_names: Vec<&str> = col_idents.iter().map(SqlIdent::as_str).collect();
        let sets: Vec<String> = col_names.iter().map(|c| format!("{} = ?", c)).collect();
        let sql = db_format_sql(format!(
            "UPDATE {} SET {} WHERE id = ?",
            qtable.as_str(),
            sets.join(", ")
        ));
        let mut q = sqlx::query(&sql);
        for k in &keys {
            q = q.bind(row.get(*k).cloned().unwrap_or_default());
        }
        q = q.bind(id);
        match exec_routed(&conn, q).await {
            Ok(res) => ok_res(res.rows_affected() as i64),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

/// `deleteById : Db -> String -> String -> Task Error Int` — returns
/// the affected row count (0 or 1).
pub fn db_delete_by_id<E: Send + From<String> + 'static>(
    conn: Db,
    table: String,
    id: String,
) -> IpeTask<E, i64> {
    Box::pin(async move {
        let qtable = match SqlIdent::parse(&table) {
            Some(t) => t,
            None => {
                return IpeResult::Err(
                    format!("db.deleteById: invalid table name {:?}", table).into(),
                );
            }
        };
        let sql = db_format_sql(format!("DELETE FROM {} WHERE id = ?", qtable.as_str()));
        match exec_routed(&conn, sqlx::query(&sql).bind(id)).await {
            Ok(res) => ok_res(res.rows_affected() as i64),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

/// `findOneByField : Db -> String -> String -> String -> Task Error (Maybe (Dict String String))`.
pub fn db_find_one_by_field<E: Send + From<String> + 'static>(
    conn: Db,
    table: String,
    field: String,
    value: String,
) -> IpeTask<E, IpeMaybe<HashMap<String, String>>> {
    Box::pin(async move {
        let (qtable, qfield) = match (SqlIdent::parse(&table), SqlIdent::parse(&field)) {
            (Some(t), Some(f)) => (t, f),
            _ => {
                return IpeResult::Err(
                    format!(
                        "db.findOneByField: invalid identifier in {:?}.{:?}",
                        table, field
                    )
                    .into(),
                );
            }
        };
        let sql = db_format_sql(format!(
            "SELECT * FROM {} WHERE {} = ? LIMIT 1",
            qtable.as_str(),
            qfield.as_str()
        ));
        match fetch_optional_routed(&conn, sqlx::query(&sql).bind(value)).await {
            Ok(Some(r)) => ok_res(IpeMaybe::Just(row_to_map(&r))),
            Ok(None) => ok_res(IpeMaybe::Nothing),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

/// `findManyByField : Db -> String -> String -> String -> Task Error (List (Dict String String))`.
pub fn db_find_many_by_field<E: Send + From<String> + 'static>(
    conn: Db,
    table: String,
    field: String,
    value: String,
) -> IpeTask<E, Vec<HashMap<String, String>>> {
    Box::pin(async move {
        let (qtable, qfield) = match (SqlIdent::parse(&table), SqlIdent::parse(&field)) {
            (Some(t), Some(f)) => (t, f),
            _ => {
                return IpeResult::Err(
                    format!(
                        "db.findManyByField: invalid identifier in {:?}.{:?}",
                        table, field
                    )
                    .into(),
                );
            }
        };
        let sql = db_format_sql(format!(
            "SELECT * FROM {} WHERE {} = ?",
            qtable.as_str(),
            qfield.as_str()
        ));
        match fetch_all_routed(&conn, sqlx::query(&sql).bind(value)).await {
            Ok(rows) => ok_res(rows.iter().map(row_to_map).collect()),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

/// `findByConditions : Db -> String -> Dict String String -> Task Error (List (Dict String String))` —
/// AND-joined equality on every key/value pair.
pub fn db_find_by_conditions<E: Send + From<String> + 'static>(
    conn: Db,
    table: String,
    conditions: HashMap<String, String>,
) -> IpeTask<E, Vec<HashMap<String, String>>> {
    Box::pin(async move {
        let qtable = match SqlIdent::parse(&table) {
            Some(t) => t,
            None => {
                return IpeResult::Err(
                    format!("db.findByConditions: invalid table {:?}", table).into(),
                );
            }
        };
        let mut keys: Vec<&String> = conditions.keys().collect();
        keys.sort();
        let qfield_idents: Vec<SqlIdent> = match keys
            .iter()
            .map(|k| SqlIdent::parse(k))
            .collect::<Option<Vec<_>>>()
        {
            Some(v) => v,
            None => {
                return IpeResult::Err(
                    "db.findByConditions: invalid column name"
                        .to_string()
                        .into(),
                );
            }
        };
        let qfields: Vec<&str> = qfield_idents.iter().map(SqlIdent::as_str).collect();
        // Refuse an unscoped SELECT: an empty condition set would return every
        // row in the table — a cross-tenant read when conditions come from
        // request-derived filters. Mirrors the `db_update_fields` empty-WHERE
        // guard. Callers wanting all rows must use `db_query` / `db_query_raw`.
        if keys.is_empty() {
            return IpeResult::Err(
                "db.findByConditions: refusing unscoped SELECT (no conditions); \
                 pass at least one condition"
                    .to_string()
                    .into(),
            );
        }
        let wheres: Vec<String> = qfields.iter().map(|c| format!("{} = ?", c)).collect();
        let sql = db_format_sql(format!(
            "SELECT * FROM {} WHERE {}",
            qtable.as_str(),
            wheres.join(" AND ")
        ));
        let mut q = sqlx::query(&sql);
        for k in &keys {
            q = q.bind(conditions.get(*k).cloned().unwrap_or_default());
        }
        match fetch_all_routed(&conn, q).await {
            Ok(rows) => ok_res(rows.iter().map(row_to_map).collect()),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

/// `queryDecode : Db -> String -> List String -> Decoder a -> Task Error (List a)` —
/// typed query with a per-row decoder (Decoder<E,A>). Builds a NULL-preserving
/// `JsonVal::Object` per row (via `row_to_json`) and runs the decoder against it.
/// Fails fast on the first decode error.
///
/// The `Decoder<E,A>` is `Box<dyn Fn(&JsonVal) -> IpeResult<E,A> + Send>`. Moving
/// it into the async block is sound: it is `Send`, and calling `decoder(&jv)` is
/// a shared-reference call (no move out of the box). No `Arc` needed.
pub fn db_query_decode<E: Send + From<String> + 'static, A: Send + 'static>(
    conn: Db,
    sql: String,
    params: Vec<String>,
    decoder: Decoder<E, A>,
) -> IpeTask<E, Vec<A>> {
    Box::pin(async move {
        let final_sql = db_format_sql(sql);
        let mut q = sqlx::query(&final_sql);
        for p in params {
            q = q.bind(p);
        }
        let rows = match fetch_all_routed(&conn, q).await {
            Ok(r) => r,
            Err(e) => return IpeResult::Err(ipe_err(&e)),
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let jv = match row_to_json(row) {
                Ok(v) => v,
                Err(e) => return IpeResult::Err(ipe_err(&e)),
            };
            match (decoder.run)(&jv) {
                IpeResult::Ok(a) => out.push(a),
                IpeResult::Err(e) => return IpeResult::Err(e),
            }
        }
        ok_res(out)
    })
}

/// `queryDecode` with `List SqlValue` params (Go v0.16.26 mixed-type) — mirror of
/// `db_query_decode` binding each param via the total `bind_sql_param` instead of
/// `q.bind(String)`. Codegen routes HERE when the params arg's solved element type
/// is `SqlValue` (ExprEmitter `isSqlValueListArg`); a homogeneous `List String`
/// keeps the `db_query_decode` (Vec<String>) path. Same fetch_all_routed +
/// row_to_json + decoder loop; same positional binding (never interpolated).
pub fn db_query_decode_params<E: Send + From<String> + 'static, A: Send + 'static>(
    conn: Db,
    sql: String,
    params: Vec<SqlParam>,
    decoder: Decoder<E, A>,
) -> IpeTask<E, Vec<A>> {
    Box::pin(async move {
        let final_sql = db_format_sql(sql);
        let mut q = sqlx::query(&final_sql);
        for p in params {
            q = bind_sql_param(q, p);
        }
        let rows = match fetch_all_routed(&conn, q).await {
            Ok(r) => r,
            Err(e) => return IpeResult::Err(ipe_err(&e)),
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let jv = match row_to_json(row) {
                Ok(v) => v,
                Err(e) => return IpeResult::Err(ipe_err(&e)),
            };
            match (decoder.run)(&jv) {
                IpeResult::Ok(a) => out.push(a),
                IpeResult::Err(e) => return IpeResult::Err(e),
            }
        }
        ok_res(out)
    })
}

/// `getByIdDecode : Db -> String -> Int -> Decoder a -> Task Error (Maybe a)` —
/// SELECT * FROM `table` WHERE id = `id` LIMIT 1; returns Nothing when no row
/// matches, Just(decoded) on success, Err on DB error or decode error.
///
/// Security: `id` is bound via a parameterised placeholder (`?`), NEVER
/// string-interpolated into SQL.
pub fn db_get_by_id_decode<E: Send + From<String> + 'static, A: Send + 'static>(
    conn: Db,
    table: String,
    id: i64,
    decoder: Decoder<E, A>,
) -> IpeTask<E, IpeMaybe<A>> {
    Box::pin(async move {
        let qtable = match SqlIdent::parse(&table) {
            Some(t) => t,
            None => {
                return IpeResult::Err(
                    format!("db.getByIdDecode: invalid table name {:?}", table).into(),
                );
            }
        };
        // id is bound as a parameter — injection-safe.
        let sql = db_format_sql(format!(
            "SELECT * FROM {} WHERE id = ? LIMIT 1",
            qtable.as_str()
        ));
        match fetch_optional_routed(&conn, sqlx::query(&sql).bind(id)).await {
            Ok(None) => ok_res(IpeMaybe::Nothing),
            Ok(Some(row)) => {
                let jv = match row_to_json(&row) {
                    Ok(v) => v,
                    Err(e) => return IpeResult::Err(ipe_err(&e)),
                };
                match (decoder.run)(&jv) {
                    IpeResult::Ok(a) => ok_res(IpeMaybe::Just(a)),
                    IpeResult::Err(e) => IpeResult::Err(e),
                }
            }
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

/// `withTransaction : Db -> (Db -> Task Error a) -> Task Error a` —
/// runs the body inside a transaction. Commits on Ok, rolls back on Err.
///
/// **Connection semantics (real isolation on any pool size).** sqlx's `Pool`
/// dispatches each `.execute()` to an arbitrary free connection, so issuing
/// BEGIN/COMMIT/ROLLBACK against the pool would scatter the transaction-control
/// statements and the body's writes across different physical connections — on a
/// multi-connection pool a rollback would then silently fail to undo the body's
/// (autocommitted) writes.
///
/// This implementation pins the whole transaction to ONE connection:
///  1. `pool.acquire()` takes a dedicated `PoolConnection` out of the pool.
///  2. The connection is stored in the `TXN_CONN` `tokio::task_local!` (behind an
///     `Arc<Mutex<..>>`) for the dynamic extent of the body.
///  3. `BEGIN`, the body, and `COMMIT`/`ROLLBACK` all run on THAT connection —
///     the body's `Db.exec`/`Db.query`/`insertRow`/… route through the `*_routed`
///     helpers, which lock the task-local connection when present.
///  4. On every exit (Ok / Err / body-error) the `PoolConnection` is dropped at
///     the end of the scope, returning it to the pool (RAII — never leaked).
///
/// **Nested `withTransaction`.** If a transaction connection is already active on
/// this task (a nested call), we DO NOT acquire a second connection or issue a
/// nested `BEGIN` (sqlite/MySQL would error; it would also deadlock on the
/// `Mutex`). Instead the inner call runs the body directly on the already-held
/// connection (flattened semantics — the inner block shares the outer
/// transaction's atomicity; an inner `Err` does not roll back independently). A
/// true SAVEPOINT-per-nesting is the ideal future refinement; flattening is the
/// simplest correct behaviour and never deadlocks.
///
/// **Nesting is gated on pool identity (AUD-03 fix).** Flattening is only
/// correct when the nested call reuses the SAME pool as the active
/// transaction — flattening a call for a DIFFERENT `Db` handle onto it would
/// silently execute that pool's operations against the wrong physical
/// connection (cross-database data corruption). `current_txn_conn_for(&conn)`
/// returns `None` when the active transaction belongs to a different pool, so
/// that case falls through to the code below and opens its OWN independent
/// transaction on `conn` — nested correctly via `TXN_CONN.scope`'s normal
/// task-local shadow/restore (the outer transaction's task-local value is
/// restored once this inner scope's future completes), not by any manual
/// stack bookkeeping.
pub fn db_with_transaction<E: Send + From<String> + 'static, A: Send + 'static>(
    conn: Db,
    body: impl FnOnce(Db) -> IpeTask<E, A> + Send + 'static,
) -> IpeTask<E, A> {
    Box::pin(async move {
        // Nested on the SAME pool: flatten onto the existing connection (no
        // second acquire, no nested BEGIN, no deadlock). A nested call on a
        // DIFFERENT pool falls through and opens its own transaction below.
        if current_txn_conn_for(&conn).is_some() {
            return body(conn).await;
        }

        // Begin a real sqlx Transaction (BEGIN is issued by `begin()`); its Drop
        // rolls back, so dropping the body future mid-transaction can't leak an
        // open txn onto a pooled connection. Held in Arc<Mutex<..>> so re-entrant
        // body ops serialise on it.
        let tx = match conn.begin().await {
            Ok(t) => t,
            Err(e) => return IpeResult::Err(ipe_err(&e)),
        };
        let tx_conn: TxnConn = std::sync::Arc::new(tokio::sync::Mutex::new(tx));

        // Run the body inside the task-local scope so every body DB op routes to
        // `tx_conn`. The body still receives the pool by value (its `Db` arg) —
        // the routing happens via the task-local, not the arg.
        let pool_for_body = conn.clone();
        let owner = pool_identity(&conn);
        let outcome = TXN_CONN
            .scope(Some((owner, tx_conn.clone())), async move {
                body(pool_for_body).await
            })
            .await;

        // Reclaim sole ownership to finish via the TYPED commit/rollback (which
        // consume the Transaction and keep its Drop-state consistent — a raw COMMIT
        // string would leave the wrapper thinking the txn is open → a redundant
        // ROLLBACK on Drop). The scope's clone is released when the scoped future
        // above completes, and tokio task-locals don't propagate into spawned
        // tasks, so the strong count is 1 here.
        let tx = match std::sync::Arc::try_unwrap(tx_conn) {
            Ok(m) => m.into_inner(),
            // Structurally unreachable (no clone escapes). Fail closed: our handle
            // is dropped here, rolling the txn back, and we report rather than
            // committing a transaction we don't solely own.
            Err(_) => {
                return IpeResult::Err(
                    "withTransaction: transaction still referenced at completion"
                        .to_string()
                        .into(),
                );
            }
        };
        match outcome {
            IpeResult::Ok(a) => match tx.commit().await {
                Ok(()) => ok_res(a),
                Err(e) => IpeResult::Err(ipe_err(&e)),
            },
            IpeResult::Err(e) => {
                // Best-effort deterministic rollback; the body's Err is reported.
                let _ = tx.rollback().await;
                IpeResult::Err(e)
            }
        }
    })
}

// ─── SqlParam — runtime-nameable parameter type for db_insert_fields etc. ─────
//
// Ipê's `SqlField` and `SqlValue` ADTs are per-project GENERATED Rust enums
// (`StdDbSqlField`, `StdDbSqlValue`).  The runtime can't name or destructure
// them, but it CAN define `SqlParam` — a parallel enum whose variants match
// SqlValue 1:1.  The codegen emits a conversion at each `insertFields` /
// `updateFields` / `insertFieldsReturning` call site:
//
//   StdDbSqlField::OmitField      → None           (column dropped from SQL)
//   StdDbSqlField::SetField(v)    → Some(v.into())  (column bound as param)
//   StdDbSqlValue::SqlString(s)   → SqlParam::Text(s)
//   StdDbSqlValue::SqlInt(i)      → SqlParam::Int(i)
//   StdDbSqlValue::SqlFloat(f)    → SqlParam::Float(f)
//   StdDbSqlValue::SqlBool(b)     → SqlParam::Bool(b)
//   StdDbSqlValue::SqlBytes(s)    → SqlParam::Bytes(s.into_bytes())
//   StdDbSqlValue::SqlDecimal(d)  → SqlParam::Text(d.to_string())  (lossless)
//   StdDbSqlValue::SqlTime(ms)    → SqlParam::Int(ms)  (Unix millis, matches Go)
//   StdDbSqlValue::SqlMoney(m)    → SqlParam::Text("ISO_CODE AMOUNT")  (see note)
//   StdDbSqlValue::SqlNull(inner) → SqlParam::Null(Box::new(inner.into_sql_param()))
//
// Money note: `StdMoneyMoney::Money(amount, currency)` is also generated; codegen
// serialises it to "CODE AMOUNT" string (same as Go's sqlMoneyToString).  If
// codegen cannot destructure Money (e.g. future Money redesign), the fallback is
// SqlParam::Text(money_to_text) where money_to_text is emitted inline.
//
// Security: table/column names are validated by `valid_sql_ident` (ASCII
// alphanumeric + `_` + `.`, rejects empty) before interpolation into SQL.
// All VALUES are positional-bound (`?`), never interpolated.
// Totality: no unwrap/panic anywhere in this module section.

/// A runtime-nameable SQL parameter value, matching the Ipê `SqlValue` ADT.
/// See the module-level comment above for the generated-ADT conversion rules.
///
/// `PartialEq` precondition (`SqlFragment` design note): every
/// constituent field type here (`String`, `i64`, `f64`, `bool`, `Vec<u8>`) is
/// already `PartialEq`, so the derive below is total and structural — no
/// hand-written impl needed.
#[derive(Clone, Debug, PartialEq)]
pub enum SqlParam {
    /// `SqlString s` — binds as TEXT.
    Text(String),
    /// `SqlInt i` / `SqlTime ms` — binds as INTEGER.
    Int(i64),
    /// `SqlFloat f` — binds as REAL.
    Float(f64),
    /// `SqlBool b` — binds as INTEGER (0 / 1), matching SQLite convention.
    Bool(bool),
    /// `SqlBytes s` — binds as BLOB.
    Bytes(Vec<u8>),
    /// `SqlNull witness` — binds a NULL typed according to `witness`'s
    /// variant, so the driver's type-OID hint (Postgres) matches the target
    /// column. `witness`'s VALUE is never read (a NULL carries no value) —
    /// only its variant tag selects the typed `Option::<T>::None` to bind.
    ///
    /// On SQLite this distinction is cosmetic (SQLite is dynamically typed —
    /// a bound NULL is a NULL regardless of the wrapping Rust type). On
    /// Postgres it is load-bearing: sqlx's extended query protocol sends a
    /// type-OID hint per bound parameter derived from the bound Rust type,
    /// and Postgres validates that hint against the target column's type at
    /// prepare time. Binding `Option::<String>::None` (OID: TEXT) against an
    /// `INTEGER`/`BOOLEAN`/`BYTEA`/`TIMESTAMP` column fails with a Postgres
    /// type-mismatch error. Boxed to keep construction cheap (one variant,
    /// rarely on a hot loop) without inflating every other variant's size.
    Null(Box<SqlParam>),
}

// ── `From<T> for SqlParam` — primitive Ipê types ────────────────────────────
//
// These impls let the emitter use `ipe_runtime::db::SqlParam::from` as a
// uniform projection function for the polymorphic `exec`/`query` params list
// (`List a` where `a` may be `String`, `Int`, `Float`, `Bool`, or `SqlValue`).
// The generated `StdDbSqlValue` type gets a parallel `From` impl emitted by
// `ipe_backend_rust::project::emit_db_projection_impls`, delegating to the
// existing `into_sql_param` inherent method.
//
// Go parity: Go's `database/sql` driver accepts `any` and type-switches at
// runtime; here the conversion is statically resolved by the Rust type system.

impl From<String> for SqlParam {
    /// Bind a Ipê `String` parameter as SQL TEXT.
    fn from(s: String) -> Self {
        SqlParam::Text(s)
    }
}

impl From<i64> for SqlParam {
    /// Bind a Ipê `Int` parameter as SQL INTEGER.
    fn from(i: i64) -> Self {
        SqlParam::Int(i)
    }
}

impl From<f64> for SqlParam {
    /// Bind a Ipê `Float` parameter as SQL REAL.
    fn from(f: f64) -> Self {
        SqlParam::Float(f)
    }
}

impl From<bool> for SqlParam {
    /// Bind a Ipê `Bool` parameter as SQL INTEGER (0 / 1), matching SQLite
    /// convention.
    fn from(b: bool) -> Self {
        SqlParam::Bool(b)
    }
}

/// Validate an SQL identifier (table or column name).
/// Allows ASCII alphanumeric characters, underscore, and dot.
/// Rejects empty strings and anything outside that character set.
/// Mirrors Go's `validSqlIdent` function in db_auth.go.
pub fn valid_sql_ident(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Bind a `SqlParam` value onto a sqlx `Query` builder.
/// Returns `IpeResult::Err` only when the DB pool is absent (no-db build).
/// Every variant is handled — this function is TOTAL.
///
/// Driver-agnostic: typed on the `DbQuery<'q>` alias (the configured backend's
/// query type) rather than a hardcoded `sqlx::Sqlite`, so a project built with
/// `[database] driver = "postgres"` (which does NOT enable sqlx's `sqlite`
/// feature) still compiles — `sqlx::Sqlite` / `SqliteArguments` would be E0433
/// there. Each bound value type (String / i64 / f64 / bool / Vec<u8> / Option)
/// impls `Encode + Type` for both Sqlite and Postgres, so the monomorphic
/// per-build `q.bind(..)` resolves on either backend.
#[cfg(feature = "db")]
fn bind_sql_param<'q>(q: DbQuery<'q>, p: SqlParam) -> DbQuery<'q> {
    match p {
        SqlParam::Text(s) => q.bind(s),
        SqlParam::Int(i) => q.bind(i),
        SqlParam::Float(f) => q.bind(f),
        SqlParam::Bool(b) => q.bind(b),
        SqlParam::Bytes(v) => q.bind(v),
        SqlParam::Null(witness) => match *witness {
            SqlParam::Text(_) => q.bind(Option::<String>::None),
            SqlParam::Int(_) => q.bind(Option::<i64>::None),
            SqlParam::Float(_) => q.bind(Option::<f64>::None),
            SqlParam::Bool(_) => q.bind(Option::<bool>::None),
            SqlParam::Bytes(_) => q.bind(Option::<Vec<u8>>::None),
            // A nested Null-of-Null witness is a degenerate shape that should
            // not arise from codegen (SqlValue's SqlNull wraps a concrete leaf
            // SqlValue variant, not another SqlNull) — fall back to a
            // TEXT-typed NULL rather than panicking; matches the pre-fix
            // SQLite-safe behaviour for this unreachable case.
            SqlParam::Null(_) => q.bind(Option::<String>::None),
        },
    }
}

// ─── Ipe.Db.Sql — SqlFragment builder ────────────────────────
//
// Closes the SQL-injection surface the removed `unsafeFindWhere` left open.
// The ONLY way to obtain a `SqlFragment` is through the combinators below —
// there is no public constructor that accepts an arbitrary `String` as SQL
// text — so a naive string-concatenated WHERE clause is a `ipe` TYPE ERROR
// (`String` where `SqlFragment` is expected) at `Db.findWhere` /
// `Db.deleteWhere`, never a runtime injection risk.
//
// Every combinator unconditionally parenthesizes its output (so composing
// `and`/`or`/`not` can never produce an ambiguous-precedence SQL string) and
// merges `binds` positionally with the `?` placeholders it emits — the two
// always stay in lockstep by construction.
//
// `invalid` is a poison marker: `sql_column` sets it on a malformed
// identifier instead of panicking or interpolating unchecked text; every
// combinator propagates the first poison it sees; the two consumers surface
// it as a `Task::Err` rather than emitting malformed SQL.

/// `Ipe.Db.Sql`'s opaque, parameterized WHERE-fragment value.
///
/// The derived `PartialEq` precondition is verified above: every `SqlParam`
/// field type is `PartialEq`, so this derive is total and structural,
/// comparing `sql` text + `binds` + `invalid` state — a meaningful equality
/// with no security concern (unlike `Secret`, nothing here is ever a secret
/// payload).
#[derive(Clone, PartialEq)]
pub struct SqlFragment {
    sql: String,
    binds: Vec<SqlParam>,
    invalid: Option<String>,
}

impl std::fmt::Debug for SqlFragment {
    /// SQL text + bind COUNT only — never bind VALUES. A bind may carry a
    /// revealed secret; this is the one place `SqlFragment` and `Secret`
    /// intersect, and it resolves the same way both items
    /// resolve elsewhere: safe by construction, no reliance on a caller
    /// remembering an escape hatch.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlFragment")
            .field("sql", &self.sql)
            .field("binds", &self.binds.len())
            .field("invalid", &self.invalid)
            .finish()
    }
}

/// `Sql.column : String -> SqlFragment` — a validated column/table reference.
/// Accepts dotted references (`users.id`) via [`valid_sql_ident`] (the
/// DOT-ACCEPTING validator — [`SqlIdent::parse`] is the table/column-name-only
/// validator used elsewhere in this module and rejects dots). An invalid
/// identifier poisons the fragment instead of panicking or interpolating
/// unchecked text.
///
/// Takes an owned `String` (not `&str`) to match every other Ipê-`String`-
/// typed kernel parameter in this module — the generic call-emission path
/// (`ipe_backend_rust::emit_expr`'s standard-path fallback) always produces an
/// owned `String` for a Ipê `String` argument, never a borrow.
pub fn sql_column(name: String) -> SqlFragment {
    if valid_sql_ident(&name) {
        SqlFragment {
            sql: name,
            binds: Vec::new(),
            invalid: None,
        }
    } else {
        SqlFragment {
            sql: String::new(),
            binds: Vec::new(),
            invalid: Some(format!("Sql.column: invalid identifier {name:?}")),
        }
    }
}

/// `Sql.param : SqlValue -> SqlFragment` — binds `v` as a single `?`
/// placeholder. Also the shared runtime symbol for `Sql.int` / `Sql.string` /
/// `Sql.float` / `Sql.bool`: each is a Ipê-level type narrowing of this same
/// generic entry point (`i64` / `String` / `f64` / `bool` all already have a
/// `From<T> for SqlParam` impl above), so no separate per-type runtime
/// function exists — see the kernel decl doc in `ipe_kernels`.
pub fn sql_param<T: Into<SqlParam>>(v: T) -> SqlFragment {
    SqlFragment {
        sql: "?".to_string(),
        binds: vec![v.into()],
        invalid: None,
    }
}

/// Shared implementation for the binary comparison/boolean combinators:
/// unconditional parens, positional bind merge, first-poison-wins.
fn sql_binop(op: &str, a: SqlFragment, b: SqlFragment) -> SqlFragment {
    let invalid = a.invalid.or(b.invalid);
    let mut binds = a.binds;
    binds.extend(b.binds);
    SqlFragment {
        sql: format!("({} {} {})", a.sql, op, b.sql),
        binds,
        invalid,
    }
}

/// `Sql.eq : SqlFragment -> SqlFragment -> SqlFragment`
pub fn sql_eq(a: SqlFragment, b: SqlFragment) -> SqlFragment {
    sql_binop("=", a, b)
}
/// `Sql.ne : SqlFragment -> SqlFragment -> SqlFragment`
pub fn sql_ne(a: SqlFragment, b: SqlFragment) -> SqlFragment {
    sql_binop("!=", a, b)
}
/// `Sql.gt : SqlFragment -> SqlFragment -> SqlFragment`
pub fn sql_gt(a: SqlFragment, b: SqlFragment) -> SqlFragment {
    sql_binop(">", a, b)
}
/// `Sql.lt : SqlFragment -> SqlFragment -> SqlFragment`
pub fn sql_lt(a: SqlFragment, b: SqlFragment) -> SqlFragment {
    sql_binop("<", a, b)
}
/// `Sql.gte : SqlFragment -> SqlFragment -> SqlFragment`
pub fn sql_gte(a: SqlFragment, b: SqlFragment) -> SqlFragment {
    sql_binop(">=", a, b)
}
/// `Sql.lte : SqlFragment -> SqlFragment -> SqlFragment`
pub fn sql_lte(a: SqlFragment, b: SqlFragment) -> SqlFragment {
    sql_binop("<=", a, b)
}
/// `Sql.and : SqlFragment -> SqlFragment -> SqlFragment`
pub fn sql_and(a: SqlFragment, b: SqlFragment) -> SqlFragment {
    sql_binop("AND", a, b)
}
/// `Sql.or : SqlFragment -> SqlFragment -> SqlFragment`
pub fn sql_or(a: SqlFragment, b: SqlFragment) -> SqlFragment {
    sql_binop("OR", a, b)
}

/// `Sql.not : SqlFragment -> SqlFragment`
pub fn sql_not(a: SqlFragment) -> SqlFragment {
    SqlFragment {
        sql: format!("(NOT {})", a.sql),
        binds: a.binds,
        invalid: a.invalid,
    }
}
/// `Sql.isNull : SqlFragment -> SqlFragment`
pub fn sql_is_null(a: SqlFragment) -> SqlFragment {
    SqlFragment {
        sql: format!("({} IS NULL)", a.sql),
        binds: a.binds,
        invalid: a.invalid,
    }
}
/// `Sql.isNotNull : SqlFragment -> SqlFragment`
pub fn sql_is_not_null(a: SqlFragment) -> SqlFragment {
    SqlFragment {
        sql: format!("({} IS NOT NULL)", a.sql),
        binds: a.binds,
        invalid: a.invalid,
    }
}

/// `Sql.like : SqlFragment -> String -> SqlFragment` — the pattern is always a
/// bound param (never interpolated), so `%`/`_` wildcards in untrusted input
/// stay data, never syntax.
pub fn sql_like(a: SqlFragment, pattern: String) -> SqlFragment {
    let mut binds = a.binds;
    binds.push(SqlParam::Text(pattern));
    SqlFragment {
        sql: format!("({} LIKE ?)", a.sql),
        binds,
        invalid: a.invalid,
    }
}

/// `Sql.inList : SqlFragment -> List SqlValue -> SqlFragment`. Empty `values`
/// emits `(1 = 0)` (always-false) rather than the SQL syntax error `IN ()` —
/// `a`'s own `sql` text is discarded in that case, so `a`'s binds (if any)
/// are dropped too (keeping the placeholder count and `binds` length in
/// lockstep); `a.invalid` still propagates so an upstream poisoned column
/// reference is not silently swallowed by the always-false shortcut.
pub fn sql_in_list(a: SqlFragment, values: Vec<SqlParam>) -> SqlFragment {
    if values.is_empty() {
        return SqlFragment {
            sql: "(1 = 0)".to_string(),
            binds: Vec::new(),
            invalid: a.invalid,
        };
    }
    let placeholders = vec!["?"; values.len()].join(", ");
    let mut binds = a.binds;
    binds.extend(values);
    SqlFragment {
        sql: format!("({} IN ({}))", a.sql, placeholders),
        binds,
        invalid: a.invalid,
    }
}

/// `Db.findWhere : Db -> String -> SqlFragment -> Task Error (List (Dict String String))`
/// — the `SqlFragment`-typed replacement for the removed `unsafeFindWhere`.
/// The WHERE clause can only be built through the `Sql.*` combinators above,
/// so `frag.sql` is always `?`-placeholder text with a matching `frag.binds`
/// list — there is no representable way to smuggle untrusted string content
/// into the SQL text.
pub fn db_find_where<E: Send + From<String> + 'static>(
    conn: Db,
    table: String,
    frag: SqlFragment,
) -> IpeTask<E, Vec<HashMap<String, String>>> {
    Box::pin(async move {
        if let Some(reason) = frag.invalid {
            return IpeResult::Err(format!("db.findWhere: {reason}").into());
        }
        let qtable = match SqlIdent::parse(&table) {
            Some(t) => t,
            None => {
                return IpeResult::Err(format!("db.findWhere: invalid table {:?}", table).into());
            }
        };
        let sql = db_format_sql(format!(
            "SELECT * FROM {} WHERE {}",
            qtable.as_str(),
            frag.sql
        ));
        let mut q = sqlx::query(&sql);
        for p in frag.binds {
            q = bind_sql_param(q, p);
        }
        match fetch_all_routed(&conn, q).await {
            Ok(rows) => ok_res(rows.iter().map(row_to_map).collect()),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

/// `Db.deleteWhere : Db -> String -> SqlFragment -> Task Error Int` — the
/// row-count deletion counterpart to [`db_find_where`].
pub fn db_delete_where<E: Send + From<String> + 'static>(
    conn: Db,
    table: String,
    frag: SqlFragment,
) -> IpeTask<E, i64> {
    Box::pin(async move {
        if let Some(reason) = frag.invalid {
            return IpeResult::Err(format!("db.deleteWhere: {reason}").into());
        }
        let qtable = match SqlIdent::parse(&table) {
            Some(t) => t,
            None => {
                return IpeResult::Err(format!("db.deleteWhere: invalid table {:?}", table).into());
            }
        };
        let sql = db_format_sql(format!(
            "DELETE FROM {} WHERE {}",
            qtable.as_str(),
            frag.sql
        ));
        let mut q = sqlx::query(&sql);
        for p in frag.binds {
            q = bind_sql_param(q, p);
        }
        match exec_routed(&conn, q).await {
            Ok(res) => ok_res(res.rows_affected() as i64),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

/// Shared logic for `db_insert_fields` and `db_insert_fields_returning`:
/// validates the table name and builds the INSERT SQL + bound-arg list.
///
/// `fields`: `Vec<(col_name, Option<SqlParam>)>` where `None` = OmitField
/// (column dropped from SQL; DB applies DEFAULT) and `Some(p)` = SetField(p).
///
/// Returns `(sql_without_returning, args)` on success, or
/// `IpeResult::Err` on invalid table/column name.  All-OmitField → returns
/// `"INSERT INTO t DEFAULT VALUES"` with an empty arg list (valid on SQLite ≥
/// 3.35 and PostgreSQL).
///
/// Security: table and column names are validated before interpolation.
/// Values are bound positionally — never interpolated.
#[cfg(feature = "db")]
fn build_insert_sql(
    kernel: &str,
    table: &str,
    fields: Vec<(String, Option<SqlParam>)>,
) -> Result<(String, Vec<SqlParam>), String> {
    if !valid_sql_ident(table) {
        return Err(format!("{}: invalid table name {:?}", kernel, table));
    }
    let mut cols: Vec<String> = Vec::new();
    let mut args: Vec<SqlParam> = Vec::new();
    for (col, opt) in fields {
        if !valid_sql_ident(&col) {
            return Err(format!("{}: invalid column name {:?}", kernel, col));
        }
        if let Some(p) = opt {
            cols.push(col);
            args.push(p);
        }
        // None → OmitField: column dropped entirely, DB applies DEFAULT.
    }
    let sql = if cols.is_empty() {
        format!("INSERT INTO {} DEFAULT VALUES", table)
    } else {
        let ph = vec!["?"; cols.len()].join(", ");
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table,
            cols.join(", "),
            ph
        )
    };
    Ok((sql, args))
}

/// `Db.insertFields : Db -> String -> List (String, SqlField) -> Task Error Int`
///
/// Builds a dynamic INSERT that includes only the `SetField` columns.
/// `OmitField` columns are dropped from the column list + VALUES clause so the
/// database applies their DEFAULT.  When every column is OmitField the runtime
/// emits `INSERT INTO <table> DEFAULT VALUES`.
///
/// Returns the inserted row's generated/provided id (lastInsertRowid on
/// sqlite; `RETURNING id` on Postgres, since Postgres's `QueryResult` carries
/// no last-insert-id concept — see [`DB_USES_RETURNING_ID`]).
///
/// Security: table + column names are identifier-validated `[A-Za-z0-9_.]`;
/// values are bound positionally — never interpolated into SQL.
/// Totality: every error path returns `IpeResult::Err`; no panic/unwrap. Never
/// fabricates `id = 0` on a non-integer primary key — surfaces a clear `Err`
/// instead (mirrors [`db_insert_row`]'s fix for the same bug class).
#[cfg(feature = "db")]
pub fn db_insert_fields<E: Send + From<String> + 'static>(
    conn: Db,
    table: String,
    fields: Vec<(String, Option<SqlParam>)>,
) -> IpeTask<E, i64> {
    Box::pin(async move {
        let (base_sql, args) = match build_insert_sql("db.insertFields", &table, fields) {
            Ok(v) => v,
            Err(e) => return IpeResult::Err(e.into()),
        };
        if DB_USES_RETURNING_ID {
            // Same rationale as `db_insert_row`: Postgres has no
            // LastInsertId, so recover the generated key via `RETURNING id`
            // instead of unconditionally calling `db_last_insert_id` (which
            // is a stub returning a fabricated `0` on the Postgres config
            // template — see `config_postgres.rs`).
            let sql = db_format_sql(format!("{base_sql} RETURNING id"));
            let mut q = sqlx::query(&sql);
            for p in args {
                q = bind_sql_param(q, p);
            }
            match fetch_one_routed(&conn, q).await {
                Ok(r) => match extract_returning_id(&r) {
                    Ok(id) => ok_res(id),
                    Err(msg) => IpeResult::Err(format!("db.insertFields: {msg}").into()),
                },
                Err(e) => IpeResult::Err(ipe_err(&e)),
            }
        } else {
            let sql = db_format_sql(base_sql);
            let mut q = sqlx::query(&sql);
            for p in args {
                q = bind_sql_param(q, p);
            }
            match exec_routed(&conn, q).await {
                Ok(res) => ok_res(db_last_insert_id(&res)),
                Err(e) => IpeResult::Err(ipe_err(&e)),
            }
        }
    })
}

/// `Db.updateFields : Db -> String -> List (String, SqlValue) -> List (String, SqlField) -> Task Error Int`
///
/// Builds a dynamic UPDATE that includes only the `SetField` columns in the SET
/// clause.  `OmitField` columns are skipped (DB keeps their existing value).
/// If every column in `set_fields` is OmitField, returns `Ok(0)` without
/// executing any SQL (no empty SET clause).
///
/// `where_cols` is a list of `(col, SqlValue)` pairs forming the WHERE clause
/// (AND-joined); an empty list means no WHERE clause (updates every row).
///
/// Security: table + column names are identifier-validated `[A-Za-z0-9_.]`;
/// values are bound positionally — never interpolated into SQL.
/// Totality: every error path returns `IpeResult::Err`; no panic/unwrap.
#[cfg(feature = "db")]
pub fn db_update_fields<E: Send + From<String> + 'static>(
    conn: Db,
    table: String,
    where_cols: Vec<(String, SqlParam)>,
    set_fields: Vec<(String, Option<SqlParam>)>,
) -> IpeTask<E, i64> {
    Box::pin(async move {
        if !valid_sql_ident(&table) {
            return IpeResult::Err(
                format!("db.updateFields: invalid table name {:?}", table).into(),
            );
        }
        // Build SET clause.
        let mut set_clauses: Vec<String> = Vec::new();
        let mut args: Vec<SqlParam> = Vec::new();
        for (col, opt) in set_fields {
            if !valid_sql_ident(&col) {
                return IpeResult::Err(
                    format!("db.updateFields: invalid SET column name {:?}", col).into(),
                );
            }
            if let Some(p) = opt {
                set_clauses.push(format!("{} = ?", col));
                args.push(p);
            }
            // None → OmitField: skip column.
        }
        if set_clauses.is_empty() {
            // Every column was OmitField — nothing to update. Go parity: return 0.
            return ok_res(0i64);
        }
        // Build WHERE clause.
        let mut where_clauses: Vec<String> = Vec::new();
        for (col, p) in where_cols {
            if !valid_sql_ident(&col) {
                return IpeResult::Err(
                    format!("db.updateFields: invalid WHERE column name {:?}", col).into(),
                );
            }
            where_clauses.push(format!("{} = ?", col));
            args.push(p);
        }
        // Refuse an unscoped UPDATE: an empty WHERE-column set would emit
        // `UPDATE <table> SET ...` with no WHERE, silently rewriting EVERY row
        // (a wrong-default footgun reachable when a request-derived WHERE list
        // comes back empty). Fail closed instead of mass-updating.
        if where_clauses.is_empty() {
            return IpeResult::Err(
                "db.updateFields: refusing unscoped UPDATE (no WHERE); pass an explicit condition"
                    .to_string()
                    .into(),
            );
        }
        let sql = format!(
            "UPDATE {} SET {} WHERE {}",
            table,
            set_clauses.join(", "),
            where_clauses.join(" AND ")
        );
        let sql = db_format_sql(sql);
        let mut q = sqlx::query(&sql);
        for p in args {
            q = bind_sql_param(q, p);
        }
        match exec_routed(&conn, q).await {
            Ok(res) => ok_res(res.rows_affected() as i64),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

/// `Db.insertFieldsReturning : Db -> String -> List (String, SqlField) -> String -> Decoder a -> Task Error (List a)`
///
/// Builds the same OmitField-aware INSERT as `db_insert_fields`, appends
/// `RETURNING <projection>`, runs it through `fetch_all`, and decodes each
/// returned row via the `Decoder<E,A>` (using `row_to_json` — NULL-preserving).
///
/// The `projection` string is caller-controlled but VALIDATED for injection
/// safety (stricter than Go): it must be `"*"` or a comma-separated list of
/// plain identifiers (`col` / `table.col`, chars `[A-Za-z0-9_.]` only). Arbitrary
/// SQL expressions and `AS` aliases are intentionally REJECTED (`Err`), as is an
/// empty projection.
///
/// Requires SQLite ≥ 3.35 (Mar 2021) or PostgreSQL — same requirement as
/// other RETURNING uses already in Ipe.Db.
///
/// Security: table + column names validated; values bound positionally; only
/// the RETURNING projection is caller-supplied (and it's not executed as DML,
/// so the risk class is different — same as `queryDecode`'s SQL string trust model).
/// Totality: every error path returns `IpeResult::Err`; no panic/unwrap.
#[cfg(feature = "db")]
pub fn db_insert_fields_returning<E: Send + From<String> + 'static, A: Send + 'static>(
    conn: Db,
    table: String,
    fields: Vec<(String, Option<SqlParam>)>,
    projection: String,
    decoder: Decoder<E, A>,
) -> IpeTask<E, Vec<A>> {
    Box::pin(async move {
        if projection.is_empty() {
            return IpeResult::Err(
                "db.insertFieldsReturning: empty RETURNING projection"
                    .to_string()
                    .into(),
            );
        }
        let (base_sql, args) = match build_insert_sql("db.insertFieldsReturning", &table, fields) {
            Ok(v) => v,
            Err(e) => return IpeResult::Err(e.into()),
        };
        // Validate the RETURNING projection — it is a caller-supplied String
        // interpolated into SQL. Allow "*" or a comma-separated list of valid
        // identifiers (col / table.col); reject anything else (SQL injection).
        let proj = projection.trim();
        let proj_ok = proj == "*" || proj.split(',').all(|t| valid_sql_ident(t.trim()));
        if !proj_ok {
            return IpeResult::Err(
                format!(
                    "db.insertFieldsReturning: invalid RETURNING projection {:?}",
                    projection
                )
                .into(),
            );
        }
        let sql = db_format_sql(format!("{} RETURNING {}", base_sql, projection));
        let mut q = sqlx::query(&sql);
        for p in args {
            q = bind_sql_param(q, p);
        }
        let rows = match fetch_all_routed(&conn, q).await {
            Ok(r) => r,
            Err(e) => return IpeResult::Err(ipe_err(&e)),
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let jv = match row_to_json(row) {
                Ok(v) => v,
                Err(e) => return IpeResult::Err(ipe_err(&e)),
            };
            match (decoder.run)(&jv) {
                IpeResult::Ok(a) => out.push(a),
                IpeResult::Err(e) => return IpeResult::Err(e),
            }
        }
        ok_res(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_is_cacheable_bare_memory_is_not_cacheable() {
        assert!(!url_is_cacheable(":memory:"));
        assert!(!url_is_cacheable("sqlite::memory:"));
        assert!(!url_is_cacheable("sqlite://:memory:"));
    }

    #[test]
    fn url_is_cacheable_mode_memory_without_shared_cache_is_not_cacheable() {
        assert!(!url_is_cacheable("file:foo.db?mode=memory"));
    }

    #[test]
    fn url_is_cacheable_mode_memory_with_shared_cache_is_cacheable() {
        // `cache=shared` mode=memory URLs are a shared named in-memory DB —
        // multiple connections to the SAME url ARE the same database, so
        // pooling is correct here (this is the regression this fix must NOT
        // break: don't overcorrect to "any mode=memory is uncacheable").
        assert!(url_is_cacheable("file:foo.db?mode=memory&cache=shared"));
    }

    #[test]
    fn url_is_cacheable_filename_containing_memory_substring_is_cacheable() {
        // The DoS-reopen regression: a legitimate file path containing the
        // substring "memory" must NOT be excluded from pooling.
        assert!(url_is_cacheable("sqlite://data/memory_bank.db?mode=rwc"));
        assert!(url_is_cacheable("sqlite:./memory_backup.sqlite"));
    }

    // Soundness: SQLite's own documented idiom wraps `:memory:` in a `file:`
    // sub-scheme (sqlite.org/inmemorydb.html —
    // `sqlite3_open("file::memory:?cache=shared", ...)`). Stripping only the
    // outer `sqlite:`/`sqlite://` scheme and not the inner `file:` one would
    // let `"file::memory:"` fall through to the default `true` (cacheable)
    // branch — silently pooling what SQLite treats as two DISTINCT private
    // databases. Per sqlite.org's documented semantics: a bare `:memory:` name
    // is unconditionally private (not URI-parsed, so `cache=shared` has no
    // effect on it even if present); only the `file:`-wrapped URI form honours
    // `cache=shared`.
    #[test]
    fn url_is_cacheable_file_wrapped_memory_without_shared_cache_is_not_cacheable() {
        assert!(!url_is_cacheable("file::memory:"));
        assert!(!url_is_cacheable("sqlite://file::memory:"));
    }

    #[test]
    fn url_is_cacheable_file_wrapped_memory_with_shared_cache_is_cacheable() {
        assert!(url_is_cacheable("file::memory:?cache=shared"));
    }

    // the IpeRow accessor is total over a Dict-shaped row — present field
    // reads back, absent field is "" (never panics), int/bool parse + default.
    #[test]
    fn ipe_row_hashmap_total() {
        let mut m: HashMap<String, String> = HashMap::new();
        m.insert("text".into(), "ping".into());
        m.insert("count".into(), "42".into());
        m.insert("flag".into(), "true".into());
        assert_eq!(db_get_string("text".into(), &m), "ping");
        assert_eq!(db_get_string("missing".into(), &m), "");
        assert_eq!(db_get_int("count".into(), &m), 42);
        assert_eq!(db_get_int("missing".into(), &m), 0);
        assert!(db_get_bool("flag".into(), &m));
        assert!(!db_get_bool("missing".into(), &m));
    }

    // `Db.getString "path" req` on an `init` handler's typed
    // request reads the named struct field; params/headers/cookies back any
    // other key; absent -> "" (total).
    #[cfg(feature = "web")]
    #[test]
    fn ipe_row_livereq_named_fields_and_dicts() {
        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("slug".into(), "general".into());
        let mut cookies: HashMap<String, String> = HashMap::new();
        cookies.insert("ipe_sid".into(), "abc".into());
        let req = crate::WebReq {
            path: "/chat/general".into(),
            query: "x=1".into(),
            method: "GET".into(),
            params,
            headers: HashMap::new(),
            cookies,
        };
        assert_eq!(db_get_string("path".into(), &req), "/chat/general");
        assert_eq!(db_get_string("method".into(), &req), "GET");
        assert_eq!(db_get_string("query".into(), &req), "x=1");
        assert_eq!(db_get_string("slug".into(), &req), "general"); // params
        assert_eq!(db_get_string("ipe_sid".into(), &req), "abc"); // cookies
        assert_eq!(db_get_string("nope".into(), &req), ""); // absent -> total ""
    }

    async fn fresh_db() -> Db {
        // A SINGLE persistent connection per test. `sqlite::memory:` gives each
        // pool connection its OWN in-memory database, so a default multi-conn
        // pool routes BEGIN / INSERT / COMMIT / SELECT to different (empty) DBs
        // — the source of a parallel-run flake: under load the pool
        // opens extra connections and ops miss the table / committed row.
        // min=max=1 pins one connection (one DB, table + transactions always
        // visible); each test still gets its own isolated pool.
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        sqlx::query("CREATE TABLE todos (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0)")
            .execute(&pool).await.expect("create table");
        pool
    }

    #[tokio::test]
    async fn ipe_err_redacts_db_row_values() {
        // A UNIQUE-constraint failure must NOT echo the offending row VALUE into
        // the Ipê-visible Error (PRINCIPLES #1 info-leak). `ipe_err` builds a
        // structural message (SQLSTATE/driver code + constraint name) from the
        // structured error fields instead of the raw Display, which on
        // PostgreSQL/MySQL embeds `Key (email)=(victim@…) already exists`.
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        sqlx::query("CREATE TABLE secrets (email TEXT UNIQUE NOT NULL)")
            .execute(&pool)
            .await
            .expect("create table");
        let secret = "victim-PII@example.com";
        let insert = format!("INSERT INTO secrets (email) VALUES ('{}')", secret);
        let r1: IpeResult<String, i64> = db_exec(pool.clone(), insert.clone(), Vec::new()).await;
        assert!(
            matches!(r1, IpeResult::Ok(_)),
            "first insert should succeed"
        );
        let r2: IpeResult<String, i64> = db_exec(pool.clone(), insert, Vec::new()).await;
        match r2 {
            IpeResult::Err(e) => {
                assert!(!e.contains(secret), "row value leaked into db error: {e}");
                assert!(
                    e.starts_with("db: database error"),
                    "expected redacted structural form, got: {e}"
                );
            }
            IpeResult::Ok(_) => panic!("duplicate insert should violate the UNIQUE constraint"),
        }
    }

    #[test]
    fn migrate_checksum_is_lowercase_sha256_hex_matching_go() {
        // G4 pin: the ledger checksum is a cross-backend DB contract. This value
        // is `sha256hex("SELECT 1;")` — identical to Go's
        // fmt.Sprintf("%x", sha256.Sum256([]byte("SELECT 1;"))). A future hasher
        // swap that broke cross-backend ledger interop would fail HERE.
        assert_eq!(
            super::migrate_checksum("SELECT 1;"),
            "17db4fd369edb9244b9f91d9aeed145c3d04ad8ba6e95d06247f07a63527d11a"
        );
    }

    #[tokio::test]
    async fn migrate_is_idempotent_and_drift_guarded() {
        let db = fresh_db().await;
        let base = vec![
            (
                "001_users".to_string(),
                "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT)".to_string(),
            ),
            (
                "002_email_idx".to_string(),
                "CREATE INDEX idx_users_email ON users(email)".to_string(),
            ),
        ];

        // First run applies both, in declaration order.
        let r1: IpeResult<String, Vec<String>> = db_migrate_apply(db.clone(), base.clone()).await;
        match r1 {
            IpeResult::Ok(v) => assert_eq!(
                v,
                vec!["001_users".to_string(), "002_email_idx".to_string()]
            ),
            IpeResult::Err(e) => panic!("first migrate: {e}"),
        }

        // Second run is idempotent — both already applied → 0 applied.
        let r2: IpeResult<String, Vec<String>> = db_migrate_apply(db.clone(), base.clone()).await;
        match r2 {
            IpeResult::Ok(v) => assert!(v.is_empty(), "expected 0 applied on re-run, got {v:?}"),
            IpeResult::Err(e) => panic!("idempotent re-run: {e}"),
        }

        // Ledger recorded exactly the two migrations.
        let ledger: IpeResult<String, Vec<HashMap<String, String>>> = db_query(
            db.clone(),
            "SELECT name, checksum FROM _ipe_migrations ORDER BY name".to_string(),
            Vec::new(),
        )
        .await;
        match ledger {
            IpeResult::Ok(rows) => assert_eq!(rows.len(), 2, "ledger rows: {rows:?}"),
            IpeResult::Err(e) => panic!("read ledger: {e}"),
        }

        // Drift: same name, edited SQL → checksum-mismatch error, nothing applied.
        let drift = vec![(
            "001_users".to_string(),
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT, name TEXT)".to_string(),
        )];
        let r3: IpeResult<String, Vec<String>> = db_migrate_apply(db.clone(), drift).await;
        match r3 {
            IpeResult::Err(e) => assert!(
                e.contains("checksum mismatch"),
                "expected drift error, got: {e}"
            ),
            IpeResult::Ok(v) => panic!("expected drift error, but applied {v:?}"),
        }

        // Adding a NEW migration after the applied ones resumes — only it applies.
        let mut extended = base.clone();
        extended.push((
            "003_posts".to_string(),
            "CREATE TABLE posts (id INTEGER PRIMARY KEY)".to_string(),
        ));
        let r4: IpeResult<String, Vec<String>> = db_migrate_apply(db.clone(), extended).await;
        match r4 {
            IpeResult::Ok(v) => assert_eq!(v, vec!["003_posts".to_string()]),
            IpeResult::Err(e) => panic!("resume migrate: {e}"),
        }
    }

    #[tokio::test]
    async fn exec_query_params_bind_mixed_sqlvalue_types() {
        // db_exec_params / db_query_params bind the full SqlParam range (the
        // Go `List SqlValue` mixed-type path) — Text/Int/Bool/Float/Null — and
        // round-trip through a SqlValue-param WHERE. `with_default` extracts the
        // Ok value (a wrong/Err result then fails the following assert).
        let db = fresh_db().await;
        // exec/execRaw now return rows-affected (i64). DDL rows-affected is
        // driver-defined → assert Ok(_); each INSERT affects exactly 1 row.
        let mk: IpeResult<String, i64> = db_exec_raw(
            db.clone(),
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT, qty INTEGER, \
             active INTEGER, price REAL)"
                .to_string(),
        )
        .await;
        assert!(matches!(mk, IpeResult::Ok(_)), "create: {mk:?}");

        let ins: IpeResult<String, i64> = db_exec_params(
            db.clone(),
            "INSERT INTO items (name, qty, active, price) VALUES (?, ?, ?, ?)".to_string(),
            vec![
                SqlParam::Text("widget".to_string()),
                SqlParam::Int(7),
                SqlParam::Bool(true),
                SqlParam::Float(9.99),
            ],
        )
        .await;
        assert!(matches!(ins, IpeResult::Ok(1)), "mixed insert: {ins:?}");

        // A row with typed NULLs (SqlNull carries a type witness so the
        // NULL binds with the right driver type-OID — see SqlParam::Null's
        // doc comment). `name` is TEXT, `price` is REAL: witness each with
        // the matching leaf variant.
        let ins2: IpeResult<String, i64> = db_exec_params(
            db.clone(),
            "INSERT INTO items (name, qty, active, price) VALUES (?, ?, ?, ?)".to_string(),
            vec![
                SqlParam::Null(Box::new(SqlParam::Text(String::new()))),
                SqlParam::Int(0),
                SqlParam::Bool(false),
                SqlParam::Null(Box::new(SqlParam::Float(0.0))),
            ],
        )
        .await;
        assert!(matches!(ins2, IpeResult::Ok(1)), "null insert: {ins2:?}");

        // SELECT with an Int SqlValue param.
        let rows: IpeResult<String, Vec<HashMap<String, String>>> = db_query_params(
            db.clone(),
            "SELECT name, qty FROM items WHERE qty = ?".to_string(),
            vec![SqlParam::Int(7)],
        )
        .await;
        let rs = rows.with_default(Vec::new());
        assert_eq!(rs.len(), 1, "expected exactly 1 matching row, got {rs:?}");
        if let Some(r) = rs.first() {
            assert_eq!(r.get("name").map(String::as_str), Some("widget"));
            assert_eq!(r.get("qty").map(String::as_str), Some("7"));
        }
    }

    #[tokio::test]
    async fn test_insert_get_by_id() {
        let db = fresh_db().await;
        let mut row = HashMap::new();
        row.insert("title".to_string(), "buy milk".to_string());
        row.insert("done".to_string(), "0".to_string());
        let id: IpeResult<String, i64> = db_insert_row(db.clone(), "todos".into(), row).await;
        let id = match id {
            IpeResult::Ok(v) => v,
            IpeResult::Err(e) => panic!("{}", e),
        };
        assert!(id > 0);

        let fetched: IpeResult<String, IpeMaybe<HashMap<String, String>>> =
            db_get_by_id(db, "todos".into(), id.to_string()).await;
        match fetched {
            IpeResult::Ok(IpeMaybe::Just(m)) => assert_eq!(m.get("title").unwrap(), "buy milk"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    /// Tier-1 regression for the `db_insert_row`/`db_insert_fields`
    /// fabricated-`id = 0` fix (Class 7 §4b). `DB_USES_RETURNING_ID` is
    /// `false` on the standalone sqlite build, so `extract_returning_id` is
    /// called directly here — bypassing the `if DB_USES_RETURNING_ID` gate —
    /// against a REAL SQLite `RETURNING id` row with a non-integer (`TEXT`)
    /// PK. This exercises the exact decode-miss path without needing a live
    /// Postgres (`DB_USES_RETURNING_ID = true` is only reachable once the §3
    /// Postgres driver template is selected by a real project build).
    #[tokio::test]
    async fn extract_returning_id_errs_on_non_integer_pk() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        sqlx::query("CREATE TABLE t (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .expect("create table");
        let row = fetch_one_routed(
            &pool,
            sqlx::query("INSERT INTO t (id) VALUES ('non-integer-pk') RETURNING id"),
        )
        .await
        .expect("insert should succeed");
        assert!(
            extract_returning_id(&row).is_err(),
            "a non-integer id column must surface Err, never a fabricated 0"
        );
    }

    #[tokio::test]
    async fn extract_returning_id_ok_on_integer_pk() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .expect("create table");
        let row = fetch_one_routed(
            &pool,
            sqlx::query("INSERT INTO t (id) VALUES (42) RETURNING id"),
        )
        .await
        .expect("insert should succeed");
        assert_eq!(extract_returning_id(&row), Ok(42));
    }

    #[tokio::test]
    async fn test_update_by_id() {
        let db = fresh_db().await;
        let mut row = HashMap::new();
        row.insert("title".to_string(), "x".to_string());
        let id: IpeResult<String, i64> = db_insert_row(db.clone(), "todos".into(), row).await;
        let id = match id {
            IpeResult::Ok(v) => v,
            _ => panic!("insert"),
        };

        let mut updates = HashMap::new();
        updates.insert("title".to_string(), "y".to_string());
        let affected: IpeResult<String, i64> =
            db_update_by_id(db.clone(), "todos".into(), id.to_string(), updates).await;
        assert!(matches!(affected, IpeResult::Ok(1)));
    }

    #[tokio::test]
    async fn test_delete_by_id() {
        let db = fresh_db().await;
        let mut row = HashMap::new();
        row.insert("title".to_string(), "z".to_string());
        let id: IpeResult<String, i64> = db_insert_row(db.clone(), "todos".into(), row).await;
        let id = match id {
            IpeResult::Ok(v) => v,
            _ => panic!("insert"),
        };
        let affected: IpeResult<String, i64> =
            db_delete_by_id(db, "todos".into(), id.to_string()).await;
        assert!(matches!(affected, IpeResult::Ok(1)));
    }

    #[tokio::test]
    async fn test_find_one_by_field() {
        let db = fresh_db().await;
        let mut row = HashMap::new();
        row.insert("title".to_string(), "find me".to_string());
        let _: IpeResult<String, i64> = db_insert_row(db.clone(), "todos".into(), row).await;
        let found: IpeResult<String, IpeMaybe<HashMap<String, String>>> =
            db_find_one_by_field(db, "todos".into(), "title".into(), "find me".into()).await;
        assert!(matches!(found, IpeResult::Ok(IpeMaybe::Just(_))));
    }

    #[tokio::test]
    async fn test_find_many_and_by_conditions() {
        let db = fresh_db().await;
        for t in ["a", "b", "c"] {
            let mut r = HashMap::new();
            r.insert("title".to_string(), t.to_string());
            r.insert("done".to_string(), "1".to_string());
            let _: IpeResult<String, i64> = db_insert_row(db.clone(), "todos".into(), r).await;
        }
        let many: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_many_by_field(db.clone(), "todos".into(), "done".into(), "1".into()).await;
        match many {
            IpeResult::Ok(v) => assert_eq!(v.len(), 3),
            _ => panic!("find many"),
        }

        let mut cond = HashMap::new();
        cond.insert("done".to_string(), "1".to_string());
        cond.insert("title".to_string(), "b".to_string());
        let one: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_by_conditions(db.clone(), "todos".into(), cond).await;
        match one {
            IpeResult::Ok(v) => assert_eq!(v.len(), 1),
            _ => panic!("conds"),
        }

        // Empty condition set MUST be refused (would otherwise return every
        // row — a cross-tenant read when request-derived filters come back empty).
        let empty_cond: HashMap<String, String> = HashMap::new();
        let refused: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_by_conditions(db.clone(), "todos".into(), empty_cond).await;
        assert!(
            matches!(refused, IpeResult::Err(_)),
            "empty conditions must be refused, got {refused:?}"
        );

        // Non-empty conditions still return filtered rows (happy-path regression).
        let mut only_done = HashMap::new();
        only_done.insert("done".to_string(), "1".to_string());
        let filtered: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_by_conditions(db, "todos".into(), only_done).await;
        match filtered {
            IpeResult::Ok(v) => assert_eq!(v.len(), 3, "expected 3 done rows"),
            _ => panic!("non-empty conditions should return rows"),
        }
    }

    #[tokio::test]
    async fn test_with_transaction_commit() {
        let db = fresh_db().await;
        let r: IpeResult<String, i64> = db_with_transaction(db.clone(), |c| {
            Box::pin(async move {
                let mut row = HashMap::new();
                row.insert("title".to_string(), "txn".to_string());
                db_insert_row(c, "todos".into(), row).await
            })
        })
        .await;
        assert!(matches!(r, IpeResult::Ok(_)));
        // The inserted row should be visible after commit:
        let found: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_many_by_field(db, "todos".into(), "title".into(), "txn".into()).await;
        match found {
            IpeResult::Ok(v) => assert_eq!(v.len(), 1),
            _ => panic!("post-commit fetch"),
        }
    }

    #[tokio::test]
    async fn test_with_transaction_rollback_returns_err() {
        // Err propagates AND the write is actually undone. With the task-local
        // dedicated-connection routing, BEGIN / INSERT / ROLLBACK all run on the
        // same connection, so the row is gone after rollback (single-conn pool).
        let db = fresh_db().await;
        let r: IpeResult<String, i64> = db_with_transaction(db.clone(), |c| {
            Box::pin(async move {
                let mut row = HashMap::new();
                row.insert("title".to_string(), "txn-err".to_string());
                let _: IpeResult<String, i64> = db_insert_row(c, "todos".into(), row).await;
                IpeResult::Err("boom".to_string())
            })
        })
        .await;
        assert!(matches!(r, IpeResult::Err(_)));
        let found: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_many_by_field(db, "todos".into(), "title".into(), "txn-err".into()).await;
        match found {
            IpeResult::Ok(v) => assert_eq!(v.len(), 0, "rollback must undo the INSERT"),
            _ => panic!("post-rollback fetch"),
        }
    }

    // Build a FILE-based sqlite pool (temp file, NOT `:memory:` — in-memory
    // sqlite is per-connection so it can't exhibit the cross-connection bug)
    // with `max_connections > 1` and WAL. Returns (pool, tempdir-guard); the
    // guard must outlive the pool so the file isn't deleted early.
    async fn fresh_file_db(max_conns: u32) -> (Db, std::path::PathBuf) {
        let mut path = std::env::temp_dir();
        // Unique per test run to avoid cross-test contamination.
        let unique = format!(
            "ipe_txn_test_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        path.push(unique);
        // Fresh file every time.
        let _ = std::fs::remove_file(&path);
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(max_conns)
            .connect(&url)
            .await
            .expect("connect file sqlite");
        // WAL: concurrent readers alongside a single writer.
        let _ = sqlx::query("PRAGMA journal_mode=WAL;").execute(&pool).await;
        let _ = sqlx::query("PRAGMA busy_timeout=5000;")
            .execute(&pool)
            .await;
        sqlx::query("CREATE TABLE todos (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0)")
            .execute(&pool).await.expect("create table");
        (pool, path)
    }

    // THE REGRESSION GATE. On a MULTI-connection (5) file-backed pool, a
    // withTransaction body that INSERTs then returns Err must roll the INSERT
    // back. Against the old bare-pool code BEGIN/INSERT/ROLLBACK scattered across
    // different connections → the INSERT autocommitted on its own connection →
    // this assert would find the row present (FAIL). With task-local routing all
    // three run on one connection → row absent (PASS).
    #[tokio::test]
    async fn test_with_transaction_cancellation_rolls_back() {
        // CANCELLATION SAFETY regression: a body future DROPPED mid-transaction
        // (here via task abort) must NOT leak an open txn onto the pooled
        // connection — the next checkout would otherwise inherit it. 1-conn pool
        // forces reuse of the exact connection the cancelled txn ran on.
        let (db, path) = fresh_file_db(1).await;
        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let started2 = started.clone();
        let dbc = db.clone();
        let handle = tokio::spawn(async move {
            let _: IpeResult<String, i64> = db_with_transaction(dbc, move |c| {
                let started2 = started2.clone();
                Box::pin(async move {
                    let mut row = HashMap::new();
                    row.insert("title".to_string(), "cancelled".to_string());
                    let _: IpeResult<String, i64> = db_insert_row(c, "todos".into(), row).await;
                    started2.notify_one(); // INSERT is in the open txn — signal, then hang
                    std::future::pending::<()>().await; // dropped by abort below
                    IpeResult::Ok(0)
                })
            })
            .await;
        });
        started.notified().await;
        handle.abort();
        let _ = handle.await;

        // Reused connection must NOT be poisoned by an inherited open txn.
        let r: IpeResult<String, i64> = db_with_transaction(db.clone(), |c| {
            Box::pin(async move {
                let mut row = HashMap::new();
                row.insert("title".to_string(), "after".to_string());
                db_insert_row(c, "todos".into(), row).await
            })
        })
        .await;
        assert!(
            matches!(r, IpeResult::Ok(_)),
            "post-cancel txn must succeed on the reused connection: {:?}",
            r
        );
        // The cancelled INSERT must have rolled back on drop (fold to a count to
        // avoid a panic!-form assertion — the risk-precheck flags raw panic!).
        let cancelled_count = match db_find_many_by_field::<String>(
            db.clone(),
            "todos".into(),
            "title".into(),
            "cancelled".into(),
        )
        .await
        {
            IpeResult::Ok(v) => v.len(),
            IpeResult::Err(_) => usize::MAX,
        };
        assert_eq!(
            cancelled_count, 0,
            "cancelled INSERT must roll back on drop"
        );
        db.close().await;
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_with_transaction_rollback_real_on_multiconn_pool() {
        let (db, path) = fresh_file_db(5).await;

        let r: IpeResult<String, i64> = db_with_transaction(db.clone(), |c| {
            Box::pin(async move {
                let mut row = HashMap::new();
                row.insert("title".to_string(), "rollback-me".to_string());
                let _: IpeResult<String, i64> = db_insert_row(c, "todos".into(), row).await;
                IpeResult::Err("forced rollback".to_string())
            })
        })
        .await;
        assert!(matches!(r, IpeResult::Err(_)), "body Err propagates");

        // The row MUST be absent — rollback actually undid the write.
        let found: IpeResult<String, Vec<HashMap<String, String>>> = db_find_many_by_field(
            db.clone(),
            "todos".into(),
            "title".into(),
            "rollback-me".into(),
        )
        .await;
        match found {
            IpeResult::Ok(v) => assert_eq!(
                v.len(),
                0,
                "ROLLBACK did not undo the INSERT on a multi-connection pool — \
                 BEGIN/INSERT/ROLLBACK landed on different connections"
            ),
            other => panic!("post-rollback fetch: {:?}", other),
        }

        db.close().await;
        let _ = std::fs::remove_file(&path);
    }

    // Ok-path on a multi-connection file pool: COMMIT must persist the row.
    #[tokio::test]
    async fn test_with_transaction_commit_real_on_multiconn_pool() {
        let (db, path) = fresh_file_db(5).await;

        let r: IpeResult<String, i64> = db_with_transaction(db.clone(), |c| {
            Box::pin(async move {
                let mut row = HashMap::new();
                row.insert("title".to_string(), "commit-me".to_string());
                db_insert_row(c, "todos".into(), row).await
            })
        })
        .await;
        assert!(matches!(r, IpeResult::Ok(_)), "body Ok");

        let found: IpeResult<String, Vec<HashMap<String, String>>> = db_find_many_by_field(
            db.clone(),
            "todos".into(),
            "title".into(),
            "commit-me".into(),
        )
        .await;
        match found {
            IpeResult::Ok(v) => assert_eq!(v.len(), 1, "COMMIT must persist the row"),
            other => panic!("post-commit fetch: {:?}", other),
        }

        db.close().await;
        let _ = std::fs::remove_file(&path);
    }

    // Nested withTransaction must NOT deadlock and must NOT acquire a second
    // connection. Flattened semantics: the inner block runs on the outer
    // transaction's connection; an outer Err rolls everything back.
    #[tokio::test]
    async fn test_with_transaction_nested_no_deadlock() {
        let (db, path) = fresh_file_db(5).await;
        let db_for_inner = db.clone();

        let r: IpeResult<String, i64> = db_with_transaction(db.clone(), move |c| {
            let inner_db = db_for_inner.clone();
            Box::pin(async move {
                let mut row = HashMap::new();
                row.insert("title".to_string(), "outer".to_string());
                let _: IpeResult<String, i64> = db_insert_row(c, "todos".into(), row).await;
                // Nested call — must reuse the held connection (no deadlock).
                db_with_transaction(inner_db, |c2| {
                    Box::pin(async move {
                        let mut row2 = HashMap::new();
                        row2.insert("title".to_string(), "inner".to_string());
                        db_insert_row(c2, "todos".into(), row2).await
                    })
                })
                .await
            })
        })
        .await;
        assert!(matches!(r, IpeResult::Ok(_)), "nested commit Ok");

        // Both rows committed (flattened into one transaction).
        let outer: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_many_by_field(db.clone(), "todos".into(), "title".into(), "outer".into()).await;
        let inner: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_many_by_field(db.clone(), "todos".into(), "title".into(), "inner".into()).await;
        assert!(
            matches!(outer, IpeResult::Ok(ref v) if v.len() == 1),
            "outer row present"
        );
        assert!(
            matches!(inner, IpeResult::Ok(ref v) if v.len() == 1),
            "inner row present"
        );

        db.close().await;
        let _ = std::fs::remove_file(&path);
    }

    // AUD-03 regression: a nested `withTransaction` call for a DIFFERENT `Db`
    // handle must open its OWN independent transaction, never flatten onto an
    // outer transaction opened on a different pool (which would silently
    // execute the nested pool's operations against the wrong physical
    // connection — cross-database data corruption). Both tests below FAIL
    // under the pre-fix code (which flattened on ANY active transaction
    // regardless of pool identity) and pass once nesting is gated on
    // `current_txn_conn_for`'s pool-identity check.
    #[tokio::test]
    async fn test_with_transaction_cross_pool_nested_targets_correct_db() {
        let (db_a, path_a) = fresh_file_db(5).await;
        let (db_b, path_b) = fresh_file_db(5).await;
        let db_b_for_inner = db_b.clone();

        let r: IpeResult<String, i64> = db_with_transaction(db_a.clone(), move |c_a| {
            let db_b_inner = db_b_for_inner.clone();
            Box::pin(async move {
                let mut row_a = HashMap::new();
                row_a.insert("title".to_string(), "in-a".to_string());
                let _: IpeResult<String, i64> = db_insert_row(c_a, "todos".into(), row_a).await;

                // Nested withTransaction on a DIFFERENT pool — must open its
                // own transaction on db_b, not flatten onto db_a's.
                db_with_transaction(db_b_inner, |c_b| {
                    Box::pin(async move {
                        let mut row_b = HashMap::new();
                        row_b.insert("title".to_string(), "in-b".to_string());
                        db_insert_row(c_b, "todos".into(), row_b).await
                    })
                })
                .await
            })
        })
        .await;
        assert!(matches!(r, IpeResult::Ok(_)), "outer+nested commit Ok");

        // The dbB row must land in dbB, NOT dbA.
        let a_has_b_row: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_many_by_field(db_a.clone(), "todos".into(), "title".into(), "in-b".into())
                .await;
        assert!(
            matches!(a_has_b_row, IpeResult::Ok(ref v) if v.is_empty()),
            "dbA must NOT contain dbB's row"
        );

        let b_has_b_row: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_many_by_field(db_b.clone(), "todos".into(), "title".into(), "in-b".into())
                .await;
        assert!(
            matches!(b_has_b_row, IpeResult::Ok(ref v) if v.len() == 1),
            "dbB must contain its own row"
        );

        let a_has_a_row: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_many_by_field(db_a.clone(), "todos".into(), "title".into(), "in-a".into())
                .await;
        assert!(
            matches!(a_has_a_row, IpeResult::Ok(ref v) if v.len() == 1),
            "dbA must contain its own row"
        );

        db_a.close().await;
        db_b.close().await;
        let _ = std::fs::remove_file(&path_a);
        let _ = std::fs::remove_file(&path_b);
    }

    #[tokio::test]
    async fn test_with_transaction_cross_pool_nested_rollback_independent() {
        let (db_a, path_a) = fresh_file_db(5).await;
        let (db_b, path_b) = fresh_file_db(5).await;
        let db_b_for_inner = db_b.clone();

        let r: IpeResult<String, i64> = db_with_transaction(db_a.clone(), move |c_a| {
            let db_b_inner = db_b_for_inner.clone();
            Box::pin(async move {
                let mut row_a = HashMap::new();
                row_a.insert("title".to_string(), "a-commits".to_string());
                let _: IpeResult<String, i64> = db_insert_row(c_a, "todos".into(), row_a).await;

                // Inner transaction on a DIFFERENT pool fails and rolls back —
                // must NOT roll back the outer dbA transaction.
                let inner: IpeResult<String, i64> = db_with_transaction(db_b_inner, |c_b| {
                    Box::pin(async move {
                        let mut row_b = HashMap::new();
                        row_b.insert("title".to_string(), "b-rolls-back".to_string());
                        let _: IpeResult<String, i64> =
                            db_insert_row(c_b, "todos".into(), row_b).await;
                        IpeResult::<String, i64>::Err("inner fails deliberately".to_string())
                    })
                })
                .await;
                assert!(
                    matches!(inner, IpeResult::Err(_)),
                    "inner reports its own error"
                );

                IpeResult::Ok(0i64)
            })
        })
        .await;
        assert!(
            matches!(r, IpeResult::Ok(_)),
            "outer commit Ok despite inner rollback"
        );

        let a_row: IpeResult<String, Vec<HashMap<String, String>>> = db_find_many_by_field(
            db_a.clone(),
            "todos".into(),
            "title".into(),
            "a-commits".into(),
        )
        .await;
        assert!(
            matches!(a_row, IpeResult::Ok(ref v) if v.len() == 1),
            "dbA row committed"
        );

        let b_row: IpeResult<String, Vec<HashMap<String, String>>> = db_find_many_by_field(
            db_b.clone(),
            "todos".into(),
            "title".into(),
            "b-rolls-back".into(),
        )
        .await;
        assert!(
            matches!(b_row, IpeResult::Ok(ref v) if v.is_empty()),
            "dbB row rolled back independently"
        );

        db_a.close().await;
        db_b.close().await;
        let _ = std::fs::remove_file(&path_a);
        let _ = std::fs::remove_file(&path_b);
    }

    #[tokio::test]
    async fn test_get_bool() {
        let mut r = HashMap::new();
        r.insert("a".to_string(), "1".to_string());
        r.insert("b".to_string(), "0".to_string());
        r.insert("c".to_string(), "true".to_string());
        r.insert("d".to_string(), "false".to_string());
        assert!(db_get_bool("a".into(), &r));
        assert!(!db_get_bool("b".into(), &r));
        assert!(db_get_bool("c".into(), &r));
        assert!(!db_get_bool("d".into(), &r));
        assert!(!db_get_bool("missing".into(), &r));
    }

    #[tokio::test]
    async fn test_query_decode() {
        let db = fresh_db().await;
        let mut row = HashMap::new();
        row.insert("title".to_string(), "decoded".to_string());
        let _: IpeResult<String, i64> = db_insert_row(db.clone(), "todos".into(), row).await;
        // Use the Decoder<E,A> API: db_decode_string reads the "title" column from
        // the NULL-preserving JsonVal::Object produced by row_to_json.
        let decoded: IpeResult<String, Vec<String>> = db_query_decode(
            db,
            "SELECT title FROM todos".into(),
            vec![],
            db_decode_string("title".to_string()),
        )
        .await;
        match decoded {
            IpeResult::Ok(v) => assert_eq!(v, vec!["decoded".to_string()]),
            _ => panic!("decode"),
        }
    }

    #[tokio::test]
    async fn test_query_decode_int_and_nullable() {
        let db = fresh_db().await;
        let mut row = HashMap::new();
        row.insert("title".to_string(), "item".to_string());
        row.insert("done".to_string(), "1".to_string());
        let _: IpeResult<String, i64> = db_insert_row(db.clone(), "todos".into(), row).await;

        // Test db_decode_int decodes the "done" column correctly.
        let decoded_int: IpeResult<String, Vec<i64>> = db_query_decode(
            db.clone(),
            "SELECT done FROM todos".into(),
            vec![],
            db_decode_int("done".to_string()),
        )
        .await;
        match decoded_int {
            IpeResult::Ok(v) => assert_eq!(v, vec![1i64]),
            _ => panic!("db_decode_int decode failed"),
        }

        // Test db_decode_bool.
        let decoded_bool: IpeResult<String, Vec<bool>> = db_query_decode(
            db.clone(),
            "SELECT done FROM todos".into(),
            vec![],
            db_decode_bool("done".to_string()),
        )
        .await;
        match decoded_bool {
            IpeResult::Ok(v) => assert_eq!(v, vec![true]),
            _ => panic!("db_decode_bool decode failed"),
        }
    }

    #[tokio::test]
    async fn test_query_decode_nullable_null() {
        // SQLite table with a nullable column.
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect");
        sqlx::query("CREATE TABLE items (id INTEGER PRIMARY KEY, label TEXT)")
            .execute(&pool)
            .await
            .expect("create");
        // Row with NULL label.
        sqlx::query("INSERT INTO items (id, label) VALUES (1, NULL)")
            .execute(&pool)
            .await
            .expect("insert null");
        // Row with non-null label.
        sqlx::query("INSERT INTO items (id, label) VALUES (2, 'hello')")
            .execute(&pool)
            .await
            .expect("insert some");

        // db_decode_nullable(db_decode_string("label")): NULL → Nothing, "hello" → Just("hello").
        // (1-arg form: inner.fields = ["label"] provides the NULL-gate column.)

        // Check NULL row → Nothing.
        let r1: IpeResult<String, Vec<IpeMaybe<String>>> = db_query_decode(
            pool.clone(),
            "SELECT label FROM items WHERE id = 1".into(),
            vec![],
            db_decode_nullable(db_decode_string("label".to_string())),
        )
        .await;
        match r1 {
            IpeResult::Ok(v) => {
                assert_eq!(v.len(), 1);
                assert!(
                    matches!(v[0], IpeMaybe::Nothing),
                    "expected Nothing for NULL, got {:?}",
                    v[0]
                );
            }
            IpeResult::Err(e) => panic!("unexpected Err on NULL row: {}", e),
        }

        // Check non-NULL row → Just("hello").
        let r2: IpeResult<String, Vec<IpeMaybe<String>>> = db_query_decode(
            pool,
            "SELECT label FROM items WHERE id = 2".into(),
            vec![],
            db_decode_nullable(db_decode_string("label".to_string())),
        )
        .await;
        match r2 {
            IpeResult::Ok(v) => {
                assert_eq!(v.len(), 1);
                assert!(
                    matches!(&v[0], IpeMaybe::Just(s) if s == "hello"),
                    "expected Just(\"hello\"), got {:?}",
                    v[0]
                );
            }
            IpeResult::Err(e) => panic!("unexpected Err on non-null row: {}", e),
        }
    }

    #[tokio::test]
    async fn test_get_by_id_decode() {
        let db = fresh_db().await;
        let mut row = HashMap::new();
        row.insert("title".to_string(), "find-me".to_string());
        let id: IpeResult<String, i64> = db_insert_row(db.clone(), "todos".into(), row).await;
        let id = match id {
            IpeResult::Ok(v) => v,
            _ => panic!("insert"),
        };

        let found: IpeResult<String, IpeMaybe<String>> = db_get_by_id_decode(
            db.clone(),
            "todos".into(),
            id,
            db_decode_string("title".to_string()),
        )
        .await;
        match found {
            IpeResult::Ok(IpeMaybe::Just(s)) => assert_eq!(s, "find-me"),
            other => panic!("unexpected: {:?}", other),
        }

        // Non-existent id → Nothing.
        let not_found: IpeResult<String, IpeMaybe<String>> = db_get_by_id_decode(
            db,
            "todos".into(),
            99999,
            db_decode_string("title".to_string()),
        )
        .await;
        assert!(matches!(not_found, IpeResult::Ok(IpeMaybe::Nothing)));
    }

    #[tokio::test]
    async fn test_db_decode_money_roundtrip() {
        // Verify db_decode_money parses "USD 12.34" → (Decimal(12.34), "USD").
        use rust_decimal::Decimal as RD;
        use std::str::FromStr;
        let val = serde_json::json!({ "price": "USD 12.34" });
        let result = (db_decode_money::<String>("price".to_string()).run)(&val);
        match result {
            IpeResult::Ok((amount, code)) => {
                assert_eq!(code, "USD");
                assert_eq!(amount.0, RD::from_str("12.34").unwrap());
            }
            IpeResult::Err(e) => panic!("unexpected Err: {}", e),
        }

        // NULL → Err.
        let val_null = serde_json::json!({ "price": null });
        assert!(matches!(
            (db_decode_money::<String>("price".to_string()).run)(&val_null),
            IpeResult::Err(_)
        ));

        // Bad format → Err.
        let val_bad = serde_json::json!({ "price": "NODECIMAL" });
        assert!(matches!(
            (db_decode_money::<String>("price".to_string()).run)(&val_bad),
            IpeResult::Err(_)
        ));
    }

    #[tokio::test]
    async fn test_row_to_json_null_preservation() {
        // Verify that a SQL NULL cell becomes JsonVal::Null (not "").
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect");
        sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)")
            .execute(&pool)
            .await
            .expect("create");
        sqlx::query("INSERT INTO t (id, v) VALUES (1, NULL)")
            .execute(&pool)
            .await
            .expect("insert");
        let row = sqlx::query("SELECT v FROM t WHERE id = 1")
            .fetch_one(&pool)
            .await
            .expect("fetch");
        let jv = row_to_json(&row).unwrap_or_else(|e| panic!("row_to_json: {e}"));
        match jv.get("v") {
            Some(JsonVal::Null) => { /* correct */ }
            other => panic!("expected JsonVal::Null, got {:?}", other),
        }
    }

    // ─── RT-DATA-001: row_to_json/column_to_json probe chain ──────────────

    /// BLOB column written via `SqlBytes` decodes as a hex `JsonVal::String`,
    /// not `JsonVal::Null`. Exercises the bytes arm of `column_to_json`.
    #[tokio::test]
    async fn test_row_to_json_blob_decodes_as_hex() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("connect: {e}"));
        sqlx::query("CREATE TABLE blobs (id INTEGER PRIMARY KEY, data BLOB)")
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("create: {e}"));
        sqlx::query("INSERT INTO blobs (id, data) VALUES (1, ?)")
            .bind(vec![0xde_u8, 0xad, 0xbe, 0xef])
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("insert: {e}"));
        let row = sqlx::query("SELECT data FROM blobs WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|e| panic!("fetch: {e}"));
        let jv = row_to_json(&row).unwrap_or_else(|e| panic!("row_to_json: {e}"));
        match jv.get("data") {
            Some(JsonVal::String(s)) => {
                assert_eq!(s, "deadbeef", "expected hex encoding of bytes");
            }
            other => panic!("expected JsonVal::String(hex), got {:?}", other),
        }
    }

    /// Bool column decodes as `JsonVal::Bool`, not `JsonVal::Null`. Exercises
    /// the bool-first probe ordering in `column_to_json`.
    #[tokio::test]
    async fn test_row_to_json_bool_column() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("connect: {e}"));
        sqlx::query("CREATE TABLE flags (id INTEGER PRIMARY KEY, active BOOLEAN NOT NULL)")
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("create: {e}"));
        sqlx::query("INSERT INTO flags (id, active) VALUES (1, TRUE)")
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("insert: {e}"));
        let row = sqlx::query("SELECT active FROM flags WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|e| panic!("fetch: {e}"));
        let jv = row_to_json(&row).unwrap_or_else(|e| panic!("row_to_json: {e}"));
        // On sqlite, BOOLEAN stores as 0/1 INTEGER; column_to_json probes bool
        // first, so we get Bool(true) rather than Number(1).
        match jv.get("active") {
            Some(JsonVal::Bool(b)) => assert!(*b, "expected true"),
            // sqlite may surface as integer — accept Number(1) as correct too
            Some(JsonVal::Number(n)) => assert_eq!(n.as_i64(), Some(1), "expected 1"),
            other => panic!("expected Bool or Number(1), got {:?}", other),
        }
    }

    /// `db_decode_bytes` round-trip: write `SqlBytes`, read back via
    /// `db_decode_bytes`, assert the original bytes survive.
    #[tokio::test]
    async fn test_db_decode_bytes_roundtrip() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("connect: {e}"));
        sqlx::query("CREATE TABLE blobs (id INTEGER PRIMARY KEY, data BLOB)")
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("create: {e}"));
        let original: Vec<u8> = vec![0x01, 0x02, 0x03, 0xff];
        sqlx::query("INSERT INTO blobs (id, data) VALUES (1, ?)")
            .bind(original.clone())
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("insert: {e}"));
        let decoded: IpeResult<String, Vec<Vec<u8>>> = db_query_decode(
            pool,
            "SELECT data FROM blobs WHERE id = 1".to_string(),
            vec![],
            db_decode_bytes("data".to_string()),
        )
        .await;
        match decoded {
            IpeResult::Ok(rows) => {
                assert_eq!(rows.len(), 1, "expected one row");
                assert_eq!(rows[0], original, "bytes did not survive round-trip");
            }
            IpeResult::Err(e) => panic!("decode failed: {e:?}"),
        }
    }

    #[tokio::test]
    async fn test_close() {
        let db = fresh_db().await;
        let r: IpeResult<String, ()> = db_close(db).await;
        assert!(matches!(r, IpeResult::Ok(())));
    }

    // ─── Ipe.Db.Sql — SqlFragment builder ────────────────────

    async fn insert_todo(db: &Db, title: &str) {
        let mut r = HashMap::new();
        r.insert("title".to_string(), title.to_string());
        let _: IpeResult<String, i64> = db_insert_row(db.clone(), "todos".into(), r).await;
    }

    /// `Db.findWhere` with a single `Sql.eq` predicate finds exactly the
    /// matching row — the `SqlFragment`-typed replacement for
    /// `unsafeFindWhere`, proving the parameterised channel (never string
    /// interpolation) still works end-to-end.
    #[tokio::test]
    async fn test_find_where_eq() {
        let db = fresh_db().await;
        insert_todo(&db, "alpha").await;
        insert_todo(&db, "beta").await;
        let frag = sql_eq(
            sql_column("title".to_string()),
            sql_param("alpha".to_string()),
        );
        let found: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_where(db, "todos".into(), frag).await;
        match found {
            IpeResult::Ok(v) => {
                assert_eq!(v.len(), 1);
                assert_eq!(v[0].get("title").map(String::as_str), Some("alpha"));
            }
            IpeResult::Err(e) => panic!("expected Ok, got Err({e})"),
        }
    }

    /// `Sql.and` composes two predicates; `Sql.gt` on the auto-increment `id`
    /// column proves numeric comparison (not just string equality).
    #[tokio::test]
    async fn test_find_where_and_gt() {
        let db = fresh_db().await;
        insert_todo(&db, "alpha").await;
        insert_todo(&db, "beta").await;
        insert_todo(&db, "beta").await;
        let frag = sql_and(
            sql_eq(
                sql_column("title".to_string()),
                sql_param("beta".to_string()),
            ),
            sql_gt(sql_column("id".to_string()), sql_param(1_i64)),
        );
        let found: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_where(db, "todos".into(), frag).await;
        match found {
            IpeResult::Ok(v) => assert_eq!(v.len(), 2),
            IpeResult::Err(e) => panic!("expected Ok, got Err({e})"),
        }
    }

    /// `Db.deleteWhere` removes exactly the matching rows and returns the
    /// affected row count.
    #[tokio::test]
    async fn test_delete_where() {
        let db = fresh_db().await;
        insert_todo(&db, "alpha").await;
        insert_todo(&db, "beta").await;
        let frag = sql_eq(
            sql_column("title".to_string()),
            sql_param("alpha".to_string()),
        );
        let deleted: IpeResult<String, i64> =
            db_delete_where(db.clone(), "todos".into(), frag).await;
        assert_eq!(deleted, IpeResult::Ok(1));
        let remaining: IpeResult<String, Vec<HashMap<String, String>>> = db_find_where(
            db,
            "todos".into(),
            sql_is_not_null(sql_column("title".to_string())),
        )
        .await;
        match remaining {
            IpeResult::Ok(v) => assert_eq!(v.len(), 1),
            IpeResult::Err(e) => panic!("expected Ok, got Err({e})"),
        }
    }

    /// `Sql.inList` with a non-empty list matches every listed value.
    #[tokio::test]
    async fn test_in_list_non_empty() {
        let db = fresh_db().await;
        insert_todo(&db, "alpha").await;
        insert_todo(&db, "beta").await;
        insert_todo(&db, "gamma").await;
        let frag = sql_in_list(
            sql_column("title".to_string()),
            vec![
                SqlParam::Text("alpha".to_string()),
                SqlParam::Text("gamma".to_string()),
            ],
        );
        let found: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_where(db, "todos".into(), frag).await;
        match found {
            IpeResult::Ok(v) => assert_eq!(v.len(), 2),
            IpeResult::Err(e) => panic!("expected Ok, got Err({e})"),
        }
    }

    /// Empty `Sql.inList` emits `(1 = 0)` (always-false) rather than the SQL
    /// syntax error `IN ()` — a real column reference stays a real column
    /// reference, but the whole predicate matches nothing.
    #[tokio::test]
    async fn test_in_list_empty_matches_nothing() {
        let db = fresh_db().await;
        insert_todo(&db, "alpha").await;
        let frag = sql_in_list(sql_column("title".to_string()), Vec::new());
        let found: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_where(db, "todos".into(), frag).await;
        match found {
            IpeResult::Ok(v) => assert_eq!(v.len(), 0),
            IpeResult::Err(e) => panic!("expected Ok, got Err({e})"),
        }
    }

    /// `Sql.column` accepts a dotted reference (`table.column`) via the
    /// DOT-ACCEPTING `valid_sql_ident`, distinct from `SqlIdent::parse`
    /// (table-name-only, dot-rejecting) used for the table argument itself.
    #[tokio::test]
    async fn test_column_accepts_dotted_reference() {
        let db = fresh_db().await;
        insert_todo(&db, "alpha").await;
        let frag = sql_eq(
            sql_column("todos.title".to_string()),
            sql_param("alpha".to_string()),
        );
        let found: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_where(db, "todos".into(), frag).await;
        match found {
            IpeResult::Ok(v) => assert_eq!(v.len(), 1),
            IpeResult::Err(e) => panic!("expected Ok, got Err({e})"),
        }
    }

    /// An invalid `Sql.column` identifier poisons the fragment instead of
    /// panicking or interpolating unchecked text; `Db.findWhere` surfaces the
    /// poison as a `Task::Err`, never malformed SQL.
    #[tokio::test]
    async fn test_poisoned_column_surfaces_as_task_err() {
        let db = fresh_db().await;
        insert_todo(&db, "alpha").await;
        // Space + semicolon are outside `valid_sql_ident`'s charset.
        let frag = sql_eq(
            sql_column("title; DROP TABLE todos".to_string()),
            sql_param("alpha".to_string()),
        );
        let found: IpeResult<String, Vec<HashMap<String, String>>> =
            db_find_where(db, "todos".into(), frag).await;
        assert!(
            matches!(found, IpeResult::Err(_)),
            "poisoned column must surface as Task::Err, got {found:?}"
        );
    }

    /// `SqlFragment`'s hand-written `Debug` shows SQL text + bind COUNT —
    /// never the bind VALUE.
    #[test]
    fn test_sqlfragment_debug_never_shows_bind_values() {
        let frag = sql_eq(
            sql_column("title".to_string()),
            sql_param("super-secret-value".to_string()),
        );
        let shown = format!("{frag:?}");
        assert!(
            !shown.contains("super-secret-value"),
            "Debug leaked a bind value: {shown}"
        );
        assert!(
            shown.contains("binds: 1"),
            "Debug should show bind count: {shown}"
        );
    }

    /// `SqlFragment` derives `PartialEq` structurally (sql + binds + invalid).
    #[test]
    fn test_sqlfragment_partial_eq() {
        let a = sql_eq(sql_column("title".to_string()), sql_param(1_i64));
        let b = sql_eq(sql_column("title".to_string()), sql_param(1_i64));
        let c = sql_eq(sql_column("title".to_string()), sql_param(2_i64));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    // ─── db_exec / db_query parameter-binding characterization (candidate B) ──────
    // These pin the param-carrying behavior of the raw exec/query kernels — the
    // two functions that route through the placeholder path. They lock the
    // contract across the build_sql → db_format_sql+bind deepening: same
    // round-trip values, same injection-safety, no behavior drift.

    /// A parameterised INSERT via db_exec then a parameterised SELECT via
    /// db_query round-trips the bound values.
    #[tokio::test]
    async fn test_exec_and_query_with_params() {
        let db = fresh_db().await;
        let ins: IpeResult<String, i64> = db_exec(
            db.clone(),
            "INSERT INTO todos (title, done) VALUES (?, ?)".into(),
            vec!["buy milk".to_string(), "0".to_string()],
        )
        .await;
        assert!(matches!(ins, IpeResult::Ok(1))); // exec returns rows-affected

        let rows: IpeResult<String, Vec<HashMap<String, String>>> = db_query(
            db,
            "SELECT title, done FROM todos WHERE title = ?".into(),
            vec!["buy milk".to_string()],
        )
        .await;
        match rows {
            IpeResult::Ok(v) => {
                assert_eq!(v.len(), 1);
                assert_eq!(v[0].get("title").unwrap(), "buy milk");
                assert_eq!(v[0].get("done").unwrap(), "0");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    /// The load-bearing safety property: a value carrying single quotes and SQL
    /// metacharacters is bound, not spliced — stored and returned VERBATIM, and
    /// the surrounding table is untouched (no injection executes).
    #[tokio::test]
    async fn test_query_param_with_quotes_and_metachars_roundtrips_safely() {
        let db = fresh_db().await;
        let nasty = "x'); DROP TABLE todos;-- O'Brien".to_string();
        let ins: IpeResult<String, i64> = db_exec(
            db.clone(),
            "INSERT INTO todos (title, done) VALUES (?, ?)".into(),
            vec![nasty.clone(), "0".to_string()],
        )
        .await;
        assert!(matches!(ins, IpeResult::Ok(1))); // exec returns rows-affected

        // The value comes back byte-for-byte (proves it was bound, not splice-escaped-into-SQL).
        let rows: IpeResult<String, Vec<HashMap<String, String>>> = db_query(
            db.clone(),
            "SELECT title FROM todos WHERE title = ?".into(),
            vec![nasty.clone()],
        )
        .await;
        match rows {
            IpeResult::Ok(v) => {
                assert_eq!(v.len(), 1);
                assert_eq!(v[0].get("title").unwrap(), &nasty);
            }
            other => panic!("unexpected: {:?}", other),
        }

        // The table still exists with exactly the one row — the DROP never ran.
        let all: IpeResult<String, Vec<HashMap<String, String>>> =
            db_query(db, "SELECT title FROM todos".into(), vec![]).await;
        match all {
            IpeResult::Ok(v) => assert_eq!(v.len(), 1, "injection must not have dropped the table"),
            other => panic!("table gone or errored: {:?}", other),
        }
    }

    #[tokio::test]
    async fn update_fields_refuses_unscoped_update() {
        let db = fresh_db().await;
        let mk: IpeResult<String, i64> = db_exec_raw(
            db.clone(),
            "CREATE TABLE acct (id INTEGER PRIMARY KEY, bal INTEGER)".to_string(),
        )
        .await;
        assert!(matches!(mk, IpeResult::Ok(_)), "create: {mk:?}");
        let _: IpeResult<String, i64> = db_exec_raw(
            db.clone(),
            "INSERT INTO acct (bal) VALUES (10), (20)".to_string(),
        )
        .await;

        // Empty WHERE-column set MUST be refused (would otherwise mass-update
        // every row), NOT silently rewrite the whole table.
        let r: IpeResult<String, i64> = db_update_fields(
            db.clone(),
            "acct".to_string(),
            vec![], // no WHERE
            vec![("bal".to_string(), Some(SqlParam::Int(0)))],
        )
        .await;
        assert!(
            matches!(r, IpeResult::Err(_)),
            "empty WHERE must be refused, got {r:?}"
        );
        // No row should have been zeroed.
        let zeroed: IpeResult<String, Vec<HashMap<String, String>>> = db_query_params(
            db.clone(),
            "SELECT bal FROM acct WHERE bal = ?".to_string(),
            vec![SqlParam::Int(0)],
        )
        .await;
        assert_eq!(
            zeroed.with_default(Vec::new()).len(),
            0,
            "no row should have been mass-updated"
        );
        // A scoped update still works (affects exactly 1 row).
        let ok: IpeResult<String, i64> = db_update_fields(
            db.clone(),
            "acct".to_string(),
            vec![("id".to_string(), SqlParam::Int(1))],
            vec![("bal".to_string(), Some(SqlParam::Int(99)))],
        )
        .await;
        assert!(
            matches!(ok, IpeResult::Ok(1)),
            "scoped update should affect 1 row: {ok:?}"
        );
    }

    // AUD-07 (a): when DATABASE_URL is absent, ipe_db_url() returns the sqlite
    // file default — never the hardcoded "sqlite::memory:" that caused silent
    // data loss on every Db.connect() call.
    #[test]
    fn ipe_db_url_fallback_is_sqlite_file() {
        if crate::system::read_env_var("DATABASE_URL").is_ok() {
            // DATABASE_URL already set; test (b) covers the env-read path.
            return;
        }
        let url = crate::config::ipe_db_url();
        assert!(
            !url.contains(":memory:"),
            "default URL must not be in-memory: {url}"
        );
        assert!(
            url.contains("ipe.db"),
            "default URL must reference a named file: {url}"
        );
    }

    // AUD-07 (b): with DATABASE_URL set, ipe_db_url() returns it verbatim.
    #[test]
    fn ipe_db_url_reads_database_url_env() {
        use crate::system::{locked_remove_var, locked_set_var};
        locked_set_var("DATABASE_URL", "postgres://ci-host/testdb_aud07");
        let url = crate::config::ipe_db_url();
        locked_remove_var("DATABASE_URL");
        assert_eq!(url, "postgres://ci-host/testdb_aud07");
    }

    // AUD-07 (c): two sequential connect_cached calls with the same file URL
    // observe the same database — data written via one pool is visible via the
    // other. Proves the old sqlite::memory: per-call-fresh-db bug is closed.
    #[tokio::test]
    async fn ipe_db_url_shared_connection_sees_same_data() {
        use crate::system::{locked_remove_var, locked_set_var};
        let tmp = format!("/tmp/ipe_aud07_shared_{}.db", std::process::id());
        let url = format!("sqlite://{}?mode=rwc", tmp);
        locked_set_var("DATABASE_URL", &url);
        let resolved = crate::config::ipe_db_url();
        locked_remove_var("DATABASE_URL");

        let conn1 = connect_cached::<String>(resolved.clone()).await;
        let conn2 = connect_cached::<String>(resolved).await;
        let pool1 = match conn1 {
            IpeResult::Ok(p) => p,
            IpeResult::Err(e) => panic!("connect1 failed: {e}"),
        };
        let pool2 = match conn2 {
            IpeResult::Ok(p) => p,
            IpeResult::Err(e) => panic!("connect2 failed: {e}"),
        };
        sqlx::query("CREATE TABLE IF NOT EXISTS aud07_t (v INTEGER NOT NULL)")
            .execute(&pool1)
            .await
            .expect("create table");
        sqlx::query("INSERT INTO aud07_t VALUES (99)")
            .execute(&pool1)
            .await
            .expect("insert");
        let (v,): (i64,) = sqlx::query_as("SELECT v FROM aud07_t")
            .fetch_one(&pool2)
            .await
            .expect("select");
        assert_eq!(v, 99, "data written via pool1 must be visible via pool2");
        let _ = std::fs::remove_file(&tmp);
    }
}

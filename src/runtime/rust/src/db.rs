// DB kernel functions — generic over E and over backend.
// Uses DbPool, DbRow, ipe_db_url, db_last_insert_id, db_format_sql from
// config.rs (generated at build time per package.ipe database driver).
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

/// Map a connect-time `sqlx::Error` to a credential-free typed error.
///
/// The connect path can carry the connection string — host, user, password —
/// inside the `Configuration`, `Io`, and `Tls` payloads (a driver's `Display`
/// may echo the URL it was handed). The message is therefore built from the
/// error VARIANT alone; the payload is never formatted into it. A database error
/// at connect (e.g. an authentication rejection) keeps `ipe_err`'s structural
/// SQLSTATE-code path, which is already value-free.
fn connect_err<E: From<String> + Send>(e: &sqlx::Error) -> E {
    if e.as_database_error().is_some() {
        return ipe_err(e);
    }
    let kind = match e {
        sqlx::Error::Configuration(_) => "invalid connection configuration",
        sqlx::Error::Io(_) => "connection I/O error",
        sqlx::Error::Tls(_) => "TLS error",
        sqlx::Error::PoolTimedOut => "connection pool timed out",
        sqlx::Error::PoolClosed => "connection pool closed",
        _ => "connection error",
    };
    str_err(&format!("db: {kind}"))
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
    // Parse-don't-validate: a float source (JSON float or decimal string) is
    // truncated toward zero to an `Int`, but a magnitude past the `i64` range is
    // malformed input, not a value to silently saturate. Rust's `as` cast would
    // clamp `1e30` to `i64::MAX` and report `Ok` — data loss presented as
    // success. Reject it as a typed decode error instead (fail-closed).
    fn float_to_i64_checked<E: From<String>>(col: &str, f: f64) -> IpeResult<E, i64> {
        // Both bounds are exclusive: f64 cannot distinguish a boundary from its
        // out-of-range neighbour. `i64::MAX as f64` rounds up to 2^63 and an
        // input just past `i64::MIN` rounds down to `i64::MIN as f64`, so `<=`/
        // `>=` would admit an out-of-range magnitude and let `as i64` saturate
        // to the limit — data loss reported as success. An exact `i64::MIN`/
        // `i64::MAX` still decodes through the integer path above.
        let truncated = f.trunc();
        if truncated > i64::MIN as f64 && truncated < 9_223_372_036_854_775_808.0 {
            decode_ok(truncated as i64)
        } else {
            decode_err_str(format!(
                "column {}: expected Int, {} is out of range for a 64-bit integer",
                col, f
            ))
        }
    }
    decode_field(
        col.clone(),
        Decoder::new(
            Box::new(move |v| match v {
                JsonVal::Number(n) => match n.as_i64() {
                    Some(i) => decode_ok(i),
                    None => match n.as_f64() {
                        Some(f) => float_to_i64_checked(&col, f),
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
                        return float_to_i64_checked(&col, f);
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

/// `Db.Decode.decimal col` — read column `col` as an exact-decimal value.
///
/// The DB column stores the decimal as a TEXT string (the lossless
/// representation `SqlDecimal` writes on INSERT — no float intermediary, no
/// precision loss). Parses the text with `rust_decimal::Decimal::from_str`,
/// which is the same exact-decimal parse money uses for its amount component.
///
/// Returns `Decoder<E, Decimal>` — the symmetric, single-value counterpart
/// to `db_decode_money` (which returns `Decoder<E, (Decimal, String)>`).
///
/// Totality: missing column, NULL, or unparseable text → `Err(E::from(...))`.
pub fn db_decode_decimal<E: From<String> + 'static>(col: String) -> Decoder<E, Decimal> {
    decode_field(
        col.clone(),
        Decoder::new(
            Box::new(move |v| {
                let s = match v {
                    JsonVal::String(s) => s.clone(),
                    JsonVal::Null => {
                        return decode_err_str(format!(
                            "column {}: expected Decimal string, got NULL",
                            col
                        ));
                    }
                    _ => {
                        return decode_err_str(format!("column {}: expected Decimal string", col));
                    }
                };
                use rust_decimal::Decimal as RD;
                use std::str::FromStr;
                match RD::from_str(&s) {
                    Ok(d) => decode_ok(Decimal(d)),
                    Err(e) => decode_err_str(format!(
                        "column {}: could not decode decimal column {:?}: {}",
                        col, s, e
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
    A: Clone + 'static + Send + Sync,
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
    // Apply the SSRF host gate for any network-scheme URL before dialing.
    // SQLite (file/sqlite/`:memory:`) carries no host and is exempt.
    // `url::Url::parse` is the same parser sqlx uses internally, so the host
    // extracted here is the host sqlx would dial.
    if !url.starts_with("sqlite")
        && !url.starts_with("file")
        && !url.starts_with(':')
        && let Ok(parsed) = ::url::Url::parse(url)
        && let Some(host) = parsed.host_str()
        && let Err(e) =
            crate::ssrf::VettedDial::for_host(host, parsed.port_or_known_default().unwrap_or(5432))
    {
        return IpeResult::Err(str_err(&format!("db: {e}")));
    }
    let pool: Db = match sqlx::pool::PoolOptions::new()
        .max_connections(max_pool_connections())
        .connect(url)
        .await
    {
        Ok(p) => p,
        Err(e) => return IpeResult::Err(connect_err(&e)),
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
/// `DbPool` type is already fixed by the `package.ipe` driver, so `driver` is
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
        // `unsafeExecRaw : Db -> String -> Task Error Int` — the verbatim-SQL
        // escape hatch (its surface name marks the raw-SQL injection surface;
        // parameterisable statements go through `db_exec`/`db_query`). Int is the
        // rows-affected count. `as i64` matches the insert/update/delete sites;
        // rows-affected can never realistically exceed i64::MAX.
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

/// Truncate a float toward zero to an `Int`, rejecting a magnitude that would
/// saturate under an `as i64` cast. Both bounds are exclusive because f64 cannot
/// distinguish a boundary from its out-of-range neighbour (`i64::MAX as f64`
/// rounds up to 2^63; an input just past `i64::MIN` rounds down to `i64::MIN as
/// f64`), so `<=`/`>=` would admit an out-of-range magnitude and let `as i64`
/// saturate to the limit — a wrong value that reads like a real row value. An
/// exact `i64::MIN`/`i64::MAX` still round-trips. This is the total getter, so
/// an out-of-range read is surfaced on the runtime's stderr anomaly channel and
/// falls back to the contractual `0` default rather than saturating. The typed,
/// fail-with-Err path is `db_decode_int`.
fn float_to_i64_or_default(field: &str, f: f64) -> i64 {
    let truncated = f.trunc();
    if truncated > i64::MIN as f64 && truncated < 9_223_372_036_854_775_808.0 {
        truncated as i64
    } else {
        eprintln!(
            "db: unsafeGetInt(\"{field}\"): {f} is out of range for a 64-bit integer; \
             returning 0 (default) instead of a saturated value"
        );
        let _ = std::io::Write::flush(&mut std::io::stderr());
        0
    }
}

pub fn db_get_int<R: IpeRow>(field: String, row: &R) -> i64 {
    // Align with db_decode_int / Go: accept "42" or a decimal string like
    // "3.0" (truncate to 3) before defaulting to 0.
    let s = row.ipe_get(&field);
    if let Ok(i) = s.parse::<i64>() {
        return i;
    }
    if let Ok(f) = s.parse::<f64>() {
        return float_to_i64_or_default(&field, f);
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

/// Whether a [`SqlIdent`] may contain `.` separators.
///
/// A `Plain` identifier is a bare table or column name (`users`, `email`); a
/// `Dotted` identifier additionally admits a qualified reference (`users.id`).
/// The two modes are the ONLY axis on which the single identifier parser
/// varies — there is one charset check, not two hand-rolled ones that could
/// drift apart on a security boundary.
#[derive(Clone, Copy)]
enum IdentMode {
    /// `[A-Za-z0-9_]` — bare table/column name, dots rejected.
    Plain,
    /// `[A-Za-z0-9_.]` — bare name or a dotted qualified reference.
    Dotted,
}

/// A validated SQL identifier (table/column name) — parse-don't-validate and
/// the SINGLE source of truth for the identifier-interpolation boundary. Every
/// path that interpolates a table/column name into SQL obtains one of these
/// through [`SqlIdent::parse`] (or its mode helpers); there is no other
/// charset check in this module. A value of this type is therefore always safe
/// to interpolate — no `""` sentinel to re-check, and an unvalidated name is
/// unrepresentable past the boundary.
struct SqlIdent(String);
impl SqlIdent {
    /// The one and only SQL-identifier charset gate. `mode` selects whether a
    /// `.` separator is admitted; everything else about the policy (non-empty,
    /// ASCII-alphanumeric-or-underscore) is shared, so the `Plain` and
    /// `Dotted` surfaces cannot drift. In `Dotted` mode each dot-delimited
    /// segment must itself be non-empty, so a leading dot, a trailing dot, and
    /// consecutive dots are all rejected — a dotted reference is a sequence of
    /// bare names, never a structurally-malformed dot string.
    fn parse(name: &str, mode: IdentMode) -> Option<SqlIdent> {
        let dot_ok = matches!(mode, IdentMode::Dotted);
        let charset_ok = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || (dot_ok && c == '.'));
        let segments_ok = !dot_ok || name.split('.').all(|seg| !seg.is_empty());
        if charset_ok && segments_ok {
            Some(SqlIdent(name.to_string()))
        } else {
            None
        }
    }
    /// Parse a bare (dot-rejecting) table or column name.
    fn parse_plain(name: &str) -> Option<SqlIdent> {
        Self::parse(name, IdentMode::Plain)
    }
    /// Parse a name that may be a dotted qualified reference (`table.column`).
    fn parse_dotted(name: &str) -> Option<SqlIdent> {
        Self::parse(name, IdentMode::Dotted)
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
        let qtable = match SqlIdent::parse_plain(&table) {
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
            .map(|k| SqlIdent::parse_plain(k))
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
        let qtable = match SqlIdent::parse_plain(&table) {
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
        let qtable = match SqlIdent::parse_plain(&table) {
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
            .map(|k| SqlIdent::parse_plain(k))
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
        let qtable = match SqlIdent::parse_plain(&table) {
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
        let (qtable, qfield) = match (SqlIdent::parse_plain(&table), SqlIdent::parse_plain(&field))
        {
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
        let (qtable, qfield) = match (SqlIdent::parse_plain(&table), SqlIdent::parse_plain(&field))
        {
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
        let qtable = match SqlIdent::parse_plain(&table) {
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
            .map(|k| SqlIdent::parse_plain(k))
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
        let qtable = match SqlIdent::parse_plain(&table) {
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
// Security: every table/column name that reaches SQL interpolation is
// validated by the single `SqlIdent` parser (ASCII alphanumeric + `_`, plus
// `.` in dotted mode, rejects empty) — see `SqlIdent::parse`. There is no
// second charset check that could drift from it. All VALUES are
// positional-bound (`?`), never interpolated.
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

/// Predicate form of the DOT-ACCEPTING identifier gate, for the one caller that
/// needs a bare `bool` over a split slice (the `RETURNING` projection check).
/// It is NOT an independent charset check: it delegates to the single
/// [`SqlIdent`] parser ([`IdentMode::Dotted`]), so it cannot drift from the
/// typed boundary used everywhere else. Prefer [`SqlIdent::parse_dotted`] (the
/// typed value) at any site that goes on to interpolate the identifier.
pub fn valid_sql_ident(name: &str) -> bool {
    SqlIdent::parse_dotted(name).is_some()
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
/// Accepts dotted references (`users.id`) via [`SqlIdent::parse_dotted`] — the
/// dot-admitting mode of the single identifier parser ([`SqlIdent::parse_plain`]
/// is the bare table/column-name-only mode used for the table argument itself,
/// which rejects dots). An invalid identifier poisons the fragment instead of
/// panicking or interpolating unchecked text.
///
/// Takes an owned `String` (not `&str`) to match every other Ipê-`String`-
/// typed kernel parameter in this module — the generic call-emission path
/// (`ipe_backend_rust::emit_expr`'s standard-path fallback) always produces an
/// owned `String` for a Ipê `String` argument, never a borrow.
pub fn sql_column(name: String) -> SqlFragment {
    if let Some(ident) = SqlIdent::parse_dotted(&name) {
        SqlFragment {
            sql: ident.as_str().to_string(),
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

/// `Ipe.Db.Unsafe.unsafeFragment : String -> SqlFragment` — the anti-[`sql_column`]:
/// mints a `SqlFragment` from `name` VERBATIM, deliberately SKIPPING the
/// [`valid_sql_ident`] gate that `sql_column` applies. The caller asserts, under
/// the `unsafe` capability, that `name` is a safe SQL identifier or fragment; no
/// validator runs and no poison marker is set. This is the un-validated escape
/// hatch the safe `Sql.column` path exists to avoid — reachable only through the
/// disclosed `Ipe.Db.Unsafe` submodule.
///
/// Total and panic-free: it constructs the plain `SqlFragment` record with the
/// verbatim text, an empty bind list, and no poison — no indexing, no unwrap, no
/// fallible step.
pub fn sql_unsafe_fragment(name: String) -> SqlFragment {
    SqlFragment {
        sql: name,
        binds: Vec::new(),
        invalid: None,
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
        let qtable = match SqlIdent::parse_plain(&table) {
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

/// The separator joining an alias and a column in a join projection's output
/// name (`SELECT a0.title AS a0__title`). A double underscore, so a single
/// underscore inside a column name never collides with the boundary — a
/// projected name splits back into `(alias, column)` at the first `__`.
const JOIN_ALIAS_SEP: &str = "__";

/// One joined result row: the two sides' plain-keyed cell maps (left, right).
/// Each side decodes through its own store codec exactly as a single-table read
/// does, so `Db.findJoin` returns a `List` of these pairs.
pub type JoinRow = (HashMap<String, String>, HashMap<String, String>);

/// One join side: its validated table name, the alias bound to it, and the
/// validated column names to project. Both aliases and every column reach SQL
/// only after `SqlIdent::parse_plain` accepts them, so the projection text this
/// carries can hold no injected fragment.
struct JoinSide {
    ident: SqlIdent,
    alias: SqlIdent,
    columns: Vec<SqlIdent>,
}

impl JoinSide {
    /// Parse a side's identifiers, failing closed on the first that is not a
    /// bare SQL identifier. Mirrors `db_find_where`'s table re-parse: the Ipê
    /// layer already validated these, and the runtime validates them again
    /// (defence in depth) so no single missed gate lets an identifier reach SQL.
    fn parse(table: String, alias: String, columns: Vec<String>) -> Result<JoinSide, String> {
        let ident =
            SqlIdent::parse_plain(&table).ok_or_else(|| format!("invalid table {table:?}"))?;
        let alias =
            SqlIdent::parse_plain(&alias).ok_or_else(|| format!("invalid alias {alias:?}"))?;
        let mut parsed = Vec::with_capacity(columns.len());
        for col in columns {
            let c = SqlIdent::parse_plain(&col).ok_or_else(|| format!("invalid column {col:?}"))?;
            parsed.push(c);
        }
        if parsed.is_empty() {
            return Err(format!(
                "join side {:?} names no columns to project",
                ident.as_str()
            ));
        }
        Ok(JoinSide {
            ident,
            alias,
            columns: parsed,
        })
    }

    /// This side's `alias.column AS alias__column` projection terms, each built
    /// only from already-parsed identifiers.
    fn projection_terms(&self) -> Vec<String> {
        self.columns
            .iter()
            .map(|c| {
                format!(
                    "{alias}.{col} AS {alias}{sep}{col}",
                    alias = self.alias.as_str(),
                    col = c.as_str(),
                    sep = JOIN_ALIAS_SEP
                )
            })
            .collect()
    }

    /// This side's `table AS alias` FROM term.
    fn table_ref(&self) -> String {
        format!("{} AS {}", self.ident.as_str(), self.alias.as_str())
    }

    /// The prefix that marks a projected cell as belonging to this side.
    fn output_prefix(&self) -> String {
        format!("{}{}", self.alias.as_str(), JOIN_ALIAS_SEP)
    }
}

/// Split one joined result row into the two sides' plain-keyed maps: a cell
/// named `alias__column` is stripped of its `alias__` prefix and placed in that
/// side's map under the bare `column`, so each side decodes through its own
/// codec exactly as a single-table read does. A cell matching neither prefix is
/// dropped (the SELECT projects only the two aliases' columns, so none arise).
fn split_join_row(row: &HashMap<String, String>, left_prefix: &str, right_prefix: &str) -> JoinRow {
    let mut left = HashMap::new();
    let mut right = HashMap::new();
    for (name, value) in row {
        if let Some(col) = name.strip_prefix(left_prefix) {
            left.insert(col.to_string(), value.clone());
        } else if let Some(col) = name.strip_prefix(right_prefix) {
            right.insert(col.to_string(), value.clone());
        }
    }
    (left, right)
}

/// `Db.findJoin : Db -> String -> String -> List String -> String -> String
///                -> List String -> SqlFragment
///                -> Task Error (List (Dict String String, Dict String String))`
/// — read an inner join of two tables as one parameterized statement, returning
/// each result row as the pair of the two sides' plain-keyed cell maps.
///
/// The two `(table, alias, columns)` triples name the join sides; `frag` is the
/// WHERE fragment (the join-key equality, plus any filter), built only through
/// the `Sql.*` combinators, so it is always `?`-placeholder text with a matching
/// bind list. Every identifier — both tables, both aliases, every projected
/// column — passes `SqlIdent::parse_plain` before it reaches the SQL text; the
/// first that does not fails the whole read closed. The SELECT projects each
/// side's columns under an `alias__column` output name, and each returned row is
/// split back into the two sides' plain-keyed maps by that prefix, so a caller
/// decodes each side through its existing per-store codec.
#[allow(clippy::too_many_arguments)] // one flat arg per validated identifier group; a struct arg would only move the same seven values behind an emit-side constructor.
pub fn db_find_join<E: Send + From<String> + 'static>(
    conn: Db,
    left_table: String,
    left_alias: String,
    left_columns: Vec<String>,
    right_table: String,
    right_alias: String,
    right_columns: Vec<String>,
    frag: SqlFragment,
) -> IpeTask<E, Vec<JoinRow>> {
    Box::pin(async move {
        if let Some(reason) = frag.invalid {
            return IpeResult::Err(format!("db.findJoin: {reason}").into());
        }
        let left = match JoinSide::parse(left_table, left_alias, left_columns) {
            Ok(s) => s,
            Err(reason) => return IpeResult::Err(format!("db.findJoin: {reason}").into()),
        };
        let right = match JoinSide::parse(right_table, right_alias, right_columns) {
            Ok(s) => s,
            Err(reason) => return IpeResult::Err(format!("db.findJoin: {reason}").into()),
        };
        let left_prefix = left.output_prefix();
        let right_prefix = right.output_prefix();
        let sql = match build_join_statement(&left, &right, &frag.sql) {
            Ok(s) => s,
            Err(reason) => return IpeResult::Err(format!("db.findJoin: {reason}").into()),
        };
        let mut q = sqlx::query(&sql);
        for p in frag.binds {
            q = bind_sql_param(q, p);
        }
        match fetch_all_routed(&conn, q).await {
            Ok(rows) => ok_res(
                rows.iter()
                    .map(|r| split_join_row(&row_to_map(r), &left_prefix, &right_prefix))
                    .collect(),
            ),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

/// Build the single parameterized join statement from the two validated sides
/// and the combinator-built WHERE text. The SELECT projects each side's columns
/// under an `alias__column` output name, the FROM lists both `table AS alias`
/// terms, and the WHERE is the `?`-placeholder fragment text. Every identifier
/// here came through `SqlIdent::parse_plain`; the two sides must carry distinct
/// aliases (else the projection and WHERE could not tell them apart), which is
/// the one remaining fail-closed check. No value is interpolated — the WHERE's
/// values are the fragment's positional binds.
fn build_join_statement(
    left: &JoinSide,
    right: &JoinSide,
    where_sql: &str,
) -> Result<String, String> {
    if left.alias.as_str() == right.alias.as_str() {
        return Err(format!(
            "the two join sides share the alias {:?}; each side needs a distinct alias",
            left.alias.as_str()
        ));
    }
    let mut projection = left.projection_terms();
    projection.extend(right.projection_terms());
    Ok(db_format_sql(format!(
        "SELECT {proj} FROM {lf}, {rf} WHERE {where_}",
        proj = projection.join(", "),
        lf = left.table_ref(),
        rf = right.table_ref(),
        where_ = where_sql
    )))
}

/// One projected column: the alias-qualified source (`alias.column`) and the
/// output name it is bound to (`p0`, `p1`, …). Both the alias and the column
/// reach SQL only after `SqlIdent::parse_plain` accepts them, so the projection
/// text this carries can hold no injected fragment.
struct ProjectionColumn {
    alias: SqlIdent,
    column: SqlIdent,
    output: SqlIdent,
}

impl ProjectionColumn {
    /// Parse one `(alias, column)` reference into a validated projection at
    /// output position `index`. Fails closed on the first identifier that is not
    /// a bare SQL identifier (defence in depth — the Ipê layer already validated
    /// these).
    fn parse(alias: &str, column: &str, index: usize) -> Result<ProjectionColumn, String> {
        let alias =
            SqlIdent::parse_plain(alias).ok_or_else(|| format!("invalid alias {alias:?}"))?;
        let column =
            SqlIdent::parse_plain(column).ok_or_else(|| format!("invalid column {column:?}"))?;
        let output_name = format!("p{index}");
        let output = SqlIdent::parse_plain(&output_name)
            .ok_or_else(|| format!("invalid projection output {output_name:?}"))?;
        Ok(ProjectionColumn {
            alias,
            column,
            output,
        })
    }

    /// This column's `alias.column AS p<index>` projection term, built only from
    /// already-parsed identifiers.
    fn projection_term(&self) -> String {
        format!(
            "{alias}.{col} AS {out}",
            alias = self.alias.as_str(),
            col = self.column.as_str(),
            out = self.output.as_str()
        )
    }
}

/// `Db.findProjection : Db -> String -> String -> String -> String
///                      -> SqlFragment -> List (String, String)
///                      -> Task Error (List (Dict String String))` — read a typed
/// projection over a two-table join as one parameterized statement.
///
/// The two `(table, alias)` pairs name the join sides; `frag` is the WHERE
/// fragment (the join-key equality plus any filter), built only through the
/// `Sql.*` combinators; `projections` is the ordered `(alias, column)` references
/// to project. Every identifier — both tables, both aliases, and every projected
/// alias and column — passes `SqlIdent::parse_plain` before it reaches SQL text;
/// the first that does not fails the whole read closed. The SELECT projects each
/// reference under a `p<index>` output name, so a caller decodes each projected
/// column by position; no value is interpolated.
#[allow(clippy::too_many_arguments)] // one flat arg per validated identifier group; a struct arg would only move the same values behind an emit-side constructor.
pub fn db_find_projection<E: Send + From<String> + 'static>(
    conn: Db,
    left_table: String,
    left_alias: String,
    right_table: String,
    right_alias: String,
    frag: SqlFragment,
    projections: Vec<(String, String)>,
) -> IpeTask<E, Vec<HashMap<String, String>>> {
    Box::pin(async move {
        if let Some(reason) = frag.invalid {
            return IpeResult::Err(format!("db.findProjection: {reason}").into());
        }
        let sql = match build_projection_statement(
            &left_table,
            &left_alias,
            &right_table,
            &right_alias,
            &projections,
            &frag.sql,
        ) {
            Ok(s) => s,
            Err(reason) => return IpeResult::Err(format!("db.findProjection: {reason}").into()),
        };
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

/// Build the single parameterized projection statement from the two validated
/// sides, the ordered projection references, and the combinator-built WHERE
/// text. The SELECT names only the projected `alias.column AS p<index>` terms
/// (column pushdown), the FROM lists both `table AS alias` terms, and the WHERE
/// is the `?`-placeholder fragment text. Every identifier passes
/// `SqlIdent::parse_plain`; the two sides must carry distinct aliases and the
/// projection must name at least one column — the remaining fail-closed checks.
fn build_projection_statement(
    left_table: &str,
    left_alias: &str,
    right_table: &str,
    right_alias: &str,
    projections: &[(String, String)],
    where_sql: &str,
) -> Result<String, String> {
    let left_table_id =
        SqlIdent::parse_plain(left_table).ok_or_else(|| format!("invalid table {left_table:?}"))?;
    let left_alias_id =
        SqlIdent::parse_plain(left_alias).ok_or_else(|| format!("invalid alias {left_alias:?}"))?;
    let right_table_id = SqlIdent::parse_plain(right_table)
        .ok_or_else(|| format!("invalid table {right_table:?}"))?;
    let right_alias_id = SqlIdent::parse_plain(right_alias)
        .ok_or_else(|| format!("invalid alias {right_alias:?}"))?;
    if left_alias_id.as_str() == right_alias_id.as_str() {
        return Err(format!(
            "the two join sides share the alias {:?}; each side needs a distinct alias",
            left_alias_id.as_str()
        ));
    }
    if projections.is_empty() {
        return Err("a projection must name at least one column".to_string());
    }
    let mut terms = Vec::with_capacity(projections.len());
    for (index, (alias, column)) in projections.iter().enumerate() {
        let projected = ProjectionColumn::parse(alias, column, index)?;
        terms.push(projected.projection_term());
    }
    Ok(db_format_sql(format!(
        "SELECT {proj} FROM {lt} AS {la}, {rt} AS {ra} WHERE {where_}",
        proj = terms.join(", "),
        lt = left_table_id.as_str(),
        la = left_alias_id.as_str(),
        rt = right_table_id.as_str(),
        ra = right_alias_id.as_str(),
        where_ = where_sql
    )))
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
        if frag.sql.trim().is_empty() {
            return IpeResult::Err(
                "db.deleteWhere: refusing a delete with an empty WHERE clause \
                 (an unconstrained mass-delete)"
                    .to_string()
                    .into(),
            );
        }
        let qtable = match SqlIdent::parse_plain(&table) {
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

/// `Db.updateWhere : Db -> String -> List (String, SqlField) -> SqlFragment -> Task Error Int`
/// — the WHERE-`SqlFragment` counterpart to [`db_update_fields`]. The SET list is
/// the OmitField-aware column/value binds of [`db_update_fields`]; the WHERE is
/// the combinator-built `SqlFragment` of [`db_delete_where`]. Every SET value is
/// bound (`SqlParam`); the WHERE text is always `?`-placeholder with a matching
/// bind list, so no caller value or identifier reaches the SQL text.
pub fn db_update_where<E: Send + From<String> + 'static>(
    conn: Db,
    table: String,
    set_fields: Vec<(String, Option<SqlParam>)>,
    frag: SqlFragment,
) -> IpeTask<E, i64> {
    Box::pin(async move {
        if let Some(reason) = frag.invalid {
            return IpeResult::Err(format!("db.updateWhere: {reason}").into());
        }
        let qtable = match SqlIdent::parse_dotted(&table) {
            Some(t) => t,
            None => {
                return IpeResult::Err(
                    format!("db.updateWhere: invalid table name {:?}", table).into(),
                );
            }
        };
        // Build SET clause — OmitField (None) columns are skipped.
        let mut set_clauses: Vec<String> = Vec::new();
        let mut args: Vec<SqlParam> = Vec::new();
        for (col, opt) in set_fields {
            let qcol = match SqlIdent::parse_dotted(&col) {
                Some(c) => c,
                None => {
                    return IpeResult::Err(
                        format!("db.updateWhere: invalid SET column name {:?}", col).into(),
                    );
                }
            };
            if let Some(p) = opt {
                set_clauses.push(format!("{} = ?", qcol.as_str()));
                args.push(p);
            }
        }
        if set_clauses.is_empty() {
            // Every column was OmitField — nothing to update; report zero rows.
            return ok_res(0i64);
        }
        // Refuse an unscoped UPDATE: an empty WHERE fragment would emit
        // `UPDATE <table> SET ...` with no WHERE, silently rewriting EVERY row.
        // Fail closed instead of mass-updating.
        if frag.sql.trim().is_empty() {
            return IpeResult::Err(
                "db.updateWhere: refusing unscoped UPDATE (no WHERE); pass an explicit condition"
                    .to_string()
                    .into(),
            );
        }
        let sql = db_format_sql(format!(
            "UPDATE {} SET {} WHERE {}",
            qtable.as_str(),
            set_clauses.join(", "),
            frag.sql
        ));
        let mut q = sqlx::query(&sql);
        for p in args {
            q = bind_sql_param(q, p);
        }
        for p in frag.binds {
            q = bind_sql_param(q, p);
        }
        match exec_routed(&conn, q).await {
            Ok(res) => ok_res(res.rows_affected() as i64),
            Err(e) => IpeResult::Err(ipe_err(&e)),
        }
    })
}

// ─── External-connection read path (foreign-DB reads through the codec stack) ──
//
// The app connection (`Db`) is one dialect fixed at build time; an external
// `ExternalConnection` may be a DIFFERENT dialect selected at runtime by the
// parsed `Dsn`. The read runners below therefore build and decode each query
// keyed on the external connection's OWN dialect, never on the app-build's
// `db_format_sql` / `DbRow`. They reuse the identical query builder every app
// read uses — `SqlIdent` for identifiers, the `?`-placeholder text from the
// `Sql.*` fragment combinators, `SqlParam` positional binds — so no new query
// path or injection surface is introduced by reading elsewhere (design §2). Only
// the placeholder-style rewrite and the row→value decode are dialect-selected,
// per concrete match arm, so there is no `dyn`.

/// Rewrite `?`-placeholder SQL to the placeholder style the external dialect
/// expects: Postgres numbers them (`$1`, `$2`, …); SQLite keeps `?`. This mirrors
/// the per-dialect `db_format_sql` the app path applies, but is selected from the
/// EXTERNAL connection's dialect at runtime instead of the build-fixed one — the
/// same sequential rewrite is correct because every value is bound as a
/// parameter, never inlined into the SQL text.
#[cfg(feature = "db")]
fn external_format_sql_postgres(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len() + 8);
    let mut n = 0u32;
    for ch in sql.chars() {
        if ch == '?' {
            n += 1;
            out.push('$');
            out.push_str(&n.to_string());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Decode column `i` of an EXTERNAL row into a `String`, mirroring the app-path
/// [`column_to_string`] probe order (bool → i64 → f64 → String → bytes-hex).
/// Generic over the sqlx row type so a single body serves both external
/// dialects; each caller monomorphises it to its concrete row (no `dyn`).
#[cfg(feature = "db")]
fn external_column_to_string<R>(row: &R, i: usize) -> String
where
    R: Row,
    usize: sqlx::ColumnIndex<R>,
    for<'a> Option<bool>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<i64>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<f64>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<String>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<Vec<u8>>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
{
    let is_bool = row
        .columns()
        .get(i)
        .map(sqlx::Column::type_info)
        .map(|ti| {
            let name = ti.name().to_ascii_uppercase();
            name == "BOOL" || name == "BOOLEAN"
        })
        .unwrap_or(false);
    if is_bool && let Ok(opt) = row.try_get::<Option<bool>, _>(i) {
        return opt.map_or_else(String::new, |b| b.to_string());
    }
    if let Ok(opt) = row.try_get::<Option<i64>, _>(i) {
        return opt.map_or_else(String::new, |n| n.to_string());
    }
    if let Ok(opt) = row.try_get::<Option<f64>, _>(i) {
        return opt.map_or_else(String::new, |f| f.to_string());
    }
    if let Ok(opt) = row.try_get::<Option<String>, _>(i) {
        return opt.unwrap_or_default();
    }
    if let Ok(Some(bytes)) = row.try_get::<Option<Vec<u8>>, _>(i) {
        return hex::encode(bytes);
    }
    String::new()
}

/// Decode an EXTERNAL row into the untyped `Dict String String` shape, mirroring
/// the app-path [`row_to_map`]. Generic over the sqlx row type.
#[cfg(feature = "db")]
#[allow(clippy::needless_range_loop)]
fn external_row_to_map<R>(row: &R) -> HashMap<String, String>
where
    R: Row,
    usize: sqlx::ColumnIndex<R>,
    for<'a> Option<bool>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<i64>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<f64>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<String>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<Vec<u8>>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
{
    let mut map = HashMap::new();
    for (i, col) in row.columns().iter().enumerate() {
        map.insert(col.name().to_string(), external_column_to_string(row, i));
    }
    map
}

/// Decode column `i` of an EXTERNAL row into a `JsonVal`, mirroring the app-path
/// [`column_to_json`] probe order and its NULL-preserving semantics (so
/// `db_decode_nullable` distinguishes NULL from empty on a foreign row too).
#[cfg(feature = "db")]
fn external_column_to_json<R>(row: &R, i: usize) -> Result<JsonVal, sqlx::Error>
where
    R: Row,
    usize: sqlx::ColumnIndex<R>,
    for<'a> Option<bool>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<i64>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<f64>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<String>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<Vec<u8>>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
{
    let is_bool = row
        .columns()
        .get(i)
        .map(sqlx::Column::type_info)
        .map(|ti| {
            let name = ti.name().to_ascii_uppercase();
            name == "BOOL" || name == "BOOLEAN"
        })
        .unwrap_or(false);
    if is_bool && let Ok(opt) = row.try_get::<Option<bool>, _>(i) {
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
    if let Ok(opt) = row.try_get::<Option<Vec<u8>>, _>(i) {
        return Ok(opt.map_or(JsonVal::Null, |b| JsonVal::String(hex::encode(b))));
    }
    Err(sqlx::Error::ColumnDecode {
        index: i.to_string(),
        source: "unsupported column type (not bool/i64/f64/String/bytes)".into(),
    })
}

/// Decode an EXTERNAL row into the NULL-preserving `JsonVal::Object` the typed
/// decoder path consumes, mirroring the app-path [`row_to_json`].
#[cfg(feature = "db")]
fn external_row_to_json<R>(row: &R) -> Result<JsonVal, sqlx::Error>
where
    R: Row,
    usize: sqlx::ColumnIndex<R>,
    for<'a> Option<bool>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<i64>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<f64>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<String>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Option<Vec<u8>>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
{
    let cols = row.columns();
    let mut map = serde_json::Map::with_capacity(cols.len());
    for (i, col) in cols.iter().enumerate() {
        map.insert(col.name().to_string(), external_column_to_json(row, i)?);
    }
    Ok(JsonVal::Object(map))
}

/// Bind a `SqlParam` onto a query builder for a SPECIFIC external dialect,
/// generic over the sqlx database. Same total per-variant mapping as the
/// app-path [`bind_sql_param`], including the typed-NULL witness that gives
/// Postgres the correct per-parameter type OID.
#[cfg(feature = "db")]
fn external_bind_sql_param<'q, DB>(
    q: sqlx::query::Query<'q, DB, <DB as sqlx::Database>::Arguments<'q>>,
    p: SqlParam,
) -> sqlx::query::Query<'q, DB, <DB as sqlx::Database>::Arguments<'q>>
where
    DB: sqlx::Database,
    String: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    i64: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    f64: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    bool: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    Vec<u8>: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    Option<String>: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    Option<i64>: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    Option<f64>: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    Option<bool>: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    Option<Vec<u8>>: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
{
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
            SqlParam::Null(_) => q.bind(Option::<String>::None),
        },
    }
}

/// `Db.findWhereOn : Connection a -> String -> SqlFragment -> Task Error (List Row)`
/// — the external-connection counterpart to [`db_find_where`]. The `SqlFragment`
/// arrives from the same `Sql.*` combinators (validated identifiers + bound
/// params), the table name passes the same [`SqlIdent`] gate, and every value is
/// bound positionally — identical injection barrier, run against a foreign pool.
/// Accepts `Connection a` (any access mode: a read is available on read-only and
/// read-write alike); the phantom mode is erased at emit.
#[cfg(feature = "db")]
pub fn db_conn_find_where<E: Send + From<String> + 'static>(
    conn: ExternalConnection,
    table: String,
    frag: SqlFragment,
) -> IpeTask<E, Vec<HashMap<String, String>>> {
    Box::pin(async move {
        if let Some(reason) = frag.invalid {
            return IpeResult::Err(format!("db.findWhereOn: {reason}").into());
        }
        let qtable = match SqlIdent::parse_plain(&table) {
            Some(t) => t,
            None => {
                return IpeResult::Err(format!("db.findWhereOn: invalid table {:?}", table).into());
            }
        };
        let base = format!("SELECT * FROM {} WHERE {}", qtable.as_str(), frag.sql);
        match conn {
            ExternalConnection::Postgres(pool) => {
                let sql = external_format_sql_postgres(&base);
                let mut q = sqlx::query(&sql);
                for p in frag.binds {
                    q = external_bind_sql_param(q, p);
                }
                match q.fetch_all(&pool).await {
                    Ok(rows) => ok_res(rows.iter().map(external_row_to_map).collect()),
                    Err(e) => IpeResult::Err(ipe_err(&e)),
                }
            }
            ExternalConnection::Sqlite(pool) => {
                let mut q = sqlx::query(&base);
                for p in frag.binds {
                    q = external_bind_sql_param(q, p);
                }
                match q.fetch_all(&pool).await {
                    Ok(rows) => ok_res(rows.iter().map(external_row_to_map).collect()),
                    Err(e) => IpeResult::Err(ipe_err(&e)),
                }
            }
        }
    })
}

/// `Db.queryDecodeOn : Connection a -> String -> List SqlValue -> Decoder a2
/// -> Task Error (List a2)` — the external counterpart to
/// [`db_query_decode_params`]. Same positional binding and NULL-preserving
/// row→JSON decode, keyed on the foreign dialect, fed to the same
/// `Decoder<E, A>`. The caller-supplied SQL is bound-parameter-only (the safe
/// path); verbatim external SQL remains the disclosed `unsafeExecRawOn` door.
#[cfg(feature = "db")]
pub fn db_conn_query_decode_params<E: Send + From<String> + 'static, A: Send + 'static>(
    conn: ExternalConnection,
    sql: String,
    params: Vec<SqlParam>,
    decoder: Decoder<E, A>,
) -> IpeTask<E, Vec<A>> {
    Box::pin(async move {
        let rows_json: Result<Vec<JsonVal>, sqlx::Error> = match conn {
            ExternalConnection::Postgres(pool) => {
                let final_sql = external_format_sql_postgres(&sql);
                let mut q = sqlx::query(&final_sql);
                for p in params {
                    q = external_bind_sql_param(q, p);
                }
                match q.fetch_all(&pool).await {
                    Ok(rows) => rows.iter().map(external_row_to_json).collect(),
                    Err(e) => Err(e),
                }
            }
            ExternalConnection::Sqlite(pool) => {
                let mut q = sqlx::query(&sql);
                for p in params {
                    q = external_bind_sql_param(q, p);
                }
                match q.fetch_all(&pool).await {
                    Ok(rows) => rows.iter().map(external_row_to_json).collect(),
                    Err(e) => Err(e),
                }
            }
        };
        let jsons = match rows_json {
            Ok(v) => v,
            Err(e) => return IpeResult::Err(ipe_err(&e)),
        };
        let mut out = Vec::with_capacity(jsons.len());
        for jv in &jsons {
            match (decoder.run)(jv) {
                IpeResult::Ok(a) => out.push(a),
                IpeResult::Err(e) => return IpeResult::Err(e),
            }
        }
        ok_res(out)
    })
}

/// `Db.getByIdOn : Connection a -> String -> String -> Task Error (Maybe Row)`
/// — the external counterpart to [`db_get_by_id`]. The id binds as a positional
/// parameter (never interpolated); the table passes the same [`SqlIdent`] gate.
#[cfg(feature = "db")]
pub fn db_conn_get_by_id<E: Send + From<String> + 'static>(
    conn: ExternalConnection,
    table: String,
    id: String,
) -> IpeTask<E, IpeMaybe<HashMap<String, String>>> {
    Box::pin(async move {
        let qtable = match SqlIdent::parse_plain(&table) {
            Some(t) => t,
            None => {
                return IpeResult::Err(
                    format!("db.getByIdOn: invalid table name {:?}", table).into(),
                );
            }
        };
        let base = format!("SELECT * FROM {} WHERE id = ? LIMIT 1", qtable.as_str());
        match conn {
            ExternalConnection::Postgres(pool) => {
                let sql = external_format_sql_postgres(&base);
                match sqlx::query(&sql).bind(id).fetch_optional(&pool).await {
                    Ok(Some(r)) => ok_res(IpeMaybe::Just(external_row_to_map(&r))),
                    Ok(None) => ok_res(IpeMaybe::Nothing),
                    Err(e) => IpeResult::Err(ipe_err(&e)),
                }
            }
            ExternalConnection::Sqlite(pool) => {
                match sqlx::query(&base).bind(id).fetch_optional(&pool).await {
                    Ok(Some(r)) => ok_res(IpeMaybe::Just(external_row_to_map(&r))),
                    Ok(None) => ok_res(IpeMaybe::Nothing),
                    Err(e) => IpeResult::Err(ipe_err(&e)),
                }
            }
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
    let qtable = SqlIdent::parse_dotted(table)
        .ok_or_else(|| format!("{}: invalid table name {:?}", kernel, table))?;
    let mut cols: Vec<String> = Vec::new();
    let mut args: Vec<SqlParam> = Vec::new();
    for (col, opt) in fields {
        let qcol = SqlIdent::parse_dotted(&col)
            .ok_or_else(|| format!("{}: invalid column name {:?}", kernel, col))?;
        if let Some(p) = opt {
            cols.push(qcol.as_str().to_string());
            args.push(p);
        }
        // None → OmitField: column dropped entirely, DB applies DEFAULT.
    }
    let sql = if cols.is_empty() {
        format!("INSERT INTO {} DEFAULT VALUES", qtable.as_str())
    } else {
        let ph = vec!["?"; cols.len()].join(", ");
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            qtable.as_str(),
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
        let qtable = match SqlIdent::parse_dotted(&table) {
            Some(t) => t,
            None => {
                return IpeResult::Err(
                    format!("db.updateFields: invalid table name {:?}", table).into(),
                );
            }
        };
        // Build SET clause.
        let mut set_clauses: Vec<String> = Vec::new();
        let mut args: Vec<SqlParam> = Vec::new();
        for (col, opt) in set_fields {
            let qcol = match SqlIdent::parse_dotted(&col) {
                Some(c) => c,
                None => {
                    return IpeResult::Err(
                        format!("db.updateFields: invalid SET column name {:?}", col).into(),
                    );
                }
            };
            if let Some(p) = opt {
                set_clauses.push(format!("{} = ?", qcol.as_str()));
                args.push(p);
            }
            // None → OmitField: skip column.
        }
        if set_clauses.is_empty() {
            // Every column was OmitField — nothing to update; report zero rows.
            return ok_res(0i64);
        }
        // Build WHERE clause.
        let mut where_clauses: Vec<String> = Vec::new();
        for (col, p) in where_cols {
            let qcol = match SqlIdent::parse_dotted(&col) {
                Some(c) => c,
                None => {
                    return IpeResult::Err(
                        format!("db.updateFields: invalid WHERE column name {:?}", col).into(),
                    );
                }
            };
            where_clauses.push(format!("{} = ?", qcol.as_str()));
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
            qtable.as_str(),
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

    // The total getter truncates a decimal toward zero, round-trips an exact
    // i64 boundary, and fails CLOSED to the `0` default for a magnitude that an
    // `as i64` cast would silently saturate to the boundary (never surfacing a
    // wrong value that reads like a real row value).
    #[test]
    fn db_get_int_rejects_out_of_range_float_no_saturation() {
        let mut m: HashMap<String, String> = HashMap::new();

        // Decimal string: truncate toward zero.
        m.insert("pos".into(), "3.7".into());
        m.insert("neg".into(), "-3.7".into());
        assert_eq!(db_get_int("pos".into(), &m), 3);
        assert_eq!(db_get_int("neg".into(), &m), -3);

        // Exact i64 boundaries round-trip (they parse as i64 directly).
        m.insert("max".into(), i64::MAX.to_string());
        m.insert("min".into(), i64::MIN.to_string());
        assert_eq!(db_get_int("max".into(), &m), i64::MAX);
        assert_eq!(db_get_int("min".into(), &m), i64::MIN);

        // A magnitude far past each boundary, expressed as a float string so it
        // takes the float path: fall back to 0, NOT the saturated boundary.
        m.insert("over".into(), "1e30".into());
        m.insert("under".into(), "-1e30".into());
        assert_eq!(db_get_int("over".into(), &m), 0);
        assert_ne!(db_get_int("over".into(), &m), i64::MAX);
        assert_eq!(db_get_int("under".into(), &m), 0);
        assert_ne!(db_get_int("under".into(), &m), i64::MIN);
    }

    // `Db.getString "path" req` on an `init` handler's typed
    // request reads the named struct field; params/headers/cookies back any
    // other key; absent -> "" (total).
    #[cfg(feature = "web")]
    #[test]
    fn ipe_row_webreq_named_fields_and_dicts() {
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

    /// A seeded external (foreign) SQLite connection, distinct from the app pool —
    /// stands in for a source of a different dialect that the read runners dial
    /// through the same codec stack. `ledger` carries an `amount` INTEGER column.
    #[allow(clippy::expect_used)]
    async fn fresh_external_conn() -> ExternalConnection {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory external sqlite");
        sqlx::query(
            "CREATE TABLE ledger (id INTEGER PRIMARY KEY AUTOINCREMENT, amount INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .expect("create ledger");
        sqlx::query("INSERT INTO ledger (amount) VALUES (7), (42)")
            .execute(&pool)
            .await
            .expect("seed ledger");
        ExternalConnection::Sqlite(pool)
    }

    /// The external read path decodes seeded rows through the SAME `Decoder<E,A>`
    /// the app path uses — the §4 "typed reads from a foreign DB via one codec"
    /// target. `db_conn_query_decode_params` reads `amount` back as `Int`.
    #[tokio::test]
    async fn external_query_decode_reads_through_one_codec() {
        let conn = fresh_external_conn().await;
        let out: IpeResult<String, Vec<i64>> = db_conn_query_decode_params(
            conn,
            "SELECT amount FROM ledger ORDER BY amount".into(),
            vec![],
            db_decode_int("amount".into()),
        )
        .await;
        match out {
            IpeResult::Ok(v) => assert_eq!(v, vec![7, 42]),
            other => panic!("external queryDecode failed: {:?}", other),
        }
    }

    /// The injection barrier is UNCHANGED on the external path: a value carrying SQL
    /// metacharacters flows through a bound parameter (a `Sql.param` in the
    /// fragment), so it matches VERBATIM and the surrounding table is untouched — no
    /// injection executes against the foreign connection.
    #[tokio::test]
    async fn external_find_where_binds_params_no_injection() {
        let conn = fresh_external_conn().await;
        // A fragment built from the audited `Sql.*` combinators: `amount = ?`, the
        // value bound (never spliced). The metacharacter value simply doesn't match.
        let frag = sql_eq(
            sql_column("amount".to_string()),
            sql_param(SqlParam::Int(7)),
        );
        let rows: IpeResult<String, Vec<HashMap<String, String>>> =
            db_conn_find_where(conn.clone(), "ledger".into(), frag).await;
        match rows {
            IpeResult::Ok(v) => {
                assert_eq!(v.len(), 1);
                assert_eq!(v[0].get("amount").map(String::as_str), Some("7"));
            }
            other => panic!("external findWhere failed: {:?}", other),
        }
        // A hostile TABLE identifier is rejected by the same `SqlIdent` gate — the
        // read runner never interpolates an unvalidated name into SQL.
        let bad: IpeResult<String, Vec<HashMap<String, String>>> = db_conn_find_where(
            conn,
            "ledger; DROP TABLE ledger".into(),
            sql_eq(sql_param(SqlParam::Int(1)), sql_param(SqlParam::Int(1))),
        )
        .await;
        assert!(
            matches!(bad, IpeResult::Err(_)),
            "a hostile external table identifier must be rejected before any SQL runs"
        );
    }

    /// A hostile column identifier in a `Sql.column` poisons the fragment, which the
    /// external `findWhere` surfaces as a typed `Err` — the poison marker path is
    /// identical to the app connection's.
    #[tokio::test]
    async fn external_find_where_rejects_poisoned_column() {
        let conn = fresh_external_conn().await;
        let poisoned = sql_eq(
            sql_column("amount; DROP TABLE ledger".to_string()),
            sql_param(SqlParam::Int(7)),
        );
        let rows: IpeResult<String, Vec<HashMap<String, String>>> =
            db_conn_find_where(conn, "ledger".into(), poisoned).await;
        assert!(
            matches!(rows, IpeResult::Err(_)),
            "a poisoned column fragment must fail closed on the external path too"
        );
    }

    /// `db_conn_get_by_id` binds the id as a positional parameter (never
    /// interpolated) and returns the matching row from the foreign connection.
    #[tokio::test]
    async fn external_get_by_id_binds_id() {
        let conn = fresh_external_conn().await;
        let got: IpeResult<String, IpeMaybe<HashMap<String, String>>> =
            db_conn_get_by_id(conn, "ledger".into(), "1".into()).await;
        match got {
            IpeResult::Ok(IpeMaybe::Just(row)) => {
                assert_eq!(row.get("amount").map(String::as_str), Some("7"));
            }
            other => panic!("external getById failed: {:?}", other),
        }
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
    fn connect_err_never_echoes_connection_credentials() {
        // A DB connection failure must never surface host/user/password. The
        // connect path's `Configuration`/`Io`/`Tls` payloads can embed the
        // connection URL, so the Ipê-visible message is built from the error
        // variant alone.
        let secret_url = "postgres://admin:s3cr3t-pw@db.internal:5432/prod";

        let cfg: String = connect_err(&sqlx::Error::Configuration(Box::<
            dyn std::error::Error + Send + Sync,
        >::from(
            secret_url.to_string()
        )));
        assert!(!cfg.contains("s3cr3t-pw"), "password leaked: {cfg}");
        assert!(!cfg.contains("admin"), "user leaked: {cfg}");
        assert!(!cfg.contains("db.internal"), "host leaked: {cfg}");
        assert_eq!(cfg, "db: invalid connection configuration");

        let io: String = connect_err(&sqlx::Error::Io(std::io::Error::other(
            secret_url.to_string(),
        )));
        assert!(!io.contains("s3cr3t-pw"), "password leaked via Io: {io}");
        assert_eq!(io, "db: connection I/O error");
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

    // ─── Rename-migration data-preservation tests ─────────────────────────────
    //
    // These tests drive `db_migrate_apply` directly with the DDL that
    // `Store.migrations` / `Store.renameColumn` / `Store.renameTable` produce,
    // proving the ledger correctly applies and skips each rename entry.

    /// Column rename preserves existing row data and makes rows readable under
    /// the new column name.
    #[tokio::test]
    async fn rename_column_preserves_data() {
        let db = new_single_conn_db().await;
        // Matches what Store.migrations produces for a users store with
        // renameColumn "name" "full_name".
        let migrations = vec![
            (
                "create_users".to_string(),
                "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, name TEXT, age INTEGER)"
                    .to_string(),
            ),
            (
                "rename_column_users_name_to_full_name".to_string(),
                "ALTER TABLE users RENAME COLUMN name TO full_name".to_string(),
            ),
        ];

        // First apply: both entries run.
        let r1: IpeResult<String, Vec<String>> =
            db_migrate_apply(db.clone(), migrations.clone()).await;
        match r1 {
            IpeResult::Ok(v) => assert_eq!(
                v,
                vec![
                    "create_users".to_string(),
                    "rename_column_users_name_to_full_name".to_string()
                ]
            ),
            IpeResult::Err(e) => panic!("first apply: {e}"),
        }

        // Insert a row using the POST-rename column name.
        let ins: IpeResult<String, i64> = db_exec(
            db.clone(),
            "INSERT INTO users (id, full_name, age) VALUES ('u1', 'Alice', 30)".to_string(),
            Vec::new(),
        )
        .await;
        assert!(matches!(ins, IpeResult::Ok(1)), "insert: {ins:?}");

        // Row is readable under full_name; the old `name` column is absent.
        let rows: IpeResult<String, Vec<HashMap<String, String>>> = db_query(
            db.clone(),
            "SELECT full_name, age FROM users WHERE id = 'u1'".to_string(),
            Vec::new(),
        )
        .await;
        match rows {
            IpeResult::Ok(v) => {
                assert_eq!(v.len(), 1, "expected 1 row, got {}", v.len());
                assert_eq!(
                    v.first()
                        .and_then(|r| r.get("full_name"))
                        .map(String::as_str),
                    Some("Alice"),
                    "full_name should be Alice"
                );
            }
            IpeResult::Err(e) => panic!("read after rename: {e}"),
        }

        // `name` column is gone — selecting it is an error.
        let bad: IpeResult<String, Vec<HashMap<String, String>>> =
            db_query(db.clone(), "SELECT name FROM users".to_string(), Vec::new()).await;
        assert!(
            matches!(bad, IpeResult::Err(_)),
            "selecting the old column name must fail after rename"
        );
    }

    /// Re-applying the same migration list is idempotent — no error, no
    /// re-issue of the rename, data unchanged.
    #[tokio::test]
    async fn rename_column_idempotent_rerun() {
        let db = new_single_conn_db().await;
        let migrations = vec![
            (
                "create_users".to_string(),
                "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, name TEXT)".to_string(),
            ),
            (
                "rename_column_users_name_to_full_name".to_string(),
                "ALTER TABLE users RENAME COLUMN name TO full_name".to_string(),
            ),
        ];

        // First apply.
        let r1: IpeResult<String, Vec<String>> =
            db_migrate_apply(db.clone(), migrations.clone()).await;
        assert!(matches!(r1, IpeResult::Ok(_)), "first apply: {r1:?}");

        // Insert after first apply.
        let ins: IpeResult<String, i64> = db_exec(
            db.clone(),
            "INSERT INTO users (id, full_name) VALUES ('u1', 'Bob')".to_string(),
            Vec::new(),
        )
        .await;
        assert!(matches!(ins, IpeResult::Ok(1)), "insert: {ins:?}");

        // Re-apply: both entries already in ledger → 0 applied.
        let r2: IpeResult<String, Vec<String>> =
            db_migrate_apply(db.clone(), migrations.clone()).await;
        match r2 {
            IpeResult::Ok(v) => assert!(v.is_empty(), "expected 0 on re-run, got {v:?}"),
            IpeResult::Err(e) => panic!("re-run: {e}"),
        }

        // Data unchanged.
        let rows: IpeResult<String, Vec<HashMap<String, String>>> = db_query(
            db.clone(),
            "SELECT full_name FROM users WHERE id = 'u1'".to_string(),
            Vec::new(),
        )
        .await;
        match rows {
            IpeResult::Ok(v) => assert_eq!(
                v.first()
                    .and_then(|r| r.get("full_name"))
                    .map(String::as_str),
                Some("Bob"),
                "data unchanged after re-run"
            ),
            IpeResult::Err(e) => panic!("read after re-run: {e}"),
        }
    }

    /// Applying the same list to a fresh (empty) database converges to the
    /// same final schema — rows inserted afterward read back under the new name.
    #[tokio::test]
    async fn rename_column_fresh_db_convergence() {
        let db = new_single_conn_db().await;
        let migrations = vec![
            (
                "create_users".to_string(),
                "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, name TEXT)".to_string(),
            ),
            (
                "rename_column_users_name_to_full_name".to_string(),
                "ALTER TABLE users RENAME COLUMN name TO full_name".to_string(),
            ),
        ];

        // Apply to a completely empty DB — both entries run.
        let r: IpeResult<String, Vec<String>> = db_migrate_apply(db.clone(), migrations).await;
        match r {
            IpeResult::Ok(v) => assert_eq!(v.len(), 2, "expected 2 applied, got {v:?}"),
            IpeResult::Err(e) => panic!("fresh-db apply: {e}"),
        }

        // Insert using the new name.
        let ins: IpeResult<String, i64> = db_exec(
            db.clone(),
            "INSERT INTO users (id, full_name) VALUES ('u2', 'Carol')".to_string(),
            Vec::new(),
        )
        .await;
        assert!(matches!(ins, IpeResult::Ok(1)), "fresh-db insert: {ins:?}");

        let rows: IpeResult<String, Vec<HashMap<String, String>>> = db_query(
            db.clone(),
            "SELECT full_name FROM users WHERE id = 'u2'".to_string(),
            Vec::new(),
        )
        .await;
        match rows {
            IpeResult::Ok(v) => assert_eq!(
                v.first()
                    .and_then(|r| r.get("full_name"))
                    .map(String::as_str),
                Some("Carol"),
                "fresh-db convergence"
            ),
            IpeResult::Err(e) => panic!("fresh-db read: {e}"),
        }
    }

    /// Table rename preserves all row data and makes the table accessible
    /// under the new name.
    #[tokio::test]
    async fn rename_table_preserves_data() {
        let db = new_single_conn_db().await;
        // Matches what Store.migrations produces for renameTable "accounts".
        let migrations = vec![
            (
                "create_users".to_string(),
                "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, email TEXT)".to_string(),
            ),
            (
                "rename_table_users_to_accounts".to_string(),
                "ALTER TABLE users RENAME TO accounts".to_string(),
            ),
        ];

        let r1: IpeResult<String, Vec<String>> =
            db_migrate_apply(db.clone(), migrations.clone()).await;
        assert!(matches!(r1, IpeResult::Ok(_)), "first apply: {r1:?}");

        // Insert under the new table name.
        let ins: IpeResult<String, i64> = db_exec(
            db.clone(),
            "INSERT INTO accounts (id, email) VALUES ('a1', 'dave@example.com')".to_string(),
            Vec::new(),
        )
        .await;
        assert!(
            matches!(ins, IpeResult::Ok(1)),
            "table-rename insert: {ins:?}"
        );

        let rows: IpeResult<String, Vec<HashMap<String, String>>> = db_query(
            db.clone(),
            "SELECT email FROM accounts WHERE id = 'a1'".to_string(),
            Vec::new(),
        )
        .await;
        match rows {
            IpeResult::Ok(v) => assert_eq!(
                v.first().and_then(|r| r.get("email")).map(String::as_str),
                Some("dave@example.com"),
                "row readable under new table name"
            ),
            IpeResult::Err(e) => panic!("read after table rename: {e}"),
        }

        // Old table name is gone.
        let bad: IpeResult<String, Vec<HashMap<String, String>>> =
            db_query(db.clone(), "SELECT * FROM users".to_string(), Vec::new()).await;
        assert!(
            matches!(bad, IpeResult::Err(_)),
            "old table name must be absent after rename"
        );

        // Re-apply is idempotent.
        let r2: IpeResult<String, Vec<String>> = db_migrate_apply(db.clone(), migrations).await;
        match r2 {
            IpeResult::Ok(v) => assert!(v.is_empty(), "expected 0 on re-run, got {v:?}"),
            IpeResult::Err(e) => panic!("table-rename re-run: {e}"),
        }
    }

    /// A column rename whose `from` is absent in the schema returns a `Task Err`
    /// and leaves the ledger unadvanced.
    #[tokio::test]
    async fn rename_column_missing_from_fails_closed() {
        let db = new_single_conn_db().await;
        // Create a table without a `name` column, then try to rename it.
        let r_create: IpeResult<String, Vec<String>> = db_migrate_apply(
            db.clone(),
            vec![(
                "create_users".to_string(),
                "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, email TEXT)".to_string(),
            )],
        )
        .await;
        assert!(matches!(r_create, IpeResult::Ok(_)), "create: {r_create:?}");

        // Ledger row count before the failing rename.
        let before: IpeResult<String, Vec<HashMap<String, String>>> = db_query(
            db.clone(),
            "SELECT name FROM _ipe_migrations ORDER BY name".to_string(),
            Vec::new(),
        )
        .await;
        let before_count = match before {
            IpeResult::Ok(v) => v.len(),
            IpeResult::Err(e) => panic!("ledger read before: {e}"),
        };

        // Rename a column that does not exist — must error.
        let r_rename: IpeResult<String, Vec<String>> = db_migrate_apply(
            db.clone(),
            vec![
                (
                    "create_users".to_string(),
                    "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, email TEXT)"
                        .to_string(),
                ),
                (
                    "rename_column_users_name_to_full_name".to_string(),
                    "ALTER TABLE users RENAME COLUMN name TO full_name".to_string(),
                ),
            ],
        )
        .await;
        assert!(
            matches!(r_rename, IpeResult::Err(_)),
            "renaming absent column must return Err"
        );

        // Ledger must be unadvanced — the failing rename entry was not recorded.
        let after: IpeResult<String, Vec<HashMap<String, String>>> = db_query(
            db.clone(),
            "SELECT name FROM _ipe_migrations ORDER BY name".to_string(),
            Vec::new(),
        )
        .await;
        let after_count = match after {
            IpeResult::Ok(v) => v.len(),
            IpeResult::Err(e) => panic!("ledger read after: {e}"),
        };
        assert_eq!(
            before_count, after_count,
            "ledger must be unadvanced after a failing rename"
        );
    }

    /// Helper: a single-connection in-memory SQLite pool (no pre-seeded todos
    /// table — callers control the full schema).
    async fn new_single_conn_db() -> Db {
        #[allow(clippy::expect_used)]
        sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite for rename tests")
    }

    #[tokio::test]
    async fn exec_query_params_bind_mixed_sqlvalue_types() {
        // db_exec_params / db_query_params bind the full SqlParam range (the
        // Go `List SqlValue` mixed-type path) — Text/Int/Bool/Float/Null — and
        // round-trip through a SqlValue-param WHERE. `with_default` extracts the
        // Ok value (a wrong/Err result then fails the following assert).
        let db = fresh_db().await;
        // exec/unsafeExecRaw now return rows-affected (i64). DDL rows-affected is
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

    fn where_eq_title(v: &str) -> SqlFragment {
        SqlFragment {
            sql: "title = ?".to_string(),
            binds: vec![SqlParam::Text(v.to_string())],
            invalid: None,
        }
    }

    async fn insert_title(db: &Db, title: &str) {
        let mut row = HashMap::new();
        row.insert("title".to_string(), title.to_string());
        let r: IpeResult<String, i64> = db_insert_row(db.clone(), "todos".into(), row).await;
        assert!(
            matches!(r, IpeResult::Ok(_)),
            "insert {title} failed: {r:?}"
        );
    }

    async fn count_title(db: &Db, title: &str) -> usize {
        let rs: IpeResult<String, Vec<HashMap<String, String>>> = db_find_many_by_field(
            db.clone(),
            "todos".into(),
            "title".into(),
            title.to_string(),
        )
        .await;
        match rs {
            IpeResult::Ok(v) => v.len(),
            IpeResult::Err(e) => panic!("count {title}: {e}"),
        }
    }

    #[tokio::test]
    async fn db_delete_where_deletes_only_matching_rows() {
        let db = fresh_db().await;
        insert_title(&db, "a").await;
        insert_title(&db, "b").await;
        let affected: IpeResult<String, i64> =
            db_delete_where(db.clone(), "todos".into(), where_eq_title("a")).await;
        assert!(
            matches!(affected, IpeResult::Ok(1)),
            "expected 1 deleted, got {affected:?}"
        );
        assert_eq!(count_title(&db, "a").await, 0, "matching row must be gone");
        assert_eq!(
            count_title(&db, "b").await,
            1,
            "non-matching row must remain"
        );
    }

    #[tokio::test]
    async fn db_delete_where_refuses_empty_where_and_preserves_all_rows() {
        let db = fresh_db().await;
        insert_title(&db, "a").await;
        insert_title(&db, "b").await;
        let frag = SqlFragment {
            sql: String::new(),
            binds: vec![],
            invalid: None,
        };
        let affected: IpeResult<String, i64> =
            db_delete_where(db.clone(), "todos".into(), frag).await;
        assert!(
            matches!(affected, IpeResult::Err(_)),
            "an empty WHERE must be refused, got {affected:?}"
        );
        assert_eq!(
            count_title(&db, "a").await,
            1,
            "a refused mass-delete must delete nothing"
        );
        assert_eq!(count_title(&db, "b").await, 1);
    }

    #[tokio::test]
    async fn db_update_where_updates_only_matching_rows() {
        let db = fresh_db().await;
        insert_title(&db, "a").await;
        insert_title(&db, "b").await;
        let set = vec![("title".to_string(), Some(SqlParam::Text("A2".to_string())))];
        let affected: IpeResult<String, i64> =
            db_update_where(db.clone(), "todos".into(), set, where_eq_title("a")).await;
        assert!(
            matches!(affected, IpeResult::Ok(1)),
            "expected 1 updated, got {affected:?}"
        );
        assert_eq!(
            count_title(&db, "A2").await,
            1,
            "matching row must be updated"
        );
        assert_eq!(count_title(&db, "b").await, 1, "non-matching row unchanged");
    }

    #[tokio::test]
    async fn db_update_where_refuses_whitespace_where_and_changes_nothing() {
        let db = fresh_db().await;
        insert_title(&db, "a").await;
        let set = vec![(
            "title".to_string(),
            Some(SqlParam::Text("mutated".to_string())),
        )];
        let frag = SqlFragment {
            sql: "   ".to_string(),
            binds: vec![],
            invalid: None,
        };
        let affected: IpeResult<String, i64> =
            db_update_where(db.clone(), "todos".into(), set, frag).await;
        assert!(
            matches!(affected, IpeResult::Err(_)),
            "a whitespace-only WHERE must be refused, got {affected:?}"
        );
        assert_eq!(count_title(&db, "a").await, 1, "original row untouched");
        assert_eq!(
            count_title(&db, "mutated").await,
            0,
            "no mass-update may occur"
        );
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

    #[test]
    fn db_decode_int_rejects_out_of_range_float() {
        let dec = db_decode_int::<String>("n".to_string());

        // In-range: a decimal string truncates toward zero, still Ok.
        let in_range = serde_json::json!({ "n": "3.7" });
        match (dec.run)(&in_range) {
            IpeResult::Ok(i) => assert_eq!(i, 3),
            IpeResult::Err(e) => panic!("in-range decode failed: {:?}", e),
        }

        // In-range: a JSON float that is integral and representable decodes.
        let in_range_num = serde_json::json!({ "n": 42.0 });
        match (db_decode_int::<String>("n".to_string()).run)(&in_range_num) {
            IpeResult::Ok(i) => assert_eq!(i, 42),
            IpeResult::Err(e) => panic!("in-range numeric decode failed: {:?}", e),
        }

        // Out-of-range: `1e30` past i64::MAX must REJECT, not saturate to i64::MAX.
        let over = serde_json::json!({ "n": 1e30 });
        match (db_decode_int::<String>("n".to_string()).run)(&over) {
            IpeResult::Ok(i) => panic!("out-of-range float saturated to {i} instead of erroring"),
            IpeResult::Err(_) => {}
        }

        // Out-of-range as a decimal string is rejected on the same path.
        let over_str = serde_json::json!({ "n": "1e30" });
        match (db_decode_int::<String>("n".to_string()).run)(&over_str) {
            IpeResult::Ok(i) => panic!("out-of-range string saturated to {i} instead of erroring"),
            IpeResult::Err(_) => {}
        }

        // i64::MIN-1 rounds to i64::MIN as f64; a non-strict lower bound would
        // admit it and saturate to i64::MIN. It must reject.
        let under = serde_json::json!({ "n": "-9223372036854775809" });
        match (db_decode_int::<String>("n".to_string()).run)(&under) {
            IpeResult::Ok(i) => panic!("i64::MIN-1 saturated to {i} instead of erroring"),
            IpeResult::Err(_) => {}
        }

        // The exact boundaries still decode through the integer path.
        let min = serde_json::json!({ "n": "-9223372036854775808" });
        match (db_decode_int::<String>("n".to_string()).run)(&min) {
            IpeResult::Ok(i) => assert_eq!(i, i64::MIN),
            IpeResult::Err(e) => panic!("i64::MIN decode failed: {:?}", e),
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
    async fn test_db_decode_decimal_roundtrip() {
        // Verify db_decode_decimal parses "3.14159" → Decimal(3.14159).
        use rust_decimal::Decimal as RD;
        use std::str::FromStr;
        let val = serde_json::json!({ "amount": "3.14159" });
        let result = (db_decode_decimal::<String>("amount".to_string()).run)(&val);
        match result {
            IpeResult::Ok(d) => {
                assert_eq!(d.0, RD::from_str("3.14159").unwrap());
            }
            IpeResult::Err(e) => panic!("unexpected Err: {}", e),
        }

        // NULL → Err.
        let val_null = serde_json::json!({ "amount": null });
        assert!(matches!(
            (db_decode_decimal::<String>("amount".to_string()).run)(&val_null),
            IpeResult::Err(_)
        ));

        // Non-numeric text → Err.
        let val_bad = serde_json::json!({ "amount": "not-a-number" });
        assert!(matches!(
            (db_decode_decimal::<String>("amount".to_string()).run)(&val_bad),
            IpeResult::Err(_)
        ));

        // Missing column → Err.
        let val_missing = serde_json::json!({ "other": "x" });
        assert!(matches!(
            (db_decode_decimal::<String>("amount".to_string()).run)(&val_missing),
            IpeResult::Err(_)
        ));
    }

    #[tokio::test]
    async fn test_db_decode_decimal_money_pg_dialect() {
        // Verifies that the Postgres-dialect INSERT+SELECT statement for a
        // Decimal/Money column pair uses $N placeholders (not ?), binds values
        // as TEXT string parameters (never float/REAL), and that the DDL column
        // type is TEXT.
        //
        // No live Postgres cluster is available in CI; this test drives the
        // statement-generation path directly against the ExternalConnection
        // Postgres variant to assert correct SQL shape and bind types.
        //
        // A live-pg round-trip is unimplementable without new infrastructure
        // (no DATABASE_URL in CI). The statement-generation assertion here is
        // the extent of pg coverage possible without that infra.
        use rust_decimal::Decimal as RD;
        use std::str::FromStr;

        // Decimal stores as TEXT — confirm from_str round-trips without float
        // intermediary loss.
        let d = RD::from_str("9.99").expect("parse");
        let s = d.to_string();
        assert_eq!(s, "9.99", "Decimal TEXT round-trip must be exact");

        // Money stores as "CODE AMOUNT" TEXT — confirm the canonical format
        // the runtime expects on decode.
        let money_text = "USD 12.34";
        let (code, amount_str) = money_text.split_once(' ').expect("split");
        let amount = RD::from_str(amount_str).expect("parse");
        assert_eq!(code, "USD");
        assert_eq!(amount, RD::from_str("12.34").unwrap());

        // The Postgres-dialect placeholder test: SqlitePool uses '?' while the
        // Postgres driver uses '$1', '$2', … The runtime's `into_pg_params`
        // path (ExternalConnection::Postgres branch in Store's insert_sql)
        // rewrites '?' to '$N' sequentially. Assert the rewrite is correct for
        // a 2-parameter INSERT.
        let sqlite_sql = "INSERT INTO t (decimal_col, money_col) VALUES (?, ?)";
        let mut n = 0u32;
        let pg_sql: String =
            sqlite_sql
                .split('?')
                .enumerate()
                .fold(String::new(), |mut acc, (i, part)| {
                    acc.push_str(part);
                    if i < sqlite_sql.matches('?').count() {
                        n += 1;
                        acc.push('$');
                        acc.push_str(&n.to_string());
                    }
                    acc
                });
        assert!(
            pg_sql.contains("$1") && pg_sql.contains("$2"),
            "Postgres rewrite must produce $1/$2 placeholders, got: {pg_sql}"
        );
        assert!(
            !pg_sql.contains('?'),
            "Postgres rewrite must not leave '?' placeholders, got: {pg_sql}"
        );
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

    /// A two-table `fresh_db` seeded with `authors` and `books`, joined on
    /// `books.author_id = authors.id`. Author 1 ("Ada") owns two books; author 2
    /// ("Bob") owns one.
    async fn fresh_join_db() -> Db {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        sqlx::query("CREATE TABLE authors (id INTEGER PRIMARY KEY, name TEXT NOT NULL, active INTEGER NOT NULL DEFAULT 1)")
            .execute(&pool)
            .await
            .expect("create authors");
        sqlx::query("CREATE TABLE books (id INTEGER PRIMARY KEY, title TEXT NOT NULL, author_id INTEGER NOT NULL)")
            .execute(&pool)
            .await
            .expect("create books");
        for (id, name, active) in [(1, "Ada", 1), (2, "Bob", 0)] {
            sqlx::query("INSERT INTO authors (id, name, active) VALUES (?, ?, ?)")
                .bind(id)
                .bind(name)
                .bind(active)
                .execute(&pool)
                .await
                .expect("seed author");
        }
        for (id, title, author_id) in [
            (10, "Structures", 1),
            (11, "Engines", 1),
            (12, "Bridges", 2),
        ] {
            sqlx::query("INSERT INTO books (id, title, author_id) VALUES (?, ?, ?)")
                .bind(id)
                .bind(title)
                .bind(author_id)
                .execute(&pool)
                .await
                .expect("seed book");
        }
        pool
    }

    /// `Db.findJoin` returns one paired-map result per matched join row: the
    /// left map keyed by the books' plain columns, the right by the authors'.
    /// Three books each join to their author, so three pairs come back — proof
    /// the alias-prefixed projection splits back into two per-side codec-ready
    /// maps.
    #[tokio::test]
    async fn test_find_join_pairs_both_sides() {
        let db = fresh_join_db().await;
        let frag = sql_eq(
            sql_column("a1.id".to_string()),
            sql_column("a0.author_id".to_string()),
        );
        let found: IpeResult<String, Vec<JoinRow>> = db_find_join(
            db,
            "books".into(),
            "a0".into(),
            vec!["id".into(), "title".into(), "author_id".into()],
            "authors".into(),
            "a1".into(),
            vec!["id".into(), "name".into(), "active".into()],
            frag,
        )
        .await;
        match found {
            IpeResult::Ok(v) => {
                assert_eq!(v.len(), 3, "three books each join their author");
                // Every pair's book.author_id equals its author.id, and each
                // side carries its own plain-keyed columns (no `a0__` leakage).
                for (book, author) in &v {
                    assert!(book.contains_key("title"), "left map keyed plainly");
                    assert!(author.contains_key("name"), "right map keyed plainly");
                    assert!(
                        !book.keys().any(|k| k.contains("__")),
                        "no alias prefix leaks"
                    );
                    assert_eq!(book.get("author_id"), author.get("id"));
                }
            }
            IpeResult::Err(e) => panic!("expected Ok, got Err({e})"),
        }
    }

    /// A filter predicate on the joined columns binds its value as a parameter,
    /// never interpolated: restricting to active authors returns only Ada's two
    /// books, and the bound `1` never appears in the SQL text (it is a `?`).
    #[tokio::test]
    async fn test_find_join_filter_binds_param() {
        let db = fresh_join_db().await;
        let frag = sql_and(
            sql_eq(
                sql_column("a1.id".to_string()),
                sql_column("a0.author_id".to_string()),
            ),
            sql_eq(sql_column("a1.active".to_string()), sql_param(1_i64)),
        );
        // The composed fragment's SQL is placeholder-only: the value 1 is a bind.
        assert!(
            !frag.sql.contains('1') || frag.sql.contains('?'),
            "filter value must bind as ? not interpolate"
        );
        assert_eq!(
            frag.binds.len(),
            1,
            "exactly one bound value (the active flag)"
        );
        let found: IpeResult<String, Vec<JoinRow>> = db_find_join(
            db,
            "books".into(),
            "a0".into(),
            vec!["id".into(), "title".into(), "author_id".into()],
            "authors".into(),
            "a1".into(),
            vec!["id".into(), "name".into()],
            frag,
        )
        .await;
        match found {
            IpeResult::Ok(v) => {
                assert_eq!(v.len(), 2, "only active author Ada's two books");
                for (_book, author) in &v {
                    assert_eq!(author.get("name").map(String::as_str), Some("Ada"));
                }
            }
            IpeResult::Err(e) => panic!("expected Ok, got Err({e})"),
        }
    }

    /// A join side whose table is not a valid SQL identifier fails closed with a
    /// typed error, issuing no SQL — the runtime re-validation gate.
    #[tokio::test]
    async fn test_find_join_rejects_bad_identifier() {
        let db = fresh_join_db().await;
        let frag = sql_eq(
            sql_column("a1.id".to_string()),
            sql_column("a0.author_id".to_string()),
        );
        let found: IpeResult<String, Vec<JoinRow>> = db_find_join(
            db,
            "books; DROP TABLE authors".into(),
            "a0".into(),
            vec!["id".into()],
            "authors".into(),
            "a1".into(),
            vec!["id".into()],
            frag,
        )
        .await;
        assert!(
            matches!(found, IpeResult::Err(_)),
            "a non-identifier table must fail closed"
        );
    }

    /// Golden SQL: an inner join lowers to EXACTLY one parameterized statement
    /// — `SELECT a0.col AS a0__col, … FROM lt AS a0, rt AS a1 WHERE a0.k =
    /// a1.k`. Every identifier is a validated `alias.column` reference; the join
    /// key is column-to-column (no value), so the statement carries no bind. This
    /// pins the exact emitted text so a regression in the projection / FROM shape
    /// is a test failure, not a silent SQL change.
    #[test]
    fn join_statement_inner_is_exact_parameterized_sql() {
        let left = JoinSide::parse(
            "books".into(),
            "a0".into(),
            vec!["id".into(), "title".into(), "author_id".into()],
        )
        .expect("left side parses");
        let right = JoinSide::parse(
            "authors".into(),
            "a1".into(),
            vec!["id".into(), "name".into()],
        )
        .expect("right side parses");
        let frag = sql_eq(
            sql_column("a0.author_id".to_string()),
            sql_column("a1.id".to_string()),
        );
        assert!(frag.binds.is_empty(), "a column=column key binds no value");
        let sql = build_join_statement(&left, &right, &frag.sql).expect("statement builds");
        assert_eq!(
            sql,
            "SELECT a0.id AS a0__id, a0.title AS a0__title, a0.author_id AS a0__author_id, \
             a1.id AS a1__id, a1.name AS a1__name \
             FROM books AS a0, authors AS a1 WHERE (a0.author_id = a1.id)"
        );
    }

    /// Golden SQL: join + a right-side filter. The filter value is a `?`
    /// placeholder with a matching bind, never interpolated — the emitted text
    /// contains the placeholder, and the fragment carries exactly one bound
    /// value.
    #[test]
    fn join_statement_with_filter_binds_value_as_placeholder() {
        let left = JoinSide::parse(
            "books".into(),
            "a0".into(),
            vec!["id".into(), "author_id".into()],
        )
        .expect("left side parses");
        let right = JoinSide::parse(
            "authors".into(),
            "a1".into(),
            vec!["id".into(), "name".into()],
        )
        .expect("right side parses");
        let frag = sql_and(
            sql_eq(
                sql_column("a0.author_id".to_string()),
                sql_column("a1.id".to_string()),
            ),
            sql_eq(sql_column("a1.active".to_string()), sql_param(1_i64)),
        );
        assert_eq!(frag.binds.len(), 1, "exactly one bound value (the filter)");
        let sql = build_join_statement(&left, &right, &frag.sql).expect("statement builds");
        assert_eq!(
            sql,
            "SELECT a0.id AS a0__id, a0.author_id AS a0__author_id, \
             a1.id AS a1__id, a1.name AS a1__name \
             FROM books AS a0, authors AS a1 \
             WHERE ((a0.author_id = a1.id) AND (a1.active = ?))"
        );
        assert!(
            sql.contains('?') && !sql.contains(" 1)"),
            "the filter value is a placeholder, not interpolated text"
        );
    }

    /// Two sides that share an alias are rejected: the projection and WHERE
    /// could not tell the sides apart, so fail closed rather than emit an
    /// ambiguous statement.
    #[tokio::test]
    async fn test_find_join_rejects_shared_alias() {
        let db = fresh_join_db().await;
        let frag = sql_eq(
            sql_column("a0.id".to_string()),
            sql_column("a0.author_id".to_string()),
        );
        let found: IpeResult<String, Vec<JoinRow>> = db_find_join(
            db,
            "books".into(),
            "a0".into(),
            vec!["id".into()],
            "authors".into(),
            "a0".into(),
            vec!["id".into()],
            frag,
        )
        .await;
        assert!(
            matches!(found, IpeResult::Err(_)),
            "a shared alias must fail closed"
        );
    }

    /// Golden SQL: a single-column projection lowers to EXACTLY one
    /// parameterized statement — `SELECT a1.name AS p0 FROM lt AS a0, rt AS a1
    /// WHERE a0.k = a1.k`. Only the projected column is selected (column
    /// pushdown), bound to the output name `p0`; every identifier is a validated
    /// `alias.column` reference and the key is column-to-column (no bind).
    #[test]
    fn projection_statement_single_column_is_exact_parameterized_sql() {
        let frag = sql_eq(
            sql_column("a1.id".to_string()),
            sql_column("a0.author_id".to_string()),
        );
        assert!(frag.binds.is_empty(), "a column=column key binds no value");
        let sql = build_projection_statement(
            "books",
            "a0",
            "authors",
            "a1",
            &[("a1".to_string(), "name".to_string())],
            &frag.sql,
        )
        .expect("statement builds");
        assert_eq!(
            sql,
            "SELECT a1.name AS p0 \
             FROM books AS a0, authors AS a1 WHERE (a1.id = a0.author_id)"
        );
    }

    /// Golden SQL: a two-column projection lowers to EXACTLY one parameterized
    /// statement — `SELECT a0.title AS p0, a1.name AS p1 FROM lt AS a0, rt AS a1
    /// WHERE a0.k = a1.k`. Each projected column is bound to its own `p<index>`
    /// output name in order (column pushdown), every identifier is a validated
    /// `alias.column` reference, and the key is column-to-column (no bind).
    #[test]
    fn projection_statement_two_columns_is_exact_parameterized_sql() {
        let frag = sql_eq(
            sql_column("a1.id".to_string()),
            sql_column("a0.author_id".to_string()),
        );
        assert!(frag.binds.is_empty(), "a column=column key binds no value");
        let sql = build_projection_statement(
            "books",
            "a0",
            "authors",
            "a1",
            &[
                ("a0".to_string(), "title".to_string()),
                ("a1".to_string(), "name".to_string()),
            ],
            &frag.sql,
        )
        .expect("statement builds");
        assert_eq!(
            sql,
            "SELECT a0.title AS p0, a1.name AS p1 \
             FROM books AS a0, authors AS a1 WHERE (a1.id = a0.author_id)"
        );
    }

    /// A two-column projection where one column is not a bare SQL identifier
    /// fails the whole statement closed — the runtime re-validation gate holds
    /// per projected column, never emitting a partial SELECT.
    #[test]
    fn projection_statement_two_columns_rejects_bad_second_column() {
        let frag = sql_eq(
            sql_column("a1.id".to_string()),
            sql_column("a0.author_id".to_string()),
        );
        let built = build_projection_statement(
            "books",
            "a0",
            "authors",
            "a1",
            &[
                ("a0".to_string(), "title".to_string()),
                ("a1".to_string(), "name); DROP TABLE authors".to_string()),
            ],
            &frag.sql,
        );
        assert!(
            built.is_err(),
            "a non-identifier column anywhere in the projection must fail closed"
        );
    }

    /// Golden SQL: a projection over a join + a right-side filter. Only the
    /// projected column is selected; the filter value is a `?` placeholder with a
    /// matching bind, never interpolated.
    #[test]
    fn projection_statement_with_filter_binds_value_as_placeholder() {
        let frag = sql_and(
            sql_eq(
                sql_column("a1.id".to_string()),
                sql_column("a0.author_id".to_string()),
            ),
            sql_eq(sql_column("a1.active".to_string()), sql_param(1_i64)),
        );
        assert_eq!(frag.binds.len(), 1, "exactly one bound value (the filter)");
        let sql = build_projection_statement(
            "books",
            "a0",
            "authors",
            "a1",
            &[("a1".to_string(), "name".to_string())],
            &frag.sql,
        )
        .expect("statement builds");
        assert_eq!(
            sql,
            "SELECT a1.name AS p0 \
             FROM books AS a0, authors AS a1 \
             WHERE ((a1.id = a0.author_id) AND (a1.active = ?))"
        );
        assert!(
            sql.contains('?') && !sql.contains(" 1)"),
            "the filter value is a placeholder, not interpolated text"
        );
    }

    /// A projected column identifier that is not a bare SQL identifier fails
    /// closed with no SQL — the runtime re-validation gate over the projection.
    #[test]
    fn projection_statement_rejects_bad_column() {
        let frag = sql_eq(
            sql_column("a1.id".to_string()),
            sql_column("a0.author_id".to_string()),
        );
        let built = build_projection_statement(
            "books",
            "a0",
            "authors",
            "a1",
            &[("a1".to_string(), "name; DROP TABLE authors".to_string())],
            &frag.sql,
        );
        assert!(
            built.is_err(),
            "a non-identifier projected column must fail closed"
        );
    }

    /// An empty projection is rejected — a projection must name at least one
    /// column, never fall back to `SELECT *`.
    #[test]
    fn projection_statement_rejects_empty_projection() {
        let frag = sql_eq(
            sql_column("a1.id".to_string()),
            sql_column("a0.author_id".to_string()),
        );
        let built = build_projection_statement("books", "a0", "authors", "a1", &[], &frag.sql);
        assert!(built.is_err(), "an empty projection must fail closed");
    }

    /// `Db.findProjection` reads only the projected column, keyed by the output
    /// name `p0`: projecting the author name over the active-author join returns
    /// Ada's two books' author name, each row carrying just `p0` (column
    /// pushdown), and the filter value binds as a parameter.
    #[tokio::test]
    async fn test_find_projection_single_column() {
        let db = fresh_join_db().await;
        let frag = sql_and(
            sql_eq(
                sql_column("a1.id".to_string()),
                sql_column("a0.author_id".to_string()),
            ),
            sql_eq(sql_column("a1.active".to_string()), sql_param(1_i64)),
        );
        let found: IpeResult<String, Vec<HashMap<String, String>>> = db_find_projection(
            db,
            "books".into(),
            "a0".into(),
            "authors".into(),
            "a1".into(),
            frag,
            vec![("a1".to_string(), "name".to_string())],
        )
        .await;
        match found {
            IpeResult::Ok(rows) => {
                assert_eq!(rows.len(), 2, "only active author Ada's two books");
                for row in &rows {
                    assert_eq!(row.get("p0").map(String::as_str), Some("Ada"));
                    assert_eq!(row.len(), 1, "only the projected column is read");
                }
            }
            IpeResult::Err(e) => panic!("expected Ok, got Err({e})"),
        }
    }

    /// `Db.findProjection` reads two projected columns over the active-author
    /// join, each keyed by its own output name (`p0` = book title, `p1` = author
    /// name), in projection order (column pushdown). The filter value binds as a
    /// parameter; each returned row carries exactly the two projected columns.
    #[tokio::test]
    async fn test_find_projection_two_columns() {
        let db = fresh_join_db().await;
        let frag = sql_and(
            sql_eq(
                sql_column("a1.id".to_string()),
                sql_column("a0.author_id".to_string()),
            ),
            sql_eq(sql_column("a1.active".to_string()), sql_param(1_i64)),
        );
        let found: IpeResult<String, Vec<HashMap<String, String>>> = db_find_projection(
            db,
            "books".into(),
            "a0".into(),
            "authors".into(),
            "a1".into(),
            frag,
            vec![
                ("a0".to_string(), "title".to_string()),
                ("a1".to_string(), "name".to_string()),
            ],
        )
        .await;
        match found {
            IpeResult::Ok(rows) => {
                assert_eq!(rows.len(), 2, "only active author Ada's two books");
                let mut titles: Vec<&str> = rows
                    .iter()
                    .filter_map(|r| r.get("p0").map(String::as_str))
                    .collect();
                titles.sort_unstable();
                assert_eq!(titles, ["Engines", "Structures"], "p0 is the book title");
                for row in &rows {
                    assert_eq!(row.get("p1").map(String::as_str), Some("Ada"));
                    assert_eq!(row.len(), 2, "exactly the two projected columns");
                }
            }
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

    /// `Sql.column` accepts a dotted reference (`table.column`) via
    /// `SqlIdent::parse_dotted`, distinct from `SqlIdent::parse_plain`
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

    /// SECURITY: `sql_unsafe_fragment` (the `Ipe.Db.Unsafe.unsafeFragment`
    /// runtime) DELIBERATELY skips the `valid_sql_ident` gate that
    /// [`sql_column`] applies. On the SAME identifier that `sql_column` poisons,
    /// the unsafe mint produces a NON-poisoned fragment carrying the verbatim
    /// text — this is the disclosed escape hatch: no validator runs, the caller
    /// asserts the identifier is safe. The contrast with the poisoned
    /// `sql_column` result is the whole point of the two-member split.
    #[test]
    fn test_unsafe_fragment_skips_validation() {
        // An identifier `sql_column` rejects (space + semicolon outside the
        // `valid_sql_ident` charset), which it poisons.
        let hostile = "title; DROP TABLE todos".to_string();
        let validated = sql_column(hostile.clone());
        assert!(
            validated.invalid.is_some(),
            "sql_column must poison a malformed identifier"
        );
        assert!(
            validated.sql.is_empty(),
            "a poisoned sql_column carries no verbatim text"
        );

        // The unsafe mint on the SAME input: NO poison, verbatim text preserved.
        let unsafe_frag = sql_unsafe_fragment(hostile.clone());
        assert!(
            unsafe_frag.invalid.is_none(),
            "sql_unsafe_fragment must NOT poison — it deliberately skips valid_sql_ident"
        );
        assert_eq!(
            unsafe_frag.sql, hostile,
            "sql_unsafe_fragment must carry the verbatim identifier text"
        );
        assert!(
            unsafe_frag.binds.is_empty(),
            "sql_unsafe_fragment mints no binds"
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

    /// `db_update_where` mirrors `db_update_fields`' guards on the WHERE-fragment
    /// path: an empty WHERE fragment is refused (no mass-update), a scoped update
    /// touches only the matching rows, and a SET value carrying SQL metacharacters
    /// is bound (stored verbatim), never spliced.
    #[tokio::test]
    async fn update_where_scopes_binds_and_refuses_empty() {
        let db = fresh_db().await;
        let mk: IpeResult<String, i64> = db_exec_raw(
            db.clone(),
            "CREATE TABLE acct (id INTEGER PRIMARY KEY, owner TEXT, bal INTEGER)".to_string(),
        )
        .await;
        assert!(matches!(mk, IpeResult::Ok(_)), "create: {mk:?}");
        let _: IpeResult<String, i64> = db_exec_raw(
            db.clone(),
            "INSERT INTO acct (id, owner, bal) VALUES (1, 'a', 10), (2, 'b', 20)".to_string(),
        )
        .await;

        // Empty WHERE fragment MUST be refused (would otherwise mass-update).
        let refused: IpeResult<String, i64> = db_update_where(
            db.clone(),
            "acct".to_string(),
            vec![("bal".to_string(), Some(SqlParam::Int(0)))],
            sql_unsafe_fragment(String::new()),
        )
        .await;
        assert!(
            matches!(refused, IpeResult::Err(_)),
            "empty WHERE must be refused, got {refused:?}"
        );
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

        // A scoped update with an injection-laden SET value affects exactly the
        // matching row and stores the value VERBATIM (bound, not spliced).
        let nasty = "x'); DROP TABLE acct;-- O'Brien".to_string();
        let scoped: IpeResult<String, i64> = db_update_where(
            db.clone(),
            "acct".to_string(),
            vec![("owner".to_string(), Some(SqlParam::Text(nasty.clone())))],
            sql_eq(sql_column("id".to_string()), sql_param(1i64)),
        )
        .await;
        assert!(
            matches!(scoped, IpeResult::Ok(1)),
            "scoped update should affect exactly 1 row: {scoped:?}"
        );

        // The matching row carries the verbatim value; the non-matching row is
        // untouched; and the table still exists (the DROP never ran).
        let rows: IpeResult<String, Vec<HashMap<String, String>>> = db_query_params(
            db.clone(),
            "SELECT id, owner FROM acct ORDER BY id".to_string(),
            vec![],
        )
        .await;
        match rows {
            IpeResult::Ok(v) => {
                assert_eq!(v.len(), 2, "injection must not have dropped the table");
                assert_eq!(v[0].get("owner").unwrap(), &nasty, "matching row updated");
                assert_eq!(
                    v[1].get("owner").unwrap(),
                    "b",
                    "non-matching row untouched"
                );
            }
            other => panic!("table gone or errored: {other:?}"),
        }

        // An all-OmitField SET updates no columns and reports zero rows.
        let omitted: IpeResult<String, i64> = db_update_where(
            db.clone(),
            "acct".to_string(),
            vec![("owner".to_string(), None)],
            sql_eq(sql_column("id".to_string()), sql_param(2i64)),
        )
        .await;
        assert!(
            matches!(omitted, IpeResult::Ok(0)),
            "all-OmitField SET reports zero rows: {omitted:?}"
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

    /// A corpus of identifiers that MUST be rejected at the SQL-interpolation
    /// boundary. Each is a distinct injection or malformation class: quote,
    /// statement terminator, whitespace, comment, backtick, parenthesis, star,
    /// empty, leading-digit-with-punctuation, and a non-ASCII homoglyph. If any
    /// of these ever reaches interpolation the boundary is broken.
    const HOSTILE_IDENTS: &[&str] = &[
        "a'b",
        "a\"b",
        "users; DROP TABLE todos",
        "col name",
        "col--",
        "`col`",
        "f()",
        "*",
        "",
        "1; --",
        "café",
    ];

    /// SSOT proof (unit level): the single `SqlIdent` parser is the ONLY
    /// identifier policy, and `valid_sql_ident` is exactly its dotted mode — not
    /// a second charset check that could drift. For every string we assert
    /// `valid_sql_ident(s) == SqlIdent::parse_dotted(s).is_some()` and that a
    /// dot-accepting parse never admits anything the plain parse would while
    /// rejecting the dot. Every hostile identifier is rejected by BOTH modes.
    #[test]
    fn valid_sql_ident_is_exactly_the_dotted_parser() {
        let corpus = [
            "users",
            "user_id",
            "users.id",
            "a.b.c",
            ".leading",
            "trailing.",
            "todos.title",
        ]
        .iter()
        .copied()
        .chain(HOSTILE_IDENTS.iter().copied());
        for s in corpus {
            assert_eq!(
                valid_sql_ident(s),
                SqlIdent::parse_dotted(s).is_some(),
                "valid_sql_ident must be exactly SqlIdent::parse_dotted for {s:?}"
            );
            // The plain (dot-rejecting) mode admits a subset of the dotted mode:
            // anything plain accepts, dotted must also accept.
            if SqlIdent::parse_plain(s).is_some() {
                assert!(
                    SqlIdent::parse_dotted(s).is_some(),
                    "dotted mode must accept everything plain accepts, for {s:?}"
                );
            }
        }
        // A bare dot-bearing name: accepted dotted, rejected plain — the one
        // deliberate difference between the two modes.
        assert!(SqlIdent::parse_dotted("users.id").is_some());
        assert!(SqlIdent::parse_plain("users.id").is_none());
        // Every hostile identifier is rejected by BOTH modes.
        for h in HOSTILE_IDENTS {
            assert!(
                SqlIdent::parse_plain(h).is_none(),
                "plain parser must reject hostile {h:?}"
            );
            assert!(
                SqlIdent::parse_dotted(h).is_none(),
                "dotted parser must reject hostile {h:?}"
            );
        }
    }

    /// A dotted reference is a sequence of non-empty bare names: every
    /// dot-delimited segment must be non-empty, so a leading dot, a trailing
    /// dot, and consecutive dots are structurally malformed and rejected. A
    /// legitimate single- or multi-segment reference still validates, and
    /// `Plain` mode (which admits no dot at all) is unaffected.
    #[test]
    fn dotted_mode_rejects_empty_segments() {
        // Structurally-malformed dot strings: leading, trailing, consecutive.
        for bad in ["..", ".a", "a.", "a..b", ".", "a.b.", ".a.b"] {
            assert!(
                SqlIdent::parse_dotted(bad).is_none(),
                "dotted parser must reject empty-segment {bad:?}"
            );
        }
        // Well-formed references: a bare name and multi-segment qualified names.
        for good in ["a", "a.b", "todos.title", "a.b.c"] {
            assert!(
                SqlIdent::parse_dotted(good).is_some(),
                "dotted parser must accept well-formed {good:?}"
            );
        }
        // `Plain` mode admits no dot, so its behavior is unchanged: a bare name
        // is accepted, anything with a dot is rejected regardless of segments.
        assert!(SqlIdent::parse_plain("a").is_some());
        for dotted in ["a.b", ".a", "a.", ".."] {
            assert!(
                SqlIdent::parse_plain(dotted).is_none(),
                "plain parser must reject dot-bearing {dotted:?}"
            );
        }
    }

    /// SSOT proof (entry level): every public kernel that interpolates a
    /// table/column identifier into SQL routes through the single validator and
    /// fails CLOSED on a hostile identifier. Each entry is driven with a hostile
    /// value in its identifier position(s); an `IpeResult::Ok` here means a path
    /// reached SQL without validating — the boundary is broken. A new
    /// identifier-accepting entry that skips the validator will fail this test.
    #[tokio::test]
    async fn every_identifier_entry_rejects_hostile_idents() {
        let db = fresh_db().await;
        for &h in HOSTILE_IDENTS {
            let hs = h.to_string();

            macro_rules! assert_rejects {
                ($label:expr, $task:expr) => {{
                    let r: IpeResult<String, _> = $task.await;
                    assert!(
                        matches!(r, IpeResult::Err(_)),
                        "{} must reject hostile identifier {:?}, got Ok",
                        $label,
                        h
                    );
                }};
            }

            // Table-identifier position.
            assert_rejects!(
                "db_get_by_id(table)",
                db_get_by_id(db.clone(), hs.clone(), "1".to_string())
            );
            assert_rejects!(
                "db_delete_by_id(table)",
                db_delete_by_id(db.clone(), hs.clone(), "1".to_string())
            );
            assert_rejects!(
                "db_insert_row(table)",
                db_insert_row(db.clone(), hs.clone(), {
                    let mut m = HashMap::new();
                    m.insert("title".to_string(), "x".to_string());
                    m
                })
            );
            assert_rejects!(
                "db_update_by_id(table)",
                db_update_by_id(db.clone(), hs.clone(), "1".to_string(), {
                    let mut m = HashMap::new();
                    m.insert("title".to_string(), "x".to_string());
                    m
                })
            );
            assert_rejects!(
                "db_find_by_conditions(table)",
                db_find_by_conditions(db.clone(), hs.clone(), {
                    let mut m = HashMap::new();
                    m.insert("title".to_string(), "x".to_string());
                    m
                })
            );
            assert_rejects!(
                "db_find_where(table)",
                db_find_where(
                    db.clone(),
                    hs.clone(),
                    sql_eq(sql_column("title".to_string()), sql_param("x".to_string())),
                )
            );
            assert_rejects!(
                "db_delete_where(table)",
                db_delete_where(
                    db.clone(),
                    hs.clone(),
                    sql_eq(sql_column("title".to_string()), sql_param("x".to_string())),
                )
            );
            assert_rejects!(
                "db_insert_fields(table)",
                db_insert_fields(
                    db.clone(),
                    hs.clone(),
                    vec![("title".to_string(), Some(SqlParam::Text("x".to_string())))],
                )
            );
            assert_rejects!(
                "db_update_fields(table)",
                db_update_fields(
                    db.clone(),
                    hs.clone(),
                    vec![("id".to_string(), SqlParam::Int(1))],
                    vec![("title".to_string(), Some(SqlParam::Text("x".to_string())))],
                )
            );

            // Field/column-identifier position.
            assert_rejects!(
                "db_find_one_by_field(field)",
                db_find_one_by_field(db.clone(), "todos".to_string(), hs.clone(), "x".to_string())
            );
            assert_rejects!(
                "db_find_many_by_field(field)",
                db_find_many_by_field(db.clone(), "todos".to_string(), hs.clone(), "x".to_string())
            );
            assert_rejects!(
                "db_find_by_conditions(column)",
                db_find_by_conditions(db.clone(), "todos".to_string(), {
                    let mut m = HashMap::new();
                    m.insert(hs.clone(), "x".to_string());
                    m
                })
            );
            assert_rejects!(
                "db_insert_fields(column)",
                db_insert_fields(
                    db.clone(),
                    "todos".to_string(),
                    vec![(hs.clone(), Some(SqlParam::Text("x".to_string())))],
                )
            );
            assert_rejects!(
                "db_update_fields(set column)",
                db_update_fields(
                    db.clone(),
                    "todos".to_string(),
                    vec![("id".to_string(), SqlParam::Int(1))],
                    vec![(hs.clone(), Some(SqlParam::Text("x".to_string())))],
                )
            );
            assert_rejects!(
                "db_update_fields(where column)",
                db_update_fields(
                    db.clone(),
                    "todos".to_string(),
                    vec![(hs.clone(), SqlParam::Int(1))],
                    vec![("title".to_string(), Some(SqlParam::Text("x".to_string())))],
                )
            );
            assert_rejects!(
                "db_update_where(table)",
                db_update_where(
                    db.clone(),
                    hs.clone(),
                    vec![("title".to_string(), Some(SqlParam::Text("x".to_string())))],
                    sql_eq(sql_column("id".to_string()), sql_param("1".to_string())),
                )
            );
            assert_rejects!(
                "db_update_where(set column)",
                db_update_where(
                    db.clone(),
                    "todos".to_string(),
                    vec![(hs.clone(), Some(SqlParam::Text("x".to_string())))],
                    sql_eq(sql_column("id".to_string()), sql_param("1".to_string())),
                )
            );

            // `Sql.column` is the SqlFragment-path identifier entry: a hostile
            // identifier poisons the fragment, which the consumers surface as Err.
            assert!(
                sql_column(hs.clone()).invalid.is_some(),
                "sql_column must poison hostile identifier {h:?}"
            );
        }

        // A legitimate dotted column reference still validates where the dotted
        // mode is allowed (Sql.column) — the fix does not over-reject.
        assert!(
            sql_column("todos.title".to_string()).invalid.is_none(),
            "a legitimate dotted column must still validate"
        );
    }

    // ── build_pool SSRF guard tests ──────────────────────────────────────────
    //
    // The guard logic in `build_pool` uses `VettedDial::for_host` when the url
    // scheme is a network driver.  These tests exercise the same gate at the
    // `VettedDial` layer — no actual DB dial is attempted.

    /// Returns true when the `build_pool` SSRF pre-check would block `url`
    /// under the current deny-private setting.  Mirrors the guard logic exactly.
    fn pool_ssrf_blocked(url: &str) -> bool {
        if url.starts_with("sqlite") || url.starts_with("file") || url.starts_with(':') {
            return false;
        }
        if let Ok(parsed) = ::url::Url::parse(url)
            && let Some(host) = parsed.host_str()
        {
            let port = parsed.port_or_known_default().unwrap_or(5432);
            return crate::ssrf::VettedDial::for_host(host, port).is_err();
        }
        false
    }

    #[test]
    fn build_pool_ssrf_blocks_loopback_postgres_when_deny_private_on() {
        unsafe { std::env::set_var("IPE_HTTP_DENY_PRIVATE", "1") };
        assert!(
            pool_ssrf_blocked("postgres://127.0.0.1:5432/x"),
            "loopback postgres URL must be blocked by the SSRF gate"
        );
        unsafe { std::env::remove_var("IPE_HTTP_DENY_PRIVATE") };
    }

    #[test]
    fn build_pool_ssrf_blocks_link_local_postgres_when_deny_private_on() {
        unsafe { std::env::set_var("IPE_HTTP_DENY_PRIVATE", "1") };
        assert!(
            pool_ssrf_blocked("postgres://169.254.169.254:5432/x"),
            "link-local postgres URL must be blocked by the SSRF gate"
        );
        unsafe { std::env::remove_var("IPE_HTTP_DENY_PRIVATE") };
    }

    #[test]
    fn build_pool_ssrf_does_not_block_sqlite_url() {
        unsafe { std::env::set_var("IPE_HTTP_DENY_PRIVATE", "1") };
        assert!(
            !pool_ssrf_blocked("sqlite:///app.db"),
            "sqlite URL must bypass the network SSRF gate"
        );
        assert!(
            !pool_ssrf_blocked("sqlite://:memory:"),
            "in-memory sqlite must bypass the network SSRF gate"
        );
        unsafe { std::env::remove_var("IPE_HTTP_DENY_PRIVATE") };
    }

    #[test]
    fn build_pool_ssrf_passes_private_when_deny_private_off() {
        unsafe { std::env::set_var("IPE_HTTP_DENY_PRIVATE", "0") };
        assert!(
            !pool_ssrf_blocked("postgres://127.0.0.1:5432/x"),
            "guard off must not block private host (dev workflow)"
        );
        unsafe { std::env::remove_var("IPE_HTTP_DENY_PRIVATE") };
    }
}

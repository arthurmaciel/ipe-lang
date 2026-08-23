// Postgres `ipe_runtime/config.rs` template.
//
// NOT compiled into the standalone `ipe-runtime-rust` crate's default build
// (the crate root always uses `config.rs`, the sqlite variant). This file
// exists purely as an `include_str!` source for `ipe_backend_rust::project`
// (see `RUNTIME_CONFIG_RS_DB_POSTGRES`), which writes it verbatim to
// `<emitted-project>/src/ipe_runtime/config.rs` when `ipe.toml`'s
// `[database] driver = "postgres"`.
//
// Every symbol name matches `config.rs`'s sqlite variant exactly (`DbPool`,
// `DbRow`, `ipe_db_url`, `db_last_insert_id`, `db_format_sql`,
// `DB_USES_RETURNING_ID`, `db_auto_id_column`) so `db.rs` — copied verbatim
// across both driver builds — never needs a driver-specific `#[cfg]`.

#[cfg(feature = "db")]
pub type DbPool = sqlx::postgres::PgPool;
#[cfg(feature = "db")]
pub type DbRow = sqlx::postgres::PgRow;
#[cfg(feature = "db")]
pub fn ipe_db_url() -> String {
    // One config precedence `env > setting-in-code > fallback`: `DATABASE_URL`
    // wins, else an installed `Db.url` setting (a `Secret`, revealed only here at
    // the point of use, never logged), else the built-in local fallback.
    crate::system::read_env_var("DATABASE_URL")
        .ok()
        .or_else(crate::app_config::resolve_db_url_override)
        .unwrap_or_else(|| "postgres://postgres@localhost/ipe".to_string())
}

#[cfg(not(feature = "db"))]
pub type DbPool = ();
#[cfg(not(feature = "db"))]
pub type DbRow = ();
#[cfg(not(feature = "db"))]
pub fn ipe_db_url() -> String {
    String::new()
}

// Backend-portability helpers. Mirrors `config.rs`'s sqlite shape.
#[cfg(feature = "db")]
pub fn db_last_insert_id(_res: &sqlx::postgres::PgQueryResult) -> i64 {
    // Postgres's `QueryResult` carries no last-insert-id concept.
    // `DB_USES_RETURNING_ID = true` routes every `db_insert_row` call through
    // the `RETURNING id` branch instead, so this fn is never called on the
    // success path for `db_insert_row`. It IS still reachable from
    // `db_insert_fields` / `db_update_fields` on the non-`RETURNING` path — but
    // `db_insert_fields` (`db.rs`) gates its own call to this fn behind
    // `DB_USES_RETURNING_ID`, exactly like `db_insert_row`, so it is unreached
    // there too. Returning 0 unconditionally would be a fabricated value if it
    // WERE ever reached; kept as a documented "should be unreachable" stub
    // rather than a panic, matching the pre-existing sqlite behaviour for a
    // table with no autoincrement column (never a new regression surface).
    0
}

#[cfg(feature = "db")]
pub fn db_format_sql(sql: String) -> String {
    // Postgres's extended query protocol uses numbered placeholders ($1, $2,
    // …), not sqlite's positional `?`. Every SQL string built throughout
    // `db.rs` is written with `?` placeholders in emission order, matching the
    // order args are bound — a straight sequential rewrite (first `?` → $1,
    // second → $2, …) is therefore correct regardless of which `db.rs`
    // function built the string, with one caveat: a literal `?` inside a
    // *quoted string literal* in the SQL text would be mis-rewritten. `db.rs`
    // never emits a caller-controlled `?` inside a quoted literal (every value
    // is bound as a parameter, never inlined as a literal), so a naive scan is
    // safe here — but any future `db.rs` change that starts inlining string
    // literals containing `?` into SQL text (e.g. a LIKE pattern build helper)
    // MUST re-verify this invariant.
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

// Whether INSERT must use `… RETURNING id` to recover the auto-id (Postgres
// has no LastInsertId).
#[cfg(feature = "db")]
pub const DB_USES_RETURNING_ID: bool = true;

// DDL fragment for an auto-incrementing primary key column.
#[cfg(feature = "db")]
pub fn db_auto_id_column() -> &'static str {
    "id BIGSERIAL PRIMARY KEY"
}

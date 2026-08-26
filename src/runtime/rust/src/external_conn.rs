//! `Ipe.Db.Connection` — a live connection to a database the application was NOT
//! built against (a user-configured external Postgres/SQLite source).
//!
//! # Why this is distinct from `Db`
//!
//! The app's own connection (`Db`) is a pool of the ONE dialect the build fixed
//! from `package.ipe`. An external source can be a DIFFERENT dialect, so its pool
//! cannot reuse the monomorphised `Db` type — it is an independent pool of the
//! driver named by the parsed [`Dsn`]. Both sqlx drivers link under the `db`
//! feature, so either dialect is buildable here.
//!
//! # The read-only-by-type guarantee
//!
//! An external source is untrusted foreign data; a monitoring/ETL reader must not
//! silently mutate it. That posture is a TYPE, not a runtime flag: the emitted
//! Ipê `Connection` carries a phantom access-mode parameter (`ReadOnly` /
//! `ReadWrite`), and every mutating kernel requires `Connection ReadWrite` in its
//! signature. A `Connection ReadOnly` from `open` therefore CANNOT type-check into
//! a write — the violation is an `ipe`-time compile error, never a runtime check.
//! The phantom is erased at emit: both modes are this one concrete
//! [`ExternalConnection`], so there is no `dyn`, one concrete pool per position.
//!
//! # Fail-closed lifecycle
//!
//! Each `open` yields its OWN pool (bounded, never a process-wide URL-keyed cache
//! that could alias two callers' credentials). An unreachable / mis-authed host is
//! a typed `Err` that leaks no credential — the connect error is built
//! structurally, never echoing the URL or the `Secret` password. `close` is total
//! and idempotent; a dropped `Connection` drops its pool via sqlx's own `Drop`.

use super::IpeResult;
use crate::core::{IpeTask, ok_res, str_err};
use crate::dsn::{Dsn, DsnDriver};
use crate::ssrf::VettedDial;

/// A live external database connection: an independent pool of the dialect the
/// [`Dsn`] named, distinct from the app's monomorphised `Db`.
///
/// The variant is the runtime dialect; the Ipê-level read/write ACCESS mode is a
/// phantom type erased before it reaches this type, so a single `ExternalConnection`
/// serves both `Connection ReadOnly` and `Connection ReadWrite` positions.
#[derive(Clone)]
pub enum ExternalConnection {
    /// A Postgres pool dialed from a networked `Dsn`.
    Postgres(sqlx::postgres::PgPool),
    /// A Sqlite pool opened from a file-backed `Dsn`.
    Sqlite(sqlx::sqlite::SqlitePool),
}

impl std::fmt::Debug for ExternalConnection {
    /// Redact to the dialect only — a pool's own `Debug` can surface the
    /// connection options (host, user), so the struct-level impl stays
    /// conservative and prints only the driver family.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let dialect = match self {
            Self::Postgres(_) => "Postgres",
            Self::Sqlite(_) => "Sqlite",
        };
        f.debug_struct("ExternalConnection")
            .field("dialect", &dialect)
            .finish()
    }
}

/// Upper bound on connections a single external pool holds — the same
/// small-pool discipline the app connection uses, so arbitrary external
/// `open` calls cannot exhaust a foreign server's connection limit.
const EXTERNAL_POOL_MAX_CONNECTIONS: u32 = 8;

/// Build a structural, credential-free connect error. The sqlx `Display` can
/// embed the connection string (host, and on some drivers the password); this
/// funnels only the error CATEGORY into the Ipê `Error`, never the URL or a
/// credential.
fn connect_err<E: From<String>>(e: &sqlx::Error) -> E {
    let category = match e {
        sqlx::Error::Io(_) => "external connect: I/O error reaching host",
        sqlx::Error::Tls(_) => "external connect: TLS negotiation failed",
        sqlx::Error::PoolTimedOut => "external connect: pool acquisition timed out",
        sqlx::Error::Configuration(_) => "external connect: invalid connection configuration",
        _ => "external connect: connection failed",
    };
    str_err(category)
}

/// Open an external connection from a parsed, validated [`Dsn`]. The `Dsn` is a
/// proof the descriptor passed every fail-closed check (known driver, present
/// host for a network driver, secure TLS posture, no control-character
/// components); this step performs the network/file act of dialing it.
///
/// Fail-closed: an unreachable host, a refused connection, a bad password, or a
/// TLS failure all surface as a typed `Err` that carries no credential. The pool
/// is independent (never a shared URL-keyed cache) and bounded.
async fn open_external<E: Send + From<String> + 'static>(
    dsn: Dsn,
) -> IpeResult<E, ExternalConnection> {
    let driver = dsn.driver();
    let url = dsn.connection_url();
    match driver {
        DsnDriver::Postgres => {
            // The `Dsn` parse boundary enforces syntax and TLS; the connect step
            // owns the SSRF host policy. Gate the host:port before dialing so a
            // `postgres://169.254.169.254/db` or loopback DSN is denied here, not
            // allowed to reach the network driver.
            let host = dsn.host().to_owned();
            let port = dsn.port();
            if let Err(e) = VettedDial::for_host(&host, port) {
                return IpeResult::Err(str_err(&format!("external connect: {e}")));
            }
            match sqlx::postgres::PgPoolOptions::new()
                .max_connections(EXTERNAL_POOL_MAX_CONNECTIONS)
                .connect(&url)
                .await
            {
                Ok(pool) => ok_res(ExternalConnection::Postgres(pool)),
                Err(e) => IpeResult::Err(connect_err(&e)),
            }
        }
        DsnDriver::Sqlite => {
            match sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(EXTERNAL_POOL_MAX_CONNECTIONS)
                .connect(&url)
                .await
            {
                Ok(pool) => ok_res(ExternalConnection::Sqlite(pool)),
                Err(e) => IpeResult::Err(connect_err(&e)),
            }
        }
    }
}

/// `Ipe.Db.open : Dsn -> Task Error (Connection ReadOnly)` — the SAFE connector.
/// The `Dsn` arrived through a validating parse, so no unchecked string reaches
/// the connector. Discloses `network` (the enforceable egress axis).
#[must_use]
pub fn db_conn_open<E: Send + From<String> + 'static>(dsn: Dsn) -> IpeTask<E, ExternalConnection> {
    Box::pin(open_external(dsn))
}

/// `Ipe.Db.close : Connection mode -> Task Error ()` — return the pool. Total and
/// idempotent: closing releases the sqlx pool; a value dropped without `close`
/// still drops its pool via sqlx's `Drop`. Never panics.
#[must_use]
pub fn db_conn_close<E: Send + From<String> + 'static>(conn: ExternalConnection) -> IpeTask<E, ()> {
    Box::pin(async move {
        match conn {
            ExternalConnection::Postgres(pool) => pool.close().await,
            ExternalConnection::Sqlite(pool) => pool.close().await,
        }
        ok_res(())
    })
}

/// `Ipe.Db.Unsafe.unsafeExecRawOn : Connection ReadWrite -> String -> Task Error Int`
/// — run verbatim, caller-authored SQL against an external connection. Requires a
/// `Connection ReadWrite` at the type level, so a read-only connection from `open`
/// cannot reach it (that is a compile error, not a runtime check). Discloses
/// `unsafe` (raw SQL) by its `Ipe.Db.Unsafe` home; returns the rows-affected
/// count.
#[must_use]
pub fn db_conn_unsafe_exec_raw_on<E: Send + From<String> + 'static>(
    conn: ExternalConnection,
    sql: String,
) -> IpeTask<E, i64> {
    Box::pin(async move {
        // Each dialect's `execute` yields a distinct `QueryResult`; reduce each to
        // the shared `u64` rows-affected in-arm so the match unifies on one type.
        let affected: Result<u64, sqlx::Error> = match &conn {
            ExternalConnection::Postgres(pool) => sqlx::query(&sql)
                .execute(pool)
                .await
                .map(|d| d.rows_affected()),
            ExternalConnection::Sqlite(pool) => sqlx::query(&sql)
                .execute(pool)
                .await
                .map(|d| d.rows_affected()),
        };
        match affected {
            // A real affected-row count never exceeds `i64::MAX`; an out-of-range
            // value clamps rather than wrapping.
            Ok(rows) => ok_res(i64::try_from(rows).unwrap_or(i64::MAX)),
            Err(e) => IpeResult::Err(str_err(&format!(
                "external exec: {}",
                match &e {
                    sqlx::Error::Database(_) => "database error",
                    _ => "execution failed",
                }
            ))),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsn::dsn_parse;

    /// Parse a Postgres DSN and check the SSRF gate that `open_external` would
    /// apply — without attempting any real network dial.  Mirrors the guard
    /// logic inserted before `PgPoolOptions::connect`.
    fn pg_ssrf_blocked(dsn_str: &str) -> bool {
        match dsn_parse::<String>(dsn_str.to_string()) {
            IpeResult::Ok(dsn) if dsn.driver() == DsnDriver::Postgres => {
                VettedDial::for_host(dsn.host(), dsn.port()).is_err()
            }
            _ => false,
        }
    }

    #[test]
    fn open_external_ssrf_blocks_loopback_postgres_when_deny_private_on() {
        unsafe { std::env::set_var("IPE_HTTP_DENY_PRIVATE", "1") };
        assert!(
            pg_ssrf_blocked("postgres://127.0.0.1:5432/db"),
            "loopback Postgres DSN must be blocked by the SSRF gate"
        );
        unsafe { std::env::remove_var("IPE_HTTP_DENY_PRIVATE") };
    }

    #[test]
    fn open_external_ssrf_blocks_link_local_postgres_when_deny_private_on() {
        unsafe { std::env::set_var("IPE_HTTP_DENY_PRIVATE", "1") };
        assert!(
            pg_ssrf_blocked("postgres://169.254.169.254:5432/db"),
            "link-local Postgres DSN must be blocked by the SSRF gate"
        );
        unsafe { std::env::remove_var("IPE_HTTP_DENY_PRIVATE") };
    }

    #[test]
    fn open_external_ssrf_error_is_not_a_connect_or_timeout_error_for_loopback() {
        unsafe { std::env::set_var("IPE_HTTP_DENY_PRIVATE", "1") };
        let dsn = match dsn_parse::<String>("postgres://127.0.0.1:5432/db".to_string()) {
            IpeResult::Ok(d) => d,
            IpeResult::Err(e) => panic!("DSN parse failed: {e}"),
        };
        let err =
            VettedDial::for_host(dsn.host(), dsn.port()).expect_err("loopback must be blocked");
        assert!(
            err.contains("blocked"),
            "SSRF block must identify as 'blocked', not a connect/TLS error: {err}"
        );
        unsafe { std::env::remove_var("IPE_HTTP_DENY_PRIVATE") };
    }

    #[test]
    fn open_external_sqlite_dsn_bypasses_ssrf_gate() {
        unsafe { std::env::set_var("IPE_HTTP_DENY_PRIVATE", "1") };
        // A SQLite DSN has no host — the gate is not applied in `open_external`.
        // Verify the driver discriminant correctly skips the gate path.
        let dsn = match dsn_parse::<String>("sqlite://data/app.db".to_string()) {
            IpeResult::Ok(d) => d,
            IpeResult::Err(e) => panic!("SQLite DSN parse failed: {e}"),
        };
        assert_eq!(dsn.driver(), DsnDriver::Sqlite);
        // SQLite host is empty; VettedDial is only called for Postgres — gate not applied.
        assert!(dsn.host().is_empty(), "sqlite DSN must have empty host");
        unsafe { std::env::remove_var("IPE_HTTP_DENY_PRIVATE") };
    }

    #[test]
    fn open_external_ssrf_passes_private_when_deny_private_off() {
        unsafe { std::env::set_var("IPE_HTTP_DENY_PRIVATE", "0") };
        assert!(
            !pg_ssrf_blocked("postgres://127.0.0.1:5432/db"),
            "guard off must not block private host (dev workflow)"
        );
        unsafe { std::env::remove_var("IPE_HTTP_DENY_PRIVATE") };
    }
}

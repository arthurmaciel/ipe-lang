Status: Accepted
Date: 2026-07-10

# 0013. Multi-driver DB: compile-time template selection, typed NULL binds, hub-local tenant gate

## Context

`[database] driver` in `sky.toml` was parsed but never consumed: the generated
Rust `DbPool`/`DbRow` types were sqlite-hardcoded, so a `postgres://…` URL
failed at runtime with type-mismatch errors. Postgres is strongly typed (sqlx
sends a type OID per bound parameter), which surfaces two further gaps that only
manifest once a Postgres path exists. Separately, the Go reference scopes the
telemetry-console spill DB to an authenticated tenant prefix; the Rust runtime
had no tenant isolation anywhere.

## Decision

- **Compile-time driver selection, not runtime dispatch.** Thread the parsed
  `DbDriver` enum (Sqlite | Postgres) from manifest parsing into codegen, then
  `include_str!`-select between two config templates — `config.rs` (sqlite) and
  a new `config_postgres.rs` — that export *identical* symbol names
  (`DbPool`, `DbRow`, `sky_db_url()`, `db_last_insert_id()`, `db_format_sql()`,
  `DB_USES_RETURNING_ID`) with driver-specific bodies. `db.rs` never branches on
  driver, so it compiles once per project. Rejected: runtime reflection /
  feature-flag negotiation — either dynamic dispatch (wrong for a hot data
  layer) or per-driver compilation of every generated project (build-time
  blowup). The driver is frozen per binary; there is no runtime negotiation.

- **Typed `SqlNull` carries a type witness.** The runtime `SqlParam::SqlNull`
  was a bare unit, discarding the Sky-side `SqlNull SqlValue` witness. Add
  `Null(Box<SqlParam>)` carrying a witness — not for a value (NULL has none) but
  for its variant tag, which selects the typed `Option::<T>::None` to bind so
  Postgres's per-parameter type OID matches the target column. Rejected: leaving
  NULL untyped and "handling it later" — the bug is already representable and
  gated on Postgres landing *this* release.

- **Tenant scope is a hub-specific concern, not a generic `Db.*` feature.**
  Build it as task-local scope (`tokio::task_local! TENANT_PREFIX`) mirroring
  Go's `tenantPrefixForSession`/`rejectCrossTenantSvc`, wired into hub's four
  `read_*` functions as an optional `AND service_name LIKE ?` clause. Go itself
  never applies this gate to plain `Db.*`; user-facing data access has no tenant
  concern.

## Consequences

- **Invariants that must keep holding:** Postgres projects use
  `DB_USES_RETURNING_ID = true` (no `last_insert_rowid`); every NULL bind's type
  must match the target column at prepare time (no untyped-NULL fallback on
  Postgres — degenerate `Null(Null(..))` falls back to TEXT rather than
  panicking). When a tenant prefix is in scope, every `hub_read_filtered_*` call
  must short-circuit to enforce the gate *before* SQL is built, never filter
  after the fact; `escape_like_prefix` must strip `%`/`_` so a tenant name like
  `"%"` cannot match all services.
- The `db_format_sql` seam (`?` → `$N` rewrite) already existed; no new call
  sites. The gate is testable via `with_tenant_prefix(…)` without live-session
  identity plumbing.

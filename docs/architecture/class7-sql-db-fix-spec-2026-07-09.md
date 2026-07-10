# Class 7 fix spec — SQL/DB runtime correctness & security

> Implementation spec for campaign-classification Class 7 (`campaign-classification-2026-07-09.md`):
> `url_is_cacheable` DoS reopen, `SqlNull`/Postgres typing, Postgres driver
> reachability, `db_insert_row` fabricated id, tenant-prefix SQL gap, and #34
> (`db_decode_money` kernel wiring). All line numbers below verified against
> the working tree at HEAD (master, `docs/architecture/progdev-metrics-oldengine.txt`
> untracked, `scripts/progressive-development/autopilot.sh` locally modified —
> neither touches the files below).
>
> Scope discipline: this spec does NOT touch `SqlFragment` (Class 6, same
> file, different lines — see `class6-secret-sqlfragment-fix-spec-2026-07-09.md`).
> Where a fix below lands in the same function `SqlFragment`'s consumers will
> later call (`db_format_sql`, `bind_sql_param`), the change is additive
> (new match arms / new driver branch) so Class 6's landing order is
> unaffected either way.

## Corrections to the backlog's framing (read this first)

Two of the five backlog bullets undersell the actual gap. Confirmed by
direct inspection, not assumption:

1. **The tenant-prefix gap is NOT "present in `hub.rs`, absent from `db.rs`."
   It is absent from BOTH.** `rg -n "tenant" runtime/src/ crates/` returns
   zero hits anywhere in this repository. `hub_current_identity`
   (`runtime/src/sky_runtime/live/hub.rs:513-519`) is hardcoded to always
   return the empty identity (`{"subject":"","email":"","claims":{}}`) with
   a comment stating plainly: *"The spill-only console has no session
   identity (that's A-territory / live-session plumbing)."* There is no
   `HubStoreReaderWithTenant` equivalent, no `rejectCrossTenantSvc`
   equivalent, no per-session identity task-local. `Hub_readFiltered{Logs,
   Metrics,Traces,Errors}` (`hub.rs:206-217,399-450` area) take a bare
   `service: String` with zero tenant scoping — any caller can pass any
   `service` string and read any tenant's rows out of the shared
   `telemetry_*` SQLite spill. §5 below builds this from scratch, using the
   Go reference (`../sky/runtime-go/rt/hub_bridge.go`,
   `../sky/runtime-go/rt/hub/store.go`) as the exact design template — it is
   the correct scope (a hub/console read-path concern, not a generic
   `Db.*` concern; Go never applies this gate to plain `Db.*` either).

2. **The `db_insert_row` fabricated-id bug is currently DEAD CODE, not a
   live bug.** `DB_USES_RETURNING_ID` is a `pub const … = false` in
   `runtime/src/sky_runtime/config.rs:41`, and `config.rs` is never
   overridden per driver anywhere in the codegen (see §3) — so
   `if DB_USES_RETURNING_ID { … }` in `db_insert_row` (`db.rs:1184`) never
   executes in any build that exists today. The bug is real (the logic is
   wrong) but **untestable end-to-end until §3 (Postgres reachability)
   lands** — confirming the backlog's own flag-ordering instinct, and
   extending it: Postgres-reachability gates BOTH the SqlNull-Postgres fix
   AND the fabricated-id fix's end-to-end regression, not just the former.
   §4 below still specifies a fix + an *isolated* unit test that does not
   need to wait (see §4's "Two-tier test strategy").

## Ordering

**Land in this order** (each step either is a hard prerequisite for the
next test to be meaningful, or is fully independent and can be reordered
freely — noted per item):

1. **§2 `url_is_cacheable`** — fully independent, zero-risk, do it first as
   a warm-up.
2. **§3 Postgres driver reachability** — do this BEFORE §4's SqlNull fix
   and BEFORE §4's end-to-end fabricated-id regression. Neither is
   meaningfully testable against a real non-sqlite driver until this
   lands. (The isolated fabricated-id unit test in §4 does not depend on
   this and may land anytime.)
3. **§4 SqlNull typed-NULL fix** — depends on §3 for its Postgres
   regression test; the Rust-side type change itself can be written
   beforehand but the test proving it matters needs a live Postgres.
4. **§4 `db_insert_row` / `db_insert_fields` fabricated-id fix** — logic
   fix + isolated unit test independent of §3; full end-to-end regression
   (`DB_USES_RETURNING_ID = true` path) depends on §3.
5. **§5 tenant-prefix gate** — fully independent of §2–§4 (different file,
   different subsystem). Can run in parallel with §2–§4.
6. **§6 #34 `Db.Decode.money` kernel wiring** — fully independent of
   §2–§5. Can run in parallel.

Given the independence, a reasonable execution plan is: land §2 solo
first (2 minutes of risk-free work), then run §3→§4 sequentially on one
lane while §5 and §6 run in parallel on two other lanes (three files never
overlap: `config.rs`+`db.rs`+`project.rs`+`skyc/project.rs` for §3/§4 vs
`hub.rs` for §5 vs `sky_kernels`/`sky_types`/`sky_lower`/`sky_backend_rust`/
`sky_canon` for §6 — §6 touches many files but none the same lines as §3/§4
touch in those shared crates, since §6 only appends new `DbDecMoney` match
arms next to the untouched `DbDecBool` arms).

---

## §2 — `url_is_cacheable` substring DoS reopen

**File:** `runtime/src/sky_runtime/db.rs:589-591`

```rust
fn url_is_cacheable(url: &str) -> bool {
    !url.contains("memory")
}
```

**Bug:** A substring match on the literal `"memory"` anywhere in the URL.
Any file-based SQLite path that happens to contain the substring
`"memory"` — e.g. `sqlite://data/memory_bank.db`, a filename a user
legitimately chose — is misclassified as an in-memory database and
excluded from pooling. This is a correctness bug dressed as a security
one: excluding a cacheable file DB from the pool cache doesn't leak
data, but it silently disables the connection-pool reuse + WAL-pragma
setup (`build_pool`, `db.rs:618-640`) for that URL, and — the actual
"DoS reopen" — a malicious or careless caller can force a NEW pool to be
built on every single `Db.connect` call by choosing a URL containing
`"memory"` as a path segment, exhausting the process's file-descriptor /
connection budget (each `build_pool` call opens fresh connections; the
existing `max_db_pools`/`max_pool_connections` caps in
`connect_cached` (`db.rs:642-673`) bound the CACHED-pool count, but an
uncacheable URL takes the `else` branch at `db.rs:667-669`, which builds
and returns a brand-new, ungoverned pool on every single call — no cap
applies to it at all).

**Fix:** Replace the substring test with the same test SQLite itself
uses to distinguish a true in-memory database: the URL's authority/path
component must be exactly `:memory:` (optionally with an `sqlite:`/
`sqlite://` scheme prefix) — i.e. the special string SQLite's C API
treats as "private, non-shared in-memory database" — OR the URI contains
the `mode=memory` query parameter (SQLite's documented URI-mode memory
flag, e.g. `file:foo.db?mode=memory&cache=shared` — note this specific
example uses `cache=shared`, which SHOULD stay cacheable; the true
"never share" case is unqualified `mode=memory` without `cache=shared`).

```rust
/// `:memory:` SQLite URLs (bare, or with a scheme prefix like `sqlite://` or
/// `sqlite:`) and URI-mode `mode=memory` WITHOUT `cache=shared` must NOT be
/// pooled: each connection is a DISTINCT in-memory database, so sharing a
/// pool would silently merge what callers expect to be isolated DBs
/// (soundness). Matching on the exact SQLite special-string / query
/// parameter — not a raw substring match on "memory" anywhere in the URL —
/// so a legitimate file path like `sqlite://data/memory_bank.db` is
/// correctly treated as cacheable.
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

    if path == ":memory:" {
        return false;
    }
    if query.split('&').any(|kv| kv == "mode=memory")
        && !query.split('&').any(|kv| kv.starts_with("cache=shared"))
    {
        return false;
    }
    true
}
```

**Note on `build_pool`'s companion check** (`db.rs:633`:
`if url.contains("sqlite") && url_is_cacheable(url)`) — that check gates
whether to run the `PRAGMA journal_mode=WAL` / `busy_timeout` statements,
which are sqlite-only and harmless if skipped; it stays a coarse
`contains("sqlite")` check since it is purely an optimisation gate (a
false-negative there just skips a beneficial PRAGMA, no soundness or DoS
implication) — do not "fix" that call site as part of this item; only
`url_is_cacheable`'s definition changes.

**Verification:**
```bash
cd /home/arthur/Documentos/comp/sky-rust
cargo test -p sky-runtime-rust --features full url_is_cacheable
```

**Regression tests** (add to `runtime/src/sky_runtime/db.rs`'s existing
`#[cfg(test)] mod tests` block, `db.rs:2040+`):

```rust
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
```

---

## §3 — Postgres driver structural unreachability

**Files:**
- `runtime/src/sky_runtime/config.rs` (whole file, 49 lines) — the runtime
  template.
- `crates/skyc/src/project.rs:38-47` (`ProjectManifest` struct) — manifest
  parsing, currently has no `[database]` section at all.
- `crates/sky_backend_rust/src/lib.rs:128-199,535-597` (`EmitCtx`
  construction) — currently only tracks `uses_db: bool`, no driver enum.
- `crates/sky_backend_rust/src/project.rs:278-284,415-475` — where
  `RUNTIME_CONFIG_RS_DB` (`include_str!` of the runtime's `config.rs`,
  verbatim, unconditionally) is selected and written to
  `src/sky_runtime/config.rs` in the emitted project.

**Confirmed root cause:** `config.rs`'s own doc comment
(`db.rs:2-3`: *"Uses DbPool, DbRow, sky_db_url, db_last_insert_id,
db_format_sql from config.rs (generated at build time per sky.toml
[database] driver)"*) states the INTENT, but no code implements it:

1. `crates/skyc/src/project.rs`'s `parse_manifest` (line 91) only
   recognises `[project]`/`name` and `[source]`/`root` — any other
   section (including a `[database]` section) falls into the `_ => {}`
   catch-all at line 131 and is silently discarded. There is no
   `ProjectManifest.database_driver` field.
2. `crates/sky_backend_rust/src/lib.rs`'s `EmitCtx` (line 128) has
   `uses_db: bool` (line 143) but no driver discriminant.
3. `crates/sky_backend_rust/src/project.rs:284` —
   `const RUNTIME_CONFIG_RS_DB: &str = include_str!("../../../runtime/src/sky_runtime/config.rs");`
   — this is a compile-time, unconditional, single-file embed. Every
   db-enabled generated project gets the EXACT sqlite-hardcoded
   `config.rs` regardless of what `sky.toml` says, because nothing ever
   reads a driver choice to select a different template.

Net effect: `[database] driver = "postgres"` in `sky.toml` (the schema
CLAUDE.md documents and users would reasonably write) is currently a
silent no-op. The Cargo.toml sqlx-feature wiring (`crates/sky_backend_rust/src/project.rs:733-748,809`)
DOES correctly add the `"postgres"` sqlx feature when the manifest
requests it (already correct, already tested —
`live_db_toml_includes_postgres`, `project.rs:1226`) — but the actual
Rust *types* (`DbPool`/`DbRow`) never follow; a project with the
postgres Cargo feature enabled still gets
`type DbPool = sqlx::sqlite::SqlitePool;` and will not even attempt to
connect to a Postgres URL, since `sky_db_url()`'s fallback default is a
sqlite URL and — more fundamentally — the pool type constructed by
`sqlx::pool::PoolOptions::<Sqlite>::new().connect(url)` in
`build_pool` (`db.rs:624-632`, generic over `Db = DbPool`) will fail to
parse a `postgres://…` URL against a `Sqlite`-typed pool at all.

### Fix — 4 parts

**Part A — parse `[database]` in the manifest.**
`crates/skyc/src/project.rs`:

```rust
/// Supported `[database] driver` values. Unrecognised / absent → `Sqlite`
/// (matches the documented default in CLAUDE.md's `sky.toml` schema table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbDriver {
    Sqlite,
    Postgres,
}

impl DbDriver {
    fn from_str_or_default(s: &str) -> Result<DbDriver, CliError> {
        match s {
            "sqlite" => Ok(DbDriver::Sqlite),
            "postgres" | "postgresql" => Ok(DbDriver::Postgres),
            other => Err(CliError::Usage(Box::leak(format!(
                "sky.toml: [database] driver = {other:?} is not supported \
                 (expected \"sqlite\" or \"postgres\")"
            ).into_boxed_str()))),
        }
    }
}
```
(Adjust `CliError::Usage`'s exact payload type to match its existing
definition — check whether it takes `&'static str` today; if so, prefer
changing `CliError::Usage` to carry an owned `String` rather than
leaking memory. Grep `enum CliError` in `crates/skyc/src/lib.rs` before
implementing — this spec assumes `&'static str` based on the existing
`Err(CliError::Usage("sky.toml: missing a \`name = \"…\"\` entry"))`
call sites at `project.rs:135,140`; leaking is NOT an acceptable
long-term pattern for a value that echoes user input — use an owned
variant.)

Add `pub driver: DbDriver` to `ProjectManifest` (`project.rs:40-47`),
defaulting to `DbDriver::Sqlite`, and extend the parse loop
(`project.rs:104-133`) with a `"[database]"` section and a `"driver"`
key:

```rust
section = if line == "[project]" {
    "[project]"
} else if line == "[source]" {
    "[source]"
} else if line == "[database]" {
    "[database]"
} else {
    "other"
};
// ...
let mut driver_str: Option<String> = None;
// ... in the match:
("[database]", "driver") => driver_str = Some(val.to_owned()),
// ... after the loop:
let driver = match driver_str {
    Some(s) => DbDriver::from_str_or_default(&s)?,
    None => DbDriver::Sqlite,
};
```

**Part B — thread the driver into `EmitCtx`.**
`crates/sky_backend_rust/src/lib.rs`: add `pub(crate) db_driver: DbDriver`
next to `uses_db: bool` (line 143); populate it at `EmitCtx::build`
(around line 535-597) from whatever already carries the parsed
`ProjectManifest` down to this layer (trace the call chain from
`skyc::build_project` — the manifest is parsed in `skyc`, `EmitCtx` is
built in `sky_backend_rust`; the manifest (or just its new `driver`
field) needs to cross that boundary as a new parameter/field on
whatever context type `sky_backend_rust::emit_program` already receives
from `skyc`). Re-export `DbDriver` from `sky_backend_rust` (or take a
lighter enum local to `sky_backend_rust` and convert at the boundary —
either is fine; prefer NOT duplicating the enum if a shared crate both
already depend on is available, else duplicate + convert rather than
introducing a new crate dependency edge).

**Part C — two `config.rs` templates, selected by driver.**
`crates/sky_backend_rust/src/project.rs`:

- Keep `RUNTIME_CONFIG_RS_DB` (line 284) as the sqlite variant (rename to
  `RUNTIME_CONFIG_RS_DB_SQLITE` for clarity, still `include_str!` of
  `runtime/src/sky_runtime/config.rs` verbatim — no changes to that file
  needed).
- Add a NEW postgres-flavored template file at
  `runtime/src/sky_runtime/config_postgres.rs` (a sibling file in the
  runtime crate, NOT compiled into the standalone `sky-runtime-rust`
  crate's default build — it exists purely as an `include_str!` source
  for codegen, mirroring how `config.rs` itself is dual-purpose today:
  compiled directly in the standalone crate, AND `include_str!`'d as a
  text template by the backend):

```rust
// runtime/src/sky_runtime/config_postgres.rs
// Postgres config.rs template — emitted verbatim by sky_backend_rust's
// project.rs when `sky.toml`'s `[database] driver = "postgres"`.
// Mirrors config.rs's sqlite shape; every symbol name matches so db.rs
// (which is identical across both driver builds) never needs a
// driver-specific `#[cfg]`.

#[cfg(feature = "db")]
pub type DbPool = sqlx::postgres::PgPool;
#[cfg(feature = "db")]
pub type DbRow = sqlx::postgres::PgRow;
#[cfg(feature = "db")]
pub fn sky_db_url() -> String {
    crate::sky_runtime::system::read_env_var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres@localhost/sky".to_string())
}

#[cfg(not(feature = "db"))]
pub type DbPool = ();
#[cfg(not(feature = "db"))]
pub type DbRow = ();
#[cfg(not(feature = "db"))]
pub fn sky_db_url() -> String {
    String::new()
}

#[cfg(feature = "db")]
pub fn db_last_insert_id(_res: &sqlx::postgres::PgQueryResult) -> i64 {
    // Postgres has no LastInsertId; DB_USES_RETURNING_ID = true routes
    // every insert through `db_insert_row`'s `RETURNING id` branch
    // instead, so this fn is never called on the success path for
    // `db_insert_row`. It IS still reachable from `db_insert_fields` /
    // `db_update_fields` (see §4's companion fix) until that fix lands —
    // returning 0 there is the pre-existing (if imprecise) sqlite
    // behaviour for a table with no autoincrement column, not a new
    // regression.
    0
}

#[cfg(feature = "db")]
pub fn db_format_sql(sql: String) -> String {
    // Postgres's extended query protocol uses numbered placeholders
    // ($1, $2, …), not sqlite's positional `?`. Every SQL string built
    // throughout db.rs is written with `?` placeholders in emission
    // order, matching the order args are bound — a straight sequential
    // rewrite (first `?` → $1, second → $2, …) is therefore correct
    // REGARDLESS of which `db.rs` function built the string, with one
    // caveat: a literal `?` inside a *quoted string literal* in the SQL
    // text would be mis-rewritten. db.rs never emits a caller-controlled
    // `?` inside a quoted literal (every value is bound as a parameter,
    // never inlined as a literal), so a naive scan is safe here — but
    // implementers MUST re-verify this invariant if any future db.rs
    // change starts inlining string literals containing `?` into SQL
    // text (e.g. a LIKE pattern build helper).
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

#[cfg(feature = "db")]
pub const DB_USES_RETURNING_ID: bool = true;

#[cfg(feature = "db")]
pub fn db_auto_id_column() -> &'static str {
    "id BIGSERIAL PRIMARY KEY"
}
```

Then in `crates/sky_backend_rust/src/project.rs` (near line 415-475,
where `runtime_config_rs` is selected):

```rust
let (cargo_toml, runtime_config_rs) = if ctx.uses_db {
    let cfg = match ctx.db_driver {
        DbDriver::Sqlite => RUNTIME_CONFIG_RS_DB_SQLITE,
        DbDriver::Postgres => RUNTIME_CONFIG_RS_DB_POSTGRES,
    };
    (db_cargo_toml(...)?, cfg)
} else {
    (CARGO_TOML, RUNTIME_CONFIG_RS)
};
```
(`RUNTIME_CONFIG_RS_DB_POSTGRES` is the new
`include_str!("../../../runtime/src/sky_runtime/config_postgres.rs")`
constant, added next to `RUNTIME_CONFIG_RS_DB_SQLITE`.)

**Part D — `db_format_sql` call-site audit.** Every existing `db.rs` call
site already routes its final SQL string through `db_format_sql(...)`
before executing (`db_get_by_id`, `db_update_by_id`, `db_delete_by_id`,
`db_find_one_by_field`, `db_find_many_by_field`, `db_insert_row`,
`db_insert_fields`, `db_insert_fields_returning`, and the generic
`db_exec`/`db_query` family) — confirmed by grep (`rg -n
"db_format_sql\("` across `db.rs`); this fix requires ZERO new call
sites in `db.rs` itself, only the new `config_postgres.rs` template
providing the driver-specific implementation of the existing seam.
**One exception to verify at implementation time:** `Db.migrate`'s DDL
statements and `db_auto_id_column()`'s consumer (search
`db_auto_id_column` call sites in `db.rs`'s migrate path) — confirm the
DDL string is ALSO passed through `db_format_sql` (for the `?`→`$N`
rewrite to matter here it would need bound params, which DDL generally
doesn't have, but the placeholder-rewrite is a no-op on a `?`-free
string regardless, so this is a low-risk verification, not a functional
gap).

### Verification

```bash
cd /home/arthur/Documentos/comp/sky-rust
cargo build -p sky_backend_rust -p skyc
cargo test -p skyc -p sky_backend_rust
```

A true end-to-end Postgres smoke test needs a running Postgres — add a
`#[ignore]`-gated integration test (run manually / in a CI job with a
Postgres service container) rather than requiring one for the default
`cargo test` gate:

```rust
// crates/skyc/tests/postgres_driver_reachability.rs (new file)
#[test]
fn manifest_parses_postgres_driver() {
    // [database] driver = "postgres" → ProjectManifest.driver == Postgres,
    // no [database] section → defaults to Sqlite. Pure parse-level test,
    // no DB connection needed — runs in the default gate.
}

#[test]
#[ignore = "requires POSTGRES_TEST_URL — run manually or in the postgres CI job"]
fn end_to_end_postgres_build_and_insert() {
    // Requires: docker/CI postgres service, POSTGRES_TEST_URL env var.
    // 1. skyc build a fixture project with [database] driver = "postgres".
    // 2. Confirm the emitted config.rs contains `PgPool`/`PgRow` and
    //    `DB_USES_RETURNING_ID = true`.
    // 3. Run the built binary against POSTGRES_TEST_URL; Db.insertRow /
    //    Db.getById round-trip.
}
```

### Regression tests (mandatory, no Postgres needed)

- `crates/skyc/src/project.rs` — `parse_manifest` unit tests: `[database]`
  absent → `Sqlite`; `driver = "sqlite"` → `Sqlite`; `driver = "postgres"`
  → `Postgres`; `driver = "mysql"` (or any unsupported value) →
  `CliError::Usage` naming the bad value.
- `crates/sky_backend_rust/src/project.rs` — a golden-style test asserting
  that `emit_program` on a db-enabled + `driver = "postgres"` fixture
  program's emitted `src/sky_runtime/config.rs` file contains
  `sqlx::postgres::PgPool` and `DB_USES_RETURNING_ID: bool = true`, and
  that the sqlite path (default / explicit `driver = "sqlite"`) is
  byte-identical to today's output (non-regression on the existing
  sqlite golden fixtures).

---

## §4 — `SqlNull` typed-NULL + `db_insert_row`/`db_insert_fields` fabricated id

### 4a. `SqlNull` binds as untyped NULL

**Files:** `runtime/src/sky_runtime/db.rs:1693,1707-1721,1787-1796,2255-2267`;
`crates/sky_backend_rust/src/project.rs:1053-1076`.

**Bug:** `SqlParam::Null` (`db.rs:1719-1720`) is a bare unit variant; the
generated conversion `StdDbSqlValue::SqlNull(_) => sky_runtime::db::SqlParam::Null`
(`project.rs:1076`, comment at :1056-1057 self-documents: *"SqlNull
carries a SqlValue type-witness that is discarded here"*) throws away
the witness `SqlValue` payload that Sky's `SqlNull SqlValue` constructor
carries (per CLAUDE.md's stdlib table: *"`SqlNull SqlValue` (recursive,
carries a type-witness for typed NULL binding)"*). `bind_sql_param`
(`db.rs:1787-1796`) always binds `SqlParam::Null` as
`q.bind(Option::<String>::None)` — a TEXT-typed NULL regardless of the
target column's actual type.

On SQLite this is harmless (SQLite is dynamically typed; a bound NULL is
a NULL no matter what Rust type wraps it). On Postgres it is NOT
harmless: sqlx's extended query protocol sends a type OID hint for each
bound parameter derived from the bound Rust type, and Postgres validates
that hint against the target column/expression type at prepare time —
binding `Option::<String>::None` (OID: TEXT) against an `INTEGER`,
`BOOLEAN`, `BYTEA`, or `TIMESTAMP` column fails with a Postgres type
-mismatch error (e.g. *"column ... is of type integer but expression is
of type text"*). This is why the fix is gated on §3 landing first for
its regression test to be meaningful.

**Fix:**

1. `runtime/src/sky_runtime/db.rs:1707-1721` — change the `Null` variant
   to carry the witness:

```rust
#[derive(Clone, Debug)]
pub enum SqlParam {
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Bytes(Vec<u8>),
    /// `SqlNull witness` — binds a NULL typed according to `witness`'s
    /// variant, so the driver's type-OID hint (Postgres) matches the
    /// target column. `witness`'s VALUE is never read (a NULL carries no
    /// value) — only its variant tag selects the typed `Option::<T>::None`
    /// to bind. Boxed to keep `SqlParam` itself unconditionally `Copy`-free
    /// but cheap (one variant, rarely constructed in a hot loop).
    Null(Box<SqlParam>),
}
```

2. `db.rs:1787-1796` (`bind_sql_param`) — dispatch on the witness:

```rust
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
            // A nested Null-of-Null witness is a degenerate shape that
            // should not arise from codegen (SqlValue's SqlNull wraps a
            // concrete leaf SqlValue variant, not another SqlNull) — fall
            // back to a TEXT-typed NULL rather than panicking; matches
            // the pre-fix SQLite-safe behaviour for this unreachable case.
            SqlParam::Null(_) => q.bind(Option::<String>::None),
        },
    }
}
```

3. `crates/sky_backend_rust/src/project.rs:1058-1077`
   (`emit_db_projection_impls`) — thread the witness through recursively
   instead of discarding it:

```rust
Self::SqlNull(inner) => {
    sky_runtime::db::SqlParam::Null(Box::new((*inner).into_sql_param()))
}
```
(Update the surrounding comment at :1056-1057 — it currently asserts the
witness IS discarded; that assertion becomes false after this change.)

4. Update the two direct `SqlParam::Null` test constructions at
   `db.rs:2260,2263` to `SqlParam::Null(Box::new(SqlParam::Text(String::new())))`
   (the existing test only checks the INSERT succeeds, not any
   type-specific behaviour, so any witness works — `Text` matches the
   pre-fix behaviour exactly, keeping that test's assertion unchanged).

### 4b. `db_insert_row` / `db_insert_fields` fabricated `id = 0`

**File:** `runtime/src/sky_runtime/db.rs:1184-1213` (`db_insert_row`),
`db.rs:1858-1879` (`db_insert_fields`).

**Bug (`db_insert_row`):**
```rust
Ok(r) => ok_res(
    r.try_get::<i64, _>("id")
        .or_else(|_| r.try_get::<i32, _>("id").map(|v| v as i64))
        .unwrap_or(0),
),
```
When the `RETURNING id` row's `id` column is neither `i64`- nor
`i32`-decodable (a non-integer primary key — `TEXT`, `UUID`, composite,
or a table whose PK column is simply not named `id`), this silently
returns `0` as if the insert produced id `0` — a fabricated, wrong
value indistinguishable from a genuine `id=0` row. Any caller code that
uses the returned id to look the row back up (`Db.getById conn table
(String.fromInt insertedId)`) will silently operate on the WRONG row (or
no row) with no error surfaced anywhere.

**Bug (`db_insert_fields`, `db.rs:1874-1876`):** unconditionally calls
`db_last_insert_id(&res)` regardless of `DB_USES_RETURNING_ID` — on the
sqlite driver this is correct (`last_insert_rowid()` always works); on
the Postgres `config.rs` template from §3, `db_last_insert_id` is a
stub returning `0` unconditionally (see §3's `config_postgres.rs`
comment) because Postgres's `QueryResult` (`PgQueryResult`) carries no
last-insert-id concept at all — so `Db.insertFields` on Postgres ALWAYS
returns a fabricated `0`, unconditionally, not just on a non-integer PK.
This is a companion correctness gap that must land in the SAME change as
the `db_insert_row` fix, or §3's Postgres reachability ships with a
silently-broken `Db.insertFields`.

**Fix — both functions return `Task Error Int`, so surface a real `Err`
instead of fabricating `0`:**

`db_insert_row` (`db.rs:1194-1201`):
```rust
match fetch_one_routed(&conn, q).await {
    Ok(r) => match r
        .try_get::<i64, _>("id")
        .or_else(|_| r.try_get::<i32, _>("id").map(|v| v as i64))
    {
        Ok(id) => ok_res(id),
        Err(_) => SkyResult::Err(
            "db.insertRow: inserted row's id column is not an integer \
             (non-integer or composite primary key) — cannot report an \
             Int id; use Db.insertFieldsReturning with a typed decoder \
             instead"
                .to_string()
                .into(),
        ),
    },
    Err(e) => SkyResult::Err(sky_err(&e)),
}
```

`db_insert_fields` (`db.rs:1858-1879`) — give it the same
`DB_USES_RETURNING_ID` branch `db_insert_row` already has, rather than
unconditionally calling `db_last_insert_id`:

```rust
pub fn db_insert_fields<E: Send + From<String> + 'static>(
    conn: Db,
    table: String,
    fields: Vec<(String, Option<SqlParam>)>,
) -> SkyTask<E, i64> {
    Box::pin(async move {
        let (base_sql, args) = match build_insert_sql("db.insertFields", &table, fields) {
            Ok(v) => v,
            Err(e) => return SkyResult::Err(e.into()),
        };
        if DB_USES_RETURNING_ID {
            let sql = db_format_sql(format!("{} RETURNING id", base_sql));
            let mut q = sqlx::query(&sql);
            for p in args {
                q = bind_sql_param(q, p);
            }
            match fetch_one_routed(&conn, q).await {
                Ok(r) => match r
                    .try_get::<i64, _>("id")
                    .or_else(|_| r.try_get::<i32, _>("id").map(|v| v as i64))
                {
                    Ok(id) => ok_res(id),
                    Err(_) => SkyResult::Err(
                        "db.insertFields: inserted row's id column is not \
                         an integer — cannot report an Int id"
                            .to_string()
                            .into(),
                    ),
                },
                Err(e) => SkyResult::Err(sky_err(&e)),
            }
        } else {
            let sql = db_format_sql(base_sql);
            let mut q = sqlx::query(&sql);
            for p in args {
                q = bind_sql_param(q, p);
            }
            match exec_routed(&conn, q).await {
                Ok(res) => ok_res(db_last_insert_id(&res)),
                Err(e) => SkyResult::Err(sky_err(&e)),
            }
        }
    })
}
```

Note the all-OmitField / `INSERT … DEFAULT VALUES` shape
(`build_insert_sql`, `db.rs:1832-1834`) is unaffected — `RETURNING id`
appends cleanly to `DEFAULT VALUES` on both drivers.

### Two-tier test strategy (per the ordering note in the corrections section)

**Tier 1 — lands now, no Postgres needed.** Refactor the id-extraction
logic into a small pure helper so it is unit-testable against a REAL
SQLite row with a non-integer `id` column (SQLite's dynamic typing lets
a column declared `TEXT` hold a non-numeric value even when named
`id`, and `RETURNING id` on such a row exercises the exact decode-miss
path without needing `DB_USES_RETURNING_ID` to be true — call the
helper directly, bypassing the `if DB_USES_RETURNING_ID` gate):

```rust
/// Extract an Int id from a `RETURNING id` row; `Err` (never a fabricated
/// 0) when the column isn't i64- or i32-decodable.
fn extract_returning_id(r: &DbRow) -> Result<i64, String> {
    r.try_get::<i64, _>("id")
        .or_else(|_| r.try_get::<i32, _>("id").map(|v| v as i64))
        .map_err(|_| "id column is not an integer".to_string())
}
```

```rust
#[tokio::test]
async fn extract_returning_id_errs_on_non_integer_pk() {
    let db = fresh_db().await;
    db_exec_raw::<String>(db.clone(), "CREATE TABLE t (id TEXT PRIMARY KEY)".to_string())
        .await;
    let row = fetch_one_routed(
        &db,
        sqlx::query("INSERT INTO t (id) VALUES ('non-integer-pk') RETURNING id"),
    )
    .await
    .expect("insert should succeed");
    assert!(extract_returning_id(&row).is_err());
}

#[tokio::test]
async fn extract_returning_id_ok_on_integer_pk() {
    let db = fresh_db().await;
    db_exec_raw::<String>(db.clone(), "CREATE TABLE t (id INTEGER PRIMARY KEY)".to_string())
        .await;
    let row = fetch_one_routed(&db, sqlx::query("INSERT INTO t (id) VALUES (42) RETURNING id"))
        .await
        .expect("insert should succeed");
    assert_eq!(extract_returning_id(&row), Ok(42));
}
```

**Tier 2 — lands after §3.** An `#[ignore]`-gated end-to-end test (same
Postgres CI job as §3's) that builds a project with a non-integer PK
table (`CREATE TABLE t (id TEXT PRIMARY KEY)`), calls `Db.insertRow`
with `DB_USES_RETURNING_ID = true` (i.e. against the real Postgres
config), and asserts the returned `Task` resolves to `Err`, never a
fabricated `Ok(0)`.

---

## §5 — Tenant-prefix SQL enforcement (build from scratch)

**File:** `runtime/src/sky_runtime/live/hub.rs`.

### Scope correction (see top-of-doc note)

This gate does not exist anywhere in this repository today. Build it to
match the Go reference exactly:
`../sky/runtime-go/rt/hub_bridge.go:110-135` (`HubStoreReaderWithTenant`
interface doc + `tenantPrefixForSession`/`rejectCrossTenantSvc` at
:531-572), `../sky/runtime-go/rt/hub/store.go:434-548`
(`LogFilter.TenantPrefix` / `escapeLikePrefix` SQL-layer scoping), and
`../sky/runtime-go/rt/hub_tenant_test.go` (the exact test-case table to
port).

### Real blocker found during research — state this explicitly to whoever implements

The Go gate's PRODUCER side (deriving a tenant prefix from
`id.Claims["tenant"]` on the current live session's identity) depends on
`SKY_CONSOLE_AUTH=app` (the row-poly `consoleAuth` callback that mints a
per-session `Identity` with `claims`). That mode is **not yet
implemented** in this Rust runtime —
`runtime/src/sky_runtime/live/console.rs:192-206` has the `ConsoleAuthMode::App`
arm explicitly stubbed: *"SKY_CONSOLE_AUTH=app (row-poly consoleAuth
callback) is not yet implemented."* `hub_current_identity`
(`hub.rs:513-519`) is hardcoded empty for the same reason.

**Consequence:** this spec builds the full CONSUMER side (SQL-layer
`LIKE`-prefix scoping + the `reject_cross_tenant_svc` kernel gate +
the task-local plumbing to CARRY a tenant prefix through a request) —
fully testable in isolation exactly like Go's own unit tests (which use
a hand-constructed `liveSession` with a `Claims` map, not a live
`consoleAuth` callback). It does NOT attempt to wire the PRODUCER side
(deriving the prefix from an actual authenticated session), because that
depends on unimplemented upstream machinery already tracked separately
(the `SKY_CONSOLE_AUTH=app` mode). **This is not a scope-narrowing
workaround** — the consumer-side gate is fully real, fully enforced, and
callable/testable via the same `with_tenant_prefix(...)` scoping
mechanism Go's tests use via `runWithLiveSession`; only the "how does the
prefix get INTO the task-local in a real running app" question is
deferred, and it is deferred onto an ALREADY-TRACKED, ALREADY-NAMED gap
(console-auth app-mode), not a newly-invented deferral. File a follow-up
backlog line explicitly cross-referencing both items so this isn't lost
(see "Follow-up" at the end of this section).

### Design (mirrors Go 1:1)

**1. Task-local tenant-prefix scope**, reusing the exact
`tokio::task_local!` pattern already established in
`runtime/src/sky_runtime/live/pubsub.rs:203-219`
(`SESSION_SID`/`with_session_sid`/`current_session_sid`):

```rust
// runtime/src/sky_runtime/live/hub.rs (new, near the top)
tokio::task_local! {
    /// The tenant-scope prefix in effect for the current request, when the
    /// session carries a `tenant` claim. Unset (→ "") outside a tenant-
    /// scoped session — every service is in-scope in that case (matches Go's
    /// `tenantPrefixForSession` returning "" when the session has no tenant
    /// claim). Populated today only by tests (`with_tenant_prefix`); the
    /// live-session → claims → this task-local wiring is a follow-up (see
    /// module doc), gated on `SKY_CONSOLE_AUTH=app` landing
    /// (`console.rs:192-206`).
    static TENANT_PREFIX: String;
}

/// Run `f` with `prefix` available to `current_tenant_prefix()`. Test-only
/// today (see module doc); the live dispatch loop will call this once the
/// `SKY_CONSOLE_AUTH=app` session-identity plumbing lands.
pub fn with_tenant_prefix<R>(prefix: String, f: impl FnOnce() -> R) -> R {
    TENANT_PREFIX.sync_scope(prefix, f)
}

fn current_tenant_prefix() -> String {
    TENANT_PREFIX.try_with(|s| s.clone()).unwrap_or_default()
}
```

**2. The gate function**, direct port of
`rejectCrossTenantSvc` (`hub_bridge.go:561-572`):

```rust
/// Enforce that an explicit service-name argument is scoped within the
/// caller's tenant. `Ok(effective_svc)` when the call may proceed (either
/// no tenant claim is in scope, so every svc is in-scope; or svc == "" so
/// the tenant prefix alone drives scoping; or svc starts with the tenant
/// prefix). `Err(())` when svc is outside the tenant's scope — the caller
/// must refuse with an Err, never silently drop the tenant filter.
fn reject_cross_tenant_svc(svc: &str, tenant_prefix: &str) -> Result<String, ()> {
    if tenant_prefix.is_empty() {
        return Ok(svc.to_string());
    }
    if svc.is_empty() {
        return Ok(String::new());
    }
    if svc.starts_with(tenant_prefix) {
        Ok(svc.to_string())
    } else {
        Err(())
    }
}
```

**3. SQL-layer `LIKE`-prefix scoping**, direct port of
`escapeLikePrefix` (`store.go:800-816`) plus the `AND service_name LIKE
?` clause (`store.go:490-492`):

```rust
/// Strip SQL LIKE wildcard characters (`%`, `_`) out of a tenant prefix
/// before it is used to build a `LIKE 'prefix%'` pattern — a tenant
/// identifier containing either character would otherwise widen its own
/// scope (e.g. a tenant literally named `%` would match every service).
/// Mirrors Go's `escapeLikePrefix` (strips rather than backslash-escapes,
/// since tenant identifiers are short alphanumeric-with-dashes slugs, not
/// arbitrary user text where preserving the literal character matters).
fn escape_like_prefix(p: &str) -> String {
    p.chars().filter(|&c| c != '%' && c != '_').collect()
}
```

Wire it into each of the four `read_*_value` builders
(`hub.rs:109-189` `read_logs_value`, `:251-291` `read_metrics_value`,
`:310-350` `read_traces_value`, `:355-387` `read_errors_value`) by
adding a `tenant_prefix: &str` parameter and, when non-empty, an
additional `AND service_name LIKE ?` clause bound to
`format!("{}%", escape_like_prefix(tenant_prefix))` — same shape as the
existing `if !service.is_empty() { sql.push_str(" AND service_name = ?"); }`
lines already in each function (`hub.rs:118-120,256-258,318-320,360-362`).

**4. Kernel-layer wiring** — `hub_read_filtered_logs` /
`hub_read_filtered_metrics` / `hub_read_filtered_traces` /
`hub_read_filtered_errors` (`hub.rs:206-217` and the `399+` area) each
gain the gate at the top of their `Box::pin(async move { … })` body:

```rust
pub fn hub_read_filtered_logs<E, A, F>(db_path: String, service: String, filter: F) -> SkyTask<E, A>
where
    E: Send + From<String> + 'static,
    A: DeserializeOwned + Send + 'static,
    F: Serialize + Send + 'static,
{
    Box::pin(async move {
        let tenant = current_tenant_prefix();
        let effective_svc = match reject_cross_tenant_svc(&service, &tenant) {
            Ok(s) => s,
            Err(()) => {
                return SkyResult::Err(
                    "hub.readFilteredLogs: service outside tenant scope"
                        .to_string()
                        .into(),
                );
            }
        };
        let f = decode_filter(filter);
        let arr = read_logs_value(&db_path, &effective_svc, &tenant, f).await;
        decode_rows(arr)
    })
}
```
(and the analogous change for metrics/traces/errors, each adding the
`&tenant` argument to its `read_*_value` call per point 3 above).

### Tests — port the Go table directly

New `#[cfg(test)] mod tenant_tests` in `hub.rs` (or extend the existing
`mod tests` at `hub.rs:983+`):

```rust
#[test]
fn reject_cross_tenant_svc_table() {
    // Ported from Go's TestRejectCrossTenantSvc table
    // (hub_tenant_test.go:239-254).
    assert_eq!(reject_cross_tenant_svc("", "tenant-"), Ok(String::new()));
    assert_eq!(
        reject_cross_tenant_svc("tenant-foo", "tenant-"),
        Ok("tenant-foo".to_string())
    );
    assert_eq!(reject_cross_tenant_svc("other-foo", "tenant-"), Err(()));
    // Prefix match must be strict (bare "tenant" does not start with
    // "tenant-" as a PREFIX match on the full string "tenant-").
    assert_eq!(reject_cross_tenant_svc("tenant", "tenant-"), Err(()));
    // No tenant claim → every svc in scope.
    assert_eq!(
        reject_cross_tenant_svc("anything", ""),
        Ok("anything".to_string())
    );
}

#[test]
fn escape_like_prefix_strips_wildcards() {
    assert_eq!(escape_like_prefix("customer-42-"), "customer-42-");
    assert_eq!(escape_like_prefix("cust%omer_42"), "customer42");
}

// The two-tenant regression: a spill DB seeded with rows for
// "customer-42-billing" and "customer-99-billing" — querying with
// tenant prefix "customer-42-" must return ONLY the customer-42 rows,
// even when called with service="" (tenant-only scope) — direct port of
// Go's TestQueryFilteredLogsJSONWithTenant_TwoTenants
// (hub/bridge_test.go:405-430).
#[tokio::test]
async fn hub_read_filtered_logs_two_tenants_no_cross_read() {
    // 1. Build a temp SQLite spill DB with the telemetry_log schema.
    // 2. Insert one row with service_name = "customer-42-billing",
    //    one with service_name = "customer-99-billing".
    // 3. with_tenant_prefix("customer-42-".to_string(), || async {
    //        hub_read_filtered_logs::<String, Vec<serde_json::Value>, _>(
    //            db_path, "".to_string(), serde_json::json!({}),
    //        ).await
    //    }) — assert Ok, exactly one row, and it's the customer-42 row.
}

#[tokio::test]
async fn hub_read_filtered_logs_rejects_explicit_cross_tenant_svc() {
    // with_tenant_prefix("customer-42-".to_string(), || async {
    //     hub_read_filtered_logs(db_path, "customer-99-billing".to_string(), ..).await
    // }) — assert Err, and assert the store was never queried (no rows
    // leaked even in the Err path — the gate must short-circuit BEFORE
    // building the SQL, not filter after the fact).
}
```

### Follow-up to file (do not lose this)

Add a backlog line: *"Wire `hub::with_tenant_prefix` from the live
session's authenticated identity once `SKY_CONSOLE_AUTH=app` lands
(`console.rs:192-206`) — the consumer-side tenant gate (§5 of the Class 7
spec) is built and tested but has no producer wiring a real deployment
can trigger yet."* This keeps the gap from silently vanishing once this
spec's code lands (a partially-wired security feature reads as "done" in
a shallow `rg -n tenant` check unless the follow-up is explicit).

---

## §6 — #34: wire `Db.Decode.money` as a reachable kernel

**Files (9 touch points across 6 crates — mirrors the `Border.shadow` /
`DbDecBool` registration shape exactly):**

`runtime/src/sky_runtime/db.rs:354-419` already has `db_decode_money`
fully implemented and tested (`test_db_decode_money_roundtrip`,
`db.rs:2944`) — **zero runtime changes needed.** The gap is entirely in
the compiler's kernel registry; `ipe-index parity --gaps` confirms:
`DbDec.money go=1 rust=0`.

### API-shape decision — return `Decoder (Decimal, String)`, not `Decoder Money`

The Go reference (`../sky/runtime-go/rt/db_decoder.go:202-244`,
`DbDec_money`) returns a full `Decoder Money`, constructing the Sky
`Money` ADT (`SkyADT{SkyName:"Money", Fields:[decimal, currency]}`)
directly at the runtime layer — including resolving the 3-letter ISO
code to a proper `Currency` ADT variant via a hand-rolled
`sqlCodeToCurrency` switch (`db_decoder.go:246+`). Go can do this because
its runtime is dynamically-typed (`any`-based `SkyADT`) — the runtime
can construct an arbitrary user ADT value at runtime with no compile-time
type dependency on the project's generated Go types.

This Rust runtime CANNOT do the equivalent: `Money` and `Currency` are
project-generated Rust types (`StdMoneyMoney`, `StdMoneyCurrency`)
depending on the project's module-name prefix, unnameable from the
`sky-runtime-rust` crate — this is exactly why `db_decode_money`
(`db.rs:404-419`'s doc comment) already returns the structural
`(Decimal, String)` pair instead, with an explicit note that a
"codegen-level wrapper" would be needed to go further.

**Decision for this spec: ship the reachable `Decoder (Decimal, String)`
kernel now** (zero runtime changes, mechanical registration only,
matches the already-implemented/tested runtime fn exactly) rather than
inventing a new per-project codegen wrapper that constructs
`StdMoneyMoney`/`StdMoneyCurrency` from scratch (that wrapper would also
need to reimplement Go's `sqlCodeToCurrency` 50+-code table, or call into
whatever `Std.Money.parseCurrency`-equivalent already exists at the Sky
source level — a materially bigger, GUARDIAN-DESIGN-shaped unit of work,
not a MECHANICAL one). **This is a recorded, intentional divergence from
Go**, not a silent gap: `Db.Decode.money : String -> Decoder (Decimal,
String)` in this Rust backend vs `Decoder Money` in Go. Document it in
`docs/divergences-from-sky.md` in the same commit. File a follow-up
(`Db.Decode.money` full parity via a `Currency`-construction codegen
wrapper) as a separate, later item — explicitly NOT bundled into this
mechanical fix, per the class's own MECHANICAL classification.

### Registration recipe (9 touch points)

1. **`crates/sky_canon/src/env.rs:956-960`** — add `"money"` to the
   `Db.Decode` qualifier's symbol allowlist:
```rust
(
    "Db.Decode",
    &[
        "string", "int", "float", "bool", "money", "nullable", "map", "andThen",
        "succeed", "fail", "map2", "map3", "map4", "required", "optional",
    ],
),
```
   **This is the touch point most likely to be missed** — it is a plain
   string list, not a `KernelFn`-named site, so grepping for
   `DbDecMoney`-shaped names will not find it. Skipping it means
   `Db.Decode.money` is rejected at canonicalisation with "unknown
   qualified name" (the v0.15.42-equivalent hardening in this repo,
   `sky_canon`) even after every other touch point below is wired.

2. **`crates/sky_kernels/src/lib.rs`** (enum, near the other `DbDec*`
   variants, ~line 600):
```rust
DbDecMoney,
```

3. **`crates/sky_kernels/src/lib.rs`** (`decl()` match, ~line 1720, next
   to `DbDecBool`'s arm):
```rust
Self::DbDecMoney => d("Db.Decode", "money", 1, Db, "db_decode_money"),
```

4. **`crates/sky_kernels/src/lib.rs`** (`ALL` const list, ~line 2681):
```rust
Self::DbDecMoney,
```

5. **`crates/sky_kernels/src/lib.rs`** (`is_db()`-style or-pattern gate,
   ~line 3169-3173 — the block already includes `Self::DbDecBool`):
```rust
| Self::DbDecMoney
```

6. **`crates/sky_types/src/constrain.rs`** (kernel scheme, ~line
   3851-3854, next to the sibling primitive decoders). Verify `decimal`
   (line 2973) and `tuple2` (line 2983) closures are in lexical scope at
   this match arm — both are defined earlier in the same enclosing
   function per the file's existing structure; if a scope boundary
   exists between them and the `K::DbDec*` arms, hoist equivalent local
   bindings rather than duplicating the closure body:
```rust
K::DbDecMoney => fun(string(), dec(tuple2(decimal(), string()))),
```

7. **`crates/sky_types/src/constrain.rs`** (completeness/exhaustiveness
   list, ~line 5642-5646, the `// Db.Decode (14)` block — update the
   count comment to 15):
```rust
K::DbDecMoney,
```

8. **`crates/sky_lower/src/lower.rs`** (arity-1 grouping or-pattern,
   ~line 7104-7109, `// ── Db.Decode arity-1 (M5b-db) ──` block):
```rust
| KernelFn::DbDecMoney
```

9. **`crates/sky_lower/src/lower.rs`** (callee dispatch, ~line 8624, next
   to `("Db.Decode", "bool")`):
```rust
("Db.Decode", "money") => Ok(Callee::Kernel(KernelFn::DbDecMoney)),
```

10. **`crates/sky_backend_rust/src/naming.rs`** (~line 699):
```rust
KernelFn::DbDecMoney => "db_decode_money",
```

11. **`crates/sky_backend_rust/src/emit_expr.rs`** (~line 1734-1747,
    "standard-path Db kernels" list — `Decoder`-returning kernels need
    no custom projection since `Decoder<E, (Decimal, String)>` is a
    runtime-native generic type, not a project-generated one):
```rust
| KernelFn::DbDecMoney
```

12. **`crates/sky_ir/src/pretty.rs`** (~line 616):
```rust
KernelFn::DbDecMoney => "Db.Decode.money",
```

(12 edits across 6 files, not 9 — corrected count after listing them
all; the "9ish" estimate in earlier notes undercounted `sky_kernels`'s 4
internal touch points.)

### Verification

```bash
cd /home/arthur/Documentos/comp/sky-rust
cargo build --workspace
cargo test -p sky_kernels -p sky_types -p sky_lower -p sky_backend_rust -p sky_canon -p sky_ir
scripts/ipe-index parity --gaps | rg "DbDec.money"   # must disappear from the gap list
```

### Regression tests (mandatory — #34 explicitly calls for a self-oracle)

1. **Canon**: a positive test in `sky_canon` confirming
   `Db.Decode.money "col"` resolves (no "unknown qualified name") —
   guards touch point 1 specifically, since it is the one most likely to
   silently regress on a future refactor of `env.rs`'s qualifier tables.
2. **Kernel-scheme unit test** (`sky_types`): `Db.Decode.money : String
   -> Decoder (Decimal, String)` — assert the inferred type matches
   exactly (catches a `tuple2`/`decimal` scope mistake in touch point 6
   at compile-check time rather than at codegen).
3. **Self-oracle golden** (new `crates/skyc/tests/golden_db_decode_money.rs`,
   mirroring the existing `m5b_db_*` golden convention noted in the
   Class 6 spec): a fixture Sky program that calls
   `Db.Decode.money "amount_col"` inside a `Db.queryDecode` pipeline,
   `skyc build`s it, and runs it against a real SQLite row written via
   `SqlMoney` — asserts the decoded `(Decimal, String)` round-trips the
   original `Money` value's amount + currency code exactly. Mark
   `oracle_divergence = true` per the existing convention (no Go
   counterpart returns this shape — Go returns `Decoder Money` per the
   API-shape decision above).
4. **Negative test**: malformed money text (missing space separator, e.g.
   `"USD1234.56"` or `"1234.56"` with no code) → `Task Err`, never a
   panic — this already has runtime coverage
   (`db_decode_money`'s existing implementation handles it, `db.rs:405-419`
   area) but add an explicit kernel-reachable end-to-end assertion now
   that the call path exists.

### Documentation updates required in the same change

- `docs/stdlib.md`: `Db.Decode.money : String -> Decoder (Decimal,
  String)` entry, noting the divergence from Go's `Decoder Money` and
  showing the one-line composition pattern
  (`Decode.map (\(amount, code) -> …) (Decode.money "col")`) users write
  to get a `Money` value.
- `docs/divergences-from-sky.md`: new entry for the `Decoder (Decimal,
  String)` vs `Decoder Money` shape (cross-reference the follow-up item
  for full parity).
- `CLAUDE.md`'s stdlib table entry for `Std.Db` already says *"paired
  with `Db.Decode.money` for round-trip"* — no wording change needed
  there (still true), but the Rust-backend-specific divergence belongs
  in `docs/divergences-from-sky.md`, not CLAUDE.md (CLAUDE.md documents
  the language/stdlib surface generally, not per-backend divergences).

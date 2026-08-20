# Data-preserving column and table renames in the Db migrate ledger

## Problem

`Ipe.Db.Store` builds a table's `CREATE TABLE` DDL from its typed column set and
hands it to the versioned `Db.migrate` ledger. The ledger is append-only and
records each migration once, keyed by name, with a checksum over its SQL text so
an edited-after-apply migration is caught as drift.

There is no safe, data-preserving way to rename a column or a table. The only
expressible paths today are:

- **Drop and recreate** through a changed `Store` — loses all row data, and in
  fact trips the drift detector (the `create_<table>` entry's checksum changes),
  so it does not even apply cleanly on an existing database.
- **`Ipe.Db.Unsafe`** with a hand-written `ALTER … RENAME` — discloses the raw
  SQL capability program-wide and abandons the ledger's idempotency and
  injection-safety guarantees.

A rename is ordinary schema evolution that must preserve data. It has no home on
the safe surface.

## Why the naive approaches fail

The ledger keys and checksums a migration by its `{ name, sql }`. Two invariants
fall out that any rename design must respect:

1. **A never-drifting create.** The `create_<table>` entry's SQL must stay byte
   stable across the table's whole life, or re-running `migrate` on an existing
   database errors with a checksum mismatch before any rename runs. So the create
   DDL cannot be re-derived from an evolving column set — the moment the columns
   change, the create entry drifts.
2. **Convergence on a fresh database.** A migration list must produce the same
   final schema whether replayed from empty or applied incrementally to a
   populated database. A rename expressed as "edit the create entry" cannot
   satisfy both: the existing database needs `ALTER … RENAME` (preserve data),
   the fresh database needs the final columns in `CREATE TABLE` directly.

The append-only ledger already resolves both — for renames as for any other
evolution — provided the rename is a **new, separate, once-applied entry** rather
than an edit to an existing one.

## Decision

Adopt the issue's **explicit-rename-step** shape (its Option 2), made safe: a
rename is a first-class migration entry whose SQL is `ALTER … RENAME`, assembled
only from `validSqlIdent`-checked identifiers. The `Db.migrate` runtime is
unchanged — it already applies arbitrary DDL once and skips on re-run.

Express the rename **on the `Store`** — `Store.renameColumn` / `Store.renameTable`
return a new `Store` whose current-column view already reflects the new name and
which carries a schema-op log — rather than as free builders producing loose
`Migration` values against a separately maintained column set. The op-log folds to
the ordered ledger entries (`Store.migrations store` = frozen create + rename
ALTERs), while ordinary reads and writes keep targeting the store's current
columns. Both the migrations and the CRUD identifiers derive from **one** value.

**Why on the store, weighed by PRINCIPLES (strict order):**

- *Correctness (2)* is the deciding principle. A free-builder design keeps the
  current column set in two places — the ordered migration list and the `Store`'s
  columns that `insert`/`get`/`all` target — which can silently disagree (the
  migration renames `name → full_name` while the store still holds
  `textColumn "name"`, so CRUD hits a column that no longer exists). Deriving both
  from one store makes that drift **unrepresentable** (single-source-of-truth +
  make-invalid-states-unrepresentable). The only residual footgun — hand-editing
  the original `fromColumns` set instead of calling `renameColumn` — changes the
  frozen create text and is caught fail-closed by the ledger's existing checksum
  drift detector, a second independent boundary (defend-in-depth).
- *Security (1) / Soundness (3)* are a tie between the two shapes: both validate
  every identifier before it reaches SQL, bind all values, and stay immutable.
- *Efficiency (4) / Readability (6)* favour the smaller free-builder diff, but the
  ordering is a strict tie-breaker: a lower principle never justifies conceding
  the Correctness hole above.

The validated rename-DDL construction is factored into one shared internal
function, so the SQL text itself has a single source of truth regardless of how it
is invoked.

**Rejected: stable column identity (the issue's Option 1).** Auto-detecting a
rename versus a drop-plus-add from two column sets is inherently ambiguous
(`name → full_name` and `drop name; add full_name` are indistinguishable), yet
the requirement is to fail closed on ambiguity — Option 1 makes ambiguity the
common case. It also needs a second persisted store (an id↔name map) that is a
second source of schema truth able to drift from the ledger, violating SSOT, for
a medium-priority gap. The explicit step reuses the entire existing ledger and
adds only pure, individually testable builder functions. (The issue's stated
non-goal — stable identifiers for client codegen — does not apply: Ipê is
single-language, so no generated client's validity is at stake.)

## Design

The code blocks below are the **proposed** interface — illustrative Ipê source
that does not yet exist in the tree and is not runnable against the current
stdlib. They are the specification to implement.

Add schema-evolution operators to `Ipe.Db.Store` that return a new `Store`
carrying a schema-op log, and a folder that renders the log to the ordered ledger
entries `Db.migrate` consumes:

```elm
-- illustrative: proposed signatures, not yet in stdlib
renameColumn : String -> String -> Store -> Store       -- from -> to -> store
renameTable  : String -> Store -> Store                 -- to -> store
migrations   : Store -> Result Error (List Migration)   -- frozen create + rename ALTERs
```

`renameColumn "name" "full_name"` updates the store's current-column view to
`full_name` (so later `insert`/`get`/`all` target `full_name`) and appends a
rename op to the log. `migrations store` folds the log into the ordered list: the
**frozen** create entry (built from the store's columns *as first constructed*,
`name = create_<table>`) followed by one `ALTER … RENAME` entry per rename op.
Identifier validation happens in `migrations` (the point SQL text is built), so
`renameColumn`/`renameTable` stay total constructors and the fail-closed `Err`
surfaces once, at rendering:

```elm
-- illustrative: proposed usage pattern, not yet runnable against current stdlib
run : Db -> Task Error (List String)
run conn =
    let
        store =
            Store.fromColumns "users"
                [ Store.textColumn "id", Store.textColumn "name", Store.intColumn "age" ]
                |> Result.map (Store.primaryKey "id")
                |> Result.map (Store.renameColumn "name" "full_name")
    in
    case store |> Result.andThen Store.migrations of
        Ok ms -> Db.migrate conn ms          -- [ create_users (name), rename_column_users_name_to_full_name ]
        Err e -> Task.fail e
```

The create entry is frozen at the original columns (`id, name, age`); each rename
is a separate appended entry, never an edit — so the create never drifts, the CRUD
identifiers and the migration identifiers come from the one store value, and the
list replays correctly on both a populated and a fresh database. `Store.create`
remains the one-shot initial-create convenience for a store with no rename ops;
once a store carries renames, `Store.migrations` is the path (and `Store.create`
routes through it).

### Validation (parse-don't-validate, fail-closed)

`Store.migrations` is the only new place rename DDL text is constructed. The
shared internal rename-DDL constructor builds exclusively from validated
identifiers and holds no values, so there is nothing to inject — the same
structural argument the module header already makes for `createSql`:

- Every identifier (the table name and each rename's `from`/`to`) is checked with
  `validSqlIdent` **before** it can reach the SQL string; the first rejection makes
  `migrations` return `Err (invalidIdentError "column" name)` / `... "table" name`
  and produces no SQL. `validSqlIdent` accepts only non-empty strings of
  `[A-Za-z0-9_.]`, rejecting quotes, semicolons, whitespace, and parentheses.
  (`renameColumn`/`renameTable` themselves are total — an invalid name is admitted
  into the op log and rejected at render, so there is exactly one failure point.)
- A no-op rename (`from == to`) makes `migrations` return `Err` via a new
  `noopRenameError` (a likely mistake that would seed a redundant ledger entry).
- No destructive DDL is generated. The evolution surface is rename-only;
  `DROP COLUMN` and other data-losing alters stay out of the safe surface (see
  Scope).

### Generated DDL and stable names

| Rename op      | `sql`                                                | `name`                                  |
| -------------- | ---------------------------------------------------- | --------------------------------------- |
| `renameColumn` | `ALTER TABLE <table> RENAME COLUMN <from> TO <to>`   | `rename_column_<table>_<from>_to_<to>`  |
| `renameTable`  | `ALTER TABLE <from> RENAME TO <to>`                  | `rename_table_<from>_to_<to>`           |

The name is derived from the already-validated identifiers, so it is a stable,
human-legible, collision-resistant ledger key. Stability is what makes the rename
apply exactly once: a second `migrate` pass finds the name already in the ledger
and skips it — it does not re-issue the ALTER (which would fail on the now-absent
old identifier). This is existing ledger behavior; the store's op-log only
supplies the entry.

Both statements are standard portable DDL. `RENAME COLUMN` requires SQLite
≥ 3.25.0; the bundled `sqlx` 0.8 SQLite driver is far newer, and PostgreSQL has
supported both forms for many releases.

### Why data is preserved

- **Existing database:** the frozen create entry is already applied and skipped;
  the rename entry runs `ALTER … RENAME`, which preserves every row and only
  changes the identifier. Idempotent on re-run (skipped by stable name).
- **Fresh database:** the create entry runs first (original columns), then the
  rename entry alters them — converging to the identical final schema.
- **Never destructive:** `ALTER … RENAME` moves no data and drops nothing. A
  rename that references a missing column fails at the database inside its own
  transaction, which rolls back and leaves the ledger unadvanced — surfaced as a
  `Task Err` (or a non-zero `ipe db migrate` exit), never a silent drop or
  recreate. That is the required fail-closed behavior.

## Scope

**In:** `Store.renameColumn`, `Store.renameTable`, `Store.migrations`, the
schema-op log on the `Store` type, and the shared internal rename-DDL constructor;
`Store.create` rerouted through `migrations`; a `noopRenameError`; doc-strings with
verified examples; tests proving data preservation, CRUD-tracks-current-column, and
fail-closed rejection.

**Out (explicit, each its own future issue if a need arrives):**

- **`DROP COLUMN` / destructive alters** — data-losing; SQLite historically needs
  a table rebuild. Keeping the evolution surface rename-only holds the destructive
  blast radius at zero.
- **Stable identifiers for client codegen** — out by the issue's own non-goal.

## Implementation plan

1. `src/stdlib/Ipe/Db/Store.ipe`
   - Extend the `Store` type with an ordered schema-op log and preserve the
     columns *as first constructed* (the frozen-create source). `fromColumns`
     seeds an empty log; `renameColumn`/`renameTable` append an op and update the
     current-column view (so CRUD targets the new name); the original column list
     is fixed once the store is built.
   - Add `renameColumn : String -> String -> Store -> Store`,
     `renameTable : String -> Store -> Store`,
     `migrations : Store -> Result Error (List Migration)`, a shared internal
     rename-DDL constructor, and `noopRenameError`. Reroute `Store.create` through
     `migrations`.
   - Reuse `validSqlIdent`, `invalidIdentError`, `createSql`, `migrationName`.
   - If a `Migration` type alias is not already exported from `Ipe.Db`, add
     `type alias Migration = { name : String, sql : String }` there (or in
     `Store`) and use it in every signature — one SSOT for the entry shape.
   - Every new export gets a `{-| … -}` doc-string with a real, compiling example
     (per the doc-string-SSOT rule — no toy `1+1`), including the evolve-then-
     `migrations` pattern above.
   - Extend the module header's security argument to cover the rename path
     (identifiers validated in `migrations` before reaching SQL, no values
     interpolated, rename-only so non-destructive, drift between CRUD columns and
     migration columns made unrepresentable by the single store value).
2. No `src/runtime/rust/src/db.rs` change. Confirm by reading `db_migrate_apply`
   that arbitrary `ALTER` DDL flows through `db_exec_raw` inside the per-migration
   transaction (it does).
3. Regenerate the doc surface if the doc-string/veneer scheme requires it; let the
   `stdlib-docs-drift` gate confirm `docs/stdlib.md` + `docs/internals/stdlib/`
   are in sync.

## Test plan

Data-preservation proof — extend the `#[tokio::test]` ledger tests in
`src/runtime/rust/src/db.rs` (tightest loop; drives the ledger directly):

1. **Column rename preserves data.** Create `users(id, name)`, insert rows, apply
   `[create, renameColumn users name→full_name]`; assert the rows are readable
   under `full_name` and `name` no longer exists.
2. **Idempotent re-run.** Re-run the same migration list; assert no error, the
   rename is not re-issued, and the data is unchanged.
3. **Fresh-database convergence.** Apply the same list to an empty database;
   assert the final schema has `full_name` and the rows inserted afterward read
   back.
4. **Table rename preserves data.** Analogous with `renameTable`.
5. **CRUD tracks the current column.** After `renameColumn "name" "full_name"`,
   an `insert`/`get`/`all` built from the evolved store targets `full_name`
   (proving the store's current-column view and the migration identifiers share
   one source); a store still holding the old name would target `name`.
6. **Fail-closed rejection (pure, via a golden or a Store unit test):** an invalid
   identifier (`"a b"`, `"x;drop"`, empty) in a rename op and a no-op
   (`from == to`) each make `Store.migrations` return `Err` and produce no SQL.
7. **Missing-column rename fails closed:** applying a column rename whose `from`
   is absent returns a `Task Err` / non-zero exit and leaves the ledger
   unadvanced (no partial state).

## Guardian review checklist

- **Injection:** every identifier passes `validSqlIdent` inside `migrations`
  before reaching the SQL string; no value is interpolated; the shared rename-DDL
  constructor is the only new SQL-text construction site. Note `validSqlIdent`
  permits `.` — confirm a dot cannot break out of an identifier in `ALTER …
  RENAME` (it cannot: no quote, semicolon, whitespace, or paren is admitted), and
  decide whether a dot in a bare column identifier warrants an extra reject
  (optional hardening, not required for safety).
- **Single source of truth (the core of this design):** the CRUD column
  identifiers and the migration column identifiers both derive from the one
  `Store` value, so a rename cannot leave writes targeting a column the schema no
  longer has. Verified by test 5.
- **Idempotency / data preservation:** the stable name guarantees exactly-once
  application; a re-run must skip, not re-issue. Verified by test 2.
- **Drift non-interaction / defend-in-depth:** a rename appends a new ledger entry
  and never edits the frozen create entry; the create checksum is unchanged
  (verified by tests 1 and 3). Confirm the residual footgun — editing the original
  column set instead of using `renameColumn` — is caught by the ledger's checksum
  drift detector, not silently accepted.
- **Fail-closed:** `from == to`, invalid identifiers, and missing-column renames
  all error and issue no partial schema change. Verified by tests 6 and 7.
- **Blast radius:** no destructive DDL is generated anywhere in the added surface.
- **Store immutability/soundness:** `renameColumn`/`renameTable` return new
  immutable `Store` values (no shared mutable state); the op log is bounded by the
  number of source-authored renames.
- **SSOT / drift gate:** generated docs match the new doc-strings.

Sibling design: `db-store-row-level-security.md` (independent; lower urgency does
not apply here — this is a standalone correctness primitive).

# Store: composite primary key + upsert — design spec

`Ipe.Db.Store` (`src/stdlib/Ipe/Db/Store.ipe`) declares only a single-column
primary key and offers no insert-or-update path. Two capabilities are missing,
each forcing hand-written raw SQL that escapes the typed, identifier-validated
Store surface:

1. A **composite (multi-column) primary key** in the generated DDL. `Store.primaryKeyNamed : String -> Draft a -> Draft a` records exactly one column (`pk : Maybe String`) and emits an *inline* column constraint (`col PRIMARY KEY`). A table needing `PRIMARY KEY (col_a, col_b)` — a table-level constraint — cannot be expressed and carries the key as hand-DDL outside Store (`tools/code-review/src/Lib/Db.ipe:89-96`, `tools/code-review/src/Lib/Migrations.ipe`).
2. **Upsert** — insert-or-update-on-conflict. Store has `insert` / `insertReturning` / `update` / `updateWhere` but no upsert, so `tools/code-review/src/Lib/Db.ipe:297-310` hand-writes `INSERT OR REPLACE INTO drain_cursor VALUES (?, ?, ?)` through `Db.exec`.

This spec designs both additions against `PRINCIPLES.md` (strict precedence
Security > Correctness > Soundness > Efficiency > Ease > Readability) and
ADR-0003's data-access invariants, and proposes them only because every
principle survives the adversarial refutation below.

## Grounding — the surface as it is

- `Draft a` / `Store a` (`Store.ipe:179-219`) carry `codec` (the SSOT for
  columns and their types), `specs : List ColumnSpec`, `pk : Maybe String`,
  `frozenColumns` / `frozenTable` (the never-drifting create source),
  `currentColumns` / `table` (post-rename view), `ops`, `indexes`.
- `ColumnSpec` (`Store.ipe:133-140`) is a closed ADT: `PrimaryKey String`,
  `Serial String`, `Unique String`, `DefaultNow String`, `DefaultText String
  String`, `DefaultInt String Int`, `TouchOnUpdate String`. Each carries its
  column name as its first field (`specColumn`, `Store.ipe:3752`).
- DDL text is built in exactly ONE place: `createSqlFromColumns`
  (`Store.ipe:3698-3722`), reached by both `createSql` (`3691`) and `migrations`
  (`949-953`). It validates the table name (`validSqlIdentPlain`), rejects any
  spec whose column is absent from the codec's columns (`specsWithinColumns`,
  fail-closed with `unknownColumnError`), then renders per-column defs. The PK is
  emitted as an inline suffix by `columnConstraints` (`3859-3907`) in canonical
  order `PRIMARY KEY → AUTOINCREMENT → UNIQUE → DEFAULT`.
- Identifier gates: `validSqlIdentPlain` (dot-free, `Store.ipe:262`) mirrors the
  runtime `SqlIdent::parse_plain`; every column key and table name already passes
  it at construction (`fromCodec` / `fromColumns`). Defence in depth: the runtime
  `db.rs` re-validates every identifier before it reaches SQL.
- `insert` (`Store.ipe:1212`) projects the row through `DbCodec.codecToBinds`,
  maps each `(col, SqlValue)` to a `SqlField` (`OmitField` for `Serial` /
  `DefaultNow` / `TouchOnUpdate`, else `SetField`), and routes to the
  `Db.insertFields` kernel. `update` routes to `Db.updateFields`. No SQL text is
  built in `.ipe`; the runtime kernels build it from validated identifiers and
  positional binds.
- Multi-driver (ADR-0003 "Multi-driver DB: compile-time selection"): the parsed
  `DbDriver` selects between two config templates (`config.rs` sqlite /
  `config_postgres.rs`) exporting **identical symbol names**; `db.rs` never
  branches on driver and compiles once per project. Per-dialect differences are
  confined to `db_format_sql` (rewrites `?` → `$n` for Postgres, quote-aware) and
  a few consts. SQLite floor is ≥3.35 (`db.rs:3874`, already required for
  `RETURNING`).

## Part 1 — composite primary key

### API

Add one builder, composing with `Draft` exactly like the existing named specs:

```
compositePrimaryKeyNamed : List String -> Draft a -> Draft a
```

- Accessor-typed sugar for the record surface (mirroring `primaryKey`'s kernel
  intercept that extracts a column name from an accessor):

```
compositePrimaryKey : List (row -> t) -> Draft row -> Draft row
```

  lowers each accessor to its column name and calls
  `compositePrimaryKeyNamed`. (Heterogeneous accessor element types make a single
  `List (row -> t)` too rigid for mixed-type keys; the kernel intercept resolves
  the columns positionally at lowering time, so the *named* form is the SSOT and
  the accessor form is a lowering convenience. If the intercept cannot type a
  mixed-type accessor list, ship `compositePrimaryKeyNamed` alone — the accessor
  form is not load-bearing.)

`compositePrimaryKeyNamed ["uid", "body_hash"]` at
`tools/code-review/src/Lib/Db.ipe` replaces the hand-DDL composite key.

### Representation — make the two PK forms mutually exclusive

`pk : Maybe String` cannot hold a column list, and a table has at most one
primary key. Make "single PK" and "composite PK" a closed sum so declaring both,
or a composite PK plus `Serial` (SQLite `AUTOINCREMENT` requires a single-column
`INTEGER PRIMARY KEY`), is a *typed error at DDL time*, not a representable
double-PK. Replace the bare `pk : Maybe String` field with:

```
type PrimaryKeyDecl
    = NoPk
    | SinglePk String
    | CompositePk (List String)      -- ≥ 2 columns, order-significant
```

- `primaryKeyNamed` sets `SinglePk name`; `compositePrimaryKeyNamed cols` sets
  `CompositePk cols`. Applying either when a PK already exists is a build-time
  `Err` (a store has one primary key — declaring a second is a representable-but-
  illegal state, refused at `createSql`, never a silent last-writer-wins).
- The by-key operations (`get` / `delete` / `update`) already assume a single
  `pk`; a `CompositePk` store has no single key column, so those operations
  return the existing `missing primary key` typed `Err` (`Store.ipe:5880`) —
  by-composite-key CRUD is out of scope for this spec (it needs a multi-column
  WHERE and is a separate, larger design). The composite PK is a *schema* fact
  (DDL + upsert conflict target), not a CRUD-by-key surface here.

`ColumnSpec` gains nothing: the composite key is a *table-level* constraint over
several columns, structurally unlike the per-column `PrimaryKey String`. Keep it
on the `PrimaryKeyDecl` field so `specColumn` / `columnConstraints` stay per-column
and the render site decides inline vs table-level from the decl.

### DDL rendering (the one site: `createSqlFromColumns`)

- `SinglePk` → unchanged: inline `col PRIMARY KEY` via `columnConstraints`.
- `CompositePk cols` → NO inline PK on any column; instead append ONE table-level
  constraint term to the column-def list:

```
CREATE TABLE IF NOT EXISTS review (uid TEXT, body_hash TEXT, …, PRIMARY KEY (uid, body_hash))
```

  Column order is the caller-declared `cols` order, preserved verbatim (a PK's
  column order is semantically load-bearing for the backing index).

Fail-closed validation added to `createSqlFromColumns`, BEFORE any text is built:

1. `cols` is non-empty and has ≥ 2 entries (a 1-element composite key is a
   `SinglePk` mis-spelled — reject with a diagnostic pointing at
   `primaryKeyNamed`).
2. Every column in `cols` passes `validSqlIdentPlain` (defence in depth over the
   construction-time gate).
3. Every column in `cols` is one of the codec's derived columns
   (`hasColumn` / the `specsWithinColumns` discipline) — an unknown column fails
   closed, never a silent no-op DDL.
4. `cols` has no duplicate (`PRIMARY KEY (a, a)` is a backend error and a
   caller mistake) — reject with a typed `Err`.
5. A `CompositePk` combined with any `Serial` spec is an `Err` (SQLite
   `AUTOINCREMENT` is single-column-`INTEGER-PK` only; Postgres composite +
   serial is legal but semantically muddled — refuse for one cross-backend rule).

Each is a new arm returning `Result Error String` from the existing single DDL
site, so `createSql` and `migrations` inherit the check automatically (SSOT: one
render, one validation).

## Part 2 — upsert

### The Correctness landmine — one defined semantics, refuted then survived

The hand-written `INSERT OR REPLACE` is exactly the trap. SQLite `INSERT OR
REPLACE` **deletes the conflicting row and inserts a new one**: it fires DELETE
triggers, changes the rowid, and resets every column the new row does not mention
to its DEFAULT (or NULL). Postgres has no `INSERT OR REPLACE`; the standard
`INSERT … ON CONFLICT (cols) DO UPDATE SET …` **updates the conflicting row in
place**, keeping unmentioned columns and the row identity. These are *different
observable semantics*. A Store upsert that emitted "whatever the engine does"
would violate Correctness (principle 2: same program, same input, same defined
output across every backend) and is REFUTED.

**Refutation survived — because one semantics is expressible identically on both
supported backends.** `INSERT … ON CONFLICT (target_cols) DO UPDATE SET c =
excluded.c, …` is supported by:

- SQLite ≥ 3.24 (2018); the repo already floors SQLite at ≥ 3.35
  (`db.rs:3874`), so it is unconditionally available.
- Postgres (native standard syntax); the `excluded` pseudo-table is the standard
  name in *both* engines.

The chosen, Ipê-defined semantics is **update-in-place on conflict**: on a
conflict at the declared target columns, the existing row is UPDATED (its
identity, rowid, and any column not in the upsert's SET list preserved; no DELETE
trigger fires). This is the *only* semantics Store exposes; `INSERT OR REPLACE`'s
delete-then-insert behaviour is never emitted. Because the identical SQL realises
identical behaviour on both backends, `db.rs` needs **no per-driver branch** — it
routes through `db_format_sql` for the `?`/`$n` rewrite exactly like every other
statement, honouring ADR-0003's "db.rs never branches on driver".

The SET list is derived, not caller-supplied SQL: every non-conflict-target,
non-DB-filled (`Serial` / `DefaultNow` / `TouchOnUpdate` — those stay
`excluded`-excluded so a client value can never overwrite a DB-owned column)
column of the row projection becomes `col = excluded.col`. Deterministic and
total: same row + same conflict target ⇒ same statement, every run.

### API

```
upsert : Db -> Store a -> a -> Task Error Int
```

- Mirrors `insert`'s shape and its OmitField discipline (`Store.ipe:1212-1237`):
  project the row through `DbCodec.codecToBinds`, build the `List (String,
  SqlField)`, and route to a new `Db.upsertFields` kernel.
- The **conflict target is not a caller argument** — it is the store's declared
  key. Store derives it from `PrimaryKeyDecl`: `SinglePk c` → `[c]`,
  `CompositePk cols` → `cols`. A store with `NoPk` and no unique constraint has
  no valid conflict target: `upsert` fails closed with a typed `Err`
  ("upsert requires a declared primary key or unique constraint as its conflict
  target"). This is the soundness anchor — a conflict target that is not a
  declared PK/unique set is a runtime error in both engines, so making it
  unrepresentable (derived only from declared PK/unique specs, never a free
  string) turns a would-be deferred blow-up into a compile-composed typed `Err`.

`upsert db drainStore cursorRow` at `tools/code-review/src/Lib/Db.ipe:297`
replaces the hand-written `INSERT OR REPLACE`, using the `drain_cursor` store's
composite PK `(uid, body_hash)` as the conflict target.

### Runtime kernel `Db.upsertFields`

Signature parallels `db_insert_fields` (`db.rs:3928`) plus the conflict-target
column list:

```
Db.upsertFields : Db -> String -> List String -> List (String, SqlField) -> Task Error Int
```

`db_upsert_fields` (new, `runtime/rust/src/db.rs`) builds, from validated
identifiers and positional binds only:

```
INSERT INTO <table> (<set-cols>) VALUES (<?…>)
  ON CONFLICT (<target-cols>) DO UPDATE SET <c = excluded.c, …>
```

- Table, every set column, and every conflict-target column pass
  `SqlIdent::parse_plain` / `parse_dotted` (fail-closed `Err` on any that don't —
  defence in depth over the Store gate). Conflict-target columns are validated
  identifiers, never interpolated free text; the `excluded.<col>` references reuse
  the *same* validated identifiers, so no second injection surface exists.
- `SET` includes only `SetField` columns that are NOT in the conflict-target set
  (a PK column is never in its own `DO UPDATE SET`). If, after excluding the
  target and the OmitFields, the SET list is empty, emit `ON CONFLICT (…) DO
  NOTHING` (a pure insert-if-absent — a legitimate, defined outcome, and what a
  key-only table wants) rather than an empty `SET` (a syntax error). This mirrors
  `db_update_fields`'s empty-SET guard (`db.rs:4017`).
- Routes through `db_format_sql` like every other statement; totality per the
  runtime deny-set — every error path returns `IpeResult::Err`, no panic/unwrap.

### SEAL — every acceptance path fails closed at ipe time

`Db.upsertFields` is a new kernel: it MUST be added to the `KernelDef` registry
(`src/compiler/kernels`), typed in the type-scheme table, lowered
(`src/compiler/lower`), and emitted (`src/compiler/backend/rust`) in lock-step,
or the build-time kernel tripwire fires. An unschemed-but-resolved kernel is the
exact exit-0-then-cargo-fail class the SEAL forbids; the tripwire makes the drift
a *build* failure, not a deferred cargo error.

## Principles evaluation (adversarial)

- **Security (1):** No new injection surface. Composite-PK columns and
  upsert conflict-target/SET columns are all validated identifiers
  (`validSqlIdentPlain` in `.ipe`, `SqlIdent::parse_*` in the runtime — defence
  in depth), never interpolated raw. The conflict target is *derived from the
  store's declared key*, so a caller cannot smuggle SQL through it. `excluded.<col>`
  reuses already-validated identifiers. Values bind positionally. SURVIVES.
- **Correctness (2):** Composite PK is deterministic DDL over caller-ordered
  columns. Upsert commits to ONE Ipê-defined semantics (update-in-place) realised
  by *identical* SQL on both supported backends — the `INSERT OR REPLACE` vs `ON
  CONFLICT` divergence is designed out, not exposed. `db.rs` needs no driver
  branch. SURVIVES (this was the refutation; it holds only because a single
  cross-backend statement exists — if a third backend without `ON CONFLICT …
  DO UPDATE` were added, this must be re-refuted then, not now).
- **Soundness (3):** Every new path is bounded and total. Composite PK with an
  unknown/duplicate/<2 column, or with `Serial`, is a typed `Err` at `createSql`
  time — never a deferred runtime blow-up. Upsert on a store with no declared
  PK/unique conflict target is a typed `Err`, not a runtime engine error.
  `PrimaryKeyDecl` makes a double-PK and a composite-1-column state
  unrepresentable. Empty-SET upsert degrades to `DO NOTHING`, no empty-SET syntax
  error. SURVIVES.
- **Efficiency (4):** Upsert is one round-trip replacing the read-then-insert
  `insertIfAbsent` pattern (`Db.ipe:130-139`, two round-trips). Composite PK is a
  pure schema fact, zero runtime cost. SURVIVES.
- **Ease (5):** Both builders compose with the existing `Draft` chain and read
  the same as `primaryKeyNamed` / `insert`. The conflict target is inferred from
  the declared key, so the common case needs no extra argument. Diagnostics name
  the fix (declare a PK; use `primaryKeyNamed` for a 1-column key). SURVIVES.
- **Readability (6):** Generated DDL reads as ordinary `PRIMARY KEY (a, b)`;
  the upsert reads as standard `ON CONFLICT … DO UPDATE SET`. The call site is
  `Store.compositePrimaryKeyNamed [...]` / `Store.upsert db store row`. SURVIVES.

## Task breakdown

Each task is independently implementable; all touch the SQL/security/soundness
surface, so each implementing lane needs a `security-soundness-guardian`.

1. **Composite PK in `Draft` DDL** — `PrimaryKeyDecl` field, `compositePrimaryKeyNamed`
   (+ optional accessor `compositePrimaryKey`), the one-PK-only build-time error.
2. **`createSql` / `migrations` render composite PK** — table-level constraint in
   `createSqlFromColumns`, with the five fail-closed validation arms; goldens.
3. **`Db.upsertFields` kernel + full SEAL** — runtime `db_upsert_fields`, registry
   / scheme / lowering / emit, `db_format_sql`-routed, `DO NOTHING` empty-SET
   degrade.
4. **`Store.upsert` API + conflict-target derivation** — the `.ipe` surface routing
   to the kernel, target derived from `PrimaryKeyDecl` / unique specs, fail-closed
   `Err` on no-target store.
5. **Prove-the-refusals tests** — every rejected path pinned: composite PK with an
   unknown column, a duplicate column, a 1-column list, a second PK, a `Serial`
   companion; upsert on a `NoPk`/no-unique store; identifier gates on
   conflict-target and SET columns.
6. Retire the hand-DDL + `INSERT OR REPLACE` in `tools/code-review/src/Lib/Db.ipe`
   / `Migrations.ipe` onto the new surface (the concrete driver + a real seal that
   the feature closes the gap).

## References

- `src/stdlib/Ipe/Db/Store.ipe` — Store surface, DDL renderer, insert/update.
- `src/runtime/rust/src/db.rs` — `db_insert_fields`, `db_update_fields`, `SqlIdent`.
- `src/runtime/rust/src/config.rs`, `config_postgres.rs` — per-driver templates,
  `db_format_sql`, `DB_USES_RETURNING_ID`.
- `src/compiler/kernels` — `KernelDef` registry (SEAL tripwire).
- `docs/adr/0003-security-render-and-data-access-invariants.md` — multi-driver
  compile-time selection; identifier validation; SqlFragment/Secret invariants.
- Concrete drivers: `tools/code-review/src/Lib/Db.ipe`, `Lib/Migrations.ipe`.

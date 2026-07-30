# Typed SQL: marking the raw-SQL escape hatch

## Problem

The untyped-holes audit flagged `Ipe.Db` for two stringly surfaces:

1. **Query text is a raw `String`.** `Db.exec` / `Db.query` / `Db.queryDecode`
   already bind their parameters positionally through sqlx, so *value*
   injection is closed. But `Db.execRaw : Db -> String -> Task Error Int` runs
   its `String` verbatim, and — as a plain peer of `exec` — nothing at the type
   or name level tells a caller it is the dangerous path. App code string-
   concatenates DDL/SQL into it. This is the residual SQL-injection surface.

2. **Result rows are a stringly `Dict String String`.** `Db.query` returns
   `List (Dict String String)`; `Db.getString` / `Db.getField` read a column by
   string key and yield `""` for a missing column, so a schema drift is a silent
   empty read rather than a decode error.

The typed answers to both already exist in the codebase:

- Parameterised binding: `Db.exec` / `Db.query` bind `List SqlValue` positionally.
- Typed WHERE fragments: `Db.findWhere` / `Db.deleteWhere` take an opaque
  `SqlFragment` built only through the `Ipe.Db.Sql.*` combinators — a naive
  string-concatenated WHERE clause is a type error, not a runtime risk. (This
  replaced the removed `Db.unsafeFindWhere`.)
- Typed row decode: `Db.queryDecode` + the `Ipe.Db.Decode.*` decoder
  combinators. A missing/mistyped column is a decode `Err`, not a silent `""`.

So the remaining work is not to *build* a typed surface — it is to stop the
raw-`String` escape hatch from being reachable as an **unmarked default**, and to
steer row reads onto the existing typed decoder.

## Fix

### 1. Rename `Db.execRaw` -> `Db.unsafeExecRaw` (security-load-bearing)

The verbatim-SQL path cannot be *removed*: `CREATE TABLE` / other DDL has no
parameterisable form, so an escape hatch must exist. Per PRINCIPLES #1 (Security
— "no injection (SQL, ...)", fail closed) and make-invalid-states-
unrepresentable, the fix is to make the hatch **lexically marked**: a caller can
no longer reach the raw path without typing `unsafe`.

This mirrors the established `unsafeFindWhere` precedent already in this
codebase. After the rename:

- `Db.unsafeExecRaw : Db -> String -> Task Error Int` — the *only* raw-SQL
  entry, and its name announces the danger. `grep unsafeExecRaw` enumerates
  every raw-SQL site for audit.
- There is **no** unmarked `execRaw` alias left behind — the injection-prone
  default is gone, not merely deprecated.

Only the Ipê-visible identifier and its pretty-printed form change. The internal
kernel variant (`DbExecRaw`) and runtime function (`db_exec_raw`) keep their
names; renaming the surface is what carries the security marker.

### 2. Row reads: typed decode is the documented path

`Db.queryDecode` + `Ipe.Db.Decode.*` is the typed row surface and already
exists. `Db.query` + `Db.getString`/`getField` remain for now (broad removal is
out of scope here and would break the un-migrated example apps), but the typed
decoder is the recommended read path. The stringly `query`/`getField` surface is
tracked as deferred breadth (follow-up issue) — it is not an injection surface
(rows are read-only output, not attacker-controlled SQL), only a schema-drift
ergonomics surface, so it is a Correctness/Readability follow-up, not a Security
one.

## Example migration (CI mirror)

`examples/sky/**` is the CI-committed raw Sky mirror and is never hand-edited;
the sweep materialises each upstream example and applies
`examples/sky/rename-map.tsv` via `sky-to-ipe-transform.py`. Upstream Sky calls
`Std.Db.execRaw`; a new map row rewrites that qualified value to
`Ipe.Db.unsafeExecRaw` (longest-prefix-wins binds it before the broad `Std.` ->
`Ipe.` row), so the mirrored DB apps keep building on the renamed surface
without any hand edit.

## Tests

- `db_exec` and every other DB golden exercise `unsafeExecRaw` for DDL and the
  typed `exec`/`queryDecode` path for data — SEAL round-trips them.
- A negative golden proves the old unmarked `execRaw` name no longer type-checks
  (unknown-qualified-member), so the injection-prone default is unreachable.

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

### 2. Row reads: typed decode is the default, stringly reads are marked

`Db.queryDecode` + `Ipe.Db.Decode.*` is the typed row surface: a missing,
NULL-where-required, or mistyped column is a decode `Err` — surfaced as a
`Task Error` the program must handle — never a silent `""`/`0`. It is the
unmarked default row-read path. The decoder describes the row shape inline, at
the call site, as a value — a single column is a primitive, a record is composed
with `map2`/`map3`/`map4`:

```
Db.queryDecode conn
    "SELECT full_name, qty FROM widgets"
    []
    (Db.Decode.map2
        (\name qty -> { name = name, qty = qty })
        (Db.Decode.string "full_name")
        (Db.Decode.int "qty"))
    -- : Task Error (List { name : String, qty : Int })
```

The stringly readers are renamed to lexically-marked `unsafe*` names, mirroring
`Db.unsafeExecRaw`:

- `Db.query` → `Db.unsafeQuery`
- `Db.getString` → `Db.unsafeGetString`
- `Db.getInt` → `Db.unsafeGetInt`
- `Db.getBool` → `Db.unsafeGetBool`
- `Db.getField` → `Db.unsafeGetField`

The untyped convenience still exists (a genuinely dynamic/ad-hoc read has no
typed replacement), but a caller can only reach it by typing `unsafe`, and
`grep unsafeGet` enumerates every drift-prone site for audit. This is the same
fail-closed story as `unsafeExecRaw` / `unsafeFindWhere`: the danger is
lexically marked, never an unmarked peer. As with `unsafeExecRaw`, only the
Ipê-visible spellings change — the `KernelFn` variants (`DbQuery`, `DbGetField`,
…) and runtime functions (`db_query_params`, `db_get_field`, …) keep their names.

This is not an injection surface (rows are read-only output, not
attacker-controlled SQL), only a schema-drift surface, so it is a
Correctness/Readability change, not a Security one.

#### How drift is caught — and what is not caught

The improvement over the stringly path is twofold:

- The column *set* the program depends on is the record type inferred from the
  decoder, so a field the program reads but the decoder never produces is a type
  error at `ipe` time — strictly better than the stringly path, where every
  field is a typeless `String` key.
- The column *name/type* vs the live table is not known to the compiler (it does
  not read the DB schema), so a drift between the decoder's column and the table
  is caught on the first row decoded, as a typed `Err`, deterministically —
  never a silent phantom value.

Three scoped limitations, so the guarantee is not overstated:

- **Range overflow of a parseable number is a lossy coercion, not an `Err`.**
  `Db.Decode.int` decodes an out-of-range numeric value through a *saturating*
  `f as i64` cast: soundness-safe (no overflow, no panic, no UB) but
  correctness-lossy (`1e30 → i64::MAX` as `Ok`, not `Err`). The totality
  guarantee (never panic) holds unconditionally; the *rejection* guarantee is
  scoped to a missing, NULL, or unparseable column. A future `checked`-int
  decode that returns `Err` on out-of-range is the natural follow-up.
- **`Db.Decode.nullable` tolerates an absent column.** It short-circuits an
  absent column to `Ok(Nothing)`, by design (it exists to tolerate SQL NULL), so
  it will not catch a *rename* of its own column the way a non-null primitive
  does. Prefer a non-null primitive where the column is genuinely required;
  reach for `nullable` only where SQL NULL is a real, expected value.
- **Empty result sets.** A pure column-name drift on a query that returns zero
  rows is not observed until a row appears (decoders run per row). This is
  inherent to any decoder that does not statically bind to the DDL, and is still
  strictly better than the stringly path (silent even *with* rows). Full
  compile-time schema binding is out of scope and noted as a possible future.

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
- `db_decode_drift` proves a decoder naming a renamed-away column short-circuits
  `queryDecode` to a caught `Task Error`, pinning that schema drift fails closed.
- `db_unsafe_row_read_marker` proves the old unmarked `Db.getField` / `Db.query`
  names no longer resolve through *either* the direct qualifier or the
  `Ffi.kernel "Db_getField"` string-alias route, while the marked `unsafe*`
  spellings resolve on both — so a smuggled alias cannot reopen the stringly read.

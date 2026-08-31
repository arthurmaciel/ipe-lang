# Store

A `Store a` is one typed database table whose schema, reads, and writes all derive
from a single `Codec a`. The codec is the one source of truth: its shape names the
columns, its encoder writes a row, its decoder reads one back. The safe surface
cannot express SQL injection — every identifier is validated once at construction,
and every value binds as a parameter.

## The mental model

Three ideas.

- **Identifiers are parsed, not validated.** `fromCodec` runs the table name and
  every derived column name through `validSqlIdent` and returns `Err` on the first
  one that isn't a plain identifier. A built `Store` therefore carries only
  accepted names, so nothing downstream re-checks — an injection attempt in a name
  never reaches SQL, because a `Store` holding a bad name has no representation.
- **Columns derive from the codec.** The column list, the `CREATE TABLE` DDL, the
  insert binds, and the row decode are one derivation from one codec. A field added
  or retyped changes exactly one place, so the schema and the round-trip cannot
  drift apart.
- **Access is deny-by-default.** `fromCodec` returns a `Draft a` — a table whose
  schema is known but whose access intent is not, and which has *no* read or write
  operation. Only `public` (an explicit, greppable, world-open declaration) or
  `secured` (policy-guarded) promotes a `Draft` to a queryable `Store`. A table
  nobody classified is unqueryable by construction — "which tables are world-open?"
  is a code search for `Store.public`, not an audit of every table.

## A worked example: the schema surface

The reads and writes are `Task`s that need a live database, but the *schema*
surface is pure — enough to show the security model without a connection. The
example under
[`examples/shapes/script/store-schema`](../../examples/shapes/script/store-schema/src/Main.ipe)
builds a `Store` from a record codec, reads back its columns, and shows identifier
validation.

The table is built from the codec, its primary key named by accessor (checked
against the row type at compile time), then classified world-open:

```ipe
users : Result Error (Store User)
users =
    Store.fromCodec "users" userCodec
        |> Result.map (\draft -> Store.primaryKey .id draft)
        |> Result.map Store.public
```

The primary-key builder reads its column from a `.field` accessor at compile time,
so it is applied directly (here inside a lambda for `Result.map`), never passed
point-free — the compiler rejects `Result.map (Store.primaryKey .id)` with a
diagnostic that tells you to wrap it.

Running it (`ipe run`):

```
columns: id, name, age
validSqlIdent "users" -> True ; validSqlIdent "users; drop table x" -> False
```

The column names come straight from the codec, and the injection attempt in a
table name is rejected before any SQL is built.

## The why

A `Draft` with no read or write operation, promoted only through a named
classification, is [make invalid states unrepresentable][principles]: an
unclassified table is not a runtime check you might forget, it is a value the read
and write functions won't accept. Validating identifiers once at construction and
binding every value as a parameter is [security][principles]'s fail-closed rule at
the SQL boundary — an identifier that isn't a plain name, or a value that would
have to be interpolated, never reaches the query text. And deriving columns,
inserts, and reads from one codec is [soundness][principles]: schema drift between
the write path and the read path is impossible when both are one derivation.

The escape hatch — a dynamic table with no record type, whose columns are named by
bare strings — lives in the capability-gated [`Ipe.Db.Store.Unsafe`](db-store-unsafe.md).

[principles]: ../../PRINCIPLES.md

## References

- **Per-symbol reference:** `ipe doc Ipe.Db.Store` — `fromCodec`, `public`,
  `secured`, `primaryKey`, `insert`, `all`, `get`, `findWhere`, `create`, and the
  query and join combinators, each with its signature.
- **Sibling guides:** [Codec](codec.md) — the single source of truth a store
  derives from. [Database codecs](db-codec.md) — that codec as a raw row and back.
  [Connection descriptors](dsn.md) — the typed `Dsn` that opens the connection.
  [The unsafe database surface](db-unsafe.md) — raw SQL and untyped reads.
  [Result](result.md) — the failure type construction returns.
- **Concepts:** [The parse-don't-validate idiom](../idioms/parse-dont-validate.md)
  — identifier validation as a construction-time parse.

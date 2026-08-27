# Ipe.Db typed join and projection query DSL

## Why

`Ipe.Db.Store` today describes one table per `Store a` and lowers a
single-table `Query a` (`where` / `order` / `limit`) to one SQL statement.
Combining two tables requires either two round trips (fetch parents, collect
keys, `inList` the children, group in memory) or a drop to `Ipe.Db.Unsafe` raw
SQL. Neither expresses a join as one typed, injection-safe statement.

This design adds composable, typed queries that **join** two stores and
**project** columns, lowering the whole pipeline to a single SQL `SELECT`. The
result: cross-table reads are one statement (no per-row fan-out), and the
projection selects exactly the columns asked for — while every identifier stays
validated and every value stays a bound parameter, exactly as the single-table
`Store` already guarantees.

The join key is that a query is a **value that lowers to SQL**, not a function
that runs over already-fetched rows. `map` / `select` over a joined query build
a SQL projection; they do not iterate a `List` in memory.

## Surface

A joined query is built by `join`, refined by `filter` / `orderBy` / `limit`,
projected by `select`, and run by `toList`:

```
join   : Store a -> (a -> k) -> Store b -> (b -> k) -> Joined a b
filter : (( Cols a, Cols b ) -> Pred) -> Joined a b -> Joined a b
orderBy: (( Cols a, Cols b ) -> Proj o) -> Order -> Joined a b -> Joined a b
select : (( Cols a, Cols b ) -> row) -> Joined a b -> Select row
toList : Db -> Select row -> Task Error (List row)
toMaybe: Db -> Select row -> Task Error (Maybe row)
count  : Db -> Joined a b -> Task Error Int
```

`join booksStore .authorId authorsStore .id` builds an inner join on
`books.author_id = authors.id`. The two key accessors name the join columns the
same way `Cond` already names a column (`eq .authorId value`); their result type
`k` must match, so a mistyped join key is a type error.

### The projected example, end to end

```
getAuthorNames : Db -> Task Error (List String)
getAuthorNames db =
    join booksStore .authorId authorsStore .id
        |> select (\( book, author ) -> author.name)
        |> toList db
```

lowers to one statement:

```
select author.name
from books as book, authors as author
where author.id = book.author_id
```

`book` and `author` inside the `select` lambda are **column records**
(`Cols Book`, `Cols Author`): a record shaped like the row type but with every
field wrapped in `Proj`. Because a field accessor is polymorphic over any record
carrying that field, `author.name` on a `Cols Author` yields a `Proj String` — a
symbolic column reference, not a fetched value. The lambda therefore returns a
projection, never a computed result, so it is lowerable by construction.

Filtering and multi-column projection compose the same way:

```
titlesByActiveAuthors : Db -> Task Error (List ( String, String ))
titlesByActiveAuthors db =
    join booksStore .authorId authorsStore .id
        |> filter (\( _, author ) -> is author.active True)
        |> select (\( book, author ) -> ( book.title, author.name ))
        |> toList db
-- select book.title, author.name
-- from books as book, authors as author
-- where author.id = book.author_id and author.active = $1
```

A `select` whose lambda returns a bare `Proj t` projects one column decoding to
`t`; a tuple or record of `Proj` fields projects those columns decoding to that
tuple or record.

## The projection sublanguage (`Proj`)

`Proj t` is a typed SQL expression yielding `t`. The **only** ways to obtain a
`Proj` are:

  * a column reference — a field of a `Cols a` record (`author.name : Proj String`);
  * a literal — `lit : t -> Proj t`, which binds as a parameter, never as text;
  * a lifted operator — a sanctioned, extensible set (`upper : Proj String ->
    Proj String`, arithmetic on `Proj` numbers, `coalesce`, comparisons that
    build a `Pred`, …), each lowering to a SQL function or operator.

Because a raw value cannot enter a `Proj` except through `lit` (a parameter),
and a `Cols` field is already `Proj`-typed, a projection that SQL cannot express
is **unrepresentable**: `String.toUpper author.name` does not type-check
(`author.name : Proj String`, not `String`), so the caller reaches for the
lifted `upper` or does the transform in ordinary code after `toList`. This is
the make-invalid-states-unrepresentable posture applied to the query boundary.

`Pred` is the joined-query predicate produced by lifted comparisons (`is`,
`eqCol`, `gtCol`, `and`, `or`, `not`) over `Proj` values — the two-table analogue
of the existing single-table `Cond`.

## Compiler support: deriving `Cols a`

`Cols a` is the one piece that needs compiler help. Ipê has no higher-kinded
types, so there is no generic mapping from a row record `a` to its all-`Proj`
version. Instead the compiler derives it from the store's `Codec`, which already
carries every field's name, type, and column name (its `Shape` is
`SRecord [(field, type)]`):

  * For each row type used as a join side, the compiler emits a `Cols`
    record whose fields mirror the row's fields, each typed `Proj <fieldType>`
    and carrying a `(tableAlias, columnName)` reference.
  * The `join` combinator binds each side's `Cols` value to that side's alias,
    then hands the pair to the `select` / `filter` / `orderBy` lambda.
  * A non-`SRecord` codec names no columns and is a build error (fail-closed),
    exactly as `fromCodec` already rejects it for the single-table store.

`Cols a` is a structural record, so it rides the existing row-polymorphic record
machinery; the new work is the derivation and its binding to an alias, not a new
kind of type.

## SQL lowering

`Joined a b` lowers to `FROM ta AS a0, tb AS a1 WHERE a0.k = a1.k [AND preds]`;
`select` sets the `SELECT` list from the projected `Proj` expressions; `orderBy`
/ `limit` / `offset` append. One statement, fully parameterized:

  * every table alias and column name passes `validSqlIdent` before it reaches
    SQL text (both join keys, every projected column, every predicate column);
  * every value binds as a parameter (`lit`, predicate right-hand sides) — never
    interpolated;
  * the projected row decodes through the `Proj` result types; schema drift (a
    missing column, a cell that is not the declared shape) is a typed `Err`.

## Security invariants (must hold — the store's whole argument, extended)

  * Identifiers are parsed, not validated: a `Joined` carries only accepted
    aliases and column names, because a join on a column absent from the codec,
    or a mis-shaped codec, is rejected at construction.
  * Values never touch SQL text: `lit` and predicate operands bind as parameters.
  * Un-lowerable projections are unrepresentable: `Proj` has no constructor that
    admits a raw runtime value except the parameterized `lit`.
  * Fail closed: anything the DSL cannot express safely is a compile or build
    error, never a silent `SELECT *` or a text-concatenated fragment.
  * Raw SQL remains behind `Ipe.Db.Unsafe`, which discloses that capability
    program-wide; this DSL adds no new way to reach raw SQL text.

## Increments

The surface above is the target. It lands in slices, each holding THE SEAL
(`ipe`-accept ⇒ emitted Rust builds) and the security invariants on its own:

  * **The join slice.** `join` → `Joined a b`, `filter` over the joined columns,
    and `toList` returning `( a, b )` tuples — both full rows decoded through the
    existing per-store codecs. One SQL join, no projection sublanguage yet
    (callers project in ordinary code after `toList`). This slice removes the
    per-row fan-out immediately and needs no `Cols` derivation, only the join
    lowering and its SQL generation. It is the fast first landing.
  * **The typed-projection slice.** `Cols a` derivation plus `select` / `Proj` /
    `lit`, giving the `author.name` surface and column pushdown (`SELECT` only
    the asked-for columns). This is the compiler piece.
  * **The lifted-operator slice.** `upper` / `coalesce` / arithmetic and richer
    join shapes (left/outer, three or more tables). Additive; deferrable.

## Divergence

The capability matches a bespoke database language's composable, typed,
single-statement joins with projection and no per-row fan-out. The projection is
expressed through typed column records and `Proj` constructors rather than an
arbitrary lambda lowered symbolically to SQL, because Ipê is a general-purpose,
eagerly-evaluated language: a projection lambda that ran on real rows would lose
the column identity, and interpreting its syntax symbolically is exactly the
implicit magic this project avoids. The typed surface buys the same one-statement
SQL and the same safety with one explicit rule — a projection is built from
column references and lifted operators — recorded as a sanctioned divergence.

## Testing and THE SEAL

  * Golden SQL for: inner join, join + filter, join + single-column projection,
    join + multi-column projection, join + order + limit — each asserting the
    exact parameterized statement and that every identifier was validated.
  * SEAL end to end: a joined, projected program `ipe`-accepts, its emitted Rust
    `cargo build`s, and the round trip returns the joined rows.
  * Injection: a table/column identifier that fails `validSqlIdent` is a build
    error; every value appears as a bound parameter, never in the SQL text.
  * Fail-closed: a join key absent from a codec, a non-`SRecord` codec, or a
    projection that cannot be lowered is a compile or build error.

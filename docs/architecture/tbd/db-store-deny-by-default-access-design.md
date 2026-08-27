# Deny-by-default access for Ipe.Db.Store

## Why

`Ipe.Db.Store` carries a real row-level access model — a `Policy` algebra
(`publicRead`, `ownerColumn`, `immutable`, `andPolicy`) attached through
`secured`, with authenticated operations (`allAs` / `getAs`) that take a
`Principal`. But attaching a policy is **opt-in**: a `Store` built with
`fromCodec` is queryable immediately, and `all` / `get` / `findWhere` /
`insert` operate on it with no policy at all. Protection applies only if the
developer remembers to build a `Secured` store and route through the
authenticated operations.

The failure mode is silent: a table holding sensitive rows, created with
`fromCodec` and queried with `all`, is wide open, and nothing in the type marks
it as unreviewed. Security is the project's first principle, and "did anyone
remember to secure this table?" is exactly the kind of question the type system
should answer, not the reviewer.

This design makes an access decision **mandatory**: a store is not queryable
until its access intent is declared. A public table declares that it is public
— an explicit, greppable, auditable choice — and a store that no one has
classified cannot be read or written at all. The unsecured-by-accident state
becomes unrepresentable.

## The type state

`fromCodec` stops returning a queryable store. It returns a **draft** — a store
whose schema is known but whose access intent is not yet declared — and the
query and mutation functions accept only a **classified** store:

```
fromCodec : String -> Codec a -> Draft a

-- declare access intent; each returns a queryable Store a
public   : Draft a -> Store a          -- an explicit, auditable public table
secured  : Policy -> Draft a -> Store a -- rows guarded by the policy algebra

all      : Db -> Store a -> Task Error (List a)
get      : Db -> Store a -> SqlValue -> Task Error (Maybe a)
insert   : Db -> Store a -> a -> Task Error Int
-- … every existing read/write keeps its shape, but over Store a
```

A `Draft a` has no read or write operation. The only way to obtain a `Store a`
is `public` or `secured`, so every queryable store has passed through a
deliberate classification. `public` is a first-class declaration, not the
absence of one — a code search for `public` enumerates exactly the tables a
reviewer must confirm are meant to be world-readable.

The column-fact builders (`primaryKey` / `serial` / `unique` / `defaultNow` /
`index`) apply to the `Draft`, before classification, since they describe the
schema, not the access policy:

```
users : Store User
users =
    fromCodec "users" userCodec
        |> serial .id
        |> unique .email
        |> secured (ownerColumn .ownerId)
```

```
countries : Store Country
countries =
    fromCodec "countries" countryCodec
        |> public
```

## Security invariant

  * No read or write function accepts a `Draft a`; every one takes a `Store a`,
    and a `Store a` exists only via `public` or `secured`. An unclassified
    (therefore unreviewed) table is unqueryable by construction.
  * `public` is an explicit term, so "which tables are world-readable" is a
    grep, not an audit of every `fromCodec` site.
  * `secured`'s existing guarantee is unchanged and now universal: it
    re-validates every policy column against the codec-derived columns
    (fail-closed on a policy naming a column the codec does not have), and the
    authenticated operations still take a `Principal`.
  * Nothing here weakens the policy algebra; it only removes the path that
    skipped it.

## What stays

`Policy`, `ownerColumn`, `publicRead`, `immutable`, `andPolicy`, `secured`,
`allAs`, `getAs`, and the `Secured` row-security semantics are unchanged. The
only structural change is that classification is now a required step between
`fromCodec` and the first query, and `public` names the previously-implicit
open case.

## Migration

This is a breaking change to the `Store` construction surface: every store that
today goes straight from `fromCodec` to `all` must add one classification step
(`|> public` or `|> secured <policy>`). The migration is mechanical and local —
one line per store — and the diff is a useful audit in itself, because it forces
each existing table to state whether it was meant to be public or guarded. The
first-party examples and any stdlib self-use migrate in the same change.

## Increments

  * **The type state.** Introduce `Draft a`, move read/write onto `Store a`, and
    add `public` / keep `secured` as the two classifiers. Column-fact builders
    move to `Draft a -> Draft a`. Migrate first-party callers. This is the whole
    security win and is self-contained.
  * **Diagnostics.** A clear compile error when a `Draft a` reaches a query
    function — one that names the missing step ("classify this store with
    `public` or `secured` before querying") rather than a bare type mismatch, so
    the mandatory-classification rule teaches itself.

## Testing and THE SEAL

  * A program that queries a classified store `ipe`-accepts and its emitted Rust
    `cargo build`s (THE SEAL); a program that queries a `Draft` is refused with
    the classification diagnostic.
  * `public` and `secured` stores both round-trip; `secured` still fail-closes on
    a policy column absent from the codec.
  * Golden diagnostics for the draft-reaching-a-query rejection.

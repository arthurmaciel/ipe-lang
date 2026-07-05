# Task combinators — the three-axis taxonomy

> Coordinator-ratified design (2026-07-05). Governs the Task composition
> surface; `parallel2..5` is a post-parity addition (#131), `do` is Idea 7
> (deferred post-parity). Everything else is shipped.

Three orthogonal axes classify every way of combining Tasks:

1. **Value dependence** — does a later computation need an earlier one's
   *result value*?
2. **Execution dependence** — do the effects run in order (sequential) or
   concurrently (parallel)?
3. **Arity shape** — heterogeneous (different types, fixed arity) or
   homogeneous (`List` of one type, any length)?

**Key law: dependent values ⇒ sequential execution.** A step that needs
another's output cannot run alongside it. So the "dependent + parallel" cells
are *logically empty*, and the table has six rows, not eight.

| Combinator | Values | Execution | Shape | Signature |
|---|---|---|---|---|
| `do` (Idea 7) / `Task.andThen` | **dependent** | sequential (forced) | heterogeneous | `(a -> Task e b) -> Task e a -> Task e b` |
| `Task.map2..5` | independent | sequential | heterogeneous | `(a -> b -> c) -> Task e a -> Task e b -> Task e c` |
| `Task.parallel2..5` (#131) | independent | **parallel** | heterogeneous | `(a -> b -> c) -> Task e a -> Task e b -> Task e c` |
| `Task.sequence` | independent | sequential | homogeneous | `List (Task e a) -> Task e (List a)` |
| `Task.parallel` | independent | **parallel** | homogeneous | `List (Task e a) -> Task e (List a)` |

(`do` is syntactic sugar over `andThen` — same row. A dependent chain of
same-typed steps is also `do`/`andThen`; `sequence` is the *independent*
special case.)

## Semantics per row

- **`do` / `andThen`** — each step may use prior results; a failing step
  stops the chain; later steps are **never started**.
- **`map2..5`** — Elm-compatible: runs in argument order; an early `Err`
  means later tasks are **never started** (their effects don't fire). Use
  when the *effects'* order matters even though the *values* are independent
  (e.g. two appends to one log).
- **`parallel2..5`** — spawn all, await all; latency = max, not sum.
  Fail-fast with **sibling abort** (same discipline as `Task.parallel`,
  #65); when several fail, the **leftmost** error is reported (positional —
  deterministic across runs, chronological would not be). Signature mirrors
  `map2..5` exactly — the families differ in the execution bit only; the
  tuple form is derivable (`parallel2 Tuple.pair ta tb`).
- **`sequence` / `parallel`** — the homogeneous counterparts of the same
  two rows; `parallel` carries the #65 sibling-abort semantics.

## Choosing

| You have… | Use |
|---|---|
| steps that feed each other | `do` / `andThen` |
| independent steps, effect order matters | `map2..5` (or `do`) |
| independent steps, want max-latency win | `parallel2..5` |
| a list of same-typed tasks, ordered effects | `sequence` |
| a list of same-typed tasks, concurrent | `parallel` |

## Contract notes

- **Data independence ≠ effect independence.** The compiler cannot verify
  that two tasks' *effects* don't interact through the outside world. The
  `parallel*` contract is explicit: *argument order is not effect order* —
  if ordering matters, use the sequential row.
- **No rollback.** Fail-fast abort skips/cancels *future or in-flight*
  siblings; effects already fired stay fired (see `Db.withTransaction` for
  all-or-nothing).
- `Task.run` / `Task.perform` (runners, not combinators) are slated for
  removal post-parity (#128) — Tasks are run by the boundary (`main`,
  handler return, `Cmd.perform`), never by user code.

## Examples — the disjoint point of each row

Each row gets two real-use-case examples; each pair shows exactly why its
row exists and why the neighbouring row would be wrong.

### `do` / `andThen` — the value flows

```elm
-- 1. Auth: the profile fetch NEEDS the token from login.
do
    token   <- Auth.login creds
    profile <- Http.get ("/api/me?token=" ++ token)
    profile

-- 2. Boot: the DB connect NEEDS the URL parsed from config.
do
    cfg  <- File.readFile "app.toml"
    conn <- Db.connect (dbUrlFrom cfg)
    conn
```

*Why not `map2`:* impossible — the second task cannot even be constructed
without the first one's result. Value dependence is the boundary.

### `Task.map2..5` — independent values, but the EFFECTS must stay ordered

```elm
-- 1. Audit-then-act: the charge must not fire before the audit line is
--    durably written; the charge does not use the log's result.
Task.map2 (\_ receipt -> receipt)
    (Log.infoWith "charge.start" [ "order", orderId ])
    (Payments.charge card amount)

-- 2. FK ordering: settings row must come after the user row exists;
--    both statements are fully known up front.
Task.map2 (\_ _ -> uid)
    (Db.exec conn "INSERT INTO users (id, name) VALUES (?, ?)" [ SqlString uid, SqlString name ])
    (Db.exec conn "INSERT INTO settings (user_id) VALUES (?)" [ SqlString uid ])
```

*Why not `do`:* no value flows between the steps — `do` would work but
states a dependency that doesn't exist. *Why not `parallel2`:* the
**effects** interact through the outside world (log-before-charge, FK
order) — concurrency would race them.

### `Task.parallel2..5` — independent values AND independent effects

```elm
-- 1. Dashboard first paint: profile and notifications come from different
--    services; total latency = max, not sum.
Task.parallel2 (\profile notes -> { profile = profile, notifications = notes })
    (Http.get "/api/me")
    (Http.get "/api/notifications")

-- 2. Price comparison: quote two suppliers simultaneously.
Task.parallel2 (\a b -> ( a, b ))
    (Http.get supplierA)
    (Http.get supplierB)
```

*Why not `map2`:* nothing orders these effects — sequential wastes a full
round-trip. *Why not `parallel` (list):* the two results have different
types; a `List` can't hold them.

### `Task.sequence` — a same-typed batch whose ORDER matters

```elm
-- 1. DB migrations: same type (each `Task Error ()`), strictly in order,
--    length varies per release.
Task.sequence (List.map runMigration pendingMigrations)

-- 2. Paginated import: page N+1's request must not fire until page N is
--    stored (server-side cursor advances on read).
Task.sequence (List.map importPage pageNumbers)
```

*Why not `parallel`:* order is the whole point (migration 3 assumes 2 ran;
the cursor moves). *Why not `map2..5`:* the batch is list-shaped and
dynamic-length, not a fixed small arity.

### `Task.parallel` — a same-typed batch, order-free

```elm
-- 1. Fan-out fetch: 50 product thumbnails, no ordering constraint,
--    latency = slowest single fetch.
Task.parallel (List.map fetchThumbnail productIds)

-- 2. Health checks: probe every service endpoint at once; first failure
--    aborts the remaining probes (#65 sibling-abort).
Task.parallel (List.map healthCheck serviceUrls)
```

*Why not `sequence`:* no effect depends on another — serial would multiply
latency by N. *Why not `parallel2..5`:* one element type, dynamic length —
the list shape is the natural fit.

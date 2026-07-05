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
- **`parallelDo`** — spawn all binds, await all; latency = max, not sum.
  Fail-fast with **sibling abort** (same discipline as `Task.parallel`,
  #65); when several fail, the **leftmost** (first-written) error is
  reported — deterministic. Binds are outer-scoped (independence
  unrepresentable); tail is pure and auto-wrapped.
- **`sequence` / `parallel`** — the homogeneous counterparts of the same
  two rows; `parallel` carries the #65 sibling-abort semantics.

## Choosing

| You have… | Use |
|---|---|
| steps that feed each other | `do` / `andThen` |
| independent steps, effect order matters | `map2..5` (or `do`) |
| independent steps, want max-latency win | `parallelDo` |
| a list of same-typed tasks, ordered effects | `sequence` |
| a list of same-typed tasks, concurrent | `parallel` |

## Considered and rejected: a "parallel do"

A `do`-like block with concurrent semantics was considered (2026-07-05) in
two forms, both rejected:

- **Implicit** (Haskell `ApplicativeDo` / Haxl style — the compiler
  analyses binds and runs data-independent ones concurrently). Rejected as
  UNSOUND under this table's own contract: *data independence ≠ effect
  independence*, and the compiler cannot see effects that interact through
  the outside world. Implicit parallelisation would silently race
  log-before-charge-shaped code that reads as sequential. Effect order in a
  `do` block must be exactly reading order, always.
- **Explicit — ACCEPTED as `parallelDo` (revised 2026-07-05),** replacing
  the `parallel2..5` function family. The construct is sound because
  independence is a SCOPING rule, not a lint: bind RHSs elaborate in the
  *outer* scope only, so binds are not in scope for each other —
  referencing a sibling bind is an ordinary unknown-name error. Dependency
  is unrepresentable, not detected. All binds/bare lines must be
  Task-typed and run concurrently (spawn all / await all; fail-fast +
  sibling abort, leftmost error — the `Task.parallel` discipline); the
  **tail must be pure** (auto-`Task.succeed`-wrapped) — a Task tail would
  smuggle a sequential step into the block; nest `parallelDo` inside `do`
  for dependent follow-up. Advantages over `parallel2..5`: named binds
  instead of positional lambda args, no arity cliff, and it rides the
  Idea-7 block machinery (same parser shape, new keyword + scoping +
  spawn/join desugar).

```elm
do
    combined <- parallelDo
        profile <- Http.get "/api/me"
        notes   <- Http.get "/api/notifications"
        { profile = profile, notifications = notes }
    render combined

parallelDo          -- hardcoded homogeneous, discard results
    fetchThumbnail1
    fetchThumbnail2
    ()
```

The rule that survives: **`do` is the sequential column, spelled top-to-
bottom; concurrency is always explicit — the `parallelDo` keyword or the
list combinator `Task.parallel`.** One glance tells you the execution
model.

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

With `andThen` alone, dependent chains nest into pyramids:

```elm
-- 1. Auth: the profile fetch NEEDS the token from login.
Auth.login creds
    |> Task.andThen (\token ->
        Http.get ("/api/me?token=" ++ token))

-- 2. Boot: the DB connect NEEDS the URL parsed from config.
File.readFile "app.toml"
    |> Task.andThen (\cfg ->
        Db.connect (dbUrlFrom cfg))
```

`do` is sugar over exactly these — same semantics, flat instead of nested:

```elm
do
    token   <- Auth.login creds
    profile <- Http.get ("/api/me?token=" ++ token)
    profile

do
    cfg  <- File.readFile "app.toml"
    conn <- Db.connect (dbUrlFrom cfg)
    conn
```

Two steps barely differ; at three or more the pyramid grows one lambda +
indent level per step while `do` stays flat — that gap is `do`'s whole
reason to exist.

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

`do` expresses the same programs — a discarded bind is exactly
"independent value, ordered effect":

```elm
do
    Log.infoWith "charge.start" [ "order", orderId ]   -- bare line: run + discard
    receipt <- Payments.charge card amount
    receipt

do
    Db.exec conn "INSERT INTO users (id, name) VALUES (?, ?)" [ SqlString uid, SqlString name ]
    Db.exec conn "INSERT INTO settings (user_id) VALUES (?)" [ SqlString uid ]
    uid
```

(`_ <- task` is the equivalent explicit form of the bare discard line.)

So `map2..5` is not the *only* home of this row — `do` subsumes it for
Task. `map2..5` earns its keep as: (a) an **expression** (composes in
pipelines, needs no statement block), (b) the **uniform cross-type family**
(`Maybe.map2` / `Result.map2` / `Decoder.map2` have no `do` — `do` is
Task-only), and (c) the terser spelling at arity 2-3.
*Why not `parallel2`:* the **effects** interact through the outside world
(log-before-charge, FK order) — concurrency would race them.

### `parallelDo` — independent values AND independent effects

```elm
-- 1. Dashboard first paint: profile and notifications come from different
--    services; total latency = max, not sum.
parallelDo
    profile <- Http.get "/api/me"
    notes   <- Http.get "/api/notifications"
    { profile = profile, notifications = notes }

-- 2. Price comparison: quote two suppliers simultaneously.
parallelDo
    a <- Http.get supplierA
    b <- Http.get supplierB
    ( a, b )
```

*Why not `map2`/`do`:* nothing orders these effects — sequential wastes a
full round-trip. *Why not `parallel` (list):* the two results have
different types; a `List` can't hold them. Binds cannot reference each
other (outer-scoped) — writing `notes`' request in terms of `profile` is an
unknown-name error, which is the construct's soundness.

### `Task.sequence` — a same-typed batch whose ORDER matters

```elm
-- 1. Three known migrations, strictly in order.
Task.sequence [ migrationA, migrationB, migrationC ]

-- 2. DB migrations at scale: same type (each `Task Error ()`), strictly in
--    order, length varies per release.
Task.sequence (List.map runMigration pendingMigrations)

-- 3. Paginated import: page N+1's request must not fire until page N is
--    stored (server-side cursor advances on read).
Task.sequence (List.map importPage pageNumbers)
```

`do` expresses the FIXED-length case directly:

```elm
do
    a <- migrationA
    b <- migrationB
    c <- migrationC
    [ a, b, c ]
```

What `do` cannot do is a **runtime-built** list — `pendingMigrations` has a
length no `do` block can spell. Dynamic length is `sequence`'s unique
capability; for a known handful of tasks, `do` and `sequence` are taste.

*Why not `parallel`:* order is the whole point (migration 3 assumes 2 ran;
the cursor moves). *Why not `map2..5`:* the batch is list-shaped and
dynamic-length, not a fixed small arity.

### `Task.parallel` — a same-typed batch, order-free

```elm
-- 1. Three known fetches, concurrently.
Task.parallel [ fetchThumbnail1, fetchThumbnail2, fetchThumbnail3 ]

-- 2. Fan-out fetch at scale: 50 product thumbnails, no ordering constraint,
--    latency = slowest single fetch.
Task.parallel (List.map fetchThumbnail productIds)

-- 3. Health checks: probe every service endpoint at once; first failure
--    aborts the remaining probes (#65 sibling-abort).
Task.parallel (List.map healthCheck serviceUrls)
```

*Why not `sequence`:* no effect depends on another — serial would multiply
latency by N. *Why not `parallelDo`:* fine for a known handful (see its section), but a
runtime-built list needs the combinator.

# 12-skyvote — TEA rearchitecture (no synchronous force)

A hand-written Ipê port of the upstream feature-voting board, rebuilt around the
async-DB effect model. The upstream forces every database read/write with
`Task.run` — a module-level synchronous `dbConn`, synchronous `exec`/`query`
wrappers in `Lib/Db`, and a synchronous clock read in `Lib/Comments`. `Task.run`
is removed from the Ipê surface (IPE-N0036) and Ipê TEA has no synchronous force,
so the token rewrite cannot express the port: it is a whole-example
rearchitecture, which lives here as an `ipe-overrides/` tree (the same mechanism
as `13-skyshop` and `27-multi-session-chat`).

What changed versus the raw upstream in `../../original/12-skyvote/`:

- The `Model` holds plain data only. An `Ipe.Web` Model is persisted to the
  session store between requests, so it may not carry the opaque `Db` handle
  (IPE-L0120). Each database effect opens its own pooled connection inside its
  Task chain — `Db.connect |> Task.andThen ensureSchema |> Task.andThen <op>`,
  wrapped once as `Lib.Db.withConn` — and only the settled DATA reaches the
  Model.
- Every `Lib` read/write (`Ideas`, `Comments`, `Auth`) returns `Task Error a`.
  Each `update` handler that used to consume a synchronous `Result` now
  dispatches a `Task.attempt` command and folds the settled `Result` into the
  Model in a dedicated response branch (`IdeasLoaded`, `DetailLoaded`,
  `RoadmapLoaded`, `VoteToggled`, `CommentPosted`, `IdeaCreated`, `SignedIn`,
  `SignedUp`).
- A route-only navigation re-renders the view without running `update`, so each
  page's data is loaded by the `Msg` that navigates there and refreshed by a
  page-scoped `Time.every` tick. The board's initial load is dispatched from
  `init`.
- The roadmap and single-idea detail reads are composed with `Task.map2` so a
  page's whole dataset settles under one response `Msg`. The roadmap's four
  status buckets live in the Model; its `view` performs no database read.
- The `Page` union is enumerated explicitly in `subscriptions`, `view`, and the
  refresh dispatcher — a catch-all `_` over a closed union is rejected
  (IPE-T0018).
- The `view` returns `Element Msg` (the Web shape's view type); the typed
  `Ipe.Html` / `Ipe.Css` page tree is lifted through the single `Ui.html` bridge
  node.
- The clock read for a comment's id now runs inside the persist Task
  (`Time.now |> Task.andThen …`), never forced before the effect.

Security improvement over the upstream: the board search and category filter are
bound query parameters, never string-interpolated into the SQL, so a search term
or category can carry no injection (the upstream concatenated them straight into
the `WHERE` clause). The password path keeps the upstream's constant response —
a wrong password and an unknown email both return the same message, revealing
nothing to an enumerator.

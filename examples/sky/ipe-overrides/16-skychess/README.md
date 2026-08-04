# 16-skychess — TEA rearchitecture (no synchronous force)

A hand-written Ipê port of the upstream chess game, rebuilt around the
async-DB effect model. White plays a human; Black is a 2-ply minimax
opponent. Games and their moves are persisted to SQLite so a game can be
reloaded and replayed.

The upstream forces every database read/write with `Task.run` — a
module-level synchronous `dbConn` plus synchronous `exec`/`query` wrappers
in `Lib/Games`, consumed mid-`update`. `Task.run` is removed from the Ipê
surface (IPE-N0036) and Ipê TEA has no synchronous force, so the token
rewrite cannot express the port: it is a whole-example rearchitecture, which
lives here as an `ipe-overrides/` tree (the same mechanism as `13-skyshop`,
`12-skyvote`, and `27-multi-session-chat`).

What changed versus the raw upstream in `../../original/16-skychess/`:

- The chess engine (`Chess/*`, `Ui/*`, `Page/Game`) is pure and carries over
  unchanged; only the database layer and its consumers are rearchitected.
- `Lib/Games` is now an async API: every operation returns `Task Error a`
  and opens its own pooled connection through `Lib.Db.withConn`
  (`Db.connect |> Lib.Schema.ensureSchema |> op`). The connection never
  escapes into the session-persisted Model, which holds plain data only
  (IPE-L0120). `Lib/Schema` owns the `CREATE TABLE` DDL; `Lib/Db` provides
  the `withConn` acquire-per-effect helper and a `field` row reader.
- `init` no longer forces a synchronous schema init; the first database
  effect ensures the schema lazily via `withConn`.
- Each database read is dispatched from `update` via `Task.attempt` and its
  settled `Result` folds into the model in a dedicated branch: `GamesLoaded`
  (home list, refreshed on name entry, back-to-home, and after a delete),
  `GameCreated` (new game), `GameLoaded` (a `Task.map2`-composed game-info +
  moves read, replayed onto the initial board), and `GameDeleted`.
- The board transition stays pure and is applied synchronously in the same
  tick; the move's durable write is a separate fire-and-forget command
  (`Task.attempt MoveSaved`), and a game-ending status write is composed
  with it via `Cmd.batch`. Persistence failures surface through the
  `MoveSaved` / `StatusSaved` response branches rather than being swallowed.
- The `view` returns `Ui.Element Msg` (the Web shape's view type); the app's
  typed `Ipe.Html` tree is reached through the single `Ui.html` bridge node.

Every user-supplied value reaches SQL as a bound `?` parameter — never
string-interpolated — so no game name, id, or move can carry an injection.

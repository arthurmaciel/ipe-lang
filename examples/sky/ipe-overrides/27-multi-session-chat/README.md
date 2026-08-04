# 27-multi-session-chat — TEA rearchitecture (no synchronous force)

A hand-written Ipê port of the upstream pub/sub chatroom, rebuilt around the
async-DB effect model. The upstream forces every database read/write with
`Task.run` — a module-level synchronous `dbConn`, synchronous `initSchema`,
`loadRoomHistory`, and a synchronous clock read in `nowString`. `Task.run` is
removed from the Ipê surface (IPE-N0036) and Ipê TEA has no synchronous force,
so the token rewrite cannot express the port: it is a whole-example
rearchitecture, which lives here as an `ipe-overrides/` tree (the same
mechanism as `13-skyshop`).

What changed versus the raw upstream in `../../original/27-multi-session-chat/`:

- The `Model` holds plain data only. A `Ipe.Web` Model is persisted to the
  session store between requests, so it may not carry the opaque `Db` handle
  (IPE-L0120). Each database effect opens its own pooled connection inside its
  Task chain (`Db.connect |> Task.andThen ensureSchema |> Task.andThen <op>`)
  and the settled data — not the connection — reaches the Model.
- `init` pre-loads a direct-linked room's history through a `Task.attempt`
  command reported by a new `Ready (Result Error (List ChatMessage))` message.
- `JoinRoom` re-reads history through a `Task.attempt` command reported by a
  new `HistoryLoaded` message instead of a synchronous `loadRoomHistory`.
- `SendMessage` reads the clock and persists inside one Task chain
  (`Time.now |> Task.andThen persist`) reported by `PersistResult`; the
  timestamp is acquired inside the Task, never forced before it.
- The topic is a typed `Topic (Dict String String)` built with `PubSub.topic`,
  shared between `Cmd.publish` and `Sub.subscribeTopic` so the compiler proves
  publisher and subscriber agree on the payload shape.
- The `view` returns `Element Msg` (the Web shape's view type); the app's raw
  HTML and stylesheet are reached through the single `Ui.html` bridge node.

The durability contract is preserved: the database is the source of truth
(persisted with the real clock read), the broadcast is a best-effort
low-latency hint, and a fresh `/chat/<room>` load reads the persisted history.

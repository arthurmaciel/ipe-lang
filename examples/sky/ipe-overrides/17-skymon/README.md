# 17-skymon — TEA rearchitecture (no synchronous force)

A hand-written Ipê port of the upstream uptime + metrics monitor, rebuilt
around the async-DB effect model. SkyMon watches HTTP endpoints and custom
metrics, records their history to SQLite, and fires alerts when a metric
crosses a threshold.

The upstream forces every database read/write and every outbound HTTP probe
with `Task.run` — a module-level synchronous `dbConn`, synchronous `exec` /
`query` wrappers in `Lib/Database`, and synchronous `Http.get` inside the
monitor/metric collectors, all consumed mid-`update` and inside `view`
helpers. `Task.run` is removed from the Ipê surface (IPE-N0036) and Ipê TEA
has no synchronous force, so the token rewrite cannot express the port: it is
a whole-example rearchitecture, which lives here as an `ipe-overrides/` tree
(the same mechanism as `13-skyshop`, `12-skyvote`, `16-skychess`, and
`27-multi-session-chat`).

What changed versus the raw upstream in `../../original/17-skymon/`:

- **Async DB + HTTP layer.** `Lib/Monitors`, `Lib/Metrics`, and `Lib/Alerts`
  are now async APIs: every operation returns `Task Error a` and opens its
  own pooled connection through `Lib.Db.withConn` (`Db.connect |>
  Lib.Schema.ensureSchema |> op`). The connection never escapes into the
  session-persisted Model, which holds plain data only (IPE-L0120): the
  settled monitor / metric / alert records plus the raw history rows.
  `Lib/Schema` owns the `CREATE TABLE` DDL; `Lib/Db` provides the `withConn`
  acquire-per-effect helper plus `field` / `intField` row readers.
- **Effect dispatch.** Each read is dispatched from `update` via
  `Task.attempt` and folds its settled `Result` into the model in a dedicated
  branch: `BoardLoaded` (the three collections, `Task.map2`-composed),
  `MonitorDetailLoaded` / `MetricDetailLoaded` (per-id history, loaded by the
  navigating `Msg`), `AlertHistoryLoaded`, and the `MonitorAdded` /
  `MonitorRemoved` / `MetricAdded` / `MetricRemoved` / `AlertAdded` /
  `AlertRemoved` write acknowledgements.
- **Check cycle.** The periodic tick composes one `ChecksRan` Task: probe
  every monitor over HTTP and record its status, collect every metric
  (`http` / `api` over HTTP, `sql` through the read-only SafeQuery gate) and
  record its value, then evaluate every alert against the freshly collected
  values, firing webhook / slack notifications and recording each fire.
- **Detail-page id ownership.** A detail page records the viewed id on the
  Model when navigating and reads it back in the response branch, rather than
  threading the same id through both the effect argument and its response
  tag — which would move-then-reuse the value.
- **Route-only nav.** The Web runtime's nav links (`ipe-nav`) re-render the
  view without running `update`, so a page's data is loaded by the `Msg` that
  navigates to it and refreshed by the page-scoped tick, never lazily in
  `view`.
- **View.** `view` returns `Ui.Element Msg`; the app's typed `Ipe.Html` tree
  is reached through the single `Ui.html` bridge node.

Divergences from the raw upstream, recorded honestly:

- **Auth.** The upstream's `Lib/Auth` (GitHub OAuth + a `sessions` table) is
  dead code — `Main.sky` never imports it and instead uses an inline
  username/password demo login (`admin` / `admin123`). This port keeps the
  same inline demo login and omits the unused `Lib/Auth` module.
- **External SQL metrics.** The upstream `sql` metric connected to an
  arbitrary external `postgres` DSN via `Db.open driver dsn`. Ipê's
  `Db.connect ()` opens only the app's configured database, so the DSN in a
  `sql` metric's config is informational and the query runs (read-only,
  through the SafeQuery SELECT gate) against the app's own connection.

Every user-supplied value reaches SQL as a bound `?` parameter — never
string-interpolated — so no monitor name, URL, metric config, or alert
threshold can carry an injection. The metric collector additionally runs
user-authored SQL only through the SafeQuery read-only gate, which rejects
any non-SELECT statement.

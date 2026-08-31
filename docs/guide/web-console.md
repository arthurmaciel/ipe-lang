# Console authentication

`Ipe.Web.Console` provides the `Identity` type and its builders for the optional
`consoleAuth` field on a `Web.app` config. When the framework gates the embedded
console in app-mode, it runs your callback per request *before* mounting console
routes, so the console inherits the app's own auth surface — no second token to
provision.

## The mental model

Three knots.

- **`consoleAuth` is a per-request gate, run before console routes mount.** The
  callback has shape `Request -> Task Error (Maybe Identity)`. `Nothing` denies
  the request — a 403 plus a structured `console.auth.denied` audit log entry.
  `Just identity` lets it continue; the identity rides the console's session
  cookie and is attached to subsequent telemetry for audit. The gate runs on
  every request, so there is no window where the console is reachable un-gated.
- **`Identity` is built through builders, not a record literal.**
  `defaultIdentity subject` starts an identity with an empty email and no claims;
  `withEmail`, `withClaim`, and `withClaims` layer on the rest. Building through
  the builder chain keeps call sites source-compatible as optional fields are
  added later — the same discipline the other typed records in the standard
  library follow.
- **It reuses the app's existing auth — one identity surface.** The point is to
  thread an app's SSO or multi-tenant session middleware straight into
  `consoleAuth`, so the console is gated by the same check the rest of the app
  uses. `subject` is the stable identifier, `email` is surfaced separately so the
  audit log line is human-scannable, and `claims` carries extra attributes a
  role-based-access layer consults.

## A worked example: gating the console

The example under
[`examples/shapes/web/console-auth`](../../examples/shapes/web/console-auth/src/Main.ipe)
is a `Web.app` that supplies a `consoleAuth` callback building an identity with
the `Ipe.Web.Console` builders.

The callback is a `Task` returning `Maybe Identity` — a real app reads a session
from the request; returning `Nothing` denies with a 403 and an audit log:

```ipe
identify : Request -> Task Error (Maybe Identity)
identify _req =
    Task.succeed
        (Just
            (Console.defaultIdentity "user-42"
                |> Console.withEmail "alice@example.com"
                |> Console.withClaim "role" "admin"
            )
        )
```

It is wired through the optional `consoleAuth` field on `Web.app`:

```ipe
main =
    Web.app
        { init = init, update = update, view = view
        , subscriptions = subscriptions
        , routes = [], notFound = Ignored
        , consoleAuth = identify
        }
```

Building it (`ipe build`) compiles the app; when the console is gated in app-mode,
the callback runs per request and only an authorised identity reaches it.

## The why

Running the gate per request before the console mounts, and denying with a 403 on
`Nothing`, is [security][principles]'s fail-closed rule at the console boundary:
absent an identity the request is refused, and the refusal is audit-logged rather
than silently swallowed. Building `Identity` through builders rather than a record
literal is [ease of use][principles] carried forward — an added field does not
break existing call sites. And returning `Maybe Identity` inside a `Task` keeps
authentication an ordinary effect the app composes, reusing the same session
check the rest of the app already runs — one identity surface, not two.

[principles]: ../../PRINCIPLES.md

## References

- **Per-symbol reference:** `ipe doc Ipe.Web.Console` — the `Identity` record and
  its builders (`defaultIdentity`, `withEmail`, `withClaim`, `withClaims`).
- **Sibling guides:** [Tasks](task.md) — the effect the `consoleAuth` callback
  returns. [Maybe](maybe.md) — the `Just`/`Nothing` allow/deny result.
  [Dictionaries](dict.md) — the `claims` map an RBAC layer consults.
- **Concepts:** [The Elm Architecture](the-elm-architecture.md) — the `Web.app`
  config `consoleAuth` extends.

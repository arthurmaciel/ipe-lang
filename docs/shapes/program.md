# Program

The plain shape: a `main` you drive yourself, with no TEA loop. `main` is a
`Task` (or a `do` block of them) that runs top to bottom and exits. Choose it
for scripts, one-shot CLI tools, cron jobs, and HTTP servers — anything whose
control flow is a sequence of effects rather than an `init`/`update`/`view`
cycle. This is also the fallback shape: a program that binds `main` to anything
other than an app entry point is a Program.

An HTTP / JSON API is a Program too — its `main` builds routes with
`Ipe.Http.Server` and ends in `Server.listen`.

## Entry point

`main = <task>` — no `app` kernel. Sequence effects with a `do` block or
`Task.andThen`; fan out with `Task.parallel [...]` bound inside a `do`.

## Minimal example

```ipe
module Main exposing (main)

import Ipe.Task as Task
import Ipe.Error as Error exposing (Error)
import Ipe.String as String
import Ipe.Io as Io


main =
    let version = "1.4.0" in
    do
        Io.println ("Preflight for v" ++ version)
        results <- Task.parallel
            [ checkBuild
            , checkChangelog
            , checkGitClean
            ]
        Io.println (report version results)


checkBuild : Task Error String
checkBuild =
    Task.succeed "build     ok  — artifact present"


checkChangelog : Task Error String
checkChangelog =
    Task.succeed "changelog ok  — entry for this version"


checkGitClean : Task Error String
checkGitClean =
    Task.succeed "git       ok  — working tree clean"


report : String -> List String -> String
report version results =
    "v" ++ version ++ " preflight passed:\n  " ++ String.join "\n  " results
```

## Running it

Run the example:

```sh
ipe run examples/shapes/program/release-preflight
```

```text
Preflight for v1.4.0
v1.4.0 preflight passed:
  build     ok  — artifact present
  changelog ok  — entry for this version
  git       ok  — working tree clean
```

It announces the run, fires the three checks concurrently with `Task.parallel`,
then prints the report and exits 0.

## Authenticated routes

A route can require an authenticated caller. `Server.getAuthed` (and its
`postAuthed` / `putAuthed` / `deleteAuthed` siblings) take a path, a
`Server.AuthConfig`, and a handler `Request -> Principal -> Task Error Response`.
Before the handler runs, fail-closed middleware reads the session token, verifies
it, and mints a `Principal` — the handler never sees an unauthenticated request,
and a missing or invalid token answers `401` without reaching your code. A
`Principal` has no constructor: the verified mint is its only origin, so a caller
identity can never be forged.

Build the config with `Server.authConfig secret tokenSource`, where the token
source is `Server.bearerToken` (the `Authorization: Bearer` header) or
`Server.cookieToken "name"` (a named cookie). Pass the minted `Principal` straight
to the secured-store operations (`Store.allAs` / `getAs` / `insertAs` / …) so the
database only ever returns rows the caller owns:

```ipe
module Main exposing (main)

import Ipe.Auth as Auth
import Ipe.Db as Db exposing (Db)
import Ipe.Db.Store as Store exposing (Secured)
import Ipe.Error exposing (Error)
import Ipe.Http.Server as Server
import Ipe.Http.Server exposing (Request, Response)
import Ipe.Secret as Secret
import Ipe.String as String
import Ipe.List as List
import Ipe.Task as Task exposing (Task)


authCfg : Server.AuthConfig
authCfg =
    Server.authConfig
        (Secret.fromString "your-32-byte-or-longer-signing-secret")
        Server.bearerToken


-- `principal` is minted by the middleware; `allAs` filters `docs` to the caller's
-- own rows through a bound SQL parameter, never string interpolation. (`Doc` is a
-- record alias; `docs` is a `Secured Doc` from `Store.secured`.)
handleMyDocs : Db -> Secured Doc -> Request -> Auth.Principal -> Task Error Response
handleMyDocs db docs _ principal =
    Task.andThen
        (\rows -> Task.succeed (Server.text (String.join "\n" (List.map .body rows))))
        (Store.allAs principal db docs)
```

Wire the handler into `Server.listen` with
`Server.getAuthed "/my/docs" authCfg (handleMyDocs db docs)`. A complete,
compiling program of this shape is
[`tests/golden/authed_store_query_seal/Main.ipe`](../../tests/golden/authed_store_query_seal/Main.ipe).

## Views as data: static rendering

`Ipe.Ui`, `Ipe.Html`, and `Ipe.Css` are top-level modules available to **any**
shape, not just the TEA ones — they are view *data types*. The boundary between a
Program and an app shape is the **live update loop**, not the data types. A
Program can build a `Html` or `Ui` tree and render it **once**, with no
`init` / `update` / `view` cycle.

Turn a tree into a string with `Html.render`:

```ipe
Html.render : Html msg -> String
```

or, as an effect, write it out with `Html.renderStatic` — its shape-neutral
sibling. A Program renders a static view without importing any app shape:

```ipe
Html.renderStatic : (Model -> Html Msg) -> Model -> Task Error ()
```

```ipe
module Main exposing (main)

import Ipe.Html as Html
import Ipe.Html.Attributes as Attr
import Ipe.Io as Io


page : Html msg
page =
    Html.div [ Attr.class "report" ]
        [ Html.h1 [] [ Html.text "Nightly report" ]
        , Html.p [] [ Html.text "All checks passed." ]
        ]


main =
    Io.println (Html.render page)
```

There is no runtime loop here, so **event handlers are inert**: an `onClick` on a
statically rendered node has nothing to dispatch to. A Program builds static
trees (an `Html Never`-style tree with no live messages); the tags, attributes,
and text render, but interactivity needs an app shape. See
[Views: Ui, Html, and Css](../language/ui.md) for the full vocabulary and the
`Ui.html` / `Ui.layout` bridges.

## Example

[`examples/shapes/program/release-preflight/`](../../examples/shapes/program/release-preflight/) — the program above.

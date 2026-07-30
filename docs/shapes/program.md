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
`Task.andThen`; fan out with `parallelDo` / `Task.parallel`.

## Minimal example

```ipe
module Main exposing (main)

import Ipe.Task as Task
import Ipe.Error as Error exposing (Error)
import Ipe.String as String
import Ipe.Io as Io


main =
    do
        version = "1.4.0"
        Io.println ("Preflight for v" ++ version)
        results <- parallelDo
            checkBuild
            checkChangelog
            checkGitClean
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

It announces the run, fires the three checks concurrently with `parallelDo`,
then prints the report and exits 0.

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

or, as an effect, write it out with `Web.renderStatic`:

```ipe
Web.renderStatic : (Model -> Html Msg) -> Model -> Task Error ()
```

```ipe
module Main exposing (main)

import Ipe.Prelude exposing (..)
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

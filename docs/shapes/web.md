# Web

A server-driven web app in [The Elm Architecture](https://guide.elm-lang.org/architecture/).
The server holds the `Model`, renders `view` to HTML, and streams patches to the
browser as `update` runs; the browser sends events back. Choose it for forms,
dashboards, and real-time UIs where the state of record lives on the server.
Because the `Model` is persisted to the session store, it must be a plain value
— the compiler rejects a `Model` carrying a function, `Cmd`, or view value with
a clear error.

## Entry point

`main = Web.app cfg`, where `cfg` is a record of
`init` / `update` / `view` / `subscriptions` plus `routes` and `notFound` for
URL routing. The view type is `view : Model -> Element Msg` — the portable
`Ipe.Ui` layout vocabulary shared with [WebView](webview.md) and
[Terminal](terminal.md). The framework applies `Ui.layout` internally to turn
that `Element` into the DOM, so a Web view is the same shape as a Terminal view
and switching between the two is a one-line change of the imported shape (see
[Views: Ui, Html, and Css](../language/ui.md)).

When you need direct DOM control — a tag or attribute `Ipe.Ui` does not
expose — author it with `Ipe.Html` and drop it into the `Element` view through
the `Ui.html : Html msg -> Element msg` node. Raw HTML is reached as a node
inside the one `Element` view, not through a separate entry point.

## Minimal example

```ipe
module Main exposing (main)

import Ipe.String as String
import Ipe.Tea.Web exposing (app)
import Ipe.Tea.Web.Cmd as Cmd
import Ipe.Tea.Web.Sub as Sub
import Ipe.Ui as Ui
import Ipe.Ui.Font as Font


type Page
    = CounterPage


type Msg
    = Increment
    | Decrement


type alias Model =
    { count : Int }


init : a -> ( Model, Cmd Msg )
init _req =
    ( { count = 0 }, Cmd.none )


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )

        Decrement ->
            ( { model | count = model.count - 1 }, Cmd.none )


subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none


view : Model -> Element Msg
view model =
    Ui.column [ Ui.spacing 16, Ui.padding 32, Ui.centerX ]
        [ Ui.el [ Font.size 20, Font.bold ] (Ui.text "Ipê counter")
        , Ui.row [ Ui.spacing 12 ]
            [ Ui.button [ Ui.padding 12 ]
                { onPress = Just Decrement, label = Ui.text "-" }
            , Ui.el [ Font.size 32, Font.bold ]
                (Ui.text (String.fromInt model.count))
            , Ui.button [ Ui.padding 12 ]
                { onPress = Just Increment, label = Ui.text "+" }
            ]
        ]


main =
    app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = []
        , notFound = CounterPage
        }
```

This is the program `ipe init` scaffolds.

## Running it

Scaffold the Web project, then run it:

```sh
ipe init myapp
```

```text
  Created Ipê project `myapp`.

  Next steps:
      cd myapp && ipe run

  Then open http://localhost:8000 and click the counter buttons.
```

`ipe run` serves the counter over HTTP on `http://localhost:8000`. Because the
UI is driven in a browser, open that address and click the buttons to see
`update` run and the view patch — a Web app needs a browser, not just a
terminal.

## Targeting the browser

`Web.app` also compiles to a pure-client single-page app under `--target wasm`,
where the whole TEA loop runs in WebAssembly. See the WASM examples under
[`examples/`](../../examples/) (`wasm/counter`, `wasm/spa`, `wasm/hydration`).

## Broadcasting: pub/sub

A Web app can broadcast a payload on a named topic to every session subscribed
to it. The bus is in-process: it lives in the running Web/live runtime, so a
publish reaches every session in the same process.

`Ipe.Tea.Web.PubSub` is the Web shape's pub/sub surface — the `Cmd`/`Sub` form
that plugs straight into the managed loop:

```text
Ipe.Tea.Web.PubSub.publish         : String -> any -> Cmd msg
Ipe.Tea.Web.PubSub.publishNoEcho   : String -> any -> Cmd msg
Ipe.Tea.Web.PubSub.subscribeTopic  : String -> (any -> msg) -> Sub msg
```

`publish` returns a `Cmd msg` to hand back from `update`, and `subscribeTopic`
returns a `Sub msg` to declare in `subscriptions` — so broadcasting on a topic
and listening on one are ordinary TEA wiring, with no `Task` plumbing. Importing
`Ipe.Tea.Web.PubSub` marks the module a TEA app (the same
[Program/TEA gate](program.md) every `Ipe.Tea.*` import applies).

`publishNoEcho` sets the broker's skip-origin bit: the publishing session's own
subscription is suppressed. `publish` echoes by default.

**Escape hatch — publishing from a `Task` pipeline.** To publish from outside
the loop — inside a `Task` chain, or from a plain Program — reach for the
top-level `Ipe.PubSub` instead: `publish : String -> any -> Task Error Int` (and
`publishNoEcho`) resolve to the number of subscribers reached. Being a `Task`,
it composes anywhere a `Task` does and does not mark a module a TEA app; the bus
only exists while a Web/live app runs, so a publish with none running resolves
to `Err`. The [`task-publish`](../../examples/shapes/web/task-publish/) example
fires it from a Web app's `update` via `Cmd.perform`.

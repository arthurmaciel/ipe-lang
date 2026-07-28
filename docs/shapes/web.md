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
URL routing. `view` returns `Html Msg`, built with the typed `Ipe.Ui` DSL.

## Minimal example

```ipe
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.String as String
import Ipe.Web exposing (app)
import Ipe.Cmd as Cmd
import Ipe.Sub as Sub
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


view : Model -> Html Msg
view model =
    Ui.layout []
        (Ui.column [ Ui.spacing 16, Ui.padding 32, Ui.centerX ]
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
        )


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
[`examples/`](../../examples/) (`wasm-counter`, `wasm-spa`, `wasm-hydration`).

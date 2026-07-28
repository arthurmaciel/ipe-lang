# WebView

A native desktop app in [The Elm Architecture](https://guide.elm-lang.org/architecture/).
It renders the same `Ipe.Ui` view as the [Web](web.md) shape, but inside a
system webview window instead of a browser served over HTTP — one binary, no
server process. Choose it for local desktop tools that want a rich UI without
shipping a web stack. The `Model` is kept in memory as a plain value.

## Entry point

`main = WebView.app cfg`, where `cfg` is a record of
`init` / `update` / `view` / `subscriptions` plus a `window` record
(`{ title : String, size : ( Int, Int ) }`). `view` returns `Html Msg`.

## Minimal example

```ipe
module Main exposing (main)

import Ipe.String as String
import Ipe.WebView as WebView
import Ipe.Cmd as Cmd
import Ipe.Sub as Sub
import Ipe.Ui as Ui
import Ipe.Ui.Font as Font


type Msg
    = Increment


type alias Model =
    { count : Int }


init : () -> ( Model, Cmd Msg )
init _unit =
    ( { count = 0 }, Cmd.none )


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )


view : Model -> Html Msg
view model =
    Ui.layout []
        (Ui.column [ Ui.spacing 16, Ui.padding 32 ]
            [ Ui.el [ Font.size 24, Font.bold ] (Ui.text "Ipê WebView")
            , Ui.button [ Ui.padding 12 ]
                { onPress = Just Increment
                , label = Ui.text ("count: " ++ String.fromInt model.count)
                }
            ]
        )


subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none


main =
    WebView.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , window = { title = "Ipê WebView", size = ( 640, 480 ) }
        }
```

## Running it

Build the binary with `ipe build`, then run it:

```sh
ipe build
ipe run
```

Running it opens a native window titled `Ipê WebView` with a counting button.
Because it opens a system window, run it on a desktop session with a display
rather than headless in CI.

# TUI

A terminal user interface in [The Elm Architecture](https://guide.elm-lang.org/architecture/).
`Tui.program` takes over the terminal, renders `view` to the screen, and turns
each keystroke into a `Msg` via `onKey`. Choose it for interactive
command-line UIs — pickers, dashboards, wizards — that redraw in place. The
`Model` is kept in memory, so it only needs `Clone`.

## Entry point

`main = Tui.program cfg`, where `cfg` is a record of
`init` / `update` / `view` / `subscriptions` / `onKey`. `view` returns a
`String` (the frame to draw), and
`onKey : { kind : String, value : String } -> Msg` maps a key event to a
message.

## Minimal example

```ipe
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.String as String
import Ipe.Tui as Tui
import Ipe.Cmd as Cmd
import Ipe.Sub as Sub


type Msg
    = KeyPressed String


type alias Model =
    { count : Int }


init : () -> ( Model, Cmd Msg )
init _unit =
    ( { count = 0 }, Cmd.none )


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        KeyPressed _key ->
            ( { model | count = model.count + 1 }, Cmd.none )


view : Model -> String
view model =
    "count: " ++ String.fromInt model.count ++ " (press any key)"


subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none


onKey : { kind : String, value : String } -> Msg
onKey event =
    KeyPressed event.value


main =
    Tui.program
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , onKey = onKey
        }
```

Build and run it with `ipe run`; it takes over the terminal and increments the
count on each key.

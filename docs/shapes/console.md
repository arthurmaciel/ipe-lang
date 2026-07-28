# Console

A line-oriented interactive tool in [The Elm Architecture](https://guide.elm-lang.org/architecture/).
`Console.app` runs the same TEA loop as the graphical shapes, but its channel is
standard input and output: it renders `view` to stdout, reads one line at a
time, turns each into a `Msg` via `onLine`, and re-renders. Choose it for
REPL-style prompts and stdin-driven tools that want managed state without a
full terminal UI. The `Model` is kept in memory, so it only needs `Clone`.

At start it renders the initial `view` once, then waits for input; at end of
input (EOF) it exits 0.

## Entry point

`main = Console.app cfg`, where `cfg` is a record of
`init` / `update` / `view` / `subscriptions` / `onLine`. `view` returns a
`String`, and `onLine : String -> Msg` maps each input line to a message.

## Minimal example

```ipe
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.String as String
import Ipe.Console exposing (app)


type Msg
    = GotLine String


type alias Model =
    { lines : Int }


init : () -> ( Model, Cmd Msg )
init _unit =
    ( { lines = 0 }, Cmd.none )


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        GotLine _line ->
            ( { model | lines = model.lines + 1 }, Cmd.none )


view : Model -> String
view model =
    "lines: " ++ String.fromInt model.lines


subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none


onLine : String -> Msg
onLine line =
    GotLine line


main =
    app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , onLine = onLine
        }
```

Run it with `ipe run examples/console-counter`; it prints `lines: 0`, then
`lines: N` after each line you type.

## Example

[`examples/console-counter/`](../../examples/console-counter/) — the program above.

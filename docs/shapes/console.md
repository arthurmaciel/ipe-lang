# Console

A line-oriented interactive tool in [The Elm Architecture](https://guide.elm-lang.org/architecture/).
`Console.app` runs the same TEA loop as the graphical shapes, but its channel is
standard input and output: it renders `view` to stdout, reads one line at a
time, turns each into a `Msg` via `onLine`, and re-renders. Choose it for
REPL-style prompts and stdin-driven tools that want managed state without a
full terminal UI. The `Model` is kept in memory as a plain value.

At start it renders the initial `view` once, then waits for input; at end of
input (EOF) it exits 0. A render is written verbatim, with no automatic
newline — a `view` that ends in `"> "` leaves the cursor on the prompt line, and
one that ends in `"\n"` puts each frame on its own line.

## Entry point

`main = Console.app cfg`, where `cfg` is a record of
`init` / `update` / `view` / `subscriptions` / `onLine`. `view` returns a
`String`, and `onLine : String -> Msg` maps each input line to a message.

## Minimal example

An accumulator calculator: each line is a command, `update` folds it into a
running total, and `view` prints the last outcome, the total, and a prompt.
Parsing happens once at the edge (`onLine` builds a `Command`), so `update` only
ever folds well-formed commands.

```ipe
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.String as String
import Ipe.System as System
import Ipe.Console exposing (app)
import Ipe.Cmd as Cmd
import Ipe.Sub as Sub


type Command
    = Add Int
    | Sub Int
    | Mul Int
    | Reset
    | Quit
    | Unknown String


type Msg
    = Entered Command


type alias Model =
    { total : Int
    , outcome : String
    }


init : () -> ( Model, Cmd Msg )
init _unit =
    ( { total = 0, outcome = "ready" }, Cmd.none )


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Entered command ->
            case command of
                Add n ->
                    ( { total = model.total + n, outcome = "added " ++ String.fromInt n }
                    , Cmd.none
                    )

                Sub n ->
                    ( { total = model.total - n, outcome = "subtracted " ++ String.fromInt n }
                    , Cmd.none
                    )

                Mul n ->
                    ( { total = model.total * n, outcome = "multiplied by " ++ String.fromInt n }
                    , Cmd.none
                    )

                Reset ->
                    ( { total = 0, outcome = "reset" }, Cmd.none )

                Quit ->
                    ( model, Cmd.perform (System.exit 0) (\_ -> Entered (Unknown "")) )

                Unknown raw ->
                    ( { model | outcome = "?  unknown command: " ++ raw }, Cmd.none )


view : Model -> String
view model =
    model.outcome ++ "\n= " ++ String.fromInt model.total ++ "\n> "


subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none


onLine : String -> Msg
onLine line =
    Entered (parse (String.trim line))


parse : String -> Command
parse line =
    case String.words line of
        verb :: rest ->
            case verb of
                "reset" ->
                    Reset

                "quit" ->
                    Quit

                "add" ->
                    parseBinary Add rest line

                "sub" ->
                    parseBinary Sub rest line

                "mul" ->
                    parseBinary Mul rest line

                _ ->
                    Unknown line

        [] ->
            Unknown line


parseBinary : (Int -> Command) -> List String -> String -> Command
parseBinary build rest line =
    case rest of
        argument :: [] ->
            case String.toInt argument of
                Just n ->
                    build n

                Nothing ->
                    Unknown line

        _ ->
            Unknown line


main =
    app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , onLine = onLine
        }
```

## Running it

Run the example and feed it a few commands. Because it reads standard input, you
can type interactively or pipe a scripted session:

```sh
printf 'add 5\nmul 10\nquit\n' | ipe run examples/console-repl
```

```text
ready
= 0
> added 5
= 5
> multiplied by 10
= 50
>
```

Each render ends in the `> ` prompt; `add 5` folds into the running total,
`mul 10` multiplies it, and `quit` exits 0.

## Example

[`examples/console-repl/`](../../examples/console-repl/) — the program above,
with a friendly hint on unknown input.

# Terminal

A terminal app in [The Elm Architecture](https://guide.elm-lang.org/architecture/).
`Ipe.Tea.Terminal` runs the managed `init` / `update` / `view` / `subscriptions`
loop over a terminal, and comes in two entries that differ by how the terminal is
driven:

- `Terminal.appScreen` — a full-screen UI addressed by keystrokes, redrawing in
  place.
- `Terminal.appLines` — a line stream: it renders to stdout, reads one line at a
  time, and re-renders. A REPL.

The `Model` is kept in memory as a plain value in both.

## `Terminal.appScreen` — full-screen UI

`main = Terminal.appScreen cfg` takes over the terminal, renders `view` to the
screen, and turns each keystroke into a `Msg` via `onKey`. Choose it for
interactive command-line UIs — pickers, dashboards, wizards — that redraw in
place.

`cfg` is a record of `init` / `update` / `view` / `subscriptions` / `onKey`,
where `view : Model -> Element Msg` is the **same typed `Ipe.Ui` element tree**
the [Web](web.md) and [WebView](webview.md) shapes render — the runtime walks the
tree and paints it to terminal cells, so one `view` function renders on web,
desktop, and terminal. `onKey : { kind : String, value : String } -> Msg` maps a
key event to a message. See [Views: Ui, Html, and Css](../ui.md) for the
`Element` vocabulary shared with the graphical shapes.

### Minimal example

The [Web](web.md) counter's `view`, rendered under Terminal via
`Terminal.appScreen` — the same `Ipe.Ui` column, row, buttons, and text.

```ipe
module Main exposing (main)

import Ipe.String as String
import Ipe.System as System
import Ipe.Tea.Terminal as Terminal
import Ipe.Cmd as Cmd
import Ipe.Sub as Sub
import Ipe.Ui as Ui
import Ipe.Ui exposing (Element)
import Ipe.Ui.Font as Font


type Msg
    = Increment
    | Decrement
    | Reset
    | Quit
    | NoOp


type alias Model =
    { count : Int }


type alias KeyEvent =
    { kind : String
    , value : String
    }


init : () -> ( Model, Cmd Msg )
init _unit =
    ( { count = 0 }, Cmd.none )


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )

        Decrement ->
            ( { model | count = model.count - 1 }, Cmd.none )

        Reset ->
            ( { count = 0 }, Cmd.none )

        Quit ->
            ( model, Cmd.perform (System.exit 0) (\_ -> NoOp) )

        NoOp ->
            ( model, Cmd.none )


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
        , Ui.el [ Font.size 12 ] (Ui.text "+/- adjust · r reset · q quit")
        ]


subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none


onKey : KeyEvent -> Msg
onKey key =
    if key.kind == "up" then
        Increment

    else if key.kind == "down" then
        Decrement

    else if key.kind == "char" && key.value == "+" then
        Increment

    else if key.kind == "char" && key.value == "-" then
        Decrement

    else if key.kind == "char" && key.value == "r" then
        Reset

    else if key.kind == "char" && key.value == "q" then
        Quit

    else
        NoOp


main =
    Terminal.appScreen
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , onKey = onKey
        }
```

Build and run it:

```sh
ipe build examples/terminal-counter
ipe run examples/terminal-counter
```

Running it takes over the terminal, draws the counter, and adjusts the count on
each key — the arrow keys and `+`/`-` change the value, `r` resets, `q` quits.
Because it drives a real terminal, run it in an interactive terminal session; it
has no headless mode, so CI can build it but cannot exercise the keystroke loop.

## `Terminal.appLines` — line-oriented REPL

`main = Terminal.appLines cfg` runs the same TEA loop, but its channel is
standard input and output: it renders `view` to stdout, reads one line at a time,
turns each into a `Msg` via `onLine`, and re-renders. Choose it for REPL-style
prompts and stdin-driven tools that want managed state without a full terminal
UI.

At start it renders the initial `view` once, then waits for input; at end of
input (EOF) it exits 0. A render is written verbatim, with no automatic
newline — a `view` that ends in `"> "` leaves the cursor on the prompt line, and
one that ends in `"\n"` puts each frame on its own line.

`cfg` is a record of `init` / `update` / `view` / `subscriptions` / `onLine`. The
view type is `view : Model -> String` (printed to stdout verbatim), and
`onLine : String -> Msg` maps each input line to a message. A line view is plain
text — it does not use the `Ipe.Ui` element tree.

### Minimal example

An accumulator calculator: each line is a command, `update` folds it into a
running total, and `view` prints the last outcome, the total, and a prompt.
Parsing happens once at the edge (`onLine` builds a `Command`), so `update` only
ever folds well-formed commands.

```ipe
module Main exposing (main)

import Ipe.String as String
import Ipe.System as System
import Ipe.Tea.Terminal exposing (appLines)
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
    appLines
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , onLine = onLine
        }
```

Run it and feed it a few commands. Because it reads standard input, you can type
interactively or pipe a scripted session:

```sh
printf 'add 5\nmul 10\nquit\n' | ipe run examples/terminal-repl
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

## Examples

- [`examples/terminal-counter/`](../../examples/terminal-counter/) — the
  `appScreen` counter above. The same `Ipe.Ui` view also backs the
  [Web](web.md) and [WebView](webview.md) counters.
- [`examples/terminal-repl/`](../../examples/terminal-repl/) — the `appLines`
  accumulator REPL above.

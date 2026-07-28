# TUI

A terminal user interface in [The Elm Architecture](https://guide.elm-lang.org/architecture/).
A TUI app takes over the terminal, renders `view` to the screen, and turns each
keystroke into a `Msg` via `onKey`. Choose it for interactive command-line UIs —
pickers, dashboards, wizards — that redraw in place. The `Model` is kept in
memory as a plain value.

## Entry points

There are two, differing only in what `view` returns:

- `main = Tui.app cfg` — `view : Model -> Element Msg`, the **same typed
  `Ipe.Ui` element tree** the [Web](web.md) and [WebView](webview.md) shapes
  render. The runtime walks the tree and paints it to terminal cells, so one
  `view` function renders on web, desktop, and terminal.
- `main = Tui.program cfg` — `view : Model -> String`, a raw frame painted
  verbatim. Use it when you want to draw the terminal yourself.

See [Views: Ui, Html, and Css](../ui.md) for the `Element` vocabulary shared with
the graphical shapes.

Either `cfg` is a record of `init` / `update` / `view` / `subscriptions` /
`onKey`, where `onKey : { kind : String, value : String } -> Msg` maps a key
event to a message.

## Minimal example

The [Web](web.md) counter's `view`, rendered under TUI via `Tui.app` — the same
`Ipe.Ui` column, row, buttons, and text.

```ipe
module Main exposing (main)

import Ipe.String as String
import Ipe.System as System
import Ipe.Tui as Tui
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
    Tui.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , onKey = onKey
        }
```

## Running it

Build the example, then run it:

```sh
ipe build examples/tui-counter
ipe run examples/tui-counter
```

`ipe build` compiles the program to a binary. Running it takes over the
terminal, draws the counter, and adjusts the count on each key — the arrow keys
and `+`/`-` change the value, `r` resets, `q` quits. Because it drives a real
terminal, run it in an interactive terminal session; it has no headless mode, so
CI can build it but cannot exercise the keystroke loop.

## Example

[`examples/tui-counter/`](../../examples/tui-counter/) — the program above. The
same `Ipe.Ui` view also backs the [Web](web.md) and [WebView](webview.md)
counters.

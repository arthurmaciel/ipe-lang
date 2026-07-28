# Views: Ui, Html, and Css

An Ipê view is built from two vocabularies and one styling module:

- **`Ipe.Ui`** — a high-level, medium-agnostic **layout** language (an
  [elm-ui](https://github.com/mdgriffith/elm-ui) port). You describe rows,
  columns, spacing, and alignment; the backend renders that description to the
  DOM (Web, WebView) or to terminal cells (TUI).
- **`Ipe.Html`** — the low-level **raw-DOM** language: direct tags, attributes,
  and events. DOM-only — it has no terminal rendering.
- **`Ipe.Css`** — typed style data, security-gated at construction.

`Ipe.Ui`, `Ipe.Html`, and `Ipe.Css` are ordinary top-level modules. They are
available to **any** shape — they are view *data types*, not something the Elm
Architecture owns. See [Program](shapes/program.md#views-as-data-static-rendering)
for using them with no TEA loop.

## `Ipe.Ui` — portable layout

`Ipe.Ui` is the portable view vocabulary. Its central type is `Element msg`, an
opaque layout tree that is **medium-agnostic**: the same `Element` renders to the
browser DOM under [Web](shapes/web.md) and [WebView](shapes/webview.md), and to
ANSI terminal cells under [TUI](shapes/tui.md).

Element builders:

```ipe
Ui.none   : Element msg
Ui.text   : String -> Element msg
Ui.el     : List (Attribute msg) -> Element msg -> Element msg
Ui.row    : List (Attribute msg) -> List (Element msg) -> Element msg
Ui.column : List (Attribute msg) -> List (Element msg) -> Element msg
```

plus `wrappedRow`, `grid`, `paragraph`, `textColumn`, `image`, `link`, `button`,
and `form`. Layout attributes (`Ui.spacing`, `Ui.padding`, `Ui.width`,
`Ui.centerX`, …) and styling submodules (`Ipe.Ui.Background`, `Ipe.Ui.Border`,
`Ipe.Ui.Font`, `Ipe.Ui.Region`, `Ipe.Ui.Input`, plus `Ipe.Ui.Keyed` and
`Ipe.Ui.Lazy`) fill out the vocabulary. A subtree is rendered to a page with
`Ui.layout` (below).

## `Ipe.Html` — raw DOM

`Ipe.Html` is the low-level escape hatch. Its type is `Html msg`, a raw-DOM node:
you name the tag and attributes directly.

```ipe
Html.div    : List (Attribute msg) -> List (Html msg) -> Html msg
Html.button : List (Attribute msg) -> List (Html msg) -> Html msg
Html.text   : String -> Html msg
```

(`Attribute` here is `Ipe.Html.Attribute`, distinct from the `Ipe.Ui` attribute.)
`Html msg` renders **only** to the DOM — there is no terminal rendering. Reach for
it when you need a tag or attribute `Ipe.Ui` does not expose; prefer `Ipe.Ui` for
portable layout.

## `Ipe.Css` — typed, security-gated styles

`Ipe.Css` is a compiled Ipê module that builds CSS as string data. It is a
security surface: `CssProp` and `CssRule` are **opaque** and every free-string
entry point is sanitized at construction (the "parse, don't validate" boundary),
backed by the `Ipe.CssSafety` leaf kernels. You cannot build a rule that smuggles
a `</style>` break-out or an `@import` injection.

```ipe
Css.property   : String -> String -> CssProp
Css.rule       : String -> List CssProp -> CssRule
Css.stylesheet : List CssRule -> String
Css.styles     : List CssProp -> String
```

Typed builders (`Css.px`, `Css.hex`, `Css.color`, `Css.padding`, …) are preferred
over the free-string entries; both are re-scanned by the same gate.

## Two vocabularies, one renderer

**Neither `Ipe.Ui` nor `Ipe.Html` is built on the other.** They are two separate
vocabularies; their relationship lives at the renderer, which knows how to paint
both to the DOM. `Element` is the portable view (Web / WebView / TUI); `Html` is
the Web / WebView-only raw-DOM escape hatch.

## Intermixing Ui and Html

Three bridges connect the two vocabularies:

**Embed a raw Html node inside a Ui layout** — `Ui.html`:

```ipe
Ui.html : Html msg -> Element msg
```

```ipe
Ui.column [ Ui.spacing 16 ]
    [ Ui.text "Above"
    , Ui.html (Html.iframe [ Attr.src "https://example.com" ] [])
    ]
```

**Attach a raw name/value attribute to a Ui element** — `Ui.htmlAttribute`:

```ipe
Ui.htmlAttribute : String -> String -> Attribute msg
```

```ipe
Ui.el [ Ui.htmlAttribute "data-testid" "counter" ] (Ui.text "42")
```

`Ui.htmlAttribute` takes a raw attribute **name and value** (both `String`) and
yields a `Ipe.Ui` attribute you can place in any element's attribute list.

**Render a Ui subtree to a Html node you can nest** — `Ui.layout`:

```ipe
Ui.layout : List (Attribute msg) -> Element msg -> Html msg
```

```ipe
page : Html Msg
page =
    Html.div []
        [ Html.h1 [] [ Html.text "Report" ]
        , Ui.layout [] (Ui.column [ Ui.spacing 8 ] rows)
        ]
```

This is why the [Web](shapes/web.md) and [WebView](shapes/webview.md) `view`
returns `Html Msg` but is written with `Ipe.Ui`: the top-level `Ui.layout []
element` turns the portable `Element` tree into the `Html` node the DOM runtime
mounts.

## Static rendering

A view tree can be turned into output **once**, with no live update loop:

```ipe
Html.render : Html msg -> String
```

`Html.render` walks a `Html msg` tree and returns the serialized HTML string.
(`Html.toString` is an alias.) Under [Web](shapes/web.md), the same idea is
available as a `Task`:

```ipe
Web.renderStatic : (Model -> Html Msg) -> Model -> Task Error ()
```

`Web.renderStatic` applies a `view` to a `Model` and writes the rendered HTML —
no TEA loop, no `update`, no subscriptions. This is the bridge that lets a plain
[Program](shapes/program.md#views-as-data-static-rendering) build and emit a view
without adopting an app shape.

## Per-shape `view` types

The four app shapes differ only in what `view` returns:

| Shape | Entry | `view` type | Renders to |
|---|---|---|---|
| [Web](shapes/web.md) | `Web.app` | `Model -> Html Msg` | DOM (`Ui.layout` wraps the `Element`) |
| [WebView](shapes/webview.md) | `WebView.app` | `Model -> Html Msg` | native webview DOM (`Ui.layout` wraps the `Element`) |
| [TUI](shapes/tui.md) | `Tui.app` | `Model -> Element Msg` | ANSI terminal cells |
| [TUI](shapes/tui.md) | `Tui.program` | `Model -> String` | raw ANSI frame, painted verbatim |
| [Console](shapes/console.md) | `Console.app` | `Model -> String` | stdout, printed verbatim |
| [Program](shapes/program.md) | plain `main` | *(none)* | — (static rendering only) |

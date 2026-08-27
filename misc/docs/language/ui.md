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
Architecture owns. See [Program](../shapes/program.md#views-as-data-static-rendering)
for using them with no TEA loop.

## `Ipe.Ui` — portable layout

`Ipe.Ui` is the portable view vocabulary. Its central type is `Element msg`, an
opaque layout tree that is **medium-agnostic**: the same `Element` renders to the
browser DOM under [Web](../shapes/web.md) and [WebView](../shapes/webview.md), and to
ANSI terminal cells under [Terminal](../shapes/terminal.md).

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

`Ui.layout` is what the framework applies internally for the
[Web](../shapes/web.md) and [WebView](../shapes/webview.md) shapes: their `view`
returns the portable `Element Msg`, and the framework wraps it in `Ui.layout []`
to produce the `Html` node the DOM runtime mounts. You reach for `Ui.layout`
directly only when nesting a `Ui` subtree inside a hand-written `Html` tree
(reached through the `Ui.html` node).

## Static rendering

A view tree can be turned into output **once**, with no live update loop:

```ipe
Html.render : Html msg -> String
```

`Html.render` walks a `Html msg` tree and returns the serialized HTML string.
(`Html.toString` is an alias.) Its effectful sibling writes that HTML as a
`Task`:

```ipe
Html.renderStatic : (Model -> Html Msg) -> Model -> Task Error ()
```

`Html.renderStatic` applies a `view` to a `Model` and writes the rendered HTML —
no TEA loop, no `update`, no subscriptions. It is shape-neutral (it lives in
`Ipe.Html`, not under any `Ipe.Tea.*` shape), so a plain
[Program](../shapes/program.md#views-as-data-static-rendering) can build and emit
a view without adopting an app shape:

```ipe
import Ipe.Html as Html

main =
    Html.renderStatic view model
```

`Html.render` is **safe by construction**: text nodes (`Html.text`) and attribute
values are HTML-escaped at the render sink, so a `<script>`, `&`, `'`, or `"` in
model data comes out as inert entity references. The safe `Ipe.Html` surface has
no way to inject raw, unescaped HTML — there is no `raw : String -> Html`.

## `Ipe.Html.Unsafe` — the raw-HTML / inline-script escape hatch

Some views must emit **trusted, verbatim** markup — a pre-sanitised HTML fragment
or an inline `<script>`. That escape hatch lives in a separate, deliberately
awkward submodule:

```ipe
Ipe.Html.Unsafe.unsafeRaw    : String -> Html msg
Ipe.Html.Unsafe.unsafeScript : String -> Html msg
```

`unsafeRaw` injects its `String` into the DOM **un-escaped**; `unsafeScript`
emits an inline `<script>` whose body is the JavaScript verbatim. Both are named
`unsafe*` and homed in `Ipe.Html.Unsafe` because they bypass the XSS barrier: the
caller owns the guarantee that the input is trusted, author-controlled content
(user data belongs in `Html.text`, escaped by construction). Importing the
submodule discloses the `unsafe` capability program-wide — `ipe capabilities`
reports it, so a dependency's raw sink is visible before you run it.

```ipe
import Ipe.Html exposing (section, text)
import Ipe.Html.Unsafe exposing (unsafeRaw, unsafeScript)

view _ =
    section []
        [ text userInput                       -- escaped: cannot inject
        , unsafeRaw "<b>trusted markup</b>"     -- verbatim: you own safety
        , unsafeScript "console.log('ready');"  -- inline <script>, verbatim
        ]
```

## `Ipe.Markdown` — markdown to `Element msg`

`Ipe.Markdown` is a pure Ipê compiled-source module that parses a markdown
string and produces a typed `Element msg` tree — no HTML string round-trip,
no raw-DOM output, no sanitiser needed. All output routes through `Ipe.Ui`
constructors, so user-supplied markdown cannot inject scripts or event
handlers into the DOM.

```ipe
Markdown.render       : String -> Element msg
Markdown.renderInline : String -> Element msg
Markdown.parseBlocks  : String -> List Block
Markdown.parseSpans   : String -> List Span
```

`render` handles multi-block documents (headings, paragraphs, fenced code
blocks, bullet and ordered lists, tables, horizontal rules, inline bold /
italic / code / links). `renderInline` handles a single line of inline
markup and is useful inside an existing paragraph context. The chrome the
renderers draw — code-block and inline-code surfaces, table borders,
horizontal rules, list markers — is derived from the surrounding theme
foreground (`currentColor`), so it tracks a light or dark page with no fixed
palette.

`parseBlocks` and `parseSpans` expose the parser itself, returning the
block-level (`Block`) and inline (`Span`) parse trees. Reach for them when you
want Markdown's parse but your own typography and palette, rather than the
built-in renderers:

```ipe
import Ipe.List as List
import Ipe.Markdown as Markdown exposing (Block(..))

-- Collect just the fenced code blocks from a document.
codeBlocks : String -> List String
codeBlocks src =
    List.filterMap keepCode (Markdown.parseBlocks src)

keepCode : Block -> Maybe String
keepCode block =
    case block of
        CodeBlock body ->
            Just body

        HeaderBlock _ _ ->
            Nothing

        ParaBlock _ ->
            Nothing

        BulletBlock _ ->
            Nothing

        NumberedBlock _ ->
            Nothing

        TableBlock _ _ ->
            Nothing

        RuleBlock ->
            Nothing
```

(`Block` is a closed union, so Ipê requires every constructor to be handled —
no catch-all `_` arm — which is why each block variant appears explicitly.)

```ipe
import Ipe.Markdown as Markdown
import Ipe.Ui as Ui

view : Model -> Element Msg
view model =
    Ui.column [ Ui.spacing 16 ]
        [ Markdown.render model.readmeText
        , Markdown.renderInline "Use `render` for full documents."
        ]
```

The supported inline delimiters (`**bold**`, `*italic*`, `` `code` ``,
`[text](url)`) are parsed without regex — a linear char-by-char scan with
graceful degradation on malformed input (an unmatched `*` is emitted as
literal text rather than discarded or panicking).

## Per-shape `view` types

The graphical shapes — Web, WebView, and `Terminal.appScreen` — share ONE `view`
type, `Model -> Element Msg`, so a view is portable across them and switching
shape is a one-line change of the imported module. `Terminal.appLines` paints a
`String` frame, a genuinely different medium.

| Shape | Entry | `view` type | Renders to |
|---|---|---|---|
| [Web](../shapes/web.md) | `Web.app` | `Model -> Element Msg` | DOM (framework applies `Ui.layout`) |
| [WebView](../shapes/webview.md) | `WebView.app` | `Model -> Element Msg` | native webview DOM (framework applies `Ui.layout`) |
| [Terminal](../shapes/terminal.md) | `Terminal.appScreen` | `Model -> Element Msg` | ANSI terminal cells |
| [Terminal](../shapes/terminal.md) | `Terminal.appLines` | `Model -> String` | stdout, printed verbatim |
| [Program](../shapes/program.md) | plain `main` | *(none)* | — (static rendering only) |

Raw `Html` is reached through the `Ui.html : Html msg -> Element msg` node, which
embeds a hand-written `Html` subtree inside the `Element` view under any
graphical shape — there is no separate whole-view raw-`Html` entry point.

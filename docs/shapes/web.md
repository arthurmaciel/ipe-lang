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

`Web.app` also compiles to a pure-client single-page app for the browser, where
the whole TEA loop runs in WebAssembly. When `package.ipe` declares
`Package.wasm Wasm.spa` or `Package.wasm Wasm.hydrate`, the compiler infers the
wasm target automatically — `ipe build` is then equivalent to `ipe build --target
wasm`, producing the browser bundle in `out/rust/www/` without an explicit flag.
`examples/wasm/spa/package.ipe` is a real working example of this layout.

Target resolution precedence (highest first): `--target wasm` CLI flag >
`IPE_TARGET=wasm` env > the manifest's `Package.wasm` mode > default native. The
`--target wasm` flag and `IPE_TARGET=wasm` both continue to work and override
the manifest. Omitting the `Package.wasm` stage entirely keeps the native default
even when the flag is absent.

See the complete examples under [`examples/`](../../examples/) (`wasm/counter`,
`wasm/spa`, `wasm/hydration`).

## URL routing and navigation

A `Model` with a `page` field is a routed app: `routes` maps URL patterns to
`page` values (via `Web.route`), and `notFound` is the fallback `page`. On each
matched URL the framework reconciles the model to the page the URL names.

By default that reconcile writes the matched page straight into `Model.page` —
navigation never reaches `update`. To own navigation in `update` instead, supply
the optional `onNavigate : page -> msg` field: every URL-driven route change is
turned into that `Msg` and dispatched through `update`, so the page transition
happens only through the `update` arm you write.

```ipe
type Msg
    = Increment
    | Navigate Page

update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )

        Navigate page ->
            ( { model | page = page }, Cmd.none )

main =
    Web.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = routeTable
        , notFound = HomePage
        , onNavigate = Navigate
        }
```

Omitting `onNavigate` is exactly the implicit form of a `Navigate` arm that sets
`page` — the two behave identically for an app that only updates `page`. Supply
`onNavigate` when a navigation should do more (load data for the new page, reset
a form, record analytics).

## Auth session lifetime

Signed session tokens carry a hard **absolute lifetime cap**: a token cannot be
used after `iat + AuthMaxLifetime`, regardless of any later re-issue. The cap is
stamped at first issue inside the signed segment and cannot be extended by the
client or by a re-issue — a tampered cap fails signature verification.

The cap defaults to **8 h**. Override it in two ways (env wins):

- **In-code**: `Web.authMaxLifetime <seconds>` in the `Web.appWith` settings list.
- **Environment**: `IPE_AUTH_MAX_LIFETIME=<seconds>` overrides the in-code setting
  without a rebuild.

A non-positive value falls closed to the 8 h default.

A session token without a `cap` claim (minted before this feature) is bounded
only by its `exp` — it does not receive an unlimited lifetime.

### Sliding (rolling) re-issue

When a cookie-based authenticated route receives a valid token that is past its
re-issue threshold (`exp - authSlideWindow / 2`), the middleware mints a fresh
token and attaches it via `Set-Cookie`. The new `exp` is
`min(now + authSlideWindow, cap)` — active sessions extend automatically while
the absolute cap is never crossed. `iat` and `cap` are carried verbatim from the
verified token and cannot be moved outward.

The sliding window defaults to **30 m**. Override it in two ways (env wins):

- **In-code**: `Web.authSlideWindow <seconds>` in the `Web.appWith` settings list.
- **Environment**: `IPE_AUTH_SLIDE_WINDOW=<seconds>` overrides the in-code setting
  without a rebuild.

A non-positive value or a value ≥ `authMaxLifetime` falls closed to a safe
default. Re-issue only fires for cookie token sources (`Server.cookieToken`) —
bearer-header tokens are API credentials the client manages directly.

## Broadcasting: pub/sub

A Web app can broadcast a payload on a named topic to every session subscribed
to it. The bus is in-process: it lives in the running Web/live runtime, so a
publish reaches every session in the same process.

`Ipe.Tea.Web.PubSub` is the Web shape's pub/sub surface — the `Cmd`/`Sub` form
that plugs straight into the managed loop:

```text
Ipe.Tea.Web.PubSub.publish        : Topic a -> a -> Cmd msg
Ipe.Tea.Web.PubSub.publishNoEcho  : Topic a -> a -> Cmd msg
Ipe.Tea.Web.PubSub.subscribeTopic : Topic a -> (a -> msg) -> Sub msg
```

`Topic a` is a typed topic handle constructed with `PubSub.topic : String ->
Topic a` from `Ipe.PubSub`. Sharing the same `Topic a` value between publisher
and subscriber is how the compiler enforces payload-type agreement at compile
time (`a` is the payload type; mismatches are a type error, not a runtime
surprise).

`publish` returns a `Cmd msg` to hand back from `update`; `subscribeTopic`
returns a `Sub msg` to declare in `subscriptions` — broadcasting and listening
are ordinary TEA wiring with no `Task` plumbing:

```ipe
-- illustrative — not a standalone runnable program
import Ipe.Tea.Web.PubSub as WebPubSub
import Ipe.PubSub exposing (topic, Topic)

chatTopic : Topic String
chatTopic =
    topic "chat"

-- in update:
--   ( model, WebPubSub.publish chatTopic model.draft )

-- in subscriptions:
--   WebPubSub.subscribeTopic chatTopic GotChat
```

Importing `Ipe.Tea.Web.PubSub` marks the module a TEA app (the same
[Program/TEA gate](program.md) every `Ipe.Tea.*` import applies).
`publishNoEcho` sets the broker's skip-origin bit so the publishing session's
own subscription is suppressed; `publish` echoes by default.

**Escape hatch — publishing from a `Task` pipeline.** To publish from outside
the loop — inside a `Task` chain, or from a plain `Program` — reach for
`Ipe.PubSub.publish : Topic a -> a -> Task Error Int` (and `publishNoEcho`)
instead. Being a `Task`, it composes anywhere a `Task` does and does not mark a
module a TEA app; the bus only exists while a Web/live app runs, so a publish
with none running resolves to `Err`. The
[`task-publish`](../../examples/shapes/web/task-publish/) example fires it from
a Web app's `update` via `Cmd.perform`.

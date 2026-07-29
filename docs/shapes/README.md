# Application shapes

One language, four ways to ship. An Ipê program's **shape** is chosen by its
entry point — the function `main` is bound to. Nothing else in the manifest
selects it; the compiler infers the shape from the entry kernel and drives
emission, the runtime bound on your `Model`/`Msg`, and the capability set.

Three shapes follow [The Elm Architecture](https://guide.elm-lang.org/architecture/)
(`init` / `update` / `view` / `subscriptions`); the fourth is a plain `main`.

| Shape | Entry point | Use it for |
|-------|-------------|------------|
| [Web](web.md) | `Web.app` | Server-driven web apps — HTML over the wire, SSE patches, sessions, routing. |
| [WebView](webview.md) | `WebView.app` | Native desktop apps rendering the same `Ipe.Ui` view in a system webview. |
| [Terminal](terminal.md) | `Terminal.appScreen` / `Terminal.appLines` | Terminal apps — a full-screen keystroke UI (`appScreen`), or a line-oriented stdin REPL (`appLines`). |
| [Program](program.md) | plain `main` | Scripts, one-shot tools, cron jobs, and HTTP servers — no TEA loop. |

Web, WebView, and `Terminal.appScreen` share the same `Ipe.Ui` view code, so one
`view : Model -> Element Msg` renders on web, desktop, and terminal. Raw escapes
are nodes inside that one view — `Ui.html` for direct DOM under the web shapes.

See [Views: Ui, Html, and Css](../ui.md) for the view vocabularies (`Ipe.Ui`,
`Ipe.Html`, `Ipe.Css`), how to intermix them, each shape's exact `view` type, and
static rendering.

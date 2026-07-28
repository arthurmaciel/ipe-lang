# Application shapes

One language, five ways to ship. An Ipê program's **shape** is chosen by its
entry point — the function `main` is bound to. Nothing else in the manifest
selects it; the compiler infers the shape from the entry kernel and drives
emission, the runtime bound on your `Model`/`Msg`, and the capability set.

Four shapes follow [The Elm Architecture](https://guide.elm-lang.org/architecture/)
(`init` / `update` / `view` / `subscriptions`); the fifth is a plain `main`.

| Shape | Entry point | Use it for |
|-------|-------------|------------|
| [Web](web.md) | `Web.app` | Server-driven web apps — HTML over the wire, SSE patches, sessions, routing. |
| [WebView](webview.md) | `WebView.app` | Native desktop apps rendering the same `Ipe.Ui` view in a system webview. |
| [TUI](tui.md) | `Tui.program` | Terminal UIs driven by keystrokes. |
| [Console](console.md) | `Console.app` | Line-oriented interactive tools — a managed stdin-driven TEA loop. |
| [Program](program.md) | plain `main` | Scripts, one-shot tools, cron jobs, and HTTP servers — no TEA loop. |

The two graphical shapes (Web, WebView) share the same `Ipe.Ui` view code, so
one view renders on web and desktop.

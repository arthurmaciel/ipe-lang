---
kind: topic
title: "shapes: the five app entry shapes"
summary: "The five program shapes (Web, WebView, Tui, Cli, Program) and when to use each."
idiom: false
aliases: ["app-shapes", "web", "webview", "tui", "cli", "program", "app-entry"]
see_also: ["main", "state", "effects"]
---

# `shapes` — the five app entry shapes

The code examples in this page are illustrative Ipê source snippets, not shell commands.

Every Ipê program has exactly one `main` value. Depending on what you want to
build — a web app, a terminal UI, a desktop webview, or a plain script — you
pick the matching entry function. These are called **shapes**.

## The five shapes

| Shape | Entry function | Output |
|-------|---------------|--------|
| Web | `Web.tea` | Browser HTML/CSS app |
| WebView | `WebView.app` | Desktop window running a web view |
| Tui | `Tui.tea` | Full-screen terminal UI |
| Cli | `Cli.tea` | Line-by-line terminal app (REPL) |
| Program | direct `Task Error ()` | Command-line script, no UI |

## Web

A `Web.tea` is a browser application driven by the TEA update loop. It renders
HTML, handles user events, and can communicate with a server:

```ipe
main : Task Error ()
main =
    Web.tea
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        }
```

## WebView

A `WebView.app` wraps a web-rendered UI in a native desktop window:

```ipe
main : Task Error ()
main =
    WebView.app
        { init = init
        , update = update
        , view = view
        }
```

## Tui

`Tui.tea` renders a full-screen TUI that redraws on each model change:

```ipe
main : Task Error ()
main =
    Tui.tea
        { init = init
        , update = update
        , view = view
        }
```

## Cli

`Cli.tea` is line-by-line (stdout-style) with event handling:

```ipe
main : Task Error ()
main =
    Cli.tea
        { init = init
        , update = update
        , view = view
        }
```

## Program (plain script)

When you do not need a UI, `main` is just a `Task Error ()` — a description
of what the script does:

```ipe
main : Task Error ()
main =
    Io.println "Hello, world!"
```

This is the simplest shape. It has no model, no update loop — just a sequence
of effects.

## Choosing a shape

- Building for the browser? → **Web**
- Building a desktop app with a web-rendered UI? → **WebView**
- Building a full-screen terminal UI? → **Tui**
- Building a line-by-line terminal REPL? → **Cli**
- Building a script, a tool, or a batch job? → **Program**

Each shape provides its own `Cmd`, `Sub`, and event model. Do not mix shapes
in one program — importing a `Cmd` from a different shape than your app's
entry gives IPE-N0035.

## Glossary

- **shape** — one of the five `main` entry patterns: Web, WebView, Tui, Cli, Program.
- **`Web.tea`** — browser TEA entry point.
- **`Tui.tea`** — full-screen terminal TUI entry point.
- **`Cli.tea`** — line-by-line terminal entry point.
- **`WebView.app`** — desktop webview entry point.

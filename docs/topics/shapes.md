---
kind: topic
title: "shapes: the four app entry shapes"
summary: "The four program shapes (Web, WebView, Terminal, Program) and when to use each."
idiom: false
aliases: ["app-shapes", "web", "webview", "terminal", "program", "app-entry"]
see_also: ["main", "state", "effects"]
---

# `shapes` — the four app entry shapes

The code examples in this page are illustrative Ipê source snippets, not shell commands.

Every Ipê program has exactly one `main` value. Depending on what you want to
build — a web app, a terminal UI, a desktop webview, or a plain script — you
pick the matching entry function. These are called **shapes**.

## The four shapes

| Shape | Entry function | Output |
|-------|---------------|--------|
| Web | `Web.app` | Browser HTML/CSS app |
| WebView | `WebView.app` | Desktop window running a web view |
| Terminal | `Terminal.appScreen` / `Terminal.appLines` | TUI or line-by-line terminal app |
| Program | direct `Task Error ()` | Command-line script, no UI |

## Web

A `Web.app` is a browser application driven by the TEA update loop. It renders
HTML, handles user events, and can communicate with a server:

```ipe
main : Task Error ()
main =
    Web.app
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

## Terminal

`Terminal.appScreen` renders a full-screen TUI that redraws on each model
change. `Terminal.appLines` is line-by-line (stdout-style) with event handling:

```ipe
-- Full-screen TUI
main : Task Error ()
main =
    Terminal.appScreen
        { init = init
        , update = update
        , view = view
        }
```

```ipe
-- Line-by-line terminal app
main : Task Error ()
main =
    Terminal.appLines
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
- Building a terminal UI or interactive CLI? → **Terminal**
- Building a script, a tool, or a batch job? → **Program**

Each shape provides its own `Cmd`, `Sub`, and event model. Do not mix shapes
in one program — importing a `Cmd` from a different shape than your app's
entry gives IPE-N0035.

## Glossary

- **shape** — one of the four `main` entry patterns: Web, WebView, Terminal, Program.
- **`Web.app`** — browser TEA entry point.
- **`Terminal.appScreen`** — full-screen TUI entry point.
- **`Terminal.appLines`** — line-by-line terminal entry point.
- **`WebView.app`** — desktop webview entry point.

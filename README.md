# Ipê

[![CI](https://github.com/arthurmaciel/ipe-lang/actions/workflows/ci.yml/badge.svg)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/ci.yml)

**Ipê** pairs **Elm's syntax** with **Sky's batteries-included runtime** — the
standard library, effect system, and application framework (web, API, CLI,
terminal, desktop) that turn a pure-functional language into a full-stack one.
It compiles to readable, `rustfmt`-clean Rust.

```sh
curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/scripts/install.sh | sh
```

```elm
-- src/Main.ipe
module Main exposing (main)
import Ipe.Prelude exposing (..)
import Ipe.Log exposing (println)

main =
    println "Hello from Ipê!"
```

```sh
ipe run src/Main.ipe        # compile + run in one step
```

Prefer building from source? `git clone https://github.com/arthurmaciel/ipe-lang
&& cd ipe-lang && cargo build --release`.

## Contents

- [Features](#features)
- [Code shapes](#code-shapes)
- [Support](#support)

## Features

- **Elm syntax** — pure functions, Hindley–Milner type inference, exhaustive
  `case`, immutable data. No `null`, no runtime exceptions.
- **Sky's batteries-included runtime** — typed HTTP, Live (SSR + real-time),
  SQL databases, auth, email, cache, pub/sub, and WebSockets, all behind a
  single `Task Error a` effect boundary.
- **Rust compiler** — the compiler itself is written in Rust: fast, parallel,
  memory-safe.
- **Rust backend** — emits readable Rust. THE SEAL is enforced: if `ipe`
  accepts your program, the generated Rust compiles.
- **Incremental compilation** — a salsa-backed query engine; `ipe watch`
  recompiles only what changed.
- **Static compilation** — `ipe build --static` produces a fully-static musl
  single binary. Copy it anywhere and run — no runtime, no dependencies.

## Code shapes

One language, five ways to ship. Pick the entry point that matches your app.

| Shape | Entry point | Use it for | TEA |
|---|---|---|---|
| `Ipe.Live` | `Live.app` | Web apps — server-rendered HTML, real-time SSE patches, sessions | ✓ |
| `Ipe.Http.Server` | `Server.listen` | Headless HTTP / JSON APIs | |
| `Ipe.Cli` | `Task.run` | One-shot tools and cron jobs | |
| `Ipe.Tui` | `Tui.app` | Terminal UIs | ✓ |
| `Ipe.Webview` | `Webview.app` | Native desktop apps | ✓ |

The three ✓ shapes follow [The Elm Architecture](https://guide.elm-lang.org/architecture/)
(`init` / `update` / `view` / `subscriptions`) — and share the **same
`Ipe.Ui` view code**, so one view renders on web, terminal, and desktop.
See [`examples/`](examples/) for a program of each shape.

## Support

Ipê is developed in the open by one person. The Rust backend tracks the
upstream [Sky](https://github.com/anzellai/sky) language; keeping pace takes
real work. If Ipê is useful to you, [support its development](https://ko-fi.com/arthur_maciel??g=1)
— it directly buys faster progress. Thank you! :)

Contributions are welcome and **every PR is human-reviewed** before merge.
The most valuable contributions are **bug reports and security/soundness
fixes** — a mis-compilation, a panic on valid input, or an unsound emit is
always worth an [issue](https://github.com/arthurmaciel/ipe-lang/issues).

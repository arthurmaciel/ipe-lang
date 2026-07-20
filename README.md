# Ipê

[![CI](https://github.com/arthurmaciel/ipe-lang/actions/workflows/ci.yml/badge.svg)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/ci.yml)

**Ipê** pairs **Elm's syntax** with **Sky's batteries-included runtime** — the
standard library, effect system, and application framework (web, API, CLI,
terminal, desktop) that turn a pure-functional language into a full-stack one.
It compiles to readable, `rustfmt`-clean Rust.

```sh
curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/scripts/install.sh | sh
```

Scaffold a new project — `ipe init` writes an `ipe.toml`, a `README.md`, and a
working `Ipe.Live` counter in `src/Main.ipe`:

```sh
ipe init counter          # or `ipe init .` to scaffold in the current directory
cd counter
ipe run                   # serves the counter at http://localhost:8000
```

Prefer to start from scratch? A minimal program is just:

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
- [Capabilities](#capabilities)
- [Editor setup (LSP)](#editor-setup-lsp)
- [Static compilation](#static-compilation)
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

## Capabilities

Every effect in Ipê flows through a capability-tagged kernel, so the compiler can
tell you exactly what a program is allowed to do — network, filesystem, env,
subprocess, clock, random, native-ffi — from its code alone, with nothing to
declare. `ipe capabilities <entry>` prints that inferred set (one per line, or
`none` for a pure program):

```
$ ipe capabilities examples/sky/02-go-stdlib/src/Main.ipe
network
clock
```

The set is generated, not hand-written, and cannot drift: a program that reaches
a new effectful kernel gains the matching capability automatically. `native-ffi`
appears whenever the program crosses into `Rust.` code, which is opaque to the
inference and the one place effects can escape the model.

## Editor setup (LSP)

`ipe lsp` speaks JSON-RPC over stdio and works with any LSP-compliant editor.
Features: type-directed completion, go-to-definition, find-references, rename,
formatting, range formatting, code actions, semantic tokens, signature help,
and inlay hints.

Completion is type-directed: where the surrounding context expects a specific
type (a function argument, a typed binding's body, an `if`/`case` branch, a
list element), candidates whose type matches are offered first and the expected
type's own constructors are surfaced — an `Int` slot never offers a `String`.
Away from such a context it falls back to every in-scope name. Every suggestion
comes from the same type-checker `ipe build` runs, so a completion the editor
offers is one the compiler accepts.

### Helix

Add to `~/.config/helix/languages.toml`:

```toml
[[language]]
name = "ipe"
scope = "source.ipe"
file-types = ["ipe"]
roots = ["sky.toml"]
language-servers = ["ipe-lsp"]

[language-server.ipe-lsp]
command = "ipe"
args = ["lsp"]
```

### Neovim (with `nvim-lspconfig`)

```lua
local lspconfig = require("lspconfig")
local configs = require("lspconfig.configs")

if not configs.ipe then
  configs.ipe = {
    default_config = {
      cmd = { "ipe", "lsp" },
      filetypes = { "ipe" },
      root_dir = lspconfig.util.root_pattern("sky.toml", ".git"),
      settings = {},
    },
  }
end

lspconfig.ipe.setup({})
```

Add the filetype detection if needed:

```lua
vim.filetype.add({ extension = { ipe = "ipe" } })
```

### VS Code

Install the [Ipê extension](https://marketplace.visualstudio.com/items?itemName=arthurmaciel.ipe-lang)
(bundles the LSP client), or configure it manually in `.vscode/settings.json`:

```json
{
  "ipe.languageServer.command": "ipe",
  "ipe.languageServer.args": ["lsp"]
}
```

If you prefer a generic LSP client (e.g. `vscode-languageclient`), register:

```json
{
  "[ipe]": {},
  "languageServerExample.trace.server": "verbose"
}
```

and point `command` to `ipe lsp` for `.ipe` files.

## Static compilation

`ipe build --static` produces a fully-static musl binary — zero runtime
dependencies, copy and run anywhere.

```sh
# Prerequisite (once):
rustup target add x86_64-unknown-linux-musl
sudo apt-get install musl-tools   # or equivalent on your distro

# Build a static binary (x86_64 Linux, dlmalloc allocator — the default):
cd examples/01-hello-world
ipe build sky.toml --out sky-out/rust --static
cd sky-out/rust
cargo build --release --target x86_64-unknown-linux-musl
```

The emitted `.cargo/config.toml` sets `+crt-static` automatically; no extra
`RUSTFLAGS` are needed.

**Allocator options** (`--allocator <name>`):

| Name | Default | Notes |
|---|---|---|
| `dlmalloc` | yes | pure Rust, no C toolchain beyond musl |
| `mimalloc` | | C opt-in; needs a musl-capable C compiler |
| `system` | | musl's malloc; requires `--allow-slow-allocator` |

**Supported targets:**

| Target | Status |
|---|---|
| `x86_64-unknown-linux-musl` | fully supported, CI-verified |
| `aarch64-unknown-linux-musl` | wired, pending toolchain confirmation (CI: `continue-on-error`) |

The aarch64 target is structurally complete — the variant exists, the CI job
runs, and cross-verification via `qemu-user-static` is scripted — but the CI
job is marked `continue-on-error` until a musl-capable AArch64 C linker is
confirmed available on the runner. Remove `continue-on-error` from
`.github/workflows/static.yml` `linux-static-arm64` once the job turns green.

## Support

Ipê is developed in the open by one person. The Rust backend tracks the
upstream [Sky](https://github.com/anzellai/sky) language; keeping pace takes
real work. If Ipê is useful to you, [support its development](https://ko-fi.com/arthur_maciel??g=1)
— it directly buys faster progress. Thank you! :)

Contributions are welcome and **every PR is human-reviewed** before merge.
The most valuable contributions are **bug reports and security/soundness
fixes** — a mis-compilation, a panic on valid input, or an unsound emit is
always worth an [issue](https://github.com/arthurmaciel/ipe-lang/issues).

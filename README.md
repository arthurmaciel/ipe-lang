<div align="center">
    <img width="250" height="251" alt="Yellow Ipê (Handroanthus serratifolius)" src="https://github.com/user-attachments/assets/870f8739-69ab-4b05-af6a-b56c3e615e1c" />
</div>

<br />

[![Install](https://github.com/arthurmaciel/ipe-lang/actions/workflows/install-smoke.yml/badge.svg?branch=main)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/install-smoke.yml)
[![Build & test](https://github.com/arthurmaciel/ipe-lang/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/ci.yml)
[![Sandbox](https://github.com/arthurmaciel/ipe-lang/actions/workflows/admission-sandbox.yml/badge.svg?branch=main)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/admission-sandbox.yml)
[![Supply-chain security](https://github.com/arthurmaciel/ipe-lang/actions/workflows/security.yml/badge.svg?branch=main)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/security.yml)
[![Static binaries](https://github.com/arthurmaciel/ipe-lang/actions/workflows/static.yml/badge.svg?branch=main)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/static.yml)
[![No-panic](https://github.com/arthurmaciel/ipe-lang/actions/workflows/panic-scan.yml/badge.svg?branch=main)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/panic-scan.yml)
[![Docs deploy](https://github.com/arthurmaciel/ipe-lang/actions/workflows/docs-pages.yml/badge.svg?branch=main)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/docs-pages.yml)

# Ipê language

> [!CAUTION]
> Although most features work, the code is under a thorough review that may last
> 3–4 months. Please consider [supporting the project](#support) so it is ready sooner.

**Ipê** (pronounced [/ip'e/](https://ipa-reader.com/?text=%09ip%E2%80%B2e&voice=Vitoria)) is a
pure-functional language with that extends [Elm](https://elm-lang.org/)'s syntax, partially implement
[Sky lang](https://sky-lang.org/) standard library and compiles to Rust. 

It aims to be a community-centered programming language — check out our [principles](PRINCIPLES.md)
to learn more abou it.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/install.sh | sh
```

## Quickstart

```sh
ipe init counter        # scaffolds a served web app — the default shape
cd counter
ipe run                 # serves at http://localhost:8000 (server-rendered HTML + live SSE)
```

On a TTY `ipe init` asks the shape (`web` / `tui` / `cli` / `server` / `script`) and, for web, the
runtime — `served` (a co-located SSR + SSE server) or `solo` (a wasm client); name them to skip
the wizard: `ipe init myapp web solo`. → [getting started](docs/guide/getting-started.md)

## Performance (dev loop)

Wall-clock, measured by [`tools/scripts/perf/bench.sh`](tools/scripts/perf) on the reference
served counter with the released binary:

- **App recompilation:** ≈ 10 seconds — needed only for a **type** change (a `Model` field, a signature).
- **Dev watch hot reload:** ≈ 500 **milliseconds** — every other edit (text, `init`, `update`, subscriptions, styles) hot-swaps into the running app, no `cargo`.
- Cold build ≈ 18 s · release binary 7.0 MB · peak RAM 7.8 MB. → [faster builds](docs/topics/faster-builds.md)

## Shapes

One language; the shape is pinned by the head of `main`, never by config. **Web — served
SSR + SSE — is the default**, tuned for fast web development, and the same `Ipe.Ui` view renders
on the terminal too.

| Shape | Entry | For |
|---|---|---|
| **Web** *(default)* | `Web.tea` | Server-rendered HTML + live SSE patches; a `solo` wasm client is opt-in |
| Tui | `Tui.tea` | Full-screen terminal UIs |
| Cli | `Cli.tea` | Line-oriented CLIs and REPLs |
| Worker | `Worker.tea` | A view-less TEA loop |
| Script | bare `main : Task Error ()` | Scripts, one-shot tools, cron — and where you call `Server.listen` |

Web / Tui / Cli / Worker follow [The Elm Architecture](https://guide.elm-lang.org/architecture/);
Web, Tui, and Cli share one `view : Model -> Element Msg`. → [examples](examples/), `ipe doc Ui`

A server is not its own runtime shape — you mount a web TEA app and hand-written endpoints into
one `Ipe.Http.Server` on a single port (the `ipe init … server` scaffold seeds exactly this):

```elm
main =
    Server.listen 8080
        [ Server.get "/api/health" health
        , Server.mountApp "/app"
            (Web.embed
                -- the same six fields as `Web.tea`, written inline
                { init = init, update = update, view = view
                , subscriptions = subscriptions, routes = [], notFound = NoOp
                }
            )
        ]
```

## Features

- **Elm syntax** — Hindley–Milner inference, exhaustive `case`, immutable data; no `null`, no runtime exceptions.
- **Batteries-included stdlib** — web (SSR + SSE), typed HTTP and SQL, auth, email, cache, pub/sub, WebSockets — all behind one `Task Error a` boundary with a typed `Error`. → `ipe doc`
- **Compiles to readable Rust**, incrementally (salsa); `ipe watch` hot-swaps most edits and recompiles only on a type change. → [faster builds](docs/topics/faster-builds.md)
- **No authored abrupt failure** — the compiler and runtime carry no `panic!` / `unwrap` / `expect` / index panic; every failure is a typed `Result` or diagnostic. → [PRINCIPLES.md](PRINCIPLES.md)
- **Capabilities are inferred, not declared** — `ipe capabilities <entry>` reports exactly what a program may do (network, fs, env, ffi, …). → [capabilities](docs/reference/capabilities.md)
- **Accessible by default** — real `<button>`s, semantic landmarks, a contrast-safe focus ring, and reduced-motion honored out of the box. → `ipe doc Ui`
- **Rust FFI** — `ipe rust add <crate>` binds a crate as a generated `Rust.<Crate>` interface (sandbox-inspected; discloses the `native-ffi` capability). → [dependencies](docs/guide/getting-started.md)
- **Delivery grammar** (replaces the retired `ipe pack`) — `ipe build web desktop|ios|android` for a fast dev bundle, `ipe release web desktop|ios|android` for a production distributable (desktop-webview or mobile system-webview shell).
- **Eject to plain Rust** — `ipe eject` vendors and tree-shakes the runtime into a standalone Cargo project you build with no `ipe` toolchain.
- **Static binary** — `ipe build --static` produces a fully-static musl single binary — copy and run anywhere. → [static compilation](#static-compilation)

## Tooling

- `ipe doc` — reference documentation from source (json / markdown / html; runs with or without a project).
- `ipe lint` / `ipe lint --fix` — advisory static analysis, configured by a `lint.ipe`. → [lint guide](docs/guide/lint.md)
- `ipe lsp` — completion, go-to-definition, find-references, rename, code actions, semantic tokens over stdio. → [editor setup](docs/topics/editor-integration.md)
- `ipe fmt` · `ipe test` · `ipe verify` · `ipe migrate` · `ipe package audit` — format, test, whole-project gate, migration, and the publish quality gate.

## Static compilation

`ipe build --static` produces a fully-static musl binary (zero runtime dependencies). Once:
`rustup target add x86_64-unknown-linux-musl` and install `musl-tools`. `x86_64` is CI-verified;
`aarch64` is wired pending toolchain confirmation. → [faster builds](docs/topics/faster-builds.md)

## Support

Contributions are **very** welcome, in order of current need:

- **Donations** — [support Ipê's development](https://ko-fi.com/arthur_maciel??g=1). Thank you!
- **Pull requests** — most valuable are security / soundness fixes (a mis-compilation, a panic on valid input, an unsound emit). Every PR must be human-reviewed before submission — there is not enough time to review unsupervised AI code.
- **Bug reports** — [report any bug you find](https://github.com/arthurmaciel/ipe-lang/issues).

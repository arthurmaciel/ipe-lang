<div align="center">
    <img width="250" height="251" alt="Yellow Ipê (Handroanthus serratifolius)" src="https://github.com/user-attachments/assets/870f8739-69ab-4b05-af6a-b56c3e615e1c" />
</div>

<br />

[![CI](https://github.com/arthurmaciel/ipe-lang/actions/workflows/ci.yml/badge.svg)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/ci.yml)
[![admission-sandbox](https://github.com/arthurmaciel/ipe-lang/actions/workflows/admission-sandbox.yml/badge.svg)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/admission-sandbox.yml)
[![security](https://github.com/arthurmaciel/ipe-lang/actions/workflows/security.yml/badge.svg)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/security.yml)
[![static](https://github.com/arthurmaciel/ipe-lang/actions/workflows/static.yml/badge.svg)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/static.yml)
[![install-smoke](https://github.com/arthurmaciel/ipe-lang/actions/workflows/install-smoke.yml/badge.svg)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/install-smoke.yml)


# Ipê language

> [!CAUTION]
>
> Although many of the features are working, the
> code is under a thorough review that may last 3 to 4 months.
>
> Please consider
> [supporting our project](https://github.com/arthurmaciel/ipe-lang#support) so we get ready soon :)

**Ipê**, pronounced [/ip'e/](https://ipa-reader.com/?text=%09ip%E2%80%B2e&voice=Vitoria), is a "thick-barked" [tree](https://en.wikipedia.org/wiki/Handroanthus_serratifolius) native from South and Central Americas. 

The Ipê programming language aims to be a community-centered programming language.  Check out our [principles](https://github.com/arthurmaciel/ipe-lang/blob/main/PRINCIPLES.md) to understand more about our social and technical values.

It pairs [Elm](https://elm-lang.org/)'s syntax with [Sky](https://sky-lang.org/)'s batteries-included
standard library - effect system, and application framework (web, API, CLI,
terminal, desktop) that turn a pure-functional language into a full-stack one.
It compiles to readable Rust.

Installation:
```sh
curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/install.sh | sh
```

```sh
ipe init counter          # or `ipe init .` to scaffold in the current directory
cd counter
ipe run                   # serves the counter at http://localhost:8000
```

Prefer to start from scratch? A minimal script program is just:

```elm
-- src/Main.ipe
module Main exposing (main)
import Ipe.Io as Io

main =
    do
      Io.println "Hello from Ipê!"
```

```sh
ipe run src/Main.ipe        # compile + run in one step
ipe type-check src/Main.ipe # type-check only — no build, no run
ipe verify                  # the whole project gate: format, type-check, build
```

Prefer building from source? 

```sh
git clone https://github.com/arthurmaciel/ipe-lang
cd ipe-lang
cargo build --release

```

## Contents

- [Features](#features)
- [Code shapes](#code-shapes)
- [Capabilities](#capabilities)
<!-- - [Dependencies](#dependencies)-->
- [Editor setup (LSP)](#editor-setup-lsp)
- [Static compilation](#static-compilation)
- [Support](#support)

## Features

- **Elm syntax** — pure functions, Hindley–Milner type inference, exhaustive
  `case`, immutable data. No `null`, no runtime exceptions.
- **Sky's batteries-included standard library** — Web live applications (SSR + real-time), typed HTTP, 
  typed SQL, auth, email, cache, pub/sub, and WebSockets, all behind a
  single `Task Error a` effect boundary. `Error` is a typed, classified value
  you construct and inspect — see [Errors](docs/language/error-handling.md).
- **Rust compiler** — the compiler itself is written in Rust: fast, parallel,
  memory-safe.
- **Rust backend** — emits readable Rust.
- **Incremental compilation** — a salsa-backed query engine; `ipe watch`
  recompiles only what changed.
- **Static compilation** — `ipe build --static` produces a fully-static musl
  single binary. Copy it anywhere and run — no runtime, no dependencies.
- **No authored abrupt failure** — the compiler's and runtime's own Rust carries
  no `panic!`, `unwrap`, `expect`, `assert!`, or indexing panic. Every failure is
  a typed `Result` or a diagnostic.

## Code shapes

One language, four ways to ship. Pick the entry point that matches your app.

| Shape | Entry point | Use it for | TEA |
|---|---|---|---|
| [`Ipe.Tea.Web`](docs/shapes/web.md) | `Web.app` | Web apps — server-rendered HTML, real-time SSE patches, sessions | ✓ |
| [`Ipe.Tea.WebView`](docs/shapes/webview.md) | `WebView.app` | Native desktop apps | ✓ |
| [`Ipe.Tea.Terminal`](docs/shapes/terminal.md) | `Terminal.appScreen` / `Terminal.appLines` | Terminal UIs (`appScreen`) and line-oriented REPLs (`appLines`) | ✓ |
| [`Program`](docs/shapes/program.md) | plain `main` | Scripts, one-shot tools, cron jobs, HTTP servers | |

The three ✓ shapes follow [The Elm Architecture](https://guide.elm-lang.org/architecture/)
(`init` / `update` / `view` / `subscriptions`) — and Web, WebView, and
`Terminal.appScreen` share the **same `Ipe.Ui` view code**, so one
`view : Model -> Element Msg` renders on web, desktop, and terminal.
See [`docs/shapes/`](docs/shapes/README.md) for a guide to each shape, and
[`examples/`](examples/) for runnable programs.

Views are built from two vocabularies — the portable `Ipe.Ui` layout language
and the raw-DOM `Ipe.Html` — plus the security-gated `Ipe.Css`. See
[Views: Ui, Html, and Css](docs/language/ui.md) for how they relate, how to intermix
them, and static rendering.

Check [language documentation](docs/language/README.md).

## Capabilities

Every effect in Ipê flows through a capability-tagged kernel, so the compiler can
tell you exactly what a program is allowed to do — network, filesystem, env,
subprocess, clock, random, native-ffi — from its code alone, with nothing to
declare. 

`ipe capabilities <entry>` prints that inferred set as a human report by
default; `--plain` gives the bare names, one per line, for a script:

```
$ ipe capabilities --plain examples/sky/ipe/02-go-stdlib/src/Main.ipe
network
clock
```

The set is generated, not hand-written, and cannot drift: a program that reaches
a new effectful kernel gains the matching capability automatically. 

`native-ffi`
appears whenever the program crosses into `Rust.` code, which is opaque to the
inference and the one place effects can escape the model.

See [**Capabilities**](docs/language/capabilities.md) for the full model.

<!--
## Dependencies

A project declares its dependencies in `package.ipe`. Three builders, each optional:

```elm
package =
    Package.named "my-app"
        |> Package.dependencies              -- Ipê packages
            [ Package.dep "http" "^1.2"      -- from the package index, by semver requirement
            , Package.depGitRev "mylib" "https://example.com/mylib.git" "abc123"
            , Package.depPath "local" "../local"
            ]
        |> Package.rustDependencies          -- Rust crates, bound as a foreign-function interface
            [ Package.rustDep "uuid" "1.10" ]
        |> Package.declares                  -- the capabilities you declare the program exercises
            [ Capability.network, Capability.clock ]
```

**Rust crates** are managed by the `ipe rust` command group:

```
$ ipe rust add uuid@1.10        # inspect and cache a crate
$ ipe rust remove uuid          # drop it
$ ipe rust install              # (re)inspect every Package.rustDependencies crate
```

Each crate is inspected inside a sandbox before it is trusted, and its
`Rust.<Crate>` interface is generated for you — no hand-written bindings.

**Ipê packages** are managed by `ipe add` / `ipe remove`:

```
$ ipe add http-extras           # resolve the latest published version
$ ipe add http-extras@^1.2       # or pin a semver requirement
$ ipe remove http-extras         # drop it from package.ipe and ipe.lock
```

`ipe add` resolves the package through the **curated index** (a git repository):
it reads the package's entry, picks the highest published version satisfying your
requirement, fetches that version's source at its pinned revision, and **verifies
the fetched source's sha256 against the hash the index pinned** before trusting
it — a mismatch is a hard error, never a warning, and nothing is written. It then
records the exact pins in `ipe.lock` and the requirement in `package.ipe`, and prints
the resolved version and its capability set (loudly, when a package uses
`native-ffi`).

`ipe.lock` pins the resolved version, source, revision, and content hash of every
dependency, so a build is reproducible from the lock even when the index is
unreachable, and a later build re-verifies the same source.

The `{ git = … }` and `{ path = … }` escapes bypass the index (for a private repo
or a local checkout) but still carry lockfile integrity — the fetched or copied
tree is hashed and locked exactly as an index dependency is.

The index checkout defaults to a standard per-user location; set `IPE_INDEX_DIR`
to point at a different checkout (a local fixture index, for offline testing).

## Auditing a package before you publish

`ipe package audit` runs the package **quality gate** on your working package and
exits non-zero with a single diagnostic naming exactly what is wrong. It is the
same gate the curated index re-runs when it accepts a version, so a green audit
means a green submission. Four checks, each a hard reject (never a warning that
lets an unsafe or dishonest version through):

- **Provenance** — no authored `panic!`/`unwrap`/`expect`/`assert` in the
  package's own FFI wrapper Rust (that code compiles unsandboxed into the shipped
  artifact, so an abrupt failure there is a soundness hole).
- **Capability honesty** — the `[capabilities]` you declare must be *exactly* the
  set the compiler infers: a capability you use but did not declare is a hidden
  effect (reject), and one you declared but never use is an over-broad claim
  (reject).
- **Enforced semver** — the public-API delta against the previous published
  version must clear the required bump; a breaking change under a mere patch bump
  is rejected. A first version has no predecessor and skips this check.
- **Supply chain** — `cargo-deny` (advisories, bans, sources) over the package's
  Rust dependency graph, plus a re-verification that every locked Ipê dependency
  still hashes to its pin.

```
$ ipe package audit                 # audit the current project
$ ipe package audit path/to/pkg     # or a specific package directory
```

A clean package prints `all Tier-1 checks passed`; a failing one names the check
and the offending line, capability, version, or dependency.
-->

## Documenting a package

`ipe doc` generates reference documentation for a package from its own source.

Your own project modules come first, under a **Project modules** heading, ahead
of the **Standard library** — so you see your API before the stdlib.

```
$ ipe doc                          # write doc/ (docs.json + Markdown + HTML) for the current project
$ ipe doc path/to/pkg --out site   # or a specific package, to a chosen directory
$ ipe doc --format html            # write only one rendering (markdown | json | html | all)
$ ipe doc serve                    # build the HTML site and preview it on loopback
$ ipe doc check                    # a CI gate: exit non-zero if a binding is undocumented
```

## Editor setup (LSP)

`ipe lsp` speaks JSON-RPC over stdio — type-directed completion, go-to-definition,
find-references, rename, formatting, code actions, semantic tokens, and more.
See [editor integration documentation](docs/editor-integration.md) for setup
instructions covering Helix, Neovim, VS Code, Emacs (lsp-mode and Doom Emacs),
and Zed.

## Ejecting to plain Rust

`ipe eject` emits a self-contained Rust Cargo project you can `cargo build` with
no `ipe` toolchain installed — the escape hatch from the runtime-crate model.


Unlike `ipe build`, which emits a project that names the Ipê runtime as a
dependency, `ipe eject` **vendors** the runtime source into the output and **tree-shakes**
it to only the modules your program reaches. 

The result is small, offline-buildable,
and auditable: plain, reviewable Rust with no external runtime path — ideal for a
Rust-only shop that must comply with a "Rust only" rule.

```sh
# Eject the program-shape example into a standalone project:
ipe eject examples/shapes/program/release-preflight/package.ipe --out /tmp/eject-demo

# Build it with plain cargo — no ipe toolchain required:
cd /tmp/eject-demo
cargo build --release
```

A program that binds a foreign Rust crate
(FFI) **cannot** be ejected (its external crates would need a registry fetch, which
the source-only contract forbids).

## Static compilation

`ipe build --static` produces a fully-static musl binary — zero runtime
dependencies, copy and run anywhere.

```sh
# Prerequisite (once):
rustup target add x86_64-unknown-linux-musl
sudo apt-get install musl-tools   # or equivalent on your distro

# Build a static binary (x86_64 Linux, dlmalloc allocator — the default):
cd examples/sky/ipe/01-hello-world
ipe build package.ipe --out out/rust --static
cd out/rust
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

## Faster builds

`ipe build` / `ipe run` compile an emitted Rust project, so most of the time is
`rustc` + linking. 

A failed emitted-crate compile is a non-zero `ipe` exit with a
named build-failure diagnostic, never a silent success. 

Optional per-machine tools — a compilation cache
([sccache](https://github.com/mozilla/sccache)), a fast linker
([mold](https://github.com/rui314/mold) / [lld](https://lld.llvm.org/)), and a
fast debug codegen backend
([cranelift](https://github.com/rust-lang/rustc_codegen_cranelift)) — cut that
substantially. See [rust performance improvement](docs/rust-perf-improvement.md)
for per-platform install and `~/.cargo/config.toml` recipes.

## Support

Contributions are **very** welcome!

There are 3 main forms to support our project. They are listed in order
of need at the current moment:

### Donations

I'd love to spend more time developing and battle testing Ipê. If you like the project, please 
[support Ipê's development](https://ko-fi.com/arthur_maciel??g=1). Thank you!


### Pull requests
The most valuable [pull requests](https://github.com/arthurmaciel/ipe-lang/pulls) are
**security/soundness fixes** — a mis-compilation, 
a panic on valid input, an unsound emit or security brech. 

Every `PR` must be human-reviewed before submitted please! Unfortunately there is not enough time to 
review unsupervised AI code :/

### Bug reports
Even if you can't propose any code yet, please [report](https://github.com/arthurmaciel/ipe-lang/issues)
 any bugs you find!

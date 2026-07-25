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
- [Dependencies](#dependencies)
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
- **No authored abrupt failure** — the compiler's and runtime's own Rust carries
  no `panic!`, `unwrap`, `expect`, `assert!`, or indexing panic: every failure is
  a typed `Result` or a diagnostic. Enforced by clippy denies plus a token-level
  scanner (which also gates generated and third-party FFI Rust), with a small,
  audited exception ledger. (`std` and third-party crate internals are outside
  this guarantee.)

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
declare. `ipe capabilities <entry>` prints that inferred set as a human report by
default; `--plain` gives the bare names, one per line, for a script:

```
$ ipe capabilities --plain examples/sky/02-go-stdlib/src/Main.ipe
network
clock
```

The set is generated, not hand-written, and cannot drift: a program that reaches
a new effectful kernel gains the matching capability automatically. `native-ffi`
appears whenever the program crosses into `Rust.` code, which is opaque to the
inference and the one place effects can escape the model.

See [**Capabilities**](docs/capabilities.md) for the full model — the eight
capabilities, how inference works, and how native code declares and is sandboxed.
Every command is human-friendly by default; data commands take `--plain` and
`--json` for scripts — see [**CLI output**](docs/cli-output.md).

## Dependencies

A project declares its dependencies in `ipe.toml`. Three sections, each optional:

```toml
[dependencies]              # Ipê packages
http  = "^1.2"              # from the package index, by semver requirement
mylib = { git = "https://example.com/mylib.git", rev = "abc123" }
local = { path = "../local" }

[rust.dependencies]         # Rust crates, bound as a foreign-function interface
uuid = "1.10"

[capabilities]              # the capabilities you declare the program exercises
declared = ["network", "clock"]
```

**Rust crates** are managed by the `ipe rust` command group:

```
$ ipe rust add uuid@1.10        # inspect and cache a crate
$ ipe rust remove uuid          # drop it
$ ipe rust install              # (re)inspect every [rust.dependencies] crate
```

Each crate is inspected inside a sandbox before it is trusted, and its
`Rust.<Crate>` interface is generated for you — no hand-written bindings.

**Ipê packages** are managed by `ipe add` / `ipe remove`:

```
$ ipe add http-extras           # resolve the latest published version
$ ipe add http-extras@^1.2       # or pin a semver requirement
$ ipe remove http-extras         # drop it from ipe.toml and ipe.lock
```

`ipe add` resolves the package through the **curated index** (a git repository):
it reads the package's entry, picks the highest published version satisfying your
requirement, fetches that version's source at its pinned revision, and **verifies
the fetched source's sha256 against the hash the index pinned** before trusting
it — a mismatch is a hard error, never a warning, and nothing is written. It then
records the exact pins in `ipe.lock` and the requirement in `ipe.toml`, and prints
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

## Editor setup (LSP)

`ipe lsp` speaks JSON-RPC over stdio — type-directed completion, go-to-definition,
find-references, rename, formatting, code actions, semantic tokens, and more.
See [`docs/editor-integration.md`](docs/editor-integration.md) for setup
instructions covering Helix, Neovim, VS Code, Emacs (lsp-mode and Doom Emacs),
and Zed.

## Static compilation

`ipe build --static` produces a fully-static musl binary — zero runtime
dependencies, copy and run anywhere.

```sh
# Prerequisite (once):
rustup target add x86_64-unknown-linux-musl
sudo apt-get install musl-tools   # or equivalent on your distro

# Build a static binary (x86_64 Linux, dlmalloc allocator — the default):
cd examples/01-hello-world
ipe build ipe.toml --out out/rust --static
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

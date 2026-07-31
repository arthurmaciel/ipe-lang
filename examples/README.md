# Ipê examples

This directory contains the Ipê-native first-party examples. Each is a
self-contained Ipê project (a `ipe.toml` manifest plus source under `src/`)
that builds with `ipe build` and targets the Rust backend.

## First-party examples

| Directory | Shape | What it demonstrates |
|-----------|-------|----------------------|
| `wasm/counter` | wasm/live | Basic TEA counter compiled to WebAssembly via `--target wasm`. The canonical "hello WASM" starting point. |
| `wasm/effects` | wasm/live | `Sub.every` timer and `Cmd.perform` side-effects exercised end-to-end in a browser via the Cmd/Sub bridge (gloo-timers). |
| `wasm/websocket` | wasm/live | `Ipe.WebSocket` client substitute: connect / onOpen / send / onMessage / close / onClose against a real WebSocket server in-browser. |
| `wasm/env-public` | wasm/live | `Ipe.Env.public` build-time config embedding: an allowlisted `API_BASE_URL` variable injected at compile time and readable in WASM at run time. |
| `wasm/spa` | wasm | SPA target: a pure-client single-page application with full TEA loop running in the browser. Uses `Web.app` which emits `wasm_app` under `--target wasm`. |
| `wasm/hydration` | wasm | SSR hydration: server-side initial render (paint) followed by WASM client takeover. |

## Shape demos

The per-shape demos linked from [`docs/shapes/`](../docs/shapes/) live under
`shapes/<shape>/`:

| Shape | Directory | What it demonstrates |
|-------|-----------|----------------------|
| terminal | `shapes/terminal/file-browser` | A keyboard-driven directory browser over `Terminal.appScreen`: `File.readDir` lists the working directory, arrow keys navigate, and the selected file's first bytes render as a raw `Ui.cells` hexdump island inside the `Ipe.Ui` view. |
| terminal | `shapes/terminal/http-shell` | An HTTP query shell over `Terminal.appLines`: each stdin line like `get <url>` performs a real `Http.get` and prints the response status + body. |
| web | `shapes/web/task-publish` | The top-level, Task-shaped `Ipe.PubSub.publish` (`String -> any -> Task Error Int`) fired from a `Ipe.Tea.Web` app's `update` via `Cmd.perform`, with the subscriber count routed back into the model. Shows the Task form composing where a broadcast bus runs. |
| program | `shapes/program/release-preflight` | A plain-`main` batch program (no TEA loop): a release-preflight check run to completion. |

## FFI examples

Shim-free auto-FFI bindings against real crates.io crates, grouped under
`ffi/`. These bind native GUI/ECS crates and are not part of the headless
example sweep.

| Directory | Binds | What it demonstrates |
|-----------|-------|----------------------|
| `ffi/bevy-game` | `bevy_ecs` | A headless ECS world tick over the shim-free auto-FFI: creates a `World`, threads it through real ECS maintenance operations, and reads the final entity count as observable state — every `World.*` call is a generated binding, zero hand-written Rust shims. |
| `ffi/iced-counter` | `iced` | A binding spike mapping Iced's Elm architecture onto Ipê's TEA over the real `iced` crate: the `Model`/`Message` struct + enum bindings are emitted and forwarded; the `update`/`view` closure-to-run wiring is in progress. |

## Sky-derived examples

The upstream `anzellai/sky` examples (00–39 + `simple` + `test_pkg`) are not
vendored here. They live as a declarative patch under `sky/` and are
materialised at sweep time:

- `sky/manifest.toml` — the authoritative list of upstream examples and their
  scope (`rename-map` patch, or excluded via `go_ffi = true`).
- `sky/rename-map.tsv` — the token-level `Sky.*`/`Std.*` → `Ipe.*` rename map
  applied by `scripts/lib/sky-to-ipe-transform.py`.
- `sky/ipe-patches/<name>.patch` — optional per-example semantic delta on top.
- `sky/README.md` — how the mirror + patch pipeline works.

Run `bash scripts/examples-sweep.sh` to materialise the mirrored examples under
`sky/<name>/`, patch them, and build + run each. The materialised trees are
git-ignored.

## Running an example

```sh
# Build and run a CLI example
ipe build ipe.toml --out out/rust
cargo run --manifest-path out/rust/Cargo.toml

# Build a WASM example (outputs to out/rust/www/)
cd examples/wasm/spa
ipe build ipe.toml --out out/rust --target wasm
```

Or use the sweep to build and run all in-scope examples at once:

```sh
IPE_SWEEP_BUILD_ONLY=1 bash scripts/examples-sweep.sh
```

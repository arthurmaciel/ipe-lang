# Ipê examples

This directory contains the Ipê-native first-party examples. Each is a
self-contained Ipê project (a `ipe.toml` manifest plus source under `src/`)
that builds with `ipe build` and targets the Rust backend.

## First-party examples

| Directory | Shape | What it demonstrates |
|-----------|-------|----------------------|
| `wasm-counter` | wasm/live | Basic TEA counter compiled to WebAssembly via `--target wasm`. The canonical "hello WASM" starting point. |
| `wasm-effects` | wasm/live | `Sub.every` timer and `Cmd.perform` side-effects exercised end-to-end in a browser via the Cmd/Sub bridge (gloo-timers). |
| `wasm-websocket` | wasm/live | `Ipe.WebSocket` client substitute: connect / onOpen / send / onMessage / close / onClose against a real WebSocket server in-browser. |
| `wasm-env-public` | wasm/live | `Ipe.Env.public` build-time config embedding: an allowlisted `API_BASE_URL` variable injected at compile time and readable in WASM at run time. |
| `wasm-spa` | wasm | SPA target: a pure-client single-page application with full TEA loop running in the browser. Uses `Live.app` which emits `wasm_app` under `--target wasm`. |
| `wasm-hydration` | wasm | SSR hydration: server-side initial render (paint) followed by WASM client takeover. |
| `console-repl` | console | An accumulator-calculator REPL over `Console.app`: each stdin line is a command (`add`/`sub`/`mul`/`reset`/`quit`) folded into a running total. |
| `tui-counter` | tui | A terminal counter over `Tui.app`, rendering the same shared `Ipe.Ui` view as the Web and WebView counters. |

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
cd examples/wasm-spa
ipe build ipe.toml --out out/rust --target wasm
```

Or use the sweep to build and run all in-scope examples at once:

```sh
IPE_SWEEP_BUILD_ONLY=1 bash scripts/examples-sweep.sh
```

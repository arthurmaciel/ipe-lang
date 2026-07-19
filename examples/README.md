# Ipê examples

This directory contains the Ipê-native first-party examples. Each is a
self-contained Ipê project (a `ipe.toml` manifest plus source under `src/`)
that builds with `ipe build` and targets the Rust backend.

## First-party examples

| Directory | Shape | What it demonstrates |
|-----------|-------|----------------------|
| `39-ffi-skyshop-core` | cli | Skyshop domain core ported to Rust FFI crates: order IDs from the real `uuid` crate via the shim-free auto-FFI bridge (`Rust.Uuid`), persistence via `Ipe.Db` SQLite (replacing Firestore). Proof of the shim-free FFI path end-to-end. |
| `40-wasm-counter` | wasm/live | Basic TEA counter compiled to WebAssembly via `--target wasm`. The canonical "hello WASM" starting point. |
| `41-money-allocate-regression` | cli | Regression suite for `Money.allocate`: zero-allocation guard (CO-INCR-001) and sign-correct residue distribution (CO-INCR-002). |
| `42-wasm-effects` | wasm/live | `Sub.every` timer and `Cmd.perform` side-effects exercised end-to-end in a browser via the M4 Cmd/Sub bridge (gloo-timers). |
| `43-wasm-websocket` | wasm/live | `Ipe.WebSocket` client substitute: connect / onOpen / send / onMessage / close / onClose against a real WebSocket server in-browser. |
| `44-wasm-env-public` | wasm/live | `Ipe.Env.public` build-time config embedding: an allowlisted `API_BASE_URL` variable injected at compile time and readable in WASM at run time. |
| `45-wasm-spa` | wasm | M6 SPA target: a pure-client single-page application with full TEA loop running in the browser. Uses `Live.app` which emits `wasm_app` under `--target wasm`. |
| `46-wasm-hydration` | wasm | M7 SSR hydration: server-side initial render (paint) followed by WASM client takeover. |

## Sky-derived examples

The upstream `anzellai/sky` examples (00–38 + `simple` + `test_pkg`) are not
vendored here. They live as a declarative patch under `sky/` and are
materialised at sweep time:

- `sky/manifest.toml` — the authoritative list of upstream examples and their
  patch status (`global-rename-map` or excluded via `go_ffi = true`).
- `sky/rename-map.tsv` — the token-level `Sky.*` → `Ipe.*` rename map applied
  by `scripts/equivalence-checks/sky-to-ipe-transform.py`.
- `sky/README.md` — how the mirror and patch pipeline work.

Run `IPE_SWEEP_MIRROR_SKY=1 bash scripts/equivalence-checks/examples-sweep.sh`
to materialise the mirrored examples under `sky/<name>/` and include them in
the sweep. The materialised trees are git-ignored.

## Running an example

```sh
# Build and run a CLI example
cd examples/41-money-allocate-regression
ipe build ipe.toml --out sky-out/rust
cargo run --manifest-path sky-out/rust/Cargo.toml

# Build a WASM example (outputs to sky-out/rust/www/)
cd examples/45-wasm-spa
ipe build ipe.toml --out sky-out/rust --target wasm
```

Or use the sweep to build and run all in-scope examples at once:

```sh
IPE_SWEEP_BUILD_ONLY=1 bash scripts/equivalence-checks/examples-sweep.sh
```

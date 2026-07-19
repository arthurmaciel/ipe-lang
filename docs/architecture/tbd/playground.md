# Ipê Playground — local setup

The playground is a split-pane browser UI (editor left, live preview right).
Clicking **Run** (or pressing `Ctrl+Enter` / `Cmd+Enter`) sends the source text
to the server, which compiles it to WASM and streams the bundle back into the
preview iframe — no page reload needed.

## Prerequisites

| Requirement | Why |
|---|---|
| Rust toolchain (`stable`) | builds the playground server and `ipe` itself |
| `wasm-pack` + `wasm32-unknown-unknown` target | `ipe build --target wasm` calls `wasm-pack` internally |
| `musl-tools` (Linux) | only needed for `--static`; not required for WASM |

Install the WASM toolchain once:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-pack
```

## Build

```sh
# 1. Build the ipe compiler binary.
cargo build --release -p ipe

# 2. Build the playground server.
cargo build --release -p ipe-playground
```

Both binaries land in `target/release/`.

## Run

The server needs three environment variables:

| Variable | What it points to |
|---|---|
| `IPE_BIN` | absolute path to the `ipe` binary |
| `IPE_RUNTIME_DIR` | absolute path to `src/runtime/rust/src` |
| `IPE_PLAYGROUND_STATIC_DIR` | directory that holds `index.html` (the playground UI) |

```sh
export IPE_BIN="$(pwd)/target/release/ipe"
export IPE_RUNTIME_DIR="$(pwd)/src/runtime/rust/src"
export IPE_PLAYGROUND_STATIC_DIR="$(pwd)/src/playground/www"

./target/release/ipe-playground
# Listening on 0.0.0.0:3000
```

Open `http://localhost:3000` in a browser.

### Optional tuning

| Variable | Default | Effect |
|---|---|---|
| `IPE_PLAYGROUND_PORT` | `3000` | server port |
| `IPE_PLAYGROUND_TARGET_DIR` | `/tmp/ipe-playground-target` | shared warm cargo target (keep it across restarts for fast recompiles) |
| `IPE_PLAYGROUND_TIMEOUT_SECS` | `120` | per-compile subprocess timeout |

Set `IPE_PLAYGROUND_TARGET_DIR` to a persistent path to avoid cold recompiles
on every restart:

```sh
export IPE_PLAYGROUND_TARGET_DIR="$HOME/.cache/ipe/playground-target"
```

## Edit-compile-preview loop

1. Edit Ipê source in the left pane (or paste any `.ipe` program).
2. Press **Run** (or `Ctrl+Enter` / `Cmd+Enter`).
3. The server compiles the source to WASM and injects the bundle into the
   preview iframe on the right.
4. Compile errors appear in the status bar; the preview shows the formatted
   diagnostics.

The first compile is slow (cargo builds all dependencies). Subsequent compiles
reuse the warm target directory and are significantly faster.

## Architecture note

Each compile runs `ipe build --target wasm` as an isolated subprocess.
CPU/memory limits are the responsibility of the operating environment
(cgroups / container runtime). The server itself imposes no kernel-level
sandboxing — that is a deployment concern. Source payloads larger than 1 MiB
are rejected before a subprocess is spawned.

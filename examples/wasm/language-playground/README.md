# Ipê language playground (in-browser Ipê → Rust, sandboxed run)

A split-pane playground. You type Ipê on the left, and the parse → resolve →
typecheck → lower → emit pipeline runs **in your browser** as a WebAssembly
module (the `ipe-wasm` crate). The right pane shows the emitted Rust or the
compiler diagnostics almost at once. **Run** sends the emitted Rust to a local
Ipê server, which builds and runs it inside a bubblewrap jail and sends the
program output back.

## Layout

| Path | What it is |
|---|---|
| `index.html` | the three-pane UI (Ipê source \| emitted Rust \| program output) |
| `pkg/` | git-ignored wasm-bindgen output the page loads |
| `setup/` | the setup program: wasm bundle, jail-runner install, offline cache warm |
| `server/` | an `Ipe.Server.Http` app: static files, `GET /health`, `POST /run` |
| `jail-runner/` | a Rust workspace member: the sandboxed build+run harness |

## Prerequisites

- The `ipe` compiler binary (build it from this repo: `cargo build -p ipe`).
- The `wasm32-unknown-unknown` rustup target and the `wasm-bindgen` CLI
  (`cargo install wasm-bindgen-cli --version 0.2.126`).
- For Run only: `bwrap` (bubblewrap).

The setup program checks for each tool. If one is missing, it stops with an
install hint and a non-zero exit.

## Setup

```sh
cd examples/wasm/language-playground/setup
ipe run              # full setup
ipe run -- bundle    # only rebuild the wasm bundle (pkg/)
```

The full setup:

1. builds `ipe-wasm` for `wasm32-unknown-unknown` (release) and runs
   `wasm-bindgen` into `../pkg/`;
2. installs `playground-jail-runner` to
   `~/.cache/ipe/playground/bin/jail-runner` with `cargo install --locked`;
3. warms the offline dependency cache (`jail-runner prewarm`, into
   `~/.cache/ipe/playground-warm`, or `$IPE_PLAYGROUND_WARM_DIR` if set).
   Warming needs the network once and takes about 2.4 GB. Every jailed build
   after that runs offline.

Every step runs as a direct argv vector, never through a shell.

## Run the server

```sh
cd examples/wasm/language-playground/server
ipe run
```

The server listens on port 8000 and serves the playground root and `/pkg`
statically. Open http://localhost:8000.

| Route | Answers |
|---|---|
| `GET /health` | `{"ok":true}`: the page enables Run only after this |
| `POST /run` | `{"rust": <emitted crate>}` → `{"ok", "unsandboxed", "output"}` |

`POST /run` fails with one of these statuses:

| Status | Meaning |
|---|---|
| 413 | the body is over 1 MiB |
| 400 | the body is not `{"rust": String}` |
| 422 | the staging allowlist refused the crate |
| 500 | the jail itself failed |

Environment:

| Variable | Effect |
|---|---|
| `IPE_PLAYGROUND_JAIL_RUNNER` | absolute path of another jail runner. A relative name is refused, never looked up on `PATH`. |
| `IPE_PLAYGROUND_WARM_DIR` | the warm cache directory |
| `IPE_HTTP_REQUEST_TIMEOUT` | the per-request deadline (default 30 s). The jail's wall-clock limit is set 5 s under it, so a jail never outlives its request. |

## How Run works

1. The in-browser compiler emits a Rust project: each file under a
   `// ==== path ====` banner, with the emitted `Cargo.toml` last.
2. The server checks the body's size before it decodes it. `server/src/Staging.ipe`
   then parses the banners into a staging plan. It accepts only `Cargo.toml` and
   `src/<segment>/…/<name>.rs`, where each segment is non-empty
   `[A-Za-z0-9_]`, with at most 64 files and 8 levels. It refuses a path
   outside that set, text before the first banner, a duplicate path, or a
   missing `Cargo.toml` / `src/main.rs`, and the whole request is rejected
   before any file is written.
3. `server/src/Runner.ipe` writes the plan under
   `~/.cache/ipe/playground-runs/<random token>/`. It then runs
   `jail-runner run <dir> --wall <secs> --warm <warm>` as a direct argv
   vector, with no shell.
4. `jail-runner` checks the layout again. It then builds the crate offline and
   runs the program inside a bubblewrap jail: network denied, host filesystem
   read-only, `prlimit` caps, a wall-clock kill, and (for the run phase) a
   seccomp filter that denies creating subprocesses. The jail is the
   `ipe_sandbox` crate the compiler SEAL uses.
5. The page shows the `── Build ──` / `── Run ──` / `── Error ──` transcript.

The security proofs live in `jail-runner/tests/sandbox_security.rs` and the
staging refusals in `server/tests/Main.ipe`.

## Static hosting (GitHub Pages)

The compiler pane needs no server. Deploy these three files, keeping their
relative layout:

```
index.html
pkg/ipe_wasm.js
pkg/ipe_wasm_bg.wasm
```

All asset and endpoint URLs are relative to the page, so it works under any
sub-path (e.g. `/compiler/playground/`). With no run server behind it, Run is
disabled with "Run needs the local server — see README". To point the page at
a run server elsewhere, add `?server=<url>`. A CI job builds `pkg/` with:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.126 --locked
cargo build -p ipe-wasm --target wasm32-unknown-unknown --release
wasm-bindgen --target web --no-typescript --out-dir pkg --out-name ipe_wasm \
  "$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/wasm32-unknown-unknown/release/ipe_wasm.wasm"
```

(`cd setup && ipe run -- bundle` runs the same steps.)

## Browser check

`tools/scripts/lib/playground-verify.mjs` drives the page in headless Chromium
(Playwright):

```sh
node tools/scripts/lib/playground-verify.mjs examples/wasm/language-playground   # Pages shape, no server
node tools/scripts/lib/playground-verify.mjs --live http://localhost:8000/       # Run through the jail
```

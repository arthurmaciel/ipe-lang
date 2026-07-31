# Ipê language playground (in-browser Ipê → Rust)

A static web page that runs the Ipê compiler **frontend** in your browser and
shows the Rust it emits. You type Ipê on the left; the parse → resolve →
typecheck → lower → emit pipeline runs as a WebAssembly module and, near
instantly, the right pane shows either the emitted Rust or the compiler
diagnostics. Everything happens client-side — there is no server.

This is the `ipe-wasm` crate (`src/wasm`) — the compiler frontend compiled to
`wasm32-unknown-unknown` — driven by a small ACE-editor page. It is not an
`ipe build --target wasm` runtime example; it builds through its own
`build.sh`.

## What it demonstrates

- The exposed `ipe-wasm` `compile(source)` surface, which returns a plain JS
  object `{ ok, diagnostics, emitted_rust }`:
  - `ok === true` → `emitted_rust` holds the emitted Rust project (each file
    under a `// ==== path ====` banner, then the emitted `Cargo.toml`).
  - `ok === false` → `diagnostics` holds the rendered compiler diagnostic.
- A live, debounced compile on every edit, plus a **Run** button and
  `Ctrl`/`Cmd`+`Enter` that compile immediately and show the emitted Rust.
- A theme selector that restyles both the ACE editor and the surrounding UI:
  the interface CSS variables are derived from the chosen ACE theme's editor
  colours, so light and dark themes both stay readable.

The compile is a pure function of the editor text. The module depends only on
the frontend crate graph — no `std::process`, no filesystem, no network — so
"compile in the browser" is a genuine result, not a stub. Turning the emitted
Rust into a running binary needs `cargo`/`rustc`, which cannot run in a
browser; that step is out of scope here (see "Not included" below).

## Build

Requires the `wasm32-unknown-unknown` rustup target and a `wasm-bindgen` CLI
matching the crate's `wasm-bindgen` version (`cargo install wasm-bindgen-cli`).

```sh
cd examples/wasm/language-playground
./build.sh
```

`build.sh` compiles `ipe-wasm` for `wasm32-unknown-unknown` (release) and runs
`wasm-bindgen` to generate the JS glue into `./pkg/` (a git-ignored build
artifact that `index.html` loads directly).

## Run

Serve this directory over HTTP with any static file server (the WASM module and
its JS glue must be fetched over `http://`, not `file://`) and open the URL it
prints:

```sh
npx serve .
# or
miniserve .
```

The ACE editor library itself is loaded from a CDN (`ace-builds` on jsDelivr),
so an internet connection is needed for the editor to appear; the compiler runs
entirely locally in your browser.

## Not included: server build + run

The tracking issue also envisions a **Run** affordance that ships the emitted
Rust to a server which builds and runs it, returning its output. Building and
running submitted Rust is remote code execution and must go through a hardened,
cross-platform sandbox (network off, filesystem jail, memory/CPU/time limits).
That server is **not** part of this example: it is deferred to a separate,
security-reviewed change so nothing here can execute submitted code. This
example is the safe, purely client-side compile preview; its **Run** button
compiles and shows the emitted Rust, and never runs it.

# The in-browser Ipê compiler

The Ipê playground compiles Ipê **in the browser**, with no server. This note
explains how: what runs as WebAssembly, what cannot and is stubbed, the
JavaScript API, and the playground's data flow. It is deliberately short —
follow the links for detail.

## What runs in the browser

The Ipê compiler emits **Rust**, and turning that Rust into a runnable binary
needs `cargo`/`rustc`, which cannot run in a browser. So "compile in the browser"
means the compiler **frontend** runs as WebAssembly:

```
Ipê source → parse → resolve → canonicalise → typecheck → lower → emit
           ⇒ diagnostics + emitted Rust (an in-memory EmittedProject)
```

`src/ipe-wasm` is that frontend, compiled to `wasm32-unknown-unknown` and
exposed to JavaScript with `wasm-bindgen`. It depends only on the frontend crate
graph — `ipe_parse`, `ipe_db`, `ipe_canon`, `ipe_types`, `ipe_lower`,
`ipe_backend`, `ipe_backend_rust`, and the embedded stdlib (`ipe_stdlib`, the
`src/stdlib` crate). None of those touches `std::process`, `std::fs`, or the
network at runtime, so the whole graph builds for wasm. The compile itself is a
demand over a cold in-memory salsa database — the exact query chain the native
CLI runs (`ipe`'s `compile_prepared`), so the browser is not a second,
divergent compiler.

## What is stubbed, and why (honest limitations)

These are genuine platform boundaries, documented, never faked results:

- **cargo/rustc** — impossible in a browser. The playground shows the emitted
  Rust; it does not build or run it.
- **`rustfmt`** — the backend normally pipes emitted files through the `rustfmt`
  subprocess for canonical output. A browser cannot spawn a process, so on
  `wasm32` the fmt pass is disabled (`ipe_backend_rust`'s `rust_fmt_disabled`
  returns `true`); the emitted Rust is valid, just not canonically formatted.
- **FFI** (`ipe_ffi`, `ipe_sandbox`) — needs the on-disk crate catalog and a
  native sandbox. `src/ipe-wasm` does not depend on them; FFI is disabled, and an
  FFI-backed import surfaces as an ordinary compiler diagnostic, not a crash.
- **filesystem project discovery** — no `ipe.toml`, no sibling-file discovery.
  The browser compiles a single entry module plus the transitive embedded-stdlib
  closure. Multi-file projects are a native-CLI feature.
- **watch / LSP / on-disk cache** — native-only; absent from the wasm module.

The compile target is `Target::WasmClient`, so the browser-bundle security gates
(server-effect kernels denied) are exactly the ones exercised.

## The JavaScript API

`wasm-bindgen` exports one function:

```js
import init, { compile } from './pkg/ipe_wasm.js';
await init();                       // load + instantiate the wasm module
const r = compile(sourceString);    // { ok, diagnostics, emitted_rust }
```

- `ok` — `true` when the frontend accepted the program.
- `diagnostics` — the rendered compiler diagnostic (colour off) when `ok` is
  `false`; empty otherwise.
- `emitted_rust` — the emitted project's files, each under a `// ==== path ====`
  banner, followed by the emitted `Cargo.toml`; empty when `ok` is `false`.

The Rust side (`src/ipe-wasm/src/lib.rs`) parses the entry once to learn its
module path, injects the stdlib closure (`src/ipe-wasm/src/stdlib_inject.rs`,
pure in-memory), builds the salsa database, and demands `ipe_db::emit_manifest`.
It never panics: every fallible step becomes a rendered outcome.

## The playground

`examples/wasm/language-playground/` is a static page (`index.html`) that loads
the wasm module and:

- edits Ipê in an **ACE editor** (Haskell highlighting, the closest bundled mode);
- offers a **theme switcher over every ACE theme** that re-themes both the editor
  and the surrounding UI (interface colours are derived from the chosen theme's
  editor background/foreground);
- **compiles live** (debounced) via `compile()`, showing emitted Rust or
  diagnostics.

Data flow: `editor change → debounce → compile(source) → render ok?emitted:diagnostics`.
Everything is client-side; the only network use is loading ACE from a CDN.

### Building it

`examples/wasm/language-playground/build.sh` builds `ipe-wasm` for
`wasm32-unknown-unknown` and runs `wasm-bindgen` (target `web`) into `pkg/`
(a gitignored build artifact). The `.github/workflows/playground.yml` workflow
rebuilds and headless-verifies the playground on any compiler change, and
publishes it to GitHub Pages from the default branch — so the wasm always tracks
the current compiler.

Because a native host config can set a system linker (`-fuse-ld=mold`) that the
wasm linker (`rust-lld`) rejects, the repo `.cargo/config.toml` overrides
`rustflags` for the `wasm32-unknown-unknown` target only.

# Roadmap

Where Ipê came from, and where it's going.

## Shipped

- **The compiler** — parser → canonicalizer → Hindley–Milner type checker →
  lowering → Rust backend, written in Rust.
- **THE SEAL** — if `ipe` accepts a program, the generated Rust `cargo build`s.
  Enforced by a golden-fixture suite and a full example sweep.
- **The stdlib** — Elm-shaped core (String/List/Dict/Maybe/Result/…) plus
  Sky's batteries: HTTP, Live (SSR + real-time), SQL, auth, email, cache,
  pub/sub, WebSockets — one `Task Error a` effect boundary.
- **Five app shapes** — `Ipe.Live`, `Ipe.Http.Server`, `Ipe.Cli`, `Ipe.Tui`,
  `Ipe.Webview` — three of them (Live/Tui/Webview) share one `Ipe.Ui` view.
- **Incremental compilation** — a salsa-backed query engine; `ipe watch`
  recompiles only what changed.
- **Static compilation** — `ipe build --static` emits a fully-static musl
  single binary. No runtime, no dependencies.
- **WASM floor** — the pure kernel set and the whole `Ipe.Ui` render surface
  compile to `wasm32-unknown-unknown`; a target-keyed security gate and a
  browser DOM sink prove a real `Ipe.Ui` app running in-browser.
- **LSP** — `ipe lsp` serves live diagnostics, hover, and document symbols
  over the incremental salsa graph.
- **FFI (sync ladder)** — `ipe add <crate>` auto-generates typed bindings to a
  real Rust crate, shim-free, through a sandboxed inspector.
- **A whole-codebase soundness audit** — every finding root-caused and
  closed: injection-hardened FFI decode, closed exhaustiveness gaps, bounded
  recursion on untrusted input, corrected Money/JWT edge cases.
- **CI + releases** — cross-platform binaries (Linux/macOS/Windows/FreeBSD),
  a one-line installer, green CI on every push.

## Next

- **WASM, past the floor** — the browser effects bridge (`Cmd`/`Sub`/`Http`/
  timers), module partitioning for client/server code, a client router, and
  SSR hydration.
- **FFI, past sync crates** — an async bridge so `ipe add` can bind real
  async crates (Stripe, Firestore, …), not just synchronous ones.
- **Static compilation, more targets** — aarch64, and a fully C-free build.
- **Kernel-wiring pass** — a few stdlib modules still have hand-written pure
  logic that should route through the (already correct) Rust kernels instead.
- **Behavioral parity CI** — the Go-backend equivalence sweep, running on
  every push, not just locally.
- **Longer horizon** — a native Ipê lint tool, exhaustiveness-aware
  wildcard warnings, and co-located property-based tests.

Open items are tracked as GitHub issues — see
[issues](https://github.com/arthurmaciel/ipe-lang/issues).

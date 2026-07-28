# Roadmap

Where Ipê came from, and where it's going.

## Shipped

- **The compiler** — parser → canonicalizer → Hindley–Milner type checker →
  lowering → Rust backend, written in Rust.
- **THE SEAL** — if `ipe` accepts a program, the generated Rust `cargo build`s.
  Enforced by a golden-fixture suite and a full example sweep.
- **The stdlib** — Elm-shaped core (String/List/Dict/Maybe/Result/…) plus
  Ipe's batteries: HTTP, Live (SSR + real-time), SQL, auth, email, cache,
  pub/sub, WebSockets — one `Task Error a` effect boundary.
- **Five app shapes** — the four TEA shapes `Ipe.Tea.Web`, `Ipe.Tea.WebView`,
  `Ipe.Tea.Tui`, `Ipe.Tea.Console`, plus the plain `Program` — three of them
  (Web/WebView/TUI) share one `Ipe.Ui` `view : Model -> Element Msg`.
- **Incremental compilation** — a salsa-backed query engine; `ipe watch`
  recompiles only what changed.
- **Static compilation** — `ipe build --static` emits a fully-static musl
  single binary. No runtime, no dependencies.
- **WASM floor** — the pure kernel set and the whole `Ipe.Ui` render surface
  compile to `wasm32-unknown-unknown`; a target-keyed security gate and a
  browser DOM sink prove a real `Ipe.Ui` app running in-browser.
- **LSP** — `ipe lsp` serves live diagnostics, hover, document symbols, and
  type-directed completion (candidates filtered and ranked by the type the
  cursor context expects) over the incremental salsa graph.
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
- **FFI, async SDKs** — the async bridge binds real SDK crates
  (`async-stripe`, `firestore`, `rs-firebase-admin-sdk`): async methods lower
  to `Task Error a` call sites, cross-crate builder params admit by resolved
  type identity, and the full skyshop storefront (`examples/sky/ipe/13-skyshop`)
  builds shim-free through THE SEAL. Remaining: broaden the honest-drop set
  (trait-generic params, fallible typed IDs).
- **Static compilation, more targets** — aarch64, and a fully C-free build.
- **Kernel-wiring pass** — a few stdlib modules still have hand-written pure
  logic that should route through the (already correct) Rust kernels instead.
- **Behavioral parity CI** — the Go-backend equivalence sweep, running on
  every push, not just locally.
- **Longer horizon** — a native Ipê lint tool, exhaustiveness-aware
  wildcard warnings, and co-located property-based tests.

Open items are tracked as GitHub issues — see
[issues](https://github.com/arthurmaciel/ipe-lang/issues).

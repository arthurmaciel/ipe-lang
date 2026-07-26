# 42. WASM client target as a cargo target of the single Rust backend

Date: 2026-07-25

## Status

Accepted and implemented. The client-WASM target ships: the runtime sink lives
under `src/runtime/rust/src/wasm/`, emission reuses `src/compiler/backend/rust`,
and the effect gate is `src/compiler/canon/src/target_gate.rs` (`Target::WasmClient`).
Server-compile playground delivery (the "B1" tier) ships alongside it
(`src/playground/`); a fully-client in-browser interpreter is a tracked follow-up.

## Context

Ipê programs are Elm-shaped: an `init`/`update`/`view` app driving a virtual DOM.
Running that app in a browser requires compiling Ipê to WebAssembly. The obvious
temptation is a second code generator — Ipê IR straight to WASM — but Ipê already
has one backend (Ipê → Rust), one ported runtime (virtual-DOM diff/render, Decimal,
Json, Regex, chrono), and one no-runtime-panic contract. A second codegen path
would fork all three and double the surface that must be proven correct.

The harder problem is security. A client bundle is fully readable by anyone who
loads the page, so a server-only effect reaching a browser build is not a bug to
lint — it is a credential leak. `File.*`, `Process.run`, server SQL and connection
strings, token signing/verification, the HTTP *server* and its session stores,
`System.getenv`/`exit`, `Email.send`: none of these may compile into a public bundle,
and "we ran a linter" is too weak a guarantee for a secret.

## Decision

Compile Ipê → Rust → `wasm32-unknown-unknown`, **reusing the Rust backend verbatim**.
WASM is a cargo target of the one backend, not a second codegen path. A new runtime
sink applies the same `Vec<Patch>` the existing `diff` produces to the real DOM via
typed `web-sys`, with delegated event listeners and one update+diff+patch per
`requestAnimationFrame`. Cmd/Sub map to a browser bridge shared with the interpreter
tier.

Rejected: a direct Ipê→WASM backend. It would fork emission, the no-panic contract,
and the security gate; abandon the ported runtime; and double the golden-oracle
surface — a correctness, soundness, and completeness regression for control the
target does not need.

The security boundary is a **three-layer gate**, applied in principle order
(security > correctness > soundness > efficiency > completeness > readability), and
built so that a server-only effect is *unrepresentable* — not merely diagnosed — in
a client module:

1. **Target-keyed kernel registry.** Under `Target::WasmClient`, server effects have
   no denotation at canonicalisation. The effect is absent from the language the
   module is checked against, so it cannot be named, let alone reached.
2. **Module partition + reachability closure.** Only the reachable client surface is
   admitted; a server module cannot be dragged in transitively.
3. **Emitted `Cargo.toml` dependency floor.** The generated crate cannot pull a
   server-only dependency, closing the gap below the language level.

Rejected as the v1 mechanism: a `Task`-capability-row carried through the type
system. The registry-plus-reachability gate makes the illegal state unrepresentable
without threading a capability row through every signature.

Two supporting decisions:

- **Content Security Policy.** WASM is eval-free; the app runs under
  `script-src 'self' 'wasm-unsafe-eval'` with no JS `'unsafe-eval'` — strictly
  tighter than a hand-written JS SPA. No eval-backed script-revival path is ported.
- **No-panic ⇒ no-trap, with one honest residual.** Guarded kernels keep the kernel
  trap class unreachable. Stack exhaustion from non-tail-recursive list ops on the
  smaller WASM stack is a reachable *structural* residual — caught, not prevented.
  Because `panic = "abort"` poisons the instance, the posture is log-and-die: a
  classified diagnostic (including a `StackOverflow` class and an error id) is
  emitted before the instance dies. Never a silent white screen; not a recovered UI
  either.

## Consequences

- There is exactly one backend, one runtime, one no-panic contract, and one security
  gate to audit — the WASM target inherits all four rather than reimplementing them.
- The security guarantee is structural, not advisory: a secret or server effect
  *cannot compile* into a public bundle, because the effect does not exist in the
  target's language. This invariant must hold for the target to stay safe — any new
  server effect must be registered target-keyed, or it silently becomes reachable.
- Browser substitutes for effect-tier kernels are admitted only where a browser
  analogue exists; where none does, the kernel is unrepresentable client-side, and
  that is the correct outcome, not a gap to paper over.
- A server-compile playground is nearly free once the target exists: a backend runs
  `ipe build --target wasm` and ships the bundle. A fully-client compile (front-end
  plus IR interpreter in WASM) is deferred to the interpreter tier and gated on an
  interpreter-≡-AOT differential-conformance invariant; compiling `rustc` itself to
  WASM is rejected.
- Open refinements tracked separately: the browser HTTP substitute crate choice, the
  JWT/bcrypt WASM crate maturity question, and an IndexedDB substitute module shape.

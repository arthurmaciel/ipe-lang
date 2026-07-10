The Rust backend development is guided by the following principles:

1. **Security** — generated code and runtime must give an attacker no foothold: no injection (SQL, shell, path, header, log), no secret leakage into logs or errors, no authentication or CSRF bypass, no timing oracle on a secret comparison, and no unbounded resource a remote party can exhaust. When a program handles untrusted input, the safe outcome must be the only reachable outcome.

2. **Correctness** — the backend must produce the right answer: for the same well-typed Sky program and the same input, the Rust output must match the Go reference's observable behaviour (ideally byte-for-byte), and any deliberate divergence must be documented rather than silently wrong. A program that compiles and runs but yields a value different from what the language semantics specify is incorrect.

3. **Soundness** — a well-typed Sky program must never be able to trigger a runtime failure in the generated Rust: no panic, no .unwrap()/.expect() blowup, no out-of-bounds index, no integer-overflow abort, no unchecked downcast, and no undefined behaviour. Where correctness is "the result is right," soundness is the stronger structural guarantee that "no input can make the program fall over" — the type system's promise is honoured all the way down to the binary.

4. **Efficiency** — within the bounds set by the three principles above, the code should be fast and lean: no needless allocation or cloning, no re-computation on hot paths, no O(n²) where O(n) is trivial, and a small binary and memory footprint. Efficiency is pursued only after security, correctness, and soundness, never by trading one of them away.

5. **Completeness** — the backend should cover as much of the Sky language and standard library as possible, so that real programs build and run without hitting an "unsupported" wall. A missing kernel or unimplemented feature is a completeness gap; it is a legitimate, documented limitation rather than a bug, but the goal is to keep shrinking that set.

6. **Readability** — The code (both the Haskell codegen and the generated Rust) should be clear, well-named, and maintainable, so the next person — human or agent — can understand and safely change it. It ranks last only in the sense that a readable name is never allowed to break correctness or a clean abstraction is never allowed to open a soundness hole; everything else being equal, the clearer form wins.
  
**The ordering is a strict tie-breaker, not a weighting**: whenever two principles conflict at a specific decision, the higher-numbered one yields to the lower — a faster path that opens a soundness hole is rejected, a more readable form that breaks correctness is rejected — so a lower principle can never justify compromising a higher one.

## The two fundamental rules

Independent of and beneath the ranked principles, every design and every code pass must obey two non-negotiable laws:

- **Parse, don't validate.** Convert untrusted or untyped input into a precise typed value at the boundary, once, so downstream code cannot re-encounter the unvalidated form. A function that takes a broad type and re-checks it everywhere is a smell; the check should happen once and produce a narrower type that makes the checked property structurally true thereafter. In this compiler: foreign/JSON/config values enter through a typed decode point; error channels are typed (`Diagnostic`/`Error`), never `String`.

- **Make invalid states unrepresentable.** Encode invariants in the types so an illegal combination cannot be constructed at all, rather than relying on a runtime guard or convention. Prefer a sum type over a bool-pair that admits impossible combinations; prefer an exhaustive `match` (no wildcard that silently swallows a new variant) so an unhandled case is a compile error; prefer a smart constructor over a public field that could hold an out-of-range value. In this compiler specifically: a kernel that the resolver recognises but the type-scheme table does not cover must be a compile-time error, never a silent flexible type variable that lets an ill-typed program pass the type-checker and fail only at the downstream Rust build (the "exit-0-then-cargo-fail" class).

These two rules are the structural machinery by which the ranked principles — especially Soundness and Correctness — are actually achieved: the ordering says *what wins in a conflict*; these two rules say *how you build code that doesn't create the conflict*.

## The seal — no exit-0-then-cargo-fail

THE SEAL is the project's core compiler mandate: **"If `skyc` accepts a program (exit 0), the emitted Rust MUST `cargo build`. Never emit codegen that type-checks in skyc but fails cargo."**

The seal is the make-invalid-states-unrepresentable rule applied to the compiler pipeline itself: a kernel the resolver recognises but the type-scheme table does not cover, an arity table that drifts from its callee table, a generic emitted where a concrete type was required — each of these is a representable-but-illegal pipeline state whose symptom is precisely an exit-0-then-cargo-fail. Closing that gap — making acceptance by `skyc` a structural proof that the downstream `cargo build` succeeds — is the point of most mechanical hardening items, and any new acceptance path (kernel, scheme, lowering arm, emitter case) must be sealed the same way: fail closed at `skyc` time, never open at `cargo` time.

## Security & soundness enforcement

The principles above are not aspirational — they are mechanically enforced at the crate level. The enforcement posture is **comply by construction**: when a lint fires, fix the code, never the lint level.

### Crate-level deny lints

- Root `Cargo.toml` `[workspace.lints.clippy]`: `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, `unreachable`, `todo`, `unimplemented`, `pedantic`, `nursery` — all `"deny"`.
- `runtime/Cargo.toml` `[lints.clippy]`: `unwrap_used`, `expect_used` deny; the only permitted exceptions are the 3 INFALLIBLE-tagged HMAC `#[allow]` sites.
- `runtime/src/lib.rs`: `#![cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::panic, clippy::unreachable, clippy::todo, clippy::unimplemented, clippy::panic_in_result_fn))]`. The only `#[allow(clippy::panic)]` sites are the 2 `ffi_polyfills` dynamic-dispatch fallbacks.
- Exactly ONE sanctioned `unsafe` block exists in the runtime — `prctl(PR_SET_PDEATHSIG)` in `live::console_proxy` — which is the reason there is no crate-wide `forbid(unsafe_code)`; every other module is `unsafe`-free.

### No `dyn Any` in emitted code — concrete over generic

The backend NEVER emits `dyn Any` / `.downcast` / type-erasure. Wildcard `any` is not polymorphism — it has exactly ONE concrete lowering (an opaque carrier type chosen per position, e.g. `Dict String String` in pub/sub payload position); only genuine named type variables (`a`, `msg`) become Rust generics, monomorphized by rustc at compile time. A generic emitted where a concrete type was possible passes a mechanical gate but can ship a silent runtime bug — always emit concrete when concrete is possible.

Current state: zero `dyn Any` in emitted-code paths (the former `OnRaw` `Arc<dyn Any>` exception was removed by #109/#156). Two documented runtime-internal *container* uses remain, both named exceptions: `runtime/src/sky_runtime/cache.rs` (value-erased `Box<dyn Any + Send>` store, downcast on `get`, documented infallible) and `runtime/src/sky_runtime/live/pubsub.rs` (TypeId-keyed broker registry container — the payload itself is never erased or downcast).

### Root causes only — no fake solutions

Never suppress a type error or warning; a defensive cover-up that hides a contract violation IS a violation. The guardian pre-final-gate outcome ladder governs every change: **clean → proceed**; **a principle is hurt → rethink and reimplement within the boundary**; **no adequate in-boundary fix exists → revert, log why, and signal the user** — never ship a silent workaround. "Pre-existing" / "known edge case" is never a shipping excuse (no-deferral): root-cause it or escalate it, never paper over it.

### Match the reference

Go/Haskell (`../sky`) parity is the default contract. Diverge ONLY where the divergence is strictly better under the principle order (Rust semantics, Unicode correctness, modern security posture) AND it is recorded in `docs/divergences-from-sky.md` with rationale and its own tests. A hack is never a "divergence".

Status: Accepted
Date: 2026-08-03

# 0054. Compilation performance: usage-gated dependencies compiled from source

## Context

A compiler that makes a hello-world program cost minutes of downstream `rustc`
loses its users before its semantics matter. The Efficiency principle covers the
toolchain's own footprint on the developer's machine, and the edit → run loop is
a first-class product surface. The stated budget: a common program compiles and
runs in **milliseconds** (dev loop) to **seconds** (release); a complex
application (server + database + UI) in a **few seconds** to **tens of seconds**.

`ipe build` / `ipe run` emit a self-contained Cargo project and hand it to
`rustc`. The dominant cost is `rustc` over the dependency closure, then over the
one emitted crate, then the link. Optional heavy surfaces (sqlx, axum, reqwest,
the crypto stack, …) were already gated, but the **floor was fixed**: every
emitted program pulled the same base `[dependencies]` — tokio, the serde stack,
regex, chrono, the crypto floor, url, uuid, and more. A bare `Io.println`
program resolved **105 crates** and cold-built in ~62 s, purely for capabilities
it never used.

The obvious shortcut — ship the runtime (and its dependency closure) **as
prebuilt binary artifacts** so the user only compiles their own program — was
considered and rejected (see Decision). The forces in tension: raw build speed
(Efficiency) versus the supply-chain, reproducibility, and audit properties of
compiling from source (Security), which the principle order ranks above
Efficiency.

## Decision

Reach the budget by **gating every optional dependency out of the floor and
compiling everything from source** — never by distributing prebuilt object code.
Four complementary mechanisms:

1. **Usage-driven dependency floor (feature split).** The emitted manifest and
   runtime module set are driven by function-level reachability: a program pulls
   only the crates its reachable kernels and reachable types actually need. Each
   optional root (serde/json, regex, chrono, the crypto floor, url, uuid,
   decimal, encoding, …) sits behind a runtime-crate cargo feature selected by a
   typed reachability predicate. A bare program approaches std-only.
2. **Precompiled runtime crate.** The runtime is emitted as a real dependency
   crate (materialised from the compiler's own embedded, version-matched source),
   not vendored inline — so an edit recompiles only the user's crate and relinks;
   the runtime artifact is reused.
3. **Build-once shared target.** A shared cargo target keyed by (compiler
   version, toolchain, feature set) compiles the dependency closure once per
   machine; every later project reuses the compiled artifacts. Reuse correctness
   is cargo's own fingerprinting — no new trust argument.
4. **IR interpreter for the dev loop.** `ipe run` for a supported (FFI-free)
   program executes the lowered IR directly, removing `rustc` from the edit → run
   loop entirely — the only path that reaches the true-milliseconds budget.
   `ipe build`, `--release`, and any FFI-bearing program keep the AOT path.

**Rejected alternatives — and why:**

- **Prebuilt binary artifacts (a shipped rlib closure / prewarmed cache).**
  Rejected on Security precedence. The runtime plus its closure is the majority
  of every shipped program's code; distributing it as object code means users
  execute artifacts they did not build, making signing, provenance, and
  reproducible-build verification load-bearing, and making us the CVE-response
  distributor for the entire pinned closure. Pinning dependency versions is
  independently good and is achieved by the blessed, source-level `Cargo.lock` —
  it does not require binary distribution. Acceptable only later as an optional,
  signed, reproducible-build-verified, opt-in accelerator — never the default
  trust path.
- **A prelinked dynamic library (`-C prefer-dynamic`).** Marginal over a
  linker-defaults improvement, and it adds an ABI-pinned moving part. An optional
  refinement, not a pillar.
- **A C-ABI runtime boundary.** Rejected on Soundness as well as Security. The
  emitted program crosses the runtime boundary in Rust-native types — generic
  `IpeTask`/`IpeResult`, payload enums, TEA closures over the user's model,
  generic collection kernels — none of which have a C representation. A C ABI
  would force type erasure, manual memory management, and `catch_unwind` at every
  entry point, converting a machine-checked safe boundary into a hand-audited
  `unsafe` one — to hide behind a wall only the monomorphic leaf kernels whose
  compile cost the shared target already amortises. It buys toolchain
  independence nobody needs at a Soundness price nobody should pay.

## Consequences

The budget is reached from source, with no new trust surface. Measured for a
bare `Io.println` program on a warm developer machine:

| Metric | Before | After the feature split | Ideal |
|---|---|---|---|
| Crates compiled | 105 | **3** (`app` + `ipe_runtime` + `libc`) | 1 |
| Cold build | ~62 s | **~6.8 s** | ~0.6 s |
| Warm rebuild (edit `main`) | ~2.4 s | **~0.34 s** | ~0.5 s |
| Binary | 1.06 MB | **0.37 MB** | 0.36 MB |

Three crates is the achieved floor — the ideal "1" would remove the runtime
crate and `libc`, which every program keeps; warm rebuild and binary size land
at the ideal. Optional surfaces stay **pay-for-use**: a JSON program measures
~14 crates, an HTTP server ~134 — each carrying only its own stack, none of it
charged to the bare program.

**The invariant that must continue to hold:** the emitted manifest, the emitted
runtime module set, and the selected feature set must agree, and must include
exactly what the reachable code needs — no less. Mis-dropping a needed crate is a
**compile-time drift error** under THE SEAL (`crate_specs_match_manifests`, the
runtime module-set and feature-set closure proofs), never a `cargo` failure on
the user's machine. The gating is fail-closed: an unknown consumer keeps the
crate; over-inclusion is the accepted precision loss. This ADR stays valid only
while every new optional surface is added through that gated, SEAL-guarded path.

The runtime crate is the irreducible dependency floor. The true-milliseconds dev
loop belongs to the IR interpreter (mechanism 4) alone; the AOT path's residual
cost is `rustc` over the user's own crate plus the link, which the shared target
and linker defaults minimise but do not remove.

This decision supersedes the working document `compilation-performance.md`, which
is removed; its strategy scoring and measured evidence are captured above.

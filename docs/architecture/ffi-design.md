# Sky → Rust FFI — architecture (design-with-it-in-mind)

> **Status:** standing design constraint. Written 2026-06-27.
> **Directive:** develop the Sky Rust compiler *with Sky→Rust FFI in mind* — the
> pipeline, IR, type system, backend, and Cargo emission must accommodate FFI from
> the start, even though FFI itself is implemented at a later milestone.
> **Reference:** the Haskell FFI subsystem — `Build/Rust/{Ffi,FfiInstance,FfiCall}.hs`,
> `Build/{FfiGen,FfiTypeParser,FfiRegistry}.hs`, and the `sky-ffi-inspect-rs`
> introspector. We mirror its *behaviour*, not its bytes.

## The one decision this forces now: kernels are a REGISTRY, not an enum

An FFI binding is a kernel whose signature came from **crate introspection**
instead of the stdlib. So stdlib kernels and FFI kernels **must share one
representation.** This promotes the previously-"backlog" kernel-registry decision
to a **committed invariant**:

- `sky_ir::Callee::Kernel(KernelId)` where `KernelId` is a resolved, opaque handle
  (interned) — **not** the M0 flat `KernelFn` enum (that was M0-only).
- A single **kernel registry** maps `KernelId → KernelEntry { sky_signature,
  per-backend emission, origin: Stdlib | Ffi{crate} }`. The stdlib populates rows
  at startup; **FFI populates rows from `.skyi` catalogues.** Same lookup, same
  call shape, same exhaustiveness story (a registry-coverage test + the
  `SKY-L0108` "kernel not available yet" fail-fast).
- Net: the IR needs **nothing FFI-specific** — an FFI call lowers to
  `Call { callee: Kernel(id), args }` exactly like a stdlib call. That is the
  payoff of designing the registry now instead of entrenching the enum.

Migrate the flat `KernelFn` → registry at **M4** (stdlib breadth), so FFI (its
sibling) drops in on the same rails.

## The FFI pipeline (mirrors the Haskell)

`sky add <crate>` →
1. **Introspect** — run `rustdoc --output-format=json` (or the `sky-ffi-inspect-rs`
   tool) on the crate in **its own process**; parse the JSON for exported fns,
   types, generics, trait bounds.
2. **Map types** — Rust type ⇄ Sky type (`FfiTypeParser` analogue). Opaque foreign
   types become Sky nominal `Con{ module, name }` (our `Ty::Con` already carries
   module+name+args, so this fits). Primitives map directly; `Result`/`Option`
   bridge to Sky `Result`/`Maybe`.
3. **Emit three artifacts** (cached under `.skycache/ffi/rust/`):
   - `<crate>.skyi` — Sky-typed signatures, so the **type checker** resolves FFI
     call sites (parse-don't-validate: an FFI call is type-checked against the
     `.skyi`, not validated later).
   - a **kernel-registry table** entry (`<crate>.kernel.json` analogue) — consumed
     by canon/lower to resolve the call to a `KernelId`.
   - `<crate>_bindings.rs` — the **wrapper**: Sky→Rust adapters (coercion, error
     wrapping) with each foreign call inside `catch_unwind` (panic → Sky `Err`).

## Where each compiler stage accommodates FFI

| Stage | FFI hook (build it expecting this) |
|---|---|
| `sky_parse` | FFI-imported names are ordinary qualified refs; no new syntax needed for *calling* them. `sky add` is a CLI/driver concern, not grammar. |
| `sky_canon` | resolve an FFI-qualified name to a `KernelId` via the registry (loaded from `.skyi`/table) — same path as a stdlib `VarKernel`. |
| `sky_types` | load FFI signatures from `.skyi` into the env; type-check FFI call sites against them. Opaque foreign types unify nominally. |
| `sky_lower` | FFI call → `Call { Kernel(id), args }`; thread concrete generic instantiations (Wall #2) into the registry entry so the wrapper generator can monomorphise. |
| `sky_backend_rust` | emit the call to the wrapper fn; **emit/collect the wrapper `.rs` into `EmittedProject.files`**; **inject the FFI crate into the emitted `Cargo.toml` `[dependencies]`**. |
| `skyc` (driver) | `sky add`/`install`/`remove`; run the inspector; manage `.skycache/ffi/`. |
| new `sky_ffi` crate | the inspector + type-parser + binding generator, **isolated** (see security). |

### Two things that must evolve from their M0 shape (foreseen, not blockers)
- **Emitted `Cargo.toml` becomes dynamic.** Today the backend embeds a fixed
  golden `CARGO_TOML` const. FFI requires injecting `[dependencies]` per the FFI
  crates a program uses → the const becomes a small generator (base manifest +
  computed FFI deps). Designed for; trivial when it lands.
- **`KernelFn` enum → registry** (above), at M4.

## Security model (principle 1 — load-bearing for FFI)

FFI is the **highest-risk surface** in the whole compiler. Gates (the guardian
ruled on this in the brainstorm):
- **Build-script / proc-macro ACE.** Introspecting or building a foreign crate
  runs its `build.rs` + proc-macros = arbitrary code at build time
  (supply-chain). `sky add` MUST emit an explicit warning and treat adding a crate
  as a trust decision. Run the inspector in its **own process**, **fail-closed** on
  any inspector error (never emit partial/guessed bindings).
- **No injection from crate metadata.** Foreign crate/type/fn names flow into
  generated Rust source — validate them as identifiers; **never** string-
  interpolate untrusted metadata into code unescaped (a malicious crate name must
  not break out of the wrapper). Same discipline as `Std.Ui`'s HTML escaping.
- **Soundness bridge (`catch_unwind`).** Every foreign call in the wrapper is
  wrapped so a panicking/aborting FFI fn becomes a Sky `Err`, preserving Sky's
  guarantee (b) — *well-typed Sky never panics* — **across** the FFI boundary.
  (Caveat: `panic = "abort"` defeats `catch_unwind`; document the build-profile
  requirement, mirror the Haskell's unwind-mode handling.)
- **Bounded.** A huge crate (Stripe-SDK-scale, 76k symbols in the Haskell
  benchmark) must not OOM/borrow the host — cap/stream introspection, DCE unused
  bindings before emission.

## Generics across FFI (Wall #2)

Demand-driven monomorphisation: a generic foreign fn called at concrete types gets
a per-instance wrapper synthesised from the call site. The **type-directed
lowering already threads concrete types** (our `SolvedTypes.regions` → `IrType`),
so lower passes the concrete instantiation to the registry entry; the binding
generator emits the monomorphic wrapper. The IR/type rep must therefore keep
foreign generic entries instantiable per call site (mirrors the Haskell
`FfiInstance`/`FfiCall`).

## Roadmap slot

FFI is a **sibling of stdlib breadth** (both ride the kernel registry):
- **M4** introduces the registry (stdlib kernels move onto it).
- **M4.5 / M5-adjacent**: the `sky_ffi` crate + `sky add` + dynamic Cargo emission
  + the security gates above. A real first milestone target is a small, safe crate
  (e.g. a pure-data crate) end-to-end, then scale toward the Stripe-SDK benchmark.

Until then: **keep the IR `Callee::Kernel` abstract** (done), **don't entrench the
flat `KernelFn` enum** as permanent (it's M0-scoped), and **keep `EmittedProject`
able to carry arbitrary files + a computed manifest** (it already carries a files
map; the manifest goes dynamic at FFI time). Those three keep FFI un-blocked.

## One-line summary

Treat an FFI binding as a kernel sourced from crate introspection → unify stdlib +
FFI behind one **kernel registry** (now committed), keep the IR's `Callee::Kernel`
abstract, plan an **isolated, fail-closed `sky_ffi`** subsystem (inspect → `.skyi`
+ table + `catch_unwind` wrapper), make emitted `Cargo.toml` dynamic, and gate the
whole thing on the FFI security model — designed-for from M0, implemented at M4+.

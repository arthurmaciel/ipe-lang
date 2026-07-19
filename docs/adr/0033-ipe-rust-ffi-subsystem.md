Status: Accepted
Date: 2026-07-04

# 0033. Ipê → Rust FFI subsystem — architecture

## Context

Ipê programs need to call arbitrary Rust crates without writing hand-authored shims.
The design target is a fully automatic binding path: a foreign `async fn` binds as
`Task Error a`, common async SDK crates (firestore, firebase, stripe) bind directly
and shim-free, and unused FFI symbols are dead-code-eliminated. The acceptance metric
is the `13-skyshop` example running with zero manual shims across firestore, firebase,
and stripe.

Two fundamental rules drive every type boundary:
1. **Parse, don't validate.** Untrusted rustdoc JSON crosses into Ipê at exactly two
   `TryFrom<wire> → Result<Domain, Diagnostic>` decode points (`PkgInfo`, `Call`).
2. **Make invalid states unrepresentable.** A `Call` that has not passed
   `validate_call` is unconstructible; a binding emitted that `cargo` then rejects
   breaks THE SEAL. Between over-drop (silent omission) and under-bind (bad emission),
   always over-drop.

## Decision

The subsystem has three components, all under `src/compiler/ffi/`:

- **`ipe-ffi-inspector`** (`tools/ipe-ffi-inspector/`): vendored, working. Runs
  post-macro-expansion rustdoc-JSON analysis in an RCE sandbox (`bwrap` primary,
  `unshare`-with-post-spawn-isolation fallback) and produces typed `PkgInfo` JSON.
  Ipê enforces strict isolation: argv quoting is explicit, not shell-quoted; the
  inspector binary runs inside a no-network, read-only-filesystem namespace.
- **`ipe_ffi` generator crate** (`src/compiler/ffi/`): produces
  `.ipei` type-env files and `kernel.json` entries that feed the kernel registry
  (`KernelId::Ffi(FfiKernelId)`). The async→`Task Error a` boundary is explicit;
  `async fn` bindings are not deferred.
- **Consumer pipeline**: `ipe add <crate>` → inspect → generate `.ipei` →
  kernel-registry seeding → dynamic `Cargo.toml` injection → cache at
  `~/.cache/ipe/ffi/rust`. Driver command `ipe add/install/remove`.

The `KernelId` two-tier sum (ADR 0028) already reserves `Ffi(FfiKernelId)` for this:
an FFI call lowers to `Call { callee: Kernel(Ffi(id)), args }` exactly like a stdlib
call. No IR changes needed for FFI.

## Consequences

- The inspector runs in the RCE sandbox on every `ipe add`; a crate whose inspection
  fails produces an `IPE-F4400` diagnostic and is over-dropped, never silently bound.
- FFI kernel entries carry `Origin::Ffi { crate, version }`; a dep upgrade that changes
  a signature is detected by the content-hash drift fence and triggers re-inspection.
- The `num_coerce.rs` port (`src/compiler/ffi/src/num_coerce.rs`) provides the table of
  safe numeric coercions between Ipê `Int`/`Float`/`Decimal` and Rust numeric types;
  unsafe coercions are over-dropped.
- Downstream: `ipe add` is gated on `ipe_ffi` generator shipping; the inspector is
  already usable standalone as a `PkgInfo` JSON producer.

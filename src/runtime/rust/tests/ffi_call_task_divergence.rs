//! Locks the `ffi-call-task-dynamic-dispatch` divergence resolution.
//!
//! `Ffi_callTask` was a runtime *registry* lookup
//! keyed by formatted name over FFI
//! bindings — a string-named, effect-unknown, reflection/`any` dispatch path by
//! construction. The Rust backend refuses that risk surface: the *dynamic* shape
//! of `Ffi.callTask` / `Ffi.callPure` (non-literal kernel name or non-literal
//! args list) has no denotation on `target=rust`.
//!
//! This used to be enforced by two runtime polyfills that returned an
//! unconstrained generic `T` and could signal the refusal only by panicking.
//! Those polyfills were unreachable dead code — no stdlib surface exposes the
//! dynamic shape to Ipê source, and the codegen emits no call to them — so they
//! were REMOVED rather than left asserting behind an `#[allow(clippy::panic)]`.
//! The impossibility is now structural: there is no runtime symbol to reach.
//!
//! Three facts make the removal sound:
//!
//!   1. **No source surface.** No `.ipe` (stdlib or example) names
//!      `Ffi.callPure` / `Ffi.callTask` in the dynamic shape; the accessor
//!      exposed to Ipê is `Kernel.kernel "Name"` direct dispatch.
//!   2. **No codegen path.** The Rust backend emits no reference to a
//!      dynamic-dispatch polyfill; the static `Ffi.callPure "<Kernel>" [lit]`
//!      shape is peephole-resolved to a direct kernel call before emit.
//!   3. **Effectful kernels route via `Kernel.kernel`, never `Ffi.callTask`.**
//!
//! The surviving `Ffi.toAny` identity path stays a runtime no-op — the codegen
//! retains concrete types, so no erasure occurs. This test locks that.

use ipe_runtime_rust::ffi_to_any_polyfill;

/// The static-dispatch side stays total: `Ffi.toAny` performs no type erasure
/// at runtime — concrete types are retained by the codegen, so the polyfill is
/// a pass-through identity.
#[test]
fn to_any_is_runtime_identity_no_erasure() {
    assert_eq!(ffi_to_any_polyfill::<i64>(42), 42);
    assert_eq!(
        ffi_to_any_polyfill::<String>("hi".to_string()),
        "hi".to_string()
    );
}

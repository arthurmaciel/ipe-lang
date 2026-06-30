//! Executable form of the `ffi-call-task-dynamic-dispatch` divergence spec
//! (`runtime-rust/docs/superpowers/specs/2026-06-15-ffi-call-task-dynamic-dispatch-design.md`).
//!
//! WHY THIS IS AN INTENTIONAL DIVERGENCE (DOCUMENT_INTENTIONAL, not future-work):
//!
//! Go's `Ffi_callTask` (`runtime-go/rt/rt.go`) is a runtime *registry* lookup
//! keyed by `fmt.Sprintf("%v", name)` over auto-generated Go-package FFI
//! bindings — a string-named, effect-unknown, reflection/`any` dispatch path by
//! construction. The Rust backend exists precisely to refuse that risk surface:
//! mirroring Go's `%v`-string registry would reintroduce the
//! `Box<dyn Any>`/downcast dynamism this backend is built to design away
//! (runtime-rust/CLAUDE.md, "NO RUNTIME ERRORS — existential"). So the
//! *dynamic* shape of `Ffi.callTask` / `Ffi.callPure` (non-literal kernel name
//! or non-literal args list) is deliberately guarded, NOT served — it is
//! at-parity-by-design, not a deferral.
//!
//! Three facts make this safe to leave as a guard rather than a feature gap:
//!
//!   1. **No-reflection guard.** The faithful Rust analogue of Go's dynamic
//!      dispatch is reflection; that is the one thing this backend must not do.
//!   2. **Zero `.sky` source emits `Ffi.callTask`.** Across the whole stdlib +
//!      every example, `grep -rn 'Ffi\.callTask' --include='*.sky'` returns 0
//!      hits outside generated `sky-out/`. The polyfill is statically dead for
//!      every well-typed Sky program shipped today.
//!   3. **Effectful kernels route via `Ffi.kernel`, never `Ffi.callTask`.** The
//!      stdlib's effectful kernels (`Http.*`, `Task.*`, `Db.*`, `Time.now`, …)
//!      are `Ffi.kernel "Name"` aliases resolved by the Stage-4 call-site
//!      rewrite / direct-dispatch paths. `Ffi.callTask`'s real consumer is
//!      auto-generated Go-package bindings, which require a Go runtime and are
//!      out of scope on `target=rust`. The "sub-project D / Task-emitting
//!      kernels" framing in the old polyfill comment is a misattribution and is
//!      retired by the spec.
//!
//! The *static* shape (`Ffi.callPure "<Kernel>" [lit]`) is handled entirely at
//! compile time by the Rust codegen peephole — it never reaches a polyfill —
//! so the runtime guard only ever fires on the dynamic shape this backend
//! refuses to serve. The principled eventual fix is a compile-time rejection of
//! the dynamic shape (a total alternative to the runtime panic, spec Q5), but
//! it is gated on a reachable trigger; until then the panic is the documented
//! temporary bridge with an actionable refactor message.
//!
//! These tests LOCK that canonical behaviour:
//!   - the dynamic-shape `callTask` polyfill panics with its actionable message;
//!   - the dynamic-shape `callPure` polyfill panics symmetrically (one boundary);
//!   - the static-dispatch identity path (`Ffi.toAny`) stays a no-op pass-through.
//!
//! NOTE ON THE EXPECTED SUBSTRING: the panic *rationale* wording is owned by the
//! polyfill/README editor and is being re-dispositioned (the stale
//! "sub-project D" phrasing is retired). The message-independent, stable anchor
//! that survives every spec-described rewrite is the `"Ffi.callTask"` /
//! `"Ffi.callPure"` call-site prefix — so the assertions match on that, not on
//! the volatile rationale text.

use sky_runtime_rust::{ffi_call_pure_polyfill, ffi_call_task_polyfill, ffi_to_any_polyfill};

/// Dynamic-shape `Ffi.callTask` (non-literal name/args) is refused by the
/// no-reflection guard with an actionable, kernel-named panic. The substring
/// is the stable call-site prefix, not the (editor-owned) rationale wording.
#[test]
#[should_panic(expected = "Ffi.callTask")]
fn dynamic_call_task_panics_with_actionable_message() {
    // A `Ffi.callTask computedName args` shape IS type-checkable (the name is
    // `String`-typed), so this is the dynamic surface the backend deliberately
    // does not serve — never a faithful registry path.
    let _: i64 = ffi_call_task_polyfill::<i64, i64>("Http_get".to_string(), vec![1]);
}

/// `callPure` and `callTask` are ONE boundary: the dynamic shape of each is
/// refused identically. Locking callPure here keeps the symmetry the spec
/// canonicalises (the callPure dynamic-vs-static story applies verbatim to
/// callTask).
#[test]
#[should_panic(expected = "Ffi.callPure")]
fn dynamic_call_pure_panics_symmetrically() {
    let _: i64 = ffi_call_pure_polyfill::<i64, i64>("Decimal_fromInt".to_string(), vec![1]);
}

/// The static-dispatch side stays total: `Ffi.toAny` performs no type erasure
/// at runtime — concrete types are retained by the codegen, so the polyfill is
/// a pass-through identity. This documents that the guard fires ONLY on the
/// dynamic shape, never on the resolved static path.
#[test]
fn to_any_is_runtime_identity_no_erasure() {
    assert_eq!(ffi_to_any_polyfill::<i64>(42), 42);
    assert_eq!(
        ffi_to_any_polyfill::<String>("hi".to_string()),
        "hi".to_string()
    );
}

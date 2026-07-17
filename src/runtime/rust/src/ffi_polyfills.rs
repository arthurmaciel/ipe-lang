// Ffi.* polyfill stubs.
//
// The Rust codegen's peephole rewriter (Sky.Generate.Rust.Builder.exprToRustInner)
// handles the static-dispatch shape of `Ffi.callPure "<Kernel>" [args]` —
// kernel name + args list both literal — by emitting a direct kernel call.
// These polyfills only get linked when a non-static-dispatch shape appears in
// user code, and fail loud with an actionable message so the user can refactor
// to the static shape (or use `Ffi.kernel` for value-level kernel selection).

/// Identity wrapper. Matches `Ffi.toAny`'s static signature `a -> any` but
/// performs no type erasure at runtime — the codegen retains concrete types.
/// Only reached when `Ffi.toAny` appears outside a peephole-matched
/// `Ffi.callPure` argument list and outside the standalone-toAny peephole.
pub fn ffi_to_any_polyfill<T>(x: T) -> T {
    x
}

/// Reached only when `Ffi.callPure` is invoked with a non-literal kernel
/// name or non-literal args list (i.e. dynamic dispatch). Sky's static
/// dispatch path is the peephole — refactor the call site to use a string
/// literal + list literal, or use `Ffi.kernel "<Name>"` for value-level
/// kernel selection.
// IRREDUCIBLE: returns an unconstrained generic `T`, so no total value can be
// synthesised. Statically dead for valid Sky (the peephole resolves the
// static-dispatch shape); this is the dynamic-dispatch-unsupported fallback.
// SKY-RUST-AUDIT:ACCEPTED (Arthur Maciel, 2026-06-13) — statically dead for valid Sky (peephole resolves it); unconstrained generic T return has no total value [ledger #3]
#[allow(clippy::panic)]
pub fn ffi_call_pure_polyfill<T, A>(name: String, _args: Vec<A>) -> T {
    panic!(
        "Ffi.callPure {:?}: dynamic dispatch is not supported on target=rust. \
         Use a string-literal kernel name + list-literal args (peephole-resolved \
         at compile time), or `Ffi.kernel \"{}\"` for value-level kernel selection.",
        name, name
    );
}

/// Same dynamic-dispatch no-reflection guard as `ffi_call_pure_polyfill`, for
/// the Task-returning variant. Reached only when `Ffi.callTask` is invoked with
/// a non-literal kernel name or non-literal args list (i.e. dynamic dispatch).
/// Serving it faithfully would require Go's `%v`-string registry — the
/// reflection/`any` surface this backend exists to refuse. The static-dispatch
/// shape is the peephole; no well-typed Sky program reaches this fallback
/// (effectful kernels reach Rust via `Ffi.kernel` direct dispatch, never via
/// `Ffi.callTask`).
// IRREDUCIBLE: returns an unconstrained generic `T`, so no total value can be
// synthesised. Statically dead for valid Sky (the peephole resolves the
// static-dispatch shape); this is the dynamic-dispatch-unsupported fallback —
// the same boundary as ffi_call_pure_polyfill.
// SKY-RUST-AUDIT:ACCEPTED (Arthur Maciel, 2026-06-13) — statically dead for valid Sky (peephole resolves it); unconstrained generic T return has no total value [ledger #3]
#[allow(clippy::panic)]
pub fn ffi_call_task_polyfill<T, A>(name: String, _args: Vec<A>) -> T {
    panic!(
        "Ffi.callTask {:?}: dynamic dispatch is not supported on target=rust \
         BY DESIGN. A string -> SkyTask<E, a> registry would need a runtime \
         reflection / `Box<dyn Any>` surface (Go's `%v`-string registry) to \
         return an unconstrained generic — exactly the dynamism this backend \
         exists to refuse, and it cannot be made total. Use a string-literal \
         kernel name + list-literal args (peephole-resolved to a direct call \
         on the Task-returning kernel at compile time), or `Ffi.kernel \"{}\"` \
         for value-level kernel selection.",
        name, name
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_any_is_identity_i64() {
        assert_eq!(ffi_to_any_polyfill::<i64>(42), 42);
    }

    #[test]
    fn to_any_is_identity_string() {
        assert_eq!(
            ffi_to_any_polyfill::<String>("hi".to_string()),
            "hi".to_string()
        );
    }

    #[test]
    #[should_panic(expected = "Ffi.callPure")]
    fn call_pure_panics_with_kernel_name() {
        let _: i64 = ffi_call_pure_polyfill::<i64, i64>("Decimal_fromInt".to_string(), vec![1]);
    }

    #[test]
    #[should_panic(expected = "dynamic dispatch is not supported on target=rust")]
    fn call_task_panics_with_by_design_hint() {
        let _: i64 = ffi_call_task_polyfill::<i64, i64>("Http_get".to_string(), vec![1]);
    }
}

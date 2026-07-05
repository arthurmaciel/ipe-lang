//! Sky.Core.Basics kernels: modBy + errorToString.
//!
//! Mirrors Go's runtime-go/rt/rt.go (Basics_modByT, etc.).

/// Sky `modBy : Int -> Int -> Int`. Divisor-first convention (Elm/pipeline order).
/// Mirrors Go's `Basics_modByT` exactly:
///   - divisor == 0  → 0
///   - r = n % divisor; if r < 0 { r += divisor }
///
/// Adjust fires ONLY when r < 0 (irrespective of divisor sign) — Go parity.
///
/// Overflow guard: `i64::MIN % -1` is undefined behaviour in Rust debug/release
/// (the mathematical result is 0).  `checked_rem` returns `None` for that case;
/// we map it to r = 0, which is the correct mathematical remainder and leaves
/// the adjust condition (`0 < 0`) false, so the final result is 0.
pub fn basics_mod_by(divisor: i64, n: i64) -> i64 {
    if divisor == 0 {
        return 0;
    }
    // checked_rem returns None only for i64::MIN % -1 (overflow); treat as 0.
    let r = n.checked_rem(divisor).unwrap_or(0);
    if r < 0 { r.wrapping_add(divisor) } else { r }
}

/// The result of Sky's `Basics.compare` — a typed three-way comparison.
///
/// Sanctioned divergence from the Sky/Go backend: Go's `Basics_compareT`
/// returns `-1 / 0 / 1` as a plain `int`.  The Rust backend returns a typed
/// enum so pattern-match on `LT / EQ / GT` is sound and exhaustive without
/// an extra range-check.  See `docs/divergences-from-sky.md §B-compare`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum SkyOrder {
    LT = 0,
    EQ = 1,
    GT = 2,
}

/// Sky `compare : comparable -> comparable -> Order`.
///
/// Mirrors Go's `Basics_compareT` semantics: `LT` when `a < b`, `GT` when
/// `a > b`, `EQ` otherwise.  The `PartialOrd` bound is correct here: Sky's
/// `comparable` covers `Int`, `Float`, `String`, `Char`, `Bool` — all of
/// which implement `PartialOrd` in Rust.  NaN-producing operations (`Float`)
/// follow Rust's `PartialOrd` convention (NaN is unordered); Sky does not
/// expose a `Float` NaN literal so this is sound in practice.
pub fn basics_compare<T: PartialOrd>(a: T, b: T) -> SkyOrder {
    if a < b {
        SkyOrder::LT
    } else if a > b {
        SkyOrder::GT
    } else {
        SkyOrder::EQ
    }
}

/// Sky `fst : (a, b) -> a` / `snd : (a, b) -> b`. Pure in stdlib, but the
/// Prelude re-export lowers as a `VarKernel "Basics" "fst"`, so the Rust
/// backend routes it to a runtime kernel. Tuples lower to Rust tuples.
pub fn basics_fst<A, B>(t: (A, B)) -> A {
    t.0
}
pub fn basics_snd<A, B>(t: (A, B)) -> B {
    t.1
}

/// Sky `identity : a -> a` and `always : a -> b -> a`. Pure in the stdlib
/// (`identity x = x`, `always x _ = x`) but the Prelude re-export lowers each as
/// a `VarKernel "Basics" …`, so the Rust backend routes them to runtime kernels
/// (same convention as `fst`/`snd`). `always` is the tupled 2-arg form the
/// codegen emits; partial application (`always 0`) is wrapped into a closure by
/// the codegen, so the plain `(A, B) -> A` shape here is correct.
pub fn basics_identity<A>(x: A) -> A {
    x
}
pub fn basics_always<A, B>(x: A, _y: B) -> A {
    x
}

/// Sky `not : Bool -> Bool` — boolean negation.
pub fn basics_not(b: bool) -> bool {
    !b
}

/// Sky `clamp : comparable -> comparable -> comparable -> comparable`
/// (`clamp lo hi x`). Polymorphic over any `PartialOrd` type — mirrors
/// `math_min` / `math_max`, so the type-checker's `Comparable a` obligation
/// (an `Ord` super-var, see `constrain_var_kernel`) rejects a function / record
/// argument before this monomorphises. Total: returns `lo` below the range,
/// `hi` above it, `x` within — no panic path. When `lo > hi` the lower bound
/// wins (matches Elm's `if x < lo then lo else if x > hi then hi else x`).
pub fn basics_clamp<T: PartialOrd>(lo: T, hi: T, x: T) -> T {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

// ── Basics numerics (#115) ──────────────────────────────────────────────────

/// Sky `negate : number -> number` — unary negation on Int or Float.
///
/// This is also the runtime target for the `-x` desugar in the parser
/// (`negate x`). Both `i64` and `f64` implement `Neg<Output = Self>`, so
/// the same generic function covers both Sky numeric primitives with no
/// runtime type dispatch — matching Go's natural `-x` operator.
pub fn basics_negate<T: ::core::ops::Neg<Output = T>>(x: T) -> T {
    -x
}

/// Sky `abs : number -> number` — absolute value on Int or Float.
///
/// Uses `T::default()` as the zero sentinel (`0_i64` / `0.0_f64`), both of
/// which satisfy `Default`. The `Copy` bound allows reusing `x` after the
/// comparison without a clone. Matches Go's `Basics_abs` semantics exactly:
/// negative values are negated, non-negatives pass through unchanged.
pub fn basics_abs<T: PartialOrd + ::core::ops::Neg<Output = T> + Copy + Default>(x: T) -> T {
    let zero = T::default();
    if x < zero { -x } else { x }
}

// ── end Basics numerics (#115) ──────────────────────────────────────────────

/// Sky `errorToString : a -> String` — universal Sky stringifier.
/// Used by Sky.Test.debugShow and friends to render any Sky value into
/// a diagnostic string. Backed by the total `SkyStringify` trait, which
/// mirrors Go's `Basics_errorToString` EXACTLY: a `String` renders UNQUOTED
/// (`hi`, not `"hi"`), scalars render like `%v`, and slices/tuples/maps follow
/// Go's space-separated layout. Every codegen-emitted record/ADT gets a
/// `SkyStringify` impl (Emitter.hs), so the bound is always satisfiable —
/// the generic `debugShow : a -> String` body type-checks and is total.
pub fn basics_error_to_string<T: crate::sky_runtime::stringify::SkyStringify>(v: T) -> String {
    v.sky_show()
}

// ── Sky.Core.Error kernels (minimal `Error = String` slice, #86) ─────────────
//
// The Sky error channel `Error` is backed by `String` (`SkyError = String` in
// the standalone crate; generated projects thread the error type as a plain
// `String`). These kernels are therefore typed on `String` directly — NOT on the
// `config::SkyError` alias, which does NOT exist in compiler-generated projects
// (their `config.rs` is regenerated without it). An error value IS its message.
// `Error.toString` reuses `basics_error_to_string` (above). The rich
// `ErrorKind`/`ErrorDetails` ADT — and the `<Kind>: <message>` rendering it
// enables — is deferred to #85; in this slice `Error.toString` echoes the
// message verbatim.

/// Every message-carrying `Error` constructor (`Error.unexpected` / `io` /
/// `network` / `ffi` / `decode` / `invalidInput` / `conflict` / `unavailable`)
/// shares this one runtime symbol: with the error channel backed by `String`, a
/// `String -> Error` constructor is the identity — the message IS the error. The
/// distinct Sky names are preserved as separate kernels for the future rich-ADT
/// upgrade (#85).
#[must_use]
pub fn sky_error_from_message(msg: String) -> String {
    msg
}

/// `Error.timeout : Error` — canonical timeout error message.
#[must_use]
pub fn sky_error_timeout() -> String {
    "timeout".to_owned()
}

/// `Error.notFound : Error` — canonical not-found error message.
#[must_use]
pub fn sky_error_not_found() -> String {
    "not found".to_owned()
}

/// `Error.permissionDenied : Error` — canonical permission-denied error message.
#[must_use]
pub fn sky_error_permission_denied() -> String {
    "permission denied".to_owned()
}

/// `Error.withMessage : String -> Error -> Error` — replace an error's message.
/// With the error channel backed by `String` the old error carries only its
/// message, so this returns the new message and discards the old value (matching
/// the upstream `Sky.Core.Error.withMessage` on a `details = Nothing` error).
#[must_use]
pub fn sky_error_with_message(msg: String, _old: String) -> String {
    msg
}

/// Sky `Debug.toString` — the `{{expr}}` string-interpolation stringifier.
/// Display-based, NOT Debug: a `String` interpolates as itself (no surrounding
/// quotes) and scalars format like Go's `%v`. Mirrors Go's `Debug_toString`
/// (`String → s`, else `Sprintf("%v", …)`).
pub fn debug_to_string<T: std::fmt::Display>(v: T) -> String {
    format!("{}", v)
}

/// Sky `Basics.toString : a -> String` — Go's `fmt.Sprintf("%v", …)`. Display-
/// based (NOT Debug, so a `String` renders unquoted and scalars format like Go's
/// `%v`); same semantics as `Debug.toString`. The `Display` bound is deliberate:
/// `toString` on a scalar (Int/Float/Bool/String) is the overwhelmingly common
/// case and matches Go exactly, while `toString` on a composite (record/ADT)
/// — which has no `Display` impl — fails at COMPILE time (E0277), never at
/// runtime. That honours the "no runtime errors" rule (Go would reflect at
/// runtime; Rust catches it before a binary exists). A future type-directed
/// lowering could route composites to a derived renderer if that case arises.
pub fn basics_to_string<T: std::fmt::Display>(v: T) -> String {
    format!("{}", v)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Sky.Core.Error kernels (#86) ─────────────────────────────────────────
    #[test]
    fn error_from_message_is_identity() {
        // Every message constructor (unexpected/io/network/…) shares this symbol;
        // with the String-backed error channel the message IS the error value.
        assert_eq!(sky_error_from_message("boom".to_owned()), "boom");
        assert_eq!(sky_error_from_message(String::new()), "");
    }
    #[test]
    fn error_nullary_constructors_are_canonical_messages() {
        assert_eq!(sky_error_timeout(), "timeout");
        assert_eq!(sky_error_not_found(), "not found");
        assert_eq!(sky_error_permission_denied(), "permission denied");
    }
    #[test]
    fn error_with_message_replaces_and_discards_old() {
        assert_eq!(
            sky_error_with_message("new".to_owned(), "old".to_owned()),
            "new"
        );
    }
    #[test]
    fn error_to_string_echoes_message_verbatim() {
        // `Error.toString` reuses `basics_error_to_string`; on a String-backed
        // error it renders the message UNQUOTED (round-trips the constructor arg).
        assert_eq!(
            basics_error_to_string(sky_error_from_message("boom".to_owned())),
            "boom"
        );
    }

    #[test]
    fn test_mod_by_positive_divisor() {
        assert_eq!(basics_mod_by(3, 10), 1);
    }
    #[test]
    fn test_mod_by_zero_divisor() {
        assert_eq!(basics_mod_by(0, 5), 0);
    }
    #[test]
    fn test_mod_by_negative_dividend_positive_divisor() {
        // -1 % 3 = -1 in Rust; Sky/Elm wants 2 (same sign as divisor)
        assert_eq!(basics_mod_by(3, -1), 2);
        assert_eq!(basics_mod_by(3, -4), 2);
    }
    #[test]
    fn test_mod_by_exact() {
        assert_eq!(basics_mod_by(5, 10), 0);
    }

    // Go parity: adjust fires only when r < 0.
    // positive divisor, positive dividend — no adjust needed.
    #[test]
    fn test_mod_by_pos_div_pos_n() {
        assert_eq!(basics_mod_by(3, 7), 1);
    }
    // negative divisor, positive dividend — r > 0, no adjust (was wrong pre-fix).
    // Go: 7 % -3 = 1; 1 >= 0 → no adjust → 1.
    #[test]
    fn test_mod_by_neg_divisor_pos_n() {
        assert_eq!(basics_mod_by(-3, 7), 1);
    }
    // negative divisor, negative dividend — r < 0 → adjust.
    // Go: -7 % -3 = -1; -1 < 0 → -1 + (-3) = -4.  Wait — divisor=-3 so r+divisor=-4.
    // Verify: Go does r += divisor → -1 + (-3) = -4.
    #[test]
    fn test_mod_by_neg_divisor_neg_n() {
        assert_eq!(basics_mod_by(-3, -7), -4);
    }
    // Overflow guard: i64::MIN % -1 must not panic, result = 0.
    #[test]
    fn test_mod_by_min_i64_neg1() {
        assert_eq!(basics_mod_by(-1, i64::MIN), 0);
    }

    #[test]
    fn test_error_to_string_i64() {
        assert_eq!(basics_error_to_string(42i64), "42");
    }
    // String renders UNQUOTED now (Go parity) — the primary fix.
    #[test]
    fn test_error_to_string_string() {
        assert_eq!(basics_error_to_string("hi".to_string()), "hi");
    }
    // Vec renders space-separated (Go's `%v`: `[1 2 3]`, NOT `[1, 2, 3]`).
    #[test]
    fn test_error_to_string_vec() {
        assert_eq!(basics_error_to_string(vec![1i64, 2, 3]), "[1 2 3]");
    }

    // Regression: identity/always were missing from the runtime (emitted as
    // `basics_identity`/`basics_always` calls but undefined → E0425).
    #[test]
    fn test_identity() {
        assert_eq!(basics_identity(7i64), 7);
    }
    #[test]
    fn test_always_returns_first() {
        assert_eq!(basics_always(7i64, "discarded"), 7);
    }

    // Basics.toString = Go's %v: Display-based, unquoted strings, clean scalars.
    #[test]
    fn test_to_string_int() {
        assert_eq!(basics_to_string(42i64), "42");
    }
    #[test]
    fn test_to_string_bool() {
        assert_eq!(basics_to_string(true), "true");
    }
    #[test]
    fn test_to_string_string_unquoted() {
        assert_eq!(basics_to_string("hi".to_string()), "hi");
    }
    #[test]
    fn test_to_string_float() {
        assert_eq!(basics_to_string(42.5f64), "42.5");
    }
}

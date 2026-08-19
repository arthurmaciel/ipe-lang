//! Ipe.Basics kernels: modBy + errorToString.
//!
//! Mirrors Go's runtime-go/rt/rt.go (`Basics_modByT`, etc.).

/// Ipê `modBy : Int -> Int -> Int`. Divisor-first convention (Elm/pipeline order).
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
#[must_use]
pub fn basics_mod_by(divisor: i64, n: i64) -> i64 {
    if divisor == 0 {
        return 0;
    }
    // checked_rem returns None only for i64::MIN % -1 (overflow); treat as 0.
    let r = n.checked_rem(divisor).unwrap_or(0);
    if r < 0 { r.wrapping_add(divisor) } else { r }
}

/// The result of Ipê's `Basics.compare` — a typed three-way comparison.
///
/// Sanctioned divergence from the Ipe/Go backend: Go's `Basics_compareT`
/// returns `-1 / 0 / 1` as a plain `int`.  The Rust backend returns a typed
/// enum so pattern-match on `LT / EQ / GT` is sound and exhaustive without
/// an extra range-check.  See `docs/divergences-from-sky.md §B-compare`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum IpeOrder {
    LT = 0,
    EQ = 1,
    GT = 2,
}

/// Ipê `compare : comparable -> comparable -> Order`.
///
/// Mirrors Go's `Basics_compareT` semantics: `LT` when `a < b`, `GT` when
/// `a > b`, `EQ` otherwise.  The `PartialOrd` bound is correct here: Ipê's
/// `comparable` covers `Int`, `Float`, `String`, `Char`, `Bool` — all of
/// which implement `PartialOrd` in Rust.  NaN-producing operations (`Float`)
/// follow Rust's `PartialOrd` convention (NaN is unordered); Ipê does not
/// expose a `Float` NaN literal so this is sound in practice.
pub fn basics_compare<T: PartialOrd>(a: T, b: T) -> IpeOrder {
    if a < b {
        IpeOrder::LT
    } else if a > b {
        IpeOrder::GT
    } else {
        IpeOrder::EQ
    }
}

/// Ipê `fst : (a, b) -> a` / `snd : (a, b) -> b`. Pure in stdlib, but the
/// Prelude re-export lowers as a `VarKernel "Basics" "fst"`, so the Rust
/// backend routes it to a runtime kernel. Tuples lower to Rust tuples.
pub fn basics_fst<A, B>(t: (A, B)) -> A {
    t.0
}
pub fn basics_snd<A, B>(t: (A, B)) -> B {
    t.1
}

/// Ipê `identity : a -> a` and `always : a -> b -> a`. Pure in the stdlib
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

/// Ipê `not : Bool -> Bool` — boolean negation.
#[must_use]
pub fn basics_not(b: bool) -> bool {
    !b
}

/// Ipê `clamp : comparable -> comparable -> comparable -> comparable`
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

// ── Basics numerics ──────────────────────────────────────────────────

/// Ipê `negate : number -> number` — wrapping unary negation on Int or Float.
///
/// This is also the runtime target for the `-x` desugar in the parser
/// (`negate x`). Uses `IpeWrappingNeg` so `negate(i64::MIN) == i64::MIN`
/// (the unique two's-complement fixed point) rather than panicking under
/// `overflow-checks=on`. `f64` negation is total and byte-identical to
/// the plain `-x` form.
pub fn basics_negate<T: IpeWrappingNeg>(x: T) -> T {
    x.ipe_wrapping_neg()
}

/// Wrapping negation for Ipê numeric types.
///
/// `i64` uses `wrapping_neg` so `i64::MIN.ipe_wrapping_neg() == i64::MIN`
/// (two's-complement identity — the unique fixed point of wrapping negate).
/// `f64` uses plain negation, which is total (never overflows in IEEE 754).
/// Do NOT reuse `SaturatingNeg` here: `negate` wraps, `abs` saturates —
/// those are different contracts.
pub trait IpeWrappingNeg: Sized {
    #[must_use]
    fn ipe_wrapping_neg(self) -> Self;
}
impl IpeWrappingNeg for i64 {
    fn ipe_wrapping_neg(self) -> Self {
        self.wrapping_neg()
    }
}
impl IpeWrappingNeg for f64 {
    fn ipe_wrapping_neg(self) -> Self {
        -self
    }
}

/// Wrapping addition for polymorphic `Number a` functions.
///
/// `i64` wraps on overflow (two's-complement); `f64` uses plain IEEE 754
/// addition (total — overflow yields ±∞, never panics). Generic `Number a`
/// functions that emit `BinOp::Add` use this trait so the generic body is
/// safe under `overflow-checks=on` regardless of how the call site
/// monomorphises.
pub trait IpeWrappingAdd<Rhs = Self>: Sized {
    type Output;
    #[must_use]
    fn ipe_wrapping_add(self, rhs: Rhs) -> Self::Output;
}
impl IpeWrappingAdd for i64 {
    type Output = i64;
    fn ipe_wrapping_add(self, rhs: i64) -> i64 {
        self.wrapping_add(rhs)
    }
}
impl IpeWrappingAdd for f64 {
    type Output = f64;
    fn ipe_wrapping_add(self, rhs: f64) -> f64 {
        self + rhs
    }
}

/// Wrapping subtraction for polymorphic `Number a` functions.
///
/// `i64` wraps on underflow; `f64` is total.
pub trait IpeWrappingSub<Rhs = Self>: Sized {
    type Output;
    #[must_use]
    fn ipe_wrapping_sub(self, rhs: Rhs) -> Self::Output;
}
impl IpeWrappingSub for i64 {
    type Output = i64;
    fn ipe_wrapping_sub(self, rhs: i64) -> i64 {
        self.wrapping_sub(rhs)
    }
}
impl IpeWrappingSub for f64 {
    type Output = f64;
    fn ipe_wrapping_sub(self, rhs: f64) -> f64 {
        self - rhs
    }
}

/// Wrapping multiplication for polymorphic `Number a` functions.
///
/// `i64` wraps on overflow; `f64` is total.
pub trait IpeWrappingMul<Rhs = Self>: Sized {
    type Output;
    #[must_use]
    fn ipe_wrapping_mul(self, rhs: Rhs) -> Self::Output;
}
impl IpeWrappingMul for i64 {
    type Output = i64;
    fn ipe_wrapping_mul(self, rhs: i64) -> i64 {
        self.wrapping_mul(rhs)
    }
}
impl IpeWrappingMul for f64 {
    type Output = f64;
    fn ipe_wrapping_mul(self, rhs: f64) -> f64 {
        self * rhs
    }
}

/// Saturating negation — used only by `basics_abs`, NOT by `basics_negate`.
///
/// `abs` saturates at `i64::MAX` for `i64::MIN` (deliberate divergence from
/// Go; see `docs/divergences-from-sky.md`). `negate` uses `IpeWrappingNeg`
/// instead (wrapping contract, not saturating).
pub trait SaturatingNeg: Sized {
    #[must_use]
    fn saturating_neg(self) -> Self;
}
impl SaturatingNeg for i64 {
    fn saturating_neg(self) -> Self {
        self.checked_neg().unwrap_or(i64::MAX)
    }
}
impl SaturatingNeg for f64 {
    fn saturating_neg(self) -> Self {
        -self
    }
}

/// Ipê `abs : number -> number` — absolute value on Int or Float.
///
/// Uses `T::default()` as the zero sentinel (`0_i64` / `0.0_f64`), both of
/// which satisfy `Default`. The `Copy` bound allows reusing `x` after the
/// comparison without a clone. Matches Go's `Basics_abs` semantics, with the
/// no-panic rule taking precedence at `i64::MIN` (Go's `int64` overflow wraps
/// silently to `i64::MIN` itself; Rust saturates to `i64::MAX` instead of
/// wrapping to a NEGATIVE "absolute value" — see `docs/divergences-from-sky.md`).
pub fn basics_abs<T: PartialOrd + SaturatingNeg + Copy + Default>(x: T) -> T {
    let zero = T::default();
    if x < zero { x.saturating_neg() } else { x }
}

// ── end Basics numerics ──────────────────────────────────────────────

/// Ipê `errorToString : a -> String` — universal Ipê stringifier.
/// Used by Ipe.Test.debugShow and friends to render any Ipê value into
/// a diagnostic string. Backed by the total `IpeStringify` trait, which
/// mirrors Go's `Basics_errorToString` EXACTLY: a `String` renders UNQUOTED
/// (`hi`, not `"hi"`), scalars render like `%v`, and slices/tuples/maps follow
/// Go's space-separated layout. Every codegen-emitted record/ADT gets a
/// `IpeStringify` impl (Emitter.hs), so the bound is always satisfiable —
/// the generic `debugShow : a -> String` body type-checks and is total.
pub fn basics_error_to_string<T: crate::stringify::IpeStringify>(v: T) -> String {
    v.ipe_show()
}

// Ipe.Error's runtime kernels live in `error.rs` — `Error` is the real
// `ipe_runtime::error::IpeError` ADT, not a bare `String`. `Error.toString`
// reuses `basics_error_to_string` above.

/// Ipê `Debug.toString` — the `{{expr}}` string-interpolation stringifier.
/// Backed by the total `IpeStringify` trait: a `String` interpolates as itself
/// (no surrounding quotes) and every value renders like Go's `%v`. Mirrors Go's
/// `Debug_toString` (`String → s`, else `Sprintf("%v", …)`). Identical to
/// [`basics_to_string`] — the interpolation canonicaliser lowers `{{expr}}` to
/// the same `Basics.toString` kernel.
pub fn debug_to_string<T: crate::stringify::IpeStringify>(v: T) -> String {
    v.ipe_show()
}

/// Ipê `Basics.toString : a -> String` — Go's `fmt.Sprintf("%v", …)`. Backed by
/// the total `IpeStringify` trait (the same path as [`basics_error_to_string`]),
/// which mirrors Go's `%v` EXACTLY and TOTALLY: a `String` renders unquoted, a
/// scalar renders like `%v`, and records / ADTs / lists / maps follow Go's
/// space-separated struct/slice/map layout. Every scalar and every
/// codegen-emitted record/ADT implements `IpeStringify` (Emitter.hs), so the
/// bound is satisfiable for scalar, record, ADT, and generic call sites alike —
/// there is no exit-0-then-cargo-fail composite hole (which a `Display` bound
/// left open, since a composite has no `Display` impl).
pub fn basics_to_string<T: crate::stringify::IpeStringify>(v: T) -> String {
    v.ipe_show()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_to_string_renders_kind_and_message() {
        // `Error.toString` reuses `basics_error_to_string`, dispatching through
        // `IpeStringify` to `IpeError::to_ipe_string`'s `"<Kind>: <message>"`.
        assert_eq!(
            basics_error_to_string(crate::error::IpeError::io("boom".to_owned())),
            "Io: boom"
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
        // -1 % 3 = -1 in Rust; Ipê/Elm wants 2 (same sign as divisor)
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

    // ── wrapping negate (IpeWrappingNeg) ───────────────────────────────
    // negate(i64::MIN) must not panic; the two's-complement fixed point is
    // i64::MIN itself (wrapping_neg(i64::MIN) == i64::MIN).
    #[test]
    fn negate_wraps_at_min_i64() {
        assert_eq!(basics_negate(i64::MIN), i64::MIN);
    }
    #[test]
    fn negate_negates_positive_i64() {
        assert_eq!(basics_negate(42i64), -42i64);
    }
    #[test]
    fn negate_negates_negative_i64() {
        assert_eq!(basics_negate(-1i64), 1i64);
    }
    #[test]
    #[allow(clippy::float_cmp)]
    fn negate_f64_is_plain_neg() {
        assert_eq!(basics_negate(1.5f64), -1.5f64);
        assert_eq!(basics_negate(-0.0f64), 0.0f64);
    }

    // ── IpeWrappingAdd ─────────────────────────────────────────────────
    #[test]
    fn ipe_wrapping_add_i64_max_plus_one() {
        assert_eq!(i64::MAX.ipe_wrapping_add(1), i64::MIN);
    }
    #[test]
    fn ipe_wrapping_add_i64_min_plus_neg_one() {
        assert_eq!(i64::MIN.ipe_wrapping_add(-1), i64::MAX);
    }
    #[test]
    #[allow(clippy::float_cmp)]
    fn ipe_wrapping_add_f64_matches_plain() {
        assert_eq!(1.0f64.ipe_wrapping_add(2.0), 1.0 + 2.0);
    }

    // ── IpeWrappingSub ─────────────────────────────────────────────────
    #[test]
    fn ipe_wrapping_sub_i64_min_minus_one() {
        assert_eq!(i64::MIN.ipe_wrapping_sub(1), i64::MAX);
    }
    #[test]
    fn ipe_wrapping_sub_i64_max_minus_neg_one() {
        assert_eq!(i64::MAX.ipe_wrapping_sub(-1), i64::MIN);
    }
    #[test]
    #[allow(clippy::float_cmp)]
    fn ipe_wrapping_sub_f64_matches_plain() {
        assert_eq!(3.0f64.ipe_wrapping_sub(1.5), 1.5);
    }

    // ── IpeWrappingMul ─────────────────────────────────────────────────
    #[test]
    fn ipe_wrapping_mul_i64_max_times_two() {
        assert_eq!(i64::MAX.ipe_wrapping_mul(2), -2);
    }
    #[test]
    fn ipe_wrapping_mul_i64_min_times_neg_one() {
        assert_eq!(i64::MIN.ipe_wrapping_mul(-1), i64::MIN);
    }
    #[test]
    #[allow(clippy::float_cmp)]
    fn ipe_wrapping_mul_f64_matches_plain() {
        assert_eq!(2.0f64.ipe_wrapping_mul(3.0), 6.0);
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

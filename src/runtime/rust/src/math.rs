//! Ipe.Math kernels.
//!
//! Signatures from src/Ipê/Type/Constrain/Expression.hs:
//!   abs    : Int -> Int                  (integer abs)
//!   min    : a -> a -> a                 (polymorphic; monomorphised at call site)
//!   max    : a -> a -> a
//!   sqrt   : Float -> Float
//!   pow    : Float -> Float -> Float
//!   floor  : Float -> Int                (note: returns Int, truncates)
//!   ceil   : Float -> Int
//!   round  : Float -> Int

#[must_use]
pub fn math_pi() -> f64 {
    std::f64::consts::PI
}
#[must_use]
pub fn math_e() -> f64 {
    std::f64::consts::E
}
/// Golden ratio φ = (1 + √5) / 2 ≈ 1.6180339887…
/// (`std::f64::consts::PHI` is nightly-only; use the literal for stable Rust.)
#[must_use]
pub fn math_phi() -> f64 {
    1.618_033_988_749_895_f64
}
/// √2 ≈ 1.4142135623…
#[must_use]
pub fn math_sqrt2() -> f64 {
    std::f64::consts::SQRT_2
}
/// Positive infinity (IEEE 754).
#[must_use]
pub fn math_inf() -> f64 {
    f64::INFINITY
}
/// Not-a-number (IEEE 754). Note: NaN ≠ NaN by IEEE 754.
#[must_use]
pub fn math_nan() -> f64 {
    f64::NAN
}

/// Returns `true` if `x` is NaN (IEEE 754).  Ipê `Math.isNaN : Float -> Bool`.
#[must_use]
pub fn math_is_nan(x: f64) -> bool {
    x.is_nan()
}

/// Saturates `i64::MIN` to `i64::MAX` (no-panic rule: `-i64::MIN` is not representable).
#[must_use]
pub fn math_abs(x: i64) -> i64 {
    x.checked_abs().unwrap_or(i64::MAX)
}

/// Integer division for Ipê's `//` operator.
///
/// Parity target: Ipê-Go `rt.IntDiv` (`runtime-go/rt/rt.go`).
///
/// Two cases the bare Rust `a / b` cannot handle on `i64`:
///
/// 1. `b == 0` — Rust panics "attempt to divide by zero"; Ipê-Go also panics
///    (`panic("rt.IntDiv: integer division by zero")`), classified as
///    `DivisionByZero` by the top-level recover and exits 101. We reproduce
///    the **panic** deliberately (not a silent 0): a `panic!` here carries the
///    standard "attempt to divide by zero" message so the process's panic
///    handler classifies it identically (`exit 101, empty stdout`), matching
///    the `intdiv_by_zero_aborts_exit_101` golden.
///
///    NOTE: Elm's `5 // 0 == 0` (returns 0) is a different design choice and
///    is **not** this port's target. Diverging from Ipê-Go to Elm-0 requires
///    an explicit `sanctioned.divergence` marker — never an implementer's call.
///
/// 2. `i64::MIN / -1` — Rust panics "attempt to divide with overflow" even
///    when `overflow-checks = false`, because this is an unconditional hardware
///    trap on x86-64. Ipê-Go uses two's-complement Go integer arithmetic and
///    returns `i64::MIN` (wraps silently). `wrapping_div` reproduces that.
///
/// For every other `(a, b)` pair the result is identical to `a / b` (truncate
/// toward zero — Go spec, Elm spec, Rust `wrapping_div` all agree).
///
/// # Panics
///
/// Panics when `b == 0`, mirroring the Ipê integer division-by-zero abort.
//
// `clippy::panic` is suppressed here deliberately: this panic IS the
// DivisionByZero abort that Ipê-Go models. The lint exists to prevent
// accidental panics; this is a classified, tested, intentional one. The
// `intdiv_by_zero_aborts_exit_101` golden verifies exit 101 / empty stdout.
#[allow(clippy::panic)]
#[must_use]
pub fn ipe_int_div(a: i64, b: i64) -> i64 {
    // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — this abort IS the modeled integer division-by-zero semantics, classified and golden-tested (exit 101) [ledger #boundary]
    assert!(b != 0, "attempt to divide by zero");
    a.wrapping_div(b)
}

// CONTRACT (documented deliberately): `min`/`max` use a real
// `PartialOrd` compare. For floats this tracks the TYPED Go path (`Math_minT`),
// NOT Go's polymorphic any-path (which routes floats through `AsInt` and compares
// truncated ints) — the typed compare is the correct one. NaN tie-break is fixed
// and total: `min(NaN, x)` / `max(NaN, x)` return the SECOND argument (`<=`/`>=`
// is false for any NaN), never panic.
pub fn math_min<T: PartialOrd>(a: T, b: T) -> T {
    if a <= b { a } else { b }
}
pub fn math_max<T: PartialOrd>(a: T, b: T) -> T {
    if a >= b { a } else { b }
}

#[must_use]
pub fn math_sqrt(x: f64) -> f64 {
    x.sqrt()
}
#[must_use]
pub fn math_pow(base: f64, exp: f64) -> f64 {
    base.powf(exp)
}

// CONTRACT (documented deliberately): the `as i64` float→int
// casts SATURATE by Rust's definition — NaN → 0, +∞ → i64::MAX, −∞ → i64::MIN,
// out-of-range finite → the nearest bound. This is TOTAL (never panics/UB) and is
// the deliberate contract; Go's `int64(math.Floor(x))` on the same extreme inputs
// is implementation-defined (not a well-defined parity target), so we pin the
// safe saturating behaviour rather than chase undefined Go output.
#[must_use]
pub fn math_floor(x: f64) -> i64 {
    x.floor() as i64
}
#[must_use]
pub fn math_ceil(x: f64) -> i64 {
    x.ceil() as i64
}

/// Ipê `round : Float -> Int` — half-away-from-zero (Go's `math.Round` semantics
/// match this). Rust's `f64::round` also goes half-away-from-zero, so we just cast.
#[must_use]
pub fn math_round(x: f64) -> i64 {
    x.round() as i64
}
#[must_use]
pub fn math_trunc(x: f64) -> i64 {
    x.trunc() as i64
}

// Exponential / logarithmic (Ipe.Math: exp, exp2, log [natural], log2, log10).
#[must_use]
pub fn math_exp(x: f64) -> f64 {
    x.exp()
}
#[must_use]
pub fn math_exp2(x: f64) -> f64 {
    x.exp2()
}
#[must_use]
pub fn math_log(x: f64) -> f64 {
    x.ln()
}
#[must_use]
pub fn math_log2(x: f64) -> f64 {
    x.log2()
}
#[must_use]
pub fn math_log10(x: f64) -> f64 {
    x.log10()
}
#[must_use]
pub fn math_cbrt(x: f64) -> f64 {
    x.cbrt()
}
#[must_use]
pub fn math_hypot(a: f64, b: f64) -> f64 {
    a.hypot(b)
}

// Trigonometric.
#[must_use]
pub fn math_sin(x: f64) -> f64 {
    x.sin()
}
#[must_use]
pub fn math_cos(x: f64) -> f64 {
    x.cos()
}
#[must_use]
pub fn math_tan(x: f64) -> f64 {
    x.tan()
}
#[must_use]
pub fn math_asin(x: f64) -> f64 {
    x.asin()
}
#[must_use]
pub fn math_acos(x: f64) -> f64 {
    x.acos()
}
#[must_use]
pub fn math_atan(x: f64) -> f64 {
    x.atan()
}
#[must_use]
pub fn math_atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

// Hyperbolic.
#[must_use]
pub fn math_sinh(x: f64) -> f64 {
    x.sinh()
}
#[must_use]
pub fn math_cosh(x: f64) -> f64 {
    x.cosh()
}
#[must_use]
pub fn math_tanh(x: f64) -> f64 {
    x.tanh()
}
#[must_use]
pub fn math_asinh(x: f64) -> f64 {
    x.asinh()
}
#[must_use]
pub fn math_acosh(x: f64) -> f64 {
    x.acosh()
}
#[must_use]
pub fn math_atanh(x: f64) -> f64 {
    x.atanh()
}

// Modulo + remainder (parity with Go's math.Mod / math.Remainder).
/// Float modulo — result has the sign of x (the dividend).
/// Equivalent to Go's `math.Mod(x, y)` and C's `fmod(x, y)`.
/// Rust's `x % y` is identical to C fmod for f64.
#[must_use]
pub fn math_mod(x: f64, y: f64) -> f64 {
    x % y
}
/// IEEE 754 balanced remainder — equivalent to Go's `math.Remainder(x, y)`.
/// The result satisfies `x = n*y + remainder` where `n` is the nearest integer
/// to `x/y` (rounded half-to-even), so `|remainder| <= |y|/2`.
///
/// Uses fmod-based argument reduction (a direct port of Go's
/// `math.Remainder`) so it stays exact for large-magnitude operands. A naive
/// `x - (x / y).round_ties_even() * y` loses precision once `|x/y|` exceeds
/// 2^53 (the quotient rounds away the low integer bits), diverging from the
/// true IEEE 754 remainder — this reduction avoids that.
#[must_use]
pub fn math_remainder(x: f64, y: f64) -> f64 {
    // Twice the smallest positive normal double (0x0020000000000000).
    const TINY: f64 = f64::MIN_POSITIVE * 2.0;
    let half_max = f64::MAX / 2.0;

    if x.is_nan() || y.is_nan() || x.is_infinite() || y == 0.0 {
        return f64::NAN;
    }
    if y.is_infinite() {
        return x;
    }

    let sign = x < 0.0;
    let mut x = x.abs();
    let y = y.abs();

    // Exact equality on abs-reduced values is intentional in IEEE 754 remainder
    // argument reduction (ported from Go's math.Remainder).
    #[allow(clippy::float_cmp)]
    if x == y {
        return if sign { -0.0 } else { 0.0 };
    }
    if y <= half_max {
        x %= y + y; // fmod; now x < 2y
    }
    if y < TINY {
        if x + x > y {
            x -= y;
            if x + x >= y {
                x -= y;
            }
        }
    } else {
        let y_half = 0.5 * y;
        if x > y_half {
            x -= y;
            if x >= y_half {
                x -= y;
            }
        }
    }
    if sign {
        x = -x;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_math_abs_positive() {
        assert_eq!(math_abs(5), 5);
    }
    #[test]
    fn test_math_abs_negative() {
        assert_eq!(math_abs(-5), 5);
    }
    #[test]
    fn test_math_abs_zero() {
        assert_eq!(math_abs(0), 0);
    }

    #[test]
    fn test_math_min_max_i64() {
        assert_eq!(math_min::<i64>(3, 5), 3);
        assert_eq!(math_max::<i64>(3, 5), 5);
        assert_eq!(math_min::<i64>(-2, -5), -5);
    }

    #[test]
    // 3.5, 2.25, 3.0, 1024.0 are all exactly representable IEEE 754 values.
    #[allow(clippy::float_cmp)]
    fn test_math_min_max_f64() {
        assert_eq!(math_min::<f64>(3.5, 2.25), 2.25);
        assert_eq!(math_max::<f64>(3.5, 2.25), 3.5);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_math_sqrt() {
        assert_eq!(math_sqrt(9.0), 3.0);
    }
    #[test]
    #[allow(clippy::float_cmp)]
    fn test_math_pow() {
        assert_eq!(math_pow(2.0, 10.0), 1024.0);
    }

    #[test]
    fn test_math_floor_ceil_round() {
        assert_eq!(math_floor(3.7), 3);
        assert_eq!(math_floor(-3.2), -4);
        assert_eq!(math_ceil(3.2), 4);
        assert_eq!(math_ceil(-3.7), -3);
        assert_eq!(math_round(3.5), 4);
        assert_eq!(math_round(-3.5), -4); // half-away-from-zero
        assert_eq!(math_round(2.4), 2);
    }

    // ── go-parity regression tests ──────────────────────

    #[test]
    fn test_math_phi() {
        // Go: math.Phi = 1.618033988749895
        let phi = math_phi();
        assert!((phi - 1.618_033_988_749_895_f64).abs() < 1e-14);
    }

    #[test]
    fn test_math_sqrt2() {
        let s = math_sqrt2();
        assert!((s - std::f64::consts::SQRT_2).abs() < 1e-15);
    }

    #[test]
    fn test_math_inf() {
        assert!(math_inf().is_infinite() && math_inf() > 0.0);
    }

    #[test]
    fn test_math_nan() {
        let n = math_nan();
        assert!(n.is_nan());
        // IEEE 754: NaN != NaN
        #[allow(clippy::eq_op, clippy::float_cmp)]
        {
            assert_ne!(n, n);
        }
    }

    #[test]
    fn test_math_is_nan() {
        assert!(math_is_nan(f64::NAN));
        assert!(!math_is_nan(0.0));
        assert!(!math_is_nan(f64::INFINITY));
        assert!(!math_is_nan(1.0));
    }

    #[test]
    // 1.5 is exactly representable; these are the exact Go oracle values.
    #[allow(clippy::float_cmp)]
    fn test_math_mod() {
        // Go: math.Mod(5.5, 2.0) = 1.5  (sign of dividend)
        assert_eq!(math_mod(5.5, 2.0), 1.5);
        assert_eq!(math_mod(-5.5, 2.0), -1.5); // sign of dividend
        assert_eq!(math_mod(5.5, -2.0), 1.5);
    }

    #[test]
    fn test_math_remainder() {
        // Go: math.Remainder(5.5, 2.0) = -0.5  (IEEE 754 balanced)
        let r = math_remainder(5.5, 2.0);
        assert!((r - (-0.5_f64)).abs() < 1e-14, "expected -0.5, got {r}");
        // round_ties_even: x/y = 2.75 → nearest even integer = 2, so 5.5 - 2*2.0 = 1.5
        let r2 = math_remainder(5.5, 4.0);
        assert!((r2 - 1.5_f64).abs() < 1e-14, "expected 1.5, got {r2}");
    }
}

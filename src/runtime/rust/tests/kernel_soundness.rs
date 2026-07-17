//! Soundness + behaviour coverage for arithmetic / random / decimal kernels.
//! Emphasis on the panic-prone sites (mod/div by zero, empty-list choice,
//! out-of-domain math) — each asserts the kernel is TOTAL (defined result,
//! never a Rust panic) plus the expected value, and the seeded-random kernels
//! assert determinism (same seed ⇒ same output).

use proptest::prelude::*;
use sky_runtime_rust::*;

// ── basics_mod_by — Elm positive-modulo, divisor 0 guarded ─────────────────

#[test]
fn mod_by_zero_returns_zero_not_panic() {
    assert_eq!(sky_runtime::basics::basics_mod_by(0, 7), 0);
    assert_eq!(sky_runtime::basics::basics_mod_by(0, -7), 0);
    assert_eq!(sky_runtime::basics::basics_mod_by(0, 0), 0);
}

#[test]
fn mod_by_positive_divisor_is_always_nonnegative() {
    assert_eq!(sky_runtime::basics::basics_mod_by(3, 7), 1);
    assert_eq!(sky_runtime::basics::basics_mod_by(3, -1), 2); // Elm: positive result
    assert_eq!(sky_runtime::basics::basics_mod_by(3, -4), 2);
    assert_eq!(sky_runtime::basics::basics_mod_by(5, 0), 0);
}

#[test]
fn mod_by_negative_divisor_matches_go() {
    // Go Basics_modByT(divisor, n): `r := n % divisor; if r < 0 { r += divisor }`.
    // basics_mod_by(-3, 7): 7 % -3 = 1 (Rust/Go % takes the dividend's sign);
    //   r=1 not < 0 ⇒ 1.
    assert_eq!(sky_runtime::basics::basics_mod_by(-3, 7), 1);
    // basics_mod_by(-3, -7): -7 % -3 = -1; r<0 ⇒ -1 + (-3) = -4.
    assert_eq!(sky_runtime::basics::basics_mod_by(-3, -7), -4);
}

#[test]
fn basics_fst_snd_identity_always() {
    assert_eq!(sky_runtime::basics::basics_fst((1i64, "x".to_string())), 1);
    assert_eq!(
        sky_runtime::basics::basics_snd((1i64, "x".to_string())),
        "x".to_string()
    );
    assert_eq!(sky_runtime::basics::basics_identity(42i64), 42);
    assert_eq!(
        sky_runtime::basics::basics_always(7i64, "ignored".to_string()),
        7
    );
}

proptest! {
    #[test]
    fn prop_mod_by_positive_divisor_in_range(d in 1i64..1_000_000, n in any::<i64>()) {
        let r = sky_runtime::basics::basics_mod_by(d, n);
        prop_assert!(r >= 0 && r < d);
    }

    #[test]
    fn prop_mod_by_zero_never_panics(n in any::<i64>()) {
        prop_assert_eq!(sky_runtime::basics::basics_mod_by(0, n), 0);
    }
}

// ── math — out-of-domain inputs are defined (NaN/inf), never panic ─────────

#[test]
fn math_out_of_domain_is_total() {
    assert!(sky_runtime::math::math_sqrt(-1.0).is_nan());
    assert_eq!(sky_runtime::math::math_sqrt(4.0), 2.0);
    assert!(sky_runtime::math::math_log(0.0).is_infinite());
    assert!(sky_runtime::math::math_log(-1.0).is_nan());
    // round of a non-finite / huge float saturates into i64 (defined `as` cast).
    let _ = sky_runtime::math::math_round(f64::NAN);
    let _ = sky_runtime::math::math_round(f64::INFINITY);
    assert_eq!(sky_runtime::math::math_round(2.5), 3);
    assert_eq!(sky_runtime::math::math_pow(2.0, 10.0), 1024.0);
}

proptest! {
    #[test]
    fn prop_math_round_never_panics(x in any::<f64>()) {
        let _ = sky_runtime::math::math_round(x); // must not panic for any f64
    }
    #[test]
    fn prop_math_sqrt_never_panics(x in any::<f64>()) {
        let _ = sky_runtime::math::math_sqrt(x);
    }
}

// ── seeded random — deterministic, in-range, empty-safe ────────────────────

#[test]
fn seeded_int_is_deterministic_and_in_range() {
    let (v1, s1) = sky_runtime::random::random_seeded_int(12345, 10, 20);
    let (v2, s2) = sky_runtime::random::random_seeded_int(12345, 10, 20);
    assert_eq!((v1, s1), (v2, s2), "same seed must give same output");
    assert!((10..=20).contains(&v1));
}

#[test]
fn seeded_int_hi_le_lo_returns_lo() {
    let (v, _) = sky_runtime::random::random_seeded_int(7, 5, 5);
    assert_eq!(v, 5);
    let (v2, _) = sky_runtime::random::random_seeded_int(7, 9, 1); // hi < lo
    assert_eq!(v2, 9);
}

#[test]
fn seeded_choice_empty_is_nothing_not_panic() {
    let (m, _): (SkyMaybe<i64>, i64) = sky_runtime::random::random_seeded_choice(42, vec![]);
    assert!(m.is_nothing());
}

#[test]
fn seeded_choice_picks_in_bounds_deterministically() {
    let items = vec!["a", "b", "c", "d"];
    let (m1, _) = sky_runtime::random::random_seeded_choice(999, items.clone());
    let (m2, _) = sky_runtime::random::random_seeded_choice(999, items.clone());
    assert!(m1.is_just());
    assert_eq!(m1, m2);
}

#[test]
fn seeded_float_in_unit_interval() {
    let (f, _) = sky_runtime::random::random_seeded_float(123);
    assert!((0.0..1.0).contains(&f));
}

proptest! {
    #[test]
    fn prop_seeded_int_always_in_range(seed in any::<i64>(), lo in -1000i64..1000, span in 0i64..1000) {
        let hi = lo + span;
        let (v, _) = sky_runtime::random::random_seeded_int(seed, lo, hi);
        prop_assert!(v >= lo && v <= hi);
    }
}

// ── decimal — divide/modulo by zero returns Err, never panics ──────────────

#[test]
fn decimal_div_by_zero_is_err() {
    let a = sky_runtime::decimal::decimal_from_int(10);
    let zero = sky_runtime::decimal::decimal_from_int(0);
    let r = sky_runtime::decimal::decimal_div::<SkyError>(a, zero);
    assert!(r.is_err());
}

#[test]
fn decimal_mod_by_zero_is_err() {
    let a = sky_runtime::decimal::decimal_from_int(10);
    let zero = sky_runtime::decimal::decimal_from_int(0);
    let r = sky_runtime::decimal::decimal_mod::<SkyError>(a, zero);
    assert!(r.is_err());
}

#[test]
fn decimal_div_normal_is_ok() {
    let a = sky_runtime::decimal::decimal_from_int(10);
    let b = sky_runtime::decimal::decimal_from_int(4);
    let r = sky_runtime::decimal::decimal_div::<SkyError>(a, b);
    assert!(r.is_ok());
    let q = r.with_default(sky_runtime::decimal::decimal_from_int(0));
    assert_eq!(sky_runtime::decimal::decimal_to_string(q), "2.5");
}

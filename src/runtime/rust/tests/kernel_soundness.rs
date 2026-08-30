| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! Soundness + behaviour coverage for arithmetic / random / decimal kernels.
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! Emphasis on the panic-prone sites (mod/div by zero, empty-list choice,
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! out-of-domain math) — each asserts the kernel is TOTAL (defined result,
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! never a Rust panic) plus the expected value, and the seeded-random kernels
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! assert determinism (same seed ⇒ same output).

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // The crate-root glob supplies the unqualified `IpeMaybe` / `IpeError` /
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // `with_default` names the `random`- and `decimal`-gated tests use; the other
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // tests fully qualify their kernel paths, so the glob is dead without either
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // feature.
#[cfg(any(feature = "random", feature = "decimal"))]
use ipe_runtime_rust::*;
use proptest::prelude::*;

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── basics_mod_by — Elm positive-modulo, divisor 0 guarded ─────────────────

#[test]
fn mod_by_zero_returns_zero_not_panic() {
    assert_eq!(ipe_runtime_rust::basics::basics_mod_by(0, 7), 0);
    assert_eq!(ipe_runtime_rust::basics::basics_mod_by(0, -7), 0);
    assert_eq!(ipe_runtime_rust::basics::basics_mod_by(0, 0), 0);
}

#[test]
fn mod_by_positive_divisor_is_always_nonnegative() {
    assert_eq!(ipe_runtime_rust::basics::basics_mod_by(3, 7), 1);
    assert_eq!(ipe_runtime_rust::basics::basics_mod_by(3, -1), 2); // Elm: positive result
    assert_eq!(ipe_runtime_rust::basics::basics_mod_by(3, -4), 2);
    assert_eq!(ipe_runtime_rust::basics::basics_mod_by(5, 0), 0);
}

#[test]
fn mod_by_negative_divisor_matches_go() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // modByT: `r := n % divisor; if r < 0 { r += divisor }`.
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // basics_mod_by(-3, 7): 7 % -3 = 1 (% takes the dividend's sign);
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     //   r=1 not < 0 ⇒ 1.
    assert_eq!(ipe_runtime_rust::basics::basics_mod_by(-3, 7), 1);
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // basics_mod_by(-3, -7): -7 % -3 = -1; r<0 ⇒ -1 + (-3) = -4.
    assert_eq!(ipe_runtime_rust::basics::basics_mod_by(-3, -7), -4);
}

#[test]
fn basics_fst_snd_identity_always() {
    assert_eq!(
        ipe_runtime_rust::basics::basics_fst((1i64, "x".to_string())),
        1
    );
    assert_eq!(
        ipe_runtime_rust::basics::basics_snd((1i64, "x".to_string())),
        "x".to_string()
    );
    assert_eq!(ipe_runtime_rust::basics::basics_identity(42i64), 42);
    assert_eq!(
        ipe_runtime_rust::basics::basics_always(7i64, "ignored".to_string()),
        7
    );
}

proptest! {
    #[test]
    fn prop_mod_by_positive_divisor_in_range(d in 1i64..1_000_000, n in any::<i64>()) {
        let r = ipe_runtime_rust::basics::basics_mod_by(d, n);
        prop_assert!(r >= 0 && r < d);
    }

    #[test]
    fn prop_mod_by_zero_never_panics(n in any::<i64>()) {
        prop_assert_eq!(ipe_runtime_rust::basics::basics_mod_by(0, n), 0);
    }
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── math — out-of-domain inputs are defined (NaN/inf), never panic ─────────

#[test]
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // Exact-equality comparisons here are intentional: sqrt(4) and pow(2,10)
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // produce exact IEEE 754 results (2.0 and 1024.0 respectively).
#[allow(clippy::float_cmp)]
fn math_out_of_domain_is_total() {
    assert!(ipe_runtime_rust::math::math_sqrt(-1.0).is_nan());
    assert_eq!(ipe_runtime_rust::math::math_sqrt(4.0), 2.0);
    assert!(ipe_runtime_rust::math::math_log(0.0).is_infinite());
    assert!(ipe_runtime_rust::math::math_log(-1.0).is_nan());
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // round of a non-finite / huge float saturates into i64 (defined `as` cast).
    let _ = ipe_runtime_rust::math::math_round(f64::NAN);
    let _ = ipe_runtime_rust::math::math_round(f64::INFINITY);
    assert_eq!(ipe_runtime_rust::math::math_round(2.5), 3);
    assert_eq!(ipe_runtime_rust::math::math_pow(2.0, 10.0), 1024.0);
}

proptest! {
    #[test]
    fn prop_math_round_never_panics(x in any::<f64>()) {
        let _ = ipe_runtime_rust::math::math_round(x); // must not panic for any f64
    }
    #[test]
    fn prop_math_sqrt_never_panics(x in any::<f64>()) {
        let _ = ipe_runtime_rust::math::math_sqrt(x);
    }
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── seeded random — deterministic, in-range, empty-safe ────────────────────
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // The `Ipe.Random` surface (`random.rs`) is behind the `random` feature; these
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // fixtures compile only when it is selected (CI's `--features full` includes it).

#[cfg(feature = "random")]
#[test]
fn seeded_int_is_deterministic_and_in_range() {
    let (v1, s1) = ipe_runtime_rust::random::random_seeded_int(12345, 10, 20);
    let (v2, s2) = ipe_runtime_rust::random::random_seeded_int(12345, 10, 20);
    assert_eq!((v1, s1), (v2, s2), "same seed must give same output");
    assert!((10..=20).contains(&v1));
}

#[cfg(feature = "random")]
#[test]
fn seeded_int_hi_le_lo_returns_lo() {
    let (v, _) = ipe_runtime_rust::random::random_seeded_int(7, 5, 5);
    assert_eq!(v, 5);
    let (v2, _) = ipe_runtime_rust::random::random_seeded_int(7, 9, 1); // hi < lo
    assert_eq!(v2, 9);
}

#[cfg(feature = "random")]
#[test]
fn seeded_choice_empty_is_nothing_not_panic() {
    let (m, _): (IpeMaybe<i64>, i64) = ipe_runtime_rust::random::random_seeded_choice(42, vec![]);
    assert!(m.is_nothing());
}

#[cfg(feature = "random")]
#[test]
fn seeded_choice_picks_in_bounds_deterministically() {
    let items = vec!["a", "b", "c", "d"];
    let (m1, _) = ipe_runtime_rust::random::random_seeded_choice(999, items.clone());
    let (m2, _) = ipe_runtime_rust::random::random_seeded_choice(999, items.clone());
    assert!(m1.is_just());
    assert_eq!(m1, m2);
}

#[cfg(feature = "random")]
#[test]
fn seeded_float_in_unit_interval() {
    let (f, _) = ipe_runtime_rust::random::random_seeded_float(123);
    assert!((0.0..1.0).contains(&f));
}

#[cfg(feature = "random")]
proptest! {
    #[test]
    fn prop_seeded_int_always_in_range(seed in any::<i64>(), lo in -1000i64..1000, span in 0i64..1000) {
        let hi = lo + span;
        let (v, _) = ipe_runtime_rust::random::random_seeded_int(seed, lo, hi);
        prop_assert!(v >= lo && v <= hi);
    }
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── decimal — divide/modulo by zero returns Err, never panics ──────────────
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // `decimal.rs` is behind the `decimal` feature, so these tests are too.

#[cfg(feature = "decimal")]
#[test]
fn decimal_div_by_zero_is_err() {
    let a = ipe_runtime_rust::decimal::decimal_from_int(10);
    let zero = ipe_runtime_rust::decimal::decimal_from_int(0);
    let r = ipe_runtime_rust::decimal::decimal_div::<IpeError>(a, zero);
    assert!(r.is_err());
}

#[cfg(feature = "decimal")]
#[test]
fn decimal_mod_by_zero_is_err() {
    let a = ipe_runtime_rust::decimal::decimal_from_int(10);
    let zero = ipe_runtime_rust::decimal::decimal_from_int(0);
    let r = ipe_runtime_rust::decimal::decimal_mod::<IpeError>(a, zero);
    assert!(r.is_err());
}

#[cfg(feature = "decimal")]
#[test]
fn decimal_div_normal_is_ok() {
    let a = ipe_runtime_rust::decimal::decimal_from_int(10);
    let b = ipe_runtime_rust::decimal::decimal_from_int(4);
    let r = ipe_runtime_rust::decimal::decimal_div::<IpeError>(a, b);
    assert!(r.is_ok());
    let q = r.with_default(ipe_runtime_rust::decimal::decimal_from_int(0));
    assert_eq!(ipe_runtime_rust::decimal::decimal_to_string(q), "2.5");
}

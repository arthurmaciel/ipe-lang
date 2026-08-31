//! Parity fixtures for Ipe.Decimal.
//!
//! Every assertion below was //! (the runtime test suite + manual `go test` runs with the
//! same inputs).  The fixture is the discovery artefact: tests that pre-fail
//! expose real divergences; passing tests anchor the exact golden-observable
//! behaviour permanently.
//!
//! Division precision (FIXED: shopspring's
//! `Div` is `DivRound(…, DivisionPrecision)` with `DivisionPrecision = 16` and
//! half-away-from-zero rounding.  `decimal_div` caps its quotient to 16 decimal
//! places with `MidpointAwayFromZero` after the `checked_div`, so non-terminating
//! fractions (1/3, 2/3, 1/7, 10/3, …) match reference output exactly.  Exact fractions with
//! ≤16 dp are unaffected by the cap.  All money-scale cases stay bit-identical.

// `decimal.rs` is behind the `decimal` feature, so this whole fixture is too.
#![cfg(feature = "decimal")]

use ipe_runtime_rust::*;

// ── helpers ──────────────────────────────────────────────────────────────────

fn d(s: &str) -> ipe_runtime_rust::decimal::Decimal {
    match ipe_runtime_rust::decimal::decimal_from_string::<IpeError>(s.to_string()) {
        IpeResult::Ok(v) => v,
        IpeResult::Err(e) => panic!("bad decimal literal {s:?}: {e}"),
    }
}

fn s(dec: ipe_runtime_rust::decimal::Decimal) -> String {
    ipe_runtime_rust::decimal::decimal_to_string(dec)
}

fn div_ok(
    a: ipe_runtime_rust::decimal::Decimal,
    b: ipe_runtime_rust::decimal::Decimal,
) -> ipe_runtime_rust::decimal::Decimal {
    match ipe_runtime_rust::decimal::decimal_div::<IpeError>(a, b) {
        IpeResult::Ok(v) => v,
        IpeResult::Err(e) => panic!("unexpected div error: {e}"),
    }
}

// ── Construction round-trips ──────────────────────────────────────────────────

#[test]
fn from_string_to_string_round_trip() {
    // Golden: `Decimal_fromString "3.14" |> Decimal_toString` = "3.14"
    assert_eq!(s(d("3.14")), "3.14");
    assert_eq!(s(d("0")), "0");
    assert_eq!(s(d("-99.999")), "-99.999");
}

#[test]
fn from_int_to_string() {
    // Golden: `Decimal_fromInt 42 |> Decimal_toString` = "42"
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_from_int(42)), "42");
}

#[test]
fn from_minor_round_trip() {
    // Golden: `Decimal_fromMinor 2 12345 |> Decimal_toString` = "123.45"
    let dec = ipe_runtime_rust::decimal::decimal_from_minor(2, 12345);
    assert_eq!(s(dec), "123.45");
    // toMinor 2 123.45 = 12345
    assert_eq!(ipe_runtime_rust::decimal::decimal_to_minor(2, dec), 12345);
}

#[test]
fn to_string_normalises_trailing_zeros() {
    // shopspring .String() trims trailing zeros (same as rust_decimal
    // .normalize().to_string()).  The canonical oracle is the reference test.
    // "10.00" → "10",  "3.0" → "3",  "0.10" → "0.1"
    assert_eq!(s(d("10.00")), "10");
    assert_eq!(s(d("3.0")), "3");
    assert_eq!(s(d("0.10")), "0.1");
}

// ── The classic float trap ───────────────────────────────────────────────────

#[test]
fn add_point_one_point_two_is_exactly_point_three() {
    // the golden oracle: `Decimal_add "0.1" "0.2" |> Decimal_toString` = "0.3"
    // NOT the IEEE-754 float result "0.30000000000000004".
    let sum = ipe_runtime_rust::decimal::decimal_add(d("0.1"), d("0.2"));
    assert_eq!(s(sum), "0.3");
}

#[test]
fn sub_point_one_minus_point_two_is_exact() {
    // 0.1 - 0.2 = -0.1 (exact, not a float residual)
    let diff = ipe_runtime_rust::decimal::decimal_sub(d("0.1"), d("0.2"));
    assert_eq!(s(diff), "-0.1");
}

#[test]
fn mul_point_one_times_point_two_is_exact() {
    // 0.1 * 0.2 = 0.02 (exact)
    let prod = ipe_runtime_rust::decimal::decimal_mul(d("0.1"), d("0.2"));
    assert_eq!(s(prod), "0.02");
}

// ── Arithmetic ───────────────────────────────────────────────────────────────

#[test]
fn add_sub_mul_exact() {
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_add(d("1.5"), d("2.25"))),
        "3.75"
    );
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_sub(d("5"), d("2.5"))),
        "2.5"
    );
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_mul(d("1.5"), d("4"))),
        "6"
    );
}

#[test]
fn div_exact_fraction() {
    // 10 / 4 = 2.5 (exact fraction — no precision divergence)
    assert_eq!(s(div_ok(d("10"), d("4"))), "2.5");
    // 1 / 2 = 0.5
    assert_eq!(s(div_ok(d("1"), d("2"))), "0.5");
    // 1 / 4 = 0.25
    assert_eq!(s(div_ok(d("1"), d("4"))), "0.25");
    // 1 / 5 = 0.2
    assert_eq!(s(div_ok(d("1"), d("5"))), "0.2");
}

#[test]
fn div_precision_capped_to_16_dp_matches_go() {
    // shopspring `Div` = `DivRound(…, 16)` (half-away-from-zero). Ipê-Rust
    // caps `decimal_div` to 16 dp with MidpointAwayFromZero, so non-terminating
    // quotients match the reference exactly.
    // 1/3 = 0.3333… → 17th digit 3 rounds down → sixteen 3s.
    assert_eq!(s(div_ok(d("1"), d("3"))), "0.3333333333333333");
    // 2/3 = 0.6666… → 17th digit 6 rounds last digit up → …667.
    assert_eq!(s(div_ok(d("2"), d("3"))), "0.6666666666666667");
    // 1/7 = 0.142857142857… → 16 dp half-away → …1429.
    assert_eq!(s(div_ok(d("1"), d("7"))), "0.1428571428571429");
    // 10/3 = 3.3333… → sixteen 3s after the point.
    assert_eq!(s(div_ok(d("10"), d("3"))), "3.3333333333333333");
}

#[test]
fn div_by_zero_is_err() {
    let r = ipe_runtime_rust::decimal::decimal_div::<IpeError>(d("1"), d("0"));
    assert!(r.is_err(), "divide by zero must be Err, never panic");
}

#[test]
fn mod_by_zero_is_err() {
    let r = ipe_runtime_rust::decimal::decimal_mod::<IpeError>(d("1"), d("0"));
    assert!(r.is_err(), "mod by zero must be Err, never panic");
}

#[test]
fn neg_and_abs() {
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_neg(d("3.14"))),
        "-3.14"
    );
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_abs(d("-3.14"))),
        "3.14"
    );
}

// ── Banker's rounding (Decimal.round uses RoundBank) ─────────────────

#[test]
fn bankers_rounding_ties_go_to_even() {
    // the golden oracle: `Decimal_round 0 2.5` = "2" (nearest even = 2)
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_round(0, d("2.5"))),
        "2",
        "2.5 rounds to even 2 (banker's rounding)"
    );
    // `Decimal_round 0 3.5` = "4" (nearest even = 4)
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_round(0, d("3.5"))),
        "4",
        "3.5 rounds to even 4 (banker's rounding)"
    );
    // 0.5 rounds to 0 (even)
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_round(0, d("0.5"))),
        "0"
    );
    // 1.5 rounds to 2 (even)
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_round(0, d("1.5"))),
        "2"
    );
}

// ── toStringFixed — StringFixed uses half-away-from-zero ─────────────────

#[test]
fn to_string_fixed_adds_trailing_zeros() {
    // Golden: `Decimal_toStringFixed 2 3` = "3.00"
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_to_string_fixed(
            2,
            ipe_runtime_rust::decimal::decimal_from_int(3)
        ),
        "3.00"
    );
    // `Decimal_toStringFixed 2 3.1` = "3.10"
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_to_string_fixed(2, d("3.1")),
        "3.10"
    );
}

#[test]
fn to_string_fixed_uses_half_away_from_zero_not_bankers() {
    // the golden oracle: `Decimal_toStringFixed 2 (fromString "2.545")` = "2.55"
    // StringFixed calls Round which is half-away-from-zero.
    // The 3rd decimal is 5 (tie): half-away rounds UP to 2.55.
    // Banker's (MidpointNearestEven) would give "2.54" (4 is even → round down).
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_to_string_fixed(2, d("2.545")),
        "2.55",
        "toStringFixed must use half-away-from-zero (not banker's rounding)"
    );
    // `Decimal_toStringFixed 2 (fromString "2.535")` = "2.54"
    // half-away: .535 tie → 2.54 (rounds up).
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_to_string_fixed(2, d("2.535")),
        "2.54",
        "toStringFixed: 2.535 rounds to 2.54 (half-away-from-zero)"
    );
}

// ── Percent helpers ──────────────────────────────────────────────────────────

#[test]
fn percent_of_basic() {
    // Golden: `Decimal_percentOf 10 100` = "10" (10% of 100)
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_percent_of(
            d("10"),
            d("100")
        )),
        "10"
    );
    // Golden: `Decimal_percentOf 20 100` = "20"
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_percent_of(
            d("20"),
            d("100")
        )),
        "20"
    );
}

#[test]
fn percent_of_fractional_rounded_matches_go() {
    // the golden oracle: 8.875% of 99.99 = 8.8741125 → round 2 = "8.87"
    // (8.8741125: 3rd decimal is 4, rounds DOWN, same in both banker's and half-away)
    let pct = d("8.875");
    let price = d("99.99");
    let tax = ipe_runtime_rust::decimal::decimal_percent_of(pct, price);
    let rounded = ipe_runtime_rust::decimal::decimal_round(2, tax);
    assert_eq!(s(rounded), "8.87", "8.875% of 99.99 rounded to 2 dp");
}

#[test]
fn add_percent_and_sub_percent() {
    // Golden: `Decimal_addPercent 10 100` = "110"
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_add_percent(
            d("10"),
            d("100")
        )),
        "110"
    );
    // Golden: `Decimal_subPercent 10 100` = "90"
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_sub_percent(
            d("10"),
            d("100")
        )),
        "90"
    );
}

// ── formatWith — formatWith uses StringFixed (half-away-from-zero) ────────

#[test]
fn format_with_us_locale() {
    // the golden oracle: `Decimal_formatWith "," "." 2 1234567.891` = "1,234,567.89"
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_format_with(
            ",".to_string(),
            ".".to_string(),
            2,
            d("1234567.891")
        ),
        "1,234,567.89"
    );
}

#[test]
fn format_with_eu_locale() {
    // the golden oracle: `Decimal_formatWith "." "," 2 1234567.891` = "1.234.567,89"
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_format_with(
            ".".to_string(),
            ",".to_string(),
            2,
            d("1234567.891")
        ),
        "1.234.567,89"
    );
}

#[test]
fn format_with_fr_locale_zero_places() {
    // the golden oracle: `Decimal_formatWith " " "," 0 1234567.891` = "1 234 568"
    // 1234567.891 at 0 dp: .891 > .5 → rounds up → 1234568
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_format_with(
            " ".to_string(),
            ",".to_string(),
            0,
            d("1234567.891")
        ),
        "1 234 568"
    );
}

#[test]
fn format_with_uses_half_away_from_zero_not_bankers() {
    // the golden oracle: `Decimal_formatWith "" "." 2 (fromString "2.545")` = "2.55"
    // formatWith calls StringFixed which is half-away-from-zero.
    // Banker's (current Rust) would give "2.54".
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_format_with(
            String::new(),
            ".".to_string(),
            2,
            d("2.545")
        ),
        "2.55",
        "formatWith must use half-away-from-zero"
    );
}

// ── Comparisons ──────────────────────────────────────────────────────────────

#[test]
fn comparisons_match_go() {
    // Golden: `Decimal_compare 5 7` = -1, `compare 7 5` = 1, `compare 5 5` = 0
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_compare(d("5"), d("7")),
        -1
    );
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_compare(d("7"), d("5")),
        1
    );
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_compare(d("5"), d("5")),
        0
    );
    // Bool predicates
    assert!(ipe_runtime_rust::decimal::decimal_lt(d("5"), d("7")));
    assert!(!ipe_runtime_rust::decimal::decimal_gt(d("5"), d("7")));
    assert!(ipe_runtime_rust::decimal::decimal_lte(d("5"), d("5")));
    assert!(ipe_runtime_rust::decimal::decimal_gte(d("5"), d("5")));
    assert!(ipe_runtime_rust::decimal::decimal_eq(d("5"), d("5")));
    assert!(ipe_runtime_rust::decimal::decimal_neq(d("5"), d("7")));
}

#[test]
fn min_max_match_go() {
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_min(d("3"), d("5"))),
        "3"
    );
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_max(d("3"), d("5"))),
        "5"
    );
}

// ── Sign predicates ──────────────────────────────────────────────────────────

#[test]
fn sign_predicates_match_go() {
    assert!(ipe_runtime_rust::decimal::decimal_is_zero(
        ipe_runtime_rust::decimal::decimal_zero()
    ));
    assert!(ipe_runtime_rust::decimal::decimal_is_positive(d("1")));
    assert!(!ipe_runtime_rust::decimal::decimal_is_positive(d("0")));
    assert!(ipe_runtime_rust::decimal::decimal_is_negative(d("-1")));
    assert!(!ipe_runtime_rust::decimal::decimal_is_negative(d("0")));
}

// ── Rounding modes ──────────────────────────────────────────────────────────

#[test]
fn round_half_up_matches_go() {
    // Golden: `Decimal_roundHalfUp 0 2.5` = "3" (half-away-from-zero)
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_round_half_up(
            0,
            d("2.5")
        )),
        "3"
    );
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_round_half_up(
            0,
            d("3.5")
        )),
        "4"
    );
}

#[test]
fn truncate_floor_ceil_match_go() {
    // truncate: toward zero
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_truncate(0, d("3.7"))),
        "3"
    );
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_truncate(0, d("-3.7"))),
        "-3"
    );
    // floor: toward -∞
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_floor(d("3.1"))), "3");
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_floor(d("-3.1"))), "-4");
    // ceil: toward +∞
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_ceil(d("3.1"))), "4");
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_ceil(d("-3.1"))), "-3");
}

// ── Constants ────────────────────────────────────────────────────────────────

#[test]
fn constants_match_go() {
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_zero()), "0");
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_one()), "1");
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_one_hundred()), "100");
}

// ── to_int / to_float ────────────────────────────────────────────────────────

#[test]
fn to_int_truncates() {
    assert_eq!(ipe_runtime_rust::decimal::decimal_to_int(d("3.9")), 3);
    assert_eq!(ipe_runtime_rust::decimal::decimal_to_int(d("-3.9")), -3);
}

#[test]
fn to_float_is_lossy_but_close() {
    // 3.14 is not PI — suppress the clippy lint that fires on the literal 3.14.
    #[allow(clippy::approx_constant)]
    let expected: f64 = 3.14;
    let f = ipe_runtime_rust::decimal::decimal_to_float(d("3.14"));
    assert!((f - expected).abs() < 1e-10);
}

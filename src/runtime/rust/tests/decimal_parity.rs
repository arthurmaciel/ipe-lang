| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! Parity fixtures for Ipe.Decimal.
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //!
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! Every assertion below was verified against the golden oracle
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! (`//! same inputs).  The fixture is the discovery artefact: tests that pre-fail
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! expose real divergences; passing tests anchor the exact observed
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! behaviour permanently.
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //!
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! Division precision (FIXED — not a divergence): shopspring's
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! `Div` is `DivRound(…, DivisionPrecision)` with `DivisionPrecision = 16` and
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! half-away-from-zero rounding.  `decimal_div` caps its quotient to 16 decimal
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! places with `MidpointAwayFromZero` after the `checked_div`, so non-terminating
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! fractions (1/3, 2/3, 1/7, 10/3, …) exact.  Exact fractions with
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! ≤16 dp are unaffected by the cap.  All money-scale cases stay bit-identical.

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // `decimal.rs` is behind the `decimal` feature, so this whole fixture is too.
#![cfg(feature = "decimal")]

use ipe_runtime_rust::*;

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── helpers ──────────────────────────────────────────────────────────────────

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

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── Construction round-trips ──────────────────────────────────────────────────

#[test]
fn from_string_to_string_round_trip() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # `Decimal_fromString "3.14" |> Decimal_toString` = "3.14"
    assert_eq!(s(d("3.14")), "3.14");
    assert_eq!(s(d("0")), "0");
    assert_eq!(s(d("-99.999")), "-99.999");
}

#[test]
fn from_int_to_string() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # `Decimal_fromInt 42 |> Decimal_toString` = "42"
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_from_int(42)), "42");
}

#[test]
fn from_minor_round_trip() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # `Decimal_fromMinor 2 12345 |> Decimal_toString` = "123.45"
    let dec = ipe_runtime_rust::decimal::decimal_from_minor(2, 12345);
    assert_eq!(s(dec), "123.45");
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // toMinor 2 123.45 = 12345
    assert_eq!(ipe_runtime_rust::decimal::decimal_to_minor(2, dec), 12345);
}

#[test]
fn to_string_normalises_trailing_zeros() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // shopspring .String() trims trailing zeros (same as rust_decimal
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // .normalize().to_string()).  The canonical oracle is the reference.
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // "10.00" → "10",  "3.0" → "3",  "0.10" → "0.1"
    assert_eq!(s(d("10.00")), "10");
    assert_eq!(s(d("3.0")), "3");
    assert_eq!(s(d("0.10")), "0.1");
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── The classic float trap ───────────────────────────────────────────────────

#[test]
fn add_point_one_point_two_is_exactly_point_three() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // Golden: `Decimal_add "0.1" "0.2" |> Decimal_toString` = "0.3"
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // NOT the IEEE-754 float result "0.30000000000000004".
    let sum = ipe_runtime_rust::decimal::decimal_add(d("0.1"), d("0.2"));
    assert_eq!(s(sum), "0.3");
}

#[test]
fn sub_point_one_minus_point_two_is_exact() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 0.1 - 0.2 = -0.1 (exact, not a float residual)
    let diff = ipe_runtime_rust::decimal::decimal_sub(d("0.1"), d("0.2"));
    assert_eq!(s(diff), "-0.1");
}

#[test]
fn mul_point_one_times_point_two_is_exact() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 0.1 * 0.2 = 0.02 (exact)
    let prod = ipe_runtime_rust::decimal::decimal_mul(d("0.1"), d("0.2"));
    assert_eq!(s(prod), "0.02");
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── Arithmetic ───────────────────────────────────────────────────────────────

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
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 10 / 4 = 2.5 (exact fraction — no precision divergence)
    assert_eq!(s(div_ok(d("10"), d("4"))), "2.5");
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 1 / 2 = 0.5
    assert_eq!(s(div_ok(d("1"), d("2"))), "0.5");
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 1 / 4 = 0.25
    assert_eq!(s(div_ok(d("1"), d("4"))), "0.25");
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 1 / 5 = 0.2
    assert_eq!(s(div_ok(d("1"), d("5"))), "0.2");
}

#[test]
fn div_precision_capped_to_16_dp_matches_go() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // shopspring `Div` = `DivRound(…, 16)` (half-away-from-zero). Ipê-Rust
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // caps `decimal_div` to 16 dp with MidpointAwayFromZero, so non-terminating
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // quotients match the the reference exactly.
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 1/3 = 0.3333… → 17th digit 3 rounds down → sixteen 3s.
    assert_eq!(s(div_ok(d("1"), d("3"))), "0.3333333333333333");
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 2/3 = 0.6666… → 17th digit 6 rounds last digit up → …667.
    assert_eq!(s(div_ok(d("2"), d("3"))), "0.6666666666666667");
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 1/7 = 0.142857142857… → 16 dp half-away → …1429.
    assert_eq!(s(div_ok(d("1"), d("7"))), "0.1428571428571429");
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 10/3 = 3.3333… → sixteen 3s after the point.
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

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── Banker's rounding (Decimal.round (RoundBank)) ─────────────────

#[test]
fn bankers_rounding_ties_go_to_even() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // Golden: `Decimal_round 0 2.5` = "2" (nearest even = 2)
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_round(0, d("2.5"))),
        "2",
        "2.5 rounds to even 2 (banker's rounding)"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // `Decimal_round 0 3.5` = "4" (nearest even = 4)
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_round(0, d("3.5"))),
        "4",
        "3.5 rounds to even 4 (banker's rounding)"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 0.5 rounds to 0 (even)
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_round(0, d("0.5"))),
        "0"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 1.5 rounds to 2 (even)
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_round(0, d("1.5"))),
        "2"
    );
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── toStringFixed — StringFixed uses half-away-from-zero ─────────────────

#[test]
fn to_string_fixed_adds_trailing_zeros() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # `Decimal_toStringFixed 2 3` = "3.00"
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_to_string_fixed(
            2,
            ipe_runtime_rust::decimal::decimal_from_int(3)
        ),
        "3.00"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // `Decimal_toStringFixed 2 3.1` = "3.10"
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_to_string_fixed(2, d("3.1")),
        "3.10"
    );
}

#[test]
fn to_string_fixed_uses_half_away_from_zero_not_bankers() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // Golden: `Decimal_toStringFixed 2 (fromString "2.545")` = "2.55"
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // StringFixed calls Round which is half-away-from-zero.
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // The 3rd decimal is 5 (tie): half-away rounds UP to 2.55.
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // Banker's (MidpointNearestEven) would give "2.54" (4 is even → round down).
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_to_string_fixed(2, d("2.545")),
        "2.55",
        "toStringFixed: half-away-from-zero, not banker's rounding"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // `Decimal_toStringFixed 2 (fromString "2.535")` = "2.54"
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // half-away: .535 tie → 2.54 (rounds up).
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_to_string_fixed(2, d("2.535")),
        "2.54",
        "toStringFixed: 2.535 rounds to 2.54 (half-away-from-zero)"
    );
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── Percent helpers ──────────────────────────────────────────────────────────

#[test]
fn percent_of_basic() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # `Decimal_percentOf 10 100` = "10" (10% of 100)
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_percent_of(
            d("10"),
            d("100")
        )),
        "10"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # `Decimal_percentOf 20 100` = "20"
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
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // Golden: 8.875% of 99.99 = 8.8741125 → round 2 = "8.87"
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // (8.8741125: 3rd decimal is 4, rounds DOWN, same in both banker's and half-away)
    let pct = d("8.875");
    let price = d("99.99");
    let tax = ipe_runtime_rust::decimal::decimal_percent_of(pct, price);
    let rounded = ipe_runtime_rust::decimal::decimal_round(2, tax);
    assert_eq!(s(rounded), "8.87", "8.875% of 99.99 rounded to 2 dp");
}

#[test]
fn add_percent_and_sub_percent() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # `Decimal_addPercent 10 100` = "110"
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_add_percent(
            d("10"),
            d("100")
        )),
        "110"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # `Decimal_subPercent 10 100` = "90"
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_sub_percent(
            d("10"),
            d("100")
        )),
        "90"
    );
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── formatWith — StringFixed (half-away-from-zero) ────────

#[test]
fn format_with_us_locale() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // Golden: `Decimal_formatWith "," "." 2 1234567.891` = "1,234,567.89"
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
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // Golden: `Decimal_formatWith "." "," 2 1234567.891` = "1.234.567,89"
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
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // Golden: `Decimal_formatWith " " "," 0 1234567.891` = "1 234 568"
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 1234567.891 at 0 dp: .891 > .5 → rounds up → 1234568
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
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // Golden: `Decimal_formatWith "" "." 2 (fromString "2.545")` = "2.55"
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // formatWith calls StringFixed (half-away-from-zero).
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // Banker's (current Rust) would give "2.54".
    assert_eq!(
        ipe_runtime_rust::decimal::decimal_format_with(
            String::new(),
            ".".to_string(),
            2,
            d("2.545")
        ),
        "2.55",
        "formatWith: half-away-from-zero"
    );
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── Comparisons ──────────────────────────────────────────────────────────────

#[test]
fn comparisons_match_go() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # `Decimal_compare 5 7` = -1, `compare 7 5` = 1, `compare 5 5` = 0
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
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // Bool predicates
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

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── Sign predicates ──────────────────────────────────────────────────────────

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

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── Rounding modes ──────────────────────────────────────────────────────────

#[test]
fn round_half_up_matches_go() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # `Decimal_roundHalfUp 0 2.5` = "3" (half-away-from-zero)
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
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // truncate: toward zero
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_truncate(0, d("3.7"))),
        "3"
    );
    assert_eq!(
        s(ipe_runtime_rust::decimal::decimal_truncate(0, d("-3.7"))),
        "-3"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // floor: toward -∞
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_floor(d("3.1"))), "3");
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_floor(d("-3.1"))), "-4");
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // ceil: toward +∞
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_ceil(d("3.1"))), "4");
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_ceil(d("-3.1"))), "-3");
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── Constants ────────────────────────────────────────────────────────────────

#[test]
fn constants_match_go() {
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_zero()), "0");
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_one()), "1");
    assert_eq!(s(ipe_runtime_rust::decimal::decimal_one_hundred()), "100");
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── to_int / to_float ────────────────────────────────────────────────────────

#[test]
fn to_int_truncates() {
    assert_eq!(ipe_runtime_rust::decimal::decimal_to_int(d("3.9")), 3);
    assert_eq!(ipe_runtime_rust::decimal::decimal_to_int(d("-3.9")), -3);
}

#[test]
fn to_float_is_lossy_but_close() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // 3.14 is not PI — suppress the clippy lint that fires on the literal 3.14.
    #[allow(clippy::approx_constant)]
    let expected: f64 = 3.14;
    let f = ipe_runtime_rust::decimal::decimal_to_float(d("3.14"));
    assert!((f - expected).abs() < 1e-10);
}

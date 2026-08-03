//! Go≡Rust parity fixtures for Ipe.Money kernels.
//!
//! Every assertion mirrors the Go oracle in
//! `runtime-go/rt/money_kernel.go` and associated tests.
//!
//! Divergence note: `rust_decimal` is 96-bit fixed-precision vs
//! Go's shopspring arbitrary-precision. Within normal monetary
//! ranges (< 10^15 major units, ≤ 28 significant digits) values are
//! bit-identical. Non-terminating fractions (1/3 etc.) may diverge
//! beyond 28 digits — documented as a neutral platform difference.

// `money.rs` (and the `decimal.rs` it builds on) is behind the `decimal`
// feature, so this whole fixture is too.
#![cfg(feature = "decimal")]

use ipe_runtime_rust::*;

// ── decimal helper ─────────────────────────────────────────────────────────

fn d(s: &str) -> ipe_runtime_rust::decimal::Decimal {
    match decimal_from_string::<IpeError>(s.to_string()) {
        IpeResult::Ok(v) => v,
        IpeResult::Err(e) => panic!("bad decimal literal {s:?}: {e}"),
    }
}

// ── Currency properties ─────────────────────────────────────────────────────

#[test]
fn minor_units_match_go() {
    // Go oracle: currencyTable in money_kernel.go
    assert_eq!(money_minor_units("USD".to_string()), 2);
    assert_eq!(money_minor_units("EUR".to_string()), 2);
    assert_eq!(money_minor_units("GBP".to_string()), 2);
    assert_eq!(money_minor_units("JPY".to_string()), 0);
    assert_eq!(money_minor_units("KRW".to_string()), 0);
    assert_eq!(money_minor_units("BHD".to_string()), 3);
    assert_eq!(money_minor_units("BTC".to_string()), 8);
    // Crypto minor units mirror the Go oracle: ETH=18, USDT=6, USDC=6.
    assert_eq!(money_minor_units("ETH".to_string()), 18);
    assert_eq!(money_minor_units("USDT".to_string()), 6);
    assert_eq!(money_minor_units("USDC".to_string()), 6);
    // Unknown code falls back to 2 (Go: `lookupCurrency` fallback {Minor: 2})
    assert_eq!(money_minor_units("XYZ".to_string()), 2);
}

#[test]
fn symbol_matches_go() {
    // Go oracle: Money_symbol
    assert_eq!(money_symbol("USD".to_string()), "$");
    assert_eq!(money_symbol("EUR".to_string()), "€");
    assert_eq!(money_symbol("GBP".to_string()), "£");
    assert_eq!(money_symbol("JPY".to_string()), "¥");
    assert_eq!(money_symbol("INR".to_string()), "₹");
    assert_eq!(money_symbol("BTC".to_string()), "₿");
    // Unknown code: Go returns the code itself as the symbol
    assert_eq!(money_symbol("XYZ".to_string()), "XYZ");
    // Lowercase input normalised before lookup
    assert_eq!(money_symbol("usd".to_string()), "$");
}

#[test]
fn is_known_currency_matches_go() {
    // Go oracle: Money_isKnownCurrency
    assert!(money_is_known_currency("USD".to_string()));
    assert!(money_is_known_currency("EUR".to_string()));
    assert!(money_is_known_currency("JPY".to_string()));
    assert!(money_is_known_currency("BTC".to_string()));
    assert!(!money_is_known_currency("XYZ".to_string()));
    assert!(!money_is_known_currency("FAKE".to_string()));
}

// ── Formatting ──────────────────────────────────────────────────────────────

#[test]
fn format_matches_go() {
    // Go oracle: Money_format → info.Symbol + d.Abs().StringFixed(places)
    // USD: 2 dp, symbol "$"
    assert_eq!(money_format("USD".to_string(), d("12.34")), "$12.34");
    // JPY: 0 dp, symbol "¥"
    assert_eq!(money_format("JPY".to_string(), d("500")), "¥500");
    // Negative USD: leading "-" before symbol
    assert_eq!(money_format("USD".to_string(), d("-12.34")), "-$12.34");
    // BHD: 3 dp, symbol "ب.د"
    assert_eq!(money_format("BHD".to_string(), d("1.234")), "ب.د1.234");
    // Lowercase code normalised
    assert_eq!(money_format("usd".to_string(), d("5.00")), "$5.00");
    // Unknown code: symbol = code, fallback 2 dp
    assert_eq!(money_format("XYZ".to_string(), d("3.14")), "XYZ3.14");
}

#[test]
fn format_with_code_matches_go() {
    // Go oracle: Money_formatWithCode → "<fixed> <UPPER_CODE>"
    assert_eq!(
        money_format_with_code("USD".to_string(), d("12.34")),
        "12.34 USD"
    );
    // Lowercase input normalised to upper
    assert_eq!(
        money_format_with_code("usd".to_string(), d("12.34")),
        "12.34 USD"
    );
    // BHD 3 dp
    assert_eq!(
        money_format_with_code("BHD".to_string(), d("1.234")),
        "1.234 BHD"
    );
    // JPY 0 dp
    assert_eq!(
        money_format_with_code("JPY".to_string(), d("500")),
        "500 JPY"
    );
}

// ── GOLDEN: Money.allocate of 100 into 3 parts ─────────────────────────────
//
// Go oracle (money_kernel_test.go TestMoney_Allocate_SumExact):
//   allocate(places=2, parts=3, amount=100) = [33.34, 33.33, 33.33]
//   sum = 100.00 exactly (fair split — first slot carries the extra cent)

#[test]
fn allocate_100_into_3_parts_golden() {
    let parts = money_allocate(2, 3, d("100"));
    assert_eq!(parts.len(), 3, "allocate(2, 3, 100) must return 3 parts");
    // First slot carries the extra cent.
    assert_eq!(
        decimal_to_string(parts[0]),
        "33.34",
        "first part must be 33.34"
    );
    assert_eq!(
        decimal_to_string(parts[1]),
        "33.33",
        "second part must be 33.33"
    );
    assert_eq!(
        decimal_to_string(parts[2]),
        "33.33",
        "third part must be 33.33"
    );
    // Sum must equal input exactly — no rounding drift.
    let sum = parts.iter().fold(d("0"), |acc, x| decimal_add(acc, *x));
    // normalize() strips trailing zeros: 100.00 → "100"
    assert_eq!(
        decimal_to_string(sum),
        "100",
        "sum of parts must equal 100 exactly"
    );
}

#[test]
fn allocate_sum_exact_for_various_splits() {
    // Go parity: any allocation must sum to the original amount.
    let cases: &[(&str, i64, i64)] = &[
        ("100", 2, 3),  // 3-way split of 100
        ("1", 2, 3),    // 1.00 into 3 → 0.34, 0.33, 0.33
        ("10", 2, 4),   // 4-way: 2.50, 2.50, 2.50, 2.50
        ("7.77", 2, 3), // 7.77 into 3 → 2.59, 2.59, 2.59
    ];
    for &(amt, places, n) in cases {
        let parts = money_allocate(places, n, d(amt));
        assert_eq!(
            i64::try_from(parts.len()).expect("parts.len fits i64"),
            n,
            "allocate({amt}, {n}) must return {n} parts"
        );
        let sum = parts.iter().fold(d("0"), |acc, x| decimal_add(acc, *x));
        // Compare via to_string_fixed to avoid normalize() stripping zeros that
        // mismatch raw string literals — use the exact same fixed format the
        // allocate implementation normalises to for the full-sum check.
        assert_eq!(
            decimal_to_string_fixed(places, sum),
            decimal_to_string_fixed(places, d(amt)),
            "sum of allocate({amt}, {n}) must equal {amt}"
        );
    }
}

#[test]
fn allocate_zero_and_negative_parts_return_empty() {
    // Go parity: parts ≤ 0 → empty list
    assert!(money_allocate(2, 0, d("100")).is_empty());
    assert!(money_allocate(2, -1, d("100")).is_empty());
}

// ── GOLDEN: Money add + format ───────────────────────────────────────────────
//
// Ipê: `Money.add (Money.fromMajor USD 10) (Money.fromMajor USD 5.50)
//        |> Money.format`
// Go parity: Dec.add(10.00, 5.50) = 15.50 → format("USD", 15.50) = "$15.50"

#[test]
fn money_add_and_format_golden() {
    // Simulate Money.add via the underlying Decimal kernel.
    let a = d("10.00");
    let b = d("5.50");
    let total = decimal_add(a, b);
    // Go oracle: format("USD", 15.50) = "$15.50"
    assert_eq!(money_format("USD".to_string(), total), "$15.50");

    // Integer add: 10 + 5 = 15 → "$15.00" (2 dp for USD)
    let total2 = decimal_add(d("10"), d("5"));
    assert_eq!(money_format("USD".to_string(), total2), "$15.00");

    // JPY (0 dp): 1000 + 500 = 1500 → "¥1500"
    let total3 = decimal_add(d("1000"), d("500"));
    assert_eq!(money_format("JPY".to_string(), total3), "¥1500");
}

// ── FX rate registry ────────────────────────────────────────────────────────
//
// The rate registry is process-global; tests that mutate it must be
// serialised to avoid flaky cross-test contamination.

fn rate_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn fx_rate_round_trip_matches_go() {
    let _g = rate_test_lock();
    let _: IpeResult<IpeError, ()> = money_clear_rates(());

    // setRate: USD→EUR = 0.92
    let set: IpeResult<IpeError, ()> =
        money_set_rate("USD".to_string(), "EUR".to_string(), d("0.92"));
    assert!(set.is_ok(), "setRate must succeed for a positive rate");

    // hasRate: USD→EUR and auto-inverse EUR→USD both true
    assert!(money_has_rate("USD".to_string(), "EUR".to_string()));
    assert!(money_has_rate("EUR".to_string(), "USD".to_string()));
    // Unregistered pair → false
    assert!(!money_has_rate("USD".to_string(), "GBP".to_string()));

    // getRate: USD→EUR = 0.92
    let got: IpeResult<IpeError, ipe_runtime_rust::decimal::Decimal> =
        money_get_rate("USD".to_string(), "EUR".to_string());
    assert!(got.is_ok());
    if let IpeResult::Ok(v) = got {
        assert_eq!(decimal_to_string(v), "0.92");
    }

    // Same-currency always returns 1 (Go: `if from == to { return 1, true }`)
    let same: IpeResult<IpeError, ipe_runtime_rust::decimal::Decimal> =
        money_get_rate("USD".to_string(), "USD".to_string());
    assert!(same.is_ok());
    if let IpeResult::Ok(v) = same {
        assert_eq!(decimal_to_string(v), "1");
    }

    // Unregistered pair → Err
    let missing: IpeResult<IpeError, ipe_runtime_rust::decimal::Decimal> =
        money_get_rate("USD".to_string(), "GBP".to_string());
    assert!(missing.is_err(), "getRate for unregistered pair must Err");

    let _: IpeResult<IpeError, ()> = money_clear_rates(());
}

#[test]
fn fx_auto_inverse_rate_capped_to_16_dp_matches_go() {
    let _g = rate_test_lock();
    let _: IpeResult<IpeError, ()> = money_clear_rates(());

    // setRate USD→EUR = 3 ⇒ auto-inverse EUR→USD = 1/3. Go derives the inverse
    // with shopspring's `Div` (DivisionPrecision = 16, half-away-from-zero), so
    // getRate of the inverse pair is sixteen 3s — Ipê-Rust caps identically.
    let set: IpeResult<IpeError, ()> = money_set_rate("USD".to_string(), "EUR".to_string(), d("3"));
    assert!(set.is_ok(), "setRate must succeed for a positive rate");

    let fwd: IpeResult<IpeError, ipe_runtime_rust::decimal::Decimal> =
        money_get_rate("USD".to_string(), "EUR".to_string());
    assert!(fwd.is_ok());
    if let IpeResult::Ok(v) = fwd {
        assert_eq!(decimal_to_string(v), "3");
    }

    let inv: IpeResult<IpeError, ipe_runtime_rust::decimal::Decimal> =
        money_get_rate("EUR".to_string(), "USD".to_string());
    assert!(inv.is_ok());
    if let IpeResult::Ok(v) = inv {
        assert_eq!(decimal_to_string(v), "0.3333333333333333");
    }

    let _: IpeResult<IpeError, ()> = money_clear_rates(());
}

#[test]
fn set_rate_zero_and_negative_rejected_matches_go() {
    let _g = rate_test_lock();
    let _: IpeResult<IpeError, ()> = money_clear_rates(());
    // Go oracle: "rate must be positive" → Err on zero or negative
    let r: IpeResult<IpeError, ()> = money_set_rate("USD".to_string(), "EUR".to_string(), d("0"));
    assert!(r.is_err(), "setRate(0) must fail");
    let r2: IpeResult<IpeError, ()> =
        money_set_rate("USD".to_string(), "EUR".to_string(), d("-0.5"));
    assert!(r2.is_err(), "setRate(-0.5) must fail");
    let _: IpeResult<IpeError, ()> = money_clear_rates(());
}

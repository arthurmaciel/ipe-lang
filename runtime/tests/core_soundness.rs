//! Soundness coverage for `sky_runtime::core` — the coercion primitives that
//! every generated FFI wrapper and kernel call routes through. The existential
//! guarantee ("no runtime panic from well-typed Sky code") lives or dies here,
//! so each test asserts BOTH the happy path AND that the failure path returns
//! `SkyResult::Err` rather than panicking / wrapping / indexing out of bounds.

use proptest::prelude::*;
use sky_runtime_rust::*;

// ── byte <-> List Int round-trips ──────────────────────────────────────────

#[test]
fn to_u8_vec_from_u8_slice_roundtrip_in_range() {
    let bytes: Vec<u8> = vec![0, 1, 65, 127, 200, 255];
    let as_ints = sky_runtime::core::from_u8_slice(&bytes);
    assert_eq!(as_ints, vec![0i64, 1, 65, 127, 200, 255]);
    let back = sky_runtime::core::to_u8_vec(&as_ints);
    assert_eq!(back, bytes);
}

#[test]
fn to_u8_vec_truncates_out_of_range_without_panic() {
    // `x as u8` is defined (wrapping) for any i64 — never a panic.
    assert_eq!(sky_runtime::core::to_u8_vec(&[256]), vec![0u8]);
    assert_eq!(sky_runtime::core::to_u8_vec(&[257]), vec![1u8]);
    assert_eq!(sky_runtime::core::to_u8_vec(&[-1]), vec![255u8]);
    assert_eq!(
        sky_runtime::core::to_u8_vec(&[i64::MAX, i64::MIN]),
        vec![255u8, 0u8]
    );
    assert_eq!(sky_runtime::core::to_u8_vec(&[]), Vec::<u8>::new());
}

// ── fixed-size array coercion: length mismatch MUST be Err, never panic ─────

#[test]
fn to_u8_array_exact_length_ok() {
    let r = sky_runtime::core::to_u8_array::<SkyError, 3>(&[1, 2, 3]);
    assert!(r.is_ok());
    assert_eq!(r.with_default([0, 0, 0]), [1u8, 2, 3]);
}

#[test]
fn to_u8_array_too_short_is_err_not_panic() {
    let r = sky_runtime::core::to_u8_array::<SkyError, 3>(&[1, 2]);
    assert!(r.is_err(), "short input must be Err, never a panic");
}

#[test]
fn to_u8_array_too_long_is_err_not_panic() {
    let r = sky_runtime::core::to_u8_array::<SkyError, 3>(&[1, 2, 3, 4, 5]);
    assert!(r.is_err(), "long input must be Err, never a panic");
}

#[test]
fn to_u8_array_zero_length_ok_on_empty_err_on_nonempty() {
    assert!(sky_runtime::core::to_u8_array::<SkyError, 0>(&[]).is_ok());
    assert!(sky_runtime::core::to_u8_array::<SkyError, 0>(&[1]).is_err());
}

#[test]
fn to_array_generic_exact_ok_mismatch_err() {
    let ok =
        sky_runtime::core::to_array::<SkyError, String, 2>(&["a".to_string(), "b".to_string()]);
    assert!(ok.is_ok());
    assert_eq!(
        ok.with_default([String::new(), String::new()]),
        ["a".to_string(), "b".to_string()]
    );

    let short = sky_runtime::core::to_array::<SkyError, String, 2>(&["a".to_string()]);
    assert!(short.is_err());
    let long = sky_runtime::core::to_array::<SkyError, i64, 2>(&[1, 2, 3]);
    assert!(long.is_err());
}

// ── SkyMaybe combinators — both variants ───────────────────────────────────

#[test]
fn sky_maybe_map_and_then_with_default() {
    let just = SkyMaybe::Just(10i64);
    let nothing: SkyMaybe<i64> = SkyMaybe::Nothing;

    assert!(just.is_just() && !just.is_nothing());
    assert!(nothing.is_nothing() && !nothing.is_just());

    assert_eq!(
        sky_runtime::core::sky_maybe_map(SkyMaybe::Just(10i64), |x| x + 1),
        SkyMaybe::Just(11)
    );
    assert_eq!(
        sky_runtime::core::sky_maybe_map(SkyMaybe::Nothing, |x: i64| x + 1),
        SkyMaybe::Nothing
    );

    assert_eq!(
        sky_runtime::core::sky_maybe_and_then(SkyMaybe::Just(10i64), |x| SkyMaybe::Just(x * 2)),
        SkyMaybe::Just(20)
    );
    assert_eq!(
        sky_runtime::core::sky_maybe_and_then(SkyMaybe::Just(10i64), |_: i64| {
            SkyMaybe::<i64>::Nothing
        }),
        SkyMaybe::Nothing
    );
    assert_eq!(
        sky_runtime::core::sky_maybe_and_then(SkyMaybe::Nothing, |x: i64| SkyMaybe::Just(x)),
        SkyMaybe::Nothing
    );

    assert_eq!(SkyMaybe::Just(7i64).with_default(0), 7);
    assert_eq!(SkyMaybe::<i64>::Nothing.with_default(0), 0);
    assert_eq!(
        sky_runtime::core::maybe_with_default(99i64, SkyMaybe::Nothing),
        99
    );
    assert_eq!(
        sky_runtime::core::maybe_with_default(99i64, SkyMaybe::Just(1)),
        1
    );
}

// ── SkyResult combinators — both variants ──────────────────────────────────

#[test]
fn sky_result_map_and_then_with_default() {
    let ok: SkyResult<SkyError, i64> = SkyResult::Ok(10);
    let err: SkyResult<SkyError, i64> = SkyResult::Err(str_err("boom"));

    assert!(ok.is_ok() && !ok.is_err());
    assert!(err.is_err() && !err.is_ok());

    let mapped = sky_runtime::core::sky_result_map(SkyResult::<SkyError, i64>::Ok(10), |x| x + 5);
    assert_eq!(mapped.with_default(0), 15);
    let mapped_err =
        sky_runtime::core::sky_result_map(SkyResult::<SkyError, i64>::Err(str_err("e")), |x| x + 5);
    assert!(mapped_err.is_err());

    let chained = sky_runtime::core::sky_result_and_then(SkyResult::<SkyError, i64>::Ok(10), |x| {
        SkyResult::Ok(x * 3)
    });
    assert_eq!(chained.with_default(0), 30);
    let chained_to_err =
        sky_runtime::core::sky_result_and_then(SkyResult::<SkyError, i64>::Ok(10), |_| {
            SkyResult::<SkyError, i64>::Err(str_err("downstream"))
        });
    assert!(chained_to_err.is_err());
    // and_then on Err must NOT run the function (short-circuit).
    let not_run = sky_runtime::core::sky_result_and_then(
        SkyResult::<SkyError, i64>::Err(str_err("upstream")),
        |_: i64| -> SkyResult<SkyError, i64> { panic!("must not be called on Err") },
    );
    assert!(not_run.is_err());

    assert_eq!(
        sky_runtime::core::result_with_default(0i64, SkyResult::<SkyError, i64>::Ok(42)),
        42
    );
    assert_eq!(
        sky_runtime::core::result_with_default(0i64, SkyResult::<SkyError, i64>::Err(str_err("x"))),
        0
    );
}

// ── result_traverse: all-ok collects; first Err short-circuits ─────────────

#[test]
fn result_traverse_all_ok_collects_in_order() {
    let r = sky_runtime::core::result_traverse::<i64, i64, SkyError>(
        |x| SkyResult::Ok(x * 10),
        vec![1, 2, 3],
    );
    assert_eq!(r.with_default(vec![]), vec![10, 20, 30]);
}

#[test]
fn result_traverse_short_circuits_on_first_err() {
    let r = sky_runtime::core::result_traverse::<i64, i64, SkyError>(
        |x| {
            if x == 2 {
                SkyResult::Err(str_err("two"))
            } else {
                SkyResult::Ok(x)
            }
        },
        vec![1, 2, 3],
    );
    assert!(r.is_err());
}

#[test]
fn result_traverse_empty_is_ok_empty() {
    let r = sky_runtime::core::result_traverse::<i64, i64, SkyError>(SkyResult::Ok, vec![]);
    assert_eq!(r.with_default(vec![99]), Vec::<i64>::new());
}

// ── sky_maybe_to_option: FFI Option-param bridge (Just->Some, Nothing->None) ─

#[test]
fn sky_maybe_to_option_both_variants() {
    assert_eq!(
        sky_runtime::core::sky_maybe_to_option(SkyMaybe::Just(5i64)),
        Some(5)
    );
    assert_eq!(
        sky_runtime::core::sky_maybe_to_option(SkyMaybe::<i64>::Nothing),
        None
    );
    // The .as_deref() path the codegen uses for Option<&str> is sound.
    let just = sky_runtime::core::sky_maybe_to_option(SkyMaybe::Just("hi".to_string()));
    assert_eq!(just.as_deref(), Some("hi"));
    let none: Option<String> = sky_runtime::core::sky_maybe_to_option(SkyMaybe::Nothing);
    assert_eq!(none.as_deref(), None);
    // The numeric-narrowing path (.map(|x| x as u16)).
    let n = sky_runtime::core::sky_maybe_to_option(SkyMaybe::Just(70000i64)).map(|x| x as u16);
    assert_eq!(n, Some(70000i64 as u16)); // defined wrapping cast, no panic
}

// ── property: byte/array coercion never panics for ANY input ───────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn prop_to_u8_array_never_panics(xs in proptest::collection::vec(any::<i64>(), 0..32)) {
        // Whatever the length, the result is total: Ok iff len==4, else Err.
        let r = sky_runtime::core::to_u8_array::<SkyError, 4>(&xs);
        prop_assert_eq!(r.is_ok(), xs.len() == 4);
    }

    #[test]
    fn prop_to_u8_vec_from_slice_roundtrip(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        let ints = sky_runtime::core::from_u8_slice(&bytes);
        let back = sky_runtime::core::to_u8_vec(&ints);
        prop_assert_eq!(back, bytes);
    }

    #[test]
    fn prop_result_traverse_preserves_length_when_all_ok(xs in proptest::collection::vec(any::<i64>(), 0..50)) {
        let n = xs.len();
        let r = sky_runtime::core::result_traverse::<i64, i64, SkyError>(SkyResult::Ok, xs);
        prop_assert_eq!(r.with_default(vec![]).len(), n);
    }
}

//! Property-based tests for the Sky Rust runtime.

use proptest::prelude::*;
use sky_runtime_rust::*;

// ═══════════════════════════════════════════════════════════════════
// Core types — direct from the runtime
// ═══════════════════════════════════════════════════════════════════

proptest! {
    #[test]
    fn result_with_default_ok(def: i64, x: i64) {
        let r: SkyResult<&str, i64> = SkyResult::Ok(x);
        prop_assert_eq!(result_with_default(def, r), x);
    }

    #[test]
    fn result_with_default_err(def: i64, s: String) {
        let r: SkyResult<String, i64> = SkyResult::Err(s);
        prop_assert_eq!(result_with_default(def, r), def);
    }

    #[test]
    fn result_map_id(x: i64) {
        let r: SkyResult<&str, i64> = SkyResult::Ok(x);
        let mapped = sky_result_map(r, |v| v);
        prop_assert_eq!(mapped, SkyResult::Ok(x));
    }

    #[test]
    fn maybe_map_id(x: i64) {
        let m: SkyMaybe<i64> = SkyMaybe::Just(x);
        let mapped = sky_maybe_map(m, |v| v);
        prop_assert_eq!(mapped, SkyMaybe::Just(x));
    }
}

// ═══════════════════════════════════════════════════════════════════
// String operations
// ═══════════════════════════════════════════════════════════════════

proptest! {
    #[test]
    fn string_length_appends(a: String, b: String) {
        let ab = string_append(a.clone(), b.clone());
        prop_assert_eq!(string_length(ab), string_length(a) + string_length(b));
    }

    #[test]
    fn string_reverse_involution(s: String) {
        let rev = string_reverse(s.clone());
        prop_assert_eq!(string_reverse(rev), s);
    }

    #[test]
    fn string_trim_noop_on_plain(s: String) {
        let plain: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        let trimmed = string_trim(plain.clone());
        prop_assert_eq!(trimmed, plain);
    }

    #[test]
    fn string_to_int_roundtrip(n: i64) {
        let s = string_from_int(n);
        let parsed = string_to_int(s);
        prop_assert_eq!(parsed, SkyMaybe::Just(n));
    }
}

// ═══════════════════════════════════════════════════════════════════
// Task combinators (require tokio feature)
// ═══════════════════════════════════════════════════════════════════

#[cfg(feature = "tokio")]
mod task_tests {
    use sky_runtime_rust::*;

    fn run<A: Send + 'static>(task: SkyTask<SkyError, A>) -> SkyResult<SkyError, A> {
        task_run(task)
    }

    fn mk_task<A: Send + 'static>(a: A) -> SkyTask<SkyError, A> {
        sky_runtime::task::task_succeed::<SkyError, A>(a)
    }

    #[test]
    fn task_succeed_ok() {
        assert_eq!(run(mk_task(42)), SkyResult::Ok(42));
    }

    #[test]
    fn task_map_ok() {
        let f = |x: i64| x + 1;
        assert_eq!(run(task_map(f, mk_task(41))), SkyResult::Ok(42));
    }

    #[test]
    fn task_and_then_ok() {
        let f = |x: i64| mk_task(x * 2);
        assert_eq!(run(task_and_then(mk_task(21), f)), SkyResult::Ok(42));
    }

    #[test]
    fn task_fail_is_err() {
        let err: SkyError = str_err("boom");
        let t: SkyTask<SkyError, i64> = sky_runtime::task::task_fail::<SkyError, i64>(err);
        assert!(run(t).is_err());
    }

    // `System.getenv : String -> Task Error String`. Regression guard: it MUST
    // return a `SkyTask` (not a bare `String`), or it fails to type-check in any
    // `Task.andThen`/`Task.run` position — and an unset var MUST short-circuit
    // with `Err` (mirroring Go's `System_getenv` ErrNotFound), not `Ok("")`,
    // so a chained Task fails identically on both backends.
    #[test]
    fn system_getenv_present_is_ok() {
        std::env::set_var("SKY_TEST_GETENV_PRESENT", "hello");
        let t: SkyTask<SkyError, String> =
            system_getenv::<SkyError>("SKY_TEST_GETENV_PRESENT".to_string());
        assert_eq!(run(t), SkyResult::Ok("hello".to_string()));
    }

    #[test]
    fn system_getenv_unset_is_err() {
        std::env::remove_var("SKY_TEST_GETENV_UNSET_XYZ_42");
        let t: SkyTask<SkyError, String> =
            system_getenv::<SkyError>("SKY_TEST_GETENV_UNSET_XYZ_42".to_string());
        assert!(run(t).is_err());
    }

    // System.getenvInt / getenvBool / getArg — Go-parity semantics (unset → Err
    // NotFound; non-int / non-bool → Err Ffi; getArg indexes the FULL arg vector
    // and is out-of-range → Ok Nothing, never Err).
    #[test]
    fn system_getenv_int_ok_and_errs() {
        std::env::set_var("SKY_TEST_INT_OK", "42");
        std::env::set_var("SKY_TEST_INT_BAD", "abc");
        std::env::remove_var("SKY_TEST_INT_UNSET");
        assert_eq!(
            run(system_getenv_int::<SkyError>("SKY_TEST_INT_OK".to_string())),
            SkyResult::Ok(42)
        );
        assert!(
            run(system_getenv_int::<SkyError>(
                "SKY_TEST_INT_BAD".to_string()
            ))
            .is_err()
        );
        assert!(
            run(system_getenv_int::<SkyError>(
                "SKY_TEST_INT_UNSET".to_string()
            ))
            .is_err()
        );
    }

    #[test]
    fn system_getenv_bool_truthy_falsy_unset() {
        std::env::set_var("SKY_TEST_BOOL_T", "yes");
        std::env::set_var("SKY_TEST_BOOL_F", "0");
        std::env::set_var("SKY_TEST_BOOL_BAD", "maybe");
        std::env::remove_var("SKY_TEST_BOOL_UNSET");
        assert_eq!(
            run(system_getenv_bool::<SkyError>(
                "SKY_TEST_BOOL_T".to_string()
            )),
            SkyResult::Ok(true)
        );
        assert_eq!(
            run(system_getenv_bool::<SkyError>(
                "SKY_TEST_BOOL_F".to_string()
            )),
            SkyResult::Ok(false)
        );
        assert!(
            run(system_getenv_bool::<SkyError>(
                "SKY_TEST_BOOL_BAD".to_string()
            ))
            .is_err()
        );
        assert!(
            run(system_getenv_bool::<SkyError>(
                "SKY_TEST_BOOL_UNSET".to_string()
            ))
            .is_err()
        );
    }

    #[test]
    fn system_get_arg_in_and_out_of_range() {
        // index 0 is the program name (the test binary) — always present.
        assert!(matches!(
            run(system_get_arg::<SkyError>(0)),
            SkyResult::Ok(SkyMaybe::Just(_))
        ));
        assert_eq!(
            run(system_get_arg::<SkyError>(9999)),
            SkyResult::Ok(SkyMaybe::Nothing)
        );
        assert_eq!(
            run(system_get_arg::<SkyError>(-1)),
            SkyResult::Ok(SkyMaybe::Nothing)
        );
    }
}

// ═══════════════════════════════════════════════════════════════════
// JSON encode/decode (require json feature)
// ═══════════════════════════════════════════════════════════════════

#[cfg(feature = "json")]
mod json_tests {
    use sky_runtime_rust::*;

    #[test]
    fn json_int_roundtrip() {
        let json = sky_runtime::json::json_enc_int(42);
        let encoded = sky_runtime::json::json_enc_encode(0, json);
        let decoder: Decoder<SkyError, i64> = sky_runtime::json::json_decode_int();
        let decoded = sky_runtime::json::decode_from_json_string(decoder, encoded);
        assert_eq!(decoded, SkyResult::Ok(42));
    }

    #[test]
    fn json_string_roundtrip() {
        let json = sky_runtime::json::json_enc_string("hello".to_string());
        let encoded = sky_runtime::json::json_enc_encode(0, json);
        let decoder: Decoder<SkyError, String> = sky_runtime::json::json_decode_string();
        let decoded = sky_runtime::json::decode_from_json_string(decoder, encoded);
        assert_eq!(decoded, SkyResult::Ok("hello".to_string()));
    }

    #[test]
    fn json_bool_roundtrip() {
        let json = sky_runtime::json::json_enc_bool(true);
        let encoded = sky_runtime::json::json_enc_encode(0, json);
        let decoder: Decoder<SkyError, bool> = sky_runtime::json::json_decode_bool();
        let decoded = sky_runtime::json::decode_from_json_string(decoder, encoded);
        assert_eq!(decoded, SkyResult::Ok(true));
    }
}

// ═══════════════════════════════════════════════════════════════════
// Standalone tests (not property-based, but useful signals)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn string_is_empty_true() {
    assert!(string_is_empty("".to_string()));
}

#[test]
fn string_to_int_fails_on_bad_input() {
    assert_eq!(string_to_int("not a number".to_string()), SkyMaybe::Nothing);
}

#[test]
fn string_to_lower_upper_consistent() {
    let s = "Hello World".to_string();
    let upper = string_to_upper(s.clone());
    let lower_upper = string_to_lower(upper);
    let lower = string_to_lower(s);
    assert_eq!(lower_upper, lower);
}

// ═══════════════════════════════════════════════════════════════════
// Byte-sequence FFI coercion helpers
// ═══════════════════════════════════════════════════════════════════

proptest! {
    // to_u8_vec then widen back is identity on in-range bytes.
    #[test]
    fn byte_vec_roundtrip(xs in proptest::collection::vec(0u8..=255, 0..64)) {
        let as_i64: Vec<i64> = xs.iter().map(|&b| b as i64).collect();
        prop_assert_eq!(to_u8_vec(&as_i64), xs.clone());
        prop_assert_eq!(from_u8_slice(&xs), as_i64);
    }

    // to_u8_array succeeds iff the input length matches N; never panics.
    #[test]
    fn to_u8_array_len_checked(xs in proptest::collection::vec(0i64..256, 0..40)) {
        let r: SkyResult<String, [u8; 16]> = to_u8_array(&xs);
        if xs.len() == 16 {
            prop_assert!(r.is_ok());
        } else {
            prop_assert!(r.is_err());
        }
    }

    /// `to_array` succeeds exactly when input length matches N, never panics.
    #[test]
    fn to_array_len_checked(xs in proptest::collection::vec(proptest::prelude::any::<i64>(), 0..16usize)) {
        const N: usize = 8;
        let result: SkyResult<SkyError, [i64; N]> = to_array::<SkyError, i64, N>(&xs);
        if xs.len() == N {
            prop_assert!(matches!(result, SkyResult::Ok(_)));
            if let SkyResult::Ok(arr) = result {
                for i in 0..N {
                    prop_assert_eq!(arr[i], xs[i]);
                }
            }
        } else {
            prop_assert!(matches!(result, SkyResult::Err(_)));
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Std.Email SMTP transport (require email feature) — deterministic error
// paths. The positive path (delivery to a local SMTP catcher) is verified
// out-of-band; here we lock in that the lettre-backed send_smtp is TOTAL:
// bad config / bad address surface a clean Err, never a panic.
// ═══════════════════════════════════════════════════════════════════

#[cfg(feature = "email")]
mod email_smtp_tests {
    use sky_runtime_rust::*;

    fn msg(from: &str) -> EmailMessage {
        EmailMessage {
            from: from.to_string(),
            to: vec!["rcpt@example.com".to_string()],
            cc: vec![],
            bcc: vec![],
            subject: "s".to_string(),
            textBody: "b".to_string(),
            htmlBody: String::new(),
            attachments: vec![],
            replyTo: String::new(),
        }
    }

    #[test]
    fn smtp_empty_host_is_err() {
        let cfg = SmtpConfig {
            host: String::new(),
            port: 0,
            user: String::new(),
            pass: String::new(),
        };
        let t = email_send::<SkyError>(EmailProvider::Smtp(cfg), msg("a@b.com"));
        assert!(task_run(t).is_err());
    }

    #[test]
    fn smtp_bad_from_address_is_err() {
        // non-empty (passes email_send's empty-from guard) but not an RFC-5322
        // mailbox → send_smtp's parse returns Err, never panics.
        let cfg = SmtpConfig {
            host: "127.0.0.1".to_string(),
            port: 2599,
            user: String::new(),
            pass: String::new(),
        };
        let t = email_send::<SkyError>(EmailProvider::Smtp(cfg), msg("not-an-email"));
        assert!(task_run(t).is_err());
    }
}

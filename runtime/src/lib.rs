//! Sky Runtime for Rust — single source of truth for Sky→Rust codegen.
//!
//! The `sky_runtime` module is copied into generated projects at build time.
//! Builder.hs emits `mod sky_runtime; use sky_runtime::*;` instead of
//! duplicating these definitions inline.
//!
//! This `lib.rs` is for standalone testing (`cargo test`).
//!
//! # No-runtime-errors gate (panic vectors)
//! Beyond the `unwrap_used`/`expect_used` deny in `Cargo.toml [lints.clippy]`,
//! the panic-prone `indexing_slicing` / `panic` / `unreachable` lints are denied
//! on NON-test library code via the `cfg_attr(not(test), …)` below. Test code
//! (lib `#[cfg(test)]` modules — skipped when `cfg(test)` is on — and the
//! separate `tests/` integration crates, which don't inherit this attribute)
//! uses these freely. The only `#[allow(clippy::panic)]` in the crate are the 2
//! `ffi_polyfills` dynamic-dispatch fallbacks (unconstrained generic `T` return
//! → no total value); see README "Soundness attention points".

#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unreachable,
        // Promoted from the quality-audit advisory set to a HARD deny: these are
        // all panic vectors a well-typed Sky program must never reach. `cargo
        // clippy` now FAILS on any of them in non-test runtime code, so risky
        // code cannot be merged (CI security-audit gate + local clippy enforce
        // it). See `## Settled rules` in CLAUDE.md.
        clippy::todo,
        clippy::unimplemented,
        clippy::panic_in_result_fn
    )
)]

pub mod sky_runtime;
pub use sky_runtime::*;

// ============================================================================
// Tests (re-exported for `cargo test` coverage)
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    // SkyResult tests
    #[test]
    fn result_ok() {
        let r: SkyResult<&str, i64> = SkyResult::Ok(42);
        assert!(r.is_ok());
        assert_eq!(r.with_default(0), 42);
    }

    #[test]
    fn result_err() {
        let r: SkyResult<&str, i64> = SkyResult::Err("error");
        assert!(r.is_err());
        assert_eq!(r.with_default(0), 0);
    }

    #[test]
    fn result_map_ok() {
        let r: SkyResult<&str, i64> = SkyResult::Ok(5);
        let mapped = sky_result_map(r, |x| x * 2);
        assert_eq!(mapped.with_default(0), 10);
    }

    #[test]
    fn result_map_err() {
        let r: SkyResult<&str, i64> = SkyResult::Err("error");
        let mapped: SkyResult<&str, i64> = sky_result_map(r, |x| x * 2);
        assert!(mapped.is_err());
    }

    #[test]
    fn result_and_then_ok() {
        let r: SkyResult<&str, i64> = SkyResult::Ok(5);
        let chained = sky_result_and_then(r, |x| SkyResult::Ok(x * 2));
        assert_eq!(chained.with_default(0), 10);
    }

    #[test]
    fn result_and_then_err() {
        let r: SkyResult<&str, i64> = SkyResult::Err("e");
        let chained = sky_result_and_then(r, |x: i64| SkyResult::Ok(x * 2));
        assert!(chained.is_err());
    }

    // SkyMaybe tests
    #[test]
    fn maybe_just() {
        let m: SkyMaybe<i64> = SkyMaybe::Just(42);
        assert!(m.is_just());
        assert_eq!(m.with_default(0), 42);
    }

    #[test]
    fn maybe_nothing() {
        let m: SkyMaybe<i64> = SkyMaybe::Nothing;
        assert!(m.is_nothing());
        assert_eq!(m.with_default(99), 99);
    }

    #[test]
    fn maybe_map_just() {
        let m: SkyMaybe<i64> = SkyMaybe::Just(5);
        let mapped = sky_maybe_map(m, |x| x * 2);
        assert_eq!(mapped.with_default(0), 10);
    }

    #[test]
    fn maybe_and_then_just() {
        let m: SkyMaybe<i64> = SkyMaybe::Just(5);
        let chained = sky_maybe_and_then(m, |x| SkyMaybe::Just(x * 2));
        assert_eq!(chained.with_default(0), 10);
    }

    // List tests — the live, codegen-emitted kernels (list.rs)
    #[test]
    fn list_filter_keeps_matching() {
        assert_eq!(
            list_filter(|x: i64| x % 2 == 0, vec![1, 2, 3, 4]),
            vec![2, 4]
        );
    }

    #[test]
    fn list_foldl_sums() {
        assert_eq!(list_foldl(|x, acc| acc + x, 0, vec![1, 2, 3]), 6);
    }

    #[test]
    fn list_range_inclusive() {
        assert_eq!(list_range(1, 3), vec![1, 2, 3]);
    }

    #[test]
    fn list_member_finds() {
        assert!(list_member(2, vec![1, 2, 3]));
        assert!(!list_member(9, vec![1, 2, 3]));
    }

    #[test]
    fn list_cons_prepends() {
        assert_eq!(sky_list_cons(0, vec![1, 2]), vec![0, 1, 2]);
    }

    // String tests — the live kernels (string.rs)
    #[test]
    fn string_ops() {
        assert_eq!(string_append("a".into(), "b".into()), "ab");
        assert_eq!(string_length("hello".into()), 5);
        assert!(string_is_empty("".into()));
    }

    #[test]
    fn string_to_int_ok() {
        assert_eq!(string_to_int("42".into()), SkyMaybe::Just(42));
    }

    #[test]
    fn string_to_int_fail() {
        assert_eq!(string_to_int("abc".into()), SkyMaybe::Nothing);
    }

    // Result helpers
    #[test]
    fn result_with_default_ok() {
        let r: SkyResult<&str, i64> = SkyResult::Ok(42);
        assert_eq!(result_with_default(0, r), 42);
    }

    #[test]
    fn result_traverse_ok() {
        let items = vec![1, 2, 3];
        let r = result_traverse(
            |x: i64| -> SkyResult<&str, i64> { SkyResult::Ok(x * 2) },
            items,
        );
        assert_eq!(r.with_default(vec![]), vec![2, 4, 6]);
    }
}

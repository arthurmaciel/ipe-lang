//! WASM pure-kernel **floor tracer** — pins the documented first-slice
//! EXPECTATION of the wasm-target epic without claiming the full target builds.
//!
//! Spec: `runtime-rust/docs/superpowers/specs/2026-06-15-wasm-target-design.md`
//! (divergence `wasm-target`, disposition `DOCUMENT_BLOCKED`).
//!
//! ## What this file IS
//! A documentation-and-invariant guard. The spec (Q7) names a single landable,
//! in-boundary slice of the blocked wasm epic: the **pure-kernel wasm floor** —
//! `List` / `String` / `Dict` / `Maybe` / `Result` + JSON, *no* Task I/O —
//! plausibly already cross-compiles to `wasm32-unknown-unknown` once two
//! blocking pieces land: (a) the `cfg`-gated `SkyTask` `Send` split, and (b) a
//! wasm Cargo/entry branch that excludes the tokio modules.
//!
//! ## What this file is NOT
//! It is **not** proof that the full wasm target builds. The native test runner
//! here cannot cross-compile to `wasm32-unknown-unknown` (no wasm toolchain is
//! assumed, and `tests/` runs under the default native feature set). So instead
//! of an actual wasm build, this file statically asserts the floor's two
//! blocking FACTS so a regression is caught:
//!
//!   1. The pure kernels under test do NOT touch any Task / tokio / async path —
//!      they are synchronous, total, `std`-only functions and therefore are
//!      genuine candidates for the `cfg`-excluded wasm floor. We prove this by
//!      *calling* a representative pure fn from each module (list / string /
//!      dict / json + Maybe / Result) with concrete values and asserting the
//!      concrete results. If any of these ever grows a Task return, an `await`,
//!      or a tokio dependency, the floor's premise breaks and this file stops
//!      compiling / passing.
//!
//!   2. `SkyTask`'s `Send` bound (`core.rs:17`) is the gate for the floor. The
//!      spec's decision (Q2) is that the bound must be relaxed ONLY via
//!      `#[cfg(target_arch = "wasm32")]`, never forked into a second type and
//!      never hidden behind a `MaybeSend` marker trait. We anchor that here with
//!      a comment pointing at the anchor line AND a compile-time assertion that
//!      on the **native** target a `SkyTask` value really is `Send` (the bound
//!      this file's host enforces). A future wasm cfg-split must keep this native
//!      assertion true while relaxing the bound under the wasm cfg only.
//!
//! Keep this small, deterministic, dependency-free, and panic-free on every
//! Sky-reachable path.

use sky_runtime_rust::*;

// ── (1) Pure-kernel floor candidates: no Task / tokio / async ──────────────
//
// Each of these is a synchronous `std`-only kernel. Calling them here is the
// proof that the floor's "pure subset" is real: if any kernel below acquired a
// Task return or an async backend, the call would no longer type-check as a
// plain value comparison and this test would fail to compile.

#[test]
fn list_kernel_is_pure_and_total() {
    // List.range / List.length — no Task, no allocation surprise, fully sync.
    let xs = sky_runtime::list::list_range(1, 5);
    assert_eq!(xs, vec![1, 2, 3, 4, 5]);
    assert_eq!(sky_runtime::list::list_length(xs), 5);

    // List.member over an empty list must NOT panic (total negative path).
    let empty: Vec<i64> = Vec::new();
    assert!(!sky_runtime::list::list_member(7, empty));
}

#[test]
fn string_kernel_is_pure_and_total() {
    // String.append / toUpper / reverse — pure transforms, no I/O.
    let s = sky_runtime::string::string_append("sky".to_string(), "wasm".to_string());
    assert_eq!(sky_runtime::string::string_to_upper(s.clone()), "SKYWASM");
    assert_eq!(sky_runtime::string::string_reverse(s), "msawyks");

    // String.toInt failure path is a Maybe, not a panic — floor-safe.
    match sky_runtime::string::string_to_int("not-a-number".to_string()) {
        SkyMaybe::Nothing => {}
        SkyMaybe::Just(_) => panic!("test bug: non-numeric string parsed as Int"),
    }
}

#[test]
fn dict_kernel_is_pure_and_total() {
    // Dict.fromList / get — pure HashMap ops, deterministic sorted iteration.
    let d =
        sky_runtime::dict::dict_from_list(vec![("a".to_string(), 1i64), ("b".to_string(), 2i64)]);
    match sky_runtime::dict::dict_get("a".to_string(), d.clone()) {
        SkyMaybe::Just(v) => assert_eq!(v, 1),
        SkyMaybe::Nothing => panic!("test bug: present key missed"),
    }
    // Absent key → Nothing (total), and keys come back sorted (pure contract).
    assert!(matches!(
        sky_runtime::dict::dict_get("zzz".to_string(), d.clone()),
        SkyMaybe::Nothing
    ));
    assert_eq!(
        sky_runtime::dict::dict_keys(d),
        vec!["a".to_string(), "b".to_string()]
    );
}

// `json` is feature-gated (the kernel uses serde_json), so this leg compiles only
// when the feature is on. Without the gate, `cargo test/clippy --all-targets` under
// a narrow non-json subset (e.g. `--features websocket_client`) fails to resolve
// `sky_runtime::json`. The list/string/dict legs below stay ungated (always-compiled).
#[cfg(feature = "json")]
#[test]
fn json_kernel_encode_is_pure_and_total() {
    // JSON encode is a pure `Int -> Value -> String` — the floor's JSON leg.
    let obj = sky_runtime::json::json_enc_object(vec![
        (
            "name".to_string(),
            sky_runtime::json::json_enc_string("sky".to_string()),
        ),
        ("n".to_string(), sky_runtime::json::json_enc_int(42)),
        ("ok".to_string(), sky_runtime::json::json_enc_bool(true)),
    ]);
    let compact = sky_runtime::json::json_enc_encode(0, obj);
    // Field order is preserved by the object encoder; assert on stable substrings
    // so the test does not depend on serde map iteration order beyond presence.
    assert!(compact.contains("\"name\":\"sky\""), "encoded: {compact}");
    assert!(compact.contains("\"n\":42"), "encoded: {compact}");
    assert!(compact.contains("\"ok\":true"), "encoded: {compact}");
}

#[test]
fn maybe_result_combinators_are_pure_and_total() {
    // Maybe.withDefault on Nothing plugs the default — no panic on the empty case.
    let n: SkyMaybe<i64> = SkyMaybe::Nothing;
    assert_eq!(n.with_default(0), 0);
    assert_eq!(SkyMaybe::Just(9i64).with_default(0), 9);

    // Result happy + error paths are values, never aborts.
    let ok: SkyResult<String, i64> = ok_res(5);
    match ok {
        SkyResult::Ok(v) => assert_eq!(v, 5),
        SkyResult::Err(_) => panic!("test bug: ok_res produced Err"),
    }
    let err: SkyResult<String, i64> = SkyResult::Err(str_err::<String>("boom"));
    assert!(matches!(err, SkyResult::Err(_)));
}

// ── (2) The `SkyTask` `Send` gate (core.rs:17) ─────────────────────────────
//
// Per the spec (Q2), the floor is blocked on relaxing `SkyTask`'s `Send` bound,
// and that relaxation MUST be `#[cfg(target_arch = "wasm32")]`-gated, never a
// forked type and never a `MaybeSend` marker trait. We cannot assert the wasm
// shape from a native test, but we CAN nail down the native invariant the future
// cfg-split must preserve: on the native (non-wasm) target a `SkyTask` value is
// `Send`. A regression that drops `Send` on native (or a fork that diverges the
// type) breaks this assertion; a correct wasm cfg-split leaves it untouched
// because it only adds a `#[cfg(target_arch = "wasm32")]` arm.

/// Compile-time witness: `T: Send`. Never called — its existence is the proof.
#[allow(dead_code)]
fn assert_send<T: Send>() {}

#[test]
fn sky_task_is_send_on_native_target() {
    // Native host (`not(target_arch = "wasm32")`) MUST keep the `Send` bound —
    // tokio `block_on` on a spawned OS thread and `tokio::spawn` both require it.
    // The future wasm cfg-split (spec Q2) relaxes this ONLY under the wasm cfg.
    #[cfg(not(target_arch = "wasm32"))]
    {
        // `SkyTask<E, A>` is `Pin<Box<dyn Future<..> + Send + 'static>>` per
        // core.rs:17, so the boxed value itself is `Send`.
        assert_send::<SkyTask<String, i64>>();
    }

    // On a hypothetical wasm build this assertion is simply skipped — the floor
    // tracer makes no `Send` claim there, matching the spec's `!Send` wasm arm.
    #[cfg(target_arch = "wasm32")]
    {
        // Intentionally empty: see spec Q2 — wasm futures are `!Send`.
    }
}

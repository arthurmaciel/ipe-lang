//! THE SEAL for user enums whose variant payloads hold a `Box`-carried function.
//!
//! A function directly in a constructor payload is stored on the `Clone`
//! `Arc<dyn Fn>` carrier, so its enum is `Clone` and reuse is sound. A function
//! under a `Maybe` payload stays on the non-`Clone` `Box` carrier (the
//! `Maybe` kernels consume it as an owned `FnOnce`), so the enum gets no
//! `Clone` impl. The fn-value reuse gate must see that function through the
//! enum's variant payloads: reusing such a value fails closed at `ipe` time
//! instead of reaching `cargo build` as a use-after-move.
//!
//! | Test | Shape | Outcome |
//! |---|---|---|
//! | `boxed_fn_payload_enum_reuse_fails_closed` | `H (Maybe (Int -> Int))` param reused | IPE-L0127 |
//! | `boxed_fn_payload_enum_linear_builds` | same enum used once | builds + prints `42` |
//! | `shared_fn_payload_enum_reuse_builds` | `G (Int -> Int)` param reused | builds + prints `30` |
//!
//! ```text
//! cargo test -p ipe --test g_misc golden_enum_fn_payload_reuse_seal
//! IPE_E2E=1 cargo test -p ipe --test g_misc golden_enum_fn_payload_reuse_seal
//! ```

use std::path::{Path, PathBuf};

use ipe::CliError;

/// A runtime `false` the optimiser cannot fold — a deliberate failure marker.
const fn false_marker() -> bool {
    std::hint::black_box(false)
}

/// Write `source` as a single-file `Main.ipe` under a fresh scratch dir keyed by
/// `name`, returning the entry path, or `None` (reported as a failure) when the
/// scratch setup fails.
fn write_single(name: &str, source: &str) -> Option<PathBuf> {
    let dir = crate::support::scratch_root()
        .join("ipec_enum_fn_payload_reuse")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    let entry = src.join("Main.ipe");
    let written = std::fs::create_dir_all(&src).and_then(|()| std::fs::write(&entry, source));
    assert!(
        written.is_ok(),
        "{name}: must write the fixture source to a scratch dir: {written:?}"
    );
    written.ok().map(|()| entry)
}

/// The scratch output dir for `name`, cleared.
fn out_dir(name: &str) -> PathBuf {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("enum_fn_payload_reuse_out")
        .join(name);
    let _ = std::fs::remove_dir_all(&out);
    out
}

/// Assert `ipe` rejects the build of `entry` with the typed `expected` code.
#[track_caller]
fn assert_rejected(name: &str, entry: &Path, expected: ipe_diagnostics::Code) {
    // A refusal test proves nothing if it skips silently: a missing runtime
    // would let the fail-closed assertion pass without driving the pipeline.
    let runtime = ipe::resolve_runtime();
    assert!(
        runtime.is_ok(),
        "{name}: runtime must resolve to prove the fail-closed refusal: {runtime:?}"
    );
    let Ok(runtime) = runtime else {
        return;
    };
    let out = out_dir(name);
    match ipe::build_with_sibling_discovery(entry, &out, &runtime) {
        Err(CliError::Pipeline { diag, .. }) => assert_eq!(
            diag.code(),
            expected,
            "{name}: expected a fail-closed {expected:?}, got a different diagnostic"
        ),
        Ok(()) => assert!(
            false_marker(),
            "{name}: ipe ACCEPTED a reuse of a non-Clone fn-payload enum (exit 0) — \
             the emitted crate would fail cargo with a use-after-move, a SEAL break"
        ),
        Err(other) => assert!(
            false_marker(),
            "{name}: non-pipeline build error: {other:?}"
        ),
    }
}

/// Build `entry`; `None` when the runtime is unavailable or `ipe` refused it
/// (the refusal is reported as a test failure).
#[track_caller]
fn accepted_out(name: &str, entry: &Path) -> Option<PathBuf> {
    let runtime = ipe::resolve_runtime().ok()?;
    let out = out_dir(name);
    match ipe::build_with_sibling_discovery(entry, &out, &runtime) {
        Ok(()) => Some(out),
        Err(err) => {
            assert!(
                false_marker(),
                "{name}: ipe REJECTED a well-formed program — a false rejection: {err}"
            );
            None
        }
    }
}

/// Under `IPE_E2E`, `cargo build` + run the crate in `out` and check its stdout.
#[track_caller]
fn assert_runs(name: &str, out: &Path, expected_stdout: &str) {
    if std::env::var("IPE_E2E").is_err() {
        return; // emit-only fast pass
    }
    let outcome = crate::support::build_and_run_emitted(name, out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "{name}: emitted crate must build and run to exit 0; stdout:\n{}",
        outcome.stdout
    );
    assert_eq!(
        outcome.stdout.trim_end(),
        expected_stdout,
        "{name}: emitted crate built (SEAL held) but ran to the wrong output"
    );
}

/// `H` holds a `Box`-carried function under `Maybe`; `twice` reuses the param.
const BOXED_FN_PAYLOAD_ENUM_REUSE: &str = r#"module Main exposing (main)

import Ipe.Io as Io


type H = H (Maybe (Int -> Int))


twice : H -> ( H, H )
twice h =
    ( h, h )


main =
    Io.println "x"
"#;

/// The same `H` consumed once — a bare move that needs no `Clone`.
const BOXED_FN_PAYLOAD_ENUM_LINEAR: &str = r#"module Main exposing (main)

import Ipe.Io as Io
import Ipe.String as String


type H = H (Maybe (Int -> Int))


apply : H -> Int -> Int
apply h x =
    case h of
        H m ->
            case m of
                Just f ->
                    f x

                Nothing ->
                    x


main =
    Io.println (String.fromInt (apply (H (Just (\n -> n + 1))) 41))
"#;

/// A function directly in the payload rides the `Arc` carrier, so `G` is
/// `Clone` and reusing the param stays accepted.
const SHARED_FN_PAYLOAD_ENUM_REUSE: &str = r#"module Main exposing (main)

import Ipe.Io as Io
import Ipe.String as String


type G = G (Int -> Int)


run : G -> Int -> Int
run g x =
    case g of
        G f ->
            f x


both : G -> Int
both g =
    run g 1 + run g 2


main =
    Io.println (String.fromInt (both (G (\n -> n * 10))))
"#;

#[test]
fn boxed_fn_payload_enum_reuse_fails_closed() {
    let name = "boxed_fn_payload_enum_reuse";
    let Some(entry) = write_single(name, BOXED_FN_PAYLOAD_ENUM_REUSE) else {
        return;
    };
    assert_rejected(name, &entry, ipe_diagnostics::IPE_L0127);
}

#[test]
fn boxed_fn_payload_enum_linear_builds() {
    let name = "boxed_fn_payload_enum_linear";
    let Some(entry) = write_single(name, BOXED_FN_PAYLOAD_ENUM_LINEAR) else {
        return;
    };
    let Some(out) = accepted_out(name, &entry) else {
        return;
    };
    assert_runs(name, &out, "42");
}

#[test]
fn shared_fn_payload_enum_reuse_builds() {
    let name = "shared_fn_payload_enum_reuse";
    let Some(entry) = write_single(name, SHARED_FN_PAYLOAD_ENUM_REUSE) else {
        return;
    };
    let Some(out) = accepted_out(name, &entry) else {
        return;
    };
    assert_runs(name, &out, "30");
}

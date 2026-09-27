//! THE SEAL for a container-first kernel whose function argument captures a
//! binding the container expression also consumes by value.
//!
//! `Maybe.map f m`, `Result.andThen f r`, … render container-first in the
//! runtime (`ipe_maybe_map(m, f)`). Rust evaluates arguments left-to-right, so
//! the container runs BEFORE `f`'s closure is built: a non-Copy binding the
//! container moves (`label` passed by value into `wrapJust label`) is gone when
//! the closure's `let label = label.clone()` capture reads it — `ipe build`
//! exit 0, then `cargo build` E0382. The backend clones every capture of `f` at
//! its container use site for EVERY kernel `kernel_swaps_first_two` reverses,
//! so the original binding survives into the closure.
//!
//! | Kernel | Shape | Contribution |
//! |---|---|---|
//! | `Maybe.map` | `Maybe.map (\v -> v ++ label) (wrapJust label)` | `aa` |
//! | `Maybe.andThen` | `Maybe.andThen (\v -> Just (v ++ label)) (wrapJust label)` | `bb` |
//! | `Result.map` | `Result.map (\v -> v ++ label) (wrapOk label)` | `cc` |
//! | `Result.andThen` | `Result.andThen (\v -> Ok (v ++ label)) (wrapOk label)` | `dd` |
//! | `Result.mapError` | `Result.mapError (\e -> e ++ label) (wrapErr label)` | `ee` |
//!
//! ```text
//! # emit check only (fast):
//! cargo test -p ipe --test g_issues golden_swapped_container_capture
//! # full (cargo build + run):
//! IPE_E2E=1 cargo test -p ipe --test g_issues golden_swapped_container_capture
//! ```

use std::path::PathBuf;

use ipe::CliError;

/// A runtime `false` the optimiser cannot fold, so `assert!(false_marker(), …)`
/// reads as a deliberate unconditional failure rather than a suspicious constant
/// condition.
const fn false_marker() -> bool {
    std::hint::black_box(false)
}

/// Write `source` as a single-file `Main.ipe` under a fresh scratch dir keyed by
/// `name`, returning the entry path (or `None` if scratch setup fails).
fn write_single(name: &str, source: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("swapped-container-capture")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).ok()?;
    let entry = src.join("Main.ipe");
    std::fs::write(&entry, source).ok()?;
    Some(entry)
}

/// The scratch output dir for `name`, cleared.
fn out_dir(name: &str) -> PathBuf {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("swapped-container-capture-out")
        .join(name);
    let _ = std::fs::remove_dir_all(&out);
    out
}

/// Assert that `source` is ACCEPTED by `ipe` (exit 0) and — under `IPE_E2E` —
/// that the emitted crate `cargo build`s and runs to `expected_stdout`.
#[track_caller]
fn assert_accepted(name: &str, source: &str, expected_stdout: &str) {
    let Some(entry) = write_single(name, source) else {
        return;
    };
    let out = out_dir(name);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    match ipe::build(&entry, &out, &runtime) {
        Ok(()) => {}
        Err(CliError::Pipeline { diag, .. }) => {
            assert!(
                false_marker(),
                "{name}: ipe REJECTED a well-formed program with {}",
                diag.code().as_str()
            );
            return;
        }
        Err(other) => {
            assert!(
                false_marker(),
                "{name}: non-pipeline build error: {other:?}"
            );
            return;
        }
    }

    if std::env::var("IPE_E2E").is_err() {
        return; // emit-only fast pass
    }
    let outcome = crate::support::build_and_run_emitted(name, &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "{name}: emitted crate must cargo-build (THE SEAL) and run to exit 0"
    );
    assert_eq!(
        outcome.stdout.trim_end(),
        expected_stdout,
        "{name}: emitted crate built (SEAL held) but ran to the wrong output"
    );
}

/// Every Ipê-callable container-first `Maybe`/`Result` kernel, each with a
/// function argument that captures the `String` its container moves.
const SWAPPED_CONTAINER_CAPTURE: &str = r#"module Main exposing (main)

import Ipe.Io as Io
import Ipe.Maybe as Maybe
import Ipe.Result as Result
import Ipe.String as String


wrapJust : String -> Maybe String
wrapJust s =
    Just s


wrapOk : String -> Result String String
wrapOk s =
    Ok s


wrapErr : String -> Result String String
wrapErr s =
    Err s


errOr : String -> Result String String -> String
errOr fallback r =
    case r of
        Ok _ ->
            fallback

        Err e ->
            e


maybeMap : String -> Maybe String
maybeMap label =
    Maybe.map (\v -> v ++ label) (wrapJust label)


maybeAndThen : String -> Maybe String
maybeAndThen label =
    Maybe.andThen (\v -> Just (v ++ label)) (wrapJust label)


resultMap : String -> Result String String
resultMap label =
    Result.map (\v -> v ++ label) (wrapOk label)


resultAndThen : String -> Result String String
resultAndThen label =
    Result.andThen (\v -> Ok (v ++ label)) (wrapOk label)


resultMapError : String -> Result String String
resultMapError label =
    Result.mapError (\e -> e ++ label) (wrapErr label)


main : Task Error ()
main =
    Io.println
        (String.join ","
            [ Maybe.withDefault "none" (maybeMap "a")
            , Maybe.withDefault "none" (maybeAndThen "b")
            , Result.withDefault "err" (resultMap "c")
            , Result.withDefault "err" (resultAndThen "d")
            , errOr "ok" (resultMapError "e")
            ]
        )
"#;

#[test]
fn swapped_container_capture_builds() {
    assert_accepted(
        "swapped_container_capture",
        SWAPPED_CONTAINER_CAPTURE,
        "aa,bb,cc,dd,ee",
    );
}

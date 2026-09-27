//! THE SEAL for a record field read whose type is a tuple of `Copy` parts.
//!
//! A tuple lowers to a bare Rust tuple, so it is `Copy` exactly when every part
//! is (`ipe_ir::ir_type_is_copy`, the one `Copy` fact shared by the lowerer's
//! `CopyLeaf` class and the backend's field-read elision). Such a read is
//! emitted as a bare copy — `(base).field` on a concrete struct, and
//! `*(base).ipe_field()` through a row-generic witness getter — with no
//! `.clone()`. The base stays usable afterwards, so the same field is read
//! twice and the record is reused.
//!
//! | Fixture | Shape | Outcome |
//! |---|---|---|
//! | `copy_tuple_field_read` | `( Int, Int )` field read twice, concrete + row-generic base | builds + prints `10` then `7` |
//!
//! ```text
//! # gate check only (fast):
//! cargo test -p ipe --test g_issues golden_copy_tuple_field_read
//! # full (cargo build + run the emitted crate):
//! IPE_E2E=1 cargo test -p ipe --test g_issues golden_copy_tuple_field_read
//! ```

use std::path::PathBuf;

use ipe::CliError;

/// A runtime `false` the optimiser cannot fold, so `assert!(false_marker(), …)`
/// reads as a deliberate unconditional failure rather than a suspicious constant
/// condition.
const fn false_marker() -> bool {
    std::hint::black_box(false)
}

/// Write `source` as a single-file `Main.ipe` under a fresh scratch dir.
fn write_single(name: &str, source: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("copy-tuple-field")
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
        .join("copy-tuple-field-out")
        .join(name);
    let _ = std::fs::remove_dir_all(&out);
    out
}

/// Assert `source` is accepted and, under `IPE_E2E`, builds and prints `expected_stdout`.
#[track_caller]
fn assert_accepted(name: &str, source: &str, expected_stdout: &str) {
    let entry = crate::support::expect_scratch_entry(name, write_single(name, source));
    let out = out_dir(name);
    let runtime = crate::support::expect_runtime(name, ipe::resolve_runtime());
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
        "{name}: emitted crate must run to exit 0"
    );
    assert_eq!(
        outcome.stdout.trim_end(),
        expected_stdout,
        "{name}: emitted crate built (SEAL held) but ran to the wrong output"
    );
}

/// `pos : ( Int, Int )` is read twice off a concrete record and twice through a
/// row-generic base; each read is a bare copy, and the record is still used
/// afterwards. Prints `10` (`3 + 4 + 3`) then `7`.
const COPY_TUPLE_FIELD_READ: &str = r#"module Main exposing (main)

import Ipe.Io as Io
import Ipe.String as String
import Ipe.Task as Task


type alias Point =
    { pos : ( Int, Int ), label : String }


sumTwice : Point -> Int
sumTwice p =
    case ( p.pos, p.pos ) of
        ( ( a, b ), ( c, _ ) ) ->
            a + b + c


rowSum : { r | pos : ( Int, Int ) } -> Int
rowSum r =
    case ( r.pos, r.pos ) of
        ( ( a, _ ), ( _, d ) ) ->
            a + d


main : Task Error ()
main =
    let
        p =
            { pos = ( 3, 4 ), label = "origin" }
    in
    Task.andThen
        (\_ -> Io.println (String.fromInt (rowSum p)))
        (Io.println (String.fromInt (sumTwice p)))
"#;

#[test]
fn copy_tuple_field_read_round_trips() {
    assert_accepted("copy_tuple_field_read", COPY_TUPLE_FIELD_READ, "10\n7");
}

//! THE SEAL for a non-`Clone` effect-carrier record whose field is read AFTER
//! the record was moved.
//!
//! The reuse gate counts a bare field read `w.tag` as a borrow, not a consume,
//! so `run w = both (consume w) w.tag` sees ONE consume and would pass. But the
//! emitted call evaluates its arguments left to right: `consume(w)` moves `w`,
//! then `(w).tag` reads the moved value — `ipe build` exit 0, then `cargo build`
//! E0382. An order-aware walk now rejects a borrow evaluated after a move with
//! IPE-L0135, while the borrow-then-consume order stays accepted.
//!
//! | Fixture | Shape | Outcome |
//! |---|---|---|
//! | `consume_then_borrow_call` | `both (consume w) w.tag` | fail-closed IPE-L0135 |
//! | `consume_then_borrow_kernel` | `String.append (label w) (String.fromInt w.tag)` | fail-closed IPE-L0135 |
//! | `borrow_then_consume_call` | `tagFirst w.tag (consume w)` | builds + prints `10` |
//! | `let_bound_borrow_then_consume` | `let t = w.tag in both (consume w) t` | builds + prints `10` |
//!
//! A kernel call's argument order is chosen by its emitter, so a move and a
//! read of the same record in sibling kernel arguments are rejected in either
//! written order; binding the field with `let` first is the fix the diagnostic
//! names, proven by the last fixture.
//!
//! ```text
//! # gate check only (fast):
//! cargo test -p ipe --test g_issues golden_l0135_consume_then_borrow
//! # full (cargo build + run the positive fixtures):
//! IPE_E2E=1 cargo test -p ipe --test g_issues golden_l0135_consume_then_borrow
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
        .join("l0135-consume-borrow")
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
        .join("l0135-consume-borrow-out")
        .join(name);
    let _ = std::fs::remove_dir_all(&out);
    out
}

/// Assert that `source` is REJECTED by `ipe` with `expected` (a typed pipeline
/// diagnostic), never accepted-then-cargo-failed.
#[track_caller]
fn assert_rejected(name: &str, source: &str, expected: ipe_diagnostics::Code) {
    let Some(entry) = write_single(name, source) else {
        return; // scratch unavailable — skip
    };
    let out = out_dir(name);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return; // runtime unavailable — skip
    };
    match ipe::build(&entry, &out, &runtime) {
        Err(CliError::Pipeline { diag, .. }) => assert_eq!(
            diag.code(),
            expected,
            "{name}: expected a fail-closed {expected:?}, got a different diagnostic"
        ),
        Ok(()) => assert!(
            false_marker(),
            "{name}: ipe ACCEPTED a field read of a non-Clone effect-carrier record \
             evaluated after the record was moved (exit 0) — the emitted crate would \
             fail cargo with E0382, a SEAL break"
        ),
        Err(other) => assert!(
            false_marker(),
            "{name}: non-pipeline build error: {other:?}"
        ),
    }
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
                "{name}: ipe REJECTED a well-formed program with {} — a false rejection \
                 (a field read evaluated BEFORE the single move is sound)",
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

/// The shared prelude: `consume` moves the whole non-`Clone` record (its type
/// embeds a `Task`) and returns the effect; `both` / `tagFirst` sequence the
/// effect with an `Int`, differing only in parameter order.
const PRELUDE: &str = r"module Main exposing (main)

import Ipe.Io as Io
import Ipe.String as String
import Ipe.Task as Task


consume : { job : Task Error Int, tag : Int } -> Task Error Int
consume r =
    case r of
        { job } ->
            job


label : { job : Task Error Int, tag : Int } -> String
label r =
    case r of
        { tag } ->
            String.fromInt tag


both : Task Error Int -> Int -> Task Error ()
both task n =
    Task.andThen
        (\x -> Io.println (String.fromInt (x + n)))
        task


tagFirst : Int -> Task Error Int -> Task Error ()
tagFirst n task =
    both task n
";

/// Consume-then-borrow in a user call: `consume w` moves `w`, then `w.tag`
/// reads it.
const CONSUME_THEN_BORROW_CALL: &str = r"

run : { job : Task Error Int, tag : Int } -> Task Error ()
run w =
    both (consume w) w.tag


main : Task Error ()
main =
    run { job = Task.succeed 7, tag = 3 }
";

/// Consume-then-borrow across sibling kernel arguments: `label w` moves `w`,
/// and `w.tag` reads it in the other argument.
const CONSUME_THEN_BORROW_KERNEL: &str = r"

describe : { job : Task Error Int, tag : Int } -> String
describe w =
    String.append (label w) (String.fromInt w.tag)


main : Task Error ()
main =
    Io.println (describe { job = Task.succeed 7, tag = 3 })
";

/// Borrow-then-consume in a user call: `w.tag` is read while `w` is still
/// owned, then `consume w` moves it. Prints `10`.
const BORROW_THEN_CONSUME_CALL: &str = r"

run : { job : Task Error Int, tag : Int } -> Task Error ()
run w =
    tagFirst w.tag (consume w)


main : Task Error ()
main =
    run { job = Task.succeed 7, tag = 3 }
";

/// The documented fix: bind the field with `let` before the consuming call.
/// Prints `10`.
const LET_BOUND_BORROW_THEN_CONSUME: &str = r"

run : { job : Task Error Int, tag : Int } -> Task Error ()
run w =
    let
        t =
            w.tag
    in
    both (consume w) t


main : Task Error ()
main =
    run { job = Task.succeed 7, tag = 3 }
";

fn program(body: &str) -> String {
    format!("{PRELUDE}{body}")
}

#[test]
fn consume_then_borrow_call_fails_closed() {
    assert_rejected(
        "consume_then_borrow_call",
        &program(CONSUME_THEN_BORROW_CALL),
        ipe_diagnostics::IPE_L0135,
    );
}

#[test]
fn consume_then_borrow_kernel_fails_closed() {
    assert_rejected(
        "consume_then_borrow_kernel",
        &program(CONSUME_THEN_BORROW_KERNEL),
        ipe_diagnostics::IPE_L0135,
    );
}

#[test]
fn borrow_then_consume_call_round_trips() {
    assert_accepted(
        "borrow_then_consume_call",
        &program(BORROW_THEN_CONSUME_CALL),
        "10",
    );
}

#[test]
fn let_bound_borrow_then_consume_round_trips() {
    assert_accepted(
        "let_bound_borrow_then_consume",
        &program(LET_BOUND_BORROW_THEN_CONSUME),
        "10",
    );
}

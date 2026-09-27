//! THE SEAL for the sequenced-task capture-clone rewrite and consume-once callees.
//!
//! A do-block statement whose rest still reads a binding has its reads of that
//! binding rewritten to clones. Three facts keep that rewrite sound:
//!
//! - the ipe-time refusal consults the one clone-ability fact, so a record
//!   holding a live app handle (no `Clone` impl, yet no effect carrier) is
//!   refused with IPE-L0135 instead of emitting a clone cargo rejects (E0599);
//! - a closure literal whose body reads the binding captures a hoisted clone,
//!   so its `move` capture never takes the binding the rest still reads (E0382);
//! - a cloned-receiver field read clones the field, never the whole record.
//!
//! A function decoded by a `Json.Decode` mapper is consume-once: calling it
//! twice, or capturing it in a closure, is refused with IPE-L0127 at ipe time
//! rather than emitting a second move of a `Box<dyn FnOnce>` (E0382 / E0507).
//!
//! | Fixture | Shape | Outcome |
//! |---|---|---|
//! | `app_handle_record_seq_reuse` | handle record read in a statement, reused in rest | fail-closed IPE-L0135 |
//! | `decoded_fn_called_twice` | mapper payload `f 1 + f 2` | fail-closed IPE-L0127 |
//! | `decoded_fn_captured` | mapper payload captured by an inner lambda | fail-closed IPE-L0127 |
//! | `decoded_fn_called_once` | mapper payload called once per branch | builds + prints `11` |
//! | `seq_closure_and_field_read` | statement closure + field reads, reused in rest | builds + prints three lines |
//!
//! ```text
//! # gate check only (fast):
//! cargo test -p ipe --test g_issues golden_l0135_seq_clone_fact
//! # full (cargo build + run the positive fixtures):
//! IPE_E2E=1 cargo test -p ipe --test g_issues golden_l0135_seq_clone_fact
//! ```

use std::path::{Path, PathBuf};

use ipe::CliError;

/// A runtime `false` the optimiser cannot fold, marking a deliberate failure.
const fn false_marker() -> bool {
    std::hint::black_box(false)
}

/// Write `source` as a single-file `Main.ipe` under a fresh scratch dir keyed by `name`.
fn write_single(name: &str, source: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("l0135-seq-clone-fact")
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
        .join("l0135-seq-clone-fact-out")
        .join(name);
    let _ = std::fs::remove_dir_all(&out);
    out
}

/// All emitted user Rust: `main.rs` plus the split user module, when present.
fn emitted_user_rust(out: &Path) -> String {
    let mut emitted = std::fs::read_to_string(out.join("src").join("main.rs")).unwrap_or_default();
    let module = out.join("src").join("ipe_mods").join("ipe_mod_main.rs");
    if let Ok(extra) = std::fs::read_to_string(module) {
        emitted.push_str(&extra);
    }
    emitted
}

/// Assert `source` is REJECTED by `ipe` with `expected`, never accepted-then-cargo-failed.
#[track_caller]
fn assert_rejected(name: &str, source: &str, expected: ipe_diagnostics::Code) {
    let entry = crate::support::expect_scratch_entry(name, write_single(name, source));
    let out = out_dir(name);
    let runtime = crate::support::expect_runtime(name, ipe::resolve_runtime());
    match ipe::build(&entry, &out, &runtime) {
        Err(CliError::Pipeline { diag, .. }) => assert_eq!(
            diag.code(),
            expected,
            "{name}: expected a fail-closed {expected:?}, got a different diagnostic"
        ),
        Ok(()) => assert!(
            false_marker(),
            "{name}: ipe ACCEPTED (exit 0) a program whose emitted crate fails cargo — \
             a SEAL break"
        ),
        Err(other) => assert!(
            false_marker(),
            "{name}: non-pipeline build error: {other:?}"
        ),
    }
}

/// Assert `source` is ACCEPTED by `ipe`, returning the emitted user Rust.
///
/// Under `IPE_E2E` the emitted crate must also `cargo build` and run to
/// `expected_stdout`.
#[track_caller]
fn assert_accepted(name: &str, source: &str, expected_stdout: &str) -> Option<String> {
    let entry = crate::support::expect_scratch_entry(name, write_single(name, source));
    let out = out_dir(name);
    let runtime = crate::support::expect_runtime(name, ipe::resolve_runtime());
    match ipe::build(&entry, &out, &runtime) {
        Ok(()) => {}
        Err(CliError::Pipeline { diag, .. }) => {
            assert!(
                false_marker(),
                "{name}: ipe REJECTED a well-formed program with {} — a false rejection",
                diag.code().as_str()
            );
            return None;
        }
        Err(other) => {
            assert!(
                false_marker(),
                "{name}: non-pipeline build error: {other:?}"
            );
            return None;
        }
    }
    let emitted = emitted_user_rust(&out);
    if std::env::var("IPE_E2E").is_ok() {
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
    Some(emitted)
}

/// A record holding a `Web.WebApp` handle: no `Clone` impl, no effect carrier.
///
/// The statement's kernel argument reads `site.name` and the rest reads `site`
/// again, so the sequencing rewrite would clone `site`.
const APP_HANDLE_RECORD_SEQ_REUSE: &str = r#"module Main exposing (main)

import Ipe.Io as Io
import Ipe.Task as Task exposing (Task)
import Ipe.Tea.Web as Web
import Ipe.Tea.Web.Cmd as Cmd
import Ipe.Tea.Web.Sub as Sub
import Ipe.Ui as Ui


type alias Model =
    { count : Int }


type Msg
    = Noop


update : Msg -> Model -> ( Model, Cmd.Cmd Msg )
update _msg model =
    ( model, Cmd.none )


view : Model -> Element Msg
view _model =
    Ui.text "hi"


embedded : Web.WebApp
embedded =
    Web.embed
        { init = \_ -> ( { count = 0 }, Cmd.none )
        , update = update
        , view = view
        , subscriptions = \_ -> Sub.none
        , routes = []
        , notFound = Noop
        }


type alias Site =
    { app : Web.WebApp, name : String }


serve : Site -> Task Error ()
serve site =
    do
        Io.println site.name
        Io.println site.name


main : Task Error ()
main =
    serve { app = embedded, name = "/" }
"#;

/// Shared prelude for the decoder-payload fixtures: a decoder whose payload is a function.
const DECODER_PRELUDE: &str = r#"module Main exposing (main)

import Ipe.Io as Io
import Ipe.Json.Decode as JsonDec exposing (Decoder)
import Ipe.List as List
import Ipe.Result
import Ipe.String


addN : Int -> Int -> Int
addN base x =
    base + x


adderDecoder : Decoder (Int -> Int)
adderDecoder =
    JsonDec.succeed (addN 5)


main =
    Io.println (String.fromInt (Result.withDefault 0 (JsonDec.decodeString applied "{}")))
"#;

/// The decoded function is called twice on one path: the second call reuses a moved box.
const DECODED_FN_CALLED_TWICE: &str = r"

applied : Decoder Int
applied =
    JsonDec.map (\f -> f 1 + f 2) adderDecoder
";

/// The decoded function is captured by an inner lambda, moving it out of that closure.
const DECODED_FN_CAPTURED: &str = r"

applied : Decoder Int
applied =
    JsonDec.map (\f -> List.sum (List.map (\x -> f x) [ 1, 2 ])) adderDecoder
";

/// Over-rejection guard: one call per branch is linear. Prints `11`.
const DECODED_FN_CALLED_ONCE: &str = r"

applied : Decoder Int
applied =
    JsonDec.map
        (\f ->
            if True then
                f 6

            else
                f 0
        )
        adderDecoder
";

/// A `Clone` record read by a statement closure and a statement kernel argument, then reused.
///
/// The closure captures a hoisted clone of `p` and the bare field read borrows
/// `p`, so the rest still owns it. Prints `Ada`, `aAdabAda`, `3`.
const SEQ_CLOSURE_AND_FIELD_READ: &str = r#"module Main exposing (main)

import Ipe.Io as Io
import Ipe.List as List
import Ipe.String as String
import Ipe.Task as Task exposing (Task)


type alias Profile =
    { name : String, age : Int }


profile : Int -> Profile
profile n =
    { name = "Ada", age = n }


run : Profile -> Task Error ()
run p =
    do
        Io.println p.name
        Io.println (String.concat (List.map (\s -> s ++ p.name) [ "a", "b" ]))
        Io.println (String.fromInt p.age)


main : Task Error ()
main =
    run (profile 3)
"#;

#[test]
fn app_handle_record_seq_reuse_fails_closed() {
    assert_rejected(
        "app_handle_record_seq_reuse",
        APP_HANDLE_RECORD_SEQ_REUSE,
        ipe_diagnostics::IPE_L0135,
    );
}

#[test]
fn decoded_fn_called_twice_fails_closed() {
    assert_rejected(
        "decoded_fn_called_twice",
        &format!("{DECODER_PRELUDE}{DECODED_FN_CALLED_TWICE}"),
        ipe_diagnostics::IPE_L0127,
    );
}

#[test]
fn decoded_fn_captured_fails_closed() {
    assert_rejected(
        "decoded_fn_captured",
        &format!("{DECODER_PRELUDE}{DECODED_FN_CAPTURED}"),
        ipe_diagnostics::IPE_L0127,
    );
}

#[test]
fn decoded_fn_called_once_round_trips() {
    let _ = assert_accepted(
        "decoded_fn_called_once",
        &format!("{DECODER_PRELUDE}{DECODED_FN_CALLED_ONCE}"),
        "11",
    );
}

#[test]
fn seq_closure_and_field_read_round_trips() {
    let Some(emitted) = assert_accepted(
        "seq_closure_and_field_read",
        SEQ_CLOSURE_AND_FIELD_READ,
        "Ada\naAdabAda\n3",
    ) else {
        return;
    };
    assert!(
        !emitted.contains("(p.clone())."),
        "a field read must clone the field, never the whole record; emitted:\n{emitted}"
    );
}

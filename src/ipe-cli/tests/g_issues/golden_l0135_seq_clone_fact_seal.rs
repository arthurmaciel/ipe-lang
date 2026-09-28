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
//! - a cloned-receiver field read clones the field, never the whole record;
//! - a read of a move-only field (one holding a function, effect carrier or
//!   app handle) moves the field out instead of cloning it, and a second read
//!   of the moved field is refused with IPE-L0135.
//!
//! A row-polymorphic parameter clones under its signature's `R: Clone` bound,
//! so the same reuse of one is accepted; a row nested under a container in the
//! signature is refused by the nested-row gate (IPE-L0131) before any clone
//! decision is reached.
//!
//! A move-only field read inside a re-callable closure is refused with
//! IPE-L0126: the `Fn` closure holds its capture by reference on each call.
//! A function-carrying row field has no `Clone`, so its signature is refused
//! with IPE-L0135. A server handler renders as `Arc<dyn Fn>`, so a `Maybe`
//! over one clones like any `Clone` field.
//!
//! A closure in a run-once slot (`Task.andThen`'s function) may move a
//! capture once; a second move inside the same body, as an eta-expanded
//! partial application duplicating a move-only argument produces, is refused
//! with IPE-L0126 rather than emitting a closure cargo rejects (E0382).
//!
//! A function decoded by a `Json.Decode` mapper is consume-once: calling it
//! twice, or capturing it in a closure, is refused with IPE-L0127 at ipe time
//! rather than emitting a second move of a `Box<dyn FnOnce>` (E0382 / E0507).
//!
//! | Fixture | Shape | Outcome |
//! |---|---|---|
//! | `app_handle_record_seq_reuse` | handle record read in a statement, reused in rest | fail-closed IPE-L0135 |
//! | `app_field_moved_once` | handle field passed by value once | builds + prints `/ mounted` |
//! | `app_field_moved_twice` | handle field passed by value twice | fail-closed IPE-L0135 |
//! | `decoded_fn_called_twice` | mapper payload `f 1 + f 2` | fail-closed IPE-L0127 |
//! | `decoded_fn_captured` | mapper payload captured by an inner lambda | fail-closed IPE-L0127 |
//! | `decoded_fn_called_once` | mapper payload called once per branch | builds + prints `11` |
//! | `seq_closure_and_field_read` | statement closure + field reads, reused in rest | builds + prints three lines |
//! | `row_param_seq_reuse` | row param read by a statement kernel, reused in rest | builds + prints `ADA`, `Ada!` |
//! | `row_list_signature_seq_reuse` | `List { r \| name : String }` param, same reuse | fail-closed IPE-L0131 |
//! | `app_field_in_recallable_closure` | handle field read inside a re-callable closure | fail-closed IPE-L0126 |
//! | `row_fun_field_signature` | row param with a function field | fail-closed IPE-L0135 |
//! | `row_maybe_fun_field_signature` | row param with a `Maybe` function field | fail-closed IPE-L0135 |
//! | `fn_field_moved_then_record_read` | `Maybe` function field moved, then whole record read | fail-closed IPE-L0127 |
//! | `fun_field_applied_then_record_read` | function field called in place, then whole record read | builds + prints `13` |
//! | `maybe_handler_field_in_closure` | `Maybe` server-handler field read in a closure | builds + prints `2` |
//! | `eta_partial_arg_twice_in_run_once_slot` | `Task.andThen (both site site)`, handle record captured twice | fail-closed IPE-L0126 |
//! | `eta_partial_arg_once_in_run_once_slot` | `Task.andThen (one site)`, handle record captured once | builds + prints `> / mounted` |
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

/// Shared prelude for the app-handle fixtures: a record holding a `Web.WebApp` handle.
///
/// The handle has no `Clone` impl and is no effect carrier; `describe` takes
/// one by value and never runs it.
const APP_HANDLE_PRELUDE: &str = r#"module Main exposing (main)

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


describe : String -> Web.WebApp -> String
describe name _app =
    name ++ " mounted"


main : Task Error ()
main =
    serve { app = embedded, name = "/" }
"#;

/// The statement's kernel argument reads `site.name` and the rest reads `site`
/// again, so the sequencing rewrite would clone `site`.
const APP_HANDLE_RECORD_SEQ_REUSE: &str = r"
serve : Site -> Task Error ()
serve site =
    do
        Io.println site.name
        Io.println site.name
";

/// The handle field is moved out of `site` once, after a read of `site.name`.
///
/// Prints `/ mounted`.
const APP_FIELD_MOVED_ONCE: &str = r"
serve : Site -> Task Error ()
serve site =
    Io.println (describe site.name site.app)
";

/// The handle field is moved out of `site`, then read again.
const APP_FIELD_MOVED_TWICE: &str = r#"
serve : Site -> Task Error ()
serve site =
    Io.println (describe site.name site.app ++ describe "again" site.app)
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

/// A row-polymorphic record read by a statement kernel call, then reused.
///
/// The statement reads `p.name` through its witness getter and the rest reads
/// `p` again, so the sequencing rewrite clones `p` on the `R: Clone` bound.
/// Prints `ADA`, `Ada!`.
const ROW_PARAM_SEQ_REUSE: &str = r#"module Main exposing (main)

import Ipe.Io as Io
import Ipe.String as String
import Ipe.Task as Task exposing (Task)


greet : { r | name : String } -> Task Error ()
greet p =
    do
        Io.println (String.toUpper p.name)
        Io.println (String.append p.name "!")


main : Task Error ()
main =
    greet { name = "Ada", age = 3 }
"#;

/// A `List` of an open row reused across a `do` block after a kernel read.
///
/// An open row nested under a container has no emission, so the signature is
/// refused before the body's clone decisions run.
const ROW_LIST_SIGNATURE_SEQ_REUSE: &str = r#"module Main exposing (main)

import Ipe.Io as Io
import Ipe.List as List
import Ipe.String as String
import Ipe.Task as Task exposing (Task)


names : List { r | name : String } -> Task Error ()
names xs =
    do
        Io.println (String.fromInt (List.length xs))
        Io.println (String.concat (List.map (\p -> p.name) xs))


main : Task Error ()
main =
    names [ { name = "Ada", age = 3 }, { name = "Bob", age = 4 } ]
"#;

#[test]
fn app_handle_record_seq_reuse_fails_closed() {
    assert_rejected(
        "app_handle_record_seq_reuse",
        &format!("{APP_HANDLE_PRELUDE}{APP_HANDLE_RECORD_SEQ_REUSE}"),
        ipe_diagnostics::IPE_L0135,
    );
}

#[test]
fn app_field_moved_once_round_trips() {
    let Some(emitted) = assert_accepted(
        "app_field_moved_once",
        &format!("{APP_HANDLE_PRELUDE}{APP_FIELD_MOVED_ONCE}"),
        "/ mounted",
    ) else {
        return;
    };
    assert!(
        !emitted.contains(".app.clone()"),
        "a move-only field read must move the field, never clone it; emitted:\n{emitted}"
    );
}

#[test]
fn app_field_moved_twice_fails_closed() {
    assert_rejected(
        "app_field_moved_twice",
        &format!("{APP_HANDLE_PRELUDE}{APP_FIELD_MOVED_TWICE}"),
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

#[test]
fn row_param_seq_reuse_round_trips() {
    let _ = assert_accepted("row_param_seq_reuse", ROW_PARAM_SEQ_REUSE, "ADA\nAda!");
}

#[test]
fn row_list_signature_seq_reuse_fails_closed() {
    assert_rejected(
        "row_list_signature_seq_reuse",
        ROW_LIST_SIGNATURE_SEQ_REUSE,
        ipe_diagnostics::IPE_L0131,
    );
}

/// A handle field read inside a re-callable closure argument.
///
/// The closure holds its capture of `site` by reference on each call, so
/// moving the handle field out of it has no sound emission.
const APP_FIELD_IN_RECALLABLE_CLOSURE: &str = r#"
twice : (String -> String) -> String
twice f =
    f "a" ++ f "b"


serve : Site -> Task Error ()
serve site =
    Io.println (twice (\s -> describe s site.app))
"#;

/// A row parameter whose known field is a function (`Box<dyn Fn>`, no `Clone`).
///
/// The witness getter only borrows the field, so the signature is refused.
const ROW_FUN_FIELD_SIGNATURE: &str = r#"module Main exposing (main)

import Ipe.Io as Io


nameOf : { r | name : String, step : Int -> Int } -> String
nameOf p =
    p.name


main =
    Io.println (nameOf { name = "Ada", step = \x -> x + 1 })
"#;

/// A row parameter whose known field is a `Maybe` over a function.
///
/// The payload is a `Box<dyn Fn>`, so the field has no `Clone` either.
const ROW_MAYBE_FUN_FIELD_SIGNATURE: &str = r#"module Main exposing (main)

import Ipe.Io as Io


nameOf : { r | name : String, step : Maybe (Int -> Int) } -> String
nameOf p =
    p.name


main =
    Io.println (nameOf { name = "Ada", step = Just (\x -> x + 1) })
"#;

/// A record carrying a `Maybe` over a function has no `Clone`.
///
/// Its function field is moved out, then the whole record is read again.
const FN_FIELD_MOVED_THEN_RECORD_READ: &str = r"module Main exposing (main)

import Ipe.Io as Io
import Ipe.String as String


type alias Holder =
    { pick : Maybe (Int -> Int), n : Int }


applyOr : Maybe (Int -> Int) -> Int
applyOr m =
    case m of
        Just g ->
            g 1

        Nothing ->
            0


size : Holder -> Int
size h =
    h.n


run : Holder -> Int
run h =
    applyOr h.pick + size h


main =
    Io.println (String.fromInt (run { pick = Just (\x -> x + 1), n = 2 }))
";

/// A function field called in place, then the whole record read.
///
/// A record-direct function field takes the `Arc` carrier, so the record
/// clones. Prints `13`.
const FUN_FIELD_APPLIED_THEN_RECORD_READ: &str = r"module Main exposing (main)

import Ipe.Io as Io
import Ipe.String as String


type alias Counter =
    { step : Int -> Int, n : Int }


total : Counter -> Int
total c =
    c.n


run : Counter -> Int
run c =
    c.step 1 + total c


main =
    Io.println (String.fromInt (run { step = \x -> x + 10, n = 2 }))
";

/// A `Maybe` over a server-handler field read inside a re-callable closure.
///
/// A server handler renders as `Arc<dyn Fn>`, so the `Maybe` clones and the
/// closure reads a clone of the field. Prints `2`.
const MAYBE_HANDLER_FIELD_IN_CLOSURE: &str = r#"module Main exposing (main)

import Ipe.Http.Server as Server
import Ipe.Io as Io
import Ipe.List as List
import Ipe.String as String
import Ipe.Task as Task exposing (Task)


type alias Routes =
    { fallback : Maybe (Server.Request -> Task Error Server.Response), name : String }


handle : Server.Request -> Task Error Server.Response
handle _req =
    Task.succeed (Server.text "ok")


hasFallback : Maybe (Server.Request -> Task Error Server.Response) -> Bool
hasFallback m =
    case m of
        Just _ ->
            True

        Nothing ->
            False


countFallbacks : Routes -> List Int -> Int
countFallbacks r xs =
    List.length (List.filter (\_ -> hasFallback r.fallback) xs)


main : Task Error ()
main =
    Io.println (String.fromInt (countFallbacks { fallback = Just handle, name = "x" } [ 1, 2 ]))
"#;

/// A partial application passing the handle record twice, in `Task.andThen`'s run-once slot.
///
/// The eta-expanded closure moves its one capture of `site` into both
/// arguments, so it has no sound emission.
const ETA_PARTIAL_ARG_TWICE_IN_RUN_ONCE_SLOT: &str = r#"
both : Site -> Site -> String -> Task Error ()
both a b greeting =
    Io.println (greeting ++ describe a.name a.app ++ describe b.name b.app)


serve : Site -> Task Error ()
serve site =
    Task.andThen (both site site) (Task.succeed "> ")
"#;

/// Over-rejection guard: the same slot moving the handle record once.
///
/// Prints `> / mounted`.
const ETA_PARTIAL_ARG_ONCE_IN_RUN_ONCE_SLOT: &str = r#"
one : Site -> String -> Task Error ()
one s greeting =
    Io.println (greeting ++ describe s.name s.app)


serve : Site -> Task Error ()
serve site =
    Task.andThen (one site) (Task.succeed "> ")
"#;

#[test]
fn app_field_in_recallable_closure_fails_closed() {
    assert_rejected(
        "app_field_in_recallable_closure",
        &format!("{APP_HANDLE_PRELUDE}{APP_FIELD_IN_RECALLABLE_CLOSURE}"),
        ipe_diagnostics::IPE_L0126,
    );
}

#[test]
fn row_fun_field_signature_fails_closed() {
    assert_rejected(
        "row_fun_field_signature",
        ROW_FUN_FIELD_SIGNATURE,
        ipe_diagnostics::IPE_L0135,
    );
}

#[test]
fn row_maybe_fun_field_signature_fails_closed() {
    assert_rejected(
        "row_maybe_fun_field_signature",
        ROW_MAYBE_FUN_FIELD_SIGNATURE,
        ipe_diagnostics::IPE_L0135,
    );
}

#[test]
fn fn_field_moved_then_record_read_fails_closed() {
    assert_rejected(
        "fn_field_moved_then_record_read",
        FN_FIELD_MOVED_THEN_RECORD_READ,
        ipe_diagnostics::IPE_L0127,
    );
}

#[test]
fn fun_field_applied_then_record_read_round_trips() {
    let _ = assert_accepted(
        "fun_field_applied_then_record_read",
        FUN_FIELD_APPLIED_THEN_RECORD_READ,
        "13",
    );
}

#[test]
fn maybe_handler_field_in_closure_round_trips() {
    let _ = assert_accepted(
        "maybe_handler_field_in_closure",
        MAYBE_HANDLER_FIELD_IN_CLOSURE,
        "2",
    );
}

#[test]
fn eta_partial_arg_twice_in_run_once_slot_fails_closed() {
    assert_rejected(
        "eta_partial_arg_twice_in_run_once_slot",
        &format!("{APP_HANDLE_PRELUDE}{ETA_PARTIAL_ARG_TWICE_IN_RUN_ONCE_SLOT}"),
        ipe_diagnostics::IPE_L0126,
    );
}

#[test]
fn eta_partial_arg_once_in_run_once_slot_round_trips() {
    let _ = assert_accepted(
        "eta_partial_arg_once_in_run_once_slot",
        &format!("{APP_HANDLE_PRELUDE}{ETA_PARTIAL_ARG_ONCE_IN_RUN_ONCE_SLOT}"),
        "> / mounted",
    );
}

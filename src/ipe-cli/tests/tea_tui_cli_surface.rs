//! Compile-time surface tests for `Ipe.App.Tea.Tui` and `Ipe.App.Tea.Cli`.
//!
//! `Ipe.App.Tea.Tui` exposes the full-screen terminal TEA entry via `Tui.app`;
//! `Ipe.App.Tea.Cli` exposes the line-oriented entry via `Cli.app`. Both `app`
//! entries are registered in the `env.rs` qualifier catalog and carry
//! `KernelClass::Terminal` (the one terminal rendering family).
//!
//! These tests are COMPILE-ONLY (the `ipe` pipeline writes the emitted project
//! but `cargo` is never invoked), so they run in CI without `IPE_E2E`.

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

fn compile(test_name: &str, source: &str) -> Result<Result<(), ipe::CliError>, BoxError> {
    let ipe_dir = std::env::temp_dir().join(format!("tea_surface_{test_name}_ipe"));
    let _ = std::fs::remove_dir_all(&ipe_dir);
    std::fs::create_dir_all(&ipe_dir)?;
    let entry = ipe_dir.join("Main.ipe");
    std::fs::write(&entry, source)?;

    let out_dir = std::env::temp_dir().join(format!("tea_surface_{test_name}_out"));
    let _ = std::fs::remove_dir_all(&out_dir);

    let runtime = ipe::resolve_runtime().map_err(|e| -> BoxError { format!("{e:?}").into() })?;
    Ok(ipe::build(&entry, &out_dir, &runtime))
}

fn assert_accepted(test_name: &str, source: &str) -> Result<(), BoxError> {
    match compile(test_name, source)? {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("{test_name}: expected ipe success, got {e:?}").into()),
    }
}

/// Assert `source` is REJECTED by the pipeline with exactly `expected` wire
/// code (never exit-0 — a wrong code or an accept both fail). Pins the SPECIFIC
/// rejection reason so a right-outcome-wrong-reason regression is caught.
fn assert_rejected_code(test_name: &str, source: &str, expected: &str) -> Result<(), BoxError> {
    match compile(test_name, source)? {
        Ok(()) => {
            Err(format!("{test_name}: expected rejection {expected}, but ipe accepted").into())
        }
        Err(ipe::CliError::Pipeline { diag, .. }) => {
            let got = diag.code().as_str();
            if got == expected {
                Ok(())
            } else {
                Err(format!(
                    "{test_name}: expected {expected}, got {got} — rejected for the WRONG reason"
                )
                .into())
            }
        }
        Err(other) => {
            Err(format!("{test_name}: expected pipeline rejection, got {other:?}").into())
        }
    }
}

/// Assert `source` is REJECTED (any pipeline diagnostic — never exit-0). Used
/// for the fail-closed ambiguity property, where the load-bearing guarantee is
/// that an under-determined view engine is turned away rather than silently
/// defaulted; the exact stage that turns it away is not the property under test.
fn assert_rejected_any(test_name: &str, source: &str) -> Result<(), BoxError> {
    match compile(test_name, source)? {
        Ok(()) => Err(format!(
            "{test_name}: an unconstrained view engine was ACCEPTED — it must be \
             rejected fail-closed, never silently defaulted to Web"
        )
        .into()),
        Err(ipe::CliError::Pipeline { .. }) => Ok(()),
        Err(other) => {
            Err(format!("{test_name}: expected pipeline rejection, got {other:?}").into())
        }
    }
}

/// Minimal `Tui.app` program — `import Ipe.App.Tea.Tui as Tui` then `Tui.app { ... }`.
const TUI_APP: &str = r#"module Main exposing (main)

import Ipe.App.Tea.Tui as Tui
import Ipe.Ui.Cells as Cells
import Ipe.Ui.Cells exposing (Screen)
import Ipe.App.Tea.Tui.Cmd
import Ipe.App.Tea.Tui.Sub

type Msg = NoOp

type alias Model = { count : Int }

type alias KeyEvent = { kind : String, value : String }

init : () -> ( Model, Cmd Msg )
init _unit =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update _msg model =
    ( model, Cmd.none )

view : Model -> Screen Msg
view _model =
    Cells.text "hello"

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

onKey : KeyEvent -> Msg
onKey _event =
    NoOp

main =
    Tui.app
        { init = init, update = update, view = view
        , subscriptions = subscriptions, onKey = onKey
        }
"#;

/// Minimal `Cli.app` program — `import Ipe.App.Tea.Cli as Cli` then `Cli.app { ... }`.
const CLI_APP: &str = r#"module Main exposing (main)

import Ipe.App.Tea.Cli as Cli
import Ipe.App.Tea.Cli.Cmd
import Ipe.App.Tea.Cli.Sub
import Ipe.Ui.Cli as Ui
import Ipe.Ui.Cli exposing (Lines)

type Msg = Line String | NoOp

type alias Model = { lines : List String }

init : () -> ( Model, Cmd Msg )
init _unit =
    ( { lines = [] }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Line s ->
            ( { model | lines = model.lines ++ [ s ] }, Cmd.none )
        NoOp ->
            ( model, Cmd.none )

view : Model -> Lines Msg
view _model =
    Ui.text "ok"

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

onLine : String -> Msg
onLine s =
    Line s

main =
    Cli.app
        { init = init, update = update, view = view
        , subscriptions = subscriptions, onLine = onLine
        }
"#;

/// A DOM view (`View Web msg`) whose container holds a terminal-cells node
/// (`View Tui msg`). The two engines are distinct nullary tags on the one
/// `View engine msg` carrier, so mixing them in one view tree fails
/// unification — a cross-engine view is unrepresentable, not a silent render.
const CROSS_ENGINE_VIEW: &str = r#"module Main exposing (main)

import Ipe.App.Tea.Web as Web
import Ipe.App.Tea.Web.Cmd as Cmd
import Ipe.App.Tea.Web.Sub as Sub
import Ipe.Ui as Ui
import Ipe.Ui.Cells as Cells

type Msg = NoOp

type alias Model = { count : Int }

init : WebReq -> ( Model, Cmd.Cmd Msg )
init _req =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd.Cmd Msg )
update _msg model =
    ( model, Cmd.none )

subscriptions : Model -> Sub.Sub Msg
subscriptions _model =
    Sub.none

-- A DOM column (`View Web Msg`) whose child is a terminal-cells node
-- (`View Tui Msg`): the engines differ, so this cannot unify.
view : Model -> View Web Msg
view _model =
    Ui.column [] [ Cells.text "nope" ]

main =
    Web.app
        { init = init, update = update, view = view
        , subscriptions = subscriptions, routes = [], notFound = NoOp
        }
"#;

/// A `View` over a tag outside the closed `{Web, Tui, Cli}` engine set. The
/// tag names no view engine, so it is rejected fail-closed at canon — never a
/// flexible variable that defers the failure downstream.
const NON_ENGINE_VIEW: &str = r"module Main exposing (v)

v : View Foo Msg
v = v
";

/// The deleted generic `Ipe.Tea.app` entry: a `main = Tea.app { … }` over an
/// `import Ipe.App.Tea` must be REJECTED at name resolution. `Program Web msg`
/// comes only from the per-engine `Web.app`; the generic entry (with its
/// under-determined-engine footgun) no longer exists, so `Tea.app` names no
/// kernel and there is no representable path to the Web sandbox through it.
const DELETED_TEA_APP: &str = r#"module Main exposing (main)

import Ipe.App.Tea as Tea
import Ipe.App.Tea.Web.Cmd as Cmd
import Ipe.App.Tea.Web.Sub as Sub
import Ipe.Ui as Ui

type Msg = NoOp

type alias Model = { count : Int }

init : WebReq -> ( Model, Cmd.Cmd Msg )
init _req =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd.Cmd Msg )
update _msg model =
    ( model, Cmd.none )

subscriptions : Model -> Sub.Sub Msg
subscriptions _model =
    Sub.none

view : Model -> View Web Msg
view _model =
    Ui.text "counter"

main =
    Tea.app
        { init = init, update = update, view = view
        , subscriptions = subscriptions
        }
"#;

/// `Tui.app` is the full-screen terminal entry kernel registered in
/// the `env.rs` qualifier catalog. A program using
/// `import Ipe.App.Tea.Tui as Tui` and `Tui.app { ... }` must compile (ipe-0).
#[test]
fn tui_app_surface_compiles() -> Result<(), BoxError> {
    assert_accepted("tui_app", TUI_APP)
}

/// A cross-engine view (a `View Tui Msg` node inside a `View Web Msg` tree)
/// fails unification: the engine tags are distinct, so
/// make-invalid-states-unrepresentable turns the mix into an IPE-T0001 type
/// mismatch rather than a silent wrong-engine render.
#[test]
fn cross_engine_view_fails_unification() -> Result<(), BoxError> {
    assert_rejected_code("cross_engine_view", CROSS_ENGINE_VIEW, "IPE-T0001")
}

/// A `View` over a non-engine tag (`View Foo Msg`) is rejected fail-closed as
/// an unknown type name (IPE-N0002) — the engine set is CLOSED, so an
/// unrecognised tag has no view denotation and never becomes a flexible var.
#[test]
fn non_engine_view_tag_is_rejected() -> Result<(), BoxError> {
    assert_rejected_code("non_engine_view", NON_ENGINE_VIEW, "IPE-N0002")
}

/// The generic `Ipe.Tea.app` entry is DELETED: `main = Tea.app { … }` must be
/// rejected at name resolution (the surface names no kernel), so no program can
/// reach the Web sandbox through the deleted generic entry — prove-the-refusal
/// for the collapse. Only the per-engine `Web.app` yields `Program Web msg`.
#[test]
fn deleted_generic_tea_app_is_rejected() -> Result<(), BoxError> {
    assert_rejected_any("deleted_tea_app", DELETED_TEA_APP)
}

/// `Cli.app` is the line-oriented terminal entry kernel registered in
/// the `env.rs` qualifier catalog. A program using
/// `import Ipe.App.Tea.Cli as Cli` and `Cli.app { ... }` must compile (ipe-0).
#[test]
fn cli_app_surface_compiles() -> Result<(), BoxError> {
    assert_accepted("cli_app", CLI_APP)
}

/// `main = Server.listen …` yields the uniform `Program Direct ()` carrier and
/// the whole pipeline accepts (ipe-0). Acceptance is the SEAL witness: the
/// `Program Direct ()` scheme + its erase-only lower to the listener's `Task ()`
/// IR must both succeed, so a carrier that failed to erase would fail here.
const SERVER_LISTEN_DIRECT: &str = r#"module Main exposing (main)

import Ipe.Http.Server as Server
import Ipe.Task as Task

main =
    Server.listen 8080
        [ Server.get "/" (\_req -> Task.succeed (Server.text "hello")) ]
"#;

#[test]
fn server_listen_infers_program_direct() -> Result<(), BoxError> {
    assert_accepted("server_listen_direct", SERVER_LISTEN_DIRECT)
}

/// `main = Script.program (…)` yields the uniform `Program Direct ()` carrier
/// and the whole pipeline accepts (ipe-0). The erase-only wrapper lowers to the
/// wrapped task's `Task ()` IR, so a script's emit is unchanged.
const SCRIPT_PROGRAM_DIRECT: &str = r#"module Main exposing (main)

import Ipe.App.Script as Script
import Ipe.Io as Io

main =
    Script.program (Io.println "hello")
"#;

#[test]
fn script_program_infers_program_direct() -> Result<(), BoxError> {
    assert_accepted("script_program_direct", SCRIPT_PROGRAM_DIRECT)
}

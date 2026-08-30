//! The `Ui.cells` shape-admissibility gate (IPE-L0132).
//!
//! `Ui.cells : List (List Char) -> Element msg` paints a raw terminal cell grid
//! and has no browser denotation. Its runtime helper degrades to plain text on a
//! non-terminal backend, so a `Web` / `WebView` program using it would `ipe`-
//! succeed and then silently render the wrong thing. The backend gate converts
//! that into a fail-closed `IPE-L0132` diagnostic the moment `Ui.cells` is
//! emitted under a web-family shape (SECURITY — fail closed, never a `cargo`
//! failure and never a panic).
//!
//! These tests are COMPILE-ONLY (they run the `ipe` pipeline + write the
//! project, but never invoke `cargo`), so they are fast and NOT gated on
//! `IPE_E2E`.

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Compile `source` through `ipe::build`, returning the pipeline result. The
/// emitted project is written to a per-test temp dir; `cargo` is never invoked.
fn compile(test_name: &str, source: &str) -> Result<Result<(), ipe::CliError>, BoxError> {
    let ipe_dir = std::env::temp_dir().join(format!("ui_cells_gate_{test_name}_ipe"));
    let _ = std::fs::remove_dir_all(&ipe_dir);
    std::fs::create_dir_all(&ipe_dir)?;
    let entry = ipe_dir.join("Main.ipe");
    std::fs::write(&entry, source)?;

    let out_dir = std::env::temp_dir().join(format!("ui_cells_gate_{test_name}_out"));
    let _ = std::fs::remove_dir_all(&out_dir);

    let runtime = ipe::resolve_runtime().map_err(|e| -> BoxError { format!("{e:?}").into() })?;
    Ok(ipe::build(&entry, &out_dir, &runtime))
}

/// Assert compilation failed with the given diagnostic code (fail-closed, an
/// `ipe`-time diagnostic — never a `cargo` failure or a panic).
fn assert_rejected_with(
    test_name: &str,
    source: &str,
    expected_code: &str,
) -> Result<(), BoxError> {
    match compile(test_name, source)? {
        Ok(()) => Err(format!("{test_name}: expected {expected_code}, but ipec succeeded").into()),
        Err(ipe::CliError::Pipeline { diag, .. }) => {
            assert_eq!(
                diag.code().as_str(),
                expected_code,
                "{test_name}: wrong diagnostic code"
            );
            Ok(())
        }
        Err(other) => Err(format!("{test_name}: expected {expected_code}, got {other:?}").into()),
    }
}

/// Assert compilation succeeded (ipe-0).
fn assert_accepted(test_name: &str, source: &str) -> Result<(), BoxError> {
    match compile(test_name, source)? {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("{test_name}: expected ipec success, got {e:?}").into()),
    }
}

/// `Ui.cells` inside a `Web.app` view — must be rejected with IPE-L0132.
const WEB_UI_CELLS: &str = r"module Main exposing (main)

import Ipe.Tea.Web as Web
import Ipe.Ui as Ui
import Ipe.Tea.Web.Cmd
import Ipe.Tea.Web.Sub

type Msg = Tick

type alias Model = { count : Int }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Tick ->
            ( { model | count = model.count + 1 }, Cmd.none )

view : Model -> Element Msg
view _model =
    Ui.cells [ [ 'h', 'i' ] ]

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Web.app
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [], notFound = Tick
        }
";

/// `Ui.cells` inside a `Webview.app` view — must be rejected with IPE-L0132.
const WEBVIEW_UI_CELLS: &str = r#"module Main exposing (main)

import Ipe.Tea.WebView as Webview
import Ipe.Ui as Ui
import Ipe.Tea.WebView.Cmd as Cmd
import Ipe.Tea.WebView.Sub as Sub

type Msg = Increment

type alias Model = { count : Int }

init : () -> ( Model, Cmd Msg )
init _unit =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )

view : Model -> Element Msg
view _model =
    Ui.cells [ [ 'o', 'k' ] ]

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Webview.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , window = { title = "Cells", size = ( 400, 300 ) }
        }
"#;

/// `Ui.cells` inside a `Terminal.appScreen` view — must be ACCEPTED (the raw
/// cell grid is a terminal primitive; this is the shape it belongs to).
const TERMINAL_UI_CELLS: &str = r#"module Main exposing (main)

import Ipe.Tea.Terminal as Terminal
import Ipe.Ui as Ui
import Ipe.Tea.Terminal.Cmd
import Ipe.Tea.Terminal.Sub

type Msg = NoOp

type alias Model = { count : Int }

init : () -> ( Model, Cmd Msg )
init _unit =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        NoOp ->
            ( model, Cmd.none )

view : Model -> Element Msg
view _model =
    Ui.column []
        [ Ui.text "grid:"
        , Ui.cells [ [ '4', '8' ], [ '6', '9' ] ]
        ]

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

type alias KeyEvent = { kind : String, value : String }

onKey : KeyEvent -> Msg
onKey _event =
    NoOp

main =
    Terminal.appScreen
        { init = init, update = update, view = view
        , subscriptions = subscriptions, onKey = onKey
        }
"#;

/// A `Web.app` view painting `Ui.cells` is a terminal-only node in a web-family
/// build: rejected fail-closed with IPE-L0132, not a cargo failure or a panic.
#[test]
fn web_view_with_ui_cells_is_rejected() -> Result<(), BoxError> {
    assert_rejected_with("web_ui_cells", WEB_UI_CELLS, "IPE-L0132")
}

/// A `Webview.app` view painting `Ui.cells` is likewise rejected with IPE-L0132.
#[test]
fn webview_view_with_ui_cells_is_rejected() -> Result<(), BoxError> {
    assert_rejected_with("webview_ui_cells", WEBVIEW_UI_CELLS, "IPE-L0132")
}

/// Non-regression control: `Ui.cells` under `Terminal.appScreen` is the shape
/// it belongs to and must compile cleanly (ipe-0).
#[test]
fn terminal_view_with_ui_cells_is_accepted() -> Result<(), BoxError> {
    assert_accepted("terminal_ui_cells", TERMINAL_UI_CELLS)
}

/// `Ui.cells` inside a `Terminal.appLines` (Cli shape) view — must be rejected
/// with IPE-L0153. A Cli view returns `String`; a character grid has no string
/// denotation.
const CLI_UI_CELLS: &str = r#"module Main exposing (main)

import Ipe.Tea.Terminal as Terminal
import Ipe.Ui as Ui

type Msg = NoOp

type alias Model = { count : Int }

init : () -> ( Model, Cmd Msg )
init _unit =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        NoOp ->
            ( model, Cmd.none )

view : Model -> String
view _model =
    Ui.cells [ [ 'h', 'i' ] ]

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

onLine : String -> Msg
onLine _line =
    NoOp

main =
    Terminal.appLines
        { init = init, update = update, view = view
        , subscriptions = subscriptions, onLine = onLine
        }
"#;

/// `Ui.cells` in a `Terminal.appLines` view is rejected with IPE-L0153.
#[test]
fn cli_view_with_ui_cells_is_rejected() -> Result<(), BoxError> {
    assert_rejected_with("cli_ui_cells", CLI_UI_CELLS, "IPE-L0153")
}

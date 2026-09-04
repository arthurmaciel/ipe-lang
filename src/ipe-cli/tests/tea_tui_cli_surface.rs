//! Compile-time surface tests for `Ipe.Tea.Tui` and `Ipe.Tea.Cli`.
//!
//! `Ipe.Tea.Tui` exposes the full-screen terminal TEA entry via `Tui.app`;
//! `Ipe.Tea.Cli` exposes the line-oriented entry via `Cli.app`. Both `app`
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

/// Minimal `Tui.app` program — `import Ipe.Tea.Tui as Tui` then `Tui.app { ... }`.
const TUI_APP: &str = r#"module Main exposing (main)

import Ipe.Tea.Tui as Tui
import Ipe.Ui.Cells as Cells
import Ipe.Ui.Cells exposing (Screen)
import Ipe.Tea.Tui.Cmd
import Ipe.Tea.Tui.Sub

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

/// Minimal `Cli.app` program — `import Ipe.Tea.Cli as Cli` then `Cli.app { ... }`.
const CLI_APP: &str = r#"module Main exposing (main)

import Ipe.Tea.Cli as Cli
import Ipe.Tea.Cli.Cmd
import Ipe.Tea.Cli.Sub

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

view : Model -> String
view _model =
    "ok"

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

/// `Tui.app` is the full-screen terminal entry kernel registered in
/// the `env.rs` qualifier catalog. A program using
/// `import Ipe.Tea.Tui as Tui` and `Tui.app { ... }` must compile (ipe-0).
#[test]
fn tui_app_surface_compiles() -> Result<(), BoxError> {
    assert_accepted("tui_app", TUI_APP)
}

/// `Cli.app` is the line-oriented terminal entry kernel registered in
/// the `env.rs` qualifier catalog. A program using
/// `import Ipe.Tea.Cli as Cli` and `Cli.app { ... }` must compile (ipe-0).
#[test]
fn cli_app_surface_compiles() -> Result<(), BoxError> {
    assert_accepted("cli_app", CLI_APP)
}

//! The `Ipe.Web` / `Ipe.Tui` / `Ipe.WebView` app-entry
//! Model-admissibility gate.
//!
//! `web_app` bounds its Model `serde::Serialize + serde::de::DeserializeOwned +
//! Clone + PartialEq`; `tui_app` / `webview_app` bound it `Clone`. Before the
//! gate, a Model storing a non-admissible value (`Cmd` / `Sub` / `Task` /
//! `Decoder` / `Db` / a function — or, for Web only, `Html` / `Element` /
//! `Color`) made `ipe` exit 0 and then `cargo build` fail on the missing trait
//! bound. The gate converts that into a fail-closed `IPE-L0120` diagnostic.
//!
//! These tests are COMPILE-ONLY (they run the `ipe` pipeline + write the
//! project, but never invoke `cargo`), so they are fast and NOT gated on
//! `IPE_E2E`.

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Compile `source` through `ipe::build`, returning the pipeline result. The
/// emitted project is written to a per-test temp dir; `cargo` is never invoked.
fn compile(test_name: &str, source: &str) -> Result<Result<(), ipe::CliError>, BoxError> {
    let ipe_dir = std::env::temp_dir().join(format!("model_adm_{test_name}_ipe"));
    let _ = std::fs::remove_dir_all(&ipe_dir);
    std::fs::create_dir_all(&ipe_dir)?;
    let entry = ipe_dir.join("Main.ipe");
    std::fs::write(&entry, source)?;

    let out_dir = std::env::temp_dir().join(format!("model_adm_{test_name}_out"));
    let _ = std::fs::remove_dir_all(&out_dir);

    let runtime = ipe::resolve_runtime().map_err(|e| -> BoxError { format!("{e:?}").into() })?;
    Ok(ipe::build(&entry, &out_dir, &runtime))
}

/// Assert compilation failed with the given diagnostic code.
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

const LIVE_GOOD: &str = r"module Main exposing (main)

import Ipe.Web as Web
import Ipe.Ui as Ui

type Msg = Increment

type alias Model = { count : Int }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )

view : Model -> Html Msg
view model =
    Ui.layout [] (Ui.text (String.fromInt model.count))

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Web.app
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [], notFound = Increment
        }
";

const LIVE_CMD_MODEL: &str = r"module Main exposing (main)

import Ipe.Web as Web
import Ipe.Ui as Ui

type Msg = Tick

type alias Model = { count : Int, pending : Cmd Msg }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { count = 0, pending = Cmd.none }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Tick ->
            ( { model | count = model.count + 1 }, Cmd.none )

view : Model -> Html Msg
view model =
    Ui.layout [] (Ui.text (String.fromInt model.count))

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Web.app
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [], notFound = Tick
        }
";

const LIVE_HTML_MODEL: &str = r#"module Main exposing (main)

import Ipe.Web as Web
import Ipe.Ui as Ui

type Msg = Tick

type alias Model = { count : Int, cached : Html Msg }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { count = 0, cached = Ui.layout [] (Ui.text "x") }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Tick ->
            ( { model | count = model.count + 1 }, Cmd.none )

view : Model -> Html Msg
view model =
    Ui.layout [] (Ui.text (String.fromInt model.count))

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Web.app
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [], notFound = Tick
        }
"#;

// A `Secret` Model field must be rejected for `Ipe.Web` — a
// `Secret` must NEVER round-trip through the session store. `Secret` is
// NON-serde by design (`ir_type_is_serde(Secret) = false`), so this is the
// SAME mechanism as `LIVE_CMD_MODEL` / `LIVE_HTML_MODEL` above, not a new gate.
const LIVE_SECRET_MODEL: &str = r#"module Main exposing (main)

import Ipe.Web as Web
import Ipe.Ui as Ui

type Msg = Tick

type alias Model = { count : Int, apiKey : Secret }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { count = 0, apiKey = Secret.fromString "sk_live_x" }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Tick ->
            ( { model | count = model.count + 1 }, Cmd.none )

view : Model -> Html Msg
view model =
    Ui.layout [] (Ui.text (String.fromInt model.count))

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Web.app
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [], notFound = Tick
        }
"#;

const TUI_GOOD: &str = r"module Main exposing (main)

import Ipe.Tui as Tui
import Ipe.Ui as Ui

type Msg = Increment | NoOp

type alias Model = { count : Int }

init : () -> ( Model, Cmd Msg )
init _unit =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )
        NoOp ->
            ( model, Cmd.none )

view : Model -> Element Msg
view model =
    Ui.column [] [ Ui.el [] (Ui.text (String.fromInt model.count)) ]

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

type alias KeyEvent = { kind : String, value : String }

onKey : KeyEvent -> Msg
onKey _event =
    NoOp

main =
    Tui.app
        { init = init, update = update, view = view
        , subscriptions = subscriptions, onKey = onKey
        }
";

const TUI_CMD_MODEL: &str = r"module Main exposing (main)

import Ipe.Tui as Tui
import Ipe.Ui as Ui

type Msg = Increment | NoOp

type alias Model = { count : Int, pending : Cmd Msg }

init : () -> ( Model, Cmd Msg )
init _unit =
    ( { count = 0, pending = Cmd.none }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )
        NoOp ->
            ( model, Cmd.none )

view : Model -> Element Msg
view model =
    Ui.column [] [ Ui.el [] (Ui.text (String.fromInt model.count)) ]

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

type alias KeyEvent = { kind : String, value : String }

onKey : KeyEvent -> Msg
onKey _event =
    NoOp

main =
    Tui.app
        { init = init, update = update, view = view
        , subscriptions = subscriptions, onKey = onKey
        }
";

#[test]
fn live_plain_data_model_is_accepted() -> Result<(), BoxError> {
    assert_accepted("live_good", LIVE_GOOD)
}

#[test]
fn live_model_with_cmd_field_is_rejected() -> Result<(), BoxError> {
    assert_rejected_with("live_cmd", LIVE_CMD_MODEL, "IPE-L0120")
}

/// The CDPeq-but-not-serde case: `Html` is `Clone`/`PartialEq` but not `serde`,
/// so a `Ipe.Web` Model storing it is rejected (unlike Tui/Webview).
#[test]
fn live_model_with_html_field_is_rejected() -> Result<(), BoxError> {
    assert_rejected_with("live_html", LIVE_HTML_MODEL, "IPE-L0120")
}

/// `Secret` in a Web Model is a compile-time `IPE-L0120`, never
/// a session-store leak — see `LIVE_SECRET_MODEL`'s doc comment.
#[test]
fn live_model_with_secret_field_is_rejected() -> Result<(), BoxError> {
    assert_rejected_with("live_secret", LIVE_SECRET_MODEL, "IPE-L0120")
}

#[test]
fn tui_plain_data_model_is_accepted() -> Result<(), BoxError> {
    assert_accepted("tui_good", TUI_GOOD)
}

#[test]
fn tui_model_with_cmd_field_is_rejected() -> Result<(), BoxError> {
    assert_rejected_with("tui_cmd", TUI_CMD_MODEL, "IPE-L0120")
}

// ── lambda-`view` gate-bypass regressions ────────────────────────────────
//
// `model_ty_of_view` must match a lambda `view`, not ONLY `Expr::FuncValue`:
// matching only `FuncValue` returns `None` for an inline LAMBDA and the caller
// skips the gate (fail-open), letting an inadmissible Model behind a lambda
// `view` sail past the gate and `cargo`-fail on the missing serde bound.
// Routing the recovery through the Lambda-aware `fn_param_ty` closes the
// bypass. These fixtures pin both directions: the gate FIRES for a lambda
// `view` with a bad Model, and does NOT false-reject a lambda `view` with a
// plain Model.

/// Model has a `Cmd` field AND `view` is an inline lambda. A `FuncValue`-only
/// gate would skip this (ipe-0 then cargo-fail); the Lambda-aware gate rejects
/// it with IPE-L0120.
const LIVE_LAMBDA_VIEW_CMD_MODEL: &str = r"module Main exposing (main)

import Ipe.Web as Web
import Ipe.Ui as Ui

type Msg = Tick

type alias Model = { count : Int, pending : Cmd Msg }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { count = 0, pending = Cmd.none }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Tick ->
            ( { model | count = model.count + 1 }, Cmd.none )

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Web.app
        { init = init, update = update
        , view = \model -> Ui.layout [] (Ui.text (String.fromInt model.count))
        , subscriptions = subscriptions
        , routes = [], notFound = Tick
        }
";

/// Non-regression control: plain-data Model + lambda `view` must be
/// ACCEPTED — proves the Lambda arm recovers the Model without false-rejecting.
const LIVE_LAMBDA_VIEW_GOOD: &str = r"module Main exposing (main)

import Ipe.Web as Web
import Ipe.Ui as Ui

type Msg = Increment

type alias Model = { count : Int }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Web.app
        { init = init, update = update
        , view = \model -> Ui.layout [] (Ui.text (String.fromInt model.count))
        , subscriptions = subscriptions
        , routes = [], notFound = Increment
        }
";

#[test]
fn live_lambda_view_with_cmd_model_is_rejected() -> Result<(), BoxError> {
    assert_rejected_with("live_lambda_cmd", LIVE_LAMBDA_VIEW_CMD_MODEL, "IPE-L0120")
}

#[test]
fn live_lambda_view_with_plain_model_is_accepted() -> Result<(), BoxError> {
    assert_accepted("live_lambda_good", LIVE_LAMBDA_VIEW_GOOD)
}

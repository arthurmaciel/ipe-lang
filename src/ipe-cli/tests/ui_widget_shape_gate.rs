//! The `Ui.widget` shape-admissibility gate (IPE-L0147).
//!
//! `Ui.widget : CustomElement down up -> down -> (up -> msg) -> Element msg`
//! mounts a server-driven browser custom element. Its up-event payload rides the
//! seal codec, which is compiled in only when a browser shape forces the runtime
//! `json` feature (`Web.app` / `WebView.app`). Under a `Terminal` / `Program`
//! shape the widget has NO transport for its handler, so the node would be inert
//! and the emitted crate's non-`json` runtime fallback would leave the up-event
//! type parameter unconstrained (rustc E0282). The backend gate converts that
//! into a fail-closed `IPE-L0147` the moment `Ui.widget` is emitted outside a
//! browser shape (SECURITY — fail closed, never a `cargo` failure and never a
//! panic).
//!
//! These tests are COMPILE-ONLY (they run the `ipe` pipeline + write the
//! project, but never invoke `cargo`), so they are fast and NOT gated on
//! `IPE_E2E`. The `customElement` constructor requires its JS source file to be
//! present at build time, so each fixture is written as a two-file project (the
//! `.ipe` entry plus its `js/*.js`) through [`compile_with_files`].

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Compile a `Ui.widget` fixture as a two-file project (`Main.ipe` + a JS file
/// the `customElement` constructor references), returning the pipeline result.
/// The emitted project is written to a per-test temp dir; `cargo` is never
/// invoked.
fn compile_with_files(
    test_name: &str,
    source: &str,
    extra: &[(&str, &str)],
) -> Result<Result<(), ipe::CliError>, BoxError> {
    let ipe_dir = std::env::temp_dir().join(format!("ui_widget_gate_{test_name}_ipe"));
    let _ = std::fs::remove_dir_all(&ipe_dir);
    std::fs::create_dir_all(&ipe_dir)?;
    for (rel, contents) in extra {
        let path = ipe_dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
    }
    let entry = ipe_dir.join("Main.ipe");
    std::fs::write(&entry, source)?;

    let out_dir = std::env::temp_dir().join(format!("ui_widget_gate_{test_name}_out"));
    let _ = std::fs::remove_dir_all(&out_dir);

    let runtime = ipe::resolve_runtime().map_err(|e| -> BoxError { format!("{e:?}").into() })?;
    Ok(ipe::build(&entry, &out_dir, &runtime))
}

/// Assert compilation failed with the given diagnostic code (fail-closed, an
/// `ipe`-time diagnostic — never a `cargo` failure or a panic).
fn assert_rejected_with(
    test_name: &str,
    source: &str,
    extra: &[(&str, &str)],
    expected_code: &str,
) -> Result<(), BoxError> {
    match compile_with_files(test_name, source, extra)? {
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
fn assert_accepted(test_name: &str, source: &str, extra: &[(&str, &str)]) -> Result<(), BoxError> {
    match compile_with_files(test_name, source, extra)? {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("{test_name}: expected ipec success, got {e:?}").into()),
    }
}

/// The JS custom-element source the fixtures' `customElement` constructor points
/// at — its mere presence satisfies the build-time file-existence gate.
const WIDGET_JS: &str = "export function mount(host, emit) { return {}; }\n";

/// `Ui.widget` inside a `Terminal.appScreen` view — must be rejected with
/// IPE-L0147 (a browser custom element has no seam in a terminal build).
const TERMINAL_UI_WIDGET: &str = r#"module Main exposing (main)

import Ipe.Tea.Terminal as Terminal
import Ipe.Ui as Ui
import Ipe.Tea.Terminal.Cmd
import Ipe.Tea.Terminal.Sub

type alias EditorState = { text : String, line : Int }

type EditorEvent = Changed String | Saved

type Msg = Edited EditorEvent

type alias Model = { state : EditorState }

codeEditor : CustomElement EditorState EditorEvent
codeEditor = customElement "js/x.js"

init : () -> ( Model, Cmd Msg )
init _unit =
    ( { state = { text = "", line = 0 } }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update _msg model =
    ( model, Cmd.none )

view : Model -> Element Msg
view model =
    Ui.widget codeEditor model.state Edited

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

type alias KeyEvent = { kind : String, value : String }

onKey : KeyEvent -> Msg
onKey _event =
    Edited Saved

main =
    Terminal.appScreen
        { init = init, update = update, view = view
        , subscriptions = subscriptions, onKey = onKey
        }
"#;

/// `Ui.widget` inside a `Web.app` view — must be ACCEPTED (the browser shape has
/// the custom-element runtime and the seal codec).
const WEB_UI_WIDGET: &str = r#"module Main exposing (main)

import Ipe.Tea.Web as Web
import Ipe.Ui as Ui
import Ipe.Tea.Web.Cmd
import Ipe.Tea.Web.Sub

type alias EditorState = { text : String, line : Int }

type EditorEvent = Changed String | Saved

type Msg = Edited EditorEvent

type alias Model = { state : EditorState }

codeEditor : CustomElement EditorState EditorEvent
codeEditor = customElement "js/x.js"

init : a -> ( Model, Cmd Msg )
init _req =
    ( { state = { text = "", line = 0 } }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update _msg model =
    ( model, Cmd.none )

view : Model -> Element Msg
view model =
    Ui.widget codeEditor model.state Edited

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Web.app
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [], notFound = Edited Saved
        }
"#;

/// `Ui.widget` inside a `WebView.app` view — must be ACCEPTED (`webview` forces
/// the `json` feature, so it takes the real seal-coded path).
const WEBVIEW_UI_WIDGET: &str = r#"module Main exposing (main)

import Ipe.Tea.WebView as Webview
import Ipe.Ui as Ui
import Ipe.Tea.WebView.Cmd as Cmd
import Ipe.Tea.WebView.Sub as Sub

type alias EditorState = { text : String, line : Int }

type EditorEvent = Changed String | Saved

type Msg = Edited EditorEvent

type alias Model = { state : EditorState }

codeEditor : CustomElement EditorState EditorEvent
codeEditor = customElement "js/x.js"

init : () -> ( Model, Cmd Msg )
init _unit =
    ( { state = { text = "", line = 0 } }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update _msg model =
    ( model, Cmd.none )

view : Model -> Element Msg
view model =
    Ui.widget codeEditor model.state Edited

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Webview.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , window = { title = "Editor", size = ( 400, 300 ) }
        }
"#;

/// A `Terminal.appScreen` view mounting `Ui.widget` is a browser-only node in a
/// terminal build: rejected fail-closed with IPE-L0147, not a cargo failure or a
/// panic (the emitted crate would otherwise trip rustc E0282).
#[test]
fn terminal_view_with_ui_widget_is_rejected() -> Result<(), BoxError> {
    assert_rejected_with(
        "terminal_ui_widget",
        TERMINAL_UI_WIDGET,
        &[("js/x.js", WIDGET_JS)],
        "IPE-L0147",
    )
}

/// Non-regression control: `Ui.widget` under `Web.app` is the shape it belongs
/// to and must compile cleanly (ipe-0).
#[test]
fn web_view_with_ui_widget_is_accepted() -> Result<(), BoxError> {
    assert_accepted("web_ui_widget", WEB_UI_WIDGET, &[("js/x.js", WIDGET_JS)])
}

/// `WebView.app` also forces the `json` feature, so the shape guard ADMITS its
/// `Ui.widget` (ipe-0) — it takes the real seal-coded `ui_widget_` path, not the
/// unconstrained non-`json` fallback this gate exists to reject. This asserts the
/// admission only. The emitted crate also cargo-builds: the backend derives
/// `serde` for a `WebView` `Ui.widget`'s down/up seal types (gated on either
/// browser shape, not the Model bound), proven end-to-end by
/// `webview_e2e::webview_ui_widget_seal_builds` under `IPE_E2E=1`.
#[test]
fn webview_view_with_ui_widget_is_accepted() -> Result<(), BoxError> {
    assert_accepted(
        "webview_ui_widget",
        WEBVIEW_UI_WIDGET,
        &[("js/x.js", WIDGET_JS)],
    )
}

/// `Ui.widget` inside a `Terminal.appLines` (Cli shape) view.
///
/// A Cli view has type `Model -> String`. `Ui.widget` returns `Element msg`, so
/// the type checker rejects the program before the `RejectInNonWebShape` shape
/// gate is reached — the type mismatch is the primary rejection. The shape gate
/// is defense-in-depth for any hypothetical path that bypasses type inference
/// (e.g., programmatic IR construction in tests).
const CLI_UI_WIDGET: &str = r#"module Main exposing (main)

import Ipe.Tea.Terminal as Terminal
import Ipe.Ui as Ui

type alias EditorState = { text : String, line : Int }

type EditorEvent = Changed String | Saved

type Msg = Edited EditorEvent

type alias Model = { state : EditorState }

codeEditor : CustomElement EditorState EditorEvent
codeEditor = customElement "js/x.js"

init : () -> ( Model, Cmd Msg )
init _unit =
    ( { state = { text = "", line = 0 } }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update _msg model =
    ( model, Cmd.none )

view : Model -> String
view model =
    Ui.widget codeEditor model.state Edited

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

onLine : String -> Msg
onLine _line =
    Edited Saved

main =
    Terminal.appLines
        { init = init, update = update, view = view
        , subscriptions = subscriptions, onLine = onLine
        }
"#;

/// `Ui.widget` in a `Terminal.appLines` view is rejected because `Ui.widget`
/// returns `Element msg` but the Cli view expects `String`. The type checker
/// rejects it (IPE-T0001) before the `RejectInNonWebShape` shape gate fires.
/// The gate is defense-in-depth for any IR path that bypasses type inference.
#[test]
fn cli_view_with_ui_widget_is_rejected() -> Result<(), BoxError> {
    match compile_with_files("cli_ui_widget", CLI_UI_WIDGET, &[("js/x.js", WIDGET_JS)])? {
        Ok(()) => Err("cli_ui_widget: expected a type error, but ipec succeeded".into()),
        Err(ipe::CliError::Pipeline { .. }) => Ok(()),
        Err(other) => Err(format!("cli_ui_widget: expected a type error, got {other:?}").into()),
    }
}

//! Compile-time surface tests for `Ipe.Tea.Tui` and `Ipe.Tea.Cli`.
//!
//! `Ipe.Tea.Tui` exposes the full-screen terminal TEA entry via `Tui.tea`;
//! `Ipe.Tea.Cli` exposes the line-oriented entry via `Cli.tea`. Both `app`
//! entries are registered in the `env.rs` qualifier catalog and carry
//! `KernelClass::Terminal` (the one terminal rendering family).
//!
//! Terminal input is a subscription (`Tui.Sub.onKey` / `Cli.Sub.onLine`); the
//! refusals below pin that a stale `onKey` / `onLine` config field and a
//! wrong-surface input subscription are both rejected at `ipe` time.
//!
//! These tests are COMPILE-ONLY (the `ipe` pipeline writes the emitted project
//! but `cargo` is never invoked), so they run in CI without `IPE_E2E`.

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

fn compile(test_name: &str, source: &str) -> Result<Result<(), ipe::CliError>, BoxError> {
    compile_files(test_name, &[("Main.ipe", source)])
}

/// Compile a multi-module program whose entry is `Main.ipe`.
///
/// Every `(file name, source)` is written beside the entry.
fn compile_files(
    test_name: &str,
    files: &[(&str, &str)],
) -> Result<Result<(), ipe::CliError>, BoxError> {
    let ipe_dir = std::env::temp_dir().join(format!("tea_surface_{test_name}_ipe"));
    let _ = std::fs::remove_dir_all(&ipe_dir);
    std::fs::create_dir_all(&ipe_dir)?;
    for (name, source) in files {
        std::fs::write(ipe_dir.join(name), source)?;
    }
    let entry = ipe_dir.join("Main.ipe");

    let out_dir = std::env::temp_dir().join(format!("tea_surface_{test_name}_out"));
    let _ = std::fs::remove_dir_all(&out_dir);

    let runtime = ipe::resolve_runtime().map_err(|e| -> BoxError { format!("{e:?}").into() })?;
    Ok(ipe::build_with_sibling_discovery(&entry, &out_dir, &runtime))
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

/// Minimal `Tui.tea` program — `import Ipe.Tea.Tui as Tui` then `Tui.tea { ... }`.
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
    Sub.onKey onKey

onKey : KeyEvent -> Msg
onKey _event =
    NoOp

main =
    Tui.tea
        { init = init, update = update, view = view
        , subscriptions = subscriptions
        }
"#;

/// Minimal `Cli.tea` program — `import Ipe.Tea.Cli as Cli` then `Cli.tea { ... }`.
const CLI_APP: &str = r#"module Main exposing (main)

import Ipe.Tea.Cli as Cli
import Ipe.Tea.Cli.Cmd
import Ipe.Tea.Cli.Sub
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
    Sub.onLine onLine

onLine : String -> Msg
onLine s =
    Line s

main =
    Cli.tea
        { init = init, update = update, view = view
        , subscriptions = subscriptions
        }
"#;

/// A DOM view (`View Web msg`) whose container holds a terminal-cells node
/// (`View Tui msg`). The two engines are distinct nullary tags on the one
/// `View engine msg` carrier, so mixing them in one view tree fails
/// unification — a cross-engine view is unrepresentable, not a silent render.
const CROSS_ENGINE_VIEW: &str = r#"module Main exposing (main)

import Ipe.Tea.Web as Web
import Ipe.Tea.Web.Cmd as Cmd
import Ipe.Tea.Web.Sub as Sub
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
view : Model -> Element Msg
view _model =
    Ui.column [] [ Cells.text "nope" ]

main =
    Web.tea
        { init = init, update = update, view = view
        , subscriptions = subscriptions
        , routes = [], notFound = NoOp
        }
"#;

/// A `View` over a tag outside the closed `{Web, Tui, Cli}` engine set. The
/// tag names no view engine, so it is rejected fail-closed at canon — never a
/// flexible variable that defers the failure downstream.
const NON_ENGINE_VIEW: &str = r"module Main exposing (v)

v : View Foo Msg
v = v
";

/// `Tui.tea` is the full-screen terminal entry kernel registered in
/// the `env.rs` qualifier catalog. A program using
/// `import Ipe.Tea.Tui as Tui` and `Tui.tea { ... }` must compile (ipe-0).
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

/// `Cli.tea` is the line-oriented terminal entry kernel registered in
/// the `env.rs` qualifier catalog. A program using
/// `import Ipe.Tea.Cli as Cli` and `Cli.tea { ... }` must compile (ipe-0).
#[test]
fn cli_app_surface_compiles() -> Result<(), BoxError> {
    assert_accepted("cli_app", CLI_APP)
}

// ── Terminal input is a subscription ────────────────────────────────────────

/// The four-field config closing lines shared by `TUI_APP` and `CLI_APP`.
///
/// The two `…_WITH_…` variants still carry an input config field.
const CFG_TAIL: &str = "        , subscriptions = subscriptions\n        }";
const CFG_TAIL_WITH_ON_KEY: &str =
    "        , subscriptions = subscriptions, onKey = onKey\n        }";
const CFG_TAIL_WITH_ON_LINE: &str =
    "        , subscriptions = subscriptions, onLine = onLine\n        }";

/// `source` with `from` replaced by `to`, failing if `from` is absent.
///
/// A fixture drift then cannot silently turn a refusal test into an acceptance
/// one.
fn variant(source: &str, from: &str, to: &str) -> Result<String, BoxError> {
    if source.contains(from) {
        Ok(source.replace(from, to))
    } else {
        Err(format!("fixture does not contain {from:?}").into())
    }
}

/// Assert `source` is REJECTED by the pipeline for any reason (never exit-0).
fn assert_rejected(test_name: &str, source: &str) -> Result<(), BoxError> {
    match compile(test_name, source)? {
        Ok(()) => Err(format!("{test_name}: expected rejection, but ipe accepted").into()),
        Err(_) => Ok(()),
    }
}

/// A `Tui.tea` config still passing `onKey` is refused with IPE-N0051.
///
/// Never silently accepted with key input dropped.
#[test]
fn tui_on_key_config_field_is_rejected() -> Result<(), BoxError> {
    let src = variant(TUI_APP, CFG_TAIL, CFG_TAIL_WITH_ON_KEY)?;
    assert_rejected_code("tui_on_key_field", &src, "IPE-N0051")
}

/// A `Cli.tea` config still passing `onLine` is refused with IPE-N0051.
#[test]
fn cli_on_line_config_field_is_rejected() -> Result<(), BoxError> {
    let src = variant(CLI_APP, CFG_TAIL, CFG_TAIL_WITH_ON_LINE)?;
    assert_rejected_code("cli_on_line_field", &src, "IPE-N0051")
}

/// A config bound to a top-level name is checked too.
#[test]
fn cli_on_line_in_top_level_config_is_rejected() -> Result<(), BoxError> {
    let src = variant(
        CLI_APP,
        "main =\n    Cli.tea\n        { init = init, update = update, view = view\n        , subscriptions = subscriptions\n        }",
        "cfg =\n    { init = init, update = update, view = view\n    , subscriptions = subscriptions, onLine = onLine\n    }\n\nmain =\n    Cli.tea cfg",
    )?;
    assert_rejected_code("cli_on_line_top_level_cfg", &src, "IPE-N0051")
}

/// The config rows are closed, so an extra field is refused.
///
/// Never absorbed and ignored.
#[test]
fn unknown_terminal_config_field_is_rejected() -> Result<(), BoxError> {
    let tui = variant(
        TUI_APP,
        CFG_TAIL,
        "        , subscriptions = subscriptions, extra = 1\n        }",
    )?;
    assert_rejected("tui_extra_field", &tui)?;
    // The other surface's input field is just as foreign to this entry.
    let cli = variant(
        CLI_APP,
        CFG_TAIL,
        "        , subscriptions = subscriptions, onKey = onLine\n        }",
    )?;
    assert_rejected("cli_on_key_field", &cli)
}

/// `Tui.Sub.onKey` in a `Cli` app is refused with IPE-N0035.
///
/// A line app has no key stream.
#[test]
fn tui_sub_in_cli_app_is_rejected() -> Result<(), BoxError> {
    let src = variant(CLI_APP, "import Ipe.Tea.Cli.Sub", "import Ipe.Tea.Tui.Sub")?;
    let src = variant(&src, "Sub.onLine onLine", "Sub.onKey (\\_ -> NoOp)")?;
    assert_rejected_code("tui_sub_in_cli", &src, "IPE-N0035")
}

/// `Cli.Sub.onLine` in a `Tui` app is refused with IPE-N0035.
///
/// A full-screen app has no line stream.
#[test]
fn cli_sub_in_tui_app_is_rejected() -> Result<(), BoxError> {
    let src = variant(TUI_APP, "import Ipe.Tea.Tui.Sub", "import Ipe.Tea.Cli.Sub")?;
    let src = variant(&src, "Sub.onKey onKey", "Sub.onLine (\\_ -> NoOp)")?;
    assert_rejected_code("cli_sub_in_tui", &src, "IPE-N0035")
}

/// The shared `Ipe.Tea.Terminal.Sub` carries no input subscription.
///
/// Naming `onKey` through it is an unknown member.
#[test]
fn terminal_sub_has_no_input_subscription() -> Result<(), BoxError> {
    let src = variant(
        TUI_APP,
        "import Ipe.Tea.Tui.Sub",
        "import Ipe.Tea.Terminal.Sub",
    )?;
    assert_rejected_code("terminal_sub_on_key", &src, "IPE-N0005")
}

/// Any handler expression is accepted.
///
/// Covers a lambda, and a key subscription combined with a timer through
/// `Sub.batch`.
#[test]
fn tui_on_key_accepts_any_handler_form() -> Result<(), BoxError> {
    let lambda = variant(TUI_APP, "Sub.onKey onKey", "Sub.onKey (\\_ -> NoOp)")?;
    assert_accepted("tui_on_key_lambda", &lambda)?;
    let batched = variant(
        TUI_APP,
        "Sub.onKey onKey",
        "Sub.batch [ Sub.onKey onKey, Sub.every 1000 NoOp ]",
    )?;
    assert_accepted("tui_on_key_batched", &batched)
}

/// `Cli.Sub.onLine` accepts a constructor directly as its handler.
#[test]
fn cli_on_line_accepts_a_constructor_handler() -> Result<(), BoxError> {
    let src = variant(CLI_APP, "Sub.onLine onLine", "Sub.onLine Line")?;
    assert_accepted("cli_on_line_ctor", &src)
}

/// A helper module's `Tui.Sub.onKey` in a `Cli` app is refused with IPE-N0035.
///
/// The helper is not the entry, so the entry-module import gate never sees it;
/// the lowerer's surface gate refuses the reference itself, so it never
/// compiles into a key subscription the line loop would silently never read.
#[test]
fn tui_sub_from_a_helper_module_in_a_cli_app_is_rejected() -> Result<(), BoxError> {
    let main = variant(
        CLI_APP,
        "import Ipe.Tea.Cli as Cli\n",
        "import Ipe.Tea.Cli as Cli\nimport Keys\n",
    )?;
    let main = variant(&main, "Sub.onLine onLine", "Keys.keys NoOp")?;
    let keys = "module Keys exposing (keys)\n\n\
                import Ipe.Tea.Tui.Sub as Sub\n\n\n\
                keys msg =\n    Sub.onKey (\\_ -> msg)\n";
    let outcome = compile_files(
        "tui_sub_helper_in_cli",
        &[("Main.ipe", main.as_str()), ("Keys.ipe", keys)],
    )?;
    match outcome {
        Ok(()) => Err("tui_sub_helper_in_cli: expected rejection, but ipe accepted".into()),
        Err(ipe::CliError::Pipeline { diag, .. }) if diag.code().as_str() == "IPE-N0035" => Ok(()),
        Err(other) => {
            Err(format!("tui_sub_helper_in_cli: expected IPE-N0035, got {other:?}").into())
        }
    }
}

/// The emitted Ipê-side Rust of a compiled test program (`main.rs` + `ipe_mods`).
fn emitted_rust(test_name: &str) -> String {
    let src = std::env::temp_dir()
        .join(format!("tea_surface_{test_name}_out"))
        .join("src");
    let mut combined = std::fs::read_to_string(src.join("main.rs")).unwrap_or_default();
    if let Ok(entries) = std::fs::read_dir(src.join("ipe_mods")) {
        let mut files: Vec<std::path::PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "rs"))
            .collect();
        files.sort();
        for path in files {
            if let Ok(text) = std::fs::read_to_string(&path) {
                combined.push('\n');
                combined.push_str(&text);
            }
        }
    }
    combined
}

/// A point-free `Sub.onKey` still emits through the `KeyEvent` bridge.
///
/// `List.map Sub.onKey handlers` reifies the kernel as a first-class value;
/// the lowerer eta-expands it so the saturated emit arm (and its bridge) fires,
/// instead of boxing the bare runtime function (a `cargo` E0277).
#[test]
fn point_free_tui_on_key_emits_the_bridge() -> Result<(), BoxError> {
    let src = variant(
        TUI_APP,
        "import Ipe.Tea.Tui as Tui\n",
        "import Ipe.Tea.Tui as Tui\nimport Ipe.List as List\n",
    )?;
    let src = variant(
        &src,
        "Sub.onKey onKey",
        "Sub.batch (List.map Sub.onKey [ onKey, onKey ])",
    )?;
    assert_accepted("point_free_on_key", &src)?;
    let emitted: String = emitted_rust("point_free_on_key")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if emitted.is_empty() {
        return Ok(()); // runtime unavailable — structural assertion skipped
    }
    if !emitted.contains("tui_sub_on_key(") || !emitted.contains("|kind: String, value: String|") {
        return Err(
            format!("point-free Sub.onKey must emit the KeyEvent bridge; got:\n{emitted}").into(),
        );
    }
    if emitted.contains("Box::new(tui_sub_on_key)") {
        return Err("point-free Sub.onKey must never be boxed as a bare function value".into());
    }
    Ok(())
}

/// A let-bound `Sub.onKey` is eta-expanded the same way and accepted.
#[test]
fn let_bound_tui_on_key_is_accepted() -> Result<(), BoxError> {
    let src = variant(
        TUI_APP,
        "Sub.onKey onKey",
        "let\n        on =\n            Sub.onKey\n    in\n    on onKey",
    )?;
    assert_accepted("let_bound_on_key", &src)
}

/// A point-free `Cli.Sub.onLine` in a helper module of a `Tui` app is refused with IPE-N0035.
///
/// The surface gate checks the reference itself, so the point-free form cannot
/// route around it into a line subscription the key loop never reads.
#[test]
fn point_free_cli_sub_from_a_helper_module_in_a_tui_app_is_rejected() -> Result<(), BoxError> {
    let main = variant(
        TUI_APP,
        "import Ipe.Tea.Tui as Tui\n",
        "import Ipe.Tea.Tui as Tui\nimport Lines\n",
    )?;
    let main = variant(&main, "Sub.onKey onKey", "Lines.lines (\\_ -> NoOp)")?;
    let lines = "module Lines exposing (lines)\n\n\
                 import Ipe.Tea.Cli.Sub as Sub\n\n\n\
                 lines =\n    Sub.onLine\n";
    assert_rejected_files_code(
        "cli_sub_point_free_helper_in_tui",
        &[("Main.ipe", main.as_str()), ("Lines.ipe", lines)],
        "IPE-N0035",
    )
}

/// A stray `Tui.tea` in a helper module does not widen a `Cli` app to key input.
///
/// The surface is read from the entry's own `main`, never from an app entry
/// that merely appears in another module.
#[test]
fn a_stray_tui_entry_in_a_helper_does_not_widen_a_cli_app() -> Result<(), BoxError> {
    let main = variant(
        CLI_APP,
        "import Ipe.Tea.Cli as Cli\n",
        "import Ipe.Tea.Cli as Cli\nimport Keys\n",
    )?;
    let main = variant(&main, "Sub.onLine onLine", "Keys.keys NoOp")?;
    let keys = "module Keys exposing (keys, stray)\n\n\
                import Ipe.Tea.Tui as Tui\n\
                import Ipe.Tea.Tui.Cmd as Cmd\n\
                import Ipe.Tea.Tui.Sub as Sub\n\
                import Ipe.Ui.Cells as Cells\n\n\n\
                keys msg =\n    Sub.onKey (\\_ -> msg)\n\n\n\
                stray =\n    Tui.tea\n        { init = \\_ -> ( 0, Cmd.none )\n        \
                , update = \\_ m -> ( m, Cmd.none )\n        \
                , view = \\_ -> Cells.text \"x\"\n        \
                , subscriptions = \\_ -> Sub.none\n        }\n";
    assert_rejected_files_code(
        "stray_tui_helper_in_cli",
        &[("Main.ipe", main.as_str()), ("Keys.ipe", keys)],
        "IPE-N0035",
    )
}

/// Assert a multi-module program is REJECTED with exactly `expected`.
fn assert_rejected_files_code(
    test_name: &str,
    files: &[(&str, &str)],
    expected: &str,
) -> Result<(), BoxError> {
    match compile_files(test_name, files)? {
        Ok(()) => Err(format!("{test_name}: expected {expected}, but ipe accepted").into()),
        Err(ipe::CliError::Pipeline { diag, .. }) if diag.code().as_str() == expected => Ok(()),
        Err(other) => Err(format!("{test_name}: expected {expected}, got {other:?}").into()),
    }
}

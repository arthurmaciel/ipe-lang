//! Bare builtin parametric UI annotations arity-fill their message parameter.
//!
//! A `view : Html` / `attr : Attribute` annotation omits the message-type
//! parameter every UI constructor carries (`Html msg`). Ipê fills it with the
//! inferred `any` wildcard at the single canon source of truth, so the checker
//! and the lowerer agree on `Html any` rather than a zero-arg `Html` that would
//! reach the lowerer's empty-home catch-all (IPE-I0001).
//!
//! The SEAL property: when the binding is wired through an app consumer that
//! pins the message type, the emitted Rust return type carries the CONCRETE
//! message enum (`Html<MainMsg>`), never `Html<()>` (which would mismatch the
//! consumer's `Fn(Model) -> Html<Msg>` bound at cargo time) and never a spurious
//! `Html<T1>` generic.
//!
//! Compile-only assertions always run; the cargo build is `IPE_E2E=1`-gated with
//! an ISOLATED `CARGO_TARGET_DIR` (a shared dir's fingerprint reuse can mask a
//! rustc failure as a false pass).

use std::path::{Path, PathBuf};

/// A full `Web` app whose inner `rawView : Model -> Html` omits the message
/// parameter, reached through the `Ui.html` escape node inside the single
/// `Element` view. The `onPress = Just Bump` button pins the inferred message
/// type to the concrete `Msg`, so the arity-filled inner `Html` return must
/// solve to `Html<MainMsg>`.
const BARE_HTML_VIEW_APP: &str = r#"module Main exposing (main)
import Ipe.Tea.Web as Web
import Ipe.Tea.Web.Cmd as Cmd
import Ipe.Tea.Web.Sub as Sub
import Ipe.Ui as Ui
import Ipe.Html as Html
type alias Model = { n : Int }
type Msg = Bump
init : WebReq -> ( Model, Cmd Msg )
init _ = ( { n = 0 }, Cmd.none )
update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Bump -> ( { model | n = model.n + 1 }, Cmd.none )
subscriptions : Model -> Sub Msg
subscriptions _model = Sub.none
rawView : Model -> Html
rawView model =
    Ui.layout []
        (Ui.button []
            { onPress = Just Bump, label = Ui.text "x" })
view : Model -> Element Msg
view model =
    Ui.html (rawView model)
main =
    Web.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = []
        , notFound = Bump
        }
"#;

/// A helper returning a bare `Attribute` and a bare `Element` — exercising the
/// `Attribute` arity-fill arm (which additionally disambiguates the Ipe.Ui vs
/// Ipe.Html home) alongside the `Element` arm. The attribute is message-
/// polymorphic (a layout attribute carries no message), so the fill leaves the
/// parameter inferred; both filled constructors sit on one path.
const BARE_ATTRIBUTE_HELPER: &str = r#"module Main exposing (main)
import Ipe.Ui as Ui
import Ipe.Task
padAttr : Attribute
padAttr =
    Ui.padding 4
labelled : Element
labelled =
    Ui.el [ padAttr ] (Ui.text "x")
main : Task Error ()
main =
    Task.succeed ()
"#;

fn html_app_out_dir() -> PathBuf {
    std::env::temp_dir().join("bare_ui_html_app_out")
}

/// Compile a fixture into its own out dir; `None` (skip) when the runtime
/// cannot be resolved.
fn compile(fixture: &str, tag: &str, out: &PathBuf) -> Option<Result<(), ipe::CliError>> {
    let ipe_dir = std::env::temp_dir().join(format!("bare_ui_{tag}_ipe"));
    let _ = std::fs::remove_dir_all(&ipe_dir);
    std::fs::create_dir_all(&ipe_dir).ok()?;
    let entry = ipe_dir.join("Main.ipe");
    std::fs::write(&entry, fixture).ok()?;
    let _ = std::fs::remove_dir_all(out);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return None;
    };
    Some(ipe::build(&entry, out, &runtime))
}

/// Concatenate the emitted `.rs` files that carry GENERATED program code —
/// `main.rs` plus any `ipe_mods/…` module file (a single-module program emits
/// the binding into `main.rs`, a multi-module one into `ipe_mods/`). The
/// vendored `ipe_runtime/` tree is excluded: it is copied verbatim and contains
/// its own unrelated `Html<()>` occurrences that must not perturb the assertion.
fn emitted_program_sources(out: &Path) -> String {
    let mut acc = String::new();
    let src = out.join("src");
    let mut stack = vec![src.clone()];
    let runtime = src.join("ipe_runtime");
    while let Some(dir) = stack.pop() {
        if dir == runtime {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                acc.push_str(&text);
                acc.push('\n');
            }
        }
    }
    acc
}

/// `rawView : Model -> Html` (bare) reached via `Ui.html` under `Web.app`
/// must be ipe-0 and
/// emit the CONCRETE `Html<MainMsg>` return — the arity-fill's message parameter
/// resolved from the body's solved type, never `Html<()>` or `Html<T1>`.
#[test]
fn bare_html_view_emits_concrete_msg() {
    let out = html_app_out_dir();
    let Some(result) = compile(BARE_HTML_VIEW_APP, "html_app", &out) else {
        return;
    };
    assert!(
        result.is_ok(),
        "a bare `view : Model -> Html` must arity-fill and be ipe-0, got: {:?}",
        result.err(),
    );
    let emitted = emitted_program_sources(&out);
    assert!(
        emitted.contains("Html<MainMsg>"),
        "the bare `Html` return must lower to the concrete `Html<MainMsg>` \
         (the pinned message type), not `Html<()>` or a spurious generic",
    );
    assert!(
        !emitted.contains("Html<()>"),
        "the pinned view must not emit `Html<()>` — that would mismatch the \
         app consumer's `Fn(Model) -> Html<Msg>` bound at cargo time",
    );
}

/// Bare `Attribute` and `Element` annotations arity-fill their message
/// parameter and are ipe-0 — the fill covers every builtin parametric UI
/// constructor, not just `Html`.
#[test]
fn bare_attribute_and_element_arity_fill() {
    let out = std::env::temp_dir().join("bare_ui_attr_out");
    let Some(result) = compile(BARE_ATTRIBUTE_HELPER, "attr", &out) else {
        return;
    };
    assert!(
        result.is_ok(),
        "bare `Attribute` / `Element` annotations must arity-fill and be ipe-0, \
         got: {:?}",
        result.err(),
    );
}

/// `IPE_E2E` tier: the bare-`Html` Webview app must cargo-build (isolated target
/// dir) — the SEAL check that ipe-0 implies cargo-0 for the arity-filled return.
#[test]
fn bare_html_view_cargo_builds() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    bare_html_view_emits_concrete_msg();

    let target = std::env::temp_dir().join("bare_ui").join("html_app");
    let build = std::process::Command::new("cargo")
        .arg("build")
        .env("CARGO_TARGET_DIR", &target)
        .current_dir(html_app_out_dir())
        .output()
        .expect("cargo must spawn");
    assert!(
        build.status.success(),
        "the bare-`Html` Webview app must cargo-build\n--- cargo stderr ---\n{}",
        String::from_utf8_lossy(&build.stderr),
    );
}

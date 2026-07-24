//! The prescriptive TEA `init` signature.
//!
//! ## Background
//!
//! Ipê pins `Live.app`'s `init` to `LiveReq -> (Model, Cmd Msg)` (mandatory)
//! rather than the reference's permissive free `req` type var. Two properties
//! are pinned here:
//!
//! 1. **Prescription is enforced.** A `Live.app` whose `init` declares `{}` (or
//!    any non-`LiveReq` shape) is a clear compile-time IPE-T0001 at the `init`
//!    cfg field (`expected LiveReq, found {}`) — not a raw unification failure
//!    and not a deferred `cargo` break.
//!
//! 2. **`LiveReq` fields are readable.** `LiveReq` is an opaque nullary `Con`
//!    at the type level (so no bare record literal can masquerade as the runtime
//!    struct — the make-invalid-states-unrepresentable posture shared with the
//!    opaque server `Request`), but its fixed field set is READABLE via the
//!    deferred `FieldAccess` pass (`LiveReqFields`). `init req = ... req.path`
//!    type-checks and lowers to `(req).path.clone()`, reading
//!    `ipe_runtime::live::LiveReq` directly — no synthesised record.
//!
//! Full design: `docs/adr/0021-tea-state-engine-and-prescriptive-init.md`;
//! divergence B24 in `docs/divergences-from-sky.md`.
//!
//! Compile-only assertions always run; the cargo build is `IPE_E2E=1`-gated
//! with an ISOLATED `CARGO_TARGET_DIR` (a shared dir's fingerprint reuse can
//! mask a rustc failure as a false pass).

use std::path::PathBuf;

/// A `Live.app` whose `init : LiveReq -> …` READS `req.path` into the Model.
/// Non-routed for brevity; plain-data Model so the the admissibility gate
/// passes, isolating the init-field + field-access behaviour.
const LIVE_INIT_READS_REQ_PATH: &str = r#"module Main exposing (main)
import Ipe.Live as Live
import Ipe.Ui as Ui
type Page = HomePage
type Msg = Noop
type alias Model = { page : Page, path : String }
init : LiveReq -> ( Model, Cmd Msg )
init req = ( { page = HomePage, path = req.path }, Cmd.none )
update : Msg -> Model -> ( Model, Cmd Msg )
update _msg model = ( model, Cmd.none )
view : Model -> any
view model = Ui.layout [] (Ui.text model.path)
subscriptions : Model -> Sub Msg
subscriptions _model = Sub.none
main =
    Live.app
        { init = init, update = update, view = view
        , subscriptions = subscriptions
        , routes = [ Live.route "/" HomePage ]
        , notFound = HomePage
        }
"#;

/// The SAME app but with `init : {} -> …` — the non-`LiveReq` shape the
/// prescriptive scheme must reject with a clear IPE-T0001.
const LIVE_INIT_UNIT_REJECTED: &str = r#"module Main exposing (main)
import Ipe.Live as Live
import Ipe.Ui as Ui
type Page = HomePage
type Msg = Noop
type alias Model = { page : Page }
init : {} -> ( Model, Cmd Msg )
init _ = ( { page = HomePage }, Cmd.none )
update : Msg -> Model -> ( Model, Cmd Msg )
update _msg model = ( model, Cmd.none )
view : Model -> any
view _model = Ui.layout [] (Ui.text "hi")
subscriptions : Model -> Sub Msg
subscriptions _model = Sub.none
main =
    Live.app
        { init = init, update = update, view = view
        , subscriptions = subscriptions
        , routes = [ Live.route "/" HomePage ]
        , notFound = HomePage
        }
"#;

fn ok_out_dir() -> PathBuf {
    std::env::temp_dir().join("i180_init_reads_req_path_out")
}

/// Compile a fixture into its own out dir; `None` (skip) when the runtime
/// cannot be resolved.
fn compile(fixture: &str, tag: &str, out: &PathBuf) -> Option<Result<(), ipe::CliError>> {
    let ipe_dir = std::env::temp_dir().join(format!("i180_{tag}_ipe"));
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

/// `init : LiveReq -> …` reading `req.path` must be ipe-0 and emit
/// `(req).path.clone()` against the runtime `LiveReq` struct — proving the
/// `LiveReqFields` deferred field-access resolution + concrete lowering.
#[test]
fn live_init_reads_req_path_field() {
    let out = ok_out_dir();
    let Some(result) = compile(LIVE_INIT_READS_REQ_PATH, "reads_req_path", &out) else {
        return;
    };
    assert!(
        result.is_ok(),
        "#180: `init : LiveReq -> …` reading `req.path` must be ipe-0, got: {:?}",
        result.err(),
    );
    let main_rs = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");
    assert!(
        main_rs.contains("ipe_runtime::live::LiveReq"),
        "#180: the emitted init must take the concrete `ipe_runtime::live::LiveReq`",
    );
    assert!(
        main_rs.contains("(req).path.clone()"),
        "#180: `req.path` must lower to a direct struct field read \
         `(req).path.clone()` — no synthesised record",
    );
}

/// `init : {} -> …` must be rejected with a clear IPE-T0001 naming the expected
/// `LiveReq` — the prescriptive scheme, fail-closed at ipe time.
#[test]
fn live_init_unit_is_rejected() {
    let out = std::env::temp_dir().join("i180_init_unit_out");
    let Some(result) = compile(LIVE_INIT_UNIT_REJECTED, "init_unit", &out) else {
        return;
    };
    let err = result.expect_err(
        "#180: `init : {} -> …` on a Live.app must be a compile error under the \
         prescriptive LiveReq scheme",
    );
    let rendered = format!("{err:?}");
    assert!(
        rendered.contains("LiveReq"),
        "#180: the rejection must name the expected `LiveReq` type, got: {rendered}",
    );
}

/// `IPE_E2E` tier: the `init : LiveReq` project must cargo-build (isolated
/// target dir) — the SEAL check that ipe-0 implies cargo-0.
#[test]
fn live_init_reads_req_path_cargo_builds() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    live_init_reads_req_path_field();

    let target = std::env::temp_dir()
        .join("i180")
        .join("init_reads_req_path");
    let build = std::process::Command::new("cargo")
        .arg("build")
        .env("CARGO_TARGET_DIR", &target)
        .current_dir(ok_out_dir())
        .output()
        .expect("cargo must spawn");
    assert!(
        build.status.success(),
        "#180: the `init : LiveReq` project must cargo-build\n--- cargo stderr ---\n{}",
        String::from_utf8_lossy(&build.stderr),
    );
}

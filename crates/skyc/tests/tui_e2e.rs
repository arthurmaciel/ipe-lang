//! End-to-end tests for `Std.Tui` / `Sky.Tui` — `Tui.app`, `Ui.column`,
//! `Ui.el`, `Ui.text`, and `String.fromInt`.
//!
//! All tests are gated on `SKY_E2E=1`.  Without it they return early so the
//! default `cargo test` stays fast.
//!
//! ## Architecture
//!
//! 1. A minimal Sky.Tui counter program is written to a temp dir.
//! 2. `skyc::build` compiles it (parse → canon → types → lower → emit Rust).
//! 3. `oracle::build_rust_binary` runs `cargo build` on the emitted project —
//!    the shared Cargo target lets crossterm/tokio compile once and be reused.
//!
//! The binary is NOT spawned: `tui_app_ui` requires a real TTY
//! (`TuiGuard::enter_mouse()` opens the alternate screen with raw mode), which
//! is not available in a CI environment.  A successful `cargo build` is the
//! proof that the full pipeline works:
//!
//! ```text
//! Tui.app cfg → constrain → lower → emit_tui_call →
//!     sky_runtime::tui::tui_app_ui(init, update, view, subs, on_key)
//! ```
//!
//! The headless render assertion — does `view` produce a frame containing `0`?
//! — is covered by the runtime-level test in `runtime/src/lib.rs` (gated by
//! `--features tui`), which uses `tui::layout::element_to_cells` directly
//! without a TTY.
//!
//! Run:
//!
//! ```text
//! SKY_E2E=1 cargo test tui_e2e
//! ```

/// A minimal Sky.Tui counter app exercising the Phase-1c wiring.
///
/// Kernels exercised:
/// - `Tui.app`      — Phase-1c: constrain scheme + 5-field cfg
/// - `Ui.column`    — vertical layout
/// - `Ui.el`        — generic element container
/// - `Ui.text`      — text leaf node
/// - `String.fromInt` — displays the counter value
/// - `Cmd.none` / `Sub.none` — baseline TEA primitives
///
/// Note: `view` returns `Element Msg` (NOT wrapped in `Ui.layout` → `Html Msg`
/// like Sky.Live).  The Tui runtime renders the Element tree directly to ANSI
/// cells; there is no HTML step.
///
/// Note: `onKey _kind _value = NoOp` is the flat `String -> String -> Msg`
/// handler required by the runtime's `FOnKey: Fn(String, String) -> Msg` bound.
const SKY_TUI_COUNTER: &str = r"module Main exposing (main)

import Std.Tui as Tui
import Std.Ui as Ui

type Msg = Increment | Decrement | NoOp

type alias Model = { count : Int }

init : () -> ( Model, Cmd Msg )
init _unit =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )
        Decrement ->
            ( { model | count = model.count - 1 }, Cmd.none )
        NoOp ->
            ( model, Cmd.none )

view : Model -> Element Msg
view model =
    Ui.column []
        [ Ui.el [] (Ui.text (String.fromInt model.count)) ]

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

onKey : String -> String -> Msg
onKey _kind _value =
    NoOp

main =
    Tui.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , onKey = onKey
        }
";

/// Shared error type for E2E helpers.
type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Compile a Sky program string, build the emitted Rust project, and return
/// the path to the compiled binary.
fn compile_and_build(test_name: &str, sky_source: &str) -> Result<std::path::PathBuf, BoxError> {
    let sky_dir = std::env::temp_dir().join(format!("tui_e2e_{test_name}_sky"));
    let _ = std::fs::remove_dir_all(&sky_dir);
    std::fs::create_dir_all(&sky_dir).map_err(|e| -> BoxError {
        format!("{test_name}: cannot create sky source dir: {e}").into()
    })?;

    let entry = sky_dir.join("Main.sky");
    std::fs::write(&entry, sky_source)
        .map_err(|e| -> BoxError { format!("{test_name}: cannot write Main.sky: {e}").into() })?;

    let out_dir = std::env::temp_dir().join(format!("tui_e2e_{test_name}_emitted"));
    let _ = std::fs::remove_dir_all(&out_dir);

    let runtime = skyc::resolve_runtime()
        .map_err(|e| -> BoxError { format!("{test_name}: runtime unavailable: {e}").into() })?;

    skyc::build(&entry, &out_dir, &runtime)
        .map_err(|e| -> BoxError { format!("{test_name}: skyc build failed: {e}").into() })?;

    let exe = oracle::build_rust_binary(test_name, &out_dir)
        .map_err(|e| -> BoxError { format!("{test_name}: cargo build failed: {e}").into() })?;

    Ok(std::path::PathBuf::from(exe))
}

/// Compile-only: the Sky.Tui counter emits a Cargo project with the `"tui"`
/// feature in the default feature list, `crossterm` and `unicode-width` deps,
/// and `sky_runtime::tui::tui_app_ui` in the `main` function.
///
/// This is a BUILD-ONLY test — it does not spawn the binary (Tui requires a
/// real TTY).  A successful `cargo build` is the assertion:
///
/// * Phase-1c constrain: `Tui.app` correctly types the 5-field cfg
///   (`init/update/view/subscriptions/onKey`).
/// * Phase-1c lower: the cfg record literal bypasses SKY-L0107 (same exemption
///   as `Live.app`).
/// * Phase-1c emit: `emit_tui_call` delegates to `tui_app_ui(…)` with the five
///   handler arguments correctly emitted.
/// * Phase-1c manifest: `tui_cargo_toml` adds `"tui"` to default features,
///   `crossterm` + `unicode-width` deps, and `"sync"` to tokio.
///
/// # Errors
///
/// Propagates any pipeline or Cargo build failure as a test error.
#[test]
fn tui_counter_build_only() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    // compile_and_build already does skyc + cargo build; success is the proof.
    let _exe = compile_and_build("tui_build_only", SKY_TUI_COUNTER)?;
    Ok(())
}

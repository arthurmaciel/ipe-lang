//! End-to-end tests for `Ipe.Terminal` `appScreen` — `Ui.column`,
//! `Ui.el`, `Ui.text`, and `String.fromInt`.
//!
//! Non-E2E tests (no `IPE_E2E` required):
//! - `tui_onkey_record_typechecks` — ipe-level regression for the `onKey :
//!   KeyEvent -> Msg` record scheme fix (T0001); verifies `Terminal.appScreen`
//!   accepts a single-argument record-typed key handler and that the emitter
//!   generates the bridging wrapper closure.
//!
//! E2E tests (gated on `IPE_E2E=1`):
//! - `tui_counter_build_only` — full ipe + cargo build with `Terminal.appScreen`
//!   and a `KeyEvent -> Msg` handler (a `String -> String -> Msg` curried shape
//!   is not valid under the scheme).
//!
//! ## Architecture
//!
//! 1. A minimal Ipe.Terminal counter program is written to a temp dir.
//! 2. `ipe::build` compiles it (parse → canon → types → lower → emit Rust).
//! 3. `e2e_support::build_rust_binary` runs `cargo build` on the emitted project —
//!    the shared Cargo target lets crossterm/tokio compile once and be reused.
//!
//! The binary is NOT spawned: `tui_app_ui` requires a real TTY
//! (`TuiGuard::enter_mouse()` opens the alternate screen with raw mode), which
//! is not available in a CI environment.  A successful `cargo build` is the
//! proof that the full pipeline works:
//!
//! ```text
//! Terminal.appScreen cfg → constrain → lower → emit_tui_call →
//!     ipe_runtime::tui::tui_app_ui(init, update, view, subs, on_key)
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
//! IPE_E2E=1 cargo test tui_e2e
//! ```

/// A minimal `Terminal.appScreen` counter exercising the `appScreen` scheme.
///
/// `onKey` is a SINGLE-argument record handler — `KeyEvent -> Msg` — matching
/// the Haskell reference scheme (`any -> msg`).  The emitter generates the
/// bridging closure:
///
/// ```text
/// |kind: String, value: String| Main_on_key(RecKindValue { kind, value })
/// ```
///
/// The curried `String -> String -> Msg` shape is not valid under the
/// scheme; it would unify `var(1)` (the msg type variable) with
/// `String -> Msg` which conflicts with its use in `update`/`subscriptions`.
///
/// Note: `view` returns `Element Msg` (NOT wrapped in `Ui.layout` → `Html Msg`
/// like Ipe.Web).  The Tui runtime renders the Element tree directly to ANSI
/// cells; there is no HTML step.
const IPE_TUI_COUNTER: &str = r"module Main exposing (main)

import Ipe.Tea.Terminal as Terminal
import Ipe.Ui as Ui
import Ipe.Cmd
import Ipe.String
import Ipe.Sub

type alias KeyEvent = { kind : String, value : String }

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

onKey : KeyEvent -> Msg
onKey _ =
    NoOp

main =
    Terminal.appScreen
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , onKey = onKey
        }
";

/// Shared error type for E2E helpers.
type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Compile a Ipê program string, build the emitted Rust project, and return
/// the path to the compiled binary.
fn compile_and_build(test_name: &str, ipe_source: &str) -> Result<std::path::PathBuf, BoxError> {
    let ipe_dir = std::env::temp_dir().join(format!("tui_e2e_{test_name}_ipe"));
    let _ = std::fs::remove_dir_all(&ipe_dir);
    std::fs::create_dir_all(&ipe_dir).map_err(|e| -> BoxError {
        format!("{test_name}: cannot create ipe source dir: {e}").into()
    })?;

    let entry = ipe_dir.join("Main.ipe");
    std::fs::write(&entry, ipe_source)
        .map_err(|e| -> BoxError { format!("{test_name}: cannot write Main.ipe: {e}").into() })?;

    let out_dir = std::env::temp_dir().join(format!("tui_e2e_{test_name}_emitted"));
    let _ = std::fs::remove_dir_all(&out_dir);

    let runtime = ipe::resolve_runtime()
        .map_err(|e| -> BoxError { format!("{test_name}: runtime unavailable: {e}").into() })?;

    ipe::build(&entry, &out_dir, &runtime)
        .map_err(|e| -> BoxError { format!("{test_name}: ipe build failed: {e}").into() })?;

    let exe = e2e_support::build_rust_binary(test_name, &out_dir)
        .map_err(|e| -> BoxError { format!("{test_name}: cargo build failed: {e}").into() })?;

    Ok(std::path::PathBuf::from(exe))
}

/// **Regression for T0001**: `Terminal.appScreen` must accept
/// `onKey : KeyEvent -> Msg` where `KeyEvent = { kind : String,
/// value : String }` (a SINGLE-argument record handler).
///
/// Typing `onKey` as `String -> String -> Msg` (two curried String arguments)
/// would cause `IPE-T0001` at the `Terminal.appScreen` call site, since example
/// code uses the record-alias shape.
///
/// After the fix, the scheme PINS the key-event argument to the closed
/// record `{ kind : String, value : String }` (the Haskell reference types it
/// `any -> msg` and Go fails at runtime on non-KeyEvent handlers; we fail at
/// compile time — same sanctioned tightening as the Model / Msg gates).
/// The emitter generates a bridging wrapper:
///
/// ```text
/// |kind: String, value: String| Main_on_key(RecKindValue { kind, value })
/// ```
///
/// This test runs WITHOUT `IPE_E2E` (ipe-level only — no cargo build), so it
/// is always live in CI.
#[test]
fn tui_onkey_record_typechecks() {
    // ── helper: write Ipê source to a temp file, run ipe::build, check ok ──
    fn compile_ok(label: &str, source: &str) -> String {
        let ipe_dir = std::env::temp_dir().join(format!("tui_onkey_{label}_ipe"));
        let _ = std::fs::remove_dir_all(&ipe_dir);
        let created = std::fs::create_dir_all(&ipe_dir);
        assert!(
            created.is_ok(),
            "{label}: cannot create temp dir: {created:?}"
        );

        let entry = ipe_dir.join("Main.ipe");
        let wrote = std::fs::write(&entry, source);
        assert!(wrote.is_ok(), "{label}: cannot write Main.ipe: {wrote:?}");

        let out_dir = std::env::temp_dir().join(format!("tui_onkey_{label}_emitted"));
        let _ = std::fs::remove_dir_all(&out_dir);

        let Ok(runtime) = ipe::resolve_runtime() else {
            // Runtime unavailable — skip silently, matching the other goldens.
            return String::new();
        };

        let built = ipe::build(&entry, &out_dir, &runtime);
        assert!(
            built.is_ok(),
            "{label}: ipe build failed (T0001 regression?): {:?}",
            built.err()
        );

        // Return emitted main.rs text for structural assertions.
        std::fs::read_to_string(out_dir.join("src").join("main.rs"))
            .unwrap_or_else(|_| String::new())
    }

    // ── Terminal.appScreen with `onKey : KeyEvent -> Msg` ────────────────────
    let app_rs = compile_ok("terminal_app_screen", IPE_TUI_COUNTER);
    if app_rs.is_empty() {
        return; // runtime unavailable — structural assertions skipped
    }

    // The emitter must produce the bridging wrapper closure.
    assert!(
        app_rs.contains("|kind: String, value: String|"),
        "Terminal.appScreen emitted Rust must contain the `|kind: String, value: String|` \
         wrapper closure (onKey record bridge); got:\n{app_rs}"
    );
    // The record struct `RecKindValue` must be referenced inside the wrapper.
    assert!(
        app_rs.contains("RecKindValue"),
        "Terminal.appScreen emitted Rust must reference `RecKindValue` struct in the wrapper; \
         got:\n{app_rs}"
    );
}

/// Compile-only: the Ipe.Terminal counter emits a Cargo project with the `"tui"`
/// feature in the default feature list, `crossterm` and `unicode-width` deps,
/// and `ipe_runtime::tui::tui_app_ui` in the `main` function.
///
/// This is a BUILD-ONLY test — it does not spawn the binary (Tui requires a
/// real TTY).  A successful `cargo build` is the assertion:
///
/// * constrain: `Terminal.appScreen` correctly types the 5-field cfg with a
///   record-typed `onKey : KeyEvent -> Msg` handler.
/// * lower: the cfg record literal bypasses IPE-L0107 (same exemption
///   as `Web.app`).
/// * emit: `emit_tui_call` delegates to `tui_app_ui(…)` with the five
///   handler arguments correctly emitted, including the `|kind, value|` wrapper.
/// * manifest: `tui_cargo_toml` adds `"tui"` to default features,
///   `crossterm` + `unicode-width` deps, and `"sync"` to tokio.
///
/// # Errors
///
/// Propagates any pipeline or Cargo build failure as a test error.
#[test]
fn tui_counter_build_only() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        return Ok(());
    }

    // compile_and_build already does ipe + cargo build; success is the proof.
    let _exe = compile_and_build("tui_build_only", IPE_TUI_COUNTER)?;
    Ok(())
}

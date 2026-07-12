//! End-to-end tests for `Std.Tui` / `Sky.Tui` — `Tui.app`, `Ui.column`,
//! `Ui.el`, `Ui.text`, and `String.fromInt`.
//!
//! Non-E2E tests (no `SKY_E2E` required):
//! - `tui_onkey_record_typechecks` — skyc-level regression for the `onKey :
//!   KeyEvent -> Msg` record scheme fix (T0001); verifies both `Tui.app` and
//!   `Tui.program` accept a single-argument record-typed key handler and that
//!   the emitter generates the bridging wrapper closure.
//!
//! E2E tests (gated on `SKY_E2E=1`):
//! - `tui_counter_build_only` — full skyc + cargo build with `Tui.app` and
//!   a `KeyEvent -> Msg` handler (the pre-fix `String -> String -> Msg` curried
//!   shape is no longer valid under the updated scheme).
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

/// A minimal `Tui.app` counter exercising the updated Phase-1c scheme.
///
/// `onKey` is a SINGLE-argument record handler — `KeyEvent -> Msg` — matching
/// the Haskell reference scheme (`any -> msg`).  The emitter generates the
/// bridging closure:
///
/// ```text
/// |kind: String, value: String| Main_on_key(RecKindValue { kind, value })
/// ```
///
/// The curried `String -> String -> Msg` shape is no longer valid under the
/// updated scheme; it would unify `var(1)` (the msg type variable) with
/// `String -> Msg` which conflicts with its use in `update`/`subscriptions`.
///
/// Note: `view` returns `Element Msg` (NOT wrapped in `Ui.layout` → `Html Msg`
/// like Sky.Live).  The Tui runtime renders the Element tree directly to ANSI
/// cells; there is no HTML step.
const SKY_TUI_COUNTER: &str = r"module Main exposing (main)

import Std.Tui as Tui
import Std.Ui as Ui

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
    Tui.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , onKey = onKey
        }
";

/// Minimal `Tui.program` source exercising `onKey : KeyEvent -> Msg` (record
/// handler) and `view : Model -> String` (the Tui.program–specific view shape).
// Note: `r#"..."#` is needed because the Sky source contains `""` (an empty
// string literal) which would terminate a plain `r"..."` raw string early.
//
// Unlike `SKY_TUI_COUNTER`, this program does NOT pipe through `Task.run`
// at the `main` level — `Task.run : Task Error a -> a` keeps `a` polymorphic
// when `main` has no type annotation, causing SKY-L0102.  `Tui.program` already
// returns `Task Unit`, which is a concrete type the module entry accepts
// directly.
const SKY_TUI_PROGRAM_ONKEY_RECORD: &str = r#"module Main exposing (main)

import Std.Tui as Tui

type alias KeyEvent = { kind : String, value : String }

type Msg = NoOp

type alias Model = { dummy : Int }

init : () -> ( Model, Cmd Msg )
init _ =
    ( { dummy = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        NoOp ->
            ( model, Cmd.none )

view : Model -> String
view _ =
    ""

subscriptions : Model -> Sub Msg
subscriptions _ =
    Sub.none

onKey : KeyEvent -> Msg
onKey _ =
    NoOp

main =
    Tui.program
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , onKey = onKey
        }
"#;

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

/// **Regression for T0001 / examples 21–22**: both `Tui.app` and `Tui.program`
/// must accept `onKey : KeyEvent -> Msg` where `KeyEvent = { kind : String,
/// value : String }` (a SINGLE-argument record handler).
///
/// Before the fix, both schemes typed `onKey` as `String -> String -> Msg`
/// (two curried String arguments).  User code in the examples used the
/// record-alias shape, which caused `SKY-T0001` at the `Tui.program` /
/// `Tui.app` call site.
///
/// After the fix, both schemes PIN the key-event argument to the closed
/// record `{ kind : String, value : String }` (the Haskell reference types it
/// `any -> msg` and Go fails at runtime on non-KeyEvent handlers; we fail at
/// compile time — same sanctioned tightening as the Model / Msg gates).
/// The emitter generates a bridging wrapper:
///
/// ```text
/// |kind: String, value: String| Main_on_key(RecKindValue { kind, value })
/// ```
///
/// This test runs WITHOUT `SKY_E2E` (skyc-level only — no cargo build), so it
/// is always live in CI.
#[test]
fn tui_onkey_record_typechecks() {
    // ── helper: write Sky source to a temp file, run skyc::build, check ok ──
    fn compile_ok(label: &str, source: &str) -> String {
        let sky_dir = std::env::temp_dir().join(format!("tui_onkey_{label}_sky"));
        let _ = std::fs::remove_dir_all(&sky_dir);
        let created = std::fs::create_dir_all(&sky_dir);
        assert!(
            created.is_ok(),
            "{label}: cannot create temp dir: {created:?}"
        );

        let entry = sky_dir.join("Main.sky");
        let wrote = std::fs::write(&entry, source);
        assert!(wrote.is_ok(), "{label}: cannot write Main.sky: {wrote:?}");

        let out_dir = std::env::temp_dir().join(format!("tui_onkey_{label}_emitted"));
        let _ = std::fs::remove_dir_all(&out_dir);

        let Ok(runtime) = skyc::resolve_runtime() else {
            // Runtime unavailable — skip silently, matching the other goldens.
            return String::new();
        };

        let built = skyc::build(&entry, &out_dir, &runtime);
        assert!(
            built.is_ok(),
            "{label}: skyc build failed (T0001 regression?): {:?}",
            built.err()
        );

        // Return emitted main.rs text for structural assertions.
        std::fs::read_to_string(out_dir.join("src").join("main.rs"))
            .unwrap_or_else(|_| String::new())
    }

    // ── 1. Tui.app with `onKey : KeyEvent -> Msg` ────────────────────────────
    let app_rs = compile_ok("tui_app", SKY_TUI_COUNTER);
    if app_rs.is_empty() {
        return; // runtime unavailable — structural assertions skipped
    }

    // The emitter must produce the bridging wrapper closure.
    assert!(
        app_rs.contains("|kind: String, value: String|"),
        "Tui.app emitted Rust must contain the `|kind: String, value: String|` \
         wrapper closure (onKey record bridge); got:\n{app_rs}"
    );
    // The record struct `RecKindValue` must be referenced inside the wrapper.
    assert!(
        app_rs.contains("RecKindValue"),
        "Tui.app emitted Rust must reference `RecKindValue` struct in the wrapper; \
         got:\n{app_rs}"
    );

    // ── 2. Tui.program with `onKey : KeyEvent -> Msg` ────────────────────────
    let prog_rs = compile_ok("tui_program", SKY_TUI_PROGRAM_ONKEY_RECORD);

    assert!(
        prog_rs.contains("|kind: String, value: String|"),
        "Tui.program emitted Rust must contain the `|kind: String, value: String|` \
         wrapper closure; got:\n{prog_rs}"
    );
    assert!(
        prog_rs.contains("RecKindValue"),
        "Tui.program emitted Rust must reference `RecKindValue` struct; got:\n{prog_rs}"
    );
}

/// Compile-only: the Sky.Tui counter emits a Cargo project with the `"tui"`
/// feature in the default feature list, `crossterm` and `unicode-width` deps,
/// and `sky_runtime::tui::tui_app_ui` in the `main` function.
///
/// This is a BUILD-ONLY test — it does not spawn the binary (Tui requires a
/// real TTY).  A successful `cargo build` is the assertion:
///
/// * Phase-1c constrain: `Tui.app` correctly types the 5-field cfg with a
///   record-typed `onKey : KeyEvent -> Msg` handler.
/// * Phase-1c lower: the cfg record literal bypasses SKY-L0107 (same exemption
///   as `Live.app`).
/// * Phase-1c emit: `emit_tui_call` delegates to `tui_app_ui(…)` with the five
///   handler arguments correctly emitted, including the `|kind, value|` wrapper.
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

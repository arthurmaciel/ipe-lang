//! End-to-end tests for `Ipe.WebView` / `Ipe.WebView` — `Webview.app`,
//! `Ui.layout`, `Ui.column`, `Ui.el`, `Ui.text`, `Ui.button`, and
//! `String.fromInt`.
//!
//! All tests are gated on `IPE_E2E=1`.  Without it they return early so the
//! default `cargo test` stays fast.
//!
//! ## Architecture
//!
//! 1. A minimal Ipe.WebView counter program is written to a temp dir.
//! 2. `ipe::build` compiles it (parse → canon → types → lower → emit Rust).
//! 3. `e2e_support::build_rust_binary` runs `cargo build` on the emitted project —
//!    the shared Cargo target lets wry/tao/webkit2gtk compile once and be reused.
//!
//! ## Test tiers
//!
//! * **Tier-A** (`webview_counter_build_only`): ipe compile + `cargo build
//!   --features webview` links cleanly.  The `webview` feature is promoted to
//!   the default feature list by `project::webview_cargo_toml`, so a plain
//!   `cargo build` already uses it — no extra flag needed.  This is the
//!   minimum G5 assertion (constrain ↔ lower qualifier-set byte-match).
//!
//! * **Tier-B** (`webview_counter_tier_b`): the compiled binary is launched
//!   under `xvfb-run -a timeout 5` to exercise the native-window open path.
//!   A timeout exit (code 124) means the window stayed alive: pass.  A
//!   non-124 non-0 exit is reported as a loud failure.  The test
//!   **loud-skips** (prints a message + returns Ok) when `xvfb-run` is absent
//!   or `DISPLAY` / `WAYLAND_DISPLAY` is unavailable (headless CI environments).
//!
//! Run:
//!
//! ```text
//! IPE_E2E=1 cargo test webview_e2e
//! ```

/// A minimal Ipe.WebView counter app exercising the `Webview.app` wiring.
///
/// Kernels exercised:
/// - `Webview.app`   — constrain scheme + 5-field cfg with nested
///   `window = { title, size }` (G4 gate)
/// - `Ui.column`     — vertical layout container
/// - `Ui.el`         — generic element container with `Ui.onClick` event attr
/// - `Ui.onClick`    — binds a click event to a Msg constructor
/// - `Ui.text`       — text leaf node
/// - `String.fromInt` — displays the counter value
/// - `Cmd.none` / `Sub.none` — baseline TEA primitives
///
/// Note: `view` returns `Element Msg` — the same portable Ipe.Ui view as
/// Ipe.Web and Ipe.Tui. The framework applies `Ui.layout` internally to render
/// the Element tree through the Webview runtime's HTML renderer.
///
/// Note: `init` takes `()` (unit), matching `Ty::Unit` in the constrain
/// scheme.  Using `Ty::Tuple([])` (empty tuple) is NOT equivalent — the two
/// don't unify, surfacing as IPE-T0001.
///
/// Note: G3 — the emitted `fn main` uses `block_on_current_thread(ipe_main())`
/// (not `block_on`), enforced by the anchor-asserted switch in `project.rs`.
/// This keeps the tao/Cocoa event loop on the true main thread (macOS + Linux
/// both require it).
///
/// Note: `window = { title = "Counter", size = ( 400, 300 ) }` is an inline
/// record literal — required by the G4 gate in `emit_webview.rs`.
const IPE_WEBVIEW_COUNTER: &str = r#"module Main exposing (main)

import Ipe.Tea.WebView as Webview
import Ipe.Ui as Ui
import Ipe.Tea.WebView.Cmd as Cmd
import Ipe.Tea.WebView.Sub as Sub
import Ipe.String

type Msg
    = Increment
    | Decrement

type alias Model =
    { count : Int }

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

view : Model -> Element Msg
view model =
    Ui.column []
        [ Ui.el [ Ui.onClick Increment ] (Ui.text "+")
        , Ui.text (String.fromInt model.count)
        , Ui.el [ Ui.onClick Decrement ] (Ui.text "-")
        ]

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Webview.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , window = { title = "Counter", size = ( 400, 300 ) }
        }
"#;

/// Shared error type for E2E helpers.
type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Compile a Ipê program string, build the emitted Rust project, and return
/// the path to the compiled binary.
///
/// The emitted project has `webview` in its default feature list (set by
/// `project::webview_cargo_toml`) so `e2e_support::build_rust_binary` — which runs
/// a plain `cargo build` — picks up the `webview` feature without any extra
/// `--features` flag.
fn compile_and_build(test_name: &str, ipe_source: &str) -> Result<std::path::PathBuf, BoxError> {
    let ipe_dir = std::env::temp_dir().join(format!("webview_e2e_{test_name}_ipe"));
    let _ = std::fs::remove_dir_all(&ipe_dir);
    std::fs::create_dir_all(&ipe_dir).map_err(|e| -> BoxError {
        format!("{test_name}: cannot create ipe source dir: {e}").into()
    })?;

    let entry = ipe_dir.join("Main.ipe");
    std::fs::write(&entry, ipe_source)
        .map_err(|e| -> BoxError { format!("{test_name}: cannot write Main.ipe: {e}").into() })?;

    let out_dir = std::env::temp_dir().join(format!("webview_e2e_{test_name}_emitted"));
    let _ = std::fs::remove_dir_all(&out_dir);

    let runtime = ipe::resolve_runtime()
        .map_err(|e| -> BoxError { format!("{test_name}: runtime unavailable: {e}").into() })?;

    ipe::build(&entry, &out_dir, &runtime)
        .map_err(|e| -> BoxError { format!("{test_name}: ipe build failed: {e}").into() })?;

    let exe = e2e_support::build_rust_binary(test_name, &out_dir)
        .map_err(|e| -> BoxError { format!("{test_name}: cargo build failed: {e}").into() })?;

    Ok(std::path::PathBuf::from(exe))
}

/// Whether a `compile_and_build` failure is the Linux `wry`/`tao` link gap —
/// the system `webkit2gtk`/`glib` dev packages missing from `pkg-config`'s
/// search path — rather than a real codegen/link regression. Scoped to the
/// exact `pkg-config` "not found" signature `wry`'s `glib-sys`/`gobject-sys`/
/// `webkit2gtk-sys` build scripts emit, so an unrelated cargo build failure
/// (a genuine SEAL break) still fails the test loudly.
#[must_use]
fn is_missing_linux_webview_system_libs(err: &str) -> bool {
    err.contains("cargo build failed") && err.contains("pkg-config exited with status code 1")
}

/// Tier-A: ipe compiles the Ipe.WebView counter, the emitted Rust project
/// links (with the `webview` + `wry` + `tao` deps from the promoted default
/// features), and the binary exists.
///
/// Assertions:
/// - constrain: `Webview.app` correctly types the 5-field cfg
///   (`init/update/view/subscriptions/window` with nested `{ title, size }`).
/// - lower: the cfg record literal bypasses IPE-L0107 (same exemption
///   as `Web.app` and `Terminal.appScreen`).
/// - emit: `emit_webview_call` → `emit_webview_app_inner` (G4 gate:
///   `window` is inline record, `size` is inline 2-tuple) → `webview_app(…)`.
/// - manifest: `webview_cargo_toml` adds `"webview"` to default
///   features, wires `webview = ["dep:wry", "dep:tao"]`, appends `wry` + `tao`
///   optional deps, and the runtime `mod.rs` gets the `webview` module line.
/// - G3: the emitted `fn main` uses `block_on_current_thread(ipe_main())`
///   (anchor-asserted in `project::emit_program`; a zero-match aborts with
///   `CompilerBug` rather than silently shipping the wrong executor).
#[test]
fn webview_counter_build_only() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        return Ok(());
    }

    // compile_and_build does ipe + cargo build; a clean binary path is the proof.
    match compile_and_build("webview_build_only", IPE_WEBVIEW_COUNTER) {
        Ok(_exe) => Ok(()),
        Err(e) if is_missing_linux_webview_system_libs(&e.to_string()) => {
            // LOUD-SKIP, same posture as Tier-B's `xvfb-run`-absent skip:
            // `wry`/`tao` link against the system `webkit2gtk`/`glib` dev
            // packages on Linux, which this runner does not install
            // (`examples-sweep.yml` documents the same gap: "webview
            // examples don't build on ipe during phase 1"; `Ipe.WebView` is
            // macOS-first per CLAUDE.md). This is an environment gap, not a
            // codegen regression — THE SEAL (ipe exit-0 ⇒ cargo exit-0) is
            // unproven on Linux here, never asserted false-green.
            println!(
                "LOUD-SKIP: Tier-A (webview build) — system `webkit2gtk`/`glib` dev \
                 packages not installed on this runner (Ipe.WebView is macOS-first; \
                 Linux support is tracked, not yet CI-verified). Install \
                 `libwebkit2gtk-4.1-dev libglib2.0-dev` to run this test for real."
            );
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Tier-B: launch the compiled binary under `xvfb-run` to exercise the
/// native-window-open path.  A timeout (exit 124) means the window stayed alive
/// long enough — that is the success condition.
///
/// LOUD-SKIP conditions (prints a clear message, returns `Ok(())`):
/// - `xvfb-run` is not on `$PATH` (headless CI without a virtual framebuffer).
/// - `DISPLAY` and `WAYLAND_DISPLAY` are both unset AND `xvfb-run` is also
///   absent — belt-and-suspenders for Linux container environments.
///
/// The test is NOT a silent-green skip — it always prints whether it ran or
/// was skipped and why.
#[test]
fn webview_counter_tier_b() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        return Ok(());
    }

    // ── xvfb-run availability check ──────────────────────────────────────────
    let xvfb_available = std::process::Command::new("which")
        .arg("xvfb-run")
        .output()
        .is_ok_and(|o| o.status.success());

    if !xvfb_available {
        println!(
            "LOUD-SKIP: Tier-B (webview paint smoke) — `xvfb-run` not found on PATH. \
             Install `xvfb-run` (e.g. `apt-get install xvfb`) to run this test."
        );
        return Ok(());
    }

    // ── Compile + build ──────────────────────────────────────────────────────
    let exe = match compile_and_build("webview_tier_b", IPE_WEBVIEW_COUNTER) {
        Ok(exe) => exe,
        Err(e) if is_missing_linux_webview_system_libs(&e.to_string()) => {
            println!(
                "LOUD-SKIP: Tier-B (webview paint smoke) — system `webkit2gtk`/`glib` dev \
                 packages not installed on this runner (same gap as Tier-A). Install \
                 `libwebkit2gtk-4.1-dev libglib2.0-dev` to run this test for real."
            );
            return Ok(());
        }
        Err(e) => return Err(e),
    };

    // ── Spawn under xvfb-run with a hard timeout ─────────────────────────────
    // `timeout 5 <binary>` is wrapped by `xvfb-run -a` which provides a
    // virtual display.  Exit code 124 = timeout killed the process = the window
    // stayed open = initial view painted = pass.  Any other non-zero exit
    // (e.g. 1 = runtime Err from the webview stub) is a failure.
    let result = std::process::Command::new("xvfb-run")
        .arg("-a")
        .arg("timeout")
        .arg("5")
        .arg(&exe)
        .output()
        .map_err(|e| -> BoxError { format!("Tier-B: failed to spawn xvfb-run: {e}").into() })?;

    let exit_code = result.status.code().unwrap_or(-1);

    if exit_code == 124 || exit_code == 0 {
        // 124 = killed by `timeout` after 5 s (window stayed open — pass).
        // 0   = clean exit before timeout (unlikely for an event-loop app, but
        //       not a failure).
        println!("Tier-B webview paint smoke: exit={exit_code} (pass — window stayed alive)");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let stdout = String::from_utf8_lossy(&result.stdout);
        Err(format!(
            "Tier-B webview paint smoke FAILED: exit={exit_code}\n\
             --- stdout ---\n{stdout}\n\
             --- stderr ---\n{stderr}"
        )
        .into())
    }
}

/// The same counter, but with `window` let-bound instead of an inline record
/// literal. Without the fix, the let-bound `window` reaches emit and fires the G4
/// `Expr::Record` guard as a spanless `CompilerBug` (`IPE-I0001`); the
/// lower gate instead rejects it with `IPE-L0119` at the offending span.
const IPE_WEBVIEW_LET_BOUND_WINDOW: &str = r#"module Main exposing (main)

import Ipe.Tea.WebView as Webview
import Ipe.Ui as Ui
import Ipe.Tea.WebView.Cmd as Cmd
import Ipe.Tea.WebView.Sub as Sub
import Ipe.String

type Msg
    = Increment
    | Decrement

type alias Model =
    { count : Int }

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

view : Model -> Element Msg
view model =
    Ui.column []
        [ Ui.el [ Ui.onClick Increment ] (Ui.text "+")
        , Ui.text (String.fromInt model.count)
        , Ui.el [ Ui.onClick Decrement ] (Ui.text "-")
        ]

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    let win = { title = "Counter", size = ( 400, 300 ) } in
    Webview.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , window = win
        }
"#;

/// End-to-end guard: a let-bound `window` on `Webview.app` must produce a clean
/// user-facing `IPE-L0119` diagnostic during lowering — NOT an
/// internal-compiler-error (`IPE-I0001`) from the emit-stage G4 `Expr::Record`
/// guard. Compile-only (`ipe::build`) — no `cargo build`, so it runs fast and
/// needs no wry/tao toolchain, and it exercises the whole parse → canon → types
/// → lower pipeline end-to-end (unlike the isolated lower unit test).
#[test]
fn let_bound_webview_window_is_ipe_l0119_not_ice() {
    let dir = std::env::temp_dir().join("l0119_webview_window_ipe");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp ipe dir");
    let entry = dir.join("Main.ipe");
    std::fs::write(&entry, IPE_WEBVIEW_LET_BOUND_WINDOW).expect("write Main.ipe");

    let out = std::env::temp_dir().join("l0119_webview_window_out");
    let _ = std::fs::remove_dir_all(&out);
    let runtime = ipe::resolve_runtime().expect("runtime available");

    let err = ipe::build(&entry, &out, &runtime)
        .expect_err("a let-bound window must be rejected, not compiled");
    // Borrow `err` so it stays available for the assertion message. `None` means
    // the error was not a `Pipeline` diagnostic at all — a failure just as much as
    // the wrong code (so the assertion is non-vacuous on both axes).
    let code = match &err {
        ipe::CliError::Pipeline { diag, .. } => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        code,
        Some(ipe_diagnostics::IPE_L0119),
        "expected a IPE-L0119 Pipeline diagnostic (not an ICE), got {err:?}"
    );
}

//! Co-located WASI (`wasm32-wasip1`) accept-path SEAL (issue #2461).
//!
//! Obligations, all fail-closed by construction:
//!
//! * **THE SEAL (emit-side):** a `Direct`/`Script` (`main : Task Error ()`)
//!   program that reaches only the sealed WASI floor (`Ipe.Io` stdio) emits a
//!   project that `cargo build --target wasm32-wasip1` — `ipe`-accepts ⇒
//!   cargo-builds. Gated on `IPE_E2E=1` (the default `cargo test` stays fast +
//!   offline).
//! * **THE SEAL (user path):** the SAME guarantee through the real CLI selector
//!   — `ipe build --target wasi` on a sealed-floor `Direct` program produces a
//!   `wasm32-wasip1` module that built. This is the path an end user walks.
//! * **The refusal (emit-side):** a program reaching a NON-viable family
//!   (`Ipe.Http`, whose reqwest/`tokio/net` stack does not build on wasip1) is
//!   turned away at `ipe` time with a typed diagnostic (IPE-N0029), never
//!   emitted — so the unbuildable shape can never reach the wasip1 `cargo build`.
//! * **The refusal (user path):** `ipe build --target wasi` on a non-WASI-viable
//!   shape (a TEA `Web` app) is refused at delivery-resolve time with a typed
//!   diagnostic, before any emit — never a permissive default.
//!
//! The refusal tests run unconditionally (no cargo, no network): they are the
//! standing check that the sealed-floor gate stays real.

use std::path::{Path, PathBuf};
use std::process::Command;

use ipe::{BuildOptions, CliError};

/// A fresh, pid-isolated scratch dir under the test tempdir (concurrent runs
/// never share a tree).
fn scratch(name: &str) -> PathBuf {
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[allow(clippy::expect_used)] // test helper: a failed scratch-dir setup IS the failure
fn write_entry(dir: &Path, source: &str) -> PathBuf {
    std::fs::create_dir_all(dir).expect("mkdir scratch");
    let entry = dir.join("Main.ipe");
    std::fs::write(&entry, source).expect("write entry");
    entry
}

fn wasi_options() -> BuildOptions {
    BuildOptions {
        target: ipe_ir::Target::WasmWasi,
        ..BuildOptions::default()
    }
}

/// Emit `source` for the co-located WASI target into `out`.
#[allow(clippy::expect_used)] // test helper: an unresolvable runtime IS the failure
fn emit_wasi(entry: &Path, out: &Path) -> Result<(), CliError> {
    let runtime = ipe::resolve_runtime().expect("runtime must resolve");
    ipe::build_with_options(entry, out, &runtime, wasi_options())
}

/// A `Direct` script that reaches only the sealed WASI floor: `Ipe.Io` stdio +
/// pure `Ipe.String`. Its `main` is a plain `Task Error ()` — the ONE control
/// model the WASI engine carries.
const DIRECT_FLOOR_SOURCE: &str = "module Main exposing (main)\n\
     \n\
     import Ipe.Io as Io\n\
     import Ipe.String as String\n\
     \n\
     main : Task Error ()\n\
     main =\n\
     \x20   Io.println (String.fromInt 42)\n";

/// A `Direct` script reaching a NON-viable family (`Ipe.Http`): reqwest pulls
/// `tokio/net`→`mio`, which does not build on `wasm32-wasip1`. It MUST be
/// refused at `ipe` time so it never reaches the wasip1 `cargo build`.
const HTTP_SHAPE_SOURCE: &str = "module Main exposing (main)\n\
     \n\
     import Ipe.Io as Io\n\
     import Ipe.Http as Http\n\
     import Ipe.Url as Url\n\
     import Ipe.Task as Task\n\
     \n\
     main : Task Error ()\n\
     main =\n\
     \x20   case Url.fromString \"https://example.com\" of\n\
     \x20       Just url ->\n\
     \x20           Task.andThen (\\_resp -> Io.println \"done\") (Http.get url)\n\
     \x20\n\
     \x20       Nothing ->\n\
     \x20           Io.println \"bad url\"\n";

/// A `Web` TEA app — a view-ful loop pinned to the `ControlModel::Tea` model,
/// which is NOT WASI-viable (its runtime spine pulls tokio/axum). `--target
/// wasi` on it must be refused at delivery-resolve time, before any emit.
const WEB_TEA_SOURCE: &str = r"module Main exposing (main)

import Ipe.Tea.Web as Web
import Ipe.Ui as Ui
import Ipe.Tea.Web.Cmd
import Ipe.String
import Ipe.Tea.Web.Sub

type Msg = Increment

type alias Model = { count : Int }

init : WebReq -> ( Model, Cmd Msg )
init _req =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )

view : Model -> Element Msg
view model =
    Ui.text (String.fromInt model.count)

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Web.tea
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [], notFound = Increment
        }
";

/// THE SEAL: a sealed-floor `Direct` program emits a project that
/// `cargo build --target wasm32-wasip1` accepts. `ipe`-accepts ⇒ cargo-builds.
#[test]
fn wasi_direct_floor_program_cargo_builds_for_wasip1() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let dir = scratch("wasi_seal_floor");
    let entry = write_entry(&dir.join("srcdir"), DIRECT_FLOOR_SOURCE);
    let out = dir.join("out");

    // ipe accepts the sealed-floor program for the WASI target and emits it.
    emit_wasi(&entry, &out).expect("sealed-floor Direct program must ipe-accept for WASI");

    // Forward CI's warm shared target when present so the emitted crate's deps
    // reuse compiled artifacts; else isolate a per-slot target.
    let target_dir = e2e_support::child_shared_target_from_env()
        .map_or_else(|| out.join("target"), PathBuf::from);
    let mut cargo = Command::new("cargo");
    cargo
        .arg("build")
        .args(["--target", "wasm32-wasip1"])
        .current_dir(&out)
        .env("CARGO_TARGET_DIR", &target_dir)
        // Drop any ambient `RUSTFLAGS` / `CARGO_ENCODED_RUSTFLAGS` a dev host or
        // CI runner exports (e.g. `-C link-arg=-fuse-ld=mold`): a global
        // `RUSTFLAGS` OUTRANKS every `[target.<triple>] rustflags` config (cargo
        // picks the FIRST source that sets flags — env before config), so leaving
        // it set would mask the emitted crate's OWN `.cargo/config.toml`
        // wasip1-linker override. Cleared here, the child build is governed by
        // exactly the config the emitter ships — so a pass PROVES the emit-side
        // seal (the mold-free link an end user gets), never a test-only env patch.
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS");
    let output = cargo.output();
    let ok = matches!(&output, Ok(o) if o.status.success());
    assert!(
        ok,
        "THE SEAL: a sealed-floor WASI Direct program must cargo-build for \
         wasm32-wasip1 (ipe-accepts ⇒ cargo-builds); got {}",
        match &output {
            Ok(o) => format!(
                "status {:?}\n--- cargo stderr ---\n{}",
                o.status,
                String::from_utf8_lossy(&o.stderr)
            ),
            Err(e) => format!("cargo failed to spawn: {e}"),
        },
    );
    if e2e_support::child_shared_target_from_env().is_none() {
        let _ = std::fs::remove_dir_all(&target_dir);
    }
}

/// The refusal: a `Direct` program reaching a non-viable family (`Ipe.Http`) is
/// turned away at `ipe` time with a typed diagnostic — fail-closed, no emit, so
/// the unbuildable shape never reaches the wasip1 `cargo build`. Runs always.
#[test]
fn wasi_http_shape_is_refused_fail_closed() {
    let dir = scratch("wasi_seal_http_refusal");
    let entry = write_entry(&dir.join("srcdir"), HTTP_SHAPE_SOURCE);
    let out = dir.join("out");

    let err = emit_wasi(&entry, &out)
        .expect_err("an Http-reaching program must be REFUSED for the WASI sealed floor");

    // The refusal is the sealed-floor gate (IPE-N0029 server-only-kernel-for-wasm),
    // a typed pipeline diagnostic — never a cargo failure, never a silent emit.
    let rendered = format!("{err}");
    assert!(
        matches!(err, CliError::Pipeline { .. }),
        "the refusal must be a typed pipeline diagnostic, got: {rendered}",
    );
    // And nothing effectful was emitted: the accept-path never opened for it.
    assert!(
        !out.join("Cargo.toml").exists(),
        "a refused WASI shape must emit no project (fail-closed before emit)",
    );
}

/// THE SEAL through the USER selector: `ipe build --target wasi` on a
/// sealed-floor `Direct` program produces a `wasm32-wasip1` module that
/// `cargo build`s. This exercises the real CLI path — parse `--target wasi`,
/// resolve the compile target, gate through `admit_triple`, emit, and run the
/// wasip1 cross-compile — not the emit helper directly. `ipe`-accepts (exit 0)
/// ⇒ cargo-builds. Gated on `IPE_E2E=1`.
#[test]
fn ipe_build_target_wasi_user_path_cargo_builds() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let dir = scratch("wasi_seal_user_path");
    let entry = write_entry(&dir.join("srcdir"), DIRECT_FLOOR_SOURCE);
    let out = dir.join("out");

    // Forward CI's warm shared target so the emitted crate's deps reuse
    // compiled artifacts; else isolate a per-slot target. The wasip1 link is
    // governed by the emitter's own `.cargo/config.toml`, so a global
    // `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS` (which outranks a `[target.<triple>]`
    // config) is cleared for this process so the child cargo `run_cli` spawns
    // sees exactly the emitted config — a pass proves the end-user seal.
    let target_dir = e2e_support::child_shared_target_from_env()
        .map_or_else(|| out.join("target"), PathBuf::from);
    // SAFETY: nextest isolates each test in its own single-threaded-at-this-point
    // process, so this env mutation does not leak to other tests and no other
    // thread races these vars; the child cargo `run_cli` spawns inherits them.
    unsafe {
        std::env::set_var("CARGO_TARGET_DIR", &target_dir);
        std::env::remove_var("RUSTFLAGS");
        std::env::remove_var("CARGO_ENCODED_RUSTFLAGS");
    }

    let args = vec![
        "build".to_owned(),
        entry.to_string_lossy().into_owned(),
        "--out".to_owned(),
        out.to_string_lossy().into_owned(),
        "--target".to_owned(),
        "wasi".to_owned(),
    ];
    // THE SEAL: `run_cli` returns `Ok` ONLY if the wasip1 `cargo build`
    // succeeded — `bundle_wasi` runs `cargo build --target wasm32-wasip1` and
    // surfaces a non-zero exit as `CliError::EmittedBuildFailed`, so an `Ok`
    // here is the end-to-end proof that the `ipe`-accepted program cargo-builds
    // for the target through the real user selector. (The module artifact path
    // is `bundle_wasi`'s own concern; a green build is the seal.)
    let result = ipe::run_cli(&args);
    assert!(
        result.is_ok(),
        "THE SEAL (user path): `ipe build --target wasi` on a sealed-floor Direct \
         program must succeed (ipe-accepts ⇒ cargo-builds for wasm32-wasip1); got {result:?}",
    );

    if e2e_support::child_shared_target_from_env().is_none() {
        let _ = std::fs::remove_dir_all(&target_dir);
    }
}

/// The user-path refusal: `ipe build --target wasi` on a non-WASI-viable shape
/// (a `Web` TEA app) is refused fail-closed with a typed diagnostic at
/// delivery-resolve time — never a permissive default, never an emit. Runs
/// unconditionally (no cargo): the standing check the selector fails closed.
#[test]
fn ipe_build_target_wasi_refuses_non_viable_shape_fail_closed() {
    let dir = scratch("wasi_seal_user_refusal");
    let entry = write_entry(&dir.join("srcdir"), WEB_TEA_SOURCE);
    let out = dir.join("out");

    let args = vec![
        "build".to_owned(),
        entry.to_string_lossy().into_owned(),
        "--out".to_owned(),
        out.to_string_lossy().into_owned(),
        "--target".to_owned(),
        "wasi".to_owned(),
    ];
    let err = ipe::run_cli(&args)
        .expect_err("a Web TEA app must be REFUSED for --target wasi (not WASI-viable)");

    // The refusal is a typed usage diagnostic naming the non-Direct shape — the
    // `admit_triple` matrix's `WasiRequiresDirectShape` cell, surfaced through
    // the CLI. Never a cargo failure, never a silent native fallback.
    let rendered = format!("{err}");
    assert!(
        rendered.contains("wasm32-wasip1") && rendered.contains("Direct"),
        "the refusal must teach the WASI/Direct rule, got: {rendered}",
    );
    // Fail-closed before emit: nothing was written for the refused shape.
    assert!(
        !out.join("Cargo.toml").exists(),
        "a refused WASI user build must emit no project (fail-closed before emit)",
    );
}

// ── the RUN path (`ipe run --target wasi`, embedded wasmtime) ────────────────

/// THE SEAL for the run path (feature on): `ipe run --target wasi` on a
/// sealed-floor `Direct` program builds the `wasm32-wasip1` module AND executes
/// it under the embedded wasmtime engine, confined by a WASI context derived
/// from the program's declared capability floor. `run_cli` returns `Ok` ONLY
/// when the guest ran to a clean exit 0 — so an `Ok` here is the end-to-end
/// proof that the `ipe`-accepted program built for the target and ran correctly
/// under the deny-by-default context. Gated on `IPE_E2E=1` (default `cargo test`
/// stays fast + offline) AND on the `wasi_run` feature (the embedded engine).
#[cfg(feature = "wasi_run")]
#[test]
fn ipe_run_target_wasi_executes_under_wasmtime() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let dir = scratch("wasi_run_exec");
    let entry = write_entry(&dir.join("srcdir"), DIRECT_FLOOR_SOURCE);
    let out = dir.join("out");

    let target_dir = e2e_support::child_shared_target_from_env()
        .map_or_else(|| out.join("target"), PathBuf::from);
    // SAFETY: nextest isolates each test in its own process, so this env mutation
    // does not leak to other tests and no other thread races these vars; the
    // wasip1 cross-compile `run_cli` drives inherits them, and clearing
    // RUSTFLAGS keeps the emitter's own `.cargo/config.toml` linker override the
    // governing one (a global RUSTFLAGS outranks a `[target.<triple>]` config).
    unsafe {
        std::env::set_var("CARGO_TARGET_DIR", &target_dir);
        std::env::remove_var("RUSTFLAGS");
        std::env::remove_var("CARGO_ENCODED_RUSTFLAGS");
    }

    let args = vec![
        "run".to_owned(),
        entry.to_string_lossy().into_owned(),
        "--out".to_owned(),
        out.to_string_lossy().into_owned(),
        "--target".to_owned(),
        "wasi".to_owned(),
    ];
    let result = ipe::run_cli(&args);
    assert!(
        result.is_ok(),
        "THE SEAL (run path): `ipe run --target wasi` on a sealed-floor Direct \
         program must build the wasm32-wasip1 module and run it to a clean exit \
         under embedded wasmtime; got {result:?}",
    );

    if e2e_support::child_shared_target_from_env().is_none() {
        let _ = std::fs::remove_dir_all(&target_dir);
    }
}

/// The run-path refusal (non-viable shape): `ipe run --target wasi` on a `Web`
/// TEA app is refused fail-closed at delivery-resolve time — the SAME
/// `admit_triple` matrix `ipe build --target wasi` gates on, so the run path
/// never opens a looser door than build. Runs unconditionally (no cargo, no
/// engine): the standing check that the selector fails closed for the run path.
#[test]
fn ipe_run_target_wasi_refuses_non_viable_shape_fail_closed() {
    let dir = scratch("wasi_run_refuse_shape");
    let entry = write_entry(&dir.join("srcdir"), WEB_TEA_SOURCE);
    let out = dir.join("out");

    let args = vec![
        "run".to_owned(),
        entry.to_string_lossy().into_owned(),
        "--out".to_owned(),
        out.to_string_lossy().into_owned(),
        "--target".to_owned(),
        "wasi".to_owned(),
    ];
    let err = ipe::run_cli(&args)
        .expect_err("a Web TEA app must be REFUSED for `ipe run --target wasi` (not WASI-viable)");

    let rendered = format!("{err}");
    assert!(
        rendered.contains("wasm32-wasip1") && rendered.contains("Direct"),
        "the run-path refusal must teach the WASI/Direct rule, got: {rendered}",
    );
    assert!(
        !out.join("Cargo.toml").exists(),
        "a refused WASI run must emit no project (fail-closed before emit)",
    );
}

/// The run-path refusal (feature off): with `wasi_run` disabled, `ipe run
/// --target wasi` returns a typed refusal naming the missing feature — never a
/// panic, never a silent native fallback, and never a (wasted) wasip1 build.
/// Runs unconditionally when the feature is off; no cargo, no network.
#[cfg(not(feature = "wasi_run"))]
#[test]
fn ipe_run_target_wasi_feature_off_is_typed_refusal() {
    let dir = scratch("wasi_run_feature_off");
    let entry = write_entry(&dir.join("srcdir"), DIRECT_FLOOR_SOURCE);
    let out = dir.join("out");

    let args = vec![
        "run".to_owned(),
        entry.to_string_lossy().into_owned(),
        "--out".to_owned(),
        out.to_string_lossy().into_owned(),
        "--target".to_owned(),
        "wasi".to_owned(),
    ];
    let err = ipe::run_cli(&args)
        .expect_err("`ipe run --target wasi` without the wasi_run feature must be refused");

    assert!(
        matches!(err, CliError::WasiRunFeatureDisabled),
        "the feature-off refusal must be the typed WasiRunFeatureDisabled, got: {err:?}",
    );
    let rendered = format!("{err}");
    assert!(
        rendered.contains("wasi_run"),
        "the refusal must name the missing feature, got: {rendered}",
    );
    // Fail-closed BEFORE the (costly) wasip1 build: nothing was emitted.
    assert!(
        !out.join("Cargo.toml").exists(),
        "the feature-off refusal must fire before any emit (no wasted build)",
    );
}

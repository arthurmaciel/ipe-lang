//! `--target wasm` build gates (spec: `docs/adr/0042-wasm-client-target.md`).
//!
//! Layer 1 — a server-only kernel named in a wasm build fails at compile time
//! with IPE-N0029 (`NameError::ServerOnlyKernelForWasm`), never at cargo
//! time and never as a runtime stub. Layer 3 — the emitted wasm project's
//! manifest is the closed cdylib template: no tokio/axum/sqlx/reqwest, no
//! `server`/`db`/`live` feature. The full browser proof (cargo build to
//! `.wasm` + a Playwright interaction) lives in the examples flow
//! (`examples/wasm-counter`).

use std::path::{Path, PathBuf};

use ipe::{BuildOptions, CliError};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
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

fn wasm_options() -> BuildOptions {
    BuildOptions {
        target: ipe_ir::Target::WasmClient,
        ..BuildOptions::default()
    }
}

#[allow(clippy::expect_used)] // test helper: an unresolvable runtime IS the failure
fn build_wasm(entry: &Path, out: &Path) -> Result<(), CliError> {
    let runtime = ipe::resolve_runtime().expect("runtime must resolve");
    ipe::build_with_options(entry, out, &runtime, wasm_options())
}

/// A pure `Ipe.Ui` TEA app compiles under the wasm target, and the emitted
/// project is the browser cdylib shape (Layer 3: dependency floor).
#[test]
fn pure_ui_app_emits_wasm_project() {
    let dir = scratch("wasm_gate_green");
    let entry = write_entry(
        &dir.join("srcdir"),
        "module Main exposing (main)\n\
         import Ipe.Prelude exposing (..)\n\
         import Ipe.String as String\n\
         import Ipe.Live exposing (app)\n\
         import Ipe.Cmd as Cmd\n\
         import Ipe.Sub as Sub\n\
         import Ipe.Ui as Ui\n\
         \n\
         type Page = CounterPage\n\
         type Msg = Increment\n\
         type alias Model = { count : Int }\n\
         \n\
         init : a -> ( Model, Cmd Msg )\n\
         init _req = ( { count = 0 }, Cmd.none )\n\
         \n\
         update : Msg -> Model -> ( Model, Cmd Msg )\n\
         update msg model =\n\
         \x20   case msg of\n\
         \x20       Increment -> ( { model | count = model.count + 1 }, Cmd.none )\n\
         \n\
         subscriptions : Model -> Sub Msg\n\
         subscriptions _model = Sub.none\n\
         \n\
         view : Model -> Html Msg\n\
         view model =\n\
         \x20   Ui.layout []\n\
         \x20       (Ui.button [] { onPress = Just Increment, label = Ui.text (String.fromInt model.count) })\n\
         \n\
         main =\n\
         \x20   app\n\
         \x20       { init = init\n\
         \x20       , update = update\n\
         \x20       , view = view\n\
         \x20       , subscriptions = subscriptions\n\
         \x20       , routes = []\n\
         \x20       , notFound = CounterPage\n\
         \x20       }\n",
    );
    let out = dir.join("out");
    build_wasm(&entry, &out).expect("pure Ipe.Ui app must build under --target wasm");

    let manifest = std::fs::read_to_string(out.join("Cargo.toml")).expect("emitted manifest");
    assert!(manifest.contains("crate-type = [\"cdylib\"]"), "{manifest}");
    assert!(manifest.contains("wasm-bindgen"), "{manifest}");
    for absent in ["tokio", "axum", "sqlx", "reqwest", "rustls"] {
        assert!(
            !manifest.contains(absent),
            "wasm manifest must not link `{absent}`:\n{manifest}"
        );
    }
    let mod_rs =
        std::fs::read_to_string(out.join("src/ipe_runtime/mod.rs")).expect("emitted mod.rs");
    // `task` IS present as of M4 (the module's pure future-combinator half —
    // `Task.map`/`andThen`/… — has no tokio dependency; the tokio-bound half
    // stays `cfg(not(target_arch = "wasm32"))` INSIDE the file — see
    // `task.rs`'s module doc). `live`/`db`/`server` stay genuinely absent —
    // no substitute, no module declared.
    for absent in ["pub mod live;", "pub mod db;", "pub mod server;"] {
        assert!(
            !mod_rs.contains(absent),
            "wasm runtime module set must not declare `{absent}`:\n{mod_rs}"
        );
    }
    assert!(mod_rs.contains("pub mod wasm;"), "{mod_rs}");
    assert!(mod_rs.contains("pub mod task;"), "{mod_rs}");
    // The static browser shell is emitted beside the crate.
    assert!(out.join("www/index.html").is_file());
    assert!(out.join("www/boot.js").is_file());
    let index = std::fs::read_to_string(out.join("www/index.html")).expect("shell");
    assert!(index.contains("wasm-unsafe-eval"), "{index}");
    assert!(
        !index.contains(" 'unsafe-eval'"),
        "no bare JS unsafe-eval token:\n{index}"
    );
}

/// Naming a server-only kernel under `--target wasm` is a compile error
/// (IPE-N0029) — the kernel has no denotation, so no secret can gain a
/// client consumer and no cargo-time failure can occur (THE SEAL).
#[test]
#[allow(clippy::panic)] // test assertion: a non-pipeline error variant IS the failure
fn server_only_kernel_fails_at_compile_time() {
    let dir = scratch("wasm_gate_red");
    let entry = write_entry(
        &dir.join("srcdir"),
        "module Main exposing (main)\n\
         import Ipe.Prelude exposing (..)\n\
         import Ipe.File as File\n\
         import Ipe.Task as Task\n\
         \n\
         main =\n\
         \x20   File.readFile \"/etc/passwd\"\n",
    );
    let out = dir.join("out");
    let err = build_wasm(&entry, &out).expect_err("File.readFile must be denied under wasm");
    let CliError::Pipeline { diag, .. } = err else {
        panic!("expected a pipeline diagnostic, got: {err:?}");
    };
    let rendered = format!("{diag:?}");
    assert!(
        rendered.contains("ServerOnlyKernelForWasm"),
        "expected IPE-N0029 ServerOnlyKernelForWasm, got: {rendered}"
    );
}

/// M5 Layer 2 (IPE-N0030): a client entry that transitively imports a
/// server-classified module (never names the server kernel itself) fails
/// with the EXACT import chain — `Main(client) -> View(shared) ->
/// Data(server: imports File.readFile)` — not just "not allowed". End-to-end
/// through the real multi-module build pipeline (not just the
/// `ipe_canon::module_classify` unit tests).
#[test]
#[allow(clippy::panic)] // test assertion: a non-pipeline error variant IS the failure
fn transitive_server_import_fails_naming_the_exact_chain() {
    let dir = scratch("wasm_gate_transitive_chain");
    let srcdir = dir.join("srcdir");
    std::fs::create_dir_all(&srcdir).expect("mkdir scratch");
    std::fs::write(
        srcdir.join("Data.ipe"),
        "module Data exposing (load)\n\
         import Ipe.Prelude exposing (..)\n\
         import Ipe.File as File\n\
         import Ipe.Task as Task\n\
         \n\
         load : Task Error String\n\
         load = File.readFile \"/etc/passwd\"\n",
    )
    .expect("write Data.ipe");
    std::fs::write(
        srcdir.join("View.ipe"),
        "module View exposing (label)\n\
         import Ipe.Prelude exposing (..)\n\
         import Data exposing (load)\n\
         import Ipe.Task as Task\n\
         \n\
         label : String\n\
         label = \"view\"\n\
         \n\
         forceLink : Task Error String\n\
         forceLink = load\n",
    )
    .expect("write View.ipe");
    let entry = write_entry(
        &srcdir,
        "module Main exposing (main)\n\
         import Ipe.Prelude exposing (..)\n\
         import Ipe.Log as Log\n\
         import View exposing (label)\n\
         \n\
         main =\n\
         \x20   Log.println label\n",
    );
    let out = dir.join("out");
    let runtime = ipe::resolve_runtime().expect("runtime must resolve");
    let err =
        ipe::build_with_sibling_discovery_with_options(&entry, &out, &runtime, wasm_options())
            .expect_err("View -> Data's File.readFile must be denied transitively");
    let CliError::Pipeline { diag, .. } = err else {
        panic!("expected a pipeline diagnostic, got: {err:?}");
    };
    let rendered = format!("{diag:?}");
    assert!(
        rendered.contains("ServerModuleReachableFromWasmClient"),
        "expected IPE-N0030 ServerModuleReachableFromWasmClient, got: {rendered}"
    );
    assert!(
        rendered.contains("Main(client)")
            && rendered.contains("View(shared)")
            && rendered.contains("Data(server: imports File.readFile)"),
        "expected the exact import chain naming Main -> View -> Data, got: {rendered}"
    );
}

/// The same server-only program builds cleanly for the native target — the
/// gate is target-keyed, not a global restriction.
#[test]
fn server_only_kernel_still_builds_natively() {
    let dir = scratch("wasm_gate_native_ok");
    let entry = write_entry(
        &dir.join("srcdir"),
        "module Main exposing (main)\n\
         import Ipe.Prelude exposing (..)\n\
         import Ipe.File as File\n\
         import Ipe.Task as Task\n\
         \n\
         main =\n\
         \x20   File.readFile \"/etc/passwd\"\n",
    );
    let out = dir.join("out");
    let runtime = ipe::resolve_runtime().expect("runtime must resolve");
    ipe::build_with_options(&entry, &out, &runtime, BuildOptions::default())
        .expect("native build of the same program must stay green");
}

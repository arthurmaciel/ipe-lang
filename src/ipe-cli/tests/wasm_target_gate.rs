//! `--target wasm` build gates (spec: `docs/architecture/wasm-target.md`).
//!
//! Layer 1 — a server-only kernel named in a wasm build fails at compile time
//! with IPE-N0029 (`NameError::ServerOnlyKernelForWasm`), never at cargo
//! time and never as a runtime stub. Layer 3 — the emitted wasm project's
//! manifest is the closed cdylib template: no tokio/axum/sqlx/reqwest, no
//! `server`/`db`/`live` feature. The full browser proof (cargo build to
//! `.wasm` + a Playwright interaction) lives in the examples flow
//! (`examples/40-wasm-counter`).

use std::path::{Path, PathBuf};

use ipe::{BuildOptions, CliError};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

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
    for absent in [
        "pub mod task;",
        "pub mod live;",
        "pub mod db;",
        "pub mod server;",
    ] {
        assert!(
            !mod_rs.contains(absent),
            "wasm runtime module set must not declare `{absent}`:\n{mod_rs}"
        );
    }
    assert!(mod_rs.contains("pub mod wasm;"), "{mod_rs}");
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

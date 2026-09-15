//! Co-located WASI (`wasm32-wasip1`) accept-path SEAL (issue #2461, increment 2).
//!
//! Two obligations, both fail-closed by construction:
//!
//! * **THE SEAL (accept):** a `Direct`/`Script` (`main : Task Error ()`) program
//!   that reaches only the sealed WASI floor (`Ipe.Io` stdio) emits a project
//!   that `cargo build --target wasm32-wasip1` — `ipe`-accepts ⇒ cargo-builds.
//!   Gated on `IPE_E2E=1` (the default `cargo test` stays fast + offline).
//! * **The refusal:** a program reaching a NON-viable family (`Ipe.Http`,
//!   whose reqwest/`tokio/net` stack does not build on wasip1) is turned away at
//!   `ipe` time with a typed diagnostic (IPE-N0029), never emitted — so the
//!   unbuildable shape can never reach the wasip1 `cargo build`.
//!
//! The refusal test runs unconditionally (no cargo, no network): it is the
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

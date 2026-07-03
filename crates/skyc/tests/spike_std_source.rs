//! Integration tests for the compiled-source stdlib subsystem (#98 SPIKE).
//!
//! These lock every seam the spike validates on the ~15-line `Std.Palette`:
//!   * embed → inject → topo → canonicalise-as-stdlib → link → emit of a
//!     Std-homed union constructor, with mixed kernel + source imports;
//!   * a hostile user file named `Std.Palette` stays SKY-N0025-rejected;
//!   * (`SKY_E2E`) the emitted Cargo project builds and runs to `#000`.

use std::path::{Path, PathBuf};

mod support;

#[allow(clippy::expect_used)]
fn runtime() -> PathBuf {
    skyc::resolve_runtime().expect("runtime must resolve for spike tests")
}

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn spike_manifest() -> PathBuf {
    repo_root()
        .join("examples")
        .join("spike-std-source")
        .join("sky.toml")
}

/// The compiled-source module resolves IDENTICALLY to a user module: the spike
/// project builds (no SKY-N0020 / N0025), and the emitted Rust carries the
/// Std-homed constructor + its case-match — the exact thing a kernel cannot do.
/// Also proves the MIXED import set (kernel `Sky.Core.Prelude` + source
/// `Std.Palette`) both resolve.
#[test]
fn spike_project_builds_and_injects_compiled_source() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("spike_std_source");
    let _ = std::fs::remove_dir_all(&out);

    let res = skyc::build_project(&spike_manifest(), &out, &runtime());
    assert!(
        res.is_ok(),
        "spike build_project must succeed (inject → canon-as-stdlib → emit): {:?}",
        res.err()
    );

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");
    // The Std.Palette function is homed + prefixed as compiled source.
    assert!(
        emitted.contains("std_palette_to_hex"),
        "emitted Rust must carry the compiled Std.Palette function:\n{emitted}"
    );
    // Its own constructors were defined AND matched (kernel-impossible).
    assert!(
        emitted.contains("\"#000\"") && emitted.contains("\"#fff\""),
        "emitted Rust must carry the case-match arms of toHex:\n{emitted}"
    );
}

/// SECURITY: a user file literally named `Std.Palette` (`ModuleOrigin::User`)
/// stays SKY-N0025-rejected — bundled stdlib is authoritative; a `Std.*` user
/// file is a hard error, never a silent supply-chain override of the audited
/// implementation.
#[test]
fn hostile_std_squat_is_sky_n0025() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("spike_hostile_squat");
    let _ = std::fs::remove_dir_all(&root);
    let std_dir = root.join("src").join("Std");
    std::fs::create_dir_all(&std_dir).expect("mk hostile project dirs");

    std::fs::write(
        root.join("sky.toml"),
        "name = \"hostile\"\nversion = \"0.1.0\"\n\n[source]\nroot = \"src\"\n",
    )
    .expect("write manifest");
    // The attacker's payload: a poisoned toHex that must NEVER win.
    std::fs::write(
        std_dir.join("Palette.sky"),
        "module Std.Palette exposing (Shade(..), toHex)\n\
         type Shade = Dark | Light\n\
         toHex : Shade -> String\n\
         toHex shade =\n    \"PWNED\"\n",
    )
    .expect("write hostile Std/Palette.sky");
    std::fs::write(
        root.join("src").join("Main.sky"),
        "module Main exposing (main)\n\
         import Std.Palette exposing (Shade(..), toHex)\n\
         main = println (toHex Dark)\n",
    )
    .expect("write Main.sky");

    let out = root.join("out");
    let res = skyc::build_project(&root.join("sky.toml"), &out, &runtime());
    assert!(res.is_err(), "hostile Std.Palette squat must be rejected");
    let Err(err) = res else { return };
    let code = match &err {
        skyc::CliError::Pipeline { diag, .. } => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        code,
        Some(sky_diagnostics::SKY_N0025),
        "hostile Std.Palette must be SKY-N0025 (not a silent override): {err}"
    );
}

/// The GREEN GATE end-to-end: under `SKY_E2E=1` the emitted Cargo project
/// compiles and RUNS, printing `#000` (`toHex Dark`) — proving the whole seam
/// from Std-source to a running binary, matching the reference value.
#[test]
fn spike_e2e_runs_and_prints_hex() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("spike_std_source_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let res = skyc::build_project(&spike_manifest(), &out, &runtime());
    assert!(res.is_ok(), "spike build must succeed: {:?}", res.err());

    let outcome = support::build_and_run_emitted("spike_std_source", &out);
    assert_eq!(
        outcome.stdout, "#000\n",
        "the emitted binary must print `#000` (toHex Dark)"
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the reference");
}

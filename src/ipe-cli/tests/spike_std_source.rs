//! Integration tests for the compiled-source stdlib subsystem.
//!
//! These lock every seam on the ~15-line `Ipe.Palette`:
//!   * embed → inject → topo → canonicalise-as-stdlib → link → emit of a
//!     Std-homed union constructor, with mixed kernel + source imports;
//!   * a hostile user file named `Ipe.Palette` stays IPE-N0025-rejected;
//!   * an `EmbeddedStdlib` module DEFINES a reserved built-in type name
//!     (`type Length`) that a user module could not — the prereq
//!     for compiled-source `Ipe.Css`;
//!   * (`IPE_E2E`) the emitted Cargo project builds and runs to `#000 42`.

use std::path::{Path, PathBuf};

mod support;

#[allow(clippy::expect_used)]
fn runtime() -> PathBuf {
    ipe::resolve_runtime().expect("runtime must resolve for spike tests")
}

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn spike_manifest() -> PathBuf {
    repo_root()
        .join("tests")
        .join("fixtures")
        .join("spike-std-source")
        .join("ipe.toml")
}

/// The compiled-source module resolves IDENTICALLY to a user module: the
/// project builds (no IPE-N0020 / N0025), and the emitted Rust carries the
/// Std-homed constructor + its case-match — the exact thing a kernel cannot do.
/// Also proves the MIXED import set (kernel `Ipe.Prelude` + source
/// `Ipe.Palette`) both resolve.
#[test]
fn spike_project_builds_and_injects_compiled_source() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("spike_std_source");
    let _ = std::fs::remove_dir_all(&out);

    let res = ipe::build_project(&spike_manifest(), &out, &runtime());
    assert!(
        res.is_ok(),
        "spike build_project must succeed (inject → canon-as-stdlib → emit): {:?}",
        res.err()
    );

    // Read `main.rs` PLUS every `ipe_mods/*.rs` the per-Ipê-module split
    // may have written: the compiled `Ipe.Palette` source
    // is emitted into its own `ipe_mods/ipe_mod_std_palette.rs`, not inline
    // in `main.rs`. The shared helper keeps the substring assertions below
    // robust to WHICH file the split placed each symbol in (same discrimination
    // the golden harness uses).
    let emitted = support::read_all_emitted_src(&out);
    // The Ipe.Palette function is homed + prefixed as compiled source.
    assert!(
        emitted.contains("ipe_palette_to_hex"),
        "emitted Rust must carry the compiled Ipe.Palette function:\n{emitted}"
    );
    // Its own constructors were defined AND matched (kernel-impossible).
    assert!(
        emitted.contains("\"#000\"") && emitted.contains("\"#fff\""),
        "emitted Rust must carry the case-match arms of toHex:\n{emitted}"
    );
    // The EmbeddedStdlib module defines a RESERVED built-in type name
    // (`type Length`). The lowerer keys it under its real home (`Ipe.Palette`),
    // so it lowers to its OWN enum + accessor — NOT the opaque runtime
    // `UiPlain::Length`. A user module declaring `type Length` would have been
    // IPE-N0026-rejected before ever reaching lowering.
    assert!(
        emitted.contains("ipe_palette_length_px"),
        "emitted Rust must carry the compiled Ipe.Palette `lengthPx` fn (reserved-name type defined by trusted stdlib):\n{emitted}"
    );
    assert!(
        !emitted.contains("UiPlain :: Length") && !emitted.contains("UiPlain::Length"),
        "the stdlib-defined `type Length` must lower to its OWN enum, never the opaque UiPlain::Length hijack:\n{emitted}"
    );
}

/// SECURITY: a user file literally named `Ipe.Palette` (`ModuleOrigin::User`)
/// stays IPE-N0025-rejected — bundled stdlib is authoritative; a `Ipe.*` user
/// file is a hard error, never a silent supply-chain override of the audited
/// implementation.
#[test]
fn hostile_std_squat_is_ipe_n0025() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("spike_hostile_squat");
    let _ = std::fs::remove_dir_all(&root);
    let std_dir = root.join("src").join("Ipe");
    std::fs::create_dir_all(&std_dir).expect("mk hostile project dirs");

    std::fs::write(
        root.join("ipe.toml"),
        "name = \"hostile\"\nversion = \"0.1.0\"\n\n[source]\nroot = \"src\"\n",
    )
    .expect("write manifest");
    // The attacker's payload: a poisoned toHex that must NEVER win, AND an
    // attempt to squat a RESERVED built-in type name (`Length`) inside the
    // reserved `Ipe.` namespace. The unforgeable `ModuleOrigin::User` tag
    // means this file gets NEITHER the N0025 namespace exemption NOR the N0026
    // reserved-builtin exemption — the namespace gate (N0025) fires first, so a
    // hostile author can never obtain the `EmbeddedStdlib`-only capability.
    std::fs::write(
        std_dir.join("Palette.ipe"),
        "module Ipe.Palette exposing (Shade(..), toHex, Length(..))\n\
         type Shade = Dark | Light\n\
         type Length = Px Int\n\
         toHex : Shade -> String\n\
         toHex shade =\n    \"PWNED\"\n",
    )
    .expect("write hostile Ipe/Palette.ipe");
    std::fs::write(
        root.join("src").join("Main.ipe"),
        "module Main exposing (main)\n\
         import Ipe.Palette exposing (Shade(..), toHex)\n\
         main = Io.println (toHex Dark)\n",
    )
    .expect("write Main.ipe");

    let out = root.join("out");
    let res = ipe::build_project(&root.join("ipe.toml"), &out, &runtime());
    assert!(res.is_err(), "hostile Ipe.Palette squat must be rejected");
    let Err(err) = res else { return };
    let code = match &err {
        ipe::CliError::Pipeline { diag, .. } => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        code,
        Some(ipe_diagnostics::IPE_N0025),
        "hostile Ipe.Palette must be IPE-N0025 (not a silent override): {err}"
    );
}

/// The GREEN GATE end-to-end: under `IPE_E2E=1` the emitted Cargo project
/// compiles and RUNS, printing `#000` (`toHex Dark`) — proving the whole seam
/// from Std-source to a running binary, matching the reference value.
#[test]
fn spike_e2e_runs_and_prints_hex() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("spike_std_source_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let res = ipe::build_project(&spike_manifest(), &out, &runtime());
    assert!(res.is_ok(), "spike build must succeed: {:?}", res.err());

    let outcome = support::build_and_run_emitted("spike_std_source", &out);
    assert_eq!(
        outcome.stdout, "#000 42\n",
        "the emitted binary must print `#000 42` (toHex Dark + lengthPx (Px 42))"
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the reference");
}

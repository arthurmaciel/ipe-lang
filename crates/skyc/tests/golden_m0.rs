//! Milestone-0 end-to-end gate: `skyc` must emit `main.rs` byte-identical to the
//! Haskell-reference golden, and (behind `SKY_E2E=1`) the emitted project must
//! build and print `1`.

use std::path::{Path, PathBuf};

mod support;

/// The `sky-rust` workspace root (two levels up from this crate's manifest).
fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

#[test]
fn emits_byte_identical_main_rs_and_vendors_runtime() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("m0")
        .join("Main.sky");
    let golden = root.join("tests").join("golden").join("m0").join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m0_emit");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    // Directory-diff the emitted project against the golden dir: asserts the
    // emitted `src/main.rs` (and the golden's checked-in root `Cargo.toml`)
    // match byte-for-byte. Replaces the former hand-rolled `read_to_string` +
    // `assert_eq!` pair — proven equal in discriminating power side by side
    // before that block was removed.
    support::assert_emitted_project_matches_golden_dir(
        &out,
        golden.parent().expect("golden has a parent dir"),
    );

    assert!(
        out.join("src")
            .join("sky_runtime")
            .join("core.rs")
            .is_file(),
        "runtime module tree must be vendored",
    );
}

/// Full spine: compile, build the emitted Cargo project, and run it. Gated on
/// `SKY_E2E=1` so the default `cargo test` stays fast (the emitted project pulls
/// real crates and takes ~1 min to compile cold).
#[test]
fn end_to_end_builds_and_prints_one() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("m0")
        .join("Main.sky");
    // Build OUTSIDE the workspace tree: an emitted project under the workspace's
    // own target/ dir is (correctly) rejected by cargo as a non-member package,
    // and the golden Cargo.toml carries no detaching `[workspace]` stanza.
    let out = std::env::temp_dir().join("skyc_m0_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let outcome = support::build_and_run_emitted("m0", &out);
    support::assert_go_parity(
        "m0",
        &repo_root().join("tests").join("golden").join("m0"),
        &outcome.stdout,
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the Go oracle");
}

/// A `CliError::Pipeline` must render as a coded, rustc/Elm-style report — not a
/// `{:?}` debug dump. We feed `build` a deliberately ill-formed `.sky` source and
/// assert the displayed error carries an `error[SKY-…]` header and the
/// `skyc explain` footer pointer.
#[test]
fn pipeline_error_renders_with_code_and_explain_pointer() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pipeline_err");
    let _ = std::fs::create_dir_all(&dir);
    let entry = dir.join("Bad.sky");
    // Malformed: a top-level declaration with no right-hand side. This is
    // rejected at the parse stage boundary, the first pipeline stage.
    let wrote = std::fs::write(&entry, "module Main exposing (main)\n\nmain =\n");
    assert!(wrote.is_ok(), "must write ill-formed source");

    // The runtime dir is never reached: the parse error fires first. Pass a
    // path that need not exist.
    let runtime = dir.join("no-runtime");
    let built = skyc::build(&entry, &dir.join("out"), &runtime);

    assert!(
        matches!(&built, Err(skyc::CliError::Pipeline { .. })),
        "expected a pipeline error, got: {built:?}"
    );
    let Err(err) = built else { return };

    let rendered = err.to_string();
    assert!(
        rendered.contains("error[SKY-"),
        "rendered error must carry a coded header, got:\n{rendered}"
    );
    assert!(
        rendered.contains("skyc explain"),
        "rendered error must point at `skyc explain`, got:\n{rendered}"
    );
}

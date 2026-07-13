//! Milestone-3b-3 character literal patterns: `case c of 'a' -> … ; 'b' -> … ;
//! _ -> …`. Char is an OPEN type, so a wildcard catch-all completes the cover.
//! `skyc` must emit `main.rs` byte-identical to the checked-in golden, and
//! (behind `SKY_E2E=1`) the emitted project must build and print `2`.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `2\n`, exit 0 — hand-verified in a temp dir.
use std::path::{Path, PathBuf};

mod support;

use support::repo_root;

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m3b3_char")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m3b3_char")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m3b3_char_emit");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    // Directory-diff the emitted project against the golden dir (byte-compares
    // the emitted `src/main.rs` against the golden `main.rs`). Replaces the
    // former hand-rolled `read_to_string` + `assert_eq!` pair with the shared
    // harness helper.
    support::assert_emitted_project_matches_golden_dir(
        &out,
        golden.parent().expect("golden has a parent dir"),
    );
}

/// Full spine: compile, build, run, assert stdout `2` — the Go-backend value.
/// Gated on `SKY_E2E=1` so the default `cargo test` stays fast.
#[test]
fn end_to_end_builds_and_prints_two() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m3b3_char_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let outcome = support::build_and_run_emitted("m3b3_char", &out);
    support::assert_go_parity(
        "m3b3_char",
        &repo_root().join("tests").join("golden").join("m3b3_char"),
        &outcome.stdout,
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the Go oracle");
}

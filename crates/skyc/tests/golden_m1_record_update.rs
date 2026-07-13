//! Milestone-1 record-update gate: `skyc` must emit `main.rs` byte-identical to
//! the checked-in golden for a program that builds a record, functionally
//! updates one field, and reads both records' fields, and (behind `SKY_E2E=1`)
//! the emitted project must build and print `43`.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `43\n`, exit 0 — verified by hand in a temp dir (so the
//! Go build artifacts never touch the reference tree):
//!
//! ```text
//! $ cd "$(mktemp -d)" && sky run Main.sky   # Go backend
//! 43
//! ```
//!
//! `p = { x = 1, y = 2 }`; `q = { p | x = 41 }`; the entry prints
//! `q.x + p.y = 43` — proving the update replaced `x` (41) and left `p`
//! untouched (`p.y = 2`). The `end_to_end_*` test below asserts the Rust
//! backend reaches the identical `43`. Running the Go toolchain inside
//! `cargo test` is impractical (it needs the Haskell `sky` binary plus a Go
//! toolchain), so the hand-verified value is the in-test oracle, documented
//! here against the Go-equivalent command.

use std::path::{Path, PathBuf};

mod support;

use support::repo_root;

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m1_record_update")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m1_record_update")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m1_record_update_emit");
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

/// Full spine: compile, build the emitted Cargo project, run it, and assert the
/// record-update program prints `43` — the same value the Go backend produces.
/// Gated on `SKY_E2E=1` so the default `cargo test` stays fast.
#[test]
fn end_to_end_builds_and_prints_forty_three() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m1_record_update_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let outcome = support::build_and_run_emitted("m1_record_update", &out);
    support::assert_go_parity(
        "m1_record_update",
        &repo_root()
            .join("tests")
            .join("golden")
            .join("m1_record_update"),
        &outcome.stdout,
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the Go oracle");
}

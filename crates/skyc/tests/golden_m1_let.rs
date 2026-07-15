//! Milestone-1 `let … in` gate: `skyc` must emit `main.rs` byte-identical to the
//! checked-in golden for single- and multi-binding `let`s, and (behind
//! `SKY_E2E=1`) the emitted project must build and print `22`.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `22\n`, exit 0 — verified by hand:
//!
//! ```text
//! $ sky run tests/golden/m1_let/Main.sky   # Go backend
//! 22
//! ```
//!
//! `double 5 = 5 + 5 = 10`; `triple 4`'s multi-binding let computes `a = 4 + 4 =
//! 8`, then `b = a + 4 = 12` (a later binding reads an earlier one); the entry's
//! inline `let total = double 5 + triple 4` is `10 + 12 = 22`. The Rust
//! `end_to_end_*` test below asserts the Rust backend reaches the identical `22`.
//! Running the Go toolchain inside `cargo test` is impractical (it needs the
//! Haskell `sky` binary plus a Go toolchain), so the hand-computed value is the
//! in-test oracle, documented here against the Go-equivalent command.

use std::path::{Path, PathBuf};

mod support;

use support::repo_root;

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m1_let")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m1_let")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m1_let_emit");
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
        support::golden_dir_of(&golden),
    );
}

/// Full spine: compile, build the emitted Cargo project, run it, and assert the
/// `let`-bound arithmetic prints `22` — the same value the Go backend produces.
/// Gated on `SKY_E2E=1` so the default `cargo test` stays fast.
#[test]
fn end_to_end_builds_and_prints_twenty_two() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m1_let_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let outcome = support::build_and_run_emitted("m1_let", &out);
    support::assert_go_parity(
        "m1_let",
        &repo_root().join("tests").join("golden").join("m1_let"),
        &outcome.stdout,
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the Go oracle");
}

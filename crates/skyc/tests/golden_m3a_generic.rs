//! Milestone-3a generic-instantiation gate: ONE generic enum used at two
//! distinct element types in one module. `skyc` must emit `main.rs`
//! byte-identical to the checked-in golden, and (behind `SKY_E2E=1`) the emitted
//! project must build and print `42`.
//!
//! `type Opt a = Som a | Non` is constructed both as `Opt Int` (`Som 41`) and as
//! `Opt Bool` (`Som (1 == 1)`); the backend emits a single `MainOpt<T1>` that
//! Rust monomorphises at each use site — the M3a generic-ADT spine.
//!
//! ```text
//! orElse o d = case o of Som x -> x ; Non -> d
//! pick o     = case o of Som b -> b ; Non -> 0 == 1
//! main = ... orElse (Som 41) 0 ... pick (Som (1 == 1)) ...   -- prints 42
//! ```
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `42\n`, exit 0 — hand-verified in a temp dir, where the
//! Go backend emits the matching generic `MainOpt[T1 any]` enum instantiated at
//! both `int` and `bool`. The hand-computed `42` is the in-test oracle.

use std::path::{Path, PathBuf};

mod support;

use support::repo_root;

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m3a_generic")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m3a_generic")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m3a_generic_emit");
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
/// ADT program prints `42` — the same value the Go backend produces. Gated on
/// `SKY_E2E=1` so the default `cargo test` stays fast. This is the
/// soundness-floor regression for a value laundered through a generic /
/// payload-carrying enum.
#[test]
fn end_to_end_builds_and_prints_forty_two() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m3a_generic_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let outcome = support::build_and_run_emitted("m3a_generic", &out);
    support::assert_go_parity(
        "m3a_generic",
        &repo_root().join("tests").join("golden").join("m3a_generic"),
        &outcome.stdout,
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the Go oracle");
}

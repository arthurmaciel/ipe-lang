//! Milestone-1 lambda + application gate: `skyc` must emit `main.rs`
//! byte-identical to the checked-in golden for anonymous functions (`\x -> e`,
//! multi-parameter `\a b -> e`, and an outer-local capture), and (behind
//! `SKY_E2E=1`) the emitted project must build and print `62`.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `62\n`, exit 0 — verified by hand:
//!
//! ```text
//! $ sky run tests/golden/m1_lambdas/Main.sky   # Go backend
//! 62
//! ```
//!
//! `inc 41` applies the let-bound lambda `\x -> x + 1` → `42`; `(\x -> x + n) 5`
//! applies an inline lambda that captures the outer local `n = 10` → `15`;
//! `add 2 3` applies the multi-parameter lambda `\a b -> a + b` → `5`. The
//! entry's `let r = inc 41 + (\x -> x + n) 5 + add 2 3` is `42 + 15 + 5 = 62`.
//! Running the Go toolchain inside `cargo test` is impractical (it needs the
//! Haskell `sky` binary plus a Go toolchain), so the hand-computed value is the
//! in-test oracle, documented here against the Go-equivalent command.

use std::path::{Path, PathBuf};

mod support;

/// The `sky-rust` workspace root (two levels up from this crate's manifest).
fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m1_lambdas")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m1_lambdas")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m1_lambdas_emit");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {:?}", runtime.err());
    let Ok(runtime) = runtime else { return };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"));
    let want = std::fs::read_to_string(&golden);
    assert!(emitted.is_ok() && want.is_ok(), "both files must read");
    assert_eq!(
        emitted.ok(),
        want.ok(),
        "emitted main.rs must equal the golden byte-for-byte"
    );
}

/// Full spine: compile, build the emitted Cargo project, run it, and assert the
/// lambda-driven arithmetic prints `62` — the same value the Go backend
/// produces. Gated on `SKY_E2E=1` so the default `cargo test` stays fast.
#[test]
fn end_to_end_builds_and_prints_sixty_two() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m1_lambdas_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let outcome = support::build_and_run_emitted("m1_lambdas", &out);
    support::assert_go_parity(
        "m1_lambdas",
        &repo_root().join("tests").join("golden").join("m1_lambdas"),
        &outcome.stdout,
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the Go oracle");
}

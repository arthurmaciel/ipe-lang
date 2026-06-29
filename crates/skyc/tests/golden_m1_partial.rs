//! Milestone-1 partial / over-application gate: `skyc` must emit `main.rs`
//! byte-identical to the checked-in golden for curried partial application
//! (eta-expansion) and over-application (saturation), and (behind `SKY_E2E=1`)
//! the emitted project must build and print `15`.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `15\n`, exit 0 — verified by hand:
//!
//! ```text
//! $ sky run tests/golden/m1_partial/Main.sky   # Go backend
//! 15
//! ```
//!
//! `let f = add 2 in f 3` partially applies the two-parameter `add`: `add 2`
//! eta-expands to `\eta_0 -> add(2, eta_0)`, and `f 3` → `5`. `over 1 2`
//! over-applies the one-parameter `over : Int -> Int -> Int` (`over a = \b ->
//! a + b`): the first arg saturates `over(1)` and the surplus `2` applies to the
//! returned closure → `3`. `applyTwice (add 1) 5` passes the partial `add 1` as
//! a first-class function and applies it twice: `add 1 (add 1 5)` → `7`. The
//! entry prints `p + o + h = 5 + 3 + 7 = 15`. Running the Go toolchain inside
//! `cargo test` is impractical (it needs the Haskell `sky` binary plus a Go
//! toolchain), so the hand-computed value is the in-test oracle, documented here
//! against the Go-equivalent command.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The `sky-rust` workspace root (two levels up from this crate's manifest).
fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m1_partial")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m1_partial")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m1_partial_emit");
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
/// partial/over-application arithmetic prints `15` — the same value the Go
/// backend produces. Gated on `SKY_E2E=1` so the default `cargo test` stays
/// fast.
#[test]
fn end_to_end_builds_and_prints_fifteen() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m1_partial_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let status = Command::new("cargo")
        .arg("build")
        .current_dir(&out)
        .env("CARGO_TARGET_DIR", out.join("target"))
        .status();
    assert!(
        matches!(&status, Ok(s) if s.success()),
        "emitted project must build: {status:?}"
    );

    let bin = out.join("target").join("debug").join("sky-app");
    let output = Command::new(&bin).output();
    assert!(
        output.is_ok(),
        "emitted binary must run: {:?}",
        output.as_ref().err()
    );
    let Ok(output) = output else { return };
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "15\n",
        "program prints 15 (Go-backend parity)"
    );
    assert!(output.status.success(), "exit 0, matching the Go oracle");
    let _ = std::fs::remove_dir_all(out.join("target"));
}

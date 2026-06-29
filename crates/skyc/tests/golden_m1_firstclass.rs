//! Milestone-1 first-class-function gate: `skyc` must emit `main.rs`
//! byte-identical to the checked-in golden for higher-order functions (a
//! function-typed parameter applied inside the callee), a top-level function
//! passed as a value by name, and a top-level function returned as a value —
//! and (behind `SKY_E2E=1`) the emitted project must build and print `51`.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `51\n`, exit 0 — verified by running it in a temp dir:
//!
//! ```text
//! $ sky run tests/golden/m1_firstclass/Main.sky   # Go backend
//! 51
//! ```
//!
//! `applyTwice : (Int -> Int) -> Int -> Int` applies its function-typed
//! parameter twice: `applyTwice (\n -> n + 3) 1` is `(1+3)+3 = 7` (a lambda
//! passed as a value) and `applyTwice inc 1` is `(1+1)+1 = 3` (the top-level
//! `inc` passed by name — reified into a boxed closure). `makeInc 0` returns the
//! top-level `inc` as a value, bound to `g`; `g 40` is `41`. The entry's total
//! is `7 + 3 + 41 = 51`. Running the Go toolchain inside `cargo test` is
//! impractical (it needs the Haskell `sky` binary plus a Go toolchain), so the
//! hand-computed value is the in-test oracle, documented here against the
//! Go-equivalent command.

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
        .join("m1_firstclass")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m1_firstclass")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m1_firstclass_emit");
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
/// first-class-function arithmetic prints `51` — the same value the Go backend
/// produces. Gated on `SKY_E2E=1` so the default `cargo test` stays fast.
#[test]
fn end_to_end_builds_and_prints_fifty_one() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m1_firstclass_e2e");
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
        "51\n",
        "program prints 51 (Go-backend parity)"
    );
    assert!(output.status.success(), "exit 0, matching the Go oracle");
}

//! Milestone-1 nested-lambda flattening: a one-parameter binding whose body is
//! a curried lambda chain (`f a = \b -> \c -> a + b + c`) declared with a
//! multi-arrow type (`Int -> Int -> Int -> Int`). `skyc` must emit `main.rs`
//! byte-identical to the checked-in golden, and (behind `SKY_E2E=1`) the emitted
//! project must build and print `6`.
//!
//! The lowerer flattens the nested lambda chain into a single multi-parameter
//! closure so the emitted `Box<dyn Fn(i64, i64) -> i64>` body matches the
//! flattened return type — without the flatten the body would be a curried
//! `Box<dyn Fn(i64) -> Box<dyn Fn(i64) -> i64>>` that cargo rejects with no
//! Sky-level diagnostic. The program exercises BOTH application reshapes against
//! the flattened closure:
//!
//! * exact-then-Apply — `let h = f 1 in h 2 3`: `f 1` saturates the declared
//!   parameter and returns the two-argument closure; `h 2 3` applies it exactly;
//! * over-applied — `f 1 2 3`: `f 1` saturates and the surplus `2 3` apply to
//!   its result through one trailing `Apply` (`(main_f(1))(2, 3)`).
//!
//! Both paths compute `1 + 2 + 3 = 6`; the entry prints the shared value, so a
//! divergence between the two reshapes would change the output.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `6\n`, exit 0 — verified by hand:
//!
//! ```text
//! $ sky run tests/golden/m1_nested_lambda/Main.sky   # Go backend
//! 6
//! ```
//!
//! Running the Go toolchain inside `cargo test` is impractical (it needs the
//! Haskell `sky` binary plus a Go toolchain), so the hand-computed value is the
//! in-test oracle, documented here against the Go-equivalent command.

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
        .join("m1_nested_lambda")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m1_nested_lambda")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m1_nested_lambda_emit");
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
/// nested-lambda flattening prints `6` — the same value the Go backend produces.
/// Gated on `SKY_E2E=1` so the default `cargo test` stays fast.
#[test]
fn end_to_end_builds_and_prints_six() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m1_nested_lambda_e2e");
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
        "6\n",
        "program prints 6 (Go-backend parity)"
    );
    assert!(output.status.success(), "exit 0, matching the Go oracle");
    let _ = std::fs::remove_dir_all(out.join("target"));
}

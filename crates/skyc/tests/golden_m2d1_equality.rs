//! Milestone-2d-1 Equatable-bound parity gate: a generic function whose body
//! compares its arguments with `==` / `/=` (`eq2 p q = p == q`) generalises to a
//! Rust generic bounded by `PartialEq` — and, unlike the Number / Comparable
//! paths, carries NO `Copy`, because `PartialEq::eq` borrows its operands.
//!
//! `eq2 : a -> a -> Bool` emits `pub fn main_eq2<T1: PartialEq>(p: T1, q: T1) ->
//! bool`. It is used at `Int` (`eq2 21 21`) for the runtime output and
//! instantiated at `Bool` (through the annotated forwarder `eqBool`) so the
//! bound is exercised at two types in one module — `main.rs` must be
//! byte-identical to the checked-in golden, and (behind `SKY_E2E=1`) the emitted
//! project must build and print `42`.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `42\n`, exit 0:
//!
//! ```text
//! $ cd "$(mktemp -d)" && sky run Main.sky   # Go backend
//! 42
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m2d1_equality")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m2d1_equality")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m2d1_equality_emit");
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
/// Equatable-generic program prints `42` — the value the Go backend produces.
/// Gated on `SKY_E2E=1` so the default `cargo test` stays fast.
#[test]
fn end_to_end_builds_and_prints_forty_two() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m2d1_equality_e2e");
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
        "42\n",
        "program prints 42 (Go-backend parity)"
    );
    assert!(output.status.success(), "exit 0, matching the Go oracle");
    let _ = std::fs::remove_dir_all(out.join("target"));
}

//! Milestone-3a recursion-soundness gate: MUTUALLY-recursive ADTs whose size
//! cycle is closed only through enum-payload edges (`Even -> Odd -> Even`).
//! `skyc` must emit `main.rs` byte-identical to the checked-in golden, and
//! (behind `SKY_E2E=1`) the emitted project must build and print `5`.
//!
//! ```text
//! type Even = EZero | ESucc Odd
//! type Odd  = OSucc Even
//! ```
//!
//! Neither enum is *directly* self-recursive — `Even` carries an `Odd` and
//! `Odd` carries an `Even` — so the pre-fix backend (which boxed only a direct
//! self-edge) emitted two enums that are each infinite-sized in Rust (E0072):
//! `skyc` exited 0 and the emitted crate then failed `cargo build`. The fix
//! boxes at least one enum-payload edge of every type-size cycle, so each enum
//! stays finite-sized, balanced by `Box::new` at construction and a deref at
//! pattern binding.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `5\n`, exit 0 — hand-verified in a temp dir. This is
//! the soundness-floor regression for a value laundered through a boxed
//! mutually-recursive payload, pinning the indirect-cycle gap so it can never
//! regress to the silent exit-0-then-cargo-fail mode.
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m3a_mutual_recursion")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m3a_mutual_recursion")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m3a_mutual_recursion_emit");
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
/// mutually-recursive ADT program prints `5` — the value the Go backend
/// produces. Gated on `SKY_E2E=1` so the default `cargo test` stays fast. This
/// is the soundness-floor regression for an indirect (mutual) recursion cycle:
/// before the fix the crate did not build at all.
#[test]
fn end_to_end_builds_and_prints_five() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m3a_mutual_recursion_e2e");
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
        "5\n",
        "program prints 5 (Go-backend parity)"
    );
    assert!(output.status.success(), "exit 0, matching the Go oracle");
    let _ = std::fs::remove_dir_all(out.join("target"));
}

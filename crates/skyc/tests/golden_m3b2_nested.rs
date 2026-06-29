//! Milestone-3b-2 record + nested patterns, end to end. The fixture exercises,
//! in one program:
//!
//! * a nested TUPLE sub-pattern inside a constructor payload (`Wrap (a, c)`),
//! * nested wildcard sub-patterns in a recursive constructor payload
//!   (`Node _ x _`),
//! * a record pattern as a single irrefutable `case` arm (`{ x, y }`), and
//! * irrefutable `let` destructure binders — a tuple `(a, b)` and a record
//!   `{ x, y }`.
//!
//! `skyc` must emit `main.rs` byte-identical to the checked-in golden, and
//! (behind `SKY_E2E=1`) the emitted project must build and print `83`.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `83\n`, exit 0 — hand-verified in a temp dir. The
//! hand-computed `3 + 5 + 42 + 33 = 83` is the in-test oracle.
use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m3b2_nested")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m3b2_nested")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m3b2_nested_emit");
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
/// program prints `83` — the same value the Go backend produces. Gated on
/// `SKY_E2E=1` so the default `cargo test` stays fast. This is the soundness-floor
/// regression for record + nested pattern lowering.
#[test]
fn end_to_end_builds_and_prints_eighty_three() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m3b2_nested_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let outcome = support::build_and_run_emitted("m3b2_nested", &out);
    assert_eq!(
        outcome.stdout, "83\n",
        "program prints 83 (Go-backend parity)"
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the Go oracle");
}

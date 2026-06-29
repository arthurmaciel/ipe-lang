//! Milestone-3b-1 parenthesised field-access gate (closes #6): `(expr).field`.
//! Field access on a *non-identifier* atom — a parenthesised expression — must
//! parse as a postfix `.field` access, matching the Go reference. `skyc` must
//! emit `main.rs` byte-identical to the checked-in golden, and (behind
//! `SKY_E2E=1`) the emitted project must build and print `42`.
//!
//! ```text
//! wrap n = { value = n }
//! r = { value = 1 }
//! main = println (String.fromInt ((wrap 41).value + (r).value))  -- 42
//! ```
//!
//! `(wrap 41).value` covers field access on a *call* result; `(r).value` covers
//! field access on a parenthesised local variable. The pre-fix parser rejected
//! both with SKY-P0011 (`stray '.'`).
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `42\n`, exit 0 — hand-verified in a temp dir (so the Go
//! build artifacts never touch the reference tree):
//!
//! ```text
//! $ cd "$(mktemp -d)" && sky run Main.sky   # Go backend
//! 42
//! ```
use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m3b1_dotfield")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m3b1_dotfield")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m3b1_dotfield_emit");
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
/// parenthesised field-access program prints `42` — the same value the Go
/// backend produces. Gated on `SKY_E2E=1` so the default `cargo test` stays
/// fast.
#[test]
fn end_to_end_builds_and_prints_forty_two() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m3b1_dotfield_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let outcome = support::build_and_run_emitted("m3b1_dotfield", &out);
    support::assert_go_parity(
        "m3b1_dotfield",
        &repo_root()
            .join("tests")
            .join("golden")
            .join("m3b1_dotfield"),
        &outcome.stdout,
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the Go oracle");
}

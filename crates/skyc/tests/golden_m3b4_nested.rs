//! Milestone-3b-4 same-top-constructor nested discrimination, end to end.
//!
//! Several `case` arms head-match the SAME top-level constructor (`Som`) and
//! discriminate on a nested constructor sub-pattern (`Som (Som x)` vs `Som Non`)
//! before a final `Non` arm. Each Sky arm lowers to its OWN Rust `match` arm in
//! source order; Rust's `match` resolves the overlap and ordering natively, so
//! there is no one-arm-per-constructor grouping.
//!
//! `skyc` must emit `main.rs` byte-identical to the checked-in golden, and
//! (behind `SKY_E2E=1`) the emitted project must build and print `52`.
//!
//! Behavioural-parity oracle: the Go reference compiler at
//! `/home/arthur/Documentos/comp/sky/sky-out/sky` compiles + runs the SAME
//! `Main.sky` to stdout `52\n`, exit 0 — hand-verified in a temp dir. The
//! hand-computed `42 + 7 + 3 = 52` is the in-test oracle.
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn example_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("m3b4_nested_opt")
        .join("Main.sky")
}

#[test]
fn emits_byte_identical_main_rs() {
    let root = repo_root();
    let entry = example_entry(&root);
    let golden = root
        .join("tests")
        .join("golden")
        .join("m3b4_nested_opt")
        .join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m3b4_nested_opt_emit");
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
/// program prints `52` — the same value the Go backend produces. Gated on
/// `SKY_E2E=1` so the default `cargo test` stays fast. This is the soundness-floor
/// regression for same-top-constructor nested discrimination.
#[test]
fn end_to_end_builds_and_prints_fifty_two() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = example_entry(&root);
    let out = std::env::temp_dir().join("skyc_m3b4_nested_opt_e2e");
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
        "52\n",
        "program prints 52 (Go-backend parity)"
    );
    assert!(output.status.success(), "exit 0, matching the Go oracle");
}

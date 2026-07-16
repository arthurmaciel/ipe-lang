//! #224 destructure move-ownership — SEAL regression.
//!
//! A `let (a, b) = pair` destructure binder whose CloneOk component `a` is read
//! TWICE by value in the body. Pre-fix, destructure was the ONE binder kind
//! that never invoked the count/clone/relay machinery, so the emitted
//! `string_append(a, string_append(a, b))` moved `a` twice → skyc-0 but
//! cargo-101 (E0382 use of moved value). The fix routes every destructure
//! component through the shared `apply_move_ownership` entry point, cloning the
//! non-last consuming read.
//!
//! THE SEAL: skyc-0 ⇒ cargo-0.
//!
//! Run:
//! ```text
//! cargo test -p skyc --test golden_i224_destructure_move_ownership
//! SKY_E2E=1 cargo test -p skyc --test golden_i224_destructure_move_ownership
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn entry_path(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("i224_destructure_move_ownership")
        .join("Main.sky")
}

/// skyc-0: the compiler accepts the program AND the reused destructure component
/// `a` is cloned on its non-last consuming read.
#[test]
fn i224_destructure_skyc_accepts_and_clones_reused_component() {
    let root = repo_root();
    let entry = entry_path(&root);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("i224_destructure_skyc_out");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP i224_destructure_move_ownership: runtime not available");
        return;
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i224_destructure_move_ownership: {:?}",
        built.err()
    );

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");

    // The reused destructure component `a` must clone on its non-last use — the
    // structural signature of the move-ownership discipline reaching a
    // destructure binder.
    assert!(
        emitted.contains("a.clone()"),
        "reused destructure component `a` must have a `.clone()` in emitted \
         Rust; got:\n{emitted}"
    );
}

/// cargo-0 ∧ run-correct: gated on `SKY_E2E=1` — THE SEAL for #224.
#[test]
fn i224_destructure_cargo_builds_and_runs() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = entry_path(&root);
    let out = std::env::temp_dir().join("skyc_i224_destructure_move_ownership_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(built.is_ok(), "skyc build must succeed: {:?}", built.err());

    let outcome = support::build_and_run_emitted("i224_destructure_move_ownership", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "i224_destructure_move_ownership must exit 0 (no E0382); stdout: {:?}",
        outcome.stdout
    );
    // combine ("x", "y") = append "x" (append "x" "y") = "xxy".
    assert!(
        outcome.stdout.contains("xxy"),
        "must print xxy; got: {:?}",
        outcome.stdout
    );
}

//! Match-arm clone relay at n == 1 — SEAL regression.
//!
//! A match-arm-bound variable (`name`, CloneOk `String`) read EXACTLY ONCE but
//! through TWO nested `move`-closure boundaries. Nesting the match-arm binder
//! site's type resolution inside an `if n > 1` guard would leave the arm var's
//! type unresolved at `n == 1`, so the per-boundary relay never runs → the
//! outer closure move-captures `name` out of the enclosing `Fn` env → skyc-0
//! but cargo-101 (E0507). So type resolution is hoisted out of the guard and
//! the arm var routes through the shared `apply_move_ownership` entry point,
//! whose `rewrite_multiuse_clones` installs the relay at n == 1.
//!
//! THE SEAL: skyc-0 ⇒ cargo-0.
//!
//! Run:
//! ```text
//! cargo test -p skyc --test golden_i222_match_arm_clone_relay
//! SKY_E2E=1 cargo test -p skyc --test golden_i222_match_arm_clone_relay
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
        .join("i222_match_arm_clone_relay")
        .join("Main.sky")
}

/// skyc-0: the compiler accepts the program AND relays `name` across the
/// intermediate boundary with a pre-clone shadow.
#[test]
fn i222_match_arm_skyc_accepts_and_relays() {
    let root = repo_root();
    let entry = entry_path(&root);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("i222_match_arm_skyc_out");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP i222_match_arm_clone_relay: runtime not available");
        return;
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i222_match_arm_clone_relay: {:?}",
        built.err()
    );

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");

    // The relay: a pre-clone shadow `let name = name.clone()` sits at the
    // intermediate boundary before the inner lambda so `name` is not moved out
    // of the enclosing `Fn` env.
    assert!(
        emitted.contains("let name = name.clone()"),
        "arm-bound `name` read once through two boundaries must get a \
         per-boundary clone relay; got:\n{emitted}"
    );
}

/// cargo-0 ∧ run-correct: gated on `SKY_E2E=1` — THE SEAL.
#[test]
fn i222_match_arm_cargo_builds_and_runs() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = entry_path(&root);
    let out = std::env::temp_dir().join("skyc_i222_match_arm_clone_relay_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(built.is_ok(), "skyc build must succeed: {:?}", built.err());

    let outcome = support::build_and_run_emitted("i222_match_arm_clone_relay", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "i222_match_arm_clone_relay must exit 0 (no E0507); stdout: {:?}",
        outcome.stdout
    );
    // handle (Just "bob") = "bob" ++ "A" ++ "B" = "bobAB".
    assert!(
        outcome.stdout.contains("bobAB"),
        "must print bobAB; got: {:?}",
        outcome.stdout
    );
}

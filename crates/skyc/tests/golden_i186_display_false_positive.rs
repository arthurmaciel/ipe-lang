//! BACKLOG #186 FALSE-POSITIVE regression — the general kernel->bound map must
//! NOT over-bound.
//!
//! The `Display` obligation (#186) is decided per-param by the EXACT `toString`
//! argument position (arg 0), mirroring #177's `SkyRow` per-row-arg precision.
//! A wildcard `any` param used ONLY as a Db row (which correctly gains `SkyRow`)
//! must NOT ALSO gain a spurious `Display` bound just because a SIBLING concrete
//! `String` param is `toString`'d in the same body.
//!
//! Why this matters: a gratuitous `T: std::fmt::Display` on a Dict-shaped row
//! (a `HashMap<String, String>`, which is NOT `Display`) would be an outright
//! E0277 SEAL violation the moment a caller instantiates the function with the
//! Dict payload — exactly the over-bounding risk a broadened kernel->bound map
//! introduces. This probe asserts the wildcard row generic carries `SkyRow`
//! (its real obligation) but NO `Display`, and that the whole crate builds and
//! runs end-to-end.
//!
//! Run:
//! ```text
//! SKY_E2E=1 cargo test -p skyc --test golden_i186_display_false_positive
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
        .join("i186_display_false_positive")
        .join("Main.sky")
}

/// skyc-0 ∧ the wildcard row generic carries `SkyRow` but NOT `Display` —
/// checked unconditionally (cheap, no `cargo`).
#[test]
fn i186_false_positive_skyc_no_spurious_display() {
    let root = repo_root();
    let entry = entry_path(&root);
    let out =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("i186_display_false_positive_skyc_out");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP i186_display_false_positive: runtime not available");
        return;
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i186_display_false_positive: {:?}",
        built.err()
    );

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");

    // The `grab` fn's wildcard row generic gets `SkyRow` (its real obligation)
    // and must NOT get `Display` (the sibling `String` is what is toString'd).
    let grab_sig = emitted
        .lines()
        .find(|l| l.contains("pub fn main_grab"))
        .unwrap_or_else(|| panic!("emitted main.rs must declare main_grab; got:\n{emitted}"));
    assert!(
        grab_sig.contains("sky_runtime::db::SkyRow"),
        "the wildcard row generic must carry its real `SkyRow` obligation; got: {grab_sig}"
    );
    assert!(
        !grab_sig.contains("std::fmt::Display"),
        "the wildcard row generic must NOT gain a spurious `Display` bound from a \
         SIBLING `String`'s `toString` — that would be over-bounding (and E0277 on \
         a Dict-shaped row); got: {grab_sig}"
    );
}

/// cargo-0 ∧ run-0 — the only check that would catch a spurious-`Display`-on-Dict
/// E0277. Gated on `SKY_E2E=1`.
#[test]
fn i186_false_positive_cargo_builds_and_runs() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = entry_path(&root);
    let out = std::env::temp_dir().join("skyc_i186_display_false_positive_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        panic!("runtime must resolve for E2E");
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for i186_display_false_positive: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted("i186_display_false_positive", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "i186_display_false_positive binary must cargo-build AND exit 0 (no \
         spurious-Display E0277); got {:?} (stdout: {:?})",
        outcome.exit_code,
        outcome.stdout
    );
    assert_eq!(
        outcome.stdout.trim(),
        "p:v",
        "stdout mismatch; got: {:?}",
        outcome.stdout
    );
}

//! Phase-5 Milestone-C pilot fixture: a deliberately MULTI-MODULE program
//! (`Main` + `Lib`) that ALSO uses `Std.Db`, captured at TODAY's (pre-split)
//! single-file `main.rs` shape.
//!
//! This is the ONE fixture that exercises the multi-module split AND the
//! `SqlValue`/`SqlField` Spine-routing (design doc §2.2) together. Milestone C
//! (Task 12, a separate follow-up) flips `emit_program` to real per-module
//! output and rewrites these goldens into the Spine + `sky_mods/*` shape; for
//! now the golden is the HONEST current single-file baseline (home-qualified
//! names like `lib_label`/`main_summary` are already present today — §1.3,
//! existing behaviour, not new emission).
//!
//! Modelled on `golden_m0.rs`, but using the shared directory-diff helper
//! `support::assert_emitted_project_matches_golden_dir` from the start (this
//! fixture is NEW — it never used the retired hand-rolled assertion).

use std::path::{Path, PathBuf};

mod support;

/// The `sky-rust` workspace root (two levels up from this crate's manifest).
fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn fixture_dir() -> PathBuf {
    repo_root()
        .join("tests")
        .join("golden")
        .join("multi_mod_split_pilot")
}

// `runtime()` is non-`#[test]` scaffolding — `expect` is the idiomatic way to
// fail loudly on a broken environment (mirrors `golden_mm.rs`'s own helper).
#[allow(clippy::expect_used)]
fn runtime() -> PathBuf {
    skyc::resolve_runtime().expect("runtime must resolve for the pilot golden test")
}

#[test]
fn emits_byte_identical_single_file_main_rs_today() {
    let fixture = fixture_dir();
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("multi_mod_split_pilot");
    let _ = std::fs::remove_dir_all(&out);

    let res = skyc::build_project(&fixture.join("sky.toml"), &out, &runtime());
    assert!(res.is_ok(), "build_project failed: {:?}", res.err());

    // Directory-diff the emitted project against the checked-in golden dir:
    // asserts the emitted `src/main.rs` AND the golden `Cargo.toml` match
    // byte-for-byte. At TODAY's single-file shape this is the whole emitted
    // Rust surface for the two-module + Std.Db program.
    support::assert_emitted_project_matches_golden_dir(&out, &fixture);
}

/// Full spine (gated on `SKY_E2E=1`): compile, build the emitted Cargo
/// project, and run it. Proves THE SEAL — the multi-module + `Std.Db` emitted
/// project actually `cargo build`s and runs. The two rows seeded in
/// `Lib.seedAndCount` are counted and printed as `seeded:2`.
#[test]
fn end_to_end_builds_and_prints_seeded_count() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let fixture = fixture_dir();
    // Build OUTSIDE the workspace tree (an emitted project under the
    // workspace target/ is rejected by cargo as a non-member package).
    let out = std::env::temp_dir().join("skyc_multi_mod_split_pilot_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let res = skyc::build_project(&fixture.join("sky.toml"), &out, &runtime());
    assert!(res.is_ok(), "build_project failed: {:?}", res.err());

    let outcome = support::build_and_run_emitted("multi_mod_split_pilot", &out);
    assert_eq!(
        outcome.stdout.trim_end(),
        "seeded:2",
        "the two seeded rows must be counted and printed via the cross-module summary"
    );
    assert_eq!(outcome.exit_code, Some(0), "exit 0");
}

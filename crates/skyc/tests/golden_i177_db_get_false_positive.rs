//! FALSE-POSITIVE guard — the structural IR-level detection.
//!
//! Deciding the `+ SkyRow` bound on a wildcard-`any` param by
//! a TEXT scan of the emitted Rust body (`rendered_body.contains("db_get_")`)
//! false-positives on text that is NOT a
//! runtime row accessor, appending `+ SkyRow` — a reference to the
//! `#[cfg(feature = "db")]` trait `sky_runtime::db::SkyRow` — to a crate that
//! never imports `Std.Db`. Result: `skyc` exit 0, then `cargo` fails with
//! `error[E0433]: could not find db in sky_runtime`. A SEAL violation
//! (skyc-0-then-cargo-fail).
//!
//! Two minimal well-typed repros:
//!   * probe C (`i177_db_get_false_positive_string_literal`): a wildcard-`any`
//!     fn whose body contains the STRING LITERAL `"db_get_string called on …"`.
//!   * probe D (`i177_db_get_false_positive_user_symbol`): a benign user fn
//!     `dbGetLabel` lowering to `main_db_get_label`, called from a
//!     wildcard-`any` fn.
//!
//! The fix replaces the text scan with a STRUCTURAL walk of the lowered IR
//! (`sky_lower`'s `body_calls_db_get_on_param`): the bound lands ONLY when the
//! body contains an actual `Db.get*` KERNEL application whose ROW argument is a
//! `Var`/`CloneVar` reference to the wildcard param. Neither a string literal
//! nor a user symbol is such an application, so NEITHER probe gains the bound —
//! both now build clean end-to-end.
//!
//! Run:
//! ```text
//! SKY_E2E=1 cargo test -p skyc --test golden_i177_db_get_false_positive
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn entry_path(root: &Path, fixture: &str) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join(fixture)
        .join("Main.sky")
}

/// skyc-0 ∧ NO spurious `SkyRow` bound — checked unconditionally (cheap, no
/// `cargo`). A `SkyRow` reference in the emitted Rust of a DB-less crate is the
/// E0433 SEAL violation this probe guards against.
// The `expect` guards a test-support invariant: a build asserted successful
// just above MUST have written `src/main.rs`; an unreadable file here means the
// emitter/fixture is broken, so aborting is the correct failure signal.
#[allow(clippy::expect_used)]
fn assert_skyc_accepts_without_sky_row(fixture: &str) {
    let root = repo_root();
    let entry = entry_path(&root, fixture);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{fixture}_skyc_out"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP {fixture}: runtime not available");
        return;
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for {fixture}: {:?}",
        built.err()
    );

    let emitted = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");

    assert!(
        !emitted.contains("SkyRow"),
        "the wildcard-`any` fn must NOT gain a `SkyRow` bound — its body makes no \
         `Db.get*` kernel call, and the crate has no `db` feature, so a `SkyRow` \
         reference is the E0433 SEAL violation #177 guards against; got main.rs:\n{emitted}"
    );
}

/// cargo-0 ∧ run-0 for the emitted DB-less project — the only check that would
/// have caught the original E0433. Gated on `SKY_E2E=1`.
fn assert_cargo_builds_and_runs(fixture: &str, expected_stdout: &str) {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = entry_path(&root, fixture);
    let out = std::env::temp_dir().join(format!("skyc_{fixture}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP {fixture}: runtime not available");
        return;
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for {fixture}: {:?}",
        built.err()
    );

    let outcome = support::build_and_run_emitted(fixture, &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "{fixture} binary must cargo-build AND exit 0 (no E0433); got {:?} (stdout: {:?})",
        outcome.exit_code,
        outcome.stdout
    );
    assert_eq!(
        outcome.stdout.trim(),
        expected_stdout,
        "{fixture} stdout mismatch; got: {:?}",
        outcome.stdout
    );
}

#[test]
fn i177_false_positive_string_literal_skyc_no_sky_row() {
    assert_skyc_accepts_without_sky_row("i177_db_get_false_positive_string_literal");
}

#[test]
fn i177_false_positive_string_literal_cargo_builds_and_runs() {
    assert_cargo_builds_and_runs(
        "i177_db_get_false_positive_string_literal",
        "db_get_string called on the payload",
    );
}

#[test]
fn i177_false_positive_user_symbol_skyc_no_sky_row() {
    assert_skyc_accepts_without_sky_row("i177_db_get_false_positive_user_symbol");
}

#[test]
fn i177_false_positive_user_symbol_cargo_builds_and_runs() {
    assert_cargo_builds_and_runs("i177_db_get_false_positive_user_symbol", "label:payload");
}

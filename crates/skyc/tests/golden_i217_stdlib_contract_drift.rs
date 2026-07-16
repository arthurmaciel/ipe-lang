//! #217 stdlib-contract-drift regression — three kernel-backed stdlib
//! surfaces (`Jwt.withClaim`, `Std.Db.Migration`/`Db.migrate`,
//! `Sky.Http.Server.Response`) whose compiler contracts had drifted from the
//! reference `../sky/sky-stdlib` signatures, so `skyc` rejected a
//! verbatim-ported reference program with `SKY-T0001` ("expected String, found
//! Value" etc.).
//!
//! The reference Sky-source signature IS the contract:
//!
//! * `withClaim : String -> JsonEnc.Value -> Claims -> Claims`
//!   (`Sky/Core/Jwt.sky:79`) — ours had drifted to `String -> String -> …`.
//! * `type alias Migration = { name : String, sql : String }` +
//!   `migrate : Db -> List Migration -> Task Error (List String)`
//!   (`Std/Db.sky:237,300`) — ours had no `Migration` and `migrate` took
//!   `List (String, String)`.
//! * `type alias Response = { status : Int, body : String, headers : Dict
//!   String String, contentType : String }` (`Sky/Http/Server.sky:66`) — ours
//!   had registered it as an opaque nominal, so a record literal was rejected.
//!
//! Each test compiles a fixture that uses the surface as the reference does.
//! The cheap tier asserts `skyc` ACCEPTS the program (no SKY-T0001). The
//! `SKY_E2E=1` tier is THE SEAL: the emitted Rust must `cargo build` and run.
//!
//! Run:
//! ```text
//! cargo test -p skyc --test golden_i217_stdlib_contract_drift
//! SKY_E2E=1 cargo test -p skyc --test golden_i217_stdlib_contract_drift
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn entry_path(root: &Path, name: &str) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join(name)
        .join("Main.sky")
}

/// Compile a fixture and assert `skyc` accepts it (the contract now matches the
/// reference). Returns the emitted output dir for an optional E2E follow-up.
fn assert_skyc_accepts(name: &str) -> Option<PathBuf> {
    let root = repo_root();
    let entry = entry_path(&root, name);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_skyc_out"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        eprintln!("SKIP {name}: runtime not available");
        return None;
    };

    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for {name} (contract converged to reference): {:?}",
        built.err()
    );
    Some(out)
}

fn e2e_build_and_run(name: &str, expect_stdout_contains: &str) {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let root = repo_root();
    let entry = entry_path(&root, name);
    let out = std::env::temp_dir().join(format!("skyc_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };
    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for {name}: {:?}",
        built.err()
    );

    // The emitted binary is run by the oracle with the TEST process's cwd
    // (`crates/skyc`), and `Db.connect ()` writes the default `sky.db` there.
    // A `sky.db` left by a PRIOR run (or a fixture edit that changed a
    // migration's SQL) would trip the checksum guard on the next run — a
    // spurious runtime failure unrelated to the contract under test. Clear the
    // stray DB from the likely cwd locations so each run starts clean; the
    // fixture also recovers migration errors to keep the SEAL check robust.
    for dir in [root.join("crates").join("skyc"), root.clone()] {
        for suffix in ["sky.db", "sky.db-shm", "sky.db-wal"] {
            let _ = std::fs::remove_file(dir.join(suffix));
        }
    }

    let outcome = support::build_and_run_emitted(name, &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "{name} emitted project must cargo-build and exit 0 (THE SEAL); stdout: {:?}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains(expect_stdout_contains),
        "{name} stdout must contain {expect_stdout_contains:?}; got: {:?}",
        outcome.stdout
    );
}

/// `Jwt.withClaim` accepts a `JsonEnc.Value` argument (string AND int claims).
#[test]
fn i217_jwt_with_claim_accepts_json_value() {
    assert_skyc_accepts("i217_jwt_withclaim_value");
}

#[test]
fn i217_jwt_with_claim_e2e() {
    e2e_build_and_run("i217_jwt_withclaim_value", "ok");
}

/// `Db.migrate` takes `List Migration`, where `Migration` is the record alias.
#[test]
fn i217_db_migration_record_accepted() {
    assert_skyc_accepts("i217_db_migration_record");
}

#[test]
fn i217_db_migration_record_e2e() {
    e2e_build_and_run("i217_db_migration_record", "applied:");
}

/// `Response` used as a record — literal construction + field projection.
#[test]
fn i217_server_response_record_accepted() {
    assert_skyc_accepts("i217_server_response_record");
}

#[test]
fn i217_server_response_record_e2e() {
    e2e_build_and_run("i217_server_response_record", "429");
}

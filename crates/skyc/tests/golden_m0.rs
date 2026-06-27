//! Milestone-0 end-to-end gate: `skyc` must emit `main.rs` byte-identical to the
//! Haskell-reference golden, and (behind `SKY_E2E=1`) the emitted project must
//! build and print `1`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The `sky-rust` workspace root (two levels up from this crate's manifest).
fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

#[test]
fn emits_byte_identical_main_rs_and_vendors_runtime() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("m0")
        .join("Main.sky");
    let golden = root.join("tests").join("golden").join("m0").join("main.rs");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m0_emit");
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

    assert!(
        out.join("src")
            .join("sky_runtime")
            .join("core.rs")
            .is_file(),
        "runtime module tree must be vendored",
    );
}

/// Full spine: compile, build the emitted Cargo project, and run it. Gated on
/// `SKY_E2E=1` so the default `cargo test` stays fast (the emitted project pulls
/// real crates and takes ~1 min to compile cold).
#[test]
fn end_to_end_builds_and_prints_one() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("m0")
        .join("Main.sky");
    // Build OUTSIDE the workspace tree: an emitted project under the workspace's
    // own target/ dir is (correctly) rejected by cargo as a non-member package,
    // and the golden Cargo.toml carries no detaching `[workspace]` stanza.
    let out = std::env::temp_dir().join("skyc_m0_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed: {:?}", built.err());

    let status = Command::new("cargo")
        .arg("build")
        .current_dir(&out)
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
        "1\n",
        "program prints 1"
    );
}

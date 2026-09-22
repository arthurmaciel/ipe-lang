#![forbid(unsafe_code)]
//! T4 (prove the refusal): `ipe build` copies the freshly built native binary
//! into `<project>/out/bin/<name>` so it is findable under the project even when
//! `CARGO_TARGET_DIR` points at a shared cache OUTSIDE the project. And the
//! clobber regression: two different projects that share the emitted crate name
//! build to the SAME shared-target path, so a naive "the binary is in the shared
//! target" strategy would hand back the WRONG project's binary — the per-project
//! copy must preserve each project's own artifact.
//!
//! Gated on `IPE_E2E=1` (drives a real `cargo build`).
//!
//! ```text
//! IPE_E2E=1 cargo nextest run -p ipe --test build_artifact_copy
//! ```

use std::fs;
use std::path::{Path, PathBuf};

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

const SRC_A: &str = "module Main exposing (main)\n\nimport Ipe.Io\n\nmain = Io.println \"AAA\"\n";
const SRC_B: &str = "module Main exposing (main)\n\nimport Ipe.Io\n\nmain = Io.println \"BBB\"\n";

/// `ipe build <entry> --out <project>/out/rust`, with `CARGO_TARGET_DIR` forced
/// to a SHARED directory OUTSIDE the project, and the emitted crate name pinned
/// so both projects collide on the same shared-target path. Returns the
/// project's `out/bin/<name>` path.
fn build_into_shared_target(
    tag: &str,
    src: &str,
    shared_target: &Path,
) -> Result<PathBuf, BoxError> {
    let ipe_bin = env!("CARGO_BIN_EXE_ipe");
    if !Path::new(ipe_bin).exists() {
        return Err("ipe binary not built".into());
    }
    let runtime_dir = ipe::resolve_runtime()
        .map_err(|e| -> BoxError { format!("runtime dir must resolve: {e}").into() })?;

    let project = std::env::temp_dir().join(format!("ipe_build_artifact_copy_{tag}"));
    let _ = fs::remove_dir_all(&project);
    fs::create_dir_all(&project)?;
    let entry = project.join("Main.ipe");
    fs::write(&entry, src)?;
    let out_dir = project.join("out").join("rust");

    let status = std::process::Command::new(ipe_bin)
        .args(["build", &entry.to_string_lossy(), "--out"])
        .arg(&out_dir)
        // The shared cache lives OUTSIDE the project — the exact Ipê-recommended
        // setup that leaves the binary unfindable without the out/bin copy.
        .env("CARGO_TARGET_DIR", shared_target)
        .env("IPE_RUNTIME_DIR", &runtime_dir)
        // Pin the emitted crate name so BOTH projects build to the identical
        // `<shared-target>/debug/<name>` path — the collision the copy defends.
        .env("IPE_EMIT_PACKAGE_NAME", "app")
        .env("NO_COLOR", "1")
        .status()
        .map_err(|e| -> BoxError { format!("spawn ipe build: {e}").into() })?;
    if !status.success() {
        return Err(format!("[{tag}] ipe build must succeed, got {status:?}").into());
    }

    // The artifact lands at `<project>/out/bin/<name>` (sibling of `out/rust`).
    Ok(project.join("out").join("bin").join("app"))
}

/// The copied binary exists, is executable, and byte-matches the artifact cargo
/// produced in the shared target.
#[test]
fn build_copies_the_artifact_into_project_out_bin() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("skipping (set IPE_E2E=1 to run)");
        return Ok(());
    }
    let shared = std::env::temp_dir().join("ipe_build_artifact_copy_sharedA");
    let _ = fs::remove_dir_all(&shared);

    let copied = build_into_shared_target("solo", SRC_A, &shared)?;
    assert!(
        copied.is_file(),
        "the built binary must be copied to {}",
        copied.display()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(&copied)?.permissions().mode();
        assert!(
            mode & 0o111 != 0,
            "the copied artifact must be executable, mode = {mode:o}"
        );
    }

    // Byte-match against the artifact in the shared target cargo produced.
    let in_target = shared.join("debug").join("app");
    let (a, b) = (fs::read(&copied)?, fs::read(&in_target)?);
    assert_eq!(
        a, b,
        "out/bin copy must be byte-identical to the shared-target artifact"
    );

    Ok(())
}

/// Clobber regression: build project A (crate name `app`), then a DIFFERENT
/// project B also named `app` into the SAME shared target — which overwrites the
/// shared-target binary with B's. A's own `out/bin/app` must STILL be A's binary
/// (it was copied within A's build), never silently replaced by B's.
#[test]
fn a_second_same_named_project_does_not_clobber_the_first_out_bin() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("skipping (set IPE_E2E=1 to run)");
        return Ok(());
    }
    let shared = std::env::temp_dir().join("ipe_build_artifact_copy_sharedAB");
    let _ = fs::remove_dir_all(&shared);

    // Project A → out/bin/app is A's binary.
    let a_bin = build_into_shared_target("clobberA", SRC_A, &shared)?;
    let a_before = fs::read(&a_bin)?;

    // Project B, same crate name, same shared target → overwrites the
    // shared-target binary with B's.
    let b_bin = build_into_shared_target("clobberB", SRC_B, &shared)?;
    let b_after = fs::read(&b_bin)?;

    // A's project-local copy is UNCHANGED — still A's binary, not B's.
    let a_after = fs::read(&a_bin)?;
    assert_eq!(
        a_before, a_after,
        "project A's out/bin/app must survive project B's same-name build into the shared target"
    );
    assert_ne!(
        a_after, b_after,
        "A's and B's project-local artifacts must differ (each kept its own binary)"
    );

    Ok(())
}

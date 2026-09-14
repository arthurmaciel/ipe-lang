//! Integration tests for `ipe init`.
//!
//! The scaffolding tests run unconditionally (no network, no cargo). The
//! build test that verifies the scaffold compiles is gated on `IPE_E2E=1`, in
//! line with the other CLI E2E tests (see `run_subcommand.rs`).

use std::fs;
use std::path::PathBuf;

/// A fresh, unique temp directory for one test (removed first if present).
fn fresh_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ipe_init_test_{tag}"));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// `ipe init <name>` scaffolds the four project files, with the project name
/// threaded into `package.ipe`.
#[test]
fn init_scaffolds_project_files() {
    let dir = fresh_dir("scaffold");
    let target = dir.join("my-app");
    let args = vec!["init".to_owned(), target.to_string_lossy().into_owned()];
    let result = ipe::run_cli(&args);
    assert!(result.is_ok(), "init must succeed: {result:?}");

    for rel in [
        "package.ipe",
        "src/Main.ipe",
        "README.md",
        ".gitignore",
        "AGENTS.md",
    ] {
        assert!(
            target.join(rel).is_file(),
            "expected scaffold file {rel} to exist"
        );
    }

    let manifest = fs::read_to_string(target.join("package.ipe")).unwrap_or_default();
    assert!(
        manifest.contains("name = \"my-app\""),
        "package.ipe must carry the project name, got:\n{manifest}"
    );

    let main = fs::read_to_string(target.join("src/Main.ipe")).unwrap_or_default();
    assert!(
        main.contains("Ipe.Tea.Web") && main.contains("Increment") && main.contains("Decrement"),
        "Main.ipe must be the Ipe.Tea.Web counter"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// A second `ipe init` on an already-scaffolded directory never clobbers the
/// user's project. In this non-interactive harness (no TTY) it succeeds without
/// prompting and leaves every existing managed file byte-for-byte untouched;
/// `--force` overwrites them.
#[test]
fn init_reconciles_existing_project_without_clobbering() {
    let dir = fresh_dir("guard");
    let target = dir.join("app");
    let target_str = target.to_string_lossy().into_owned();

    let first = ipe::run_cli(&["init".to_owned(), target_str.clone()]);
    assert!(first.is_ok(), "first init must succeed: {first:?}");

    // A user edit to a managed file must survive a bare re-init.
    let main_path = target.join("src").join("Main.ipe");
    let edited = "-- my own program\nmodule Main exposing (main)\n";
    fs::write(&main_path, edited).expect("edit Main.ipe");

    let second = ipe::run_cli(&["init".to_owned(), target_str.clone()]);
    assert!(
        second.is_ok(),
        "re-init in an existing dir must not error (no TTY: it reconciles), got: {second:?}"
    );
    let after = fs::read_to_string(&main_path).unwrap_or_default();
    assert_eq!(
        after, edited,
        "a bare re-init must not overwrite an existing managed file"
    );

    // A missing managed file IS restored by a non-interactive re-init.
    let gitignore = target.join(".gitignore");
    fs::remove_file(&gitignore).expect("remove .gitignore");
    let third = ipe::run_cli(&["init".to_owned(), target_str.clone()]);
    assert!(third.is_ok(), "re-init must succeed: {third:?}");
    assert!(
        gitignore.is_file(),
        "a missing managed file must be restored by re-init"
    );

    // `--force` overwrites even an edited managed file.
    let forced = ipe::run_cli(&["init".to_owned(), target_str, "--force".to_owned()]);
    assert!(forced.is_ok(), "init --force must succeed: {forced:?}");
    let restored = fs::read_to_string(&main_path).unwrap_or_default();
    assert!(
        restored.contains("Increment") && restored.contains("Decrement"),
        "init --force must overwrite the managed file with the scaffold"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// `ipe init` with an unrecognised flag returns a command-usage error for
/// `init` — so the caller shows init's help — never a panic.
#[test]
fn init_unknown_flag_returns_usage_error() {
    let result = ipe::run_cli(&["init".to_owned(), "--bogus".to_owned()]);
    assert!(
        matches!(
            result,
            Err(ipe::CliError::CommandUsage {
                command: "init",
                ..
            })
        ),
        "unknown flag must yield a command-usage error, got: {result:?}"
    );
}

/// E2E (gated on `IPE_E2E=1`): the scaffold produced by `ipe init` compiles —
/// `ipe::build` runs the full pipeline and emits a Rust project, then
/// `cargo build` on that project must succeed (THE SEAL).
#[test]
fn init_scaffold_builds() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let runtime = ipe::resolve_runtime();
    let Ok(runtime_dir) = runtime else {
        return;
    };

    let dir = fresh_dir("build");
    let target = dir.join("counter");
    let init = ipe::run_cli(&["init".to_owned(), target.to_string_lossy().into_owned()]);
    assert!(init.is_ok(), "init must succeed: {init:?}");

    let entry = target.join("src").join("Main.ipe");
    let out_dir = target.join("out");
    let built = ipe::build(&entry, &out_dir, &runtime_dir);
    assert!(
        built.is_ok(),
        "ipe build on the scaffold must succeed: {built:?}"
    );

    let cargo_status = std::process::Command::new("cargo")
        .arg("build")
        .current_dir(&out_dir)
        .env("CARGO_TARGET_DIR", out_dir.join("target"))
        .status();
    assert!(
        matches!(&cargo_status, Ok(s) if s.success()),
        "cargo build on the emitted scaffold must succeed: {cargo_status:?}"
    );

    let _ = fs::remove_dir_all(out_dir.join("target"));
    let _ = fs::remove_dir_all(&dir);
}

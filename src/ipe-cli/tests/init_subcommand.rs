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
/// threaded into `ipe.toml`.
#[test]
fn init_scaffolds_project_files() {
    let dir = fresh_dir("scaffold");
    let target = dir.join("my-app");
    let args = vec!["init".to_owned(), target.to_string_lossy().into_owned()];
    let result = ipe::run_cli(&args);
    assert!(result.is_ok(), "init must succeed: {result:?}");

    for rel in [
        "ipe.toml",
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

    let toml = fs::read_to_string(target.join("ipe.toml")).unwrap_or_default();
    assert!(
        toml.contains("name = \"my-app\""),
        "ipe.toml must carry the project name, got:\n{toml}"
    );
    assert!(
        toml.contains("entry = \"src/Main.ipe\""),
        "ipe.toml must name the entry, got:\n{toml}"
    );

    let main = fs::read_to_string(target.join("src/Main.ipe")).unwrap_or_default();
    assert!(
        main.contains("Ipe.Live") && main.contains("Increment") && main.contains("Decrement"),
        "Main.ipe must be the Ipe.Live counter"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// A second `ipe init` on an already-scaffolded directory is refused unless
/// `--force` is supplied.
#[test]
fn init_refuses_existing_project_without_force() {
    let dir = fresh_dir("guard");
    let target = dir.join("app");
    let target_str = target.to_string_lossy().into_owned();

    let first = ipe::run_cli(&["init".to_owned(), target_str.clone()]);
    assert!(first.is_ok(), "first init must succeed: {first:?}");

    let second = ipe::run_cli(&["init".to_owned(), target_str.clone()]);
    assert!(
        matches!(
            second,
            Err(ipe::CliError::CommandUsage {
                command: "init",
                ..
            })
        ),
        "re-init without --force must be refused (and show init's help), got: {second:?}"
    );

    let forced = ipe::run_cli(&["init".to_owned(), target_str, "--force".to_owned()]);
    assert!(forced.is_ok(), "init --force must succeed: {forced:?}");

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

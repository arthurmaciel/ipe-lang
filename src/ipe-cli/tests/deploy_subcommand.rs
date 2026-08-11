//! Tests for the `ipe deploy` subcommand — argument parsing and emit-shape
//! checks. The full end-to-end deploy (compile + cargo build + bundle layout)
//! requires the musl target installed and is gated on `IPE_E2E=1`.

use ipe::cli_args::parse_deploy;
use std::path::PathBuf;

// ── Argument parsing ─────────────────────────────────────────────────────────

/// No arguments: entry defaults to None (project-aware default) and embed is
/// off.
#[test]
fn deploy_no_args_defaults() {
    let args: Vec<String> = vec![];
    let parsed = parse_deploy(&args).expect("no-arg parse must succeed");
    assert!(parsed.entry.is_none());
    assert!(parsed.out.is_none());
    assert!(parsed.target.is_none());
    assert!(parsed.runtime.is_none());
    assert!(!parsed.embed);
}

/// `--embed` flag sets the embed field.
#[test]
fn deploy_embed_flag() {
    let args: Vec<String> = vec!["--embed".into()];
    let parsed = parse_deploy(&args).expect("--embed parse must succeed");
    assert!(parsed.embed);
}

/// `--out <dir>` is accepted.
#[test]
fn deploy_out_flag() {
    let args: Vec<String> = vec!["--out".into(), "dist/".into()];
    let parsed = parse_deploy(&args).expect("--out parse must succeed");
    assert_eq!(parsed.out.as_deref(), Some("dist/"));
}

/// `--target <triple>` is accepted.
#[test]
fn deploy_target_flag() {
    let args: Vec<String> = vec!["--target".into(), "x86_64-unknown-linux-musl".into()];
    let parsed = parse_deploy(&args).expect("--target parse must succeed");
    assert_eq!(parsed.target.as_deref(), Some("x86_64-unknown-linux-musl"));
}

/// A positional entry is captured.
#[test]
fn deploy_positional_entry() {
    let args: Vec<String> = vec!["src/Main.ipe".into()];
    let parsed = parse_deploy(&args).expect("entry parse must succeed");
    assert_eq!(parsed.entry.as_deref(), Some("src/Main.ipe"));
}

/// An unknown flag returns a usage error.
#[test]
fn deploy_unknown_flag_is_usage_error() {
    let args: Vec<String> = vec!["--no-such-flag".into()];
    let result = parse_deploy(&args);
    assert!(
        result.is_err(),
        "unknown flag must yield a parse error, got: {result:?}"
    );
}

/// `--out` given twice is a usage error.
#[test]
fn deploy_out_twice_is_usage_error() {
    let args: Vec<String> = vec!["--out".into(), "a/".into(), "--out".into(), "b/".into()];
    let result = parse_deploy(&args);
    assert!(
        result.is_err(),
        "--out twice must yield a parse error, got: {result:?}"
    );
}

/// `--target` given twice is a usage error.
#[test]
fn deploy_target_twice_is_usage_error() {
    let args: Vec<String> = vec![
        "--target".into(),
        "x86_64-unknown-linux-musl".into(),
        "--target".into(),
        "aarch64-unknown-linux-musl".into(),
    ];
    let result = parse_deploy(&args);
    assert!(
        result.is_err(),
        "--target twice must yield a parse error, got: {result:?}"
    );
}

/// `ipe deploy` in an empty directory with no ipe.toml or src/Main.ipe returns
/// a usage error (not a panic or a silent wrong result).
#[test]
fn deploy_no_project_returns_usage_error() {
    let dir = {
        let d = std::env::temp_dir().join("ipe_deploy_test_empty");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    };
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(&dir).unwrap();

    let result = ipe::run_cli(&["deploy".to_owned()]);

    std::env::set_current_dir(&prev).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(
            result,
            Err(ipe::CliError::CommandUsage {
                command: "deploy",
                ..
            })
        ),
        "bare `ipe deploy` in an empty dir must yield a deploy command-usage error, got: {result:?}"
    );
}

/// An unsupported `--target` triple is a usage error.
#[test]
fn deploy_unsupported_target_is_usage_error() {
    // Parse succeeds (the closed-enum gate is at the CLI dispatch level), but
    // the run_deploy call rejects an unknown triple. This test exercises the
    // full dispatch path to confirm the gate fires.
    //
    // To avoid needing a project, use a temp dir. The error fires before
    // any filesystem stat on the project.
    let dir = {
        let d = std::env::temp_dir().join("ipe_deploy_test_bad_triple");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // Create a minimal ipe.toml to pass the "no project" gate.
        std::fs::write(d.join("ipe.toml"), "[package]\nname = \"test\"\n").unwrap();
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(
            d.join("src").join("Main.ipe"),
            "main : Task Error ()\nmain = Task.succeed ()\n",
        )
        .unwrap();
        d
    };
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(&dir).unwrap();

    let result = ipe::run_cli(&[
        "deploy".to_owned(),
        "--target".to_owned(),
        "totally-bogus-triple".to_owned(),
    ]);

    std::env::set_current_dir(&prev).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        result.is_err(),
        "deploy with a bogus target triple must error, got Ok"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("unsupported target") || msg.contains("bogus"),
        "error must mention the unsupported target, got: {msg}"
    );
}

/// `ipe deploy` on a pure Ipê app (no native/FFI content, no `ipe.profile`)
/// must return a clear typed error naming the entry, not a raw Io error about
/// a missing source file. No partial bundle must be left on disk.
///
/// The test exercises the guard in `run_deploy` that detects the pure-app case
/// via capability inference before any cargo build is attempted, so it runs
/// fast without the musl target or `IPE_E2E=1`.
#[test]
fn deploy_pure_app_gives_clear_error_not_raw_io() {
    // Write a minimal pure Ipê program (no Rust.* FFI, no NativeFfi capability).
    // The module header is required for lower_entry to accept the file.
    let dir = {
        let d = std::env::temp_dir().join("ipe_deploy_test_pure_app");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("Main.ipe"),
            concat!(
                "module Main exposing (main)\n\n",
                "import Ipe.String as String\n",
                "import Ipe.Io as Io\n\n",
                "main : Task ()\n",
                "main =\n",
                "    Io.println (String.toUpper \"hello\")\n",
            ),
        )
        .unwrap();
        d
    };
    let entry = dir.join("Main.ipe");
    let out_dir = dir.join("deploy");

    let result = ipe::run_cli(&[
        "deploy".to_owned(),
        entry.to_string_lossy().into_owned(),
        "--out".to_owned(),
        out_dir.to_string_lossy().into_owned(),
    ]);

    // The result must be the typed pure-app refusal, not a raw Io error or a
    // panic. No partial bundle directory must exist.
    assert!(
        matches!(result, Err(ipe::CliError::DeployPureApp { .. })),
        "a pure Ipê app must yield DeployPureApp, got: {result:?}"
    );

    // The error message must name the entry and explain the situation clearly
    // (not expose a raw "missing source file" Io error).
    let msg = if let Err(e) = result {
        e.to_string()
    } else {
        String::new()
    };
    assert!(
        msg.contains("pure") || msg.contains("no native") || msg.contains("no capability"),
        "error message must explain the pure-app condition, got: {msg}"
    );
    assert!(
        !msg.contains("io error") && !msg.contains("missing source file"),
        "error must not expose a raw Io message, got: {msg}"
    );

    // No partial bundle must have been written.
    assert!(
        !out_dir.join("bundle").exists(),
        "no partial bundle dir must exist after a pure-app refusal"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The `DeployPureApp` error names the entry path in its Display output.
#[test]
fn deploy_pure_app_error_display_names_entry() {
    let err = ipe::CliError::DeployPureApp {
        entry: PathBuf::from("/some/project/Main.ipe"),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("Main.ipe"),
        "DeployPureApp Display must name the entry path, got: {msg}"
    );
    assert!(
        !msg.contains("io error"),
        "DeployPureApp Display must not expose a raw Io message, got: {msg}"
    );
}

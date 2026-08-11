//! Tests for the `ipe deploy` subcommand — argument parsing and emit-shape
//! checks. The full end-to-end deploy (compile + cargo build + bundle layout)
//! requires the musl target installed and is gated on `IPE_E2E=1`.

use ipe::cli_args::parse_deploy;

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

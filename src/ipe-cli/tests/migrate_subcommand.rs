//! Integration tests for `ipe migrate config`.
//!
//! The command runs in the current working directory (it reads `ipe.toml` and
//! writes `package.ipe` there), so the tests that invoke it are serialized
//! against each other via a CWD guard, mirroring `build_no_arg.rs`. None of
//! these touch cargo or the network.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// A fresh temp directory unique to this process and call, with a minimal
/// `src/Main.ipe` so the reader's source-root existence check passes when it
/// re-reads any produced `package.ipe`.
fn fresh_project(tag: &str) -> std::io::Result<PathBuf> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ipe_migrate_it_{tag}_{}_{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src"))?;
    fs::write(
        dir.join("src").join("Main.ipe"),
        "module Main exposing (main)\nmain = 0\n",
    )?;
    Ok(dir)
}

/// Serializes the tests that mutate the process-global current directory.
static CWD_GUARD: Mutex<()> = Mutex::new(());

macro_rules! in_dir {
    ($dir:expr, $body:expr) => {{
        let _cwd_guard = CWD_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir($dir).unwrap();
        let out = $body;
        std::env::set_current_dir(&prev).unwrap();
        out
    }};
}

/// `ipe migrate config` renders an `ipe.toml` into a `package.ipe` that carries
/// the migrated fields, leaves the `ipe.toml` in place, and re-parses to the same
/// manifest (the readback path exercises the P1 reader).
#[test]
fn migrate_writes_a_readable_package_ipe() {
    let dir = fresh_project("write").unwrap();
    fs::write(
        dir.join("ipe.toml"),
        "[project]\nname = \"my-app\"\nversion = \"1.2.3\"\n[database]\ndriver = \"postgres\"\n",
    )
    .expect("write ipe.toml");

    let result = in_dir!(
        &dir,
        ipe::run_cli(&["migrate".to_owned(), "config".to_owned()])
    );
    assert!(result.is_ok(), "migrate config must succeed: {result:?}");

    let pkg = dir.join("package.ipe");
    assert!(pkg.is_file(), "package.ipe must be written");
    assert!(
        dir.join("ipe.toml").is_file(),
        "the ipe.toml is never deleted by migrate"
    );

    let text = fs::read_to_string(&pkg).unwrap_or_default();
    assert!(
        text.contains("Package.named \"my-app\""),
        "carries the name:\n{text}"
    );
    assert!(
        text.contains("Package.version \"1.2.3\""),
        "carries the version:\n{text}"
    );
    assert!(
        text.contains("Package.database Package.postgres"),
        "carries the driver:\n{text}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// With no `ipe.toml` in the directory, `migrate config` fails cleanly (a
/// command-usage error routed to `migrate`'s help), never a panic.
#[test]
fn migrate_without_ipe_toml_is_a_usage_error() {
    let dir = fresh_project("no_toml").unwrap();
    let result = in_dir!(
        &dir,
        ipe::run_cli(&["migrate".to_owned(), "config".to_owned()])
    );
    assert!(
        matches!(
            result,
            Err(ipe::CliError::CommandUsage {
                command: "migrate",
                ..
            })
        ),
        "a missing ipe.toml must yield a migrate command-usage error, got: {result:?}"
    );
    assert!(
        !dir.join("package.ipe").is_file(),
        "no package.ipe is written when there is nothing to migrate"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// `migrate config` refuses to clobber an existing `package.ipe` without
/// `--force`; with `--force` it overwrites.
#[test]
fn migrate_refuses_to_clobber_without_force() {
    let dir = fresh_project("clobber").unwrap();
    fs::write(dir.join("ipe.toml"), "[project]\nname = \"fresh\"\n").expect("write ipe.toml");
    let sentinel = "-- hand-authored, do not clobber\npackage =\n    Package.named \"kept\"\n";
    fs::write(dir.join("package.ipe"), sentinel).expect("write package.ipe");

    let refused = in_dir!(
        &dir,
        ipe::run_cli(&["migrate".to_owned(), "config".to_owned()])
    );
    assert!(
        matches!(
            refused,
            Err(ipe::CliError::CommandUsage {
                command: "migrate",
                ..
            })
        ),
        "an existing package.ipe must not be clobbered without --force, got: {refused:?}"
    );
    assert_eq!(
        fs::read_to_string(dir.join("package.ipe")).unwrap_or_default(),
        sentinel,
        "the existing package.ipe is left byte-for-byte untouched"
    );

    let forced = in_dir!(
        &dir,
        ipe::run_cli(&[
            "migrate".to_owned(),
            "config".to_owned(),
            "--force".to_owned(),
        ])
    );
    assert!(forced.is_ok(), "migrate --force must succeed: {forced:?}");
    let after = fs::read_to_string(dir.join("package.ipe")).unwrap_or_default();
    assert!(
        after.contains("Package.named \"fresh\"") && !after.contains("kept"),
        "--force overwrites with the migrated manifest:\n{after}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// An unknown `migrate` subcommand is a clean command-usage error.
#[test]
fn migrate_unknown_subcommand_is_a_usage_error() {
    let result = ipe::run_cli(&["migrate".to_owned(), "bogus".to_owned()]);
    assert!(
        matches!(
            result,
            Err(ipe::CliError::CommandUsage {
                command: "migrate",
                ..
            })
        ),
        "unknown subcommand must yield a migrate command-usage error, got: {result:?}"
    );
}

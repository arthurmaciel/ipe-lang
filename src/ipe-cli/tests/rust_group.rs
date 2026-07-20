//! `ipe rust` — the Rust foreign-function command group. The individual
//! subcommands (`add` / `remove` / `install`) live under it; this covers the
//! group dispatch and its help surface, not the sandboxed inspection paths.

use std::process::Command;

/// Run `ipe <args>` with `NO_COLOR=1` and return `(exit_success, stdout, stderr)`.
fn run(args: &[&str]) -> (bool, String, String) {
    match Command::new(env!("CARGO_BIN_EXE_ipe"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
    {
        Ok(o) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        ),
        Err(e) => (false, String::new(), format!("spawn failed: {e}")),
    }
}

#[test]
fn bare_rust_prints_group_help_and_succeeds() {
    let (ok, stdout, _) = run(&["rust"]);
    assert!(ok, "`ipe rust` alone must exit 0");
    for sub in ["add", "remove", "install"] {
        assert!(
            stdout.contains(sub),
            "group help must list the `{sub}` subcommand, got:\n{stdout}"
        );
    }
}

#[test]
fn unknown_rust_subcommand_fails() {
    let (ok, _, stderr) = run(&["rust", "frobnicate"]);
    assert!(!ok, "an unknown `ipe rust` subcommand must exit non-zero");
    assert!(
        stderr.contains("frobnicate"),
        "the error must name the bad subcommand, got:\n{stderr}"
    );
}

#[test]
fn rust_remove_without_a_crate_is_usage_error() {
    // `ipe rust remove` needs a crate name; with none it is misuse. This
    // proves the subcommand dispatch reaches `run_remove` (no cache touched).
    let (ok, _, stderr) = run(&["rust", "remove"]);
    assert!(!ok, "`ipe rust remove` with no crate must fail");
    assert!(
        stderr.contains("ipe rust remove"),
        "the usage message must name the `ipe rust remove` form, got:\n{stderr}"
    );
}

#[test]
fn rust_help_page_names_the_command() {
    let (ok, stdout, _) = run(&["rust", "--help"]);
    assert!(ok, "`ipe rust --help` exits 0");
    assert!(
        stdout.contains("ipe rust"),
        "help page names the command, got:\n{stdout}"
    );
}

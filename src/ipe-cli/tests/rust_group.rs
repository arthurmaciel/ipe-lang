//! `ipe rust` — the Rust foreign-function command group. The individual
//! subcommands (`add` / `remove` / `install`) live under it; this covers the
//! group dispatch and its help surface, not the sandboxed inspection paths.

use std::process::Command;

mod support;

/// Run `ipe <args>` with `NO_COLOR=1` and return `(exit_success, stdout, stderr)`.
fn run(args: &[&str]) -> (bool, String, String) {
    match Command::new(support::ipe_bin())
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
fn bare_rust_is_misuse_and_shows_help() {
    // A bare `ipe rust` (no subcommand) is misuse, not a help request: it exits
    // non-zero and shows its `--help` page — the uniform misuse discipline every
    // subcommand-parent shares (matching bare `ipe package`).
    let (bare_ok, _, bare_stderr) = run(&["rust"]);
    assert!(!bare_ok, "bare `ipe rust` must exit non-zero (misuse)");
    assert!(
        bare_stderr.contains("ipe rust") && bare_stderr.contains("add"),
        "bare `ipe rust` must show its usage/help naming its subcommands, got:\n{bare_stderr}"
    );

    // An explicit `ipe rust --help` is still a help request: exit 0, page to stdout.
    let (help_ok, help_stdout, _) = run(&["rust", "--help"]);
    assert!(help_ok, "`ipe rust --help` must exit 0");
    assert!(
        help_stdout.contains("ipe rust"),
        "`ipe rust --help` prints the page to stdout, got:\n{help_stdout}"
    );
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

//! `ipe add` / `ipe remove` — the package-authoring commands' argument handling
//! and help surface. The resolution behaviour (fetch, verify, lock) is covered
//! by `add_resolve.rs`; this covers usage errors and the help pages.

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
fn add_without_a_package_name_is_a_usage_error() {
    let (ok, _, stderr) = run(&["add"]);
    assert!(!ok, "`ipe add` with no package must be misuse");
    assert!(
        stderr.contains("ipe add"),
        "the usage message must name the command, got:\n{stderr}"
    );
}

#[test]
fn add_and_remove_have_help_pages() {
    for cmd in ["add", "remove"] {
        let (ok, stdout, _) = run(&[cmd, "--help"]);
        assert!(ok, "`ipe {cmd} --help` must exit 0");
        assert!(
            stdout.contains(&format!("ipe {cmd}")),
            "help page for `{cmd}` must name it, got:\n{stdout}"
        );
    }
}

#[test]
fn top_level_help_lists_the_external_packages_section() {
    let (ok, stdout, _) = run(&["--help"]);
    assert!(ok);
    assert!(
        stdout.contains("Using external packages"),
        "top-level help must carry the Using external packages section, got:\n{stdout}"
    );
}

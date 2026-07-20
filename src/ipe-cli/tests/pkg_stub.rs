//! `ipe add` / `ipe remove` — the package-authoring commands. Resolution ships
//! with the index (SP3); until then these parse their arguments and report the
//! not-yet-available state, exiting non-zero (never a silent no-op).

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
fn add_a_package_reports_not_yet_available_and_exits_nonzero() {
    let (ok, _, stderr) = run(&["add", "some-package"]);
    assert!(!ok, "`ipe add` must exit non-zero until the index ships");
    assert!(
        stderr.contains("some-package"),
        "the message must name the requested package, got:\n{stderr}"
    );
    assert!(
        stderr.contains("index"),
        "the message must point at the index (SP3), got:\n{stderr}"
    );
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
fn remove_a_package_reports_not_yet_available_and_exits_nonzero() {
    let (ok, _, stderr) = run(&["remove", "some-package"]);
    assert!(!ok, "`ipe remove` must exit non-zero until the index ships");
    assert!(
        stderr.contains("some-package"),
        "the message must name the requested package, got:\n{stderr}"
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
fn top_level_help_lists_a_package_authoring_section() {
    let (ok, stdout, _) = run(&["--help"]);
    assert!(ok);
    assert!(
        stdout.contains("Package authoring"),
        "top-level help must carry the Package authoring section, got:\n{stdout}"
    );
}

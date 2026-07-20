//! Integration tests for the `ipe` help system: the sectioned top-level
//! screen, per-command `--help` pages, exit codes, and stream routing.
//!
//! These run the built binary as a subprocess (with `NO_COLOR=1`, so the
//! captured, non-terminal output is deterministic plain text) to observe real
//! exit codes and the stdout/stderr split — properties the library API alone
//! cannot show.

use std::process::Command;

/// One `ipe` run's observable result: exit success plus decoded streams.
struct Run {
    /// Whether the process exited zero.
    ok: bool,
    /// Decoded stdout (lossy, so a decode failure never aborts a test).
    stdout: String,
    /// Decoded stderr (lossy).
    stderr: String,
}

/// Run `ipe <args>` with `NO_COLOR=1` and a non-terminal stdout. A spawn
/// failure is folded into a non-`ok` result carrying the error on stderr, so
/// callers surface it through an ordinary assertion rather than a panic.
fn run(args: &[&str]) -> Run {
    match Command::new(env!("CARGO_BIN_EXE_ipe"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
    {
        Ok(output) => Run {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
        Err(e) => Run {
            ok: false,
            stdout: String::new(),
            stderr: format!("failed to spawn ipe {args:?}: {e}"),
        },
    }
}

/// Every command name and every section title, for coverage assertions.
const COMMANDS: &[&str] = &[
    "init", "build", "run", "watch", "fix", "fmt", "rust", "explain", "lsp", "version",
];
const SECTIONS: &[&str] = &["Development", "Foreign-function interface (FFI)", "Tools"];

#[test]
fn top_level_help_lists_every_command_and_section() {
    let r = run(&["--help"]);
    assert!(r.ok, "`--help` must exit 0");

    // The header must carry the real "Ipê" bytes, not a filtered spelling.
    assert!(
        r.stdout.contains("Ipê language"),
        "header must read `Ipê language`"
    );
    assert!(
        r.stdout.contains(env!("CARGO_PKG_VERSION")),
        "header must carry the version"
    );

    for section in SECTIONS {
        assert!(
            r.stdout.contains(section),
            "top-level help missing section `{section}`"
        );
    }
    for cmd in COMMANDS {
        assert!(
            r.stdout.contains(&format!("ipe {cmd}")),
            "top-level help missing `ipe {cmd}`"
        );
    }

    // With NO_COLOR set, the output is clean plain text.
    assert!(
        !r.stdout.contains('\x1b'),
        "NO_COLOR output must carry no ANSI escapes"
    );
}

#[test]
fn no_args_prints_top_level_help_and_succeeds() {
    let r = run(&[]);
    assert!(r.ok, "no args must exit 0");
    assert!(r.stdout.contains("Ipê language"));
    assert!(r.stdout.contains("Development"));
}

#[test]
fn unknown_command_shows_help_on_stderr_and_fails() {
    let r = run(&["definitely-not-a-command"]);
    assert!(!r.ok, "an unknown command must exit non-zero");
    assert!(r.stdout.is_empty(), "misuse must not write help to stdout");
    assert!(
        r.stderr.contains("Ipê language"),
        "misuse must show the help on stderr"
    );
    assert!(
        r.stderr.contains("Development"),
        "misuse help must include the sections"
    );
}

#[test]
fn every_command_has_a_help_page_via_flag_and_via_help_word() {
    for cmd in COMMANDS {
        for form in [[*cmd, "--help"], ["help", *cmd]] {
            let r = run(&form);
            assert!(r.ok, "`ipe {form:?}` must exit 0");
            assert!(
                r.stdout.contains(&format!("ipe {cmd}")),
                "`ipe {form:?}` must show the `{cmd}` synopsis"
            );
            assert!(!r.stdout.contains('\x1b'), "NO_COLOR page must be plain");
        }
    }
}

#[test]
fn command_help_lists_that_commands_options() {
    let r = run(&["build", "--help"]);
    assert!(r.ok);
    // A flag unique to `build` and its description must appear on the page.
    assert!(
        r.stdout.contains("[--emit-ir]"),
        "build --help must list --emit-ir"
    );
    assert!(
        r.stdout.contains("intermediate representation"),
        "and describe it"
    );
    // A flag that is NOT a build flag must not appear.
    assert!(
        !r.stdout.contains("--features"),
        "build --help must not list add's flags"
    );
}

#[test]
fn help_word_alone_and_help_of_unknown_both_succeed() {
    assert!(run(&["help"]).ok, "`ipe help` must exit 0");
    let r = run(&["help", "no-such-command"]);
    assert!(
        r.ok,
        "`ipe help <unknown>` must fall back to the top-level, exit 0"
    );
    assert!(r.stdout.contains("Ipê language"));
}

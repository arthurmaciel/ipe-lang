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
    "init", "build", "run", "watch", "fix", "fmt", "add", "remove", "rust", "explain", "lsp",
    "version",
];
const SECTIONS: &[&str] = &[
    "Development",
    "Using external packages",
    "Package authoring",
    "Foreign-function interface (FFI)",
    "Tools",
];

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

    // The old "Run any line above…" footer sentence is gone — the screen ends at
    // the report-bugs footer, which alone remains.
    assert!(
        !r.stdout.contains("Run any line above"),
        "the how-to-read footer sentence must be removed"
    );
    assert!(
        r.stdout.contains("If you find any bugs, please report them at"),
        "the report-bugs footer must remain"
    );
}

#[test]
fn package_authoring_section_holds_package_and_external_packages_holds_add_remove() {
    let r = run(&["--help"]);
    assert!(r.ok);

    // Slice the two adjacent sections out of the screen by their headings so we
    // can assert which commands live under each.
    let authoring = section_body(&r.stdout, "Package authoring");
    assert!(
        authoring.contains("ipe package"),
        "`package` must sit under `Package authoring`, got:\n{authoring}"
    );

    let external = section_body(&r.stdout, "Using external packages");
    assert!(
        external.contains("ipe add") && external.contains("ipe remove"),
        "`add`/`remove` must stay under `Using external packages`, got:\n{external}"
    );
    assert!(
        !external.contains("ipe package"),
        "`package` must have moved out of `Using external packages`"
    );
}

/// The lines belonging to the section titled `heading`: everything from the
/// heading up to the next blank line (sections are separated by a blank line).
fn section_body(screen: &str, heading: &str) -> String {
    screen
        .lines()
        .skip_while(|l| l.trim() != heading)
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn command_misuse_shows_that_commands_help_on_stderr() {
    // An unknown flag to a known command is misuse: the command's OWN `--help`
    // page goes to stderr, exit is non-zero, and stdout stays empty.
    let r = run(&["build", "--definitely-not-a-flag"]);
    assert!(!r.ok, "a misused command must exit non-zero");
    assert!(r.stdout.is_empty(), "misuse must not write to stdout");
    assert!(
        r.stderr.contains("unknown flag"),
        "the specific reason must lead the misuse output"
    );
    assert!(
        r.stderr.contains("ipe build") && r.stderr.contains("[--emit-ir]"),
        "misuse must show the command's full --help page on stderr"
    );
    // It is the command's page, not the top-level screen.
    assert!(
        !r.stderr.contains("Most used commands"),
        "a command misuse shows the command page, not the top-level screen"
    );
}

#[test]
fn command_help_page_is_indented_by_the_gutter() {
    // Every non-blank line of a command's `--help` page sits in the two-space
    // gutter (this page IS a command's misuse output).
    let r = run(&["fix", "--help"]);
    assert!(r.ok);
    for line in r.stdout.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            line.starts_with("  "),
            "help line must start in the gutter: {line:?}"
        );
    }
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
fn mistyped_command_suggests_the_nearest_match() {
    let r = run(&["biuld"]);
    assert!(!r.ok, "a mistyped command must exit non-zero");
    assert!(
        r.stderr.contains("unknown command `biuld`"),
        "the typed token must be echoed, got:\n{}",
        r.stderr
    );
    assert!(
        r.stderr.contains("maybe `build`?"),
        "a near-miss must suggest the closest command, got:\n{}",
        r.stderr
    );
}

#[test]
fn wildly_unknown_command_offers_no_misleading_guess() {
    let r = run(&["definitely-not-a-command"]);
    assert!(
        !r.stderr.contains("maybe `"),
        "a token far from every command must not guess, got:\n{}",
        r.stderr
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

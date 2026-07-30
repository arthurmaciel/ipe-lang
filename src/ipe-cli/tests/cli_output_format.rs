//! Integration tests for the human-first output model: the `--plain` / `--json`
//! machine forms on the data-producing commands (`capabilities`, `diff`,
//! `version`, `explain` with no code), the reject-both-flags usage error, and
//! the flush-left / stable-schema guarantees pipelines depend on.
//!
//! These run the built binary as a subprocess (with `NO_COLOR=1` so the captured
//! non-terminal output is deterministic plain text) to observe the real streams,
//! exit codes, and byte-level output shape.
#![forbid(unsafe_code)]
// Test fixture setup: a failed `expect` IS the failure signal — the harness
// reports it as the test failure, which is the intended behaviour here.
#![allow(clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

/// One `ipe` run's observable result.
struct Run {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Run `ipe <args>` with `NO_COLOR=1`. A spawn failure folds into a non-`ok`
/// result carrying the error on stderr, surfaced through an ordinary assertion.
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

/// A source file whose inferred capability set is a known, non-empty pair
/// (`network`, `clock`), for the capabilities-form assertions.
fn sample_entry() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/sky/ipe/02-go-stdlib/src/Main.ipe");
    path.to_string_lossy().into_owned()
}

// ---- version ---------------------------------------------------------------

#[test]
fn version_default_is_human_and_guttered() {
    let r = run(&["version"]);
    assert!(r.ok);
    // Human form is framed (leading newline) and guttered (two-space indent).
    // The frame opens with a blank line, so the first non-blank line is `  ipe …`.
    assert!(
        r.stdout
            .lines()
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| l.starts_with("  ipe ")),
        "human version must be guttered: {:?}",
        r.stdout
    );
    // The frame: output starts and ends with a newline.
    assert!(
        r.stdout.starts_with('\n'),
        "human version must open with a newline: {:?}",
        r.stdout
    );
}

#[test]
fn version_plain_is_the_bare_flush_left_string() {
    let r = run(&["version", "--plain"]);
    assert!(r.ok);
    let out = r.stdout.trim_end();
    assert!(!out.starts_with(' '), "--plain must be flush-left: {out:?}");
    // Exactly the version, nothing else.
    assert_eq!(out, env!("CARGO_PKG_VERSION"));
}

#[test]
fn version_json_carries_the_version_field() {
    let r = run(&["version", "--json"]);
    assert!(r.ok);
    let out = r.stdout.trim();
    assert_eq!(
        out,
        format!("{{\"version\":\"{}\"}}", env!("CARGO_PKG_VERSION")),
        "--json must be the stable single-field object"
    );
    assert!(!out.starts_with(' '), "--json must be flush-left");
}

// ---- capabilities ----------------------------------------------------------

#[test]
fn capabilities_default_is_human_labelled() {
    let r = run(&["capabilities", &sample_entry()]);
    assert!(r.ok, "stderr: {}", r.stderr);
    // Human form is a guttered, labelled report, not the bare list.
    assert!(
        r.stdout.contains("security capabilit"),
        "human capabilities must be labelled: {:?}",
        r.stdout
    );
    assert!(
        r.stdout
            .lines()
            .all(|l| l.is_empty() || l.starts_with("  ")),
        "human capabilities must be guttered"
    );
}

#[test]
fn capabilities_plain_is_the_bare_scriptable_list() {
    let r = run(&["capabilities", "--plain", &sample_entry()]);
    assert!(r.ok, "stderr: {}", r.stderr);
    // The historical scriptable output: bare names, one per line, flush-left.
    // This is the migration guarantee — pipelines adopt `--plain` unchanged.
    assert_eq!(r.stdout, "network\nclock\n");
}

#[test]
fn capabilities_json_is_a_stable_object() {
    let r = run(&["capabilities", "--json", &sample_entry()]);
    assert!(r.ok, "stderr: {}", r.stderr);
    assert_eq!(
        r.stdout.trim(),
        "{\"capabilities\":[\"network\",\"clock\"]}"
    );
}

// ---- explain (the code list) ----------------------------------------------

#[test]
fn explain_list_plain_is_tab_separated_flush_left() {
    let r = run(&["explain", "--plain"]);
    assert!(r.ok);
    let first = r.stdout.lines().next().unwrap_or_default();
    assert!(!first.starts_with(' '), "--plain rows must be flush-left");
    assert!(
        first.contains('\t'),
        "--plain rows must be tab-separated (code<TAB>title): {first:?}"
    );
}

#[test]
fn explain_list_json_is_a_codes_array() {
    let r = run(&["explain", "--json"]);
    assert!(r.ok);
    let out = r.stdout.trim();
    assert!(
        out.starts_with("{\"codes\":["),
        "json must open the codes array"
    );
    assert!(out.contains("\"code\":") && out.contains("\"title\":"));
}

#[test]
fn explain_of_a_single_code_rejects_the_format_flags() {
    // The machine forms apply to the list, not to a single code's teaching page.
    let r = run(&["explain", "--json", "IPE-L0131"]);
    assert!(!r.ok, "explaining a code with --json must be a usage error");
    assert!(r.stdout.is_empty());
    assert!(
        r.stderr.contains("--plain / --json apply to the code list"),
        "the reason must name the misuse: {}",
        r.stderr
    );
    // Error-shows-help: the explain command's page appears on stderr.
    assert!(r.stderr.contains("ipe explain"));
}

// ---- reject both flags -----------------------------------------------------

#[test]
fn plain_and_json_together_is_a_usage_error_showing_help() {
    for cmd in [
        vec!["version", "--plain", "--json"],
        vec!["capabilities", "--plain", "--json"],
        vec!["explain", "--plain", "--json"],
    ] {
        let r = run(&cmd);
        assert!(!r.ok, "`ipe {cmd:?}` must fail on both flags");
        assert!(r.stdout.is_empty(), "misuse must not write to stdout");
        assert!(
            r.stderr.contains("mutually exclusive"),
            "the reason must name the conflict for {cmd:?}: {}",
            r.stderr
        );
        // Error-shows-help: the command's own page follows the reason.
        let name = cmd.first().copied().unwrap_or_default();
        assert!(
            r.stderr.contains(&format!("ipe {name}")),
            "misuse must show the command's --help for {cmd:?}"
        );
    }
}

#[test]
fn a_repeated_format_flag_is_rejected() {
    let r = run(&["version", "--plain", "--plain"]);
    assert!(!r.ok);
    assert!(r.stderr.contains("more than once"));
}

// ---- diff ------------------------------------------------------------------

/// Write two tiny package trees — the second adds an exposed value, a compatible
/// change — and return their paths for a `diff` in report mode.
fn compatible_pkg_pair(tag: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!(
        "ipe-fmt-diff-{}-{tag}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let old = base.join("old");
    let new = base.join("new");
    for dir in [&old, &new] {
        std::fs::create_dir_all(dir.join("src")).expect("create temp src dir");
    }
    let v1 = "module Lib exposing (double)\n\nimport Ipe.Prelude exposing (..)\n\n\n\
              double : Int -> Int\ndouble n =\n    n + n\n";
    let v2 = "module Lib exposing (double, triple)\n\nimport Ipe.Prelude exposing (..)\n\n\n\
              double : Int -> Int\ndouble n =\n    n + n\n\n\n\
              triple : Int -> Int\ntriple n =\n    n + n + n\n";
    std::fs::write(old.join("src").join("Lib.ipe"), v1).expect("write old Lib");
    std::fs::write(new.join("src").join("Lib.ipe"), v2).expect("write new Lib");
    (old, new)
}

#[test]
fn diff_plain_emits_flush_left_records() {
    let (old, new) = compatible_pkg_pair("plain");
    let r = run(&[
        "diff",
        "--plain",
        &old.to_string_lossy(),
        &new.to_string_lossy(),
    ]);
    assert!(r.ok, "stderr: {}", r.stderr);
    // A flush-left `change<TAB>…` row for the added value, then a `bump` verdict.
    assert!(
        r.stdout.lines().any(|l| l.starts_with("change\t")),
        "plain diff must carry a flush-left change record:\n{}",
        r.stdout
    );
    let bump = r
        .stdout
        .lines()
        .find(|l| l.starts_with("bump\t"))
        .unwrap_or_default();
    assert!(
        bump.contains("compatible") && bump.contains("patch"),
        "plain diff must carry a bump verdict record: {bump:?}"
    );
    assert!(
        !r.stdout.lines().any(|l| l.starts_with(' ')),
        "plain diff must be flush-left"
    );
}

#[test]
fn diff_json_is_a_stable_object() {
    let (old, new) = compatible_pkg_pair("json");
    let r = run(&[
        "diff",
        "--json",
        &old.to_string_lossy(),
        &new.to_string_lossy(),
    ]);
    assert!(r.ok, "stderr: {}", r.stderr);
    let out = r.stdout.trim();
    for field in [
        "\"compatibility\":\"compatible\"",
        "\"required\":\"patch\"",
        "\"floor\":",
        "\"changes\":[",
    ] {
        assert!(out.contains(field), "json diff missing {field}:\n{out}");
    }
    assert!(!out.starts_with(' '), "json diff must be flush-left");
}

// ---- gutter + frame: human commands ----------------------------------------

/// `ipe init <dir>` prints a next-steps message: every non-blank line is
/// indented by the two-space gutter and the output opens and closes with a
/// blank line (the frame).
#[test]
fn init_human_output_is_guttered_and_framed() {
    let dir = std::env::temp_dir().join(format!(
        "ipe-148-init-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let target = dir.join("my-app");
    let r = run(&["init", &target.to_string_lossy()]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(r.ok, "init must succeed; stderr: {}", r.stderr);
    // Every non-blank stdout line sits in the two-space gutter.
    for line in r.stdout.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            line.starts_with("  "),
            "init output must be guttered, got: {line:?}"
        );
    }
    // The frame: output opens and closes with a blank line.
    assert!(
        r.stdout.starts_with('\n'),
        "init output must open with a newline, got: {:?}",
        &r.stdout[..r.stdout.len().min(20)]
    );
    assert!(
        r.stdout.ends_with('\n'),
        "init output must close with a newline"
    );
}

/// `ipe upgrade --dry-run` is a human-facing confirmation: guttered + framed,
/// flush stdout (not machine output).
#[test]
fn upgrade_dry_run_is_guttered_and_framed() {
    let r = run(&["upgrade", "--dry-run"]);
    assert!(r.ok, "upgrade --dry-run must exit 0; stderr: {}", r.stderr);
    // Every non-blank line is indented by the gutter.
    for line in r.stdout.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            line.starts_with("  "),
            "upgrade --dry-run output must be guttered, got: {line:?}"
        );
    }
    assert!(
        r.stdout.starts_with('\n'),
        "upgrade --dry-run must open with a newline"
    );
    assert!(
        r.stdout.ends_with('\n'),
        "upgrade --dry-run must close with a newline"
    );
    // The actual command is in the output.
    assert!(
        r.stdout.contains("would run:"),
        "upgrade --dry-run must name the command it would run"
    );
}

// ---- upgrade no-prebuilt-binary error rendering ----------------------------

/// A genuine CLI misuse (`ipe upgrade --bad-flag`) must print the `upgrade`
/// command's `--help` page — a bad flag IS misuse, so help is appropriate.
#[test]
fn upgrade_bad_flag_shows_help() {
    let r = run(&["upgrade", "--bad-flag"]);
    assert!(!r.ok, "a bad upgrade flag must exit non-zero");
    // The help page for `upgrade` must appear on stderr.
    assert!(
        r.stderr.contains("upgrade"),
        "bad upgrade flag must print upgrade help; stderr: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("--dry-run") || r.stderr.contains("Options"),
        "bad upgrade flag must include options/help text; stderr: {}",
        r.stderr
    );
}

/// `CliError::UpgradeNoPrebuilt` must render as a self-contained diagnostic
/// — no `--help` page, no `ipe: ` prefix — because it is an operational
/// failure (the release exists but the CI artifacts are still uploading), not
/// CLI misuse.
#[test]
fn upgrade_no_prebuilt_renders_message_without_help() {
    let err = ipe::CliError::UpgradeNoPrebuilt {
        version: "v9.9.9".to_owned(),
        platform: "linux-x64".to_owned(),
    };
    let msg = err.to_string();
    // The message names the version and platform.
    assert!(
        msg.contains("v9.9.9"),
        "UpgradeNoPrebuilt message must name the version; got: {msg:?}"
    );
    assert!(
        msg.contains("linux-x64"),
        "UpgradeNoPrebuilt message must name the platform; got: {msg:?}"
    );
    // The message includes the "still being generated" hint.
    assert!(
        msg.contains("still being generated"),
        "UpgradeNoPrebuilt message must include the generation hint; got: {msg:?}"
    );
    // The message points at building from source.
    assert!(
        msg.contains("cargo install"),
        "UpgradeNoPrebuilt message must point at cargo install; got: {msg:?}"
    );
    // The error MUST NOT contain the `--help` / options block that misuse shows.
    assert!(
        !msg.contains("--dry-run") && !msg.contains("Options:"),
        "UpgradeNoPrebuilt must not include the --help options page; got: {msg:?}"
    );
    // Every non-blank line must be guttered (2-space indent) — same as all
    // other human-facing output.
    for line in msg.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            line.starts_with("  "),
            "UpgradeNoPrebuilt message must be guttered; got line: {line:?}"
        );
    }
}

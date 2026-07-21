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
        .join("../../examples/sky/02-go-stdlib/src/Main.ipe");
    path.to_string_lossy().into_owned()
}

// ---- version ---------------------------------------------------------------

#[test]
fn version_default_is_human_and_guttered() {
    let r = run(&["version"]);
    assert!(r.ok);
    // Human form sits in the two-space gutter and names `ipe`.
    assert!(
        r.stdout.starts_with("  ipe "),
        "human version must be guttered: {:?}",
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
        r.stdout.lines().all(|l| l.is_empty() || l.starts_with("  ")),
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
    assert_eq!(r.stdout.trim(), "{\"capabilities\":[\"network\",\"clock\"]}");
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
    assert!(out.starts_with("{\"codes\":["), "json must open the codes array");
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
        assert!(
            r.stderr.contains(&format!("ipe {}", cmd[0])),
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

//! End-to-end tests for `ipe doc` — the documentation-generation command.
//!
//! Covers the closed `DocMode` subcommand surface (generate vs check, with
//! invalid flag combinations rejected), `docs.json` generation over a real
//! fixture package, and the coverage gate's exit code on a missing doc-comment.
//!
//! Each test returns `io::Result` so filesystem setup propagates with `?` rather
//! than `unwrap`/`expect` (both workspace-denied). A setup failure fails the test
//! by the returned `Err`, exactly as a panic would, without a denied construct.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
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

/// The `str` form of a path, empty when it is not valid UTF-8 (never the case
/// for the temp paths these tests build).
fn as_str(path: &Path) -> &str {
    path.to_str().unwrap_or_default()
}

/// A fresh, unique temp directory for one test (removed first if present).
fn fresh_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ipe_doc_test_{tag}"));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// Write a fully-documented one-module package under `dir/src` and return `dir`.
fn documented_package(tag: &str) -> io::Result<PathBuf> {
    let dir = fresh_dir(tag);
    let src = dir.join("src");
    fs::create_dir_all(&src)?;
    fs::write(
        src.join("Shapes.ipe"),
        "-- | Shapes — a tiny geometry library.\n\
         module Shapes exposing (Shape, area)\n\n\
         -- | A geometric shape.\n\
         type Shape\n    = Circle Float\n    | Rectangle Float Float\n\n\
         -- | `area shape` — the area of `shape`.\n\
         area : Shape -> Float\n\
         area shape =\n    case shape of\n        \
         Circle r ->\n            r\n\n        \
         Rectangle w h ->\n            w * h\n",
    )?;
    Ok(dir)
}

#[test]
fn generate_writes_docs_json_and_markdown() -> io::Result<()> {
    let pkg = documented_package("generate")?;
    let out = pkg.join("out");
    let (ok, stdout, stderr) = run(&["doc", as_str(&pkg), "--out", as_str(&out)]);
    assert!(ok, "generate must succeed:\n{stdout}\n{stderr}");

    let json = fs::read_to_string(out.join("docs.json"))?;
    // The versioned schema and the exposed surface (module, union, value) with
    // its checker-provided signature and its scanned doc-comment.
    assert!(
        json.contains("\"version\": 1"),
        "schema is versioned:\n{json}"
    );
    assert!(json.contains("\"name\": \"Shapes\""));
    assert!(json.contains("\"name\": \"Shape\""));
    assert!(json.contains("\"name\": \"area\""));
    assert!(
        json.contains("Shape -> Float"),
        "the value's checker signature is present:\n{json}"
    );
    assert!(
        json.contains("A geometric shape."),
        "the union's doc-comment is present:\n{json}"
    );

    let md = fs::read_to_string(out.join("Shapes.md"))?;
    assert!(md.contains("# Shapes"));
    assert!(md.contains("### `area"));
    Ok(())
}

#[test]
fn check_passes_when_every_binding_is_documented() -> io::Result<()> {
    let pkg = documented_package("check_pass")?;
    let (ok, stdout, stderr) = run(&["doc", "check", as_str(&pkg)]);
    assert!(
        ok,
        "check must exit 0 for a fully-documented package:\n{stdout}\n{stderr}"
    );
    Ok(())
}

#[test]
fn check_fails_and_names_the_undocumented_binding() -> io::Result<()> {
    let dir = fresh_dir("check_fail");
    let src = dir.join("src");
    fs::create_dir_all(&src)?;
    // `shout` is exposed but carries no `-- |` comment.
    fs::write(
        src.join("Bare.ipe"),
        "module Bare exposing (greet, shout)\n\n\
         -- | `greet name` — a greeting.\n\
         greet : String -> String\n\
         greet name =\n    name\n\n\
         shout : String -> String\n\
         shout name =\n    name\n",
    )?;

    let (ok, _stdout, stderr) = run(&["doc", "check", as_str(&dir)]);
    assert!(!ok, "check must exit non-zero on a missing doc-comment");
    assert!(
        stderr.contains("Bare.shout"),
        "the report must name the undocumented binding, got:\n{stderr}"
    );
    // The gate reports plainly — it is not a command misuse, so it does not dump
    // the `--help` page.
    assert!(
        !stderr.contains("Options:"),
        "a coverage failure must not print the command help page, got:\n{stderr}"
    );
    Ok(())
}

#[test]
fn check_rejects_the_out_flag() {
    // `--out` is a generate-only flag; under `check` it is unrepresentable in
    // `DocMode` and rejected at the boundary.
    let (ok, _stdout, stderr) = run(&["doc", "check", "--out", "x"]);
    assert!(!ok, "`ipe doc check --out` must be rejected");
    assert!(
        stderr.contains("--out"),
        "the error must mention the offending flag, got:\n{stderr}"
    );
}

#[test]
fn unknown_flag_is_rejected() {
    let (ok, _stdout, stderr) = run(&["doc", "--bogus"]);
    assert!(!ok, "an unknown flag must be rejected");
    assert!(
        stderr.contains("bogus"),
        "the error must name the bad flag, got:\n{stderr}"
    );
}

#[test]
fn help_page_describes_only_the_shipped_surface() {
    let (ok, stdout, _) = run(&["doc", "--help"]);
    assert!(ok, "`ipe doc --help` exits 0");
    assert!(stdout.contains("ipe doc"), "help names the command");
    assert!(
        stdout.contains("check"),
        "help mentions the check subcommand"
    );
    // Deferred surfaces must not be advertised.
    assert!(
        !stdout.contains("serve") && !stdout.to_lowercase().contains("html"),
        "help must not advertise unshipped serve/HTML, got:\n{stdout}"
    );
}

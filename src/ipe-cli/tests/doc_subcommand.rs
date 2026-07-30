//! End-to-end tests for `ipe doc` — the documentation-generation command.
//!
//! Covers the closed `DocMode` subcommand surface (generate / serve / check,
//! with invalid flag combinations rejected), `docs.json` + Markdown + HTML
//! generation over a real fixture package, cross-reference resolution (an
//! in-package type links, a built-in does not), the coverage gate's exit code on
//! a missing doc-comment, and the `serve` preview binding a free loopback port
//! and returning the index page.
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
fn generate_writes_a_self_contained_html_site_with_anchors_and_xrefs() -> io::Result<()> {
    let pkg = documented_package("html")?;
    let out = pkg.join("out");
    let (ok, stdout, stderr) = run(&["doc", as_str(&pkg), "--out", as_str(&out)]);
    assert!(ok, "generate must succeed:\n{stdout}\n{stderr}");

    // The site is self-contained: an index page, a per-module page, bundled CSS.
    let index = fs::read_to_string(out.join("index.html"))?;
    assert!(index.contains("<!DOCTYPE html>"));
    assert!(
        index.contains("href=\"style.css\""),
        "the index links the bundled CSS:\n{index}"
    );
    assert!(
        index.contains("href=\"Shapes.html\""),
        "the index lists the module:\n{index}"
    );
    assert!(
        out.join("style.css").exists(),
        "the CSS is written beside it"
    );

    let page = fs::read_to_string(out.join("Shapes.html"))?;
    // Stable per-entry anchors, identical to the docs.json anchor scheme.
    assert!(
        page.contains("id=\"Shape\""),
        "the type has an anchor:\n{page}"
    );
    assert!(
        page.contains("id=\"area\""),
        "the value has an anchor:\n{page}"
    );
    // A cross-reference: the in-package `Shape` links; the builtin `Float` does
    // not.
    assert!(
        page.contains("<a href=\"Shapes.html#Shape\">"),
        "the in-package type links:\n{page}"
    );
    assert!(
        !page.contains(">Float</a>"),
        "a built-in type is plain text, never a dangling link:\n{page}"
    );
    Ok(())
}

#[test]
fn docs_json_records_resolved_cross_references() -> io::Result<()> {
    let pkg = documented_package("xref_json")?;
    let out = pkg.join("out");
    let (ok, _o, _e) = run(&["doc", as_str(&pkg), "--out", as_str(&out)]);
    assert!(ok);

    let json = fs::read_to_string(out.join("docs.json"))?;
    // `area : Shape -> Float` records exactly one reference — the in-package
    // `Shape` — and none for the built-in `Float`.
    assert!(
        json.contains("\"anchor\": \"Shapes#Shape\""),
        "the in-package reference is recorded:\n{json}"
    );
    Ok(())
}

#[test]
fn serve_binds_a_free_port_and_serves_the_index() -> io::Result<()> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;

    let pkg = documented_package("serve")?;
    // Spawn `ipe doc serve` with an auto-selected port and read the URL it prints.
    let mut child = Command::new(env!("CARGO_BIN_EXE_ipe"))
        .args(["doc", "serve", as_str(&pkg)])
        .env("NO_COLOR", "1")
        // Never let the preview pop (or spawn) a browser opener under test.
        .env("IPE_DOC_NO_OPEN", "1")
        .stdout(std::process::Stdio::piped())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("serve produced no stdout handle"))?;
    let mut lines = BufReader::new(stdout).lines();
    // The announce is framed (a leading blank line + a 2-space gutter), so scan
    // for the line carrying the URL rather than assuming it is the first line.
    let announce = lines
        .by_ref()
        .map_while(Result::ok)
        .find(|l| l.contains("http://"))
        .ok_or_else(|| io::Error::other("serve printed no URL"))?;

    // Extract `127.0.0.1:<port>` from the announce line.
    let addr = announce
        .split_whitespace()
        .find(|w| w.starts_with("http://"))
        .and_then(|u| u.trim_start_matches("http://").split('/').next())
        .map(str::to_owned);
    let Some(addr) = addr else {
        let _ = child.kill();
        return Err(io::Error::other(format!(
            "no URL in serve announce: {announce}"
        )));
    };

    // Fetch `/` with a hand-rolled HTTP/1.1 GET and confirm it is the index page.
    let result = (|| -> io::Result<String> {
        let mut stream = TcpStream::connect(&addr)?;
        stream.write_all(
            format!("GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes(),
        )?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Ok(response)
    })();

    // Always reap the server, whatever the fetch did.
    let _ = child.kill();
    let _ = child.wait();

    let response = result?;
    assert!(
        response.contains("200 OK"),
        "serve returns 200 for /:\n{response}"
    );
    assert!(
        response.contains("<h1>API documentation</h1>"),
        "serve returns the index HTML:\n{response}"
    );
    assert!(
        response.contains("Shapes.html"),
        "the served index lists the module:\n{response}"
    );
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
fn help_page_describes_the_shipped_surface() {
    let (ok, stdout, _) = run(&["doc", "--help"]);
    assert!(ok, "`ipe doc --help` exits 0");
    assert!(stdout.contains("ipe doc"), "help names the command");
    assert!(
        stdout.contains("check"),
        "help mentions the check subcommand"
    );
    // The now-shipped surface is advertised: the serve subcommand, the HTML
    // rendering, and the --write-format / --port flags.
    assert!(
        stdout.contains("serve"),
        "help mentions serve, got:\n{stdout}"
    );
    assert!(
        stdout.to_uppercase().contains("HTML"),
        "help mentions HTML, got:\n{stdout}"
    );
    assert!(
        stdout.contains("--write-format") && stdout.contains("--port"),
        "help mentions the generate and serve flags, got:\n{stdout}"
    );
    // The new --list and module-query surface is also advertised.
    assert!(
        stdout.contains("--list"),
        "help mentions --list, got:\n{stdout}"
    );
    assert!(
        stdout.contains("--plain") && stdout.contains("--json"),
        "help mentions --plain/--json for queries, got:\n{stdout}"
    );
    // Still-deferred surfaces must not be advertised.
    assert!(
        !stdout.to_lowercase().contains("search"),
        "help must not advertise unshipped full-text search, got:\n{stdout}"
    );
}

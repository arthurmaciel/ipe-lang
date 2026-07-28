#![forbid(unsafe_code)]
//! End-to-end `ipe package audit`: the SP4 Tier-1 package gate.
//!
//! A clean package passes; a package with (a) an undeclared `network`
//! capability, (b) a semver under-bump against a published predecessor, or (c) a
//! `panic!` in author-supplied FFI Rust each REJECT with that check's diagnostic.
//! The gate is a security boundary — a check that passed when it should reject
//! would be a hole — so every reject case asserts both the non-zero exit AND the
//! specific diagnostic, not just failure.

// A failed `expect` in test setup IS the failure signal the harness reports.
#![allow(clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A fresh, unique temp package directory with a `src/` subdir.
fn temp_pkg(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ipe-audit-test-{}-{}-{}",
        std::process::id(),
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create temp src dir");
    dir
}

/// Write `ipe.toml` and `src/Main.ipe` for a package.
fn write_package(pkg: &Path, manifest: &str, main: &str) {
    std::fs::write(pkg.join("ipe.toml"), manifest).expect("write ipe.toml");
    std::fs::write(pkg.join("src").join("Main.ipe"), main).expect("write Main.ipe");
}

/// The workspace root (two levels up from this crate's manifest) — the CWD the
/// audit runs from so `resolve_runtime` finds the in-repo runtime tree.
fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Run `ipe package audit <pkg>` with an isolated (empty) index directory unless
/// `index` overrides it, returning `(success, stdout, stderr)`.
fn run_audit(pkg: &Path, index: &Path) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_ipe"))
        .arg("package")
        .arg("audit")
        .arg(pkg)
        .arg("--index")
        .arg(index)
        .current_dir(repo_root())
        .output()
        .expect("run ipe package audit");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A pure program that exercises no capability.
const PURE_MAIN: &str = "module Main exposing (main)\n\
                         \n\
                         import Ipe.String as String\n\
                         import Ipe.Io as Io\n\
                         \n\
                         main : Task ()\n\
                         main =\n\
                         \x20   Io.println (String.toUpper \"hello\")\n";

/// A program that makes a network request — its inferred capability set is
/// `{network}`.
const NETWORK_MAIN: &str = "module Main exposing (main)\n\
                            \n\
                            import Ipe.Http as Http\n\
                            import Ipe.Task as Task\n\
                            import Ipe.Io as Io\n\
                            \n\
                            main : Task ()\n\
                            main =\n\
                            \x20   Http.get \"http://example.com\"\n\
                            \x20       |> Task.andThen (\\_ -> Io.println \"done\")\n";

/// An empty index checkout root (no `packages/` entries) — used when a package
/// has no published predecessor, so the enforced-semver check skips.
fn empty_index(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ipe-audit-index-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("packages")).expect("create index packages dir");
    dir
}

#[test]
fn a_clean_package_passes() {
    let pkg = temp_pkg("clean");
    write_package(
        &pkg,
        "name = \"clean-pkg\"\nversion = \"0.1.0\"\n\n[source]\nroot = \"src\"\n",
        PURE_MAIN,
    );
    let index = empty_index("clean");

    let (ok, stdout, stderr) = run_audit(&pkg, &index);
    assert!(
        ok,
        "a clean package must pass the gate; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("all Tier-1 checks passed"),
        "the pass line is printed; got:\n{stdout}"
    );
}

#[test]
fn an_undeclared_network_capability_rejects() {
    let pkg = temp_pkg("undeclared-net");
    // The program uses `network` but declares NOTHING — a hidden effect.
    write_package(
        &pkg,
        "name = \"leaky-pkg\"\nversion = \"0.1.0\"\n\n[source]\nroot = \"src\"\n",
        NETWORK_MAIN,
    );
    let index = empty_index("undeclared-net");

    let (ok, stdout, stderr) = run_audit(&pkg, &index);
    assert!(
        !ok,
        "an undeclared network capability must reject; stdout:\n{stdout}"
    );
    assert!(
        stderr.contains("capability consistency"),
        "the reject names the capability check; got:\n{stderr}"
    );
    assert!(
        stderr.contains("network") && stderr.contains("used but NOT declared"),
        "the diagnostic names the hidden `network` effect; got:\n{stderr}"
    );
}

#[test]
fn an_overdeclared_capability_rejects() {
    let pkg = temp_pkg("overdeclared");
    // The pure program declares `filesystem` it never uses — an over-broad claim.
    write_package(
        &pkg,
        "name = \"broad-pkg\"\nversion = \"0.1.0\"\n\n[source]\nroot = \"src\"\n\n\
         [capabilities]\ndeclared = [\"filesystem\"]\n",
        PURE_MAIN,
    );
    let index = empty_index("overdeclared");

    let (ok, _stdout, stderr) = run_audit(&pkg, &index);
    assert!(!ok, "an over-broad declaration must reject");
    assert!(
        stderr.contains("declared but NOT used") && stderr.contains("filesystem"),
        "the diagnostic names the unused `filesystem` claim; got:\n{stderr}"
    );
}

#[test]
fn an_unimported_sibling_capability_rejects() {
    // The whole-package hole: `Main` is pure and never imports `Extra`, but the
    // package SHIPS `Extra`, which makes a network call. A downstream consumer
    // can `import Extra`, so the package's honest capability set is `{network}` —
    // declaring nothing is a hidden effect the gate must reject even though the
    // entry's own reachability closure is capability-free.
    let pkg = temp_pkg("sibling-cap");
    write_package(
        &pkg,
        "name = \"sibling-pkg\"\nversion = \"0.1.0\"\n\n[source]\nroot = \"src\"\n",
        // Pure Main — does NOT import Extra.
        "module Main exposing (main)\n\nimport Ipe.Io as Io\n\n\
import Ipe.Io
         main : Task ()\nmain =\n\x20   Io.println \"hi\"\n",
    );
    // An exposed sibling that reaches the network, unimported by Main.
    std::fs::write(
        pkg.join("src").join("Extra.ipe"),
        "module Extra exposing (fetch)\n\nimport Ipe.Http as Http\n\
         import Ipe.Task as Task\nimport Ipe.Io as Io\n\n\
import Ipe.Http
import Ipe.Io
         fetch : Task ()\nfetch =\n\
         \x20   Http.get \"http://example.com\"\n\
         \x20       |> Task.andThen (\\_ -> Io.println \"done\")\n",
    )
    .expect("write Extra");
    let index = empty_index("sibling-cap");

    let (ok, stdout, stderr) = run_audit(&pkg, &index);
    assert!(
        !ok,
        "a network-using sibling module must reject even when Main never imports \
         it; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("capability consistency")
            && stderr.contains("network")
            && stderr.contains("used but NOT declared"),
        "the diagnostic names the hidden sibling `network` effect; got:\n{stderr}"
    );
}

#[test]
fn a_semver_underbump_rejects() {
    let pkg = temp_pkg("underbump-new");
    // The new version is a BREAKING change (Lib.double's type changed) but only
    // bumps the patch — an under-bump the gate must reject.
    let manifest = "name = \"semver-pkg\"\nversion = \"0.1.1\"\n\n[source]\nroot = \"src\"\n";
    std::fs::write(pkg.join("ipe.toml"), manifest).expect("write ipe.toml");
    std::fs::write(
        pkg.join("src").join("Main.ipe"),
        "module Main exposing (main)\n\nimport Ipe.Io as Io\n\n\
import Ipe.Io
         main : Task ()\nmain =\n\x20   Io.println \"hi\"\n",
    )
    .expect("write Main");
    std::fs::write(
        pkg.join("src").join("Lib.ipe"),
        "module Lib exposing (double)\n\nimport Ipe.Prelude exposing (..)\n\n\n\
import Ipe.String
         double : Int -> String\ndouble n =\n\x20   String.fromInt (n + n)\n",
    )
    .expect("write new Lib");

    // The predecessor 0.1.0 exposes `double : Int -> Int`; publish it into a
    // git-backed index the audit fetches + hash-verifies as the baseline.
    let index = published_predecessor_index(
        "semver-pkg",
        "0.1.0",
        "module Lib exposing (double)\n\nimport Ipe.Prelude exposing (..)\n\n\n\
         double : Int -> Int\ndouble n =\n\x20   n + n\n",
        "module Main exposing (main)\n\nimport Ipe.Io as Io\n\n\
import Ipe.Io
         main : Task ()\nmain =\n\x20   Io.println \"hi\"\n",
    );

    let (ok, stdout, stderr) = run_audit(&pkg, &index.index_root);
    assert!(
        !ok,
        "a breaking change under a patch bump must reject; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("enforced semver"),
        "the reject names the semver check; got:\n{stderr}"
    );
    assert!(
        stderr.contains("0.2.0"),
        "the diagnostic names the required floor; got:\n{stderr}"
    );
}

#[test]
fn a_panic_in_author_ffi_rust_rejects() {
    let pkg = temp_pkg("ffi-panic");
    write_package(
        &pkg,
        "name = \"ffi-pkg\"\nversion = \"0.1.0\"\n\n[source]\nroot = \"src\"\n",
        PURE_MAIN,
    );
    // Plant an author-supplied FFI wrapper (`_bindings.rs`) that panics. It has
    // no `.consumer.json`, so the catalog loader ignores it (the build stays
    // clean) — but the provenance scan reads it as author Rust and rejects.
    let cache = pkg.join(".ipe/cache/ffi/rust");
    std::fs::create_dir_all(&cache).expect("create ffi cache dir");
    std::fs::write(
        cache.join("mycrate_bindings.rs"),
        "pub fn wrap() -> i64 {\n    panic!(\"author wrote an abrupt failure\");\n}\n",
    )
    .expect("write author bindings");
    let index = empty_index("ffi-panic");

    let (ok, stdout, stderr) = run_audit(&pkg, &index);
    assert!(
        !ok,
        "an authored panic in FFI wrapper Rust must reject; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("provenance panic-scan"),
        "the reject names the provenance check; got:\n{stderr}"
    );
    assert!(
        stderr.contains("author-supplied FFI Rust") && stderr.contains("panic!"),
        "the diagnostic attributes the panic to author Rust and names it; got:\n{stderr}"
    );
    assert!(
        stderr.contains("mycrate_bindings.rs"),
        "the diagnostic points at the offending file; got:\n{stderr}"
    );
}

/// A published-predecessor index backed by a real git repo, so the audit's
/// semver check can fetch + hash-verify the baseline source exactly as it would
/// in production.
struct PublishedIndex {
    index_root: PathBuf,
}

/// Build a git repo holding version `version` of `name` (a `src/Lib.ipe` +
/// `src/Main.ipe`), then write an index entry pinning that commit and the tree's
/// content hash, so `ipe package audit --index <root>` resolves it as the
/// predecessor.
fn published_predecessor_index(name: &str, version: &str, lib: &str, main: &str) -> PublishedIndex {
    // The predecessor's source repo.
    let src_repo = temp_pkg(&format!("{name}-src-{version}"));
    std::fs::write(src_repo.join("src").join("Lib.ipe"), lib).expect("write baseline Lib");
    std::fs::write(src_repo.join("src").join("Main.ipe"), main).expect("write baseline Main");

    git(&src_repo, &["init", "--quiet"]);
    git(&src_repo, &["add", "-A"]);
    git(
        &src_repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "--quiet",
            "-m",
            "v",
        ],
    );
    let rev = git_stdout(&src_repo, &["rev-parse", "HEAD"]);
    let rev = rev.trim();

    // The content hash the resolver verifies against is computed over the
    // fetched tree; `ipe::resolve::hash_source_tree` computes the exact same hash
    // the gate re-derives, so the baseline verifies rather than tripping the
    // hash-mismatch boundary.
    let sha = ipe::resolve::hash_source_tree(&src_repo).expect("hash baseline tree");

    let index_root = empty_index(&format!("{name}-{version}"));
    let entry = format!(
        "name = \"{name}\"\npublisher = \"tester\"\n\n[[version]]\nversion = \"{version}\"\n\
         source = \"{}\"\nrev = \"{rev}\"\nsha256 = \"{sha}\"\ncapabilities = []\n",
        src_repo.display()
    );
    std::fs::write(
        index_root.join("packages").join(format!("{name}.toml")),
        entry,
    )
    .expect("write index entry");

    PublishedIndex { index_root }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

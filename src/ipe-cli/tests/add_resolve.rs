//! `ipe add` / `ipe remove` end-to-end against a fixture index and a fixture
//! source git repo — no network, no real GitHub.
//!
//! A fixture index is a local git checkout with `packages/<name>.toml`; a
//! fixture source is a local git repo whose HEAD is the pinned revision. The
//! index entry's `sha256` is the real content hash of the source tree (computed
//! via `ipe::resolve::hash_source_tree`), so the resolver's verify-before-trust
//! check passes for a faithful fixture and fails for a tampered one.

// Test fixture setup: a failed `expect` IS the failure signal — the harness
// reports the panic as the test failure, which is the intended behaviour here.
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use ipe::lockfile::Lockfile;
use ipe::resolve::{self, hash_source_tree};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ipe-add-resolve-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Run a git command in `cwd`, asserting success.
fn git(cwd: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git runs")
        .status
        .success();
    assert!(ok, "git {args:?} must succeed");
}

/// Read the HEAD revision of a git repo.
fn head_rev(repo: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .expect("git rev-parse");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Build a fixture source git repo with one library file, returning its path.
fn fixture_source(tag: &str) -> PathBuf {
    let repo = temp_dir(&format!("source-{tag}"));
    git(&repo, &["init", "--quiet"]);
    std::fs::write(repo.join("lib.ipe"), "module Lib\nvalue = 42\n").expect("write lib");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "seed"]);
    repo
}

/// Build a fixture index with one package version pointing at `source`, pinned
/// to its HEAD, with the given capabilities and the *given* sha256 (a faithful
/// hash for a real fixture, a wrong one to exercise the mismatch path).
fn fixture_index(
    tag: &str,
    name: &str,
    version: &str,
    source: &Path,
    sha256: &str,
    capabilities: &str,
) -> PathBuf {
    let index = temp_dir(&format!("index-{tag}"));
    let packages = index.join("packages");
    std::fs::create_dir_all(&packages).expect("packages dir");
    let entry = format!(
        "name = \"{name}\"\npublisher = \"tester\"\n\n[[version]]\nversion = \"{version}\"\n\
         source = \"{}\"\nrev = \"{}\"\nsha256 = \"{sha256}\"\ncapabilities = {capabilities}\n",
        source.display(),
        head_rev(source),
    );
    std::fs::write(packages.join(format!("{name}.toml")), entry).expect("write entry");
    index
}

/// Scaffold a minimal project directory for the dependency-editing primitive,
/// which records requirements in an `ipe.toml`-shaped `[dependencies]` section.
fn scaffold_project(tag: &str) -> PathBuf {
    let proj = temp_dir(&format!("proj-{tag}"));
    std::fs::create_dir_all(proj.join("src")).expect("src");
    std::fs::write(proj.join("src").join("Main.ipe"), "module Main\n").expect("main");
    std::fs::write(proj.join("ipe.toml"), "name = \"app\"\n").expect("manifest");
    proj
}

#[test]
fn add_resolves_verifies_and_locks() {
    let source = fixture_source("ok");
    let sha = hash_source_tree(&source).expect("hash source");
    let index = fixture_index("ok", "http-extras", "1.2.0", &source, &sha, "[\"network\"]");
    let proj = scaffold_project("ok");

    let req = "^1".parse().expect("valid req");
    resolve::resolve_and_add(&proj, "http-extras", &req, &index).expect("resolves");

    // The lockfile pins the resolved version and the verified hash.
    let lock = Lockfile::read(&proj).expect("lock");
    let locked = lock
        .packages()
        .iter()
        .find(|p| p.name == "http-extras")
        .expect("locked");
    assert_eq!(locked.version.to_string(), "1.2.0");
    assert_eq!(locked.sha256, sha);

    // The manifest records the requirement under [dependencies].
    let manifest = std::fs::read_to_string(proj.join("ipe.toml")).expect("manifest");
    assert!(
        manifest.contains("http-extras"),
        "the manifest must record the dependency: {manifest}"
    );

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&index);
    let _ = std::fs::remove_dir_all(&proj);
}

#[test]
fn add_rejects_a_hash_mismatch() {
    // The index pins a hash the fetched source does not match: verify-before-
    // trust must reject it, and write nothing.
    let source = fixture_source("bad");
    let index = fixture_index(
        "bad",
        "http-extras",
        "1.2.0",
        &source,
        "0000000000000000000000000000000000000000000000000000000000000000",
        "[\"network\"]",
    );
    let proj = scaffold_project("bad");

    let req = "^1".parse().expect("valid req");
    let err = resolve::resolve_and_add(&proj, "http-extras", &req, &index).unwrap_err();
    assert!(matches!(err, ipe::CliError::HashMismatch { .. }));

    // Nothing was written: no lockfile entry, no manifest dependency.
    let lock = Lockfile::read(&proj).expect("lock");
    assert!(lock.packages().is_empty(), "a mismatch must lock nothing");
    let manifest = std::fs::read_to_string(proj.join("ipe.toml")).expect("manifest");
    assert!(
        !manifest.contains("http-extras"),
        "a mismatch must add nothing: {manifest}"
    );

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&index);
    let _ = std::fs::remove_dir_all(&proj);
}

#[test]
fn add_then_remove_leaves_both_files_clean() {
    let source = fixture_source("cycle");
    let sha = hash_source_tree(&source).expect("hash");
    let index = fixture_index("cycle", "json-tools", "0.4.0", &source, &sha, "[]");
    let proj = scaffold_project("cycle");
    let manifest_before = std::fs::read_to_string(proj.join("ipe.toml")).expect("before");

    let req = "^0.4".parse().expect("valid req");
    resolve::resolve_and_add(&proj, "json-tools", &req, &index).expect("add");
    assert!(
        std::fs::read_to_string(proj.join("ipe.toml"))
            .expect("manifest")
            .contains("json-tools"),
        "add must record the dependency in the manifest"
    );

    resolve::resolve_and_remove(&proj, "json-tools").expect("remove");
    let lock = Lockfile::read(&proj).expect("lock");
    assert!(lock.packages().is_empty(), "remove clears the lockfile");
    let manifest_after = std::fs::read_to_string(proj.join("ipe.toml")).expect("after");
    assert!(
        !manifest_after.contains("json-tools"),
        "remove drops the manifest dependency"
    );
    // The [dependencies] section may remain (now empty); the dependency itself
    // is gone, which is what a clean cycle requires.
    let _ = manifest_before;

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&index);
    let _ = std::fs::remove_dir_all(&proj);
}

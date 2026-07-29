#![forbid(unsafe_code)]
//! End-to-end `ipe diff`: the gate primitive over two real package trees, and
//! the CLI's exit behaviour in report and `--check` modes.

// Test fixture setup: a failed `expect`/`panic` IS the failure signal — the
// harness reports it as the test failure, which is the intended behaviour here.
#![allow(clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use ipe::diff::{Compatibility, RequiredBump, check_semver_bump};
use semver::Version;

/// A fresh temp package directory, unique per test.
fn temp_pkg(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ipe-diffcli-{}-{}-{}",
        std::process::id(),
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    std::fs::create_dir_all(dir.join("src")).expect("create temp src dir");
    dir
}

fn write_lib(pkg: &Path, source: &str) {
    std::fs::write(pkg.join("src").join("Lib.ipe"), source).expect("write Lib");
}

const V1: &str = r"module Lib exposing (double)

import Ipe.Prelude exposing (..)


double : Int -> Int
double n =
    n + n
";

// A breaking change: `double`'s type changed.
const V2_BREAKING: &str = r"module Lib exposing (double)

import Ipe.Prelude exposing (..)
import Ipe.String


double : Int -> String
double n =
    String.fromInt (n + n)
";

// A compatible change: a new exposed value added.
const V2_COMPATIBLE: &str = r"module Lib exposing (double, triple)

import Ipe.Prelude exposing (..)


double : Int -> Int
double n =
    n + n


triple : Int -> Int
triple n =
    n + n + n
";

#[test]
fn check_semver_bump_classifies_a_breaking_change() {
    let old = temp_pkg("brk-old");
    let new = temp_pkg("brk-new");
    write_lib(&old, V1);
    write_lib(&new, V2_BREAKING);

    let rep = check_semver_bump(&old, &new, &Version::new(0, 1, 0), &Version::new(0, 1, 1))
        .expect("diff succeeds");
    assert_eq!(rep.compatibility, Compatibility::Breaking);
    assert_eq!(rep.required, RequiredBump::Minor);
    assert_eq!(rep.floor, Version::new(0, 2, 0));
    assert!(!rep.satisfied, "a patch bump under-bumps a breaking change");
}

#[test]
fn check_semver_bump_classifies_a_compatible_change() {
    let old = temp_pkg("cmp-old");
    let new = temp_pkg("cmp-new");
    write_lib(&old, V1);
    write_lib(&new, V2_COMPATIBLE);

    let rep = check_semver_bump(&old, &new, &Version::new(0, 1, 0), &Version::new(0, 1, 1))
        .expect("diff succeeds");
    assert_eq!(rep.compatibility, Compatibility::Compatible);
    assert_eq!(rep.required, RequiredBump::Patch);
    assert!(rep.satisfied, "a patch bump clears a compatible change");
}

#[test]
fn cli_diff_reports_and_exits_zero() {
    let old = temp_pkg("cli-old");
    let new = temp_pkg("cli-new");
    write_lib(&old, V1);
    write_lib(&new, V2_COMPATIBLE);

    let out = Command::new(env!("CARGO_BIN_EXE_ipe"))
        .arg("diff")
        .arg(&old)
        .arg(&new)
        .output()
        .expect("run ipe diff");
    assert!(out.status.success(), "report mode exits 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("added Lib.triple"),
        "report names the added value; got:\n{stdout}"
    );
    assert!(
        stdout.contains("compatible"),
        "report names the compatibility; got:\n{stdout}"
    );
}

#[test]
fn cli_diff_check_rejects_an_underbump() {
    let old = temp_pkg("chk-old");
    let new = temp_pkg("chk-new");
    write_lib(&old, V1);
    write_lib(&new, V2_BREAKING);

    let out = Command::new(env!("CARGO_BIN_EXE_ipe"))
        .arg("diff")
        .arg(&old)
        .arg(&new)
        .arg("--check")
        .arg("0.1.0")
        .arg("0.1.1")
        .output()
        .expect("run ipe diff --check");
    assert!(
        !out.status.success(),
        "an under-bump of a breaking change is rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("0.2.0"),
        "the error names the required floor; got:\n{stderr}"
    );
}

#[test]
fn cli_diff_check_accepts_a_sufficient_bump() {
    let old = temp_pkg("acc-old");
    let new = temp_pkg("acc-new");
    write_lib(&old, V1);
    write_lib(&new, V2_BREAKING);

    let out = Command::new(env!("CARGO_BIN_EXE_ipe"))
        .arg("diff")
        .arg(&old)
        .arg(&new)
        .arg("--check")
        .arg("0.1.0")
        .arg("0.2.0")
        .output()
        .expect("run ipe diff --check");
    assert!(
        out.status.success(),
        "a minor bump clears a breaking change; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

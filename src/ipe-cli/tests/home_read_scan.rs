#![forbid(unsafe_code)]
//! Refuses a direct read of the home directory anywhere in the workspace's
//! production sources.
//!
//! An unset, empty, or relative `HOME` resolves against whatever the current
//! working directory is, so every home read goes through one validated accessor
//! that yields an absolute path or nothing: `ipe_sandbox::home::home_dir` on the
//! compiler side and `system::home_dir` in the standalone runtime. Those two
//! files are the only ones allowed to touch the raw variable.

use std::path::{Path, PathBuf};

/// Workspace-relative files that own a validated home accessor.
const ACCESSOR_FILES: &[&str] = &[
    "src/compiler/sandbox/src/home.rs",
    "src/runtime/rust/src/system.rs",
];

/// Whitespace-free prefixes of a raw home read. Each ends at the key's closing
/// quote, not the call's `)`, so a trailing-comma argument list still matches.
const RAW_HOME_READS: &[&str] = &[
    "var(\"HOME\"",
    "var_os(\"HOME\"",
    "var(\"USERPROFILE\"",
    "var_os(\"USERPROFILE\"",
    "env::home_dir",
    "dirs::home_dir",
];

/// The raw home reads `src` contains, whitespace and line breaks ignored.
fn raw_home_reads(src: &str) -> Vec<&'static str> {
    let flat: String = src.chars().filter(|c| !c.is_whitespace()).collect();
    RAW_HOME_READS
        .iter()
        .copied()
        .filter(|needle| flat.contains(needle))
        .collect()
}

/// Recursively collect every production `.rs` file under `dir` into `out`.
///
/// Integration-test trees and build output are skipped: neither ships.
fn collect_production_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            let skipped = path
                .file_name()
                .is_some_and(|n| n == "tests" || n == "target");
            if !skipped {
                collect_production_rs(&path, out);
            }
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_production_source_reads_the_home_directly() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    collect_production_rs(&workspace.join("src"), &mut files);
    assert!(
        !files.is_empty(),
        "the scan found no sources; the walk root is wrong"
    );

    let mut offenders = Vec::new();
    for path in files {
        let rel = path
            .strip_prefix(&workspace)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if ACCESSOR_FILES.contains(&rel.as_str()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let hits = raw_home_reads(&text);
        if !hits.is_empty() {
            offenders.push((rel, hits));
        }
    }

    assert!(
        offenders.is_empty(),
        "raw home reads found; use `ipe_sandbox::home::home_dir` (compiler) or \
         `system::home_dir` (runtime) instead: {offenders:?}"
    );
}

#[test]
fn every_accessor_file_exists() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for rel in ACCESSOR_FILES {
        assert!(
            workspace.join(rel).is_file(),
            "allow-listed accessor `{rel}` is gone; drop it from the list"
        );
    }
}

#[test]
fn a_planted_raw_home_read_is_detected() {
    let planted = [
        "let h = std::env::var_os(\"HOME\");",
        "let h = std::env::var( \"HOME\" ).ok();",
        "let h = env::var_os(\n    \"USERPROFILE\",\n);",
        "let h = read_env_var_os(\"HOME\");",
        "#[allow(deprecated)] let h = std::env::home_dir();",
        "use std::env::home_dir;",
        "let h = dirs::home_dir();",
    ];
    for src in planted {
        assert!(
            !raw_home_reads(src).is_empty(),
            "the scan missed a raw home read: {src:?}"
        );
    }
}

#[test]
fn the_validated_accessors_and_home_writes_are_not_flagged() {
    let clean = [
        "let h = ipe_sandbox::home::home_dir();",
        "let h = crate::env_dir::home();",
        "cmd.env(\"HOME\", scratch);",
        "let v = std::env::var_os(\"HOMEBREW_PREFIX\");",
    ];
    for src in clean {
        assert!(
            raw_home_reads(src).is_empty(),
            "the scan flagged a sanctioned form: {src:?}"
        );
    }
}

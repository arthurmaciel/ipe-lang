#![forbid(unsafe_code)]
//! Refuses every remote transfer in the `ipe` crate that bypasses `remote_ingest`.
//!
//! `remote_ingest` is the one place a `git` or `curl` child is built: it
//! isolates the child from user and system configuration, bounds it by one
//! deadline per transfer, measures what it writes, and kills its whole process
//! group on refusal. An HTTP body read through `ureq` is bounded only by
//! `remote_ingest::read_capped`. This scan refuses, anywhere under `src/` but
//! `remote_ingest.rs`, a literal `Command::new("git"` or `Command::new("curl"`
//! and an unbounded `ureq` body read (`into_json`, `into_string`, or an
//! `into_reader` not handed straight to `read_capped`). Sources are compared
//! with all whitespace removed, so line breaks and spacing cannot split a
//! match.

use std::path::{Path, PathBuf};

/// The one module allowed to build a `git` or `curl` child.
const SPAWN_OWNER: &str = "remote_ingest.rs";

/// Directory nesting the walk descends before refusing to go deeper.
const MAX_DEPTH: usize = 16;

/// Spellings of a raw remote-tool spawn, whitespace removed.
const RAW_SPAWNS: &[&str] = &[
    "Command::new(\"git\"",
    "Command::new(\"curl\"",
    "Command::new(r\"git\"",
    "Command::new(r\"curl\"",
];

/// Unbounded `ureq` body reads, whitespace removed.
const UNBOUNDED_BODY_READS: &[&str] = &[".into_json(", ".into_string()"];

/// The one bounded body read: a response reader handed straight to `read_capped`.
const READER: &str = ".into_reader()";

/// The only call an `into_reader` may appear as the first argument of.
const CAPPED_CALL: &str = "read_capped(";

/// The `ipe` crate's `src/` directory.
fn src_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if depth > MAX_DEPTH {
        return Err(std::io::Error::other(format!(
            "source tree deeper than {MAX_DEPTH}: {}",
            dir.display()
        )));
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, depth + 1, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// `source` with every whitespace character removed.
fn flatten(source: &str) -> String {
    source.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Whether the `into_reader` ending at `before` is the first argument of `read_capped`.
fn reader_is_capped(before: &str) -> bool {
    let receiver = before.trim_end_matches(|c: char| c.is_ascii_alphanumeric() || c == '_');
    receiver.len() < before.len() && receiver.ends_with(CAPPED_CALL)
}

/// Every refused spelling in one flattened source.
fn violations(flat: &str) -> Vec<String> {
    let mut found: Vec<String> = RAW_SPAWNS
        .iter()
        .filter(|needle| flat.contains(*needle))
        .map(|needle| (*needle).to_owned())
        .collect();
    if flat.contains("ureq") {
        found.extend(
            UNBOUNDED_BODY_READS
                .iter()
                .filter(|needle| flat.contains(*needle))
                .map(|needle| (*needle).to_owned()),
        );
    }
    found.extend(
        flat.match_indices(READER)
            .filter(|(at, _)| !flat.get(..*at).is_some_and(reader_is_capped))
            .map(|_| READER.to_owned()),
    );
    found
}

#[test]
fn no_remote_transfer_bypasses_remote_ingest() {
    let mut files = Vec::new();
    rust_files(&src_root(), 0, &mut files).expect("source tree walkable");
    assert!(
        files
            .iter()
            .any(|path| path.file_name().is_some_and(|name| name == SPAWN_OWNER)),
        "the scan must see {SPAWN_OWNER}, or it is scanning the wrong tree"
    );
    let offenders: Vec<String> = files
        .iter()
        .filter(|path| path.file_name().is_none_or(|name| name != SPAWN_OWNER))
        .flat_map(|path| {
            let source = std::fs::read_to_string(path).expect("source readable");
            violations(&flatten(&source))
                .into_iter()
                .map(move |needle| format!("{}: {needle}", path.display()))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "remote transfers must go through remote_ingest (Git / Curl / read_capped):\n{}",
        offenders.join("\n")
    );
}

#[test]
fn matcher_refuses_each_bypass() {
    let refused = [
        "let c = std::process::Command::new(\n    \"git\"\n);",
        "Command::new(\"curl\").arg(url)",
        "Command :: new ( r\"git\" )",
        "use ureq; let v: Value = resp.into_json()?;",
        "use ureq; let s = resp.into_string()?;",
        "let r = resp.into_reader();",
        "read_capped(std::io::empty()).and(resp.into_reader())",
        "read_capped(\n  (resp).into_reader(), 4)",
    ];
    for sample in refused {
        assert!(
            !violations(&flatten(sample)).is_empty(),
            "matcher must refuse: {sample}"
        );
    }
}

#[test]
fn matcher_admits_the_bounded_forms() {
    let admitted = [
        "remote_ingest::read_capped(\n    response.into_reader(),\n    MAX,\n)",
        "Git::isolated(dir).args([\"fetch\"])",
        "Command::new(\"sh\").arg(\"-s\")",
        "let s = value.into_string();",
    ];
    for sample in admitted {
        assert!(
            violations(&flatten(sample)).is_empty(),
            "matcher must admit: {sample}"
        );
    }
}

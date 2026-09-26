#![forbid(unsafe_code)]
//! Ratchet on raw terminal printing in the `ipe` CLI.
//!
//! Human output goes through the one renderer (`ipe::screen`): a framed,
//! guttered, toned screen. Machine output goes through
//! `ipe::screen::emit_machine`. A raw `print!` / `println!` / `eprint!` /
//! `eprintln!` bypasses both, so its output escapes the frame (no header, no
//! gutter, no tone, no bug footer).
//!
//! [`BUDGET`] records, per source file, the raw print macros still waiting to be
//! routed through the renderer. The count must match EXACTLY:
//!
//! * a new raw print (or a new file with one) fails — route it through
//!   `ipe::screen` instead;
//! * a routed site lowers the count, and the budget must be lowered with it, so
//!   the ratchet only ever tightens.

use std::path::{Path, PathBuf};

/// Files that still print raw, with their exact raw print macro counts. A file
/// absent here must print nothing raw.
const BUDGET: &[(&str, usize)] = &[
    ("advisory.rs", 1),
    ("audit.rs", 9),
    ("bin/gen_cli_docs.rs", 1),
    ("clean.rs", 3),
    ("cli_args.rs", 1),
    ("diff.rs", 5),
    ("doc.rs", 28),
    ("driver/build_pipeline.rs", 1),
    ("driver/commands.rs", 21),
    ("driver/commands_pkg.rs", 26),
    ("driver/tests/mod.rs", 1),
    ("ffi.rs", 18),
    ("fmt.rs", 4),
    ("health.rs", 11),
    ("init.rs", 6),
    ("login.rs", 7),
    ("migrate.rs", 2),
    ("publish.rs", 4),
    ("resolve.rs", 4),
    ("run_sandbox.rs", 1),
    ("watch.rs", 35),
];

/// The raw print macros the ratchet counts.
const MACROS: &[&str] = &["print!(", "println!(", "eprint!(", "eprintln!("];

/// Count the raw print macro invocations in `text`: an occurrence counts only
/// when it is not the tail of a longer identifier (`eprint!(` is not a
/// `print!(`).
fn count_raw_prints(text: &str) -> usize {
    MACROS
        .iter()
        .map(|mac| {
            text.match_indices(mac)
                .filter(|(at, _)| {
                    text.get(..*at)
                        .and_then(|before| before.chars().next_back())
                        .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
                })
                .count()
        })
        .sum()
}

/// Every `.rs` file under `dir`, recursively, relative to `root` with `/`
/// separators.
fn rust_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(rel) = path.strip_prefix(root)
            {
                let rel = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                found.push((rel, path));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn raw_prints_match_the_budget_exactly() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let files = rust_files(&src);
    assert!(
        !files.is_empty(),
        "no sources found under {}",
        src.display()
    );

    let mut drift = Vec::new();
    for (rel, path) in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            drift.push(format!("{rel}: unreadable"));
            continue;
        };
        let actual = count_raw_prints(&text);
        let budget = BUDGET
            .iter()
            .find(|(file, _)| file == rel)
            .map_or(0, |(_, n)| *n);
        if actual != budget {
            drift.push(format!("{rel}: {actual} raw prints, budget {budget}"));
        }
    }
    for (file, _) in BUDGET {
        if !files.iter().any(|(rel, _)| rel == file) {
            drift.push(format!("{file}: budgeted but no longer exists"));
        }
    }
    assert!(
        drift.is_empty(),
        "raw print budget drift — route new output through `ipe::screen`, and lower the \
         budget when a site is routed:\n  {}",
        drift.join("\n  ")
    );
}

#[test]
fn the_counter_ignores_longer_macro_names() {
    assert_eq!(count_raw_prints("eprintln!(x); println!(y);"), 2);
    assert_eq!(count_raw_prints("eprint!(a) print!(b)"), 2);
    assert_eq!(count_raw_prints("my_println!(z)"), 0);
    assert_eq!(count_raw_prints("write!(out, x)"), 0);
}

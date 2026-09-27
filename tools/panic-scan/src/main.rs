//! Flag authored abrupt-failure constructs in the production regions of Rust files.
//!
//! `panic-scan <file.rs>…` scans the listed files; `panic-scan --walk <dir>…`
//! scans every `.rs` file under the listed directories. Exit 0 when clean, 1
//! when a banned construct is found, 2 when a file cannot be audited.
//!
//! Test code ([`panic_scan::is_test_path`]) is skipped only once its test-only
//! premise is confirmed on disk ([`panic_scan::check_test_path`]); an
//! unconfirmed test path fails closed with exit 2, because a `pub mod tests;`
//! would put its body in the production build. Files under `templates/` hold
//! emitted-program Rust copied verbatim into every generated binary; the
//! emitted-output package gate covers them, not this compiler-code scan.
//! Inline `#[cfg(test)]` bodies are skipped by the scanner itself.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Flag selecting directory-walk mode.
const WALK_FLAG: &str = "--walk";

/// Exit status for a file that cannot be audited.
const EXIT_UNAUDITABLE: u8 = 2;

/// Why a run stops before its verdict.
type Unauditable = String;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(true) => ExitCode::FAILURE,
        Ok(false) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("panic-scan: {reason}");
            ExitCode::from(EXIT_UNAUDITABLE)
        }
    }
}

/// Scan the files the arguments name; `Ok(true)` when a banned construct is found.
fn run(args: &[String]) -> Result<bool, Unauditable> {
    let files = match args.split_first() {
        Some((flag, dirs)) if flag == WALK_FLAG => walk_all(dirs)?,
        _ => args.iter().map(PathBuf::from).collect(),
    };
    // Files sharing a parent directory share their outermost test marker, so
    // one confirmed premise covers the whole directory.
    let mut verified_test_dirs: BTreeSet<PathBuf> = BTreeSet::new();
    let mut found = false;
    for path in &files {
        if panic_scan::is_template_path(path) {
            continue;
        }
        if panic_scan::is_test_path(path) {
            let dir = path.parent().unwrap_or(Path::new(""));
            if !verified_test_dirs.contains(dir) {
                panic_scan::check_test_path(Path::new(""), path).map_err(|e| {
                    format!(
                        "{e} — cannot confirm {} is test-only; fail closed",
                        path.display()
                    )
                })?;
                verified_test_dirs.insert(dir.to_path_buf());
            }
            continue;
        }
        found |= scan_file(path)?;
    }
    Ok(found)
}

/// Scan one production file, printing each hit; `Ok(true)` when any is found.
fn scan_file(path: &Path) -> Result<bool, Unauditable> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    // A file the scanner cannot lex is unaudited, not clean: "cannot analyze"
    // must never read as "no panics".
    let hits = panic_scan::scan_str(&src).map_err(|e| {
        format!(
            "{}: could not lex ({e}) — cannot audit; fail closed",
            path.display()
        )
    })?;
    for hit in &hits {
        println!(
            "{}:{}: banned abrupt-failure construct `{}`",
            path.display(),
            hit.line,
            hit.tok
        );
    }
    Ok(!hits.is_empty())
}

/// Every `.rs` file under `dirs`, in sorted order.
///
/// A walk that finds no Rust file is refused: an empty scan would read as a pass.
fn walk_all(dirs: &[String]) -> Result<Vec<PathBuf>, Unauditable> {
    let mut files = Vec::new();
    for dir in dirs {
        walk(Path::new(dir), &mut files)?;
    }
    if files.is_empty() {
        return Err(format!(
            "{WALK_FLAG} found no .rs file under {dirs:?} — nothing audited; fail closed"
        ));
    }
    Ok(files)
}

/// Append every `.rs` file under `dir` to `files`.
///
/// A symlinked directory is not descended; a symlinked `.rs` entry is listed,
/// so the scan reads its target rather than skipping it.
fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), Unauditable> {
    let unreadable = |e: std::io::Error| format!("cannot walk {}: {e}", dir.display());
    let mut entries = std::fs::read_dir(dir)
        .map_err(unreadable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(unreadable)?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(unreadable)?;
        if file_type.is_dir() {
            walk(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

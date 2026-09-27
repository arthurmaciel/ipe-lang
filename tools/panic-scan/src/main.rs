//! Flag authored abrupt-failure constructs in the production regions of Rust files.
//!
//! `panic-scan <file>…` scans the listed files; `panic-scan --walk <dir>…`
//! scans every `.rs` file and `Cargo.toml` under the listed directories. Exit 0
//! when clean, 1 when a banned construct is found, 2 when the input cannot be
//! audited — including when there is no input at all.
//!
//! Test code ([`panic_scan::is_test_path`]) is skipped only once its test-only
//! premise is confirmed on disk ([`panic_scan::check_test_path`]); an
//! unconfirmed test path fails closed with exit 2, because a `pub mod tests;`
//! would put its body in the production build. For the same reason a
//! production `#[path]` or `include!` naming test code, a `Cargo.toml` whose
//! production target lies in test code, and a symlinked directory the walk
//! cannot vouch for all exit 2. Files under `templates/` hold emitted-program
//! Rust copied verbatim into every generated binary; the emitted-output package
//! gate covers them, not this compiler-code scan. Inline `#[cfg(test)]` bodies
//! are skipped by the scanner itself.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use panic_scan::{TestPathError, TestPathInclude};

/// Flag selecting directory-walk mode.
const WALK_FLAG: &str = "--walk";

/// File name of a crate manifest.
const MANIFEST_FILE: &str = "Cargo.toml";

/// Exit status for input that cannot be audited.
const EXIT_UNAUDITABLE: u8 = 2;

/// Why a run stops before its verdict.
#[derive(Debug)]
enum Unauditable {
    /// No file or directory was named.
    NoInput,
    /// A walk found no Rust file to scan.
    EmptyWalk { dirs: NonEmpty<PathBuf> },
    /// A test path's test-only premise, or a manifest's targets, do not hold.
    TestPath {
        path: PathBuf,
        source: TestPathError,
    },
    /// A file or directory could not be read.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A file does not lex as Rust tokens.
    Lex {
        path: PathBuf,
        source: proc_macro2::LexError,
    },
    /// A walk met a symlink to a directory, whose contents it cannot vouch for.
    SymlinkedDirectory { path: PathBuf },
    /// A production file compiles in a source the scan skips as test code.
    TestPathInclude {
        path: PathBuf,
        includes: Vec<TestPathInclude>,
    },
}

impl fmt::Display for Unauditable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoInput => write!(
                f,
                "no file named (usage: panic-scan <file>… | panic-scan {WALK_FLAG} <dir>…) — nothing audited; fail closed"
            ),
            Self::EmptyWalk { dirs } => write!(
                f,
                "{WALK_FLAG} found no .rs file under {:?} — nothing audited; fail closed",
                dirs.iter().collect::<Vec<_>>()
            ),
            Self::TestPath { path, source } => write!(
                f,
                "{source} — cannot confirm {} is test-only; fail closed",
                path.display()
            ),
            Self::Io { path, source } => {
                write!(f, "cannot read {}: {source} — fail closed", path.display())
            }
            Self::Lex { path, source } => write!(
                f,
                "{}: could not lex ({source}) — cannot audit; fail closed",
                path.display()
            ),
            Self::SymlinkedDirectory { path } => write!(
                f,
                "{}: symlinked directory — the walk does not follow it, so it cannot be audited; fail closed",
                path.display()
            ),
            Self::TestPathInclude { path, includes } => {
                write!(f, "{}:", path.display())?;
                for include in includes {
                    write!(f, " {include};")?;
                }
                f.write_str(" fail closed")
            }
        }
    }
}

/// A list with at least one element.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NonEmpty<T> {
    first: T,
    rest: Vec<T>,
}

impl<T> NonEmpty<T> {
    /// The list `items`, or `None` when it is empty.
    fn from_vec(items: Vec<T>) -> Option<Self> {
        let mut items = items.into_iter();
        items.next().map(|first| Self {
            first,
            rest: items.collect(),
        })
    }

    /// Every element, first to last.
    fn iter(&self) -> impl Iterator<Item = &T> {
        std::iter::once(&self.first).chain(self.rest.iter())
    }
}

/// What the arguments ask the scan to cover.
#[derive(Debug, PartialEq, Eq)]
enum Mode {
    /// Scan exactly these files.
    Files(NonEmpty<PathBuf>),
    /// Scan every `.rs` file and manifest under these directories.
    Walk(NonEmpty<PathBuf>),
}

impl Mode {
    /// Parse the command-line arguments; naming no file or directory is no input.
    fn parse(args: &[String]) -> Result<Self, Unauditable> {
        let (walk, paths) = match args.split_first() {
            Some((flag, dirs)) if flag == WALK_FLAG => (true, dirs),
            _ => (false, args),
        };
        let paths = NonEmpty::from_vec(paths.iter().map(PathBuf::from).collect())
            .ok_or(Unauditable::NoInput)?;
        Ok(if walk {
            Self::Walk(paths)
        } else {
            Self::Files(paths)
        })
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match Mode::parse(&args).and_then(|mode| run(&mode)) {
        Ok(true) => ExitCode::FAILURE,
        Ok(false) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("panic-scan: {reason}");
            ExitCode::from(EXIT_UNAUDITABLE)
        }
    }
}

/// Scan the files `mode` covers; `Ok(true)` when a banned construct is found.
fn run(mode: &Mode) -> Result<bool, Unauditable> {
    let files = match mode {
        Mode::Files(files) => files.iter().cloned().collect(),
        Mode::Walk(dirs) => walk_all(dirs)?,
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
                panic_scan::check_test_path(Path::new(""), path).map_err(|source| {
                    Unauditable::TestPath {
                        path: path.clone(),
                        source,
                    }
                })?;
                verified_test_dirs.insert(dir.to_path_buf());
            }
            continue;
        }
        if is_manifest(path) {
            panic_scan::check_manifest(path).map_err(|source| Unauditable::TestPath {
                path: path.clone(),
                source,
            })?;
            continue;
        }
        found |= scan_file(path)?;
    }
    Ok(found)
}

/// Whether `path` names a crate manifest.
fn is_manifest(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == MANIFEST_FILE)
}

/// Whether `path` names a Rust source file.
fn is_rust_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "rs")
}

/// Scan one production file, printing each hit; `Ok(true)` when any is found.
fn scan_file(path: &Path) -> Result<bool, Unauditable> {
    let src = std::fs::read_to_string(path).map_err(|source| Unauditable::Io {
        path: path.to_path_buf(),
        source,
    })?;
    // A file the scanner cannot lex is unaudited, not clean: "cannot analyze"
    // must never read as "no panics".
    let scan = panic_scan::scan_source(&src).map_err(|source| Unauditable::Lex {
        path: path.to_path_buf(),
        source,
    })?;
    if !scan.test_path_includes.is_empty() {
        return Err(Unauditable::TestPathInclude {
            path: path.to_path_buf(),
            includes: scan.test_path_includes,
        });
    }
    for hit in &scan.hits {
        println!(
            "{}:{}: banned abrupt-failure construct `{}`",
            path.display(),
            hit.line,
            hit.tok
        );
    }
    Ok(!scan.hits.is_empty())
}

/// Every `.rs` file and manifest under `dirs`, in sorted order.
///
/// A walk that finds no Rust file is refused: an empty scan would read as a pass.
fn walk_all(dirs: &NonEmpty<PathBuf>) -> Result<Vec<PathBuf>, Unauditable> {
    let mut files = Vec::new();
    for dir in dirs.iter() {
        walk(dir, &mut files)?;
    }
    if files.iter().any(|path| is_rust_file(path)) {
        Ok(files)
    } else {
        Err(Unauditable::EmptyWalk { dirs: dirs.clone() })
    }
}

/// Append every `.rs` file and manifest under `dir` to `files`.
///
/// A symlink is judged by its target: a symlinked file is listed, so the scan
/// reads its target; a symlinked directory is refused, because the walk does
/// not follow it and so cannot vouch for what it holds.
fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), Unauditable> {
    let unreadable = |source: std::io::Error| Unauditable::Io {
        path: dir.to_path_buf(),
        source,
    };
    let mut entries = std::fs::read_dir(dir)
        .map_err(unreadable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(unreadable)?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(unreadable)?;
        if file_type.is_symlink() {
            let target = std::fs::metadata(&path).map_err(|source| Unauditable::Io {
                path: path.clone(),
                source,
            })?;
            if target.is_dir() {
                return Err(Unauditable::SymlinkedDirectory { path });
            }
        }
        if file_type.is_dir() {
            walk(&path, files)?;
        } else if is_rust_file(&path) || is_manifest(&path) {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| String::from(*arg)).collect()
    }

    #[test]
    fn naming_no_file_or_directory_is_no_input() {
        for list in [&[][..], &[WALK_FLAG][..]] {
            let mode = Mode::parse(&args(list));
            assert!(matches!(mode, Err(Unauditable::NoInput)), "{list:?}");
        }
    }

    #[test]
    fn arguments_select_the_mode() {
        let mode = Mode::parse(&args(&["a.rs", "b.rs"]));
        assert!(
            matches!(&mode, Ok(Mode::Files(files)) if files.iter().count() == 2),
            "{mode:?}"
        );
        let mode = Mode::parse(&args(&[WALK_FLAG, "src"]));
        assert!(
            matches!(&mode, Ok(Mode::Walk(dirs)) if dirs.first.as_path() == Path::new("src")),
            "{mode:?}"
        );
    }
}

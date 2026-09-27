//! Which Rust files are test code, and the on-disk proof behind that claim.
//!
//! [`is_test_path`] is the ONE path rule every tool uses to exempt a file from a
//! production-only check. It is component-based: a `tests` directory anywhere on
//! the path, or a file named `tests.rs`. A name that merely contains `tests`
//! (`contests.rs`, `tests_util.rs`, `unit_tests/`) is production.
//!
//! The rule rests on a premise the path alone cannot show: a `tests` directory
//! or `tests.rs` inside a crate's source tree is the body of an out-of-line
//! `#[cfg(test)] mod tests;`. A `pub mod tests;` would compile that body into
//! the production build while every path-based check skipped it.
//! [`check_test_path`] confirms the premise on disk — the directory is a crate's
//! integration-test directory (its parent holds `Cargo.toml`), or a sibling
//! module file declares `mod tests;` under a test-only attribute — and fails
//! closed otherwise.

use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use proc_macro2::{Delimiter, TokenStream, TokenTree};

use crate::attr_gates_test_only;

/// Directory name of a crate's integration tests or an out-of-line test module.
const TEST_DIR: &str = "tests";

/// File name of an out-of-line test module's body.
const TEST_MODULE_FILE: &str = "tests.rs";

/// Directory name of the emitted-program Rust copied into generated binaries.
const TEMPLATE_DIR: &str = "templates";

/// Module files that may declare a child module living in directory `dir`.
const DECLARING_FILES_IN_DIR: &[&str] = &["mod.rs", "lib.rs", "main.rs"];

/// Whether `rel` names test code.
///
/// True for a path with a `tests` directory component or a final `tests.rs`
/// component. Pass a path relative to a source root: an absolute path whose
/// ancestor happens to be named `tests` would read as test code (and then
/// fail [`check_test_path`], which is the fail-closed outcome).
#[must_use]
pub fn is_test_path(rel: &Path) -> bool {
    test_marker(rel).is_some()
}

/// Whether `rel` lies under an emitted-program `templates` directory.
///
/// That Rust is copied verbatim into every generated binary and is covered by
/// the emitted-output package gate, not by the compiler-code scan.
#[must_use]
pub fn is_template_path(rel: &Path) -> bool {
    rel.parent()
        .is_some_and(|dir| dir.components().any(|c| c.as_os_str() == TEMPLATE_DIR))
}

/// Whether `rel` is test code AND its test-only premise holds under `root`.
///
/// The fail-closed form for report tools: a test path whose premise cannot be
/// confirmed is treated as production, so it stays in scope.
#[must_use]
pub fn is_verified_test_path(root: &Path, rel: &Path) -> bool {
    is_test_path(rel) && check_test_path(root, rel).is_ok()
}

/// Confirm that test path `rel` (relative to `root`) is compiled only for tests.
///
/// A path that is not a test path is trivially `Ok`. Otherwise the outermost
/// `tests` directory (or the `tests.rs` file) must be a crate's integration-test
/// directory, or be declared `mod tests;` under a test-only attribute
/// (`#[cfg(test)]`, `#[cfg(all(test, …))]`) by a module file beside it:
/// `<dir>/mod.rs`, `<dir>/lib.rs`, `<dir>/main.rs`, or `<dir>.rs`.
///
/// # Errors
///
/// [`TestPathError`] when no module file declares the test module, when one
/// declares it without a test-only attribute, or when a candidate declaring
/// file cannot be read or lexed.
pub fn check_test_path(root: &Path, rel: &Path) -> Result<(), TestPathError> {
    let Some(marker) = test_marker(rel) else {
        return Ok(());
    };
    let dir = root.join(&marker.declaring_dir);
    if marker.kind == MarkerKind::Directory && dir.join("Cargo.toml").is_file() {
        return Ok(());
    }
    let mut declared = false;
    for file in declaring_candidates(&dir) {
        if !file.is_file() {
            continue;
        }
        let src = match std::fs::read_to_string(&file) {
            Ok(src) => src,
            Err(error) => return Err(TestPathError::Unreadable { file, error }),
        };
        match tests_declaration(&src) {
            Ok(Declaration::Absent) => {}
            Ok(Declaration::TestOnly) => declared = true,
            Ok(Declaration::Ungated) => {
                return Err(TestPathError::Ungated {
                    declaring_file: file,
                });
            }
            Err(error) => return Err(TestPathError::Unlexable { file, error }),
        }
    }
    if declared {
        Ok(())
    } else {
        Err(TestPathError::Undeclared {
            test_module: dir.join(marker.kind.name()),
        })
    }
}

/// Why a test path's test-only premise does not hold.
#[derive(Debug)]
pub enum TestPathError {
    /// No module file beside the test module declares `mod tests;`.
    Undeclared { test_module: PathBuf },
    /// `declaring_file` declares `mod tests;` without a test-only attribute.
    Ungated { declaring_file: PathBuf },
    /// A candidate declaring file could not be read.
    Unreadable {
        file: PathBuf,
        error: std::io::Error,
    },
    /// A candidate declaring file does not lex as Rust tokens.
    Unlexable {
        file: PathBuf,
        error: proc_macro2::LexError,
    },
}

impl fmt::Display for TestPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undeclared { test_module } => write!(
                f,
                "{}: no sibling module file declares `#[cfg(test)] mod tests;` for this test module",
                test_module.display()
            ),
            Self::Ungated { declaring_file } => write!(
                f,
                "{}: declares `mod tests;` without `#[cfg(test)]`, so its test module compiles into production",
                declaring_file.display()
            ),
            Self::Unreadable { file, error } => {
                write!(f, "{}: cannot read ({error})", file.display())
            }
            Self::Unlexable { file, error } => {
                write!(f, "{}: could not lex ({error})", file.display())
            }
        }
    }
}

impl std::error::Error for TestPathError {}

/// Whether the test marker is a `tests` directory or a `tests.rs` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerKind {
    Directory,
    ModuleFile,
}

impl MarkerKind {
    const fn name(self) -> &'static str {
        match self {
            Self::Directory => TEST_DIR,
            Self::ModuleFile => TEST_MODULE_FILE,
        }
    }
}

/// The outermost test marker on a path and the directory that holds it.
#[derive(Debug, PartialEq, Eq)]
struct TestMarker {
    declaring_dir: PathBuf,
    kind: MarkerKind,
}

/// Locate the outermost `tests` directory or final `tests.rs` on `rel`.
fn test_marker(rel: &Path) -> Option<TestMarker> {
    let mut declaring_dir = PathBuf::new();
    let mut components = rel.components().peekable();
    while let Some(component) = components.next() {
        let is_last = components.peek().is_none();
        if let Component::Normal(name) = component {
            let kind = if is_last {
                (name == TEST_MODULE_FILE).then_some(MarkerKind::ModuleFile)
            } else {
                (name == TEST_DIR).then_some(MarkerKind::Directory)
            };
            if let Some(kind) = kind {
                return Some(TestMarker {
                    declaring_dir,
                    kind,
                });
            }
        }
        declaring_dir.push(component);
    }
    None
}

/// Module files that may declare `mod tests;` for a test module inside `dir`.
fn declaring_candidates(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = DECLARING_FILES_IN_DIR
        .iter()
        .map(|name| dir.join(name))
        .collect();
    if dir.file_name().is_some() {
        files.push(dir.with_extension("rs"));
    }
    files
}

/// How a module file declares its out-of-line `tests` child module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Declaration {
    /// No top-level `mod tests;`.
    Absent,
    /// `mod tests;` under a test-only attribute.
    TestOnly,
    /// `mod tests;` that also compiles outside the `test` cfg.
    Ungated,
}

/// Find the top-level `mod tests;` in `src` and whether a test-only attribute gates it.
///
/// Attributes accumulate across one item: a test-only `#[…]` arms the gate, and
/// the end of an item (a top-level `;` or brace body) disarms it, so an
/// attribute on an earlier sibling item never gates the declaration.
fn tests_declaration(src: &str) -> Result<Declaration, proc_macro2::LexError> {
    let toks: Vec<TokenTree> = TokenStream::from_str(src)?.into_iter().collect();
    let mut gated = false;
    for (i, tok) in toks.iter().enumerate() {
        match tok {
            TokenTree::Punct(p) if p.as_char() == '#' => {
                if let Some(TokenTree::Group(g)) = toks.get(i + 1) {
                    if g.delimiter() == Delimiter::Bracket && attr_gates_test_only(&g.stream()) {
                        gated = true;
                    }
                }
            }
            TokenTree::Punct(p) if p.as_char() == ';' => gated = false,
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => gated = false,
            TokenTree::Ident(id) if id == "mod" => {
                let names_tests =
                    matches!(toks.get(i + 1), Some(TokenTree::Ident(name)) if name == TEST_DIR);
                let is_out_of_line =
                    matches!(toks.get(i + 2), Some(TokenTree::Punct(p)) if p.as_char() == ';');
                if names_tests && is_out_of_line {
                    return Ok(if gated {
                        Declaration::TestOnly
                    } else {
                        Declaration::Ungated
                    });
                }
            }
            _ => {}
        }
    }
    Ok(Declaration::Absent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_directories_and_module_files_are_test_paths() {
        for rel in [
            "tests/cli.rs",
            "src/ipe-cli/tests/cli.rs",
            "src/driver/tests/mod.rs",
            "src/driver/tests/nested/deep.rs",
            "src/tests.rs",
            "src/unit/tests.rs",
            "./tests/x.rs",
        ] {
            assert!(is_test_path(Path::new(rel)), "{rel} must be a test path");
        }
    }

    #[test]
    fn names_merely_containing_tests_are_production() {
        for rel in [
            "src/contests.rs",
            "src/tests_util.rs",
            "src/unit_tests/mod.rs",
            "src/testsuite/a.rs",
            "src/lib.rs",
            "tests",
            "src/tests.rs.bak",
            "",
        ] {
            assert!(!is_test_path(Path::new(rel)), "{rel} must be production");
        }
    }

    #[test]
    fn the_outermost_marker_names_the_declaring_directory() {
        let marker = test_marker(Path::new("crate/src/a/tests/b/tests/c.rs"));
        assert_eq!(
            marker,
            Some(TestMarker {
                declaring_dir: PathBuf::from("crate/src/a"),
                kind: MarkerKind::Directory,
            })
        );
        let marker = test_marker(Path::new("crate/src/a/tests.rs"));
        assert_eq!(
            marker,
            Some(TestMarker {
                declaring_dir: PathBuf::from("crate/src/a"),
                kind: MarkerKind::ModuleFile,
            })
        );
    }

    #[test]
    fn templates_directories_are_template_paths() {
        assert!(is_template_path(Path::new("src/ipe-cli/templates/main.rs")));
        assert!(!is_template_path(Path::new("src/templates.rs")));
        assert!(!is_template_path(Path::new("src/my_templates/a.rs")));
    }

    #[test]
    fn a_test_only_attribute_gates_the_declaration() {
        for src in [
            "#[cfg(test)]\nmod tests;",
            "#[cfg(test)]\npub(crate) mod tests;",
            "/// Unit tests.\n#[cfg(all(test, unix))]\nmod tests;",
            "#[cfg(test)]\n#[path = \"t/mod.rs\"]\nmod tests;",
            "mod a;\n#[cfg(test)]\nmod tests;\nmod b;",
        ] {
            assert_eq!(
                tests_declaration(src).ok(),
                Some(Declaration::TestOnly),
                "{src:?}"
            );
        }
    }

    #[test]
    fn a_declaration_without_a_test_only_attribute_is_ungated() {
        for src in [
            "mod tests;",
            "pub mod tests;",
            "#[cfg(any(test, feature = \"x\"))]\nmod tests;",
            "#[cfg(not(test))]\nmod tests;",
            "#[cfg(test)]\nuse x;\nmod tests;",
            "#[cfg(test)]\nfn helper() {}\nmod tests;",
            "#![cfg(test)]\nmod tests;",
        ] {
            assert_eq!(
                tests_declaration(src).ok(),
                Some(Declaration::Ungated),
                "{src:?}"
            );
        }
    }

    #[test]
    fn an_inline_or_missing_tests_module_is_no_declaration() {
        for src in [
            "fn f() {}",
            "#[cfg(test)]\nmod tests {\n    fn t() {}\n}",
            "mod contests;",
            "fn f() { mod tests; }",
        ] {
            assert_eq!(
                tests_declaration(src).ok(),
                Some(Declaration::Absent),
                "{src:?}"
            );
        }
    }

    #[test]
    fn a_non_test_path_needs_no_premise() {
        assert!(check_test_path(Path::new("/nonexistent"), Path::new("src/lib.rs")).is_ok());
    }

    #[test]
    fn an_undeclared_test_module_fails_closed() {
        let result = check_test_path(Path::new("/nonexistent"), Path::new("src/tests/a.rs"));
        assert!(
            matches!(result, Err(TestPathError::Undeclared { .. })),
            "{result:?}"
        );
        assert!(!is_verified_test_path(
            Path::new("/nonexistent"),
            Path::new("src/tests/a.rs")
        ));
    }
}

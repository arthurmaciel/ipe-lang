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
//! [`check_test_path`] confirms the premise on disk and fails closed otherwise:
//!
//! * an integration-test directory sits beside a `Cargo.toml` whose explicit
//!   production targets ([`check_manifest`]) name no test code, and no module
//!   file beside it declares the directory as a production module;
//! * any other test module is declared `mod tests;` under a test-only `cfg` by
//!   a sibling module file, every declaration there is test-only, and no other
//!   production item in that file names `tests`.

use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use proc_macro2::{Delimiter, Ident, TokenStream, TokenTree};

use crate::cfg_pred_is_test_only;
use crate::manifest::{ManifestError, parse_manifest};

/// Directory name of a crate's integration tests or an out-of-line test module.
const TEST_DIR: &str = "tests";

/// File name of an out-of-line test module's body.
const TEST_MODULE_FILE: &str = "tests.rs";

/// Directory name of the emitted-program Rust copied into generated binaries.
const TEMPLATE_DIR: &str = "templates";

/// File name of a crate manifest.
const MANIFEST_FILE: &str = "Cargo.toml";

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
/// `tests` directory (or the `tests.rs` file) is judged by the module files
/// beside it — `<dir>/mod.rs`, `<dir>/lib.rs`, `<dir>/main.rs`, `<dir>.rs` —
/// and, for a directory, the `Cargo.toml` beside it. Every candidate module
/// file must be free of production declarations of `tests`; then either the
/// manifest's explicit production targets name no test code, or some candidate
/// declares `mod tests;` under a test-only `cfg`.
///
/// # Errors
///
/// [`TestPathError`] when no module file declares the test module, when a
/// production item declares or names it, when the manifest names test code as
/// a production target, or when a candidate file cannot be read or parsed.
pub fn check_test_path(root: &Path, rel: &Path) -> Result<(), TestPathError> {
    let Some(marker) = test_marker(rel) else {
        return Ok(());
    };
    let dir = root.join(&marker.declaring_dir);
    let mut declared = false;
    for file in declaring_candidates(&dir) {
        if !file.is_file() {
            continue;
        }
        let src = std::fs::read_to_string(&file).map_err(|error| TestPathError::Unreadable {
            file: file.clone(),
            error,
        })?;
        match tests_declaration(&src) {
            Ok(Declaration::Absent) => {}
            Ok(Declaration::TestOnly) => declared = true,
            Ok(Declaration::Ungated(by)) => {
                return Err(TestPathError::Ungated {
                    declaring_file: file,
                    by,
                });
            }
            Err(error) => return Err(TestPathError::Unlexable { file, error }),
        }
    }
    let manifest = dir.join(MANIFEST_FILE);
    if marker.kind == MarkerKind::Directory && manifest.is_file() {
        return check_manifest(&manifest);
    }
    if declared {
        Ok(())
    } else {
        Err(TestPathError::Undeclared {
            test_module: dir.join(marker.kind.name()),
        })
    }
}

/// Confirm that no explicit production target of `manifest` names test code.
///
/// A target names test code when its path, as written, has a `tests`
/// component or ends in `tests.rs`; such a file would be skipped by every
/// path-based check while compiling into the production build.
///
/// # Errors
///
/// [`TestPathError::Unreadable`] or [`TestPathError::ManifestUnparseable`] when
/// the manifest cannot be read, and [`TestPathError::TestTarget`] when a
/// production target names test code.
pub fn check_manifest(manifest: &Path) -> Result<(), TestPathError> {
    let src = std::fs::read_to_string(manifest).map_err(|error| TestPathError::Unreadable {
        file: manifest.to_path_buf(),
        error,
    })?;
    let targets = parse_manifest(&src).map_err(|error| TestPathError::ManifestUnparseable {
        manifest: manifest.to_path_buf(),
        error,
    })?;
    targets.first_test_path().map_or(Ok(()), |target| {
        Err(TestPathError::TestTarget {
            manifest: manifest.to_path_buf(),
            target: target.to_owned(),
        })
    })
}

/// Why a test path's test-only premise does not hold.
#[derive(Debug)]
pub enum TestPathError {
    /// No module file beside the test module declares `mod tests;`.
    Undeclared { test_module: PathBuf },
    /// `declaring_file` reaches the test module from production code.
    Ungated {
        declaring_file: PathBuf,
        by: UngatedBy,
    },
    /// A candidate declaring file or manifest could not be read.
    Unreadable {
        file: PathBuf,
        error: std::io::Error,
    },
    /// A candidate declaring file does not lex as Rust tokens.
    Unlexable {
        file: PathBuf,
        error: proc_macro2::LexError,
    },
    /// A manifest leaves the TOML subset the target reader accepts.
    ManifestUnparseable {
        manifest: PathBuf,
        error: ManifestError,
    },
    /// A manifest names test code as a production target.
    TestTarget { manifest: PathBuf, target: String },
}

/// The production item that reaches a test module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UngatedBy {
    /// A `mod tests;` without a test-only `cfg`.
    PlainMod,
    /// A production macro invocation or definition whose tokens name `tests`.
    MacroNamingTests,
    /// Any other production item whose tokens name `tests`.
    ItemNamingTests,
}

impl fmt::Display for UngatedBy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PlainMod => "declares `mod tests;` without `#[cfg(test)]`",
            Self::MacroNamingTests => "invokes a production macro that names `tests`",
            Self::ItemNamingTests => "has a production item that names `tests`",
        })
    }
}

impl fmt::Display for TestPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undeclared { test_module } => write!(
                f,
                "{}: no sibling module file declares `#[cfg(test)] mod tests;` for this test module",
                test_module.display()
            ),
            Self::Ungated { declaring_file, by } => write!(
                f,
                "{}: {by}, so its test module may compile into production",
                declaring_file.display()
            ),
            Self::Unreadable { file, error } => {
                write!(f, "{}: cannot read ({error})", file.display())
            }
            Self::Unlexable { file, error } => {
                write!(f, "{}: could not lex ({error})", file.display())
            }
            Self::ManifestUnparseable { manifest, error } => {
                write!(
                    f,
                    "{}: cannot read build targets ({error})",
                    manifest.display()
                )
            }
            Self::TestTarget { manifest, target } => write!(
                f,
                "{}: production target `{target}` lies in test code, which path-based checks skip",
                manifest.display()
            ),
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

/// How a module file reaches its `tests` child module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Declaration {
    /// No top-level item names `tests`.
    Absent,
    /// At least one `mod tests;` under a test-only `cfg`, and no production item names `tests`.
    TestOnly,
    /// A production item declares or names `tests`.
    Ungated(UngatedBy),
}

/// Classify every top-level item of `src` that reaches a `tests` module.
///
/// Every item counts, not only the first: one test-only `mod tests;` never
/// excuses a sibling production item that also declares or names `tests`.
fn tests_declaration(src: &str) -> Result<Declaration, proc_macro2::LexError> {
    let mut declared = false;
    for item in items(TokenStream::from_str(src)?) {
        if item.attrs.iter().any(attr_is_test_cfg) {
            declared |= matches!(&item.kind, ItemKind::ModDecl(name) if name == TEST_DIR);
            continue;
        }
        let by = match item.kind {
            ItemKind::ModDecl(name) if name == TEST_DIR => Some(UngatedBy::PlainMod),
            ItemKind::MacroCall if item.names_tests => Some(UngatedBy::MacroNamingTests),
            _ if item.names_tests => Some(UngatedBy::ItemNamingTests),
            _ => None,
        };
        if let Some(by) = by {
            return Ok(Declaration::Ungated(by));
        }
    }
    Ok(if declared {
        Declaration::TestOnly
    } else {
        Declaration::Absent
    })
}

/// One top-level item: its outer attributes and what it is.
#[derive(Debug)]
struct Item {
    /// Bracket bodies of the item's outer `#[…]` attributes.
    attrs: Vec<TokenStream>,
    kind: ItemKind,
    /// Whether any token of the item, attributes included, is the identifier `tests`.
    names_tests: bool,
}

/// The shape of a top-level item, as far as test-module reachability needs.
#[derive(Debug, PartialEq, Eq)]
enum ItemKind {
    /// `[pub[(…)]] mod <name>;`, the name unraw.
    ModDecl(String),
    /// A macro invocation or `macro_rules!` definition.
    MacroCall,
    /// Anything else, inner attributes included.
    Other,
}

/// Split a file's token stream into top-level items.
///
/// An item is its outer attributes followed by body tokens up to and including
/// a top-level `;` or brace group. An inner attribute `#![…]` is an item of its
/// own with no outer attributes.
fn items(stream: TokenStream) -> Vec<Item> {
    let mut items = Vec::new();
    let mut attrs: Vec<TokenStream> = Vec::new();
    let mut body: Vec<TokenTree> = Vec::new();
    let mut toks = stream.into_iter().peekable();
    while let Some(tok) = toks.next() {
        if body.is_empty() && is_punct(&tok, '#') {
            if let Some(TokenTree::Group(g)) = toks.peek() {
                if g.delimiter() == Delimiter::Bracket {
                    attrs.push(g.stream());
                    toks.next();
                    continue;
                }
            }
            if toks.peek().is_some_and(|t| is_punct(t, '!')) {
                let bang = toks.next();
                let inner: TokenStream = [Some(tok), bang, toks.next()]
                    .into_iter()
                    .flatten()
                    .collect();
                items.push(Item {
                    attrs: Vec::new(),
                    kind: ItemKind::Other,
                    names_tests: stream_names_tests(inner),
                });
                continue;
            }
        }
        let ends_item = is_punct(&tok, ';')
            || matches!(&tok, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace);
        body.push(tok);
        if ends_item {
            items.push(finish_item(
                std::mem::take(&mut attrs),
                std::mem::take(&mut body),
            ));
        }
    }
    if !(attrs.is_empty() && body.is_empty()) {
        items.push(finish_item(attrs, body));
    }
    items
}

/// Build an [`Item`] from its outer attributes and body tokens.
fn finish_item(attrs: Vec<TokenStream>, body: Vec<TokenTree>) -> Item {
    let names_tests =
        attrs.iter().cloned().any(stream_names_tests) || body.iter().cloned().any(tree_names_tests);
    Item {
        kind: item_kind(&body),
        attrs,
        names_tests,
    }
}

/// Classify an item body.
fn item_kind(body: &[TokenTree]) -> ItemKind {
    let rest = strip_visibility(body);
    if let [
        TokenTree::Ident(kw),
        TokenTree::Ident(name),
        TokenTree::Punct(semi),
    ] = rest
    {
        if kw == "mod" && semi.as_char() == ';' {
            return ItemKind::ModDecl(unraw(name));
        }
    }
    let invokes_macro = rest
        .windows(2)
        .any(|pair| matches!(pair, [TokenTree::Ident(_), TokenTree::Punct(bang)] if bang.as_char() == '!'));
    if invokes_macro {
        ItemKind::MacroCall
    } else {
        ItemKind::Other
    }
}

/// Drop a leading `pub` or `pub(…)` from an item body.
fn strip_visibility(body: &[TokenTree]) -> &[TokenTree] {
    match body {
        [TokenTree::Ident(vis), TokenTree::Group(g), rest @ ..]
            if vis == "pub" && g.delimiter() == Delimiter::Parenthesis =>
        {
            rest
        }
        [TokenTree::Ident(vis), rest @ ..] if vis == "pub" => rest,
        _ => body,
    }
}

/// Whether attribute body `attr` is `cfg(P)` with a test-only predicate `P`.
fn attr_is_test_cfg(attr: &TokenStream) -> bool {
    let toks: Vec<TokenTree> = attr.clone().into_iter().collect();
    matches!(
        toks.as_slice(),
        [TokenTree::Ident(id), TokenTree::Group(g)]
            if id == "cfg" && g.delimiter() == Delimiter::Parenthesis && cfg_pred_is_test_only(&g.stream())
    )
}

/// Whether `tok` is the punctuation character `ch`.
fn is_punct(tok: &TokenTree, ch: char) -> bool {
    matches!(tok, TokenTree::Punct(p) if p.as_char() == ch)
}

/// An identifier's name without a raw `r#` prefix.
fn unraw(id: &Ident) -> String {
    let name = id.to_string();
    name.strip_prefix("r#")
        .map_or_else(|| name.clone(), str::to_owned)
}

/// Whether any token in `stream`, at any depth, is the identifier `tests`.
fn stream_names_tests(stream: TokenStream) -> bool {
    stream.into_iter().any(tree_names_tests)
}

/// Whether token `tok`, or any token nested in it, is the identifier `tests`.
fn tree_names_tests(tok: TokenTree) -> bool {
    match tok {
        TokenTree::Ident(id) => unraw(&id) == TEST_DIR,
        TokenTree::Group(g) => stream_names_tests(g.stream()),
        TokenTree::Punct(_) | TokenTree::Literal(_) => false,
    }
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
            "#[cfg(test)]\nmod r#tests;",
            "#[cfg(test)]\nmod tests;\n#[cfg(test)]\nuse tests::helper;",
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
            "pub mod r#tests;",
            "#[cfg(any(test, feature = \"x\"))]\nmod tests;",
            "#[cfg(not(test))]\nmod tests;",
            "#[cfg(test)]\nuse x;\nmod tests;",
            "#[cfg(test)]\nfn helper() {}\nmod tests;",
            "#![cfg(test)]\nmod tests;",
            "#[test]\nmod tests;",
        ] {
            assert_eq!(
                tests_declaration(src).ok(),
                Some(Declaration::Ungated(UngatedBy::PlainMod)),
                "{src:?}"
            );
        }
    }

    #[test]
    fn every_declaration_is_checked_not_only_the_first() {
        for src in [
            "#[cfg(test)]\nmod tests;\n#[cfg(not(test))]\npub mod tests;",
            "#[cfg(test)]\nmod tests;\nmod tests;",
            "#[cfg(test)]\nmod tests;\n#[cfg(feature = \"x\")]\npub mod tests;",
        ] {
            assert_eq!(
                tests_declaration(src).ok(),
                Some(Declaration::Ungated(UngatedBy::PlainMod)),
                "{src:?}"
            );
        }
    }

    #[test]
    fn a_production_item_naming_tests_is_ungated() {
        for (src, by) in [
            (
                "#[cfg(test)]\nmod tests;\n#[cfg(not(test))]\ndecl!(tests);",
                UngatedBy::MacroNamingTests,
            ),
            ("decl! { tests }", UngatedBy::MacroNamingTests),
            (
                "macro_rules! m { () => { pub mod tests; } }",
                UngatedBy::MacroNamingTests,
            ),
            ("pub use tests::*;", UngatedBy::ItemNamingTests),
            ("mod tests {\n    mod inner;\n}", UngatedBy::ItemNamingTests),
            ("fn f() { mod tests; }", UngatedBy::ItemNamingTests),
            (
                "#[cfg_attr(not(test), path = \"x.rs\")]\nmod other { use super::tests; }",
                UngatedBy::ItemNamingTests,
            ),
            (
                "#![doc = \"x\"]\n#![cfg_attr(tests, x)]",
                UngatedBy::ItemNamingTests,
            ),
        ] {
            assert_eq!(
                tests_declaration(src).ok(),
                Some(Declaration::Ungated(by)),
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
            "/// The tests live elsewhere.\nfn f() {}",
            "const S: &str = \"tests\";",
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

    /// A fresh directory under the system temp dir holding `files`.
    fn scratch_crate(name: &str, files: &[(&str, &[u8])]) -> std::io::Result<PathBuf> {
        let root = std::env::temp_dir().join(format!(
            "panic-scan-test-path-{}-{name}",
            std::process::id()
        ));
        if root.exists() {
            std::fs::remove_dir_all(&root)?;
        }
        for (rel, contents) in files {
            let path = root.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, contents)?;
        }
        Ok(root)
    }

    #[test]
    fn an_unreadable_declaring_file_fails_closed() -> std::io::Result<()> {
        let root = scratch_crate("unreadable", &[("lib.rs", b"\xff\xfe mod tests;")])?;
        let result = check_test_path(&root, Path::new("tests/a.rs"));
        assert!(
            matches!(result, Err(TestPathError::Unreadable { .. })),
            "{result:?}"
        );
        std::fs::remove_dir_all(root)
    }

    #[test]
    fn an_unlexable_declaring_file_fails_closed() -> std::io::Result<()> {
        let root = scratch_crate(
            "unlexable",
            &[("lib.rs", b"#[cfg(test)]\nmod tests;\n\"open")],
        )?;
        let result = check_test_path(&root, Path::new("tests/a.rs"));
        assert!(
            matches!(result, Err(TestPathError::Unlexable { .. })),
            "{result:?}"
        );
        std::fs::remove_dir_all(root)
    }

    #[test]
    fn a_shadowing_second_declaration_fails_closed_on_disk() -> std::io::Result<()> {
        let root = scratch_crate(
            "shadowed",
            &[(
                "src/lib.rs",
                b"#[cfg(test)]\nmod tests;\n#[cfg(not(test))]\npub mod tests;\n",
            )],
        )?;
        let result = check_test_path(&root, Path::new("src/tests/mod.rs"));
        assert!(
            matches!(
                result,
                Err(TestPathError::Ungated {
                    by: UngatedBy::PlainMod,
                    ..
                })
            ),
            "{result:?}"
        );
        std::fs::remove_dir_all(root)
    }

    #[test]
    fn a_plain_crate_manifest_proves_its_integration_tests() -> std::io::Result<()> {
        let root = scratch_crate(
            "manifest-ok",
            &[
                (
                    "Cargo.toml",
                    b"[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
                ),
                ("src/lib.rs", b"pub fn f() {}\n"),
            ],
        )?;
        let result = check_test_path(&root, Path::new("tests/cli.rs"));
        assert!(result.is_ok(), "{result:?}");
        std::fs::remove_dir_all(root)
    }

    #[test]
    fn a_manifest_target_under_tests_fails_closed() -> std::io::Result<()> {
        for (name, manifest) in [
            ("manifest-lib", &b"[lib]\npath = \"tests/lib.rs\"\n"[..]),
            (
                "manifest-bin",
                b"[[bin]]\nname = \"x\"\npath = \"tests/main.rs\"\n",
            ),
            (
                "manifest-build",
                b"[package]\nname = \"x\"\nbuild = \"tests/build.rs\"\n",
            ),
        ] {
            let root = scratch_crate(name, &[("Cargo.toml", manifest)])?;
            let result = check_test_path(&root, Path::new("tests/lib.rs"));
            assert!(
                matches!(result, Err(TestPathError::TestTarget { .. })),
                "{name}: {result:?}"
            );
            std::fs::remove_dir_all(root)?;
        }
        Ok(())
    }

    #[test]
    fn an_unparseable_manifest_fails_closed() -> std::io::Result<()> {
        let root = scratch_crate("manifest-bad", &[("Cargo.toml", b"[lib\npath = 1\n")])?;
        let result = check_test_path(&root, Path::new("tests/a.rs"));
        assert!(
            matches!(result, Err(TestPathError::ManifestUnparseable { .. })),
            "{result:?}"
        );
        std::fs::remove_dir_all(root)
    }

    #[test]
    fn a_manifest_does_not_excuse_a_production_declaration() -> std::io::Result<()> {
        let root = scratch_crate(
            "manifest-ungated",
            &[
                (
                    "Cargo.toml",
                    b"[package]\nname = \"x\"\n\n[lib]\npath = \"lib.rs\"\n",
                ),
                ("lib.rs", b"pub mod tests;\n"),
            ],
        )?;
        let result = check_test_path(&root, Path::new("tests/mod.rs"));
        assert!(
            matches!(
                result,
                Err(TestPathError::Ungated {
                    by: UngatedBy::PlainMod,
                    ..
                })
            ),
            "{result:?}"
        );
        std::fs::remove_dir_all(root)
    }
}

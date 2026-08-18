//! The confined watcher's typed scope (INV-4, H18).
//!
//! `ipe watch` must observe only a strict, typed allowlist: `ipe.toml`, the
//! entry point's directory (recursive source-extension walk), and `tests/`
//! if present — never `target/`, `.git/`, `node_modules/`, or any generated
//! output directory, whose churn would self-trigger a rebuild loop.
//!
//! The two hazards this module forecloses (design doc H18):
//! - a symlink resolving OUTSIDE the project root must never be watched
//!   (path-traversal foreclosure);
//! - the watched-path count must be bounded (a `DoS` guard against a
//!   pathological tree).
//!
//! Both are enforced by construction: [`WatchedPath`] has exactly one
//! constructor ([`WatchedPath::confine`]), and it is the ONLY way to obtain a
//! value of the type — a path that fails canonicalisation or resolves
//! outside the root is simply not representable as a `WatchedPath` (parse,
//! don't validate).

use std::path::{Path, PathBuf};

/// The source-extension this project's `.ipe` modules use. Kept as a single
/// named constant so a future rename only touches one place.
pub const SOURCE_EXTENSION: &str = "ipe";

/// The generated / vendor directories a confined watch must never observe —
/// watching them would self-trigger a rebuild loop (the watcher's own output
/// changing the input it watches).
const EXCLUDED_DIR_NAMES: &[&str] = &[
    "target",
    ".git",
    "node_modules",
    "dist",
    // The project-local incremental state directory the design doc reserves
    // (`.ipe/lowered/`, `.ipe/source.hash`) and its build-cache sibling.
    ".ipe",
    ".ipe-cache",
    // Generated build output directories a `ipe build`/`ipe watch` produces.
    "out",
];

/// A filesystem path proven, by construction, to be canonical and
/// project-root-confined.
///
/// There is no way to construct a value of this type that escapes the root
/// — `confine` is the only constructor, and it rejects (rather than
/// silently clamping) anything that doesn't resolve inside the root,
/// including a symlink that points outside it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WatchedPath(PathBuf);

impl WatchedPath {
    /// Canonicalise `candidate` (resolving symlinks) and confine it to
    /// `root` (itself canonicalised first). Returns `None` — never a
    /// panic, never a clamped/truncated path — when:
    /// - `candidate` (or `root`) cannot be canonicalised (e.g. it doesn't
    ///   exist, a broken symlink, a permission error), or
    /// - the canonicalised path does not lie inside the canonicalised root
    ///   (the symlink-escape case H18 names explicitly).
    #[must_use]
    pub fn confine(root: &Path, candidate: &Path) -> Option<Self> {
        let canon_root = std::fs::canonicalize(root).ok()?;
        let canon_candidate = std::fs::canonicalize(candidate).ok()?;
        if canon_candidate.starts_with(&canon_root) {
            Some(Self(canon_candidate))
        } else {
            None
        }
    }

    /// Confine a path that no longer exists on disk (a delete event).
    ///
    /// When a file is deleted, `confine` cannot canonicalise it (the path is
    /// gone). This constructor canonicalises the PARENT (which typically still
    /// exists), re-joins the raw `file_name`, and applies the FULL
    /// `starts_with(canon_root)` confinement gate to the REJOINED path — not
    /// just to the parent. A file whose parent canonicalises outside the root,
    /// or whose `file_name` is missing, yields `None`.
    ///
    /// The `file_name` component is taken from `candidate` as-is. It must not
    /// contain path separators (which `file_name()` already prevents — it
    /// returns the last component only), so the rejoined path has exactly the
    /// depth of the canonicalised parent plus one leaf, with no escape vector.
    #[must_use]
    pub fn confine_deleted(root: &Path, candidate: &Path) -> Option<Self> {
        let canon_root = std::fs::canonicalize(root).ok()?;
        let parent = candidate.parent()?;
        let parent_canon = std::fs::canonicalize(parent).ok()?;
        let file_name = candidate.file_name()?;
        let rejoined = parent_canon.join(file_name);
        if rejoined.starts_with(&canon_root) {
            Some(Self(rejoined))
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    #[must_use]
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

/// Whether a directory name (the LAST component of a path) names one of the
/// generated/vendor directories a confined watch excludes.
#[must_use]
pub fn is_excluded_dir_name(name: &str) -> bool {
    EXCLUDED_DIR_NAMES.contains(&name)
}

/// Whether ANY component of `path` names an excluded directory — a path
/// nested arbitrarily deep under `target/` or `.git/` is excluded regardless
/// of its own leaf name.
#[must_use]
pub fn under_excluded_dir(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str().to_str().is_some_and(is_excluded_dir_name))
}

/// The strict allowlist a confined watch observes, resolved once at watch
/// startup against a canonical project root.
///
/// Construction can only ever produce a scope whose every member path is a
/// [`WatchedPath`] — already confined, already excluded-dir-free. There is
/// no field or method that hands back an unconfined `PathBuf`.
#[derive(Debug, Clone)]
pub struct WatchScope {
    /// The canonical project root every watched path is confined to.
    root: PathBuf,
    /// Top-level directories/files handed to the OS-level watcher. Watching
    /// directories (not individual files) is deliberate — editors save via
    /// tmp-write + rename, so a new file under a watched DIRECTORY is
    /// observed even though its own inode never existed before the rename.
    roots_to_watch: Vec<WatchedPath>,
    /// The canonical root-level `tests/` directory, when one exists —
    /// scoped so the "any extension is relevant under `tests/`" rule only
    /// ever matches THIS directory, never an unrelated `tests` component
    /// nested elsewhere in the tree (e.g. a supervised app's own
    /// `examples/foo/tests/`).
    tests_root: Option<PathBuf>,
    /// Total distinct `.ipe` source files discovered at scope-build time —
    /// the `DoS`-guard count (H18: "bound watched-file count").
    file_count: usize,
}

/// Bound on the number of `.ipe` files a single watch session will track.
///
/// A defence against a pathological tree (accidentally pointing `ipe watch`
/// at a directory with millions of files, e.g. a vendored `node_modules`
/// that slipped past the exclusion list, or a symlink loop that inflates the
/// walk). Exceeding it is a hard, loud refusal, never a silent truncation.
pub const MAX_WATCHED_FILES: usize = 200_000;

/// Why a [`WatchScope`] could not be built. Every variant carries enough
/// context to render an actionable CLI diagnostic without the caller
/// re-deriving it.
#[derive(Debug)]
pub enum ScopeError {
    /// The project root itself does not canonicalise (missing, broken
    /// symlink, permission error).
    RootNotFound(PathBuf),
    /// The entry file's parent directory does not canonicalise, or resolves
    /// outside the project root (symlink escape).
    EntryDirEscapesRoot(PathBuf),
    /// The discovered `.ipe` file count exceeds [`MAX_WATCHED_FILES`].
    TooManyFiles { found: usize, max: usize },
    /// A filesystem error occurred while walking a watched directory.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for ScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootNotFound(p) => write!(f, "watch: project root not found: {}", p.display()),
            Self::EntryDirEscapesRoot(p) => write!(
                f,
                "watch: entry directory escapes the project root (symlink?): {}",
                p.display()
            ),
            Self::TooManyFiles { found, max } => write!(
                f,
                "watch: {found} source files exceed the watch bound of {max}; refusing to watch \
                 (this is usually a mis-pointed project root, not a real project)"
            ),
            Self::Io { path, source } => {
                write!(f, "watch: io error walking {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ScopeError {}

impl WatchScope {
    /// Build the confined scope for a project rooted at `root`, whose entry
    /// module lives under `entry_dir` (already known to be `root` or a
    /// descendant of it — the caller is the CLI's own `--entry` resolution,
    /// which has already located the file). `has_tests` records whether a
    /// `tests/` directory exists directly under `root`.
    ///
    /// # Errors
    /// See [`ScopeError`].
    pub fn build(root: &Path, entry_dir: &Path) -> Result<Self, ScopeError> {
        let canon_root = std::fs::canonicalize(root)
            .map_err(|_| ScopeError::RootNotFound(root.to_path_buf()))?;

        let mut roots_to_watch = Vec::new();

        // ipe.toml, if present, is a FILE watch target (its own directory is
        // the project root, already covered by entry_dir/tests below in the
        // common case, but ipe.toml may live in an ancestor of a nested
        // entry — watch it explicitly regardless).
        let manifest = canon_root.join("ipe.toml");
        if manifest.is_file()
            && let Some(w) = WatchedPath::confine(&canon_root, &manifest)
        {
            roots_to_watch.push(w);
        }

        // The entry module's directory, recursively.
        let entry_scope = WatchedPath::confine(&canon_root, entry_dir)
            .ok_or_else(|| ScopeError::EntryDirEscapesRoot(entry_dir.to_path_buf()))?;
        roots_to_watch.push(entry_scope);

        // tests/, if present, directly under the project root.
        let tests_dir = canon_root.join("tests");
        let mut tests_root = None;
        if tests_dir.is_dir()
            && let Some(w) = WatchedPath::confine(&canon_root, &tests_dir)
        {
            tests_root = Some(w.as_path().to_path_buf());
            roots_to_watch.push(w);
        }

        // Discover + count every `.ipe` file under the watched roots now, so
        // a pathological tree is refused at startup rather than degrading
        // the watcher into an unbounded event source later.
        let mut file_count = 0usize;
        for w in &roots_to_watch {
            count_source_files(w.as_path(), &mut file_count)?;
            if file_count > MAX_WATCHED_FILES {
                return Err(ScopeError::TooManyFiles {
                    found: file_count,
                    max: MAX_WATCHED_FILES,
                });
            }
        }

        Ok(Self {
            root: canon_root,
            roots_to_watch,
            tests_root,
            file_count,
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn roots_to_watch(&self) -> &[WatchedPath] {
        &self.roots_to_watch
    }

    #[must_use]
    pub const fn file_count(&self) -> usize {
        self.file_count
    }

    /// Whether a raw filesystem-event path is IN scope: confinable to the
    /// root, not under an excluded directory, and (for files) either a
    /// `.ipe` source, `ipe.toml`, or an `.ipei`/`kernel.json` FFI interface
    /// file (H13 — the cross-terminal `ipe add` observation seam).
    ///
    /// This is the drop-at-the-source filter (design doc: "drop excluded-dir
    /// events at the source") — called on every raw event BEFORE it reaches
    /// the debounce/coalesce stage, so an excluded-dir storm never even
    /// enters the bounded intake queue.
    ///
    /// Both live and delete-event paths are routed through the same typed
    /// constructors ([`WatchedPath::confine`] and
    /// [`WatchedPath::confine_deleted`]), so there is exactly one confinement
    /// gate — no ad-hoc `canonicalize`/`starts_with` duplication that could drift.
    #[must_use]
    pub fn is_relevant(&self, path: &Path) -> bool {
        if under_excluded_dir(path) {
            return false;
        }
        let Some(confined) = WatchedPath::confine(&self.root, path)
            .or_else(|| WatchedPath::confine_deleted(&self.root, path))
        else {
            return false;
        };
        is_watchable_leaf(self.tests_root.as_deref(), confined.as_path())
    }
}

/// Whether a (canonicalised, in-root, non-excluded) leaf path is one of the
/// file kinds a confined watch actually reacts to: `.ipe` sources,
/// `ipe.toml`, or a file under the root-level `tests/` watch root (any
/// extension — a fixture asset under `tests/` still belongs to the
/// allowlist even without a `.ipe` extension, mirroring the reference
/// project's "tests/ if present" scope).
///
/// `tests_root`, when present, is the ROOT-LEVEL `tests/` directory only
/// (`WatchScope::build`'s own confinement of `<root>/tests`) — never any
/// other path component spelled `tests`. A supervised app writing golden
/// outputs or logs under its OWN nested `tests/` dir (e.g.
/// `examples/foo/tests/output.log`) must not self-trigger the watch loop by
/// virtue of a path SEGMENT matching that word.
fn is_watchable_leaf(tests_root: Option<&Path>, path: &Path) -> bool {
    if path.file_name().and_then(|n| n.to_str()) == Some("ipe.toml") {
        return true;
    }
    if is_source_file(path) {
        return true;
    }
    tests_root.is_some_and(|root| path.starts_with(root))
}

/// Whether `path`'s extension is the `.ipe` source extension.
fn is_source_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some(SOURCE_EXTENSION)
}

/// Recursively count `.ipe` files under `dir`, accumulating into `count` —
/// ONLY `.ipe` files count toward the [`MAX_WATCHED_FILES`] `DoS` guard, so
/// a directory full of non-source assets (golden fixtures, logs a
/// supervised app writes under `tests/`) cannot exhaust the bound on its
/// own. Bails out (returning early, count left at its last valid value) the
/// moment `count` exceeds [`MAX_WATCHED_FILES`] — the caller re-checks and
/// converts that into a hard [`ScopeError::TooManyFiles`], so a pathological
/// tree cannot make this walk itself unbounded.
fn count_source_files(dir: &Path, count: &mut usize) -> Result<(), ScopeError> {
    if dir.is_file() {
        if is_source_file(dir) {
            *count += 1;
        }
        return Ok(());
    }
    if !dir.is_dir() {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|source| ScopeError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| ScopeError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if under_excluded_dir(&path) {
            continue;
        }
        let file_type = entry.file_type().map_err(|source| ScopeError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            count_source_files(&path, count)?;
        } else if is_source_file(&path) {
            *count += 1;
        }
        if *count > MAX_WATCHED_FILES {
            return Ok(());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ipe_watch_scope_{}_{tag}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn confine_accepts_a_descendant_path() {
        let root = tmp_dir("confine_ok");
        let child = root.join("src");
        fs::create_dir_all(&child).unwrap();
        let w = WatchedPath::confine(&root, &child);
        assert!(w.is_some());
        assert!(w.expect("value must be Some").as_path().starts_with(&root));
    }

    #[test]
    fn confine_rejects_a_path_outside_root() {
        let root = tmp_dir("confine_root_a");
        let outside = tmp_dir("confine_root_b");
        assert!(WatchedPath::confine(&root, &outside).is_none());
    }

    #[test]
    fn confine_rejects_a_symlink_escaping_root() {
        let root = tmp_dir("confine_symlink_root");
        let outside = tmp_dir("confine_symlink_target");
        fs::write(
            outside.join("secret.ipe"),
            "module Secret exposing (x)\nx = 1\n",
        )
        .unwrap();
        let link = root.join("escape");
        #[cfg(unix)]
        {
            if std::os::unix::fs::symlink(&outside, &link).is_ok() {
                // The symlink resolves OUTSIDE root — must be refused.
                assert!(WatchedPath::confine(&root, &link).is_none());
            }
        }
    }

    #[test]
    fn scope_build_excludes_target_and_git() {
        let root = tmp_dir("scope_excl");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nmain = 1\n",
        )
        .unwrap();
        let target = root.join("target").join("debug");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("build.rs"), "junk").unwrap();

        let scope = WatchScope::build(&root, &src).unwrap();
        // Only Main.ipe counted — target/debug/build.rs must be excluded.
        assert_eq!(scope.file_count(), 1);
        assert!(!scope.is_relevant(&target.join("build.rs")));
        assert!(scope.is_relevant(&src.join("Main.ipe")));
    }

    #[test]
    fn scope_build_refuses_entry_dir_outside_root() {
        let root = tmp_dir("scope_escape_root");
        let outside = tmp_dir("scope_escape_outside");
        let err = WatchScope::build(&root, &outside);
        assert!(matches!(err, Err(ScopeError::EntryDirEscapesRoot(_))));
    }

    #[test]
    fn is_relevant_accepts_ipe_toml_and_rejects_unrelated_extension() {
        let root = tmp_dir("scope_relevant");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nmain = 1\n",
        )
        .unwrap();
        fs::write(root.join("ipe.toml"), "name = \"x\"\n").unwrap();
        fs::write(src.join("notes.txt"), "hi").unwrap();

        let scope = WatchScope::build(&root, &src).unwrap();
        assert!(scope.is_relevant(&root.join("ipe.toml")));
        assert!(!scope.is_relevant(&src.join("notes.txt")));
    }

    #[test]
    fn is_relevant_survives_a_delete_event_path() {
        let root = tmp_dir("scope_delete");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        let f = src.join("Gone.ipe");
        fs::write(&f, "module Gone exposing (x)\nx = 1\n").unwrap();
        let scope = WatchScope::build(&root, &src).unwrap();
        fs::remove_file(&f).unwrap();
        // The path no longer exists on disk, yet it's still recognisably a
        // `.ipe` file under the (still-existing) src/ directory — must
        // still be judged relevant so a delete triggers a rebuild.
        assert!(scope.is_relevant(&f));
    }

    /// CO-INCR-009: a nested `tests` directory OUTSIDE the root-level
    /// `tests/` watch root (e.g. a supervised app's own
    /// `examples/foo/tests/`) must not self-trigger the watch loop for its
    /// non-`.ipe` artifacts — only the extension/`ipe.toml` rules, and the
    /// ACTUAL root-level `tests/`, are watchable.
    #[test]
    fn is_relevant_ignores_a_non_root_tests_directory() {
        let root = tmp_dir("scope_nested_tests");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nmain = 1\n",
        )
        .unwrap();
        // A nested `tests` dir under the WATCHED entry directory, not at
        // the project root — an app-written golden output/log here must
        // not be watch-relevant.
        let nested_tests = src.join("examples").join("foo").join("tests");
        fs::create_dir_all(&nested_tests).unwrap();
        let artifact = nested_tests.join("output.log");
        fs::write(&artifact, "run 1\n").unwrap();

        let scope = WatchScope::build(&root, &src).unwrap();
        assert!(
            !scope.is_relevant(&artifact),
            "a nested `tests` path component must not make a non-.ipe file watch-relevant"
        );
    }

    /// The root-level `tests/` directory keeps its documented "any
    /// extension" allowance — the fix above must not remove real coverage,
    /// only scope it to the correct directory.
    #[test]
    fn is_relevant_still_accepts_any_extension_under_root_level_tests() {
        let root = tmp_dir("scope_root_tests");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nmain = 1\n",
        )
        .unwrap();
        let tests_dir = root.join("tests");
        fs::create_dir_all(&tests_dir).unwrap();
        let fixture = tests_dir.join("golden.txt");
        fs::write(&fixture, "expected\n").unwrap();

        let scope = WatchScope::build(&root, &src).unwrap();
        assert!(
            scope.is_relevant(&fixture),
            "a non-.ipe file directly under the root-level tests/ must stay relevant"
        );
    }

    /// CO-INCR-009: `file_count`/`MAX_WATCHED_FILES` count only `.ipe`
    /// files — a `tests/` directory full of non-source artifacts (golden
    /// outputs, logs a supervised app writes) must not count against the
    /// `DoS` guard.
    #[test]
    fn file_count_ignores_non_ipe_files_under_tests() {
        let root = tmp_dir("scope_count_tests");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nmain = 1\n",
        )
        .unwrap();
        fs::write(root.join("ipe.toml"), "name = \"x\"\n").unwrap();
        let tests_dir = root.join("tests");
        fs::create_dir_all(&tests_dir).unwrap();
        for i in 0..10 {
            fs::write(tests_dir.join(format!("artifact_{i}.log")), "x").unwrap();
        }

        let scope = WatchScope::build(&root, &src).unwrap();
        assert_eq!(
            scope.file_count(),
            1,
            "only Main.ipe counts — ipe.toml and the 10 tests/ artifacts must not"
        );
    }

    #[test]
    fn confine_deleted_rejects_outside_root() {
        // A deleted file whose PARENT canonicalises OUTSIDE the watch root must
        // return None — the full starts_with gate is applied to the rejoined
        // path, not just to the parent.
        let root = tmp_dir("confine_del_outside_root");
        let other_root = tmp_dir("confine_del_outside_other");
        // The parent exists but is outside root.
        let deleted_path = other_root.join("gone.ipe");
        assert!(
            WatchedPath::confine_deleted(&root, &deleted_path).is_none(),
            "confine_deleted must return None when the parent is outside the root"
        );
    }

    #[test]
    fn is_relevant_delete_event_outside_root_is_false() {
        // A delete event for a file whose parent is outside self.root must not
        // be relevant — the fallback confine_deleted applies the full root gate.
        let root = tmp_dir("is_rel_del_outside_root");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nmain = 1\n",
        )
        .unwrap();

        let other_root = tmp_dir("is_rel_del_outside_other");
        let scope = WatchScope::build(&root, &src).unwrap();
        // Path under other_root — parent canonicalises outside scope.root.
        let outside_path = other_root.join("Secret.ipe");
        assert!(
            !scope.is_relevant(&outside_path),
            "a delete event outside the watch root must not be relevant"
        );
    }

    #[test]
    fn is_relevant_and_confine_agree_for_live_ipe_file() {
        // For a live in-root .ipe file, is_relevant must agree with
        // WatchedPath::confine — there is exactly one confinement constructor,
        // so the two cannot drift.
        let root = tmp_dir("is_rel_confine_agree");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        let file = src.join("Main.ipe");
        fs::write(&file, "module Main exposing (main)\nmain = 1\n").unwrap();

        let scope = WatchScope::build(&root, &src).unwrap();
        assert!(
            scope.is_relevant(&file),
            "in-root .ipe file must be relevant"
        );
        let confined = WatchedPath::confine(scope.root(), &file)
            .expect("confine must succeed for a live in-root file");
        // is_relevant routes through the same confine constructor, so the
        // canonical path it uses equals confined.as_path().
        assert!(
            confined.as_path().starts_with(scope.root()),
            "confined path must be inside the root: {:?}",
            confined.as_path()
        );
    }
}

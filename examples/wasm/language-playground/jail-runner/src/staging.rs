//! What a staged project may contain, and the trusted files the harness adds.
//!
//! The server stages only the client's emitted crate (`Cargo.toml` plus
//! `src/`). Everything else the jailed build reads — the runtime crate, the
//! lockfile, the vendored crate sources, the prebuilt dependency artifacts — comes from the
//! harness itself, so [`check_project_layout`] refuses a project that already
//! holds anything beyond those two entries.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use ipe_wasm::RUNTIME_DEP_DIR;

/// The program prewarm compiles and builds to fill the warm cache.
pub const PREWARM_PROGRAM: &str = "module Main exposing (main)\n\nimport Ipe.Io as Io\n\nmain : Task Error ()\nmain =\n    Io.println \"hello\"\n";

/// Deepest directory nesting walked in a staged project or a warm tree.
pub const MAX_TREE_DEPTH: usize = 32;

/// Seconds after the Unix epoch stamped as the modification time of every
/// materialised runtime file.
///
/// A fixed stamp makes the runtime written for one request, to cargo's
/// freshness check, the same source prewarm compiled, so the warm artifacts
/// stay fresh and only the user crate is rebuilt.
pub const RUNTIME_SOURCE_MTIME_SECS: u64 = 1_000_000_000;

const MANIFEST: &str = "Cargo.toml";
const SOURCE_DIR: &str = "src";
const ENTRY_POINT: &str = "main.rs";

/// The runtime crate source, as crate-relative path to file text.
pub type RuntimeFiles = BTreeMap<PathBuf, String>;

/// Why a staged project was refused before any build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    /// A top-level entry other than `Cargo.toml` and `src`.
    UnexpectedEntry(PathBuf),
    /// `Cargo.toml` is absent or not a regular file.
    MissingManifest,
    /// `src/main.rs` is absent or not a regular file.
    MissingEntryPoint,
    /// A symbolic link, or an entry that is neither a file nor a directory.
    NotPlain(PathBuf),
    /// Directories nested deeper than [`MAX_TREE_DEPTH`].
    TooDeep(PathBuf),
    /// The project tree could not be read.
    Unreadable {
        /// The entry that failed.
        path: PathBuf,
        /// The underlying error.
        detail: String,
    },
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedEntry(path) => write!(
                f,
                "staged project holds `{}`; only `{MANIFEST}` and `{SOURCE_DIR}/` are accepted",
                path.display()
            ),
            Self::MissingManifest => write!(f, "staged project has no regular `{MANIFEST}`"),
            Self::MissingEntryPoint => {
                write!(
                    f,
                    "staged project has no regular `{SOURCE_DIR}/{ENTRY_POINT}`"
                )
            }
            Self::NotPlain(path) => write!(
                f,
                "staged project entry `{}` is not a plain file or directory",
                path.display()
            ),
            Self::TooDeep(path) => write!(
                f,
                "staged project nests deeper than {MAX_TREE_DEPTH} directories at `{}`",
                path.display()
            ),
            Self::Unreadable { path, detail } => {
                write!(
                    f,
                    "cannot read staged project entry `{}`: {detail}",
                    path.display()
                )
            }
        }
    }
}

/// Accept a staged project only when it is exactly the client's emitted crate.
///
/// The top level holds a regular `Cargo.toml` and a `src/` directory and
/// nothing else; `src/` holds only regular files and directories, at most
/// [`MAX_TREE_DEPTH`] deep, including a regular `src/main.rs`. Symbolic links
/// are refused everywhere.
///
/// # Errors
///
/// The [`LayoutError`] naming the first offending entry.
pub fn check_project_layout(project: &Path) -> Result<(), LayoutError> {
    let mut manifest = false;
    for entry in read_entries(project)? {
        let (path, kind) = entry;
        let name = path.file_name().map(PathBuf::from).unwrap_or_default();
        match (name.to_str(), kind) {
            (Some(MANIFEST), EntryKind::File) => manifest = true,
            (Some(SOURCE_DIR), EntryKind::Dir) => check_plain_tree(&path, 1)?,
            (Some(MANIFEST | SOURCE_DIR), EntryKind::Other) => {
                return Err(LayoutError::NotPlain(name));
            }
            _ => return Err(LayoutError::UnexpectedEntry(name)),
        }
    }
    if !manifest {
        return Err(LayoutError::MissingManifest);
    }
    let entry_point = project.join(SOURCE_DIR).join(ENTRY_POINT);
    let is_regular = std::fs::symlink_metadata(entry_point).is_ok_and(|meta| meta.is_file());
    if !is_regular {
        return Err(LayoutError::MissingEntryPoint);
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Dir,
    Other,
}

/// The entries of `dir`, classified without following symbolic links.
fn read_entries(dir: &Path) -> Result<Vec<(PathBuf, EntryKind)>, LayoutError> {
    let unreadable = |path: &Path, error: &std::io::Error| LayoutError::Unreadable {
        path: path.to_path_buf(),
        detail: error.to_string(),
    };
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| unreadable(dir, &e))? {
        let entry = entry.map_err(|e| unreadable(dir, &e))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| unreadable(&path, &e))?;
        let kind = if file_type.is_file() {
            EntryKind::File
        } else if file_type.is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::Other
        };
        out.push((path, kind));
    }
    Ok(out)
}

fn check_plain_tree(dir: &Path, depth: usize) -> Result<(), LayoutError> {
    if depth > MAX_TREE_DEPTH {
        return Err(LayoutError::TooDeep(dir.to_path_buf()));
    }
    for (path, kind) in read_entries(dir)? {
        match kind {
            EntryKind::File => {}
            EntryKind::Dir => check_plain_tree(&path, depth.saturating_add(1))?,
            EntryKind::Other => return Err(LayoutError::NotPlain(path)),
        }
    }
    Ok(())
}

/// Why trusted files could not be written into a project.
#[derive(Debug)]
pub enum StageError {
    /// A file path that is empty, absolute, or climbs out of its root.
    UnsafePath(PathBuf),
    /// A filesystem operation failed.
    Io {
        /// The path being written.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
}

impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsafePath(path) => {
                write!(
                    f,
                    "refusing to stage file at unsafe path `{}`",
                    path.display()
                )
            }
            Self::Io { path, source } => write!(f, "cannot write `{}`: {source}", path.display()),
        }
    }
}

/// Write the runtime crate into `project`'s [`RUNTIME_DEP_DIR`], which must not
/// exist yet.
///
/// Every file carries the fixed [`RUNTIME_SOURCE_MTIME_SECS`] stamp.
///
/// # Errors
///
/// [`StageError`] when the directory already exists, a path is unsafe, or a
/// write fails.
pub fn write_runtime(project: &Path, runtime: &RuntimeFiles) -> Result<(), StageError> {
    let root = project.join(RUNTIME_DEP_DIR);
    std::fs::create_dir(&root).map_err(|source| StageError::Io {
        path: root.clone(),
        source,
    })?;
    let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(RUNTIME_SOURCE_MTIME_SECS);
    for (rel, text) in runtime {
        write_new_file(&root, rel, text, Some(stamp))?;
    }
    Ok(())
}

/// Write each emitted file under `root`.
///
/// # Errors
///
/// [`StageError`] when a path is unsafe, a file already exists, or a write
/// fails.
pub fn write_emitted(root: &Path, files: &BTreeMap<String, String>) -> Result<(), StageError> {
    for (rel, text) in files {
        write_new_file(root, Path::new(rel), text, None)?;
    }
    Ok(())
}

/// Create `root/rel` (never overwriting) holding `text`, optionally stamped.
fn write_new_file(
    root: &Path,
    rel: &Path,
    text: &str,
    modified: Option<SystemTime>,
) -> Result<(), StageError> {
    let safe = rel.components().next().is_some()
        && rel.components().all(|c| matches!(c, Component::Normal(_)));
    if !safe {
        return Err(StageError::UnsafePath(rel.to_path_buf()));
    }
    let path = root.join(rel);
    let io = |source| StageError::Io {
        path: path.clone(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let mut file = std::fs::File::create_new(&path).map_err(io)?;
    file.write_all(text.as_bytes()).map_err(io)?;
    if let Some(modified) = modified {
        file.set_modified(modified).map_err(io)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        LayoutError, MAX_TREE_DEPTH, RUNTIME_SOURCE_MTIME_SECS, RuntimeFiles, StageError,
        check_project_layout, write_emitted, write_runtime,
    };
    use ipe_wasm::RUNTIME_DEP_DIR;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    fn emitted_crate(root: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::write(root.join("Cargo.toml"), "[package]\n")?;
        std::fs::write(root.join("src").join("main.rs"), "fn main() {}\n")
    }

    #[test]
    fn the_emitted_crate_alone_is_accepted() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        emitted_crate(dir.path())?;
        std::fs::create_dir_all(dir.path().join("src").join("nested"))?;
        std::fs::write(dir.path().join("src").join("nested").join("m.rs"), "")?;
        assert_eq!(check_project_layout(dir.path()), Ok(()));
        Ok(())
    }

    #[test]
    fn a_client_supplied_harness_entry_is_refused() -> std::io::Result<()> {
        for name in [
            RUNTIME_DEP_DIR,
            "Cargo.lock",
            "crate-target",
            "cargo-home",
            "build.rs",
        ] {
            let dir = tempfile::tempdir()?;
            emitted_crate(dir.path())?;
            std::fs::write(dir.path().join(name), "")?;
            assert_eq!(
                check_project_layout(dir.path()),
                Err(LayoutError::UnexpectedEntry(PathBuf::from(name))),
                "{name} must be refused"
            );
        }
        Ok(())
    }

    #[test]
    fn a_symbolic_link_is_refused_anywhere() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        emitted_crate(dir.path())?;
        std::os::unix::fs::symlink("/etc/passwd", dir.path().join("src").join("leak.rs"))?;
        assert!(matches!(
            check_project_layout(dir.path()),
            Err(LayoutError::NotPlain(_))
        ));

        let dir = tempfile::tempdir()?;
        std::fs::create_dir_all(dir.path().join("src"))?;
        std::fs::write(dir.path().join("src").join("main.rs"), "")?;
        std::os::unix::fs::symlink("/etc/passwd", dir.path().join("Cargo.toml"))?;
        assert_eq!(
            check_project_layout(dir.path()),
            Err(LayoutError::NotPlain(PathBuf::from("Cargo.toml")))
        );
        Ok(())
    }

    #[test]
    fn a_missing_manifest_or_entry_point_is_refused() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir_all(dir.path().join("src"))?;
        std::fs::write(dir.path().join("src").join("main.rs"), "")?;
        assert_eq!(
            check_project_layout(dir.path()),
            Err(LayoutError::MissingManifest)
        );

        let dir = tempfile::tempdir()?;
        std::fs::create_dir_all(dir.path().join("src"))?;
        std::fs::write(dir.path().join("Cargo.toml"), "")?;
        assert_eq!(
            check_project_layout(dir.path()),
            Err(LayoutError::MissingEntryPoint)
        );
        Ok(())
    }

    #[test]
    fn nesting_past_the_ceiling_is_refused() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        emitted_crate(dir.path())?;
        let mut deep = dir.path().join("src");
        for _ in 0..MAX_TREE_DEPTH {
            deep.push("d");
        }
        std::fs::create_dir_all(&deep)?;
        assert!(matches!(
            check_project_layout(dir.path()),
            Err(LayoutError::TooDeep(_))
        ));
        Ok(())
    }

    #[test]
    fn the_runtime_is_written_fresh_and_stamped() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let runtime: RuntimeFiles = BTreeMap::from([(
            PathBuf::from("src").join("lib.rs"),
            "pub fn f() {}\n".to_owned(),
        )]);
        assert!(write_runtime(dir.path(), &runtime).is_ok());
        let written = dir.path().join(RUNTIME_DEP_DIR).join("src").join("lib.rs");
        assert_eq!(std::fs::read_to_string(&written)?, "pub fn f() {}\n");
        let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(RUNTIME_SOURCE_MTIME_SECS);
        assert_eq!(std::fs::metadata(&written)?.modified()?, stamp);
        // A second write never lands over an existing runtime directory.
        assert!(matches!(
            write_runtime(dir.path(), &runtime),
            Err(StageError::Io { .. })
        ));
        Ok(())
    }

    #[test]
    fn an_escaping_or_absolute_path_is_refused() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        for rel in ["../escape.rs", "/abs.rs", "src/../../x.rs", ""] {
            let files = BTreeMap::from([(rel.to_owned(), String::new())]);
            assert!(
                matches!(
                    write_emitted(dir.path(), &files),
                    Err(StageError::UnsafePath(_))
                ),
                "{rel:?} must be refused"
            );
        }
        Ok(())
    }
}

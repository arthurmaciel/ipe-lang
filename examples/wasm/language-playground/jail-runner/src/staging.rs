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

use ipe_wasm::{DepManifest, DepManifestError, RUNTIME_DEP_DIR};

/// The program prewarm compiles and builds to fill the warm cache.
pub const PREWARM_PROGRAM: &str = "module Main exposing (main)\n\nimport Ipe.Io as Io\n\nmain : Task Error ()\nmain =\n    Io.println \"hello\"\n";

/// Deepest directory nesting walked in a warm tree.
pub const MAX_TREE_DEPTH: usize = 32;

/// The most files one staged project may hold, `Cargo.toml` included.
///
/// The server's staging allowlist (`maxFiles` in `server/src/Staging.ipe`)
/// admits the same number; a test pins the two together.
pub const MAX_STAGED_FILES: usize = 64;

/// The most path segments under `src/` one staged file may have.
///
/// The server's staging allowlist (`maxSegments` in `server/src/Staging.ipe`)
/// admits the same number; a test pins the two together.
pub const MAX_SOURCE_SEGMENTS: usize = 8;

/// The most entries one staged project may hold, directories included.
///
/// Every directory must hold a file, so no legal project needs more.
const MAX_STAGED_ENTRIES: usize = MAX_STAGED_FILES * MAX_SOURCE_SEGMENTS;

/// Seconds after the Unix epoch stamped as the modification time of every
/// materialised runtime file.
///
/// A fixed stamp makes the runtime written for one request, to cargo's
/// freshness check, the same source prewarm compiled, so the warm artifacts
/// stay fresh and only the user crate is rebuilt.
pub const RUNTIME_SOURCE_MTIME_SECS: u64 = 1_000_000_000;

const MANIFEST: &str = "Cargo.toml";

/// The largest `Cargo.toml` the layout check reads; the compiler's rendering
/// is a small fraction of it.
const MAX_MANIFEST_BYTES: u64 = 16 * 1024;
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
    /// A source path with more than [`MAX_SOURCE_SEGMENTS`] segments under `src/`.
    TooDeep(PathBuf),
    /// More than [`MAX_STAGED_FILES`] files, or more entries than they can need.
    TooManyFiles,
    /// A name under `src/` other than a `[A-Za-z0-9_]` directory or `<name>.rs` file.
    BadSourceName(PathBuf),
    /// A directory under `src/` that holds nothing; the server never stages one.
    EmptyDir(PathBuf),
    /// `Cargo.toml` is larger than [`MAX_MANIFEST_BYTES`].
    ManifestTooLarge,
    /// `Cargo.toml` is not the manifest the compiler renders.
    ForeignManifest(DepManifestError),
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
                "staged source `{}` has more than {MAX_SOURCE_SEGMENTS} segments under `{SOURCE_DIR}/`",
                path.display()
            ),
            Self::TooManyFiles => {
                write!(f, "staged project holds more than {MAX_STAGED_FILES} files")
            }
            Self::BadSourceName(path) => write!(
                f,
                "staged source `{}` is not a `[A-Za-z0-9_]` directory or `<name>.rs` file",
                path.display()
            ),
            Self::EmptyDir(path) => {
                write!(f, "staged source directory `{}` is empty", path.display())
            }
            Self::ManifestTooLarge => write!(
                f,
                "staged `{MANIFEST}` is larger than {MAX_MANIFEST_BYTES} bytes"
            ),
            Self::ForeignManifest(error) => write!(f, "staged `{MANIFEST}` refused: {error}"),
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
/// The same rule the server's staging allowlist applies: the top level holds a
/// regular `Cargo.toml` and a `src/` directory and nothing else; under `src/`
/// every directory is a non-empty `[A-Za-z0-9_]` name holding something, every
/// file a regular `<name>.rs` at most [`MAX_SOURCE_SEGMENTS`] segments deep, a
/// regular `src/main.rs` among them, and at most [`MAX_STAGED_FILES`] files in
/// all. Symbolic links are refused everywhere. `Cargo.toml` must be exactly a
/// manifest the compiler renders ([`DepManifest`]): the client picks runtime
/// features from a closed set and nothing else, so no dependency, build script,
/// or source override it names can reach cargo.
///
/// # Errors
///
/// The [`LayoutError`] naming the first offending entry.
pub fn check_project_layout(project: &Path) -> Result<(), LayoutError> {
    let mut manifest = false;
    let mut walk = SourceWalk::default();
    for entry in read_entries(project, &mut walk)? {
        let (path, kind) = entry;
        let name = path.file_name().map(PathBuf::from).unwrap_or_default();
        match (name.to_str(), kind) {
            (Some(MANIFEST), EntryKind::File) => {
                manifest = true;
                walk.count_file()?;
            }
            (Some(SOURCE_DIR), EntryKind::Dir) => check_source_tree(&path, 0, &mut walk)?,
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
    check_manifest(&project.join(MANIFEST))
}

/// Accept `path` only when it holds a manifest the compiler renders.
fn check_manifest(path: &Path) -> Result<(), LayoutError> {
    use std::io::Read as _;
    let unreadable = |error: std::io::Error| LayoutError::Unreadable {
        path: PathBuf::from(MANIFEST),
        detail: error.to_string(),
    };
    let file = std::fs::File::open(path).map_err(unreadable)?;
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if u64::try_from(bytes.len()).map_or(true, |len| len > MAX_MANIFEST_BYTES) {
        return Err(LayoutError::ManifestTooLarge);
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| LayoutError::ForeignManifest(DepManifestError::NotTemplate))?;
    DepManifest::parse(text)
        .map(|_| ())
        .map_err(LayoutError::ForeignManifest)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Dir,
    Other,
}

/// What a walk of one staged project has counted so far.
#[derive(Default)]
struct SourceWalk {
    files: usize,
    entries: usize,
}

impl SourceWalk {
    /// Count one more file, refusing past [`MAX_STAGED_FILES`].
    const fn count_file(&mut self) -> Result<(), LayoutError> {
        self.files = self.files.saturating_add(1);
        if self.files > MAX_STAGED_FILES {
            return Err(LayoutError::TooManyFiles);
        }
        Ok(())
    }

    /// Count one more entry read, refusing past [`MAX_STAGED_ENTRIES`].
    const fn count_entry(&mut self) -> Result<(), LayoutError> {
        self.entries = self.entries.saturating_add(1);
        if self.entries > MAX_STAGED_ENTRIES {
            return Err(LayoutError::TooManyFiles);
        }
        Ok(())
    }
}

/// Whether `name` is one allowlisted path segment: non-empty ASCII `[A-Za-z0-9_]`.
fn is_source_segment(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Check the tree under `src/`, where `dir` sits `segments` segments below it.
fn check_source_tree(
    dir: &Path,
    segments: usize,
    walk: &mut SourceWalk,
) -> Result<(), LayoutError> {
    let entries = read_entries(dir, walk)?;
    if entries.is_empty() && segments > 0 {
        return Err(LayoutError::EmptyDir(dir.to_path_buf()));
    }
    let depth = segments.saturating_add(1);
    for (path, kind) in entries {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        match kind {
            EntryKind::File => {
                let stem = name.strip_suffix(".rs").unwrap_or_default();
                if !is_source_segment(stem) {
                    return Err(LayoutError::BadSourceName(path));
                }
                if depth > MAX_SOURCE_SEGMENTS {
                    return Err(LayoutError::TooDeep(path));
                }
                walk.count_file()?;
            }
            EntryKind::Dir => {
                if !is_source_segment(name) {
                    return Err(LayoutError::BadSourceName(path));
                }
                // A file inside sits one segment deeper still.
                if depth >= MAX_SOURCE_SEGMENTS {
                    return Err(LayoutError::TooDeep(path));
                }
                check_source_tree(&path, depth, walk)?;
            }
            EntryKind::Other => return Err(LayoutError::NotPlain(path)),
        }
    }
    Ok(())
}

/// The entries of `dir`, classified without following symbolic links, each
/// counted against the walk's entry ceiling as it is read.
fn read_entries(
    dir: &Path,
    walk: &mut SourceWalk,
) -> Result<Vec<(PathBuf, EntryKind)>, LayoutError> {
    let unreadable = |path: &Path, error: &std::io::Error| LayoutError::Unreadable {
        path: path.to_path_buf(),
        detail: error.to_string(),
    };
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| unreadable(dir, &e))? {
        let entry = entry.map_err(|e| unreadable(dir, &e))?;
        walk.count_entry()?;
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
        LayoutError, MAX_SOURCE_SEGMENTS, MAX_STAGED_FILES, PREWARM_PROGRAM,
        RUNTIME_SOURCE_MTIME_SECS, RuntimeFiles, StageError, check_project_layout, write_emitted,
        write_runtime,
    };
    use ipe_wasm::{DepManifestError, RUNTIME_DEP_DIR};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    /// The `Cargo.toml` the compiler emits for [`PREWARM_PROGRAM`].
    fn emitted_manifest() -> std::io::Result<String> {
        ipe_wasm::emit_files(PREWARM_PROGRAM)
            .ok()
            .and_then(|mut files| files.remove("Cargo.toml"))
            .ok_or_else(|| std::io::Error::other("the prewarm program emits no manifest"))
    }

    fn emitted_crate(root: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::write(root.join("Cargo.toml"), emitted_manifest()?)?;
        std::fs::write(root.join("src").join("main.rs"), "fn main() {}\n")
    }

    #[test]
    fn a_manifest_the_compiler_never_renders_is_refused() -> std::io::Result<()> {
        let emitted = emitted_manifest()?;
        let refused = [
            (
                emitted.replace(
                    "[dependencies]\n",
                    "[dependencies]\nevil = { path = \"/etc\" }\n",
                ),
                LayoutError::ForeignManifest(DepManifestError::NotTemplate),
            ),
            (
                format!("{emitted}[patch.crates-io]\n"),
                LayoutError::ForeignManifest(DepManifestError::NotTemplate),
            ),
            (
                "[package]\n".to_owned(),
                LayoutError::ForeignManifest(DepManifestError::NotTemplate),
            ),
            (
                String::new(),
                LayoutError::ForeignManifest(DepManifestError::NotTemplate),
            ),
            (" ".repeat(17 * 1024), LayoutError::ManifestTooLarge),
        ];
        for (manifest, error) in refused {
            let dir = tempfile::tempdir()?;
            emitted_crate(dir.path())?;
            std::fs::write(dir.path().join("Cargo.toml"), manifest)?;
            assert_eq!(check_project_layout(dir.path()), Err(error));
        }
        let dir = tempfile::tempdir()?;
        emitted_crate(dir.path())?;
        std::fs::write(dir.path().join("Cargo.toml"), [0xff_u8, 0xfe])?;
        assert_eq!(
            check_project_layout(dir.path()),
            Err(LayoutError::ForeignManifest(DepManifestError::NotTemplate))
        );
        Ok(())
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

    /// A project holding the emitted crate plus a source at `rel` under `src/`.
    fn crate_with(rel: &str) -> std::io::Result<tempfile::TempDir> {
        let dir = tempfile::tempdir()?;
        emitted_crate(dir.path())?;
        let path = dir.path().join("src").join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, "")?;
        Ok(dir)
    }

    #[test]
    fn nesting_past_the_ceiling_is_refused() -> std::io::Result<()> {
        let at_ceiling = ["d"; MAX_SOURCE_SEGMENTS - 1].join("/") + "/m.rs";
        assert_eq!(
            check_project_layout(crate_with(&at_ceiling)?.path()),
            Ok(())
        );
        let past = ["d"; MAX_SOURCE_SEGMENTS].join("/") + "/m.rs";
        assert!(matches!(
            check_project_layout(crate_with(&past)?.path()),
            Err(LayoutError::TooDeep(_))
        ));
        Ok(())
    }

    #[test]
    fn a_name_outside_the_source_allowlist_is_refused() -> std::io::Result<()> {
        for rel in [
            "data.json",
            ".x.rs",
            ".rs",
            "a-b.rs",
            "\u{e9}.rs",
            "a b/m.rs",
            ".hidden/m.rs",
            "build",
        ] {
            assert!(
                matches!(
                    check_project_layout(crate_with(rel)?.path()),
                    Err(LayoutError::BadSourceName(_))
                ),
                "{rel:?} must be refused"
            );
        }
        Ok(())
    }

    #[test]
    fn an_empty_source_directory_is_refused() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        emitted_crate(dir.path())?;
        std::fs::create_dir_all(dir.path().join("src").join("empty"))?;
        assert!(matches!(
            check_project_layout(dir.path()),
            Err(LayoutError::EmptyDir(_))
        ));
        Ok(())
    }

    #[test]
    fn more_files_than_the_server_stages_are_refused() -> std::io::Result<()> {
        // `Cargo.toml` and `src/main.rs` count: this is exactly the ceiling.
        let dir = tempfile::tempdir()?;
        emitted_crate(dir.path())?;
        for n in 0..MAX_STAGED_FILES - 2 {
            std::fs::write(dir.path().join("src").join(format!("m{n}.rs")), "")?;
        }
        assert_eq!(check_project_layout(dir.path()), Ok(()));
        std::fs::write(dir.path().join("src").join("one_more.rs"), "")?;
        assert_eq!(
            check_project_layout(dir.path()),
            Err(LayoutError::TooManyFiles)
        );
        Ok(())
    }

    /// The integer `name` is bound to in the server's `Staging.ipe`.
    #[test]
    fn the_layout_ceilings_match_the_server_allowlist() {
        let source = include_str!("../../server/src/Staging.ipe");
        assert_eq!(
            crate::ipe_source::constant(source, "maxFiles"),
            Some(MAX_STAGED_FILES)
        );
        assert_eq!(
            crate::ipe_source::constant(source, "maxSegments"),
            Some(MAX_SOURCE_SEGMENTS)
        );
        let chars: String = ('a'..='z')
            .chain('A'..='Z')
            .chain('0'..='9')
            .chain(['_'])
            .collect();
        assert!(
            source.contains(&format!("segmentChars =\n    \"{chars}\"\n")),
            "the server's segment charset drifted from `[A-Za-z0-9_]`"
        );
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

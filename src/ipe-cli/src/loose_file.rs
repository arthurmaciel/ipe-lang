//! Loose-file resolution — the one module set a `.ipe` file under no `package.ipe` compiles to.
//!
//! `ipe build`, `ipe watch`, `ipe lint`, the single-entry analysis commands
//! and `ipe lsp` all resolve a loose file here, so the editor and the batch
//! build can never disagree about which modules make up the program. The
//! set is the entry plus the transitive closure of the sibling modules its
//! imports name: each import probes exactly one path, only regular files
//! contained in the entry's directory are read, and the closure is capped by
//! [`LooseFileLimits`]. The directory holding the entry is never listed, so a
//! loose file in `/tmp` or `$HOME` reads nothing unrelated to it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use ipe_intern::{Interner, Symbol};

use crate::{CliError, io_bounded, project};

/// Upper bound on the user modules a loose-file load follows through imports.
pub const MAX_LOOSE_FILE_MODULES: usize = 256;

/// Upper bound on the source bytes a loose-file load reads across its whole closure.
pub const MAX_LOOSE_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// The ceilings one loose-file load is held to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LooseFileLimits {
    /// Most modules the closure may hold, the entry included.
    pub modules: usize,
    /// Most source bytes the closure may hold, the entry included.
    pub bytes: u64,
}

impl LooseFileLimits {
    /// The limits every CLI and editor surface loads a loose file under.
    pub const DEFAULT: Self = Self {
        modules: MAX_LOOSE_FILE_MODULES,
        bytes: MAX_LOOSE_FILE_BYTES,
    };
}

/// Where a `.ipe` file's project is rooted.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProjectRoot {
    /// A package directory holding a `package.ipe` manifest.
    Package(PathBuf),
    /// A file under no manifest, compiled alone plus the siblings it imports.
    LooseFile(PathBuf),
}

impl ProjectRoot {
    /// Classify `file` by its nearest `package.ipe`.
    ///
    /// A workspace folder holding a manifest wins; otherwise the manifest
    /// walk-up from the file decides (it probes one `package.ipe` path per
    /// ancestor and lists no directory).
    #[must_use]
    pub fn of(workspace_root: Option<&Path>, file: &Path) -> Self {
        workspace_root
            .filter(|root| project::manifest_in_dir(root).is_some())
            .map(Path::to_path_buf)
            .or_else(|| {
                crate::find_manifest_for_ipe_file(file)
                    .and_then(|manifest| manifest.parent().map(Path::to_path_buf))
            })
            .map_or_else(|| Self::LooseFile(file.to_path_buf()), Self::Package)
    }
}

/// The user modules a loose file resolves to, before stdlib and FFI injection.
#[derive(Debug)]
pub struct LooseFileSources {
    /// Every loaded module's path and source text, keyed by module path.
    pub sources: BTreeMap<Vec<String>, (PathBuf, String)>,
    /// The loaded modules as discovery records, in module-path order.
    pub discovered: Vec<project::DiscoveredModule>,
    /// The entry's declared module path.
    pub entry_module: Vec<String>,
}

/// Load a loose file plus the transitive closure of sibling modules it imports.
///
/// An import `A.B` resolves to `<dir>/A/B.ipe`, where `<dir>` is the entry's
/// directory; only that one path is probed, and only a regular file (not a
/// symlink) that canonicalizes inside `<dir>` is read. The read itself walks
/// `A` then `B.ipe` from `<dir>` refusing every symlink, and reads from the
/// handle it opened, so a file swapped after the checks can neither escape
/// `<dir>` nor block the load. An import with no such file (the stdlib, a
/// typo) is left for the compiler to resolve or report. A sibling that fails
/// to parse is still loaded — the compiler reports its errors — but
/// contributes no further imports. `entry_text` shadows the entry's disk
/// bytes (an unsaved editor buffer).
///
/// # Errors
/// [`CliError::Pipeline`] when the entry does not parse; [`CliError::Io`]
/// when the entry or a probed module cannot be read;
/// [`CliError::DiscoveryLimitReached`] when the import closure exceeds
/// `limits`.
pub fn resolve_loose_file(
    entry: &Path,
    entry_text: Option<&str>,
    limits: LooseFileLimits,
) -> Result<LooseFileSources, CliError> {
    let entry_source = match entry_text {
        Some(text) => text.to_owned(),
        None => io_bounded::read_to_string_capped(entry, io_bounded::SOURCE_READ_CAP)?,
    };
    let mut interner = Interner::new();
    let parsed = ipe_parse::parse_module(&entry_source, &mut interner).map_err(|diag| {
        CliError::Pipeline {
            file: entry.to_path_buf(),
            src: entry_source.clone(),
            diag: Box::new(diag),
        }
    })?;
    let entry_module = module_segments(&parsed.name.value, &interner);
    let mut pending = imported_modules(&parsed, &interner);
    let mut total_bytes = source_bytes(&entry_source);
    let mut probed: BTreeSet<Vec<String>> = BTreeSet::from([entry_module.clone()]);
    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    sources.insert(entry_module.clone(), (entry.to_path_buf(), entry_source));

    let source_dir = entry_directory(entry);
    if let Ok(canonical_dir) = fs::canonicalize(source_dir) {
        while let Some(module) = pending.pop() {
            if !probed.insert(module.clone()) {
                continue;
            }
            let Some(path) = sibling_module_file(source_dir, &canonical_dir, &module) else {
                continue;
            };
            if sources.len() >= limits.modules {
                return Err(closure_too_large(
                    entry,
                    &format!("more than {} sibling modules", limits.modules),
                ));
            }
            let Some(source) = read_sibling_module(&canonical_dir, &module, &path)? else {
                continue;
            };
            total_bytes = total_bytes.saturating_add(source_bytes(&source));
            if total_bytes > limits.bytes {
                return Err(closure_too_large(
                    entry,
                    &format!("more than {} bytes of sibling source", limits.bytes),
                ));
            }
            if let Ok(parsed) = ipe_parse::parse_module(&source, &mut interner) {
                pending.extend(imported_modules(&parsed, &interner));
            }
            sources.insert(module, (path, source));
        }
    }

    let discovered = sources
        .iter()
        .map(|(module, (path, _))| project::DiscoveredModule {
            path: path.clone(),
            module_path: module.clone(),
        })
        .collect();
    Ok(LooseFileSources {
        sources,
        discovered,
        entry_module,
    })
}

/// The refusal for a closure past one of its [`LooseFileLimits`].
fn closure_too_large(entry: &Path, what: &str) -> CliError {
    CliError::DiscoveryLimitReached {
        detail: format!("`{}` imports {what}", entry.display()),
    }
}

/// The byte length of `source`, as counted against [`LooseFileLimits::bytes`].
fn source_bytes(source: &str) -> u64 {
    u64::try_from(source.len()).unwrap_or(u64::MAX)
}

/// The directory sibling imports resolve against: the entry's parent, or `.` for a bare file name.
fn entry_directory(entry: &Path) -> &Path {
    entry
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// The on-disk file for sibling `module` under `source_dir`.
///
/// `None` unless every segment is a module segment and the path is a regular
/// file (not a symlink) that canonicalizes inside `canonical_dir`, the
/// canonical form of `source_dir`. The returned path keeps `source_dir`'s
/// spelling so diagnostics name the file as the user wrote its directory.
fn sibling_module_file(
    source_dir: &Path,
    canonical_dir: &Path,
    module: &[String],
) -> Option<PathBuf> {
    if module.is_empty()
        || !module
            .iter()
            .all(|segment| project::is_module_segment(segment))
    {
        return None;
    }
    let mut path = source_dir.to_path_buf();
    path.extend(module);
    path.set_extension("ipe");
    let is_regular_file = fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_file());
    let is_contained =
        fs::canonicalize(&path).is_ok_and(|canonical| canonical.starts_with(canonical_dir));
    (is_regular_file && is_contained).then_some(path)
}

/// Read sibling `module` from the handle [`open_module_beneath`] returns.
///
/// `None` when the opened file is not a regular file (a FIFO or device
/// swapped in after the path checks); `path` only names the file in errors.
///
/// # Errors
/// [`CliError::Io`] when the walk to the file or its read fails;
/// [`CliError::FileTooLarge`] past [`io_bounded::SOURCE_READ_CAP`].
fn read_sibling_module(
    canonical_dir: &Path,
    module: &[String],
    path: &Path,
) -> Result<Option<String>, CliError> {
    let io_error = |source| CliError::Io {
        path: path.to_path_buf(),
        source,
    };
    let file = open_module_beneath(canonical_dir, module).map_err(io_error)?;
    if !file.metadata().map_err(io_error)?.is_file() {
        return Ok(None);
    }
    io_bounded::read_open_file_capped(file, path, io_bounded::SOURCE_READ_CAP).map(Some)
}

/// Open `<canonical_dir>/A/B.ipe` for module `A.B`, one segment at a time, refusing every symlink.
///
/// Each directory is opened relative to its parent's handle with
/// `O_NOFOLLOW`, and the file with `O_NOFOLLOW | O_NONBLOCK`, so the opened
/// file lies inside `canonical_dir` by construction and a FIFO never blocks
/// the open.
#[cfg(unix)]
fn open_module_beneath(canonical_dir: &Path, module: &[String]) -> std::io::Result<fs::File> {
    use rustix::fs::{Mode, OFlags};
    let Some((file_segment, dir_segments)) = module.split_last() else {
        return Err(std::io::ErrorKind::NotFound.into());
    };
    let mut dir = rustix::fs::open(
        canonical_dir,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    for segment in dir_segments {
        dir = rustix::fs::openat(&dir, segment.as_str(), dir_flags, Mode::empty())?;
    }
    let file_flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let file_name = format!("{file_segment}.ipe");
    let file = rustix::fs::openat(&dir, file_name.as_str(), file_flags, Mode::empty())?;
    Ok(fs::File::from(file))
}

/// Open `<canonical_dir>/A/B.ipe` for module `A.B`.
#[cfg(not(unix))]
fn open_module_beneath(canonical_dir: &Path, module: &[String]) -> std::io::Result<fs::File> {
    let mut path = canonical_dir.to_path_buf();
    path.extend(module);
    path.set_extension("ipe");
    fs::File::open(path)
}

/// Every module path `parsed` imports.
fn imported_modules(parsed: &ipe_syntax::Module, interner: &Interner) -> Vec<Vec<String>> {
    parsed
        .imports
        .iter()
        .map(|import| module_segments(&import.name.value, interner))
        .collect()
}

/// Render interned module-name segments as strings.
fn module_segments(symbols: &[Symbol], interner: &Interner) -> Vec<String> {
    symbols
        .iter()
        .map(|symbol| interner.resolve(*symbol).unwrap_or_default().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{
        CliError, LooseFileLimits, LooseFileSources, ProjectRoot, open_module_beneath,
        read_sibling_module, resolve_loose_file, sibling_module_file,
    };

    /// A fresh, canonical scratch directory unique to `name` and this process.
    #[allow(clippy::expect_used)] // test fixture: an unwritable temp dir IS the failure
    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ipe-loose-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        fs::canonicalize(&dir).expect("canonical scratch dir")
    }

    #[allow(clippy::expect_used)] // test fixture: an unwritable temp dir IS the failure
    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(path, text).expect("write fixture file");
    }

    fn user_modules(loaded: &LooseFileSources) -> Vec<Vec<String>> {
        loaded.sources.keys().cloned().collect()
    }

    fn module(segments: &[&str]) -> Vec<String> {
        segments.iter().map(|s| (*s).to_owned()).collect()
    }

    /// The default limits with the module ceiling lowered to `modules`.
    const fn modules_only(modules: usize) -> LooseFileLimits {
        LooseFileLimits {
            modules,
            ..LooseFileLimits::DEFAULT
        }
    }

    #[test]
    #[allow(clippy::expect_used)] // a load failure IS the regression under test
    fn loose_file_beside_unreadable_directories_loads_alone() {
        let dir = scratch_dir("unreadable");
        let entry = dir.join("Main.ipe");
        write(
            &entry,
            "module Main exposing (main)\n\nimport Ipe.Io as Io\n\nmain = Io.println \"hi\"\n",
        );
        // Unrelated neighbours: a sibling module nobody imports and a
        // directory the process may not read (a systemd-private dir in `/tmp`).
        write(
            &dir.join("Other.ipe"),
            "module Other exposing (x)\n\nx = 1\n",
        );
        let locked = dir.join("locked");
        write(
            &locked.join("Hidden.ipe"),
            "module Hidden exposing (y)\n\ny = 2\n",
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
                .expect("lock the unrelated directory");
        }

        let loaded = resolve_loose_file(&entry, None, LooseFileLimits::DEFAULT);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
        }
        let _ = fs::remove_dir_all(&dir);
        let loaded = loaded.expect("a loose file loads without listing its directory");
        assert_eq!(user_modules(&loaded), vec![module(&["Main"])]);
        assert_eq!(loaded.entry_module, module(&["Main"]));
    }

    #[test]
    #[allow(clippy::expect_used)] // a load failure IS the regression under test
    fn loose_file_follows_the_sibling_modules_it_imports() {
        let dir = scratch_dir("imports");
        let entry = dir.join("Main.ipe");
        write(
            &entry,
            "module Main exposing (main)\n\nimport Helper\nimport Ipe.Io as Io\n\nmain = Io.println Helper.greeting\n",
        );
        write(
            &dir.join("Helper.ipe"),
            "module Helper exposing (greeting)\n\nimport Lib.Util\n\ngreeting = Lib.Util.word\n",
        );
        write(
            &dir.join("Lib").join("Util.ipe"),
            "module Lib.Util exposing (word)\n\nword = \"hi\"\n",
        );
        write(
            &dir.join("Unused.ipe"),
            "module Unused exposing (z)\n\nz = 3\n",
        );

        let loaded = resolve_loose_file(&entry, None, LooseFileLimits::DEFAULT);
        let _ = fs::remove_dir_all(&dir);
        let loaded = loaded.expect("imported siblings resolve");
        assert_eq!(
            user_modules(&loaded),
            vec![
                module(&["Helper"]),
                module(&["Lib", "Util"]),
                module(&["Main"])
            ]
        );
        assert_eq!(loaded.discovered.len(), 3);
    }

    #[test]
    #[allow(clippy::expect_used)] // a load failure IS the regression under test
    fn open_buffer_imports_drive_the_loose_file_closure() {
        let dir = scratch_dir("overlay");
        let entry = dir.join("Main.ipe");
        write(&entry, "module Main exposing (main)\n\nmain = 1\n");
        write(
            &dir.join("Helper.ipe"),
            "module Helper exposing (x)\n\nx = 1\n",
        );
        let buffer = "module Main exposing (main)\n\nimport Helper\n\nmain = Helper.x\n";

        let loaded = resolve_loose_file(&entry, Some(buffer), LooseFileLimits::DEFAULT);
        let _ = fs::remove_dir_all(&dir);
        let loaded = loaded.expect("overlay entry loads");
        assert_eq!(
            user_modules(&loaded),
            vec![module(&["Helper"]), module(&["Main"])]
        );
        assert_eq!(
            loaded
                .sources
                .get(&module(&["Main"]))
                .map(|(_, text)| text.as_str()),
            Some(buffer)
        );
    }

    #[test]
    fn loose_file_import_closure_past_the_module_limit_is_refused() {
        let dir = scratch_dir("limit");
        let entry = dir.join("Main.ipe");
        write(
            &entry,
            "module Main exposing (main)\n\nimport A\n\nmain = A.a\n",
        );
        write(
            &dir.join("A.ipe"),
            "module A exposing (a)\n\nimport B\n\na = B.b\n",
        );
        write(&dir.join("B.ipe"), "module B exposing (b)\n\nb = 1\n");

        let at_limit = resolve_loose_file(&entry, None, modules_only(3));
        let past_limit = resolve_loose_file(&entry, None, modules_only(2));
        let _ = fs::remove_dir_all(&dir);
        assert!(at_limit.is_ok(), "three modules fit a limit of three");
        assert!(
            matches!(past_limit, Err(CliError::DiscoveryLimitReached { .. })),
            "a closure one module past the limit is refused"
        );
    }

    #[cfg(unix)]
    #[test]
    #[allow(clippy::expect_used)] // a load failure IS the regression under test
    fn loose_file_never_follows_an_import_symlinked_out_of_its_directory() {
        let outside = scratch_dir("symlink-outside");
        write(
            &outside.join("Secret.ipe"),
            "module Secret exposing (s)\n\ns = 1\n",
        );
        let dir = scratch_dir("symlink");
        let entry = dir.join("Main.ipe");
        write(
            &entry,
            "module Main exposing (main)\n\nimport Secret\n\nmain = Secret.s\n",
        );
        std::os::unix::fs::symlink(outside.join("Secret.ipe"), dir.join("Secret.ipe"))
            .expect("plant symlink");

        let loaded = resolve_loose_file(&entry, None, LooseFileLimits::DEFAULT);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
        let loaded = loaded.expect("entry still loads");
        assert_eq!(user_modules(&loaded), vec![module(&["Main"])]);
    }

    #[cfg(unix)]
    #[test]
    #[allow(clippy::expect_used)] // a load failure IS the regression under test
    fn loose_file_never_follows_a_directory_symlinked_out_of_its_directory() {
        let outside = scratch_dir("dir-symlink-outside");
        write(
            &outside.join("Util.ipe"),
            "module Lib.Util exposing (u)\n\nu = 1\n",
        );
        let dir = scratch_dir("dir-symlink");
        let entry = dir.join("Main.ipe");
        write(
            &entry,
            "module Main exposing (main)\n\nimport Lib.Util\n\nmain = Lib.Util.u\n",
        );
        std::os::unix::fs::symlink(&outside, dir.join("Lib")).expect("plant symlink");

        let loaded = resolve_loose_file(&entry, None, LooseFileLimits::DEFAULT);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
        let loaded = loaded.expect("entry still loads");
        assert_eq!(user_modules(&loaded), vec![module(&["Main"])]);
    }

    #[test]
    fn file_inside_a_package_resolves_to_the_package_root() {
        let dir = scratch_dir("package-root");
        write(
            &dir.join("package.ipe"),
            "module Package exposing (package)\n",
        );
        let entry = dir.join("src").join("Main.ipe");
        write(&entry, "module Main exposing (main)\n\nmain = 1\n");

        let from_walk_up = ProjectRoot::of(None, &entry);
        let from_workspace = ProjectRoot::of(Some(&dir), &entry);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(from_walk_up, ProjectRoot::Package(dir.clone()));
        assert_eq!(from_workspace, ProjectRoot::Package(dir));
    }

    #[test]
    fn file_under_no_manifest_is_a_loose_file() {
        let dir = scratch_dir("loose-root");
        let entry = dir.join("Main.ipe");
        write(&entry, "module Main exposing (main)\n\nmain = 1\n");

        let root = ProjectRoot::of(Some(&dir), &entry);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(root, ProjectRoot::LooseFile(entry));
    }

    #[test]
    fn loose_file_import_closure_past_the_byte_budget_is_refused() {
        let dir = scratch_dir("bytes");
        let entry = dir.join("Main.ipe");
        let entry_text = "module Main exposing (main)\n\nimport A\n\nmain = A.a\n";
        let sibling_text = "module A exposing (a)\n\na = 1\n";
        write(&entry, entry_text);
        write(&dir.join("A.ipe"), sibling_text);
        let closure_bytes = u64::try_from(entry_text.len() + sibling_text.len()).unwrap_or(0);
        let budget = |bytes| LooseFileLimits {
            bytes,
            ..LooseFileLimits::DEFAULT
        };

        let at_budget = resolve_loose_file(&entry, None, budget(closure_bytes));
        let past_budget = resolve_loose_file(&entry, None, budget(closure_bytes - 1));
        let _ = fs::remove_dir_all(&dir);
        assert!(
            at_budget.is_ok(),
            "a closure exactly at the byte budget loads"
        );
        assert!(
            matches!(past_budget, Err(CliError::DiscoveryLimitReached { .. })),
            "a closure one byte past the budget is refused"
        );
    }

    #[test]
    fn repeated_imports_load_each_sibling_once() {
        let dir = scratch_dir("diamond");
        let entry = dir.join("Main.ipe");
        write(
            &entry,
            "module Main exposing (main)\n\nimport Left\nimport Right\nimport Ipe.Io as Io\n\nmain = Left.l\n",
        );
        write(
            &dir.join("Left.ipe"),
            "module Left exposing (l)\n\nimport Shared\nimport Ipe.Io as Io\n\nl = Shared.s\n",
        );
        write(
            &dir.join("Right.ipe"),
            "module Right exposing (r)\n\nimport Shared\nimport Ipe.Io as Io\n\nr = Shared.s\n",
        );
        write(
            &dir.join("Shared.ipe"),
            "module Shared exposing (s)\n\ns = 1\n",
        );

        // A limit of four fits the diamond only if `Shared` counts once.
        let loaded = resolve_loose_file(&entry, None, modules_only(4));
        let _ = fs::remove_dir_all(&dir);
        assert!(
            loaded.is_ok_and(|loaded| loaded.sources.len() == 4),
            "the diamond loads Main, Left, Right and Shared once each"
        );
    }

    /// Only the containment check refuses a module reached through a directory symlinked outside.
    #[cfg(unix)]
    #[test]
    #[allow(clippy::expect_used)] // test fixture: a failed symlink IS the failure
    fn sibling_path_through_a_parent_symlinked_outside_is_refused_by_containment() {
        let outside = scratch_dir("parent-link-outside");
        write(&outside.join("B.ipe"), "module A.B exposing (b)\n\nb = 1\n");
        let dir = scratch_dir("parent-link");
        std::os::unix::fs::symlink(&outside, dir.join("A")).expect("plant symlink");

        let probed = dir.join("A").join("B.ipe");
        let passes_regular_file_check =
            fs::symlink_metadata(&probed).is_ok_and(|meta| meta.file_type().is_file());
        let path_level = sibling_module_file(&dir, &dir, &module(&["A", "B"]));
        let handle_level = open_module_beneath(&dir, &module(&["A", "B"]));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
        assert!(
            passes_regular_file_check,
            "the file behind the symlinked parent is a regular file"
        );
        assert_eq!(path_level, None, "containment refuses the escaping path");
        assert!(
            handle_level.is_err(),
            "the no-follow walk refuses the symlinked directory"
        );
    }

    /// Only the regular-file check refuses an in-directory file symlink.
    #[cfg(unix)]
    #[test]
    #[allow(clippy::expect_used)] // test fixture: a failed symlink IS the failure
    fn sibling_file_symlinked_inside_the_directory_is_refused_as_not_a_regular_file() {
        let dir = scratch_dir("file-link");
        let entry = dir.join("Main.ipe");
        write(
            &entry,
            "module Main exposing (main)\n\nimport X\n\nmain = X.x\n",
        );
        write(&dir.join("Real.ipe"), "module X exposing (x)\n\nx = 1\n");
        std::os::unix::fs::symlink(dir.join("Real.ipe"), dir.join("X.ipe")).expect("plant symlink");

        let passes_containment =
            fs::canonicalize(dir.join("X.ipe")).is_ok_and(|canonical| canonical.starts_with(&dir));
        let path_level = sibling_module_file(&dir, &dir, &module(&["X"]));
        let handle_level = open_module_beneath(&dir, &module(&["X"]));
        let loaded = resolve_loose_file(&entry, None, LooseFileLimits::DEFAULT);
        let _ = fs::remove_dir_all(&dir);
        assert!(
            passes_containment,
            "the symlink target stays in the directory"
        );
        assert_eq!(path_level, None, "the regular-file check refuses a symlink");
        assert!(
            handle_level.is_err(),
            "the no-follow open refuses the final symlink"
        );
        let loaded = loaded.expect("entry still loads");
        assert_eq!(user_modules(&loaded), vec![module(&["Main"])]);
    }

    #[test]
    fn sibling_path_with_a_parent_or_empty_segment_is_refused() {
        let dir = scratch_dir("segments");
        let sub = dir.join("sub");
        write(&sub.join("X.ipe"), "module X exposing (x)\n\nx = 1\n");

        let plain = sibling_module_file(&sub, &sub, &module(&["X"]));
        let parent = sibling_module_file(&sub, &sub, &module(&["..", "sub", "X"]));
        let empty = sibling_module_file(&sub, &sub, &module(&["", "X"]));
        let none = sibling_module_file(&sub, &sub, &[]);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(plain, Some(sub.join("X.ipe")), "the control path resolves");
        assert_eq!(parent, None, "a `..` segment is refused");
        assert_eq!(empty, None, "an empty segment is refused");
        assert_eq!(none, None, "an empty module path is refused");
    }

    /// A FIFO swapped in after the path checks is refused without blocking the load.
    #[cfg(unix)]
    #[test]
    #[allow(clippy::expect_used)] // test fixture: a failed `mkfifo` IS the failure
    fn sibling_fifo_is_refused_without_blocking() {
        let dir = scratch_dir("fifo");
        let fifo = dir.join("Pipe.ipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(made.success(), "mkfifo creates the fixture");

        let path_level = sibling_module_file(&dir, &dir, &module(&["Pipe"]));
        let handle_level = read_sibling_module(&dir, &module(&["Pipe"]), &fifo);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(path_level, None, "the path check refuses a FIFO");
        assert!(
            matches!(handle_level, Ok(None)),
            "the handle check refuses a FIFO without reading it"
        );
    }
}

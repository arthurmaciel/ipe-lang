//! Loose-file resolution — the one module set a `.ipe` file under no `package.ipe` compiles to.
//!
//! `ipe build`, `ipe watch`, `ipe lint`, the single-entry analysis commands
//! and `ipe lsp` all resolve a loose file here, so the editor and the batch
//! build can never disagree about which modules make up the program. The
//! set is the entry plus the transitive closure of the sibling modules its
//! imports name: each import probes exactly one path, only regular files
//! contained in the entry's directory are read, and the closure is capped by
//! [`LooseFileLimits`]. No unrelated file is ever opened, so a loose file in
//! `/tmp` or `$HOME` reads nothing unrelated to it. A directory is listed
//! only when a probed name's case-swapped spelling also resolves — always on
//! a case-insensitive filesystem, and on a case-sensitive one when such a
//! sibling exists — and only to compare entry names against the probed name's
//! exact spelling, so one file never loads under two module keys (`import
//! Helper` and `import HELPER`). Each directory is listed at most once per
//! load, and the names listed across the load are capped by
//! [`LooseFileLimits::listed_names`].

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
#[cfg(unix)]
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};

use ipe_intern::{Interner, Symbol};

use crate::{CliError, io_bounded, project};

/// Upper bound on the user modules a loose-file load follows through imports.
pub const MAX_LOOSE_FILE_MODULES: usize = 256;

/// Upper bound on the source bytes a loose-file load reads across its whole closure.
pub const MAX_LOOSE_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Upper bound on the distinct module paths a loose-file load probes, the entry included.
pub const MAX_LOOSE_FILE_PROBES: usize = 4096;

/// Upper bound on the directory entry names a loose-file load lists for exact-spelling checks.
pub const MAX_LOOSE_FILE_LISTED_NAMES: usize = 65_536;

/// The ceilings one loose-file load is held to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LooseFileLimits {
    /// Most modules the closure may hold, the entry included.
    pub modules: usize,
    /// Most source bytes the closure may hold, the entry included.
    pub bytes: u64,
    /// Most distinct module paths the closure's imports may name, the entry included.
    pub probes: usize,
    /// Most directory entry names the exact-spelling checks may list, summed over every directory.
    pub listed_names: usize,
}

impl LooseFileLimits {
    /// The limits every CLI and editor surface loads a loose file under.
    pub const DEFAULT: Self = Self {
        modules: MAX_LOOSE_FILE_MODULES,
        bytes: MAX_LOOSE_FILE_BYTES,
        probes: MAX_LOOSE_FILE_PROBES,
        listed_names: MAX_LOOSE_FILE_LISTED_NAMES,
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

    /// The one directory this project's artifacts (its FFI cache) live in.
    ///
    /// A package's manifest directory, or a loose file's own directory:
    /// nothing above either is ever consulted, so a loose file in `/tmp`
    /// never trusts a cache another user planted in a parent directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        match self {
            Self::Package(dir) => dir,
            Self::LooseFile(file) => entry_directory(file),
        }
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
    /// Every sibling file the closure's imports probe, relative to the entry's directory.
    ///
    /// A probed file that does not exist yet is listed too, so a watcher
    /// sees it appear.
    pub probed_files: Vec<PathBuf>,
}

/// The outcome of reading one vetted sibling: its path and text.
type SiblingRead = Result<(PathBuf, String), CliError>;

/// Load a loose file plus the transitive closure of sibling modules it imports.
///
/// An import `A.B` resolves to `<dir>/A/B.ipe`, where `<dir>` is the entry's
/// directory; only that one path is probed, and only a regular file under
/// real (non-symlink) directories that canonicalizes inside `<dir>` is read.
/// On unix the read walks `A` then `B.ipe` from a `<dir>` handle opened once,
/// refusing every symlink, and reads from the handle it opened, so a file
/// swapped after the checks can neither escape `<dir>` nor block the load.
/// Other platforms reopen the checked path and have no such race guarantee.
/// An import with no such file (the stdlib, a typo) or reached through a
/// symlink is left for the compiler to resolve or report. A sibling that
/// fails to parse is still loaded — the compiler reports its errors — but
/// contributes no further imports.
/// `entry_text` shadows the entry's disk bytes (an unsaved editor buffer).
///
/// # Errors
/// [`CliError::Pipeline`] when the entry does not parse;
/// [`CliError::SourceRefused`] when the entry or a probed module is a FIFO,
/// device, socket or other non-regular file, or lies where the process may
/// not look (an unreadable or exec-only directory); [`CliError::Io`] when
/// the entry or a probed module otherwise cannot be read;
/// [`CliError::FileTooLarge`] when one file passes
/// [`io_bounded::SOURCE_READ_CAP`]; [`CliError::DiscoveryLimitReached`] when
/// the import closure exceeds `limits`.
pub fn resolve_loose_file(
    entry: &Path,
    entry_text: Option<&str>,
    limits: LooseFileLimits,
) -> Result<LooseFileSources, CliError> {
    let entry_source = match entry_text {
        Some(text) if source_bytes(text) > limits.bytes => {
            return Err(bytes_past_budget(entry, limits));
        }
        Some(text) => text.to_owned(),
        None => charge_budget(
            io_bounded::read_to_string_capped(entry, budget_cap(limits.bytes)),
            entry,
            limits.bytes,
            limits,
        )?,
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
    let mut probed_files = Vec::new();
    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    sources.insert(entry_module.clone(), (entry.to_path_buf(), entry_source));

    let source_dir = SourceDir::open(entry_directory(entry));
    let mut spelling = Spelling::new(FsListing, entry, limits.listed_names);
    while let Some(module) = pending.pop() {
        if probed.contains(&module) {
            continue;
        }
        if probed.len() >= limits.probes {
            return Err(closure_too_large(
                entry,
                &format!("more than {} distinct modules", limits.probes),
            ));
        }
        probed.insert(module.clone());
        let Some(relative) = module_file(&module) else {
            continue;
        };
        probed_files.push(relative);
        let vetted = source_dir
            .as_ref()
            .map(|dir| dir.vet(&module, &mut spelling))
            .transpose()?;
        let Some(sibling) = vetted.flatten() else {
            continue;
        };
        if sources.len() >= limits.modules {
            return Err(closure_too_large(
                entry,
                &format!("more than {} sibling modules", limits.modules),
            ));
        }
        let remaining = limits.bytes.saturating_sub(total_bytes);
        let read = sibling.read(budget_cap(remaining));
        let (path, source) = charge_budget(read, entry, remaining, limits)?;
        total_bytes = total_bytes.saturating_add(source_bytes(&source));
        if total_bytes > limits.bytes {
            return Err(bytes_past_budget(entry, limits));
        }
        if let Ok(parsed) = ipe_parse::parse_module(&source, &mut interner) {
            pending.extend(imported_modules(&parsed, &interner));
        }
        sources.insert(module, (path, source));
    }

    let discovered = sources
        .iter()
        .map(|(module, (path, _))| project::DiscoveredModule::user(path.clone(), module.clone()))
        .collect();
    Ok(LooseFileSources {
        sources,
        discovered,
        entry_module,
        probed_files,
    })
}

/// The refusal for a closure past one of its [`LooseFileLimits`].
fn closure_too_large(entry: &Path, what: &str) -> CliError {
    CliError::DiscoveryLimitReached {
        detail: format!("`{}` imports {what}", entry.display()),
    }
}

/// The refusal for a closure past [`LooseFileLimits::bytes`].
fn bytes_past_budget(entry: &Path, limits: LooseFileLimits) -> CliError {
    closure_too_large(
        entry,
        &format!("more than {} bytes of source", limits.bytes),
    )
}

/// The most bytes one file may be read to with `remaining` bytes of budget left.
fn budget_cap(remaining: u64) -> u64 {
    remaining.min(io_bounded::SOURCE_READ_CAP)
}

/// Turn a read the byte budget cut short, not the per-file cap, into the closure refusal.
fn charge_budget<T>(
    read: Result<T, CliError>,
    entry: &Path,
    remaining: u64,
    limits: LooseFileLimits,
) -> Result<T, CliError> {
    match read {
        Err(CliError::FileTooLarge { .. }) if remaining < io_bounded::SOURCE_READ_CAP => {
            Err(bytes_past_budget(entry, limits))
        }
        other => other,
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

/// The file module `A.B` lives in, `A/B.ipe`, relative to the entry's directory.
///
/// `None` unless the path is non-empty and every segment is a module
/// segment, so no `..`, separator or empty component can reach the path.
fn module_file(module: &[String]) -> Option<PathBuf> {
    let (file_segment, dir_segments) = module.split_last()?;
    module
        .iter()
        .all(|segment| project::is_module_segment(segment))
        .then(|| {
            let mut path: PathBuf = dir_segments.iter().collect();
            path.push(format!("{file_segment}.ipe"));
            path
        })
}

/// The entry's directory, resolved once before any sibling is probed.
struct SourceDir<'e> {
    /// The directory as the entry path spells it, so diagnostics name files as the user wrote them.
    spelled: &'e Path,
    /// The canonical form of `spelled`, the containment bound.
    canonical: PathBuf,
    /// The handle every sibling read walks down from, opened `O_NOFOLLOW`.
    ///
    /// An open failure is kept rather than raised: it surfaces only when a
    /// vetted sibling is read, so an entry importing nothing on disk loads
    /// from a directory it may not list.
    #[cfg(unix)]
    handle: Result<OwnedFd, rustix::io::Errno>,
}

impl<'e> SourceDir<'e> {
    /// Resolve `spelled`, or `None` when it does not canonicalize (no sibling can be read then).
    fn open(spelled: &'e Path) -> Option<Self> {
        let canonical = fs::canonicalize(spelled).ok()?;
        #[cfg(unix)]
        let handle = {
            use rustix::fs::{Mode, OFlags};
            rustix::fs::open(
                canonical.as_path(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
        };
        Some(Self {
            spelled,
            canonical,
            #[cfg(unix)]
            handle,
        })
    }

    /// The sibling file for `module`, when every check a read relies on holds.
    ///
    /// `Ok(None)` — the import is left to the compiler — unless every
    /// intermediate directory is a real directory (not a symlink), the file
    /// is a regular file (not a symlink), every segment is spelled on disk
    /// exactly as the import spells it, and the file canonicalizes inside
    /// this directory. The two layers agree: a path the no-follow walk in
    /// [`VettedSibling::read`] would refuse is refused here first, so only a
    /// swap after these checks reaches that walk.
    ///
    /// # Errors
    /// [`CliError::SourceRefused`] when the probed file exists but is a FIFO,
    /// device, socket or directory, or when a directory on the way may not
    /// be searched — a module the process cannot see is refused, never
    /// mistaken for a missing one; any error of [`Spelling::is_exact`].
    fn vet<'d>(
        &'d self,
        module: &'d [String],
        spelling: &mut Spelling<'_, impl DirListing>,
    ) -> Result<Option<VettedSibling<'d>>, CliError> {
        let (Some(relative), Some((file_segment, dir_segments))) =
            (module_file(module), module.split_last())
        else {
            return Ok(None);
        };
        let path = self.spelled.join(relative);
        let mut dir = self.spelled.to_path_buf();
        for segment in dir_segments {
            let child = dir.join(segment);
            if !lstat_kind(&child, &path)?.is_some_and(|kind| kind.is_dir())
                || !spelling.is_exact(&dir, segment, &path)?
            {
                return Ok(None);
            }
            dir = child;
        }
        let Some(kind) = lstat_kind(&path, &path)? else {
            return Ok(None);
        };
        if kind.is_symlink() || !spelling.is_exact(&dir, &format!("{file_segment}.ipe"), &path)? {
            return Ok(None);
        }
        if !kind.is_file() {
            return Err(io_bounded::source_refused(
                &path,
                io_bounded::SourceRefusal::NotRegularFile,
            ));
        }
        let is_contained =
            fs::canonicalize(&path).is_ok_and(|canonical| canonical.starts_with(&self.canonical));
        Ok(is_contained.then_some(VettedSibling {
            dir: self,
            segments: module,
            path,
        }))
    }
}

/// The type of the file at `path` without following a final symlink; `None` when it is absent.
///
/// # Errors
/// [`CliError::SourceRefused`] with [`io_bounded::SourceRefusal::AccessDenied`],
/// naming `probed`, when a directory on the way may not be searched.
fn lstat_kind(path: &Path, probed: &Path) -> Result<Option<fs::FileType>, CliError> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta.file_type())),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Err(
            io_bounded::source_refused(probed, io_bounded::SourceRefusal::AccessDenied),
        ),
        Err(_) => Ok(None),
    }
}

/// A sibling module that passed [`SourceDir::vet`]; reading consumes it.
struct VettedSibling<'d> {
    /// The directory the sibling was vetted against.
    dir: &'d SourceDir<'d>,
    /// The module path, one directory segment per element, the file stem last.
    segments: &'d [String],
    /// The file's path as the user spelled the entry's directory, for diagnostics.
    path: PathBuf,
}

impl VettedSibling<'_> {
    /// Read the sibling from the handle the no-follow walk opens, at most `cap` bytes.
    ///
    /// # Errors
    /// [`CliError::SourceRefused`] when the opened file is not a regular file
    /// (a FIFO or device swapped in after the checks), a symlink was swapped
    /// in on the way, or the directory handle may not be opened (an exec-only
    /// directory); [`CliError::Io`] when the walk or the read otherwise fails;
    /// [`CliError::FileTooLarge`] past `cap`.
    fn read(self, cap: u64) -> SiblingRead {
        let file = self
            .open()
            .map_err(|source| io_bounded::open_error(&self.path, source))?;
        let file = io_bounded::regular_file(file, &self.path)?;
        io_bounded::read_open_file_capped(file, &self.path, cap).map(|source| (self.path, source))
    }

    /// Open the sibling beneath the directory handle, refusing every symlink.
    #[cfg(unix)]
    fn open(&self) -> std::io::Result<fs::File> {
        let dir = self
            .dir
            .handle
            .as_ref()
            .map_err(|errno| std::io::Error::from(*errno))?;
        open_module_beneath(dir.as_fd(), self.segments)
    }

    /// Open the sibling by its canonical path; off unix nothing stops a swap after the checks.
    #[cfg(not(unix))]
    fn open(&self) -> std::io::Result<fs::File> {
        let mut path = self.dir.canonical.clone();
        path.extend(self.segments);
        path.set_extension("ipe");
        fs::File::open(path)
    }
}

/// Open `A/B.ipe` for module `A.B` beneath `dir`, one segment at a time, refusing every symlink.
///
/// Each directory is opened relative to its parent's handle with
/// `O_NOFOLLOW`, and the file with `O_NOFOLLOW | O_NONBLOCK | O_NOCTTY`, so
/// the opened file lies beneath `dir` by construction, a FIFO never blocks
/// the open, and a terminal device never becomes the controlling terminal.
#[cfg(unix)]
fn open_module_beneath(dir: BorrowedFd<'_>, module: &[String]) -> std::io::Result<fs::File> {
    use rustix::fs::{Mode, OFlags};
    let Some((file_segment, dir_segments)) = module.split_last() else {
        return Err(std::io::ErrorKind::NotFound.into());
    };
    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut descended: Option<OwnedFd> = None;
    for segment in dir_segments {
        let parent = descended.as_ref().map_or(dir, AsFd::as_fd);
        descended = Some(rustix::fs::openat(
            parent,
            segment.as_str(),
            dir_flags,
            Mode::empty(),
        )?);
    }
    let parent = descended.as_ref().map_or(dir, AsFd::as_fd);
    let file_flags =
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;
    let file_name = format!("{file_segment}.ipe");
    let file = rustix::fs::openat(parent, file_name.as_str(), file_flags, Mode::empty())?;
    Ok(fs::File::from(file))
}

/// The filesystem reads the exact-spelling check makes.
trait DirListing {
    /// Whether `path` may name an entry: `false` only when the lookup reports it absent.
    fn may_resolve(&self, path: &Path) -> bool;

    /// The entry names of `dir`, at most `limit` of them; `None` when it holds more.
    ///
    /// # Errors
    /// The I/O error opening `dir` or reading one of its entries.
    fn names(&self, dir: &Path, limit: usize) -> std::io::Result<Option<Vec<OsString>>>;
}

/// The real filesystem.
struct FsListing;

impl DirListing for FsListing {
    fn may_resolve(&self, path: &Path) -> bool {
        !fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    }

    fn names(&self, dir: &Path, limit: usize) -> std::io::Result<Option<Vec<OsString>>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(dir)? {
            if names.len() >= limit {
                return Ok(None);
            }
            names.push(entry?.file_name());
        }
        Ok(Some(names))
    }
}

/// The exact-spelling checks of one load: each directory is listed at most
/// once, and every listing is charged to one budget of entry names.
struct Spelling<'e, L> {
    /// The filesystem reads the checks make.
    listing: L,
    /// The load's entry file, named by the budget refusal.
    entry: &'e Path,
    /// Every directory listed so far, with its entry names.
    listed: BTreeMap<PathBuf, BTreeSet<OsString>>,
    /// The entry names the load may still list.
    remaining: usize,
    /// The whole budget, [`LooseFileLimits::listed_names`].
    limit: usize,
}

impl<'e, L: DirListing> Spelling<'e, L> {
    /// The checks for the load of `entry`, listing at most `limit` entry names in all.
    fn new(listing: L, entry: &'e Path, limit: usize) -> Self {
        Self {
            listing,
            entry,
            listed: BTreeMap::new(),
            remaining: limit,
            limit,
        }
    }

    /// Whether the existing entry `dir/name` is spelled on disk exactly as `name`.
    ///
    /// A case-insensitive filesystem resolves `HELPER.ipe` to `Helper.ipe`,
    /// which would load one file under two module keys. When the case-swapped
    /// spelling is absent, the lookup that found `name` was exact and nothing
    /// is listed; otherwise the directory's entry names are compared against
    /// `name`.
    ///
    /// # Errors
    /// [`CliError::DiscoveryLimitReached`] when the listing would pass the
    /// load's budget; [`CliError::SourceRefused`] with
    /// [`io_bounded::SourceRefusal::AccessDenied`], naming `probed`, when the
    /// listing is denied; [`CliError::Io`] when it otherwise fails. A listing
    /// that cannot be completed never reads as a misspelling.
    fn is_exact(&mut self, dir: &Path, name: &str, probed: &Path) -> Result<bool, CliError> {
        if !self.listing.may_resolve(&dir.join(swap_ascii_case(name))) {
            return Ok(true);
        }
        if let Some(names) = self.listed.get(dir) {
            return Ok(names.contains(OsStr::new(name)));
        }
        let names = match self.listing.names(dir, self.remaining) {
            Ok(Some(names)) => names,
            Ok(None) => {
                return Err(closure_too_large(
                    self.entry,
                    &format!(
                        "modules whose directories list more than {} entries",
                        self.limit
                    ),
                ));
            }
            Err(error) => return Err(listing_error(dir, probed, error)),
        };
        self.remaining = self.remaining.saturating_sub(names.len());
        let names: BTreeSet<OsString> = names.into_iter().collect();
        let is_exact = names.contains(OsStr::new(name));
        self.listed.insert(dir.to_path_buf(), names);
        Ok(is_exact)
    }
}

/// The typed failure for a directory listing that could not be read.
fn listing_error(dir: &Path, probed: &Path, source: std::io::Error) -> CliError {
    if source.kind() == std::io::ErrorKind::PermissionDenied {
        io_bounded::source_refused(probed, io_bounded::SourceRefusal::AccessDenied)
    } else {
        CliError::Io {
            path: dir.to_path_buf(),
            source,
        }
    }
}

/// `name` with every ASCII letter's case inverted.
fn swap_ascii_case(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c.to_ascii_uppercase()
            }
        })
        .collect()
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
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{
        CliError, DirListing, FsListing, LooseFileLimits, LooseFileSources, ProjectRoot,
        SiblingRead, SourceDir, Spelling, VettedSibling, io_bounded, module_file,
        resolve_loose_file, swap_ascii_case,
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

    /// The path the resolver's checks vet for `module` under `dir`, if any.
    fn vetted_path(dir: &Path, module: &[String]) -> Option<PathBuf> {
        let source_dir = SourceDir::open(dir)?;
        let entry = dir.join("Main.ipe");
        let mut spelling = Spelling::new(FsListing, &entry, LooseFileLimits::DEFAULT.listed_names);
        source_dir
            .vet(module, &mut spelling)
            .ok()
            .flatten()
            .map(|sibling| sibling.path)
    }

    /// Read `module` under `dir` skipping the path checks, as a swap after them would.
    fn read_unvetted(dir: &Path, module: &[String]) -> Option<SiblingRead> {
        let source_dir = SourceDir::open(dir)?;
        let path = dir.join(module_file(module)?);
        let sibling = VettedSibling {
            dir: &source_dir,
            segments: module,
            path,
        };
        Some(sibling.read(io_bounded::SOURCE_READ_CAP))
    }

    fn names(entries: &[&str]) -> Vec<OsString> {
        entries.iter().map(OsString::from).collect()
    }

    /// An in-memory case-insensitive filesystem: each directory maps to its entry names.
    ///
    /// A path resolves when its parent holds a name equal to it ignoring
    /// ASCII case; `failure` makes every listing fail with that error kind.
    struct CaseInsensitive {
        dirs: BTreeMap<PathBuf, Vec<OsString>>,
        failure: Option<std::io::ErrorKind>,
        listings: Cell<usize>,
    }

    impl CaseInsensitive {
        fn new(dirs: &[(&str, &[&str])]) -> Self {
            Self {
                dirs: dirs
                    .iter()
                    .map(|(dir, entries)| (PathBuf::from(dir), names(entries)))
                    .collect(),
                failure: None,
                listings: Cell::new(0),
            }
        }

        fn failing(dirs: &[(&str, &[&str])], failure: std::io::ErrorKind) -> Self {
            Self {
                failure: Some(failure),
                ..Self::new(dirs)
            }
        }
    }

    impl DirListing for &CaseInsensitive {
        fn may_resolve(&self, path: &Path) -> bool {
            let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                return false;
            };
            self.dirs
                .get(parent)
                .is_some_and(|entries| entries.iter().any(|entry| entry.eq_ignore_ascii_case(name)))
        }

        fn names(&self, dir: &Path, limit: usize) -> std::io::Result<Option<Vec<OsString>>> {
            self.listings.set(self.listings.get().saturating_add(1));
            if let Some(kind) = self.failure {
                return Err(kind.into());
            }
            let entries = self.dirs.get(dir).cloned().unwrap_or_default();
            Ok((entries.len() <= limit).then_some(entries))
        }
    }

    fn spelling(fs: &CaseInsensitive, limit: usize) -> Spelling<'static, &CaseInsensitive> {
        Spelling::new(fs, Path::new("/p/Main.ipe"), limit)
    }

    fn probed() -> PathBuf {
        PathBuf::from("/p/Helper.ipe")
    }

    #[test]
    fn a_case_variant_is_not_the_exact_spelling() {
        let fs = CaseInsensitive::new(&[("/p", &["Helper.ipe", "Lib"])]);
        let mut spelling = spelling(&fs, 16);
        let dir = Path::new("/p");
        for variant in ["HELPER.ipe", "helper.ipe", "Helper.IPE"] {
            assert!(
                matches!(spelling.is_exact(dir, variant, &probed()), Ok(false)),
                "`{variant}` resolves case-insensitively but is not the exact spelling"
            );
        }
        assert!(matches!(
            spelling.is_exact(dir, "lib", &probed()),
            Ok(false)
        ));
    }

    #[test]
    fn the_exact_spelling_is_found_among_case_variants() {
        let fs = CaseInsensitive::new(&[("/p", &["helper.ipe", "Other.ipe", "Helper.ipe", "Lib"])]);
        let mut spelling = spelling(&fs, 16);
        let dir = Path::new("/p");
        assert!(matches!(
            spelling.is_exact(dir, "Helper.ipe", &probed()),
            Ok(true)
        ));
        assert!(matches!(spelling.is_exact(dir, "Lib", &probed()), Ok(true)));
    }

    #[test]
    fn a_directory_is_listed_once_per_load() {
        let fs = CaseInsensitive::new(&[("/p", &["Helper.ipe", "Other.ipe"])]);
        let mut spelling = spelling(&fs, 2);
        let dir = Path::new("/p");
        for name in ["Helper.ipe", "HELPER.ipe", "Other.ipe", "Helper.ipe"] {
            assert!(spelling.is_exact(dir, name, &probed()).is_ok());
        }
        assert_eq!(fs.listings.get(), 1, "later probes reuse the first listing");
    }

    #[test]
    fn an_absent_case_swap_is_exact_without_listing() {
        let fs = CaseInsensitive::new(&[("/p", &[])]);
        let mut spelling = spelling(&fs, 0);
        assert!(matches!(
            spelling.is_exact(Path::new("/p"), "Helper.ipe", &probed()),
            Ok(true)
        ));
        assert_eq!(fs.listings.get(), 0);
    }

    #[test]
    fn the_listing_budget_is_exact_and_aggregate() {
        let fs = CaseInsensitive::new(&[
            ("/p", &["Helper.ipe", "Lib"]),
            ("/p/Lib", &["Util.ipe", "Other.ipe"]),
        ]);
        let mut at_budget = spelling(&fs, 4);
        assert!(matches!(
            at_budget.is_exact(Path::new("/p"), "Lib", &probed()),
            Ok(true)
        ));
        assert!(matches!(
            at_budget.is_exact(Path::new("/p/Lib"), "Util.ipe", &probed()),
            Ok(true)
        ));

        let mut past_budget = spelling(&fs, 3);
        assert!(matches!(
            past_budget.is_exact(Path::new("/p"), "Lib", &probed()),
            Ok(true)
        ));
        assert!(
            matches!(
                past_budget.is_exact(Path::new("/p/Lib"), "Util.ipe", &probed()),
                Err(CliError::DiscoveryLimitReached { .. })
            ),
            "one entry name past the load's budget is refused, never read as a misspelling"
        );
    }

    #[test]
    fn a_denied_listing_is_refused_as_access_denied() {
        let fs = CaseInsensitive::failing(
            &[("/p", &["Helper.ipe"])],
            std::io::ErrorKind::PermissionDenied,
        );
        let mut spelling = spelling(&fs, 16);
        assert!(matches!(
            spelling.is_exact(Path::new("/p"), "HELPER.ipe", &probed()),
            Err(CliError::SourceRefused {
                reason: io_bounded::SourceRefusal::AccessDenied,
                ..
            })
        ));
    }

    #[test]
    fn a_failed_listing_is_an_io_error() {
        let fs = CaseInsensitive::failing(&[("/p", &["Helper.ipe"])], std::io::ErrorKind::Other);
        let mut spelling = spelling(&fs, 16);
        assert!(matches!(
            spelling.is_exact(Path::new("/p"), "Helper.ipe", &probed()),
            Err(CliError::Io { .. })
        ));
    }

    /// A case-swapped sibling (case-sensitive filesystem) forces a listing that finds the exact name.
    #[test]
    fn a_case_swapped_sibling_does_not_hide_the_exact_spelling() {
        let dir = scratch_dir("case-swapped-sibling");
        write(&dir.join("Helper.ipe"), "module Helper exposing (x)\n");
        write(&dir.join("hELPER.IPE"), "module Other exposing (y)\n");
        let vetted = vetted_path(&dir, &module(&["Helper"]));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(vetted, Some(dir.join("Helper.ipe")));
    }

    #[test]
    fn swapping_case_changes_every_module_file_name() {
        assert_eq!(swap_ascii_case("Helper.ipe"), "hELPER.IPE");
        assert_eq!(swap_ascii_case("My_Mod2"), "mY_mOD2");
    }

    #[test]
    fn an_exactly_spelled_sibling_is_vetted() {
        let dir = scratch_dir("exact-case");
        write(
            &dir.join("Lib").join("Helper.ipe"),
            "module Lib.Helper exposing (x)\n",
        );
        assert_eq!(
            vetted_path(&dir, &module(&["Lib", "Helper"])),
            Some(dir.join("Lib").join("Helper.ipe"))
        );
        assert_eq!(vetted_path(&dir, &module(&["LIB", "Helper"])), None);
        assert_eq!(vetted_path(&dir, &module(&["Lib", "HELPER"])), None);
        let _ = fs::remove_dir_all(&dir);
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
        let mut probed_files = loaded.probed_files;
        probed_files.sort();
        assert_eq!(
            probed_files,
            vec![
                PathBuf::from("Helper.ipe"),
                Path::new("Ipe").join("Io.ipe"),
                Path::new("Lib").join("Util.ipe"),
            ],
            "every probed sibling path is listed, the missing stdlib one included"
        );
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

    #[test]
    fn loose_file_import_probes_past_the_probe_limit_are_refused() {
        let dir = scratch_dir("probes");
        let entry = dir.join("Main.ipe");
        // Neither import exists on disk: probing alone must be bounded.
        write(
            &entry,
            "module Main exposing (main)\n\nimport A\nimport B\n\nmain = 1\n",
        );
        let probes = |probes| LooseFileLimits {
            probes,
            ..LooseFileLimits::DEFAULT
        };

        let at_limit = resolve_loose_file(&entry, None, probes(3));
        let past_limit = resolve_loose_file(&entry, None, probes(2));
        let _ = fs::remove_dir_all(&dir);
        assert!(at_limit.is_ok(), "Main, A and B fit a probe limit of three");
        assert!(
            matches!(past_limit, Err(CliError::DiscoveryLimitReached { .. })),
            "a closure naming one module past the probe limit is refused"
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

    /// The path check refuses a directory symlink that stays inside, as the no-follow walk would.
    #[cfg(unix)]
    #[test]
    #[allow(clippy::expect_used)] // a load failure IS the regression under test
    fn loose_file_never_follows_a_directory_symlinked_inside_its_directory() {
        let dir = scratch_dir("dir-symlink-inside");
        let entry = dir.join("Main.ipe");
        write(
            &entry,
            "module Main exposing (main)\n\nimport A.B\n\nmain = A.B.b\n",
        );
        write(
            &dir.join("Real").join("B.ipe"),
            "module A.B exposing (b)\n\nb = 1\n",
        );
        std::os::unix::fs::symlink(dir.join("Real"), dir.join("A")).expect("plant symlink");

        let probed = dir.join("A").join("B.ipe");
        let passes_regular_file_check =
            fs::symlink_metadata(&probed).is_ok_and(|meta| meta.file_type().is_file());
        let passes_containment =
            fs::canonicalize(&probed).is_ok_and(|canonical| canonical.starts_with(&dir));
        let path_level = vetted_path(&dir, &module(&["A", "B"]));
        let handle_level = read_unvetted(&dir, &module(&["A", "B"]));
        let loaded = resolve_loose_file(&entry, None, LooseFileLimits::DEFAULT);
        let _ = fs::remove_dir_all(&dir);
        assert!(
            passes_regular_file_check && passes_containment,
            "only the symlinked directory component sets this path apart"
        );
        assert_eq!(
            path_level, None,
            "the path check refuses the symlinked directory"
        );
        assert!(
            matches!(handle_level, Some(Err(_))),
            "the no-follow walk refuses the symlinked directory"
        );
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
            "a sibling read one byte past the budget is refused as a closure limit"
        );
    }

    #[test]
    fn loose_file_entry_past_the_byte_budget_is_refused_before_parsing() {
        let dir = scratch_dir("entry-bytes");
        let entry = dir.join("Main.ipe");
        let entry_text = "module Main exposing (main)\n\nmain = 1\n";
        write(&entry, entry_text);
        let entry_bytes = u64::try_from(entry_text.len()).unwrap_or(0);
        let budget = LooseFileLimits {
            bytes: entry_bytes - 1,
            ..LooseFileLimits::DEFAULT
        };

        let from_disk = resolve_loose_file(&entry, None, budget);
        let from_buffer = resolve_loose_file(&entry, Some(entry_text), budget);
        let _ = fs::remove_dir_all(&dir);
        assert!(
            matches!(from_disk, Err(CliError::DiscoveryLimitReached { .. })),
            "an entry file one byte past the budget is refused"
        );
        assert!(
            matches!(from_buffer, Err(CliError::DiscoveryLimitReached { .. })),
            "an editor buffer one byte past the budget is refused"
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

    /// Both layers refuse a module reached through a directory symlinked outside.
    #[cfg(unix)]
    #[test]
    #[allow(clippy::expect_used)] // test fixture: a failed symlink IS the failure
    fn sibling_path_through_a_parent_symlinked_outside_is_refused() {
        let outside = scratch_dir("parent-link-outside");
        write(&outside.join("B.ipe"), "module A.B exposing (b)\n\nb = 1\n");
        let dir = scratch_dir("parent-link");
        std::os::unix::fs::symlink(&outside, dir.join("A")).expect("plant symlink");

        let probed = dir.join("A").join("B.ipe");
        let passes_regular_file_check =
            fs::symlink_metadata(&probed).is_ok_and(|meta| meta.file_type().is_file());
        let path_level = vetted_path(&dir, &module(&["A", "B"]));
        let handle_level = read_unvetted(&dir, &module(&["A", "B"]));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
        assert!(
            passes_regular_file_check,
            "the file behind the symlinked parent is a regular file"
        );
        assert_eq!(path_level, None, "the path check refuses the escaping path");
        assert!(
            matches!(handle_level, Some(Err(_))),
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
        let path_level = vetted_path(&dir, &module(&["X"]));
        let handle_level = read_unvetted(&dir, &module(&["X"]));
        let loaded = resolve_loose_file(&entry, None, LooseFileLimits::DEFAULT);
        let _ = fs::remove_dir_all(&dir);
        assert!(
            passes_containment,
            "the symlink target stays in the directory"
        );
        assert_eq!(path_level, None, "the regular-file check refuses a symlink");
        assert!(
            matches!(
                handle_level,
                Some(Err(CliError::SourceRefused {
                    reason: io_bounded::SourceRefusal::NotRegularFile,
                    ..
                }))
            ),
            "the no-follow open refuses the final symlink as not a regular file"
        );
        let loaded = loaded.expect("entry still loads");
        assert_eq!(user_modules(&loaded), vec![module(&["Main"])]);
    }

    #[test]
    fn sibling_path_with_a_parent_or_empty_segment_is_refused() {
        let dir = scratch_dir("segments");
        let sub = dir.join("sub");
        write(&sub.join("X.ipe"), "module X exposing (x)\n\nx = 1\n");

        let plain = vetted_path(&sub, &module(&["X"]));
        let parent = vetted_path(&sub, &module(&["..", "sub", "X"]));
        let empty = vetted_path(&sub, &module(&["", "X"]));
        let none = vetted_path(&sub, &[]);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(plain, Some(sub.join("X.ipe")), "the control path resolves");
        assert_eq!(parent, None, "a `..` segment is refused");
        assert_eq!(empty, None, "an empty segment is refused");
        assert_eq!(none, None, "an empty module path is refused");
    }

    /// Make `path` a FIFO.
    #[cfg(unix)]
    #[allow(clippy::expect_used)] // test fixture: a failed `mkfifo` IS the failure
    fn make_fifo(path: &Path) {
        let made = std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .expect("run mkfifo");
        assert!(made.success(), "mkfifo creates the fixture");
    }

    /// Whether `result` is the typed refusal for `reason`.
    #[cfg(unix)]
    fn is_refused<T>(result: &Result<T, CliError>, reason: io_bounded::SourceRefusal) -> bool {
        matches!(result, Err(CliError::SourceRefused { reason: got, .. }) if *got == reason)
    }

    /// An imported FIFO is refused by both layers without blocking the load.
    ///
    /// Every open is non-blocking, so the test needs no writer and no timeout.
    #[cfg(unix)]
    #[test]
    fn sibling_fifo_is_refused_without_blocking() {
        let dir = scratch_dir("fifo");
        let entry = dir.join("Main.ipe");
        write(
            &entry,
            "module Main exposing (main)\n\nimport Pipe\n\nmain = Pipe.x\n",
        );
        make_fifo(&dir.join("Pipe.ipe"));

        let mut spelling = Spelling::new(FsListing, &entry, LooseFileLimits::DEFAULT.listed_names);
        let path_level = SourceDir::open(&dir).map(|source_dir| {
            source_dir
                .vet(&module(&["Pipe"]), &mut spelling)
                .map(|sibling| sibling.map(|vetted| vetted.path))
        });
        let handle_level = read_unvetted(&dir, &module(&["Pipe"]));
        let loaded = resolve_loose_file(&entry, None, LooseFileLimits::DEFAULT);
        let _ = fs::remove_dir_all(&dir);
        let not_regular = io_bounded::SourceRefusal::NotRegularFile;
        assert!(
            path_level.is_some_and(|vetted| is_refused(&vetted, not_regular)),
            "the path check refuses a FIFO"
        );
        assert!(
            handle_level.is_some_and(|read| is_refused(&read, not_regular)),
            "the handle check refuses a FIFO without reading it"
        );
        assert!(
            is_refused(&loaded, not_regular),
            "the load refuses an imported FIFO"
        );
    }

    /// A FIFO as the entry itself is refused at once, never blocking on a writer.
    #[cfg(unix)]
    #[test]
    fn entry_fifo_is_refused_without_blocking() {
        let dir = scratch_dir("entry-fifo");
        let entry = dir.join("Main.ipe");
        make_fifo(&entry);
        let loaded = resolve_loose_file(&entry, None, LooseFileLimits::DEFAULT);
        let _ = fs::remove_dir_all(&dir);
        assert!(
            is_refused(&loaded, io_bounded::SourceRefusal::NotRegularFile),
            "a FIFO entry is refused as not a regular file"
        );
    }

    /// An import under a directory the process may not search is refused, not reported missing.
    ///
    /// Skipped when the permission bits are not enforced (running as root).
    #[cfg(unix)]
    #[test]
    #[allow(clippy::expect_used)] // test fixture: an unchangeable mode IS the failure
    fn sibling_under_an_unreadable_directory_is_refused_as_access_denied() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = scratch_dir("locked-import");
        let entry = dir.join("Main.ipe");
        write(
            &entry,
            "module Main exposing (main)\n\nimport Locked.Hidden\n\nmain = Locked.Hidden.y\n",
        );
        let locked = dir.join("Locked");
        write(
            &locked.join("Hidden.ipe"),
            "module Locked.Hidden exposing (y)\n\ny = 2\n",
        );
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .expect("lock the directory");
        let privileged = fs::read_dir(&locked).is_ok();
        let loaded = resolve_loose_file(&entry, None, LooseFileLimits::DEFAULT);
        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&dir);
        if privileged {
            eprintln!("skipped: running as root, directory permissions are not enforced");
            return;
        }
        assert!(
            is_refused(&loaded, io_bounded::SourceRefusal::AccessDenied),
            "an import the process may not look up is refused as access denied"
        );
    }

    /// Siblings in an exec-only directory are refused as access denied, not a raw I/O error.
    ///
    /// The entry opens by name, but the directory handle every sibling read
    /// walks from needs the read bit. Skipped when running as root.
    #[cfg(unix)]
    #[test]
    #[allow(clippy::expect_used)] // test fixture: an unchangeable mode IS the failure
    fn sibling_in_an_exec_only_directory_is_refused_as_access_denied() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = scratch_dir("exec-only");
        let entry = dir.join("Main.ipe");
        write(
            &entry,
            "module Main exposing (main)\n\nimport Helper\n\nmain = Helper.x\n",
        );
        write(
            &dir.join("Helper.ipe"),
            "module Helper exposing (x)\n\nx = 1\n",
        );
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o311)).expect("drop the read bit");
        let privileged = fs::read_dir(&dir).is_ok();
        let loaded = resolve_loose_file(&entry, None, LooseFileLimits::DEFAULT);
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&dir);
        if privileged {
            eprintln!("skipped: running as root, directory permissions are not enforced");
            return;
        }
        assert!(
            is_refused(&loaded, io_bounded::SourceRefusal::AccessDenied),
            "a sibling read from an exec-only directory is refused as access denied"
        );
    }

    /// A spelling listing the process may not read is refused, never taken for a misspelling.
    ///
    /// The case-swapped sibling forces the listing of the exec-only `Lib`.
    /// Skipped when running as root.
    #[cfg(unix)]
    #[test]
    #[allow(clippy::expect_used)] // test fixture: an unchangeable mode IS the failure
    fn an_unlistable_spelling_directory_is_refused_as_access_denied() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = scratch_dir("unlistable-spelling");
        let entry = dir.join("Main.ipe");
        write(
            &entry,
            "module Main exposing (main)\n\nimport Lib.Helper\n\nmain = Lib.Helper.x\n",
        );
        let lib = dir.join("Lib");
        write(
            &lib.join("Helper.ipe"),
            "module Lib.Helper exposing (x)\n\nx = 1\n",
        );
        write(&lib.join("hELPER.IPE"), "module Other exposing (y)\n");
        fs::set_permissions(&lib, fs::Permissions::from_mode(0o311)).expect("drop the read bit");
        let privileged = fs::read_dir(&lib).is_ok();
        let loaded = resolve_loose_file(&entry, None, LooseFileLimits::DEFAULT);
        let _ = fs::set_permissions(&lib, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&dir);
        if privileged {
            eprintln!("skipped: running as root, directory permissions are not enforced");
            return;
        }
        assert!(
            is_refused(&loaded, io_bounded::SourceRefusal::AccessDenied),
            "an unlistable directory is refused as access denied"
        );
    }
}

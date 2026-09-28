//! Private scratch directories and files, the one primitive every temporary write goes through.
//!
//! A scratch path is handed to external writers (`curl -o`, `sh`, the jail) that
//! re-resolve it by name, so it is only safe while no other user can read,
//! predict, or replace any component of it. Every constructor therefore
//! establishes:
//!
//! - the base is resolved and proven trusted: on Unix it is canonicalised and
//!   every ancestor is a real directory owned by the effective user or root,
//!   writable by no one else unless sticky; elsewhere, where the standard
//!   library offers no owner-only creation, it must resolve inside the current
//!   user's profile directory, which the OS keeps private to that user;
//! - entries are created under the resolved base, never the given path, so a
//!   link on the given path re-pointed after the check cannot redirect them;
//! - the private directory name carries 128 bits of OS CSPRNG entropy (an
//!   unavailable CSPRNG fails the creation, never weakens the name); it is
//!   created exclusively (mode 0700), a collision retries with a fresh name a
//!   bounded number of times, and it is re-verified with `symlink_metadata`: a
//!   real directory, owned by the effective user, no group/other permission bits;
//! - a scratch file is created inside such a directory with `O_EXCL` +
//!   `O_NOFOLLOW` + mode 0600, and its handle is verified with `fstat`; read it
//!   back through the retained handle, never by re-opening the name.
//!
//! A check that fails refuses with [`io::ErrorKind::PermissionDenied`] carrying
//! a [`ScratchError`] or a [`ScratchRootRefusal`]; nothing is created under an
//! untrusted base and nothing is written through a planted link.

use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

/// Attempts at a fresh name before creation gives up.
const MAX_ATTEMPTS: usize = 8;

/// Bytes of OS CSPRNG entropy in every scratch name (128 bits).
const ENTROPY_BYTES: usize = 16;

/// Longest caller label kept in a scratch name.
const MAX_LABEL_CHARS: usize = 64;

/// The label used when a caller label confines to nothing.
const FALLBACK_LABEL: &str = "scratch";

/// The variable naming the current user's profile directory on Windows.
pub const PROFILE_VAR: &str = "USERPROFILE";

/// Why a scratch location was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScratchRefusal {
    /// The entry is not a real directory (a symlink, a file, or another type).
    NotADirectory,
    /// The entry is not a regular file.
    NotARegularFile,
    /// The entry is owned by a user other than the effective user (or root, for a base).
    ForeignOwner,
    /// The entry is writable by other users without the sticky bit.
    WritableByOthers,
    /// A private entry grants group or other permission bits.
    NotPrivate,
}

impl fmt::Display for ScratchRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotADirectory => "not a real directory",
            Self::NotARegularFile => "not a regular file",
            Self::ForeignOwner => "owned by another user",
            Self::WritableByOthers => "writable by other users without the sticky bit",
            Self::NotPrivate => "grants group or other permissions",
        })
    }
}

impl std::error::Error for ScratchRefusal {}

/// A refused scratch location: the path and the reason.
#[derive(Debug)]
pub struct ScratchError {
    /// The path that failed verification.
    pub path: PathBuf,
    /// Why it was refused.
    pub refusal: ScratchRefusal,
}

impl fmt::Display for ScratchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "refusing scratch location {}: {}",
            self.path.display(),
            self.refusal
        )
    }
}

impl std::error::Error for ScratchError {}

/// Wrap a refusal of `path` as a `PermissionDenied` I/O error.
fn refused(path: &Path, refusal: ScratchRefusal) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        ScratchError {
            path: path.to_path_buf(),
            refusal,
        },
    )
}

/// Why a scratch root cannot be proven private to the current user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScratchRootRefusal {
    /// No absolute profile directory is set, so no root can be proven private.
    NoProfile,
    /// A path could not be resolved to its canonical form.
    Unresolvable {
        /// The path that failed to resolve.
        path: PathBuf,
        /// The rendered OS error.
        detail: String,
    },
    /// The root resolves outside the profile directory.
    OutsideProfile {
        /// The canonical scratch root.
        root: PathBuf,
        /// The canonical profile directory.
        profile: PathBuf,
    },
}

/// The remedy every root refusal names.
const FIX: &str = "set TEMP and TMP to a directory inside your user profile, \
                   such as %LOCALAPPDATA%\\Temp (the Windows default)";

impl fmt::Display for ScratchRootRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoProfile => write!(
                f,
                "refusing to create scratch files: {PROFILE_VAR} does not name an absolute \
                 directory, so the scratch location cannot be proven private to you; {FIX}"
            ),
            Self::Unresolvable { path, detail } => write!(
                f,
                "refusing to create scratch files: cannot resolve {} ({detail}), so it cannot \
                 be proven private to you; {FIX}",
                path.display()
            ),
            Self::OutsideProfile { root, profile } => write!(
                f,
                "refusing to create scratch files under {}: it lies outside your user profile \
                 ({}), so other local users may read or modify what is written there; {FIX}",
                root.display(),
                profile.display()
            ),
        }
    }
}

impl std::error::Error for ScratchRootRefusal {}

impl From<ScratchRootRefusal> for io::Error {
    fn from(refusal: ScratchRootRefusal) -> Self {
        Self::new(io::ErrorKind::PermissionDenied, refusal)
    }
}

/// Every one of the [`MAX_ATTEMPTS`] fresh names already existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamesExhausted;

impl fmt::Display for NamesExhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not create a unique scratch entry after {MAX_ATTEMPTS} attempts"
        )
    }
}

impl std::error::Error for NamesExhausted {}

/// The ownership and permission facts a verdict needs about one filesystem entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryFacts {
    /// The entry's file type, as seen without following a final symlink.
    pub kind: EntryKind,
    /// Owning uid.
    pub owner: u32,
    /// Owning gid.
    pub group: u32,
    /// Full mode bits (permission, sticky, setuid/setgid).
    pub mode: u32,
}

/// The file type of an entry, as seen without following a final symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A real directory.
    Directory,
    /// A regular file.
    File,
    /// Anything else, a symlink included.
    Other,
}

/// The effective identity scratch entries must belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    /// Effective uid.
    pub euid: u32,
    /// Effective gid.
    pub egid: u32,
}

const ROOT_UID: u32 = 0;
const STICKY: u32 = 0o1000;
const GROUP_WRITE: u32 = 0o020;
const OTHER_WRITE: u32 = 0o002;
const GROUP_OTHER_BITS: u32 = 0o077;

/// Whether `entry` may be an ancestor of (or be) the base a private directory is created under.
///
/// The base must be a directory owned by the effective user or root. A world-
/// writable base must be sticky; a group-writable one must be sticky or be the
/// effective user's own directory with the effective group (a user-private group).
///
/// # Errors
/// The [`ScratchRefusal`] naming the first violated condition.
pub const fn base_verdict(entry: EntryFacts, who: Identity) -> Result<(), ScratchRefusal> {
    if !matches!(entry.kind, EntryKind::Directory) {
        return Err(ScratchRefusal::NotADirectory);
    }
    if entry.owner != who.euid && entry.owner != ROOT_UID {
        return Err(ScratchRefusal::ForeignOwner);
    }
    let sticky = entry.mode & STICKY != 0;
    if entry.mode & OTHER_WRITE != 0 && !sticky {
        return Err(ScratchRefusal::WritableByOthers);
    }
    let own_private_group = entry.owner == who.euid && entry.group == who.egid;
    if entry.mode & GROUP_WRITE != 0 && !sticky && !own_private_group {
        return Err(ScratchRefusal::WritableByOthers);
    }
    Ok(())
}

/// Whether `entry` is a private scratch entry of the expected `kind`.
///
/// It must be exactly that kind (never a symlink), owned by the effective user,
/// and grant no group or other permission bits.
///
/// # Errors
/// The [`ScratchRefusal`] naming the first violated condition.
pub const fn private_verdict(
    entry: EntryFacts,
    kind: EntryKind,
    who: Identity,
) -> Result<(), ScratchRefusal> {
    match (entry.kind, kind) {
        (EntryKind::Directory, EntryKind::Directory) | (EntryKind::File, EntryKind::File) => {}
        (EntryKind::Directory | EntryKind::File | EntryKind::Other, EntryKind::File) => {
            return Err(ScratchRefusal::NotARegularFile);
        }
        (
            EntryKind::Directory | EntryKind::File | EntryKind::Other,
            EntryKind::Directory | EntryKind::Other,
        ) => return Err(ScratchRefusal::NotADirectory),
    }
    if entry.owner != who.euid {
        return Err(ScratchRefusal::ForeignOwner);
    }
    if entry.mode & GROUP_OTHER_BITS != 0 {
        return Err(ScratchRefusal::NotPrivate);
    }
    Ok(())
}

/// Decide whether a canonical scratch root lies inside a canonical profile.
///
/// Both paths are expected canonical (absolute, links resolved). Anything that
/// is not provably inside is refused: a profile that is relative, holds a `..`
/// segment, or names a bare filesystem root (which would make every path
/// "private"); a root that is relative, holds a `..` segment, or lies outside
/// the profile, compared component-wise so `/home/al` never contains
/// `/home/alice`.
///
/// # Errors
/// Returns the [`ScratchRootRefusal`] that the root fails.
pub fn root_within_profile(root: &Path, profile: &Path) -> Result<(), ScratchRootRefusal> {
    let has_parent_dir = |p: &Path| p.components().any(|c| matches!(c, Component::ParentDir));
    let names_a_directory = profile
        .components()
        .any(|c| matches!(c, Component::Normal(_)));
    if !profile.is_absolute() || has_parent_dir(profile) || !names_a_directory {
        return Err(ScratchRootRefusal::NoProfile);
    }
    if root.is_absolute() && !has_parent_dir(root) && root.starts_with(profile) {
        Ok(())
    } else {
        Err(ScratchRootRefusal::OutsideProfile {
            root: root.to_path_buf(),
            profile: profile.to_path_buf(),
        })
    }
}

/// Resolve `root` and `profile`, require the root inside the profile, and return the resolved root.
///
/// `profile` is the raw value of [`PROFILE_VAR`]; an unset, empty, or relative
/// value proves nothing and is refused. Resolution follows every link, so a
/// link inside the profile that points outside it is refused too. Entries must
/// be created under the returned path, not under `root`, so that a link on
/// `root` re-pointed after the check cannot redirect the creation.
///
/// # Errors
/// Returns a [`ScratchRootRefusal`] when either path cannot be resolved or the
/// resolved root lies outside the resolved profile.
pub fn verify_root_within_profile(
    root: &Path,
    profile: Option<&OsStr>,
) -> Result<PathBuf, ScratchRootRefusal> {
    let profile = profile
        .map(Path::new)
        .filter(|p| p.is_absolute())
        .ok_or(ScratchRootRefusal::NoProfile)?;
    let root = canonical(root)?;
    root_within_profile(&root, &canonical(profile)?)?;
    Ok(root)
}

/// The canonical form of `path`, or the refusal naming why it has none.
fn canonical(path: &Path) -> Result<PathBuf, ScratchRootRefusal> {
    std::fs::canonicalize(path).map_err(|e| ScratchRootRefusal::Unresolvable {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })
}

#[cfg(unix)]
mod platform {
    use super::{EntryFacts, EntryKind, Identity};
    use std::os::unix::fs::MetadataExt as _;

    /// The effective uid and gid of this process.
    #[must_use]
    pub fn identity() -> Identity {
        Identity {
            euid: rustix::process::geteuid().as_raw(),
            egid: rustix::process::getegid().as_raw(),
        }
    }

    /// The verdict facts of `meta`.
    #[must_use]
    pub fn facts(meta: &std::fs::Metadata) -> EntryFacts {
        let ft = meta.file_type();
        let kind = if ft.is_dir() {
            EntryKind::Directory
        } else if ft.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        EntryFacts {
            kind,
            owner: meta.uid(),
            group: meta.gid(),
            mode: meta.mode(),
        }
    }
}

/// Resolve `base` to the trusted canonical directory private entries are created under.
///
/// Creates `base` when absent, canonicalises it (so no ancestor is a symlink), and
/// requires [`base_verdict`] of every ancestor.
#[cfg(unix)]
fn trusted_base(base: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt as _;
    // Components this call creates are private, so they pass `base_verdict`
    // without the owner-group exception.
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(base)?;
    let canonical = std::fs::canonicalize(base)?;
    let who = platform::identity();
    for dir in canonical.ancestors() {
        let meta = std::fs::symlink_metadata(dir)?;
        base_verdict(platform::facts(&meta), who).map_err(|r| refused(dir, r))?;
    }
    Ok(canonical)
}

/// Resolve `base` to the directory private entries are created under, proven inside the user's profile.
///
/// Non-unix hosts carry no POSIX owner or mode bits, so an entry inherits its
/// parent's access control. The nearest existing ancestor of `base` is proven
/// inside the profile before any missing component is created, and `base`
/// itself is proven again once it exists; the resolved path is returned.
#[cfg(not(unix))]
fn trusted_base(base: &Path) -> io::Result<PathBuf> {
    let home = crate::home::home_dir();
    let profile = home.as_deref().map(Path::as_os_str);
    let existing = base
        .ancestors()
        .find(|a| !a.as_os_str().is_empty() && a.exists())
        .unwrap_or(base);
    verify_root_within_profile(existing, profile)?;
    std::fs::create_dir_all(base)?;
    verify_root_within_profile(base, profile).map_err(io::Error::from)
}

/// Verify that `path` is a private directory of the effective user.
///
/// # Errors
/// A `PermissionDenied` [`ScratchError`] when it is a symlink, not a directory,
/// foreign-owned, or grants group/other bits; the lookup error otherwise.
pub fn verify_private_dir(path: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    verify_private(path, &meta, EntryKind::Directory)
}

#[cfg(unix)]
fn verify_private(path: &Path, meta: &std::fs::Metadata, kind: EntryKind) -> io::Result<()> {
    private_verdict(platform::facts(meta), kind, platform::identity()).map_err(|r| refused(path, r))
}

#[cfg(not(unix))]
fn verify_private(path: &Path, meta: &std::fs::Metadata, kind: EntryKind) -> io::Result<()> {
    let ft = meta.file_type();
    let ok = match kind {
        EntryKind::Directory => ft.is_dir() && !ft.is_symlink(),
        EntryKind::File => ft.is_file() && !ft.is_symlink(),
        EntryKind::Other => false,
    };
    if ok {
        return Ok(());
    }
    Err(refused(
        path,
        if matches!(kind, EntryKind::File) {
            ScratchRefusal::NotARegularFile
        } else {
            ScratchRefusal::NotADirectory
        },
    ))
}

/// Confine a caller label to one bounded, non-empty path component.
///
/// ASCII alphanumerics, `-` and `_` are kept, every other character becomes `_`,
/// and at most [`MAX_LABEL_CHARS`] characters survive, so a label can never add
/// a path component, name `.`/`..`, or reach an alternate data stream.
fn confined_label(label: &str) -> String {
    let confined: String = label
        .chars()
        .take(MAX_LABEL_CHARS)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if confined.is_empty() {
        FALLBACK_LABEL.to_owned()
    } else {
        confined
    }
}

/// `<label>-<pid>-<32 hex chars>`, the hex drawn from `fill`.
fn scratch_name(
    label: &str,
    fill: &mut impl FnMut(&mut [u8]) -> Result<(), getrandom::Error>,
) -> io::Result<String> {
    use std::fmt::Write as _;
    let mut entropy = [0u8; ENTROPY_BYTES];
    fill(&mut entropy)?;
    let mut name = confined_label(label);
    let _ = write!(name, "-{}-", std::process::id());
    for byte in entropy {
        let _ = write!(name, "{byte:02x}");
    }
    Ok(name)
}

/// Create an entry under the trusted `base` with `create`, retrying a collision with a fresh name.
///
/// At most [`MAX_ATTEMPTS`] names are tried; `fill` supplies each name's entropy
/// and its failure fails the creation before any entry is attempted.
fn create_unique<T>(
    base: &Path,
    label: &str,
    mut fill: impl FnMut(&mut [u8]) -> Result<(), getrandom::Error>,
    mut create: impl FnMut(&Path) -> io::Result<T>,
) -> io::Result<(PathBuf, T)> {
    for _ in 0..MAX_ATTEMPTS {
        let path = base.join(scratch_name(label, &mut fill)?);
        match create(&path) {
            Ok(entry) => return Ok((path, entry)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, NamesExhausted))
}

/// Create the directory `path` exclusively with mode 0700; only the final component is created.
#[cfg(unix)]
fn exclusive_mkdir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(false)
        .create(path)
}

/// Create the directory `path` exclusively, inheriting the proven-private parent's access control.
#[cfg(not(unix))]
fn exclusive_mkdir(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

/// Open a new file at `path`: exclusive, never through a final symlink, mode 0600.
#[cfg(unix)]
fn exclusive_open(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

/// Open a new file at `path` exclusively, inheriting the proven-private parent's access control.
#[cfg(not(unix))]
fn exclusive_open(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
}

// ── ScratchDir ───────────────────────────────────────────────────────────────

/// A verified private, unpredictably named temporary directory, removed on drop.
///
/// Use [`ScratchDir::path`] for the directory itself and [`ScratchDir::child`] to
/// build paths inside it; [`ScratchDir::into_path`] transfers ownership without
/// automatic cleanup.
#[derive(Debug)]
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    /// Create a private directory under the OS temp root.
    ///
    /// # Errors
    /// See [`ScratchDir::new_under`].
    pub fn new(label: &str) -> io::Result<Self> {
        Self::new_under(&std::env::temp_dir(), label)
    }

    /// Create a private directory directly under the resolved `base`, creating `base` when absent.
    ///
    /// The name is `<label>-<pid>-<32 hex CSPRNG chars>`; `label` is a diagnostic
    /// tag only, confined to one bounded component (ASCII alphanumerics, `-` and
    /// `_`; others become `_`; at most 64 characters).
    ///
    /// # Errors
    /// `PermissionDenied` with a [`ScratchError`] when `base` (or an ancestor) or
    /// the created directory fails verification, or with a [`ScratchRootRefusal`]
    /// when a non-unix `base` cannot be proven inside the user's profile; the
    /// `getrandom` error when the OS CSPRNG is unavailable; `AlreadyExists`
    /// carrying [`NamesExhausted`] when every fresh name collided; any other I/O
    /// error.
    pub fn new_under(base: &Path, label: &str) -> io::Result<Self> {
        let base = trusted_base(base)?;
        let (path, ()) = create_unique(&base, label, getrandom::fill, exclusive_mkdir)?;
        // A refused entry is not ours to remove.
        verify_private_dir(&path)?;
        Ok(Self(path))
    }

    /// The path of this scratch directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Build a path for a child entry inside this directory (not created by this call).
    #[must_use]
    pub fn child(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// Re-verify that this directory is still a private directory of the effective user.
    ///
    /// # Errors
    /// See [`verify_private_dir`].
    pub fn verify(&self) -> io::Result<()> {
        verify_private_dir(&self.0)
    }

    /// Consume this guard and return the directory path without removing it.
    #[must_use]
    pub fn into_path(self) -> PathBuf {
        let path = self.0.clone();
        std::mem::forget(self);
        path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ── ScratchFile ──────────────────────────────────────────────────────────────

/// A verified private temporary file inside its own private [`ScratchDir`].
///
/// The owned [`File`] handle outlives the name: read back what an external writer
/// wrote through [`ScratchFile::read_all`], never by re-opening the path. The file
/// and its directory are removed on drop.
#[derive(Debug)]
pub struct ScratchFile {
    /// The open handle; [`ScratchFile::rewind`] before reading external writes.
    pub file: File,
    path: PathBuf,
    // Declared last so the handle closes before the directory is removed.
    dir: ScratchDir,
}

impl ScratchFile {
    /// Create a private file inside a fresh private directory under the OS temp root.
    ///
    /// Both the directory and the file are named from `label` as in
    /// [`ScratchDir::new_under`]; the file is the confined label itself.
    ///
    /// # Errors
    /// See [`ScratchDir::new_under`]; also `PermissionDenied` with a
    /// [`ScratchError`] when the opened handle is not a private regular file of the
    /// effective user, and any open error (a planted symlink fails `O_NOFOLLOW`).
    pub fn create(label: &str) -> io::Result<Self> {
        Self::create_in(ScratchDir::new(label)?, &confined_label(label))
    }

    /// Create the file `name` inside the private directory `dir`, taking ownership of it.
    fn create_in(dir: ScratchDir, name: &str) -> io::Result<Self> {
        let path = dir.child(name);
        let file = exclusive_open(&path)?;
        verify_private(&path, &file.metadata()?, EntryKind::File)?;
        dir.verify()?;
        Ok(Self { file, path, dir })
    }

    /// The path of this scratch file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The private directory holding this file.
    #[must_use]
    pub const fn dir(&self) -> &ScratchDir {
        &self.dir
    }

    /// Rewind the retained handle to offset 0.
    ///
    /// # Errors
    /// Propagates any `seek` error.
    pub fn rewind(&mut self) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(0)).map(|_| ())
    }

    /// Read all bytes through the retained handle, from offset 0.
    ///
    /// # Errors
    /// Propagates any seek or read error.
    pub fn read_all(&mut self) -> io::Result<Vec<u8>> {
        self.rewind()?;
        let mut buf = Vec::new();
        self.file.read_to_end(&mut buf)?;
        Ok(buf)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Identity = Identity {
        euid: 1000,
        egid: 1000,
    };

    const fn dir(owner: u32, group: u32, mode: u32) -> EntryFacts {
        EntryFacts {
            kind: EntryKind::Directory,
            owner,
            group,
            mode,
        }
    }

    /// A fresh directory tree unique to this test, removed on drop.
    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> io::Result<Self> {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let root = std::env::temp_dir().join(format!(
                "ipe-private-scratch-{tag}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(root.join("profile").join("temp"))?;
            std::fs::create_dir_all(root.join("shared"))?;
            Ok(Self(root))
        }

        fn profile(&self) -> PathBuf {
            self.0.join("profile")
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn base_verdict_accepts_own_root_and_sticky_bases() {
        assert_eq!(base_verdict(dir(1000, 1000, 0o700), ME), Ok(()));
        assert_eq!(base_verdict(dir(0, 0, 0o755), ME), Ok(()));
        assert_eq!(base_verdict(dir(0, 0, 0o1777), ME), Ok(()));
        assert_eq!(base_verdict(dir(1000, 1000, 0o775), ME), Ok(()));
    }

    #[test]
    fn base_verdict_refuses_foreign_owner() {
        assert_eq!(
            base_verdict(dir(1001, 1001, 0o755), ME),
            Err(ScratchRefusal::ForeignOwner)
        );
    }

    #[test]
    fn base_verdict_refuses_non_sticky_world_or_group_writable() {
        assert_eq!(
            base_verdict(dir(0, 0, 0o777), ME),
            Err(ScratchRefusal::WritableByOthers)
        );
        assert_eq!(
            base_verdict(dir(1000, 1000, 0o777), ME),
            Err(ScratchRefusal::WritableByOthers)
        );
        assert_eq!(
            base_verdict(dir(0, 100, 0o775), ME),
            Err(ScratchRefusal::WritableByOthers)
        );
        assert_eq!(
            base_verdict(dir(1000, 100, 0o775), ME),
            Err(ScratchRefusal::WritableByOthers)
        );
    }

    #[test]
    fn base_verdict_refuses_a_symlink() {
        let link = EntryFacts {
            kind: EntryKind::Other,
            ..dir(1000, 1000, 0o777)
        };
        assert_eq!(base_verdict(link, ME), Err(ScratchRefusal::NotADirectory));
    }

    #[test]
    fn private_verdict_accepts_only_owned_owner_only_entries() {
        assert_eq!(
            private_verdict(dir(1000, 1000, 0o700), EntryKind::Directory, ME),
            Ok(())
        );
        assert_eq!(
            private_verdict(dir(1001, 1000, 0o700), EntryKind::Directory, ME),
            Err(ScratchRefusal::ForeignOwner)
        );
        assert_eq!(
            private_verdict(dir(0, 0, 0o700), EntryKind::Directory, ME),
            Err(ScratchRefusal::ForeignOwner)
        );
        assert_eq!(
            private_verdict(dir(1000, 1000, 0o750), EntryKind::Directory, ME),
            Err(ScratchRefusal::NotPrivate)
        );
        assert_eq!(
            private_verdict(dir(1000, 1000, 0o700), EntryKind::File, ME),
            Err(ScratchRefusal::NotARegularFile)
        );
        let link = EntryFacts {
            kind: EntryKind::Other,
            ..dir(1000, 1000, 0o700)
        };
        assert_eq!(
            private_verdict(link, EntryKind::Directory, ME),
            Err(ScratchRefusal::NotADirectory)
        );
    }

    #[test]
    fn root_inside_or_equal_to_profile_is_accepted() {
        let profile = Path::new("/home/alice");
        assert_eq!(
            root_within_profile(Path::new("/home/alice/AppData/Local/Temp"), profile),
            Ok(())
        );
        assert_eq!(root_within_profile(profile, profile), Ok(()));
    }

    #[test]
    fn root_outside_profile_is_refused() {
        let refused = root_within_profile(Path::new("/tmp"), Path::new("/home/alice"));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
    }

    #[test]
    fn sibling_sharing_a_name_prefix_is_refused() {
        let refused =
            root_within_profile(Path::new("/home/alice-shared"), Path::new("/home/alice"));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
    }

    #[test]
    fn parent_dir_escape_is_refused() {
        let refused =
            root_within_profile(Path::new("/home/alice/../bob"), Path::new("/home/alice"));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
    }

    #[test]
    fn relative_root_is_refused() {
        let refused = root_within_profile(Path::new("alice/temp"), Path::new("/home/alice"));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
    }

    #[test]
    fn relative_profile_proves_nothing() {
        assert_eq!(
            root_within_profile(Path::new("/home/alice/temp"), Path::new("alice")),
            Err(ScratchRootRefusal::NoProfile)
        );
    }

    #[test]
    fn filesystem_root_profile_proves_nothing() {
        assert_eq!(
            root_within_profile(Path::new("/tmp"), Path::new("/")),
            Err(ScratchRootRefusal::NoProfile)
        );
    }

    #[test]
    fn unset_empty_or_relative_profile_variable_is_refused() -> io::Result<()> {
        let tree = Tree::new("noprofile")?;
        let root = tree.profile().join("temp");
        for profile in [
            None,
            Some(OsStr::new("")),
            Some(OsStr::new("relative/profile")),
        ] {
            assert_eq!(
                verify_root_within_profile(&root, profile),
                Err(ScratchRootRefusal::NoProfile)
            );
        }
        Ok(())
    }

    #[test]
    fn resolved_root_inside_profile_is_accepted() -> io::Result<()> {
        let tree = Tree::new("inside")?;
        let profile = tree.profile();
        let resolved = std::fs::canonicalize(profile.join("temp"))?;
        assert_eq!(
            verify_root_within_profile(&profile.join("temp"), Some(profile.as_os_str())),
            Ok(resolved)
        );
        Ok(())
    }

    /// The accepted root is returned resolved, so creation lands under the
    /// checked location even when the given path runs through a link.
    #[cfg(unix)]
    #[test]
    fn accepted_root_is_returned_resolved() -> io::Result<()> {
        let tree = Tree::new("resolved")?;
        let profile = tree.profile();
        let link = profile.join("temp-link");
        std::os::unix::fs::symlink(profile.join("temp"), &link)?;
        let resolved = verify_root_within_profile(&link, Some(profile.as_os_str()));
        assert_eq!(resolved, Ok(std::fs::canonicalize(profile.join("temp"))?));
        Ok(())
    }

    #[test]
    fn shared_root_outside_profile_is_refused() -> io::Result<()> {
        let tree = Tree::new("shared")?;
        let profile = tree.profile();
        let refused = verify_root_within_profile(&tree.0.join("shared"), Some(profile.as_os_str()));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
        Ok(())
    }

    #[test]
    fn dot_dot_path_leaving_profile_is_refused() -> io::Result<()> {
        let tree = Tree::new("dotdot")?;
        let profile = tree.profile();
        let escaping = profile.join("temp").join("..").join("..").join("shared");
        let refused = verify_root_within_profile(&escaping, Some(profile.as_os_str()));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn link_inside_profile_pointing_outside_is_refused() -> io::Result<()> {
        let tree = Tree::new("link")?;
        let profile = tree.profile();
        let link = profile.join("temp-link");
        std::os::unix::fs::symlink(tree.0.join("shared"), &link)?;
        let refused = verify_root_within_profile(&link, Some(profile.as_os_str()));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
        Ok(())
    }

    #[test]
    fn missing_root_is_refused() -> io::Result<()> {
        let tree = Tree::new("missing")?;
        let profile = tree.profile();
        let refused =
            verify_root_within_profile(&profile.join("absent"), Some(profile.as_os_str()));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::Unresolvable { .. })
        ));
        Ok(())
    }

    #[test]
    fn every_root_refusal_names_the_fix_and_denies_permission() {
        let refusals = [
            ScratchRootRefusal::NoProfile,
            ScratchRootRefusal::Unresolvable {
                path: PathBuf::from("/x"),
                detail: "not found".to_owned(),
            },
            ScratchRootRefusal::OutsideProfile {
                root: PathBuf::from("/tmp"),
                profile: PathBuf::from("/home/alice"),
            },
        ];
        for refusal in refusals {
            assert!(
                refusal.to_string().contains("set TEMP and TMP"),
                "{refusal}"
            );
            assert_eq!(
                io::Error::from(refusal).kind(),
                io::ErrorKind::PermissionDenied
            );
        }
    }

    /// A base outside the user's profile is refused before anything is created.
    #[cfg(windows)]
    #[test]
    fn windows_base_outside_profile_is_refused() {
        let system_root = ipe_env::var_os("SystemRoot");
        assert!(system_root.is_some(), "SystemRoot must be set on Windows");
        let Some(system_root) = system_root else {
            return;
        };
        let refused = ScratchDir::new_under(Path::new(&system_root), "ipe-test");
        assert_eq!(
            refused.map(drop).map_err(|e| e.kind()),
            Err(io::ErrorKind::PermissionDenied)
        );
    }

    /// An unavailable CSPRNG fails the creation before any entry is attempted;
    /// no weaker name is ever produced.
    #[test]
    fn entropy_failure_fails_closed() -> io::Result<()> {
        let tree = Tree::new("entropy")?;
        let mut attempts = 0usize;
        let result = create_unique(
            &tree.0,
            "ipe-test",
            |_: &mut [u8]| Err(getrandom::Error::UNSUPPORTED),
            |p: &Path| {
                attempts += 1;
                exclusive_mkdir(p)
            },
        );
        assert!(result.is_err());
        assert_eq!(attempts, 0);
        assert_eq!(
            std::fs::read_dir(&tree.0)?.count(),
            2,
            "only the fixture tree"
        );
        Ok(())
    }

    /// Fixed entropy collides on every attempt after the first entry, and the
    /// retry loop stops at its bound with `AlreadyExists`.
    #[test]
    fn collisions_stop_at_the_attempt_bound() -> io::Result<()> {
        let tree = Tree::new("collide")?;
        let zeros = |buf: &mut [u8]| {
            buf.fill(0);
            Ok::<(), getrandom::Error>(())
        };
        create_unique(&tree.0, "ipe-test", zeros, exclusive_mkdir)?;
        let mut attempts = 0usize;
        let second = create_unique(&tree.0, "ipe-test", zeros, |p: &Path| {
            attempts += 1;
            exclusive_mkdir(p)
        });
        assert!(second.is_err(), "every name collides");
        let Err(err) = second else {
            return Ok(());
        };
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(err.to_string().contains("unique scratch entry"), "{err}");
        assert_eq!(attempts, MAX_ATTEMPTS);
        Ok(())
    }

    /// A hostile label cannot add a path component, reach an alternate data
    /// stream, or grow the name without bound.
    #[test]
    fn label_is_confined_to_one_bounded_component() -> io::Result<()> {
        let mut fill = |buf: &mut [u8]| {
            buf.fill(0xab);
            Ok::<(), getrandom::Error>(())
        };
        let hostile = format!("../../etc/x:y\\z{}", "a".repeat(500));
        let name = scratch_name(&hostile, &mut fill)?;
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{name}"
        );
        assert!(name.starts_with("______etc_x_y_z"), "{name}");
        assert!(name.ends_with(&"ab".repeat(ENTROPY_BYTES)), "{name}");
        assert!(name.len() <= MAX_LABEL_CHARS + 2 + 10 + 2 * ENTROPY_BYTES);
        Ok(())
    }

    /// Every hostile or empty label yields exactly one entry directly under the resolved base.
    #[test]
    fn hostile_label_creates_one_entry_directly_under_the_base() -> io::Result<()> {
        let root = ScratchDir::new("ipe-scratch-label")?;
        let base = std::fs::canonicalize(root.path())?;
        let mut kept = Vec::new();
        for bad in ["", "a/b", "../x", "a\0b", ".", ".."] {
            let sd = ScratchDir::new_under(root.path(), bad)?;
            assert_eq!(sd.path().parent(), Some(base.as_path()), "label {bad:?}");
            kept.push(sd);
        }
        let sf = ScratchFile::create("../x")?;
        assert_eq!(sf.path().parent(), Some(sf.dir().path()));
        assert_eq!(std::fs::read_dir(root.path())?.count(), kept.len());
        Ok(())
    }

    #[test]
    fn unique_entries_differ_and_live_under_base() -> io::Result<()> {
        let tree = Tree::new("unique")?;
        let base = std::fs::canonicalize(&tree.0)?;
        let a = ScratchDir::new_under(&tree.0, "ipe-test")?;
        let b = ScratchDir::new_under(&tree.0, "ipe-test")?;
        let c = ScratchFile::create_in(ScratchDir::new_under(&tree.0, "ipe-test")?, "f")?;
        assert_ne!(a.path(), b.path());
        for entry in [a.path(), b.path(), c.dir().path()] {
            assert_eq!(entry.parent(), Some(base.as_path()));
        }
        Ok(())
    }

    #[test]
    fn scratch_dir_raii_cleanup() -> io::Result<()> {
        let path = {
            let sd = ScratchDir::new("ipe-scratch-raii")?;
            assert!(sd.path().is_dir());
            sd.path().to_path_buf()
        };
        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn scratch_file_lives_in_its_own_private_dir_and_is_removed() -> io::Result<()> {
        let (file_path, dir_path) = {
            let sf = ScratchFile::create("ipe-scratch-file")?;
            assert_eq!(sf.path().parent(), Some(sf.dir().path()));
            sf.dir().verify()?;
            (sf.path().to_path_buf(), sf.dir().path().to_path_buf())
        };
        assert!(!file_path.exists());
        assert!(!dir_path.exists());
        Ok(())
    }

    #[cfg(unix)]
    mod unix {
        use super::super::*;
        use super::Tree;
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        }

        #[test]
        fn created_dir_is_0700_and_owned_by_the_effective_user() -> io::Result<()> {
            let sd = ScratchDir::new("ipe-scratch-mode")?;
            let meta = std::fs::symlink_metadata(sd.path())?;
            assert!(meta.file_type().is_dir());
            assert_eq!(meta.mode() & 0o777, 0o700);
            assert_eq!(meta.uid(), rustix::process::geteuid().as_raw());
            Ok(())
        }

        #[test]
        fn created_file_is_0600() -> io::Result<()> {
            let sf = ScratchFile::create("ipe-scratch-fmode")?;
            let meta = std::fs::symlink_metadata(sf.path())?;
            assert!(meta.file_type().is_file());
            assert_eq!(meta.mode() & 0o777, 0o600);
            Ok(())
        }

        /// Entries are owner-only and their exclusive creators refuse an existing name.
        #[test]
        fn unix_entries_are_owner_only() -> io::Result<()> {
            let tree = Tree::new("mode")?;
            let dir = ScratchDir::new_under(&tree.0, "ipe-test")?;
            let file = ScratchFile::create_in(ScratchDir::new_under(&tree.0, "ipe-test")?, "f")?;
            let mode = |p: &Path| std::fs::metadata(p).map(|m| m.permissions().mode() & 0o777);
            assert_eq!(mode(dir.path())? & 0o077, 0);
            assert_eq!(mode(file.path())? & 0o077, 0);
            assert_eq!(
                exclusive_mkdir(dir.path()).map_err(|e| e.kind()),
                Err(io::ErrorKind::AlreadyExists)
            );
            assert_eq!(
                exclusive_open(file.path()).map(drop).map_err(|e| e.kind()),
                Err(io::ErrorKind::AlreadyExists)
            );
            Ok(())
        }

        #[test]
        fn symlinked_private_dir_is_refused_and_target_untouched() -> io::Result<()> {
            let root = ScratchDir::new("ipe-scratch-symdir")?;
            let target = root.child("target");
            exclusive_mkdir(&target)?;
            std::fs::write(target.join("canary"), b"canary")?;
            let link = root.child("link");
            std::os::unix::fs::symlink(&target, &link)?;

            let err = verify_private_dir(&link).err();
            assert_eq!(
                err.as_ref().map(io::Error::kind),
                Some(io::ErrorKind::PermissionDenied)
            );
            assert_eq!(std::fs::read(target.join("canary"))?, b"canary");
            Ok(())
        }

        #[test]
        fn group_or_world_accessible_private_dir_is_refused() -> io::Result<()> {
            let root = ScratchDir::new("ipe-scratch-open")?;
            let open = root.child("open");
            exclusive_mkdir(&open)?;
            for mode in [0o777, 0o750, 0o705] {
                set_mode(&open, mode)?;
                let err = verify_private_dir(&open).err();
                assert_eq!(
                    err.as_ref().map(io::Error::kind),
                    Some(io::ErrorKind::PermissionDenied),
                    "mode {mode:o} must be refused"
                );
            }
            Ok(())
        }

        #[test]
        fn non_sticky_world_writable_base_is_refused_and_left_empty() -> io::Result<()> {
            let root = ScratchDir::new("ipe-scratch-wbase")?;
            let base = root.child("base");
            exclusive_mkdir(&base)?;
            set_mode(&base, 0o777)?;
            let err = ScratchDir::new_under(&base, "ipe-under").err();
            assert_eq!(
                err.as_ref().map(io::Error::kind),
                Some(io::ErrorKind::PermissionDenied)
            );
            assert_eq!(std::fs::read_dir(&base)?.count(), 0);
            Ok(())
        }

        #[test]
        fn sticky_world_writable_base_is_accepted() -> io::Result<()> {
            let root = ScratchDir::new("ipe-scratch-sbase")?;
            let base = root.child("base");
            exclusive_mkdir(&base)?;
            set_mode(&base, 0o1777)?;
            let sd = ScratchDir::new_under(&base, "ipe-under")?;
            sd.verify()?;
            Ok(())
        }

        #[test]
        fn missing_base_components_are_created_private() -> io::Result<()> {
            let root = ScratchDir::new("ipe-scratch-mkbase")?;
            let outer = root.child("outer");
            let base = outer.join("inner");
            let sd = ScratchDir::new_under(&base, "ipe-under")?;
            sd.verify()?;
            for created in [&outer, &base] {
                let mode = std::fs::symlink_metadata(created)?.permissions().mode();
                assert_eq!(
                    mode & 0o7777,
                    0o700,
                    "{} must be created 0700",
                    created.display()
                );
            }
            Ok(())
        }

        #[test]
        fn base_reached_through_a_symlink_is_canonicalised() -> io::Result<()> {
            let root = ScratchDir::new("ipe-scratch-lbase")?;
            let real = root.child("real");
            exclusive_mkdir(&real)?;
            let link = root.child("link");
            std::os::unix::fs::symlink(&real, &link)?;
            let sd = ScratchDir::new_under(&link, "ipe-under")?;
            assert_eq!(
                sd.path().parent(),
                Some(std::fs::canonicalize(&real)?.as_path())
            );
            Ok(())
        }

        #[test]
        fn preplanted_symlink_at_the_file_path_is_refused_and_target_untouched() -> io::Result<()> {
            let root = ScratchDir::new("ipe-scratch-symfile")?;
            let canary = root.child("canary");
            std::fs::write(&canary, b"canary")?;
            let dir = ScratchDir::new_under(root.path(), "ipe-private")?;
            std::os::unix::fs::symlink(&canary, dir.child("tag"))?;

            let err = ScratchFile::create_in(dir, "tag").err();
            assert!(err.is_some(), "a planted symlink must not be opened");
            assert_eq!(std::fs::read(&canary)?, b"canary");
            Ok(())
        }

        #[test]
        fn dangling_symlink_at_the_file_path_creates_nothing() -> io::Result<()> {
            let root = ScratchDir::new("ipe-scratch-dangle")?;
            let victim = root.child("victim");
            let dir = ScratchDir::new_under(root.path(), "ipe-private")?;
            std::os::unix::fs::symlink(&victim, dir.child("tag"))?;

            assert!(ScratchFile::create_in(dir, "tag").is_err());
            assert!(std::fs::symlink_metadata(&victim).is_err());
            Ok(())
        }
    }
}

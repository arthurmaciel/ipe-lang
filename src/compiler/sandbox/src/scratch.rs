//! Private scratch directories and files, the one primitive every temporary write goes through.
//!
//! A scratch path is handed to external writers (`curl -o`, `sh`, the jail) that
//! re-resolve it by name, so it is only safe while no other user can replace any
//! component of it. Every constructor therefore establishes:
//!
//! - the base is canonicalised and every ancestor is a real directory owned by
//!   the effective user or root, writable by no one else unless sticky;
//! - the private directory is created exclusively (mode 0700, 128-bit entropy
//!   name) and re-verified with `symlink_metadata`: a real directory, owned by
//!   the effective user, no group/other permission bits;
//! - a scratch file is created inside such a directory with `O_EXCL` +
//!   `O_NOFOLLOW` + mode 0600, and its handle is verified with `fstat`.
//!
//! A check that fails refuses with [`io::ErrorKind::PermissionDenied`] carrying
//! a [`ScratchError`]; nothing is created under an untrusted base and nothing is
//! written through a planted link.

use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Maximum retry attempts when an exclusive-create collision occurs.
const MAX_RETRIES: usize = 8;

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
    /// The caller-supplied name prefix is empty or contains a path separator.
    InvalidPrefix,
}

impl fmt::Display for ScratchRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotADirectory => "not a real directory",
            Self::NotARegularFile => "not a regular file",
            Self::ForeignOwner => "owned by another user",
            Self::WritableByOthers => "writable by other users without the sticky bit",
            Self::NotPrivate => "grants group or other permissions",
            Self::InvalidPrefix => "scratch prefix is empty or contains a path separator",
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
        (_, EntryKind::File) => return Err(ScratchRefusal::NotARegularFile),
        _ => return Err(ScratchRefusal::NotADirectory),
    }
    if entry.owner != who.euid {
        return Err(ScratchRefusal::ForeignOwner);
    }
    if entry.mode & GROUP_OTHER_BITS != 0 {
        return Err(ScratchRefusal::NotPrivate);
    }
    Ok(())
}

/// Whether `prefix` is usable as a single path component label.
///
/// # Errors
/// [`ScratchRefusal::InvalidPrefix`] when it is empty or contains a separator or NUL.
fn prefix_verdict(prefix: &str) -> Result<(), ScratchRefusal> {
    if prefix.is_empty()
        || prefix
            .chars()
            .any(|c| std::path::is_separator(c) || c == '\0')
    {
        return Err(ScratchRefusal::InvalidPrefix);
    }
    Ok(())
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

/// Resolve `base` to the directory private entries are created under.
///
/// Non-unix hosts carry no POSIX owner or mode bits; the base is created and used
/// as given (a canonical Windows path is a verbatim `\\?\` path not every consumer
/// accepts).
#[cfg(not(unix))]
fn trusted_base(base: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(base)?;
    Ok(base.to_path_buf())
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

/// Read 16 bytes (128 bits) of OS entropy.
#[cfg(unix)]
fn read_entropy() -> io::Result<[u8; 16]> {
    let mut buf = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf)
}

/// Read 16 bytes of per-process randomised hash state as the name entropy.
///
/// Non-unix hosts have no `/dev/urandom`; `RandomState` is seeded from the OS
/// generator. A guessed name only costs a retry: creation is exclusive either way.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)] // one signature with the fallible unix reader
fn read_entropy() -> io::Result<[u8; 16]> {
    use std::hash::{BuildHasher as _, Hasher as _};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0u128, |d| d.as_nanos());
    let half = |salt: u8| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u32(std::process::id());
        h.write_u8(salt);
        h.finish()
    };
    Ok(((u128::from(half(1)) << 64) | u128::from(half(0))).to_le_bytes())
}

/// One candidate name: `<prefix>-<pid>-<32 hex entropy chars>`.
fn candidate_name(prefix: &str) -> io::Result<String> {
    use std::fmt::Write as _;
    let mut name = format!("{prefix}-{}-", std::process::id());
    for b in read_entropy()? {
        let _ = write!(name, "{b:02x}");
    }
    Ok(name)
}

#[cfg(unix)]
fn exclusive_mkdir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(false)
        .create(path)
}

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
    pub fn new(prefix: &str) -> io::Result<Self> {
        Self::new_under(&std::env::temp_dir(), prefix)
    }

    /// Create a private directory under `base`, creating `base` when absent.
    ///
    /// `prefix` is a diagnostic label for the name; the rest of the name is
    /// 128 bits of entropy.
    ///
    /// # Errors
    /// `InvalidInput` for an unusable prefix; `PermissionDenied` with a
    /// [`ScratchError`] when `base` (or an ancestor) or the created directory fails
    /// verification; `AlreadyExists` when every attempt collides; any other I/O error.
    pub fn new_under(base: &Path, prefix: &str) -> io::Result<Self> {
        prefix_verdict(prefix).map_err(|r| io::Error::new(io::ErrorKind::InvalidInput, r))?;
        let base = trusted_base(base)?;
        for _ in 0..MAX_RETRIES {
            let path = base.join(candidate_name(prefix)?);
            match exclusive_mkdir(&path) {
                Ok(()) => {
                    // A refused entry is not ours to remove.
                    verify_private_dir(&path)?;
                    return Ok(Self(path));
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique scratch directory after repeated attempts",
        ))
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
    /// Create a private file named `prefix` inside a fresh private directory under the OS temp root.
    ///
    /// # Errors
    /// See [`ScratchDir::new_under`]; also `PermissionDenied` with a
    /// [`ScratchError`] when the opened handle is not a private regular file of the
    /// effective user, and any open error (a planted symlink fails `O_NOFOLLOW`).
    pub fn create(prefix: &str) -> io::Result<Self> {
        Self::create_in(ScratchDir::new(prefix)?, prefix)
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
    fn prefix_with_a_separator_is_refused() -> io::Result<()> {
        let root = ScratchDir::new("ipe-scratch-prefix")?;
        for bad in ["", "a/b", "../x"] {
            let err = ScratchDir::new_under(root.path(), bad).err();
            assert_eq!(
                err.as_ref().map(io::Error::kind),
                Some(io::ErrorKind::InvalidInput),
                "prefix {bad:?} must be refused"
            );
        }
        assert_eq!(std::fs::read_dir(root.path())?.count(), 0);
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

//! Files a secret is kept in: proven owner-only on the open handle, or refused.
//!
//! Every credential ipe keeps on disk (the publish token, the signing key's
//! private half, the per-user cache salt) is created, staged, housed and read
//! back through this module alone, relative to an [`OwnerDir`]: a directory
//! handle held open once every component of its path, ancestors included,
//! was proven on its own handle unwritable by other users
//! ([`crate::proven_dir`]). No secret path is resolved again after that proof,
//! so swapping a component for a link between the check and the use redirects
//! nothing.
//!
//! A creation mode is only a request, which a filesystem that ignores
//! permission bits (vfat, exfat, some network and FUSE mounts) silently drops;
//! so every file handle this module hands out has been checked by `fstat` to
//! be a regular file owned by the effective user with no group or other
//! permission bit. A handle failing that check is never returned: a file this
//! module created is removed, and a file it was asked to read is refused.

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use crate::proven_dir::{EntryName, NewFileMode, ProvenDir, ProvenDirError};

/// Where this host can keep a secret file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretStore {
    /// A file whose owner-only access is checked on its open handle.
    OwnerOnlyFile,
    /// No owner-only check exists, so no secret is kept in a file.
    Unsupported,
}

/// The secret store this build's target provides.
#[cfg(unix)]
pub const HOST_SECRET_STORE: SecretStore = SecretStore::OwnerOnlyFile;

/// The secret store this build's target provides.
///
/// Windows grants file access by ACL, not by an owner and mode bits, so no
/// owner-only proof exists there and every secret file is refused.
#[cfg(not(unix))]
pub const HOST_SECRET_STORE: SecretStore = SecretStore::Unsupported;

/// Why a secret file was not created, housed or read.
#[derive(Debug)]
pub enum SecretFileError {
    /// The store cannot keep a file readable by its owner alone.
    Unsupported,
    /// Creating, opening or inspecting the file failed (its name is taken, say).
    Io(io::Error),
    /// The path is not private to the invoking user.
    ///
    /// Another user owns it or an ancestor, a group or other user can write
    /// it or an ancestor, a link stands where a directory must be, or its
    /// filesystem dropped the owner-only mode it was created with.
    NotOwnerOnly(PathBuf),
    /// The path names a directory, FIFO, device or socket, not a regular file.
    NotRegularFile(PathBuf),
}

impl From<ProvenDirError> for SecretFileError {
    fn from(refusal: ProvenDirError) -> Self {
        match refusal {
            ProvenDirError::Unsupported => Self::Unsupported,
            ProvenDirError::Untrusted { path, .. }
            | ProvenDirError::UntrustedLink(path)
            | ProvenDirError::SymlinkLeaf(path) => Self::NotOwnerOnly(path),
            other @ (ProvenDirError::NotAbsolute(_)
            | ProvenDirError::Absent(_)
            | ProvenDirError::NotADirectory(_)
            | ProvenDirError::TooManyLinks(_)
            | ProvenDirError::TooDeep(_)
            | ProvenDirError::Io { .. }) => Self::Io(other.into_io()),
        }
    }
}

/// The permission bits granting the owning group or any other user access.
const GROUP_OR_OTHER_ACCESS: u32 = 0o077;

/// Whether `mode` and owner `uid` keep a file private to the effective user `euid`.
///
/// Private means owned by `euid` with no group or other permission bit set;
/// the file-type bits of `mode` are ignored.
#[must_use]
pub const fn is_owner_only(mode: u32, uid: u32, euid: u32) -> bool {
    uid == euid && mode & GROUP_OR_OTHER_ACCESS == 0
}

/// Refuse a `store` that cannot keep a secret file owner-only.
///
/// A caller checks this before any work a refusal would waste, such as
/// asking for a grant or generating a key.
///
/// # Errors
/// [`SecretFileError::Unsupported`] when `store` is [`SecretStore::Unsupported`].
pub const fn require(store: SecretStore) -> Result<(), SecretFileError> {
    match store {
        SecretStore::OwnerOnlyFile => Ok(()),
        SecretStore::Unsupported => Err(SecretFileError::Unsupported),
    }
}

/// A random, per-process suffix naming the temporary files of one secret write.
///
/// Only [`Self::fresh`] makes one, so a temporary name is never guessable in
/// advance by another local user who would pre-seed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempSuffix(String);

/// The random bytes in a [`TempSuffix`].
const TEMP_SUFFIX_BYTES: usize = 8;

impl TempSuffix {
    /// A suffix of this process id and fresh bytes from the OS CSPRNG.
    ///
    /// # Errors
    /// The CSPRNG failure, as an I/O error.
    pub fn fresh() -> io::Result<Self> {
        let mut bytes = [0u8; TEMP_SUFFIX_BYTES];
        getrandom::fill(&mut bytes).map_err(io::Error::from)?;
        Ok(Self(format!(
            "{}.{}",
            std::process::id(),
            hex::encode(bytes)
        )))
    }

    /// The hidden temporary name `.<name>.<suffix>.tmp` beside `final_name`.
    ///
    /// # Errors
    /// An [`io::ErrorKind::InvalidInput`] error if the name is not one component.
    pub fn name_for(&self, final_name: &EntryName) -> io::Result<EntryName> {
        let mut name = OsString::from(".");
        name.push(final_name.as_os_str());
        name.push(format!(".{}.tmp", self.0));
        EntryName::new(&name).ok_or_else(|| io::ErrorKind::InvalidInput.into())
    }
}

/// A directory housing secret files, held open once proven private to the invoker.
///
/// Every ancestor passed [`crate::owner_trust::container_breach`] and the
/// directory itself [`crate::owner_trust::breach`], each on its own held
/// handle; every operation acts on an entry directly inside it through that
/// handle.
#[derive(Debug)]
pub struct OwnerDir(ProvenDir);

impl OwnerDir {
    /// The path the directory was asked for.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.0.path()
    }

    /// The path of the entry `name` inside this directory, for diagnostics.
    #[must_use]
    pub fn path_of(&self, name: &EntryName) -> PathBuf {
        self.0.path_of(name)
    }

    /// Create the new file `name` owner-only, proven so before any byte is written.
    ///
    /// The name is created exclusively: an existing file or symlink there,
    /// even a dangling one, is refused, never truncated or followed. A created
    /// file whose handle fails the owner-only check is removed again.
    ///
    /// # Errors
    /// [`SecretFileError::Io`] when creating it failed, or
    /// [`SecretFileError::NotOwnerOnly`] when the created file is not private.
    pub fn create_new(&self, name: &EntryName) -> Result<File, SecretFileError> {
        self.create_proven(name, host::prove_owner_only)
    }

    /// Create `name` exclusively mode `0600`, then keep it only if `prove` admits its handle.
    fn create_proven(
        &self,
        name: &EntryName,
        prove: impl FnOnce(&File, &Path) -> Result<(), SecretFileError>,
    ) -> Result<File, SecretFileError> {
        let file = self
            .0
            .create_file(name, NewFileMode::OwnerOnly)
            .map_err(SecretFileError::Io)?;
        if let Err(refusal) = prove(&file, &self.path_of(name)) {
            drop(file);
            let _ = self.0.remove_file(name);
            return Err(refusal);
        }
        Ok(file)
    }

    /// Create the temporary file staging a secret for `final_name`, owner-only.
    ///
    /// The file is created by [`Self::create_new`] at [`TempSuffix::name_for`],
    /// in this directory, so a rename or link into place is atomic.
    ///
    /// # Errors
    /// As [`Self::create_new`].
    pub fn create_temp_for(
        &self,
        final_name: &EntryName,
        suffix: &TempSuffix,
    ) -> Result<(File, EntryName), SecretFileError> {
        let temp = suffix.name_for(final_name).map_err(SecretFileError::Io)?;
        self.create_new(&temp).map(|file| (file, temp))
    }

    /// Create the new, non-secret file `name` world-readable, exclusively.
    ///
    /// # Errors
    /// The failed create; an existing name or symlink is refused, never followed.
    pub fn create_public(&self, name: &EntryName) -> io::Result<File> {
        self.0.create_file(name, NewFileMode::WorldReadable)
    }

    /// Open the existing secret `name` for reading, proven owner-only on the open handle.
    ///
    /// A symlink is refused, never followed, and opening never blocks on a
    /// FIFO. A file another user owns, or any group or other user may access,
    /// is refused.
    ///
    /// # Errors
    /// [`SecretFileError::Io`] when the file cannot be opened (absent, a
    /// symlink, unreadable), [`SecretFileError::NotRegularFile`] when it is
    /// not a regular file, or [`SecretFileError::NotOwnerOnly`] when it is not
    /// private.
    pub fn open_existing(&self, name: &EntryName) -> Result<File, SecretFileError> {
        let file = self.0.open_file(name).map_err(SecretFileError::Io)?;
        host::prove_owner_only(&file, &self.path_of(name))?;
        Ok(file)
    }

    /// Rename the entry `from` to `to`, replacing any file at `to`.
    ///
    /// # Errors
    /// The failed `renameat`.
    pub fn rename(&self, from: &EntryName, to: &EntryName) -> io::Result<()> {
        self.0.rename(from, to)
    }

    /// Hard-link the entry `from` to the unused name `to`.
    ///
    /// # Errors
    /// The failed `linkat`; an existing `to` is refused, never replaced.
    pub fn hard_link(&self, from: &EntryName, to: &EntryName) -> io::Result<()> {
        self.0.hard_link(from, to)
    }

    /// Remove the non-directory entry `name`.
    ///
    /// # Errors
    /// The failed `unlinkat`.
    pub fn remove(&self, name: &EntryName) -> io::Result<()> {
        self.0.remove_file(name)
    }

    /// Whether nothing, not even a dangling link, holds the name `name`.
    ///
    /// # Errors
    /// The failed `fstatat`, other than the entry being absent.
    pub fn is_vacant(&self, name: &EntryName) -> io::Result<bool> {
        self.0.is_vacant(name)
    }
}

/// Hold the directory `dir` that houses secret files, creating it, once proven private.
///
/// Missing components are created mode `0700`. Every component is opened
/// without following a final link and checked on its held handle: an
/// ancestor must be owned by root or the invoker and unwritable by others
/// unless sticky, and `dir` itself must be the invoker's with no other user
/// able to write it (write for the invoker's own effective group is admitted
/// on the terms of [`crate::owner_trust::breach`]).
///
/// # Errors
/// [`SecretFileError::Unsupported`] when `store` cannot keep secrets,
/// [`SecretFileError::Io`] when creating or inspecting a component failed, or
/// [`SecretFileError::NotOwnerOnly`] naming the component another user could
/// write or replace.
pub fn create_owner_dir(store: SecretStore, dir: &Path) -> Result<OwnerDir, SecretFileError> {
    require(store)?;
    Ok(OwnerDir(ProvenDir::create(dir)?))
}

/// Hold the existing directory `dir` that houses secret files, once proven private.
///
/// As [`create_owner_dir`], except that nothing is created: an absent
/// component is an [`io::ErrorKind::NotFound`] error.
///
/// # Errors
/// As [`create_owner_dir`].
pub fn open_owner_dir(store: SecretStore, dir: &Path) -> Result<OwnerDir, SecretFileError> {
    require(store)?;
    Ok(OwnerDir(ProvenDir::open(dir)?))
}

/// Split `path` into its directory and its final entry name.
///
/// # Errors
/// An [`io::ErrorKind::InvalidInput`] error when `path` has no plain final component.
pub fn split_entry(path: &Path) -> Result<(&Path, EntryName), SecretFileError> {
    let invalid = || SecretFileError::Io(io::ErrorKind::InvalidInput.into());
    let name = path
        .file_name()
        .and_then(EntryName::new)
        .ok_or_else(invalid)?;
    let dir = path.parent().ok_or_else(invalid)?;
    Ok((dir, name))
}

/// Open the existing secret file `path` for reading, through its proven directory.
///
/// The directory is held by [`open_owner_dir`] and the file opened inside it
/// by [`OwnerDir::open_existing`].
///
/// # Errors
/// As [`open_owner_dir`] and [`OwnerDir::open_existing`].
pub fn open_existing(store: SecretStore, path: &Path) -> Result<File, SecretFileError> {
    require(store)?;
    let (dir, name) = split_entry(path)?;
    open_owner_dir(store, dir)?.open_existing(&name)
}

/// The owner-only proof of a host with Unix permissions.
#[cfg(unix)]
mod host {
    use std::fs::File;
    use std::os::unix::fs::MetadataExt as _;
    use std::path::Path;

    use super::{SecretFileError, is_owner_only};
    use crate::owner_trust::Invoker;

    /// Refuse the open `file` at `path` unless it is a regular file private to the invoker.
    pub fn prove_owner_only(file: &File, path: &Path) -> Result<(), SecretFileError> {
        prove_owner_only_as(file, path, Invoker::current().uid)
    }

    /// Refuse the open `file` at `path` unless it is a regular file private to `euid`.
    pub fn prove_owner_only_as(file: &File, path: &Path, euid: u32) -> Result<(), SecretFileError> {
        let meta = file.metadata().map_err(SecretFileError::Io)?;
        if !meta.file_type().is_file() {
            return Err(SecretFileError::NotRegularFile(path.to_path_buf()));
        }
        if is_owner_only(meta.mode(), meta.uid(), euid) {
            Ok(())
        } else {
            Err(SecretFileError::NotOwnerOnly(path.to_path_buf()))
        }
    }
}

/// A host with no owner-only check, where no file is ever proven private.
#[cfg(not(unix))]
mod host {
    use std::fs::File;
    use std::path::Path;

    use super::SecretFileError;

    /// Refuse: no file can be proven owner-only here.
    pub const fn prove_owner_only(_file: &File, _path: &Path) -> Result<(), SecretFileError> {
        Err(SecretFileError::Unsupported)
    }
}

#[cfg(test)]
mod tests;

//! Windows directory primitives: path acts pinned under a held, reparse-free handle.
//!
//! Windows offers no handle-relative open without raw system calls, so every
//! act names one entry of a held directory by the directory's real path. That
//! path is proven, level by level from the volume root, to cross no reparse
//! point (symbolic link, junction, mount point), and each held handle denies
//! delete sharing: while it is open neither it nor any directory above it can
//! be renamed or removed, so the proven path keeps naming it. Every entry is
//! opened with `FILE_FLAG_OPEN_REPARSE_POINT`, so a reparse point planted at the
//! final component is refused or removed as itself, never traversed.

use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Component, Path, PathBuf};

use super::{DirId, EntryKind};

/// `FILE_FLAG_BACKUP_SEMANTICS`: allows opening a directory handle.
const BACKUP_SEMANTICS: u32 = 0x0200_0000;
/// `FILE_FLAG_OPEN_REPARSE_POINT`: opens a reparse point itself, never its target.
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
/// `FILE_SHARE_READ | FILE_SHARE_WRITE`, without `FILE_SHARE_DELETE`.
const SHARE_NO_DELETE: u32 = 0x1 | 0x2;
/// `FILE_ATTRIBUTE_DIRECTORY`.
const ATTR_DIRECTORY: u32 = 0x10;
/// `FILE_ATTRIBUTE_REPARSE_POINT`.
const ATTR_REPARSE_POINT: u32 = 0x400;

/// A held directory handle and the reparse-free path proven to name it.
#[derive(Debug)]
pub struct Dir {
    file: File,
    real: PathBuf,
}

/// The error for a reparse point met where a plain entry was required.
fn reparse_point(path: &Path) -> io::Error {
    io::Error::other(format!("{} is a reparse point", path.display()))
}

/// Open `real` as a directory handle, never through a reparse point at its final component.
fn open_dir_at(real: PathBuf) -> io::Result<Dir> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(SHARE_NO_DELETE)
        .custom_flags(BACKUP_SEMANTICS | OPEN_REPARSE_POINT)
        .open(&real)?;
    let attributes = file.metadata()?.file_attributes();
    if attributes & ATTR_REPARSE_POINT != 0 {
        return Err(reparse_point(&real));
    }
    if attributes & ATTR_DIRECTORY == 0 {
        return Err(io::ErrorKind::NotADirectory.into());
    }
    Ok(Dir { file, real })
}

/// Open `path` as a directory, following links on the way.
///
/// The canonical path is walked again from the volume root, each level held
/// and opened without following a reparse point, so the returned handle is the
/// one its real path names.
///
/// # Errors
/// [`io::ErrorKind::NotFound`] when absent; [`io::ErrorKind::NotADirectory`]
/// for a non-directory; another error on another failure, a level turned into
/// a reparse point mid-walk included.
pub fn open_following(path: &Path) -> io::Result<Dir> {
    let real = std::fs::canonicalize(path)?;
    let mut parts = real.components();
    let (Some(Component::Prefix(prefix)), Some(Component::RootDir)) = (parts.next(), parts.next())
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no volume root", real.display()),
        ));
    };
    let mut root = PathBuf::from(prefix.as_os_str());
    root.push(Component::RootDir.as_os_str());
    let mut dir = open_dir_at(root)?;
    for part in parts {
        let Component::Normal(name) = part else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a canonical path", real.display()),
            ));
        };
        dir = dir.open_dir(name)?;
    }
    Ok(dir)
}

/// The identity of the directory looking `path` up now reaches, following links.
///
/// # Errors
/// When `path` cannot be opened.
pub fn id_of_path(path: &Path) -> io::Result<DirId> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(BACKUP_SEMANTICS)
        .open(path)?;
    id_of(&file)
}

/// The volume serial number and file index of the object `file` holds.
fn id_of(file: &File) -> io::Result<DirId> {
    let info = winapi_util::file::information(file)?;
    Ok(DirId {
        dev: info.volume_serial_number(),
        ino: info.file_index(),
    })
}

/// Whether `name` is a single plain entry name.
///
/// A verbatim path is not normalised, so a separator, `.`, `..`, or a `:`
/// (which would address an alternate data stream) must never reach it.
fn is_plain_name(name: &OsStr) -> bool {
    let text = name.to_string_lossy();
    !text.is_empty()
        && text != "."
        && text != ".."
        && !text.chars().any(|c| {
            c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
}

impl Dir {
    /// The real path of the entry `name`, which must be a plain name.
    fn entry(&self, name: &OsStr) -> io::Result<PathBuf> {
        if is_plain_name(name) {
            Ok(self.real.join(name))
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a plain entry name", name.to_string_lossy()),
            ))
        }
    }

    /// Open the subdirectory `name`, failing on a reparse point or a non-directory.
    pub fn open_dir(&self, name: &OsStr) -> io::Result<Self> {
        open_dir_at(self.entry(name)?)
    }

    /// Classify the entry `name` without following a reparse point.
    ///
    /// Every reparse point counts as a link.
    pub fn kind(&self, name: &OsStr) -> io::Result<EntryKind> {
        match std::fs::symlink_metadata(self.entry(name)?) {
            Ok(meta) => {
                let attributes = meta.file_attributes();
                Ok(if attributes & ATTR_REPARSE_POINT != 0 {
                    EntryKind::Symlink
                } else if attributes & ATTR_DIRECTORY != 0 {
                    EntryKind::Directory
                } else {
                    EntryKind::Other
                })
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(EntryKind::Absent),
            Err(e) => Err(e),
        }
    }

    /// Create the subdirectory `name`.
    pub fn mkdir(&self, name: &OsStr) -> io::Result<()> {
        std::fs::create_dir(self.entry(name)?)
    }

    /// Exclusively create the new file `name` for writing.
    pub fn create_new(&self, name: &OsStr) -> io::Result<File> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(OPEN_REPARSE_POINT)
            .open(self.entry(name)?)
    }

    /// Open the existing entry `name` for reading, failing on a reparse point.
    pub fn open_file(&self, name: &OsStr) -> io::Result<File> {
        let path = self.entry(name)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(OPEN_REPARSE_POINT)
            .open(&path)?;
        if file.metadata()?.file_attributes() & ATTR_REPARSE_POINT != 0 {
            return Err(reparse_point(&path));
        }
        Ok(file)
    }

    /// Rename the entry `from` over the entry `to`, both in this directory.
    pub fn rename(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        std::fs::rename(self.entry(from)?, self.entry(to)?)
    }

    /// Remove the non-directory entry `name`; a reparse point is removed as itself.
    ///
    /// A directory junction or directory link is removed as the link it is;
    /// a plain directory is refused.
    pub fn unlink(&self, name: &OsStr) -> io::Result<()> {
        let path = self.entry(name)?;
        let attributes = std::fs::symlink_metadata(&path)?.file_attributes();
        if attributes & ATTR_DIRECTORY == 0 {
            std::fs::remove_file(&path)
        } else if attributes & ATTR_REPARSE_POINT != 0 {
            std::fs::remove_dir(&path)
        } else {
            Err(io::ErrorKind::IsADirectory.into())
        }
    }

    /// Remove the empty subdirectory `name`.
    pub fn rmdir(&self, name: &OsStr) -> io::Result<()> {
        std::fs::remove_dir(self.entry(name)?)
    }

    /// The names of this directory's entries.
    pub fn names(&self) -> io::Result<impl Iterator<Item = io::Result<OsString>>> {
        Ok(std::fs::read_dir(&self.real)?.map(|entry| entry.map(|entry| entry.file_name())))
    }

    /// Open the directory above this one; `None` at the volume root.
    pub fn parent(&self) -> io::Result<Option<Self>> {
        self.real
            .parent()
            .map(|parent| open_dir_at(parent.to_path_buf()))
            .transpose()
    }

    /// The identity of this directory.
    pub fn id(&self) -> io::Result<DirId> {
        id_of(&self.file)
    }
}

//! Directory handles held open, so every act names one entry of a proven directory.
//!
//! A path-based check followed by a path-based act leaves a window in which a
//! level can be swapped for a symbolic link. Here every level is opened relative
//! to the handle of the level above it with `O_NOFOLLOW`, and every create,
//! rename, and unlink names a single entry of a held handle — a link planted at
//! any level is refused, never traversed, and a level swapped after it was
//! opened no longer matters because the held handle still names the real one.

use std::ffi::OsStr;
use std::io::{Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, CWD, Dir, FileType, Mode, OFlags};
use rustix::io::Errno;

use super::{
    MARKER_HEADER, MARKER_READ_CAP, MARKER_TEXT, OWNERSHIP_MARKER, OutputRefusal, temp_suffix,
};
use crate::{CliError, io_err};

/// Deepest directory nesting [`HeldDir::remove_entry`] descends.
///
/// Each level keeps two descriptors open (the handle and its listing), so the
/// ceiling also bounds descriptor use.
pub const MAX_REMOVE_DEPTH: usize = 128;

/// The device and inode of a directory, its identity across path lookups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirId {
    dev: u64,
    ino: u64,
}

/// What an entry of a held directory is, read without following a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// No entry by that name.
    Absent,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// Anything else: a regular file, a FIFO, a socket, a device.
    Other,
}

/// An open directory handle and the path it was reached by.
///
/// The path serves diagnostics only; every act goes through the handle.
#[derive(Debug)]
pub struct HeldDir {
    dir: std::fs::File,
    path: PathBuf,
}

/// Flags for opening a directory handle.
fn dir_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC
}

/// Flags for exclusively creating a new file that is never a link.
fn new_file_flags() -> OFlags {
    OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

/// Permission bits a new file is created with, before the umask.
fn file_mode() -> Mode {
    Mode::RUSR | Mode::WUSR | Mode::RGRP | Mode::WGRP | Mode::ROTH | Mode::WOTH
}

/// Permission bits a new directory is created with, before the umask.
fn dir_mode() -> Mode {
    Mode::RWXU | Mode::RWXG | Mode::RWXO
}

/// The `CliError` for `errno` met at `path`.
fn errno_err(path: &Path, errno: Errno) -> CliError {
    io_err(path, std::io::Error::from(errno))
}

impl HeldDir {
    /// Open `path` as a directory, following links on the way.
    ///
    /// Used only for the ancestors of an owned directory, which ipe does not
    /// own; the owned directory itself is always opened with [`HeldDir::open`].
    /// `Ok(None)` when absent.
    ///
    /// # Errors
    /// [`OutputRefusal::NotADirectory`] when `path` is not a directory;
    /// [`CliError::Io`] on another failure.
    pub fn open_following(path: &Path) -> Result<Option<Self>, CliError> {
        let target = if path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            path
        };
        match rustix::fs::openat(CWD, target, dir_flags(), Mode::empty()) {
            Ok(fd) => Ok(Some(Self {
                dir: std::fs::File::from(fd),
                path: path.to_path_buf(),
            })),
            Err(e) if e == Errno::NOENT => Ok(None),
            Err(e) if e == Errno::NOTDIR => {
                Err(OutputRefusal::NotADirectory(path.to_path_buf()).into())
            }
            Err(e) => Err(errno_err(path, e)),
        }
    }

    /// Open `path` as a directory whose final component is never a link.
    ///
    /// The parent is reached following links (it is not ipe's); the final
    /// component is opened relative to it with `O_NOFOLLOW`. `Ok(None)` when
    /// `path` or its parent is absent.
    ///
    /// # Errors
    /// [`OutputRefusal::Symlink`] or [`OutputRefusal::NotADirectory`] for a
    /// link or a non-directory; [`CliError::Io`] on another failure.
    pub fn open(path: &Path) -> Result<Option<Self>, CliError> {
        let Some(name) = path.file_name() else {
            return Self::open_following(path);
        };
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        Self::open_following(parent)?.map_or(Ok(None), |parent| parent.child(name))
    }

    /// The path this handle was reached by.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The identity of the directory this handle holds.
    ///
    /// # Errors
    /// [`CliError::Io`] when the handle cannot be stat'd.
    pub fn id(&self) -> Result<DirId, CliError> {
        let meta = self.dir.metadata().map_err(|e| io_err(&self.path, e))?;
        Ok(DirId {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }

    /// Classify the entry `name` without following a link.
    ///
    /// # Errors
    /// [`CliError::Io`] on a failure other than absence.
    pub fn kind_of(&self, name: &OsStr) -> Result<EntryKind, CliError> {
        match rustix::fs::statat(&self.dir, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => Ok(match FileType::from_raw_mode(stat.st_mode) {
                FileType::Directory => EntryKind::Directory,
                FileType::Symlink => EntryKind::Symlink,
                FileType::RegularFile
                | FileType::Fifo
                | FileType::Socket
                | FileType::CharacterDevice
                | FileType::BlockDevice
                | FileType::Unknown => EntryKind::Other,
            }),
            Err(e) if e == Errno::NOENT => Ok(EntryKind::Absent),
            Err(e) => Err(errno_err(&self.path.join(name), e)),
        }
    }

    /// Open the subdirectory `name`, refusing a link or a non-directory.
    ///
    /// `Ok(None)` when absent.
    ///
    /// # Errors
    /// [`OutputRefusal::Symlink`] or [`OutputRefusal::NotADirectory`]; [`CliError::Io`]
    /// on another failure.
    pub fn child(&self, name: &OsStr) -> Result<Option<Self>, CliError> {
        let path = self.path.join(name);
        match rustix::fs::openat(
            &self.dir,
            name,
            dir_flags() | OFlags::NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(fd) => Ok(Some(Self {
                dir: std::fs::File::from(fd),
                path,
            })),
            Err(e) if e == Errno::NOENT => Ok(None),
            Err(e) => match self.kind_of(name)? {
                EntryKind::Symlink => Err(OutputRefusal::Symlink(path).into()),
                EntryKind::Other => Err(OutputRefusal::NotADirectory(path).into()),
                EntryKind::Absent | EntryKind::Directory => Err(errno_err(&path, e)),
            },
        }
    }

    /// Open the subdirectory `name`, creating it when absent.
    ///
    /// The flag is `true` when this call created it.
    ///
    /// # Errors
    /// As [`HeldDir::child`].
    pub fn create_child(&self, name: &OsStr) -> Result<(Self, bool), CliError> {
        let path = self.path.join(name);
        let created = match rustix::fs::mkdirat(&self.dir, name, dir_mode()) {
            Ok(()) => true,
            Err(e) if e == Errno::EXIST => false,
            Err(e) => return Err(errno_err(&path, e)),
        };
        self.child(name)?.map_or_else(
            || Err(errno_err(&path, Errno::NOENT)),
            |child| Ok((child, created)),
        )
    }

    /// Whether this directory carries a genuine ownership marker.
    ///
    /// The marker must be a regular file (never a link) whose first line is
    /// [`MARKER_HEADER`].
    ///
    /// # Errors
    /// [`CliError::Io`] on a read failure other than absence.
    pub fn has_marker(&self) -> Result<bool, CliError> {
        let name = OsStr::new(OWNERSHIP_MARKER);
        let path = self.path.join(name);
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let file = match rustix::fs::openat(&self.dir, name, flags, Mode::empty()) {
            Ok(fd) => std::fs::File::from(fd),
            Err(e) if e == Errno::NOENT => return Ok(false),
            Err(e) => {
                return match self.kind_of(name)? {
                    EntryKind::Symlink | EntryKind::Absent => Ok(false),
                    EntryKind::Directory | EntryKind::Other => Err(errno_err(&path, e)),
                };
            }
        };
        let meta = file.metadata().map_err(|e| io_err(&path, e))?;
        if !meta.is_file() {
            return Ok(false);
        }
        let mut head = Vec::new();
        file.take(MARKER_READ_CAP)
            .read_to_end(&mut head)
            .map_err(|e| io_err(&path, e))?;
        Ok(head.starts_with(MARKER_HEADER.as_bytes()))
    }

    /// Whether this directory holds nothing but the marker or an in-flight marker temp file.
    ///
    /// # Errors
    /// [`CliError::Io`] when the directory cannot be listed.
    pub fn is_empty(&self) -> Result<bool, CliError> {
        let entries = Dir::read_from(&self.dir).map_err(|e| errno_err(&self.path, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| errno_err(&self.path, e))?;
            let name = OsStr::from_bytes(entry.file_name().to_bytes());
            if is_dot(name) {
                continue;
            }
            if !super::is_marker_name(&name.to_string_lossy()) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Mark this directory ipe-owned, or refuse it as user territory.
    ///
    /// Already marked: nothing to do. Empty: the marker is written. Anything
    /// else is refused with [`OutputRefusal::NotIpeOwned`] and left untouched.
    ///
    /// # Errors
    /// [`OutputRefusal::NotIpeOwned`]; [`CliError::Io`] on a filesystem failure.
    pub fn adopt(&self) -> Result<(), CliError> {
        if self.has_marker()? {
            Ok(())
        } else if self.is_empty()? {
            self.write_marker()
        } else {
            Err(OutputRefusal::NotIpeOwned(self.path.clone()).into())
        }
    }

    /// Write the marker atomically through a uniquely named temp file and a rename.
    ///
    /// The temp name (`.ipe-output.<pid>.<n>.tmp`) is the one other name
    /// [`HeldDir::is_empty`] tolerates, so a claim in flight never makes the
    /// directory look user-owned.
    ///
    /// # Errors
    /// [`CliError::Io`] on a filesystem failure.
    pub fn write_marker(&self) -> Result<(), CliError> {
        let tmp = format!("{OWNERSHIP_MARKER}.{}.tmp", temp_suffix());
        self.replace_file(
            OsStr::new(OWNERSHIP_MARKER),
            OsStr::new(&tmp),
            None,
            |file| file.write_all(MARKER_TEXT.as_bytes()),
        )
    }

    /// Replace the file `name` with the result of `fill`, atomically.
    ///
    /// A link at `name` is refused. The content goes to an exclusively created
    /// temp file (`.<name>.ipe-tmp.<pid>.<n>`) that is renamed over `name`, so
    /// nothing is ever written through an existing entry.
    ///
    /// # Errors
    /// [`OutputRefusal::Symlink`]; [`CliError::Io`] on a filesystem failure.
    pub fn write_file(
        &self,
        name: &OsStr,
        permissions: Option<std::fs::Permissions>,
        fill: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
    ) -> Result<(), CliError> {
        if self.kind_of(name)? == EntryKind::Symlink {
            return Err(OutputRefusal::Symlink(self.path.join(name)).into());
        }
        let tmp = format!(".{}.ipe-tmp.{}", name.to_string_lossy(), temp_suffix());
        self.replace_file(name, OsStr::new(&tmp), permissions, fill)
    }

    /// Fill the new file `tmp`, then rename it over `name`; `tmp` is removed on failure.
    fn replace_file(
        &self,
        name: &OsStr,
        tmp: &OsStr,
        permissions: Option<std::fs::Permissions>,
        fill: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
    ) -> Result<(), CliError> {
        let tmp_path = self.path.join(tmp);
        let mut staged = rustix::fs::openat(&self.dir, tmp, new_file_flags(), file_mode())
            .map(std::fs::File::from)
            .map_err(|e| errno_err(&tmp_path, e))?;
        let filled = fill(&mut staged).and_then(|()| {
            permissions.map_or(Ok(()), |permissions| staged.set_permissions(permissions))
        });
        drop(staged);
        let result = filled.map_err(|e| io_err(&tmp_path, e)).and_then(|()| {
            rustix::fs::renameat(&self.dir, tmp, &self.dir, name)
                .map_err(|e| errno_err(&self.path.join(name), e))
        });
        if result.is_err() {
            let _ = rustix::fs::unlinkat(&self.dir, tmp, AtFlags::empty());
        }
        result
    }

    /// Remove the entry `name` — a whole directory tree or a single file.
    ///
    /// An absent entry is already removed. A link at `name` is refused; a link
    /// met inside the tree is removed as the link it is, never followed.
    ///
    /// # Errors
    /// [`OutputRefusal::Symlink`]; [`OutputRefusal::TooDeep`] for a tree nested
    /// deeper than [`MAX_REMOVE_DEPTH`]; [`CliError::Io`] on a filesystem failure.
    pub fn remove_entry(&self, name: &OsStr) -> Result<(), CliError> {
        match self.kind_of(name)? {
            EntryKind::Absent => Ok(()),
            EntryKind::Symlink => Err(OutputRefusal::Symlink(self.path.join(name)).into()),
            EntryKind::Directory => self.remove_dir(name, 0),
            EntryKind::Other => self.unlink(name),
        }
    }

    /// Unlink the non-directory entry `name`; a link is removed, never followed.
    ///
    /// An absent entry is already removed.
    ///
    /// # Errors
    /// [`CliError::Io`] on a filesystem failure, a directory at `name` included.
    pub fn unlink(&self, name: &OsStr) -> Result<(), CliError> {
        match rustix::fs::unlinkat(&self.dir, name, AtFlags::empty()) {
            Ok(()) => Ok(()),
            Err(e) if e == Errno::NOENT => Ok(()),
            Err(e) => Err(errno_err(&self.path.join(name), e)),
        }
    }

    /// Empty the subdirectory `name` at nesting `depth`, then remove it.
    fn remove_dir(&self, name: &OsStr, depth: usize) -> Result<(), CliError> {
        let path = self.path.join(name);
        if depth >= MAX_REMOVE_DEPTH {
            return Err(OutputRefusal::TooDeep {
                path,
                limit: MAX_REMOVE_DEPTH,
            }
            .into());
        }
        if let Some(child) = self.child(name)? {
            child.remove_contents(depth)?;
        }
        match rustix::fs::unlinkat(&self.dir, name, AtFlags::REMOVEDIR) {
            Ok(()) => Ok(()),
            Err(e) if e == Errno::NOENT => Ok(()),
            Err(e) => Err(errno_err(&path, e)),
        }
    }

    /// Remove every entry of this directory, which sits at nesting `depth`.
    fn remove_contents(&self, depth: usize) -> Result<(), CliError> {
        let entries = Dir::read_from(&self.dir).map_err(|e| errno_err(&self.path, e))?;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) if e == Errno::NOENT => continue,
                Err(e) => return Err(errno_err(&self.path, e)),
            };
            let name = OsStr::from_bytes(entry.file_name().to_bytes());
            if is_dot(name) {
                continue;
            }
            match self.kind_of(name)? {
                EntryKind::Absent => {}
                EntryKind::Directory => self.remove_dir(name, depth.saturating_add(1))?,
                EntryKind::Symlink | EntryKind::Other => self.unlink(name)?,
            }
        }
        Ok(())
    }
}

/// Whether `name` is the `.` or `..` entry of a listing.
fn is_dot(name: &OsStr) -> bool {
    name == "." || name == ".."
}

/// Test-only hook run after each level of an owned-path walk is held.
#[cfg(test)]
pub type LevelHook = Box<dyn FnMut(&Path)>;

#[cfg(test)]
thread_local! {
    static LEVEL_HOOK: std::cell::RefCell<Option<LevelHook>> = const { std::cell::RefCell::new(None) };
}

/// Install (or clear) the hook run after each level of an owned-path walk is held.
///
/// It lets a test swap a level for a link in the window between opening one
/// level and acting through it.
#[cfg(test)]
pub fn set_level_hook(hook: Option<LevelHook>) {
    LEVEL_HOOK.with(|slot| *slot.borrow_mut() = hook);
}

/// Run the test hook, if any, for the level at `path`.
#[cfg(test)]
pub fn level_held(path: &Path) {
    let taken = LEVEL_HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = taken {
        hook(path);
        LEVEL_HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(hook);
            }
        });
    }
}

/// Outside tests there is no hook: holding a level has no side effect.
#[cfg(not(test))]
pub const fn level_held(_path: &Path) {}

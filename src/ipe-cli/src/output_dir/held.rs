//! Directory handles held open, so every act names one entry of a proven directory.
//!
//! A path-based check followed by a path-based act leaves a window in which a
//! level can be swapped for a symbolic link. Here every level is opened through
//! the held level above it without following a link, and every create, rename,
//! and unlink names a single entry of a held handle — a link planted at any
//! level is refused, never traversed, and a level swapped after it was opened
//! no longer matters because the held handle still names the real one. This
//! holds from the first held level (the anchor) down: the levels above it are
//! not ipe's and are opened following links ([`HeldDir::open_following`]), so a
//! caller proves what the anchor is on its canonical path.
//!
//! A subdirectory is removed only through [`Released`], a proof that its name
//! still named the held directory the instant its handles were let go, and only
//! by a primitive that removes nothing but an empty directory — a file, a link,
//! or a populated tree swapped in at the name is refused, never removed.
//!
//! Opening, classifying, reading, and identifying a held level go through
//! [`ipe_fs_open`], the one home of handle-relative opens; the per-platform
//! write acts live in `unix` (descriptor-relative `*at` calls) and `windows`
//! (handle-relative creates and deletes; path acts run under a sentinel pin).

use std::ffi::OsStr;
use std::io::{self, Read as _, Seek as _, Write as _};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use ipe_fs_open::{ByteCap, EntryName, FileKind, OpenRefusal};

use super::{
    CLAIM_FILE, CLAIM_POLL, MARKER_HEADER, MARKER_READ_CAP, MARKER_TEXT, MAX_CLAIM_POLLS,
    OWNERSHIP_MARKER, OutputRefusal, temp_suffix,
};
use crate::{CliError, io_err};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as sys;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as sys;
#[cfg(not(any(unix, windows)))]
compile_error!("held output-directory handles are implemented for Unix and Windows only");

/// Deepest directory nesting [`HeldDir::remove_entry`] descends.
///
/// Each level keeps two handles open (the directory and its listing), so the
/// ceiling also bounds handle use.
pub const MAX_REMOVE_DEPTH: usize = 128;

/// The volume and file number of a directory or file, its identity across path lookups.
pub type DirId = ipe_fs_open::FileId;

/// How much of the marker file a marker read takes.
///
/// A `MARKER_READ_CAP` of zero underflows here and fails the build.
const MARKER_CAP: ByteCap =
    ByteCap::from_nonzero(NonZeroU64::MIN.saturating_add(MARKER_READ_CAP - 1));

/// Who owns a held directory, as [`HeldDir::ownership`] reads it for display and preflight.
///
/// Only [`HeldDir::claim`] decides ownership; this reading never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    /// It carries a genuine ownership marker and no claim file.
    Marked,
    /// It holds a claim file: a claim is running, or a crashed one awaits takeover.
    Claiming,
    /// It holds nothing but a regular-file marker or claim file.
    Empty,
    /// It holds something else and carries no marker: user territory.
    User,
}

/// Whether a held directory is ipe's now, as [`HeldDir::owned_now`] reads it.
///
/// The one ownership predicate every act that trusts a directory consults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedNow {
    /// A genuine marker is linked and no claim file exists.
    Owned,
    /// A genuine marker is linked beside a claim file, so its claim has not committed.
    Claiming,
    /// No genuine marker is linked, or it was replaced while being read.
    Unowned,
}

/// Proof that a directory is ipe's, built only by [`HeldDir::claim`].
#[must_use]
#[derive(Debug)]
pub struct Claimed(());

/// The one byte a claim file holds once its holder begins publishing the marker.
const PHASE_FINALIZING: u8 = 1;

/// How far a claim file's holder got, recorded in the claim file's length.
///
/// The phase byte is synced before the marker is created, so a marker beside a
/// still-empty claim file was never written by that claim's holder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaimPhase {
    /// The holder is still deciding: no marker write has begun.
    Pending,
    /// The holder began publishing the marker.
    Finalizing,
}

/// What sits at the marker name of a held directory.
#[derive(Debug)]
enum MarkerState {
    /// Nothing.
    Absent,
    /// A regular file with the marker header, held open so its identity is pinned.
    Genuine(ipe_fs_open::RegularFile),
    /// A non-file, a link, or a file without the marker header.
    NotGenuine,
}

/// What an entry of a held directory is for an ownership listing, read without following a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListedKind {
    /// A regular file.
    RegularFile,
    /// A directory, a link, a FIFO, a socket, a device, or a Windows reparse point.
    Other,
}

/// A step of [`HeldDir::claim`] a test can act at, through `set_claim_hook`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimPoint {
    /// The claim file was locked, before its identity was re-checked.
    AfterLock,
    /// The pre-check found nothing foreign, before the phase byte is written.
    AfterPreCheck,
    /// The marker was published, before the post-check lists the directory.
    AfterPublish,
    /// The post-check found nothing foreign, before the claim file is removed.
    BeforeCommit,
    /// One attempt found the claim held elsewhere, before the claim sleeps.
    Polled,
    /// [`HeldDir::owned_now`] read a genuine marker, before it looks for a claim file.
    MarkerRead,
    /// A caller created the directory, before it claims it.
    Created,
}

/// What an entry of a held directory is, read without following a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// No entry by that name.
    Absent,
    /// A directory.
    Directory,
    /// A symbolic link (on Windows, any reparse point).
    Symlink,
    /// Anything else: a regular file, a FIFO, a socket, a device.
    Other,
}

/// An open directory handle and the path it was reached by.
///
/// The path serves diagnostics only; every act goes through the handle.
#[derive(Debug)]
pub struct HeldDir {
    dir: ipe_fs_open::HeldDir,
    path: PathBuf,
}

/// The [`Released`] proof, with fields no code outside this submodule can set.
///
/// A struct literal cannot name a private field from outside its module, so
/// the only way to produce a `Released` is `Released::new`, called solely
/// from [`HeldDir::release_proven`].
mod released {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    /// Proof that an entry named a held subdirectory the instant its last handle was released.
    ///
    /// Only [`HeldDir::release_proven`](super::HeldDir::release_proven) makes
    /// one, and [`HeldDir::rmdir_released`](super::HeldDir::rmdir_released) is
    /// the only removal of a subdirectory, so no subdirectory is ever removed
    /// by a name that was not re-proven first.
    #[derive(Debug)]
    pub(super) struct Released<'n> {
        name: &'n OsStr,
        path: PathBuf,
    }

    impl<'n> Released<'n> {
        /// Construct the proof that `name` still names the held subdirectory at `path`.
        ///
        /// Called only by [`HeldDir::release_proven`](super::HeldDir::release_proven).
        pub(super) const fn new(name: &'n OsStr, path: PathBuf) -> Self {
            Self { name, path }
        }

        /// The re-proven name.
        pub(super) const fn name(&self) -> &'n OsStr {
            self.name
        }

        /// The re-proven path.
        pub(super) fn path(&self) -> &Path {
            &self.path
        }

        /// Consume the proof, returning the path it names.
        pub(super) fn into_path(self) -> PathBuf {
            self.path
        }
    }
}
use released::Released;

/// What removing a re-proven, released subdirectory found at its name.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Rmdir {
    /// The empty directory was removed.
    Removed,
    /// Nothing is there any more.
    Vanished,
    /// A directory holding entries is there, at this path; it is kept.
    Refilled(PathBuf),
}

/// Whether a failed directory removal met something other than a directory at the name.
///
/// A file answers `NotADirectory` on both platforms, a Unix symbolic link
/// too; a Windows reparse point is refused before the removal.
fn is_replacement(error: &io::Error) -> bool {
    #[cfg(windows)]
    if sys::is_reparse_refusal(error) {
        return true;
    }
    error.kind() == io::ErrorKind::NotADirectory
}

/// The error for an act on `path` that failed with `error`.
///
/// An entry another program holds open (Windows) is refused with
/// [`OutputRefusal::InUse`], whose message names the fix; anything else is a
/// [`CliError::Io`].
fn act_err(path: &Path, error: io::Error) -> CliError {
    #[cfg(windows)]
    if sys::is_in_use(&error) {
        return OutputRefusal::InUse(path.to_path_buf()).into();
    }
    io_err(path, error)
}

/// The error for an open or read of `path` that `refusal` turned back.
///
/// An entry another program holds open is refused with
/// [`OutputRefusal::InUse`], as [`act_err`] refuses a write act on one.
fn refused(path: &Path, refusal: OpenRefusal) -> CliError {
    match refusal {
        OpenRefusal::InUse => OutputRefusal::InUse(path.to_path_buf()).into(),
        OpenRefusal::Absent
        | OpenRefusal::Link
        | OpenRefusal::NotRegular(_)
        | OpenRefusal::Denied
        | OpenRefusal::TooLarge(_)
        | OpenRefusal::TooManyEntries(_)
        | OpenRefusal::BadName
        | OpenRefusal::NotUtf8
        | OpenRefusal::Io(_) => act_err(path, refusal.into_io()),
    }
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
    /// [`OutputRefusal::ReparsePoint`] when a level of it is a reparse point
    /// (Windows); [`CliError::Io`] on another failure.
    pub fn open_following(path: &Path) -> Result<Option<Self>, CliError> {
        match ipe_fs_open::HeldDir::open_root(path) {
            Ok(dir) => Ok(Some(Self {
                dir,
                path: path.to_path_buf(),
            })),
            Err(OpenRefusal::Absent) => Ok(None),
            Err(OpenRefusal::NotRegular(_) | OpenRefusal::Io(io::ErrorKind::NotADirectory)) => {
                Err(OutputRefusal::NotADirectory(path.to_path_buf()).into())
            }
            #[cfg(windows)]
            Err(OpenRefusal::Link) => Err(OutputRefusal::ReparsePoint(path.to_path_buf()).into()),
            Err(refusal) => Err(refused(path, refusal)),
        }
    }

    /// Open `path` as a directory whose final component is never a link.
    ///
    /// The parent is reached following links (it is not ipe's); the final
    /// component is opened through it without following a link. `Ok(None)`
    /// when `path` or its parent is absent.
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
        self.dir
            .id()
            .map_err(|refusal| refused(&self.path, refusal))
    }

    /// The entry `name` of this directory as one plain entry name.
    ///
    /// # Errors
    /// [`CliError::Io`] of [`io::ErrorKind::InvalidInput`] when it is not one.
    fn entry(&self, name: &OsStr) -> Result<EntryName, CliError> {
        EntryName::new(name).ok_or_else(|| {
            act_err(
                &self.path.join(name),
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not a plain entry name", name.to_string_lossy()),
                ),
            )
        })
    }

    /// Classify the entry `name` without following a link.
    ///
    /// # Errors
    /// [`CliError::Io`] on a failure other than absence.
    pub fn kind_of(&self, name: &OsStr) -> Result<EntryKind, CliError> {
        let entry = self.entry(name)?;
        match self.dir.kind_of(&entry) {
            Ok(None) => Ok(EntryKind::Absent),
            Ok(Some(FileKind::Dir)) => Ok(EntryKind::Directory),
            Ok(Some(FileKind::Symlink)) => Ok(EntryKind::Symlink),
            Ok(Some(
                FileKind::Regular
                | FileKind::Fifo
                | FileKind::Socket
                | FileKind::Device
                | FileKind::Other,
            )) => Ok(EntryKind::Other),
            Err(refusal) => Err(refused(&self.path.join(name), refusal)),
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
        let entry = self.entry(name)?;
        let path = self.path.join(name);
        match self.dir.child_dir(&entry) {
            Ok(dir) => Ok(Some(Self { dir, path })),
            Err(OpenRefusal::Absent) => Ok(None),
            Err(OpenRefusal::Link) => Err(OutputRefusal::Symlink(path).into()),
            Err(OpenRefusal::NotRegular(_)) => Err(OutputRefusal::NotADirectory(path).into()),
            Err(refusal) => Err(refused(&path, refusal)),
        }
    }

    /// Open the subdirectory `name`, creating it when absent.
    ///
    /// The flag is `true` when this call created it.
    ///
    /// # Errors
    /// As [`HeldDir::child`].
    pub fn create_child(&self, name: &OsStr) -> Result<(Self, bool), CliError> {
        let entry = self.entry(name)?;
        let path = self.path.join(name);
        let created = match sys::mkdir(&self.dir, &entry) {
            Ok(()) => true,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
            Err(e) => return Err(act_err(&path, e)),
        };
        self.child(name)?.map_or_else(
            || Err(act_err(&path, io::ErrorKind::NotFound.into())),
            |child| Ok((child, created)),
        )
    }

    /// Whether this directory is ipe's now, as [`HeldDir::owned_now`] reads it.
    ///
    /// A claim still in flight is not ownership.
    ///
    /// # Errors
    /// As [`HeldDir::owned_now`].
    pub fn has_marker(&self) -> Result<bool, CliError> {
        Ok(self.owned_now()? == OwnedNow::Owned)
    }

    /// Whether this directory is ipe's now, read through this handle alone.
    ///
    /// A genuine marker is read and held open, a claim file is looked for, and
    /// the marker name is proven to still link the held marker. The held
    /// marker pins its file number, and only a claim whose claim file exists
    /// ever unlinks a marker, so with no claim file and the same marker linked
    /// before and after, no unmark ran in between.
    ///
    /// # Errors
    /// [`CliError::Io`] on a filesystem failure.
    pub fn owned_now(&self) -> Result<OwnedNow, CliError> {
        let MarkerState::Genuine(marker) = self.genuine_marker()? else {
            return Ok(OwnedNow::Unowned);
        };
        claim_point(ClaimPoint::MarkerRead, &self.path);
        if self.claim_present()? {
            return Ok(OwnedNow::Claiming);
        }
        let name = OsStr::new(OWNERSHIP_MARKER);
        let held = marker
            .id()
            .map_err(|refusal| refused(&self.path.join(name), refusal))?;
        Ok(if self.links(name, held)? {
            OwnedNow::Owned
        } else {
            OwnedNow::Unowned
        })
    }

    /// What sits at the marker name: nothing, a genuine marker held open, or anything else.
    ///
    /// A genuine marker is a regular file (never a link) whose first line is
    /// [`MARKER_HEADER`]. A link, a directory, a non-file, or a wrong header is
    /// [`MarkerState::NotGenuine`], never [`MarkerState::Absent`].
    fn genuine_marker(&self) -> Result<MarkerState, CliError> {
        let name = OsStr::new(OWNERSHIP_MARKER);
        let entry = self.entry(name)?;
        let path = self.path.join(name);
        let mut file = match self.dir.open_regular(&entry) {
            Ok(file) => file,
            Err(OpenRefusal::Absent) => return Ok(MarkerState::Absent),
            Err(OpenRefusal::Link | OpenRefusal::NotRegular(_)) => {
                return Ok(MarkerState::NotGenuine);
            }
            Err(refusal) => return Err(refused(&path, refusal)),
        };
        let head = file
            .read_head(MARKER_CAP)
            .map_err(|refusal| refused(&path, refusal))?;
        Ok(if head.starts_with(MARKER_HEADER.as_bytes()) {
            MarkerState::Genuine(file)
        } else {
            MarkerState::NotGenuine
        })
    }

    /// Whether an entry named [`CLAIM_FILE`] is present, of any kind.
    ///
    /// One being deleted (Windows keeps it listed until its last handle
    /// closes) counts as present.
    fn claim_present(&self) -> Result<bool, CliError> {
        let name = OsStr::new(CLAIM_FILE);
        let entry = self.entry(name)?;
        match self.dir.kind_of(&entry) {
            Ok(None) => Ok(false),
            Ok(Some(_)) => Ok(true),
            Err(refusal) if sys::is_delete_pending_refusal(refusal) => Ok(true),
            Err(refusal) => Err(refused(&self.path.join(name), refusal)),
        }
    }

    /// Whether the entry `name` links the very object whose identity is `held`, read without following a link.
    ///
    /// An absent entry, or one being deleted, links nothing.
    fn links(&self, name: &OsStr, held: DirId) -> Result<bool, CliError> {
        let entry = self.entry(name)?;
        match self.dir.entry_id(&entry) {
            Ok(linked) => Ok(linked == Some(held)),
            Err(refusal) if sys::is_delete_pending_refusal(refusal) => Ok(false),
            Err(refusal) => Err(refused(&self.path.join(name), refusal)),
        }
    }

    /// Whether the entry `name` links the very file the open `file` holds.
    fn links_file(&self, name: &OsStr, file: &std::fs::File) -> Result<bool, CliError> {
        let held =
            DirId::of_file(file).map_err(|refusal| refused(&self.path.join(name), refusal))?;
        self.links(name, held)
    }

    /// Whether this directory holds an entry no claim tolerates.
    ///
    /// Only a regular-file marker and a regular-file claim file are tolerated
    /// ([`super::tolerated_entry`]); the listing stops at the first other entry.
    fn holds_foreign(&self) -> Result<bool, CliError> {
        let names = sys::names(&self.dir).map_err(|e| act_err(&self.path, e))?;
        for name in names {
            let name = match name {
                Ok(name) => name,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(act_err(&self.path, e)),
            };
            let entry = self.entry(&name)?;
            let kind = match self.dir.kind_of(&entry) {
                Ok(None) => continue,
                Ok(Some(FileKind::Regular)) => ListedKind::RegularFile,
                Ok(Some(
                    FileKind::Dir
                    | FileKind::Symlink
                    | FileKind::Fifo
                    | FileKind::Socket
                    | FileKind::Device
                    | FileKind::Other,
                )) => ListedKind::Other,
                Err(refusal) if name == CLAIM_FILE && sys::is_delete_pending_refusal(refusal) => {
                    continue;
                }
                Err(refusal) => return Err(refused(&self.path.join(&name), refusal)),
            };
            if super::tolerated_entry(&name, kind) == super::Tolerated::Foreign {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether this directory holds nothing but a regular-file marker or claim file.
    ///
    /// # Errors
    /// [`CliError::Io`] when the directory cannot be listed.
    pub fn is_empty(&self) -> Result<bool, CliError> {
        Ok(!self.holds_foreign()?)
    }

    /// Who owns this directory, read without writing, for display and preflight.
    ///
    /// A directory found non-empty has its ownership read again: a concurrent
    /// claim publishes its marker before it fills the directory, so only a
    /// directory still unmarked and unclaimed on the second read is user
    /// territory. Only [`HeldDir::claim`] decides.
    ///
    /// # Errors
    /// [`CliError::Io`] on a filesystem failure.
    pub fn ownership(&self) -> Result<Ownership, CliError> {
        match self.owned_now()? {
            OwnedNow::Owned => return Ok(Ownership::Marked),
            OwnedNow::Claiming => return Ok(Ownership::Claiming),
            OwnedNow::Unowned => {}
        }
        if self.claim_present()? {
            return Ok(Ownership::Claiming);
        }
        if self.is_empty()? {
            return Ok(Ownership::Empty);
        }
        Ok(match self.owned_now()? {
            OwnedNow::Owned => Ownership::Marked,
            OwnedNow::Claiming => Ownership::Claiming,
            OwnedNow::Unowned if self.claim_present()? => Ownership::Claiming,
            OwnedNow::Unowned => Ownership::User,
        })
    }

    /// Make this directory ipe's, or refuse it as user territory.
    ///
    /// The one transition into ownership. Every decision runs under an
    /// exclusive lock on a claim file created no-replace in the directory, and
    /// the marker is published no-replace between a pre-check and a post-check
    /// listing, so an entry planted at any instant before the marker is linked
    /// refuses the claim and is never touched. A claim file a crashed claim
    /// left is taken over in place. The wait for a claim held elsewhere is
    /// bounded by `MAX_CLAIM_POLLS` polls `CLAIM_POLL` apart.
    ///
    /// # Errors
    /// [`OutputRefusal::NotIpeOwned`] for user territory or a non-file claim
    /// name; [`OutputRefusal::ClaimInterrupted`] for a crashed claim's marker
    /// beside foreign entries, or a partial marker it left;
    /// [`OutputRefusal::ClaimBusy`] past the wait bound;
    /// [`OutputRefusal::ClaimLockUnavailable`] when the filesystem refuses the
    /// lock; [`CliError::Io`] on a filesystem failure.
    pub fn claim(&self) -> Result<Claimed, CliError> {
        for poll in 1..=MAX_CLAIM_POLLS {
            if let Some(claimed) = self.try_claim()? {
                return Ok(claimed);
            }
            claim_point(ClaimPoint::Polled, &self.path);
            if poll < MAX_CLAIM_POLLS {
                std::thread::sleep(CLAIM_POLL);
            }
        }
        Err(OutputRefusal::ClaimBusy {
            dir: self.path.clone(),
            waited: CLAIM_POLL.saturating_mul(MAX_CLAIM_POLLS),
        }
        .into())
    }

    /// One claim attempt; `None` when the claim file is held elsewhere or was replaced.
    fn try_claim(&self) -> Result<Option<Claimed>, CliError> {
        if self.owned_now()? == OwnedNow::Owned {
            return Ok(Some(Claimed(())));
        }
        let Some(mut claim) = self.lock_claim()? else {
            return Ok(None);
        };
        claim_point(ClaimPoint::AfterLock, &self.path);
        if !self.links_file(OsStr::new(CLAIM_FILE), &claim)? {
            return Ok(None);
        }
        let phase = self.phase_of(&mut claim)?;
        match (phase, self.genuine_marker()?) {
            (ClaimPhase::Pending, MarkerState::NotGenuine) => {
                self.drop_claim(claim)?;
                Err(OutputRefusal::NotIpeOwned(self.path.clone()).into())
            }
            (ClaimPhase::Finalizing, MarkerState::NotGenuine) => {
                Err(OutputRefusal::ClaimInterrupted(self.path.clone()).into())
            }
            (ClaimPhase::Pending, MarkerState::Genuine(_)) => self.commit(claim).map(Some),
            (ClaimPhase::Finalizing, MarkerState::Genuine(_)) => {
                if self.holds_foreign()? {
                    Err(OutputRefusal::ClaimInterrupted(self.path.clone()).into())
                } else {
                    self.commit(claim).map(Some)
                }
            }
            (ClaimPhase::Pending | ClaimPhase::Finalizing, MarkerState::Absent) => {
                self.publish(claim).map(Some)
            }
        }
    }

    /// Create or open the claim file and take its exclusive lock without waiting.
    ///
    /// `None` when another claimant holds the lock, or the name vanished or is
    /// being deleted. A claim file this call created is removed again when the
    /// filesystem refuses the lock.
    fn lock_claim(&self) -> Result<Option<std::fs::File>, CliError> {
        let name = OsStr::new(CLAIM_FILE);
        let entry = self.entry(name)?;
        let path = self.path.join(name);
        let (claim, created) = match sys::create_claim(&self.dir, &entry) {
            Ok(claim) => (claim, true),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                match sys::open_claim(&self.dir, &entry) {
                    Ok(claim) => (claim, false),
                    Err(e) => return self.unopenable_claim(e),
                }
            }
            Err(e) if sys::is_delete_pending(&e) => return Ok(None),
            Err(e) => return Err(act_err(&path, e)),
        };
        if !claim.metadata().map_err(|e| act_err(&path, e))?.is_file() {
            return Err(OutputRefusal::NotIpeOwned(self.path.clone()).into());
        }
        match claim.try_lock() {
            Ok(()) => Ok(Some(claim)),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) if sys::is_delete_pending(&e) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => {
                if created && self.links_file(name, &claim)? {
                    self.unlink(name)?;
                }
                Err(OutputRefusal::ClaimLockUnavailable {
                    dir: self.path.clone(),
                    kind: e.kind(),
                }
                .into())
            }
        }
    }

    /// The outcome of opening the existing claim name, which failed with `error`.
    ///
    /// A link or a directory there refuses the claim; a name that vanished or
    /// is being deleted is retried.
    fn unopenable_claim(&self, error: io::Error) -> Result<Option<std::fs::File>, CliError> {
        let name = OsStr::new(CLAIM_FILE);
        if error.kind() == io::ErrorKind::NotFound {
            return Ok(None);
        }
        match self.dir.kind_of(&self.entry(name)?) {
            Ok(Some(FileKind::Symlink | FileKind::Dir)) => {
                Err(OutputRefusal::NotIpeOwned(self.path.clone()).into())
            }
            Ok(None) => Ok(None),
            Ok(Some(_)) | Err(_) if sys::is_delete_pending(&error) => Ok(None),
            Ok(Some(_)) | Err(_) => Err(act_err(&self.path.join(name), error)),
        }
    }

    /// The phase the locked `claim` records, parsed from the file itself.
    ///
    /// A claim file only this protocol wrote has one link and holds nothing
    /// ([`ClaimPhase::Pending`]) or the one byte [`PHASE_FINALIZING`]
    /// ([`ClaimPhase::Finalizing`]). Anything else at the claim name, a second
    /// link to a file elsewhere or a file with other contents, is not ipe's: it
    /// refuses the claim [`OutputRefusal::NotIpeOwned`] and is never written to
    /// nor removed.
    fn phase_of(&self, claim: &mut std::fs::File) -> Result<ClaimPhase, CliError> {
        let path = self.path.join(CLAIM_FILE);
        let not_ours = || Err(OutputRefusal::NotIpeOwned(self.path.clone()).into());
        if ipe_fs_open::link_count(claim).map_err(|refusal| refused(&path, refusal))? != 1 {
            return not_ours();
        }
        let len = claim.metadata().map_err(|e| act_err(&path, e))?.len();
        if len == 0 {
            return Ok(ClaimPhase::Pending);
        }
        if len != 1 {
            return not_ours();
        }
        let mut byte = [0_u8];
        claim
            .seek(io::SeekFrom::Start(0))
            .and_then(|_| claim.read_exact(&mut byte))
            .map_err(|e| act_err(&path, e))?;
        if byte == [PHASE_FINALIZING] {
            Ok(ClaimPhase::Finalizing)
        } else {
            not_ours()
        }
    }

    /// Pre-check, publish the marker, post-check, and commit, all under the locked `claim`.
    ///
    /// A refusal removes the claim file and every marker this claim linked; an
    /// I/O failure after the phase byte leaves the claim file for the next
    /// claim to take over.
    fn publish(&self, mut claim: std::fs::File) -> Result<Claimed, CliError> {
        if self.holds_foreign()? {
            self.drop_claim(claim)?;
            return Err(OutputRefusal::NotIpeOwned(self.path.clone()).into());
        }
        claim_point(ClaimPoint::AfterPreCheck, &self.path);
        claim
            .seek(io::SeekFrom::Start(0))
            .and_then(|_| claim.write_all(&[PHASE_FINALIZING]))
            .and_then(|()| claim.sync_data())
            .map_err(|e| act_err(&self.path.join(CLAIM_FILE), e))?;
        let marker = match self.publish_marker() {
            Ok(marker) => marker,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                self.drop_claim(claim)?;
                return Err(OutputRefusal::NotIpeOwned(self.path.clone()).into());
            }
            Err(e) => return Err(act_err(&self.path.join(OWNERSHIP_MARKER), e)),
        };
        claim_point(ClaimPoint::AfterPublish, &self.path);
        if self.holds_foreign()? {
            self.unmark_if(&marker)?;
            self.drop_claim(claim)?;
            return Err(OutputRefusal::NotIpeOwned(self.path.clone()).into());
        }
        claim_point(ClaimPoint::BeforeCommit, &self.path);
        self.commit(claim)
    }

    /// Create the marker no-replace, fill it, and sync it, returning its open handle.
    ///
    /// Called only by [`HeldDir::claim`], under the claim lock. A marker whose
    /// fill fails is removed while its name still links it.
    fn publish_marker(&self) -> io::Result<std::fs::File> {
        let entry = EntryName::new(OsStr::new(OWNERSHIP_MARKER))
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let mut marker = sys::create_new(&self.dir, &entry)?;
        match marker
            .write_all(MARKER_TEXT.as_bytes())
            .and_then(|()| marker.sync_data())
        {
            Ok(()) => Ok(marker),
            Err(e) => {
                let _ = self.unmark_if(&marker);
                Err(e)
            }
        }
    }

    /// Unlink the marker only while its name still links the file `marker` holds.
    fn unmark_if(&self, marker: &std::fs::File) -> Result<(), CliError> {
        let name = OsStr::new(OWNERSHIP_MARKER);
        if self.links_file(name, marker)? {
            self.unlink(name)?;
        }
        Ok(())
    }

    /// Remove the claim file the locked `claim` is, then release the lock.
    ///
    /// The name is unlinked only while it still links the locked file, so a
    /// claimant never removes a claim file another claimant holds.
    fn drop_claim(&self, claim: std::fs::File) -> Result<(), CliError> {
        let name = OsStr::new(CLAIM_FILE);
        if self.links_file(name, &claim)? {
            self.unlink(name)?;
        }
        drop(claim);
        Ok(())
    }

    /// Commit the claim: the claim file is removed, and the directory is ipe's.
    fn commit(&self, claim: std::fs::File) -> Result<Claimed, CliError> {
        self.drop_claim(claim)?;
        Ok(Claimed(()))
    }

    /// Replace the file `name` with the result of `fill`, atomically.
    ///
    /// A link at `name` is refused. The content goes to an exclusively created
    /// temp file (`.<name>.ipe-tmp.<pid>.<n>`) that is renamed over `name`, so
    /// nothing is ever written through an existing entry.
    ///
    /// The marker and claim file names, in any ASCII case, are refused: only
    /// [`HeldDir::claim`] writes either.
    ///
    /// # Errors
    /// [`OutputRefusal::UnsafeComponent`] for the marker or claim file name;
    /// [`OutputRefusal::Symlink`]; [`CliError::Io`] on a filesystem failure.
    pub fn write_file(
        &self,
        name: &OsStr,
        permissions: Option<std::fs::Permissions>,
        fill: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
    ) -> Result<(), CliError> {
        if name.eq_ignore_ascii_case(OWNERSHIP_MARKER) || name.eq_ignore_ascii_case(CLAIM_FILE) {
            return Err(OutputRefusal::UnsafeComponent(self.path.join(name)).into());
        }
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
        fill: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
    ) -> Result<(), CliError> {
        let (target, staging) = (self.entry(name)?, self.entry(tmp)?);
        let tmp_path = self.path.join(tmp);
        let mut staged = sys::create_new(&self.dir, &staging).map_err(|e| act_err(&tmp_path, e))?;
        let filled = fill(&mut staged).and_then(|()| {
            permissions.map_or(Ok(()), |permissions| staged.set_permissions(permissions))
        });
        drop(staged);
        let result = filled.map_err(|e| act_err(&tmp_path, e)).and_then(|()| {
            sys::rename(&self.dir, &staging, &target).map_err(|e| act_err(&self.path.join(name), e))
        });
        if result.is_err() {
            let _ = sys::unlink(&self.dir, &staging);
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
        let entry = self.entry(name)?;
        match sys::unlink(&self.dir, &entry) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(act_err(&self.path.join(name), e)),
        }
    }

    /// Empty the subdirectory `name` at nesting `depth`, then remove it.
    ///
    /// An absent subdirectory is already removed. The removal goes through
    /// [`HeldDir::remove_held`], so it re-proves the name first.
    fn remove_dir(&self, name: &OsStr, depth: usize) -> Result<(), CliError> {
        if depth >= MAX_REMOVE_DEPTH {
            return Err(OutputRefusal::TooDeep {
                path: self.path.join(name),
                limit: MAX_REMOVE_DEPTH,
            }
            .into());
        }
        self.child(name)?
            .map_or(Ok(()), |child| self.remove_held(name, child, depth))
    }

    /// Empty the held subdirectory `child` (at nesting `depth`), then remove the entry `name` it was opened as.
    ///
    /// The contents are removed through `child`'s own handle, so a swap of
    /// `name` after it was opened cannot redirect them; the final removal
    /// re-proves that `name` still names `child`. An entry that vanishes after
    /// the proof is already removed.
    fn remove_held(&self, name: &OsStr, child: Self, depth: usize) -> Result<(), CliError> {
        child.remove_contents(depth)?;
        let released = self.release_proven(name, child)?;
        match self.rmdir_released(released)? {
            Rmdir::Removed | Rmdir::Vanished => Ok(()),
            Rmdir::Refilled(path) => Err(act_err(&path, io::ErrorKind::DirectoryNotEmpty.into())),
        }
    }

    /// Remove every entry of this directory, which sits at nesting `depth`.
    fn remove_contents(&self, depth: usize) -> Result<(), CliError> {
        let names = sys::names(&self.dir).map_err(|e| act_err(&self.path, e))?;
        for name in names {
            let name = match name {
                Ok(name) => name,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(act_err(&self.path, e)),
            };
            match self.kind_of(&name)? {
                EntryKind::Absent => {}
                EntryKind::Directory => self.remove_dir(&name, depth.saturating_add(1))?,
                EntryKind::Symlink | EntryKind::Other => self.unlink(&name)?,
            }
        }
        Ok(())
    }

    /// Open the directory above this one through the handle, not the logical path.
    ///
    /// `Ok(None)` at the filesystem root.
    ///
    /// # Errors
    /// [`CliError::Io`] when the parent cannot be opened or stat'd.
    pub fn parent(&self) -> Result<Option<Self>, CliError> {
        let path = self.path.parent().unwrap_or(&self.path).to_path_buf();
        match self.dir.parent() {
            Ok(dir) => Ok(dir.map(|dir| Self { dir, path })),
            Err(refusal) => Err(refused(&path, refusal)),
        }
    }

    /// Whether looking `path` up now reaches this very directory.
    ///
    /// # Errors
    /// [`CliError::Io`] when `path` cannot be stat'd or the handle cannot be.
    pub fn is_at(&self, path: &Path) -> Result<bool, CliError> {
        let found = DirId::of_path(path).map_err(|refusal| refused(path, refusal))?;
        Ok(found == self.id()?)
    }

    /// Empty the held subdirectory `child`, then remove the entry `name` it was opened as.
    ///
    /// The contents are removed through `child`'s own handle, so a swap of
    /// `name` after it was opened cannot redirect them; the final removal
    /// re-proves that `name` still names `child` before unlinking it.
    ///
    /// # Errors
    /// [`OutputRefusal::Replaced`] when `name` no longer names `child`;
    /// [`OutputRefusal::Symlink`] for a link there; [`OutputRefusal::TooDeep`];
    /// [`OutputRefusal::InUse`] for an entry another program holds open
    /// (Windows); [`CliError::Io`] on another filesystem failure.
    pub fn remove_proven(&self, name: &OsStr, child: Self) -> Result<(), CliError> {
        self.remove_held(name, child, 0)
    }

    /// Remove the held subdirectory `child`, opened as `name`, when it is empty.
    ///
    /// `false` when it is not: emptiness is proven through `child`'s own
    /// handle, so a non-empty directory is kept without a removal attempt. An
    /// empty one is removed once `name` is re-proven to name it; one refilled
    /// in between is kept.
    ///
    /// # Errors
    /// [`OutputRefusal::Replaced`] when `name` no longer names `child`, or
    /// names a non-directory by the time it is removed;
    /// [`OutputRefusal::Symlink`] for a link there; [`OutputRefusal::InUse`]
    /// for an entry another program holds open (Windows); [`CliError::Io`] on
    /// another filesystem failure.
    pub fn remove_empty_dir(&self, name: &OsStr, child: Self) -> Result<bool, CliError> {
        if !child.holds_no_entries()? {
            return Ok(false);
        }
        let released = self.release_proven(name, child)?;
        Ok(matches!(self.rmdir_released(released)?, Rmdir::Removed))
    }

    /// Re-prove that `name` still names the held subdirectory `child`, then release it.
    ///
    /// Every handle to `child` is closed on return, since a held directory
    /// cannot be removed on every platform. The returned proof is the only way
    /// to reach [`HeldDir::rmdir_released`].
    ///
    /// # Errors
    /// [`OutputRefusal::Replaced`] when `name` no longer names `child`;
    /// [`OutputRefusal::Symlink`] or [`OutputRefusal::NotADirectory`] for a
    /// link or a non-directory there; [`CliError::Io`] on a filesystem failure.
    fn release_proven<'n>(&self, name: &'n OsStr, child: Self) -> Result<Released<'n>, CliError> {
        let same = match self.child(name)? {
            Some(now) => now.id()? == child.id()?,
            None => false,
        };
        drop(child);
        let path = self.path.join(name);
        if same {
            Ok(Released::new(name, path))
        } else {
            Err(OutputRefusal::Replaced(path).into())
        }
    }

    /// Remove the subdirectory `released` proved, through the empty-directory-only primitive.
    ///
    /// Whatever was swapped in at the name after the proof is never removed
    /// unless it is itself an empty directory: a non-directory or a Unix
    /// symbolic link is refused as a replacement, and a directory holding
    /// entries is reported refilled and kept. On Windows a directory junction
    /// swapped in between the attribute check and the removal call is removed
    /// as the junction it is — the link only, its target untouched, no data
    /// loss — rather than refused.
    ///
    /// # Errors
    /// [`OutputRefusal::Replaced`] for a non-directory or a Unix symbolic link
    /// at the name; [`OutputRefusal::InUse`] for an entry another program
    /// holds open (Windows); [`CliError::Io`] on another filesystem failure.
    fn rmdir_released(&self, released: Released<'_>) -> Result<Rmdir, CliError> {
        let entry = self.entry(released.name())?;
        subdir_released(released.path());
        let path = released.into_path();
        match sys::rmdir(&self.dir, &entry) {
            Ok(()) => Ok(Rmdir::Removed),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Rmdir::Vanished),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::AlreadyExists
                ) =>
            {
                Ok(Rmdir::Refilled(path))
            }
            Err(e) if is_replacement(&e) => Err(OutputRefusal::Replaced(path).into()),
            Err(e) => Err(act_err(&path, e)),
        }
    }

    /// Whether this directory has no entries at all.
    ///
    /// # Errors
    /// [`CliError::Io`] when the directory cannot be listed.
    fn holds_no_entries(&self) -> Result<bool, CliError> {
        let mut names = sys::names(&self.dir).map_err(|e| act_err(&self.path, e))?;
        names.next().map_or(Ok(true), |name| {
            name.map(|_| false).map_err(|e| act_err(&self.path, e))
        })
    }

    /// Unlink every non-directory entry under this directory whose relative path `keep` rejects.
    ///
    /// `rel` is this directory's path relative to the root `keep` judges and is
    /// restored on return. Directories are descended through held handles and
    /// kept; a link is judged and removed as the link it is, never followed.
    ///
    /// # Errors
    /// [`OutputRefusal::TooDeep`] past `max_depth`; [`OutputRefusal::Symlink`]
    /// when a subdirectory is swapped for a link mid-walk; [`CliError::Io`] on a
    /// filesystem failure.
    pub fn prune<F: Fn(&Path) -> bool>(
        &self,
        rel: &mut PathBuf,
        keep: &F,
        depth: usize,
        max_depth: usize,
    ) -> Result<(), CliError> {
        if depth > max_depth {
            return Err(OutputRefusal::TooDeep {
                path: self.path.clone(),
                limit: max_depth,
            }
            .into());
        }
        let names = sys::names(&self.dir).map_err(|e| act_err(&self.path, e))?;
        for name in names {
            let name = match name {
                Ok(name) => name,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(act_err(&self.path, e)),
            };
            rel.push(&name);
            let result = match self.kind_of(&name) {
                Ok(EntryKind::Absent) => Ok(()),
                Ok(EntryKind::Directory) => self.child(&name).and_then(|child| {
                    child.map_or(Ok(()), |child| {
                        level_held(child.path());
                        child.prune(rel, keep, depth.saturating_add(1), max_depth)
                    })
                }),
                Ok(EntryKind::Symlink | EntryKind::Other) => {
                    if keep(rel) {
                        Ok(())
                    } else {
                        self.unlink(&name)
                    }
                }
                Err(e) => Err(e),
            };
            rel.pop();
            result?;
        }
        Ok(())
    }

    /// Whether the entry `name` is a regular file holding exactly `contents`.
    ///
    /// A link, a non-file, an absent entry, or any read failure counts as not
    /// holding them, so the caller rewrites. At most one byte past `contents`
    /// is read, so a longer file is never read without bound.
    #[must_use]
    pub fn holds_contents(&self, name: &OsStr, contents: &[u8]) -> bool {
        let Some(entry) = EntryName::new(name) else {
            return false;
        };
        let Ok(file) = self.dir.open_regular(&entry) else {
            return false;
        };
        let Ok(len) = u64::try_from(contents.len()) else {
            return false;
        };
        if file.len() != len {
            return false;
        }
        let cap = ByteCap::from_nonzero(NonZeroU64::new(len).unwrap_or(NonZeroU64::MIN));
        file.read_bytes(cap)
            .is_ok_and(|existing| existing == contents)
    }
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

/// Test-only hook run after a subdirectory is re-proven and released, before its removal.
#[cfg(test)]
pub type ReleaseHook = Box<dyn FnMut(&Path)>;

#[cfg(test)]
thread_local! {
    static RELEASE_HOOK: std::cell::RefCell<Option<ReleaseHook>> = const { std::cell::RefCell::new(None) };
}

/// Install (or clear) the hook run between a subdirectory's re-proof and its removal.
///
/// It lets a test swap or refill the entry in the window the proof cannot close.
#[cfg(test)]
pub fn set_release_hook(hook: Option<ReleaseHook>) {
    RELEASE_HOOK.with(|slot| *slot.borrow_mut() = hook);
}

/// Run the release hook, if any, for the subdirectory at `path`.
#[cfg(test)]
fn subdir_released(path: &Path) {
    let taken = RELEASE_HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = taken {
        hook(path);
        RELEASE_HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(hook);
            }
        });
    }
}

/// Outside tests there is no hook: releasing a subdirectory has no side effect.
#[cfg(not(test))]
const fn subdir_released(_path: &Path) {}

/// Test-only hook run at each [`ClaimPoint`] a claim of this thread reaches.
#[cfg(test)]
pub type ClaimHook = Box<dyn FnMut(ClaimPoint, &Path)>;

#[cfg(test)]
thread_local! {
    static CLAIM_HOOK: std::cell::RefCell<Option<ClaimHook>> = const { std::cell::RefCell::new(None) };
}

/// Install (or clear) the hook run at each [`ClaimPoint`] a claim of this thread reaches.
///
/// It lets a test plant, swap, or race an entry between two steps of a claim.
#[cfg(test)]
pub fn set_claim_hook(hook: Option<ClaimHook>) {
    CLAIM_HOOK.with(|slot| *slot.borrow_mut() = hook);
}

/// Run the claim hook, if any, at `point` of a claim of the directory at `path`.
#[cfg(test)]
pub fn claim_point(point: ClaimPoint, path: &Path) {
    let taken = CLAIM_HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = taken {
        hook(point, path);
        CLAIM_HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(hook);
            }
        });
    }
}

/// Outside tests there is no hook: reaching a claim step has no side effect.
#[cfg(not(test))]
pub const fn claim_point(_point: ClaimPoint, _path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::Cell;
    use std::rc::Rc;

    /// A fresh, empty scratch directory unique to this test process.
    fn scratch(tag: &str) -> PathBuf {
        let dir = ipe_test_temp::temp_root().join(format!("ipe_held_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("make scratch");
        std::fs::canonicalize(&dir).expect("canonicalize scratch")
    }

    /// Open `base` and its subdirectory `name` as held handles.
    fn hold(base: &Path, name: &OsStr) -> (HeldDir, HeldDir) {
        let parent = HeldDir::open(base)
            .expect("open base")
            .expect("base exists");
        let child = parent
            .child(name)
            .expect("open child")
            .expect("child exists");
        (parent, child)
    }

    /// Run `swap` once, after the subdirectory at `at` is released and before its removal.
    fn on_release(at: PathBuf, swap: impl FnOnce() + 'static) {
        let mut swap = Some(swap);
        set_release_hook(Some(Box::new(move |released: &Path| {
            if released == at
                && let Some(swap) = swap.take()
            {
                swap();
            }
        })));
    }

    /// An empty directory refilled after its release is kept, its new entry intact.
    #[test]
    fn remove_empty_dir_keeps_a_directory_refilled_after_its_release() {
        let base = scratch("refilled");
        let name = OsStr::new("doomed");
        let doomed = base.join(name);
        std::fs::create_dir(&doomed).expect("make doomed");
        let (parent, child) = hold(&base, name);
        let late = doomed.join("late.txt");
        let written = late.clone();
        on_release(doomed, move || {
            std::fs::write(&written, "late").expect("refill");
        });

        let removed = parent.remove_empty_dir(name, child);
        set_release_hook(None);
        assert!(
            matches!(removed, Ok(false)),
            "the refilled directory is kept, got {removed:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&late).ok().as_deref(),
            Some("late"),
            "the refilled entry survives"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A file swapped in for an empty directory after its release is refused, never removed.
    #[test]
    fn remove_empty_dir_refuses_a_file_swapped_in_after_its_release() {
        let base = scratch("empty_file_swap");
        let name = OsStr::new("doomed");
        let doomed = base.join(name);
        std::fs::create_dir(&doomed).expect("make doomed");
        let (parent, child) = hold(&base, name);
        let (from, aside) = (doomed.clone(), base.join("doomed.aside"));
        on_release(doomed.clone(), move || {
            std::fs::rename(&from, &aside).expect("move doomed aside");
            std::fs::write(&from, "keep").expect("plant file");
        });

        let removed = parent.remove_empty_dir(name, child);
        set_release_hook(None);
        assert!(
            matches!(
                removed,
                Err(CliError::OutputRefused(OutputRefusal::Replaced(_)))
            ),
            "the swapped-in file is refused, got {removed:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&doomed).ok().as_deref(),
            Some("keep"),
            "the swapped-in file survives"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A symlink swapped in for an empty directory after its release is refused, never removed.
    #[cfg(unix)]
    #[test]
    fn remove_empty_dir_refuses_a_symlink_swapped_in_after_its_release() {
        let base = scratch("empty_symlink_swap");
        let name = OsStr::new("doomed");
        let doomed = base.join(name);
        std::fs::create_dir(&doomed).expect("make doomed");
        let (parent, child) = hold(&base, name);
        let (from, aside) = (doomed.clone(), base.join("doomed.aside"));
        on_release(doomed.clone(), move || {
            std::fs::rename(&from, &aside).expect("move doomed aside");
            std::os::unix::fs::symlink(&aside, &from).expect("plant symlink");
        });

        let removed = parent.remove_empty_dir(name, child);
        set_release_hook(None);
        assert!(
            matches!(
                removed,
                Err(CliError::OutputRefused(OutputRefusal::Replaced(_)))
            ),
            "the swapped-in symlink is refused, got {removed:?}"
        );
        assert!(
            std::fs::symlink_metadata(&doomed).is_ok_and(|meta| meta.file_type().is_symlink()),
            "the swapped-in symlink survives"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A file swapped in for an emptied tree after its release is refused, never removed.
    #[test]
    fn remove_proven_refuses_a_file_swapped_in_after_its_release() {
        let base = scratch("proven_file_swap");
        let name = OsStr::new("doomed");
        let doomed = base.join(name);
        std::fs::create_dir_all(doomed.join("sub")).expect("make tree");
        std::fs::write(doomed.join("sub").join("f.txt"), "gone").expect("tree file");
        let (parent, child) = hold(&base, name);
        let (from, aside) = (doomed.clone(), base.join("doomed.aside"));
        on_release(doomed.clone(), move || {
            std::fs::rename(&from, &aside).expect("move doomed aside");
            std::fs::write(&from, "keep").expect("plant file");
        });

        let removed = parent.remove_proven(name, child);
        set_release_hook(None);
        assert!(
            matches!(
                removed,
                Err(CliError::OutputRefused(OutputRefusal::Replaced(_)))
            ),
            "the swapped-in file is refused, got {removed:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&doomed).ok().as_deref(),
            Some("keep"),
            "the swapped-in file survives"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An inner directory of a removed tree is re-proven before its removal.
    ///
    /// On Unix a held directory can be renamed away, so the test swaps an
    /// empty impostor in while the tree below it is removed: the removal is
    /// refused and the impostor survives. On Windows a held directory cannot
    /// be renamed at all, so the swap itself is refused and the tree goes.
    #[test]
    fn an_inner_directory_swapped_mid_removal_is_never_removed_by_name() {
        let base = scratch("inner_swap");
        let inner = base.join("tree").join("inner");
        std::fs::create_dir_all(inner.join("leaf")).expect("make tree");
        std::fs::write(inner.join("leaf").join("f.txt"), "gone").expect("tree file");
        let parent = HeldDir::open(&base.join("tree"))
            .expect("open tree")
            .expect("tree exists");
        let swapped = Rc::new(Cell::new(None));
        let seen = Rc::clone(&swapped);
        let (from, aside) = (inner.clone(), base.join("inner.aside"));
        on_release(inner.join("leaf"), move || {
            let moved = std::fs::rename(&from, &aside).is_ok();
            if moved {
                std::fs::create_dir(&from).expect("plant impostor");
            }
            seen.set(Some(moved));
        });

        let removed = parent.remove_entry(OsStr::new("inner"));
        set_release_hook(None);
        #[cfg(unix)]
        {
            assert_eq!(swapped.get(), Some(true), "a held directory moves on Unix");
            assert!(
                matches!(
                    removed,
                    Err(CliError::OutputRefused(OutputRefusal::Replaced(_)))
                ),
                "the swapped inner directory is refused, got {removed:?}"
            );
            assert!(inner.is_dir(), "the impostor survives");
        }
        #[cfg(windows)]
        {
            assert_eq!(swapped.get(), Some(false), "a held directory cannot move");
            assert!(
                removed.is_ok(),
                "the unswapped tree is removed, got {removed:?}"
            );
            assert!(!inner.exists(), "the tree is gone");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A held subdirectory another handle keeps open without delete sharing is refused, in use.
    #[cfg(windows)]
    #[test]
    fn remove_entry_of_a_held_subdirectory_in_use_elsewhere_is_refused() {
        use std::os::windows::fs::OpenOptionsExt as _;

        /// `FILE_FLAG_BACKUP_SEMANTICS`: allows opening a directory handle.
        const BACKUP_SEMANTICS: u32 = 0x0200_0000;
        /// `FILE_SHARE_READ | FILE_SHARE_WRITE`, without `FILE_SHARE_DELETE`.
        const SHARE_NO_DELETE: u32 = 0x1 | 0x2;

        let base = scratch("win_in_use");
        let name = OsStr::new("busy");
        let busy = base.join(name);
        std::fs::create_dir(&busy).expect("make busy");
        let other = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(SHARE_NO_DELETE)
            .custom_flags(BACKUP_SEMANTICS)
            .open(&busy)
            .expect("hold busy open elsewhere");

        let parent = HeldDir::open(&base)
            .expect("open base")
            .expect("base exists");
        let removed = parent.remove_entry(name);
        assert!(
            matches!(
                removed,
                Err(CliError::OutputRefused(OutputRefusal::InUse(_)))
            ),
            "the in-use directory is refused, got {removed:?}"
        );
        assert!(busy.is_dir(), "the busy directory survives");

        drop(other);
        parent
            .remove_entry(name)
            .expect("removable once the other handle releases it");
        assert!(!busy.exists(), "the directory is gone once free");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An empty scratch directory `out` under a fresh base, held open.
    fn held_empty(tag: &str) -> (PathBuf, HeldDir) {
        let base = scratch(tag);
        let out = base.join("out");
        std::fs::create_dir(&out).expect("make out");
        let dir = HeldDir::open(&out).expect("open out").expect("out exists");
        (base, dir)
    }

    /// Run `act` at every `at` step a claim of this thread reaches.
    fn on_claim(at: ClaimPoint, mut act: impl FnMut(&Path) + 'static) {
        set_claim_hook(Some(Box::new(move |point: ClaimPoint, path: &Path| {
            if point == at {
                act(path);
            }
        })));
    }

    /// The identity of the entry `name` of `dir`, read without following a link.
    fn id_at(dir: &Path, name: &str) -> DirId {
        HeldDir::open(dir)
            .expect("open dir")
            .expect("dir exists")
            .dir
            .entry_id(&EntryName::new(OsStr::new(name)).expect("plain name"))
            .expect("entry identity")
            .expect("entry present")
    }

    /// Whether `dir` holds an entry named `name`, of any kind.
    fn present(dir: &Path, name: &str) -> bool {
        std::fs::symlink_metadata(dir.join(name)).is_ok()
    }

    /// Whether `result` is the refusal `NotIpeOwned`.
    const fn not_ipe_owned<T>(result: &Result<T, CliError>) -> bool {
        matches!(
            result,
            Err(CliError::OutputRefused(OutputRefusal::NotIpeOwned(_)))
        )
    }

    /// A file planted after the pre-check refuses the claim, and is kept byte for byte.
    #[test]
    fn a_file_planted_after_the_pre_check_is_refused_and_kept() {
        let (base, dir) = held_empty("planted_after_pre");
        on_claim(ClaimPoint::AfterPublish, |path| {
            std::fs::write(path.join("user.txt"), "user bytes").expect("plant user file");
        });
        let claimed = dir.claim();
        set_claim_hook(None);
        assert!(not_ipe_owned(&claimed), "got {claimed:?}");
        let out = dir.path();
        assert!(!present(out, OWNERSHIP_MARKER), "the marker is unlinked");
        assert!(!present(out, CLAIM_FILE), "the claim file is removed");
        assert_eq!(
            std::fs::read(out.join("user.txt")).ok().as_deref(),
            Some(b"user bytes".as_slice()),
            "the user file is kept"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A file planted at the marker name before the publish is never replaced.
    #[test]
    fn a_marker_name_planted_before_publish_is_never_replaced() {
        let (base, dir) = held_empty("planted_marker");
        on_claim(ClaimPoint::AfterPreCheck, |path| {
            std::fs::write(path.join(OWNERSHIP_MARKER), "user bytes").expect("plant marker name");
        });
        let claimed = dir.claim();
        set_claim_hook(None);
        assert!(not_ipe_owned(&claimed), "got {claimed:?}");
        let out = dir.path();
        assert_eq!(
            std::fs::read(out.join(OWNERSHIP_MARKER)).ok().as_deref(),
            Some(b"user bytes".as_slice()),
            "the planted file keeps its bytes"
        );
        assert!(!present(out, CLAIM_FILE), "the claim file is removed");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Two claims racing on one empty directory, each then writing a file, leave it owned.
    #[test]
    fn two_claims_on_one_empty_dir_never_unmark_each_other() {
        const CLAIMANTS: usize = 2;
        let base = scratch("two_claims");
        for round in 0..200 {
            let out = base.join(format!("r{round}"));
            std::fs::create_dir(&out).expect("make round dir");
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(CLAIMANTS));
            let claims: Vec<_> = (0..CLAIMANTS)
                .map(|n| {
                    let (out, barrier) = (out.clone(), std::sync::Arc::clone(&barrier));
                    std::thread::Builder::new()
                        .spawn(move || {
                            let dir = HeldDir::open(&out)
                                .and_then(|dir| {
                                    dir.ok_or_else(|| io_err(&out, io::ErrorKind::NotFound.into()))
                                })
                                .map_err(|e| format!("{e:?}"))?;
                            barrier.wait();
                            dir.claim().map(|_| ()).map_err(|e| format!("{e:?}"))?;
                            dir.write_file(OsStr::new(&format!("f{n}.txt")), None, |file| {
                                file.write_all(b"built")
                            })
                            .map_err(|e| format!("{e:?}"))
                        })
                        .expect("spawn claim thread")
                })
                .collect();
            for (n, claim) in claims.into_iter().enumerate() {
                let result = claim.join().expect("claim thread");
                assert!(result.is_ok(), "round {round}: claimant {n}: {result:?}");
            }
            let dir = HeldDir::open(&out).expect("open out").expect("out exists");
            let now = dir.owned_now();
            assert!(
                matches!(now, Ok(OwnedNow::Owned)),
                "round {round}: the directory stays ipe's, got {now:?}"
            );
            assert!(!present(&out, CLAIM_FILE), "round {round}: no claim file");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A claim locked after another claim committed and filled the directory returns `Claimed`.
    #[test]
    fn a_pending_claim_over_a_committed_marker_returns_claimed() {
        let (base, dir) = held_empty("pending_over_committed");
        let seen = Rc::new(Cell::new(None));
        let record = Rc::clone(&seen);
        on_claim(ClaimPoint::AfterLock, move |path| {
            if record.get().is_none() {
                std::fs::write(path.join(OWNERSHIP_MARKER), MARKER_TEXT).expect("commit marker");
                std::fs::write(path.join("a.txt"), "built").expect("fill");
                record.set(Some(id_at(path, OWNERSHIP_MARKER)));
            }
        });
        let claimed = dir.claim();
        set_claim_hook(None);
        assert!(claimed.is_ok(), "got {claimed:?}");
        let out = dir.path();
        assert_eq!(
            seen.get(),
            Some(id_at(out, OWNERSHIP_MARKER)),
            "the committed marker is the one still linked"
        );
        assert!(
            out.join("a.txt").is_file(),
            "the other claim's file is kept"
        );
        assert!(!present(out, CLAIM_FILE), "the claim file is removed");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A claim file a crashed claim left empty is taken over in place.
    #[test]
    fn a_stale_pending_claim_is_taken_over_in_place() {
        let (base, dir) = held_empty("stale_pending");
        let out = dir.path().to_path_buf();
        drop(std::fs::File::create(out.join(CLAIM_FILE)).expect("leave a claim file"));
        let planted = id_at(&out, CLAIM_FILE);
        let locked = Rc::new(Cell::new(None));
        let record = Rc::clone(&locked);
        on_claim(ClaimPoint::AfterLock, move |path| {
            record.set(Some(id_at(path, CLAIM_FILE)));
        });
        let claimed = dir.claim();
        set_claim_hook(None);
        assert!(claimed.is_ok(), "got {claimed:?}");
        assert_eq!(
            locked.get(),
            Some(planted),
            "the left claim file is the one locked"
        );
        assert!(
            dir.has_marker().expect("read ownership"),
            "the directory is ipe's"
        );
        assert!(!present(&out, CLAIM_FILE), "the claim file is removed");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A claim file locked elsewhere refuses `ClaimBusy` after exactly `MAX_CLAIM_POLLS` polls.
    #[test]
    fn a_live_claim_refuses_busy_at_the_bound() {
        let (base, dir) = held_empty("live_claim");
        let out = dir.path().to_path_buf();
        let holder = std::fs::File::create(out.join(CLAIM_FILE)).expect("create claim file");
        holder.try_lock().expect("hold the claim lock");
        let polls = Rc::new(Cell::new(0_u32));
        let count = Rc::clone(&polls);
        on_claim(ClaimPoint::Polled, move |_| {
            count.set(count.get().saturating_add(1));
        });
        let claimed = dir.claim();
        set_claim_hook(None);
        assert!(
            matches!(
                claimed,
                Err(CliError::OutputRefused(OutputRefusal::ClaimBusy { .. }))
            ),
            "got {claimed:?}"
        );
        assert_eq!(polls.get(), MAX_CLAIM_POLLS, "the wait is bounded");
        assert!(
            !present(&out, OWNERSHIP_MARKER),
            "a busy claim writes nothing"
        );
        drop(holder);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A claim whose locked claim file was unlinked and recreated never publishes on the old file.
    #[cfg(unix)]
    #[test]
    fn a_claim_on_an_unlinked_inode_reevaluates() {
        let (base, dir) = held_empty("unlinked_claim");
        let aside = base.join("old.claim");
        let moved = aside.clone();
        let mut swapped = false;
        on_claim(ClaimPoint::AfterLock, move |path| {
            if !swapped {
                swapped = true;
                std::fs::rename(path.join(CLAIM_FILE), &moved).expect("move claim aside");
                drop(std::fs::File::create(path.join(CLAIM_FILE)).expect("recreate claim"));
            }
        });
        let claimed = dir.claim();
        set_claim_hook(None);
        assert!(claimed.is_ok(), "got {claimed:?}");
        assert_eq!(
            std::fs::metadata(&aside).map(|meta| meta.len()).ok(),
            Some(0),
            "no phase byte is ever written to the unlinked claim file"
        );
        assert!(
            dir.has_marker().expect("read ownership"),
            "the directory is ipe's"
        );
        assert!(
            !present(dir.path(), CLAIM_FILE),
            "the claim file is removed"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A finalizing claim's crash is refused beside a user file and taken over otherwise.
    #[test]
    fn a_finalizing_crash_with_a_user_file_is_interrupted() {
        let (base, dir) = held_empty("finalizing_user");
        let out = dir.path().to_path_buf();
        std::fs::write(out.join(CLAIM_FILE), [1_u8]).expect("finalizing claim file");
        std::fs::write(out.join(OWNERSHIP_MARKER), MARKER_TEXT).expect("marker");
        std::fs::write(out.join("user.txt"), "mine").expect("user file");
        let claimed = dir.claim();
        assert!(
            matches!(
                claimed,
                Err(CliError::OutputRefused(OutputRefusal::ClaimInterrupted(_)))
            ),
            "got {claimed:?}"
        );
        for name in [CLAIM_FILE, OWNERSHIP_MARKER, "user.txt"] {
            assert!(present(&out, name), "{name} is kept");
        }

        let (base_marked, marked) = held_empty("finalizing_marked");
        let out = marked.path().to_path_buf();
        std::fs::write(out.join(CLAIM_FILE), [1_u8]).expect("finalizing claim file");
        std::fs::write(out.join(OWNERSHIP_MARKER), MARKER_TEXT).expect("marker");
        let claimed = marked.claim();
        assert!(
            claimed.is_ok(),
            "a published marker is committed, got {claimed:?}"
        );
        assert!(!present(&out, CLAIM_FILE), "the claim file is removed");

        let (base_unpublished, unpublished) = held_empty("finalizing_bare");
        let out = unpublished.path().to_path_buf();
        std::fs::write(out.join(CLAIM_FILE), [1_u8]).expect("finalizing claim file");
        let claimed = unpublished.claim();
        assert!(
            claimed.is_ok(),
            "an unpublished claim is redone, got {claimed:?}"
        );
        assert!(
            unpublished.has_marker().expect("read ownership"),
            "the marker is published"
        );
        assert!(!present(&out, CLAIM_FILE), "the claim file is removed");
        for base in [base, base_marked, base_unpublished] {
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    /// Every entry the listing does not tolerate, by name and kind, refuses the claim.
    #[test]
    fn listing_is_kind_aware() {
        let (base_temp_dir, temp_dir) = held_empty("kind_temp_dir");
        let leftover = temp_dir.path().join(".ipe-output.1.2.tmp");
        std::fs::create_dir(&leftover).expect("temp-named directory");
        std::fs::write(leftover.join("f.txt"), "mine").expect("file inside");
        let claimed = temp_dir.claim();
        assert!(
            not_ipe_owned(&claimed),
            "a temp-named directory, got {claimed:?}"
        );
        assert!(leftover.join("f.txt").is_file(), "its file is kept");
        assert!(!present(temp_dir.path(), CLAIM_FILE), "no claim file left");

        let (base_temp_file, temp_file) = held_empty("kind_temp_file");
        std::fs::write(temp_file.path().join(".ipe-output.1.2.tmp"), "mine").expect("temp file");
        let claimed = temp_file.claim();
        assert!(
            not_ipe_owned(&claimed),
            "a temp-named file, got {claimed:?}"
        );
        assert!(!present(temp_file.path(), CLAIM_FILE), "no claim file left");

        let (base_claim_dir, claim_dir) = held_empty("kind_claim_dir");
        std::fs::create_dir(claim_dir.path().join(CLAIM_FILE)).expect("directory at claim name");
        let claimed = claim_dir.claim();
        assert!(
            not_ipe_owned(&claimed),
            "a directory at the claim name, got {claimed:?}"
        );
        assert!(claim_dir.path().join(CLAIM_FILE).is_dir(), "it is kept");
        assert!(
            !present(claim_dir.path(), OWNERSHIP_MARKER),
            "no marker written"
        );

        #[cfg(unix)]
        {
            let (base_link, link) = held_empty("kind_marker_link");
            let target = base_link.join("elsewhere");
            std::fs::write(&target, MARKER_TEXT).expect("link target");
            std::os::unix::fs::symlink(&target, link.path().join(OWNERSHIP_MARKER))
                .expect("link at the marker name");
            let claimed = link.claim();
            assert!(
                not_ipe_owned(&claimed),
                "a link at the marker name, got {claimed:?}"
            );
            assert!(!present(link.path(), CLAIM_FILE), "no claim file left");
            assert_eq!(
                std::fs::read_to_string(&target).ok().as_deref(),
                Some(MARKER_TEXT),
                "the link target is untouched"
            );
            let _ = std::fs::remove_dir_all(&base_link);
        }
        for base in [base_temp_dir, base_temp_file, base_claim_dir] {
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    /// A marker beside a claim file is not ownership, nor is one swapped while it is read.
    #[test]
    fn owned_now_is_false_while_claiming_or_swapped() {
        let (base, dir) = held_empty("owned_now");
        let out = dir.path().to_path_buf();
        std::fs::write(out.join(OWNERSHIP_MARKER), MARKER_TEXT).expect("marker");
        std::fs::write(out.join(CLAIM_FILE), b"").expect("claim file");
        let now = dir.owned_now();
        assert!(matches!(now, Ok(OwnedNow::Claiming)), "got {now:?}");
        assert!(
            !dir.has_marker().expect("read ownership"),
            "a claim in flight is not ownership"
        );
        std::fs::remove_file(out.join(CLAIM_FILE)).expect("remove claim file");
        let now = dir.owned_now();
        assert!(matches!(now, Ok(OwnedNow::Owned)), "got {now:?}");

        #[cfg(unix)]
        {
            let aside = base.join("old.marker");
            on_claim(ClaimPoint::MarkerRead, move |path| {
                std::fs::rename(path.join(OWNERSHIP_MARKER), &aside).expect("move marker aside");
                std::fs::write(path.join(OWNERSHIP_MARKER), MARKER_TEXT).expect("swap marker");
            });
            let now = dir.owned_now();
            set_claim_hook(None);
            assert!(matches!(now, Ok(OwnedNow::Unowned)), "got {now:?}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A claim file this protocol never wrote is refused, and is neither written to nor removed.
    #[test]
    fn a_claim_name_ipe_never_wrote_is_refused_and_kept() {
        let (base, dir) = held_empty("foreign_claim_file");
        let out = dir.path().to_path_buf();
        std::fs::write(out.join(CLAIM_FILE), "user notes").expect("user file at the claim name");
        let claimed = dir.claim();
        assert!(not_ipe_owned(&claimed), "got {claimed:?}");
        assert_eq!(
            std::fs::read(out.join(CLAIM_FILE)).ok().as_deref(),
            Some(b"user notes".as_slice()),
            "the user file keeps its bytes"
        );
        assert!(!present(&out, OWNERSHIP_MARKER), "no marker is written");

        std::fs::write(out.join(CLAIM_FILE), b"u").expect("one foreign byte");
        let claimed = dir.claim();
        assert!(not_ipe_owned(&claimed), "a foreign byte, got {claimed:?}");
        assert_eq!(
            std::fs::read(out.join(CLAIM_FILE)).ok().as_deref(),
            Some(b"u".as_slice()),
            "the one-byte file is kept"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A claim name hard-linked to a file elsewhere is refused, and the linked file is untouched.
    #[test]
    fn a_hard_linked_claim_name_is_refused_and_its_file_untouched() {
        let (base, dir) = held_empty("linked_claim_file");
        let out = dir.path().to_path_buf();
        for bytes in [b"".as_slice(), [PHASE_FINALIZING].as_slice()] {
            let victim = base.join("victim");
            std::fs::write(&victim, bytes).expect("victim file");
            std::fs::hard_link(&victim, out.join(CLAIM_FILE)).expect("link the claim name");
            let claimed = dir.claim();
            assert!(not_ipe_owned(&claimed), "got {claimed:?}");
            assert_eq!(
                std::fs::read(&victim).ok().as_deref(),
                Some(bytes),
                "the linked file is never written to"
            );
            assert!(present(&out, CLAIM_FILE), "the link is kept");
            assert!(!present(&out, OWNERSHIP_MARKER), "no marker is written");
            std::fs::remove_file(out.join(CLAIM_FILE)).expect("drop the link");
            std::fs::remove_file(&victim).expect("drop the victim");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `write_file` refuses the marker and claim file names, in any ASCII case.
    #[test]
    fn write_file_refuses_the_marker_and_claim_names() {
        let (base, dir) = held_empty("write_reserved");
        let _claimed = dir.claim().expect("claim");
        for name in [
            OWNERSHIP_MARKER,
            CLAIM_FILE,
            ".IPE-OUTPUT",
            ".Ipe-Output.Claim",
        ] {
            let wrote = dir.write_file(OsStr::new(name), None, |file| file.write_all(b"forged"));
            assert!(
                matches!(
                    wrote,
                    Err(CliError::OutputRefused(OutputRefusal::UnsafeComponent(_)))
                ),
                "{name} is refused, got {wrote:?}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join(OWNERSHIP_MARKER))
                .ok()
                .as_deref(),
            Some(MARKER_TEXT),
            "the marker is untouched"
        );
        assert!(!present(dir.path(), CLAIM_FILE), "no claim file is written");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An empty subdirectory turned into a junction is refused as a link, never entered.
    #[cfg(windows)]
    #[test]
    fn a_junction_child_is_link() {
        use super::super::test_links::junction_in_place;

        let base = scratch("junction_child");
        let victim = base.join("victim");
        std::fs::create_dir(&victim).expect("make victim");
        std::fs::write(victim.join("keep.txt"), "keep").expect("victim file");
        let link = base.join("link");
        std::fs::create_dir(&link).expect("make link dir");
        junction_in_place(&link, &victim);

        let held = HeldDir::open(&base)
            .expect("open base")
            .expect("base exists");
        let name = EntryName::new(OsStr::new("link")).expect("plain name");
        let refused = held.dir.child_dir(&name);
        assert_eq!(refused.err(), Some(OpenRefusal::Link));
        let child = held.child(OsStr::new("link"));
        assert!(
            matches!(
                child,
                Err(CliError::OutputRefused(OutputRefusal::Symlink(_)))
            ),
            "the junction is refused as a link, got {child:?}"
        );
        assert!(victim.join("keep.txt").is_file(), "the target is untouched");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A held empty directory turned into a junction refuses its listing rather than listing the target.
    #[cfg(windows)]
    #[test]
    fn a_held_empty_dir_turned_junction_refuses_entries() {
        use super::super::test_links::junction_in_place;

        let base = scratch("junction_held");
        let victim = base.join("victim");
        std::fs::create_dir(&victim).expect("make victim");
        std::fs::write(victim.join("keep.txt"), "keep").expect("victim file");
        let level = base.join("level");
        std::fs::create_dir(&level).expect("make level");
        let held = HeldDir::open(&level)
            .expect("open level")
            .expect("level exists");
        junction_in_place(&level, &victim);

        let cap = ipe_fs_open::EntryCap::new(16).expect("non-zero cap");
        let listed = held.dir.entries(cap);
        assert_eq!(listed.err(), Some(OpenRefusal::Link));
        assert!(victim.join("keep.txt").is_file(), "the target is untouched");
        drop(held);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A directory at the marker name is refused, never read as an empty directory awaiting its marker.
    #[test]
    fn a_directory_at_the_marker_name_refuses_ownership() {
        let base = scratch("marker_dir");
        let marker = base.join(OWNERSHIP_MARKER);
        std::fs::create_dir(&marker).expect("make marker-named dir");
        std::fs::write(marker.join("keep.txt"), "keep").expect("user file");
        let held = HeldDir::open(&base)
            .expect("open base")
            .expect("base exists");
        let ownership = held.ownership();
        assert!(
            matches!(ownership, Ok(Ownership::User)),
            "a directory at the marker name is user territory, got {ownership:?}"
        );
        let claimed = held.claim();
        assert!(
            matches!(
                claimed,
                Err(CliError::OutputRefused(OutputRefusal::NotIpeOwned(_)))
            ),
            "no claim takes it, got {claimed:?}"
        );
        assert!(
            marker.join("keep.txt").is_file(),
            "the user file is untouched"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A symbolic link at the marker name is no marker, and never followed.
    #[cfg(unix)]
    #[test]
    fn a_link_at_the_marker_name_is_no_marker() {
        let base = scratch("marker_link");
        let elsewhere = base.join("elsewhere");
        std::fs::write(&elsewhere, MARKER_TEXT).expect("write lookalike marker");
        let level = base.join("level");
        std::fs::create_dir(&level).expect("make level");
        std::os::unix::fs::symlink(&elsewhere, level.join(OWNERSHIP_MARKER)).expect("link marker");
        let held = HeldDir::open(&level)
            .expect("open level")
            .expect("level exists");
        let marked = held.has_marker();
        assert!(
            matches!(marked, Ok(false)),
            "a linked marker is not followed, got {marked:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A subdirectory another program holds open without sharing is refused, in use, when entered.
    #[cfg(windows)]
    #[test]
    fn a_child_held_open_elsewhere_is_in_use() {
        use std::os::windows::fs::OpenOptionsExt as _;

        /// `FILE_FLAG_BACKUP_SEMANTICS`: allows opening a directory handle.
        const BACKUP_SEMANTICS: u32 = 0x0200_0000;

        let base = scratch("win_child_in_use");
        let busy = base.join("busy");
        std::fs::create_dir(&busy).expect("make busy");
        let other = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .custom_flags(BACKUP_SEMANTICS)
            .open(&busy)
            .expect("hold busy open elsewhere");
        let parent = HeldDir::open(&base)
            .expect("open base")
            .expect("base exists");
        let child = parent.child(OsStr::new("busy"));
        assert!(
            matches!(child, Err(CliError::OutputRefused(OutputRefusal::InUse(_)))),
            "the in-use subdirectory is refused, got {child:?}"
        );
        drop(other);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An in-use refusal stays in use on every platform; every other refusal is an I/O error of its kind.
    #[test]
    fn every_open_refusal_maps_to_its_cli_error() {
        let path = Path::new("held/entry");
        let in_use = refused(path, OpenRefusal::InUse);
        assert!(
            matches!(&in_use, CliError::OutputRefused(OutputRefusal::InUse(p)) if p == path),
            "an in-use entry is refused as in use, got {in_use:?}"
        );
        let cap = ByteCap::new(4).expect("non-zero cap");
        let entries = ipe_fs_open::EntryCap::new(4).expect("non-zero cap");
        for refusal in [
            OpenRefusal::Absent,
            OpenRefusal::Link,
            OpenRefusal::NotRegular(FileKind::Fifo),
            OpenRefusal::Denied,
            OpenRefusal::TooLarge(cap),
            OpenRefusal::TooManyEntries(entries),
            OpenRefusal::BadName,
            OpenRefusal::NotUtf8,
            OpenRefusal::Io(io::ErrorKind::Interrupted),
        ] {
            let error = refused(path, refusal);
            assert!(
                matches!(&error, CliError::Io { source, .. } if source.kind() == refusal.kind()),
                "{refusal:?} is an I/O error of its kind, got {error:?}"
            );
        }
    }
}

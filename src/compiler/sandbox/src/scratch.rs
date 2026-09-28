//! Private scratch directories and files, the one primitive every compiler-side temporary write goes through.
//!
//! The invariant (a trusted base, exclusive `O_EXCL` + `O_NOFOLLOW` creation,
//! owner-only modes, a CSPRNG name, a re-verified entry, one-component
//! [`ScratchLeaf`] children) is defined once, in the runtime's
//! `src/runtime/rust/src/scratch.rs`, which this module compiles verbatim and
//! re-exports. This module adds only the compiler's RAII owners and supplies
//! the two host inputs the shared core takes as arguments: the OS CSPRNG and
//! the user's profile directory.
//!
//! A scratch path is handed to external writers (`curl -o`, `sh`, the jail)
//! that re-resolve it by name; read what they wrote back through the retained
//! [`ScratchFile`] handle, never by re-opening the name.

use std::ffi::OsString;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[path = "../../../runtime/rust/src/scratch.rs"]
mod shared;

pub use shared::*;

/// Fill `buf` from the OS CSPRNG; an unavailable CSPRNG is an error, never a weaker source.
fn os_entropy(buf: &mut [u8]) -> io::Result<()> {
    getrandom::fill(buf).map_err(io::Error::from)
}

/// The user's profile directory, which proves a base trusted where ownership cannot.
fn profile_dir() -> Option<OsString> {
    crate::home::home_dir().map(PathBuf::into_os_string)
}

/// The raw inherited scratch anchor a jail launcher hands down (see [`ScratchAnchor`]).
fn anchor_env() -> Option<OsString> {
    ipe_env::var_os(ANCHOR_VAR)
}

// ── ScratchDir ───────────────────────────────────────────────────────────────

/// A verified private temporary directory, removed with its contents on drop.
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
    /// tag only, confined by [`confined_label`].
    ///
    /// # Errors
    /// See [`create_private_dir`].
    pub fn new_under(base: &Path, label: &str) -> io::Result<Self> {
        create_private_dir(base, label, os_entropy, profile_dir, anchor_env).map(Self)
    }

    /// The path of this scratch directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Build the path of the entry `leaf` directly inside this directory (not created by this call).
    #[must_use]
    pub fn child(&self, leaf: &ScratchLeaf) -> PathBuf {
        self.0.join(leaf)
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
/// The owned [`File`](std::fs::File) handle outlives the name: read back what
/// an external writer wrote through [`ScratchFile::read_all`], never by
/// re-opening the path. The file and its directory are removed on drop.
#[derive(Debug)]
pub struct ScratchFile {
    /// The open handle; [`ScratchFile::rewind`] before reading external writes.
    pub file: std::fs::File,
    path: PathBuf,
    // Declared last so the handle closes before the directory is removed.
    dir: ScratchDir,
}

impl ScratchFile {
    /// Create a private file inside a fresh private directory under the OS temp root.
    ///
    /// Both the directory and the file are named from `label`: the directory as
    /// in [`ScratchDir::new_under`], the file as the confined label itself.
    ///
    /// # Errors
    /// See [`ScratchDir::new_under`] and [`ScratchFile::create_in`].
    pub fn create(label: &str) -> io::Result<Self> {
        let leaf = ScratchLeaf::new(&confined_label(label))?;
        Self::create_in(ScratchDir::new(label)?, &leaf)
    }

    /// Create the file `leaf` inside the private directory `dir`, taking ownership of it.
    ///
    /// # Errors
    /// See [`create_private_file`].
    pub fn create_in(dir: ScratchDir, leaf: &ScratchLeaf) -> io::Result<Self> {
        let (path, file) = create_private_file(dir.path(), leaf)?;
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

// ── HeldScratchAnchor ────────────────────────────────────────────────────────

/// A host-proven private directory held open for a jailed process to inherit as its [`ScratchAnchor`].
///
/// [`HeldScratchAnchor::hold`] proves the directory with the full ancestor
/// walk (on the host, where owners are real), opens it WITHOUT close-on-exec,
/// and confirms the descriptor holds exactly the proven node. Keep the value
/// alive until the process is replaced: the descriptor then survives the exec
/// into the jail, where [`inherited_anchor`] re-judges what it holds.
#[cfg(unix)]
#[derive(Debug)]
pub struct HeldScratchAnchor {
    held: std::fs::File,
    anchor: ScratchAnchor,
}

#[cfg(unix)]
impl HeldScratchAnchor {
    /// Prove `dir` and hold it open as an inheritable anchor.
    ///
    /// # Errors
    /// `PermissionDenied` when `dir` is not a private directory of the
    /// effective user under trusted ancestors, or when the opened descriptor
    /// holds another node; the lookup or `open` error otherwise.
    pub fn hold(dir: &Path) -> io::Result<Self> {
        use rustix::fs::{Mode, OFlags};
        use std::os::fd::AsRawFd as _;
        let (canonical, node) = prove_anchor_dir(dir)?;
        // No `CLOEXEC`: the descriptor must survive the exec into the jail.
        let fd = rustix::fs::open(
            &canonical,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        let held = std::fs::File::from(fd);
        let raw = u32::try_from(held.as_raw_fd()).map_err(io::Error::other)?;
        let anchor = verify_anchor_handle(&canonical, &held, raw, node)?;
        Ok(Self { held, anchor })
    }

    /// The anchor the held descriptor stands for.
    #[must_use]
    pub const fn anchor(&self) -> ScratchAnchor {
        self.anchor
    }

    /// The open directory handle.
    #[must_use]
    pub const fn file(&self) -> &std::fs::File {
        &self.held
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(name: &str) -> io::Result<ScratchLeaf> {
        ScratchLeaf::new(name).map_err(io::Error::from)
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

    /// A missing base outside the user's profile is refused without creating
    /// any of its ancestors.
    #[cfg(windows)]
    #[test]
    fn windows_missing_base_outside_profile_creates_nothing() {
        let system_root = ipe_env::var_os("SystemRoot");
        assert!(system_root.is_some(), "SystemRoot must be set on Windows");
        let Some(system_root) = system_root else {
            return;
        };
        let missing =
            Path::new(&system_root).join(format!("ipe-missing-scratch-{}", std::process::id()));
        let refused = ScratchDir::new_under(&missing.join("inner"), "ipe-test");
        assert_eq!(
            refused.map(drop).map_err(|e| e.kind()),
            Err(io::ErrorKind::PermissionDenied)
        );
        assert!(!missing.exists(), "a refused base must not be created");
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
        let root = ScratchDir::new("ipe-scratch-unique")?;
        let base = std::fs::canonicalize(root.path())?;
        let a = ScratchDir::new_under(root.path(), "ipe-test")?;
        let b = ScratchDir::new_under(root.path(), "ipe-test")?;
        let c =
            ScratchFile::create_in(ScratchDir::new_under(root.path(), "ipe-test")?, &leaf("f")?)?;
        assert_ne!(a.path(), b.path());
        for entry in [a.path(), b.path(), c.dir().path()] {
            assert_eq!(entry.parent(), Some(base.as_path()));
        }
        Ok(())
    }

    /// A child path is exactly one component below its directory.
    #[test]
    fn child_stays_directly_inside_the_directory() -> io::Result<()> {
        let sd = ScratchDir::new("ipe-scratch-child")?;
        let child = sd.child(&leaf("Main.ipe")?);
        assert_eq!(child.parent(), Some(sd.path()));
        for bad in ["", "..", ".", "a/b", "..\\x", "c:x", "a\0b"] {
            assert!(ScratchLeaf::new(bad).is_err(), "{bad:?} must not be a leaf");
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

    #[test]
    fn writes_are_read_back_through_the_retained_handle() -> io::Result<()> {
        let mut sf = ScratchFile::create("ipe-scratch-read")?;
        std::io::Write::write_all(&mut sf.file, b"payload")?;
        assert_eq!(sf.read_all()?, b"payload");
        Ok(())
    }

    #[cfg(unix)]
    mod unix {
        use super::super::*;
        use super::leaf;
        use std::os::unix::fs::MetadataExt as _;

        /// A held anchor survives exec and is honoured for exactly its own node.
        #[test]
        fn held_anchor_is_inheritable_and_honoured() -> io::Result<()> {
            let sd = ScratchDir::new("ipe-scratch-anchor")?;
            let held = HeldScratchAnchor::hold(sd.path())?;
            let flags = rustix::io::fcntl_getfd(held.file())?;
            assert!(!flags.contains(rustix::io::FdFlags::CLOEXEC));
            let meta = std::fs::symlink_metadata(sd.path())?;
            assert_eq!(
                held.anchor().node(),
                NodeId {
                    dev: meta.dev(),
                    ino: meta.ino(),
                }
            );
            let raw = held.anchor().encode();
            let who = Identity {
                euid: rustix::process::geteuid().as_raw(),
                egid: rustix::process::getegid().as_raw(),
            };
            assert_eq!(
                inherited_anchor(Some(raw.as_os_str()), who),
                Some(held.anchor().node())
            );
            Ok(())
        }

        /// A directory that is not private to the effective user is never held as an anchor.
        #[test]
        fn shared_directory_is_refused_as_an_anchor() -> io::Result<()> {
            use std::os::unix::fs::PermissionsExt as _;
            let root = ScratchDir::new("ipe-scratch-anchor-shared")?;
            let shared = root.child(&leaf("shared")?);
            std::fs::create_dir(&shared)?;
            std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755))?;
            let err = HeldScratchAnchor::hold(&shared).err();
            assert_eq!(
                err.as_ref().map(io::Error::kind),
                Some(io::ErrorKind::PermissionDenied)
            );
            Ok(())
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
        fn preplanted_symlink_at_the_file_path_is_refused_and_target_untouched() -> io::Result<()> {
            let root = ScratchDir::new("ipe-scratch-symfile")?;
            let canary = root.child(&leaf("canary")?);
            std::fs::write(&canary, b"canary")?;
            let dir = ScratchDir::new_under(root.path(), "ipe-private")?;
            let tag = leaf("tag")?;
            std::os::unix::fs::symlink(&canary, dir.child(&tag))?;

            let err = ScratchFile::create_in(dir, &tag).err();
            assert!(err.is_some(), "a planted symlink must not be opened");
            assert_eq!(std::fs::read(&canary)?, b"canary");
            Ok(())
        }

        #[test]
        fn dangling_symlink_at_the_file_path_creates_nothing() -> io::Result<()> {
            let root = ScratchDir::new("ipe-scratch-dangle")?;
            let victim = root.child(&leaf("victim")?);
            let dir = ScratchDir::new_under(root.path(), "ipe-private")?;
            let tag = leaf("tag")?;
            std::os::unix::fs::symlink(&victim, dir.child(&tag))?;

            assert!(ScratchFile::create_in(dir, &tag).is_err());
            assert!(std::fs::symlink_metadata(&victim).is_err());
            Ok(())
        }
    }
}

//! Files a secret is kept in: proven owner-only on the open handle, or refused.
//!
//! Every credential ipe keeps on disk (the publish token, the signing key's
//! private half, the per-user cache salt) is created, staged, housed and read
//! back through this module alone. A creation mode is only a request, which a
//! filesystem that ignores permission bits (vfat, exfat, some network and FUSE
//! mounts) silently drops; so every handle this module hands out has been
//! checked by `fstat` to be a regular file owned by the effective user with no
//! group or other permission bit. A handle failing that check is never
//! returned: a file this module created is removed, and a file it was asked to
//! read is refused.

use std::fs::File;
use std::path::{Path, PathBuf};

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
#[cfg(not(unix))]
pub const HOST_SECRET_STORE: SecretStore = SecretStore::Unsupported;

/// Why a secret file was not created, housed or read.
#[derive(Debug)]
pub enum SecretFileError {
    /// The store cannot keep a file readable by its owner alone.
    Unsupported,
    /// Creating, opening or inspecting the file failed (its name is taken, say).
    Io(std::io::Error),
    /// The path is not private to the invoking user.
    ///
    /// Another user owns it, a group or other permission bit is set on it, or
    /// its filesystem dropped the owner-only mode it was created with.
    NotOwnerOnly(PathBuf),
    /// The path names a directory, FIFO, device or socket, not a regular file.
    NotRegularFile(PathBuf),
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

/// Create the new file `path` owner-only, proven so before any byte is written.
///
/// The name is created exclusively: an existing file or symlink there, even a
/// dangling one, is refused, never truncated or followed. A created file
/// whose handle fails the owner-only check is removed again.
///
/// # Errors
/// [`SecretFileError::Unsupported`] when `store` cannot keep the file
/// owner-only, [`SecretFileError::Io`] when creating it failed, or
/// [`SecretFileError::NotOwnerOnly`] when the created file is not private.
pub fn create_new(store: SecretStore, path: &Path) -> Result<File, SecretFileError> {
    require(store)?;
    host::create_owner_only(path)
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
    pub fn fresh() -> std::io::Result<Self> {
        let mut bytes = [0u8; TEMP_SUFFIX_BYTES];
        getrandom::fill(&mut bytes).map_err(std::io::Error::from)?;
        Ok(Self(format!(
            "{}.{}",
            std::process::id(),
            hex::encode(bytes)
        )))
    }

    /// The hidden temporary name `.<name>.<suffix>.tmp` beside `final_path`.
    #[must_use]
    pub fn beside(&self, final_path: &Path) -> PathBuf {
        let name = final_path
            .file_name()
            .map_or_else(|| "secret".into(), std::ffi::OsStr::to_string_lossy);
        final_path.with_file_name(format!(".{name}.{}.tmp", self.0))
    }
}

/// Create the temporary file staging a secret for `final_path`, owner-only.
///
/// The file is created by [`create_new`] at [`TempSuffix::beside`], in the
/// same directory as `final_path` so a rename or link into place is atomic.
///
/// # Errors
/// As [`create_new`].
pub fn create_temp_beside(
    store: SecretStore,
    final_path: &Path,
    suffix: &TempSuffix,
) -> Result<(File, PathBuf), SecretFileError> {
    let temp_path = suffix.beside(final_path);
    create_new(store, &temp_path).map(|file| (file, temp_path))
}

/// Create the directory `dir` that houses secret files, and prove no other user can write it.
///
/// Missing components are created mode `0700`. The final directory, new or
/// already present, is refused when another user owns it or can write to it,
/// since such a user could replace a secret file inside it. Write access for
/// the invoker's own effective group is admitted on the terms of
/// [`crate::owner_trust::breach`]. Only the final directory is checked: an
/// ancestor another user can write to is not refused here.
///
/// # Errors
/// [`SecretFileError::Unsupported`] when `store` cannot keep secrets,
/// [`SecretFileError::Io`] when creating or inspecting it failed, or
/// [`SecretFileError::NotOwnerOnly`] when another user could write to it.
pub fn create_owner_dir(store: SecretStore, dir: &Path) -> Result<(), SecretFileError> {
    require(store)?;
    host::create_owner_dir(dir)
}

/// Open the existing secret file `path` for reading, proven owner-only on the open handle.
///
/// A final symlink is refused, never followed, and opening never blocks on a
/// FIFO. A file another user owns, or any group or other user may access, is
/// refused.
///
/// # Errors
/// [`SecretFileError::Unsupported`] when `store` cannot keep secrets,
/// [`SecretFileError::Io`] when the file cannot be opened (absent, a symlink,
/// unreadable), [`SecretFileError::NotRegularFile`] when it is not a regular
/// file, or [`SecretFileError::NotOwnerOnly`] when it is not private.
pub fn open_existing(store: SecretStore, path: &Path) -> Result<File, SecretFileError> {
    require(store)?;
    host::open_owner_only(path)
}

/// The owner-only file operations of a host with Unix permissions.
#[cfg(unix)]
mod host {
    use std::fs::File;
    use std::os::unix::fs::MetadataExt as _;
    use std::path::Path;

    use super::{SecretFileError, is_owner_only};
    use crate::owner_trust::{Invoker, Stamp, breach};

    /// Refuse the open `file` at `path` unless it is a regular file private to `euid`.
    fn prove_owner_only(file: &File, path: &Path, euid: u32) -> Result<(), SecretFileError> {
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

    /// Create `path` exclusively with mode `0600`, then prove the handle owner-only.
    pub fn create_owner_only(path: &Path) -> Result<File, SecretFileError> {
        create_owner_only_as(path, Invoker::current().uid)
    }

    /// Create `path` exclusively with mode `0600`, then prove the handle private to `euid`.
    ///
    /// A created file failing the proof is removed before the refusal returns.
    pub fn create_owner_only_as(path: &Path, euid: u32) -> Result<File, SecretFileError> {
        use std::os::unix::fs::OpenOptionsExt as _;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(SecretFileError::Io)?;
        if let Err(refusal) = prove_owner_only(&file, path, euid) {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(refusal);
        }
        Ok(file)
    }

    /// Create `dir` and its missing parents mode `0700`, then refuse it if another user can write it.
    pub fn create_owner_dir(dir: &Path) -> Result<(), SecretFileError> {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(SecretFileError::Io)?;
        let meta = std::fs::metadata(dir).map_err(SecretFileError::Io)?;
        if meta.is_dir() && breach(Stamp::of(&meta), Invoker::current()).is_none() {
            Ok(())
        } else {
            Err(SecretFileError::NotOwnerOnly(dir.to_path_buf()))
        }
    }

    /// Open `path` read-only without following a final link, then prove the handle owner-only.
    pub fn open_owner_only(path: &Path) -> Result<File, SecretFileError> {
        use rustix::fs::{Mode, OFlags};
        let flags =
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;
        let file = rustix::fs::open(path, flags, Mode::empty())
            .map(File::from)
            .map_err(|errno| SecretFileError::Io(errno.into()))?;
        prove_owner_only(&file, path, Invoker::current().uid)?;
        Ok(file)
    }
}

/// A host with no owner-only check, where every secret-file operation is refused.
#[cfg(not(unix))]
mod host {
    use std::fs::File;
    use std::path::Path;

    use super::SecretFileError;

    /// Refuse: no owner-only file can be created here.
    pub const fn create_owner_only(_path: &Path) -> Result<File, SecretFileError> {
        Err(SecretFileError::Unsupported)
    }

    /// Refuse: no directory can be proven private here.
    pub const fn create_owner_dir(_dir: &Path) -> Result<(), SecretFileError> {
        Err(SecretFileError::Unsupported)
    }

    /// Refuse: no file can be proven owner-only here.
    pub const fn open_owner_only(_path: &Path) -> Result<File, SecretFileError> {
        Err(SecretFileError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty scratch directory for one test.
    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ipe-secret-file-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    /// The names in `dir`, sorted.
    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("readdir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn only_a_mode_without_group_or_other_bits_owned_by_the_invoker_is_owner_only() {
        assert!(is_owner_only(0o100_600, 1000, 1000));
        assert!(is_owner_only(0o100_400, 1000, 1000));
        assert!(is_owner_only(0o100_700, 1000, 1000));
        assert!(!is_owner_only(0o100_644, 1000, 1000), "world-readable");
        assert!(!is_owner_only(0o100_640, 1000, 1000), "group-readable");
        assert!(!is_owner_only(0o100_604, 1000, 1000), "other-readable");
        assert!(!is_owner_only(0o100_620, 1000, 1000), "group-writable");
        assert!(!is_owner_only(0o100_777, 1000, 1000), "mode-ignoring mount");
        assert!(!is_owner_only(0o100_600, 1001, 1000), "foreign owner");
        assert!(!is_owner_only(0o100_600, 0, 1000), "root-owned for a user");
    }

    #[test]
    fn an_unsupported_store_is_refused_up_front() {
        assert!(matches!(
            require(SecretStore::Unsupported),
            Err(SecretFileError::Unsupported)
        ));
        assert!(matches!(require(SecretStore::OwnerOnlyFile), Ok(())));
    }

    #[test]
    fn an_unsupported_store_creates_nothing() {
        let dir = test_dir("unsupported");
        let path = dir.join("secret");
        let created = create_new(SecretStore::Unsupported, &path);
        assert!(
            matches!(created, Err(SecretFileError::Unsupported)),
            "an unsupported store must refuse, got {created:?}"
        );
        let suffix = TempSuffix::fresh().expect("csprng");
        let staged = create_temp_beside(SecretStore::Unsupported, &path, &suffix);
        assert!(
            matches!(staged, Err(SecretFileError::Unsupported)),
            "an unsupported store must refuse a temp file, got {staged:?}"
        );
        let housed = create_owner_dir(SecretStore::Unsupported, &dir.join("sub"));
        assert!(
            matches!(housed, Err(SecretFileError::Unsupported)),
            "an unsupported store must refuse a secret dir, got {housed:?}"
        );
        assert!(entries(&dir).is_empty(), "a refusal must create nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unsupported_store_reads_no_existing_file() {
        let dir = test_dir("unsupported-read");
        let path = dir.join("secret");
        std::fs::write(&path, "kept").expect("seed file");
        let opened = open_existing(SecretStore::Unsupported, &path);
        assert!(
            matches!(opened, Err(SecretFileError::Unsupported)),
            "an unsupported store must refuse the read, got {opened:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_host_store_creates_on_unix_and_refuses_elsewhere() {
        let dir = test_dir("host-store");
        let path = dir.join("secret");
        let created = create_new(HOST_SECRET_STORE, &path);
        if cfg!(unix) {
            assert!(
                created.is_ok(),
                "a Unix host creates the secret: {created:?}"
            );
        } else {
            assert!(
                matches!(created, Err(SecretFileError::Unsupported)),
                "a host without owner-only files must refuse, got {created:?}"
            );
            assert!(entries(&dir).is_empty(), "the refusal creates nothing");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_temp_name_is_hidden_beside_its_final_name_and_unpredictable() {
        let dir = test_dir("temp-name");
        let path = dir.join("token");
        let first = TempSuffix::fresh().expect("csprng");
        let second = TempSuffix::fresh().expect("csprng");
        assert_ne!(first, second, "two suffixes must differ");
        let temp = first.beside(&path);
        assert_eq!(temp.parent(), Some(dir.as_path()));
        let name = temp
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        assert!(
            name.starts_with(".token.") && temp.extension() == Some(std::ffi::OsStr::new("tmp")),
            "unexpected temp name {name}"
        );
        assert!(
            name.len() > ".token..tmp".len() + TEMP_SUFFIX_BYTES * 2,
            "the suffix must carry the random bytes: {name}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_secret_file_is_owner_only_from_creation() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = test_dir("owner-only");
        let path = dir.join("secret");
        let created = create_new(SecretStore::OwnerOnlyFile, &path);
        assert!(created.is_ok(), "owner-only create failed: {created:?}");
        let mode = std::fs::metadata(&path)
            .expect("secret file metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "secret file must be 0600, got {mode:04o}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_staged_temp_file_is_owner_only_beside_its_final_name() {
        let dir = test_dir("temp-owner-only");
        let path = dir.join("token");
        let suffix = TempSuffix::fresh().expect("csprng");
        let staged = create_temp_beside(SecretStore::OwnerOnlyFile, &path, &suffix);
        assert!(staged.is_ok(), "temp create failed: {staged:?}");
        let Ok((_file, temp)) = staged else { return };
        assert_eq!(temp, suffix.beside(&path));
        let reopened = open_existing(SecretStore::OwnerOnlyFile, &temp);
        assert!(
            reopened.is_ok(),
            "the staged file is owner-only: {reopened:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_name_is_refused_not_truncated() {
        let dir = test_dir("existing");
        let path = dir.join("secret");
        std::fs::write(&path, "kept").expect("seed existing file");
        let created = create_new(HOST_SECRET_STORE, &path);
        assert!(
            matches!(
                created,
                Err(SecretFileError::Io(_) | SecretFileError::Unsupported)
            ),
            "an existing name must be refused, got {created:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read existing file"),
            "kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_symlink_at_the_secret_name_is_refused_and_never_followed() {
        let dir = test_dir("symlink");
        let outside = test_dir("symlink-outside");
        let existing_target = outside.join("existing");
        std::fs::write(&existing_target, "victim").expect("seed target");
        let dangling_target = outside.join("absent");

        let to_existing = dir.join("secret");
        std::os::unix::fs::symlink(&existing_target, &to_existing).expect("plant symlink");
        let created = create_new(SecretStore::OwnerOnlyFile, &to_existing);
        assert!(
            matches!(&created, Err(SecretFileError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists),
            "a symlink at the secret name must be refused, got {created:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&existing_target).expect("read target"),
            "victim",
            "the link's target must be untouched"
        );

        let to_dangling = dir.join("dangling");
        std::os::unix::fs::symlink(&dangling_target, &to_dangling).expect("plant dangling");
        let created = create_new(SecretStore::OwnerOnlyFile, &to_dangling);
        assert!(
            matches!(&created, Err(SecretFileError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists),
            "a dangling symlink at the secret name must be refused, got {created:?}"
        );
        assert!(
            std::fs::symlink_metadata(&dangling_target).is_err(),
            "the dangling link's target must not be created"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn an_owner_only_secret_is_read_back() {
        use std::io::Read as _;
        let dir = test_dir("read-back");
        let path = dir.join("secret");
        let created = create_new(SecretStore::OwnerOnlyFile, &path);
        assert!(created.is_ok(), "create failed: {created:?}");
        drop(created);
        std::fs::write(&path, "kept").expect("write secret");
        let opened = open_existing(SecretStore::OwnerOnlyFile, &path);
        assert!(opened.is_ok(), "an owner-only secret must open: {opened:?}");
        let Ok(mut file) = opened else { return };
        let mut text = String::new();
        file.read_to_string(&mut text).expect("read secret");
        assert_eq!(text, "kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_group_or_world_accessible_secret_is_refused_on_read() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = test_dir("exposed-read");
        for mode in [0o644, 0o640, 0o604, 0o660] {
            let path = dir.join(format!("secret-{mode:o}"));
            std::fs::write(&path, "exposed").expect("seed secret");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                .expect("chmod secret");
            let opened = open_existing(SecretStore::OwnerOnlyFile, &path);
            assert!(
                matches!(&opened, Err(SecretFileError::NotOwnerOnly(p)) if *p == path),
                "a mode-{mode:o} secret must be refused, got {opened:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_or_non_file_secret_is_refused_on_read() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = test_dir("link-read");
        let target = dir.join("target");
        std::fs::write(&target, "kept").expect("seed target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
            .expect("chmod target");
        let link = dir.join("secret");
        std::os::unix::fs::symlink(&target, &link).expect("plant symlink");
        let opened = open_existing(SecretStore::OwnerOnlyFile, &link);
        assert!(
            matches!(opened, Err(SecretFileError::Io(_))),
            "a final symlink must not be followed, got {opened:?}"
        );
        let sub = dir.join("sub");
        std::fs::create_dir(&sub).expect("mkdir");
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        let opened = open_existing(SecretStore::OwnerOnlyFile, &sub);
        assert!(
            matches!(&opened, Err(SecretFileError::NotRegularFile(p)) if *p == sub),
            "a directory must be refused, got {opened:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_created_file_failing_the_owner_proof_is_refused_and_removed() {
        let dir = test_dir("create-refused");
        let path = dir.join("secret");
        let other_user = crate::owner_trust::Invoker::current().uid.wrapping_add(1);
        let created = host::create_owner_only_as(&path, other_user);
        assert!(
            matches!(&created, Err(SecretFileError::NotOwnerOnly(p)) if *p == path),
            "a created file owned by someone other than the euid must be refused, got {created:?}"
        );
        assert!(
            std::fs::symlink_metadata(&path).is_err(),
            "the refused file must be removed"
        );
        assert!(entries(&dir).is_empty(), "the refusal must leave nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_secret_is_refused_on_read_without_blocking() {
        let dir = test_dir("fifo-read");
        let fifo = dir.join("secret");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(made.success(), "mkfifo failed: {made:?}");
        let opened = open_existing(SecretStore::OwnerOnlyFile, &fifo);
        assert!(
            matches!(&opened, Err(SecretFileError::NotRegularFile(p)) if *p == fifo),
            "a FIFO must be refused as not a regular file, got {opened:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_secret_dir_is_owner_only_and_a_foreign_writable_one_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = test_dir("owner-dir");
        let fresh = dir.join("a").join("b");
        let housed = create_owner_dir(SecretStore::OwnerOnlyFile, &fresh);
        assert!(
            housed.is_ok(),
            "a fresh secret dir must be created: {housed:?}"
        );
        let mode = std::fs::metadata(&fresh)
            .expect("dir metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode & 0o077,
            0,
            "a new secret dir must be 0700, got {mode:04o}"
        );

        let exposed = dir.join("exposed");
        std::fs::create_dir(&exposed).expect("mkdir");
        std::fs::set_permissions(&exposed, std::fs::Permissions::from_mode(0o777))
            .expect("chmod world-writable");
        let housed = create_owner_dir(SecretStore::OwnerOnlyFile, &exposed);
        assert!(
            matches!(&housed, Err(SecretFileError::NotOwnerOnly(p)) if *p == exposed),
            "a world-writable secret dir must be refused, got {housed:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

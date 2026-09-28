//! Files a secret is written to: created owner-only, or refused.
//!
//! Every credential ipe keeps on disk (the publish token, the signing key's
//! private half, the per-user cache salt) is created here. A file is created
//! only where the host can make it readable by its owner alone before any byte
//! lands in it; everywhere else the creation is refused, so a secret never
//! inherits a directory's looser default access.

use std::fs::File;
use std::path::Path;

/// Where this host can keep a secret file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretStore {
    /// A file created mode `0600`, readable and writable by its owner only.
    OwnerOnlyFile,
    /// No owner-only file mode exists, so no secret is written to a file.
    Unsupported,
}

/// The secret store this build's target provides.
pub const HOST_SECRET_STORE: SecretStore = if cfg!(unix) {
    SecretStore::OwnerOnlyFile
} else {
    SecretStore::Unsupported
};

/// Why a secret file was not created.
#[derive(Debug)]
pub enum SecretFileError {
    /// The store cannot keep a file readable by its owner alone.
    Unsupported,
    /// Creating the file failed (its name is taken, say).
    Io(std::io::Error),
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

/// Create the new file `path` owner-only, before any byte is written to it.
///
/// The name is created exclusively: an existing file or symlink there is
/// refused, never truncated or followed.
///
/// # Errors
/// [`SecretFileError::Unsupported`] when `store` cannot keep the file
/// owner-only, or [`SecretFileError::Io`] when creating it failed.
pub fn create_new(store: SecretStore, path: &Path) -> Result<File, SecretFileError> {
    require(store)?;
    create_owner_only(path)
}

/// Create `path` exclusively with mode `0600` set at creation.
#[cfg(unix)]
fn create_owner_only(path: &Path) -> Result<File, SecretFileError> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(SecretFileError::Io)
}

/// Refuse: this target has no owner-only file mode to create `path` with.
#[cfg(not(unix))]
fn create_owner_only(_path: &Path) -> Result<File, SecretFileError> {
    Err(SecretFileError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty scratch directory for one test.
    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ipe-secret-file-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    #[test]
    fn host_secret_store_tracks_target_family() {
        let expected = if cfg!(unix) {
            SecretStore::OwnerOnlyFile
        } else {
            SecretStore::Unsupported
        };
        assert_eq!(HOST_SECRET_STORE, expected);
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
    fn an_unsupported_store_creates_no_file() {
        let dir = test_dir("unsupported");
        let path = dir.join("secret");
        let created = create_new(SecretStore::Unsupported, &path);
        assert!(
            matches!(created, Err(SecretFileError::Unsupported)),
            "an unsupported store must refuse, got {created:?}"
        );
        assert!(
            std::fs::symlink_metadata(&path).is_err(),
            "a refused secret file must not exist"
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
}

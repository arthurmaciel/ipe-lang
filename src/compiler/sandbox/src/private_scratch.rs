//! Owner-private, exclusive creation of scratch directories and files.
//!
//! Scratch entries hold downloaded artifacts and build inputs, so no other local
//! user may read or modify them. On Unix the entry itself is created owner-only
//! (directory mode 0700, file mode 0600), which holds whatever its parent's
//! permissions are. Elsewhere the standard library offers no owner-only
//! creation: the entry inherits its parent's access control. There, creation is
//! refused unless the parent resolves inside the current user's profile
//! directory, which the OS keeps private to that user; a shared temp directory
//! (e.g. `C:\Temp`) is turned back with a refusal that names the fix.
//!
//! Both constructors create exclusively: a pre-existing entry, including a
//! dangling symlink, yields `AlreadyExists` rather than being followed.

use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The variable naming the current user's profile directory on Windows.
pub const PROFILE_VAR: &str = "USERPROFILE";

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

/// The remedy every refusal names.
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

/// Decide whether a canonical scratch root lies inside a canonical profile.
///
/// Both paths are expected canonical (absolute, links resolved). Anything that
/// is not provably inside is refused: a relative path, a `..` segment, or a
/// root outside the profile, compared component-wise so `/home/al` never
/// contains `/home/alice`.
///
/// # Errors
///
/// Returns the [`ScratchRootRefusal`] that the root fails.
pub fn root_within_profile(root: &Path, profile: &Path) -> Result<(), ScratchRootRefusal> {
    let has_parent_dir = |p: &Path| p.components().any(|c| matches!(c, Component::ParentDir));
    if !profile.is_absolute() || has_parent_dir(profile) {
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

/// Resolve `root` and `profile` and require the root to lie inside the profile.
///
/// `profile` is the raw value of [`PROFILE_VAR`]; an unset, empty, or relative
/// value proves nothing and is refused. Resolution follows every link, so a
/// link inside the profile that points outside it is refused too.
///
/// # Errors
///
/// Returns a [`ScratchRootRefusal`] when either path cannot be resolved or the
/// resolved root lies outside the resolved profile.
pub fn verify_root_within_profile(
    root: &Path,
    profile: Option<&OsStr>,
) -> Result<(), ScratchRootRefusal> {
    let profile = profile
        .map(Path::new)
        .filter(|p| p.is_absolute())
        .ok_or(ScratchRootRefusal::NoProfile)?;
    root_within_profile(&canonical(root)?, &canonical(profile)?)
}

/// The canonical form of `path`, or the refusal naming why it has none.
fn canonical(path: &Path) -> Result<PathBuf, ScratchRootRefusal> {
    std::fs::canonicalize(path).map_err(|e| ScratchRootRefusal::Unresolvable {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })
}

/// Create the directory `path` exclusively, private to the current user.
///
/// Only the final component is created, so any pre-existing entry yields
/// `AlreadyExists`.
///
/// # Errors
///
/// Returns `AlreadyExists` for a pre-existing entry, `PermissionDenied`
/// carrying a [`ScratchRootRefusal`] when the parent cannot be proven private,
/// or the OS error of the creation.
#[cfg(unix)]
pub fn create_dir_exclusive(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(false)
        .create(path)
}

/// Create the directory `path` exclusively, private to the current user.
///
/// Only the final component is created, so any pre-existing entry yields
/// `AlreadyExists`.
///
/// # Errors
///
/// Returns `AlreadyExists` for a pre-existing entry, `PermissionDenied`
/// carrying a [`ScratchRootRefusal`] when the parent cannot be proven private,
/// or the OS error of the creation.
#[cfg(not(unix))]
pub fn create_dir_exclusive(path: &Path) -> io::Result<()> {
    verify_parent_private(path)?;
    std::fs::create_dir(path)
}

/// Create and open the file `path` exclusively for read and write, private to
/// the current user.
///
/// # Errors
///
/// Returns `AlreadyExists` for a pre-existing entry, `PermissionDenied`
/// carrying a [`ScratchRootRefusal`] when the parent cannot be proven private,
/// or the OS error of the creation.
#[cfg(unix)]
pub fn create_file_exclusive(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Create and open the file `path` exclusively for read and write, private to
/// the current user.
///
/// # Errors
///
/// Returns `AlreadyExists` for a pre-existing entry, `PermissionDenied`
/// carrying a [`ScratchRootRefusal`] when the parent cannot be proven private,
/// or the OS error of the creation.
#[cfg(not(unix))]
pub fn create_file_exclusive(path: &Path) -> io::Result<File> {
    verify_parent_private(path)?;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
}

/// Refuse to create `path` unless its parent lies inside the user's profile.
#[cfg(not(unix))]
fn verify_parent_private(path: &Path) -> Result<(), ScratchRootRefusal> {
    let parent = path
        .parent()
        .ok_or_else(|| ScratchRootRefusal::Unresolvable {
            path: path.to_path_buf(),
            detail: "no parent directory".to_owned(),
        })?;
    verify_root_within_profile(parent, std::env::var_os(PROFILE_VAR).as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory tree unique to this test, removed on drop.
    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let root = std::env::temp_dir().join(format!(
                "ipe-private-scratch-{tag}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(root.join("profile").join("temp")).expect("profile tree");
            std::fs::create_dir_all(root.join("shared")).expect("shared tree");
            Self(root)
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
    fn unset_empty_or_relative_profile_variable_is_refused() {
        let tree = Tree::new("noprofile");
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
    }

    #[test]
    fn resolved_root_inside_profile_is_accepted() {
        let tree = Tree::new("inside");
        let profile = tree.profile();
        assert_eq!(
            verify_root_within_profile(&profile.join("temp"), Some(profile.as_os_str())),
            Ok(())
        );
    }

    #[test]
    fn shared_root_outside_profile_is_refused() {
        let tree = Tree::new("shared");
        let profile = tree.profile();
        let refused = verify_root_within_profile(&tree.0.join("shared"), Some(profile.as_os_str()));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
    }

    #[test]
    fn dot_dot_path_leaving_profile_is_refused() {
        let tree = Tree::new("dotdot");
        let profile = tree.profile();
        let escaping = profile.join("temp").join("..").join("..").join("shared");
        let refused = verify_root_within_profile(&escaping, Some(profile.as_os_str()));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn link_inside_profile_pointing_outside_is_refused() {
        let tree = Tree::new("link");
        let profile = tree.profile();
        let link = profile.join("temp-link");
        std::os::unix::fs::symlink(tree.0.join("shared"), &link).expect("symlink");
        let refused = verify_root_within_profile(&link, Some(profile.as_os_str()));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
    }

    #[test]
    fn missing_root_is_refused() {
        let tree = Tree::new("missing");
        let profile = tree.profile();
        let refused =
            verify_root_within_profile(&profile.join("absent"), Some(profile.as_os_str()));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::Unresolvable { .. })
        ));
    }

    #[test]
    fn every_refusal_names_the_fix_and_denies_permission() {
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

    #[cfg(unix)]
    #[test]
    fn unix_entries_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let tree = Tree::new("mode");
        let dir = tree.0.join("dir");
        create_dir_exclusive(&dir).expect("dir");
        let file = tree.0.join("file");
        drop(create_file_exclusive(&file).expect("file"));
        let mode = |p: &Path| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode(&dir) & 0o077, 0);
        assert_eq!(mode(&file) & 0o077, 0);
        assert_eq!(
            create_dir_exclusive(&dir).map_err(|e| e.kind()),
            Err(io::ErrorKind::AlreadyExists)
        );
        assert_eq!(
            create_file_exclusive(&file).map(drop).map_err(|e| e.kind()),
            Err(io::ErrorKind::AlreadyExists)
        );
    }
}

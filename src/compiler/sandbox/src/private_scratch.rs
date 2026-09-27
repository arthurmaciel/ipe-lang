//! Owner-private, exclusive, unpredictably-named scratch directories and files.
//!
//! Scratch entries hold downloaded artifacts and build inputs, so no other local
//! user may read, modify, or predict them. Every name carries 128 bits of OS
//! CSPRNG entropy; an unavailable CSPRNG fails the creation, never weakens the
//! name. On Unix the entry itself is created owner-only (directory mode 0700,
//! file mode 0600), which holds whatever its parent's permissions are.
//! Elsewhere the standard library offers no owner-only creation: the entry
//! inherits its parent's access control. There, creation is refused unless the
//! parent resolves inside the current user's profile directory, which the OS
//! keeps private to that user; a shared temp directory (e.g. `C:\Temp`) is
//! turned back with a refusal that names the fix. The entry is then created
//! under the resolved parent, so no link on the original path can be re-pointed
//! between the check and the creation.
//!
//! Creation is exclusive: a pre-existing entry, including a dangling symlink,
//! is never followed; a collision retries with a fresh name, a bounded number
//! of times.

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

/// Attempts at a fresh name before creation gives up.
const MAX_ATTEMPTS: usize = 8;

/// Bytes of OS CSPRNG entropy in every scratch name (128 bits).
const ENTROPY_BYTES: usize = 16;

/// Longest caller label kept in a scratch name.
const MAX_LABEL_CHARS: usize = 64;

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
///
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

/// Resolve `root` and `profile`, require the root to lie inside the profile,
/// and return the resolved root.
///
/// `profile` is the raw value of [`PROFILE_VAR`]; an unset, empty, or relative
/// value proves nothing and is refused. Resolution follows every link, so a
/// link inside the profile that points outside it is refused too. Entries must
/// be created under the returned path, not under `root`, so that a link on
/// `root` re-pointed after the check cannot redirect the creation.
///
/// # Errors
///
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

/// Create a fresh, owner-private directory directly under `base` and return its
/// path.
///
/// The name is `<label>-<pid>-<32 hex CSPRNG chars>`; `label` is a diagnostic
/// tag only, reduced to ASCII alphanumerics, `-` and `_` (others become `_`)
/// and at most 64 characters, so it can never add a path component.
///
/// # Errors
///
/// Returns `PermissionDenied` carrying a [`ScratchRootRefusal`] when `base`
/// cannot be proven private, the `getrandom` error when the OS CSPRNG is
/// unavailable, `AlreadyExists` carrying [`NamesExhausted`] when every fresh
/// name collided, or the OS error of the creation.
pub fn create_unique_dir(base: &Path, label: &str) -> io::Result<PathBuf> {
    let (path, ()) = create_unique(
        &private_base(base)?,
        label,
        getrandom::fill,
        create_dir_exclusive,
    )?;
    Ok(path)
}

/// Create and open a fresh, owner-private file directly under `base` for read
/// and write, returning its path and handle.
///
/// Naming follows [`create_unique_dir`].
///
/// # Errors
///
/// As [`create_unique_dir`].
pub fn create_unique_file(base: &Path, label: &str) -> io::Result<(PathBuf, File)> {
    create_unique(
        &private_base(base)?,
        label,
        getrandom::fill,
        create_file_exclusive,
    )
}

/// The directory entries under `base` are created in: `base` itself on Unix,
/// where each entry's own mode keeps it private.
#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)] // one signature with the non-unix arm, which can refuse
fn private_base(base: &Path) -> Result<PathBuf, ScratchRootRefusal> {
    Ok(base.to_path_buf())
}

/// The directory entries under `base` are created in: the resolved `base`, once
/// proven inside the user's profile.
#[cfg(not(unix))]
fn private_base(base: &Path) -> Result<PathBuf, ScratchRootRefusal> {
    verify_root_within_profile(base, std::env::var_os(PROFILE_VAR).as_deref())
}

/// Create an entry under the proven-private `base` with `create`, retrying a
/// collision with a fresh name at most [`MAX_ATTEMPTS`] times.
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

/// `<label>-<pid>-<32 hex chars>`, the hex drawn from `fill`.
fn scratch_name(
    label: &str,
    fill: &mut impl FnMut(&mut [u8]) -> Result<(), getrandom::Error>,
) -> io::Result<String> {
    use std::fmt::Write as _;
    let mut entropy = [0u8; ENTROPY_BYTES];
    fill(&mut entropy)?;
    let mut name: String = label
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
    let _ = write!(name, "-{}-", std::process::id());
    for byte in entropy {
        let _ = write!(name, "{byte:02x}");
    }
    Ok(name)
}

/// Create the directory `path` exclusively with mode 0700; only the final
/// component is created.
#[cfg(unix)]
fn create_dir_exclusive(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(false)
        .create(path)
}

/// Create the directory `path` exclusively; only the final component is
/// created, inheriting the proven-private parent's access control.
#[cfg(not(unix))]
fn create_dir_exclusive(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

/// Create and open the file `path` exclusively for read and write, mode 0600.
#[cfg(unix)]
fn create_file_exclusive(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Create and open the file `path` exclusively for read and write, inheriting
/// the proven-private parent's access control.
#[cfg(not(unix))]
fn create_file_exclusive(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
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
        let resolved = std::fs::canonicalize(profile.join("temp")).expect("canonical");
        assert_eq!(
            verify_root_within_profile(&profile.join("temp"), Some(profile.as_os_str())),
            Ok(resolved)
        );
    }

    #[test]
    fn filesystem_root_profile_proves_nothing() {
        let root = Path::new("/");
        assert_eq!(
            root_within_profile(Path::new("/tmp"), root),
            Err(ScratchRootRefusal::NoProfile)
        );
    }

    /// The accepted root is returned resolved, so creation lands under the
    /// checked location even when the given path runs through a link.
    #[cfg(unix)]
    #[test]
    fn accepted_root_is_returned_resolved() {
        let tree = Tree::new("resolved");
        let profile = tree.profile();
        let link = profile.join("temp-link");
        std::os::unix::fs::symlink(profile.join("temp"), &link).expect("symlink");
        let resolved = verify_root_within_profile(&link, Some(profile.as_os_str()));
        assert_eq!(
            resolved,
            Ok(std::fs::canonicalize(profile.join("temp")).expect("canonical"))
        );
    }

    /// An unavailable CSPRNG fails the creation before any entry is attempted;
    /// no weaker name is ever produced.
    #[test]
    fn entropy_failure_fails_closed() {
        let tree = Tree::new("entropy");
        let mut attempts = 0usize;
        let result = create_unique(
            &tree.0,
            "ipe-test",
            |_: &mut [u8]| Err(getrandom::Error::UNSUPPORTED),
            |p: &Path| {
                attempts += 1;
                create_dir_exclusive(p)
            },
        );
        assert!(result.is_err());
        assert_eq!(attempts, 0);
    }

    /// Fixed entropy collides on every attempt after the first entry, and the
    /// retry loop stops at its bound with `AlreadyExists`.
    #[test]
    fn collisions_stop_at_the_attempt_bound() {
        let tree = Tree::new("collide");
        let zeros = |buf: &mut [u8]| {
            buf.fill(0);
            Ok(())
        };
        let first = create_unique(&tree.0, "ipe-test", zeros, create_dir_exclusive);
        assert!(first.is_ok());
        let mut attempts = 0usize;
        let second = create_unique(&tree.0, "ipe-test", zeros, |p: &Path| {
            attempts += 1;
            create_dir_exclusive(p)
        });
        let err = second.map(drop).expect_err("every name collides");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(err.to_string().contains("unique scratch entry"), "{err}");
        assert_eq!(attempts, MAX_ATTEMPTS);
    }

    /// A hostile label cannot add a path component, reach an alternate data
    /// stream, or grow the name without bound.
    #[test]
    fn label_is_confined_to_one_bounded_component() {
        let mut fill = |buf: &mut [u8]| {
            buf.fill(0xab);
            Ok(())
        };
        let hostile = format!("../../etc/x:y\\z{}", "a".repeat(500));
        let name = scratch_name(&hostile, &mut fill).expect("name");
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{name}"
        );
        assert!(name.starts_with("______etc_x_y_z"), "{name}");
        assert!(name.ends_with(&"ab".repeat(ENTROPY_BYTES)), "{name}");
        assert!(name.len() <= MAX_LABEL_CHARS + 2 + 10 + 2 * ENTROPY_BYTES);
    }

    #[test]
    fn unique_entries_differ_and_live_under_base() {
        let tree = Tree::new("unique");
        let a = create_unique_dir(&tree.0, "ipe-test").expect("dir a");
        let b = create_unique_dir(&tree.0, "ipe-test").expect("dir b");
        let (c, _file) = create_unique_file(&tree.0, "ipe-test").expect("file");
        assert_ne!(a, b);
        for entry in [&a, &b, &c] {
            assert_eq!(entry.parent(), Some(tree.0.as_path()));
        }
    }

    /// A base outside the user's profile is refused before anything is created.
    #[cfg(windows)]
    #[test]
    fn windows_base_outside_profile_is_refused() {
        let system_root = std::env::var_os("SystemRoot").expect("SystemRoot");
        let refused = create_unique_dir(Path::new(&system_root), "ipe-test");
        assert_eq!(
            refused.map_err(|e| e.kind()),
            Err(io::ErrorKind::PermissionDenied)
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
        let dir = create_unique_dir(&tree.0, "ipe-test").expect("dir");
        let (file, handle) = create_unique_file(&tree.0, "ipe-test").expect("file");
        drop(handle);
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

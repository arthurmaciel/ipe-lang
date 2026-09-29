// The single source of truth for which variable names the invoking user's home
// directory and how its raw value parses into a `HomeDir`.
//
// Both the runtime's `system::home_dir` and the compiler sandbox's
// `ipe_sandbox::home::home_dir` resolve the home through `HomeDir::parse`, the
// ONE constructor for the type, so the two cannot disagree on the variable (per
// OS), on the UTF-8 decode, or on which values name a directory. The file is
// std-only: the runtime references it as a sibling module (`super::home_core`),
// so it vendors with `mod ipe_runtime` into every emitted app, and the sandbox
// `include!`s this exact file because the runtime cannot depend on the
// compiler.
//
// Regular (`//`) comments, not inner docs (`//!`): this file is `include!`d
// verbatim into a sandbox module, where an inner doc after the `include!` item
// is an illegal mid-file inner attribute.

/// The variable naming the invoking user's profile directory on Windows.
pub const WINDOWS_HOME_VAR: &str = "USERPROFILE";

/// The platform variable naming the invoking user's home directory.
#[cfg(windows)]
pub const HOME_VAR: &str = WINDOWS_HOME_VAR;

/// The platform variable naming the invoking user's home directory.
#[cfg(not(windows))]
pub const HOME_VAR: &str = "HOME";

/// A verified home directory: non-empty, absolute, and, on Windows, not a
/// verbatim or device-namespace prefix.
///
/// [`HomeDir::parse`] is the only constructor, so every holder already knows
/// its path names a real directory rather than re-deriving that judgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeDir(std::path::PathBuf);

impl HomeDir {
    /// Parse a raw home value: `Some` only for a valid-UTF-8, non-empty,
    /// absolute path that names a directory.
    ///
    /// Unset and non-UTF-8 values name no directory, and so does a relative
    /// one: a relative home would resolve against whatever the working
    /// directory happens to be. On Windows a verbatim (`\\?\...`) or
    /// device-namespace (`\\.\...`) prefix is refused too — `Path::is_absolute`
    /// accepts them, but they name a raw device or an unparsed literal path,
    /// not a directory. The value is otherwise kept verbatim (no
    /// normalisation), so every consumer sees the same path.
    #[must_use]
    pub fn parse(raw: Option<std::ffi::OsString>) -> Option<Self> {
        let path = std::path::PathBuf::from(raw?.into_string().ok()?);
        if !path.is_absolute() {
            return None;
        }
        #[cfg(windows)]
        if has_disallowed_windows_prefix(&path) {
            return None;
        }
        Some(Self(path))
    }

    /// Unwrap into the verified path.
    #[must_use]
    pub fn into_path(self) -> std::path::PathBuf {
        self.0
    }
}

/// Is `path`'s leading component a Windows verbatim or device-namespace prefix?
///
/// Such a prefix (`\\?\...`, `\\.\...`, `\\?\UNC\...`) names a raw device or an
/// unparsed literal path, not a filesystem directory a `HomeDir` can safely be
/// — even though `Path::is_absolute` accepts it. A plain drive (`C:\...`) or
/// UNC (`\\server\share\...`) prefix is unaffected.
#[cfg(windows)]
fn has_disallowed_windows_prefix(path: &std::path::Path) -> bool {
    use std::path::{Component, Prefix};
    matches!(
        path.components().next(),
        Some(Component::Prefix(p)) if matches!(
            p.kind(),
            Prefix::Verbatim(_) | Prefix::VerbatimUNC(_, _) | Prefix::VerbatimDisk(_) | Prefix::DeviceNS(_)
        )
    )
}

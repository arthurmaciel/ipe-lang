//! The invoking user's home directory, read from the environment in one place.
//!
//! Every compiler-side read of the platform home variable (`HOME`, or
//! `USERPROFILE` on Windows) goes through [`home_dir`]. The value is attacker-
//! reachable environment input that decides where caches, scratch roots, and
//! toolchain binds live, so it is parsed once here: an unset, empty, or
//! relative value names no directory. A relative home would silently resolve
//! against the working directory, redirecting writes to wherever the process
//! happens to run. `ipe_env` refuses every home name, so this module's raw
//! read is the only way a compiler-side crate reaches the value.

use std::ffi::OsString;
use std::path::PathBuf;

/// The platform variable naming the invoking user's home directory.
///
/// Private: the raw name is never handed out, so no caller can pair it with a
/// raw environment read and skip the absolute-path parse in [`home_dir_from`].
#[cfg(windows)]
const HOME_VAR: &str = "USERPROFILE";
/// The platform variable naming the invoking user's home directory.
#[cfg(not(windows))]
const HOME_VAR: &str = "HOME";

/// The invoking user's home directory, when the environment names an absolute one.
#[must_use]
#[allow(clippy::disallowed_methods)] // the sole home reader: parsed absolute-or-nothing below
pub fn home_dir() -> Option<PathBuf> {
    home_dir_from(std::env::var_os(HOME_VAR))
}

/// Parse a raw home value: `Some` only for a non-empty absolute path.
#[must_use]
pub fn home_dir_from(raw: Option<OsString>) -> Option<PathBuf> {
    raw.map(PathBuf::from).filter(|p| p.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absolute_home_is_accepted() {
        #[cfg(not(windows))]
        let abs = "/home/u";
        #[cfg(windows)]
        let abs = r"C:\Users\u";
        assert_eq!(home_dir_from(Some(abs.into())), Some(PathBuf::from(abs)));
    }

    #[test]
    fn an_unset_empty_or_relative_home_names_no_directory() {
        assert_eq!(home_dir_from(None), None);
        for raw in ["", ".", "home/u", "./home", "../home", "~"] {
            assert_eq!(home_dir_from(Some(raw.into())), None, "{raw:?}");
        }
    }
}

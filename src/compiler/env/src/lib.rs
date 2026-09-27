//! The one audited reader of the process environment for compiler-side code.
//!
//! The root `clippy.toml` denies `std::env::{var, var_os, vars, vars_os}`
//! workspace-wide, so every environment read in a crate governed by it routes
//! through [`var`] / [`var_os`] here (or carries a per-site
//! `#[allow(clippy::disallowed_methods)]` in a file the `home_read_scan` test
//! pins as audited). The two readers mirror their `std` namesakes except that a
//! home variable is never returned: the invoking user's home is attacker-
//! reachable input that decides where caches and scratch roots live, and its
//! one validated reader is `ipe_sandbox::home::home_dir` (absolute or nothing).
//! A home read through this crate — by literal, by a constant, or by a key
//! computed at runtime — is refused, so no spelling of the key reaches the raw
//! value. There is no whole-environment iterator: an iteration would hand the
//! home value out under its own name.

use std::env::VarError;
use std::ffi::{OsStr, OsString};

/// Variables that name (a part of) the invoking user's home directory.
///
/// Compared ASCII case-insensitively: Windows environment names are
/// case-insensitive, so `Home` reads the same value as `HOME` there.
const HOME_NAMES: [&str; 4] = ["HOME", "USERPROFILE", "HOMEDRIVE", "HOMEPATH"];

/// Whether `key` names a home variable this crate refuses to read.
#[must_use]
pub fn is_home_key(key: &OsStr) -> bool {
    key.to_str()
        .is_some_and(|k| HOME_NAMES.iter().any(|h| h.eq_ignore_ascii_case(k)))
}

/// The UTF-8 value of `key`, as `std::env::var` — `NotPresent` for a home key.
///
/// # Errors
///
/// `VarError::NotPresent` when `key` is unset or names a home variable, and
/// `VarError::NotUnicode` when its value is not valid UTF-8.
#[allow(clippy::disallowed_methods)] // the audited reader: home keys refused first
pub fn var<K: AsRef<OsStr>>(key: K) -> Result<String, VarError> {
    let key = key.as_ref();
    if is_home_key(key) {
        return Err(VarError::NotPresent);
    }
    std::env::var(key)
}

/// The raw value of `key`, as `std::env::var_os` — `None` for a home key.
#[must_use]
#[allow(clippy::disallowed_methods)] // the audited reader: home keys refused first
pub fn var_os<K: AsRef<OsStr>>(key: K) -> Option<OsString> {
    let key = key.as_ref();
    if is_home_key(key) {
        return None;
    }
    std::env::var_os(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spelling_of_a_home_key_is_refused() {
        for key in [
            "HOME",
            "home",
            "Home",
            "USERPROFILE",
            "UserProfile",
            "HOMEDRIVE",
            "HOMEPATH",
            "homepath",
        ] {
            assert!(is_home_key(OsStr::new(key)), "{key:?}");
            assert_eq!(var_os(key), None, "{key:?}");
            assert_eq!(var(key), Err(VarError::NotPresent), "{key:?}");
        }
    }

    #[test]
    fn a_home_key_held_in_a_runtime_value_is_refused() {
        let computed: String = ["HO", "ME"].concat();
        assert_eq!(var_os(&computed), None);
        assert_eq!(var(OsString::from(computed)), Err(VarError::NotPresent));
    }

    #[test]
    fn a_neighbouring_key_is_not_refused() {
        for key in [
            "HOMEBREW_PREFIX",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "XDG_CACHE_HOME",
            "HOME_",
            "",
        ] {
            assert!(!is_home_key(OsStr::new(key)), "{key:?}");
        }
    }

    #[test]
    fn a_non_home_key_reads_as_std_does() {
        #[allow(clippy::disallowed_methods)] // the oracle this reader must mirror
        let raw = std::env::var_os("CARGO_MANIFEST_DIR");
        assert_eq!(var_os("CARGO_MANIFEST_DIR"), raw);
        assert_eq!(
            var("CARGO_MANIFEST_DIR").ok(),
            raw.and_then(|v| v.into_string().ok())
        );
    }
}

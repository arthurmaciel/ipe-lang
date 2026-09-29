//! The invoking user's home directory, read from the environment in one place.
//!
//! Every compiler-side read of the platform home variable (`HOME`, or
//! `USERPROFILE` on Windows) goes through [`home_dir`]. The value is attacker-
//! reachable environment input that decides where caches, scratch roots, and
//! toolchain binds live, so it is parsed once here: an unset, empty,
//! relative, or non-UTF-8 value names no directory. A relative home would
//! silently resolve against the working directory, redirecting writes to
//! wherever the process happens to run. `ipe_env` refuses every home name, so
//! this module's raw read is the only way a compiler-side crate reaches the
//! value.

use std::ffi::OsString;
use std::path::PathBuf;

/// The runtime's home-variable name and value parser, spliced in verbatim.
///
/// The one source lives in the runtime tree because the runtime is vendored
/// into emitted apps and cannot depend on the compiler.
mod home_core {
    include!("../../../runtime/rust/src/home_core.rs");
}

pub use home_core::WINDOWS_HOME_VAR;

/// The invoking user's home directory, when the environment names an absolute one.
#[must_use]
#[allow(clippy::disallowed_methods)] // the sole home reader: parsed absolute-or-nothing below
pub fn home_dir() -> Option<PathBuf> {
    home_dir_from(std::env::var_os(home_core::HOME_VAR))
}

/// Parse a raw home value: `Some` only for a valid `HomeDir`.
///
/// Delegates entirely to the shared [`home_core::HomeDir::parse`] — the UTF-8
/// decode, the absolute check, and the Windows verbatim/device-prefix refusal
/// all live there, so this function makes no parsing decision of its own. The
/// runtime reader (`system::home_dir_from_var`) delegates to the same
/// constructor; both components thus accept exactly the same values, though
/// they run in separate processes, so this is agreement on one environment
/// fact, not a shared-process escape.
#[must_use]
pub fn home_dir_from(raw: Option<OsString>) -> Option<PathBuf> {
    home_core::HomeDir::parse(raw).map(home_core::HomeDir::into_path)
}

/// A tool-home variable (`CARGO_HOME`, `RUSTUP_HOME`) set to a relative path.
///
/// The tool honours a relative value against its working directory, so no
/// fixed directory can be derived from it: every consumer refuses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelativeToolHome {
    /// The offending variable.
    pub var: &'static str,
}

impl std::fmt::Display for RelativeToolHome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}` must be an absolute path", self.var)
    }
}

impl std::error::Error for RelativeToolHome {}

/// A tool home (`CARGO_HOME`, `RUSTUP_HOME`): `var` when set, else `<home>/<fallback>`.
///
/// # Errors
/// [`RelativeToolHome`] when `var` is set, non-empty, and relative.
pub fn tool_home(var: &'static str, fallback: &str) -> Result<Option<PathBuf>, RelativeToolHome> {
    tool_home_from(var, ipe_env::var_os(var), home_dir(), fallback)
}

/// Resolve a tool home from the raw variable value and the resolved home.
///
/// An empty value counts as unset.
///
/// # Errors
/// [`RelativeToolHome`] when `raw` is non-empty and relative.
pub fn tool_home_from(
    var: &'static str,
    raw: Option<OsString>,
    home: Option<PathBuf>,
    fallback: &str,
) -> Result<Option<PathBuf>, RelativeToolHome> {
    raw.filter(|raw| !raw.is_empty()).map_or_else(
        || Ok(home.filter(|h| h.is_absolute()).map(|h| h.join(fallback))),
        |raw| {
            let path = PathBuf::from(raw);
            if path.is_absolute() {
                Ok(Some(path))
            } else {
                Err(RelativeToolHome { var })
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shared with `ipe_runtime_rust::system`'s `home_dir_tests`: the same
    // `(raw, expected)` rows drive both crates' home readers.
    include!("../../../runtime/rust/tests/data/home_cases.rs");

    #[test]
    fn every_home_parse_case_matches_the_shared_table() {
        for (raw, expected) in HOME_PARSE_CASES.iter().chain(HOME_PARSE_PLATFORM_CASES) {
            assert_eq!(
                home_dir_from(raw.map(OsString::from)),
                expected.map(PathBuf::from),
                "{raw:?}"
            );
        }
    }

    /// A non-UTF-8 raw value is refused even when byte-for-byte absolute.
    #[cfg(unix)]
    #[test]
    fn a_non_utf8_home_value_is_refused() {
        use std::os::unix::ffi::OsStrExt as _;
        let raw = std::ffi::OsStr::from_bytes(b"/home/\xff").to_os_string();
        assert_eq!(home_dir_from(Some(raw)), None);
    }

    /// The shared home variable is `USERPROFILE` on Windows and `HOME` elsewhere.
    #[test]
    fn the_home_variable_is_the_platform_convention() {
        let expected = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        assert_eq!(home_core::HOME_VAR, expected);
        assert_eq!(WINDOWS_HOME_VAR, "USERPROFILE");
    }

    /// Every shared home name is one `ipe_env` refuses to read.
    #[test]
    fn every_shared_home_name_is_refused_by_ipe_env() {
        for name in [home_core::HOME_VAR, WINDOWS_HOME_VAR] {
            assert!(ipe_env::HOME_NAMES.contains(&name), "{name}");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn a_tool_home_is_the_absolute_variable_or_the_home_fallback() {
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(
            tool_home_from(
                "CARGO_HOME",
                Some("/opt/cargo".into()),
                home.clone(),
                ".cargo"
            ),
            Ok(Some(PathBuf::from("/opt/cargo")))
        );
        for raw in [None, Some(OsString::new())] {
            assert_eq!(
                tool_home_from("CARGO_HOME", raw, home.clone(), ".cargo"),
                Ok(Some(PathBuf::from("/home/u/.cargo")))
            );
        }
        assert_eq!(tool_home_from("CARGO_HOME", None, None, ".cargo"), Ok(None));
    }

    #[test]
    fn a_relative_tool_home_is_refused() {
        for raw in [".", "cargo", "./cargo", "../cargo", "~/.cargo"] {
            assert_eq!(
                tool_home_from("CARGO_HOME", Some(raw.into()), None, ".cargo"),
                Err(RelativeToolHome { var: "CARGO_HOME" }),
                "{raw:?}"
            );
        }
    }
}

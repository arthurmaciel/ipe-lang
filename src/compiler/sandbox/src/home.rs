//! The invoking user's home directory, read from the environment in one place.
//!
//! Every compiler-side read of the platform home variable (`HOME`, or
//! `USERPROFILE` on Windows) goes through [`home_dir`]. The value is attacker-
//! reachable environment input that decides where caches, scratch roots, and
//! toolchain binds live, so it is parsed once here: an unset, empty, or
//! relative value names no directory. A relative home would silently resolve
//! against the working directory, redirecting writes to wherever the process
//! happens to run.

use std::ffi::OsString;
use std::path::PathBuf;

/// The invoking user's home directory, when the environment names an absolute one.
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    /// The platform variable naming the invoking user's home directory.
    #[cfg(windows)]
    const HOME_VAR: &str = "USERPROFILE";
    /// The platform variable naming the invoking user's home directory.
    #[cfg(not(windows))]
    const HOME_VAR: &str = "HOME";
    home_dir_from(std::env::var_os(HOME_VAR))
}

/// Parse a raw home value: `Some` only for a non-empty absolute path.
#[must_use]
pub fn home_dir_from(raw: Option<OsString>) -> Option<PathBuf> {
    raw.map(PathBuf::from).filter(|p| p.is_absolute())
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
    tool_home_from(var, std::env::var_os(var), home_dir(), fallback)
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

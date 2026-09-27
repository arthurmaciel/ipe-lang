//! Directory discovery from the environment, absolute paths only.
//!
//! Every per-user location the CLI derives from the environment (`HOME`,
//! `XDG_*_HOME`, `CARGO_HOME`, `RUSTUP_HOME`, explicit `IPE_*` overrides) is
//! resolved here. A value that is unset, empty, or relative names no directory:
//! a relative path would silently resolve against whatever the current working
//! directory happens to be, so it is never used. Ambient variables (`HOME`,
//! `XDG_*`) fall through to the next candidate, as the XDG spec requires; an
//! explicit override that is set but not absolute is refused outright
//! ([`explicit_override`]), never silently replaced by the default.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::CliError;

/// The directory a raw environment value names, when it is absolute.
#[must_use]
pub fn absolute(raw: Option<OsString>) -> Option<PathBuf> {
    raw.map(PathBuf::from).filter(|p| p.is_absolute())
}

/// The directory the environment variable `var` names, when it is absolute.
#[must_use]
pub fn absolute_var(var: &str) -> Option<PathBuf> {
    absolute(std::env::var_os(var))
}

/// The platform variable naming the invoking user's home directory.
#[cfg(windows)]
const HOME_VAR: &str = "USERPROFILE";
/// The platform variable naming the invoking user's home directory.
#[cfg(not(windows))]
const HOME_VAR: &str = "HOME";

/// The invoking user's home directory, when it is absolute.
#[must_use]
pub fn home() -> Option<PathBuf> {
    absolute_var(HOME_VAR)
}

/// A tool home: the variable `var` when absolute, else `<home>/<fallback>`.
///
/// Mirrors how `CARGO_HOME`/`RUSTUP_HOME` default to `~/.cargo`/`~/.rustup`.
#[must_use]
pub fn tool_home(var: &str, fallback: &str) -> Option<PathBuf> {
    tool_home_from(std::env::var_os(var), home(), fallback)
}

/// Resolve a tool home from the raw variable value and the resolved home.
#[must_use]
pub fn tool_home_from(
    raw: Option<OsString>,
    home: Option<PathBuf>,
    fallback: &str,
) -> Option<PathBuf> {
    absolute(raw).or_else(|| home.map(|h| h.join(fallback)))
}

/// An explicit directory override: `None` when unset, the path when absolute.
///
/// # Errors
/// [`CliError::EnvDirNotAbsolute`] when the variable is set (empty included) but
/// is not an absolute path.
pub fn explicit_override(
    var: &'static str,
    raw: Option<OsString>,
) -> Result<Option<PathBuf>, CliError> {
    raw.map_or(Ok(None), |raw| {
        absolute(Some(raw))
            .map(Some)
            .ok_or(CliError::EnvDirNotAbsolute { var })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_accepts_only_an_absolute_path() {
        assert_eq!(
            absolute(Some("/abs/dir".into())),
            Some(PathBuf::from("/abs/dir"))
        );
        assert_eq!(absolute(Some("relative/dir".into())), None);
        assert_eq!(absolute(Some("./dir".into())), None);
        assert_eq!(absolute(Some(OsString::new())), None);
        assert_eq!(absolute(None), None);
    }

    #[test]
    fn tool_home_prefers_an_absolute_variable() {
        let got = tool_home_from(Some("/opt/cargo".into()), Some("/home/u".into()), ".cargo");
        assert_eq!(got, Some(PathBuf::from("/opt/cargo")));
    }

    #[test]
    fn tool_home_ignores_a_relative_variable() {
        for raw in ["", "cargo", "./cargo"] {
            let got = tool_home_from(Some(raw.into()), Some("/home/u".into()), ".cargo");
            assert_eq!(got, Some(PathBuf::from("/home/u/.cargo")), "{raw:?}");
            assert_eq!(
                tool_home_from(Some(raw.into()), None, ".cargo"),
                None,
                "{raw:?}"
            );
        }
    }

    #[test]
    fn explicit_override_unset_defers_to_the_default() {
        assert!(matches!(explicit_override("IPE_INDEX_DIR", None), Ok(None)));
    }

    #[test]
    fn explicit_override_absolute_is_used() {
        let got = explicit_override("IPE_INDEX_DIR", Some("/srv/index".into()));
        assert!(matches!(got, Ok(Some(p)) if p == PathBuf::from("/srv/index")));
    }

    #[test]
    fn explicit_override_relative_or_empty_is_refused() {
        for raw in ["index", "./index", "../index", ""] {
            let got = explicit_override("IPE_INDEX_DIR", Some(raw.into()));
            assert!(
                matches!(
                    got,
                    Err(CliError::EnvDirNotAbsolute {
                        var: "IPE_INDEX_DIR"
                    })
                ),
                "`{raw}` must be refused"
            );
        }
    }

    #[test]
    fn refusal_names_the_variable() {
        let got = explicit_override("IPE_HOME", Some("rel".into()));
        assert!(matches!(got, Err(CliError::EnvDirNotAbsolute { .. })));
        let Err(err) = got else { return };
        assert!(
            err.to_string()
                .starts_with("IPE_HOME is set but is not an absolute path")
        );
        assert_eq!(err.machine_kind(), "env-dir-not-absolute");
    }
}

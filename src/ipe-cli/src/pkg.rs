//! The `ipe add` / `ipe remove` package-authoring commands.
//!
//! These add and remove Ipê packages (not Rust crates — that is `ipe rust`).
//! Resolution — fetching the index, resolving a version, downloading, verifying
//! the checksum, and writing the lockfile — ships with the package index (SP3).
//! Until then each command parses its arguments and reports the not-yet-
//! available state, exiting non-zero so a script never mistakes it for success.

use crate::CliError;

/// `ipe add <package>[@<version>]` — add an Ipê package dependency.
///
/// Parses the package name, then reports that resolution ships with the index.
/// Never a silent no-op: it always exits non-zero until SP3 lands.
///
/// # Errors
/// [`CliError::Usage`] when no package is named; [`CliError::UsageOwned`]
/// reporting the not-yet-available state otherwise.
pub fn run_add(rest: &[String]) -> Result<(), CliError> {
    let package = package_arg(rest, "add")?;
    Err(CliError::UsageOwned(format!(
        "ipe add: adding the Ipê package `{package}` needs the package index, which \
         ships with the index (SP3). For a Rust crate dependency, use `ipe rust add {package}`."
    )))
}

/// `ipe remove <package>` — remove an Ipê package dependency.
///
/// # Errors
/// [`CliError::Usage`] when no package is named; [`CliError::UsageOwned`]
/// reporting the not-yet-available state otherwise.
pub fn run_remove(rest: &[String]) -> Result<(), CliError> {
    let package = package_arg(rest, "remove")?;
    Err(CliError::UsageOwned(format!(
        "ipe remove: removing the Ipê package `{package}` needs the package index, which \
         ships with the index (SP3). For a Rust crate dependency, use `ipe rust remove {package}`."
    )))
}

/// The single positional package argument shared by `add` and `remove`. Extra
/// positionals or none at all are misuse.
///
/// # Errors
/// [`CliError::UsageOwned`] naming the command's usage.
fn package_arg<'a>(rest: &'a [String], command: &str) -> Result<&'a str, CliError> {
    match rest {
        [one] => Ok(one.as_str()),
        _ => Err(CliError::UsageOwned(format!(
            "usage: ipe {command} <package>[@<version>]"
        ))),
    }
}

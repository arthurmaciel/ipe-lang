//! The `ipe add` / `ipe remove` package-authoring commands.
//!
//! These add and remove Ipê packages (not Rust crates — that is `ipe rust`).
//! `ipe add <name>[@<req>]` resolves the package through the index (fetch,
//! hash-verify, lock), records it in `ipe.toml` + `ipe.lock`, and prints its
//! resolved version and capability set. `ipe remove <name>` drops it from both
//! files. The resolution itself lives in [`crate::resolve`].

use std::path::PathBuf;

use crate::CliError;
use crate::resolve;

/// `ipe add <package>[@<req>]` — add an Ipê package dependency.
///
/// The optional `@<req>` is a semver requirement (`http@^1.2`); absent, `*`
/// (the latest published version) is used. Resolution reads the index (the
/// checkout at `IPE_INDEX_DIR`, or the standard location), fetches and verifies
/// the source, and writes `ipe.toml` + `ipe.lock`.
///
/// # Errors
/// [`CliError::UsageOwned`] when no package is named or the requirement is
/// malformed; the resolution errors ([`CliError::Resolve`],
/// [`CliError::HashMismatch`], [`CliError::Io`]) otherwise.
pub fn run_add(rest: &[String]) -> Result<(), CliError> {
    let (name, req) = parse_add_arg(rest)?;
    let project_root = project_root()?;
    resolve::resolve_and_add(&project_root, name, &req, &resolve::index_root())
}

/// `ipe remove <package>` — remove an Ipê package dependency from `ipe.toml`
/// and `ipe.lock`.
///
/// # Errors
/// [`CliError::UsageOwned`] when no package is named; [`CliError::Io`] on a
/// filesystem failure.
pub fn run_remove(rest: &[String]) -> Result<(), CliError> {
    let package = package_arg(rest, "remove")?;
    let project_root = project_root()?;
    resolve::resolve_and_remove(&project_root, package)
}

/// The project root the command operates on: the current directory, which must
/// hold an `ipe.toml`.
///
/// # Errors
/// [`CliError::UsageOwned`] when there is no `ipe.toml` here.
fn project_root() -> Result<PathBuf, CliError> {
    let cwd = std::env::current_dir().map_err(|e| CliError::Io {
        path: PathBuf::from("."),
        source: e,
    })?;
    if cwd.join("ipe.toml").is_file() {
        Ok(cwd)
    } else {
        Err(CliError::UsageOwned(
            "ipe add/remove: no `ipe.toml` in the current directory (run inside an Ipê project)"
                .to_owned(),
        ))
    }
}

/// Parse `ipe add`'s single argument into a package name and a version
/// requirement. The `name@req` split takes the requirement after the first `@`;
/// with no `@`, the requirement is `*` (the latest published version).
///
/// # Errors
/// [`CliError::UsageOwned`] on the wrong number of arguments or a malformed
/// requirement.
fn parse_add_arg(rest: &[String]) -> Result<(&str, semver::VersionReq), CliError> {
    let arg = package_arg(rest, "add")?;
    let (name, req_str) = arg.split_once('@').map_or((arg, "*"), |(n, r)| (n, r));
    if name.is_empty() {
        return Err(CliError::UsageOwned(
            "usage: ipe add <package>[@<version>]".to_owned(),
        ));
    }
    let req = req_str.parse::<semver::VersionReq>().map_err(|e| {
        CliError::UsageOwned(format!(
            "ipe add: `{req_str}` is not a valid version requirement: {e}"
        ))
    })?;
    Ok((name, req))
}

/// The single positional package argument shared by `add` and `remove`. Extra
/// positionals or none at all are misuse, and a leading-`-` token is an unknown
/// flag (rejected as such rather than accepted as a package name) so a flag typo
/// cannot masquerade as a dependency name and slip past with an exit-0 "nothing
/// to remove".
///
/// # Errors
/// [`CliError::UsageOwned`] naming the command's usage, or the shared
/// unknown-flag phrasing on a leading-`-` token.
fn package_arg<'a>(rest: &'a [String], command: &str) -> Result<&'a str, CliError> {
    match rest {
        [one] if one.starts_with('-') => Err(crate::cli_args::usage_unknown_flag(command, one)),
        [one] => Ok(one.as_str()),
        _ => Err(CliError::UsageOwned(format!(
            "usage: ipe {command} <package>[@<version>]"
        ))),
    }
}

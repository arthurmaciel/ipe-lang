//! The `ipe add` / `ipe remove` package-authoring commands.
//!
//! These add and remove Ipê packages (not Rust crates — that is `ipe rust`).
//! `ipe add <name>[@<req>]` resolves the package through the index (fetch,
//! hash-verify, lock) and records the requirement in the project manifest;
//! `ipe remove <name>` drops it. The resolution itself lives in
//! [`crate::resolve`].
//!
//! Rewriting a `package.ipe`'s `Package.dependencies` list in place — preserving
//! the author's formatting and comments — is a comment-preserving AST edit that
//! is not yet implemented, so these commands report the manual step to take
//! rather than corrupting the manifest source.

use std::path::PathBuf;

use crate::CliError;

/// The clear message both commands surface until the `package.ipe`
/// dependency-list AST rewrite is implemented: the resolution/lock machinery
/// exists, but editing the manifest source is done by hand.
const MANUAL_DEP_EDIT: &str = "ipe add/remove: editing a package.ipe `Package.dependencies` list is not yet automated — \
     add or remove the `Package.dep \"<name>\" \"<req>\"` entry in package.ipe by hand";

/// `ipe add <package>[@<req>]` — add an Ipê package dependency.
///
/// # Errors
/// [`CliError::UsageOwned`] when no package is named, the requirement is
/// malformed, there is no `package.ipe` here, or the manifest-source edit is not
/// yet automated.
pub fn run_add(rest: &[String]) -> Result<(), CliError> {
    let (_name, _req) = parse_add_arg(rest)?;
    require_project_manifest()?;
    Err(CliError::Usage(MANUAL_DEP_EDIT))
}

/// `ipe remove <package>` — remove an Ipê package dependency.
///
/// # Errors
/// [`CliError::UsageOwned`] when no package is named, there is no `package.ipe`
/// here, or the manifest-source edit is not yet automated.
pub fn run_remove(rest: &[String]) -> Result<(), CliError> {
    let _package = package_arg(rest, "remove")?;
    require_project_manifest()?;
    Err(CliError::Usage(MANUAL_DEP_EDIT))
}

/// Confirm the current directory is an Ipê project — it holds a `package.ipe`.
///
/// # Errors
/// [`CliError::UsageOwned`] when there is no `package.ipe` here (with the
/// migration hint when only a legacy `ipe.toml` is present).
fn require_project_manifest() -> Result<(), CliError> {
    let cwd = std::env::current_dir().map_err(|e| CliError::Io {
        path: PathBuf::from("."),
        source: e,
    })?;
    if crate::project::manifest_in_dir(&cwd).is_some() {
        return Ok(());
    }
    if crate::project::migration_pending(&cwd) {
        return Err(CliError::Usage(crate::project::MIGRATE_CONFIG_HINT));
    }
    Err(CliError::Usage(
        "ipe add/remove: no `package.ipe` in the current directory (run inside an Ipê project)",
    ))
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

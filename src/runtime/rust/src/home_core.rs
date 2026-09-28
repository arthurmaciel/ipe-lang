// The single source of truth for which variable names the invoking user's home
// directory and how its raw value parses.
//
// Both the runtime's `system::home_dir` and the compiler sandbox's
// `ipe_sandbox::home::home_dir` resolve the home through this one file, so the
// two cannot disagree on the variable (per OS) or on which values name a
// directory. The file is std-only: the runtime references it as a sibling
// module (`super::home_core`), so it vendors with `mod ipe_runtime` into every
// emitted app, and the sandbox `include!`s this exact file because the runtime
// cannot depend on the compiler.
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

/// Parse a raw home value: `Some` only for a non-empty absolute path.
///
/// Unset, empty, and relative values name no directory: a relative home would
/// resolve against whatever the working directory happens to be. The value is
/// kept verbatim (no normalisation), so both consumers see the same path.
#[must_use]
pub fn home_from_str(raw: Option<String>) -> Option<std::path::PathBuf> {
    raw.map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
}

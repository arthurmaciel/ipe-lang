//! The host environment re-exported, uninterpreted, into a jail's scrubbed env.
//!
//! A jail starts from an empty environment and receives only a fixed minimal
//! set (`LANG`, and on Windows `PATH` / `SystemRoot`) plus the names the
//! profile's `env` capability granted. [`granted`] is the one raw read behind
//! every such re-export and is crate-private: only this crate's jail builders
//! call it, with a fixed base name or a name drawn from `profile.env_allowlist`.
//! Outside the crate the only passthrough is [`granted_env`], which reads
//! nothing but the profile's own allowlist. A consented name — a home variable
//! included — is forwarded verbatim to the jailed child and never decides a
//! compiler-side path. Compiler-side path decisions read the home only through
//! `crate::home::home_dir` and every other variable through `ipe_env`, which
//! refuses home names.

use std::ffi::OsString;

use crate::run_jail::SandboxProfile;

/// The host value of `name`, for re-export into a jail's scrubbed environment.
#[must_use]
#[allow(clippy::disallowed_methods)] // capability passthrough: forwarded verbatim, never interpreted
pub(crate) fn granted(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// The host values of `profile`'s granted env names, in allowlist order.
///
/// A granted name the host leaves unset is omitted (never an empty value), and
/// a name outside the allowlist is never read.
#[must_use]
pub fn granted_env(profile: &SandboxProfile) -> Vec<(String, OsString)> {
    profile
        .env_allowlist
        .iter()
        .filter_map(|name| granted(name).map(|value| (name.clone(), value)))
        .collect()
}

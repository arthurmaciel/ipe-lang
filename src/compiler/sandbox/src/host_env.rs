//! The host environment re-exported, uninterpreted, into a jail's scrubbed env.
//!
//! A jail starts from an empty environment and receives only a fixed minimal
//! set (`LANG`, and on Windows `PATH` / `SystemRoot`) plus the names the
//! profile's `env` capability granted. [`granted`] is the one raw read behind
//! every such re-export: its value is forwarded verbatim to the jailed child
//! and never decides a compiler-side path, so a program that was granted a
//! home variable receives it as data. Compiler-side path decisions read the
//! home only through `crate::home::home_dir` and every other variable through
//! `ipe_env`, which refuses home names.

use std::ffi::OsString;

/// The host value of `name`, for re-export into a jail's scrubbed environment.
#[must_use]
#[allow(clippy::disallowed_methods)] // capability passthrough: forwarded verbatim, never interpreted
pub fn granted(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

//! The OS temp root for test code.
//!
//! Production code never names the shared temp base: it creates every
//! temporary entry through the scratch primitive (`ipe_sandbox::scratch`, or
//! the runtime's `scratch_core`), which verifies the base and creates entries
//! exclusively under it. Clippy bans the standard library's and `tempfile`'s
//! temp-root lookups everywhere else, so a test that needs the root reads it
//! here. This crate is a dev-dependency only; the scratch text gate refuses a
//! manifest that lists it under `[dependencies]` or `[build-dependencies]`.

use std::path::PathBuf;

/// The OS temp root, as `std::env::temp_dir`.
#[must_use]
#[allow(clippy::disallowed_methods)] // the sanctioned test-support reader of the temp root
pub fn temp_root() -> PathBuf {
    std::env::temp_dir()
}

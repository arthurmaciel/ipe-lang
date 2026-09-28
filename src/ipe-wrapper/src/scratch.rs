//! Unpredictable, exclusively-created scratch directory for the jail's writable
//! mount.
//!
//! Naming, exclusive creation, and owner privacy are all
//! [`ipe_sandbox::private_scratch`]'s: the name carries 128 bits of OS CSPRNG
//! entropy, a pre-existing entry or symlink causes a bounded retry rather than
//! being followed, and a location that cannot be proven private is refused. The
//! RAII guard removes the directory on drop, so callers need no manual cleanup.

use std::io;
use std::path::{Path, PathBuf};

use ipe_sandbox::private_scratch::create_unique_dir;

/// An exclusively-created, owner-private, unpredictably-named temporary directory.
///
/// Removed on drop (best-effort).
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    /// Create a new exclusively-owned temporary directory whose name is
    /// unpredictable.
    ///
    /// # Errors
    ///
    /// As [`ipe_sandbox::private_scratch::create_unique_dir`].
    pub fn new(prefix: &str) -> io::Result<Self> {
        Ok(Self(create_unique_dir(&std::env::temp_dir(), prefix)?))
    }

    /// The path of this scratch directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

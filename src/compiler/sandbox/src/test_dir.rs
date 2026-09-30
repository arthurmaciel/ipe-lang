//! A test-only scratch directory, removed when dropped.

use std::path::{Path, PathBuf};

use crate::scratch::ScratchDir;

/// A fresh canonical scratch directory, removed with its contents on drop.
pub struct TestDir {
    /// The canonical path of `_dir`.
    path: PathBuf,
    /// The private directory, removed on drop.
    _dir: ScratchDir,
}

impl TestDir {
    /// A fresh private directory named after `label`.
    ///
    /// # Errors
    /// The I/O error when the directory cannot be created or resolved.
    pub fn new(label: &str) -> std::io::Result<Self> {
        let dir = ScratchDir::new(&format!("ipe-sandbox-{label}"))?;
        let path = std::fs::canonicalize(dir.path())?;
        Ok(Self { path, _dir: dir })
    }

    /// The canonical directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

//! A test-only scratch directory, removed when dropped.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A fresh canonical scratch directory, removed with its contents on drop.
pub struct TestDir(PathBuf);

impl TestDir {
    /// A fresh directory named after `label`, unique within the test process.
    ///
    /// # Errors
    /// The I/O error when the directory cannot be created or resolved.
    pub fn new(label: &str) -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ipe-sandbox-{label}-{}-{serial}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        std::fs::canonicalize(&dir).map(Self)
    }

    /// The canonical directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

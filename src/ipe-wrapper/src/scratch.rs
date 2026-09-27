//! Unpredictable, exclusively-created scratch directory for the jail's writable
//! mount.
//!
//! The name carries 128 bits of OS CSPRNG entropy (not just a PID), creation uses
//! exclusive `create_dir` (fail on a pre-existing entry or symlink rather than
//! follow it), and the directory is mode 0700.  The RAII guard removes it on
//! drop, so callers need no manual cleanup.

use std::io;
use std::path::{Path, PathBuf};

/// Maximum retry attempts when an exclusive-create collision occurs.
const MAX_RETRIES: usize = 8;

/// Read 16 bytes (128 bits) from the OS CSPRNG.
///
/// `getrandom` reaches each target's real CSPRNG (`getrandom(2)` on Linux,
/// `BCryptGenRandom` on Windows) with no weaker fallback, so an unavailable
/// source is an error and the scratch name is never predictable.
fn read_entropy() -> io::Result<[u8; 16]> {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf)?;
    Ok(buf)
}

/// Format 16 entropy bytes as a 32-character lowercase hex string.
fn hex32(bytes: [u8; 16]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(32);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// An exclusively-created, mode-0700, unpredictably-named temporary directory.
///
/// Removed on drop (best-effort).
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    /// Create a new exclusively-owned temporary directory whose name is
    /// unpredictable.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] when entropy cannot be read or when all
    /// [`MAX_RETRIES`] attempts fail with `AlreadyExists`.
    pub fn new(prefix: &str) -> io::Result<Self> {
        let base = std::env::temp_dir();
        for _ in 0..MAX_RETRIES {
            let name = format!("{prefix}-{}-{}", std::process::id(), hex32(read_entropy()?));
            let path = base.join(&name);
            match exclusive_mkdir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique scratch directory after repeated attempts",
        ))
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

/// Create a directory exclusively — fail with `AlreadyExists` rather than
/// following a pre-existing entry or a symlink.
#[cfg(unix)]
fn exclusive_mkdir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(false)
        .create(path)
}

#[cfg(not(unix))]
fn exclusive_mkdir(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

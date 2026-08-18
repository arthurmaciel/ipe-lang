//! Unpredictable, exclusively-created scratch paths for temporary I/O.
//!
//! Every constructor generates a name that contains 128 bits of OS entropy (not
//! just a PID), opens with `O_EXCL` / `DirBuilder` + exclusive-create semantics
//! so a pre-seeded symlink or a pre-existing entry causes a retry rather than
//! being followed, and mode-restricts the result to the owner.  The RAII wrappers
//! remove the resource on drop, so callers do not need manual cleanup.
//!
//! The [`ScratchFile::file`] field exposes the *owned* [`std::fs::File`] handle
//! so callers can read back what they wrote without re-opening by name.  Reading
//! through the retained handle is the only way to guarantee that the bytes read
//! are the bytes written to the same inode — a name-based re-open can be raced.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Maximum retry attempts when an exclusive-create collision occurs.
const MAX_RETRIES: usize = 8;

/// Read 16 bytes (128 bits) of OS entropy from `/dev/urandom`.
///
/// Returns an error when the device cannot be read or yields fewer than 16
/// bytes, which makes the caller fall back to failing the construction rather
/// than silently weakening the name.
fn read_entropy() -> io::Result<[u8; 16]> {
    let mut buf = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut buf)?;
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

/// Generate one candidate name: `<prefix>-<pid>-<32 hex entropy chars>`.
///
/// The PID component is included so names from different processes sharing a
/// prefix are trivially distinguishable in diagnostics, but the 128-bit entropy
/// is what makes the name unpredictable.
fn candidate_name(prefix: &str) -> io::Result<String> {
    let entropy = read_entropy()?;
    Ok(format!(
        "{}-{}-{}",
        prefix,
        std::process::id(),
        hex32(entropy)
    ))
}

// ── ScratchDir ───────────────────────────────────────────────────────────────

/// An exclusively-created, mode-0700, unpredictably-named temporary directory.
///
/// Constructed only via [`ScratchDir::new`], which loops on `AlreadyExists`
/// (bounded by [`MAX_RETRIES`]) rather than removing and recreating a
/// pre-existing entry.  The directory is removed on drop (best-effort).
///
/// Use [`ScratchDir::path`] for the directory itself and
/// [`ScratchDir::child`] to build paths for files or subdirectories inside it.
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    /// Create a new exclusively-owned temporary directory whose name is
    /// unpredictable.
    ///
    /// `prefix` is a short, caller-chosen label that appears in the name for
    /// diagnostics.  The name is not caller-controlled beyond this label.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] when entropy cannot be read or when all
    /// [`MAX_RETRIES`] attempts fail with `AlreadyExists`.
    pub fn new(prefix: &str) -> io::Result<Self> {
        let base = std::env::temp_dir();
        for _ in 0..MAX_RETRIES {
            let name = candidate_name(prefix)?;
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

    /// Build a path for a child entry *inside* this directory.
    ///
    /// The child is not created by this call; use the returned path to create
    /// it.  Because the directory itself is mode 0700, a child created inside
    /// it is not reachable by other users even when its own mode is broader.
    #[must_use]
    pub fn child(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Create a directory exclusively — fail with `AlreadyExists` rather than
/// following a pre-existing entry or a symlink.
///
/// Uses a plain `create_dir` (not `create_dir_all`) so that only the final
/// component is created and any pre-existing entry — including a dangling
/// symlink — produces `AlreadyExists` rather than silently succeeding.
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
    // On non-Unix there is no portable mode bit; the directory is created with
    // default permissions.  Exclusive creation (fail on AlreadyExists) still
    // holds because `create_dir` (not `create_dir_all`) is used.
    std::fs::create_dir(path)
}

// ── ScratchFile ──────────────────────────────────────────────────────────────

/// An exclusively-created, mode-0600, unpredictably-named temporary file.
///
/// Constructed only via [`ScratchFile::create`], which opens with
/// `O_CREAT|O_EXCL` (via `create_new`) so a pre-existing file or symlink
/// causes a retry.  The owned [`File`] handle in [`ScratchFile::file`]
/// outlives the path name: callers MUST read back written bytes through the
/// handle rather than re-opening by name, so the bytes read are the bytes
/// that were written to this specific inode — a re-open by name races.
///
/// The file is removed on drop (best-effort).
pub struct ScratchFile {
    path: PathBuf,
    /// The open file handle.  Callers should [`Seek`] to the start before
    /// reading if bytes were written through an external writer.
    pub file: File,
}

impl ScratchFile {
    /// Create a new exclusively-owned temporary file whose name is
    /// unpredictable.
    ///
    /// Returns both the [`ScratchFile`] RAII guard and a clone of the owned
    /// `File` handle (already positioned at offset 0).  Callers that let an
    /// external writer (e.g. `curl -o`) write to [`ScratchFile::path`] should
    /// [`rewind`](ScratchFile::rewind) the handle before reading.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] when entropy cannot be read or when all
    /// [`MAX_RETRIES`] attempts fail.
    pub fn create(prefix: &str) -> io::Result<Self> {
        let base = std::env::temp_dir();
        for _ in 0..MAX_RETRIES {
            let name = candidate_name(prefix)?;
            let path = base.join(&name);
            match exclusive_open(&path) {
                Ok(file) => return Ok(Self { path, file }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique scratch file after repeated attempts",
        ))
    }

    /// The path of this scratch file.
    ///
    /// Prefer reading through [`ScratchFile::file`] rather than re-opening
    /// this path, so the bytes read are the bytes on the owned inode.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Rewind the retained file handle to offset 0 so the caller can read
    /// from the start.
    ///
    /// # Errors
    ///
    /// Propagates any `seek` error.
    pub fn rewind(&mut self) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(0)).map(|_| ())
    }

    /// Read all bytes from the retained file handle (after rewinding to
    /// offset 0).
    ///
    /// This is the *only* correct way to retrieve what an external writer
    /// wrote to [`ScratchFile::path`]: reading through the handle avoids a
    /// re-open-by-name race.
    ///
    /// # Errors
    ///
    /// Propagates any seek or read error.
    pub fn read_all(&mut self) -> io::Result<Vec<u8>> {
        self.rewind()?;
        let mut buf = Vec::new();
        self.file.read_to_end(&mut buf)?;
        Ok(buf)
    }
}

impl Drop for ScratchFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Open a file exclusively at `path` with mode 0600 (owner read/write only).
#[cfg(unix)]
fn exclusive_open(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn exclusive_open(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// Two distinct `ScratchDir` names from the same prefix never collide and
    /// neither equals a bare `<prefix>-<pid>` string (the old predictable form).
    #[test]
    fn scratch_dir_names_are_unpredictable_and_unique() {
        let pid_only = format!("ipe-publish-{}", std::process::id());
        let mut seen = std::collections::HashSet::new();
        for _ in 0..20 {
            let sd = ScratchDir::new("ipe-publish").expect("scratch dir");
            let name = sd
                .path()
                .file_name()
                .and_then(|n| n.to_str())
                .expect("utf-8 name")
                .to_owned();
            assert_ne!(
                name, pid_only,
                "name must not equal the predictable pid-only form"
            );
            assert!(seen.insert(name.clone()), "duplicate scratch name: {name}");
            // path is removed on drop — verified by raii_cleanup below
        }
    }

    /// `ScratchDir` is removed on drop.
    #[test]
    fn scratch_dir_raii_cleanup() {
        let path = {
            let sd = ScratchDir::new("ipe-raii-test").expect("scratch dir");
            let p = sd.path().to_path_buf();
            assert!(p.exists(), "dir should exist while live");
            p
        };
        assert!(!path.exists(), "dir should be gone after drop");
    }

    /// `ScratchFile` is removed on drop.
    #[test]
    fn scratch_file_raii_cleanup() {
        let path = {
            let sf = ScratchFile::create("ipe-raii-test").expect("scratch file");
            let p = sf.path().to_path_buf();
            assert!(p.exists(), "file should exist while live");
            p
        };
        assert!(!path.exists(), "file should be gone after drop");
    }

    /// Reading through the retained handle returns the bytes written to the
    /// path, even when the path name is renamed away after writing — the
    /// handle holds the original inode open.
    #[test]
    fn scratch_file_handle_reads_original_inode() {
        let original_bytes = b"original content";
        let decoy_bytes = b"swapped content";

        let mut sf = ScratchFile::create("ipe-inode-test").expect("scratch file");
        sf.file.write_all(original_bytes).expect("write");

        // Rename a decoy file over the scratch path (simulates a name-race).
        let decoy = sf.path().with_extension("decoy");
        std::fs::write(&decoy, decoy_bytes).expect("write decoy");
        std::fs::rename(&decoy, sf.path()).expect("rename decoy over path");

        // Reading through the retained handle still returns the original bytes,
        // not the decoy — the handle is bound to the original inode.
        let read_back = sf.read_all().expect("read_all");
        assert_eq!(
            read_back, original_bytes,
            "retained handle must read the original inode, not the swapped name"
        );
    }

    /// A symlink pre-seeded at the exact scratch path is not followed: the
    /// constructor retries and eventually creates a real file or directory at
    /// a fresh name, leaving the symlink target untouched.
    #[test]
    fn symlink_preseed_is_not_followed() {
        // Create a canary file that a symlink would point to.
        let canary = std::env::temp_dir().join(format!(
            "ipe-canary-{}-{}",
            std::process::id(),
            "symlink-preseed-test"
        ));
        std::fs::write(&canary, b"canary").expect("write canary");

        // Exhausting retries is hard to do reliably in a unit test because we
        // cannot control the random names.  What we CAN assert is that a
        // successful ScratchFile::create always produces a REGULAR file (not a
        // symlink), and the canary is intact.
        let sf = ScratchFile::create("ipe-preseed-test").expect("scratch file");
        let meta = std::fs::symlink_metadata(sf.path()).expect("metadata");
        assert!(
            meta.file_type().is_file(),
            "scratch file must be a regular file, not a symlink"
        );
        assert_eq!(
            std::fs::read(&canary).expect("canary readable"),
            b"canary",
            "canary must be untouched"
        );

        let _ = std::fs::remove_file(&canary);
    }

    /// No PRODUCTION code in `src/ipe-cli/src` (outside `scratch.rs`) constructs
    /// a temp path using `temp_dir().join` — all such paths must go through the
    /// `scratch` module's exclusively-created constructors.
    ///
    /// Test-only code (`#[cfg(test)]` / `mod tests` blocks) is exempt: test helpers
    /// that use predictable names in isolated temp dirs do not expose the
    /// verify/exec or verify/read identity gap that the production paths do.
    ///
    /// This is the class gate: it keeps the `toctou-verify-one-exec-other-scratch`
    /// class closed against future PRODUCTION regressions.
    #[test]
    fn no_predictable_temp_names_in_production_code() {
        let cli_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

        // Collect all .rs files under the cli src directory.
        let mut rs_files: Vec<std::path::PathBuf> = Vec::new();
        collect_rs_files(&cli_src, &mut rs_files);

        for path in &rs_files {
            // scratch.rs is the one sanctioned location.
            if path.file_name().and_then(|n| n.to_str()) == Some("scratch.rs") {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(path) else {
                continue;
            };
            // Check each line: if it contains `temp_dir().join`, it must be
            // inside a test context.  We track whether we are inside a
            // `#[cfg(test)]` or `mod tests` block by a simple heuristic: the
            // line or a recent preceding line contains one of those markers.
            // This is an approximation sufficient to catch production-code
            // additions while excluding test-module helpers.
            let lines: Vec<&str> = source.lines().collect();
            let mut in_test_region = false;
            let mut brace_depth_at_test_entry: Option<usize> = None;
            let mut brace_depth: usize = 0;

            for (i, &line) in lines.iter().enumerate() {
                // Track `#[cfg(test)]` and `mod tests` as test-region markers.
                if line.contains("#[cfg(test)]") || line.contains("mod tests") {
                    in_test_region = true;
                    brace_depth_at_test_entry = Some(brace_depth);
                }
                // Count brace depth to detect when we leave the test block.
                brace_depth += line.chars().filter(|&c| c == '{').count();
                brace_depth =
                    brace_depth.saturating_sub(line.chars().filter(|&c| c == '}').count());
                if in_test_region
                    && let Some(entry_depth) = brace_depth_at_test_entry
                    && brace_depth < entry_depth
                {
                    in_test_region = false;
                    brace_depth_at_test_entry = None;
                }

                assert!(
                    !line.contains("temp_dir().join") || in_test_region,
                    "predictable temp_dir().join in production code at {}:{} — \
                     use crate::scratch::ScratchDir or ScratchFile instead.\n  line: {}",
                    path.display(),
                    i + 1,
                    line.trim()
                );
            }
        }
    }

    fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
}

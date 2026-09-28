//! The one bounded reader for an untrusted workspace file, and the `lint.ipe`
//! loader built on it.
//!
//! A workspace file is attacker-shaped: it may be swapped for a FIFO, a
//! directory, a device, or a multi-GiB blob between any two path lookups. So
//! the path is resolved exactly once — [`read_workspace_file`] opens it, then
//! proves the *opened handle* (never the path again) is a regular file, then
//! reads at most `max + 1` bytes. On Unix the open is `O_NONBLOCK`, so opening
//! a FIFO with no writer returns at once and the handle check refuses it
//! instead of the caller blocking forever. Symlinks are followed: the handle
//! check applies to whatever the link resolved to, which is the file the read
//! actually consumes.

use std::io::Read as _;
use std::path::Path;

use crate::config::{ConfigError, LINT_CONFIG_FILE, LINT_CONFIG_MAX_BYTES, LintConfig};

/// Why [`read_workspace_file`] yielded no text.
#[derive(Debug)]
pub enum WorkspaceReadError {
    /// The opened handle is not a regular file (a directory, FIFO, socket, or
    /// device).
    NotAFile,
    /// The file could not be opened, inspected, or read.
    Unreadable(std::io::Error),
    /// The file holds more than the caller's cap, in bytes.
    TooLarge {
        /// The cap the file exceeded.
        max: u64,
    },
    /// The file is not valid UTF-8.
    NotUtf8,
}

impl std::fmt::Display for WorkspaceReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAFile => f.write_str("not a regular file"),
            Self::Unreadable(e) => write!(f, "unreadable: {e}"),
            Self::TooLarge { max } => write!(f, "exceeds {max} bytes"),
            Self::NotUtf8 => f.write_str("not valid UTF-8"),
        }
    }
}

impl std::error::Error for WorkspaceReadError {}

/// Open `path` once and read it as text, bounded by `max` bytes.
///
/// `Ok(None)` when nothing exists at `path`. The regular-file proof is taken
/// on the opened handle, so no swap between check and read is possible; the
/// read never buffers more than `max + 1` bytes. A file exactly at the cap is
/// accepted; one byte over is refused.
///
/// # Errors
/// [`WorkspaceReadError`] when the handle is not a regular file, the open or
/// read fails, the content is over `max`, or it is not UTF-8.
pub fn read_workspace_file(path: &Path, max: u64) -> Result<Option<String>, WorkspaceReadError> {
    let file = match open_nonblocking(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(WorkspaceReadError::Unreadable(e)),
    };
    let meta = file.metadata().map_err(WorkspaceReadError::Unreadable)?;
    if !meta.is_file() {
        return Err(WorkspaceReadError::NotAFile);
    }
    let mut buf = Vec::new();
    file.take(max.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(WorkspaceReadError::Unreadable)?;
    if !u64::try_from(buf.len()).is_ok_and(|len| len <= max) {
        return Err(WorkspaceReadError::TooLarge { max });
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|_| WorkspaceReadError::NotUtf8)
}

/// Open `path` read-only without blocking on a FIFO that has no writer.
#[cfg(unix)]
fn open_nonblocking(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

/// Open `path` read-only. A workspace path cannot name a Windows named pipe
/// (those live only under `\\.\pipe\`), so a plain open cannot block.
#[cfg(not(unix))]
fn open_nonblocking(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

/// Why [`load_lint_config`] yielded no configuration.
#[derive(Debug)]
pub enum LintConfigLoadError {
    /// `lint.ipe` exists but could not be read within bounds.
    Read(WorkspaceReadError),
    /// `lint.ipe` was read but is not a valid configuration.
    Invalid(ConfigError),
}

impl std::fmt::Display for LintConfigLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(e) => write!(f, "{LINT_CONFIG_FILE}: {e}"),
            Self::Invalid(e) => std::fmt::Display::fmt(e, f),
        }
    }
}

impl std::error::Error for LintConfigLoadError {}

/// Load the [`LintConfig`] from the `lint.ipe` in `dir`.
///
/// An absent file is the default configuration. Every other outcome goes
/// through [`read_workspace_file`] with [`LINT_CONFIG_MAX_BYTES`], so a FIFO,
/// a directory, or an oversized file is refused, never waited on or buffered.
///
/// # Errors
/// [`LintConfigLoadError`] when the file cannot be read within bounds or does
/// not parse as a configuration.
pub fn load_lint_config(dir: &Path) -> Result<LintConfig, LintConfigLoadError> {
    let path = dir.join(LINT_CONFIG_FILE);
    match read_workspace_file(&path, LINT_CONFIG_MAX_BYTES).map_err(LintConfigLoadError::Read)? {
        None => Ok(LintConfig::default()),
        Some(text) => crate::config::read_lint_config(&text, &path.display().to_string())
            .map_err(LintConfigLoadError::Invalid),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{LintConfigLoadError, WorkspaceReadError, load_lint_config, read_workspace_file};
    use crate::config::{LINT_CONFIG_FILE, LINT_CONFIG_MAX_BYTES};

    /// A fresh, empty scratch directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ipe-lint-wsread-{}-{name}", std::process::id()));
        if dir.exists() {
            assert!(std::fs::remove_dir_all(&dir).is_ok(), "clear {dir:?}");
        }
        assert!(std::fs::create_dir_all(&dir).is_ok(), "create {dir:?}");
        dir
    }

    #[test]
    fn an_absent_file_is_none_and_the_default_config() {
        let dir = scratch("absent");
        assert!(matches!(
            read_workspace_file(&dir.join("nope"), 16),
            Ok(None)
        ));
        assert!(load_lint_config(&dir).is_ok());
    }

    #[test]
    fn a_file_exactly_at_the_cap_is_read() {
        let dir = scratch("at-cap");
        let path = dir.join("f");
        assert!(std::fs::write(&path, [b'a'; 16]).is_ok());
        assert!(matches!(read_workspace_file(&path, 16), Ok(Some(t)) if t.len() == 16));
    }

    #[test]
    fn a_file_one_byte_over_the_cap_is_refused() {
        let dir = scratch("over-cap");
        let path = dir.join("f");
        assert!(std::fs::write(&path, [b'a'; 17]).is_ok());
        assert!(matches!(
            read_workspace_file(&path, 16),
            Err(WorkspaceReadError::TooLarge { max: 16 })
        ));
    }

    #[test]
    fn an_oversized_lint_config_is_refused() {
        let dir = scratch("lint-over-cap");
        let over = usize::try_from(LINT_CONFIG_MAX_BYTES.saturating_add(1));
        assert!(
            matches!(over, Ok(n) if std::fs::write(dir.join(LINT_CONFIG_FILE), vec![b' '; n]).is_ok())
        );
        assert!(matches!(
            load_lint_config(&dir),
            Err(LintConfigLoadError::Read(
                WorkspaceReadError::TooLarge { .. }
            ))
        ));
    }

    #[test]
    fn a_directory_is_refused() {
        let dir = scratch("dir");
        assert!(std::fs::create_dir_all(dir.join(LINT_CONFIG_FILE)).is_ok());
        let refused = load_lint_config(&dir);
        #[cfg(unix)]
        assert!(matches!(
            refused,
            Err(LintConfigLoadError::Read(WorkspaceReadError::NotAFile))
        ));
        #[cfg(not(unix))]
        assert!(matches!(refused, Err(LintConfigLoadError::Read(_))));
    }

    #[test]
    fn non_utf8_is_refused() {
        let dir = scratch("non-utf8");
        let path = dir.join("f");
        assert!(std::fs::write(&path, [0xff, 0xfe]).is_ok());
        assert!(matches!(
            read_workspace_file(&path, 16),
            Err(WorkspaceReadError::NotUtf8)
        ));
    }

    /// Run `load_lint_config(dir)` on a helper thread and require an answer
    /// within a deadline, so a regression that blocks fails instead of hanging.
    #[cfg(unix)]
    fn load_promptly(dir: PathBuf) -> Option<Result<crate::LintConfig, LintConfigLoadError>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(load_lint_config(&dir));
        });
        rx.recv_timeout(Duration::from_secs(10)).ok()
    }

    /// Create a FIFO at `path` with the system `mkfifo`.
    #[cfg(unix)]
    fn mkfifo(path: &std::path::Path) {
        let made = std::process::Command::new("mkfifo").arg(path).status();
        assert!(
            matches!(made, Ok(s) if s.success()),
            "mkfifo {path:?}: {made:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let dir = scratch("fifo");
        mkfifo(&dir.join(LINT_CONFIG_FILE));
        assert!(matches!(
            load_promptly(dir),
            Some(Err(LintConfigLoadError::Read(WorkspaceReadError::NotAFile)))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_fifo_is_refused_without_blocking() {
        let dir = scratch("fifo-link");
        let fifo = dir.join("pipe");
        mkfifo(&fifo);
        assert!(std::os::unix::fs::symlink(&fifo, dir.join(LINT_CONFIG_FILE)).is_ok());
        assert!(matches!(
            load_promptly(dir),
            Some(Err(LintConfigLoadError::Read(WorkspaceReadError::NotAFile)))
        ));
    }
}

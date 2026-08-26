//! Bounded file-I/O utilities for the CLI/FFI layer.
//!
//! Every CLI or FFI-cache file that must be turned into a `String` goes through
//! [`read_to_string_capped`] — the single capped reader. This mirrors the
//! runtime's own `file.rs` ceiling and closes the same defect class for the
//! compile-time surfaces: an unbounded `std::fs::read_to_string` is not
//! reachable from this crate's public paths.
//!
//! Cap constants are declared here as the single source of truth so a new call
//! site cannot silently introduce a different ceiling.

use std::io::Read as _;
use std::path::Path;

use crate::CliError;

// ── Per-surface read caps ─────────────────────────────────────────────────────

/// Maximum bytes for a `package.ipe` or legacy `ipe.toml` manifest file.
///
/// 512 KiB is generous for any real manifest while refusing a device node or a
/// multi-GiB file that would exhaust memory.
pub const MANIFEST_READ_CAP: u64 = 512 * 1024;

/// Maximum bytes for a single Ipê source file (`*.ipe`).
///
/// 8 MiB matches the runtime's file-read ceiling and is well above any
/// realistic source file.
pub const SOURCE_READ_CAP: u64 = 8 * 1024 * 1024;

/// Maximum bytes for an FFI cache artifact (JSON / Rust source fragments
/// stored under `.ipe/cache/ffi/rust/`).
///
/// 4 MiB is sufficient for any generated bindings file while refusing a
/// device node or accidentally-swapped large binary.
pub const FFI_CACHE_READ_CAP: u64 = 4 * 1024 * 1024;

/// Maximum bytes for miscellaneous small CLI-internal files (lock files,
/// index entries, OAuth tokens, Cargo profile fragments, etc.).
pub const SMALL_FILE_READ_CAP: u64 = 1024 * 1024;

// ── Capped reader ─────────────────────────────────────────────────────────────

/// Read a file to a `String`, refusing past `max` bytes with a typed
/// [`CliError::FileTooLarge`] instead of allocating without a ceiling.
///
/// Reads `max + 1` bytes in one pass via [`std::io::Read::take`] and checks
/// the actual byte count, so it never buffers more than the cap. A file
/// exactly at the cap succeeds; a file one byte over fails.
///
/// This is the ONLY approved path for turning a CLI/FFI-cache file path into a
/// `String`. Call it with the appropriate [`MANIFEST_READ_CAP`] /
/// [`SOURCE_READ_CAP`] / [`FFI_CACHE_READ_CAP`] / [`SMALL_FILE_READ_CAP`]
/// constant — never pass an ad-hoc magic number.
///
/// # Errors
///
/// - [`CliError::Io`] if the file cannot be opened or read.
/// - [`CliError::FileTooLarge`] if the file exceeds `max` bytes.
/// - [`CliError::Io`] (kind `InvalidData`) if the content is not valid UTF-8.
pub fn read_to_string_capped(path: &Path, max: u64) -> Result<String, CliError> {
    let f = std::fs::File::open(path).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut buf = Vec::new();
    f.take(max.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| CliError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
    if buf.len() as u64 > max {
        return Err(CliError::FileTooLarge {
            path: path.to_path_buf(),
            max,
        });
    }
    String::from_utf8(buf).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp(name: &str, content: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("ipe_iob_{name}_{}.bin", std::process::id()));
        std::fs::write(&p, content).expect("write temp");
        p
    }

    #[test]
    fn under_cap_reads_full_content() {
        let p = write_temp("under", b"hello world");
        let result = read_to_string_capped(&p, SMALL_FILE_READ_CAP);
        let _ = std::fs::remove_file(&p);
        assert_eq!(result.expect("under cap must succeed"), "hello world");
    }

    #[test]
    fn exactly_at_cap_is_ok() {
        let content = vec![b'a'; 16];
        let p = write_temp("exact", &content);
        let result = read_to_string_capped(&p, 16);
        let _ = std::fs::remove_file(&p);
        assert_eq!(result.expect("exactly at cap must succeed").len(), 16);
    }

    #[test]
    fn one_byte_over_cap_is_typed_error() {
        let content = vec![b'a'; 17];
        let p = write_temp("over", &content);
        let result = read_to_string_capped(&p, 16);
        let _ = std::fs::remove_file(&p);
        assert!(
            matches!(result, Err(CliError::FileTooLarge { .. })),
            "one byte over cap must be FileTooLarge, got: {result:?}"
        );
    }

    #[test]
    fn manifest_cap_refuses_gigabyte_device_node_simulation() {
        // Simulate oversized input: write a file just past the manifest cap.
        // 512 KiB + 1 byte; the literal avoids a u64→usize cast lint in tests.
        let content = vec![b'x'; 512 * 1024 + 1];
        let p = write_temp("manifest_over", &content);
        let result = read_to_string_capped(&p, MANIFEST_READ_CAP);
        let _ = std::fs::remove_file(&p);
        assert!(
            matches!(result, Err(CliError::FileTooLarge { .. })),
            "file over manifest cap must be FileTooLarge"
        );
    }

    #[test]
    fn missing_file_is_io_error() {
        let p = std::path::PathBuf::from("/nonexistent/path/that/cannot/exist");
        let result = read_to_string_capped(&p, 1024);
        assert!(
            matches!(result, Err(CliError::Io { .. })),
            "missing file must be Io error"
        );
    }
}

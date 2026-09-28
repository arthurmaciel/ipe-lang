//! The runtime source walk shared by the `syn` source scans
//! (`no_ungated_dial.rs`, `no_panicking_print_macro.rs`).

use std::path::{Path, PathBuf};

/// The most directory entries the source walk reads before it fails.
const MAX_SOURCE_ENTRIES: usize = 8192;

/// Every `.rs` file under `root`, as its `/`-separated path relative to `root`
/// and its contents, sorted by path.
///
/// A symbolic link is refused rather than followed or skipped, so every file
/// the build can read is one the walk read.
#[allow(clippy::expect_used)] // an unreadable source must fail the scan, never be skipped
pub fn rust_sources(root: &Path) -> Vec<(String, String)> {
    let mut dirs: Vec<PathBuf> = vec![root.to_path_buf()];
    let mut files: Vec<PathBuf> = Vec::new();
    let mut entries = 0usize;
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("read a source directory") {
            let entry = entry.expect("read a source directory entry");
            entries = entries.saturating_add(1);
            assert!(
                entries <= MAX_SOURCE_ENTRIES,
                "more than {MAX_SOURCE_ENTRIES} entries under {}",
                root.display()
            );
            let kind = entry.file_type().expect("read a source entry's type");
            let path = entry.path();
            assert!(
                !kind.is_symlink(),
                "{}: a symbolic link in the source tree is not scanned",
                path.display()
            );
            if kind.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    let mut sources: Vec<(String, String)> = files
        .iter()
        .map(|path| {
            let name = path
                .strip_prefix(root)
                .expect("a walked file lies under the root")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            (
                name,
                std::fs::read_to_string(path).expect("read a source file"),
            )
        })
        .collect();
    sources.sort();
    sources
}

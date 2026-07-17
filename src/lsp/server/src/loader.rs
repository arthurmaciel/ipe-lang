//! The project-resolution seam between the server loop and the CLI driver.
//!
//! Discovering a project (manifest walk-up, sibling discovery, stdlib
//! injection) is driver logic that reads the filesystem — it lives in the
//! `ipe` crate, which depends on this one. The server therefore receives it
//! as a [`ProjectLoader`] implementation; the query layer below stays free
//! of hidden inputs, and tests substitute a fixture loader with no
//! filesystem at all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use ipe_db::ModuleOrigin;

/// One resolved module of a loaded project.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LoadedFile {
    /// The module's on-disk source path (absolute on the real driver path).
    pub path: PathBuf,
    /// The module's source text as the loader resolved it (disk bytes,
    /// except the anchor file when an overlay was supplied).
    pub text: String,
    /// The driver-vouched trust tag ([`ipe_db::SourceFile`]'s `origin`).
    pub origin: ModuleOrigin,
}

/// A fully resolved project: every in-scope module plus the entry module.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LoadedProject {
    /// Module path → resolved file, for every module in the build
    /// (user sources plus the injected stdlib closure).
    pub files: BTreeMap<Vec<String>, LoadedFile>,
    /// The entry module's path segments (e.g. `["Main"]`).
    pub entry_module: Vec<String>,
}

/// A project-resolution failure. Carries the driver's rendered detail; the
/// server logs it and degrades to single-file service, never crashes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LoadError {
    /// Human-readable failure detail (already rendered by the driver).
    pub detail: String,
}

/// Resolves the project that contains an opened document.
pub trait ProjectLoader {
    /// Resolve the project containing `open_file`.
    ///
    /// `workspace_root` is the editor's workspace folder (used when it holds
    /// a manifest); `open_text`, when present, is the editor's current
    /// buffer for `open_file` and shadows its disk bytes during resolution
    /// (the VFS overlay applied at the discovery step).
    ///
    /// # Errors
    /// [`LoadError`] when no project shape can be resolved around
    /// `open_file` (no manifest, undiscoverable module layout, I/O failure).
    fn load(
        &self,
        workspace_root: Option<&Path>,
        open_file: &Path,
        open_text: Option<&str>,
    ) -> Result<LoadedProject, LoadError>;
}

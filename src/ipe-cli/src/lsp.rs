//! `ipe lsp` — the JSON-RPC-over-stdio language server subcommand.
//!
//! The server loop and every feature handler live in `ipe_lsp_server` /
//! `ipe_lsp_features`; this module supplies the one driver-side ingredient
//! the server cannot own — project resolution. [`DriverLoader`] first
//! classifies the opened document as a [`ProjectRoot`]. A package routes
//! through the SAME manifest-discovery/stdlib-injection code path `ipe build`
//! and `ipe watch` use, so the module set the editor analyzes can never
//! diverge from the one the batch build compiles. A loose file (no
//! `package.ipe` above it) resolves through [`crate::loose_file`], the same
//! resolver `ipe build` and `ipe watch` use for it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ipe_lsp_server::{LoadError, LoadedFile, LoadedProject, ProjectLoader};

use crate::loose_file::{LooseFileLimits, ProjectRoot, resolve_loose_file};
use crate::{CliError, project, text, watch};

/// The user modules one project root resolves to, before stdlib and FFI injection.
struct UserSources {
    sources: BTreeMap<Vec<String>, (PathBuf, String)>,
    discovered: Vec<project::DiscoveredModule>,
    entry_module: Vec<String>,
    blame_path: PathBuf,
}

/// Resolve the user modules of `root`.
///
/// `open_text` shadows the loose file's disk bytes; a package reads disk
/// only (its layout comes from the manifest, not the open buffer).
fn resolve_user_sources(
    root: &ProjectRoot,
    open_text: Option<&str>,
) -> Result<UserSources, CliError> {
    match root {
        ProjectRoot::Package(dir) => {
            let resolved = watch::resolve_project_sources(dir, None)?;
            Ok(UserSources {
                sources: resolved.sources,
                discovered: resolved.discovered,
                entry_module: resolved.entry_path,
                blame_path: resolved.blame_path,
            })
        }
        ProjectRoot::LooseFile(file) => {
            let loaded = resolve_loose_file(file, open_text, LooseFileLimits::DEFAULT)?;
            Ok(UserSources {
                sources: loaded.sources,
                discovered: loaded.discovered,
                entry_module: loaded.entry_module,
                blame_path: file.clone(),
            })
        }
    }
}

/// Render a driver failure as the server's load error.
fn load_error(err: &CliError) -> LoadError {
    LoadError {
        detail: err.to_string(),
    }
}

struct DriverLoader;

impl ProjectLoader for DriverLoader {
    fn load(
        &self,
        workspace_root: Option<&Path>,
        open_file: &Path,
        open_text: Option<&str>,
    ) -> Result<LoadedProject, LoadError> {
        let root = ProjectRoot::of(workspace_root, open_file);
        let UserSources {
            mut sources,
            mut discovered,
            entry_module,
            blame_path,
        } = resolve_user_sources(&root, open_text).map_err(|e| load_error(&e))?;
        let injected = project::inject_compiled_std_closure(&mut sources, &mut discovered);
        // Load the FFI catalog and inject installed-crate interface modules so
        // the LSP sees `Rust.<Crate>` bindings exactly as `ipe build` does. A
        // missing/empty catalog is fine (no crates installed); a tampered
        // cache is surfaced as a `LoadError`.
        let ffi_injected = crate::ffi::prepare_ffi(&mut sources, &blame_path)
            .map_err(|e| load_error(&e))?
            .injected;
        let files = sources
            .into_iter()
            .map(|(module, (path, text))| {
                let origin = if injected.contains(&module) {
                    ipe_canon::ModuleOrigin::EmbeddedStdlib
                } else if ffi_injected.contains(&module) {
                    ipe_canon::ModuleOrigin::FfiInterface
                } else {
                    ipe_canon::ModuleOrigin::User
                };
                (module, LoadedFile { path, text, origin })
            })
            .collect();
        Ok(LoadedProject {
            files,
            entry_module,
        })
    }
}

/// `ipe lsp` — serve the Language Server Protocol over stdio until the
/// client disconnects.
///
/// # Errors
/// [`CliError`] on misuse (unexpected arguments) or a protocol-level
/// failure; never for a compile diagnostic (those flow to the editor).
pub fn run_lsp(rest: &[String]) -> Result<(), CliError> {
    if !rest.is_empty() {
        return Err(CliError::Usage(text::lsp_takes_no_arguments()));
    }
    ipe_lsp_server::run_stdio(&DriverLoader).map_err(|e| CliError::UsageOwned(text::lsp_failed(&e)))
}

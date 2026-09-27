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
            })
        }
        ProjectRoot::LooseFile(file) => {
            let loaded = resolve_loose_file(file, open_text, LooseFileLimits::DEFAULT)?;
            Ok(UserSources {
                sources: loaded.sources,
                discovered: loaded.discovered,
                entry_module: loaded.entry_module,
            })
        }
    }
}

/// Type a driver failure as the server's load error, keeping its rendered text.
///
/// A ceiling, an untrusted FFI cache, or a refused FFI catalog is refused;
/// every other failure is one the server degrades around. The match names
/// every variant, so a new [`CliError`] fails the build here until it is
/// classified rather than defaulting to a degradable failure.
fn load_error(err: &CliError) -> LoadError {
    let detail = err.to_string();
    match err {
        CliError::Io { .. } => LoadError::Io(detail),
        CliError::FileTooLarge { .. } | CliError::DiscoveryLimitReached { .. } => {
            LoadError::Limit(detail)
        }
        CliError::FfiCacheUntrusted { .. } | CliError::FfiCatalogRefused(_) => {
            LoadError::FfiUntrusted(detail)
        }
        CliError::Usage(_)
        | CliError::UsageOwned(_)
        | CliError::UnknownCommand { .. }
        | CliError::Pipeline { .. }
        | CliError::RuntimeNotFound
        | CliError::RuntimeDirInvalid { .. }
        | CliError::RuntimeHomeUnknown
        | CliError::RuntimeMaterializeFailed { .. }
        | CliError::RuntimeVersionMismatch { .. }
        | CliError::EmittedBuildFailed { .. }
        | CliError::UnknownCode { .. }
        | CliError::DocNotFound { .. }
        | CliError::StaticRefusal(_)
        | CliError::CapabilityMismatch { .. }
        | CliError::Resolve(_)
        | CliError::HashMismatch { .. }
        | CliError::Diff(_)
        | CliError::SemverRejected { .. }
        | CliError::PackageAudit(_)
        | CliError::Publish(_)
        | CliError::DocCoverage(_)
        | CliError::DocExamplesFailed(_)
        | CliError::CommandUsage { .. }
        | CliError::UnknownGroupSub { .. }
        | CliError::VerifyFailed { .. }
        | CliError::TestFailed { .. }
        | CliError::UpgradeNoPrebuilt { .. }
        | CliError::ToolchainMissing(_)
        | CliError::HealthCritical
        | CliError::LintGateFailed
        | CliError::EjectUnsupported { .. }
        | CliError::DiagnosticJsonEmitted
        | CliError::DeviceNamedModule { .. }
        | CliError::PathEscape { .. }
        | CliError::OutputRefused(_)
        | CliError::UpgradeFeedUnreachable
        | CliError::UpgradeCheckExit { .. }
        | CliError::AdvisoryVulnerable(_)
        | CliError::AdvisoryDbUnreachable { .. }
        | CliError::AdvisoryDbMalformed { .. }
        | CliError::WasiRunFeatureDisabled
        | CliError::WasiRunFailed { .. }
        | CliError::WasiRunExited { .. } => LoadError::Pipeline(detail),
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
        } = resolve_user_sources(&root, open_text).map_err(|e| load_error(&e))?;
        let injected = project::inject_compiled_std_closure(&mut sources, &mut discovered);
        // Load the FFI catalog and inject installed-crate interface modules so
        // the LSP sees `Rust.<Crate>` bindings exactly as `ipe build` does. A
        // missing/empty catalog is fine (no crates installed); a tampered
        // cache is surfaced as a `LoadError`. The catalog is scoped by the
        // same `root` the sources were resolved from.
        let ffi_injected = crate::ffi::prepare_ffi_in(&mut sources, &root)
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

#[cfg(test)]
mod tests {
    use ipe_lsp_server::LoadDisposition;

    use super::*;
    use crate::ffi::FfiCatalogRefusal;

    fn tmp_dir(name: &str) -> PathBuf {
        let tmp = std::env::temp_dir().join(format!("ipe-lsp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("mk tmp");
        tmp
    }

    #[test]
    fn every_catalog_refusal_is_refused_not_degraded() {
        let refusals = [
            FfiCatalogRefusal::ReservedModule {
                slug: "evil".to_owned(),
            },
            FfiCatalogRefusal::ReservedWrapperPrefix {
                slug: "evil".to_owned(),
                ident: "ipe_asserted_x".to_owned(),
            },
            FfiCatalogRefusal::Artifact(Box::new(ipe_ffi::diag::Diagnostic::ArtifactIo {
                path: "x.consumer.json".to_owned(),
                detail: "truncated".to_owned(),
            })),
        ];
        for refusal in refusals {
            let err = load_error(&CliError::FfiCatalogRefused(refusal));
            assert!(
                matches!(err, LoadError::FfiUntrusted(_)),
                "catalog refusal must type as FfiUntrusted: {err:?}"
            );
            assert_eq!(err.disposition(), LoadDisposition::Refuse);
        }
    }

    #[test]
    fn a_tampered_catalog_refuses_the_load() {
        let tmp = tmp_dir("tampered-catalog");
        let cache = tmp.join(".ipe/cache/ffi/rust");
        std::fs::create_dir_all(&cache).expect("mk cache");
        std::fs::write(cache.join("x.consumer.json"), "{ not json").expect("write consumer");
        let main = tmp.join("Main.ipe");
        let text = "module Main exposing (main)\n\nmain = 1\n";
        std::fs::write(&main, text).expect("write Main.ipe");
        let loaded = DriverLoader.load(None, &main, Some(text));
        let err = loaded.err();
        assert!(
            matches!(err, Some(LoadError::FfiUntrusted(_))),
            "a tampered catalog must be refused: {err:?}"
        );
        assert_eq!(
            err.as_ref().map(LoadError::disposition),
            Some(LoadDisposition::Refuse)
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

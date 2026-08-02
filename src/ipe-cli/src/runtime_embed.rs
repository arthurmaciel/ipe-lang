//! The runtime crate source, embedded in the `ipe` binary and materialized on
//! demand.
//!
//! A copied `ipe` binary must be able to build a program with nothing beside it
//! but a Rust toolchain — no separate runtime download, no version-mismatch
//! hazard. To make that true, the runtime crate SOURCE (`src/runtime/rust`: its
//! `Cargo.toml` + `src/**`) is compiled into this binary as a byte-tree and
//! written to `<IPE_HOME>/runtime/<version>/rust` the first time a build needs
//! it. Because the binary materializes its OWN embedded source, the runtime the
//! emitted project links against matches the compiler that emitted it by
//! construction — there is no cross-machine version to drift.
//!
//! Only SOURCE is embedded; it is compiled locally by the user's toolchain. No
//! prebuilt object code ships (that would be an ABI/trust surface, not a source
//! distribution).
//!
//! # `IPE_HOME`
//! The materialized runtime lives under `IPE_HOME`, resolved once:
//! 1. `$IPE_HOME` — explicit override.
//! 2. `$XDG_DATA_HOME/ipe` — the XDG data root, when set.
//! 3. `$HOME/.ipe` — the default.
//!
//! Nothing is ever written outside the resolved `IPE_HOME`.

use std::path::{Path, PathBuf};

use include_dir::{Dir, include_dir};

use crate::CliError;

/// The runtime crate source tree, embedded at build time from the workspace
/// `src/runtime/rust` directory (the crate ROOT, so the materialized tree is a
/// self-contained buildable crate). `tests/` is present in the source tree but
/// is not needed to build the crate as a path dependency; it is skipped on
/// materialize (see [`materialize`]) so the on-disk runtime is the minimal
/// buildable crate.
static RUNTIME_CRATE: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../runtime/rust");

/// The compiler's own version — the workspace-synced `version` field.
///
/// The embedded runtime crate is materialized WITH this version (see
/// [`concretize_manifest`]), so the on-disk runtime version equals the compiler
/// version by construction.
pub const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The runtime crate's package name.
///
/// This is the single token that identifies a directory as a valid runtime crate
/// root. A candidate whose `Cargo.toml` does not declare this package is rejected
/// — never walked past to a wrong runtime.
pub const RUNTIME_PACKAGE: &str = "ipe-runtime-rust";

/// A resolved, verified runtime crate root: the directory holding the runtime
/// `Cargo.toml` an emitted project names as its path dependency, paired with the
/// version that root declares.
///
/// The only way to obtain one is through [`resolve`], which constructs it solely
/// from a candidate that has been checked to hold a `Cargo.toml` declaring the
/// [`RUNTIME_PACKAGE`]. An empty, half-present, or wrong-package directory can
/// never be represented as a `ResolvedRuntime` — a wrong runtime is
/// unrepresentable, not merely unlikely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRuntime {
    root: PathBuf,
    version: String,
}

impl ResolvedRuntime {
    /// The verified, absolute crate root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The version the resolved crate's `Cargo.toml` declares.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }
}

/// The relative path, from a runtime crate root, of the manifest.
const MANIFEST: &str = "Cargo.toml";

/// The `version.workspace = true` line the embedded manifest carries (it inherits
/// the version from the workspace at build time). A standalone materialized crate
/// has no parent workspace, so this line is rewritten to a concrete version.
const WORKSPACE_VERSION_LINE: &str = "version.workspace = true";

/// Read a candidate crate root's manifest and return its declared version iff the
/// manifest declares the runtime package. `None` means "not a runtime crate
/// root" — the caller treats that as a hard refusal, never a walk-on.
fn runtime_version_at(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(MANIFEST)).ok()?;
    let declares_package = text
        .lines()
        .any(|l| l.trim() == format!("name = \"{RUNTIME_PACKAGE}\""));
    if !declares_package {
        return None;
    }
    // A materialized crate carries a concrete `version = "…"`; an in-repo crate
    // inherits it via `version.workspace = true`. In-repo, the version is the
    // compiler's own (both are the workspace version), so report that.
    let concrete = text.lines().find_map(|l| {
        let l = l.trim();
        l.strip_prefix("version = \"")
            .and_then(|rest| rest.strip_suffix('"'))
            .map(str::to_owned)
    });
    Some(concrete.unwrap_or_else(|| COMPILER_VERSION.to_owned()))
}

/// Verify that `root` is a runtime crate root and, if so, canonicalize it into a
/// [`ResolvedRuntime`]. A non-runtime directory yields `Ok(None)`; the caller
/// decides whether that is a hard error (an explicit override) or a fall-through
/// (an in-repo probe).
fn verify(root: &Path) -> Result<Option<ResolvedRuntime>, CliError> {
    let Some(version) = runtime_version_at(root) else {
        return Ok(None);
    };
    let canonical = root.canonicalize().map_err(|e| CliError::Io {
        path: root.to_path_buf(),
        source: e,
    })?;
    Ok(Some(ResolvedRuntime {
        root: canonical,
        version,
    }))
}

/// Resolve `IPE_HOME` (see the module doc). The returned path is not created;
/// [`materialize`] creates it under the runtime subdirectory as needed.
///
/// # Errors
/// [`CliError::RuntimeHomeUnknown`] when no override, no `XDG_DATA_HOME`, and no
/// `HOME` are set — there is no directory to materialize into.
pub fn ipe_home() -> Result<PathBuf, CliError> {
    if let Some(dir) = std::env::var_os("IPE_HOME") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let xdg = PathBuf::from(xdg);
        if xdg.is_absolute() {
            return Ok(xdg.join("ipe"));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".ipe"));
    }
    Err(CliError::RuntimeHomeUnknown)
}

/// Rewrite the embedded manifest's `version.workspace = true` into a concrete
/// `version = "<COMPILER_VERSION>"`, so the materialized crate builds standalone
/// (it has no parent workspace to inherit from) AND declares exactly the
/// compiler's version. Every other line is preserved verbatim.
fn concretize_manifest(manifest: &str) -> String {
    manifest
        .lines()
        .map(|line| {
            if line.trim() == WORKSPACE_VERSION_LINE {
                format!("version = \"{COMPILER_VERSION}\"")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + if manifest.ends_with('\n') { "\n" } else { "" }
}

/// Write the embedded crate tree under `dest` (a fresh directory), rewriting the
/// root manifest's version and skipping the `tests/` subtree. Fails closed on the
/// first filesystem error — a partially written tree under `dest` is a temp dir
/// the caller discards, never the live runtime.
fn write_tree(dir: &Dir<'_>, dest: &Path, at_root: bool) -> Result<(), CliError> {
    std::fs::create_dir_all(dest).map_err(|e| CliError::Io {
        path: dest.to_path_buf(),
        source: e,
    })?;
    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(sub) => {
                let name = sub.path().file_name().unwrap_or_default();
                // `tests/` is not part of the buildable dependency and carries a
                // nested proof crate; skip it so the on-disk runtime is minimal.
                if at_root && name == std::ffi::OsStr::new("tests") {
                    continue;
                }
                write_tree(sub, &dest.join(name), false)?;
            }
            include_dir::DirEntry::File(file) => {
                let name = file.path().file_name().unwrap_or_default();
                let target = dest.join(name);
                if at_root && name == std::ffi::OsStr::new(MANIFEST) {
                    let text = std::str::from_utf8(file.contents()).map_err(|_| {
                        CliError::RuntimeMaterializeFailed {
                            detail: "embedded runtime manifest is not valid UTF-8".to_owned(),
                        }
                    })?;
                    std::fs::write(&target, concretize_manifest(text)).map_err(|e| {
                        CliError::Io {
                            path: target,
                            source: e,
                        }
                    })?;
                } else {
                    std::fs::write(&target, file.contents()).map_err(|e| CliError::Io {
                        path: target,
                        source: e,
                    })?;
                }
            }
        }
    }
    Ok(())
}

/// Materialize the embedded runtime source to `<IPE_HOME>/runtime/<version>/rust`
/// and return the verified root.
///
/// Atomic: the whole tree is written to a sibling temp directory under the same
/// `runtime/` parent, then renamed into place — a build never sees a
/// half-written runtime. Idempotent: if `<version>/rust` already exists and
/// verifies as a runtime crate root, the write is skipped (the fast path). Every
/// path is under `IPE_HOME`; nothing is written elsewhere.
///
/// # Errors
/// - [`CliError::RuntimeHomeUnknown`] if no `IPE_HOME` can be resolved.
/// - [`CliError::Io`] / [`CliError::RuntimeMaterializeFailed`] on any filesystem
///   failure — a disk-full or permission error is a loud, fail-closed refusal,
///   never a wrong or empty runtime.
pub fn materialize() -> Result<ResolvedRuntime, CliError> {
    let home = ipe_home()?;
    let runtime_parent = home.join("runtime");
    let version_dir = runtime_parent.join(COMPILER_VERSION);
    let final_root = version_dir.join("rust");

    // Fast path: an already-materialized, valid runtime for this version.
    if let Some(resolved) = verify(&final_root)? {
        return Ok(resolved);
    }

    std::fs::create_dir_all(&runtime_parent).map_err(|e| CliError::Io {
        path: runtime_parent.clone(),
        source: e,
    })?;

    // Stage the whole `<version>` directory in a sibling temp, then rename it
    // into place: the observable `<version>/rust` appears atomically and fully
    // formed. The temp name carries the pid so concurrent `ipe` processes do not
    // collide on the staging directory.
    let staging = runtime_parent.join(format!(
        ".staging-{COMPILER_VERSION}-{}",
        std::process::id()
    ));
    // A leftover staging dir from a crashed prior run would poison the write; the
    // remove is best-effort and only ever touches our own staging path.
    let _ = std::fs::remove_dir_all(&staging);
    let staged_root = staging.join("rust");

    let write_result = write_tree(&RUNTIME_CRATE, &staged_root, true);
    if let Err(e) = write_result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    // Verify the staged tree BEFORE it becomes the live runtime — a staged tree
    // that does not verify is a bug in the embed, surfaced loudly here.
    let staged_ok = verify(&staged_root)?.is_some();
    if !staged_ok {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(CliError::RuntimeMaterializeFailed {
            detail: "the staged runtime tree did not verify as a runtime crate root".to_owned(),
        });
    }

    // Rename the staged `<version>` into place. If a concurrent process won the
    // race and created `<version>` first, treat an existing valid target as
    // success (idempotent) and discard our staging copy.
    if std::fs::rename(&staging, &version_dir).is_err() {
        let _ = std::fs::remove_dir_all(&staging);
        if verify(&final_root)?.is_none() {
            return Err(CliError::RuntimeMaterializeFailed {
                detail: format!("could not place the runtime at {}", version_dir.display()),
            });
        }
    }

    verify(&final_root)?.ok_or_else(|| CliError::RuntimeMaterializeFailed {
        detail: format!(
            "materialized runtime at {} did not verify",
            final_root.display()
        ),
    })
}

/// Resolve the runtime crate root for the dependency-model emit. The resolution
/// chain, in order:
///
/// 1. `$IPE_RUNTIME_DIR` — an explicit override (dev / advanced). It MUST name a
///    runtime crate root (`Cargo.toml` declaring [`RUNTIME_PACKAGE`]); anything
///    else is a loud error, including a targeted hint when it points at the inner
///    `src/ipe_runtime` module directory instead of the crate root.
/// 2. In-repo `src/runtime/rust` — an upward walk from the current directory, so
///    a repository checkout builds against the live source (dev workflow and
///    goldens unchanged).
/// 3. Otherwise materialize the embedded source to `<IPE_HOME>/runtime/<version>/rust`
///    and use that (the normal installed path).
///
/// # Errors
/// - [`CliError::RuntimeDirInvalid`] when `$IPE_RUNTIME_DIR` is set but is not a
///   runtime crate root.
/// - Any error from [`materialize`] when the embedded fallback is taken.
pub fn resolve() -> Result<ResolvedRuntime, CliError> {
    if let Some(dir) = std::env::var_os("IPE_RUNTIME_DIR") {
        return resolve_override(&PathBuf::from(dir));
    }

    if let Some(resolved) = resolve_in_repo()? {
        return Ok(resolved);
    }

    materialize()
}

/// Resolve the `$IPE_RUNTIME_DIR` override: verify it names a runtime crate root,
/// else a loud, typed refusal (with a targeted hint when it points at the inner
/// module directory).
fn resolve_override(path: &Path) -> Result<ResolvedRuntime, CliError> {
    if let Some(resolved) = verify(path)? {
        return Ok(resolved);
    }
    // A common mistake: pointing at the inner module directory
    // (`…/src/ipe_runtime` or `…/rust/src`) rather than the crate root — detect
    // it so the error can say exactly what to fix.
    let points_at_inner = path.file_name() == Some(std::ffi::OsStr::new("ipe_runtime"))
        || path.ends_with("rust/src")
        || path.ends_with("runtime/rust/src");
    Err(CliError::RuntimeDirInvalid {
        path: path.to_path_buf(),
        points_at_inner,
    })
}

/// Upward walk from the current directory for the in-repo `src/runtime/rust`
/// crate root. `Ok(None)` means no in-repo runtime is above the current
/// directory (the normal case for an installed binary run outside a checkout).
fn resolve_in_repo() -> Result<Option<ResolvedRuntime>, CliError> {
    let cwd = std::env::current_dir().map_err(|e| CliError::Io {
        path: PathBuf::from("."),
        source: e,
    })?;
    let mut here: Option<&Path> = Some(cwd.as_path());
    while let Some(dir) = here {
        let candidate = dir.join("src").join("runtime").join("rust");
        if let Some(resolved) = verify(&candidate)? {
            return Ok(Some(resolved));
        }
        here = dir.parent();
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded runtime crate's declared version equals the compiler
    /// version. This is the version guarantee: the binary materializes its own
    /// embedded source, and that source names the compiler's version, so an
    /// emitted project can never link a mismatched runtime (barring the explicit
    /// `IPE_RUNTIME_DIR` override).
    #[test]
    fn embedded_runtime_version_matches_compiler() {
        let manifest = RUNTIME_CRATE
            .get_file("Cargo.toml")
            .expect("embedded runtime crate must carry a Cargo.toml");
        let text = std::str::from_utf8(manifest.contents()).expect("embedded manifest is UTF-8");
        // The in-repo manifest inherits the version from the workspace; the
        // concretized form names it literally. Either way the effective version
        // is the compiler's.
        assert!(
            text.contains(WORKSPACE_VERSION_LINE),
            "embedded runtime manifest must inherit the workspace version"
        );
        let concretized = concretize_manifest(text);
        assert!(
            concretized.contains(&format!("version = \"{COMPILER_VERSION}\"")),
            "concretized runtime manifest must declare the compiler version {COMPILER_VERSION}"
        );
        assert!(
            !concretized.contains(WORKSPACE_VERSION_LINE),
            "concretized manifest must not keep the workspace-inherit line"
        );
    }

    /// The embedded tree declares the runtime package — a materialize would
    /// verify.
    #[test]
    fn embedded_runtime_declares_package() {
        let manifest = RUNTIME_CRATE
            .get_file("Cargo.toml")
            .expect("embedded runtime crate must carry a Cargo.toml");
        let text = std::str::from_utf8(manifest.contents()).unwrap();
        assert!(
            text.lines()
                .any(|l| l.trim() == format!("name = \"{RUNTIME_PACKAGE}\""))
        );
    }

    /// `concretize_manifest` touches only the version line and preserves the rest
    /// verbatim (including a trailing newline).
    #[test]
    fn concretize_preserves_other_lines() {
        let input = "[package]\nname = \"ipe-runtime-rust\"\nversion.workspace = true\nedition = \"2024\"\n";
        let out = concretize_manifest(input);
        assert_eq!(
            out,
            format!(
                "[package]\nname = \"ipe-runtime-rust\"\nversion = \"{COMPILER_VERSION}\"\nedition = \"2024\"\n"
            )
        );
    }
}

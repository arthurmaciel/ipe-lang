//! The `ipe add` / `ipe install` / `ipe remove` commands and the build-time
//! FFI seam: interface-module injection + backend emission-input assembly.
//!
//! `ipe add <crate>` runs the `ipe-ffi-inspector` inside the `ipe_sandbox`
//! bubblewrap jail (fetch posture: network on, everything else confined),
//! decodes the inspection, and writes the six cache artifacts under
//! `<project>/.ipe/cache/ffi/rust/`. At build time the driver loads that
//! cache, injects one `Rust.<Crate>` interface module per installed crate
//! (origin [`ipe_canon::ModuleOrigin::FfiInterface`] — unforgeable), and
//! hands the backend its [`ipe_backend_rust::FfiEmit`] inputs.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ipe_ffi::driver::{CrateName, FfiCache, InstalledCrate};

use crate::CliError;

/// The project-relative FFI cache directory.
const CACHE_REL: &str = ".ipe/cache/ffi/rust";

/// Walk up from `start` (a file or directory) looking for an existing FFI
/// artifact cache; the first hit is the project's cache root.
#[must_use]
pub fn find_cache_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        Some(start)
    } else {
        start.parent()
    };
    while let Some(d) = dir {
        let candidate = d.join(CACHE_REL);
        if candidate.is_dir() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

/// Load the installed-crate catalog for a build rooted at (or blamed on)
/// `blame_path`. Absent cache ⇒ empty catalog.
///
/// # Errors
/// [`CliError::Pipeline`] wrapping the catalog loader's diagnostic (a
/// tampered or half-written cache is refused, never silently skipped).
pub fn load_catalog_for(blame_path: &Path) -> Result<Vec<InstalledCrate>, CliError> {
    let Some(cache_root) = find_cache_root(blame_path) else {
        return Ok(Vec::new());
    };
    ipe_ffi::driver::load_catalog(&cache_root)
        .map_err(|diag| CliError::UsageOwned(diag.to_string()))
}

/// Inject each installed crate's interface module into the build's source
/// map, returning the set of injected module paths (they earn
/// [`ipe_canon::ModuleOrigin::FfiInterface`] at input creation).
///
/// # Errors
/// [`CliError::UsageOwned`] when a project module already claims an
/// installed crate's `Rust.*` module path.
pub fn inject_interfaces(
    sources: &mut BTreeMap<Vec<String>, (PathBuf, String)>,
    catalog: &[InstalledCrate],
    cache_root_hint: &Path,
) -> Result<BTreeSet<Vec<String>>, CliError> {
    let mut injected = BTreeSet::new();
    for c in catalog {
        let mod_path: Vec<String> = c.module_name.split('.').map(str::to_owned).collect();
        if sources.contains_key(&mod_path) {
            return Err(CliError::UsageOwned(format!(
                "module `{}` clashes with the installed FFI crate `{}` — the `Rust.*` \
                 namespace is reserved for FFI interface modules",
                c.module_name, c.slug
            )));
        }
        let pseudo_path = cache_root_hint.join(format!("{}.ipe", c.slug));
        sources.insert(mod_path.clone(), (pseudo_path, c.interface_source.clone()));
        injected.insert(mod_path);
    }
    Ok(injected)
}

/// Assemble the backend emission inputs from the catalog: the merged
/// module-qualified opaque-type map, the de-duplicated pinned dep lines, and
/// the combined `src/ffi.rs` (one `pub mod <slug>` per crate).
///
/// # Errors
/// [`CliError::UsageOwned`] when two installed crates pin the SAME
/// dependency name to different lines — an unbuildable `Cargo.toml` refused
/// here rather than discovered by `cargo`.
pub fn assemble_emit(
    catalog: &[InstalledCrate],
) -> Result<Option<ipe_backend_rust::FfiEmit>, CliError> {
    use std::fmt::Write as _;
    if catalog.is_empty() {
        return Ok(None);
    }
    let mut foreign_types: BTreeMap<String, String> = BTreeMap::new();
    let mut dep_by_name: BTreeMap<String, String> = BTreeMap::new();
    let mut bindings_source = String::from(
        "//! Foreign-crate FFI wrappers — one module per installed crate.\n\
         //! Generated from the project's `.ipe/cache/ffi/rust` artifacts.\n",
    );
    for c in catalog {
        for (name, path) in &c.opaque_types {
            foreign_types.insert(format!("{}.{name}", c.module_name), path.clone());
        }
        for line in &c.cargo_deps {
            let name = line.split('=').next().unwrap_or(line).trim().to_owned();
            match dep_by_name.get(&name) {
                Some(prev) if prev != line => {
                    return Err(CliError::UsageOwned(format!(
                        "installed FFI crates pin dependency `{name}` to conflicting \
                         lines:\n  {prev}\n  {line}\nre-add one of the crates so the \
                         pins agree"
                    )));
                }
                _ => {
                    dep_by_name.insert(name, line.clone());
                }
            }
        }
        // Writing into a String is infallible.
        let _ = write!(
            bindings_source,
            "\npub mod {slug} {{\n{body}}}\npub use {slug}::*;\n",
            slug = c.slug,
            body = c.bindings_source
        );
    }
    Ok(Some(ipe_backend_rust::FfiEmit {
        foreign_types,
        dep_lines: dep_by_name.into_values().collect(),
        bindings_source,
    }))
}

/// Locate the `ipe-ffi-inspector` binary: beside the running `ipe`
/// executable first, then `$PATH`.
fn inspector_binary() -> Result<PathBuf, CliError> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("ipe-ffi-inspector");
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("ipe-ffi-inspector");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(CliError::Usage(
        "ipe add: `ipe-ffi-inspector` not found beside the `ipe` binary or on PATH",
    ))
}

/// Run the inspector for `krate` inside the sandbox jail and return its
/// stdout (the inspection JSON).
fn run_inspector(
    krate: &CrateName,
    features: &[String],
    allow_build_scripts: bool,
) -> Result<String, CliError> {
    let inspector = inspector_binary()?;
    let argv = ipe_ffi::driver::inspector_argv(krate, features, None, allow_build_scripts);
    let mut payload: Vec<OsString> = vec![inspector.clone().into_os_string()];
    payload.extend(argv);

    let caps = ipe_sandbox::probe();
    let mechanism = ipe_sandbox::select_mechanism(&caps);
    let unsandboxed_ok = ipe_sandbox::unsandboxed_override_set();
    if !matches!(mechanism, ipe_sandbox::Mechanism::Bwrap(_)) && !unsandboxed_ok {
        return Err(CliError::Usage(
            "ipe add: no bubblewrap isolation available — install `bwrap`, or set \
             IPE_FFI_ALLOW_UNSANDBOXED=1 to accept running the crate's build scripts \
             UNSANDBOXED (dangerous)",
        ));
    }

    let io_err = |detail: String| CliError::UsageOwned(format!("ipe add: {detail}"));
    if matches!(mechanism, ipe_sandbox::Mechanism::Bwrap(_)) {
        // Fetch posture: network on (the inspector downloads + documents the
        // crate); everything else confined. The toolchain lives under the
        // invoking user's home, which the jail masks — re-bind it read-only.
        let scoped_tmp = std::env::temp_dir().join(format!(
            "ipe-ffi-add-{}-{}",
            krate.as_str(),
            std::process::id()
        ));
        std::fs::create_dir_all(&scoped_tmp).map_err(|e| io_err(e.to_string()))?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut toolchain_ro_binds = Vec::new();
        // The inspector binary itself may live under a masked mount
        // (`$HOME/...` target dirs) — re-bind its directory read-only.
        if let Some(dir) = inspector.parent() {
            toolchain_ro_binds.push(dir.to_path_buf());
        }
        let mut path_prepend = Vec::new();
        let mut rustup_home = None;
        if let Some(home) = home {
            let cargo_bin = home.join(".cargo/bin");
            if cargo_bin.is_dir() {
                path_prepend.push(cargo_bin);
                toolchain_ro_binds.push(home.join(".cargo"));
            }
            let rustup = home.join(".rustup");
            if rustup.is_dir() {
                toolchain_ro_binds.push(rustup.clone());
                rustup_home = Some(rustup);
            }
        }
        let spec = ipe_sandbox::JailSpec {
            network: ipe_sandbox::NetworkPolicy::FetchOnly,
            scoped_tmp: scoped_tmp.clone(),
            registry_cache: None,
            toolchain: None,
            toolchain_ro_binds,
            path_prepend,
            rustup_home,
            limits: ipe_sandbox::ResourceLimits::default(),
        };
        let out = ipe_sandbox::run_in_bwrap_jail(&caps, &spec, &payload)
            .map_err(|d| io_err(format!("sandboxed inspection failed: {d:?}")))?;
        let _ = std::fs::remove_dir_all(&scoped_tmp);
        if out.status != Some(0) {
            return Err(io_err(format!(
                "inspector exited with {:?}\n{}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        return String::from_utf8(out.stdout)
            .map_err(|_| io_err("inspector produced non-UTF-8 output".to_owned()));
    }

    // Explicit unsandboxed override.
    eprintln!("WARNING: running the FFI inspector UNSANDBOXED (IPE_FFI_ALLOW_UNSANDBOXED=1)");
    let (program, rest) = payload.split_first().ok_or(CliError::Usage("ipe add"))?;
    let out = std::process::Command::new(program)
        .args(rest)
        .output()
        .map_err(|e| io_err(e.to_string()))?;
    if !out.status.success() {
        return Err(io_err(format!(
            "inspector exited with {:?}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    String::from_utf8(out.stdout)
        .map_err(|_| io_err("inspector produced non-UTF-8 output".to_owned()))
}

/// Shared tail of `add` / `install`: inspect one crate + write its artifacts.
fn add_one(
    cache: &FfiCache,
    krate: &CrateName,
    features: &[String],
    allow_build_scripts: bool,
) -> Result<(), CliError> {
    let json = run_inspector(krate, features, allow_build_scripts)?;
    // A multi-crate inspector run emits a JSON array; `ipe add` runs one
    // crate, but tolerate the array wrapper by unwrapping a singleton.
    let doc_text = match serde_json::from_str::<serde_json::Value>(&json) {
        Ok(serde_json::Value::Array(items)) if items.len() == 1 => items
            .first()
            .map(serde_json::Value::to_string)
            .unwrap_or(json),
        _ => json,
    };
    let (pkg, paths) = ipe_ffi::driver::install_from_inspection(cache, &doc_text)
        .map_err(|diag| CliError::UsageOwned(diag.to_string()))?;
    let iface = ipe_ffi::interface::crate_interface(&pkg);
    println!(
        "added `{}` v{}: {} bindings ({} skipped) -> {}",
        pkg.name(),
        pkg.version(),
        iface.bindings.len(),
        iface.skipped.len(),
        paths.interface.display()
    );
    Ok(())
}

/// `ipe add <crate> [--features a,b] [--yes]`.
///
/// # Errors
/// [`CliError`] on misuse, a refused inspection, or a cache-write failure.
pub fn run_add(rest: &[String]) -> Result<(), CliError> {
    let mut krate: Option<String> = None;
    let mut features: Vec<String> = Vec::new();
    let mut assume_yes = false;
    let mut allow_build_scripts = false;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--features" => {
                let raw = it
                    .next()
                    .ok_or(CliError::Usage("ipe add: --features needs a value"))?;
                features.extend(raw.split(',').map(str::to_owned));
            }
            "--yes" => assume_yes = true,
            "--allow-build-scripts" => allow_build_scripts = true,
            other if krate.is_none() => krate = Some(other.to_owned()),
            _ => {
                return Err(CliError::Usage(
                    "usage: ipe add <crate> [--features a,b] [--yes]",
                ));
            }
        }
    }
    let raw = krate.ok_or(CliError::Usage(
        "usage: ipe add <crate> [--features a,b] [--yes]",
    ))?;
    let krate = CrateName::parse(&raw).map_err(|diag| CliError::UsageOwned(diag.to_string()))?;

    if !assume_yes {
        use std::io::Write as _;
        println!("{}", ipe_ffi::driver::trust_summary(&krate, "", None, 0));
        print!("[y/N] ");
        let _ = std::io::stdout().flush();
        if !crate::read_yes_no() {
            return Err(CliError::Usage("ipe add: aborted"));
        }
    }

    let cache = FfiCache::at_project_root(Path::new("."));
    add_one(&cache, &krate, &features, allow_build_scripts)
}

/// `ipe remove <crate>`.
///
/// # Errors
/// [`CliError`] on misuse or a cache-delete failure.
pub fn run_remove(rest: &[String]) -> Result<(), CliError> {
    let [raw] = rest else {
        return Err(CliError::Usage("usage: ipe remove <crate>"));
    };
    let cache = FfiCache::at_project_root(Path::new("."));
    let slug = ipe_ffi::driver::slugify(raw);
    cache
        .remove_package(&slug)
        .map_err(|diag| CliError::UsageOwned(diag.to_string()))?;
    println!("removed `{raw}`");
    Ok(())
}

/// `ipe install [--yes]` — (re)inspect every `[rust.dependencies]` crate in
/// the project's `sky.toml`.
///
/// # Errors
/// [`CliError`] on misuse, a missing manifest, or any per-crate failure.
pub fn run_install(rest: &[String]) -> Result<(), CliError> {
    let assume_yes = matches!(rest, [flag] if flag == "--yes") || rest.is_empty();
    if !assume_yes {
        return Err(CliError::Usage("usage: ipe install [--yes]"));
    }
    let manifest = Path::new("sky.toml");
    if !manifest.is_file() {
        return Err(CliError::Usage(
            "ipe install: no sky.toml in the current directory",
        ));
    }
    let text = std::fs::read_to_string(manifest)
        .map_err(|e| CliError::UsageOwned(format!("ipe install: {e}")))?;
    let deps = rust_dependencies_from_manifest(&text);
    if deps.is_empty() {
        println!("ipe install: no [rust.dependencies] entries");
        return Ok(());
    }
    let cache = FfiCache::at_project_root(Path::new("."));
    for (name, _version) in deps {
        let krate =
            CrateName::parse(&name).map_err(|diag| CliError::UsageOwned(diag.to_string()))?;
        add_one(&cache, &krate, &[], false)?;
    }
    Ok(())
}

/// Extract `name = "version"` pairs from the manifest's
/// `[rust.dependencies]` / `["rust.dependencies"]` table.
fn rust_dependencies_from_manifest(text: &str) -> Vec<(String, String)> {
    let mut in_table = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_table = line == "[rust.dependencies]" || line == "[\"rust.dependencies\"]";
            continue;
        }
        if !in_table || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let name = k.trim().trim_matches('"').to_owned();
            let version = v.trim().trim_matches('"').to_owned();
            out.push((name, version));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_rust_dependencies_table_parses_both_spellings() {
        let text = "[project]\nname = \"x\"\n\n[\"rust.dependencies\"]\nsemver = \"1\"\n\n[live]\nport = 1\n";
        assert_eq!(
            rust_dependencies_from_manifest(text),
            vec![("semver".to_owned(), "1".to_owned())]
        );
        let text2 = "[rust.dependencies]\nuuid = \"1.10\"\n";
        assert_eq!(
            rust_dependencies_from_manifest(text2),
            vec![("uuid".to_owned(), "1.10".to_owned())]
        );
    }

    #[test]
    fn conflicting_dep_pins_are_refused() {
        let mk = |slug: &str, line: &str| InstalledCrate {
            slug: slug.to_owned(),
            module_name: format!("Rust.{slug}"),
            kernel_name: format!("Rust_{slug}"),
            interface_source: String::new(),
            bindings_source: String::new(),
            opaque_types: BTreeMap::new(),
            cargo_deps: vec![line.to_owned()],
            wrapper_idents: BTreeSet::new(),
        };
        let ok = assemble_emit(&[mk("a", "serde = \"=1.0.1\""), mk("b", "serde = \"=1.0.1\"")]);
        assert!(ok.is_ok_and(|e| e.is_some_and(|e| e.dep_lines == vec!["serde = \"=1.0.1\""])));
        let clash = assemble_emit(&[mk("a", "serde = \"=1.0.1\""), mk("b", "serde = \"=1.0.2\"")]);
        assert!(clash.is_err());
    }
}

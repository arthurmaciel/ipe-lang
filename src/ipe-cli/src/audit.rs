//! `ipe package audit` — the SP4 universal Tier-1 package gate.
//!
//! The gate answers one question: is this package version safe and honest enough
//! for the curated index to serve it? It runs four checks over the working
//! package and is a hard **accept** or **reject with a diagnostic** — never a
//! warning that lets an unsafe version through. The author runs it locally as a
//! pre-flight; the index CI re-runs the SAME [`run_audit`] path as the
//! authoritative gate, so the two verdicts cannot diverge.
//!
//! The four Tier-1 checks (see
//! `docs/adr/0044-package-coordination-manifest-index-gate.md`), each wired to existing machinery:
//!
//! 1. **Provenance panic-scan** — author-supplied FFI wrapper Rust
//!    (`*_bindings.rs` in the project's FFI cache) is scanned with the SAME token
//!    scanner the repo's abrupt-failure hook runs ([`panic_scan`]); an authored
//!    abrupt-failure construct there is a user error the package is rejected for,
//!    because that Rust compiles unsandboxed into the shipped artifact. Our
//!    EMITTED Rust is NOT the author's concern (plan §1a routes emitted-Rust hits
//!    to our CI, not the author's) and is already gated by the compiler's own
//!    `tools/panic-scan` CI over the backend `src/` templates — the backend even
//!    emits one deliberate, guarded polyfill `panic!` into every project — so the
//!    author gate scans ONLY author Rust, keeping the provenance boundary exact
//!    by construction.
//! 2. **Capability consistency** — the inferred capability set (the call-graph
//!    union that backs `ipe capabilities`) must EQUAL the manifest's declared
//!    `[capabilities]`. A used-but-undeclared capability is a hidden effect; a
//!    declared-but-unused one is an over-broad, misleading claim. Either rejects.
//! 3. **Enforced semver** — `ipe diff` / [`crate::diff::check_semver_bump`]
//!    between this version's public API and the previous published version; an
//!    under-bump rejects. A first version (no predecessor) skips this check.
//! 4. **Supply chain** — `cargo-deny` over the emitted project's dependency
//!    graph, plus the resolver's content-hash re-assertion over any Ipê package
//!    dependencies (verify-before-trust, re-checked at publish).
//!
//! For a native-bearing package (one declaring the `native-ffi` axis or binding
//! a `[rust.dependencies]` crate) a fifth check, [`crate::audit_native::native_tier2`],
//! runs after the four Tier-1 checks: it builds and exercises the package's
//! native code inside a jail scoped to its declared capability set and
//! reconciles observed-vs-declared, fail-closed (ADR 0046). It genuinely
//! certifies only the wired-and-proven platforms (`linux-x64` under
//! bwrap+seccomp, `macos-arm64` under `sandbox-exec` Seatbelt, `freebsd-x64`
//! under `jail(8)`); other platforms remain a documented refuse-to-certify and
//! the surface never claims Tier-2 for them.
//!
//! Deferred (not this layer): Tier-2 on Windows. Its returning build jail landed,
//! but the audit's Tier-2 probe wrapper is a POSIX shell fixture driven through a
//! `/bin/sh` invocation prefix, and the Windows jail runs `payload[0]` directly
//! through `CreateProcessW` (no shell), so a Windows-native probe wrapper is
//! needed before the audit can certify there — a design change beyond a
//! `cfg`-gate promotion. Also deferred: run-time sandbox isolation hardening.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use ipe_ir::Capability;

use crate::CliError;
use crate::cli_args::OutputFormat;
use crate::project::{self, ProjectManifest};
use crate::scratch::ScratchDir;

/// The package-gate checks, in the fixed order [`run_audit`] runs them. Naming
/// the check that rejected lets the diagnostic say exactly which gate failed.
///
/// The first four are the universal Tier-1 checks; [`Self::NativeTier2`] is the
/// native-code capability-enforcement check, appended only for native-bearing
/// packages (ADR 0046). [`Self::NativeBindingRegen`] is the prerequisite step
/// that runs before Tier-1 for native-bearing packages: it regenerates the FFI
/// bindings from the pinned `[rust.dependencies]` inside the sandbox so the
/// gate never trusts committed or absent bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// 0 — sandboxed FFI binding regeneration for native packages: runs the
    /// same jailed generator as `ipe rust install` does against the pinned
    /// `[rust.dependencies]`, writing gate-owned bindings the Tier-1 checks
    /// then read. Absent or committed bindings are never trusted.
    ///
    /// Also fires when a manifest declares `[rust.wrapper]` bindings: wrapper
    /// bindings are author-asserted (a local path with no registry pin, rev, or
    /// content hash), so the gate has no independent source to regenerate from
    /// and rejects the package at admission.
    NativeBindingRegen,
    /// 1a — abrupt-failure token scan over author-supplied FFI wrapper Rust.
    Provenance,
    /// 1b — inferred vs declared capability set.
    Capability,
    /// 1c — enforced semver bump vs the previous published version.
    Semver,
    /// 1d — `cargo-deny` + content-hash integrity over the dependency graph.
    SupplyChain,
    /// Tier-2 — differential-confinement enforcement of a native package's
    /// declared capability set against what its built+exercised native code
    /// actually demands.
    NativeTier2,
}

impl Check {
    /// A short label for the check, shown in a passing line and a reject header.
    const fn label(self) -> &'static str {
        match self {
            Self::NativeBindingRegen => "native FFI binding regeneration",
            Self::Provenance => "provenance panic-scan",
            Self::Capability => "capability consistency",
            Self::Semver => "enforced semver",
            Self::SupplyChain => "supply chain",
            Self::NativeTier2 => "native Tier-2 capability enforcement",
        }
    }
}

/// A rejection from one check: the check that failed and a one-diagnostic
/// message naming exactly what is wrong and (where applicable) where.
///
/// A closed value — every reject the gate can emit is one of these, carrying its
/// own already-rendered message — so the CLI boundary need only print it and
/// exit non-zero. Making the reject a typed value rather than a bare string keeps
/// the check that failed inspectable by tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejection {
    /// Which Tier-1 check rejected the package.
    pub check: Check,
    /// The human-readable diagnostic: what is wrong, and where.
    pub message: String,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "package audit rejected by the {} check:\n{}",
            self.check.label(),
            self.message
        )
    }
}

/// The already-built package the four checks read from: its parsed manifest, its
/// `package.ipe` path, and the directory it was emitted into. Preparing these once
/// keeps each check a pure function of a ready package rather than re-deriving
/// paths and re-building.
#[derive(Debug)]
struct Prepared {
    /// The parsed manifest (name, version, declared capabilities, deps).
    manifest: ProjectManifest,
    /// The `package.ipe` path (the semver check's public-API extraction root is
    /// its parent; the build's blame path).
    manifest_path: PathBuf,
    /// The directory the package was emitted into (the `cargo-deny` target for
    /// the supply-chain check).
    emitted_dir: PathBuf,
}

/// The wrapper-owned Tier-2 admission probe fixture, embedded in the binary and
/// materialized to a runtime scratch path on use. Tier-2 copies it into the
/// jail's scratch and runs it as the exit-owning wrapper (ADR 0046).
///
/// The fixture SOURCE is embedded at build time (the tracked fixture files stay
/// the single source of truth); a shipped binary can find it with no source
/// checkout beside it. Nothing depends on a compile-time source path at runtime.
///
/// The wrapper is platform-native: a POSIX `/bin/sh` script on Linux/macOS/
/// FreeBSD (driven via a `/usr/bin/env … /bin/sh` invocation prefix), and a
/// PowerShell `.ps1` on Windows (the Windows jail runs `payload[0]` directly
/// through `CreateProcessW` with no shell, so `powershell.exe -File` is the
/// interpreter). Both implement the SAME wrapper-owned per-axis exit contract
/// the decoder reads. The platform-appropriate one is materialized with the
/// file name Tier-2's jail expects, so the extension it resolves by is preserved.
const TIER2_PROBE_POSIX: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/admission/untrusted-build.sh"
));
const TIER2_PROBE_WINDOWS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/admission/untrusted-build.ps1"
));

/// Materialize the platform-appropriate embedded Tier-2 probe fixture to a
/// per-process scratch file and return its path.
///
/// # Errors
/// [`CliError::Io`] when the scratch directory or the fixture file cannot be
/// written — a fail-closed refusal, never a run against a missing wrapper.
fn tier2_probe_fixture() -> Result<PathBuf, CliError> {
    let (name, bytes): (&str, &[u8]) = if cfg!(target_os = "windows") {
        ("untrusted-build.ps1", TIER2_PROBE_WINDOWS)
    } else {
        ("untrusted-build.sh", TIER2_PROBE_POSIX)
    };
    let scratch = ScratchDir::new("ipe-tier2-fixture").map_err(|e| CliError::Io {
        path: PathBuf::from("ipe-tier2-fixture"),
        source: e,
    })?;
    let path = scratch.child(name);
    std::fs::write(&path, bytes).map_err(|e| CliError::Io {
        path: path.clone(),
        source: e,
    })?;
    // Caller cleans up via `remove_dir_all` after use; Drop is skipped here.
    std::mem::forget(scratch);
    Ok(path)
}

/// `ipe package audit [<path>]` — run the full Tier-1 gate on the working
/// package and exit non-zero with the failing check's diagnostic.
///
/// `<path>` is a project directory or a `package.ipe` (defaults to the current
/// directory). The package MUST be a project (have a `package.ipe`): the gate
/// checks a publishable package, and the manifest carries the declared
/// capabilities, version, and dependency graph every check reads.
///
/// # Errors
/// [`CliError::Usage`] / [`CliError::UsageOwned`] on argument misuse or a
/// package with no manifest; [`CliError::Pipeline`] / [`CliError::Io`] when the
/// package cannot be built or read; [`CliError::PackageAudit`] when a Tier-1
/// check rejects the package (the gate's hard reject).
pub fn run_audit(rest: &[String]) -> Result<(), CliError> {
    let (path, index_root, format) = parse_audit_args(rest)?;
    let prepared = prepare(&path)?;
    let name = prepared.manifest.name.clone();
    let version = prepared
        .manifest
        .version
        .as_ref()
        .map_or_else(|| "(unversioned)".to_owned(), ToString::to_string);

    let outcome = audit_gate(&prepared, index_root.as_deref());

    match format {
        OutputFormat::Json => emit_audit_json(&name, &version, &outcome),
        // `--plain` and the default share the human renderer; the audit verdict
        // has no separate flush-left line form, so `--plain` is the human report
        // (the format parse already rejected `--plain --json` together).
        OutputFormat::Human | OutputFormat::Plain => match outcome {
            Ok(tier2) => {
                print!(
                    "{}",
                    crate::style::frame(&crate::style::gutter(&passing_summary(
                        &name, &version, &tier2
                    )))
                );
                Ok(())
            }
            Err(err) => Err(err),
        },
    }
}

/// Run the full Tier-1 gate, then Tier-2 for native-bearing packages, returning
/// the first rejection or the Tier-2 outcome on a clean pass.
///
/// The checks run Security-first: the provenance scan (an authored abrupt-failure
/// construct in author Rust is a soundness hole in the SHIPPED artifact) and the
/// capability honesty check run before the semver and supply-chain checks; the
/// FIRST rejection is the verdict. A pure Ipê package skips Tier-2 (Tier-1 already
/// gated it exactly); a native package builds and exercises its native code under
/// a declared-scoped jail and reconciles observed-vs-declared, fail-closed
/// (ADR 0046).
fn audit_gate(
    prepared: &Prepared,
    index_root: Option<&Path>,
) -> Result<crate::audit_native::Tier2Outcome, CliError> {
    provenance_panic_scan(prepared)?;
    capability_consistency(prepared)?;
    enforced_semver(prepared, index_root)?;
    supply_chain(prepared)?;

    crate::audit_native::native_tier2(&crate::audit_native::NativeAudit {
        declared: &prepared.manifest.capabilities,
        has_rust_deps: !prepared.manifest.rust_dependencies.is_empty(),
        root: &prepared.manifest.root,
        emitted_dir: &prepared.emitted_dir,
        probe_fixture: tier2_probe_fixture()?,
    })
}

/// Emit the compact JSON audit verdict to stdout, then map a rejection to the
/// already-emitted sentinel so the process still exits non-zero without printing
/// a second human message. On a pass, the object records the certified verdict
/// and the Tier-2 disposition; on a reject, `certified` is `false` and `reason`
/// carries the failing check's one-line message.
fn emit_audit_json(
    name: &str,
    version: &str,
    outcome: &Result<crate::audit_native::Tier2Outcome, CliError>,
) -> Result<(), CliError> {
    println!("{}", audit_verdict_json(name, version, outcome));

    match outcome {
        Ok(_) => Ok(()),
        // The verdict object was already written to stdout; return the sentinel
        // so the exit is non-zero with nothing more printed.
        Err(_) => Err(CliError::DiagnosticJsonEmitted),
    }
}

/// Build the compact JSON audit verdict object (the pure core of
/// [`emit_audit_json`]): a certified pass with its Tier-2 disposition, or a
/// `certified:false` object carrying the failing check's one-line reason.
fn audit_verdict_json(
    name: &str,
    version: &str,
    outcome: &Result<crate::audit_native::Tier2Outcome, CliError>,
) -> String {
    use crate::audit_native::Tier2Outcome;
    use crate::cli_args::json;

    match outcome {
        Ok(Tier2Outcome::SkippedPureIpe) => json::object(&[
            ("package", json::string(name)),
            ("version", json::string(version)),
            ("tier1", json::string("pass")),
            ("tier2", json::string("skipped")),
            ("certified", "true".to_owned()),
        ]),
        Ok(Tier2Outcome::Certified { platform }) => json::object(&[
            ("package", json::string(name)),
            ("version", json::string(version)),
            ("tier1", json::string("pass")),
            ("tier2", json::string("pass")),
            ("platform", json::string(platform)),
            ("certified", "true".to_owned()),
        ]),
        Err(err) => json::object(&[
            ("package", json::string(name)),
            ("version", json::string(version)),
            ("certified", "false".to_owned()),
            ("reason", json::string(&err.to_string())),
        ]),
    }
}

/// Compose the passing summary, advertising Tier-2 ONLY for what genuinely ran
/// (the honest surface, ADR 0046). A pure Ipê package's summary is Tier-1 only,
/// with the standing note that Tier-2 does not apply. A native package certified
/// on a wired platform (`linux-x64`, `macos-arm64`, or `freebsd-x64`) names that
/// platform and states that a Tier-2 certification is per-host — vouching only
/// for the platform whose jail actually ran, never claimed cross-host.
fn passing_summary(name: &str, version: &str, tier2: &crate::audit_native::Tier2Outcome) -> String {
    use crate::audit_native::Tier2Outcome;
    match tier2 {
        Tier2Outcome::SkippedPureIpe => format!(
            "package audit: {name} {version} — all Tier-1 checks passed. (Pure Ipê package: \
             native Tier-2 does not apply.)"
        ),
        Tier2Outcome::Certified { platform } => format!(
            "package audit: {name} {version} — all Tier-1 checks passed; native Tier-2 capability \
             enforcement (build+link reachability of the package's FFI bindings under a \
             declared-scoped jail) passed on: {platform}. A Tier-2 certification is per-host — it \
             vouches only for the platform whose jail actually ran; running the audit on another \
             wired platform certifies that platform in turn."
        ),
    }
}

/// Parse `ipe package audit`'s tail: an optional positional `<path>`, an
/// optional `--index <dir>` (the curated index checkout the semver check reads
/// the previous published version from; defaults to the resolver's index root),
/// and the shared `--plain` / `--json` output-format flags.
///
/// # Errors
/// [`CliError::UsageOwned`] on an unknown flag, a missing `--index` value, a
/// second positional, or `--plain --json` together.
fn parse_audit_args(rest: &[String]) -> Result<(PathBuf, Option<PathBuf>, OutputFormat), CliError> {
    let mut path: Option<PathBuf> = None;
    let mut index: Option<PathBuf> = None;
    let mut format: Option<OutputFormat> = None;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--index" => {
                let value = it
                    .next()
                    .ok_or(CliError::Usage("ipe package audit: --index needs a value"))?;
                if index.is_some() {
                    return Err(CliError::Usage(
                        "ipe package audit: --index given more than once",
                    ));
                }
                index = Some(PathBuf::from(value));
            }
            "--plain" => set_format(&mut format, OutputFormat::Plain)?,
            "--json" => set_format(&mut format, OutputFormat::Json)?,
            flag if flag.starts_with('-') => {
                return Err(crate::cli_args::usage_unknown_flag("package audit", flag));
            }
            positional => {
                if path.is_some() {
                    return Err(CliError::Usage(
                        "ipe package audit: expected a single <path> argument",
                    ));
                }
                path = Some(PathBuf::from(positional));
            }
        }
    }
    Ok((
        path.unwrap_or_else(|| PathBuf::from(".")),
        index,
        format.unwrap_or_default(),
    ))
}

/// Fold a requested output format into `slot`, rejecting `--plain --json`
/// together (or a repeat) so a machine consumer never gets a silently-chosen
/// winner — the same mutual-exclusion the shared format parse enforces.
fn set_format(slot: &mut Option<OutputFormat>, requested: OutputFormat) -> Result<(), CliError> {
    match slot {
        None => {
            *slot = Some(requested);
            Ok(())
        }
        Some(existing) if *existing == requested => Err(CliError::Usage(
            "ipe package audit: an output-format flag was given more than once",
        )),
        Some(_) => Err(CliError::Usage(
            "ipe package audit: --plain and --json are mutually exclusive",
        )),
    }
}

/// Locate the package's `package.ipe`, parse the manifest, and build the package to
/// its emitted Rust in a fresh temp directory (never the project's own `out/`, so
/// the audit leaves no artifact behind and cannot race a concurrent build).
///
/// For native-bearing packages (those with `[rust.dependencies]`), the FFI
/// bindings are regenerated from the pinned crates before the build. Any
/// committed `.ipe/cache/ffi/rust` in the fetched tree is removed first so the
/// audit never reads publisher-supplied bindings — only the gate-owned,
/// freshly-generated ones pass the ownership check that follows.
///
/// A `Package.wrapper`-only package is rejected before the build: wrapper
/// bindings are author-asserted (a local source path, no registry pin, rev, or
/// hash), so the gate has no independent pinned source to regenerate from. The
/// only fail-closed option is rejection — committing author-written wrapper
/// `_bindings.rs` that the gate cannot re-derive must never reach a certified
/// build.
///
/// # Errors
/// [`CliError::UsageOwned`] when `path` names no `package.ipe`;
/// [`CliError::PackageAudit`] with [`Check::NativeBindingRegen`] when the
/// manifest declares a `Package.wrapper` stage; the build errors
/// ([`CliError::Pipeline`] / [`CliError::Io`] / [`CliError::StaticRefusal`])
/// otherwise.
fn prepare(path: &Path) -> Result<Prepared, CliError> {
    let manifest_path = locate_manifest(path)?;
    let manifest = project::parse_manifest(&manifest_path)?;

    // Reject wrapper-only packages: the gate cannot regenerate wrapper bindings
    // from an independent pinned source, so a committed `_bindings.rs` must
    // never be trusted. Fail closed here rather than reading author-supplied
    // wrapper Rust that was never gate-owned.
    if manifest.has_rust_wrapper {
        return Err(reject(
            Check::NativeBindingRegen,
            "this package declares a `Package.wrapper` stage whose bindings are \
             author-asserted (a local source path with no registry pin, rev, or \
             content hash). The audit gate has no independent pinned source to \
             regenerate wrapper bindings from, so it cannot vouch for them. \
             A wrapper-bearing package cannot be certified until the gate gains a \
             regenerable, pinned wrapper source."
                .to_owned(),
        ));
    }

    // Regenerate FFI bindings for native-bearing packages before the build so
    // the compiler finds `Rust.<Crate>` interface modules.
    if !manifest.rust_dependencies.is_empty() {
        regenerate_ffi_bindings(&manifest_path)?;
    }

    // `audit_scratch_dir` creates the directory exclusively with 128-bit OS
    // entropy — no stale-dir removal needed; a fresh exclusive dir is always empty.
    let emitted_dir = audit_scratch_dir(&manifest.name)?;
    // Resolve the runtime exactly as `ipe build` does — one resolver for every
    // command. Under the default dependency model the emitted project names the
    // runtime as a path dependency, which the build materializes from the
    // embedded source under `IPE_HOME`; no vendored module tree is needed, so an
    // empty sentinel is passed and `build_project` never reads it. Only the
    // vendored/wasm shape needs a concrete module tree. Resolving here through a
    // separate walk-up (that never materialized) is what made `audit` fail on a
    // clean machine while `build` succeeded.
    let runtime_dir = crate::resolve_vendored_runtime_dir(None, !crate::runtime_dep_from_env())?;
    crate::build_project(&manifest_path, &emitted_dir, &runtime_dir)?;

    Ok(Prepared {
        manifest,
        manifest_path,
        emitted_dir,
    })
}

/// Regenerate FFI bindings for a native-bearing package into the project's own
/// `.ipe/cache/ffi/rust`, so the gate audits bindings it derived from the pinned
/// crate rather than any the publisher may have committed.
///
/// Any committed cache in the fetched source tree is removed first — the gate
/// never reads publisher-supplied bindings. The freshly generated cache is
/// owned by the invoking process's uid and not world-writable, so the
/// `ffi::find_cache_root` ownership check passes for all subsequent reads.
///
/// Build scripts are always enabled here (equivalent to `--allow-build-scripts`)
/// because the bwrap jail (network-denied) is the sole confinement boundary;
/// skipping build scripts would silently omit crates that require them to
/// generate their API surface.
///
/// Failure is fail-closed: a regeneration error maps to a typed
/// [`Check::NativeBindingRegen`] rejection rather than letting the audit
/// continue with an absent or incomplete cache.
///
/// # Errors
/// [`CliError::PackageAudit`] with [`Check::NativeBindingRegen`] when the
/// jailed regeneration fails or the cache path escapes the project root via a
/// symlinked component; [`CliError::Io`] when the existing committed cache
/// cannot be removed.
fn regenerate_ffi_bindings(manifest_path: &Path) -> Result<(), CliError> {
    let project_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));

    // Resolve the cache path once and assert it stays inside the project root.
    // An attacker-authored package tree can ship `.ipe/cache/ffi` as a symlink
    // to an out-of-tree target (committed via `git add -f`). `remove_dir_all`
    // traverses intermediate symlinks and would delete the target; the
    // subsequent write would go through the surviving symlink. We walk every
    // path component between `project_root` and the intended cache dir with
    // `symlink_metadata` (no-follow) and reject on the first symlink found,
    // before any delete or write occurs.
    let safe_cache = ffi_cache_path_or_reject(project_root)?;

    if safe_cache.exists() {
        std::fs::remove_dir_all(&safe_cache).map_err(|e| CliError::Io {
            path: safe_cache.clone(),
            source: e,
        })?;
    }
    crate::ffi::install_registry_deps_for_project(manifest_path, true).map_err(|e| {
        reject(
            Check::NativeBindingRegen,
            format!(
                "failed to regenerate FFI bindings from the package's \
                 `[rust.dependencies]` inside the sandbox: {e}\n\
                 The audit requires a clean, gate-owned binding generation; \
                 a failure here means the native package cannot be certified."
            ),
        )
    })
}

/// Resolve the `.ipe/cache/ffi/rust` path under `project_root` and verify that
/// no component of the relative suffix is a symlink (no-follow check).
///
/// Returns the resolved path on success, or a [`Check::NativeBindingRegen`]
/// rejection when any component is a symlink or the resolved path escapes the
/// canonical project root. The check uses [`std::fs::symlink_metadata`] so it
/// never follows symlinks.
fn ffi_cache_path_or_reject(project_root: &Path) -> Result<PathBuf, CliError> {
    // The relative components we walk: `.ipe`, `cache`, `ffi`, `rust`.
    const CACHE_COMPONENTS: &[&str] = &[".ipe", "cache", "ffi", "rust"];

    let mut current = project_root.to_path_buf();
    for component in CACHE_COMPONENTS {
        current.push(component);
        // Check this component without following the symlink.
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(reject(
                    Check::NativeBindingRegen,
                    format!(
                        "the package's cache path `{}` contains a symlink at `{}`; \
                         the audit rejects this to prevent out-of-tree writes through \
                         a symlinked intermediate path component",
                        project_root.join(".ipe/cache/ffi/rust").display(),
                        current.display(),
                    ),
                ));
            }
            // Not present yet — the delete is a no-op and the write will create it.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            // Exists and is a real directory or file — continue walking.
            Ok(_) => {}
            Err(e) => {
                return Err(CliError::Io {
                    path: current,
                    source: e,
                });
            }
        }
    }
    Ok(current)
}

/// Resolve `path` (a directory or a `package.ipe`) to its manifest file.
///
/// # Errors
/// [`CliError::UsageOwned`] when the directory holds no `package.ipe`, or `path`
/// is neither a directory nor a `package.ipe`.
fn locate_manifest(path: &Path) -> Result<PathBuf, CliError> {
    if path.is_dir() {
        if let Some(manifest) = crate::project::manifest_in_dir(path) {
            return Ok(manifest);
        }
        if crate::project::migration_pending(path) {
            return Err(CliError::Usage(crate::project::MIGRATE_CONFIG_HINT));
        }
        return Err(CliError::UsageOwned(format!(
            "ipe package audit: no `package.ipe` in `{}` — the gate audits a publishable Ipê \
             package, which needs a manifest",
            path.display()
        )));
    }
    if path.file_name().and_then(|n| n.to_str()) == Some(crate::package_manifest::PACKAGE_IPE)
        && path.is_file()
    {
        return Ok(path.to_path_buf());
    }
    Err(CliError::UsageOwned(format!(
        "ipe package audit: `{}` is neither an Ipê project directory nor a package.ipe",
        path.display()
    )))
}

/// Create and return an exclusive per-package audit scratch directory under the
/// OS temp root. The name is unpredictable (128-bit OS entropy) so a same-user
/// attacker cannot pre-seed or symlink it.
fn audit_scratch_dir(package: &str) -> Result<PathBuf, CliError> {
    let prefix = format!("ipe-audit-{package}");
    let scratch = ScratchDir::new(&prefix).map_err(|e| CliError::Io {
        path: PathBuf::from(&prefix),
        source: e,
    })?;
    let path = scratch.path().to_path_buf();
    // The directory is cleaned up by the caller's best-effort `remove_dir_all`;
    // we transfer ownership of the path and skip Drop here.
    std::mem::forget(scratch);
    Ok(path)
}

// ===========================================================================
// 1a. Provenance panic-scan
// ===========================================================================

/// Scan the package's Rust for authored abrupt-failure constructs, attributing
/// each hit to its provenance.
///
/// - a hit in author-supplied FFI wrapper Rust (`_bindings.rs` in the project's
///   FFI cache) is a **user error**: the gate rejects the package, pointing at
///   the file and line. This is the security boundary the check exists to close —
///   author Rust compiles unsandboxed into the shipped artifact, so an authored
///   `panic!`/`unwrap` there is a soundness hole the package must not ship with.
/// - a hit in our EMITTED Rust is attributed to the COMPILER, not the author. Per
///   the plan (§1a) an emitted-Rust hit is OUR CI's concern, never the author's,
///   so the author-facing package gate does not scan it here: the emitted surface
///   is already covered by the compiler's own `tools/panic-scan` CI over the
///   backend's `src/` templates (`.github/workflows/panic-scan.yml`). That
///   separation is not incidental — the backend's FIXED epilogue emits one
///   deliberate, `#[allow(unreachable_code)]`-guarded polyfill `panic!` into
///   every project's `main.rs`, so scanning emitted output as an author gate
///   would reject every package for a construct that is neither the author's nor
///   accidental codegen. The provenance boundary is therefore exact by
///   construction: the gate rejects ONLY the author-supplied FFI wrapper Rust.
///
/// # Errors
/// [`CliError::PackageAudit`] when an author FFI Rust file contains an abrupt-
/// failure construct; [`CliError::Io`] on a read failure.
fn provenance_panic_scan(prepared: &Prepared) -> Result<(), CliError> {
    // Author FFI wrapper Rust is the one surface this check gates: it compiles
    // unsandboxed into the shipped artifact, so an authored abrupt-failure
    // construct there is a soundness hole the package must not ship with.
    if let Some(hit) = scan_author_ffi_rust(prepared)? {
        return Err(reject(
            Check::Provenance,
            format!(
                "author-supplied FFI Rust contains an abrupt-failure construct — a package \
                 that can `{}` at runtime is not safe to publish.\n  {}:{}: `{}`\n\
                 replace it with a `Result`/error return; the gate forbids authored \
                 panic/unwrap/expect/assert in shipped Rust.",
                hit.tok,
                hit.file.display(),
                hit.line,
                hit.tok
            ),
        ));
    }
    Ok(())
}

/// One flagged construct with its provenance file — a [`panic_scan::Hit`]
/// (line + token) paired with the file it was found in.
struct LocatedHit {
    file: PathBuf,
    line: usize,
    tok: String,
}

/// Scan the project's author-supplied FFI wrapper Rust (`*_bindings.rs` under the
/// FFI cache) for the first abrupt-failure construct, if any. Returns `None` when
/// the package carries no FFI cache or no author Rust hit.
///
/// This is the exact author-Rust surface the `_bindings.rs` naming marks: the FFI
/// cache stores one `<slug>_bindings.rs` per installed crate, the hand-written
/// wrapper the inspection produced from the author's `[rust.define.*]` decls.
/// The interface `.ipe` modules (origin [`ipe_canon::ModuleOrigin::FfiInterface`])
/// are Ipê, not Rust; the `_bindings.rs` files are the author Rust the scan
/// attributes to the user.
///
/// # Errors
/// [`CliError::Io`] on a read failure.
fn scan_author_ffi_rust(prepared: &Prepared) -> Result<Option<LocatedHit>, CliError> {
    let cache_root = prepared.manifest.root.join(".ipe/cache/ffi/rust");
    if !cache_root.is_dir() {
        return Ok(None);
    }
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rust_files(&cache_root, &mut files)?;
    files.sort();
    for file in files {
        // Only the `_bindings.rs` wrapper is author-authored Rust that compiles
        // into the crate; the other cache artifacts (`.ipei`, `consumer.json`,
        // `<slug>.ipe`) are interface metadata, not Rust.
        let is_bindings = file
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("_bindings.rs"));
        if !is_bindings {
            continue;
        }
        if let Some(hit) = first_hit(&file)? {
            return Ok(Some(hit));
        }
    }
    Ok(None)
}

/// Run the shared [`panic_scan`] token scanner over one file, returning its first
/// hit (lowest line) if any.
///
/// Fail closed on a non-lexing file: a `_bindings.rs` the scanner cannot
/// tokenise is opaque — the no-panic audit cannot attest its content. The
/// scanner refuses rather than admits (the same posture `capability_scan`
/// takes for a non-lexing source). A `cargo build` of a non-lexing file would
/// fail, but that is a separate later step; this gate must refuse eagerly,
/// before the file reaches `cargo`.
///
/// # Errors
/// [`CliError::Io`] on a file-read failure; [`CliError::PackageAudit`] when
/// the file does not lex as Rust tokens.
fn first_hit(file: &Path) -> Result<Option<LocatedHit>, CliError> {
    let src =
        crate::io_bounded::read_to_string_capped(file, crate::io_bounded::FFI_CACHE_READ_CAP)?;
    let hits = panic_scan::scan_str(&src).map_err(|_| {
        reject(
            Check::Provenance,
            format!(
                "emitted `{}` does not lex as Rust tokens — the no-panic audit cannot attest \
                 its content; the file is refused rather than admitted",
                file.display()
            ),
        )
    })?;
    Ok(hits.into_iter().next().map(|h| LocatedHit {
        file: file.to_path_buf(),
        line: h.line,
        tok: h.tok,
    }))
}

/// Recursively collect every `.rs` file under `dir` into `out`.
///
/// # Errors
/// [`CliError::Io`] on a directory-read failure.
fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), CliError> {
    let entries = std::fs::read_dir(dir).map_err(|e| CliError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| CliError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| CliError::Io {
            path: path.clone(),
            source: e,
        })?;
        if file_type.is_dir() {
            collect_rust_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    Ok(())
}

// ===========================================================================
// 1b. Capability consistency
// ===========================================================================

/// Verify the manifest's declared `[capabilities]` set EQUALS the set inferred
/// over the WHOLE package — no hidden effect, no over-broad claim.
///
/// Uses [`crate::infer_package_capabilities`] (the union over every shipped
/// module, not just `Main`'s reachability closure) so a sibling module a consumer
/// could `import` cannot smuggle in an undeclared effect. This is the same
/// whole-tree posture the enforced-semver check takes over the public API — the
/// declared set the index records is the consumer's consent surface, so it must
/// cover the whole shipped module set. `native-ffi` is inferred like any other
/// axis (it enters the set when a module crosses into `Rust.` code) and, when
/// present and consistent, is surfaced loudly per §1b.
///
/// # Errors
/// [`CliError::PackageAudit`] when the declared and inferred sets differ;
/// [`CliError::Pipeline`] / [`CliError::Io`] when the package cannot be lowered
/// at all.
fn capability_consistency(prepared: &Prepared) -> Result<(), CliError> {
    use std::fmt::Write as _;

    let declared: BTreeSet<Capability> = prepared.manifest.capabilities.clone();
    let inferred = crate::infer_package_capabilities(&prepared.manifest_path)?;

    if declared == inferred {
        if declared.contains(&Capability::NativeFfi) {
            // Surfaced loudly per §1b: a package the user consents to as crossing
            // into opaque native code, whose true effect set cannot be inferred
            // from Ipê alone beyond the `native-ffi` marker itself.
            print!(
                "{}",
                crate::style::frame(&crate::style::gutter(&format!(
                    "package audit: note — `{}` exercises the `native-ffi` capability; its \
                     native effects cannot be inferred from Ipê alone.",
                    prepared.manifest.name
                )))
            );
        }
        return Ok(());
    }

    let mut message = String::from(
        "the declared `[capabilities]` set does not match the package's inferred effects \
         — the declared set must be exactly the truth the user consents to.",
    );
    let missing: Vec<&'static str> = inferred.difference(&declared).map(|c| c.as_str()).collect();
    let extra: Vec<&'static str> = declared.difference(&inferred).map(|c| c.as_str()).collect();
    if !missing.is_empty() {
        let _ = write!(
            message,
            "\n  used but NOT declared (a hidden effect): {}",
            missing.join(", ")
        );
    }
    if !extra.is_empty() {
        let _ = write!(
            message,
            "\n  declared but NOT used (an over-broad claim): {}",
            extra.join(", ")
        );
    }
    Err(reject(Check::Capability, message))
}

// ===========================================================================
// 1c. Enforced semver
// ===========================================================================

/// Enforce the semver bump between this version's public API and the previous
/// published version fetched from the index.
///
/// Looks up the package in the index; the highest published version strictly
/// below the manifest's declared version is the predecessor. When the package is
/// not in the index, or has no version below this one, this is a FIRST version —
/// the check has no predecessor to diff against and skips (per §1c). Otherwise it
/// runs [`crate::diff::check_semver_bump`] and rejects an under-bump.
///
/// The predecessor's public API is rebuilt from its pinned source (fetched +
/// hash-verified through the resolver), so the baseline is exactly the bytes the
/// index registered — the plan's §7 "rebuild from pinned source" resolution of
/// the baseline-availability open question.
///
/// # Errors
/// [`CliError::PackageAudit`] on an under-bump or a missing manifest version;
/// [`CliError::Diff`] when a tree cannot be diffed; resolution errors otherwise.
fn enforced_semver(prepared: &Prepared, index_root: Option<&Path>) -> Result<(), CliError> {
    let Some(new_version) = prepared.manifest.version.clone() else {
        return Err(reject(
            Check::Semver,
            "the manifest declares no `version = \"…\"` — the enforced-semver check needs a \
             version to compare against the previous published one."
                .to_owned(),
        ));
    };

    let index_root = index_root.map_or_else(crate::resolve::index_root, Path::to_path_buf);
    // Absent ⇒ a first submission; no predecessor to enforce.
    // Unreadable ⇒ fail closed: a corrupt predecessor must not silently pass
    // as "first version" — propagate the error so the gate refuses.
    let Some(entry) =
        crate::index::read_entry_lookup(&index_root, &prepared.manifest.name).absent_or_err()?
    else {
        print!(
            "{}",
            crate::style::frame(&crate::style::gutter(&format!(
                "package audit: `{}` has no previously published version in the index — \
                 skipping the enforced-semver check (first version).",
                prepared.manifest.name
            )))
        );
        return Ok(());
    };

    // The predecessor is the highest published version strictly BELOW this one.
    let Some(previous) = entry
        .versions
        .iter()
        .filter(|v| v.version < new_version)
        .max_by(|a, b| a.version.cmp(&b.version))
    else {
        print!(
            "{}",
            crate::style::frame(&crate::style::gutter(&format!(
                "package audit: `{}` has no published version below {new_version} — \
                 skipping the enforced-semver check (first version).",
                prepared.manifest.name
            )))
        );
        return Ok(());
    };

    // Fetch + hash-verify the predecessor's pinned source, then diff the two
    // public APIs. `fetch_and_verify_baseline` returns the checkout root the
    // predecessor's `src/` lives under.
    let baseline = crate::resolve::fetch_and_verify_index_version(
        &prepared.manifest.root,
        &prepared.manifest.name,
        previous,
    )?;

    let new_tree = prepared
        .manifest_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let report =
        crate::diff::check_semver_bump(&baseline, &new_tree, &previous.version, &new_version)?;
    if report.satisfied {
        Ok(())
    } else {
        Err(reject(
            Check::Semver,
            format!(
                "version {new_version} does not clear the required {} bump over the previous \
                 published {} — the new version must be at least {}.",
                report.required.as_str(),
                previous.version,
                report.floor
            ),
        ))
    }
}

// ===========================================================================
// 1d. Supply chain
// ===========================================================================

/// Run `cargo-deny` over the emitted project's dependency graph, and re-assert
/// the content-hash integrity of any Ipê package dependencies against their index
/// pins.
///
/// `cargo-deny check` applies the workspace's supply-chain posture (advisories,
/// bans, licenses, sources — see `deny.toml`) to the emitted Cargo project; a
/// non-zero exit is a reject. The Ipê-package hash re-assertion reuses the
/// resolver's lockfile pins so a fetched dependency whose bytes drifted from the
/// registered hash is caught here too (the resolver verifies at install; the gate
/// re-verifies at publish).
///
/// When `cargo-deny` is not installed, the advisory/bans scan is skipped with a
/// loud warning (a missing dev tool is not an unsafe package), while the
/// hash-integrity half still runs. The authoritative index-CI gate always
/// installs cargo-deny, so enforcement is never actually skipped there.
///
/// # Errors
/// [`CliError::PackageAudit`] when `cargo-deny` reports a violation, fails to run
/// for any reason other than not being installed, or a locked dependency's hash
/// no longer verifies.
fn supply_chain(prepared: &Prepared) -> Result<(), CliError> {
    let manifest = prepared.emitted_dir.join("Cargo.toml");
    if !manifest.is_file() {
        // No emitted Cargo project means no Rust dependency graph to vet; the
        // Ipê-package integrity re-check below still applies.
        return verify_locked_dependency_hashes(prepared);
    }

    // Spawn `cargo-deny` directly rather than the `cargo deny` subcommand, so
    // that a machine without cargo-deny yields a `NotFound` spawn error (handled
    // as a skip below) instead of `cargo` running and reporting "no such
    // subcommand", which would masquerade as a supply-chain violation.
    let mut command = Command::new("cargo-deny");
    command
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("check")
        // Advisories + bans + sources are the supply-chain axes; licenses are a
        // project-policy axis the workspace's own gate owns, not the package gate.
        .arg("advisories")
        .arg("bans")
        .arg("sources");
    // Apply the SAME advisory/bans/sources posture the workspace uses (plan
    // §1d) — its `deny.toml` ledgers the advisories the vendored runtime's
    // dependency tree legitimately carries (e.g. the `rsa` timing advisory the
    // runtime pins behind an optional feature). Without it the check would
    // default-reject every emitted package for a runtime dependency the
    // workspace has already vetted. `--config` is a `check` argument, so it
    // follows the subcommand. Absent a resolvable config, cargo-deny falls back
    // to its defaults.
    let derived_config = derive_deny_config(&prepared.emitted_dir)?;
    if let Some(config) = &derived_config {
        command.arg("--config").arg(config);
    }
    let output = command.output();

    match output {
        Ok(out) if out.status.success() => verify_locked_dependency_hashes(prepared),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            Err(reject(
                Check::SupplyChain,
                format!(
                    "cargo-deny reported a supply-chain violation over the package's Rust \
                     dependency graph:\n{}",
                    stderr.trim()
                ),
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // cargo-deny is not installed. This is a missing dev tool, not an
            // unsafe package: conflating the two would fail every audit run on a
            // machine without cargo-deny. The authoritative gate — the package
            // index CI — always installs it, so advisory/bans enforcement is
            // never actually skipped there. Locally, skip that scan with a loud
            // warning; the lockfile hash-integrity half still runs.
            eprintln!(
                "{}",
                crate::style::gutter(
                    "warning: supply-chain advisory scan skipped — cargo-deny is not installed \
                     (`cargo install cargo-deny`). The package index enforces it; lockfile hash \
                     integrity is still verified."
                )
            );
            verify_locked_dependency_hashes(prepared)
        }
        Err(e) => Err(reject(
            Check::SupplyChain,
            format!(
                "could not run `cargo deny` ({e}) — install cargo-deny \
                 (`cargo install cargo-deny`) so the gate can vet the dependency graph."
            ),
        )),
    }
}

/// Re-assert that every locked Ipê package dependency's cached source still
/// hashes to the pin recorded in `ipe.lock` — the resolver's verify-before-trust
/// boundary, re-checked at publish.
///
/// # Errors
/// [`CliError::PackageAudit`] when a locked dependency's cached tree no longer
/// matches its pinned hash.
fn verify_locked_dependency_hashes(prepared: &Prepared) -> Result<(), CliError> {
    match crate::resolve::verify_lockfile_hashes(&prepared.manifest.root) {
        Ok(()) => Ok(()),
        Err(CliError::HashMismatch {
            package,
            expected,
            actual,
        }) => Err(reject(
            Check::SupplyChain,
            format!(
                "the cached source of the locked Ipê dependency `{package}` no longer matches \
                 its pinned hash — the dependency tree drifted from what the index registered.\n\
                 \x20 expected: {expected}\n  actual:   {actual}"
            ),
        )),
        Err(other) => Err(other),
    }
}

/// Locate the workspace's `deny.toml` so the supply-chain check applies the same
/// posture the workspace CI does. Walks up from the current directory, then from
/// the resolved runtime tree's ancestry (the runtime lives inside the workspace,
/// so `deny.toml` sits at the workspace root above it). Returns `None` when no
/// `deny.toml` is found.
fn locate_workspace_deny_config() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    if let Ok(runtime) = crate::resolve_runtime() {
        roots.push(runtime);
    }
    for root in roots {
        let mut here: Option<&Path> = Some(root.as_path());
        while let Some(dir) = here {
            let candidate = dir.join("deny.toml");
            if candidate.is_file() {
                return Some(candidate);
            }
            here = dir.parent();
        }
    }
    None
}

/// Derive a cargo-deny config for the EMITTED project from the workspace's
/// `deny.toml`, dropping its `[graph]` section.
///
/// The workspace config's `[graph] features = ["full"]` names the RUNTIME crate's
/// own feature set, which the emitted `ipe-app` does not have — passing the
/// workspace config verbatim makes `cargo metadata` fail on the unknown feature.
/// The advisory/license/bans/sources POLICY is exactly what the gate must apply,
/// so this copies every section EXCEPT `[graph]` into a derived config written
/// beside the emitted project, and returns its path. Returns `None` when no
/// workspace `deny.toml` is found (cargo-deny then uses its defaults).
///
/// # Errors
/// [`CliError::Io`] on a read/write failure.
fn derive_deny_config(emitted_dir: &Path) -> Result<Option<PathBuf>, CliError> {
    let Some(source) = locate_workspace_deny_config() else {
        return Ok(None);
    };
    let text =
        crate::io_bounded::read_to_string_capped(&source, crate::io_bounded::SMALL_FILE_READ_CAP)?;

    // Line-filter out the `[graph]` table (up to the next top-level `[section]`).
    // The remaining tables (`[advisories]`, `[licenses]`, `[bans]`, `[sources]`)
    // are the emitted-project-independent policy the gate applies.
    let mut out = String::with_capacity(text.len());
    let mut in_graph = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            in_graph = trimmed.starts_with("[graph]");
        }
        if !in_graph {
            out.push_str(line);
            out.push('\n');
        }
    }

    let derived = emitted_dir.join("ipe-audit-deny.toml");
    std::fs::write(&derived, out).map_err(|e| CliError::Io {
        path: derived.clone(),
        source: e,
    })?;
    Ok(Some(derived))
}

/// Build a [`CliError::PackageAudit`] for `check` carrying `message`.
const fn reject(check: Check, message: String) -> CliError {
    CliError::PackageAudit(Rejection { check, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn audit_parses_output_format_flags() {
        let (_, _, fmt) = parse_audit_args(&args(&["--json"])).expect("json");
        assert_eq!(fmt, OutputFormat::Json);
        let (_, _, fmt) = parse_audit_args(&args(&["--plain"])).expect("plain");
        assert_eq!(fmt, OutputFormat::Plain);
        let (_, _, fmt) = parse_audit_args(&args(&[])).expect("default");
        assert_eq!(fmt, OutputFormat::Human);
    }

    #[test]
    fn audit_rejects_plain_and_json_together() {
        assert!(parse_audit_args(&args(&["--plain", "--json"])).is_err());
        assert!(parse_audit_args(&args(&["--json", "--json"])).is_err());
    }

    #[test]
    fn audit_verdict_json_is_compact_pass_and_fail() {
        use crate::audit_native::Tier2Outcome;

        let pass = audit_verdict_json("http", "1.2.0", &Ok(Tier2Outcome::SkippedPureIpe));
        assert_eq!(
            pass,
            "{\"package\":\"http\",\"version\":\"1.2.0\",\"tier1\":\"pass\",\"tier2\":\"skipped\",\"certified\":true}"
        );

        let fail = audit_verdict_json(
            "http",
            "1.2.0",
            &Err(reject(
                Check::Capability,
                "used but not declared".to_owned(),
            )),
        );
        assert!(fail.contains("\"certified\":false"), "fail verdict: {fail}");
        assert!(fail.contains("\"reason\":"), "carries a reason: {fail}");
        // Byte-uniform compact: no space after a comma.
        assert!(!fail.contains(", "), "compact: {fail}");
    }

    /// The Tier-2 probe fixture the gate runs is materialized from the embedded
    /// bytes, NOT read from a compile-time source path — so a shipped binary with
    /// no source checkout beside it still finds the wrapper. The returned path
    /// exists on disk (a runtime scratch file, never the `CARGO_MANIFEST_DIR`
    /// source tree) and its bytes equal the embedded copy exactly.
    #[test]
    fn tier2_probe_fixture_materializes_from_the_embedded_copy() {
        let path = tier2_probe_fixture().expect("materialize the embedded probe fixture");
        assert!(
            path.is_file(),
            "the materialized probe fixture must exist on disk at {}",
            path.display()
        );
        assert!(
            !path.starts_with(env!("CARGO_MANIFEST_DIR")),
            "the materialized fixture must live under a runtime scratch path, not the source tree"
        );
        let expected: &[u8] = if cfg!(target_os = "windows") {
            TIER2_PROBE_WINDOWS
        } else {
            TIER2_PROBE_POSIX
        };
        let on_disk = std::fs::read(&path).expect("read the materialized probe fixture");
        assert_eq!(
            on_disk, expected,
            "the materialized fixture bytes must equal the embedded copy"
        );
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("the fixture path has a file name");
        let expected_name = if cfg!(target_os = "windows") {
            "untrusted-build.ps1"
        } else {
            "untrusted-build.sh"
        };
        assert_eq!(
            name, expected_name,
            "the fixture keeps the platform-native file name the jail resolves by"
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("the fixture has a parent dir"));
    }

    /// The embedded probe fixtures are the byte-exact contents of the tracked
    /// fixture files: the tracked files are the single source of truth, embedded
    /// (not duplicated inline). If a fixture is edited, this equality asserts the
    /// binary carries the new bytes — no hand-sync, no drift.
    #[test]
    fn embedded_probe_fixtures_match_the_tracked_files() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/admission");
        let posix = std::fs::read(base.join("untrusted-build.sh"))
            .expect("read the tracked POSIX probe fixture");
        assert_eq!(
            posix, TIER2_PROBE_POSIX,
            "the embedded POSIX fixture must equal the tracked file"
        );
        let windows = std::fs::read(base.join("untrusted-build.ps1"))
            .expect("read the tracked Windows probe fixture");
        assert_eq!(
            windows, TIER2_PROBE_WINDOWS,
            "the embedded Windows fixture must equal the tracked file"
        );
    }

    #[test]
    fn tier2_probe_fixture_dir_is_created_exclusively() {
        // `tier2_probe_fixture` creates its scratch dir through `ScratchDir`, so
        // the name is unpredictable — `ipe-tier2-fixture-<pid>-<32 hex entropy>` —
        // and the dir is created exclusively (a pre-seeded entry is not followed).
        // A predictable pid-only name would let a same-user attacker pre-seed the
        // path; the entropy component is what closes that.
        let fixture_path =
            tier2_probe_fixture().expect("tier2_probe_fixture must succeed on a clean temp dir");
        let dir = fixture_path.parent().expect("fixture path has parent");
        let dir_name = dir.file_name().unwrap_or_default().to_string_lossy();
        let suffix = dir_name
            .strip_prefix("ipe-tier2-fixture-")
            .expect("scratch dir uses the ipe-tier2-fixture- prefix");
        // The suffix is `<pid>-<32 hex entropy>`: the pid, a dash, then 32 hex
        // chars. The entropy makes the name unpredictable (not the bare pid).
        let (pid_part, hex_part) = suffix
            .split_once('-')
            .expect("suffix is <pid>-<hex entropy>");
        assert!(
            pid_part.chars().all(|c| c.is_ascii_digit()),
            "pid component is decimal: {pid_part}"
        );
        assert_eq!(
            pid_part,
            std::process::id().to_string(),
            "pid in dir name matches the current process"
        );
        assert_eq!(hex_part.len(), 32, "128 bits of entropy as 32 hex chars");
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "entropy component is hex: {hex_part}"
        );
        assert!(
            fixture_path.exists(),
            "fixture file was written at {fixture_path:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Build a unique throwaway directory under the OS temp root for a test.
    /// Returns the path; the caller must remove it when done.
    fn make_test_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ipe-audit-test-{tag}-{}-{}",
            std::process::id(),
            // A per-call counter keeps multiple calls in the same test from
            // colliding.  A static is fine here — tests run in the same process.
            {
                use std::sync::atomic::{AtomicU64, Ordering};
                static N: AtomicU64 = AtomicU64::new(0);
                N.fetch_add(1, Ordering::Relaxed)
            }
        ));
        std::fs::create_dir_all(&dir).expect("create test scratch dir");
        dir
    }

    /// A manifest that carries a `[rust.wrapper]` section is rejected at the
    /// `prepare` step with a [`Check::NativeBindingRegen`] rejection.
    ///
    /// Wrapper bindings are author-asserted (local path, no registry pin, rev, or
    /// hash); the gate has no independent source to regenerate from, so it must
    /// refuse rather than read committed author-written `_bindings.rs`.
    #[test]
    fn prepare_rejects_rust_wrapper_at_admission() {
        use std::io::Write as _;

        let dir = make_test_dir("wrapper-reject");
        let src = dir.join("src");
        std::fs::create_dir(&src).expect("create src/");
        // A minimal main module so the project looks structurally valid.
        std::fs::write(src.join("Main.ipe"), "module Main exposing (..)\n")
            .expect("write Main.ipe");

        let manifest_path = dir.join("package.ipe");
        let mut f = std::fs::File::create(&manifest_path).expect("create package.ipe");
        writeln!(
            f,
            "module Package exposing (package)\n\n\npackage =\n    Package.named \"wrapper-pkg\"\n\
             \x20       |> Package.version \"0.1.0\"\n\
             \x20       |> Package.wrapper (Rust.wrapper \"./my-crate\" |> Rust.expose [ \"some_fn\" ])\n"
        )
        .expect("write package.ipe");

        let err = prepare(&dir).expect_err("prepare must reject a wrapper-only package");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            matches!(err, CliError::PackageAudit(_)),
            "expected PackageAudit, got: {err:?}"
        );
        if let CliError::PackageAudit(ref r) = err {
            assert_eq!(
                r.check,
                Check::NativeBindingRegen,
                "wrong check: must be NativeBindingRegen, got {:?}",
                r.check
            );
            assert!(
                r.message.contains("Package.wrapper"),
                "rejection message must name the Package.wrapper stage: {}",
                r.message
            );
        }
    }

    /// A manifest with only `[rust.dependencies]` (no `[rust.wrapper]`) must NOT
    /// be rejected by the wrapper admission guard — the `[rust.dependencies]` path
    /// still proceeds to the regeneration step unchanged.
    ///
    /// This test exercises only the admission predicate through manifest parsing
    /// (not the full `prepare` which requires a sandboxed inspector), confirming
    /// that the new wrapper guard does not perturb the existing
    /// `[rust.dependencies]` path.
    #[test]
    fn prepare_does_not_reject_rust_dependencies_only_manifest() {
        use std::io::Write as _;

        let dir = make_test_dir("dep-only");
        let src = dir.join("src");
        std::fs::create_dir(&src).expect("create src/");
        std::fs::write(src.join("Main.ipe"), "module Main exposing (..)\n")
            .expect("write Main.ipe");

        let manifest_path = dir.join("package.ipe");
        let mut f = std::fs::File::create(&manifest_path).expect("create package.ipe");
        writeln!(
            f,
            "module Package exposing (package)\n\n\npackage =\n    Package.named \"dep-pkg\"\n\
             \x20       |> Package.version \"0.1.0\"\n\
             \x20       |> Package.rustDependencies [ Package.rustDep \"uuid\" \"1\" ]\n\
             \x20       |> Package.declares [ Capability.nativeFfi ]\n"
        )
        .expect("write package.ipe");

        let manifest = crate::project::parse_manifest(&manifest_path)
            .expect("manifest with rust dependencies must parse");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            !manifest.has_rust_wrapper,
            "has_rust_wrapper must be false for a rust-dependencies-only manifest"
        );
    }

    /// A manifest with no FFI sections (pure Ipê) is not affected by the wrapper
    /// guard and must parse with `has_rust_wrapper = false`.
    #[test]
    fn pure_ipe_manifest_has_rust_wrapper_is_false() {
        use std::io::Write as _;

        let dir = make_test_dir("pure-ipe");
        let src = dir.join("src");
        std::fs::create_dir(&src).expect("create src/");
        std::fs::write(src.join("Main.ipe"), "module Main exposing (..)\n")
            .expect("write Main.ipe");

        let manifest_path = dir.join("package.ipe");
        let mut f = std::fs::File::create(&manifest_path).expect("create package.ipe");
        writeln!(
            f,
            "module Package exposing (package)\n\n\npackage =\n    Package.named \"pure-pkg\"\n\
             \x20       |> Package.version \"0.1.0\"\n"
        )
        .expect("write package.ipe");

        let manifest =
            crate::project::parse_manifest(&manifest_path).expect("pure manifest must parse");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            !manifest.has_rust_wrapper,
            "has_rust_wrapper must be false for a pure-Ipê manifest"
        );
    }

    /// `ffi_cache_path_or_reject` refuses a cache path whose `.ipe` component
    /// is a symlink, preventing a delete-through-symlink attack — the same
    /// containment that guards `[rust.dependencies]` regeneration.
    #[test]
    #[cfg(unix)]
    fn ffi_cache_path_or_reject_refuses_symlinked_cache_component() {
        use std::os::unix::fs::symlink;

        let dir = make_test_dir("symlink-reject");
        let target = make_test_dir("symlink-target");

        // Plant `.ipe` as a symlink pointing outside the project root.
        let ipe_link = dir.join(".ipe");
        symlink(&target, &ipe_link).expect("create .ipe symlink");

        let err =
            ffi_cache_path_or_reject(&dir).expect_err("must reject a symlinked .ipe component");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&target);
        assert!(
            matches!(err, CliError::PackageAudit(_)),
            "expected PackageAudit, got: {err:?}"
        );
        if let CliError::PackageAudit(ref r) = err {
            assert_eq!(
                r.check,
                Check::NativeBindingRegen,
                "wrong check on symlink reject: {:?}",
                r.check
            );
            assert!(
                r.message.contains("symlink"),
                "rejection message must mention symlink: {}",
                r.message
            );
        }
    }

    // -----------------------------------------------------------------------
    // first_hit — non-lexing source must refuse (instance #3 regression)
    // -----------------------------------------------------------------------

    #[test]
    fn first_hit_non_lexing_source_is_refused_not_admitted() {
        // A `_bindings.rs` that does not lex as Rust tokens must produce Err,
        // not Ok(None). Returning Ok(None) would silently pass the no-panic
        // gate for a file whose content cannot be attested.
        let dir = make_test_dir("first-hit-nonlex");
        let file = dir.join("x_bindings.rs");
        // Unterminated raw string — proc-macro2 cannot tokenise this.
        std::fs::write(&file, r#"fn f() { let x = r##"unterminated"#)
            .expect("write non-lexing fixture");
        let result = super::first_hit(&file);
        assert!(
            result.is_err(),
            "a non-lexing _bindings.rs must return Err (refuse), not Ok(None)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_hit_clean_lexing_source_is_ok_none() {
        // A well-formed, panic-free file must still return Ok(None).
        let dir = make_test_dir("first-hit-clean");
        let file = dir.join("x_bindings.rs");
        std::fs::write(&file, "pub fn add(a: i64, b: i64) -> i64 { a + b }")
            .expect("write clean fixture");
        let result = super::first_hit(&file);
        assert!(result.is_ok(), "a clean file must return Ok(_)");
        assert!(
            result.unwrap().is_none(),
            "a panic-free file must return Ok(None)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_hit_file_with_panic_is_ok_some() {
        // A file with a panic! must produce Ok(Some(hit)), unchanged.
        let dir = make_test_dir("first-hit-panic");
        let file = dir.join("x_bindings.rs");
        std::fs::write(&file, "pub fn f() { panic!(\"oops\"); }").expect("write panic fixture");
        let result = super::first_hit(&file);
        assert!(result.is_ok(), "a lexing file must return Ok(_)");
        assert!(
            result.unwrap().is_some(),
            "a panic-bearing file must return Ok(Some(_))"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `prepare` resolves the runtime through the SAME path `ipe build` uses — the
    /// materialize-capable resolver — never a separate walk-up that cannot
    /// materialize. Under the default dependency model no vendored module tree is
    /// needed, so with `IPE_RUNTIME_DIR` unset the resolution succeeds with an
    /// empty sentinel (the emitted project names the runtime as a path dependency
    /// the build materializes). This is the property whose absence made `audit`
    /// fail on a clean machine where `build` succeeded; a regression that re-splits
    /// the path (calling `resolve_runtime`, which walks up for an in-repo tree and
    /// errors when none is found) fails here.
    #[test]
    fn audit_runtime_resolution_needs_no_in_repo_tree_under_the_dep_model() {
        // The dependency model is the default. Only the vendored/wasm shape needs a
        // concrete module tree; the default does not, so resolution yields the
        // empty sentinel with no `IPE_RUNTIME_DIR` and no in-repo walk-up.
        let needs_vendored = !crate::runtime_dep_from_env();
        assert!(
            !needs_vendored,
            "the default dependency model must not require a vendored runtime tree"
        );
        let resolved = crate::resolve_vendored_runtime_dir(None, needs_vendored)
            .expect("the dep-model runtime resolution must not require an in-repo tree");
        assert_eq!(
            resolved,
            PathBuf::new(),
            "the dep-model path returns the empty sentinel; the build materializes the runtime"
        );
    }
}

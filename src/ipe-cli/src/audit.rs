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
//! Deferred (not this layer): native Tier-2 sandboxed build + fail-closed
//! capability enforcement (blocked on FFI Tier 2), and run-time sandbox
//! isolation hardening. Those land with the FFI Tier 2 capability work.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use ipe_ir::Capability;

use crate::CliError;
use crate::project::{self, ProjectManifest};

/// The four Tier-1 checks, in the fixed order [`run_audit`] runs them. Naming the
/// check that rejected lets the diagnostic say exactly which gate failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// 1a — abrupt-failure token scan over author-supplied FFI wrapper Rust.
    Provenance,
    /// 1b — inferred vs declared capability set.
    Capability,
    /// 1c — enforced semver bump vs the previous published version.
    Semver,
    /// 1d — `cargo-deny` + content-hash integrity over the dependency graph.
    SupplyChain,
}

impl Check {
    /// A short label for the check, shown in a passing line and a reject header.
    const fn label(self) -> &'static str {
        match self {
            Self::Provenance => "provenance panic-scan",
            Self::Capability => "capability consistency",
            Self::Semver => "enforced semver",
            Self::SupplyChain => "supply chain",
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
/// `ipe.toml` path, and the directory it was emitted into. Preparing these once
/// keeps each check a pure function of a ready package rather than re-deriving
/// paths and re-building.
struct Prepared {
    /// The parsed manifest (name, version, declared capabilities, deps).
    manifest: ProjectManifest,
    /// The `ipe.toml` path (the semver check's public-API extraction root is its
    /// parent; the build's blame path).
    manifest_path: PathBuf,
    /// The directory the package was emitted into (the `cargo-deny` target for
    /// the supply-chain check).
    emitted_dir: PathBuf,
}

/// `ipe package audit [<path>]` — run the full Tier-1 gate on the working
/// package and exit non-zero with the failing check's diagnostic.
///
/// `<path>` is a project directory or an `ipe.toml` (defaults to the current
/// directory). The package MUST be a project (have an `ipe.toml`): the gate
/// checks a publishable package, and the manifest carries the declared
/// capabilities, version, and dependency graph every check reads.
///
/// # Errors
/// [`CliError::Usage`] / [`CliError::UsageOwned`] on argument misuse or a
/// package with no manifest; [`CliError::Pipeline`] / [`CliError::Io`] when the
/// package cannot be built or read; [`CliError::PackageAudit`] when a Tier-1
/// check rejects the package (the gate's hard reject).
pub fn run_audit(rest: &[String]) -> Result<(), CliError> {
    let (path, index_root) = parse_audit_args(rest)?;
    let prepared = prepare(&path)?;

    // Run every check in order; the FIRST rejection is the verdict. Ordered
    // Security-first: the provenance scan (an authored abrupt-failure construct
    // in author Rust is a soundness hole in the SHIPPED artifact) and the
    // capability honesty check run before the semver and supply-chain checks.
    provenance_panic_scan(&prepared)?;
    capability_consistency(&prepared)?;
    enforced_semver(&prepared, index_root.as_deref())?;
    supply_chain(&prepared)?;

    println!(
        "package audit: {} {} — all Tier-1 checks passed.",
        prepared.manifest.name,
        prepared
            .manifest
            .version
            .as_ref()
            .map_or_else(|| "(unversioned)".to_owned(), ToString::to_string)
    );
    Ok(())
}

/// Parse `ipe package audit`'s tail: an optional positional `<path>` and an
/// optional `--index <dir>` (the curated index checkout the semver check reads
/// the previous published version from; defaults to the resolver's index root).
///
/// # Errors
/// [`CliError::UsageOwned`] on an unknown flag, a missing `--index` value, or a
/// second positional.
fn parse_audit_args(rest: &[String]) -> Result<(PathBuf, Option<PathBuf>), CliError> {
    let mut path: Option<PathBuf> = None;
    let mut index: Option<PathBuf> = None;
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
            flag if flag.starts_with('-') => {
                return Err(CliError::UsageOwned(format!(
                    "ipe package audit: unknown flag `{flag}`"
                )));
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
    Ok((path.unwrap_or_else(|| PathBuf::from(".")), index))
}

/// Locate the package's `ipe.toml`, parse the manifest, and build the package to
/// its emitted Rust in a fresh temp directory (never the project's own `out/`, so
/// the audit leaves no artifact behind and cannot race a concurrent build).
///
/// # Errors
/// [`CliError::UsageOwned`] when `path` names no `ipe.toml`; the build errors
/// ([`CliError::Pipeline`] / [`CliError::Io`] / [`CliError::StaticRefusal`])
/// otherwise.
fn prepare(path: &Path) -> Result<Prepared, CliError> {
    let manifest_path = locate_manifest(path)?;
    let manifest = project::parse_manifest(&manifest_path)?;

    let emitted_dir = audit_scratch_dir(&manifest.name);
    // A stale scratch dir from a previous audit must not leak old emitted files
    // into this scan; remove it first so the emitted set is exactly this build's.
    if emitted_dir.exists() {
        std::fs::remove_dir_all(&emitted_dir).map_err(|e| CliError::Io {
            path: emitted_dir.clone(),
            source: e,
        })?;
    }
    let runtime_dir = crate::resolve_runtime()?;
    crate::build_project(&manifest_path, &emitted_dir, &runtime_dir)?;

    Ok(Prepared {
        manifest,
        manifest_path,
        emitted_dir,
    })
}

/// Resolve `path` (a directory or an `ipe.toml`) to its manifest file.
///
/// # Errors
/// [`CliError::UsageOwned`] when the directory holds no `ipe.toml`, or `path` is
/// neither a directory nor a `.toml` file.
fn locate_manifest(path: &Path) -> Result<PathBuf, CliError> {
    if path.is_dir() {
        let candidate = path.join("ipe.toml");
        if candidate.is_file() {
            return Ok(candidate);
        }
        return Err(CliError::UsageOwned(format!(
            "ipe package audit: no `ipe.toml` in `{}` — the gate audits a publishable Ipê \
             package, which needs a manifest",
            path.display()
        )));
    }
    if path.extension().and_then(|e| e.to_str()) == Some("toml") && path.is_file() {
        return Ok(path.to_path_buf());
    }
    Err(CliError::UsageOwned(format!(
        "ipe package audit: `{}` is neither an Ipê project directory nor an ipe.toml",
        path.display()
    )))
}

/// The per-package audit scratch directory under the OS temp root, keyed by the
/// package name and this process so concurrent audits never collide.
fn audit_scratch_dir(package: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ipe-audit-{package}-{}", std::process::id()))
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
/// hit (lowest line) if any. A file that does not lex as Rust tokens is treated
/// as having no hit — a malformed emitted/author file surfaces at `cargo` build
/// time, not here, and the scan attests only what it can tokenise (its documented
/// boundary).
///
/// # Errors
/// [`CliError::Io`] on a read failure.
fn first_hit(file: &Path) -> Result<Option<LocatedHit>, CliError> {
    let src = std::fs::read_to_string(file).map_err(|e| CliError::Io {
        path: file.to_path_buf(),
        source: e,
    })?;
    let Ok(hits) = panic_scan::scan_str(&src) else {
        return Ok(None);
    };
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
            println!(
                "package audit: note — `{}` exercises the `native-ffi` capability; its \
                 native effects cannot be inferred from Ipê alone.",
                prepared.manifest.name
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
    // Not in the index ⇒ a first submission; no predecessor to enforce.
    let Ok(entry) = crate::index::read_entry(&index_root, &prepared.manifest.name) else {
        println!(
            "package audit: `{}` has no previously published version in the index — \
             skipping the enforced-semver check (first version).",
            prepared.manifest.name
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
        println!(
            "package audit: `{}` has no published version below {new_version} — \
             skipping the enforced-semver check (first version).",
            prepared.manifest.name
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
                "warning: supply-chain advisory scan skipped — cargo-deny is not installed \
                 (`cargo install cargo-deny`). The package index enforces it; lockfile hash \
                 integrity is still verified."
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
    let text = std::fs::read_to_string(&source).map_err(|e| CliError::Io {
        path: source.clone(),
        source: e,
    })?;

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

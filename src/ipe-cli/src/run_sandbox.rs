//! Wiring the runtime capability sandbox around `ipe run`'s final exec.
//!
//! `ipe run` compiles, `cargo build`s, and then runs the emitted `ipe-app`
//! binary. This module inserts the fail-closed jail between the build and the
//! run: it resolves the program's capability set (`inferred ∪ declared`), lowers
//! it to a [`SandboxProfile`], establishes the OS jail, and execs the app inside
//! it. An undeclared effect the app attempts fails at the OS boundary; a
//! declared one works.
//!
//! Fail-closed everywhere: if the jail cannot be built (a missing primitive, an
//! unsupported platform), the run **refuses** rather than running unconfined.
//! The only override is [`OVERRIDE_ENV`], and it is a hard error — not a
//! warning — when the set includes a high-value native axis.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ipe_ir::Capability;
use ipe_sandbox::run_jail::{
    self, DatabaseAxis, RunJailDefect, SandboxProfile,
};

use crate::CliError;
use crate::project::ProjectManifest;

/// The narrow, run-jail-specific unsandboxed override — DISTINCT from the
/// FFI-compile override (`IPE_FFI_ALLOW_UNSANDBOXED`). It downgrades the
/// refusal to a warning ONLY for a pure / low-value-axis program; for any
/// high-value native axis it is a hard error (there is no flag that runs
/// admitted native code unconfined).
pub const OVERRIDE_ENV: &str = "IPE_ALLOW_UNSANDBOXED";

/// The resolved capability sets for a program about to run.
pub struct ResolvedCapabilities {
    /// The Ipê-inferred set (over the reachable kernels).
    pub inferred: BTreeSet<Capability>,
    /// The manifest-declared set (empty for a single-file program).
    pub declared: BTreeSet<Capability>,
}

impl ResolvedCapabilities {
    /// The authoritative union — the set the jail is built from.
    #[must_use]
    pub fn union(&self) -> BTreeSet<Capability> {
        self.inferred.union(&self.declared).copied().collect()
    }
}

/// Lower a database driver to the concrete axis the profile needs. Every
/// project has a resolved driver (it defaults to SQLite), so this is total —
/// there is no "unknown driver" path at `ipe run` (the fail-closed
/// [`DatabaseAxis::NotApplicable`] path exists for callers that genuinely
/// cannot resolve one).
#[must_use]
pub fn axis_for_driver(driver: ipe_backend_rust::DbDriver, has_database: bool) -> DatabaseAxis {
    if !has_database {
        return DatabaseAxis::NotApplicable;
    }
    match driver {
        // A file-backed store is a filesystem effect.
        ipe_backend_rust::DbDriver::Sqlite => DatabaseAxis::Filesystem,
        // A TCP-connected server is a network effect.
        ipe_backend_rust::DbDriver::Postgres => DatabaseAxis::Network,
    }
}

/// Build the [`SandboxProfile`] for a program from its resolved capabilities and
/// the project's database driver.
///
/// # Errors
///
/// [`CliError::UsageOwned`] wrapping the [`RunJailDefect`] display when the
/// profile cannot be lowered (an unknown database driver — fail-closed).
pub fn build_profile(
    caps: &ResolvedCapabilities,
    driver: ipe_backend_rust::DbDriver,
) -> Result<SandboxProfile, CliError> {
    let union = caps.union();
    let has_database = union.contains(&Capability::Database);
    let axis = axis_for_driver(driver, has_database);
    // The env allowlist is empty in the first cut: the manifest declares the
    // `env` axis (on/off), not per-variable names. Fewer re-exported vars is
    // the tighter, fail-closed direction; per-name env is a tracked refinement.
    let env_allowlist: Vec<String> = Vec::new();
    run_jail::profile_from_capabilities(&caps.inferred, &caps.declared, axis, &env_allowlist)
        .map_err(|e| CliError::UsageOwned(RunJailDefect::Profile(e).to_string()))
}

/// Whether the resolved override env var is set to exactly `"1"` (mirroring the
/// build jail's strict `== "1"`, never a loose `is_some`).
#[must_use]
pub fn override_requested() -> bool {
    std::env::var_os(OVERRIDE_ENV).is_some_and(|v| v == "1")
}

/// Decide what to do when the jail cannot be established for `union`.
///
/// Returns `Ok(true)` to proceed unconfined (the override applies and the set is
/// low-value only, with a printed warning), or an `Err` refusal. A high-value
/// native axis makes the override a hard error — there is no unconfined run of
/// admitted native code.
///
/// # Errors
///
/// [`CliError::UsageOwned`] carrying the refusal (`IPE-F4413`) — either the raw
/// defect, or the "override is a hard error for a native axis" message.
pub fn resolve_refusal(
    defect: &RunJailDefect,
    union: &BTreeSet<Capability>,
) -> Result<bool, CliError> {
    if !override_requested() {
        // No override: the defect is the refusal, verbatim.
        return Err(CliError::UsageOwned(defect.to_string()));
    }
    if run_jail::is_low_value_only(union) {
        // Pure / clock / random only: the override may downgrade to a warning.
        eprintln!(
            "warning: {OVERRIDE_ENV}=1 — running WITHOUT a capability jail. This program's \
             capability set is low-value (empty, or clock/random only), so nothing high-value \
             runs unconfined, but the OS-level guarantee is off. Never set this in CI."
        );
        return Ok(true);
    }
    // A high-value native axis is present: the override is a hard error.
    let names: Vec<&str> = union
        .iter()
        .filter(|c| !matches!(c, Capability::Clock | Capability::Random))
        .map(|c| c.as_str())
        .collect();
    Err(CliError::UsageOwned(format!(
        "{}: {defect}\n  {OVERRIDE_ENV}=1 cannot override this: the program reaches a high-value \
         native axis ({}) that would run with your full authority unconfined. There is no flag \
         that runs admitted native code without a jail — install the jail primitives, or narrow \
         the program.",
        RunJailDefect::UnsupportedPlatform { reason: "" }.code().as_str(),
        names.join(", ")
    )))
}

/// Establish the jail and exec `app` inside it, or apply the fail-closed
/// refusal / override policy.
///
/// On success (Linux, jail established) this **does not return** — it replaces
/// the current process with the jailed app. When the override lets a low-value
/// program run unconfined, it returns `Ok(())` and the caller performs the
/// ordinary unjailed exec.
///
/// # Errors
///
/// [`CliError::UsageOwned`] on any fail-closed refusal.
pub fn jail_and_exec(
    profile: &SandboxProfile,
    union: &BTreeSet<Capability>,
    scoped_tmp: &Path,
    working_tree: &Path,
    app: &Path,
    app_args: &[OsString],
) -> Result<(), CliError> {
    let wants_wall_clock = profile.limits.wall_secs.is_some();
    let tools = match run_jail::probe_run_jail_tools(wants_wall_clock) {
        Ok(t) => t,
        Err(defect) => {
            // The jail primitive is unavailable / platform unsupported: apply
            // the override-or-refuse policy. If the override lets a low-value
            // program through, fall back to an unjailed run.
            return resolve_refusal(&defect, union).map(|_proceed_unconfined| ());
        }
    };
    match run_jail::exec_in_run_jail(&tools, profile, scoped_tmp, working_tree, app, app_args) {
        // `exec_in_run_jail` returns only on failure.
        Err(defect) => Err(CliError::UsageOwned(defect.to_string())),
        Ok(never) => match never {},
    }
}

/// Create the per-run scoped writable tempdir (the jail's only writable mount
/// when `filesystem` is absent). Placed under the system temp dir, uniquely
/// named.
///
/// # Errors
///
/// [`CliError::Io`] when the directory cannot be created.
pub fn make_scoped_tmp() -> Result<PathBuf, CliError> {
    let base = std::env::temp_dir();
    let unique = format!(
        "ipe-run-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let dir = base.join(unique);
    std::fs::create_dir_all(&dir).map_err(|e| CliError::Io {
        path: dir.clone(),
        source: e,
    })?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(inferred: &[Capability], declared: &[Capability]) -> ResolvedCapabilities {
        ResolvedCapabilities {
            inferred: inferred.iter().copied().collect(),
            declared: declared.iter().copied().collect(),
        }
    }

    #[test]
    fn union_merges_inferred_and_declared() {
        let c = caps(&[Capability::Network], &[Capability::Filesystem]);
        let u = c.union();
        assert!(u.contains(&Capability::Network));
        assert!(u.contains(&Capability::Filesystem));
    }

    #[test]
    fn sqlite_driver_lowers_database_to_filesystem() {
        assert_eq!(
            axis_for_driver(ipe_backend_rust::DbDriver::Sqlite, true),
            DatabaseAxis::Filesystem
        );
    }

    #[test]
    fn postgres_driver_lowers_database_to_network() {
        assert_eq!(
            axis_for_driver(ipe_backend_rust::DbDriver::Postgres, true),
            DatabaseAxis::Network
        );
    }

    #[test]
    fn no_database_axis_is_not_applicable() {
        assert_eq!(
            axis_for_driver(ipe_backend_rust::DbDriver::Sqlite, false),
            DatabaseAxis::NotApplicable
        );
    }

    #[test]
    fn build_profile_grants_the_union() {
        let c = caps(&[Capability::Network], &[Capability::Subprocess]);
        let p = build_profile(&c, ipe_backend_rust::DbDriver::Sqlite).expect("profile");
        assert!(p.network);
        assert!(p.subprocess);
    }

    #[test]
    fn refusal_without_override_is_verbatim() {
        let defect = RunJailDefect::PrimitiveUnavailable {
            missing: vec!["bwrap"],
        };
        let union: BTreeSet<Capability> = [Capability::Network].into_iter().collect();
        // No override env set in this test process.
        let r = resolve_refusal(&defect, &union);
        assert!(r.is_err());
        let msg = format!("{}", r.unwrap_err());
        assert!(msg.contains("IPE-F4413"), "{msg}");
    }

    #[test]
    fn a_low_value_only_set_is_recognised() {
        let low: BTreeSet<Capability> = [Capability::Clock, Capability::Random].into_iter().collect();
        assert!(run_jail::is_low_value_only(&low));
        let high: BTreeSet<Capability> = [Capability::Network].into_iter().collect();
        assert!(!run_jail::is_low_value_only(&high));
    }
}

/// Resolve the inferred and declared capability sets for a run, given the
/// project manifest (if any) and the resolved entry file used for single-file
/// inference.
///
/// For a manifest project the inferred set is the package-wide union (every
/// shipped module), matching the package-capability posture; the declared set
/// is the manifest's `[capabilities]`. For a single file there is no manifest,
/// so inference is over the entry alone and the declared set is empty.
///
/// # Errors
///
/// Propagates lowering / IO failures from capability inference.
pub fn resolve_for_run(
    manifest: Option<&ProjectManifest>,
    manifest_path: Option<&Path>,
    entry: &Path,
) -> Result<ResolvedCapabilities, CliError> {
    match (manifest, manifest_path) {
        (Some(m), Some(mpath)) => {
            let inferred = crate::infer_package_capabilities(mpath)?;
            Ok(ResolvedCapabilities {
                inferred,
                declared: m.capabilities.clone(),
            })
        }
        _ => {
            let program = crate::lower_entry(entry)?;
            Ok(ResolvedCapabilities {
                inferred: ipe_lower::program_capabilities(&program),
                declared: BTreeSet::new(),
            })
        }
    }
}

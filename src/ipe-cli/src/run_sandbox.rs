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

    #[test]
    fn floor_static_source_carries_the_capfloor_marker() {
        let p = SandboxProfile {
            network: true,
            ..SandboxProfile::maximally_isolated()
        };
        let src = capfloor_static_source(&p);
        // The floor LINE is encoded as byte values, not literal text — so assert
        // on the static shape and confirm the byte array decodes to the marker.
        assert!(src.contains("IPE_CAPABILITY_FLOOR"), "{src}");
        assert!(src.contains("#[used]"), "{src}");
        // The bytes are the exact `to_capfloor_line()` output.
        let line = p.to_capfloor_line();
        let first_byte = line.as_bytes()[0].to_string();
        assert!(src.contains(&format!("[{first_byte}, ")), "byte array present: {src}");
        assert!(line.contains("net=true"), "line grants network: {line}");
    }

    #[test]
    fn inject_floor_reference_is_idempotent_under_strip_and_reinject() {
        // A minimal emitted-main shape.
        let base = "fn ipe_main() {}\n\nfn main() {\n    run();\n}\n";
        let profile = SandboxProfile::maximally_isolated();
        // First injection: reference inside main + static appended.
        let referenced = inject_floor_reference(base).expect("anchor present");
        let once = format!("{referenced}{}", capfloor_static_source(&profile));
        assert!(once.contains("black_box(&IPE_CAPABILITY_FLOOR)"));
        // Re-emitting: strip then re-inject must not stack a second block.
        let stripped = strip_capfloor_block(&once);
        assert!(!stripped.contains("IPE_CAPABILITY_FLOOR"), "strip removed the ref+static");
        let re = inject_floor_reference(&stripped).expect("anchor present");
        let twice = format!("{re}{}", capfloor_static_source(&profile));
        assert_eq!(
            twice.matches("static IPE_CAPABILITY_FLOOR").count(),
            1,
            "exactly one floor static after re-emit"
        );
        assert_eq!(
            twice.matches("black_box(&IPE_CAPABILITY_FLOOR)").count(),
            1,
            "exactly one floor reference after re-emit"
        );
    }

    #[test]
    fn inject_floor_reference_refuses_a_missing_main_anchor() {
        assert!(inject_floor_reference("fn not_main() {}\n").is_err());
    }

    #[test]
    fn profile_axes_reconstructs_the_granted_set() {
        use ipe_sandbox::run_jail::FilesystemScope;
        let p = SandboxProfile {
            network: true,
            filesystem: FilesystemScope::WorkingTreeReadWrite,
            subprocess: false,
            env_allowlist: vec!["X".to_owned()],
            limits: ipe_sandbox::run_jail::RunResourceLimits::default(),
        };
        let axes = profile_axes(&p);
        assert!(axes.contains(&Capability::Network));
        assert!(axes.contains(&Capability::Filesystem));
        assert!(axes.contains(&Capability::Env));
        assert!(!axes.contains(&Capability::Subprocess));
    }
}

/// Reconstruct the capability axes a profile grants, as a `Capability` set — the
/// input to the override/refusal policy for a deployed artifact (which has no
/// source to re-infer from). `database` is not reconstructed: it was already
/// lowered to `network`/`filesystem` when the profile was built, so the axes
/// here are the concrete OS-enforced ones.
#[must_use]
pub fn profile_axes(profile: &SandboxProfile) -> BTreeSet<Capability> {
    use ipe_sandbox::run_jail::FilesystemScope;
    let mut set = BTreeSet::new();
    if profile.network {
        set.insert(Capability::Network);
    }
    if matches!(profile.filesystem, FilesystemScope::WorkingTreeReadWrite) {
        set.insert(Capability::Filesystem);
    }
    if profile.subprocess {
        set.insert(Capability::Subprocess);
    }
    if !profile.env_allowlist.is_empty() {
        set.insert(Capability::Env);
    }
    set
}

/// The Rust source of a `#[used] #[link_section]` static that embeds the
/// capability floor into the emitted binary's `.ipe.capfloor` ELF section.
///
/// `ipe exec` reads this section *passively off disk* (never by executing the
/// binary) as the authoritative floor a tampered `ipe.profile` cannot go below.
/// `#[used]` keeps the linker from garbage-collecting an otherwise-unreferenced
/// static; the section name matches [`ipe_sandbox::run_jail::CAPFLOOR_SECTION`].
#[must_use]
pub fn capfloor_static_source(profile: &SandboxProfile) -> String {
    let line = profile.to_capfloor_line();
    let bytes = line.as_bytes();
    // A byte array literal so the section holds exactly the floor line (no NUL,
    // no rustc string-merging surprises).
    let mut arr = String::new();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            arr.push_str(", ");
        }
        arr.push_str(&b.to_string());
    }
    // The floor lands in `.rodata` (an ALLOCATED section) rather than a custom
    // named section: an allocated section survives `strip` (the deploy artifact
    // builds release with `strip = true`), whereas a non-alloc custom section is
    // stripped away. `#[used]` + `#[no_mangle]` keep the linker from
    // garbage-collecting the never-read static. `ipe exec` finds the floor by
    // scanning the binary for the unique `ipe-capfloor` marker — see
    // `ipe_sandbox::run_jail::scan_capfloor`.
    format!(
        "\n// The runtime capability FLOOR, embedded read-only in `.rodata` so a\n\
         // tampered ipe.profile cannot request less isolation than this binary was\n\
         // built for. `ipe exec` scans this out of the binary WITHOUT running it.\n\
         // `.rodata` survives `strip`; a custom link-section would not.\n\
         #[used]\n\
         #[unsafe(no_mangle)]\n\
         pub static IPE_CAPABILITY_FLOOR: [u8; {}] = [{arr}];\n",
        bytes.len()
    )
}

/// Write the deployable enforcement artifacts into an emitted native project:
/// the strictly-parsed `ipe.profile` next to the crate, and the capability-floor
/// static appended to the emitted `src/main.rs` (embedded in the binary).
///
/// The profile is a *convenience mirror* the launcher parses; the authoritative
/// floor is the embedded section. A profile weaker than the floor is refused at
/// launch (`ipe exec`), so tampering the mirror alone cannot under-isolate.
///
/// # Errors
///
/// [`CliError::Io`] on any filesystem failure.
pub fn write_build_artifacts(out_dir: &Path, profile: &SandboxProfile) -> Result<(), CliError> {
    // 1. The ipe.profile mirror.
    let profile_path = out_dir.join("ipe.profile");
    std::fs::write(&profile_path, profile.to_profile_string()).map_err(|e| CliError::Io {
        path: profile_path,
        source: e,
    })?;

    // 2. Embed the capfloor into the emitted main.rs: a `#[used]` static holding
    //    the floor bytes, PLUS a `black_box` read of it at the top of `fn main`
    //    so the linker genuinely retains the bytes (a mere `#[used]` is
    //    garbage-collected by an aggressive linker like `mold`, and `strip`
    //    removes the unreferenced data). The read keeps the bytes in `.rodata`,
    //    where `strip` cannot touch them; `ipe exec` scans them out passively.
    //    Idempotent: a re-build replaces any prior floor block + reference.
    let main_rs = out_dir.join("src").join("main.rs");
    let existing = std::fs::read_to_string(&main_rs).map_err(|e| CliError::Io {
        path: main_rs.clone(),
        source: e,
    })?;
    let base = strip_capfloor_block(&existing);
    let referenced = inject_floor_reference(&base)?;
    let with_floor = format!("{referenced}{}", capfloor_static_source(profile));
    std::fs::write(&main_rs, with_floor).map_err(|e| CliError::Io {
        path: main_rs,
        source: e,
    })?;
    Ok(())
}

/// Remove any previously-appended capfloor block AND its main-body reference, so
/// re-emitting is idempotent (both are delimited by unique markers).
fn strip_capfloor_block(src: &str) -> String {
    const BLOCK_MARKER: &str = "\n// The runtime capability FLOOR, embedded read-only";
    const REF_MARKER: &str = "    // Retain the embedded capability floor";
    let without_block = src
        .find(BLOCK_MARKER)
        .map_or_else(|| src.to_owned(), |i| src[..i].to_owned());
    // Drop the injected reference line (and its comment) if present.
    without_block
        .lines()
        .filter(|l| !l.starts_with(REF_MARKER) && !l.contains("IPE_CAPABILITY_FLOOR"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// The line the linker retains the floor by: a `black_box` read of the static at
/// the top of `fn main`. Marks the floor bytes as genuinely used so no linker GC
/// or `strip` removes them.
const FLOOR_REFERENCE: &str = "    // Retain the embedded capability floor (keeps it past linker GC + strip).\n    std::hint::black_box(&IPE_CAPABILITY_FLOOR);\n";

/// Inject the floor reference at the top of `fn main() {` in the emitted source.
///
/// # Errors
///
/// [`CliError::UsageOwned`] if the `fn main` anchor is absent (the emitted
/// program shape has drifted — refuse rather than emit an unreferenced floor
/// that a linker would collect).
fn inject_floor_reference(src: &str) -> Result<String, CliError> {
    const ANCHOR: &str = "fn main() {\n";
    let idx = src.find(ANCHOR).ok_or_else(|| {
        CliError::UsageOwned(
            "ipe build: the emitted `fn main` anchor is absent, so the capability floor cannot be \
             retained past linker GC — refusing to write an unenforceable artifact"
                .to_owned(),
        )
    })?;
    let insert_at = idx + ANCHOR.len();
    let mut out = String::with_capacity(src.len() + FLOOR_REFERENCE.len());
    out.push_str(&src[..insert_at]);
    out.push_str(FLOOR_REFERENCE);
    out.push_str(&src[insert_at..]);
    Ok(out)
}

/// Read and verify the deployed artifact's floor against its `ipe.profile`, then
/// return the profile to jail with. The authoritative floor is the binary's
/// embedded `.ipe.capfloor` section, read passively.
///
/// # Errors
///
/// [`CliError::UsageOwned`] on a missing/tampered profile or a profile weaker
/// than the embedded floor (both refuse-to-run).
pub fn load_and_verify_artifact(
    profile_path: &Path,
    binary_path: &Path,
) -> Result<SandboxProfile, CliError> {
    use ipe_sandbox::run_jail;

    // Parse the profile mirror strictly (parse-fail ⇒ refuse).
    let profile_text = std::fs::read_to_string(profile_path).map_err(|e| CliError::Io {
        path: profile_path.to_path_buf(),
        source: e,
    })?;
    let profile = run_jail::parse_profile(&profile_text).map_err(|e| {
        CliError::UsageOwned(format!(
            "{}: {e} — refusing to run (a profile that does not parse is not honored)",
            RunJailDefect::ProfileWeakerThanFloor.code().as_str()
        ))
    })?;

    // Read the authoritative floor from the binary's embedded `.rodata` bytes
    // (passively — the binary is NOT executed). A binary with no readable floor
    // refuses.
    let binary = std::fs::read(binary_path).map_err(|e| CliError::Io {
        path: binary_path.to_path_buf(),
        source: e,
    })?;
    let floor = run_jail::scan_capfloor(&binary).ok_or_else(|| {
        CliError::UsageOwned(format!(
            "{}: the binary carries no readable capability floor — refusing to run an artifact \
             whose floor cannot be verified",
            RunJailDefect::ProfileWeakerThanFloor.code().as_str()
        ))
    })?;

    // The profile MUST isolate at least as much as the embedded floor.
    if !profile.satisfies_capfloor(&floor) {
        return Err(CliError::UsageOwned(
            RunJailDefect::ProfileWeakerThanFloor.to_string(),
        ));
    }
    Ok(profile)
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

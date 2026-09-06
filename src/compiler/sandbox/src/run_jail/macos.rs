//! The macOS run jail: the `sandbox-exec` (Seatbelt/SBPL) arms plus the
//! in-memory sealed-app delivery macOS uses in place of Linux's sealing memfd.
//! Compiled only on macOS; every other target gets the refuse stubs in
//! [`super`].

#![cfg(target_os = "macos")]

use std::ffi::OsString;
use std::path::Path;

use super::{RunJailDefect, RunJailTools, SandboxProfile};

/// macOS: probe for `sandbox-exec`, the run jail's only primitive.
///
/// The `bwrap`/`prlimit` tools the Linux jail needs do not apply. The macOS
/// [`exec_in_run_jail`] arm re-resolves `sandbox-exec` itself and does NOT read
/// the returned tools, so the returned [`RunJailTools`] is an inert "primitive
/// present" token whose fields all hold the resolved `sandbox-exec` path (never
/// used for a Linux tool invocation on macOS). Fail-closed: an absent
/// `sandbox-exec` refuses.
///
/// `_wants_wall_clock` is unused on macOS: the SBPL run jail carries no external
/// wall-clock helper.
///
/// # Errors
///
/// [`RunJailDefect::PrimitiveUnavailable`] when `sandbox-exec` is absent.
pub fn probe_run_jail_tools(_wants_wall_clock: bool) -> Result<RunJailTools, RunJailDefect> {
    let Some(sandbox_exec) = crate::build_jail::find_in_path("sandbox-exec") else {
        return Err(RunJailDefect::PrimitiveUnavailable {
            missing: vec!["sandbox-exec"],
        });
    };
    Ok(RunJailTools {
        bwrap: sandbox_exec.clone(),
        prlimit: sandbox_exec.clone(),
        timeout: Some(sandbox_exec),
    })
}

/// Run the emitted `app` binary inside the macOS `sandbox-exec` Seatbelt jail
/// described by `profile`, replacing the current process on success (Unix
/// `exec`).
///
/// The macOS counterpart to the `Linux` [`exec_in_run_jail`]: it lowers
/// the SAME [`SandboxProfile`] to a Seatbelt SBPL profile via the SAME
/// [`crate::build_jail::sbpl_from_profile`] the Tier-2 `build_in_jail` uses —
/// there is ONE SBPL generator, so what confines a Tier-2 build and what confines
/// the shipped app at run time cannot drift. It writes the profile into the
/// always-writable scratch and `exec`s `sandbox-exec -f <profile> <app> <args>`.
/// There is NO shell token anywhere — the payload is a direct argv, so the
/// quoting/injection class does not exist.
///
/// The SBPL enforces the network, filesystem, and subprocess axes; the `env` axis
/// is enforced HERE in the launcher (Seatbelt cannot scrub env): the environment
/// is cleared and only the profile's allowlisted names re-exported, via the SAME
/// [`crate::build_jail::macos_scrubbed_env`] the build jail and the e2e use — so
/// all four runtime-enforced axes are contained, matching the Linux jail, and the
/// FFI admit path's `Holds` verdict is honest on macOS.
///
/// `scoped_tmp` is the one always-writable scratch (also where the SBPL profile
/// is written); `working_tree` is writable only when the profile grants the
/// filesystem axis. Fail-closed: an absent `sandbox-exec`, an unwritable profile,
/// or a failed `exec` REFUSES — the capability-bearing app is never run
/// unconfined.
///
/// The jail's actual deny behaviour is proven by the `macos-run-jail` CI job on a
/// real macOS runner; this crate builds the arm and unit-tests the pure SBPL
/// lowering on any host, but only a macOS runner exercises `sandbox-exec`.
///
/// # Errors
///
/// Any [`RunJailDefect`]; on success it does not return.
pub fn exec_in_run_jail(
    _tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    app: &Path,
    app_args: &[OsString],
) -> Result<std::convert::Infallible, RunJailDefect> {
    use std::os::unix::process::CommandExt as _;

    // `sandbox-exec` is the mandatory macOS jail primitive. Absent ⇒ refuse; the
    // capability-bearing app is never run unconfined.
    let Some(sandbox_exec) = crate::build_jail::find_in_path("sandbox-exec") else {
        return Err(RunJailDefect::PrimitiveUnavailable {
            missing: vec!["sandbox-exec"],
        });
    };

    // Lower the SAME profile through the SAME SBPL generator the build jail uses,
    // and write it into the always-writable scratch (never a shared temp path
    // that could race or persist).
    let sbpl = crate::build_jail::sbpl_from_profile(profile, scoped_tmp, working_tree);
    let profile_file = scoped_tmp.join("ipe-run.sb");
    if let Err(e) = std::fs::write(&profile_file, sbpl.as_bytes()) {
        return Err(RunJailDefect::Spawn {
            detail: format!("could not write the SBPL run-jail profile: {e}"),
        });
    }

    // argv: sandbox-exec -f <profile> <app> <app_args…>. Direct argv, no shell.
    let mut cmd = std::process::Command::new(&sandbox_exec);
    cmd.arg("-f").arg(&profile_file).arg(app).args(app_args);
    // Enforce the `env` axis in the launcher (Seatbelt cannot scrub env): clear
    // the inherited environment and re-export ONLY the scrubbed base plus the
    // profile's allowlisted names, mirroring the Linux jail's `--clearenv`. The
    // scrub is the SAME `macos_scrubbed_env` the e2e proves, so what confines the
    // env at run time and what the test asserts cannot drift.
    let host_env = |k: &str| std::env::var_os(k);
    cmd.env_clear();
    for (name, value) in crate::build_jail::macos_scrubbed_env(profile, scoped_tmp, &host_env) {
        cmd.env(name, value);
    }
    let err = cmd.exec();
    // `exec` only returns on failure; the scratch profile is inert either way.
    Err(RunJailDefect::Spawn {
        detail: err.to_string(),
    })
}

/// A verified embedded app binary held in memory.
///
/// macOS has no `memfd_create` and `sandbox-exec` cannot exec an inherited
/// descriptor, so the sealed-fd delivery the Linux arm uses is unavailable. The
/// bytes are held here after the single verification read; the exec arm writes
/// them ONCE to an exclusively-created (`O_EXCL`, mode 0700, unpredictably
/// named) file inside `scoped_tmp` and execs that. `scoped_tmp` is itself an
/// exclusively-created, unpredictable, owner-only directory, so no other path
/// exists to pre-seed or swap between the write and the exec.
pub struct SealedApp {
    bytes: Vec<u8>,
}

impl SealedApp {
    /// Read the app bytes for the capability-floor verification scan.
    #[must_use]
    pub fn read_sealed_bytes(&self) -> Result<Vec<u8>, RunJailDefect> {
        Ok(self.bytes.clone())
    }
}

/// Hold `bytes` for a later exclusive write-then-exec inside the jail scratch.
///
/// The Linux counterpart seals a memfd; macOS keeps the verified bytes in
/// memory because it has no sealing memfd and cannot fd-exec through
/// `sandbox-exec`.
///
/// # Errors
///
/// Infallible in practice; returns `Result` for arm-parity with the Linux
/// [`write_sealed_app_memfd`].
pub fn write_sealed_app_memfd(bytes: &[u8]) -> Result<SealedApp, RunJailDefect> {
    Ok(SealedApp {
        bytes: bytes.to_vec(),
    })
}

/// macOS embed-mode exec: write the verified bytes to an exclusively-created
/// file inside the exclusive `scoped_tmp` scratch and exec it under
/// `sandbox-exec`.
///
/// `sandbox-exec` cannot exec an inherited descriptor, so the same-inode
/// guarantee is delivered structurally instead: `scoped_tmp` is an
/// exclusively-created, unpredictable, owner-only directory, and the app file is
/// created with `O_EXCL` under a random name, so between the write and the exec
/// there is no predictable path an attacker can pre-seed or swap.
///
/// # Errors
///
/// Any [`RunJailDefect`]; on success it does not return.
pub fn exec_embedded_in_run_jail(
    tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    app: &SealedApp,
    app_args: &[OsString],
) -> Result<std::convert::Infallible, RunJailDefect> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    // Exclusive-create the app file under the exclusive scratch dir. A random
    // name plus `create_new` (O_EXCL) means a pre-seeded entry fails rather than
    // being followed.
    let mut entropy = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut entropy))
        .is_err()
    {
        return Err(RunJailDefect::Spawn {
            detail: "could not read OS entropy for the embedded app path".to_owned(),
        });
    }
    let mut hex = String::with_capacity(32);
    for b in entropy {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    let app_path = scoped_tmp.join(format!("ipe-app-{hex}"));
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&app_path)
    {
        Ok(f) => f,
        Err(e) => {
            return Err(RunJailDefect::Spawn {
                detail: format!("could not create the embedded app file: {e}"),
            });
        }
    };
    if let Err(e) = file.write_all(&app.bytes) {
        return Err(RunJailDefect::Spawn {
            detail: format!("could not write the embedded app file: {e}"),
        });
    }
    drop(file);

    exec_in_run_jail(
        tools,
        profile,
        scoped_tmp,
        working_tree,
        &app_path,
        app_args,
    )
}

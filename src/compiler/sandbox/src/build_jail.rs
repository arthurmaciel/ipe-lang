//! The *returning* build jail: run a command under the same confinement the
//! runtime jail uses, then decode its result into a typed [`JailOutcome`].
//!
//! The runtime jail ([`crate::run_jail::exec_in_run_jail`]) `exec`s the emitted
//! app and never returns — it is the launcher of a confined process. Tier-2
//! package admission needs the opposite shape: run an untrusted native build (or
//! a capability probe) inside a jail scoped to the package's *declared*
//! capability set, then observe the *outcome* and reconcile declared-vs-demanded.
//! This module is that returning entry.
//!
//! ## The confinement is not forked
//!
//! The jail is lowered from the SAME [`crate::run_jail::SandboxProfile`] the
//! runtime jail runs under. On `Linux/x86_64` it lowers via the SAME
//! [`crate::run_jail::run_jail_argv`] + [`crate::seccomp`] program the runtime
//! jail uses; on macOS it lowers the SAME profile to a Seatbelt SBPL profile
//! ([`sbpl_from_profile`]) enforced by `sandbox-exec`. Either way there is a
//! single source of the confining profile: what Tier-2 confines a build to and
//! what the shipped artifact is confined to at run time cannot drift.
//!
//! ## The denial signal is wrapper-owned
//!
//! Untrusted code inside the jail must not be able to forge a clean result. The
//! per-axis denial signal is the admission probe's own **exit-code contract**
//! (a fixed disjoint code range, [`AXIS_EXIT_NETWORK`] / [`AXIS_EXIT_FILESYSTEM`]),
//! not scraped from the payload's stdout. A [`JailOutcome::Clean`] is only ever
//! produced by *positive proof* of the probe's clean exit; every unrecognised
//! exit, signal, or ambiguous state decodes to a non-`Clean` outcome.

use std::ffi::OsString;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use crate::run_jail::run_jail_argv;
use crate::run_jail::{RunJailDefect, RunJailTools, SandboxProfile};
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use crate::seccomp;
// The SBPL text is a pure function of the profile, so its deny/allow surface is
// unit-testable on any host (compiled under `test` on Linux); the macOS jail
// that feeds it to `sandbox-exec` is `cfg(target_os = "macos")`. `FilesystemScope`
// names the profile's write scope the SBPL lowers.
#[cfg(any(target_os = "macos", test))]
use crate::run_jail::FilesystemScope;

// ── the per-axis denial exit-code contract ──────────────────────────────────
//
// These codes are the wrapper-owned signal the Tier-2 admission probe emits
// (`tests/fixtures/admission/untrusted-build.sh`, `PROBE_MODE=tier2`). They live
// in a disjoint range from the enforce/control codes (2–5) so a differential
// denial can never be confused with a broken-jail control failure. A single
// source of truth: the fixture and this decoder must agree, asserted by the
// crate's build-jail tests.

/// The probe exited cleanly: every action it attempted under the
/// declared-scoped jail succeeded — no withheld axis was demanded.
pub const AXIS_EXIT_CLEAN: i32 = 0;

/// The wrapper's untrusted child build failed for an ordinary (non-capability)
/// reason and no withheld axis was demanded — an ordinary build-fails-in-jail.
///
/// It is NOT a per-axis denial, so it decodes to [`JailOutcome::BuildFailed`]
/// (a reject), never [`JailOutcome::Clean`]. The load-bearing hinge: the wrapper
/// never exits `AXIS_EXIT_CLEAN` when the child build failed, or a broken build
/// would forge a clean certify.
pub const TIER2_EXIT_BUILD_FAILED: i32 = 6;

/// The probe's network action was denied by the jail — the native code demanded
/// the `network` axis the declared-scoped jail withheld.
pub const AXIS_EXIT_NETWORK: i32 = 10;

/// The probe's out-of-scratch filesystem write was denied by the jail — the
/// native code demanded the `filesystem` axis the declared-scoped jail withheld.
pub const AXIS_EXIT_FILESYSTEM: i32 = 11;

// ── the capability axis a denial names ───────────────────────────────────────

/// The confinement axis a per-axis denial names.
///
/// Only the axes the admission probe can actually exercise are representable — a
/// denial can name `network` or `filesystem`, never `clock` (which carries no OS
/// control) or `native-ffi` (an epistemic marker). Making the non-probeable axes
/// unrepresentable here means [`JailOutcome::Denied`] can only ever carry an axis
/// the probe genuinely observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityAxis {
    /// Outbound/inbound network — a socket the probe opened was denied.
    Network,
    /// An out-of-scratch filesystem write the probe attempted was denied.
    Filesystem,
}

impl CapabilityAxis {
    /// The stable lowercase axis name, matching the capability wire vocabulary.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Filesystem => "filesystem",
        }
    }

    /// Decode a wrapper-owned per-axis exit code, or `None` when the code is not
    /// a recognised per-axis denial. `None` is the fail-closed hinge: the caller
    /// never treats an unrecognised code as clean.
    #[must_use]
    const fn from_exit_code(code: i32) -> Option<Self> {
        match code {
            AXIS_EXIT_NETWORK => Some(Self::Network),
            AXIS_EXIT_FILESYSTEM => Some(Self::Filesystem),
            _ => None,
        }
    }
}

// ── the typed outcome ────────────────────────────────────────────────────────

/// The outcome of a build/probe run inside the declared-scoped jail.
///
/// Make-invalid-states-unrepresentable: the illegal state — "admitted despite a
/// denial" — is not expressible. There is exactly one clean variant, produced
/// only by positive proof of the probe's clean exit; every other observable
/// result (a named denial, an ordinary build failure, an unestablished jail) is
/// a distinct non-clean variant. A caller that admits only on [`Self::Clean`]
/// cannot be fooled into admitting a jail that never ran or a payload that was
/// denied a capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JailOutcome {
    /// The jail was established and the probe exited cleanly: no withheld axis
    /// was demanded. The ONLY outcome an admit path may accept.
    Clean,
    /// The jail was established and the probe was denied a capability the
    /// declared-scoped jail withheld. `axis` names which — the used-but-undeclared
    /// signal.
    Denied {
        /// The confinement axis the denial named.
        axis: CapabilityAxis,
    },
    /// The jail was established and the payload exited non-zero for a reason that
    /// is NOT a recognised per-axis denial — an ordinary compile/link/test error,
    /// a signal, or an unrecognised exit code. Fail-closed: an ambiguous failure
    /// is never `Clean`.
    BuildFailed {
        /// A short, non-sensitive reason for the diagnostic (never the payload's
        /// own stdout — that is untrusted).
        reason: String,
    },
    /// No jail could be established (a missing primitive, an unsupported
    /// platform, an unbuildable profile, a spawn failure). The untrusted build
    /// was NOT run unconfined — the outcome is a refusal, never a clean pass.
    Unavailable {
        /// The refusal that prevented the jail from being established.
        defect: RunJailDefect,
    },
}

impl JailOutcome {
    /// Whether this outcome is the single admit-eligible one. The admit predicate
    /// lives here so it is total over the outcome and inspectable by tests.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        matches!(self, Self::Clean)
    }

    /// Decode a completed jailed run into a typed outcome, fail-closed.
    ///
    /// `exit` is the payload's exit code, `None` when it was killed by a signal
    /// or the wall clock. The decode is the security-load-bearing §2.4 mechanic:
    ///
    /// - a clean exit (`0`) ⇒ [`Self::Clean`] — the only positive-proof branch;
    /// - a recognised per-axis code ⇒ [`Self::Denied`] naming the axis;
    /// - a signal / no exit code ⇒ [`Self::BuildFailed`] (never clean);
    /// - any other non-zero exit ⇒ [`Self::BuildFailed`] (an ordinary build
    ///   error, or an unrecognised code — both non-clean, both reject).
    #[must_use]
    pub fn decode(exit: Option<i32>) -> Self {
        let Some(code) = exit else {
            return Self::BuildFailed {
                reason: "the jailed probe was killed by a signal or the wall clock \
                         (no exit code) — treated as a build failure, never clean"
                    .to_owned(),
            };
        };
        if code == AXIS_EXIT_CLEAN {
            return Self::Clean;
        }
        if let Some(axis) = CapabilityAxis::from_exit_code(code) {
            return Self::Denied { axis };
        }
        Self::BuildFailed {
            reason: format!(
                "the jailed probe exited {code}, which is neither a clean exit nor a \
                 recognised per-axis denial — treated as a build failure, never clean"
            ),
        }
    }
}

// ── the returning build-jail entry ───────────────────────────────────────────

/// Run `payload` inside a jail lowered from `profile`, wait for it, and return
/// the decoded [`JailOutcome`].
///
/// This is the returning counterpart to [`crate::run_jail::exec_in_run_jail`]:
/// it spawns the confined process and waits, rather than replacing the current
/// process. The confinement is identical — the same seccomp program for the
/// profile's subprocess axis, the same [`run_jail_argv`] flag vocabulary — so a
/// build observed under Tier-2 is confined exactly as the shipped artifact will
/// be at run time.
///
/// A jail that cannot be established (unsupported platform, a seccomp program
/// that cannot be compiled for this architecture, a spawn failure) yields
/// [`JailOutcome::Unavailable`] — the untrusted payload is never run unconfined
/// on any path.
///
/// # Errors
///
/// This function does not return `Err`: every failure to establish or run the
/// jail is folded into a non-`Clean` [`JailOutcome`], so a caller reconciling
/// declared-vs-demanded has a total value to match on and cannot mistake an
/// establishment failure for a clean run.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[must_use]
pub fn build_in_jail(
    tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    extra_ro_binds: &[PathBuf],
    payload: &[OsString],
) -> JailOutcome {
    let Some(program) = seccomp::subprocess_deny_program(profile.subprocess) else {
        return JailOutcome::Unavailable {
            defect: RunJailDefect::UnsupportedPlatform {
                reason: "no seccomp filter can be compiled for this architecture",
            },
        };
    };
    let bytes = seccomp::program_bytes(&program);
    let seccomp_fd = match crate::run_jail::write_seccomp_memfd(&bytes) {
        Ok(fd) => fd,
        Err(defect) => return JailOutcome::Unavailable { defect },
    };
    // Own the memfd so it is closed when this function returns. bwrap reads the
    // seccomp filter by fd number during jail setup — before the child runs — so
    // closing it after the child is waited on is safe. Unlike the run jail
    // (which `exec`s and never returns), this build jail RETURNS and is called
    // once per axis in the audit tightening loop; a raw fd would leak one memfd
    // per call in the long-lived audit/CI process.
    let seccomp_owned = unsafe { OwnedFd::from_raw_fd(seccomp_fd) };

    let host_env = |k: &str| std::env::var_os(k);
    let argv = run_jail_argv(
        tools,
        profile,
        scoped_tmp,
        working_tree,
        extra_ro_binds,
        Some(seccomp_owned.as_raw_fd()),
        &host_env,
        payload,
    );

    let outcome = spawn_and_decode(&argv);
    // `seccomp_owned` drops here (after the child has been waited on inside
    // `spawn_and_decode`), closing the memfd. Keep it explicit so the ordering
    // is not subject to a future reorder.
    drop(seccomp_owned);
    outcome
}

/// Run `payload` inside a `sandbox-exec` Seatbelt jail lowered from `profile`,
/// wait for it, and return the decoded [`JailOutcome`] (macOS).
///
/// The macOS counterpart to the `Linux/x86_64` [`build_in_jail`]: it lowers the
/// SAME [`SandboxProfile`] to a Seatbelt SBPL profile ([`sbpl_from_profile`]),
/// writes it to a scratch-local file, and spawns
/// `sandbox-exec -f <profile> <payload>`, so a build observed under Tier-2 is
/// confined exactly as the shipped artifact will be at run time (single source
/// of the confining profile).
///
/// `scoped_tmp` is the one always-writable scratch; `working_tree` is writable
/// only when the profile grants the filesystem axis. `extra_ro_binds` is unused
/// on macOS (Seatbelt filters an existing view of the real filesystem rather
/// than constructing a bind mount namespace); it is accepted so the entry has
/// the same signature on every platform.
///
/// A jail that cannot be established (no `sandbox-exec`, an SBPL profile that
/// cannot be written, a spawn failure) yields [`JailOutcome::Unavailable`] — the
/// untrusted payload is never run unconfined on any path. Fail-closed.
///
/// The macOS jail's actual deny behaviour is verified by the `macos-latest` CI
/// job (`audit_native` E2E + the admission enforce-vs-control duality), not by a
/// local run: this crate builds and unit-tests the pure SBPL lowering on any
/// host, but only a real macOS runner exercises `sandbox-exec`.
///
/// # Errors
///
/// This function does not return `Err`: every failure to establish or run the
/// jail is folded into a non-`Clean` [`JailOutcome`].
#[cfg(target_os = "macos")]
#[must_use]
pub fn build_in_jail(
    _tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    _extra_ro_binds: &[PathBuf],
    payload: &[OsString],
) -> JailOutcome {
    // `sandbox-exec` is the mandatory macOS jail primitive. Absent ⇒ refuse; the
    // untrusted payload is never run unconfined.
    let Some(sandbox_exec) = find_in_path("sandbox-exec") else {
        return JailOutcome::Unavailable {
            defect: RunJailDefect::PrimitiveUnavailable {
                missing: vec!["sandbox-exec"],
            },
        };
    };

    // The SBPL profile is written into the always-writable scratch so it never
    // races or persists on a shared temp path, and is removed after the run.
    let sbpl = sbpl_from_profile(profile, scoped_tmp, working_tree);
    let profile_file = scoped_tmp.join("ipe-tier2.sb");
    if let Err(e) = std::fs::write(&profile_file, sbpl.as_bytes()) {
        return JailOutcome::Unavailable {
            defect: RunJailDefect::Spawn {
                detail: format!("could not write the SBPL profile: {e}"),
            },
        };
    }

    // argv: sandbox-exec -f <profile> <payload…>. No shell token anywhere — the
    // payload is a direct argv, so the quoting/injection class does not exist.
    let mut argv: Vec<OsString> = Vec::with_capacity(payload.len() + 3);
    argv.push(sandbox_exec.into_os_string());
    argv.push("-f".into());
    argv.push(profile_file.clone().into_os_string());
    argv.extend(payload.iter().cloned());

    let outcome = spawn_and_decode(&argv);
    // Best-effort cleanup; a leftover profile in the scratch is inert.
    let _ = std::fs::remove_file(&profile_file);
    outcome
}

/// Non-Linux, non-macOS stub: the returning build jail is a documented
/// refuse-gap off the wired platforms, mirroring
/// [`crate::run_jail::exec_in_run_jail`].
#[cfg(not(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos")))]
#[must_use]
pub fn build_in_jail(
    _tools: &RunJailTools,
    _profile: &SandboxProfile,
    _scoped_tmp: &Path,
    _working_tree: &Path,
    _extra_ro_binds: &[PathBuf],
    _payload: &[OsString],
) -> JailOutcome {
    JailOutcome::Unavailable {
        defect: RunJailDefect::UnsupportedPlatform {
            reason: "build jail is wired only on Linux/x86_64 and macOS",
        },
    }
}

/// Spawn the jail argv, wait, and decode the exit into a [`JailOutcome`].
///
/// A spawn failure is [`JailOutcome::Unavailable`] (the payload never ran); a
/// wait failure is a [`JailOutcome::BuildFailed`] (the jail ran but its result
/// is unobservable — never clean).
#[cfg(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos"))]
fn spawn_and_decode(argv: &[OsString]) -> JailOutcome {
    let Some((program, rest)) = argv.split_first() else {
        return JailOutcome::Unavailable {
            defect: RunJailDefect::Spawn {
                detail: "empty jail argv".to_owned(),
            },
        };
    };
    let child = std::process::Command::new(program)
        .args(rest)
        .stdin(std::process::Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return JailOutcome::Unavailable {
                defect: RunJailDefect::Spawn {
                    detail: e.to_string(),
                },
            };
        }
    };
    match child.wait() {
        Ok(status) => JailOutcome::decode(status.code()),
        Err(e) => JailOutcome::BuildFailed {
            reason: format!("could not await the jailed probe: {e}"),
        },
    }
}

// ── the macOS SBPL lowering ──────────────────────────────────────────────────

/// Lower a [`SandboxProfile`] to a Seatbelt SBPL profile text for `sandbox-exec`.
///
/// PURE TEXT — no process is spawned — so the exact deny/allow surface is
/// unit-testable on any host, exactly like the Linux [`run_jail_argv`]. The
/// macOS jail's *runtime* deny behaviour is the `macos-latest` CI job; this
/// function is what that jail enforces, proven correct here.
///
/// The profile is a `(allow default)` base with *targeted denials*, mirroring
/// `scripts/admission/jail-macos.sh`: on recent macOS a `(deny default)` base
/// blocks so many benign system operations that even explicit `(allow …)`
/// overrides leave the shell and its tools unable to run, so the working base is
/// allow-default plus the two threat denials Tier-2 differentially confines:
///
/// - **network**: withheld (`!profile.network`) ⇒ `(deny network*)` and the
///   low-level socket operations, so a probe socket is denied and the
///   `network` axis is observable. Granted ⇒ no network denial.
/// - **filesystem**: the scratch (`scoped_tmp`) is always writable; the working
///   tree is writable ONLY when the profile grants the filesystem axis. A
///   blanket `(deny file-write*)` withholds every other path, so an
///   out-of-scratch write under a filesystem-withholding profile is denied and
///   the `filesystem` axis is observable.
///
/// The scratch and (when granted) the working tree are written as `(subpath …)`
/// allow rules so the probe's benign write and a granted filesystem effect
/// succeed; the system temp locations the shell itself uses are always allowed
/// so the jail does not false-deny the launcher.
#[cfg(any(target_os = "macos", test))]
#[must_use]
pub fn sbpl_from_profile(
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
) -> String {
    use std::fmt::Write as _;

    // SBPL string literals are double-quoted; a path with an embedded `"` or `\`
    // would break the grammar. Escape both so a crafted scratch path cannot
    // inject SBPL. Writing to a String is infallible.
    let quote = |p: &Path| -> String {
        let mut out = String::from("\"");
        for ch in p.to_string_lossy().chars() {
            if ch == '"' || ch == '\\' {
                out.push('\\');
            }
            out.push(ch);
        }
        out.push('"');
        out
    };

    let mut s = String::from("(version 1)\n\n");
    // Allow-default base: the shell and its tools work; we selectively deny the
    // threats Tier-2 confines.
    s.push_str("(allow default)\n\n");

    // Network: deny unless the profile grants it. When granted, no denial is
    // emitted, so the allow-default base leaves the network reachable.
    if !profile.network {
        s.push_str("(deny network*)\n");
        s.push_str("(deny network-outbound)\n");
        s.push_str("(deny network-inbound)\n");
        s.push_str("(deny network-bind)\n\n");
    }

    // Filesystem: deny writes everywhere, then re-allow the always-writable
    // scratch and — only when the axis is granted — the working tree. The
    // blanket deny makes an out-of-scratch write observable as the filesystem
    // axis under a withholding profile.
    s.push_str("(deny file-write*)\n");
    let _ = writeln!(s, "(allow file-write* (subpath {}))", quote(scoped_tmp));
    if matches!(profile.filesystem, FilesystemScope::WorkingTreeReadWrite) {
        let _ = writeln!(s, "(allow file-write* (subpath {}))", quote(working_tree));
    }
    // The system temp locations the shell and its children write to; allowing
    // them prevents a false-deny of the launcher itself (never a threat path —
    // they are not the differentially-confined out-of-scratch escape target).
    s.push_str("(allow file-write* (subpath \"/private/var/folders\"))\n");
    s.push_str("(allow file-write* (subpath \"/private/tmp\"))\n");

    s
}

/// Resolve a program name to an absolute path via `PATH`, or `None` when absent.
/// The macOS jail refuses (fail-closed) when its `sandbox-exec` primitive is
/// missing.
#[cfg(target_os = "macos")]
fn find_in_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_exit_is_the_only_clean_outcome() {
        assert_eq!(
            JailOutcome::decode(Some(AXIS_EXIT_CLEAN)),
            JailOutcome::Clean
        );
        assert!(JailOutcome::decode(Some(AXIS_EXIT_CLEAN)).is_clean());
    }

    #[test]
    fn a_network_denial_exit_names_the_network_axis() {
        assert_eq!(
            JailOutcome::decode(Some(AXIS_EXIT_NETWORK)),
            JailOutcome::Denied {
                axis: CapabilityAxis::Network
            }
        );
    }

    #[test]
    fn a_filesystem_denial_exit_names_the_filesystem_axis() {
        assert_eq!(
            JailOutcome::decode(Some(AXIS_EXIT_FILESYSTEM)),
            JailOutcome::Denied {
                axis: CapabilityAxis::Filesystem
            }
        );
    }

    #[test]
    fn an_ordinary_nonzero_exit_is_a_build_failure_never_clean() {
        // A compile error (exit 1) is a build failure, not a capability denial —
        // both reject, the distinction only shapes the diagnostic.
        let o = JailOutcome::decode(Some(1));
        assert!(matches!(o, JailOutcome::BuildFailed { .. }));
        assert!(!o.is_clean());
    }

    #[test]
    fn an_unrecognised_exit_code_decodes_fail_closed_to_non_clean() {
        // A garbled / unexpected exit code is NOT clean and NOT a named denial —
        // fail-closed to BuildFailed so ambiguity can never admit.
        for code in [7, 42, 99, 125, 255, i32::MIN, i32::MAX] {
            let o = JailOutcome::decode(Some(code));
            assert!(
                matches!(o, JailOutcome::BuildFailed { .. }),
                "exit {code} must decode to BuildFailed, got {o:?}"
            );
            assert!(!o.is_clean(), "exit {code} must not be clean");
        }
    }

    #[test]
    fn a_signal_kill_with_no_exit_code_is_a_build_failure_never_clean() {
        // Killed by a signal or the wall clock ⇒ no exit code ⇒ never clean.
        let o = JailOutcome::decode(None);
        assert!(matches!(o, JailOutcome::BuildFailed { .. }));
        assert!(!o.is_clean());
    }

    #[test]
    fn the_tier2_build_failed_hinge_decodes_to_build_failed_never_clean() {
        // The wrapper's ordinary-child-build-failure code (6) MUST decode to a
        // reject, never Clean — the hinge on which a false certify would turn.
        let o = JailOutcome::decode(Some(TIER2_EXIT_BUILD_FAILED));
        assert!(matches!(o, JailOutcome::BuildFailed { .. }), "got {o:?}");
        assert!(!o.is_clean());
        // It is NOT a per-axis denial (disjoint from 10/11 and from the clean 0).
        assert!(CapabilityAxis::from_exit_code(TIER2_EXIT_BUILD_FAILED).is_none());
        assert_ne!(TIER2_EXIT_BUILD_FAILED, AXIS_EXIT_CLEAN);
    }

    #[test]
    fn the_per_axis_codes_are_disjoint_from_the_control_range() {
        // The Tier-2 per-axis codes must not collide with the enforce/control
        // codes 2–5, or a broken-jail control failure would read as a denial.
        for control in [2, 3, 4, 5] {
            assert!(CapabilityAxis::from_exit_code(control).is_none());
        }
        assert_eq!(
            CapabilityAxis::from_exit_code(AXIS_EXIT_NETWORK),
            Some(CapabilityAxis::Network)
        );
        assert_eq!(
            CapabilityAxis::from_exit_code(AXIS_EXIT_FILESYSTEM),
            Some(CapabilityAxis::Filesystem)
        );
    }

    #[test]
    fn axis_names_match_the_capability_wire_vocabulary() {
        assert_eq!(CapabilityAxis::Network.as_str(), "network");
        assert_eq!(CapabilityAxis::Filesystem.as_str(), "filesystem");
    }

    #[test]
    fn only_the_clean_variant_is_admit_eligible() {
        assert!(JailOutcome::Clean.is_clean());
        assert!(
            !JailOutcome::Denied {
                axis: CapabilityAxis::Network
            }
            .is_clean()
        );
        assert!(
            !JailOutcome::BuildFailed {
                reason: "x".to_owned()
            }
            .is_clean()
        );
        assert!(
            !JailOutcome::Unavailable {
                defect: RunJailDefect::UnsupportedPlatform { reason: "x" }
            }
            .is_clean()
        );
    }

    // ── the macOS SBPL lowering (pure text — runs on any host) ────────────────
    //
    // These prove the SBPL the macOS jail feeds `sandbox-exec` denies exactly the
    // withheld axis and confines writes to the scratch. They are the primary
    // local verification of the macOS confinement; the jail's *runtime* deny
    // behaviour is the macos-latest CI job.

    fn scoped(network: bool, filesystem: FilesystemScope) -> SandboxProfile {
        SandboxProfile {
            network,
            filesystem,
            ..SandboxProfile::maximally_isolated()
        }
    }

    #[test]
    fn sbpl_denies_network_when_the_network_axis_is_withheld() {
        let p = scoped(false, FilesystemScope::Isolated);
        let sbpl = sbpl_from_profile(&p, Path::new("/tmp/scratch"), Path::new("/work/tree"));
        assert!(sbpl.starts_with("(version 1)"), "{sbpl}");
        assert!(sbpl.contains("(allow default)"), "{sbpl}");
        // Every network-denial rule is present when the axis is withheld.
        for rule in [
            "(deny network*)",
            "(deny network-outbound)",
            "(deny network-inbound)",
            "(deny network-bind)",
        ] {
            assert!(sbpl.contains(rule), "missing {rule}: {sbpl}");
        }
    }

    #[test]
    fn sbpl_allows_network_when_the_network_axis_is_granted() {
        let p = scoped(true, FilesystemScope::Isolated);
        let sbpl = sbpl_from_profile(&p, Path::new("/tmp/scratch"), Path::new("/work/tree"));
        // A granted network axis emits NO network denial, so the allow-default
        // base leaves the network reachable.
        assert!(
            !sbpl.contains("(deny network"),
            "network granted must emit no network denial: {sbpl}"
        );
    }

    #[test]
    fn sbpl_confines_writes_to_the_scratch_when_filesystem_is_withheld() {
        let p = scoped(false, FilesystemScope::Isolated);
        let sbpl = sbpl_from_profile(&p, Path::new("/tmp/scratch"), Path::new("/work/tree"));
        // A blanket write denial, then the scratch re-allowed — an out-of-scratch
        // write is denied so the filesystem axis is observable.
        assert!(sbpl.contains("(deny file-write*)"), "{sbpl}");
        assert!(
            sbpl.contains("(allow file-write* (subpath \"/tmp/scratch\"))"),
            "scratch must be writable: {sbpl}"
        );
        // The working tree is NOT writable under a filesystem-withholding profile.
        assert!(
            !sbpl.contains("(allow file-write* (subpath \"/work/tree\"))"),
            "the working tree must not be writable when filesystem is withheld: {sbpl}"
        );
    }

    #[test]
    fn sbpl_grants_the_working_tree_write_when_filesystem_is_granted() {
        let p = scoped(false, FilesystemScope::WorkingTreeReadWrite);
        let sbpl = sbpl_from_profile(&p, Path::new("/tmp/scratch"), Path::new("/work/tree"));
        // The blanket deny stays (so a path outside BOTH scratch and tree is
        // still denied), and the working tree is re-allowed.
        assert!(sbpl.contains("(deny file-write*)"), "{sbpl}");
        assert!(
            sbpl.contains("(allow file-write* (subpath \"/work/tree\"))"),
            "the working tree must be writable when filesystem is granted: {sbpl}"
        );
    }

    #[test]
    fn sbpl_escapes_quotes_and_backslashes_in_paths() {
        // A scratch path with an embedded quote/backslash must not break the SBPL
        // grammar — both are escaped so a crafted path cannot inject a rule.
        let p = scoped(false, FilesystemScope::Isolated);
        let sbpl = sbpl_from_profile(&p, Path::new("/tmp/scr\"atch\\x"), Path::new("/work/tree"));
        assert!(
            sbpl.contains("(allow file-write* (subpath \"/tmp/scr\\\"atch\\\\x\"))"),
            "the quote and backslash must be escaped: {sbpl}"
        );
    }

    #[test]
    fn sbpl_maximally_isolated_denies_network_and_confines_writes() {
        // The floor: nothing granted → network denied, writes confined to scratch.
        let sbpl = sbpl_from_profile(
            &SandboxProfile::maximally_isolated(),
            Path::new("/s"),
            Path::new("/w"),
        );
        assert!(sbpl.contains("(deny network*)"), "{sbpl}");
        assert!(sbpl.contains("(deny file-write*)"), "{sbpl}");
        assert!(
            !sbpl.contains("(allow file-write* (subpath \"/w\"))"),
            "no working-tree write on the isolated floor: {sbpl}"
        );
    }
}

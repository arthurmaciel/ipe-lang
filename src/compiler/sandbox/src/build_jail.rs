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

    // The Linux jail's env is scrubbed inside the bwrap argv (`--clearenv` +
    // allowlisted re-export), so no launcher-side env override is needed here.
    let outcome = spawn_and_decode(&argv, None);
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

    // Enforce the `env` axis in the launcher (Seatbelt cannot scrub env),
    // mirroring the run jail and the Linux build jail's bwrap `--clearenv`, so a
    // Tier-2 build is confined on the env axis exactly as the shipped app is.
    let host_env = |k: &str| std::env::var_os(k);
    let scrubbed_env = macos_scrubbed_env(profile, scoped_tmp, &host_env);

    let outcome = spawn_and_decode(&argv, Some(&scrubbed_env));
    // Best-effort cleanup; a leftover profile in the scratch is inert.
    let _ = std::fs::remove_file(&profile_file);
    outcome
}

/// Run `payload` inside a Windows Job Object + AppContainer jail lowered from
/// `profile`, wait for it, and return the decoded [`JailOutcome`] (Windows).
///
/// The Windows counterpart to the `Linux/x86_64` and macOS [`build_in_jail`]: it
/// lowers the SAME [`SandboxProfile`] through the SAME Win32 sequence the run jail
/// uses ([`crate::run_jail::build_windows_jailed`] → `windows_jail::run_confined`)
/// — a Job Object (subprocess axis, kill-on-close so no orphan survives the audit
/// call), an AppContainer lowbox token (filesystem + network axes: the
/// `internetClient` capability SID is added iff `profile.network`, and the scratch
/// — plus the working tree when the filesystem axis is granted — is ACLed to the
/// container SID), and a launcher-side environment scrub (env axis). So a build
/// observed under Tier-2 is confined exactly as the shipped artifact is at run
/// time (one jail source, no fork).
///
/// The `FILE_PERSISTENT_ACLS` volume probe gates the launch BEFORE spawn: on a
/// volume that does not persist+enforce DACLs the ACL boundary is a no-op, so the
/// jail refuses ([`JailOutcome::Unavailable`]) rather than run the untrusted build
/// with a silently-unconfined filesystem axis (the probe lives inside
/// `windows_jail::run_confined`, ahead of `CreateProcessW`).
///
/// `extra_ro_binds` is unused on Windows (AppContainer filters an existing view of
/// the real filesystem rather than constructing a bind-mount namespace); it is
/// accepted so the entry has the same signature on every platform.
///
/// A jail that cannot be established (a Job Object / token / capability SID / ACL
/// / attribute list / `CreateProcessW` that cannot be built, or a non-ACL scratch
/// volume) yields [`JailOutcome::Unavailable`] — the untrusted payload is never
/// run unconfined on any path. Fail-closed. Every kernel object is RAII-released
/// on every path, so the audit's per-axis tightening loop leaks nothing.
///
/// The jail's actual deny behaviour is proven by the `windows-tier2` CI job on a
/// real `windows-2022` runner (a hosted runner — a Job Object / AppContainer is a
/// plain Win32 sequence, no Docker Windows daemon needed); this crate builds the
/// arm and unit-tests the pure pieces on any host.
///
/// # Errors
///
/// This function does not return `Err`: every failure to establish or run the
/// jail is folded into a non-`Clean` [`JailOutcome`].
#[cfg(target_os = "windows")]
#[must_use]
#[allow(clippy::doc_markdown, clippy::too_long_first_doc_paragraph)]
pub fn build_in_jail(
    _tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    _extra_ro_binds: &[PathBuf],
    payload: &[OsString],
) -> JailOutcome {
    // The Windows jail RETURNS the child's exit code (Windows has no `exec`-
    // replace), which decodes through the SAME `JailOutcome::decode` the Linux
    // and macOS arms use: `0` is the sole `Clean` branch; a per-axis code names
    // the axis; any ambiguous exit is `BuildFailed`. A jail that could not be
    // established (a missing primitive, a non-ACL scratch volume gated by the
    // pre-spawn `FILE_PERSISTENT_ACLS` probe, a failed `CreateProcessW`) is a
    // `RunJailDefect` → `Unavailable`; the untrusted build never runs unconfined.
    match crate::run_jail::build_windows_jailed(profile, scoped_tmp, working_tree, payload) {
        Ok(code) => JailOutcome::decode(Some(win_exit_to_i32(code))),
        Err(defect) => JailOutcome::Unavailable { defect },
    }
}

/// Map a Windows process exit code (`u32` from `GetExitCodeProcess`) to the
/// signed exit the wrapper-owned per-axis contract is decoded against.
///
/// The Tier-2 probe's exit codes (`0`, `6`, `10`, `11`) are small non-negative
/// values that round-trip identically; a value above `i32::MAX` (a process that
/// exited with a high-bit code) cannot be a recognised per-axis code, so it
/// saturates to a value that [`JailOutcome::decode`] treats as `BuildFailed`
/// (fail-closed — never `Clean`, never a spurious named denial).
#[cfg(any(target_os = "windows", test))]
const fn win_exit_to_i32(code: u32) -> i32 {
    // A reinterpreting cast would fold `0x8000_0000..` into negative codes that
    // could alias a legitimate signed value; instead clamp anything past
    // `i32::MAX` to `i32::MAX`, which decodes to `BuildFailed`.
    #[allow(clippy::cast_possible_wrap)]
    if code <= i32::MAX as u32 {
        code as i32
    } else {
        i32::MAX
    }
}

/// Run `payload` inside a FreeBSD `jail(8)` lowered from `profile`, wait for it,
/// and return the decoded [`JailOutcome`] (FreeBSD).
///
/// The FreeBSD counterpart to the other [`build_in_jail`] arms. FreeBSD has no
/// run-jail arm today, so this introduces the returning build-jail lowering
/// directly onto a scratch-rooted `jail(2)` (established via the `jail(8)` CLI —
/// the same external-primitive shape the macOS arm uses with `sandbox-exec`, so
/// no new `unsafe` FFI is introduced). Per ADR 0051 the `jail(2)` posture is a
/// sanctioned alternative to `cap_enter` for lowering the axes:
///
/// - **network** — `ip4=disable ip6=disable` (plus `allow.raw_sockets=0`): a
///   process in the jail has no global network namespace, so an outbound socket
///   under a network-withholding profile is denied → the probe's exit-`10`
///   decodes to `Denied { network }`.
/// - **filesystem** — the jail runs as an unprivileged user (`exec.jail_user`)
///   whose only writable directory is the scratch (owned by that user); an
///   out-of-scratch write is denied → exit-`11` decodes to `Denied { filesystem }`.
///   When the filesystem axis is granted the working tree is made writable to the
///   same user so a granted effect is not false-denied.
/// - **subprocess** — a withheld subprocess axis is a genuine kernel denial of
///   process creation, not mere omission: the jail is created with a process
///   limit (`children.max=0`) so `fork`/`pdfork`/`exec` of a NEW process inside
///   the jail is denied by the kernel. Its observable is the killed child's
///   `BuildFailed` (subprocess is confined-but-not-differentially-probed — only
///   `Network`/`Filesystem` are `Denied { axis }`-nameable), a reject either way.
/// - **env** — scrubbed in the launcher (a jail does not scrub the inherited
///   environment), via the SAME allowlist the macOS/Windows arms use.
///
/// `extra_ro_binds` is unused on FreeBSD (the jail chroots an existing view of the
/// real filesystem rather than building a bind-mount namespace); it is accepted so
/// the entry has the same signature on every platform.
///
/// A jail that cannot be established (`jail` absent, a scratch that cannot be
/// created/chowned, a `jail(8)` invocation that fails to enter the jail) yields
/// [`JailOutcome::Unavailable`] — the untrusted payload is never run unconfined on
/// any path. Fail-closed. The scratch is removed on return so nothing leaks across
/// the audit's per-axis tightening loop.
///
/// The jail's actual deny behaviour is proven by the `freebsd-tier2` CI job inside
/// a `vmactions/freebsd-vm` VM (no native FreeBSD GitHub runner exists); this
/// crate builds the arm and unit-tests the pure lowering on any host.
///
/// # Errors
///
/// This function does not return `Err`: every failure to establish or run the
/// jail is folded into a non-`Clean` [`JailOutcome`].
#[cfg(target_os = "freebsd")]
#[must_use]
#[allow(clippy::doc_markdown, clippy::too_long_first_doc_paragraph)]
pub fn build_in_jail(
    _tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    _extra_ro_binds: &[PathBuf],
    payload: &[OsString],
) -> JailOutcome {
    freebsd_jail::build_in_jail(profile, scoped_tmp, working_tree, payload)
}

/// Off Linux/x86_64, macOS, Windows, and FreeBSD the returning build jail is a
/// documented refuse-gap, mirroring [`crate::run_jail::exec_in_run_jail`].
#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd"
)))]
#[must_use]
// Kept a plain `fn` (not `const fn`) so its signature matches the real
// `build_in_jail` arms, which cannot be `const` (they spawn a process).
#[allow(clippy::missing_const_for_fn)]
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
            reason: "build jail is wired only on Linux/x86_64, macOS, Windows, and FreeBSD",
        },
    }
}

/// Spawn the jail argv, wait, and decode the exit into a [`JailOutcome`].
///
/// A spawn failure is [`JailOutcome::Unavailable`] (the payload never ran); a
/// wait failure is a [`JailOutcome::BuildFailed`] (the jail ran but its result
/// is unobservable — never clean).
///
/// `env_override`, when `Some`, clears the inherited environment and sets exactly
/// the given `(name, value)` pairs — the macOS jails' `env`-axis enforcement (the
/// Linux jail scrubs env inside bwrap, so it passes `None`).
#[cfg(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos"))]
fn spawn_and_decode(
    argv: &[OsString],
    env_override: Option<&[(OsString, OsString)]>,
) -> JailOutcome {
    let Some((program, rest)) = argv.split_first() else {
        return JailOutcome::Unavailable {
            defect: RunJailDefect::Spawn {
                detail: "empty jail argv".to_owned(),
            },
        };
    };
    let mut command = std::process::Command::new(program);
    command.args(rest).stdin(std::process::Stdio::null());
    if let Some(pairs) = env_override {
        command.env_clear();
        for (name, value) in pairs {
            command.env(name, value);
        }
    }
    let child = command.spawn();
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
/// - **subprocess**: withheld (`!profile.subprocess`) ⇒ `(deny process-fork)`,
///   the NEW-process denial. Every way to create a new process
///   (`fork`/`vfork`/`posix_spawn`) forks under Seatbelt, so a fork denial
///   confines the app to a single process — mirroring the Linux jail's seccomp
///   denial of the task-creation family, and closing the `native-ffi`-via-a-
///   helper escape (a helper is a new process, so it must fork). `process-exec*`
///   is deliberately NOT denied: Seatbelt applies the profile before
///   `sandbox-exec` execs the target in place, so an exec denial would refuse
///   that mandatory initial exec and the app would never start. An in-place
///   `execve` (no fork) replaces the one jailed process rather than creating a
///   second, so it is within the single-process contract. Granted ⇒ no process
///   denial, so the allow-default base leaves spawning reachable.
///
/// The scratch and (when granted) the working tree are written as `(subpath …)`
/// allow rules so the probe's benign write and a granted filesystem effect
/// succeed; the system temp locations the shell itself uses are always allowed
/// so the jail does not false-deny the launcher.
///
/// The `env` axis is NOT enforced here: Seatbelt cannot scrub environment
/// variables, so the run/build launcher clears the environment down to the
/// profile's `env_allowlist` (mirroring the Linux jail's `--clearenv`) BEFORE
/// handing control to `sandbox-exec`. See [`macos_scrubbed_env`].
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
    s.push_str("(allow file-write* (subpath \"/private/tmp\"))\n\n");

    // Subprocess: deny NEW-process creation unless the profile grants it. On
    // macOS `sandbox-exec -f <profile> <app>` applies the profile and THEN
    // `execve`s <app> in place (no fork) — so a `(deny process-exec*)` here would
    // catch that mandatory initial exec and the app would never start. Denying
    // `process-fork` alone is the correct lowering: every way to create a NEW
    // process (`fork`/`vfork`/`posix_spawn`, all of which fork under Seatbelt)
    // requires the fork primitive, so a fork denial confines the app to a single
    // process. An `execve` that does NOT fork (an in-place image replacement)
    // stays permitted so the launcher's own initial exec of the target runs; that
    // replaces the one jailed process rather than creating a second one, so it is
    // within the single-process contract, not an escape (the Seatbelt profile
    // persists across the replacing exec, so network/filesystem stay confined for
    // the new image). This mirrors the Linux jail's seccomp denial of the
    // task-creation family, which — because Linux installs the filter AFTER the
    // initial exec — can deny the exec family outright; Seatbelt applies the
    // profile BEFORE the target exec, so only the fork axis is denied here. The
    // `native-ffi`-via-exec escape (opaque native code shelling out to a HELPER)
    // is still closed: a helper is a new process and so must fork. When granted,
    // no denial is emitted, so the allow-default base leaves spawning reachable.
    if !profile.subprocess {
        s.push_str("(deny process-fork)\n");
    }

    s
}

/// The environment the macOS launcher `exec`s `sandbox-exec` under — the `env`
/// axis's enforcement point.
///
/// Seatbelt cannot scrub environment variables, so — exactly as the Linux jail's
/// `--clearenv` + allowlisted re-export does inside `run_jail_argv` — the macOS
/// launcher clears the inherited environment down to a fixed minimal base
/// (`PATH`, `TMPDIR`) plus `LANG` (when the host sets it) plus ONLY the names in
/// `profile.env_allowlist` (when the host sets them). A name absent from the
/// host is simply not re-exported (never a placeholder); a name the profile does
/// not allow is absent from the child even if the host sets it.
///
/// PURE — it maps a host-env lookup to the child's `(name, value)` pairs — so the
/// exact scrub surface is unit-testable on any host and provable by the e2e
/// duality (`sandbox-exec` inherits exactly this env), with no second env list to
/// drift from the launcher.
///
/// `TMPDIR` is set to the always-writable scratch, matching the Linux jail (the
/// child's temp writes land in the one writable mount, not a masked host temp).
///
/// The FreeBSD build-jail arm reuses this SAME function (a jail does not scrub the
/// inherited environment either), so there is ONE launcher-side env allowlist for
/// every non-Linux arm — no second list that can drift from `profile.env_allowlist`.
#[cfg(any(target_os = "macos", target_os = "freebsd", test))]
#[must_use]
pub fn macos_scrubbed_env(
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    host_env: &dyn Fn(&str) -> Option<OsString>,
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = Vec::new();
    // The fixed minimal base, mirroring the Linux jail's re-exported allowlist.
    env.push((OsString::from("PATH"), OsString::from("/usr/bin:/bin")));
    env.push((OsString::from("TMPDIR"), scoped_tmp.as_os_str().to_owned()));
    if let Some(lang) = host_env("LANG") {
        env.push((OsString::from("LANG"), lang));
    }
    // Only the profile's declared env names re-enter, and only when the host
    // actually sets them (granted-but-unset ⇒ simply absent, never a placeholder).
    for name in &profile.env_allowlist {
        if let Some(value) = host_env(name) {
            env.push((OsString::from(name), value));
        }
    }
    env
}

/// Resolve a program name to an absolute path via `PATH`, or `None` when absent.
/// The macOS build/run jails (`sandbox-exec`) and the FreeBSD build jail (`jail`)
/// both refuse (fail-closed) when their mandatory primitive is missing, so the
/// resolver is shared.
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
pub(crate) fn find_in_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|candidate| candidate.is_file())
}

/// The FreeBSD returning build-jail arm: establish a scratch-rooted `jail(2)`
/// (via the `jail(8)` CLI) lowering the profile's axes, scrub the env in the
/// launcher, run the payload confined, wait, and decode the exit into a
/// [`JailOutcome`] — fail-closed at every establishment step.
#[cfg(target_os = "freebsd")]
mod freebsd_jail {
    use super::{JailOutcome, find_in_path, macos_scrubbed_env};
    use crate::run_jail::{FilesystemScope, RunJailDefect, SandboxProfile};
    use std::ffi::OsString;
    use std::path::Path;

    /// The unprivileged user the jailed payload runs as. Its only writable
    /// directory is the scratch (chowned to it below), so an out-of-scratch write
    /// is denied by ownership — the observable of a withheld filesystem axis.
    const JAIL_USER: &str = "nobody";

    /// Run `payload` inside a `jail(8)`-established jail lowered from `profile` and
    /// decode the outcome. See [`super::build_in_jail`] (FreeBSD) for the per-axis
    /// lowering and the fail-closed contract.
    pub(super) fn build_in_jail(
        profile: &SandboxProfile,
        scoped_tmp: &Path,
        working_tree: &Path,
        payload: &[OsString],
    ) -> JailOutcome {
        // `jail` is the mandatory primitive. Absent ⇒ refuse; the untrusted build
        // is never run unconfined.
        let Some(jail_bin) = find_in_path("jail") else {
            return JailOutcome::Unavailable {
                defect: RunJailDefect::PrimitiveUnavailable {
                    missing: vec!["jail"],
                },
            };
        };

        // A withheld subprocess axis MUST be a genuine kernel denial of process
        // creation, not mere omission (ADR 0051). `rctl(8)` with
        // `jail:NAME:maxproc:deny=1` is the jail posture that denies `fork`/
        // `pdfork`/`exec` of a new process at the kernel boundary: the rule is
        // pre-registered by jail name and the kernel enforces it when the jail is
        // created below (the documented rctl.conf pre-registration pattern). It
        // requires the `racct`/`rctl` kernel facility; if that facility is off,
        // `rctl -a` fails and this refuses (`Unavailable`) — a withheld-subprocess
        // jail that cannot deny process creation is a refuse-gap, never a silent
        // `Clean` that ran the build unconfined. When subprocess is GRANTED no such
        // rule is added, so the differentially-probed net/fs canary (which grants
        // subprocess so the probe can fork its python3/nc/rm helper) does not depend
        // on rctl at all.
        let rctl = if profile.subprocess {
            None
        } else {
            match find_in_path("rctl") {
                Some(path) => Some(path),
                None => {
                    return JailOutcome::Unavailable {
                        defect: RunJailDefect::PrimitiveUnavailable {
                            // Without rctl a withheld-subprocess jail cannot deny
                            // process creation — a refuse-gap, never an unconfined run.
                            missing: vec!["rctl"],
                        },
                    };
                }
            }
        };

        // The scratch must be writable to the unprivileged jail user; the launcher
        // (root on the FreeBSD CI VM, as jail creation requires) chowns it. A
        // failed chown refuses — a scratch the payload cannot write is a broken
        // jail, never run.
        if let Err(defect) = chown_to_jail_user(scoped_tmp) {
            return JailOutcome::Unavailable { defect };
        }
        if profile.filesystem == FilesystemScope::WorkingTreeReadWrite
            && let Err(defect) = chown_to_jail_user(working_tree)
        {
            return JailOutcome::Unavailable { defect };
        }

        let jail_name = per_run_jail_name();
        let argv = jail_argv(&jail_bin, &jail_name, profile, working_tree, payload);

        // Env scrub in the launcher (the jail does not scrub the inherited env),
        // via the SAME allowlist the macOS/Windows arms use — one env list.
        let host_env = |k: &str| std::env::var_os(k);
        let scrubbed = macos_scrubbed_env(profile, scoped_tmp, &host_env);

        // Apply the rctl process-cap rule (withheld subprocess only) BEFORE the
        // jail runs, then remove it on return regardless of outcome so nothing
        // leaks across the audit's per-axis tightening loop.
        let _rctl_guard = match &rctl {
            Some(rctl_bin) => match RctlRule::apply(rctl_bin, &jail_name) {
                Ok(guard) => Some(guard),
                Err(defect) => return JailOutcome::Unavailable { defect },
            },
            None => None,
        };

        let outcome = spawn_and_decode(&argv, &scrubbed);
        // The jail is `persist=0`, so it is torn down when the payload exits; the
        // scratch chown is inert. Nothing else to release beyond the rctl guard.
        outcome
    }

    /// Build the `jail -c … command=<payload>` argv lowering the profile's axes.
    ///
    /// - `path=/` chroots the jail to the live root (read-only to the
    ///   unprivileged user except the chowned scratch/working tree).
    /// - `ip4=disable ip6=disable allow.raw_sockets=0` withhold the network
    ///   namespace unconditionally when the axis is absent; when granted, the host
    ///   network is inherited.
    /// - `exec.jail_user=nobody` runs the payload unprivileged so an out-of-scratch
    ///   write is denied by ownership.
    /// - `persist=0` tears the jail down when the payload exits (no orphan jail).
    fn jail_argv(
        jail_bin: &Path,
        jail_name: &str,
        profile: &SandboxProfile,
        working_tree: &Path,
        payload: &[OsString],
    ) -> Vec<OsString> {
        let mut argv: Vec<OsString> = Vec::new();
        argv.push(jail_bin.as_os_str().to_owned());
        argv.push(OsString::from("-c"));
        argv.push(OsString::from(format!("name={jail_name}")));
        argv.push(OsString::from("path=/"));
        argv.push(OsString::from("host.hostname=ipe-tier2-jail"));
        // Network: withheld ⇒ no IPv4/IPv6/raw sockets at all (a socket is denied
        // → the probe's exit-10 decodes to Denied { network }). Granted ⇒ inherit
        // the host network so a granted effect is not false-denied.
        if profile.network {
            argv.push(OsString::from("ip4=inherit"));
            argv.push(OsString::from("ip6=inherit"));
        } else {
            argv.push(OsString::from("ip4=disable"));
            argv.push(OsString::from("ip6=disable"));
            argv.push(OsString::from("allow.raw_sockets=0"));
        }
        argv.push(OsString::from("allow.sysvipc=0"));
        argv.push(OsString::from("persist=0"));
        argv.push(OsString::from(format!("exec.jail_user={JAIL_USER}")));
        // The working tree is made writable to the jail user (chowned by the
        // caller) only when the filesystem axis is granted, so a granted effect is
        // not false-denied; when withheld it stays owned by the launcher and the
        // unprivileged payload cannot write it. No extra jail parameter is needed —
        // the ownership on the chrooted path is the boundary.
        let _ = working_tree;
        // The command: `jail(8)`'s `command` parameter collects EVERYTHING on the
        // command line following it as the argv to `execvp` inside the jail — one
        // command-line argument per argv slot, with NO shell and NO quote removal.
        // So the payload's first token rides on `command=<tok0>` and every
        // remaining token is pushed as its OWN separate argv element to the `jail`
        // process. Each stays one OS-level argument end to end (Rust hands each
        // `arg` to `jail` as a distinct `execve` slot, and `jail` forwards them as
        // distinct slots to the payload), so a scratch, working-tree, or
        // `CARGO_HOME` path bearing whitespace survives byte-for-byte with no
        // quoting and no re-split — there is no single space-joined string for
        // `jail` to mis-tokenise, and no shell to interpret metacharacters.
        let mut command = OsString::from("command=");
        let Some((first, rest)) = payload.split_first() else {
            // An empty payload cannot be run; the caller decodes the resulting
            // spawn/exec failure as a non-clean outcome (never a silent Clean).
            argv.push(command);
            return argv;
        };
        command.push(first);
        argv.push(command);
        for tok in rest {
            argv.push(tok.clone());
        }
        argv
    }

    /// Chown `path` to the unprivileged jail user so the confined payload can write
    /// its scratch. Uses the `chown(8)` CLI (no new `unsafe` FFI); a failure refuses.
    fn chown_to_jail_user(path: &Path) -> Result<(), RunJailDefect> {
        let Some(chown) = find_in_path("chown") else {
            return Err(RunJailDefect::PrimitiveUnavailable {
                missing: vec!["chown"],
            });
        };
        let status = std::process::Command::new(chown)
            .arg(JAIL_USER)
            .arg(path)
            .status();
        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(RunJailDefect::Spawn {
                detail: format!("chown {} to {JAIL_USER} failed ({s})", path.display()),
            }),
            Err(e) => Err(RunJailDefect::Spawn {
                detail: format!("could not run chown for {}: {e}", path.display()),
            }),
        }
    }

    /// A per-run jail name unique enough that concurrent audit calls do not collide.
    fn per_run_jail_name() -> String {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("ipe_tier2_{pid}_{nanos}")
    }

    /// An `rctl(8)` process-cap rule (`jail:NAME:maxproc:deny=1`) applied for a
    /// withheld-subprocess jail and removed on drop, so a withheld subprocess axis
    /// is a genuine kernel denial of process creation and no rule leaks across the
    /// audit loop.
    struct RctlRule {
        rctl_bin: std::path::PathBuf,
        rule: String,
    }

    impl RctlRule {
        fn apply(rctl_bin: &Path, jail_name: &str) -> Result<Self, RunJailDefect> {
            let rule = format!("jail:{jail_name}:maxproc:deny=1");
            let status = std::process::Command::new(rctl_bin)
                .arg("-a")
                .arg(&rule)
                .status();
            match status {
                Ok(s) if s.success() => Ok(Self {
                    rctl_bin: rctl_bin.to_path_buf(),
                    rule,
                }),
                Ok(s) => Err(RunJailDefect::Spawn {
                    detail: format!(
                        "rctl could not deny process creation for the withheld-subprocess jail \
                         ({s}); refusing to run without a kernel process-creation denial"
                    ),
                }),
                Err(e) => Err(RunJailDefect::Spawn {
                    detail: format!("could not run rctl to deny process creation: {e}"),
                }),
            }
        }
    }

    impl Drop for RctlRule {
        fn drop(&mut self) {
            // Best-effort removal of the rule this guard added; a leftover rule on a
            // torn-down (`persist=0`) jail is inert, but remove it so the audit loop
            // leaves no residue.
            let _ = std::process::Command::new(&self.rctl_bin)
                .arg("-r")
                .arg(&self.rule)
                .status();
        }
    }

    /// Spawn the jail argv with the scrubbed environment, wait, and decode the exit
    /// into a [`JailOutcome`]. A spawn failure is `Unavailable` (the payload never
    /// ran); a wait failure is `BuildFailed` (the jail ran but its result is
    /// unobservable — never clean); the exit code decodes through the SAME
    /// [`JailOutcome::decode`] the other arms use.
    fn spawn_and_decode(argv: &[OsString], scrubbed: &[(OsString, OsString)]) -> JailOutcome {
        let Some((program, rest)) = argv.split_first() else {
            return JailOutcome::Unavailable {
                defect: RunJailDefect::Spawn {
                    detail: "empty jail argv".to_owned(),
                },
            };
        };
        let mut command = std::process::Command::new(program);
        command.args(rest).stdin(std::process::Stdio::null());
        command.env_clear();
        for (name, value) in scrubbed {
            command.env(name, value);
        }
        let child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                return JailOutcome::Unavailable {
                    defect: RunJailDefect::Spawn {
                        detail: e.to_string(),
                    },
                };
            }
        };
        let mut child = child;
        match child.wait() {
            Ok(status) => JailOutcome::decode(status.code()),
            Err(e) => JailOutcome::BuildFailed {
                reason: format!("could not await the jailed probe: {e}"),
            },
        }
    }
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

    fn with_subprocess(subprocess: bool) -> SandboxProfile {
        SandboxProfile {
            subprocess,
            ..SandboxProfile::maximally_isolated()
        }
    }

    #[test]
    fn sbpl_denies_process_creation_when_the_subprocess_axis_is_withheld() {
        // Withheld subprocess ⇒ the NEW-process denial (`process-fork`) is present,
        // mirroring the Linux seccomp denial of the task-creation family and
        // closing the native-ffi-via-a-helper escape (a helper is a new process, so
        // it must fork). `process-exec*` is deliberately NOT denied: Seatbelt
        // applies the profile before `sandbox-exec` execs the target in place, so an
        // exec denial would refuse that mandatory initial exec and the app would
        // never start (see `sbpl_from_profile`). A fork denial alone confines the
        // app to a single process while permitting the launcher's initial exec.
        let sbpl = sbpl_from_profile(
            &with_subprocess(false),
            Path::new("/tmp/scratch"),
            Path::new("/work/tree"),
        );
        assert!(sbpl.contains("(deny process-fork)"), "{sbpl}");
        assert!(
            !sbpl.contains("(deny process-exec"),
            "the initial target exec must stay permitted (Seatbelt applies the \
             profile before exec'ing the target): {sbpl}"
        );
    }

    #[test]
    fn sbpl_allows_process_creation_when_the_subprocess_axis_is_granted() {
        // Granted subprocess ⇒ NO process denial, so the allow-default base
        // leaves spawning reachable (no false-deny of a granted axis).
        let sbpl = sbpl_from_profile(
            &with_subprocess(true),
            Path::new("/tmp/scratch"),
            Path::new("/work/tree"),
        );
        assert!(
            !sbpl.contains("(deny process-exec"),
            "subprocess granted must emit no process-exec denial: {sbpl}"
        );
        assert!(
            !sbpl.contains("(deny process-fork)"),
            "subprocess granted must emit no process-fork denial: {sbpl}"
        );
    }

    #[test]
    fn maximally_isolated_sbpl_denies_all_four_confinement_axes() {
        // The floor enforces every runtime-enforced axis Seatbelt CAN enforce:
        // network, filesystem, subprocess. (Env is enforced by the launcher, not
        // the SBPL — see `macos_scrubbed_env`.) This is the `Holds`-is-honest
        // invariant: `Holds` may only claim axes the jail actually confines. The
        // subprocess axis is the NEW-process denial (`process-fork`); the initial
        // target exec must stay permitted, so `process-exec*` is NOT denied (see
        // `sbpl_from_profile`).
        let sbpl = sbpl_from_profile(
            &SandboxProfile::maximally_isolated(),
            Path::new("/s"),
            Path::new("/w"),
        );
        assert!(sbpl.contains("(deny network*)"), "network: {sbpl}");
        assert!(sbpl.contains("(deny file-write*)"), "filesystem: {sbpl}");
        assert!(
            sbpl.contains("(deny process-fork)"),
            "subprocess fork: {sbpl}"
        );
        assert!(
            !sbpl.contains("(deny process-exec"),
            "the initial target exec must stay permitted: {sbpl}"
        );
    }

    // ── the macOS env-axis launcher scrub (pure — runs on any host) ───────────

    fn env_map(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, OsString> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(*v)))
            .collect()
    }

    #[test]
    fn scrubbed_env_keeps_only_the_base_when_no_env_axis_is_granted() {
        // An env-withholding profile re-exports only the fixed base (PATH, TMPDIR,
        // and LANG when the host sets it); every other host var is dropped.
        let host = env_map(&[("SECRET", "leak"), ("LANG", "en_US.UTF-8")]);
        let lookup = |k: &str| host.get(k).cloned();
        let env = macos_scrubbed_env(
            &SandboxProfile::maximally_isolated(),
            Path::new("/tmp/scratch"),
            &lookup,
        );
        let names: Vec<&str> = env
            .iter()
            .map(|(n, _)| n.to_str().unwrap_or_default())
            .collect();
        assert!(names.contains(&"PATH"), "{names:?}");
        assert!(names.contains(&"TMPDIR"), "{names:?}");
        assert!(names.contains(&"LANG"), "{names:?}");
        assert!(
            !names.contains(&"SECRET"),
            "a non-allowlisted host var must be dropped: {names:?}"
        );
        // TMPDIR is the scratch, matching the Linux jail.
        let tmpdir = env
            .iter()
            .find(|(n, _)| n == "TMPDIR")
            .map(|(_, v)| v.clone());
        assert_eq!(tmpdir, Some(OsString::from("/tmp/scratch")));
    }

    #[test]
    fn scrubbed_env_re_exports_only_allowlisted_names_that_the_host_sets() {
        // A granted env name that the host sets re-enters; a granted name the host
        // does NOT set is simply absent (never a placeholder); a host var not on
        // the allowlist stays dropped.
        let host = env_map(&[("ALLOWED", "yes"), ("SECRET", "leak")]);
        let lookup = |k: &str| host.get(k).cloned();
        let profile = SandboxProfile {
            env_allowlist: vec!["ALLOWED".to_owned(), "GRANTED_BUT_UNSET".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        let env = macos_scrubbed_env(&profile, Path::new("/s"), &lookup);
        let by_name: std::collections::HashMap<String, OsString> = env
            .iter()
            .map(|(n, v)| (n.to_string_lossy().into_owned(), v.clone()))
            .collect();
        assert_eq!(by_name.get("ALLOWED"), Some(&OsString::from("yes")));
        assert!(
            !by_name.contains_key("GRANTED_BUT_UNSET"),
            "a granted-but-unset name must be absent, not a placeholder"
        );
        assert!(
            !by_name.contains_key("SECRET"),
            "a non-allowlisted host var must be dropped even when the host sets it"
        );
    }

    // ── the Windows exit-code mapping (pure — runs on any host) ───────────────
    //
    // The Windows arm reads a `u32` from `GetExitCodeProcess` and decodes it
    // through the SAME `JailOutcome::decode` the other arms use. These prove the
    // `u32 -> i32` bridge keeps the wrapper-owned per-axis contract intact and
    // fails closed on an out-of-range code.

    #[test]
    fn windows_exit_maps_the_per_axis_codes_identically() {
        // The wrapper-owned codes (clean/build-failed/network/filesystem) round-trip
        // unchanged, so the Windows arm decodes them exactly as the Unix arms do.
        for code in [
            AXIS_EXIT_CLEAN,
            TIER2_EXIT_BUILD_FAILED,
            AXIS_EXIT_NETWORK,
            AXIS_EXIT_FILESYSTEM,
        ] {
            let mapped = win_exit_to_i32(u32::try_from(code).expect("small code"));
            assert_eq!(mapped, code, "code {code} must round-trip");
        }
    }

    #[test]
    fn windows_clean_exit_decodes_to_clean_and_only_clean() {
        assert_eq!(
            JailOutcome::decode(Some(win_exit_to_i32(0))),
            JailOutcome::Clean
        );
        // A network / filesystem denial names its axis after the u32 bridge.
        assert_eq!(
            JailOutcome::decode(Some(win_exit_to_i32(10))),
            JailOutcome::Denied {
                axis: CapabilityAxis::Network
            }
        );
        assert_eq!(
            JailOutcome::decode(Some(win_exit_to_i32(11))),
            JailOutcome::Denied {
                axis: CapabilityAxis::Filesystem
            }
        );
    }

    #[test]
    fn windows_high_bit_exit_is_build_failed_never_clean_or_denied() {
        // A high-bit exit code (a process that exited with e.g. 0xC000_0005) must
        // never alias a per-axis code or the clean 0 — it saturates to a value that
        // decodes to BuildFailed. Fail-closed: ambiguity can never admit.
        for code in [0x8000_0000u32, 0xC000_0005u32, u32::MAX] {
            let outcome = JailOutcome::decode(Some(win_exit_to_i32(code)));
            assert!(
                matches!(outcome, JailOutcome::BuildFailed { .. }),
                "high-bit exit {code:#x} must decode to BuildFailed, got {outcome:?}"
            );
            assert!(!outcome.is_clean());
        }
    }

    // The FreeBSD `jail_argv` builder lives behind `cfg(target_os = "freebsd")`, so
    // its exact argv cannot be asserted off FreeBSD. What is portable — and is the
    // property the CI failure hinged on — is that the payload is threaded to
    // `jail(8)` as ONE OS-level argv element per token (no space-joined string, no
    // quoting): `command=<tok0>` plus each remaining token as its own element. This
    // pure helper models exactly that mapping so the space-bearing-path invariant is
    // provable on any host; `jail_argv` calls the same shape.
    fn jail_command_args(payload: &[std::ffi::OsString]) -> Vec<std::ffi::OsString> {
        let mut out: Vec<std::ffi::OsString> = Vec::new();
        let mut command = std::ffi::OsString::from("command=");
        let Some((first, rest)) = payload.split_first() else {
            out.push(command);
            return out;
        };
        command.push(first);
        out.push(command);
        for tok in rest {
            out.push(tok.clone());
        }
        out
    }

    #[test]
    fn each_payload_token_is_its_own_jail_command_arg_intact() {
        use std::ffi::OsString;

        // `jail(8)` execvp's `command` with NO shell and NO quote removal — one
        // command-line argument per argv slot. A path with a space, an embedded
        // single quote, and a shell metacharacter must each stay exactly one intact
        // argv element (the space must NOT split a path, the quote/metachar must NOT
        // be interpreted), which passing each token as its own OS-level argument
        // guarantees.
        let payload = [
            OsString::from("/usr/bin/env"),
            OsString::from("PROBE_MODE=tier2"),
            OsString::from("/tmp/dir with space/x"),
            OsString::from("it's a $HOME; rm -rf /"),
        ];
        // The first token rides on `command=`; every other token — including the
        // space-bearing path (must NOT split) and the metacharacter-bearing token
        // (must NOT be interpreted) — is its OWN intact argv element.
        let expected = vec![
            OsString::from("command=/usr/bin/env"),
            OsString::from("PROBE_MODE=tier2"),
            OsString::from("/tmp/dir with space/x"),
            OsString::from("it's a $HOME; rm -rf /"),
        ];
        assert_eq!(jail_command_args(&payload), expected);
    }

    #[test]
    fn an_empty_jail_payload_yields_a_bare_command_arg() {
        // An empty payload produces just `command=` (no program); the caller decodes
        // the resulting exec failure as a non-clean outcome, never a silent Clean.
        let args = jail_command_args(&[]);
        assert_eq!(args, vec![std::ffi::OsString::from("command=")]);
    }
}

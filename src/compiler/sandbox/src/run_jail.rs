//! The runtime jail around the *emitted app* (as opposed to the build-time jail
//! around an untrusted crate compile in [`crate`]).
//!
//! A program's capability set is inferred from pure Ipê and declared for native
//! `Rust.` code, but nothing confines the emitted binary when it *runs*: a Tier
//! 2 wrapper is arbitrary native Rust that can make any syscall. This module is
//! the fail-closed jail that runs the binary confined to exactly its
//! declared-plus-inferred set — declared effects work, undeclared ones are
//! impossible.
//!
//! The pipeline is: a [`Capability`] set → a platform-independent
//! [`SandboxProfile`] ([`profile_from_capabilities`]) → a `bwrap` argv + a
//! seccomp program ([`run_jail_argv`] + [`crate::seccomp`]). The profile is the
//! serializable value the built artifact carries (`ipe.profile`); the argv is
//! what a launcher execs.
//!
//! ## Invariants
//!
//! - **Deny-by-default, structurally.** [`profile_from_capabilities`] is an
//!   exhaustive `match Capability` with no `_` catch-all, so a newly-added
//!   capability variant fails to *compile* until it is classified — an
//!   unclassified variant can never default to "allowed". [`SandboxProfile`] has
//!   no `Default` that yields an all-allowed value: the empty set lowers to the
//!   maximally-isolated profile.
//! - **Fail-closed on an unknown database driver.** `database` lowers to
//!   `network` or `filesystem` per the driver; an unknown/missing driver is an
//!   error, never a silently-dropped axis.
//! - **Reuse the mechanism, not the numbers.** The argv reuses the build jail's
//!   `bwrap`/`prlimit` flag vocabulary but defines its own [`RunResourceLimits`]
//!   (no wall-clock kill by default — a long-lived server is legitimate) and
//!   adds the baseline denials the build argv lacks (`--proc /proc`,
//!   `no_new_privs`, the seccomp filter).

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ipe_diagnostics::{Code, IPE_F4413};
use ipe_kernels::Capability;

use crate::seccomp;

// ── the database axis (a run-jail input, resolved from ipe.toml) ─────────────

/// How `Capability::Database` lowers for this project — the driver decides
/// whether a database effect is really a network effect (a TCP driver) or a
/// filesystem effect (an embedded/SQLite file). Resolved by the CLI from the
/// `ipe.toml` driver selection before the profile is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseAxis {
    /// A TCP-connected database (Postgres, MySQL, …) → the `network` control.
    Network,
    /// A file-backed database (SQLite, an embedded store) → the `filesystem`
    /// control.
    Filesystem,
    /// No `database` capability is present, so the driver is irrelevant. Kept
    /// distinct from a *missing* driver: the caller supplies this only when the
    /// set does not include `database`, so an unknown driver where one is needed
    /// is still an error.
    NotApplicable,
}

/// Why a capability set could not be lowered to a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    /// The set includes `database` but the `ipe.toml` driver could not be
    /// resolved to a concrete axis. Fail-closed: dropping the axis would
    /// *tighten* the jail past what the program needs (a false-deny), and
    /// guessing an axis could under-isolate — so this refuses.
    UnknownDatabaseDriver,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownDatabaseDriver => write!(
                f,
                "the program uses `database`, but the ipe.toml database driver could not be \
                 resolved to a concrete axis (network or filesystem); refusing to build a jail \
                 that might over- or under-isolate it"
            ),
        }
    }
}

impl std::error::Error for ProfileError {}

// ── the platform-independent profile ────────────────────────────────────────

/// The filesystem scope a program runs under.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FilesystemScope {
    /// `filesystem` absent: `/` read-only, home/tmp masked, exactly one scoped
    /// writable tempdir. The maximally-isolated filesystem view.
    Isolated,
    /// `filesystem` granted: the working tree is bound read-write (still coarse
    /// — any path under it — per the first-cut map).
    WorkingTreeReadWrite,
}

/// The resource caps for a run-jailed app — distinct from the build jail's
/// `ResourceLimits`, whose values (10 GiB AS, 900 s wall/CPU) are tuned to kill
/// a giant one-shot rustdoc and would wrongly kill a legitimate long-lived
/// server (a false-deny).
///
/// The mechanism (`prlimit` + optional `timeout`) is reused; the numbers are
/// not. `wall_secs = None` means no wall-clock kill (a server runs
/// indefinitely); a runaway is still bounded by the CPU/memory/proc caps, which
/// stay mandatory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunResourceLimits {
    /// Address-space cap in bytes.
    pub as_bytes: u64,
    /// CPU-seconds cap (bounds a busy-loop even without a wall clock).
    pub cpu_secs: u64,
    /// Wall-clock cap in seconds, or `None` for no wall-clock kill.
    pub wall_secs: Option<u64>,
    /// Open-file-descriptor cap.
    pub fd_cap: u64,
    /// Process-count cap.
    pub proc_cap: u64,
}

impl Default for RunResourceLimits {
    fn default() -> Self {
        // A long-lived app, not a one-shot build: no wall clock, a generous CPU
        // ceiling (a runaway loop is still bounded, a slow server is not), a
        // large-but-finite address space, and sane FD/process caps.
        Self {
            as_bytes: 4 * 1024 * 1024 * 1024,
            cpu_secs: 86_400,
            wall_secs: None,
            fd_cap: 4096,
            proc_cap: 512,
        }
    }
}

/// The platform-independent description of a run jail: what the emitted app may
/// touch, derived from its capability set. A per-platform *builder*
/// ([`run_jail_argv`] on Linux) turns this into a concrete jail.
///
/// This is the value serialized into an artifact's `ipe.profile`. It is
/// deliberately NOT `Default`-constructible to an all-allowed value — the empty
/// capability set lowers to the maximally-isolated profile
/// ([`profile_from_capabilities`]), and a profile is only ever *built from a
/// set*, never conjured permissive.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SandboxProfile {
    /// `true` iff `network` is granted — the net namespace is shared with the
    /// host. `false` → a fresh empty net namespace (loopback only).
    pub network: bool,
    /// The filesystem scope.
    pub filesystem: FilesystemScope,
    /// The environment variables re-exported into the scrubbed env (the `env`
    /// axis's allowlist). Empty when `env` is absent.
    pub env_allowlist: Vec<String>,
    /// `true` iff `subprocess` is granted — the seccomp filter permits the
    /// task-creation family. `false` → the create family is denied (threads
    /// excepted).
    pub subprocess: bool,
    /// The resource caps.
    pub limits: RunResourceLimits,
}

impl SandboxProfile {
    /// The maximally-isolated profile: nothing granted. This is what the empty
    /// capability set lowers to, and the floor a tampered `ipe.profile` cannot
    /// go below undetected.
    #[must_use]
    pub fn maximally_isolated() -> Self {
        Self {
            network: false,
            filesystem: FilesystemScope::Isolated,
            env_allowlist: Vec::new(),
            subprocess: false,
            limits: RunResourceLimits::default(),
        }
    }

    /// Whether `self` isolates *at least* as much as `floor` on every axis — the
    /// launcher's tamper check. A profile that grants an axis the floor does not
    /// is weaker and MUST be refused (a doctored `ipe.profile` cannot widen the
    /// jail below what the binary was built for).
    ///
    /// "At least as isolated" is per-axis: `self` may not grant `network`,
    /// `subprocess`, a wider filesystem, or any env var the `floor` does not
    /// also grant. (Resource limits are not a confinement axis and are not
    /// compared here.)
    #[must_use]
    pub fn is_at_least_as_isolated_as(&self, floor: &Self) -> bool {
        let network_ok = !self.network || floor.network;
        let subprocess_ok = !self.subprocess || floor.subprocess;
        let fs_ok = match (&self.filesystem, &floor.filesystem) {
            (FilesystemScope::Isolated, _) => true,
            (FilesystemScope::WorkingTreeReadWrite, FilesystemScope::WorkingTreeReadWrite) => true,
            (FilesystemScope::WorkingTreeReadWrite, FilesystemScope::Isolated) => false,
        };
        // Every env var self grants must be in the floor's allowlist.
        let env_ok = self
            .env_allowlist
            .iter()
            .all(|v| floor.env_allowlist.contains(v));
        network_ok && subprocess_ok && fs_ok && env_ok
    }
}

/// Lower a capability set to a [`SandboxProfile`].
///
/// The profile set is `inferred ∪ declared` — the union guarantees *no
/// false-deny*: every capability anyone claims the program has is relaxed, so a
/// legitimately-declared effect is never blocked. `db_axis` resolves `database`
/// to a concrete axis (see [`DatabaseAxis`]); it is consulted only when the set
/// includes `database`.
///
/// The `match Capability` is **exhaustive with no `_`**: a new capability
/// variant fails to compile here until classified, so it can never silently
/// default to "allowed".
///
/// # Errors
///
/// [`ProfileError::UnknownDatabaseDriver`] when the set includes `database` but
/// `db_axis` is [`DatabaseAxis::NotApplicable`] (the driver could not be
/// resolved) — fail-closed rather than drop or mis-lower the axis.
pub fn profile_from_capabilities(
    inferred: &BTreeSet<Capability>,
    declared: &BTreeSet<Capability>,
    db_axis: DatabaseAxis,
    env_allowlist: &[String],
) -> Result<SandboxProfile, ProfileError> {
    let mut profile = SandboxProfile::maximally_isolated();

    // The union is the authoritative set (no false-deny).
    let union: BTreeSet<Capability> = inferred.union(declared).copied().collect();

    for &cap in &union {
        match cap {
            Capability::Network => profile.network = true,
            Capability::Filesystem => {
                profile.filesystem = FilesystemScope::WorkingTreeReadWrite;
            }
            Capability::Database => match db_axis {
                DatabaseAxis::Network => profile.network = true,
                DatabaseAxis::Filesystem => {
                    profile.filesystem = FilesystemScope::WorkingTreeReadWrite;
                }
                DatabaseAxis::NotApplicable => return Err(ProfileError::UnknownDatabaseDriver),
            },
            Capability::Env => {
                // The env axis is granted: the manifest-named variables re-enter
                // the scrubbed environment. An empty allowlist here still means
                // "env granted but nothing named to re-export" — the axis is on,
                // the set of exported names is just empty.
                profile.env_allowlist = env_allowlist.to_vec();
            }
            Capability::Subprocess => profile.subprocess = true,
            // `clock`/`random` carry no OS control in the first cut: denying
            // time or RNG syscalls breaks far more than it contains, and neither
            // is a high-value exfiltration axis. Explicit no-op arms, not a
            // catch-all, so a new variant is still forced to be classified.
            Capability::Clock | Capability::Random => {}
            // `native-ffi` widens nothing by itself — its role is epistemic (it
            // means inference is blind, so the declared set is the ceiling). It
            // opens no control here; an explicit no-op arm.
            Capability::NativeFfi => {}
        }
    }

    Ok(profile)
}

// ── the Linux jail argv builder ─────────────────────────────────────────────

/// The paths of the host tools the run jail needs. `bwrap` and `prlimit` are
/// mandatory; `timeout` is only needed when the profile sets a wall-clock cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunJailTools {
    /// `bwrap` — the namespaces + `--seccomp` loader.
    pub bwrap: PathBuf,
    /// `prlimit` — the resource caps (mandatory).
    pub prlimit: PathBuf,
    /// `timeout` — the wall clock, needed only when `limits.wall_secs` is set.
    pub timeout: Option<PathBuf>,
}

/// Build the `bwrap` argv that runs `payload` under the jail `profile`
/// describes.
///
/// Pure — no process is spawned — so the exact isolation surface is
/// unit-testable, exactly like the build jail's `bwrap_argv`. This does NOT
/// share code with `bwrap_argv`: that builder's non-optional `timeout` prefix is
/// load-bearing for the *build* jail (an untrusted compile must always have a
/// wall clock), whereas the run jail wants no wall clock for a long-lived
/// server. Reusing the *flag vocabulary* is deliberate; sharing the *builder*
/// would couple two different resource-limit policies.
///
/// `scoped_tmp` is the one writable tempdir (used both as the `Isolated`
/// filesystem's sole writable mount and as `TMPDIR`). `working_tree` is bound
/// read-write only under [`FilesystemScope::WorkingTreeReadWrite`].
/// `seccomp_fd` is the file-descriptor number the caller has arranged to carry
/// the compiled seccomp program (passed to `bwrap --seccomp <fd>`); `None` means
/// no filter is attached (the caller must have refused already if a filter was
/// required).
///
/// The env is scrubbed with `--clearenv`; only the fixed minimal allowlist
/// (`PATH`, `TMPDIR`, `LANG`) plus the profile's `env_allowlist` re-enter. There
/// is NO shell token anywhere in the result.
#[must_use]
pub fn run_jail_argv(
    tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    seccomp_fd: Option<i32>,
    host_env: &dyn Fn(&str) -> Option<OsString>,
    payload: &[OsString],
) -> Vec<OsString> {
    let mut argv: Vec<OsString> = Vec::new();

    // Optional wall clock (only when the profile sets one AND `timeout` is
    // present; the caller refuses a wall-clock profile with no `timeout`).
    if let (Some(wall), Some(timeout)) = (profile.limits.wall_secs, &tools.timeout) {
        argv.push(timeout.clone().into());
        argv.push("--kill-after=5s".into());
        argv.push(wall.to_string().into());
    }

    argv.push(tools.bwrap.clone().into());

    // The network namespace is unshared UNLESS `network` is granted. IPC/UTS/
    // cgroup are unconditionally unshared (SysV shmem / abstract sockets are
    // covert channels that ride IPC independent of the network axis). PID is
    // unconditionally unshared (no host-PID visibility even when subprocess is
    // granted).
    if !profile.network {
        argv.push("--unshare-net".into());
    }
    for flag in [
        "--unshare-pid",
        "--unshare-uts",
        "--unshare-ipc",
        "--unshare-cgroup",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
    ] {
        argv.push(flag.into());
    }

    // Read-only root, then a FRESH proc mask OVER it. Order matters: the
    // `--ro-bind / /` exposes the host `/proc` (which leaks sibling env via
    // `/proc/<pid>/environ`, defeating `--clearenv`, and is a user-namespace
    // escape lever); the later `--proc /proc` masks it. bwrap applies mount ops
    // in argv order, so proc-after-robind wins.
    argv.push("--ro-bind".into());
    argv.push("/".into());
    argv.push("/".into());
    argv.push("--proc".into());
    argv.push("/proc".into());
    // A fresh minimal devtmpfs (the ro-bound host `/dev` nodes carry no device
    // permissions inside the user namespace).
    argv.push("--dev".into());
    argv.push("/dev".into());
    // Mask the home/tmp trees.
    for tmpfs in ["/home", "/root", "/tmp"] {
        argv.push("--tmpfs".into());
        argv.push(tmpfs.into());
    }

    // The one writable mount (always), and the working tree read-write only
    // when the filesystem axis is granted.
    argv.push("--bind".into());
    argv.push(scoped_tmp.into());
    argv.push(scoped_tmp.into());
    if profile.filesystem == FilesystemScope::WorkingTreeReadWrite {
        argv.push("--bind".into());
        argv.push(working_tree.into());
        argv.push(working_tree.into());
        argv.push("--chdir".into());
        argv.push(working_tree.into());
    } else {
        argv.push("--chdir".into());
        argv.push(scoped_tmp.into());
    }

    // The seccomp filter (subprocess denial + baseline denials). Attached via a
    // pre-arranged fd. `no_new_privs` is set by bubblewrap by default (it always
    // calls `PR_SET_NO_NEW_PRIVS` unless `--cap-add` is used, which the run jail
    // never does), so the "no privilege gain" claim is mechanical.
    if let Some(fd) = seccomp_fd {
        argv.push("--seccomp".into());
        argv.push(fd.to_string().into());
    }

    // Scrubbed env: the fixed minimal allowlist, then the profile's declared
    // env names re-exported from the host (only when `env` was granted). A named
    // var absent from the host is simply not re-exported (never a placeholder).
    argv.push("--setenv".into());
    argv.push("PATH".into());
    argv.push("/usr/bin:/bin".into());
    argv.push("--setenv".into());
    argv.push("TMPDIR".into());
    argv.push(scoped_tmp.into());
    if let Some(lang) = host_env("LANG") {
        argv.push("--setenv".into());
        argv.push("LANG".into());
        argv.push(lang);
    }
    for name in &profile.env_allowlist {
        if let Some(value) = host_env(name) {
            argv.push("--setenv".into());
            argv.push(name.into());
            argv.push(value);
        }
    }

    // Resource caps via prlimit, then the payload with NO shell. The wall clock
    // (if any) is the outer `timeout`; prlimit bounds AS/CPU/FDs/procs.
    argv.push("--".into());
    argv.push(tools.prlimit.clone().into());
    argv.push(format!("--as={}", profile.limits.as_bytes).into());
    argv.push(format!("--cpu={}", profile.limits.cpu_secs).into());
    argv.push(format!("--nofile={}", profile.limits.fd_cap).into());
    argv.push(format!("--nproc={}", profile.limits.proc_cap).into());
    argv.push("--".into());
    argv.extend(payload.iter().cloned());
    argv
}

// ── refusal + the fail-closed platform decision ─────────────────────────────

/// Why the run jail could not be established, or refused to run. The whole
/// family carries the [`IPE_F4413`] taxonomy code, the run-jail sibling of the
/// build jail's [`crate::SandboxDefect`] / `IPE-F4410`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunJailDefect {
    /// A required jail primitive is absent on this host (`bwrap`/`prlimit`, or a
    /// wall-clock profile with no `timeout`). Fail-closed: refuse, never run
    /// unconfined.
    PrimitiveUnavailable {
        /// The missing primitive name(s).
        missing: Vec<&'static str>,
    },
    /// No sound jail can be built on this platform (not Linux, or seccomp cannot
    /// be compiled for this architecture). The documented refuse-gap: a
    /// non-empty native/high-value set refuses to run here.
    UnsupportedPlatform {
        /// A short reason for the diagnostic.
        reason: &'static str,
    },
    /// The capability profile could not be built (see [`ProfileError`]).
    Profile(ProfileError),
    /// The jailed process could not be spawned.
    Spawn {
        /// The rendered OS error.
        detail: String,
    },
    /// A tampered `ipe.profile` requested *less* isolation than the capability
    /// floor embedded in the binary. Refuse — a weaker profile cannot widen the
    /// jail below what the binary was built for.
    ProfileWeakerThanFloor,
}

impl RunJailDefect {
    /// The stable taxonomy code (`IPE-F4413` for the whole family).
    #[must_use]
    pub const fn code(&self) -> Code {
        IPE_F4413
    }
}

impl std::fmt::Display for RunJailDefect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = self.code().as_str();
        match self {
            Self::PrimitiveUnavailable { missing } => write!(
                f,
                "{code}: cannot establish a runtime jail around the app — missing {} — refusing \
                 to run a capability-bearing program unconfined; install bubblewrap (bwrap) and \
                 util-linux (prlimit)",
                missing.join(", ")
            ),
            Self::UnsupportedPlatform { reason } => write!(
                f,
                "{code}: no runtime jail can be built on this platform ({reason}); refusing to run \
                 a native-capability program unconfined"
            ),
            Self::Profile(e) => write!(f, "{code}: {e}"),
            Self::Spawn { detail } => {
                write!(f, "{code}: failed to spawn the jailed app: {detail}")
            }
            Self::ProfileWeakerThanFloor => write!(
                f,
                "{code}: the artifact's ipe.profile requests less isolation than the capability \
                 floor embedded in the binary — refusing to run under a weakened profile"
            ),
        }
    }
}

impl std::error::Error for RunJailDefect {}

/// Whether the resolved capability set is entirely on the low-value axes
/// (`clock`/`random`) or empty — the only case the narrow `IPE_ALLOW_UNSANDBOXED`
/// override may downgrade to a warning. Any high-value native axis (network,
/// filesystem, database, env, subprocess, native-ffi) makes the override a hard
/// error: there is no flag that runs admitted native code unconfined.
#[must_use]
pub fn is_low_value_only(union: &BTreeSet<Capability>) -> bool {
    union.iter().all(|c| {
        matches!(c, Capability::Clock | Capability::Random)
    })
}

/// The build-time platform verdict: can a sound run jail be built on THIS
/// target at all? Linux with `x86_64` seccomp support → yes. Everything else is
/// the documented refuse-gap.
///
/// This is a `const` reflection of the compile target, independent of host tool
/// availability (which [`RunJailTools`] probing covers). On a non-Linux or
/// non-`x86_64` target no argv/seccomp this crate builds would confine the app,
/// so the honest answer is "refuse", never "run unconfined".
#[must_use]
pub const fn platform_supports_jail() -> bool {
    cfg!(all(target_os = "linux", target_arch = "x86_64"))
}

/// Probe the host for the run-jail tools and decide whether a jail can be built,
/// returning the tools or the fail-closed refusal.
///
/// `wants_wall_clock` = the profile sets a wall-clock cap, so `timeout` is
/// additionally required. `bwrap` and `prlimit` are always required.
///
/// # Errors
///
/// [`RunJailDefect::UnsupportedPlatform`] on a non-Linux/x86_64 target;
/// [`RunJailDefect::PrimitiveUnavailable`] when a required host tool is absent.
pub fn probe_run_jail_tools(wants_wall_clock: bool) -> Result<RunJailTools, RunJailDefect> {
    if !platform_supports_jail() {
        return Err(RunJailDefect::UnsupportedPlatform {
            reason: "runtime jail is Linux/x86_64-only in the first cut",
        });
    }
    let caps = crate::probe();
    let mut missing: Vec<&'static str> = Vec::new();
    if caps.bwrap.is_none() {
        missing.push("bwrap");
    }
    if caps.prlimit.is_none() {
        missing.push("prlimit");
    }
    if wants_wall_clock && caps.timeout.is_none() {
        missing.push("timeout");
    }
    if !missing.is_empty() {
        return Err(RunJailDefect::PrimitiveUnavailable { missing });
    }
    // `platform_supports_jail` + the probes above guarantee these are `Some`.
    let (Some(bwrap), Some(prlimit)) = (caps.bwrap, caps.prlimit) else {
        return Err(RunJailDefect::PrimitiveUnavailable {
            missing: vec!["bwrap", "prlimit"],
        });
    };
    Ok(RunJailTools {
        bwrap,
        prlimit,
        timeout: caps.timeout,
    })
}

/// Run the emitted `app` binary inside the run jail described by `profile`,
/// replacing the current process on success (Unix `exec`).
///
/// This compiles the seccomp program for the profile's subprocess axis, places
/// it on an inheritable file descriptor, builds the `bwrap` argv referencing
/// that fd, and `exec`s it. The seccomp fd is deliberately left WITHOUT the
/// close-on-exec flag so `bwrap` inherits it; every other fd stays cloexec.
///
/// On a non-Linux target this is a compile-time refusal shape — the whole body
/// is `cfg(target_os = "linux")`; other targets return
/// [`RunJailDefect::UnsupportedPlatform`].
///
/// # Errors
///
/// Any [`RunJailDefect`]; on success (Linux) it does not return.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn exec_in_run_jail(
    tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    app: &Path,
    app_args: &[OsString],
) -> Result<std::convert::Infallible, RunJailDefect> {
    use std::os::unix::process::CommandExt as _;

    // Compile the seccomp program for this profile. `None` ⇒ this architecture
    // has no filter we can emit — refuse (fail-closed), never run unfiltered.
    let Some(program) = seccomp::subprocess_deny_program(profile.subprocess) else {
        return Err(RunJailDefect::UnsupportedPlatform {
            reason: "no seccomp filter can be compiled for this architecture",
        });
    };
    let bytes = seccomp::program_bytes(&program);
    let seccomp_fd = write_seccomp_memfd(&bytes)?;

    let mut payload: Vec<OsString> = Vec::with_capacity(app_args.len() + 1);
    payload.push(app.as_os_str().to_owned());
    payload.extend(app_args.iter().cloned());

    let host_env = |k: &str| std::env::var_os(k);
    let argv = run_jail_argv(
        tools,
        profile,
        scoped_tmp,
        working_tree,
        Some(seccomp_fd),
        &host_env,
        &payload,
    );

    let (program_path, rest) = argv.split_first().ok_or(RunJailDefect::Spawn {
        detail: "empty jail argv".to_owned(),
    })?;
    let mut cmd = std::process::Command::new(program_path);
    cmd.args(rest);
    // The seccomp fd MUST survive exec so bwrap can read the program from it;
    // clear its close-on-exec flag right before exec via a pre_exec hook.
    let fd = seccomp_fd;
    // SAFETY: `pre_exec` runs in the child between fork and exec. `fcntl` with
    // `F_SETFD`/`0` is async-signal-safe and touches only this process's fd
    // table; no allocation, no lock. A failure returns an error that aborts the
    // exec, so a jail that could not un-cloexec its filter fd refuses rather
    // than running the app without the filter.
    unsafe {
        cmd.pre_exec(move || {
            let flags = libc_fcntl_getfd(fd)?;
            let cleared = flags & !FD_CLOEXEC;
            libc_fcntl_setfd(fd, cleared)?;
            Ok(())
        });
    }
    let err = cmd.exec();
    Err(RunJailDefect::Spawn {
        detail: err.to_string(),
    })
}

/// Non-Linux stub: the run jail is a documented refuse-gap off Linux/x86_64.
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub fn exec_in_run_jail(
    _tools: &RunJailTools,
    _profile: &SandboxProfile,
    _scoped_tmp: &Path,
    _working_tree: &Path,
    _app: &Path,
    _app_args: &[OsString],
) -> Result<std::convert::Infallible, RunJailDefect> {
    Err(RunJailDefect::UnsupportedPlatform {
        reason: "runtime jail is Linux/x86_64-only in the first cut",
    })
}

// The two `fcntl` operations the pre_exec hook needs, wrapped so the raw
// `extern "C"` surface is contained. `FD_CLOEXEC` is the close-on-exec flag.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const FD_CLOEXEC: i32 = 1;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn memfd_create(name: *const core::ffi::c_char, flags: core::ffi::c_uint) -> i32;
    fn write(fd: i32, buf: *const core::ffi::c_void, count: usize) -> isize;
    fn lseek(fd: i32, offset: i64, whence: i32) -> i64;
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const F_GETFD: i32 = 1;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const F_SETFD: i32 = 2;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn libc_fcntl_getfd(fd: i32) -> std::io::Result<i32> {
    // SAFETY: a plain fcntl(F_GETFD) query on an owned fd; no memory is touched.
    let r = unsafe { fcntl(fd, F_GETFD) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(r)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn libc_fcntl_setfd(fd: i32, flags: i32) -> std::io::Result<()> {
    // SAFETY: fcntl(F_SETFD, flags) on an owned fd; the variadic arg is a plain
    // int as the ABI requires.
    let r = unsafe { fcntl(fd, F_SETFD, flags) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Write the compiled seccomp program to an anonymous in-memory file and return
/// its file descriptor, rewound to offset 0, ready for `bwrap --seccomp <fd>`.
///
/// A `memfd` is used rather than a temp file so the program bytes never touch
/// the filesystem (nothing to race or tamper on disk) and the fd is
/// self-cleaning when closed.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn write_seccomp_memfd(bytes: &[u8]) -> Result<i32, RunJailDefect> {
    let spawn = |detail: String| RunJailDefect::Spawn { detail };
    let name = c"ipe-seccomp";
    // SAFETY: `memfd_create` with a valid NUL-terminated name and 0 flags
    // returns a new fd or -1; no memory is shared.
    let fd = unsafe { memfd_create(name.as_ptr(), 0) };
    if fd < 0 {
        return Err(spawn(format!(
            "memfd_create for the seccomp program failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    // Write the whole program. A short write is a hard error — a truncated
    // seccomp program would be a malformed/rejected filter, so refuse.
    let mut written: usize = 0;
    while written < bytes.len() {
        let remaining = &bytes[written..];
        // SAFETY: `write` reads `remaining.len()` bytes from a valid slice
        // pointer into the owned memfd; the slice outlives the call.
        let n = unsafe {
            write(
                fd,
                remaining.as_ptr().cast::<core::ffi::c_void>(),
                remaining.len(),
            )
        };
        if n <= 0 {
            return Err(spawn(format!(
                "writing the seccomp program to the memfd failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        written += usize::try_from(n).unwrap_or(0);
    }
    // Rewind so bwrap reads the program from the start.
    // SAFETY: lseek to absolute offset 0 (SEEK_SET = 0) on the owned fd.
    if unsafe { lseek(fd, 0, 0) } < 0 {
        return Err(spawn(format!(
            "rewinding the seccomp memfd failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(fd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(caps: &[Capability]) -> BTreeSet<Capability> {
        caps.iter().copied().collect()
    }

    #[test]
    fn empty_set_lowers_to_maximally_isolated() {
        let p = profile_from_capabilities(
            &BTreeSet::new(),
            &BTreeSet::new(),
            DatabaseAxis::NotApplicable,
            &[],
        )
        .expect("empty set lowers");
        assert_eq!(p, SandboxProfile::maximally_isolated());
        assert!(!p.network);
        assert_eq!(p.filesystem, FilesystemScope::Isolated);
        assert!(!p.subprocess);
        assert!(p.env_allowlist.is_empty());
    }

    #[test]
    fn network_capability_grants_network_only() {
        let p = profile_from_capabilities(
            &set(&[Capability::Network]),
            &BTreeSet::new(),
            DatabaseAxis::NotApplicable,
            &[],
        )
        .expect("lowers");
        assert!(p.network);
        assert_eq!(p.filesystem, FilesystemScope::Isolated);
        assert!(!p.subprocess);
    }

    #[test]
    fn the_union_of_inferred_and_declared_is_used() {
        // inferred = {filesystem}, declared = {network}: both must be granted
        // (no false-deny — a declared axis is relaxed even if not inferred).
        let p = profile_from_capabilities(
            &set(&[Capability::Filesystem]),
            &set(&[Capability::Network]),
            DatabaseAxis::NotApplicable,
            &[],
        )
        .expect("lowers");
        assert!(p.network);
        assert_eq!(p.filesystem, FilesystemScope::WorkingTreeReadWrite);
    }

    #[test]
    fn database_lowers_to_network_or_filesystem_per_driver() {
        let net = profile_from_capabilities(
            &set(&[Capability::Database]),
            &BTreeSet::new(),
            DatabaseAxis::Network,
            &[],
        )
        .expect("lowers");
        assert!(net.network);
        assert_eq!(net.filesystem, FilesystemScope::Isolated);

        let file = profile_from_capabilities(
            &set(&[Capability::Database]),
            &BTreeSet::new(),
            DatabaseAxis::Filesystem,
            &[],
        )
        .expect("lowers");
        assert!(!file.network);
        assert_eq!(file.filesystem, FilesystemScope::WorkingTreeReadWrite);
    }

    #[test]
    fn database_with_an_unknown_driver_fails_closed() {
        let r = profile_from_capabilities(
            &set(&[Capability::Database]),
            &BTreeSet::new(),
            DatabaseAxis::NotApplicable,
            &[],
        );
        assert_eq!(r, Err(ProfileError::UnknownDatabaseDriver));
    }

    #[test]
    fn env_capability_re_exports_the_named_allowlist() {
        let p = profile_from_capabilities(
            &set(&[Capability::Env]),
            &BTreeSet::new(),
            DatabaseAxis::NotApplicable,
            &["DATABASE_URL".to_owned(), "API_KEY".to_owned()],
        )
        .expect("lowers");
        assert_eq!(p.env_allowlist, vec!["DATABASE_URL", "API_KEY"]);
    }

    #[test]
    fn clock_and_random_carry_no_control() {
        let p = profile_from_capabilities(
            &set(&[Capability::Clock, Capability::Random]),
            &BTreeSet::new(),
            DatabaseAxis::NotApplicable,
            &[],
        )
        .expect("lowers");
        // No axis is opened by clock/random.
        assert_eq!(p, SandboxProfile::maximally_isolated());
    }

    #[test]
    fn native_ffi_alone_opens_no_control() {
        let p = profile_from_capabilities(
            &set(&[Capability::NativeFfi]),
            &BTreeSet::new(),
            DatabaseAxis::NotApplicable,
            &[],
        )
        .expect("lowers");
        assert_eq!(p, SandboxProfile::maximally_isolated());
    }

    #[test]
    fn floor_comparison_refuses_a_widened_profile() {
        let floor = SandboxProfile::maximally_isolated();
        // A profile that grants network is weaker than a maximally-isolated
        // floor → refuse.
        let widened = SandboxProfile {
            network: true,
            ..SandboxProfile::maximally_isolated()
        };
        assert!(!widened.is_at_least_as_isolated_as(&floor));
        // The floor itself is at least as isolated as itself.
        assert!(floor.is_at_least_as_isolated_as(&floor));
    }

    #[test]
    fn floor_comparison_allows_a_tighter_profile() {
        // floor grants network; a profile that does NOT grant it is tighter →
        // allowed (more isolation than the floor is never a violation).
        let floor = SandboxProfile {
            network: true,
            filesystem: FilesystemScope::WorkingTreeReadWrite,
            ..SandboxProfile::maximally_isolated()
        };
        let tighter = SandboxProfile::maximally_isolated();
        assert!(tighter.is_at_least_as_isolated_as(&floor));
    }

    #[test]
    fn floor_comparison_checks_env_var_subset() {
        let floor = SandboxProfile {
            env_allowlist: vec!["A".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        // Granting an env var the floor does not is a violation.
        let extra_env = SandboxProfile {
            env_allowlist: vec!["A".to_owned(), "SECRET".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        assert!(!extra_env.is_at_least_as_isolated_as(&floor));
        // A subset is fine.
        let subset = SandboxProfile {
            env_allowlist: vec!["A".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        assert!(subset.is_at_least_as_isolated_as(&floor));
    }

    fn tools() -> RunJailTools {
        RunJailTools {
            bwrap: PathBuf::from("/usr/bin/bwrap"),
            prlimit: PathBuf::from("/usr/bin/prlimit"),
            timeout: Some(PathBuf::from("/usr/bin/timeout")),
        }
    }

    fn rendered(profile: &SandboxProfile, seccomp_fd: Option<i32>) -> Vec<String> {
        let no_env = |_: &str| None;
        run_jail_argv(
            &tools(),
            profile,
            Path::new("/work/tmp-1"),
            Path::new("/work/tree"),
            seccomp_fd,
            &no_env,
            &[OsString::from("/work/tree/target/debug/ipe-app")],
        )
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
    }

    #[test]
    fn maximally_isolated_argv_denies_net_masks_proc_and_scrubs_env() {
        let argv = rendered(&SandboxProfile::maximally_isolated(), Some(10));
        let joined = argv.join(" ");
        // No wall clock (default RunResourceLimits has wall_secs = None), so
        // bwrap is the first program.
        assert!(joined.starts_with("/usr/bin/bwrap"), "{joined}");
        // Net unshared (network absent), namespaces fresh, env scrubbed.
        for flag in [
            "--unshare-net",
            "--unshare-pid",
            "--unshare-ipc",
            "--clearenv",
        ] {
            assert!(argv.contains(&flag.to_owned()), "missing {flag}: {joined}");
        }
        // Fresh proc mask AFTER the ro-bind of root — the ordering that masks
        // the host /proc.
        let ro_root = joined.find("--ro-bind / /").expect("ro-bind root");
        let proc = joined.find("--proc /proc").expect("proc mask");
        assert!(proc > ro_root, "proc mask must follow the ro-bind: {joined}");
        // Seccomp filter attached.
        assert!(joined.contains("--seccomp 10"), "{joined}");
        // Resource caps then the payload, no shell.
        assert!(joined.contains("-- /usr/bin/prlimit --as="), "{joined}");
        assert!(
            joined.ends_with("-- /work/tree/target/debug/ipe-app"),
            "{joined}"
        );
        assert!(!joined.contains("sh -c"), "{joined}");
    }

    #[test]
    fn network_granted_shares_the_net_namespace() {
        let p = SandboxProfile {
            network: true,
            ..SandboxProfile::maximally_isolated()
        };
        let argv = rendered(&p, None);
        assert!(
            !argv.contains(&"--unshare-net".to_owned()),
            "network granted must NOT unshare net: {}",
            argv.join(" ")
        );
        // IPC is still unshared unconditionally.
        assert!(argv.contains(&"--unshare-ipc".to_owned()));
    }

    #[test]
    fn filesystem_granted_binds_the_working_tree_read_write() {
        let p = SandboxProfile {
            filesystem: FilesystemScope::WorkingTreeReadWrite,
            ..SandboxProfile::maximally_isolated()
        };
        let joined = rendered(&p, None).join(" ");
        assert!(
            joined.contains("--bind /work/tree /work/tree"),
            "working tree not bound rw: {joined}"
        );
        assert!(joined.contains("--chdir /work/tree"), "{joined}");
    }

    #[test]
    fn env_allowlist_re_exports_only_named_present_vars() {
        let p = SandboxProfile {
            env_allowlist: vec!["DATABASE_URL".to_owned(), "ABSENT".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        let host = |k: &str| {
            if k == "DATABASE_URL" {
                Some(OsString::from("postgres://x"))
            } else {
                None
            }
        };
        let argv = run_jail_argv(
            &tools(),
            &p,
            Path::new("/work/tmp-1"),
            Path::new("/work/tree"),
            None,
            &host,
            &[OsString::from("app")],
        );
        let joined: Vec<String> = argv.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let s = joined.join(" ");
        assert!(s.contains("--setenv DATABASE_URL postgres://x"), "{s}");
        // An absent named var is simply not re-exported.
        assert!(!s.contains("ABSENT"), "{s}");
    }

    #[test]
    fn a_wall_clock_profile_wraps_in_timeout() {
        let p = SandboxProfile {
            limits: RunResourceLimits {
                wall_secs: Some(30),
                ..RunResourceLimits::default()
            },
            ..SandboxProfile::maximally_isolated()
        };
        let joined = rendered(&p, None).join(" ");
        assert!(
            joined.starts_with("/usr/bin/timeout --kill-after=5s 30 /usr/bin/bwrap"),
            "{joined}"
        );
    }
}

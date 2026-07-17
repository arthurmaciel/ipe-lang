//! The isolation jail every untrusted-crate compile/inspect runs inside.
//!
//! `ipe add <crate>` compiles foreign code, and compiling Rust executes
//! foreign code (`build.rs`, proc-macros) — remote code execution gated only
//! by a crate name. This crate confines that RCE surface so the FFI
//! decode/emit core (`ipe_ffi`) stays process-capability-free:
//!
//! * **bubblewrap is the primary jail** — network denied, `/` read-only, one
//!   scoped writable tempdir, env scrubbed to an allowlist, fresh
//!   PID/UTS/IPC/cgroup namespaces, rlimit + wall-clock caps.
//! * **`unshare` is the fallback**, sound ONLY with a post-spawn proof: the
//!   child must assert every namespace it claimed actually took effect
//!   before any untrusted code runs ([`prove_isolation`]) — `unshare` can
//!   partially no-op yet exit 0.
//! * **Refusal is the default** (`IPE-F4410`) when neither mechanism can
//!   prove isolation. The only override is `IPE_FFI_ALLOW_UNSANDBOXED=1`,
//!   which the driver must surface with a printed trust warning.
//!
//! There is no shell anywhere in this crate: every invocation is a direct
//! argv (`std::process::Command`), so the quoting/injection class does not
//! exist here.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use ipe_diagnostics::{Code, IPE_F4410};

/// Why a jail could not be established or a jailed run failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxDefect {
    /// Neither `bwrap` nor a provable `unshare` fallback is available.
    NoIsolationMechanism,
    /// The `unshare` fallback could not prove a namespace took effect.
    IsolationUnproven(IsolationDefect),
    /// The jailed process could not be spawned or awaited.
    Spawn {
        /// The program that failed to spawn.
        program: String,
        /// The rendered OS error.
        detail: String,
    },
    /// The jailed process produced more output than the configured cap.
    OutputCapExceeded {
        /// The configured cap in bytes.
        cap_bytes: u64,
    },
}

impl SandboxDefect {
    /// The stable taxonomy code (`IPE-F4410` for the whole family).
    #[must_use]
    pub const fn code(&self) -> Code {
        IPE_F4410
    }
}

impl fmt::Display for SandboxDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoIsolationMechanism => write!(
                f,
                "{}: cannot establish an isolation jail (bwrap absent, unshare absent or \
                 unprovable); refusing to compile an untrusted crate unsandboxed",
                self.code().as_str()
            ),
            Self::IsolationUnproven(d) => write!(
                f,
                "{}: the unshare fallback could not prove its isolation: {d}",
                self.code().as_str()
            ),
            Self::Spawn { program, detail } => write!(
                f,
                "{}: failed to run the jailed process `{program}`: {detail}",
                self.code().as_str()
            ),
            Self::OutputCapExceeded { cap_bytes } => write!(
                f,
                "{}: the jailed process exceeded the {cap_bytes}-byte output cap",
                self.code().as_str()
            ),
        }
    }
}

impl std::error::Error for SandboxDefect {}

/// Which isolation assertion failed inside the `unshare` fallback child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IsolationDefect {
    /// The child is not PID 1 — the PID namespace did not take effect.
    NotPidOne,
    /// A namespace id matches the parent's — that namespace did not detach.
    NamespaceUnchanged(&'static str),
    /// A non-loopback network interface is visible in the "new" net ns.
    NonLoopbackInterface(String),
    /// A default route exists in the "new" net ns.
    DefaultRoutePresent,
    /// A `/proc` read needed for the proof failed (fail closed).
    ProcUnreadable(String),
}

impl fmt::Display for IsolationDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPidOne => f.write_str("child is not PID 1 in its namespace"),
            Self::NamespaceUnchanged(ns) => {
                write!(f, "the {ns} namespace id still matches the host's")
            }
            Self::NonLoopbackInterface(name) => {
                write!(f, "non-loopback interface `{name}` is visible")
            }
            Self::DefaultRoutePresent => f.write_str("a default route is present"),
            Self::ProcUnreadable(detail) => write!(f, "cannot read /proc for the proof: {detail}"),
        }
    }
}

// ── capability probe ────────────────────────────────────────────────────────

/// The host tools the jail can be built from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// `bwrap` (bubblewrap) — the primary jail.
    pub bwrap: Option<PathBuf>,
    /// `unshare` — the fallback, requiring the post-spawn proof.
    pub unshare: Option<PathBuf>,
    /// `prlimit` — resource caps.
    pub prlimit: Option<PathBuf>,
    /// `timeout` — the wall clock.
    pub timeout: Option<PathBuf>,
}

/// Probe `PATH` for the jail tools.
#[must_use]
pub fn probe() -> Capabilities {
    Capabilities {
        bwrap: find_in_path("bwrap"),
        unshare: find_in_path("unshare"),
        prlimit: find_in_path("prlimit"),
        timeout: find_in_path("timeout"),
    }
}

fn find_in_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|candidate| candidate.is_file())
}

/// The isolation mechanism selected for a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mechanism {
    /// Bubblewrap — fails closed by itself.
    Bwrap(PathBuf),
    /// `unshare` — the child MUST run [`prove_isolation`] before any
    /// untrusted code.
    UnshareCandidate(PathBuf),
    /// Neither is available: refuse (`IPE-F4410`).
    Refused,
}

/// Select the strongest available mechanism (bwrap > unshare > refusal).
#[must_use]
pub fn select_mechanism(caps: &Capabilities) -> Mechanism {
    if let Some(b) = &caps.bwrap {
        return Mechanism::Bwrap(b.clone());
    }
    if let Some(u) = &caps.unshare {
        return Mechanism::UnshareCandidate(u.clone());
    }
    Mechanism::Refused
}

/// Whether the operator explicitly opted into unsandboxed execution. The
/// driver MUST print a trust warning when honouring this.
#[must_use]
pub fn unsandboxed_override_set() -> bool {
    std::env::var_os("IPE_FFI_ALLOW_UNSANDBOXED").is_some_and(|v| v == "1")
}

// ── jail specification ──────────────────────────────────────────────────────

/// Network posture of one jailed phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPolicy {
    /// Compile / introspect: a NEW empty net namespace — no egress.
    Denied,
    /// The explicit fetch phase: network stays on; every other control
    /// (read-only `/`, scrubbed env, caps) still applies.
    FetchOnly,
}

/// Resource caps for one jailed invocation (env-overridable by the driver,
/// which prints a warning when it does).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Address-space cap in bytes.
    pub rss_bytes: u64,
    /// CPU-seconds cap.
    pub cpu_secs: u64,
    /// Wall-clock cap in seconds (enforced by `timeout`).
    pub wall_secs: u64,
    /// Open-file-descriptor cap.
    pub fd_cap: u64,
    /// Process-count cap.
    pub proc_cap: u64,
    /// Maximum bytes read from the jailed process's stdout.
    pub out_cap_bytes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            rss_bytes: 4 * 1024 * 1024 * 1024,
            cpu_secs: 300,
            wall_secs: 420,
            fd_cap: 256,
            proc_cap: 512,
            out_cap_bytes: 256 * 1024 * 1024,
        }
    }
}

/// One jailed invocation: where it may write, what it may see, how big it
/// may grow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JailSpec {
    /// Network posture for this phase.
    pub network: NetworkPolicy,
    /// The per-invocation scoped tempdir — the ONLY writable mount.
    pub scoped_tmp: PathBuf,
    /// Pre-fetched crate sources, bound read-only (compile jail).
    pub registry_cache: Option<PathBuf>,
    /// The pinned nightly toolchain name exported as `RUSTUP_TOOLCHAIN`.
    pub toolchain: Option<String>,
    /// Toolchain directories re-bound read-only AFTER the `/home` tmpfs mask
    /// (a rustup install lives under the invoking user's home, which the
    /// tmpfs would otherwise hide). Read-only: the payload can execute the
    /// toolchain but never mutate it.
    pub toolchain_ro_binds: Vec<PathBuf>,
    /// Directories prepended to the jail's `PATH` (toolchain `bin` dirs).
    pub path_prepend: Vec<PathBuf>,
    /// The rustup root exported as `RUSTUP_HOME` (the env is scrubbed, so the
    /// proxy binaries cannot discover it from `$HOME`).
    pub rustup_home: Option<PathBuf>,
    /// Resource caps.
    pub limits: ResourceLimits,
}

/// The full jail argv for one payload: `timeout … bwrap … prlimit … payload`.
///
/// Pure — no process is spawned — so the exact isolation surface is
/// unit-testable. The env is scrubbed with `--clearenv`; only the fixed
/// allowlist re-enters. There is NO shell token anywhere in the result.
#[must_use]
pub fn bwrap_argv(
    bwrap: &Path,
    prlimit: Option<&Path>,
    timeout: Option<&Path>,
    spec: &JailSpec,
    payload: &[OsString],
) -> Vec<OsString> {
    let mut argv: Vec<OsString> = Vec::new();
    if let Some(t) = timeout {
        argv.push(t.into());
        argv.push("--kill-after=5s".into());
        argv.push(spec.limits.wall_secs.to_string().into());
    }
    argv.push(bwrap.into());
    if spec.network == NetworkPolicy::Denied {
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
    argv.push("--ro-bind".into());
    argv.push("/".into());
    argv.push("/".into());
    // A fresh minimal devtmpfs: the ro-bound host `/dev` nodes carry no device
    // permissions inside the user namespace, so `/dev/null` opens fail EACCES
    // — which cargo hits when wiring child stdio.
    argv.push("--dev".into());
    argv.push("/dev".into());
    for tmpfs in ["/home", "/root", "/tmp"] {
        argv.push("--tmpfs".into());
        argv.push(tmpfs.into());
    }
    if let Some(cache) = &spec.registry_cache {
        argv.push("--ro-bind".into());
        argv.push(cache.clone().into());
        argv.push(cache.clone().into());
    }
    // Re-expose the toolchain through the tmpfs mask, read-only.
    for dir in &spec.toolchain_ro_binds {
        argv.push("--ro-bind".into());
        argv.push(dir.clone().into());
        argv.push(dir.clone().into());
    }
    argv.push("--bind".into());
    argv.push(spec.scoped_tmp.clone().into());
    argv.push(spec.scoped_tmp.clone().into());
    argv.push("--chdir".into());
    argv.push(spec.scoped_tmp.clone().into());
    let cargo_home = spec.scoped_tmp.join("cargo-home");
    let mut path_value = String::new();
    for dir in &spec.path_prepend {
        path_value.push_str(&dir.to_string_lossy());
        path_value.push(':');
    }
    path_value.push_str("/usr/bin:/bin");
    let mut setenvs: Vec<(&str, OsString)> = Vec::new();
    // The fetch phase must reach the registry; every compile/introspect
    // phase stays offline.
    if spec.network == NetworkPolicy::Denied {
        setenvs.push(("CARGO_NET_OFFLINE", "1".into()));
    }
    setenvs.push(("CARGO_HOME", cargo_home.into()));
    setenvs.push(("PATH", path_value.into()));
    setenvs.push(("TMPDIR", spec.scoped_tmp.clone().into()));
    for (key, value) in setenvs {
        argv.push("--setenv".into());
        argv.push(key.into());
        argv.push(value);
    }
    if let Some(tc) = &spec.toolchain {
        argv.push("--setenv".into());
        argv.push("RUSTUP_TOOLCHAIN".into());
        argv.push(tc.into());
    }
    if let Some(rustup_home) = &spec.rustup_home {
        argv.push("--setenv".into());
        argv.push("RUSTUP_HOME".into());
        argv.push(rustup_home.clone().into());
    }
    argv.push("--".into());
    if let Some(p) = prlimit {
        argv.push(p.into());
        argv.push(format!("--as={}", spec.limits.rss_bytes).into());
        argv.push(format!("--cpu={}", spec.limits.cpu_secs).into());
        argv.push(format!("--nofile={}", spec.limits.fd_cap).into());
        argv.push(format!("--nproc={}", spec.limits.proc_cap).into());
        argv.push(format!("--fsize={}", spec.limits.out_cap_bytes).into());
        argv.push("--".into());
    }
    argv.extend(payload.iter().cloned());
    argv
}

// ── unshare-fallback isolation proof ────────────────────────────────────────

/// The parent's namespace identities, captured before spawning the fallback
/// child so the child can prove its own differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NsIds {
    /// `/proc/self/ns/net` link target.
    pub net: String,
    /// `/proc/self/ns/mnt` link target.
    pub mnt: String,
    /// `/proc/self/ns/uts` link target.
    pub uts: String,
    /// `/proc/self/ns/ipc` link target.
    pub ipc: String,
}

/// Read the calling process's namespace ids.
///
/// # Errors
///
/// [`IsolationDefect::ProcUnreadable`] when `/proc/self/ns/*` cannot be
/// read (fail closed — no proof without the ids).
pub fn current_ns_ids() -> Result<NsIds, IsolationDefect> {
    let read = |ns: &str| -> Result<String, IsolationDefect> {
        std::fs::read_link(format!("/proc/self/ns/{ns}"))
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| IsolationDefect::ProcUnreadable(format!("ns/{ns}: {e}")))
    };
    Ok(NsIds {
        net: read("net")?,
        mnt: read("mnt")?,
        uts: read("uts")?,
        ipc: read("ipc")?,
    })
}

/// The post-spawn isolation proof the `unshare` fallback child MUST run as
/// its first action, before any untrusted code.
///
/// `unshare` can partially fail or silently no-op yet still exit 0, leaving
/// a process with full host networking — so nothing is assumed: the child
/// asserts it is PID 1, that every claimed namespace id differs from the
/// parent's, and that the net namespace is truly empty (no non-loopback
/// interface, no default route).
///
/// # Errors
///
/// The first failed assertion; the caller MUST hard-fail to the refusal
/// path — never proceed to compile on the assumption `unshare` worked.
pub fn prove_isolation(parent: &NsIds) -> Result<(), IsolationDefect> {
    if std::process::id() != 1 {
        return Err(IsolationDefect::NotPidOne);
    }
    let own = current_ns_ids()?;
    for (name, ours, parents) in [
        ("net", &own.net, &parent.net),
        ("mnt", &own.mnt, &parent.mnt),
        ("uts", &own.uts, &parent.uts),
        ("ipc", &own.ipc, &parent.ipc),
    ] {
        if ours == parents {
            return Err(IsolationDefect::NamespaceUnchanged(match name {
                "net" => "net",
                "mnt" => "mnt",
                "uts" => "uts",
                _ => "ipc",
            }));
        }
    }
    assert_net_namespace_empty()
}

/// Assert the current net namespace has no non-loopback interface and no
/// default route.
///
/// # Errors
///
/// The first visible escape hatch, or [`IsolationDefect::ProcUnreadable`]
/// when the check itself cannot run (fail closed).
pub fn assert_net_namespace_empty() -> Result<(), IsolationDefect> {
    let ifaces = std::fs::read_dir("/sys/class/net")
        .map_err(|e| IsolationDefect::ProcUnreadable(format!("/sys/class/net: {e}")))?;
    for entry in ifaces {
        let entry =
            entry.map_err(|e| IsolationDefect::ProcUnreadable(format!("/sys/class/net: {e}")))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name != "lo" {
            return Err(IsolationDefect::NonLoopbackInterface(name));
        }
    }
    let route = std::fs::read_to_string("/proc/net/route")
        .map_err(|e| IsolationDefect::ProcUnreadable(format!("/proc/net/route: {e}")))?;
    // Each data row is `Iface\tDestination\t…`; destination 00000000 is the
    // default route.
    for line in route.lines().skip(1) {
        let mut cols = line.split_whitespace();
        let (Some(_iface), Some(dest)) = (cols.next(), cols.next()) else {
            continue;
        };
        if dest == "00000000" {
            return Err(IsolationDefect::DefaultRoutePresent);
        }
    }
    Ok(())
}

// ── jailed execution ────────────────────────────────────────────────────────

/// Output of a jailed run, with stdout bounded by the configured cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JailedOutput {
    /// Exit status code (`None` when killed by a signal / the wall clock).
    pub status: Option<i32>,
    /// Captured stdout, at most `out_cap_bytes`.
    pub stdout: Vec<u8>,
    /// Captured stderr, bounded by the same cap.
    pub stderr: Vec<u8>,
}

/// Run `payload` inside the bubblewrap jail described by `spec`.
///
/// Stdout is read with a hard byte cap — a 76k-symbol crate must not OOM
/// the host; exceeding the cap is a defect, not a truncation.
///
/// # Errors
///
/// [`SandboxDefect::Spawn`] when the jail cannot start;
/// [`SandboxDefect::OutputCapExceeded`] when the payload out-talks the cap.
pub fn run_in_bwrap_jail(
    caps: &Capabilities,
    spec: &JailSpec,
    payload: &[OsString],
) -> Result<JailedOutput, SandboxDefect> {
    let Some(bwrap) = &caps.bwrap else {
        return Err(SandboxDefect::NoIsolationMechanism);
    };
    let argv = bwrap_argv(
        bwrap,
        caps.prlimit.as_deref(),
        caps.timeout.as_deref(),
        spec,
        payload,
    );
    let (program, rest) = argv
        .split_first()
        .ok_or(SandboxDefect::NoIsolationMechanism)?;
    let spawn_err = |e: std::io::Error| SandboxDefect::Spawn {
        program: program.to_string_lossy().into_owned(),
        detail: e.to_string(),
    };
    let mut child = std::process::Command::new(program)
        .args(rest)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(spawn_err)?;
    let cap = spec.limits.out_cap_bytes;
    let stdout = read_bounded(child.stdout.take(), cap).map_err(spawn_err)?;
    let stderr = read_bounded(child.stderr.take(), cap).map_err(spawn_err)?;
    let status = child.wait().map_err(spawn_err)?;
    match (stdout, stderr) {
        (Some(out), Some(err)) => Ok(JailedOutput {
            status: status.code(),
            stdout: out,
            stderr: err,
        }),
        _ => Err(SandboxDefect::OutputCapExceeded { cap_bytes: cap }),
    }
}

/// Read a stream up to `cap` bytes; `Ok(None)` when the stream exceeds it.
fn read_bounded<R: std::io::Read>(
    stream: Option<R>,
    cap: u64,
) -> Result<Option<Vec<u8>>, std::io::Error> {
    use std::io::Read;
    let Some(stream) = stream else {
        return Ok(Some(Vec::new()));
    };
    let mut buf = Vec::new();
    // Take one extra byte: reaching it proves the cap was exceeded.
    stream.take(cap.saturating_add(1)).read_to_end(&mut buf)?;
    if u64::try_from(buf.len()).unwrap_or(u64::MAX) > cap {
        return Ok(None);
    }
    Ok(Some(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> JailSpec {
        JailSpec {
            network: NetworkPolicy::Denied,
            scoped_tmp: PathBuf::from("/work/tmp-1"),
            registry_cache: Some(PathBuf::from("/work/registry")),
            toolchain: Some("nightly-2026-01-01".to_owned()),
            toolchain_ro_binds: Vec::new(),
            path_prepend: Vec::new(),
            rustup_home: None,
            limits: ResourceLimits::default(),
        }
    }

    fn rendered_argv(spec: &JailSpec) -> Vec<String> {
        let payload: Vec<OsString> = vec!["ipe-ffi-inspector".into(), "semver".into()];
        bwrap_argv(
            Path::new("/usr/bin/bwrap"),
            Some(Path::new("/usr/bin/prlimit")),
            Some(Path::new("/usr/bin/timeout")),
            spec,
            &payload,
        )
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
    }

    #[test]
    fn jail_argv_denies_network_scrubs_env_and_bounds_resources() {
        let argv = rendered_argv(&spec());
        let joined = argv.join(" ");
        // Wall clock wraps everything.
        assert!(joined.starts_with("/usr/bin/timeout --kill-after=5s 420 /usr/bin/bwrap"));
        // Network denied, namespaces fresh, tty detached, env scrubbed.
        for flag in [
            "--unshare-net",
            "--unshare-pid",
            "--unshare-uts",
            "--unshare-ipc",
            "--unshare-cgroup",
            "--die-with-parent",
            "--new-session",
            "--clearenv",
        ] {
            assert!(argv.contains(&flag.to_owned()), "missing {flag}: {joined}");
        }
        // Read-only root; tmpfs over every home; one writable mount.
        assert!(joined.contains("--ro-bind / /"), "{joined}");
        assert!(joined.contains("--tmpfs /home"), "{joined}");
        assert!(
            joined.contains("--ro-bind /work/registry /work/registry"),
            "{joined}"
        );
        assert!(
            joined.contains("--bind /work/tmp-1 /work/tmp-1"),
            "{joined}"
        );
        assert!(joined.contains("--chdir /work/tmp-1"), "{joined}");
        // Env allowlist only — offline cargo, scoped CARGO_HOME, fixed PATH.
        assert!(joined.contains("--setenv CARGO_NET_OFFLINE 1"), "{joined}");
        assert!(
            joined.contains("--setenv CARGO_HOME /work/tmp-1/cargo-home"),
            "{joined}"
        );
        assert!(joined.contains("--setenv PATH /usr/bin:/bin"), "{joined}");
        assert!(
            joined.contains("--setenv RUSTUP_TOOLCHAIN nightly-2026-01-01"),
            "{joined}"
        );
        // Resource caps via prlimit, then the payload with NO shell.
        assert!(
            joined
                .contains("-- /usr/bin/prlimit --as=4294967296 --cpu=300 --nofile=256 --nproc=512"),
            "{joined}"
        );
        assert!(joined.ends_with("-- ipe-ffi-inspector semver"), "{joined}");
        assert!(!joined.contains("sh -c"), "{joined}");
    }

    #[test]
    fn fetch_phase_keeps_network_but_every_other_control() {
        let mut s = spec();
        s.network = NetworkPolicy::FetchOnly;
        let argv = rendered_argv(&s);
        assert!(!argv.contains(&"--unshare-net".to_owned()));
        // Everything else still applies.
        assert!(argv.contains(&"--clearenv".to_owned()));
        assert!(argv.contains(&"--unshare-pid".to_owned()));
    }

    #[test]
    fn no_secret_bearing_env_enters_the_jail() {
        let argv = rendered_argv(&spec());
        let setenv_keys: Vec<&String> = argv
            .iter()
            .enumerate()
            .filter(|&(i, a)| a == "--setenv" && i + 1 < argv.len())
            .filter_map(|(i, _)| argv.get(i + 1))
            .collect();
        for key in &setenv_keys {
            assert!(
                matches!(
                    key.as_str(),
                    "CARGO_NET_OFFLINE"
                        | "CARGO_HOME"
                        | "PATH"
                        | "TMPDIR"
                        | "RUSTUP_TOOLCHAIN"
                        | "RUSTUP_HOME"
                ),
                "unexpected env var {key} enters the jail"
            );
        }
    }

    #[test]
    fn mechanism_selection_prefers_bwrap_then_unshare_then_refuses() {
        let both = Capabilities {
            bwrap: Some("/usr/bin/bwrap".into()),
            unshare: Some("/usr/bin/unshare".into()),
            ..Capabilities::default()
        };
        assert_eq!(
            select_mechanism(&both),
            Mechanism::Bwrap("/usr/bin/bwrap".into())
        );
        let only_unshare = Capabilities {
            unshare: Some("/usr/bin/unshare".into()),
            ..Capabilities::default()
        };
        assert_eq!(
            select_mechanism(&only_unshare),
            Mechanism::UnshareCandidate("/usr/bin/unshare".into())
        );
        assert_eq!(
            select_mechanism(&Capabilities::default()),
            Mechanism::Refused
        );
    }

    #[test]
    fn isolation_proof_fails_outside_a_jail() {
        // This test process is neither PID 1 nor in fresh namespaces, so the
        // proof must fail closed — the exact property the fallback needs.
        let parent = current_ns_ids().expect("host /proc is readable");
        assert_eq!(prove_isolation(&parent), Err(IsolationDefect::NotPidOne));
    }

    #[test]
    fn bounded_read_flags_cap_excess_instead_of_truncating() {
        let data = b"0123456789".to_vec();
        let ok = read_bounded(Some(&data[..]), 10).expect("read");
        assert_eq!(ok, Some(data.clone()));
        let over = read_bounded(Some(&data[..]), 9).expect("read");
        assert_eq!(over, None);
    }

    #[test]
    fn default_limits_match_the_spec_table() {
        let l = ResourceLimits::default();
        assert_eq!(l.rss_bytes, 4 * 1024 * 1024 * 1024);
        assert_eq!(l.cpu_secs, 300);
        assert_eq!(l.wall_secs, 420);
        assert_eq!(l.fd_cap, 256);
        assert_eq!(l.proc_cap, 512);
        assert_eq!(l.out_cap_bytes, 256 * 1024 * 1024);
    }

    #[test]
    fn defect_display_carries_the_refusal_code() {
        let d = SandboxDefect::NoIsolationMechanism;
        assert_eq!(d.code().as_str(), "IPE-F4410");
        assert!(d.to_string().contains("IPE-F4410"));
        assert!(d.to_string().contains("refusing"));
    }
}

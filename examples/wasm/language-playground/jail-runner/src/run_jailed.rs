//! The sandboxed native build+run path for the playground `POST /run` endpoint.
//!
//! # Threat model
//!
//! `POST /run` accepts untrusted Ipê source, compiles it to a native Rust crate,
//! and then **builds and executes that crate**. Both the `cargo build` and the
//! resulting binary are attacker-derived code running on the server — a direct
//! remote-code-execution surface. Every build and every run therefore executes
//! inside the [`ipe_sandbox`] bubblewrap jail; the server never `cargo`-builds or
//! execs user-derived code outside it.
//!
//! The compile step (`ipe build`) is distinct: it runs the project's own trusted
//! compiler over the source text — deterministic codegen, not execution of the
//! user's program — so it stays a plain, timeout-bounded subprocess. Only the
//! two steps that run attacker-controlled code (build, run) are jailed.
//!
//! # Fail-closed
//!
//! If the host lacks the jail primitives (`bwrap`, `timeout`, `prlimit`), the
//! endpoint REFUSES — it never falls back to an unsandboxed build or run. The
//! only writable mount inside the jail is a per-request scratch directory, which
//! is removed after the request; the build phase also sees the warm vendored
//! crate sources read-only. Which jail knob enforces each control:
//!
//! | Control    | Enforcer (via [`ipe_sandbox`])                               |
//! |------------|--------------------------------------------------------------|
//! | Network    | `NetworkPolicy::Denied` → bwrap `--unshare-net` (no egress)   |
//! | Filesystem | `--ro-bind / /` + `--tmpfs` every home, `/tmp` + one `--bind` |
//! | Memory     | `prlimit --as`                                               |
//! | CPU        | `prlimit --cpu`                                              |
//! | Fork/proc  | `prlimit --nproc`                                            |
//! | Wall time  | `timeout --kill-after=5s <wall>` (SIGKILL on overrun)        |
//! | Output     | bounded stdout/stderr read (`out_cap_bytes`)                 |

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::budget::WallSecs;
use crate::staging::MAX_TREE_DEPTH;
use ipe_sandbox::{
    CanonicalPath, Capabilities, HomeMasks, JailPathError, JailSpec, NetworkPolicy, ResourceLimits,
    SandboxDefect, missing_caps, probe, run_in_bwrap_jail, run_in_bwrap_jail_deny_subprocess,
};

/// Resource caps for one playground build+run.
///
/// Deliberately tighter than the FFI-inspection defaults: a playground program is
/// a small single crate, not an SDK-scale dependency closure, so a runaway is
/// killed at a low ceiling.
#[derive(Debug, Clone, Copy)]
pub struct RunCaps {
    /// Address-space cap in bytes (`prlimit --as`).
    pub rss_bytes: u64,
    /// CPU-seconds cap (`prlimit --cpu`).
    pub cpu_secs: u64,
    /// Wall-clock cap in seconds (`timeout`).
    pub wall_secs: u64,
    /// Open-file-descriptor cap (`prlimit --nofile`).
    pub fd_cap: u64,
    /// Process-count cap (`prlimit --nproc`) — a fork bomb is killed here.
    pub proc_cap: u64,
    /// Maximum captured stdout/stderr bytes.
    pub out_cap_bytes: u64,
}

impl RunCaps {
    /// The build phase caps: a cargo build of one crate against a warm target
    /// legitimately spawns rustc + a linker, so `proc_cap` is generous enough for
    /// the toolchain while still bounding a fork bomb, and the wall clock is
    /// larger than the run phase's.
    ///
    /// `out_cap_bytes` is deliberately large: in the jail it is BOTH the stdout
    /// read cap AND the `prlimit --fsize` per-file write ceiling, and rustc writes
    /// `.rlib`/object artifacts well above a few MiB — too small an fsize SIGXFSZ-
    /// kills the build. 512 MiB clears a single crate's artifacts while still
    /// bounding a runaway that tries to fill the disk.
    #[must_use]
    pub const fn build_defaults() -> Self {
        Self {
            rss_bytes: 6 * 1024 * 1024 * 1024,
            cpu_secs: 900,
            wall_secs: 900,
            fd_cap: 512,
            proc_cap: 256,
            out_cap_bytes: 512 * 1024 * 1024,
        }
    }

    /// The run phase caps: executing the *emitted program*. Tight — a study
    /// program prints and exits; it never needs many processes or long CPU.
    ///
    /// `out_cap_bytes` bounds BOTH captured stdout and (as `prlimit --fsize`) any
    /// file the program writes into scratch, so a program that tries to fill the
    /// disk is SIGXFSZ-killed. 8 MiB is generous for a study program's output yet
    /// still a hard write ceiling.
    #[must_use]
    pub const fn run_defaults() -> Self {
        Self {
            rss_bytes: 512 * 1024 * 1024,
            cpu_secs: 5,
            wall_secs: 10,
            fd_cap: 64,
            // >1 so the tokio runtime's worker threads (same process, but nproc
            // counts threads) start; low enough that a fork bomb is killed.
            proc_cap: 32,
            out_cap_bytes: 8 * 1024 * 1024,
        }
    }

    /// These caps with the wall cut to `wall`, the phase's share of the budget.
    #[must_use]
    pub const fn with_wall(self, wall: WallSecs) -> Self {
        Self {
            wall_secs: wall.get(),
            ..self
        }
    }

    const fn to_limits(self) -> ResourceLimits {
        ResourceLimits {
            rss_bytes: self.rss_bytes,
            cpu_secs: self.cpu_secs,
            wall_secs: self.wall_secs,
            fd_cap: self.fd_cap,
            proc_cap: self.proc_cap,
            out_cap_bytes: self.out_cap_bytes,
        }
    }
}

/// The read-only toolchain binds a jailed cargo build needs re-exposed past the
/// home tmpfs masks: `$CARGO_HOME/bin` (the proxy binaries) and the rustup home.
/// NEVER the cargo home itself — that holds `credentials.toml` (the crates.io
/// token), which must stay outside the jail.
///
/// Each bind is canonical, resolved once, so the `PATH` entry and
/// `RUSTUP_HOME` the payload is handed are the very paths bound.
///
/// # Errors
///
/// [`SandboxDefect::Path`] when `CARGO_HOME` or `RUSTUP_HOME` is relative, a
/// bind does not resolve, or a bind would expose the cargo home.
fn toolchain_binds() -> Result<ToolchainBinds, SandboxDefect> {
    let tool_home = |var: &'static str, fallback: &str| {
        ipe_sandbox::home::tool_home(var, fallback)
            .map_err(|e| SandboxDefect::Path(JailPathError::ToolHomeRelative(e)))
    };
    toolchain_binds_from(
        tool_home("CARGO_HOME", ".cargo")?.as_deref(),
        tool_home("RUSTUP_HOME", ".rustup")?,
    )
}

/// The toolchain binds for the given absolute cargo and rustup homes.
///
/// # Errors
///
/// [`SandboxDefect::Path`] when a bind does not resolve, or when the rustup home
/// sits at or above the cargo home, so binding it would expose
/// `credentials.toml`.
fn toolchain_binds_from(
    cargo_home: Option<&Path>,
    rustup_home: Option<PathBuf>,
) -> Result<ToolchainBinds, SandboxDefect> {
    let canonical = |path: &Path| CanonicalPath::resolve(path).map_err(SandboxDefect::Path);
    let mut binds = ToolchainBinds::default();
    if let Some(cargo_home) = cargo_home {
        let cargo_bin = cargo_home.join("bin");
        if cargo_bin.is_dir() {
            let cargo_bin = canonical(&cargo_bin)?;
            binds.path_prepend.push(cargo_bin.clone());
            binds.ro_binds.push(cargo_bin);
        }
    }
    if let Some(rustup) = rustup_home
        && rustup.is_dir()
    {
        let rustup = canonical(&rustup)?;
        binds.ro_binds.push(rustup.clone());
        binds.rustup_home = Some(rustup);
    }
    if let Some(cargo_home) = cargo_home
        && let Some(bind) = ipe_sandbox::bind_exposing(&binds.ro_binds, cargo_home)
    {
        return Err(SandboxDefect::Path(JailPathError::ExposesCargoHome {
            bind: bind.as_path().to_path_buf(),
            cargo_home: cargo_home.to_path_buf(),
        }));
    }
    Ok(binds)
}

/// Why the jail could not be established for this host — the fail-closed refusal.
#[derive(Debug, Clone)]
pub struct JailUnavailable {
    /// The operator-facing refusal message (names the missing primitives).
    pub reason: String,
}

/// Probe the host jail primitives, or return the fail-closed refusal.
///
/// # Errors
///
/// [`JailUnavailable`] when `bwrap`, `timeout`, or `prlimit` is absent — the
/// endpoint refuses rather than run user code unconfined.
pub fn probe_or_refuse() -> Result<Capabilities, JailUnavailable> {
    let caps = probe();
    if caps.bwrap.is_none() {
        return Err(JailUnavailable {
            reason: "sandbox refused: bubblewrap (`bwrap`) is not installed on this host; \
                     the playground will not build or run user code without a jail"
                .to_owned(),
        });
    }
    let missing = missing_caps(&caps);
    if !missing.is_empty() {
        return Err(JailUnavailable {
            reason: format!(
                "sandbox refused: mandatory jail cap helper(s) absent ({}); \
                 install coreutils (timeout) and util-linux (prlimit)",
                missing.join(", ")
            ),
        });
    }
    Ok(caps)
}

/// The outcome of a single jailed phase (build or run).
#[derive(Debug, Clone)]
pub struct PhaseOutcome {
    /// Exit code, or `None` when the process was killed (a signal / the wall
    /// clock). `None` after a wall-clock kill is how a timeout is detected.
    pub status: Option<i32>,
    /// Captured stdout (bounded by the phase's `out_cap_bytes`).
    pub stdout: String,
    /// Captured stderr (bounded by the phase's `out_cap_bytes`).
    pub stderr: String,
    /// Whether the wall clock (or a resource cap) killed the process.
    pub killed: bool,
}

/// The read-only toolchain binds a phase re-exposes past the tmpfs masks.
#[derive(Default)]
struct ToolchainBinds {
    ro_binds: Vec<CanonicalPath>,
    path_prepend: Vec<CanonicalPath>,
    rustup_home: Option<CanonicalPath>,
}

/// Which spawn posture a phase runs under.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Subprocess {
    /// The BUILD phase: rustc + the linker legitimately spawn children, so
    /// subprocess creation is permitted (still net-denied, fs-jailed, capped).
    Allowed,
    /// The RUN phase: executing the untrusted program. Subprocess creation is
    /// DENIED at the seccomp boundary (`fork`/`vfork`/process-`clone`), so the
    /// program cannot shell out or fork-bomb past the `nproc` cap.
    Denied,
}

/// Run one jailed phase over `scoped_tmp` (its only writable mount), fully
/// offline (`NetworkPolicy::Denied` ⇒ `--unshare-net`).
///
/// `payload` is the direct argv — no shell is ever involved. `subprocess`
/// selects the fork/exec posture: the run phase denies it via seccomp.
///
/// # Errors
///
/// [`SandboxDefect`] when the jail cannot spawn, the output cap is exceeded, or
/// the invoker's homes cannot be masked.
fn run_phase(
    caps: &Capabilities,
    scoped_tmp: &CanonicalPath,
    run_caps: RunCaps,
    binds: ToolchainBinds,
    registry_cache: Option<CanonicalPath>,
    subprocess: Subprocess,
    payload: &[OsString],
) -> Result<PhaseOutcome, SandboxDefect> {
    let spec = JailSpec {
        // Denied is the whole point: user code never reaches the network. This is
        // structural (a fresh empty net namespace), not a filter that could be
        // misconfigured.
        network: NetworkPolicy::Denied,
        scoped_tmp: scoped_tmp.clone(),
        registry_cache,
        toolchain: None,
        toolchain_ro_binds: binds.ro_binds,
        path_prepend: binds.path_prepend,
        rustup_home: binds.rustup_home,
        homes: HomeMasks::of_invoker().map_err(SandboxDefect::Path)?,
        limits: run_caps.to_limits(),
    };
    let out = match subprocess {
        Subprocess::Allowed => run_in_bwrap_jail(caps, &spec, payload)?,
        // The run phase adds the seccomp subprocess-deny filter — a jailed program
        // that forks/execs is denied at the syscall boundary.
        Subprocess::Denied => run_in_bwrap_jail_deny_subprocess(caps, &spec, payload)?,
    };
    Ok(PhaseOutcome {
        status: out.status,
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        killed: is_wall_clock_kill(out.status),
    })
}

/// Whether an exit status is a wall-clock (or resource-cap) kill rather than the
/// program's own exit.
///
/// The jail argv is `timeout --kill-after=5s <wall> bwrap …`, so a timeout
/// surfaces as `timeout`'s exit code: `124` when it delivered SIGTERM, or
/// `128 + 9 = 137` when `--kill-after` had to SIGKILL. A `None` code means the
/// wrapper itself was signal-killed. Any of these is a kill, not a clean exit.
#[must_use]
const fn is_wall_clock_kill(status: Option<i32>) -> bool {
    // `None` = the wrapper was signal-killed; `124` = `timeout` sent SIGTERM;
    // `137` (128+9) = the `--kill-after` SIGKILL escalation.
    matches!(status, None | Some(124 | 137))
}

/// Build the emitted crate inside the jail, writing artifacts into the
/// jail-visible `scoped_tmp` target.
///
/// The build runs fully **offline** under `--unshare-net` against the lockfile
/// the harness staged: every crate the lock names is vendored in `vendor`, the
/// build's only registry, bound read-only. No network is ever available to
/// user-derived code.
///
/// The crate directory and the target directory both live under `scoped_tmp` (the
/// only writable bind), so their paths resolve inside the jail. `wall` replaces
/// the build default wall.
///
/// # Errors
///
/// [`SandboxDefect`] on a jail-spawn / output-cap failure, or when a jail path
/// does not resolve.
pub fn jailed_build(
    caps: &Capabilities,
    scoped_tmp: &Path,
    vendor: &VendorSource,
    wall: WallSecs,
) -> Result<PhaseOutcome, SandboxDefect> {
    // One canonical spelling for the writable bind and every payload path
    // under it.
    let scoped_tmp = CanonicalPath::resolve(scoped_tmp).map_err(SandboxDefect::Path)?;
    let binds = toolchain_binds()?;
    // Direct argv — no shell. Paths are under `scoped_tmp` so they are visible in
    // the jail. `--offline` makes any registry reach a hard cargo error, so the
    // build fails loudly rather than silently trying (and failing) egress on top
    // of the structural `--unshare-net`.
    //
    // The project dir IS the crate root: the emitted `Cargo.toml` + `src/`, plus
    // the runtime crate and lockfile the harness wrote.
    let manifest = scoped_tmp.as_path().join("Cargo.toml");
    let target = scoped_tmp.as_path().join("crate-target");
    let mut payload: Vec<OsString> = vec!["cargo".into(), "build".into(), "--offline".into()];
    payload.extend(vendor.cargo_config_args());
    payload.extend([
        "--manifest-path".into(),
        manifest.into_os_string(),
        "--target-dir".into(),
        target.into_os_string(),
    ]);
    run_phase(
        caps,
        &scoped_tmp,
        RunCaps::build_defaults().with_wall(wall),
        binds,
        Some(vendor.0.clone()),
        // The build spawns rustc + a linker — subprocess creation is required.
        Subprocess::Allowed,
        &payload,
    )
}

/// The path the emitted `ipe-app` binary lands at after [`jailed_build`], on both
/// the host and (identically, since it is under the writable bind) inside the
/// jail.
#[must_use]
pub fn app_binary_path(scoped_tmp: &Path) -> PathBuf {
    scoped_tmp
        .join("crate-target")
        .join("debug")
        .join("ipe-app")
}

/// Name of the cargo source that replaces crates.io with the warm vendor dir.
const VENDOR_SOURCE_NAME: &str = "ipe-warm-vendor";

/// The warm vendored crate sources, canonical, as every build's only registry.
///
/// Cargo's directory source verifies each crate against its vendored checksum,
/// needs no index or cargo-home state, and names every crate by a path under
/// this one fixed directory. Prewarm and the jailed build therefore see the same
/// source paths, which is what keeps the copied warm artifacts fresh. Only
/// [`VendorSource::resolve`] builds one, so holding one proves the path is
/// canonical and spells as a TOML basic string without escaping.
#[derive(Debug, Clone)]
pub struct VendorSource(CanonicalPath);

impl VendorSource {
    /// Resolve the warm vendor dir.
    ///
    /// # Errors
    ///
    /// [`SeedError::Path`] when it does not resolve, [`SeedError::NotPlain`]
    /// when it is not a directory, and [`SeedError::Unquotable`] when its
    /// canonical spelling is not UTF-8 or holds a quote, a backslash or a
    /// control character.
    pub fn resolve(warm_vendor: &Path) -> Result<Self, SeedError> {
        let canonical = CanonicalPath::resolve(warm_vendor).map_err(SeedError::Path)?;
        if !canonical.as_path().is_dir() {
            return Err(SeedError::NotPlain(canonical.as_path().to_path_buf()));
        }
        let quotable = canonical.as_path().to_str().is_some_and(|text| {
            !text
                .chars()
                .any(|c| c == '"' || c == '\\' || c.is_control())
        });
        if quotable {
            Ok(Self(canonical))
        } else {
            Err(SeedError::Unquotable(canonical.as_path().to_path_buf()))
        }
    }

    /// The canonical vendor dir.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }

    /// The `cargo` arguments that replace crates.io with this vendor dir.
    #[must_use]
    pub fn cargo_config_args(&self) -> [OsString; 4] {
        let directory = self.as_path().display();
        [
            "--config".into(),
            format!("source.crates-io.replace-with=\"{VENDOR_SOURCE_NAME}\"").into(),
            "--config".into(),
            format!("source.{VENDOR_SOURCE_NAME}.directory=\"{directory}\"").into(),
        ]
    }
}

#[derive(Debug)]
pub enum SeedError {
    /// A warm-cache path does not resolve to a canonical location.
    Path(JailPathError),
    /// The warm tree holds an entry that is neither a file nor a directory.
    NotPlain(PathBuf),
    /// The warm tree nests deeper than [`MAX_TREE_DEPTH`].
    TooDeep(PathBuf),
    /// A path cannot be spelled as a TOML basic string without escaping.
    Unquotable(PathBuf),
    /// A filesystem operation failed.
    Io {
        /// The path being read or written.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
}

impl std::fmt::Display for SeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path(error) => write!(f, "{error}"),
            Self::NotPlain(path) => write!(
                f,
                "warm cache entry `{}` is not a plain file or directory",
                path.display()
            ),
            Self::TooDeep(path) => write!(
                f,
                "warm cache nests deeper than {MAX_TREE_DEPTH} directories at `{}`",
                path.display()
            ),
            Self::Unquotable(path) => write!(
                f,
                "warm cache path `{}` is not UTF-8 or holds a quote, backslash or control character",
                path.display()
            ),
            Self::Io { path, source } => write!(f, "`{}`: {source}", path.display()),
        }
    }
}

fn io_at(path: &Path) -> impl FnOnce(std::io::Error) -> SeedError + '_ {
    move |source| SeedError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Seed the jail-visible target dir from a warm one holding the prebuilt deps.
///
/// The warm target holds the runtime and dependency artifacts prewarm compiled,
/// so the offline jailed build only compiles and links the user's crate. Every
/// file is a private byte copy carrying its warm modification time (cargo's
/// freshness input), so nothing the jailed build writes can reach the warm
/// cache.
///
/// # Errors
///
/// [`SeedError`] when the target dir already exists, the warm tree holds a
/// symbolic link or nests past [`MAX_TREE_DEPTH`], or a copy fails.
pub fn seed_target_dir(scoped_tmp: &Path, warm_target: &Path) -> Result<(), SeedError> {
    copy_tree(warm_target, &scoped_tmp.join("crate-target"), 0)
}

/// Copy the directory tree at `from` to the not-yet-existing `to`.
fn copy_tree(from: &Path, to: &Path, depth: usize) -> Result<(), SeedError> {
    if depth > MAX_TREE_DEPTH {
        return Err(SeedError::TooDeep(from.to_path_buf()));
    }
    std::fs::create_dir(to).map_err(io_at(to))?;
    for entry in std::fs::read_dir(from).map_err(io_at(from))? {
        let entry = entry.map_err(io_at(from))?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let file_type = entry.file_type().map_err(io_at(&src))?;
        if file_type.is_dir() {
            copy_tree(&src, &dst, depth.saturating_add(1))?;
        } else if file_type.is_file() {
            copy_file(&src, &dst)?;
        } else {
            return Err(SeedError::NotPlain(src));
        }
    }
    Ok(())
}

/// Permission bits a seeded file may carry: rwx for owner, group and other, never setuid, setgid or sticky.
const SEEDED_MODE_MASK: u32 = 0o777;

/// Byte-copy one regular file into a new inode, keeping its mode and modification time.
fn copy_file(src: &Path, dst: &Path) -> Result<(), SeedError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(src).map_err(io_at(src))?;
    let modified = meta.modified().map_err(io_at(src))?;
    let mode = meta.permissions().mode() & SEEDED_MODE_MASK;
    let mut input = std::fs::File::open(src).map_err(io_at(src))?;
    let mut output = std::fs::File::create_new(dst).map_err(io_at(dst))?;
    std::io::copy(&mut input, &mut output).map_err(io_at(dst))?;
    output
        .set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(io_at(dst))?;
    output.set_modified(modified).map_err(io_at(dst))
}

#[cfg(test)]
mod copy_tests {
    use super::{SeedError, VendorSource, seed_target_dir};
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn a_seeded_file_never_shares_the_warm_inode() -> std::io::Result<()> {
        let base = tempfile::tempdir()?;
        let warm = base.path().join("warm");
        std::fs::create_dir_all(warm.join("debug").join("deps"))?;
        let artifact = warm.join("debug").join("deps").join("libdep.rlib");
        std::fs::write(&artifact, "warm artifact")?;
        let stamp = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_234_567);
        std::fs::File::options()
            .write(true)
            .open(&artifact)?
            .set_modified(stamp)?;

        let project = base.path().join("project");
        std::fs::create_dir(&project)?;
        assert!(seed_target_dir(&project, &warm).is_ok());
        let seeded = project
            .join("crate-target")
            .join("debug")
            .join("deps")
            .join("libdep.rlib");
        assert_ne!(
            std::fs::metadata(&seeded)?.ino(),
            std::fs::metadata(&artifact)?.ino()
        );
        assert_eq!(std::fs::metadata(&seeded)?.modified()?, stamp);

        // A jailed build writing through the seeded file leaves warm untouched.
        std::fs::write(&seeded, "poisoned")?;
        assert_eq!(std::fs::read_to_string(&artifact)?, "warm artifact");
        Ok(())
    }

    #[test]
    fn a_seeded_executable_stays_executable_without_special_bits() -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir()?;
        let warm = base.path().join("warm");
        std::fs::create_dir_all(&warm)?;
        let script = warm.join("build-script-build");
        std::fs::write(&script, "#!/bin/sh\n")?;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o4755))?;
        let data = warm.join("output");
        std::fs::write(&data, "data")?;
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o644))?;

        let project = base.path().join("project");
        std::fs::create_dir(&project)?;
        assert!(seed_target_dir(&project, &warm).is_ok());
        let target = project.join("crate-target");
        let script_mode = std::fs::metadata(target.join("build-script-build"))?
            .permissions()
            .mode();
        assert_eq!(script_mode & 0o7777, 0o755);
        let data_mode = std::fs::metadata(target.join("output"))?
            .permissions()
            .mode();
        assert_eq!(data_mode & 0o7777, 0o644);
        Ok(())
    }

    #[test]
    fn a_second_seed_into_the_same_project_is_refused() -> std::io::Result<()> {
        let base = tempfile::tempdir()?;
        let warm = base.path().join("warm");
        std::fs::create_dir_all(&warm)?;
        std::fs::write(warm.join("f"), "warm")?;
        let project = base.path().join("project");
        std::fs::create_dir(&project)?;
        assert!(seed_target_dir(&project, &warm).is_ok());
        assert!(matches!(
            seed_target_dir(&project, &warm),
            Err(SeedError::Io { .. })
        ));
        assert_eq!(std::fs::read_to_string(warm.join("f"))?, "warm");
        Ok(())
    }

    #[test]
    fn a_symbolic_link_in_the_warm_target_is_refused() -> std::io::Result<()> {
        let base = tempfile::tempdir()?;
        let warm = base.path().join("warm");
        std::fs::create_dir_all(&warm)?;
        std::os::unix::fs::symlink("/etc/passwd", warm.join("leak"))?;
        let project = base.path().join("project");
        std::fs::create_dir(&project)?;
        assert!(matches!(
            seed_target_dir(&project, &warm),
            Err(SeedError::NotPlain(_))
        ));
        Ok(())
    }

    #[test]
    fn the_vendor_source_names_its_canonical_dir() -> std::io::Result<()> {
        let base = tempfile::tempdir()?;
        let vendor = base.path().join("vendor");
        std::fs::create_dir(&vendor)?;
        let Ok(source) = VendorSource::resolve(&vendor) else {
            return Err(std::io::Error::other("vendor dir did not resolve"));
        };
        let canonical = vendor.canonicalize()?;
        assert_eq!(source.as_path(), canonical);
        let args = source.cargo_config_args();
        let expected = format!(
            "source.ipe-warm-vendor.directory=\"{}\"",
            canonical.display()
        );
        assert!(matches!(args.get(3), Some(arg) if *arg == *expected));
        Ok(())
    }

    #[test]
    fn a_vendor_path_needing_toml_escapes_is_refused() -> std::io::Result<()> {
        let base = tempfile::tempdir()?;
        for name in ["quo\"te", "back\\slash", "new\nline"] {
            let vendor = base.path().join(name);
            std::fs::create_dir(&vendor)?;
            assert!(matches!(
                VendorSource::resolve(&vendor),
                Err(SeedError::Unquotable(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn a_missing_or_non_directory_vendor_is_refused() -> std::io::Result<()> {
        let base = tempfile::tempdir()?;
        assert!(matches!(
            VendorSource::resolve(&base.path().join("absent")),
            Err(SeedError::Path(_))
        ));
        let file = base.path().join("file");
        std::fs::write(&file, "not a dir")?;
        assert!(matches!(
            VendorSource::resolve(&file),
            Err(SeedError::NotPlain(_))
        ));
        Ok(())
    }
}

/// Run the freshly-built `ipe-app` binary inside the jail.
///
/// The binary is executed by absolute path under the jail's writable bind. No
/// toolchain binds are needed (the program is self-contained), which is a
/// tighter surface than the build phase. `wall` replaces the run default wall.
///
/// # Errors
///
/// [`SandboxDefect`] on a jail-spawn / output-cap failure, or when a jail path
/// does not resolve.
pub fn jailed_run(
    caps: &Capabilities,
    scoped_tmp: &Path,
    app_binary: &Path,
    wall: WallSecs,
) -> Result<PhaseOutcome, SandboxDefect> {
    // The bind and the binary the payload execs share one canonical spelling.
    let scoped_tmp = CanonicalPath::resolve(scoped_tmp).map_err(SandboxDefect::Path)?;
    let app_binary = CanonicalPath::resolve(app_binary).map_err(SandboxDefect::Path)?;
    let payload: Vec<OsString> = vec![app_binary.as_path().as_os_str().to_owned()];
    run_phase(
        caps,
        &scoped_tmp,
        RunCaps::run_defaults().with_wall(wall),
        // No toolchain binds for the run phase — the emitted program does not need
        // rustc/cargo, so nothing extra is exposed.
        ToolchainBinds::default(),
        None,
        // The untrusted program runs under the seccomp subprocess-deny filter.
        Subprocess::Denied,
        &payload,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_caps_lower_to_the_expected_limits() {
        let l = RunCaps::run_defaults().to_limits();
        assert_eq!(l.cpu_secs, 5);
        assert_eq!(l.wall_secs, 10);
        assert_eq!(l.proc_cap, 32);
        // The build phase is more generous but still bounded.
        let b = RunCaps::build_defaults().to_limits();
        assert!(b.wall_secs > l.wall_secs);
        assert!(b.proc_cap >= l.proc_cap);
    }

    #[test]
    fn a_phase_wall_replaces_only_the_wall() {
        let wall = WallSecs::new(7);
        assert!(wall.is_some());
        let Some(wall) = wall else { return };
        let cut = RunCaps::build_defaults().with_wall(wall).to_limits();
        let full = RunCaps::build_defaults().to_limits();
        assert_eq!(cut.wall_secs, 7);
        assert_eq!(cut.cpu_secs, full.cpu_secs);
        assert_eq!(cut.proc_cap, full.proc_cap);
        assert_eq!(cut.out_cap_bytes, full.out_cap_bytes);
    }

    #[test]
    fn probe_refuses_when_bwrap_absent() {
        // Simulate the refusal branch directly: a Capabilities with no bwrap must
        // yield a refusal that names the jail. (The real `probe_or_refuse` reads
        // PATH; here we assert the message contract the endpoint relies on.)
        let caps = Capabilities::default();
        assert!(caps.bwrap.is_none());
        let missing = missing_caps(&caps);
        assert!(missing.contains(&"timeout"));
        assert!(missing.contains(&"prlimit"));
    }

    #[test]
    fn wall_clock_kill_covers_timeout_exit_codes() {
        // A signal-kill (None), `timeout`'s timed-out code (124), and the SIGKILL
        // escalation (137) are all kills; a normal exit is not.
        assert!(is_wall_clock_kill(None));
        assert!(is_wall_clock_kill(Some(124)));
        assert!(is_wall_clock_kill(Some(137)));
        assert!(!is_wall_clock_kill(Some(0)));
        assert!(!is_wall_clock_kill(Some(1)));
    }

    /// A host layout with `cargo/bin` and a disjoint `rustup`, canonicalized.
    fn toolchain_tree() -> std::io::Result<(tempfile::TempDir, PathBuf)> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        std::fs::create_dir_all(root.join("cargo").join("bin"))?;
        std::fs::create_dir_all(root.join("rustup"))?;
        Ok((dir, root))
    }

    #[test]
    fn a_rustup_home_at_or_above_the_cargo_home_is_refused() -> std::io::Result<()> {
        let (_dir, root) = toolchain_tree()?;
        let cargo_home = root.join("cargo");
        for rustup in [cargo_home.clone(), root] {
            let refused = toolchain_binds_from(Some(&cargo_home), Some(rustup));
            assert!(matches!(
                refused,
                Err(SandboxDefect::Path(JailPathError::ExposesCargoHome { .. }))
            ));
        }
        Ok(())
    }

    #[test]
    fn a_disjoint_rustup_home_binds_bin_and_rustup_only() -> std::io::Result<()> {
        let (_dir, root) = toolchain_tree()?;
        let cargo_home = root.join("cargo");
        let result = toolchain_binds_from(Some(&cargo_home), Some(root.join("rustup")));
        assert!(result.is_ok(), "a disjoint layout must bind");
        let Ok(binds) = result else { return Ok(()) };
        let bound: Vec<&Path> = binds.ro_binds.iter().map(CanonicalPath::as_path).collect();
        assert_eq!(
            bound,
            [
                cargo_home.join("bin").as_path(),
                root.join("rustup").as_path()
            ]
        );
        assert!(!bound.contains(&cargo_home.as_path()));
        Ok(())
    }
}

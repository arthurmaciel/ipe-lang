//! The jailed build step for the opt-in "Run for real" endpoint.
//!
//! # The threat this closes
//!
//! The playground's `/compile` route runs `cargo build` on a crate emitted
//! from *untrusted user source*. Compiling Rust executes native code at build
//! time — `build.rs` build-scripts and proc-macros run as the host user
//! regardless of `--target wasm32`. `--target=wasm32` sandboxes the RUN (the
//! emitted artifact is wasm); it does nothing for the BUILD. So the build is
//! an RCE surface and must be jailed.
//!
//! # Reused, not reinvented
//!
//! This module does NOT invent a sandbox. It drives [`ipe_sandbox`] — the same
//! hardened bubblewrap + prlimit primitive the FFI crate-inspection path uses
//! to compile untrusted foreign crates. [`ipe_sandbox::bwrap_argv`] is a pure,
//! unit-tested argv builder that fails closed: it cannot represent an uncapped
//! jail (the wall clock and rlimits are non-optional), and a host missing
//! `bwrap`/`prlimit`/`timeout` is refused upstream ([`ipe_sandbox::probe`] +
//! [`ipe_sandbox::missing_caps`]) rather than run unsandboxed.
//!
//! # The jail this build runs in
//!
//! One [`ipe_sandbox::run_in_bwrap_jail`] invocation of the whole
//! `ipe build --target wasm` command, with [`ipe_sandbox::NetworkPolicy::Denied`]:
//!
//! * **network denied** — a fresh empty net namespace (`--unshare-net`); the
//!   build has zero sockets. A hostile `build.rs` cannot exfiltrate or fetch.
//! * **filesystem confined** — `/` is read-only; every home is masked by a
//!   tmpfs; the ONLY writable mount is a fresh per-request scratch dir. The
//!   toolchain (`~/.cargo`, `~/.rustup`), the `ipe` binary, the vendored
//!   runtime source, and the warm dependency target are re-exposed read-only.
//! * **resource-capped** — `prlimit` caps address space / CPU / open files /
//!   process count / file size; `timeout` caps wall clock. A fork-bomb hits
//!   `--nproc`, an OOM alloc hits `--as`, an infinite loop hits the wall clock
//!   — all SIGKILLed.
//! * **env scrubbed** — `--clearenv`; only a fixed allowlist re-enters, so no
//!   host secret in the environment reaches the build.
//! * **offline** — `CARGO_NET_OFFLINE=1` plus the pinned fixed dependency set
//!   (the emitted crate depends only on the in-repo runtime + browser-safe
//!   crates, already vendored/cached), so no crates.io fetch runs a hostile
//!   transitive `build.rs`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ipe_sandbox::{
    Capabilities, JailSpec, JailedOutput, NetworkPolicy, ResourceLimits, SandboxDefect,
    missing_caps, run_in_bwrap_jail,
};

// `bwrap_argv` is re-exported for the argv-rendering unit test below; the
// runtime path uses `run_in_bwrap_jail`, which builds the same argv internally.

/// Why a jailed build could not be established or run.
#[derive(Debug)]
pub enum BuildJailError {
    /// The host cannot host a sound jail (bwrap or a cap helper is absent) and
    /// the operator did not opt into an unsandboxed run. Carries the operator
    /// message to surface. Fail-closed: we never build unsandboxed silently.
    Unsupported(String),
    /// The jail was built but the jailed process failed to spawn / drained
    /// more output than the cap.
    Sandbox(SandboxDefect),
}

impl std::fmt::Display for BuildJailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(m) => write!(f, "{m}"),
            Self::Sandbox(d) => write!(f, "{d}"),
        }
    }
}

impl std::error::Error for BuildJailError {}

/// The read-only host paths the jailed `ipe build` needs re-exposed through the
/// home-tmpfs mask, and the warm target dir it writes dependency artifacts to.
///
/// All are resolved once at startup ([`crate` `AppState`]); a request never
/// chooses a path, so there is no path-injection surface here.
#[derive(Clone, Debug)]
pub struct BuildToolchain {
    /// The `ipe` binary (run inside the jail to emit + drive cargo).
    pub ipe_bin: PathBuf,
    /// The vendored runtime source root (`--runtime`), bound read-only.
    pub runtime_dir: PathBuf,
    /// Toolchain roots re-bound read-only after the `/home` tmpfs mask —
    /// typically `~/.cargo` (cargo/rustc/wasm-bindgen proxies) and `~/.rustup`
    /// (the actual toolchains + the wasm32 sysroot). Read-only: the build may
    /// execute the toolchain but never mutate it.
    pub toolchain_ro_binds: Vec<PathBuf>,
    /// Dirs prepended to the jail `PATH` (the toolchain `bin` dirs).
    pub path_prepend: Vec<PathBuf>,
    /// `RUSTUP_HOME` for the jailed toolchain proxies (env is scrubbed, so they
    /// cannot rediscover it from `$HOME`).
    pub rustup_home: Option<PathBuf>,
    /// The warm shared dependency target dir, bound WRITABLE so the fixed dep
    /// set is compiled once and reused. Only the trusted fixed deps land here;
    /// the user crate's own artifacts go to the per-request scratch dir.
    pub warm_target_dir: PathBuf,
}

/// The build request, already validated (size-capped) by the caller.
pub struct JailedBuild<'a> {
    /// The per-request scratch dir — the primary writable mount. The user
    /// source is written here (by the caller) and the emitted `out/` lands
    /// here.
    pub scratch: &'a Path,
    /// The `src/Main.ipe` entry inside `scratch`.
    pub entry: &'a Path,
    /// Where `ipe build` writes the emitted crate + bundle, inside `scratch`.
    pub out_dir: &'a Path,
    /// Resource caps for this build.
    pub limits: ResourceLimits,
}

/// Resource caps tuned for a single playground wasm build (not a whole SDK's
/// crate inspection like the FFI default). A warm target means only the user
/// crate recompiles, so these are tighter than [`ResourceLimits::default`].
#[must_use]
pub fn build_limits(wall_secs: u64) -> ResourceLimits {
    ResourceLimits {
        // 6 GiB address space: rustc + the wasm codegen map far more virtual
        // than resident; this SIGKILLs a runaway alloc while a real build's
        // resident set stays well under the host memory guard.
        rss_bytes: 6 * 1024 * 1024 * 1024,
        cpu_secs: wall_secs,
        wall_secs,
        fd_cap: 512,
        // cargo + rustc + build-script children of ONE user crate; a fork-bomb
        // trips this ceiling long before it exhausts host PIDs.
        proc_cap: 256,
        out_cap_bytes: 64 * 1024 * 1024,
    }
}

/// Probe the host for a sound build jail.
///
/// Returns the [`Capabilities`] when bwrap + both cap helpers are present.
///
/// # Errors
///
/// [`BuildJailError::Unsupported`] naming exactly what is missing when the host
/// cannot host a capped bwrap jail. The caller MUST refuse to enable the
/// endpoint (fail closed) rather than run the build unsandboxed.
pub fn probe_build_jail() -> Result<Capabilities, BuildJailError> {
    let caps = ipe_sandbox::probe();
    if caps.bwrap.is_none() {
        return Err(BuildJailError::Unsupported(
            "sandbox unavailable: `bwrap` (bubblewrap) is not on PATH; refusing to run \
             untrusted-source builds unsandboxed. Install bubblewrap to enable \
             IPE_PLAYGROUND_RUN."
                .to_owned(),
        ));
    }
    let missing = missing_caps(&caps);
    if !missing.is_empty() {
        return Err(BuildJailError::Unsupported(format!(
            "sandbox unavailable: mandatory cap helper(s) absent ({}); refusing to run \
             untrusted-source builds without a wall clock and rlimits. Install coreutils \
             (timeout) and util-linux (prlimit) to enable IPE_PLAYGROUND_RUN.",
            missing.join(", ")
        )));
    }
    Ok(caps)
}

/// Build the payload argv: `ipe build <entry> --out <out> --runtime <rt>
/// --target wasm`. No shell — a direct argv, so the quoting/injection class
/// does not exist.
fn build_payload(tc: &BuildToolchain, job: &JailedBuild<'_>) -> Vec<OsString> {
    vec![
        tc.ipe_bin.clone().into(),
        "build".into(),
        job.entry.into(),
        "--out".into(),
        job.out_dir.into(),
        "--runtime".into(),
        tc.runtime_dir.clone().into(),
        "--target".into(),
        "wasm".into(),
    ]
}

/// Assemble the [`JailSpec`] for one jailed wasm build.
///
/// The scratch dir is the primary writable mount; the warm target is added as
/// a second writable mount (`rw_binds`) so dependency artifacts persist across
/// requests (deps compiled once). `CARGO_TARGET_DIR` inside the jail points at
/// the warm target so cargo reuses it. All security-critical argv construction
/// stays inside the audited [`ipe_sandbox::bwrap_argv`] primitive.
fn jail_spec(tc: &BuildToolchain, job: &JailedBuild<'_>) -> JailSpec {
    let mut toolchain_ro_binds = tc.toolchain_ro_binds.clone();
    // The ipe binary and the runtime source must survive the /home tmpfs mask.
    toolchain_ro_binds.push(tc.ipe_bin.clone());
    toolchain_ro_binds.push(tc.runtime_dir.clone());
    JailSpec {
        network: NetworkPolicy::Denied,
        scoped_tmp: job.scratch.to_path_buf(),
        // No registry cache bind: the warm target already holds compiled deps,
        // and offline mode means no fetch. (A vendored registry could be added
        // here read-only if a cold build were ever wanted.)
        registry_cache: None,
        toolchain: None,
        toolchain_ro_binds,
        path_prepend: tc.path_prepend.clone(),
        rustup_home: tc.rustup_home.clone(),
        // The warm dependency target: a shared WRITABLE mount so the fixed dep
        // set compiles once and is reused. `CARGO_TARGET_DIR` points cargo at
        // it. The user crate's own artifacts land here too, but each build is
        // network-denied + fresh-scratch, so nothing user-controlled persists
        // beyond compiled object files in an operator-owned cache.
        rw_binds: vec![tc.warm_target_dir.clone()],
        setenvs: vec![(
            "CARGO_TARGET_DIR".to_owned(),
            tc.warm_target_dir.clone().into_os_string(),
        )],
        limits: job.limits,
    }
}

/// Run one jailed `ipe build --target wasm`.
///
/// The whole command runs inside a network-denied, resource-capped bubblewrap
/// jail with a read-only `/` and a single writable scratch dir (+ the shared
/// warm target). On success the emitted bundle is at `job.out_dir/www/`; the
/// caller reads it out of the scratch dir (which it owns and cleans up).
///
/// This is a thin driver over the audited [`ipe_sandbox::run_in_bwrap_jail`]
/// primitive — no argv is constructed here; the jail's isolation surface is
/// exactly the primitive's, unit-tested in `ipe_sandbox`.
///
/// # Errors
///
/// [`BuildJailError::Sandbox`] when the jail cannot spawn or the build
/// out-talks the output cap. A non-zero exit inside the jail (a compile
/// failure) is NOT an error here — it is returned in the [`JailedOutput`]
/// status for the caller to render as diagnostics.
pub fn run_jailed_build(
    caps: &Capabilities,
    tc: &BuildToolchain,
    job: &JailedBuild<'_>,
) -> Result<JailedOutput, BuildJailError> {
    let spec = jail_spec(tc, job);
    let payload = build_payload(tc, job);
    run_in_bwrap_jail(caps, &spec, &payload).map_err(BuildJailError::Sandbox)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toolchain() -> BuildToolchain {
        BuildToolchain {
            ipe_bin: PathBuf::from("/opt/ipe/bin/ipe"),
            runtime_dir: PathBuf::from("/opt/ipe/runtime"),
            toolchain_ro_binds: vec![
                PathBuf::from("/home/u/.cargo"),
                PathBuf::from("/home/u/.rustup"),
            ],
            path_prepend: vec![PathBuf::from("/home/u/.cargo/bin")],
            rustup_home: Some(PathBuf::from("/home/u/.rustup")),
            warm_target_dir: PathBuf::from("/home/u/.cache/ipe/warm"),
        }
    }

    fn job<'a>(scratch: &'a Path, entry: &'a Path, out: &'a Path) -> JailedBuild<'a> {
        JailedBuild {
            scratch,
            entry,
            out_dir: out,
            limits: build_limits(120),
        }
    }

    #[test]
    fn payload_is_ipe_build_wasm_with_no_shell() {
        let tc = toolchain();
        let j = job(
            Path::new("/scratch/req-1"),
            Path::new("/scratch/req-1/src/Main.ipe"),
            Path::new("/scratch/req-1/out"),
        );
        let p: Vec<String> = build_payload(&tc, &j)
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(p[0], "/opt/ipe/bin/ipe");
        assert_eq!(p[1], "build");
        assert_eq!(p[2], "/scratch/req-1/src/Main.ipe");
        assert!(p.contains(&"--target".to_owned()));
        assert!(p.contains(&"wasm".to_owned()));
        // No shell token anywhere.
        assert!(!p.iter().any(|a| a == "sh" || a == "-c" || a.contains("&&")));
    }

    #[test]
    fn jail_spec_denies_network_and_binds_toolchain_ro() {
        let tc = toolchain();
        let j = job(
            Path::new("/scratch/req-1"),
            Path::new("/scratch/req-1/src/Main.ipe"),
            Path::new("/scratch/req-1/out"),
        );
        let spec = jail_spec(&tc, &j);
        assert_eq!(spec.network, NetworkPolicy::Denied);
        assert_eq!(spec.scoped_tmp, PathBuf::from("/scratch/req-1"));
        // ipe bin + runtime + cargo + rustup all re-exposed read-only.
        assert!(spec.toolchain_ro_binds.contains(&PathBuf::from("/opt/ipe/bin/ipe")));
        assert!(spec.toolchain_ro_binds.contains(&PathBuf::from("/opt/ipe/runtime")));
        assert!(spec.toolchain_ro_binds.contains(&PathBuf::from("/home/u/.cargo")));
        assert!(spec.toolchain_ro_binds.contains(&PathBuf::from("/home/u/.rustup")));
    }

    #[test]
    fn rendered_argv_denies_network_scrubs_env_caps_and_mounts_warm_target_writable() {
        let tc = toolchain();
        let j = job(
            Path::new("/scratch/req-1"),
            Path::new("/scratch/req-1/src/Main.ipe"),
            Path::new("/scratch/req-1/out"),
        );
        let spec = jail_spec(&tc, &j);
        let payload = build_payload(&tc, &j);
        let argv = ipe_sandbox::bwrap_argv(
            Path::new("/usr/bin/bwrap"),
            Path::new("/usr/bin/prlimit"),
            Path::new("/usr/bin/timeout"),
            &spec,
            &payload,
        );
        let joined: Vec<String> = argv.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let s = joined.join(" ");
        // Wall clock wraps everything.
        assert!(s.starts_with("/usr/bin/timeout --kill-after=5s 120 /usr/bin/bwrap"), "{s}");
        // Network denied + env scrubbed + read-only root.
        assert!(argv.contains(&"--unshare-net".into()), "{s}");
        assert!(argv.contains(&"--clearenv".into()), "{s}");
        assert!(s.contains("--ro-bind / /"), "{s}");
        // Offline cargo.
        assert!(s.contains("--setenv CARGO_NET_OFFLINE 1"), "{s}");
        // Warm target spliced in as a WRITABLE bind + CARGO_TARGET_DIR, BEFORE
        // the prlimit payload separator (so it is bwrap's mount, not payload).
        assert!(
            s.contains("--bind /home/u/.cache/ipe/warm /home/u/.cache/ipe/warm"),
            "{s}"
        );
        assert!(s.contains("--setenv CARGO_TARGET_DIR /home/u/.cache/ipe/warm"), "{s}");
        // Resource caps present.
        assert!(s.contains("--as=6442450944"), "{s}");
        assert!(s.contains("--nproc=256"), "{s}");
        // Scratch is the writable per-request mount + chdir.
        assert!(s.contains("--bind /scratch/req-1 /scratch/req-1"), "{s}");
        assert!(s.contains("--chdir /scratch/req-1"), "{s}");
        // No shell.
        assert!(!s.contains("sh -c"), "{s}");
    }

    #[test]
    fn build_limits_are_tighter_than_the_ffi_default() {
        let l = build_limits(120);
        assert_eq!(l.wall_secs, 120);
        assert_eq!(l.cpu_secs, 120);
        assert_eq!(l.rss_bytes, 6 * 1024 * 1024 * 1024);
        assert_eq!(l.proc_cap, 256);
        // Tighter than the FFI default (10 GiB / 900 s / 512 procs).
        let def = ResourceLimits::default();
        assert!(l.rss_bytes < def.rss_bytes);
        assert!(l.wall_secs < def.wall_secs);
        assert!(l.proc_cap < def.proc_cap);
    }
}

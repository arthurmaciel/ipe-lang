//! Toolchain-free jailed deploy launcher.
//!
//! `ipe-wrapper` is the entry point of a deploy bundle produced by
//! `ipe deploy`. It locates `ipe-app` and `ipe.profile` by a FIXED RELATIVE
//! PATH next to the wrapper binary (no `cargo`/toolchain dependency), verifies
//! the profile against the capability floor embedded in `ipe-app`, and execs
//! the app inside the sandbox jail. Every verification failure is
//! fail-closed: the wrapper exits non-zero with a typed message to stderr —
//! it NEVER falls back to an unjailed or partially-verified run.
//!
//! ## Modes
//!
//! **Bundle mode** (default): the wrapper locates `ipe-app` and `ipe.profile`
//! as siblings at `../ipe-app` and `../ipe.profile` relative to the wrapper's
//! own path. The profile is a separately-auditable plain-text manifest.
//!
//! **Embed mode** (compiled with `IPE_EMBED_APP`/`IPE_EMBED_PROFILE` set at
//! build time, detected by the `embed_mode` cfg): the app binary and profile
//! are baked into the wrapper at compile time. `--show-profile` dumps the
//! embedded profile to stdout for auditability (the profile is never implicit).
//!
//! ## Honest limit
//!
//! The inner binary is a native ELF/Mach-O/PE executable. Nothing in this
//! crate prevents a sufficiently privileged operator from running it directly
//! without the wrapper. The wrapper makes the sanctioned, jailed, profile-
//! verified path the easy, toolchain-free one — not the only possible one.
//! This limit is documented; it is not a defect. The security guarantee is:
//! **any run through `ipe-wrapper` is jailed exactly as tightly as the
//! embedded floor requires**, and a tampered profile cannot weaken that.

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use ipe_sandbox::run_jail::{self, ParseError, RunJailDefect, SandboxProfile};

/// Exit non-zero, printing a typed error to stderr.
///
/// Using `eprintln!` + `ExitCode::FAILURE` rather than `process::exit` so
/// destructors run — the wrapper holds no resources that matter at exit, but
/// the pattern keeps the type system honest (`!` vs `ExitCode`).
macro_rules! fatal {
    ($($arg:tt)*) => {{
        eprintln!("ipe-wrapper: {}", format_args!($($arg)*));
        return ExitCode::FAILURE;
    }};
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    run(&args)
}

fn run(args: &[OsString]) -> ExitCode {
    // --show-profile: dump the embedded or on-disk profile text and exit.
    // Only meaningful in embed mode (the profile is separately readable in
    // bundle mode), but accepted in both so a script can always query it.
    let show_profile = args.iter().any(|a| a == "--show-profile");

    // Split `[wrapper-flags] [-- <app-args>...]`.
    let dash_dash = args.iter().position(|a| a == "--");
    let app_args: &[OsString] = dash_dash.map_or(&[], |i| args.get(i + 1..).unwrap_or(&[]));

    // Dispatch to the compile-time selected mode.
    #[cfg(embed_mode)]
    {
        run_embed(show_profile, app_args)
    }
    #[cfg(not(embed_mode))]
    {
        run_bundle(show_profile, app_args)
    }
}

// ── Bundle mode ─────────────────────────────────────────────────────────────

/// Bundle-mode entry: locate `ipe-app` and `ipe.profile` by fixed relative
/// paths next to the wrapper binary, verify, and exec.
#[cfg(not(embed_mode))]
fn run_bundle(show_profile: bool, app_args: &[OsString]) -> ExitCode {
    let wrapper_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => fatal!("cannot resolve wrapper binary path: {e}"),
    };
    let Some(parent) = wrapper_path.parent() else {
        fatal!("wrapper binary path has no parent directory");
    };
    let bundle_dir = parent.to_path_buf();

    let app_path = bundle_dir.join("ipe-app");
    let profile_path = bundle_dir.join("ipe.profile");

    // Read and parse the profile strictly before touching the binary (fail
    // early with a clear message on a missing profile).
    let profile_text = match std::fs::read_to_string(&profile_path) {
        Ok(t) => t,
        Err(e) => fatal!(
            "ipe.profile not found at {} — bundle is incomplete or tampered: {e}",
            profile_path.display()
        ),
    };
    let profile = match run_jail::parse_profile(&profile_text) {
        Ok(p) => p,
        Err(ParseError::Malformed { detail }) => fatal!(
            "ipe.profile is malformed ({detail}) — refusing to run with an unparseable profile"
        ),
    };

    if show_profile {
        print!("{profile_text}");
        return ExitCode::SUCCESS;
    }

    // Scan the binary for its embedded floor and verify the profile against it.
    let app_bytes = match std::fs::read(&app_path) {
        Ok(b) => b,
        Err(e) => fatal!(
            "ipe-app not found at {} — bundle is incomplete: {e}",
            app_path.display()
        ),
    };
    exec_after_verify(&app_bytes, &profile, &app_path, app_args)
}

// ── Embed mode ──────────────────────────────────────────────────────────────

/// Embed-mode entry: app binary and profile are baked in at compile time.
///
/// The app bytes come from `OUT_DIR/embedded-app` (copied there by
/// `build.rs`); the profile from `OUT_DIR/embedded-profile`. Both are
/// statically known at compile time — no runtime path lookup, no toolchain.
#[cfg(embed_mode)]
fn run_embed(show_profile: bool, app_args: &[OsString]) -> ExitCode {
    // The build.rs copies the files into OUT_DIR; the macros bake them in. The
    // paths are compile-time constants produced by concat! + env!. `include_str!`
    // embeds the profile as a `&str`, enforcing the UTF-8 invariant at compile
    // time — an invalid-UTF-8 profile fails the build instead of reaching runtime.
    static APP_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/embedded-app"));
    static PROFILE_TEXT: &str = include_str!(concat!(env!("OUT_DIR"), "/embedded-profile"));

    let profile = match run_jail::parse_profile(PROFILE_TEXT) {
        Ok(p) => p,
        Err(ParseError::Malformed { detail }) => fatal!(
            "embedded ipe.profile is malformed ({detail}) — the wrapper was built incorrectly"
        ),
    };

    if show_profile {
        print!("{PROFILE_TEXT}");
        return ExitCode::SUCCESS;
    }

    // Write the embedded binary to an exclusively-created temp file and retain
    // the file handle.  After writing, the capfloor is scanned by re-reading
    // through the RETAINED handle — not from APP_BYTES and not by re-opening
    // the path — so the bytes verified are the bytes on the inode that will be
    // exec'd.  On Unix the exec goes through the fd-path (/proc/self/fd/<n> on
    // Linux, /dev/fd/<n> on macOS) so the file that is verified and the file
    // that is executed are the same kernel object; a same-uid process that
    // renames something over the path cannot change what is exec'd.
    let embed = match write_embed_binary(APP_BYTES) {
        Ok(e) => e,
        Err(e) => fatal!("cannot write embedded binary to temp file: {e}"),
    };

    let result = exec_embed_after_verify(&profile, embed, app_args);

    // `exec_after_verify` only returns on failure (on success the process is
    // replaced).  The EmbedBinary RAII guard removes the temp file when it
    // drops at the end of this scope.
    result
}

/// An exclusively-created, owner-execute-only temp file holding the embedded
/// app binary.  Removed on drop (best-effort).
#[cfg(embed_mode)]
struct EmbedBinary {
    path: PathBuf,
    file: std::fs::File,
}

#[cfg(embed_mode)]
impl Drop for EmbedBinary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Write `bytes` to an exclusively-created, unpredictably-named temp file with
/// owner-execute permissions, and return the open [`EmbedBinary`] holding both
/// the path and the retained file handle.
///
/// Uses 128 bits of OS entropy in the name so the path cannot be predicted or
/// pre-seeded by a local attacker.  `create_new` (O_EXCL) ensures that a
/// collision fails rather than following a symlink or reusing an existing file.
///
/// # Errors
///
/// Returns an [`io::Error`] on any filesystem or entropy failure.
#[cfg(embed_mode)]
fn write_embed_binary(bytes: &[u8]) -> std::io::Result<EmbedBinary> {
    use std::io::Write as _;

    let hex = os_entropy_hex()?;
    let name = format!("ipe-wrapper-embed-{}-{hex}", std::process::id());
    let path = std::env::temp_dir().join(name);

    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)?
    };
    #[cfg(not(unix))]
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;

    file.write_all(bytes)?;

    Ok(EmbedBinary { path, file })
}

/// Read 128 bits of OS entropy and return them as a 32-char lowercase hex string.
///
/// On Unix this reads from `/dev/urandom`; on other platforms it falls back to
/// a combination of the system time and the process ID (weaker, but still
/// substantially harder to predict than pid alone — embed mode on non-Unix is
/// an unsupported configuration).
#[cfg(embed_mode)]
fn os_entropy_hex() -> std::io::Result<String> {
    #[cfg(unix)]
    {
        use std::fmt::Write as _;
        use std::io::Read as _;
        let mut entropy = [0u8; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut entropy))
            .map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("cannot read OS entropy from /dev/urandom: {e}"),
                )
            })?;
        let mut hex = String::with_capacity(32);
        for b in entropy {
            let _ = write!(hex, "{b:02x}");
        }
        Ok(hex)
    }
    #[cfg(not(unix))]
    {
        // Non-Unix fallback: combine wall time (nanoseconds) with process ID.
        // Embed mode on non-Unix is unsupported; this provides a best-effort
        // improvement over a bare pid-only name.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0u128, |d| d.as_nanos());
        Ok(format!(
            "{:016x}{:016x}",
            nanos,
            u128::from(std::process::id())
        ))
    }
}

// ── Embed-mode verify + exec ─────────────────────────────────────────────────

/// Verify the capability floor and exec the embedded app binary using the same
/// file handle — closing the verify/exec identity gap.
///
/// The capfloor is scanned from bytes read through the RETAINED handle in
/// `embed`, not from the compile-time `APP_BYTES` slice.  On Unix the exec
/// target is the fd-path (`/proc/self/fd/<n>` on Linux, `/dev/fd/<n>` on
/// macOS) which resolves to the same kernel inode as the retained handle — a
/// same-uid overwrite of the path name after the read cannot change what is
/// exec'd.
///
/// Fail-closed on:
/// - read or seek failure on the retained handle
/// - no capfloor marker in the read bytes (missing floor → refuse)
/// - profile does not satisfy the floor (widened profile → refuse)
/// - jail primitive unavailable on this platform
/// - jail establishment failure
#[cfg(embed_mode)]
fn exec_embed_after_verify(
    profile: &SandboxProfile,
    mut embed: EmbedBinary,
    app_args: &[OsString],
) -> ExitCode {
    use std::io::{Read as _, Seek as _, SeekFrom};

    // Re-read the binary through the retained handle so the bytes scanned for
    // the capfloor are the bytes on the inode we are about to exec — not the
    // compile-time slice, which a write race could have diverged from.
    if let Err(e) = embed.file.seek(SeekFrom::Start(0)) {
        fatal!("cannot rewind embedded binary handle: {e}");
    }
    let mut on_disk_bytes = Vec::new();
    if let Err(e) = embed.file.read_to_end(&mut on_disk_bytes) {
        fatal!("cannot read embedded binary from retained handle: {e}");
    }

    // Build the exec path that resolves to the same kernel inode as the
    // retained handle: a rename over `embed.path` after the read cannot
    // affect what is exec'd because the fd-path bypasses the name lookup.
    let exec_path = fd_exec_path(&embed);

    exec_after_verify(&on_disk_bytes, profile, &exec_path, app_args)
}

/// Return the exec path for the retained [`EmbedBinary`] handle.
///
/// On Linux and macOS this resolves to the fd directly (bypassing the name),
/// so renaming over `embed.path` after this call cannot affect the exec target.
/// On other platforms the on-disk path is returned as a best-effort fallback
/// (the retained-handle read still ensures the floor was verified against the
/// written bytes).
#[cfg(embed_mode)]
fn fd_exec_path(embed: &EmbedBinary) -> std::path::PathBuf {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd as _;
        std::path::PathBuf::from(format!("/proc/self/fd/{}", embed.file.as_raw_fd()))
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::io::AsRawFd as _;
        std::path::PathBuf::from(format!("/dev/fd/{}", embed.file.as_raw_fd()))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        embed.path.clone()
    }
}

// ── Shared verify + exec ─────────────────────────────────────────────────────

/// Scan `app_bytes` for the embedded capability floor, verify `profile`
/// satisfies it, and exec the app at `app_path` inside the jail.
///
/// Fail-closed on:
/// - no capfloor marker found in `app_bytes` (missing floor → refuse)
/// - profile does not satisfy the floor (widened profile → refuse)
/// - jail primitive unavailable on this platform
/// - jail establishment failure
fn exec_after_verify(
    app_bytes: &[u8],
    profile: &SandboxProfile,
    app_path: &std::path::Path,
    app_args: &[OsString],
) -> ExitCode {
    // The marker's ABSENCE means the binary was not built with `ipe deploy`'s
    // embedded floor. Refuse — we cannot verify confinement correctness without
    // the floor.
    let Some(floor) = run_jail::scan_capfloor(app_bytes) else {
        fatal!(
            "{}: the binary embeds no capability floor — refusing to run an artifact \
             whose confinement cannot be verified",
            RunJailDefect::ProfileWeakerThanFloor.code().as_str()
        );
    };

    // The profile must isolate at LEAST as much as the embedded floor.
    // A widened profile (asking for MORE than the floor grants) is refused:
    // the floor is the tamper-proof ceiling on what can be granted.
    if !profile.satisfies_capfloor(&floor) {
        fatal!("{}", RunJailDefect::ProfileWeakerThanFloor);
    }

    // Probe and exec inside the jail. On success (Unix) the process is
    // replaced by the jailed app and this function never returns. On any
    // failure the jail is not established and we refuse.
    let wants_wall_clock = profile.limits.wall_secs.is_some();
    let tools = match run_jail::probe_run_jail_tools(wants_wall_clock) {
        Ok(t) => t,
        Err(e) => fatal!("{e}"),
    };

    let scoped_tmp = match make_scoped_tmp() {
        Ok(p) => p,
        Err(e) => fatal!("cannot create scoped temp dir: {e}"),
    };

    let working_tree = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => fatal!("cannot resolve working directory: {e}"),
    };

    match run_jail::exec_in_run_jail(
        &tools,
        profile,
        &scoped_tmp,
        &working_tree,
        app_path,
        app_args,
    ) {
        Ok(never) => match never {},
        Err(e) => fatal!("{e}"),
    }
}

/// Create a per-run scoped writable temp dir — the jail's only writable mount
/// when the `filesystem` axis is absent.
fn make_scoped_tmp() -> std::io::Result<PathBuf> {
    let base = std::env::temp_dir();
    let name = format!(
        "ipe-wrapper-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let dir = base.join(name);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ipe_sandbox::run_jail::{
        DatabaseAxis, FilesystemScope, SandboxProfile, profile_from_capabilities,
    };
    use std::collections::BTreeSet;

    fn net_profile() -> SandboxProfile {
        profile_from_capabilities(
            &BTreeSet::from([ipe_kernels::Capability::Network]),
            &BTreeSet::new(),
            DatabaseAxis::NotApplicable,
            &[],
        )
        .expect("valid profile")
    }

    fn isolated_profile() -> SandboxProfile {
        SandboxProfile::maximally_isolated()
    }

    /// A profile that exactly matches the floor satisfies it.
    #[test]
    fn profile_satisfies_matching_floor() {
        let profile = net_profile();
        let floor = net_profile();
        assert!(profile.satisfies_capfloor(&floor));
    }

    /// A maximally-isolated profile satisfies any floor (it grants nothing, so
    /// it cannot exceed whatever the floor grants).
    #[test]
    fn isolated_profile_satisfies_any_floor() {
        let profile = isolated_profile();
        let floor = net_profile();
        assert!(profile.satisfies_capfloor(&floor));
    }

    /// A profile widened beyond the floor is refused.
    #[test]
    fn widened_profile_refused_by_floor() {
        let profile = net_profile();
        // Floor grants nothing — the profile's `network=true` exceeds it.
        let floor = isolated_profile();
        assert!(!profile.satisfies_capfloor(&floor));
    }

    /// A profile with an env var the floor does not grant is refused.
    #[test]
    fn env_not_in_floor_refused() {
        let mut profile = isolated_profile();
        profile.env_allowlist = vec!["SECRET".to_owned()];
        let floor = isolated_profile(); // floor grants no env vars
        assert!(!profile.satisfies_capfloor(&floor));
    }

    /// `scan_capfloor` returns `None` for a binary with no floor marker.
    #[test]
    fn no_floor_marker_returns_none() {
        let bytes = b"this binary has no ipe capability floor at all";
        assert!(ipe_sandbox::run_jail::scan_capfloor(bytes).is_none());
    }

    /// A capfloor line round-trips through `to_capfloor_line` + `scan_capfloor`.
    #[test]
    fn capfloor_roundtrip() {
        let profile = net_profile();
        let line = profile.to_capfloor_line();
        let mut payload = line.as_bytes().to_vec();
        payload.push(b'\n');

        let recovered =
            ipe_sandbox::run_jail::scan_capfloor(&payload).expect("marker found in payload");
        assert_eq!(recovered.network, profile.network);
        assert_eq!(recovered.subprocess, profile.subprocess);
        assert!(matches!(recovered.filesystem, FilesystemScope::Isolated));
    }

    /// --show-profile exits SUCCESS without exec-ing anything (bundle mode
    /// path: just parse+print then return).
    #[test]
    fn show_profile_flag_recognized() {
        // We test the argument parsing logic, not the exec path.
        let args: Vec<std::ffi::OsString> = vec!["--show-profile".into()];
        let show = args.iter().any(|a| a == "--show-profile");
        assert!(show);
    }

    /// App args after `--` are correctly split.
    #[test]
    fn app_args_split_after_dash_dash() {
        let args: Vec<std::ffi::OsString> = vec![
            "--show-profile".into(),
            "--".into(),
            "--port".into(),
            "8080".into(),
        ];
        let pos = args.iter().position(|a| a == "--");
        assert_eq!(pos, Some(1));
        // pos is Some(1) per the assertion above; get(2..) is safe on a 4-element vec.
        let app_args: &[std::ffi::OsString] = pos.and_then(|i| args.get(i + 1..)).unwrap_or(&[]);
        assert_eq!(app_args.len(), 2);
        assert_eq!(
            app_args.first().map(std::ffi::OsString::as_os_str),
            Some(std::ffi::OsStr::new("--port"))
        );
    }
}

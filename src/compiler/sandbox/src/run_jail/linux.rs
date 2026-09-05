//! The Linux (`x86_64`/`aarch64`) run jail: the `bwrap`+seccomp arms plus the raw
//! `memfd`/`fcntl` FFI they rest on. Compiled only on the supported Linux
//! targets; every other target gets the refuse stubs in [`super`].

#![cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{
    RunJailDefect, RunJailTools, SandboxProfile, run_jail_argv, run_jail_argv_with_delivery,
};
use crate::seccomp;

/// Probe the host for the run-jail primitives and decide whether a jail can be
/// built, returning the tools or the fail-closed refusal.
///
/// The required primitives are per-OS, so this function is cfg-split to match the
/// same platforms [`exec_in_run_jail`] confines: on Linux (`x86_64`/`aarch64`) it requires
/// `bwrap` + `prlimit` (+ `timeout` when the profile sets a wall clock); on macOS
/// it requires `sandbox-exec`. Off both it is the refuse-gap. `wants_wall_clock`
/// selects whether `timeout` is additionally required (Linux only).
///
/// # Errors
///
/// [`RunJailDefect::UnsupportedPlatform`] off every jailed target;
/// [`RunJailDefect::PrimitiveUnavailable`] when a required primitive is absent.
pub fn probe_run_jail_tools(wants_wall_clock: bool) -> Result<RunJailTools, RunJailDefect> {
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
    // The probes above guarantee these are `Some`.
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

/// Return `true` when bwrap can successfully establish a `--unshare-net`
/// namespace on this host.
///
/// Unprivileged user namespaces on some Linux configurations (notably GitHub
/// Actions runners) cannot configure loopback inside a net namespace — bwrap
/// auto-runs `RTM_NEWADDR` to bring up 127.0.0.1 and the kernel rejects it
/// with `EPERM`, killing bwrap before any payload executes. Callers that
/// require `--unshare-net` should call this first and skip (not fail) when it
/// returns `false`.
///
/// This is a capability check, not an isolation bypass: the `--unshare-net`
/// flag itself is never removed from the real jail argv; the result only
/// determines whether the host can even start the jail.
#[must_use]
pub fn netns_jail_available(bwrap: &std::path::Path) -> bool {
    // Run bwrap with the minimal net-isolation flags, wrapping /bin/true.
    // Exit 0 → the netns came up cleanly. Any non-zero (including the
    // "loopback: Failed RTM_NEWADDR: Operation not permitted" bwrap error)
    // → the netns jail cannot be established on this host.
    std::process::Command::new(bwrap)
        .args(["--unshare-net", "--ro-bind", "/", "/", "--", "/bin/true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
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

    // Re-expose the app binary's directory read-only past the home/tmp tmpfs
    // masks (a `CARGO_TARGET_DIR` under `~/.cache` is otherwise hidden). The
    // binary's *parent* is bound so a relocated dynamic loader path or a
    // co-located artifact resolves; the read-only bind means the app can exec
    // it but never mutate it.
    let mut extra_ro_binds: Vec<PathBuf> = Vec::new();
    if let Some(app_dir) = app.parent() {
        extra_ro_binds.push(app_dir.to_path_buf());
    }

    let host_env = |k: &str| std::env::var_os(k);
    let argv = run_jail_argv(
        tools,
        profile,
        scoped_tmp,
        working_tree,
        &extra_ro_binds,
        Some(seccomp_fd),
        &host_env,
        &payload,
    );

    let (program_path, rest) = argv.split_first().ok_or_else(|| RunJailDefect::Spawn {
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

/// Exec an embedded app held only in a sealed anonymous descriptor.
///
/// Delivering the app into the jail from that descriptor rather than from a host
/// path closes the verify/exec identity gap for `ipe-wrapper` embed mode.
///
/// The wrapper writes the embedded bytes to a sealed memfd
/// ([`write_sealed_app_memfd`]), verifies the capability floor by reading the
/// SEALED fd, then calls this. bwrap inherits the (non-cloexec, sealed) fd
/// across the process replacement and materialises the app inside the jail via
/// `--file` at a fixed sandbox path — so the bytes executed are provably the
/// sealed bytes that were verified; a same-uid attacker has no host path to
/// pre-seed or swap.
///
/// # Errors
///
/// Any [`RunJailDefect`]; on success (Linux) it does not return.
pub fn exec_embedded_in_run_jail(
    tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    app: &SealedApp,
    app_args: &[OsString],
) -> Result<std::convert::Infallible, RunJailDefect> {
    use std::os::unix::process::CommandExt as _;

    let Some(program) = seccomp::subprocess_deny_program(profile.subprocess) else {
        return Err(RunJailDefect::UnsupportedPlatform {
            reason: "no seccomp filter can be compiled for this architecture",
        });
    };
    let bytes = seccomp::program_bytes(&program);
    let seccomp_fd = write_seccomp_memfd(&bytes)?;
    let app_fd = app.as_raw_fd();

    // The in-jail path the app is materialised at. It sits under `scoped_tmp`,
    // the one always-writable bind, so bwrap can create it after the mounts.
    let dest = scoped_tmp.join("ipe-app");

    let mut payload: Vec<OsString> = Vec::with_capacity(app_args.len() + 1);
    payload.push(dest.as_os_str().to_owned());
    payload.extend(app_args.iter().cloned());

    let host_env = |k: &str| std::env::var_os(k);
    let argv = run_jail_argv_with_delivery(
        tools,
        profile,
        scoped_tmp,
        working_tree,
        &[],
        Some(seccomp_fd),
        Some((app_fd, &dest)),
        &host_env,
        &payload,
    );

    let (program_path, rest) = argv.split_first().ok_or_else(|| RunJailDefect::Spawn {
        detail: "empty jail argv".to_owned(),
    })?;
    let mut cmd = std::process::Command::new(program_path);
    cmd.args(rest);
    // Both the seccomp filter fd and the sealed app fd MUST survive the exec so
    // bwrap can read them; clear their close-on-exec flags right before exec.
    let seccomp_fd_move = seccomp_fd;
    let app_fd_move = app_fd;
    // SAFETY: `pre_exec` runs in the child between fork and exec (here it is the
    // process-replacing `exec`, so there is no fork — the closure runs in this
    // process just before execve). `clear_cloexec` performs only
    // async-signal-safe `fcntl` calls on owned fds; a failure aborts the exec,
    // so a jail that could not un-cloexec a required fd refuses rather than
    // running the app without its filter or without a delivered binary.
    unsafe {
        cmd.pre_exec(move || {
            clear_cloexec(seccomp_fd_move)?;
            clear_cloexec(app_fd_move)?;
            Ok(())
        });
    }
    let err = cmd.exec();
    Err(RunJailDefect::Spawn {
        detail: err.to_string(),
    })
}

// The two `fcntl` operations the pre_exec hook needs, wrapped so the raw
// `extern "C"` surface is contained. `FD_CLOEXEC` is the close-on-exec flag.
const FD_CLOEXEC: i32 = 1;

unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn memfd_create(name: *const core::ffi::c_char, flags: core::ffi::c_uint) -> i32;
    fn write(fd: i32, buf: *const core::ffi::c_void, count: usize) -> isize;
    fn read(fd: i32, buf: *mut core::ffi::c_void, count: usize) -> isize;
    fn lseek(fd: i32, offset: i64, whence: i32) -> i64;
    fn close(fd: i32) -> i32;
}

// memfd sealing constants (`<linux/memfd.h>` / `<linux/fcntl.h>`). A sealing
// memfd is created with `MFD_ALLOW_SEALING`; `F_ADD_SEALS` then applies the
// seal set. `F_SEAL_SEAL` forbids further seals — after it the byte content and
// size are frozen and cannot be re-opened writable by anyone holding the fd.
const MFD_ALLOW_SEALING: core::ffi::c_uint = 0x0002;
const F_ADD_SEALS: i32 = 1033;
const F_SEAL_SEAL: i32 = 0x0001;
const F_SEAL_SHRINK: i32 = 0x0002;
const F_SEAL_GROW: i32 = 0x0004;
const F_SEAL_WRITE: i32 = 0x0008;

const F_GETFD: i32 = 1;
const F_SETFD: i32 = 2;

fn libc_fcntl_getfd(fd: i32) -> std::io::Result<i32> {
    // SAFETY: a plain fcntl(F_GETFD) query on an owned fd; no memory is touched.
    let r = unsafe { fcntl(fd, F_GETFD) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(r)
}

fn libc_fcntl_setfd(fd: i32, flags: i32) -> std::io::Result<()> {
    // SAFETY: fcntl(F_SETFD, flags) on an owned fd; the variadic arg is a plain
    // int as the ABI requires.
    let r = unsafe { fcntl(fd, F_SETFD, flags) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Clear the close-on-exec flag on `fd` so an inherited fd (the seccomp memfd)
/// survives an exec. Async-signal-safe — safe to call from a `pre_exec` hook.
///
/// Shared by the run jail's own launcher and the captured-child build jail's
/// subprocess-denied variant, so the fd-inheritance handling is defined once.
///
/// # Errors
///
/// [`std::io::Error`] when either `fcntl` fails.
pub fn clear_cloexec(fd: i32) -> std::io::Result<()> {
    let flags = libc_fcntl_getfd(fd)?;
    libc_fcntl_setfd(fd, flags & !FD_CLOEXEC)
}

/// Write the compiled seccomp program to an anonymous in-memory file and return
/// its file descriptor, rewound to offset 0, ready for `bwrap --seccomp <fd>`.
///
/// A `memfd` is used rather than a temp file so the program bytes never touch
/// the filesystem (nothing to race or tamper on disk) and the fd is
/// self-cleaning when closed.
///
/// # Errors
///
/// [`RunJailDefect::Spawn`] on any `memfd_create`, write, or seek failure — a
/// truncated or unwritten filter would be a malformed seccomp program, so the
/// jail refuses rather than run unfiltered.
pub fn write_seccomp_memfd(bytes: &[u8]) -> Result<i32, RunJailDefect> {
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
        let Some(remaining) = bytes.get(written..) else {
            break;
        };
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

/// A sealed anonymous file holding the embedded app binary, owned by its raw
/// descriptor.  The descriptor is closed on drop.
///
/// The bytes are frozen by `F_SEAL_WRITE | F_SEAL_SHRINK | F_SEAL_GROW |
/// F_SEAL_SEAL`, so what a caller verifies by reading the fd is exactly what the
/// jail delivers from the same fd — there is no on-disk name to race, and no
/// writable re-open is possible even for a process holding the fd.
pub struct SealedApp {
    fd: i32,
}

impl SealedApp {
    /// The raw descriptor of the sealed anonymous file.
    #[must_use]
    pub const fn as_raw_fd(&self) -> i32 {
        self.fd
    }

    /// Read the full sealed contents by reading through the fd.
    ///
    /// Reads from offset 0 without disturbing the caller's later use of the fd
    /// (bwrap re-reads it from 0 itself via `--file`, but this rewinds after to
    /// be safe).  The bytes returned are the sealed bytes — the same inode the
    /// jail will deliver.
    ///
    /// # Errors
    ///
    /// [`RunJailDefect::Spawn`] on any seek or read failure.
    pub fn read_sealed_bytes(&self) -> Result<Vec<u8>, RunJailDefect> {
        let spawn = |detail: String| RunJailDefect::Spawn { detail };
        // SAFETY: lseek to absolute offset 0 (SEEK_SET = 0) on the owned fd.
        if unsafe { lseek(self.fd, 0, 0) } < 0 {
            return Err(spawn(format!(
                "rewinding the sealed app memfd failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        let mut out: Vec<u8> = Vec::new();
        // Heap-allocated read buffer (a large on-stack array is a stack-size
        // hazard).
        let mut chunk = vec![0u8; 65536];
        loop {
            // SAFETY: `read` writes at most `chunk.len()` bytes into the owned,
            // fully-initialised `chunk` buffer; the pointer and length describe
            // exactly that buffer.
            let n = unsafe {
                read(
                    self.fd,
                    chunk.as_mut_ptr().cast::<core::ffi::c_void>(),
                    chunk.len(),
                )
            };
            if n < 0 {
                return Err(spawn(format!(
                    "reading the sealed app memfd failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            if n == 0 {
                break;
            }
            let read_len = usize::try_from(n).unwrap_or(0);
            if let Some(slice) = chunk.get(..read_len) {
                out.extend_from_slice(slice);
            }
        }
        // Rewind so a subsequent consumer reads from the start.
        // SAFETY: lseek to absolute offset 0 on the owned fd.
        if unsafe { lseek(self.fd, 0, 0) } < 0 {
            return Err(spawn(format!(
                "rewinding the sealed app memfd after read failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(out)
    }
}

impl Drop for SealedApp {
    fn drop(&mut self) {
        // SAFETY: `close` on the owned fd; after this the descriptor is not used.
        unsafe {
            close(self.fd);
        }
    }
}

/// Write `bytes` to an anonymous, sealing-capable in-memory file, seal it
/// against any further write/resize, and return the owned [`SealedApp`].
///
/// The returned fd is NON-close-on-exec so it is inherited across the
/// wrapper→bwrap process replacement, letting bwrap materialise the app inside
/// the jail from the same sealed inode via `--file`.  Sealing (`F_SEAL_WRITE |
/// F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL`) makes the verified-then-executed
/// bytes provably identical: no path lookup, no writable re-open.
///
/// # Errors
///
/// [`RunJailDefect::Spawn`] on any syscall failure.
pub fn write_sealed_app_memfd(bytes: &[u8]) -> Result<SealedApp, RunJailDefect> {
    let spawn = |detail: String| RunJailDefect::Spawn { detail };
    let name = c"ipe-embedded-app";
    // SAFETY: `memfd_create` with a valid NUL-terminated name and the
    // `MFD_ALLOW_SEALING` flag returns a new fd or -1; no memory is shared.
    // `MFD_CLOEXEC` is deliberately NOT set: the fd must survive the exec into
    // bwrap so bwrap can read the app from it.
    let fd = unsafe { memfd_create(name.as_ptr(), MFD_ALLOW_SEALING) };
    if fd < 0 {
        return Err(spawn(format!(
            "memfd_create for the embedded app failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    let sealed = SealedApp { fd };
    // Write the whole binary. A short write is a hard error — a truncated app
    // would be a corrupt executable, so refuse.
    let mut written: usize = 0;
    while written < bytes.len() {
        let Some(remaining) = bytes.get(written..) else {
            break;
        };
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
                "writing the embedded app to the memfd failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        written += usize::try_from(n).unwrap_or(0);
    }
    // Seal against write, shrink, grow, and further sealing. After this the
    // byte content and size are frozen for the lifetime of the fd.
    let seals = F_SEAL_WRITE | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL;
    // SAFETY: fcntl(F_ADD_SEALS, seals) on the owned sealing-capable memfd; the
    // variadic arg is a plain int as the ABI requires.
    if unsafe { fcntl(fd, F_ADD_SEALS, seals) } < 0 {
        return Err(spawn(format!(
            "sealing the embedded app memfd failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    // Rewind so the first reader (verification scan) starts at the beginning.
    // SAFETY: lseek to absolute offset 0 (SEEK_SET = 0) on the owned fd.
    if unsafe { lseek(fd, 0, 0) } < 0 {
        return Err(spawn(format!(
            "rewinding the embedded app memfd failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(sealed)
}

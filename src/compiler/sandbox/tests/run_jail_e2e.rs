//! End-to-end proof of the run jail's OS-boundary containment — the SEAL's
//! security half.
//!
//! These are REAL jailed runs (they spawn `bwrap`), so they are gated behind
//! `IPE_E2E=1` and skip cleanly when bubblewrap or the cap helpers are absent.
//! They prove the two load-bearing properties directly at the kernel boundary,
//! not by the app's own choice:
//!
//! - **Fail-closed on the undeclared.** A network connect from an isolated
//!   (network-absent) jail fails — the fresh empty net namespace has no route
//!   off-host. A `fork`/subprocess from a subprocess-absent jail is EPERM'd by
//!   the seccomp filter.
//! - **No false-deny.** A thread-spawning program boots under the isolated jail
//!   (the seccomp filter allows the thread-create path), and a network connect
//!   from a network-GRANTED jail is not blocked by the namespace.

#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
// This is an integration test harness: `expect`/`unwrap` on setup steps make a
// mis-set-up test fail loudly (the correct behavior for a test), and the raw
// FFI + slice handling mirror the production `run_jail` glue it exercises.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::redundant_closure_for_method_calls,
    clippy::map_unwrap_or
)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use ipe_sandbox::run_jail::{
    FilesystemScope, RunJailTools, RunResourceLimits, SandboxProfile, run_jail_argv,
};

/// Serialize the jailed runs: each creates a `memfd` and clears its
/// close-on-exec flag in a `pre_exec` hook, which is a process-global fd-table
/// mutation — running two in parallel races on fd numbers. A single lock makes
/// the whole harness deterministic regardless of `--test-threads`.
static JAIL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Skip unless `IPE_E2E=1` and the jail tools are present.
fn e2e_tools() -> Option<RunJailTools> {
    if std::env::var_os("IPE_E2E").is_none_or(|v| v != "1") {
        return None;
    }
    let caps = ipe_sandbox::probe();
    let bwrap = caps.bwrap?;
    let prlimit = caps.prlimit?;
    Some(RunJailTools {
        bwrap,
        prlimit,
        timeout: caps.timeout,
    })
}

/// Compile the seccomp program for a profile and place it on an inheritable fd,
/// then build + run the jail argv for `payload`, returning the exit status code.
///
/// This mirrors `exec_in_run_jail` but *spawns* (waits) instead of `exec`-ing,
/// so the test can assert on the outcome. The seccomp fd is created with
/// `memfd_create` and its close-on-exec flag cleared so `bwrap` inherits it.
fn run_jailed(tools: &RunJailTools, profile: &SandboxProfile, payload: &[OsString]) -> Option<i32> {
    use std::os::unix::io::FromRawFd as _;
    use std::os::unix::process::CommandExt as _;

    // Hold the global lock across the whole spawn — the memfd + cloexec-clear is
    // a process-wide fd-table mutation.
    let _guard = JAIL_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let program = ipe_sandbox::seccomp::subprocess_deny_program(profile.subprocess)
        .expect("x86_64 seccomp program");
    let bytes = ipe_sandbox::seccomp::program_bytes(&program);

    // memfd for the seccomp program.
    let fd = unsafe { memfd_create(c"ipe-seccomp-test".as_ptr(), 0) };
    assert!(fd >= 0, "memfd_create failed");
    let mut written = 0usize;
    while written < bytes.len() {
        let n = unsafe { write(fd, bytes[written..].as_ptr().cast(), bytes.len() - written) };
        assert!(n > 0, "write to memfd failed");
        written += usize::try_from(n).unwrap_or(0);
    }
    unsafe { lseek(fd, 0, 0) };

    let scoped = std::env::temp_dir().join(format!("ipe-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&scoped).expect("scoped tmp");
    let host_env = |k: &str| std::env::var_os(k);
    let argv = run_jail_argv(
        tools,
        profile,
        &scoped,
        &scoped,
        &[PathBuf::from("/usr/bin"), PathBuf::from("/bin")],
        Some(fd),
        &host_env,
        payload,
    );
    let (prog, rest) = argv.split_first().expect("non-empty argv");
    let mut cmd = Command::new(prog);
    cmd.args(rest);
    let fd_copy = fd;
    unsafe {
        cmd.pre_exec(move || {
            // Clear close-on-exec so bwrap inherits the seccomp fd.
            let flags = fcntl(fd_copy, 1); // F_GETFD
            if flags < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if fcntl(fd_copy, 2, flags & !1) < 0 {
                // F_SETFD, clear FD_CLOEXEC
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let status = cmd.status().expect("spawn jailed process");
    // Reap the memfd.
    drop(unsafe { std::fs::File::from_raw_fd(fd) });
    let _ = std::fs::remove_dir_all(&scoped);
    status.code()
}

unsafe extern "C" {
    fn memfd_create(name: *const core::ffi::c_char, flags: core::ffi::c_uint) -> i32;
    fn write(fd: i32, buf: *const core::ffi::c_void, n: usize) -> isize;
    fn lseek(fd: i32, off: i64, whence: i32) -> i64;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

fn isolated() -> SandboxProfile {
    SandboxProfile::maximally_isolated()
}

fn net_granted() -> SandboxProfile {
    SandboxProfile {
        network: true,
        ..SandboxProfile::maximally_isolated()
    }
}

fn subprocess_granted() -> SandboxProfile {
    SandboxProfile {
        subprocess: true,
        filesystem: FilesystemScope::WorkingTreeReadWrite,
        limits: RunResourceLimits::default(),
        ..SandboxProfile::maximally_isolated()
    }
}

#[test]
fn undeclared_network_is_denied_at_the_os_boundary() {
    let Some(tools) = e2e_tools() else { return };
    // A bash /dev/tcp connect to a public resolver. In an isolated jail the
    // fresh empty net namespace has no route → the connect fails (non-zero).
    let payload: Vec<OsString> = ["/bin/bash", "-c", "exec 3<>/dev/tcp/1.1.1.1/53"]
        .iter()
        .map(OsString::from)
        .collect();
    let code = run_jailed(&tools, &isolated(), &payload);
    assert_ne!(
        code,
        Some(0),
        "an isolated jail must NOT reach the network (fail-closed)"
    );
}

#[test]
fn declared_network_reaches_the_network() {
    let Some(tools) = e2e_tools() else { return };
    // The SAME connect, in a network-GRANTED jail, is not blocked by the
    // namespace. (If the host itself has no outbound route this is skipped by
    // checking the unjailed baseline first.)
    let baseline = Command::new("/bin/bash")
        .args(["-c", "exec 3<>/dev/tcp/1.1.1.1/53"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !baseline {
        return; // no outbound route on this host — nothing to prove.
    }
    let payload: Vec<OsString> = ["/bin/bash", "-c", "exec 3<>/dev/tcp/1.1.1.1/53"]
        .iter()
        .map(OsString::from)
        .collect();
    let code = run_jailed(&tools, &net_granted(), &payload);
    assert_eq!(
        code,
        Some(0),
        "a network-granted jail must reach the network (no false-deny)"
    );
}

#[test]
fn undeclared_subprocess_fork_is_denied() {
    let Some(tools) = e2e_tools() else { return };
    // `/bin/true` via a fork+exec shell: bash forks a child to exec it. In a
    // subprocess-absent jail the fork/vfork is EPERM'd → the spawn fails.
    // Force a real fork: two commands so bash cannot exec-optimize into the
    // single child (a lone external command is exec-replaced, no fork). The
    // `$(…)` substitution forks a subshell — exactly the fork the jail denies.
    let payload: Vec<OsString> = ["/bin/bash", "-c", "echo $(/bin/true); /bin/true"]
        .iter()
        .map(OsString::from)
        .collect();
    let code = run_jailed(&tools, &isolated(), &payload);
    assert_ne!(
        code,
        Some(0),
        "a subprocess-absent jail must deny fork/exec of a child (fail-closed)"
    );
}

#[test]
fn granted_subprocess_can_fork() {
    let Some(tools) = e2e_tools() else { return };
    // Same forced-fork shape as the negative test, but subprocess granted → the
    // fork/exec succeeds.
    let payload: Vec<OsString> = ["/bin/bash", "-c", "echo $(/bin/true); /bin/true"]
        .iter()
        .map(OsString::from)
        .collect();
    let code = run_jailed(&tools, &subprocess_granted(), &payload);
    assert_eq!(
        code,
        Some(0),
        "a subprocess-granted jail must permit fork/exec (no false-deny)"
    );
}

#[test]
fn a_thread_spawning_program_boots_under_the_isolated_jail() {
    let Some(tools) = e2e_tools() else { return };
    // Prove the seccomp filter's clone3/clone-thread allowance does not
    // false-deny a threaded program. `nproc` (coreutils) reads /proc and the CPU
    // affinity; a simpler thread probe is a python one-liner that spawns a
    // thread (CPython uses pthread_create → clone3 on glibc>=2.34).
    if ipe_sandbox::probe().bwrap.is_none() {
        return;
    }
    let py = "/usr/bin/python3";
    if !Path::new(py).exists() {
        return;
    }
    let payload: Vec<OsString> = [
        py,
        "-c",
        "import threading,sys; t=threading.Thread(target=lambda: None); t.start(); t.join(); print('ok')",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    let code = run_jailed(&tools, &isolated(), &payload);
    assert_eq!(
        code,
        Some(0),
        "a threaded program must boot under the isolated jail (threads allowed)"
    );
}

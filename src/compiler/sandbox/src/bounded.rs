//! Plain-child runs bounded by one byte cap and one wall-clock deadline.
//!
//! The child leads its own process group, so the deadline, a cap breach, and
//! the post-exit sweep stop every process it forked, not only the direct
//! child.

use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::{DrainOutcome, JailedOutput, SandboxDefect, Stream, read_bounded};

/// How long a killed group gets to close its pipes before the drain abandons it.
const KILL_GRACE: Duration = Duration::from_secs(2);

/// Poll interval while waiting for a child whose pipes are already closed.
const EXIT_POLL: Duration = Duration::from_millis(10);

/// Run `cmd` as a plain child, capturing both streams under one byte cap and
/// stopping its whole process group at `wall`.
///
/// Stdin is closed and both output streams are piped, whatever `cmd` set. The
/// streams are drained concurrently, so a stream-heavy child never wedges the
/// parent. On unix the child leads a fresh process group: expiry and a cap
/// breach kill that group, and once the direct child exits any process it left
/// behind in the group is killed before the child is reaped.
///
/// # Errors
///
/// [`SandboxDefect::Spawn`] when the child cannot start or a stream cannot be
/// read; [`SandboxDefect::OutputCapExceeded`] when either stream out-talks
/// `cap_bytes`; [`SandboxDefect::WallClockExceeded`] when the child is still
/// running (or its pipes are still held open) at `wall`.
pub fn run_captured_bounded(
    cmd: &mut Command,
    cap_bytes: u64,
    wall: Duration,
) -> Result<JailedOutput, SandboxDefect> {
    let program = cmd.get_program().to_os_string();
    let spawn_err = |detail: String| SandboxDefect::Spawn {
        program: program.to_string_lossy().into_owned(),
        detail,
    };
    let started = Instant::now();
    let Some(deadline) = started.checked_add(wall) else {
        return Err(SandboxDefect::WallClockExceeded { wall });
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| spawn_err(e.to_string()))?;
    let drained = drain_until(&mut child, cap_bytes, deadline);
    let timed_out = drained.timed_out || !wait_until(&mut child, deadline);
    // The direct child has exited (or been killed) but is not yet reaped, so
    // its pid still names the group: sweep anything it left behind.
    stop_group(&mut child);
    let status = child.wait().map_err(|e| spawn_err(e.to_string()))?;
    if timed_out {
        return Err(SandboxDefect::WallClockExceeded { wall });
    }
    if let Some(e) = drained.read_error {
        return Err(spawn_err(e.to_string()));
    }
    match (drained.stdout, drained.stderr) {
        (Some(stdout), Some(stderr)) if !drained.cap_exceeded => Ok(JailedOutput {
            status: status.code(),
            stdout,
            stderr,
        }),
        // A breach, or a stream that never reported: fail closed on the cap
        // defect rather than fabricate an empty-output success.
        _ => Err(SandboxDefect::OutputCapExceeded { cap_bytes }),
    }
}

/// What the concurrent drain of one child's two streams produced.
struct Drained {
    stdout: Option<Vec<u8>>,
    stderr: Option<Vec<u8>>,
    cap_exceeded: bool,
    timed_out: bool,
    read_error: Option<std::io::Error>,
}

/// Drain both streams under `cap` until both close or `deadline` passes.
///
/// A cap breach, a read error, or the deadline kills the group at once. A
/// group that still holds a pipe open [`KILL_GRACE`] after being killed (a
/// process that left the group) is abandoned: its drain thread is detached
/// rather than joined, so the caller's bound holds.
fn drain_until(child: &mut Child, cap: u64, deadline: Instant) -> Drained {
    let (tx, rx) = mpsc::channel::<DrainOutcome>();
    let mut threads = Vec::with_capacity(2);
    for (stream, handle) in [
        (Stream::Stdout, child.stdout.take().map(Pipe::Out)),
        (Stream::Stderr, child.stderr.take().map(Pipe::Err)),
    ] {
        let tx = tx.clone();
        threads.push(std::thread::spawn(move || {
            let _ = tx.send(DrainOutcome {
                stream,
                result: read_bounded(handle, cap),
            });
        }));
    }
    drop(tx);
    let mut drained = Drained {
        stdout: None,
        stderr: None,
        cap_exceeded: false,
        timed_out: false,
        read_error: None,
    };
    let mut received = 0_u8;
    let mut abandoned = false;
    while received < 2 {
        let budget = if drained.timed_out {
            KILL_GRACE
        } else {
            deadline.saturating_duration_since(Instant::now())
        };
        match rx.recv_timeout(budget) {
            Ok(outcome) => {
                received = received.saturating_add(1);
                match outcome.result {
                    Ok(Some(bytes)) => match outcome.stream {
                        Stream::Stdout => drained.stdout = Some(bytes),
                        Stream::Stderr => drained.stderr = Some(bytes),
                    },
                    Ok(None) => {
                        drained.cap_exceeded = true;
                        stop_group(child);
                    }
                    Err(e) => {
                        drained.read_error = Some(e);
                        stop_group(child);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) if !drained.timed_out => {
                drained.timed_out = true;
                stop_group(child);
            }
            Err(RecvTimeoutError::Timeout) => {
                abandoned = true;
                break;
            }
            // Every thread sends before it returns: a closed channel means
            // every report is in.
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    if !abandoned {
        for thread in threads {
            let _ = thread.join();
        }
    }
    drained
}

/// One of the child's two output pipes, as one readable type.
enum Pipe {
    Out(std::process::ChildStdout),
    Err(std::process::ChildStderr),
}

impl std::io::Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Out(out) => out.read(buf),
            Self::Err(err) => err.read(buf),
        }
    }
}

/// Wait for the direct child to exit without reaping it; `false` when
/// `deadline` passed first (the group is then killed).
fn wait_until(child: &mut Child, deadline: Instant) -> bool {
    loop {
        match exited_unreaped(child) {
            Ok(true) => return true,
            Ok(false) => {}
            // The child can no longer be observed: stop it and report expiry,
            // never an unbounded wait.
            Err(_) => {
                stop_group(child);
                return false;
            }
        }
        if Instant::now() >= deadline {
            stop_group(child);
            return false;
        }
        std::thread::sleep(EXIT_POLL);
    }
}

/// Whether the direct child has exited, leaving it unreaped so its pid keeps
/// naming its process group.
#[cfg(unix)]
fn exited_unreaped(child: &Child) -> std::io::Result<bool> {
    use rustix::process::{Pid, WaitId, WaitidOptions, waitid};
    let options = WaitidOptions::EXITED | WaitidOptions::NOHANG | WaitidOptions::NOWAIT;
    waitid(WaitId::Pid(Pid::from_child(child)), options)
        .map(|status| status.is_some())
        .map_err(std::io::Error::from)
}

/// Whether the direct child has exited (no process groups off unix).
#[cfg(not(unix))]
fn exited_unreaped(child: &mut Child) -> std::io::Result<bool> {
    child.try_wait().map(|status| status.is_some())
}

/// Kill the child's whole process group, then the child itself.
///
/// Called only while the child is unreaped, so its pid cannot have been
/// reused for another group.
fn stop_group(child: &mut Child) {
    #[cfg(unix)]
    {
        use rustix::process::{Pid, Signal, kill_process_group};
        let _ = kill_process_group(Pid::from_child(child), Signal::Kill);
    }
    let _ = child.kill();
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::run_captured_bounded;
    use crate::SandboxDefect;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        cmd
    }

    /// Whether the process whose pid is the first line of `stdout` is gone.
    fn stray_is_gone(pid: &str) -> bool {
        let proc = std::path::Path::new("/proc").join(pid.trim());
        (0..200).any(|_| {
            let gone = !proc.exists()
                || std::fs::read_to_string(proc.join("stat"))
                    .is_ok_and(|stat| stat.split_whitespace().nth(2) == Some("Z"));
            if !gone {
                std::thread::sleep(Duration::from_millis(10));
            }
            gone
        })
    }

    #[test]
    fn a_child_that_exits_returns_its_output() {
        let out = run_captured_bounded(&mut sh("printf hello"), 1024, Duration::from_secs(30));
        assert!(matches!(out, Ok(ref o) if o.stdout == b"hello" && o.status == Some(0)));
    }

    #[test]
    fn a_child_past_its_wall_is_refused_promptly() {
        let started = Instant::now();
        let out = run_captured_bounded(&mut sh("sleep 30"), 1024, Duration::from_millis(300));
        assert!(matches!(out, Err(SandboxDefect::WallClockExceeded { .. })));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_grandchild_holding_the_pipe_is_killed_at_the_wall() {
        let Ok(dir) = crate::test_dir::TestDir::new("bounded-wall") else {
            return;
        };
        let pid_file = dir.path().join("pid");
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let started = Instant::now();
        let out = run_captured_bounded(&mut sh(&script), 1024, Duration::from_millis(500));
        assert!(matches!(out, Err(SandboxDefect::WallClockExceeded { .. })));
        assert!(started.elapsed() < Duration::from_secs(10));
        let pid = std::fs::read_to_string(&pid_file).unwrap_or_default();
        assert!(!pid.trim().is_empty());
        assert!(
            stray_is_gone(&pid),
            "the backgrounded grandchild outlived the wall"
        );
    }

    #[test]
    fn a_stray_left_behind_by_an_exited_child_is_swept() {
        let out = run_captured_bounded(
            &mut sh("sleep 30 >/dev/null 2>&1 & echo $!"),
            1024,
            Duration::from_secs(30),
        );
        assert!(out.is_ok(), "the child exits at once: {out:?}");
        let Ok(out) = out else {
            return;
        };
        let pid = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(!pid.trim().is_empty());
        assert!(stray_is_gone(&pid), "the stray outlived its parent's run");
    }

    #[test]
    fn a_child_past_the_output_cap_is_refused() {
        let out = run_captured_bounded(
            &mut sh("head -c 262144 /dev/zero >&2; echo done"),
            64 * 1024,
            Duration::from_secs(30),
        );
        assert!(matches!(out, Err(SandboxDefect::OutputCapExceeded { .. })));
    }
}

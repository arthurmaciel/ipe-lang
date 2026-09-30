//! Parent-death floor contract for `system::spawn_hardened{,_tokio}`.
//!
//! `PR_SET_PDEATHSIG` fires when the THREAD that forked the child exits, so a
//! hardened child must be forked by the process-lifetime spawner thread:
//!
//! 1. a child requested from a thread that has since exited (a plain
//!    `std::thread`, or a reaped tokio blocking-pool thread) stays alive;
//! 2. a child whose parent PROCESS is `SIGKILL`ed dies with it;
//! 3. the tokio entry point refuses outside a runtime rather than spawning.
#![cfg(target_os = "linux")]

use ipe_runtime_rust::system::spawn_hardened;
use std::io::BufRead as _;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Set on the re-executed probe; its presence selects probe mode.
const PROBE_MODE_ENV: &str = "IPE_PDEATH_PROBE";

/// Prefix of the stdout line on which the probe reports its hardened child's pid.
const PROBE_PID_PREFIX: &str = "ipe-pdeath-probe-pid=";

/// How long a child must outlive its requesting thread to count as alive.
const OUTLIVE: Duration = Duration::from_millis(300);

/// Ceiling on every poll in this file.
const POLL_CEILING: Duration = Duration::from_secs(10);

fn sleep_30() -> Command {
    let mut cmd = Command::new("/bin/sleep");
    cmd.arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

fn kill_and_reap(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn a_child_outlives_the_thread_that_requested_it() {
    let mut child = std::thread::spawn(|| spawn_hardened(sleep_30()))
        .join()
        .expect("requesting thread")
        .expect("hardened spawn");
    std::thread::sleep(OUTLIVE);
    let running = child.try_wait().expect("poll child").is_none();
    kill_and_reap(child);
    assert!(
        running,
        "the child must outlive the thread that requested it"
    );
}

/// The pid's `/proc` state letter, or `None` once the pid no longer exists.
fn proc_state(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = stat.rsplit_once(')')?;
    rest.trim_start().chars().next()
}

#[test]
fn a_hardened_child_dies_with_its_killed_parent() {
    #[allow(clippy::disallowed_methods)] // an integration test has no crate-private env accessor
    let probe_mode = std::env::var_os(PROBE_MODE_ENV).is_some();
    if probe_mode {
        // Probe mode: spawn the hardened grandchild, report its pid on stdout,
        // then block on it until the outer test SIGKILLs this probe.
        let mut child = spawn_hardened(sleep_30()).expect("probe hardened spawn");
        println!("{PROBE_PID_PREFIX}{}", child.id());
        let _ = child.wait();
        return;
    }

    let mut probe = Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "a_hardened_child_dies_with_its_killed_parent",
            "--exact",
            "--nocapture",
        ])
        .env(PROBE_MODE_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("re-exec probe");
    let stdout = probe.stdout.take().expect("probe stdout pipe");

    // The reader ends at EOF: the probe is killed below, and the grandchild
    // holds no copy of the pipe.
    let (pid_tx, pid_rx) = std::sync::mpsc::channel::<u32>();
    let reader = std::thread::spawn(move || {
        let reported = std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .find_map(|line| line.strip_prefix(PROBE_PID_PREFIX)?.trim().parse().ok());
        if let Some(pid) = reported {
            let _ = pid_tx.send(pid);
        }
    });
    let grandchild = pid_rx.recv_timeout(POLL_CEILING).ok();
    let _ = probe.kill();
    let _ = probe.wait();
    reader.join().expect("probe stdout reader");
    let grandchild = grandchild.expect("the probe must report its hardened child's pid");

    let killed = Instant::now();
    let died = loop {
        match proc_state(grandchild) {
            None | Some('Z') => break true,
            Some(_) if killed.elapsed() >= POLL_CEILING => break false,
            Some(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    if !died {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &grandchild.to_string()])
            .status();
    }
    assert!(
        died,
        "a hardened child must die when its parent process is killed"
    );
}

#[cfg(feature = "web")]
mod tokio_entry {
    use super::OUTLIVE;
    use ipe_runtime_rust::system::{SpawnRefusal, spawn_hardened_tokio};
    use std::time::Duration;

    fn tokio_sleep_30() -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("/bin/sleep");
        cmd.arg("30").kill_on_drop(true);
        cmd
    }

    #[test]
    fn a_child_outlives_the_reaped_blocking_thread_that_requested_it() {
        let keep_alive = Duration::from_millis(50);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_keep_alive(keep_alive)
            .enable_all()
            .build()
            .expect("runtime");
        let running = rt.block_on(async {
            let mut child = tokio::task::spawn_blocking(|| spawn_hardened_tokio(tokio_sleep_30()))
                .await
                .expect("blocking task")
                .expect("hardened tokio spawn");
            tokio::time::sleep(keep_alive + OUTLIVE).await;
            let running = child.try_wait().expect("poll child").is_none();
            let _ = child.start_kill();
            let _ = child.wait().await;
            running
        });
        assert!(running, "the child must outlive the reaped blocking thread");
    }

    /// Called straight from a task on a current-thread runtime (the web
    /// console-proxy shape), the spawn completes: the spawner registers the
    /// child with the runtime while its only thread waits for the reply.
    #[test]
    fn a_current_thread_runtime_spawns_without_deadlock() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let status = rt.block_on(async {
            let mut cmd = tokio::process::Command::new("/bin/true");
            cmd.kill_on_drop(true);
            let mut child = spawn_hardened_tokio(cmd).expect("hardened tokio spawn");
            tokio::time::timeout(Duration::from_secs(10), child.wait()).await
        });
        let status = status
            .expect("the child must be reaped in time")
            .expect("wait");
        assert!(status.success(), "hardened /bin/true must exit 0");
    }

    #[test]
    fn the_tokio_entry_refuses_outside_a_runtime() {
        let refused = spawn_hardened_tokio(tokio::process::Command::new("/bin/true"));
        assert!(
            matches!(refused, Err(SpawnRefusal::NoRuntime)),
            "{refused:?}"
        );
    }
}

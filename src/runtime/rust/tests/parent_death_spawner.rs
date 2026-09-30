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
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Names the file the re-executed probe writes its hardened child's pid to.
const PROBE_PID_FILE_ENV: &str = "IPE_PDEATH_PROBE_PID_FILE";

/// How long a child must outlive its requesting thread to count as alive.
const OUTLIVE: Duration = Duration::from_millis(300);

/// Ceiling on every poll in this file.
const POLL_CEILING: Duration = Duration::from_secs(10);

fn sleep_30() -> Command {
    let mut cmd = Command::new("/bin/sleep");
    cmd.arg("30").stdin(Stdio::null());
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
    if let Some(pid_file) = std::env::var_os(PROBE_PID_FILE_ENV) {
        // Probe mode: spawn the hardened grandchild, publish its pid, then block
        // on it until the outer test SIGKILLs this probe.
        let mut child = spawn_hardened(sleep_30()).expect("probe hardened spawn");
        let staged = std::path::PathBuf::from(&pid_file).with_extension("staged");
        std::fs::write(&staged, child.id().to_string()).expect("stage pid");
        std::fs::rename(&staged, &pid_file).expect("publish pid");
        let _ = child.wait();
        return;
    }

    let pid_file =
        std::env::temp_dir().join(format!("ipe-pdeath-probe-{}.pid", std::process::id()));
    let _ = std::fs::remove_file(&pid_file);
    let mut probe = Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "a_hardened_child_dies_with_its_killed_parent",
            "--exact",
            "--nocapture",
        ])
        .env(PROBE_PID_FILE_ENV, &pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("re-exec probe");

    let started = Instant::now();
    let grandchild = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            break Some(pid);
        }
        if started.elapsed() >= POLL_CEILING {
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let _ = probe.kill();
    let _ = probe.wait();
    let _ = std::fs::remove_file(&pid_file);
    let grandchild = grandchild.expect("the probe must publish its hardened child's pid");

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

    #[test]
    fn the_tokio_entry_refuses_outside_a_runtime() {
        let refused = spawn_hardened_tokio(tokio::process::Command::new("/bin/true"));
        assert!(
            matches!(refused, Err(SpawnRefusal::NoRuntime)),
            "{refused:?}"
        );
    }
}

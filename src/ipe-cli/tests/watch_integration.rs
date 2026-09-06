#![forbid(unsafe_code)]
//! End-to-end proofs for `ipe watch` (`crate::watch`). The cancellation proof
//! lives separately in
//! `watch_cancellation.rs`, deterministically, since racing a real
//! file-save against warm salsa recompute — which is DELIBERATELY fast —
//! is not a reliable timing window for an E2E test).
//!
//! Gated on `IPE_E2E=1` exactly like `server_e2e.rs`: every scenario here
//! drives a REAL `cargo build` of the emitted project and spawns the
//! resulting binary, so these are slow (first build pays the full
//! dependency-compile cost) but honest — no mocked compiler, no mocked
//! process supervisor.
//!
//! ```text
//! IPE_E2E=1 cargo nextest run -p ipe --test watch_integration
//! ```

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ipe::watch::{WatchEvent, WatchHandle, WatchOptions};

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A minimal `Ipe.Http.Server` fixture, parameterised on the response body
/// so a test can edit it in place and observe the swap. Reads its port from
/// `IPE_SERVER_PORT` — the SAME convention `server_e2e.rs` already
/// establishes in this repo, and what `watch::child_env` drives from
/// `WatchOptions::port`.
fn server_fixture(body: &str) -> String {
    format!(
        "module Main exposing (main)\n\n\
         import Ipe.Http.Server as Server\n\
         import Ipe.Maybe\n\
         import Ipe.String\n\
         import Ipe.System\n\
         import Ipe.Task\n\n\
         main =\n    \
             let port = Maybe.withDefault 8080 (String.toInt (System.getenvOr \"IPE_SERVER_PORT\" \"8080\"))\n    \
             in\n    \
             Server.listen port\n        \
                 [ Server.get \"/\" (\\req -> Task.succeed (Server.text \"{body}\")) ]\n"
    )
}

/// A DELIBERATELY unparseable `.ipe` file — a dangling `let` with no `in`,
/// which fails at parse time (never reaches type-check, let alone emit).
const BROKEN_SOURCE: &str = "module Main exposing (main)\n\nmain =\n    let x = 1\n";

fn fresh_dirs(tag: &str) -> Result<(PathBuf, PathBuf), BoxError> {
    let base = std::env::temp_dir().join(format!(
        "watch_e2e_{tag}_{}_{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    let ipe_dir = base.join("ipe");
    let out_dir = base.join("out");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&ipe_dir)
        .map_err(|e| -> BoxError { format!("mkdir {}: {e}", ipe_dir.display()).into() })?;
    Ok((ipe_dir, out_dir))
}

/// Poll `GET / HTTP/1.1` on `127.0.0.1:port` for up to `timeout`, returning
/// `true` the moment the body matches `want`, or `false` on timeout.
/// Mirrors `server_e2e.rs`'s own raw-socket polling — no extra HTTP
/// dependency.
///
/// Every caller's cold-build budget is generous on purpose: `start_watch`'s
/// `cargo build` is a genuinely isolated build (no shared cargo target — a
/// real `ipe watch` session must not silently reuse a stale one), competing
/// for CPU with every other test nextest runs in parallel. A tight deadline
/// here fails on scheduler contention, not on a real regression.
fn wait_for_body(port: u16, want: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if http_get_body(port).is_some_and(|body| body.contains(want)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn http_get_body(port: u16) -> Option<String> {
    let mut stream = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().ok()?,
        Duration::from_millis(200),
    )
    .ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok()?;
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .ok()?;
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    text.split("\r\n\r\n").nth(1).map(str::to_owned)
}

/// A thread-safe sink for [`WatchEvent`]s, used to count `RebuildStarted`s
/// for the coalescing proof without capturing stderr or racing timing.
#[derive(Clone, Default)]
struct EventSink(Arc<Mutex<Vec<WatchEvent>>>);

impl EventSink {
    fn push(&self, event: WatchEvent) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }

    fn count_rebuild_started(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|e| matches!(e, WatchEvent::RebuildStarted { .. }))
            .count()
    }

    fn as_callback(&self) -> Arc<dyn Fn(WatchEvent) + Send + Sync> {
        let this = self.clone();
        Arc::new(move |e| this.push(e))
    }
}

fn start_watch(
    entry: &Path,
    out_dir: &Path,
    port: u16,
    sink: &EventSink,
) -> Result<
    (
        std::thread::JoinHandle<Result<(), ipe::CliError>>,
        WatchHandle,
    ),
    BoxError,
> {
    let runtime_dir = ipe::resolve_runtime()
        .map_err(|e| -> BoxError { format!("runtime dir must resolve: {e}").into() })?;
    let mut opts = WatchOptions::new(entry.to_path_buf(), out_dir.to_path_buf(), runtime_dir);
    opts.port = port;
    // Tight debounce so the tests don't pay the default's full latency
    // budget while still comfortably coalescing a same-window double-save.
    opts.debounce = ipe_watch::DebounceConfig {
        quiescence: Duration::from_millis(120),
        hard_cap: Duration::from_millis(600),
    };
    opts.on_event = Some(sink.as_callback());
    Ok(ipe::watch::spawn(opts))
}

fn write_main(ipe_dir: &Path, source: &str) -> Result<(), BoxError> {
    std::fs::write(ipe_dir.join("Main.ipe"), source)
        .map_err(|e| -> BoxError { format!("write Main.ipe: {e}").into() })
}

/// Stop the watch session and propagate a thread panic (never swallowed) or
/// a setup-level `CliError` as a plain test failure.
fn stop_and_join(
    handle: &WatchHandle,
    join: std::thread::JoinHandle<Result<(), ipe::CliError>>,
) -> Result<(), BoxError> {
    handle.stop();
    join.join().map_or_else(
        |_| Err("watch thread panicked".into()),
        |result| result.map_err(|e| -> BoxError { e.to_string().into() }),
    )
}

/// Find a live process whose `/proc/<pid>/environ` contains the exact
/// `key=value` pair `ipe watch` injects into its supervised child's
/// environment (`watch::child_env` sets `IPE_WEB_PORT`/`IPE_SERVER_PORT`
/// to the configured port — see `watch.rs`). Matching on the environment
/// rather than `cmdline`/the executable PATH is deliberate: the emitted
/// binary's actual on-disk location depends on where `cargo build` puts it
/// (honouring `CARGO_TARGET_DIR` if the test-runner's own environment sets
/// one — exactly the isolation convention this workspace's agent lanes
/// use), so asserting anything about that path here would make the test
/// depend on incidental build-cache configuration rather than the one thing
/// this test actually needs: an unambiguous handle on the correct PID.
/// `/proc/<pid>/environ` entries are NUL-separated, and NUL (0x00) is valid
/// single-byte UTF-8, so `String::from_utf8_lossy` preserves it verbatim —
/// matching `"KEY=VALUE\0"` (trailing NUL included) rules out a value that
/// merely starts with the same digits as another test's port. Linux-only:
/// the whole bug-3 regression below needs `/proc` for a black-box
/// PID-liveness check without adding any new production API surface just
/// for a test.
#[cfg(target_os = "linux")]
fn find_pid_by_environ_kv(key: &str, value: &str) -> Option<u32> {
    let needle = format!("{key}={value}\0");
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(environ) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        if String::from_utf8_lossy(&environ).contains(&needle) {
            return Some(pid);
        }
    }
    None
}

/// Whether `pid` still names a live process. `/proc/<pid>` disappears the
/// moment the process is BOTH dead AND reaped (a zombie still has an entry
/// until its parent `wait()`s it) — which is exactly the property the
/// bug-3 regression needs: `SupervisorState::shutdown`'s `stop_gracefully`
/// calls `child.wait()` after killing it, so a lingering `/proc/<pid>` here
/// would mean the child was signalled but never actually reaped, not merely
/// "not yet observed dead".
#[cfg(target_os = "linux")]
fn pid_is_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[test]
fn watch_rebuild_on_save_swaps_the_running_binary() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("skipping (set IPE_E2E=1 to run)");
        return Ok(());
    }
    let (ipe_dir, out_dir) = fresh_dirs("rebuild_swap")?;
    write_main(&ipe_dir, &server_fixture("v1"))?;

    let sink = EventSink::default();
    let port = 19151;
    let (join, handle) = start_watch(&ipe_dir.join("Main.ipe"), &out_dir, port, &sink)?;

    assert!(
        wait_for_body(port, "v1", Duration::from_mins(4)),
        "initial cold build+spawn must serve v1 within budget"
    );

    write_main(&ipe_dir, &server_fixture("v2"))?;
    assert!(
        wait_for_body(port, "v2", Duration::from_mins(2)),
        "warm rebuild must swap the running binary to serve v2"
    );

    stop_and_join(&handle, join)
}

#[test]
fn watch_keeps_last_good_binary_alive_on_a_syntax_error() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("skipping (set IPE_E2E=1 to run)");
        return Ok(());
    }
    let (ipe_dir, out_dir) = fresh_dirs("last_good")?;
    write_main(&ipe_dir, &server_fixture("v1"))?;

    let sink = EventSink::default();
    let port = 19152;
    let (join, handle) = start_watch(&ipe_dir.join("Main.ipe"), &out_dir, port, &sink)?;

    assert!(
        wait_for_body(port, "v1", Duration::from_mins(4)),
        "initial cold build+spawn must serve v1"
    );

    // INV-3: a red build (here, a parse failure) must never touch the
    // running process. Introduce the deliberate syntax error, then assert
    // the server is STILL serving v1 after a window comfortably longer
    // than a real rebuild would take.
    write_main(&ipe_dir, BROKEN_SOURCE)?;
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        http_get_body(port).is_some_and(|b| b.contains("v1")),
        "last-good binary must still be serving v1 after a red build"
    );

    // Recovery: fixing the source must produce a fresh green build and
    // restart onto it. The recovery rebuild is a full isolated `cargo build`
    // (no shared target, exactly like the initial cold build), so it gets the
    // same generous cold-build budget — a tighter deadline here fails on a
    // loaded runner's slow rebuild, not on a real regression.
    write_main(&ipe_dir, &server_fixture("v2"))?;
    assert!(
        wait_for_body(port, "v2", Duration::from_mins(4)),
        "watch must recover once the syntax error is fixed"
    );

    stop_and_join(&handle, join)
}

#[test]
fn watch_coalesces_a_rapid_double_save_into_one_rebuild() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("skipping (set IPE_E2E=1 to run)");
        return Ok(());
    }
    let (ipe_dir, out_dir) = fresh_dirs("coalesce")?;
    write_main(&ipe_dir, &server_fixture("v1"))?;

    let sink = EventSink::default();
    let port = 19153;
    let (join, handle) = start_watch(&ipe_dir.join("Main.ipe"), &out_dir, port, &sink)?;

    assert!(
        wait_for_body(port, "v1", Duration::from_mins(4)),
        "initial cold build+spawn must serve v1"
    );

    let rebuilds_before = sink.count_rebuild_started();

    // Two writes ~20ms apart — well inside the 120ms quiescence window
    // configured in `start_watch` — must coalesce into exactly ONE
    // rebuild cycle, and the LAST write (v3) must be what ships.
    write_main(&ipe_dir, &server_fixture("v2"))?;
    std::thread::sleep(Duration::from_millis(20));
    write_main(&ipe_dir, &server_fixture("v3"))?;

    assert!(
        wait_for_body(port, "v3", Duration::from_mins(2)),
        "the LAST write in the coalesced burst must be what ships"
    );

    let rebuilds_after = sink.count_rebuild_started();
    assert_eq!(
        rebuilds_after - rebuilds_before,
        1,
        "a rapid double-save inside the quiescence window must coalesce into exactly one rebuild"
    );

    stop_and_join(&handle, join)
}

/// Bug-3 regression: an embedder that lets a [`WatchHandle`] fall out of
/// scope WITHOUT ever calling `stop()` — the exact shape of a caller bug, or
/// a panic unwinding through a scope that holds one — must not leak the
/// supervised child process as an orphan. `Drop for WatchHandle` is the
/// safety net; this proves it actually reaps the child, not merely that it
/// compiles.
///
/// Linux-only (`/proc`-based PID liveness — see `find_pid_by_environ_kv`/
/// `pid_is_alive`): no new production API surface was added just to make
/// this observable from a black-box test.
#[cfg(target_os = "linux")]
#[test]
fn dropping_a_watch_handle_without_stop_still_reaps_the_supervised_child() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("skipping (set IPE_E2E=1 to run)");
        return Ok(());
    }
    let (ipe_dir, out_dir) = fresh_dirs("drop_reaps_child")?;
    write_main(&ipe_dir, &server_fixture("v1"))?;

    let sink = EventSink::default();
    let port = 19155;
    let (join, handle) = start_watch(&ipe_dir.join("Main.ipe"), &out_dir, port, &sink)?;

    assert!(
        wait_for_body(port, "v1", Duration::from_mins(4)),
        "initial cold build+spawn must serve v1"
    );

    // `watch::child_env` sets `IPE_WEB_PORT=<port>` in the supervised
    // child's own environment, unique to this test's port — a stronger
    // handle on the right PID than the executable's on-disk path (which
    // moves if the test-runner's own environment sets `CARGO_TARGET_DIR`).
    let child_pid = find_pid_by_environ_kv("IPE_WEB_PORT", &port.to_string())
        .expect("the supervised child process must be discoverable via /proc once v1 is serving");
    assert!(
        pid_is_alive(child_pid),
        "sanity: the child must be alive right after v1 is confirmed serving"
    );

    // Simulate the abnormal-exit shape the bug report describes: drop the
    // `WatchHandle` directly, never calling `stop()`. `Drop::drop` is the
    // ONLY thing standing between this and an orphaned `ipe-app` server
    // holding a real port open forever.
    drop(handle);

    // `Drop`'s synchronous wait-for-shutdown (bounded by
    // `SHUTDOWN_WAIT_BUDGET` inside `watch.rs`) means the child is
    // GUARANTEED fully reaped by the time `drop(handle)` above returns — no
    // polling loop needed here, unlike a fire-and-forget shutdown request
    // would require.
    assert!(
        !pid_is_alive(child_pid),
        "WatchHandle::drop must reap the supervised child even when stop() was never called \
         (it must have been both killed AND wait()ed — a lingering zombie also fails this)"
    );

    // The orchestrator thread has also fully exited by now (`Drop` waited
    // for its own done-signal, which only fires after `run_inner` returns)
    // — this join is a formality, not a wait.
    join.join().map_or_else(
        |_| Err("watch thread panicked".into()),
        |result| result.map_err(|e| -> BoxError { e.to_string().into() }),
    )
}

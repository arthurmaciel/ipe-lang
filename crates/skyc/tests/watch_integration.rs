#![forbid(unsafe_code)]
//! End-to-end proofs for `ipe watch` (`crate::watch`, Phase 7 — Tasks
//! 21-24; Task 25's cancellation proof lives separately in
//! `watch_cancellation.rs`, deterministically, since racing a real
//! file-save against warm salsa recompute — which is DELIBERATELY fast —
//! is not a reliable timing window for an E2E test).
//!
//! Gated on `SKY_E2E=1` exactly like `server_e2e.rs`: every scenario here
//! drives a REAL `cargo build` of the emitted project and spawns the
//! resulting binary, so these are slow (first build pays the full
//! dependency-compile cost) but honest — no mocked compiler, no mocked
//! process supervisor.
//!
//! ```text
//! SKY_E2E=1 cargo nextest run -p skyc --test watch_integration
//! ```

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use skyc::watch::{WatchEvent, WatchHandle, WatchOptions};

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A minimal `Sky.Http.Server` fixture, parameterised on the response body
/// so a test can edit it in place and observe the swap. Reads its port from
/// `SKY_SERVER_PORT` — the SAME convention `server_e2e.rs` already
/// establishes in this repo, and what `watch::child_env` drives from
/// `WatchOptions::port`.
fn server_fixture(body: &str) -> String {
    format!(
        "module Main exposing (main)\n\n\
         import Sky.Http.Server as Server\n\n\
         main =\n    \
             let port = Maybe.withDefault 8080 (String.toInt (System.getenvOr \"SKY_SERVER_PORT\" \"8080\"))\n    \
             in\n    \
             Server.listen port\n        \
                 [ Server.get \"/\" (\\req -> Task.succeed (Server.text \"{body}\")) ]\n"
    )
}

/// A DELIBERATELY unparseable `.sky` file — a dangling `let` with no `in`,
/// which fails at parse time (never reaches type-check, let alone emit).
const BROKEN_SOURCE: &str = "module Main exposing (main)\n\nmain =\n    let x = 1\n";

fn fresh_dirs(tag: &str) -> Result<(PathBuf, PathBuf), BoxError> {
    let base = std::env::temp_dir().join(format!(
        "watch_e2e_{tag}_{}_{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    let sky_dir = base.join("sky");
    let out_dir = base.join("out");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&sky_dir)
        .map_err(|e| -> BoxError { format!("mkdir {}: {e}", sky_dir.display()).into() })?;
    Ok((sky_dir, out_dir))
}

/// Poll `GET / HTTP/1.1` on `127.0.0.1:port` for up to `timeout`, returning
/// `true` the moment the body matches `want`, or `false` on timeout.
/// Mirrors `server_e2e.rs`'s own raw-socket polling — no extra HTTP
/// dependency.
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
        std::thread::JoinHandle<Result<(), skyc::CliError>>,
        WatchHandle,
    ),
    BoxError,
> {
    let runtime_dir = skyc::resolve_runtime()
        .map_err(|e| -> BoxError { format!("runtime dir must resolve: {e}").into() })?;
    let mut opts = WatchOptions::new(entry.to_path_buf(), out_dir.to_path_buf(), runtime_dir);
    opts.port = port;
    // Tight debounce so the tests don't pay the default's full latency
    // budget while still comfortably coalescing a same-window double-save.
    opts.debounce = sky_watch::DebounceConfig {
        quiescence: Duration::from_millis(120),
        hard_cap: Duration::from_millis(600),
    };
    opts.on_event = Some(sink.as_callback());
    Ok(skyc::watch::spawn(opts))
}

fn write_main(sky_dir: &Path, source: &str) -> Result<(), BoxError> {
    std::fs::write(sky_dir.join("Main.sky"), source)
        .map_err(|e| -> BoxError { format!("write Main.sky: {e}").into() })
}

/// Stop the watch session and propagate a thread panic (never swallowed) or
/// a setup-level `CliError` as a plain test failure.
fn stop_and_join(
    handle: &WatchHandle,
    join: std::thread::JoinHandle<Result<(), skyc::CliError>>,
) -> Result<(), BoxError> {
    handle.stop();
    join.join().map_or_else(
        |_| Err("watch thread panicked".into()),
        |result| result.map_err(|e| -> BoxError { e.to_string().into() }),
    )
}

#[test]
fn watch_rebuild_on_save_swaps_the_running_binary() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        eprintln!("skipping (set SKY_E2E=1 to run)");
        return Ok(());
    }
    let (sky_dir, out_dir) = fresh_dirs("rebuild_swap")?;
    write_main(&sky_dir, &server_fixture("v1"))?;

    let sink = EventSink::default();
    let port = 19151;
    let (join, handle) = start_watch(&sky_dir.join("Main.sky"), &out_dir, port, &sink)?;

    assert!(
        wait_for_body(port, "v1", Duration::from_secs(120)),
        "initial cold build+spawn must serve v1 within budget"
    );

    write_main(&sky_dir, &server_fixture("v2"))?;
    assert!(
        wait_for_body(port, "v2", Duration::from_secs(60)),
        "warm rebuild must swap the running binary to serve v2"
    );

    stop_and_join(&handle, join)
}

#[test]
fn watch_keeps_last_good_binary_alive_on_a_syntax_error() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        eprintln!("skipping (set SKY_E2E=1 to run)");
        return Ok(());
    }
    let (sky_dir, out_dir) = fresh_dirs("last_good")?;
    write_main(&sky_dir, &server_fixture("v1"))?;

    let sink = EventSink::default();
    let port = 19152;
    let (join, handle) = start_watch(&sky_dir.join("Main.sky"), &out_dir, port, &sink)?;

    assert!(
        wait_for_body(port, "v1", Duration::from_secs(120)),
        "initial cold build+spawn must serve v1"
    );

    // INV-3: a red build (here, a parse failure) must never touch the
    // running process. Introduce the deliberate syntax error, then assert
    // the server is STILL serving v1 after a window comfortably longer
    // than a real rebuild would take.
    write_main(&sky_dir, BROKEN_SOURCE)?;
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        http_get_body(port).is_some_and(|b| b.contains("v1")),
        "last-good binary must still be serving v1 after a red build"
    );

    // Recovery: fixing the source must produce a fresh green build and
    // restart onto it.
    write_main(&sky_dir, &server_fixture("v2"))?;
    assert!(
        wait_for_body(port, "v2", Duration::from_secs(60)),
        "watch must recover once the syntax error is fixed"
    );

    stop_and_join(&handle, join)
}

#[test]
fn watch_coalesces_a_rapid_double_save_into_one_rebuild() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        eprintln!("skipping (set SKY_E2E=1 to run)");
        return Ok(());
    }
    let (sky_dir, out_dir) = fresh_dirs("coalesce")?;
    write_main(&sky_dir, &server_fixture("v1"))?;

    let sink = EventSink::default();
    let port = 19153;
    let (join, handle) = start_watch(&sky_dir.join("Main.sky"), &out_dir, port, &sink)?;

    assert!(
        wait_for_body(port, "v1", Duration::from_secs(120)),
        "initial cold build+spawn must serve v1"
    );

    let rebuilds_before = sink.count_rebuild_started();

    // Two writes ~20ms apart — well inside the 120ms quiescence window
    // configured in `start_watch` — must coalesce into exactly ONE
    // rebuild cycle, and the LAST write (v3) must be what ships.
    write_main(&sky_dir, &server_fixture("v2"))?;
    std::thread::sleep(Duration::from_millis(20));
    write_main(&sky_dir, &server_fixture("v3"))?;

    assert!(
        wait_for_body(port, "v3", Duration::from_secs(60)),
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

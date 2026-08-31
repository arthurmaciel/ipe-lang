#![forbid(unsafe_code)]
//! End-to-end proof for the DEV-ONLY blue-green front proxy in `ipe watch`
//! (`crate::watch` with `WatchOptions::bluegreen`).
//!
//! The load-bearing property: a rebuild behind the proxy does NOT drop the
//! browser's connection. The test holds ONE keep-alive TCP socket open to the
//! user-facing proxy port across a rebuild and asserts the SAME socket still
//! serves — now the NEW binary — after the cutover. Without the proxy, a
//! rebuild kills the old binary to free the port, so that socket would be
//! dropped (connection reset) and the assertion would fail.
//!
//! Gated on `IPE_E2E=1` exactly like `watch_integration.rs`: it drives a REAL
//! `cargo build` of the emitted web project and spawns the resulting binary.
//!
//! ```text
//! IPE_E2E=1 cargo nextest run -p ipe --test watch_bluegreen
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ipe::watch::{WatchHandle, WatchOptions};

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A minimal `Ipe.Web` (TEA) app whose rendered page carries `marker` in its
/// heading, so a `GET /` can observe which binary is live. Uses `Web.app`, so
/// the emitted entry contains `ipe_runtime::web::web_app` — the marker
/// `watch::is_ipe_web_project` keys the `/_ipe/readyz` readiness probe on.
fn web_fixture(marker: &str) -> String {
    format!(
        "module Main exposing (main)\n\n\
         import Ipe.Tea.Web as Web\n\
         import Ipe.Ui as Ui\n\
         import Ipe.Tea.Web.Cmd as Cmd\n\
         import Ipe.Tea.Web.Sub as Sub\n\n\
         type Msg = Noop\n\n\
         type alias Model = {{ count : Int }}\n\n\
         init : a -> ( Model, Cmd Msg )\n\
         init _req = ( {{ count = 0 }}, Cmd.none )\n\n\
         update : Msg -> Model -> ( Model, Cmd Msg )\n\
         update _msg model = ( model, Cmd.none )\n\n\
         subscriptions : Model -> Sub Msg\n\
         subscriptions _model = Sub.none\n\n\
         view : Model -> Element Msg\n\
         view _model =\n    \
             Ui.column [ Ui.spacing 8 ] [ Ui.el [] (Ui.text \"{marker}\") ]\n\n\
         main =\n    \
             Web.app\n        \
                 {{ init = init\n        \
                 , update = update\n        \
                 , view = view\n        \
                 , subscriptions = subscriptions\n        \
                 , routes = []\n        \
                 , notFound = Noop\n        \
                 }}\n"
    )
}

fn fresh_dirs(tag: &str) -> Result<(PathBuf, PathBuf), BoxError> {
    let base = std::env::temp_dir().join(format!(
        "watch_bg_{tag}_{}_{}",
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

fn write_main(ipe_dir: &Path, source: &str) -> Result<(), BoxError> {
    std::fs::write(ipe_dir.join("Main.ipe"), source)
        .map_err(|e| -> BoxError { format!("write Main.ipe: {e}").into() })
}

fn start_watch(
    entry: &Path,
    out_dir: &Path,
    port: u16,
    bluegreen: bool,
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
    opts.bluegreen = bluegreen;
    opts.debounce = ipe_watch::DebounceConfig {
        quiescence: Duration::from_millis(120),
        hard_cap: Duration::from_millis(600),
    };
    Ok(ipe::watch::spawn(opts))
}

/// One `GET / HTTP/1.1` on an ALREADY-OPEN keep-alive socket, returning the
/// response body. Reads the status line + headers, then the body per its
/// `Content-Length` (or, absent one, until the read times out). Crucially it
/// does NOT open a new connection — the whole point is to prove the SAME
/// socket survives a rebuild.
fn get_body_keepalive(stream: &mut TcpStream) -> Option<String> {
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: keep-alive\r\n\r\n")
        .ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut content_length: Option<usize> = None;
    let mut status_ok = false;
    let mut first = true;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).ok()?;
        if n == 0 {
            return None; // socket closed — the failure this test guards against
        }
        if first {
            status_ok = line.starts_with("HTTP/1.1 2") || line.starts_with("HTTP/1.0 2");
            first = false;
        } else if let Some(v) = line
            .to_ascii_lowercase()
            .strip_prefix("content-length:")
        {
            content_length = v.trim().parse().ok();
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
    }
    if !status_ok {
        return Some(String::new());
    }
    if let Some(len) = content_length {
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).ok()?;
        Some(String::from_utf8_lossy(&body).into_owned())
    } else {
        // No Content-Length: read what's buffered within a short window.
        let mut body = Vec::new();
        let _ = reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(500)));
        let _ = reader.read_to_end(&mut body);
        Some(String::from_utf8_lossy(&body).into_owned())
    }
}

/// Poll a FRESH connection to `port` until `GET /` serves a body containing
/// `want`, or `timeout` elapses. Used to wait for a cold build / a cutover to
/// land before exercising the kept-alive socket.
fn wait_for_marker(port: u16, want: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let Ok(addr) = format!("127.0.0.1:{port}").parse() else {
        return false;
    };
    while Instant::now() < deadline {
        if let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(200))
            && get_body_keepalive(&mut s).is_some_and(|b| b.contains(want))
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

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

/// One rebuild's client-visible outage tail: sampling `GET /` on `port` every
/// 5ms, the wall-clock from the last successful serve of the OLD marker to the
/// first successful serve of the NEW marker, together with how many of those
/// samples FAILED to connect/serve (the socket-drop + reconnect cost the proxy
/// removes). Returns `(tail, failed_samples)`.
///
/// A FRESH connection per sample deliberately measures the browser-reconnect
/// experience: the direct path drops the socket on restart, so a browser must
/// reconnect — and during the respawn window every reconnect attempt is
/// refused. The proxy holds the port throughout, so reconnects never fail and
/// the tail collapses to just the cutover.
fn measure_rebuild_tail(
    port: u16,
    old_marker: &str,
    new_marker: &str,
    budget: Duration,
) -> (Duration, usize) {
    let start = Instant::now();
    let mut last_old_seen = start;
    let mut failed = 0usize;
    let Ok(addr) = format!("127.0.0.1:{port}").parse::<std::net::SocketAddr>() else {
        return (Duration::ZERO, 0);
    };
    loop {
        if start.elapsed() > budget {
            return (start.elapsed(), failed);
        }
        let served = TcpStream::connect_timeout(&addr, Duration::from_millis(150))
            .ok()
            .and_then(|mut s| {
                let _ = s.set_read_timeout(Some(Duration::from_millis(300)));
                get_body_keepalive(&mut s)
            });
        match served {
            Some(body) if body.contains(new_marker) => {
                // First serve of the new binary — the tail is measured from the
                // last successful old serve to here.
                return (last_old_seen.elapsed(), failed);
            }
            Some(body) if body.contains(old_marker) => {
                last_old_seen = Instant::now();
            }
            // A failed connect/serve OR an empty/unknown body during the window
            // is a reconnect the client would have had to make.
            _ => failed += 1,
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The MEASUREMENT harness (not a pass/fail gate — it prints a table). For a
/// given mode it does 3 rebuilds and reports the per-rebuild client-visible
/// outage tail and the number of failed reconnect samples. Run explicitly:
///
/// ```text
/// IPE_E2E=1 cargo test -p ipe --test watch_bluegreen -- --ignored --nocapture measure
/// ```
fn run_measurement(bluegreen: bool, port: u16, tag: &str) -> Result<(), BoxError> {
    let (ipe_dir, out_dir) = fresh_dirs(tag)?;
    write_main(&ipe_dir, &web_fixture("M-0"))?;
    let (join, handle) = start_watch(&ipe_dir.join("Main.ipe"), &out_dir, port, bluegreen)?;
    assert!(
        wait_for_marker(port, "M-0", Duration::from_mins(5)),
        "cold build must serve M-0"
    );

    let mode = if bluegreen { "proxy cutover" } else { "direct (drop+reconnect)" };
    let mut tails = Vec::new();
    for i in 1..=3u32 {
        let old = format!("M-{}", i - 1);
        let new = format!("M-{i}");
        // Kick a background prober that measures the tail across the edit.
        let old_c = old.clone();
        let new_c = new.clone();
        let prober =
            std::thread::spawn(move || measure_rebuild_tail(port, &old_c, &new_c, Duration::from_mins(2)));
        // Give the prober a beat to establish the baseline, then edit.
        std::thread::sleep(Duration::from_millis(50));
        write_main(&ipe_dir, &web_fixture(&new))?;
        let (tail, failed) = prober.join().map_err(|_| -> BoxError { "prober panicked".into() })?;
        eprintln!(
            "[measure] {mode:<26} rebuild {i}: tail = {:>7.1} ms, failed reconnect samples = {failed}",
            tail.as_secs_f64() * 1000.0
        );
        tails.push(tail);
    }
    let count = u32::try_from(tails.len()).unwrap_or(1).max(1);
    let avg = tails.iter().sum::<Duration>() / count;
    eprintln!(
        "[measure] {mode:<26} AVERAGE tail over 3 rebuilds = {:.1} ms",
        avg.as_secs_f64() * 1000.0
    );

    stop_and_join(&handle, join)
}

#[test]
#[ignore = "measurement harness (prints a table); run explicitly with --ignored --nocapture"]
fn measure_direct_path_tail() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("skipping (set IPE_E2E=1 to run)");
        return Ok(());
    }
    run_measurement(false, 19181, "measure_direct")
}

#[test]
#[ignore = "measurement harness (prints a table); run explicitly with --ignored --nocapture"]
fn measure_bluegreen_path_tail() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("skipping (set IPE_E2E=1 to run)");
        return Ok(());
    }
    run_measurement(true, 19182, "measure_bluegreen")
}

/// The core proof: a rebuild behind the blue-green proxy does not drop the
/// browser's connection, and the same socket afterwards serves the NEW binary.
#[test]
fn bluegreen_rebuild_keeps_the_client_connection_alive() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("skipping (set IPE_E2E=1 to run)");
        return Ok(());
    }
    let (ipe_dir, out_dir) = fresh_dirs("keepalive")?;
    write_main(&ipe_dir, &web_fixture("MARKER-V1"))?;

    let port = 19171;
    let (join, handle) = start_watch(&ipe_dir.join("Main.ipe"), &out_dir, port, true)?;

    assert!(
        wait_for_marker(port, "MARKER-V1", Duration::from_mins(5)),
        "cold build must serve v1 through the blue-green proxy"
    );

    // Open ONE keep-alive socket to the proxy port and confirm it serves v1.
    let mut client = TcpStream::connect(format!("127.0.0.1:{port}"))
        .map_err(|e| -> BoxError { format!("connect to proxy: {e}").into() })?;
    client.set_read_timeout(Some(Duration::from_secs(5)))?;
    let first = get_body_keepalive(&mut client);
    assert!(
        first.as_deref().is_some_and(|b| b.contains("MARKER-V1")),
        "the kept-alive socket must serve v1 first: {first:?}"
    );

    // Trigger a rebuild to a new binary and wait (on FRESH probes) for the
    // cutover to land.
    write_main(&ipe_dir, &web_fixture("MARKER-V2"))?;
    assert!(
        wait_for_marker(port, "MARKER-V2", Duration::from_mins(3)),
        "the rebuild must cut over to v2 behind the proxy"
    );

    // THE ASSERTION: the SAME socket, never reconnected, still works and now
    // serves the new binary. A dropped connection (the pre-proxy behaviour)
    // would make this `None` (socket closed) or an error.
    let second = get_body_keepalive(&mut client);
    assert!(
        second.is_some(),
        "the client socket must SURVIVE the rebuild (not be dropped): got None (closed)"
    );
    assert!(
        second.as_deref().is_some_and(|b| b.contains("MARKER-V2")),
        "the surviving socket must serve the NEW binary after cutover: {second:?}"
    );

    stop_and_join(&handle, join)
}

/// The flag-OFF control: with `bluegreen` disabled, `ipe watch` keeps its
/// direct-bind behaviour — a rebuild still swaps the served binary (proving
/// the default path is unchanged). It does NOT assert socket survival (the
/// direct path drops connections on restart by design).
#[test]
fn flag_off_direct_path_still_swaps_the_binary() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("skipping (set IPE_E2E=1 to run)");
        return Ok(());
    }
    let (ipe_dir, out_dir) = fresh_dirs("flagoff")?;
    write_main(&ipe_dir, &web_fixture("OFF-V1"))?;

    let port = 19172;
    let (join, handle) = start_watch(&ipe_dir.join("Main.ipe"), &out_dir, port, false)?;

    assert!(
        wait_for_marker(port, "OFF-V1", Duration::from_mins(5)),
        "cold build must serve v1 on the direct path"
    );

    write_main(&ipe_dir, &web_fixture("OFF-V2"))?;
    assert!(
        wait_for_marker(port, "OFF-V2", Duration::from_mins(3)),
        "the direct (flag-off) path must still swap the running binary to v2"
    );

    stop_and_join(&handle, join)
}

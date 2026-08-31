//! End-to-end proof for `ipe watch`'s appearance hot-swap classifier.
//!
//! Under `IPE_WATCH_HOT_APPEARANCE`, an edit to a hoisted style-value literal in
//! a web `view` is classified `AppearanceOnly` and pushed to the running app as a
//! `LiteralTable` patch — no `cargo build`, no process restart. A structural edit
//! (add an element) is classified `Logic` and recompiles. This test asserts both
//! outcomes at the watch-loop + process level:
//!
//! - a style-value edit fires [`WatchEvent::AppearanceHotSwapped`], produces NO
//!   new `RebuildStarted`, and leaves the SAME server PID serving (no restart);
//! - a structural edit fires `RebuildStarted` and swaps the server PID.
//!
//! The browser-visible half — the endpoint applying the patch and pushing the
//! resulting VDOM diff over SSE — is proven by the Step-2 runtime/client test
//! (`ipe_runtime::web`'s `apply_literal_patch_to_web_sessions`); here we prove the
//! watch loop reaches that endpoint for a style edit and recompiles for a logic
//! edit, which is the classifier's contract.
//!
//! Gated on `IPE_E2E=1` like the sibling `watch_integration.rs`: it drives a real
//! `cargo build` and spawns the emitted binary. Linux-only for the PID-liveness
//! check via `/proc`, exactly as `watch_integration.rs`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ipe::watch::{WatchEvent, WatchHandle, WatchOptions};

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A minimal `Web.app` whose view carries a hoistable `Ui.padding` style value
/// and a marker text so an HTTP read can confirm the app is up. `padding` is the
/// dominant appearance edit and hoists to a `LiteralTable` default under the flag.
fn web_fixture(padding: u32, extra_text: &str) -> String {
    format!(
        "module Main exposing (main)\n\n\
         import Ipe.Tea.Web as Web\n\
         import Ipe.Ui as Ui\n\
         import Ipe.Tea.Web.Cmd\n\
         import Ipe.Tea.Web.Sub\n\
         import Ipe.String\n\n\
         type alias Model = {{ count : Int }}\n\n\
         type Msg = Noop\n\n\
         init : a -> ( Model, Cmd Msg )\n\
         init _req =\n    \
             ( {{ count = 0 }}, Cmd.none )\n\n\
         update : Msg -> Model -> ( Model, Cmd Msg )\n\
         update _msg model =\n    \
             ( model, Cmd.none )\n\n\
         view : Model -> Element Msg\n\
         view _model =\n    \
             Ui.column [ Ui.padding {padding} ]\n        \
                 [ Ui.text \"marker\"{extra_text} ]\n\n\
         subscriptions : Model -> Sub Msg\n\
         subscriptions _model =\n    \
             Sub.none\n\n\
         main =\n    \
             Web.app\n        \
                 {{ init = init, update = update, view = view, subscriptions = subscriptions\n        \
                 , routes = [], notFound = Noop\n        \
                 }}\n",
    )
}

/// A `Web.app` whose view carries a hoistable **attribute value** (`Ui.name`)
/// and a **static text** node whose content is `text` — both widened
/// appearance-literal kinds (Step 5). A marker text confirms the app is up.
fn web_fixture_attr_text(name: &str, text: &str) -> String {
    format!(
        "module Main exposing (main)\n\n\
         import Ipe.Tea.Web as Web\n\
         import Ipe.Ui as Ui\n\
         import Ipe.Tea.Web.Cmd\n\
         import Ipe.Tea.Web.Sub\n\
         import Ipe.String\n\n\
         type alias Model = {{ count : Int }}\n\n\
         type Msg = Noop\n\n\
         init : a -> ( Model, Cmd Msg )\n\
         init _req =\n    \
             ( {{ count = 0 }}, Cmd.none )\n\n\
         update : Msg -> Model -> ( Model, Cmd Msg )\n\
         update _msg model =\n    \
             ( model, Cmd.none )\n\n\
         view : Model -> Element Msg\n\
         view _model =\n    \
             Ui.column [ Ui.name \"{name}\" ]\n        \
                 [ Ui.text \"marker\", Ui.text \"{text}\" ]\n\n\
         subscriptions : Model -> Sub Msg\n\
         subscriptions _model =\n    \
             Sub.none\n\n\
         main =\n    \
             Web.app\n        \
                 {{ init = init, update = update, view = view, subscriptions = subscriptions\n        \
                 , routes = [], notFound = Noop\n        \
                 }}\n",
    )
}

fn fresh_dirs(tag: &str) -> Result<(PathBuf, PathBuf), BoxError> {
    let base = std::env::temp_dir().join(format!(
        "watch_hot_{tag}_{}_{}",
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
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn wait_for_serving(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if http_get_body(port).is_some_and(|b| b.contains("marker")) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Poll `cond` until it holds or `timeout` elapses; `true` if it held.
fn wait_for(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[derive(Clone, Default)]
struct EventSink(Arc<Mutex<Vec<WatchEvent>>>);

impl EventSink {
    fn push(&self, event: WatchEvent) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }

    fn count_hot_swapped(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|e| matches!(e, WatchEvent::AppearanceHotSwapped { .. }))
            .count()
    }

    fn count_restarted(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|e| matches!(e, WatchEvent::Restarted { .. }))
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
    opts.debounce = ipe_watch::DebounceConfig {
        quiescence: Duration::from_millis(120),
        hard_cap: Duration::from_millis(600),
    };
    opts.on_event = Some(sink.as_callback());
    Ok(ipe::watch::spawn(opts))
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

/// The live server PID, matched by the exact `IPE_WEB_PORT=<port>` env pair the
/// supervised child carries. Same `/proc`-environ technique as
/// `watch_integration.rs`, so it does not depend on the emitted binary's path.
#[cfg(target_os = "linux")]
fn server_pid(port: u16) -> Option<u32> {
    let needle = format!("IPE_WEB_PORT={port}\0");
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

#[test]
#[cfg(target_os = "linux")]
fn style_edit_hot_swaps_without_rebuild_and_structural_edit_recompiles() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("skipping (set IPE_E2E=1 to run)");
        return Ok(());
    }
    // Enable the flag for THIS watch process (and, by inheritance, the spawned
    // child, whose `/_ipe/hot-appearance` route needs it too). nextest isolates
    // each test in its own process, so this does not leak to other tests. The set
    // happens before any watch thread is spawned, so no other thread races this
    // var — the only precondition `set_var` needs.
    // SAFETY: single-threaded at this point (no watch thread spawned yet), and no
    // other code in this isolated test process reads or writes this var.
    unsafe {
        std::env::set_var("IPE_WATCH_HOT_APPEARANCE", "1");
    }

    let (ipe_dir, out_dir) = fresh_dirs("swap")?;
    write_main(&ipe_dir, &web_fixture(12, ""))?;

    let sink = EventSink::default();
    let port = 19171;
    let (join, handle) = start_watch(&ipe_dir.join("Main.ipe"), &out_dir, port, &sink)?;

    assert!(
        wait_for_serving(port, Duration::from_mins(4)),
        "initial cold build must serve the web app"
    );
    // The app answers HTTP the instant its readiness probe passes, which is
    // slightly BEFORE the loop emits the cold build's `Restarted` event. Wait for
    // that event to settle so the restart count is stable before the edit — else
    // the baseline races the generation-1 restart and looks like a spurious +1.
    assert!(
        wait_for(Duration::from_secs(10), || sink.count_restarted() >= 1),
        "the cold build must record its initial Restarted event"
    );
    let pid_before = server_pid(port).ok_or("server PID must be discoverable after cold build")?;
    // A hot-swap must add NO further restart. (`RebuildStarted` fires at the START
    // of every edit cycle, hot-swap included, so it is NOT the no-cargo signal —
    // `Restarted` is.)
    let restarts_before = sink.count_restarted();

    // ── Style-value edit: padding 12 -> 16. Appearance-only ⇒ hot-swap. ──
    write_main(&ipe_dir, &web_fixture(16, ""))?;
    let swap_start = Instant::now();
    let hot_swapped = wait_for(Duration::from_secs(20), || sink.count_hot_swapped() > 0);
    let swap_elapsed = swap_start.elapsed();
    assert!(
        hot_swapped,
        "a padding-value edit must be hot-swapped (AppearanceHotSwapped), not recompiled"
    );
    // Give any (erroneous) rebuild a generous window to have started a cargo
    // build + restart, so "no restart" is a real observation, not a race win.
    std::thread::sleep(Duration::from_secs(2));
    // No restart happened — the running binary was never swapped (⇒ no cargo).
    assert_eq!(
        sink.count_restarted(),
        restarts_before,
        "a hot-swapped style edit must NOT restart the app (no cargo rebuild)"
    );
    // The server process is unchanged — the app was never restarted.
    let pid_after_style = server_pid(port).ok_or("server PID must still be discoverable")?;
    assert_eq!(
        pid_before, pid_after_style,
        "a hot-swapped style edit must leave the SAME server process running"
    );
    // The app still answers (proving the swap did not crash the running server).
    assert!(
        http_get_body(port).is_some_and(|b| b.contains("marker")),
        "the app must keep serving after a hot-swap"
    );
    eprintln!(
        "[measure] style padding 12->16 hot-swap round-trip (edit -> AppearanceHotSwapped): {} ms \
         (no cargo, no restart)",
        swap_elapsed.as_millis()
    );

    // ── Structural edit: add an element. Logic ⇒ recompile + restart. ──
    let restarts_before_struct = sink.count_restarted();
    write_main(&ipe_dir, &web_fixture(16, "\n        , Ui.text \"added\""))?;
    // A recompile swaps the running binary — wait for a NEW PID serving the added
    // element, proving both the recompile AND the restart onto a new process.
    let restarted = wait_for(Duration::from_mins(2), || {
        server_pid(port).is_some_and(|pid| pid != pid_before)
            && http_get_body(port).is_some_and(|b| b.contains("added"))
    });
    assert!(
        restarted,
        "a structural edit (added element) must recompile and restart onto a new binary"
    );
    // The `Restarted` event lands just after the new binary answers HTTP; wait for
    // it to settle before asserting the count grew (same readiness-vs-event race).
    assert!(
        wait_for(Duration::from_secs(10), || {
            sink.count_restarted() > restarts_before_struct
        }),
        "a structural edit must recompile and restart (a new Restarted event)"
    );

    stop_and_join(&handle, join)
}

/// The widened surface (Step 5): an **attribute-value** edit and a **text-value**
/// edit each hot-swap without a rebuild, exactly like the style-value path, while
/// a structural add still recompiles. Records both round-trips (Step 6 preview).
#[test]
#[cfg(target_os = "linux")]
fn attribute_and_text_edits_hot_swap_without_rebuild() -> Result<(), BoxError> {
    if std::env::var("IPE_E2E").is_err() {
        eprintln!("skipping (set IPE_E2E=1 to run)");
        return Ok(());
    }
    // SAFETY: single-threaded here (no watch thread spawned yet); nextest isolates
    // this process, so the var neither races nor leaks.
    unsafe {
        std::env::set_var("IPE_WATCH_HOT_APPEARANCE", "1");
    }

    let (ipe_dir, out_dir) = fresh_dirs("attrtext")?;
    write_main(&ipe_dir, &web_fixture_attr_text("card", "Hello"))?;

    let sink = EventSink::default();
    let port = 19172;
    let (join, handle) = start_watch(&ipe_dir.join("Main.ipe"), &out_dir, port, &sink)?;

    assert!(
        wait_for_serving(port, Duration::from_mins(4)),
        "initial cold build must serve the web app"
    );
    assert!(
        wait_for(Duration::from_secs(10), || sink.count_restarted() >= 1),
        "the cold build must record its initial Restarted event"
    );
    let pid_before = server_pid(port).ok_or("server PID must be discoverable after cold build")?;

    // ── Attribute-value edit: name "card" -> "panel". Appearance-only. ──
    let restarts_before_attr = sink.count_restarted();
    let hot_before_attr = sink.count_hot_swapped();
    write_main(&ipe_dir, &web_fixture_attr_text("panel", "Hello"))?;
    let attr_start = Instant::now();
    let attr_swapped = wait_for(Duration::from_secs(20), || {
        sink.count_hot_swapped() > hot_before_attr
    });
    let attr_elapsed = attr_start.elapsed();
    assert!(
        attr_swapped,
        "an attribute-value edit must be hot-swapped, not recompiled"
    );
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        sink.count_restarted(),
        restarts_before_attr,
        "a hot-swapped attribute edit must NOT restart the app"
    );
    assert_eq!(
        server_pid(port),
        Some(pid_before),
        "a hot-swapped attribute edit must leave the SAME server process running"
    );
    eprintln!(
        "[measure] attribute name card->panel hot-swap round-trip: {} ms (no cargo, no restart)",
        attr_elapsed.as_millis()
    );

    // ── Text-value edit: text "Hello" -> "Goodbye". Appearance-only. ──
    let restarts_before_text = sink.count_restarted();
    let hot_before_text = sink.count_hot_swapped();
    write_main(&ipe_dir, &web_fixture_attr_text("panel", "Goodbye"))?;
    let text_start = Instant::now();
    let text_swapped = wait_for(Duration::from_secs(20), || {
        sink.count_hot_swapped() > hot_before_text
    });
    let text_elapsed = text_start.elapsed();
    assert!(
        text_swapped,
        "a text-value edit must be hot-swapped, not recompiled"
    );
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        sink.count_restarted(),
        restarts_before_text,
        "a hot-swapped text edit must NOT restart the app"
    );
    assert_eq!(
        server_pid(port),
        Some(pid_before),
        "a hot-swapped text edit must leave the SAME server process running"
    );
    assert!(
        http_get_body(port).is_some_and(|b| b.contains("marker")),
        "the app must keep serving after the text hot-swap"
    );
    eprintln!(
        "[measure] text Hello->Goodbye hot-swap round-trip: {} ms (no cargo, no restart)",
        text_elapsed.as_millis()
    );

    // ── Structural edit: add a third text node. Logic ⇒ recompile + restart. ──
    let restarts_before_struct = sink.count_restarted();
    write_main(
        &ipe_dir,
        &web_fixture_attr_text("panel", "Goodbye").replace(
            "Ui.text \"Goodbye\" ]",
            "Ui.text \"Goodbye\", Ui.text \"added\" ]",
        ),
    )?;
    let restarted = wait_for(Duration::from_mins(2), || {
        server_pid(port).is_some_and(|pid| pid != pid_before)
            && http_get_body(port).is_some_and(|b| b.contains("added"))
    });
    assert!(
        restarted,
        "a structural edit (added text node) must recompile and restart onto a new binary"
    );
    assert!(
        wait_for(Duration::from_secs(10), || {
            sink.count_restarted() > restarts_before_struct
        }),
        "a structural edit must recompile and restart (a new Restarted event)"
    );

    stop_and_join(&handle, join)
}

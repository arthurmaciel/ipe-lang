//! Honest end-to-end tests for `Std.Live` / `Sky.Live` — `Live.app`, `Ui.layout`,
//! `Ui.column`, `Ui.el`, `Ui.onClick`, `Ui.text`, and `String.fromInt`.
//!
//! All tests are gated on `SKY_E2E=1`.  Without it they return early so the
//! default `cargo test` stays fast.
//!
//! ## Architecture
//!
//! 1. A minimal Sky.Live counter program is written to a temp dir.
//! 2. `skyc::build` compiles it (parse → canon → types → lower → emit Rust).
//! 3. `oracle::build_rust_binary` runs `cargo build` on the emitted project —
//!    the shared Cargo target (`~/.cargo/config.toml`) lets axum/tokio/serde
//!    compile once and be reused.
//! 4. An ephemeral TCP port is reserved via `TcpListener::bind("0")` → drop.
//! 5. The binary is spawned with `SKY_LIVE_PORT=<port>` and `SKY_CSRF=off`.
//!    `SKY_CSRF=off` disables the double-submit cookie check so test GETs
//!    exercise the full page render without cookie plumbing.
//! 6. Readiness: reads the child's stderr until `[sky.live] listening on`.
//! 7. `GET /` is sent via raw `TcpStream`; the response body must contain
//!    the initial counter value rendered as `>0<`, proving the full Live
//!    pipeline ran:
//!    `live_app → init → view → render_page → HTML served`.
//!
//! Run:
//!
//! ```text
//! SKY_E2E=1 cargo test live_e2e
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Shared error type for E2E helpers.
type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

// ── Sky program ───────────────────────────────────────────────────────────────

/// A minimal Sky.Live counter app.
///
/// Kernels exercised:
/// - `Live.app`     — Phase-1b B1/B2 fix: constrain scheme + serde derives
/// - `Ui.layout`    — converts Element tree to HTML
/// - `Ui.column`    — vertical layout (wired in M7 Phase 0)
/// - `Ui.el`        — generic element container with onClick attribute
/// - `Ui.onClick`   — binds a click event to a Msg
/// - `Ui.text`      — text leaf node
/// - `String.fromInt` — displays the counter value
/// - `Cmd.none` / `Sub.none` — baseline TEA primitives
///
/// The rendered initial page will contain the text `>0<` (the counter starts at
/// zero, rendered inside a text element).  No `Ui.button` is used because that
/// function is not a raw kernel — it is defined in sky-stdlib as a Sky function
/// and is therefore outside Phase-1b scope.
const SKY_LIVE_COUNTER: &str = r#"module Main exposing (main)

import Std.Live as Live
import Std.Ui as Ui

type Msg = Increment | Decrement

type alias Model = { count : Int }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )
        Decrement ->
            ( { model | count = model.count - 1 }, Cmd.none )

view : Model -> Html Msg
view model =
    Ui.layout []
        (Ui.column []
            [ Ui.el [ Ui.onClick Increment ] (Ui.text "+")
            , Ui.text (String.fromInt model.count)
            , Ui.el [ Ui.onClick Decrement ] (Ui.text "-")
            ])

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Live.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = []
        , notFound = Increment
        }
"#;

/// #93 seal: a `Std.Live` program with a NON-Model view-helper record that holds
/// an `Html` field (`Section = { title : String, body : Html Msg }`) and a
/// plain-data Model.
///
/// `Section` is `CDPeq`-supporting (`Html<Msg>` derives Clone/Debug/PartialEq) but
/// NOT serde-supporting. Before the #93 fix the emitter gated the serde derive on
/// the `CDPeq` flag, so `Section` got `#[derive(..., serde::Serialize,
/// serde::Deserialize)]` forced onto it under `uses_live` → `skyc` exit 0 then
/// `cargo build` E0277 (`Html<MainMsg>: Serialize` unsatisfied). The fix gates the
/// serde derive on the per-record serde flag, so `Section` keeps its `CDPeq` derive
/// WITHOUT serde and the project is cargo-buildable. The Model (`{ count : Int }`)
/// is plain data and still gets serde. `#91`'s Model-admissibility gate is NOT
/// tripped because the non-serde record is a view helper, not the Model.
const SKY_LIVE_HTML_HELPER: &str = r#"module Main exposing (main)

import Std.Live as Live
import Std.Ui as Ui

type Msg = Increment | Decrement

type alias Model = { count : Int }

type alias Section = { title : String, body : Html Msg }

renderSection : Section -> Element Msg
renderSection section =
    Ui.column [] [ Ui.text section.title, Ui.html section.body ]

init : a -> ( Model, Cmd Msg )
init _req =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )
        Decrement ->
            ( { model | count = model.count - 1 }, Cmd.none )

view : Model -> Html Msg
view model =
    Ui.layout []
        (renderSection { title = "Count", body = Ui.layout [] (Ui.text (String.fromInt model.count)) })

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Live.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = []
        , notFound = Increment
        }
"#;

/// A routed Sky.Live app: two pages, nullary page ctors, `routes`/`notFound`
/// supplied. The Model carries a `page` field → `emit_live_app_inner` takes
/// the T5 routed branch and emits `live_app_routed` instead of `live_app`.
///
/// Exercises the full T5 emit path through the compiler:
/// - open-record unification of the 6-field `Live.app` cfg (T2/T3)
/// - `routed_page_field` detection in `emit_live_app_inner` (T5)
/// - `set_page` closure generation (T5)
/// - `live_app_routed` runtime entry (already ported in `runtime/`)
///
/// This is the same structure as `examples/09-live-counter/src/Main.sky`.
const SKY_LIVE_ROUTED: &str = r#"module Main exposing (main)

import Std.Live as Live
import Std.Ui as Ui

type Page
    = CounterPage
    | AboutPage

type Msg = Increment | GoAbout | GoCounter

type alias Model =
    { page : Page
    , count : Int
    }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { page = CounterPage, count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        Increment ->
            ( { model | count = model.count + 1 }, Cmd.none )
        GoAbout ->
            ( { model | page = AboutPage }, Cmd.none )
        GoCounter ->
            ( { model | page = CounterPage }, Cmd.none )

view : Model -> Html Msg
view model =
    Ui.layout []
        (Ui.column []
            [ Ui.text (String.fromInt model.count)
            , Ui.el [ Ui.onClick Increment ] (Ui.text "+")
            ])

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Live.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = [ Live.route "/" CounterPage, Live.route "/about" AboutPage ]
        , notFound = CounterPage
        }
"#;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Compile a Sky program string, build the emitted Rust project, and return
/// the path to the compiled binary.
///
/// # Errors
///
/// Returns an error on any pipeline or Cargo build failure.
fn compile_and_build(test_name: &str, sky_source: &str) -> Result<PathBuf, BoxError> {
    let sky_dir = std::env::temp_dir().join(format!("live_e2e_{test_name}_sky"));
    let _ = std::fs::remove_dir_all(&sky_dir);
    std::fs::create_dir_all(&sky_dir).map_err(|e| -> BoxError {
        format!("{test_name}: cannot create sky source dir: {e}").into()
    })?;

    let entry = sky_dir.join("Main.sky");
    std::fs::write(&entry, sky_source)
        .map_err(|e| -> BoxError { format!("{test_name}: cannot write Main.sky: {e}").into() })?;

    let out_dir = std::env::temp_dir().join(format!("live_e2e_{test_name}_emitted"));
    let _ = std::fs::remove_dir_all(&out_dir);

    let runtime = skyc::resolve_runtime()
        .map_err(|e| -> BoxError { format!("{test_name}: runtime unavailable: {e}").into() })?;

    skyc::build(&entry, &out_dir, &runtime)
        .map_err(|e| -> BoxError { format!("{test_name}: skyc build failed: {e}").into() })?;

    let exe = oracle::build_rust_binary(test_name, &out_dir)
        .map_err(|e| -> BoxError { format!("{test_name}: cargo build failed: {e}").into() })?;

    Ok(PathBuf::from(exe))
}

/// Reserve an ephemeral loopback port by binding then immediately dropping a
/// `TcpListener`.  The OS assigns port 0 → an unused port.
///
/// There is a small TOCTOU window between the drop and the Sky server binding
/// the same port; in practice the window is negligible on a loopback test.
///
/// # Errors
///
/// Returns an error if the OS refuses to bind.
fn pick_ephemeral_port() -> Result<u16, BoxError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| -> BoxError { format!("cannot bind ephemeral port: {e}").into() })?;
    let port = listener
        .local_addr()
        .map_err(|e| -> BoxError { format!("cannot read ephemeral port: {e}").into() })?
        .port();
    // Drop `listener` here — releases the port for the Sky Live server.
    Ok(port)
}

/// RAII guard: kills the wrapped child process on `Drop`.
struct ProcessGuard(Child);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawn the Sky Live binary and wait until it signals readiness via
/// `[sky.live] listening on` on stderr.
///
/// # Errors
///
/// Returns an error if the binary cannot be spawned or the ready signal does
/// not appear within 10 s.
fn spawn_and_wait_ready(
    test_name: &str,
    exe: &std::path::Path,
    port: u16,
) -> Result<ProcessGuard, BoxError> {
    let mut child = Command::new(exe)
        // Sky.Live reads its port from SKY_LIVE_PORT (default 8000).
        .env("SKY_LIVE_PORT", port.to_string())
        // Disable the double-submit CSRF check so raw TcpStream GETs work.
        .env("SKY_CSRF", "off")
        // Disable the dev console proxy. The console child binary is pre-built
        // and cached on this machine; without this gate it is spawned on its
        // own ephemeral port and emits its own `[sky.live] listening on`
        // to the inherited stderr pipe before the parent app has bound its
        // port. The test sees that line, declares the server ready, and then
        // immediately tries to connect to the parent's port — which is not
        // bound yet — getting ECONNREFUSED. Setting SKY_CONSOLE_EMBED=off
        // makes gate_allows() return false so no child is spawned and the
        // only `[sky.live] listening on` line in the stderr pipe is the
        // parent's own (emitted AFTER the TCP listener is bound).
        .env("SKY_CONSOLE_EMBED", "off")
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| -> BoxError {
            format!("{test_name}: cannot spawn Live binary: {e}").into()
        })?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| -> BoxError { format!("{test_name}: child stderr pipe was None").into() })?;

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();

    loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(
                format!("{test_name}: Sky Live did not signal readiness within 10 s").into(),
            );
        }
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                let _ = child.wait();
                return Err(format!(
                    "{test_name}: Sky Live process exited before signalling ready"
                )
                .into());
            }
            Ok(_) => {
                // The live runtime emits: `[sky.live] listening on http://0.0.0.0:<port>`
                if line.contains("[sky.live] listening on") {
                    return Ok(ProcessGuard(child));
                }
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{test_name}: error reading child stderr: {e}").into());
            }
        }
    }
}

/// Send a raw HTTP/1.1 request and return the split `(raw_headers, body)`.
///
/// `extra_headers` is a slice of `(name, value)` pairs added after the
/// standard `Host` and `Connection` headers.  For POST requests `body` is
/// the bytes to send; pass `None` for GET.  The helper automatically adds a
/// `Content-Length` header when `body` is `Some`.
///
/// Reads up to 64 KiB (sufficient for a full HTML page from the counter app).
///
/// # Errors
///
/// Returns an error if the stream write or read fails.
fn http_send(
    test_name: &str,
    addr: &str,
    method: &str,
    path: &str,
    extra_headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<(String, String), BoxError> {
    let mut stream = TcpStream::connect(addr).map_err(|e| -> BoxError {
        format!("{test_name}: cannot connect to Sky Live server: {e}").into()
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| -> BoxError { format!("{test_name}: set_read_timeout failed: {e}").into() })?;

    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (k, v) in extra_headers {
        request.push_str(k);
        request.push_str(": ");
        request.push_str(v);
        request.push_str("\r\n");
    }
    if let Some(b) = body {
        request.push_str("Content-Length: ");
        request.push_str(&b.len().to_string());
        request.push_str("\r\n");
    }
    request.push_str("\r\n");

    stream
        .write_all(request.as_bytes())
        .map_err(|e| -> BoxError { format!("{test_name}: write headers failed: {e}").into() })?;
    if let Some(b) = body {
        stream
            .write_all(b)
            .map_err(|e| -> BoxError { format!("{test_name}: write body failed: {e}").into() })?;
    }

    let mut buf = Vec::with_capacity(65536);
    stream
        .read_to_end(&mut buf)
        .map_err(|e| -> BoxError { format!("{test_name}: read failed: {e}").into() })?;

    let response = String::from_utf8_lossy(&buf).into_owned();
    match response.find("\r\n\r\n") {
        Some(idx) => Ok((response[..idx].to_owned(), response[idx + 4..].to_owned())),
        None => Ok((response, String::new())),
    }
}

/// Convenience wrapper: `GET <path>` with no extra headers; returns the body.
fn http_get(test_name: &str, addr: &str, path: &str) -> Result<String, BoxError> {
    let (_, body) = http_send(test_name, addr, "GET", path, &[], None)?;
    Ok(body)
}

/// Extract the value of a named cookie from raw HTTP response headers.
///
/// Searches each `Set-Cookie:` line for `<name>=<value>` (header name
/// matched case-insensitively).
///
/// Returns `None` when no matching `Set-Cookie` line is present.
fn extract_cookie(raw_headers: &str, name: &str) -> Option<String> {
    for line in raw_headers.lines() {
        if !line.to_ascii_lowercase().starts_with("set-cookie:") {
            continue;
        }
        let value_part = line["set-cookie:".len()..].trim();
        // Cookie string: `NAME=VALUE; attr=…`
        let prefix = format!("{name}=");
        if value_part
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
        {
            let after = &value_part[prefix.len()..];
            let end = after.find(';').unwrap_or(after.len());
            return Some(after[..end].trim().to_string());
        }
    }
    None
}

/// Extract the `data-sky-hid` attribute value from the nearest element that
/// directly contains the given text node.
///
/// Searches backwards from `>TEXT<` for `data-sky-hid="…"` within the
/// element's opening tag — the runtime emits this attribute on every element
/// that carries at least one event handler, keyed to the handler-index lookup.
///
/// Returns `None` when no match is found (e.g. the element does not carry a
/// click handler, or the text does not appear in the page).
fn extract_hid_near_text(html: &str, text: &str) -> Option<String> {
    let marker = format!(">{text}<");
    let text_pos = html.find(&marker)?;
    // Everything before the `>` that closes the element's start tag.
    let before = &html[..text_pos];
    let attr_prefix = "data-sky-hid=\"";
    // `rfind`: take the NEAREST (last) occurrence — the direct parent's id,
    // not an ancestor's.
    let hid_pos = before.rfind(attr_prefix)?;
    let after = &before[hid_pos + attr_prefix.len()..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

/// Extract the `data-sky-hid` attribute value from the FIRST `<tag …>` open
/// tag in `html` (e.g. `"form"`) — used for elements (like `<form>`) that
/// don't necessarily wrap a distinguishing text node directly.
///
/// Returns `None` when the tag or the attribute is not present.
fn extract_hid_for_open_tag(html: &str, tag: &str) -> Option<String> {
    let open_marker = format!("<{tag} ");
    let tag_pos = html.find(&open_marker)?;
    let after_tag = &html[tag_pos..];
    let tag_end = after_tag.find('>')?;
    let tag_slice = &after_tag[..tag_end];
    let attr_prefix = "data-sky-hid=\"";
    let hid_pos = tag_slice.find(attr_prefix)?;
    let after = &tag_slice[hid_pos + attr_prefix.len()..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// `GET /` on a Sky.Live counter app returns an HTML page containing the
/// initial counter value rendered as the text node `>0<`.
///
/// This test proves the FULL Phase-1b pipeline end-to-end:
///
/// ```text
/// Sky source
///   → skyc (parse → canon → types → lower → emit Rust with "live" feature)
///   → cargo build
///   → sky_runtime::live::live_app(init, update, view, subs, …)
///   → init(LiveReq) → (Model{count:0}, Cmd::None)
///   → view(model)   → Html tree with text node "0"
///   → render_page   → full HTML document
///   → axum HTTP response
///   → test asserts ">0<" appears in the body
/// ```
///
/// The assertion uses `>0<` rather than the bare character `'0'` to avoid
/// false positives from CSS values, sky-ids, or other numeric occurrences in
/// the generated page markup.
///
/// The `live` Cargo feature is injected by `emit_program` when `uses_live` is
/// set (B1 + B2 fix for the Phase-1b blockers).  Without the Phase-1b fixes
/// the build would fail with `exit 0 then cargo fail` (constraint scheme
/// missing) or `cargo build error` (serde derives absent).
///
/// # Errors
///
/// Propagates any pipeline, build, spawn, or HTTP error as a test error.
#[test]
fn live_get_root_contains_initial_count() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let test_name = "live_get_root";
    let exe = compile_and_build(test_name, SKY_LIVE_COUNTER)?;
    let port = pick_ephemeral_port()?;
    let _guard = spawn_and_wait_ready(test_name, &exe, port)?;

    let addr = format!("127.0.0.1:{port}");
    let body = http_get(test_name, &addr, "/")?;

    // The rendered page wraps every text node in an element with a sky-id.
    // The counter is `Ui.text (String.fromInt 0)` → renders text "0" inside
    // an element.  We assert the `>0<` sequence to distinguish the counter
    // text node from other numeric occurrences in page markup (sky-ids, CSS
    // values, etc.).
    assert!(
        body.contains(">0<"),
        "live_get_root: initial counter (>0<) not found in GET / body\n\
         --- first 2000 bytes ---\n{}",
        &body[..body.len().min(2000)]
    );
    // Sanity: must be an HTML document, not an error message.
    assert!(
        body.contains("<!DOCTYPE html>") || body.contains("<html"),
        "live_get_root: response does not look like HTML\n\
         --- first 500 bytes ---\n{}",
        &body[..body.len().min(500)]
    );
    Ok(())
}

/// Compile-only: the Sky.Live counter emits a Cargo project with the `"live"`
/// feature in the default feature list.
///
/// This is a BUILD-ONLY test — it does not spawn the binary.  A successful
/// `cargo build` is the assertion.  This specifically regression-tests the
/// Phase-1b B2 fix: if `serde::Serialize / Deserialize` derives are absent on
/// the `Msg` enum and `Model` struct, `cargo build` will fail with a trait-
/// bound error from `sky_runtime::live::live_app`.
///
/// # Errors
///
/// Propagates any pipeline or Cargo build failure as a test error.
#[test]
fn live_counter_build_only() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    // compile_and_build already does skyc + cargo build; success is the proof.
    let _exe = compile_and_build("live_build_only", SKY_LIVE_COUNTER)?;
    Ok(())
}

/// #93 seal — BUILD-ONLY: a Std.Live program with a NON-Model view-helper record
/// holding an `Html` field must compile end-to-end.
///
/// A successful `skyc` + `cargo build` IS the assertion. Before the #93 fix this
/// project was `skyc` exit 0 then `cargo build` E0277 (`Html<MainMsg>: Serialize`
/// unsatisfied on the `Section` struct's forced serde derive). See
/// `SKY_LIVE_HTML_HELPER` for the full rationale.
///
/// # Errors
///
/// Propagates any pipeline or Cargo build failure as a test error.
#[test]
fn live_html_helper_record_build_only() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let _exe = compile_and_build("live_html_helper", SKY_LIVE_HTML_HELPER)?;
    Ok(())
}

/// A click on the `+` element increments the counter: `GET /` → extract
/// session cookie + sky-id of the `+` element → `POST /_sky/event` (click) →
/// wait for the async model update → `GET /` with session cookie → body
/// contains `>1<`.
///
/// ## Wire protocol exercised
///
/// 1. `GET /` → axum sets `sky_sid=<sid>` in `Set-Cookie`.  The initial model
///    (`count = 0`) is rendered with `>0<`.
/// 2. The rendered HTML carries `data-sky-hid="<sky-id>"` on every element
///    that has event handlers.  The `Ui.el [ Ui.onClick Increment ]` element
///    (containing `+`) is found by searching backwards from `>+<` for the
///    nearest `data-sky-hid`.
/// 3. `POST /_sky/event` with body
///    `{"id":"<sky-id>","msg":"click","args":[],"sessionId":""}` and
///    `Cookie: sky_sid=<sid>`.  The runtime authenticates via the COOKIE only;
///    the body's `sessionId` is ignored per the security policy.
/// 4. The event is enqueued via `try_send` and processed asynchronously by
///    `drive_session`.  A 200 ms sleep after the POST gives the driver time to
///    commit the model update.
/// 5. A second `GET /` with `Cookie: sky_sid=<sid>` re-renders the live
///    session (store hit → re-view with current model) → the body must contain
///    `>1<`.
///
/// This test is the definitive proof that the full TEA loop works:
/// `init → view → event → update → re-view` over the HTTP/1.1 wire.
///
/// # Errors
///
/// Propagates any pipeline, build, spawn, HTTP, or assertion error.
#[test]
fn live_onclick_increments_counter() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let test_name = "live_onclick";
    let exe = compile_and_build(test_name, SKY_LIVE_COUNTER)?;
    let port = pick_ephemeral_port()?;
    let _guard = spawn_and_wait_ready(test_name, &exe, port)?;
    let addr = format!("127.0.0.1:{port}");

    // ── Step 1: GET / — initial page, extract session cookie + sky-id ─────────
    let (raw_headers, body) = http_send(test_name, &addr, "GET", "/", &[], None)?;

    let sid = extract_cookie(&raw_headers, "sky_sid").ok_or_else(|| -> BoxError {
        format!(
            "{test_name}: no sky_sid cookie in GET / response\n\
             --- raw headers ---\n{raw_headers}"
        )
        .into()
    })?;

    let hid = extract_hid_near_text(&body, "+").ok_or_else(|| -> BoxError {
        format!(
            "{test_name}: could not find data-sky-hid near '>+<' in GET / body\n\
             --- first 2000 bytes ---\n{}",
            &body[..body.len().min(2000)]
        )
        .into()
    })?;

    // ── Step 2: POST /_sky/event — Increment click ───────────────────────────
    //
    // `msg` carries the event name (the runtime prefers `event`, then `msg`,
    // then defaults to "click").  `sessionId` is retained in the body for
    // wire-compat but ignored by the server; auth is via the Cookie header.
    let event_body = format!(r#"{{"id":"{hid}","msg":"click","args":[],"sessionId":""}}"#);
    let cookie_header = format!("sky_sid={sid}");
    let (_, post_body) = http_send(
        test_name,
        &addr,
        "POST",
        "/_sky/event",
        &[
            ("Content-Type", "application/json"),
            ("Cookie", &cookie_header),
        ],
        Some(event_body.as_bytes()),
    )?;

    assert!(
        post_body.contains("patches"),
        "{test_name}: POST /_sky/event did not return a patches ACK\nbody: {post_body}"
    );

    // ── Step 3: wait for async model update ─────────────────────────────────
    //
    // `event_handler` uses `try_send` (non-blocking) — the actual model update
    // happens in the `drive_session` task.  200 ms is sufficient for the
    // in-process channel round-trip on any reasonable test host.
    std::thread::sleep(Duration::from_millis(200));

    // ── Step 4: GET / with session cookie — should show count = 1 ───────────
    let (_, body2) = http_send(
        test_name,
        &addr,
        "GET",
        "/",
        &[("Cookie", &cookie_header)],
        None,
    )?;

    assert!(
        body2.contains(">1<"),
        "{test_name}: counter not incremented to 1 after Increment click\n\
         --- first 2000 bytes of second GET / ---\n{}",
        &body2[..body2.len().min(2000)]
    );

    Ok(())
}

/// T5 seal — BUILD-ONLY: a routed `Live.app` with a `page` field in the Model
/// must compile and produce a Cargo project that links against
/// `live_app_routed` rather than `live_app`.
///
/// This regression-tests the full T3→T5 emit path:
/// - T3: 6-field open-record constraint passes type-checking.
/// - T5: `routed_page_field` detects the `page` field in the Model and the
///   emitter branches to `live_app_routed` with a generated `set_page` closure.
///
/// A successful `skyc` + `cargo build` is the assertion.  If T5 emits
/// `live_app` instead of `live_app_routed`, or emits a malformed `set_page`
/// closure, `cargo build` surfaces the type mismatch.
///
/// # Errors
///
/// Propagates any pipeline or Cargo build failure as a test error.
#[test]
fn live_routed_app_build_only() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let _exe = compile_and_build("live_routed_build_only", SKY_LIVE_ROUTED)?;
    Ok(())
}

/// M5e seal — BUILD-ONLY: a Sky.Live app that uses `Cmd.publish` and
/// `Sub.subscribeTopic` must compile end-to-end without a `CompilerBug`
/// diagnostic.
///
/// Kernels exercised:
/// - `Cmd.publish : String -> Dict String String -> Cmd msg`  (M5e wired)
/// - `Sub.subscribeTopic : String -> (Dict String String -> msg) -> Sub msg`
///   (M5d wired — exercised here as the natural pair to `Cmd.publish`)
///
/// A successful `skyc` + `cargo build` is the assertion.  Before the M5e wiring
/// the compiler emitted a `CompilerBug` diagnostic when it encountered
/// `Cmd.publish` or `Cmd.publishNoEcho` — exit-0 was structurally impossible.
///
/// The app structure mirrors `examples/27-multi-session-chat` at its simplest:
/// one pub/sub topic `"chat"`, a `BroadcastMsg` that carries the payload dict,
/// and an `update` arm that publishes then clears the pending message.
///
/// # Errors
///
/// Propagates any pipeline or Cargo build failure as a test error.
const SKY_PUBSUB_LIVE: &str = r#"module Main exposing (main)

import Std.Live as Live
import Std.Ui as Ui

type Msg
    = BroadcastMsg (Dict String String)
    | TypeMsg String

type alias Model =
    { pending : String
    , received : List String
    }

chatTopic : String
chatTopic =
    "chat"

init : a -> ( Model, Cmd Msg )
init _req =
    ( { pending = "", received = [] }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        TypeMsg s ->
            ( { model | pending = s }, Cmd.none )

        BroadcastMsg payload ->
            let
                text =
                    Maybe.withDefault "" (Dict.get "text" payload)
            in
            ( { model | received = model.received ++ [ text ] }
            , Cmd.none
            )

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.subscribeTopic chatTopic BroadcastMsg

sendMessage : Model -> Cmd Msg
sendMessage model =
    let
        payload =
            Dict.fromList [ ( "text", model.pending ) ]
    in
    Cmd.publish chatTopic payload

view : Model -> Html Msg
view model =
    Ui.layout []
        (Ui.column []
            [ Ui.text (String.fromInt (List.length model.received))
            ])

main =
    Live.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = []
        , notFound = TypeMsg ""
        }
"#;

#[test]
fn live_pubsub_cmd_publish_and_sub_subscribe_topic_build_only() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let _exe = compile_and_build("live_pubsub_build_only", SKY_PUBSUB_LIVE)?;
    Ok(())
}

/// Regression: `Cmd.publish` / `Cmd.publishNoEcho` must accept a record payload
/// (or any Sky value), not only `Dict String String`.
///
/// Before the fix the constrain scheme was `String -> Dict String String -> Cmd
/// msg`.  Publishing `{ count : Int, name : String }` produced SKY-T0001
/// (type mismatch at examples/37-composite-live-shop/src/Update.sky:34:38).
/// After the fix the scheme is `String -> any -> Cmd msg` (var(1) for payload),
/// matching the reference runtime which is generic in T.
///
/// This test also asserts `cargo build` succeeds — verifying the re-export
/// (`cmd_publish`, `cmd_publish_no_echo` in `RUNTIME_MOD_RS_LIVE_APPEND`) is
/// present; without it the emitted project would fail with E0425 (seal violation).
const SKY_PUBSUB_RECORD_PAYLOAD: &str = r#"module Main exposing (main)

import Std.Live as Live
import Std.Ui as Ui

type alias CartItem =
    { count : Int
    , name : String
    }

type Msg
    = AddItem CartItem
    | SendCart CartItem

type alias Model =
    { items : List CartItem
    }

cartTopic : String
cartTopic =
    "cart"

init : a -> ( Model, Cmd Msg )
init _req =
    ( { items = [] }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        AddItem item ->
            ( { model | items = model.items ++ [ item ] }
            , Cmd.batch
                [ Cmd.publish cartTopic item
                , Cmd.publishNoEcho cartTopic item
                ]
            )

        SendCart item ->
            ( model, Cmd.publish cartTopic item )

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.subscribeTopic cartTopic AddItem

view : Model -> Html Msg
view model =
    Ui.layout []
        (Ui.column []
            [ Ui.text (String.fromInt (List.length model.items))
            ])

main =
    Live.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = []
        , notFound = SendCart { count = 0, name = "" }
        }
"#;

#[test]
fn live_pubsub_publish_polymorphic_record_payload_build_only() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let _exe = compile_and_build(
        "live_pubsub_record_payload_build_only",
        SKY_PUBSUB_RECORD_PAYLOAD,
    )?;
    Ok(())
}

/// #162 regression — the CANONICAL CLAUDE.md "forms with passwords" idiom:
/// `Ui.form [Ui.onSubmit DoSignIn] [...]` where `DoSignIn : Creds -> Msg` is a
/// TYPED-RECORD payload constructor (not a bare Msg). This is the exact shape
/// `examples/19-skyforum`'s `View/Login.sky` and `examples/27-multi-session-chat`
/// use in production.
///
/// Before the fix: `skyc build` exits 0, but the emitted crate fails `cargo
/// build` with E0277 — `ui_on_submit_`'s generic bound `F: Fn(T) -> M + Send +
/// Sync + 'static` was never satisfiable because the emit site passed the
/// codegen's boxed closure value (`Box<dyn Fn(T) -> M + Send + 'static>` — the
/// generic `IrType::Fun` rendering, which never claims `+Sync`) straight
/// through as `F`. A trait object's auto-trait set is exactly its bound list,
/// so that box could never satisfy `+ Sync` regardless of what it captured —
/// a genuine skyc-accept/cargo-reject SEAL violation on the single
/// most-recommended Sky.Live form pattern. Fixed by re-wrapping the boxed
/// value in a freshly-declared closure at the `KernelFn::UiOnSubmit` /
/// `HtmlEventShape::Raw` emit sites (`sky_backend_rust::emit_expr`) instead of
/// forwarding the box itself — see that arm's comment for the full mechanism.
const SKY_ONSUBMIT_TYPED_RECORD: &str = r#"module Main exposing (main)

import Std.Live as Live
import Std.Ui as Ui

type alias Creds =
    { username : String
    , password : String
    }

type Msg
    = DoSignIn Creds

type alias Model =
    { lastUsername : String
    }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { lastUsername = "" }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        DoSignIn creds ->
            ( { model | lastUsername = creds.username }, Cmd.none )

view : Model -> Html Msg
view model =
    Ui.layout []
        (Ui.column []
            [ Ui.form
                [ Ui.onSubmit DoSignIn ]
                [ Ui.input [ Ui.name "username" ]
                , Ui.input [ Ui.name "password" ]
                , Ui.input [ Ui.htmlAttribute "type" "submit" ]
                ]
            , Ui.text model.lastUsername
            ])

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Live.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = []
        , notFound = DoSignIn { username = "", password = "" }
        }
"#;

/// #162 seal — BUILD-ONLY: the typed-record `Ui.onSubmit` form must compile
/// end-to-end. A successful `skyc` + `cargo build` IS the assertion — before
/// the fix this was `skyc` exit 0 then `cargo build` E0277 (`dyn Fn(Creds) ->
/// MainMsg + Send` cannot be shared between threads safely).
///
/// # Errors
///
/// Propagates any pipeline or Cargo build failure as a test error.
#[test]
fn live_onsubmit_typed_record_build_only() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let _exe = compile_and_build(
        "live_onsubmit_typed_record_build_only",
        SKY_ONSUBMIT_TYPED_RECORD,
    )?;
    Ok(())
}

/// #162 seal — FULL E2E: submitting the typed-record form over the wire must
/// dispatch `DoSignIn` with the DECODED `Creds` record, proving the fix is not
/// merely a compile-time patch that leaves the handler undispatchable at
/// runtime (the historical failure mode of the pre-#109/#156 `OnRaw` variant).
///
/// ## Wire protocol exercised
///
/// 1. `GET /` → session cookie + the `<form>`'s `data-sky-hid`.
/// 2. `POST /_sky/event` with
///    `{"id":"<hid>","event":"submit","args":[{"username":"alice","password":"s3cr3t"}],"sessionId":""}`
///    — mirrors what the browser's delegated form-submit binder sends
///    (`live/mod.rs`'s `event == "submit"` branch treats `args[0]` as the
///    form-data object).
/// 3. A second `GET /` with the session cookie must show `>alice<` — proving
///    `resolve_form` → `decode_form_or_warn::<Creds>` → `DoSignIn` → `update`
///    ran end-to-end with the CONCRETE decoded record, not a type-erased or
///    dropped payload.
///
/// # Errors
///
/// Propagates any pipeline, build, spawn, HTTP, or assertion error.
#[test]
fn live_onsubmit_typed_record_dispatches_decoded_payload() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let test_name = "live_onsubmit_typed_record";
    let exe = compile_and_build(test_name, SKY_ONSUBMIT_TYPED_RECORD)?;
    let port = pick_ephemeral_port()?;
    let _guard = spawn_and_wait_ready(test_name, &exe, port)?;
    let addr = format!("127.0.0.1:{port}");

    // ── Step 1: GET / — initial page, extract session cookie + form hid ─────
    let (raw_headers, body) = http_send(test_name, &addr, "GET", "/", &[], None)?;

    let sid = extract_cookie(&raw_headers, "sky_sid").ok_or_else(|| -> BoxError {
        format!(
            "{test_name}: no sky_sid cookie in GET / response\n\
             --- raw headers ---\n{raw_headers}"
        )
        .into()
    })?;

    let hid = extract_hid_for_open_tag(&body, "form").ok_or_else(|| -> BoxError {
        format!(
            "{test_name}: could not find data-sky-hid on <form> in GET / body\n\
             --- first 2000 bytes ---\n{}",
            &body[..body.len().min(2000)]
        )
        .into()
    })?;

    // ── Step 2: POST /_sky/event — submit with the typed-record form data ──
    let event_body = format!(
        r#"{{"id":"{hid}","event":"submit","args":[{{"username":"alice","password":"s3cr3t"}}],"sessionId":""}}"#
    );
    let cookie_header = format!("sky_sid={sid}");
    let (_, post_body) = http_send(
        test_name,
        &addr,
        "POST",
        "/_sky/event",
        &[
            ("Content-Type", "application/json"),
            ("Cookie", &cookie_header),
        ],
        Some(event_body.as_bytes()),
    )?;

    assert!(
        post_body.contains("patches"),
        "{test_name}: POST /_sky/event (submit) did not return a patches ACK\nbody: {post_body}"
    );

    // ── Step 3: wait for async model update ─────────────────────────────────
    std::thread::sleep(Duration::from_millis(200));

    // ── Step 4: GET / with session cookie — should show the decoded username
    let (_, body2) = http_send(
        test_name,
        &addr,
        "GET",
        "/",
        &[("Cookie", &cookie_header)],
        None,
    )?;

    assert!(
        body2.contains(">alice<"),
        "{test_name}: DoSignIn was not dispatched with the decoded Creds \
         record (expected the re-rendered page to contain the username \
         \"alice\")\n--- first 2000 bytes of second GET / ---\n{}",
        &body2[..body2.len().min(2000)]
    );

    Ok(())
}

/// #167 regression — the "ignore form data, always dispatch this fixed
/// action" idiom: `Std.Html.Events.onSubmit Confirm` where `Confirm : Msg`
/// carries NO payload (a nullary constructor, not a decoder function). This
/// is the exact shape `examples/12-skyvote`'s `Page/AuthPage.sky` /
/// `Page/Submit.sky` / `Page/Detail.sky` use throughout (`onSubmit
/// DoSignUp` / `DoSignIn` / `SubmitIdea` / `SubmitComment`) — form fields
/// are already synced into `Model` via `onInput`/`onChange`; `onSubmit`
/// just triggers the action, ignoring the posted `FormData` entirely.
///
/// Before the fix: `skyc build` exits 0 (`HtmlOnSubmit`'s Sky-level scheme
/// deliberately leaves the argument type unconstrained — decoupled from
/// `msg`, see `constrain.rs`'s `HtmlEventShape::Raw` arm — so a Msg-typed
/// value type-checks fine there), but the emitted crate fails `cargo
/// build` with E0618 ("expected function, found `MainMsg`") — the emit
/// site unconditionally treated the argument as a callable decoder
/// (`(payload_s)(_x)`), but a bare nullary constructor reference lowers to
/// a plain `Expr::Ctor` VALUE (`lower_expr`'s `VarCtor` arm), never a
/// function. Fixed by routing a provably-non-callable argument shape to
/// the new `html_on_raw_fixed_` runtime helper (dispatches the fixed value
/// directly, no decode attempt) instead of `html_on_raw_`
/// (`sky_backend_rust::emit_expr`'s `is_definitely_not_callable` gate).
const SKY_ONSUBMIT_BARE_MSG: &str = r#"module Main exposing (main)

import Std.Live as Live
import Std.Html exposing (..)
import Std.Html.Attributes exposing (..)
import Std.Html.Events exposing (onSubmit, onInput)

type Msg
    = UpdateName String
    | Confirm

type alias Model =
    { name : String
    , confirmed : Bool
    }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { name = "", confirmed = False }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        UpdateName n ->
            ( { model | name = n }, Cmd.none )

        Confirm ->
            ( { model | confirmed = True }, Cmd.none )

view : Model -> Html Msg
view model =
    div []
        [ form
            [ onSubmit Confirm ]
            [ input [ type_ "text", name "name", value model.name, onInput UpdateName ]
            , button [ type_ "submit" ] [ text "Go" ]
            ]
        , text
            (if model.confirmed then
                "confirmed:" ++ model.name

             else
                "not-confirmed"
            )
        ]

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Live.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = []
        , notFound = Confirm
        }
"#;

/// #167 seal — BUILD-ONLY: the bare-Msg `onSubmit` form must compile
/// end-to-end. A successful `skyc` + `cargo build` IS the assertion —
/// before the fix this was `skyc` exit 0 then `cargo build` E0618.
///
/// # Errors
///
/// Propagates any pipeline or Cargo build failure as a test error.
#[test]
fn live_onsubmit_bare_msg_build_only() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let _exe = compile_and_build("live_onsubmit_bare_msg_build_only", SKY_ONSUBMIT_BARE_MSG)?;
    Ok(())
}

/// #167 seal — FULL E2E: submitting the bare-Msg form over the wire must
/// dispatch `Confirm` regardless of the posted `FormData` — proving the fix
/// is not merely a compile-time patch that leaves the handler
/// undispatchable (or, worse, silently swallowed by a decode failure — see
/// `html_on_raw_fixed_`'s doc for why it deliberately does NOT route
/// through `decode_form_or_warn`). The POSTed form body deliberately
/// carries a REAL field value (`name=alice`, matching the `<input
/// name="name">` in the view) to prove the fixed dispatch survives
/// non-empty form data, not just the trivial empty-body case.
///
/// ## Wire protocol exercised
///
/// 1. `GET /` → session cookie + the `<form>`'s `data-sky-hid`.
/// 2. `POST /_sky/event` with
///    `{"id":"<hid>","event":"submit","args":[{"name":"alice"}],"sessionId":""}`
///    — mirrors what the browser's delegated form-submit binder sends.
/// 3. A second `GET /` with the session cookie must show the "confirmed:"
///    marker — proving `resolve_form` → `html_on_raw_fixed_`'s closure →
///    `Confirm` → `update` ran end-to-end, dispatching the FIXED value
///    (not the posted `name` field, which `update`'s `Confirm` arm never
///    reads — `model.name` stays empty since no separate `onInput` event
///    was sent, so `"confirmed:alice"` would be a FALSE pass here; the
///    assertion below checks the "confirmed:" prefix only).
///
/// # Errors
///
/// Propagates any pipeline, build, spawn, HTTP, or assertion error.
#[test]
fn live_onsubmit_bare_msg_dispatches_fixed_msg() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let test_name = "live_onsubmit_bare_msg";
    let exe = compile_and_build(test_name, SKY_ONSUBMIT_BARE_MSG)?;
    let port = pick_ephemeral_port()?;
    let _guard = spawn_and_wait_ready(test_name, &exe, port)?;
    let addr = format!("127.0.0.1:{port}");

    // ── Step 1: GET / — initial page, extract session cookie + form hid ─────
    let (raw_headers, body) = http_send(test_name, &addr, "GET", "/", &[], None)?;

    let sid = extract_cookie(&raw_headers, "sky_sid").ok_or_else(|| -> BoxError {
        format!(
            "{test_name}: no sky_sid cookie in GET / response\n\
             --- raw headers ---\n{raw_headers}"
        )
        .into()
    })?;

    let hid = extract_hid_for_open_tag(&body, "form").ok_or_else(|| -> BoxError {
        format!(
            "{test_name}: could not find data-sky-hid on <form> in GET / body\n\
             --- first 2000 bytes ---\n{}",
            &body[..body.len().min(2000)]
        )
        .into()
    })?;

    // ── Step 2: POST /_sky/event — submit with REAL (but ignored) form data ─
    let event_body =
        format!(r#"{{"id":"{hid}","event":"submit","args":[{{"name":"alice"}}],"sessionId":""}}"#);
    let cookie_header = format!("sky_sid={sid}");
    let (_, post_body) = http_send(
        test_name,
        &addr,
        "POST",
        "/_sky/event",
        &[
            ("Content-Type", "application/json"),
            ("Cookie", &cookie_header),
        ],
        Some(event_body.as_bytes()),
    )?;

    assert!(
        post_body.contains("patches"),
        "{test_name}: POST /_sky/event (submit) did not return a patches ACK\nbody: {post_body}"
    );

    // ── Step 3: wait for async model update ─────────────────────────────────
    std::thread::sleep(Duration::from_millis(200));

    // ── Step 4: GET / with session cookie — should show the fixed dispatch ──
    let (_, body2) = http_send(
        test_name,
        &addr,
        "GET",
        "/",
        &[("Cookie", &cookie_header)],
        None,
    )?;

    assert!(
        body2.contains("confirmed:"),
        "{test_name}: Confirm was not dispatched on bare-Msg onSubmit \
         (expected the re-rendered page to contain \"confirmed:\")\n\
         --- first 2000 bytes of second GET / ---\n{}",
        &body2[..body2.len().min(2000)]
    );

    Ok(())
}

// ── #170 — onSubmit COMPOUND-literal payloads (record / tuple / list) ─────────
//
// #167 closed the bare nullary-`Ctor` onSubmit payload (`onSubmit Confirm`,
// `Confirm : Msg`). The same emit gate (`is_definitely_not_callable`) left
// three sibling literal shapes on the wrap-and-call path even though they are
// EQUALLY provably-non-callable structural values: a record literal
// (`{ … }`), a tuple literal (`(…, …)`), and a list literal (`[…]`).
//
// `HtmlOnSubmit`'s payload type is `var(1)` in `constrain.rs`'s
// `HtmlEventShape::Raw` arm — decoupled from `msg`, hence UNCONSTRAINED — so
// these shapes type-check in skyc; the well-typed, SEAL-relevant program is one
// whose `Msg` type IS that record / tuple / list (`type alias Msg = { … }`
// etc), where the literal is a genuine `Msg` value. Before #170 each such
// program was `skyc` exit 0 then `cargo build` E0618 ("expected function,
// found …") — the emit site unconditionally wrapped the value in
// `(payload_s)(_x)`. #170 extends `is_definitely_not_callable` to the three
// compound literal variants (`Expr::Record` / `Expr::Tuple` / `Expr::List`),
// routing them to `html_on_raw_fixed_` (fixed dispatch, no decode) — sealing
// the class. A successful `skyc` + `cargo build` IS the assertion.

/// #170 — `onSubmit` payload is a RECORD literal, `Msg` is a record alias.
const SKY_ONSUBMIT_RECORD_LITERAL: &str = r#"module Main exposing (main)

import Std.Live as Live
import Std.Html exposing (..)
import Std.Html.Attributes exposing (..)
import Std.Html.Events exposing (onSubmit)

type alias Msg =
    { action : String }

type alias Model =
    { last : String }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { last = "" }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    ( { model | last = msg.action }, Cmd.none )

view : Model -> Html Msg
view model =
    div []
        [ form
            [ onSubmit { action = "confirmed" } ]
            [ button [ type_ "submit" ] [ text "Go" ] ]
        , text model.last
        ]

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Live.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = []
        , notFound = { action = "" }
        }
"#;

/// #170 — `onSubmit` payload is a TUPLE literal, `Msg` is a tuple alias.
const SKY_ONSUBMIT_TUPLE_LITERAL: &str = r#"module Main exposing (main)

import Std.Live as Live
import Std.Html exposing (..)
import Std.Html.Attributes exposing (..)
import Std.Html.Events exposing (onSubmit)

type alias Msg =
    ( String, Int )

type alias Model =
    { last : String }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { last = "" }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    let
        ( label, _n ) =
            msg
    in
    ( { model | last = label }, Cmd.none )

view : Model -> Html Msg
view model =
    div []
        [ form
            [ onSubmit ( "confirmed", 1 ) ]
            [ button [ type_ "submit" ] [ text "Go" ] ]
        , text model.last
        ]

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Live.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = []
        , notFound = ( "", 0 )
        }
"#;

/// #170 — `onSubmit` payload is a LIST literal, `Msg` is a list alias.
const SKY_ONSUBMIT_LIST_LITERAL: &str = r#"module Main exposing (main)

import Std.Live as Live
import Std.Html exposing (..)
import Std.Html.Attributes exposing (..)
import Std.Html.Events exposing (onSubmit)

type alias Msg =
    List String

type alias Model =
    { count : Int }

init : a -> ( Model, Cmd Msg )
init _req =
    ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    ( { model | count = List.length msg }, Cmd.none )

view : Model -> Html Msg
view model =
    div []
        [ form
            [ onSubmit [ "a", "b", "c" ] ]
            [ button [ type_ "submit" ] [ text "Go" ] ]
        , text (String.fromInt model.count)
        ]

subscriptions : Model -> Sub Msg
subscriptions _model =
    Sub.none

main =
    Live.app
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        , routes = []
        , notFound = [ "" ]
        }
"#;

/// #170 seal — BUILD-ONLY: a RECORD-literal `onSubmit` payload must compile
/// end-to-end. A successful `skyc` + `cargo build` IS the assertion — before
/// the fix this was `skyc` exit 0 then `cargo build` E0618.
///
/// # Errors
///
/// Propagates any pipeline or Cargo build failure as a test error.
#[test]
fn live_onsubmit_record_literal_build_only() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let _exe = compile_and_build(
        "live_onsubmit_record_literal_build_only",
        SKY_ONSUBMIT_RECORD_LITERAL,
    )?;
    Ok(())
}

/// #170 seal — BUILD-ONLY: a TUPLE-literal `onSubmit` payload must compile
/// end-to-end (see [`SKY_ONSUBMIT_TUPLE_LITERAL`]).
///
/// # Errors
///
/// Propagates any pipeline or Cargo build failure as a test error.
#[test]
fn live_onsubmit_tuple_literal_build_only() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let _exe = compile_and_build(
        "live_onsubmit_tuple_literal_build_only",
        SKY_ONSUBMIT_TUPLE_LITERAL,
    )?;
    Ok(())
}

/// #170 seal — BUILD-ONLY: a LIST-literal `onSubmit` payload must compile
/// end-to-end (see [`SKY_ONSUBMIT_LIST_LITERAL`]).
///
/// # Errors
///
/// Propagates any pipeline or Cargo build failure as a test error.
#[test]
fn live_onsubmit_list_literal_build_only() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let _exe = compile_and_build(
        "live_onsubmit_list_literal_build_only",
        SKY_ONSUBMIT_LIST_LITERAL,
    )?;
    Ok(())
}

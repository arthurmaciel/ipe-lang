//! Honest end-to-end tests for `Sky.Http.Server` — `Server.listen`,
//! `Server.get`, `Server.text`, and `Server.param`.
//!
//! All tests are gated on `SKY_E2E=1`.  Without it they return early so the
//! default `cargo test` stays fast.
//!
//! ## Architecture
//!
//! Each test:
//!
//! 1. Writes a Sky program to a fresh temp dir.
//! 2. Compiles it through `skyc::build` (full pipeline: parse → canon → types →
//!    lower → emit Rust).
//! 3. Builds the emitted Cargo project with `oracle::build_rust_binary` — the
//!    global `~/.cargo/config.toml` shared target is used so dependencies
//!    (axum, tokio, tower-http, …) compile once and are reused.
//! 4. Acquires an ephemeral port by binding `TcpListener::bind("127.0.0.1:0")`,
//!    reading the assigned port, then dropping the listener so the Sky server
//!    can bind the same port moments later.
//! 5. Spawns the compiled binary as a child process, passing
//!    `SKY_SERVER_PORT=<port>` and `SKY_HTTP_BIND=127.0.0.1` so the server
//!    binds loopback-only.
//! 6. Polls for server readiness: retries `TcpStream::connect` every 50 ms for
//!    up to 10 s.  The ready signal is the appearance of
//!    `[sky.http.server] listening on` in the child's stderr.
//! 7. Sends raw HTTP/1.1 requests via `TcpStream` (no extra dependencies) and
//!    asserts the response body.
//! 8. A `ProcessGuard` wrapper kills the child process on `Drop`, so the port
//!    is always released even when a test fails.
//!
//! Run:
//!
//! ```text
//! SKY_E2E=1 cargo test server_e2e
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Shared error type for E2E helpers.
type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

// ── Sky programs ──────────────────────────────────────────────────────────────

/// A minimal Sky HTTP server. Reads its port from `SKY_SERVER_PORT`.
///
/// Routes:
/// * `GET /`             → body `hello server`
/// * `GET /greet/:name`  → body `hi <name>` (exercises `Server.param`)
const SKY_SERVER_PROGRAM: &str = r#"module Main exposing (main)

import Sky.Http.Server as Server

main =
    let port = Maybe.withDefault 8080 (String.toInt (System.getenvOr "SKY_SERVER_PORT" "8080"))
    in
    Server.listen port
        [ Server.get "/" (\req -> Task.succeed (Server.text "hello server"))
        , Server.get "/greet/:name" (\req ->
            let name = Maybe.withDefault "world" (Server.param "name" req)
            in
            Task.succeed (Server.text ("hi " ++ name)))
        ]
"#;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Compile a Sky program string, build the emitted Rust project, and return
/// the path to the compiled binary.
///
/// # Errors
///
/// Returns an error on any pipeline or Cargo build failure.
fn compile_and_build(test_name: &str, sky_source: &str) -> Result<PathBuf, BoxError> {
    let sky_dir = std::env::temp_dir().join(format!("server_e2e_{test_name}_sky"));
    let _ = std::fs::remove_dir_all(&sky_dir);
    std::fs::create_dir_all(&sky_dir).map_err(|e| -> BoxError {
        format!("{test_name}: cannot create sky source dir: {e}").into()
    })?;

    let entry = sky_dir.join("Main.sky");
    std::fs::write(&entry, sky_source)
        .map_err(|e| -> BoxError { format!("{test_name}: cannot write Main.sky: {e}").into() })?;

    let out_dir = std::env::temp_dir().join(format!("server_e2e_{test_name}_emitted"));
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
    // Drop `listener` here — releases the port for the Sky server.
    Ok(port)
}

/// RAII guard: kills the wrapped child process on `Drop`.
///
/// This ensures the port is always released and the OS-level child is always
/// reaped, even when a test assertion fails or panics.
struct ProcessGuard(Child);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        // Best-effort: ignore errors — if the process already exited the kill
        // will return an error we discard, and `wait` cleans up the zombie.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawn the Sky server binary and wait until it signals readiness.
///
/// Readiness is detected by watching the child's stderr for the line
/// `[sky.http.server] listening on`.  The `server_listen` runtime emits this
/// line immediately after `tokio::net::TcpListener::bind` succeeds, so
/// receiving it means the socket is open and accepts connections.
///
/// Times out after 10 s; returns an error if the line never appears.
///
/// # Errors
///
/// Returns an error if the binary cannot be spawned or the ready signal does
/// not appear within the timeout.
fn spawn_and_wait_ready(
    test_name: &str,
    exe: &std::path::Path,
    port: u16,
) -> Result<ProcessGuard, BoxError> {
    let mut child = Command::new(exe)
        .env("SKY_SERVER_PORT", port.to_string())
        .env("SKY_HTTP_BIND", "127.0.0.1")
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| -> BoxError {
            format!("{test_name}: cannot spawn server binary: {e}").into()
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
            return Err(format!("{test_name}: server did not signal readiness within 10 s").into());
        }
        line.clear();
        // `read_line` blocks until a newline or EOF.  A short-lived server that
        // exits before printing the ready line will produce EOF, which
        // `read_line` signals as 0 bytes.
        match reader.read_line(&mut line) {
            Ok(0) => {
                // Child exited without printing the ready line.
                let _ = child.wait();
                return Err(
                    format!("{test_name}: server process exited before signalling ready").into(),
                );
            }
            Ok(_) => {
                if line.contains("[sky.http.server] listening on") {
                    // Server is ready.  Detach the reader (it stays alive on
                    // the guard's `stderr` pipe until the guard is dropped).
                    return Ok(ProcessGuard(child));
                }
                // Any other line: keep reading.
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{test_name}: error reading child stderr: {e}").into());
            }
        }
    }
}

/// Sky server that echoes the POST body verbatim.
///
/// Route: `POST /echo` → body `Server.body req`
const SKY_POST_ECHO_PROGRAM: &str = r#"module Main exposing (main)

import Sky.Http.Server as Server

main =
    let port = Maybe.withDefault 8080 (String.toInt (System.getenvOr "SKY_SERVER_PORT" "8080"))
    in
    Server.listen port
        [ Server.post "/echo" (\req -> Task.succeed (Server.text (Server.body req)))
        ]
"#;

/// Sky server that exercises all four request-introspection kernels in one
/// handler: method, path, header, queryParam, and param.
///
/// Route: `POST /introspect/:tag?q=<val>` with header `X-Probe: <val>`
/// → body `<method>|<path>|<probe>|<q>|<tag>`
const SKY_INTROSPECT_PROGRAM: &str = r#"module Main exposing (main)

import Sky.Http.Server as Server

main =
    let port = Maybe.withDefault 8080 (String.toInt (System.getenvOr "SKY_SERVER_PORT" "8080"))
    in
    Server.listen port
        [ Server.post "/introspect/:tag" (\req ->
            Task.succeed (Server.text
                (Server.method req ++ "|" ++ Server.path req
                 ++ "|" ++ Maybe.withDefault "?" (Server.header "x-probe" req)
                 ++ "|" ++ Maybe.withDefault "?" (Server.queryParam "q" req)
                 ++ "|" ++ Maybe.withDefault "?" (Server.param "tag" req))))
        ]
"#;

/// Sky server that uses BOTH `Server.listen` and a `Db.open` call — the
/// minimal program that sets both `uses_server` and `uses_db` in the lowerer,
/// exercising the server+db manifest composition (GAP 1 fix).
///
/// `Db.open : String -> String -> Task Error Db` (driver, url).
/// `Db.connect : () -> Task Error Db` takes unit, not a URL string, so we use
/// `Db.open` here for clarity.
const SKY_SERVER_AND_DB_PROGRAM: &str = r#"module Main exposing (main)

import Sky.Http.Server as Server
import Std.Db as Db

main =
    Task.andThen
        (\conn ->
            let port = Maybe.withDefault 8080 (String.toInt (System.getenvOr "SKY_SERVER_PORT" "8080"))
            in
            Server.listen port
                [ Server.get "/" (\req -> Task.succeed (Server.text "ok"))
                ])
        (Db.open "sqlite" (System.getenvOr "DATABASE_URL" "sqlite::memory:"))
"#;

/// Send a raw HTTP/1.1 GET request on `stream` and return the full response
/// body (everything after the blank-header separator `\r\n\r\n`).
///
/// Reads up to 8 KiB; sufficient for the small test responses.
///
/// # Errors
///
/// Returns an error if the stream write or read fails.
fn http_get(test_name: &str, addr: &str, path: &str) -> Result<String, BoxError> {
    let mut stream = TcpStream::connect(addr).map_err(|e| -> BoxError {
        format!("{test_name}: cannot connect to server: {e}").into()
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| -> BoxError { format!("{test_name}: set_read_timeout failed: {e}").into() })?;

    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| -> BoxError { format!("{test_name}: write failed: {e}").into() })?;

    let mut buf = Vec::with_capacity(4096);
    stream
        .read_to_end(&mut buf)
        .map_err(|e| -> BoxError { format!("{test_name}: read failed: {e}").into() })?;

    let response = String::from_utf8_lossy(&buf).into_owned();
    // Split headers from body on the blank-line separator.
    let body = if let Some(idx) = response.find("\r\n\r\n") {
        response[idx + 4..].to_owned()
    } else {
        // No separator → return the whole response so the assertion fails with
        // a useful diff.
        response
    };
    Ok(body)
}

/// Send a raw HTTP/1.1 POST request with `body` and optional extra `headers`
/// (each `"Name: Value"`) to `addr/path`.  Returns the response body.
///
/// # Errors
///
/// Returns an error if the stream write or read fails.
fn http_post(
    test_name: &str,
    addr: &str,
    path: &str,
    body: &str,
    extra_headers: &[&str],
) -> Result<String, BoxError> {
    let mut stream = TcpStream::connect(addr).map_err(|e| -> BoxError {
        format!("{test_name}: cannot connect to server: {e}").into()
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| -> BoxError { format!("{test_name}: set_read_timeout failed: {e}").into() })?;

    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n",
        body.len()
    );
    for h in extra_headers {
        request.push_str(h);
        request.push_str("\r\n");
    }
    request.push_str("Connection: close\r\n\r\n");
    request.push_str(body);

    stream
        .write_all(request.as_bytes())
        .map_err(|e| -> BoxError { format!("{test_name}: write failed: {e}").into() })?;

    let mut buf = Vec::with_capacity(4096);
    stream
        .read_to_end(&mut buf)
        .map_err(|e| -> BoxError { format!("{test_name}: read failed: {e}").into() })?;

    let response = String::from_utf8_lossy(&buf).into_owned();
    let resp_body = if let Some(idx) = response.find("\r\n\r\n") {
        response[idx + 4..].to_owned()
    } else {
        response
    };
    Ok(resp_body)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// `Server.get "/"` returns the expected body with a 200 status.
///
/// Proves the full M6-server pipeline end-to-end:
/// Sky source → skyc → emitted Rust (with `server` feature injected) →
/// `sky_runtime::server::server_listen` + `server_get` + `server_text` →
/// axum HTTP server → response received by the test.
///
/// # Errors
///
/// Propagates any pipeline, build, spawn, or HTTP error as a test error.
#[test]
fn server_get_root() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let exe = compile_and_build("server_get_root", SKY_SERVER_PROGRAM)?;
    let port = pick_ephemeral_port()?;
    let _guard = spawn_and_wait_ready("server_get_root", &exe, port)?;

    let addr = format!("127.0.0.1:{port}");
    let body = http_get("server_get_root", &addr, "/")?;

    assert_eq!(
        body, "hello server",
        "server_get_root: unexpected response body\n--- actual ---\n{body}"
    );
    Ok(())
}

/// `Server.get "/greet/:name"` with `Server.param` returns the expected body.
///
/// Exercises `server_param` — the `name` path capture is extracted via
/// `Server.param "name" req` and interpolated into the response.  Proves the
/// `Request.params` field is populated by `server_listen`'s axum route
/// extraction and readable via the typed kernel.
///
/// # Errors
///
/// Propagates any pipeline, build, spawn, or HTTP error as a test error.
#[test]
fn server_get_param() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let exe = compile_and_build("server_get_param", SKY_SERVER_PROGRAM)?;
    let port = pick_ephemeral_port()?;
    let _guard = spawn_and_wait_ready("server_get_param", &exe, port)?;

    let addr = format!("127.0.0.1:{port}");
    let body = http_get("server_get_param", &addr, "/greet/alice")?;

    assert_eq!(
        body, "hi alice",
        "server_get_param: unexpected response body\n--- actual ---\n{body}"
    );
    Ok(())
}

/// `Server.body` reads the POST request body and echoes it verbatim.
///
/// Proves the completeness gap (GAP 2) is closed: a POST handler CAN read
/// the request body via `Server.body req`.  Before the fix, there was no
/// `Server.body` kernel so POST bodies were unreadable — a completeness loss.
///
/// Full pipeline: Sky source → skyc (new `ServerBody` kernel) →
/// emitted `server_body(req)` Rust call → axum populates `req.body` from the
/// request → handler echoes it.
///
/// # Errors
///
/// Propagates any pipeline, build, spawn, or HTTP error as a test error.
#[test]
fn post_body_echo() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let exe = compile_and_build("post_body_echo", SKY_POST_ECHO_PROGRAM)?;
    let port = pick_ephemeral_port()?;
    let _guard = spawn_and_wait_ready("post_body_echo", &exe, port)?;

    let addr = format!("127.0.0.1:{port}");
    let body = http_post("post_body_echo", &addr, "/echo", "hello-post-42", &[])?;

    assert_eq!(
        body, "hello-post-42",
        "post_body_echo: response body must echo the POST body exactly\n--- actual ---\n{body}"
    );
    Ok(())
}

/// A single handler reads all five request-introspection dimensions:
/// `Server.method`, `Server.path`, `Server.header`, `Server.queryParam`,
/// and `Server.param` (path capture).
///
/// Proves the `body|path|method` kernel triples wired correctly alongside
/// the existing `header|queryParam|param` accessors.
///
/// Expected response: `POST|/introspect/abc|pv|val|abc`
///
/// # Errors
///
/// Propagates any pipeline, build, spawn, or HTTP error as a test error.
#[test]
fn request_introspection() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    let exe = compile_and_build("request_introspection", SKY_INTROSPECT_PROGRAM)?;
    let port = pick_ephemeral_port()?;
    let _guard = spawn_and_wait_ready("request_introspection", &exe, port)?;

    let addr = format!("127.0.0.1:{port}");
    let body = http_post(
        "request_introspection",
        &addr,
        "/introspect/abc?q=val",
        "",
        &["X-Probe: pv"],
    )?;

    assert_eq!(
        body, "POST|/introspect/abc|pv|val|abc",
        "request_introspection: unexpected response body\n--- actual ---\n{body}"
    );
    Ok(())
}

/// A program that uses BOTH `Server.listen` and `Db.connect` compiles,
/// cargo-builds, and emits a manifest with BOTH `"server"` AND `"db"` in the
/// default feature list.
///
/// This is the GAP 1 regression test: before the fix, `server_cargo_toml`
/// anchored on `"json"]` which is absent when `db_cargo_toml` has already run
/// (the list becomes `"json", "db"]`), causing a `CompilerBug` ICE.  The
/// generic anchor now finds the closing `]` regardless of what precedes it.
///
/// Test is BUILD-ONLY — the compiled binary is not spawned (it would start a
/// server and block).  A successful `cargo build` is the assertion.
///
/// # Errors
///
/// Propagates any pipeline or Cargo build failure as a test error.
#[test]
fn server_and_db_compose() -> Result<(), BoxError> {
    if std::env::var("SKY_E2E").is_err() {
        return Ok(());
    }

    // compile_and_build already does skyc + cargo build; success is the proof.
    let _exe = compile_and_build("server_and_db_compose", SKY_SERVER_AND_DB_PROGRAM)?;
    Ok(())
}

#![forbid(unsafe_code)]
//! Protocol conformance over an in-memory connection: initialize handshake,
//! live diagnostics on `didOpen`, convergence to clean on `didChange` — no
//! filesystem, no subprocess (the fixture loader resolves a virtual
//! project).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::{Notification as _, PublishDiagnostics};

use ipe_lsp_server::{LoadError, LoadedFile, LoadedProject, ModuleOrigin, ProjectLoader};

const VIRTUAL_PATH: &str = "/ipe-lsp-protocol-test/Main.ipe";
const CLEAN: &str = "module Main exposing (main)\n\nimport Ipe.Io as Io\nimport Ipe.String as String\n\nmain : Task Error ()\nmain =\n    Io.println (String.fromInt 1)\n";
const TYPE_ERROR: &str = "module Main exposing (main)\n\nmain : Int\nmain = \"nope\"\n";

/// Resolves a single-module virtual project from the open buffer — the
/// filesystem never participates.
struct FixtureLoader;

impl ProjectLoader for FixtureLoader {
    fn load(
        &self,
        _workspace_root: Option<&Path>,
        open_file: &Path,
        open_text: Option<&str>,
    ) -> Result<LoadedProject, LoadError> {
        let mut files = BTreeMap::new();
        files.insert(
            vec!["Main".to_owned()],
            LoadedFile {
                path: open_file.to_path_buf(),
                text: open_text.unwrap_or(CLEAN).to_owned(),
                origin: ModuleOrigin::User,
            },
        );
        Ok(LoadedProject {
            files,
            entry_module: vec!["Main".to_owned()],
        })
    }
}

#[allow(clippy::expect_used)] // test helper: silence past the deadline IS the failure
fn recv_response(client: &Connection, id: i32) -> serde_json::Value {
    let deadline = Duration::from_secs(30);
    loop {
        let msg = client
            .receiver
            .recv_timeout(deadline)
            .expect("server reply within deadline");
        if let Message::Response(response) = msg
            && response.id == RequestId::from(id)
        {
            assert!(
                response.error.is_none(),
                "error reply: {:?}",
                response.error
            );
            return response.result.unwrap_or(serde_json::Value::Null);
        }
    }
}

/// Wait for a `publishDiagnostics` for `uri` whose payload satisfies
/// `accept`; intermediate pushes for other states are skipped.
#[allow(clippy::expect_used)] // test helper: silence past the deadline IS the failure
fn await_diagnostics(
    client: &Connection,
    uri: &str,
    accept: impl Fn(&[serde_json::Value]) -> bool,
) -> Vec<serde_json::Value> {
    let deadline = Duration::from_secs(30);
    loop {
        let msg = client
            .receiver
            .recv_timeout(deadline)
            .expect("diagnostics push within deadline");
        let Message::Notification(note) = msg else {
            continue;
        };
        if note.method != PublishDiagnostics::METHOD {
            continue;
        }
        let params = note.params;
        if params.get("uri").and_then(serde_json::Value::as_str) != Some(uri) {
            continue;
        }
        let diags: Vec<serde_json::Value> = params
            .get("diagnostics")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        if accept(&diags) {
            return diags;
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)] // one linear protocol script, deliberately unsplit
fn did_open_publishes_compiler_diagnostics_and_did_change_clears_them() {
    let (server_side, client) = Connection::memory();
    let server = std::thread::spawn(move || {
        ipe_lsp_server::run_with_connection(&server_side, &FixtureLoader)
    });

    // initialize / initialized.
    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(1),
            "initialize".to_owned(),
            serde_json::json!({ "capabilities": {} }),
        )))
        .expect("send initialize");
    let init_result = recv_response(&client, 1);
    assert!(
        init_result
            .pointer("/capabilities/textDocumentSync")
            .is_some_and(serde_json::Value::is_object),
        "sync capability advertised: {init_result}"
    );
    assert_eq!(
        init_result.pointer("/capabilities/positionEncoding"),
        Some(&serde_json::json!("utf-16")),
        "utf-16 is the default when the client offers nothing"
    );
    client
        .sender
        .send(Message::Notification(Notification::new(
            "initialized".to_owned(),
            serde_json::json!({}),
        )))
        .expect("send initialized");

    // didOpen with a type error → a real compiler diagnostic arrives.
    let uri = format!("file://{VIRTUAL_PATH}");
    client
        .sender
        .send(Message::Notification(Notification::new(
            "textDocument/didOpen".to_owned(),
            serde_json::json!({
                "textDocument": {
                    "uri": uri, "languageId": "ipe", "version": 1, "text": TYPE_ERROR
                }
            }),
        )))
        .expect("send didOpen");
    let diags = await_diagnostics(&client, &uri, |diags| !diags.is_empty());
    assert_eq!(diags.len(), 1, "{diags:?}");
    let diag = diags.first().expect("one diagnostic");
    assert_eq!(diag.get("code"), Some(&serde_json::json!("IPE-T0001")));
    assert_eq!(diag.get("source"), Some(&serde_json::json!("ipe")));
    assert_eq!(
        diag.pointer("/range/start/line"),
        Some(&serde_json::json!(3)),
        "the range points at the failing expression: {diag}"
    );
    assert!(
        diag.get("message")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|m| m.contains("type mismatch")),
        "{diag}"
    );

    // didChange (full-text replacement) fixes the buffer → cleared.
    client
        .sender
        .send(Message::Notification(Notification::new(
            "textDocument/didChange".to_owned(),
            serde_json::json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{ "text": CLEAN }]
            }),
        )))
        .expect("send didChange");
    let cleared = await_diagnostics(&client, &uri, <[serde_json::Value]>::is_empty);
    assert!(cleared.is_empty());

    // Hover over the `1` in `String.fromInt 1` (line 7, character 31) → `Int`.
    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(3),
            "textDocument/hover".to_owned(),
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": 7, "character": 31 }
            }),
        )))
        .expect("send hover");
    let hover = recv_response(&client, 3);
    assert_eq!(
        hover.pointer("/contents/value").and_then(|v| v.as_str()),
        Some("Int"),
        "{hover}"
    );

    // Document symbols → the one top-level binding `main`.
    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(4),
            "textDocument/documentSymbol".to_owned(),
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )))
        .expect("send documentSymbol");
    let symbols = recv_response(&client, 4);
    let names: Vec<&str> = symbols
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|s| s.get("name").and_then(|n| n.as_str()))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(names, vec!["main"], "{symbols}");

    // An incremental (ranged) edit re-introduces the error: replace `1`
    // on line 7 (`String.fromInt 1`) with a string literal.
    client
        .sender
        .send(Message::Notification(Notification::new(
            "textDocument/didChange".to_owned(),
            serde_json::json!({
                "textDocument": { "uri": uri, "version": 3 },
                "contentChanges": [{
                    "range": {
                        "start": { "line": 7, "character": 31 },
                        "end": { "line": 7, "character": 32 }
                    },
                    "text": "\"nope\""
                }]
            }),
        )))
        .expect("send ranged didChange");
    let diags = await_diagnostics(&client, &uri, |diags| !diags.is_empty());
    assert_eq!(
        diags.first().and_then(|d| d.get("code")),
        Some(&serde_json::json!("IPE-T0001"))
    );

    // shutdown / exit — the loop terminates cleanly.
    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(2),
            "shutdown".to_owned(),
            serde_json::Value::Null,
        )))
        .expect("send shutdown");
    let _ = recv_response(&client, 2);
    client
        .sender
        .send(Message::Notification(Notification::new(
            "exit".to_owned(),
            serde_json::Value::Null,
        )))
        .expect("send exit");
    server
        .join()
        .expect("server thread joins")
        .expect("server exits clean");
}

#[test]
fn unknown_request_gets_method_not_found_not_a_hang() {
    let (server_side, client) = Connection::memory();
    let server = std::thread::spawn(move || {
        ipe_lsp_server::run_with_connection(&server_side, &FixtureLoader)
    });

    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(1),
            "initialize".to_owned(),
            serde_json::json!({ "capabilities": {} }),
        )))
        .expect("send initialize");
    let _ = recv_response(&client, 1);
    client
        .sender
        .send(Message::Notification(Notification::new(
            "initialized".to_owned(),
            serde_json::json!({}),
        )))
        .expect("send initialized");

    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(7),
            "textDocument/unknownIpeMethod".to_owned(),
            serde_json::json!({}),
        )))
        .expect("send unknown method");
    let deadline = Duration::from_secs(30);
    let err = loop {
        let msg = client
            .receiver
            .recv_timeout(deadline)
            .expect("reply within deadline");
        if let Message::Response(response) = msg
            && response.id == RequestId::from(7)
        {
            break response.error.expect("unimplemented method must error");
        }
    };
    assert_eq!(err.code, lsp_server::ErrorCode::MethodNotFound as i32);

    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(2),
            "shutdown".to_owned(),
            serde_json::Value::Null,
        )))
        .expect("send shutdown");
    let _ = recv_response(&client, 2);
    client
        .sender
        .send(Message::Notification(Notification::new(
            "exit".to_owned(),
            serde_json::Value::Null,
        )))
        .expect("send exit");
    server
        .join()
        .expect("server thread joins")
        .expect("server exits clean");
}

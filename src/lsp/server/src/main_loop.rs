//! The single-writer main loop: document sync in receipt order on this
//! thread, diagnostics on cancellable worker threads, latest-generation-wins
//! publishing.

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::thread;

use crossbeam_channel::{Receiver, Sender, select};
use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Notification as _, PublishDiagnostics,
};
use lsp_types::{InitializeParams, PublishDiagnosticsParams, TextDocumentContentChangeEvent, Url};

use ipe_lsp_features::{PositionEncoding, diagnostics, offset};

use crate::ServerError;
use crate::loader::{LoadedFile, LoadedProject, ModuleOrigin, ProjectLoader};

/// One finished diagnostics computation, tagged with the input generation it
/// was computed against so a stale batch is recognisably droppable.
struct DiagnosticsBatch {
    generation: u64,
    per_uri: Vec<(Url, Vec<lsp_types::Diagnostic>)>,
}

struct State {
    workspace_root: Option<PathBuf>,
    encoding: PositionEncoding,
    db: ipe_db::IpeDatabase,
    root: Option<ipe_db::SourceRoot>,
    entry_module: Vec<String>,
    /// The last loaded project layout (disk truth; overlays shadow it).
    disk: BTreeMap<Vec<String>, LoadedFile>,
    /// Open editor buffers, keyed by normalized path.
    overlays: BTreeMap<PathBuf, String>,
    module_of_path: BTreeMap<PathBuf, Vec<String>>,
    /// Whether the current layout is the degraded single-file fallback
    /// (project resolution failed — retry a full load on the next edit).
    fallback: bool,
    generation: u64,
    /// Last non-empty payload per URI, for change-suppression and clearing.
    last_published: BTreeMap<Url, Vec<lsp_types::Diagnostic>>,
}

impl State {
    /// The document URI of a user module, when it has an on-disk path.
    fn uri_for_module(&self, module: &[String]) -> Option<Url> {
        self.module_of_path
            .iter()
            .find(|(_, m)| m.as_slice() == module)
            .and_then(|(path, _)| Url::from_file_path(path).ok())
    }

    /// Resolve a document URI to its module path, input handle, and current
    /// input text.
    fn locate(&self, uri: &Url) -> Option<(Vec<String>, ipe_db::SourceFile, String)> {
        let path = normalize(&uri.to_file_path().ok()?);
        let module = self.module_of_path.get(&path)?.clone();
        let root = self.root?;
        let file = root.files(&self.db).get(&module).copied()?;
        let text = file.text(&self.db).clone();
        Some((module, file, text))
    }

    fn new(workspace_root: Option<PathBuf>, encoding: PositionEncoding) -> Self {
        Self {
            workspace_root,
            encoding,
            db: ipe_db::IpeDatabase::new(),
            root: None,
            entry_module: Vec::new(),
            disk: BTreeMap::new(),
            overlays: BTreeMap::new(),
            module_of_path: BTreeMap::new(),
            fallback: false,
            generation: 0,
            last_published: BTreeMap::new(),
        }
    }
}

pub fn run(
    connection: &Connection,
    init: &InitializeParams,
    encoding: PositionEncoding,
    loader: &dyn ProjectLoader,
) -> Result<(), ServerError> {
    let (diag_tx, diag_rx): (Sender<DiagnosticsBatch>, Receiver<DiagnosticsBatch>) =
        crossbeam_channel::unbounded();
    let mut state = State::new(workspace_root_of(init), encoding);

    loop {
        select! {
            recv(connection.receiver) -> msg => {
                let Ok(msg) = msg else { break };
                match msg {
                    Message::Request(request) => {
                        match connection.handle_shutdown(&request) {
                            Ok(true) => break,
                            Ok(false) => handle_request(&state, connection, request),
                            Err(err) => return Err(ServerError::new(err)),
                        }
                    }
                    Message::Notification(notification) => {
                        handle_notification(&mut state, loader, &notification, &diag_tx);
                    }
                    Message::Response(_) => {}
                }
            }
            recv(diag_rx) -> batch => {
                if let Ok(batch) = batch {
                    publish(&mut state, connection, batch);
                }
            }
        }
    }
    Ok(())
}

fn workspace_root_of(init: &InitializeParams) -> Option<PathBuf> {
    if let Some(folder) = init
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        && let Ok(path) = folder.uri.to_file_path()
    {
        return Some(path);
    }
    #[allow(deprecated)] // `root_uri` is the fallback older clients still send.
    init.root_uri
        .as_ref()
        .and_then(|uri| uri.to_file_path().ok())
}

fn handle_request(state: &State, connection: &Connection, request: Request) {
    let result: Option<serde_json::Value> = match request.method.as_str() {
        "textDocument/hover" => Some(hover_result(state, &request.params)),
        "textDocument/documentSymbol" => Some(document_symbols_result(state, &request.params)),
        "textDocument/documentLink" => Some(document_links_result(state, &request.params)),
        "textDocument/foldingRange" => Some(folding_ranges_result(state, &request.params)),
        "textDocument/completion" => Some(completion_result(state, &request.params)),
        "textDocument/definition" => Some(definition_result(state, &request.params)),
        "textDocument/references" => Some(references_result(state, &request.params)),
        "textDocument/prepareRename" => Some(prepare_rename_result(state, &request.params)),
        "textDocument/rename" => Some(rename_result(state, &request.params)),
        "textDocument/formatting" => Some(formatting_result(state, &request.params)),
        "textDocument/rangeFormatting" => Some(range_formatting_result(state, &request.params)),
        "textDocument/codeAction" => Some(code_action_result(state, &request.params)),
        "textDocument/semanticTokens/full" => {
            Some(semantic_tokens_full_result(state, &request.params))
        }
        "textDocument/signatureHelp" => Some(signature_help_result(state, &request.params)),
        "textDocument/inlayHint" => Some(inlay_hints_result(state, &request.params)),
        _ => None,
    };
    let response = match result {
        Some(value) => Response::new_ok(request.id, value),
        None => Response::new_err(
            request.id,
            lsp_server::ErrorCode::MethodNotFound as i32,
            format!("ipe-lsp does not handle `{}` yet", request.method),
        ),
    };
    let _ = connection.sender.send(Message::Response(response));
}

/// `textDocument/hover` — the solved type of the innermost expression at the
/// cursor. `null` for an unknown document, an unsolvable program, or a
/// position on no expression (never a guess).
fn hover_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) = serde_json::from_value::<lsp_types::HoverParams>(params.clone()) else {
        return serde_json::Value::Null;
    };
    let position = params.text_document_position_params;
    let Some((_module, file, text)) = state.locate(&position.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let Some(root) = state.root else {
        return serde_json::Value::Null;
    };
    let Some(entry_file) = root.files(&state.db).get(&state.entry_module).copied() else {
        return serde_json::Value::Null;
    };
    let byte = offset::position_to_offset(&text, position.position, state.encoding);
    let byte = u32::try_from(byte).unwrap_or(u32::MAX);
    ipe_lsp_features::hover::hover(&state.db, root, entry_file, file, byte).map_or(
        serde_json::Value::Null,
        |info| {
            let hover = lsp_types::Hover {
                contents: lsp_types::HoverContents::Scalar(
                    lsp_types::MarkedString::LanguageString(lsp_types::LanguageString {
                        language: "ipe".to_owned(),
                        value: info.ty,
                    }),
                ),
                range: Some(ipe_lsp_features::offset::span_to_range(
                    &text,
                    info.span,
                    state.encoding,
                )),
            };
            serde_json::to_value(hover).unwrap_or(serde_json::Value::Null)
        },
    )
}

/// `textDocument/documentLink` — every resolved `import` as a link to the
/// imported module's file.
fn document_links_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) = serde_json::from_value::<lsp_types::DocumentLinkParams>(params.clone()) else {
        return serde_json::Value::Null;
    };
    let Some((_module, file, text)) = state.locate(&params.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let Some(root) = state.root else {
        return serde_json::Value::Null;
    };
    let links: Vec<lsp_types::DocumentLink> =
        ipe_lsp_features::links::document_links(&state.db, root, file)
            .into_iter()
            .filter_map(|link| {
                let target = state.uri_for_module(&link.target_module)?;
                Some(lsp_types::DocumentLink {
                    range: ipe_lsp_features::offset::span_to_range(
                        &text,
                        link.span,
                        state.encoding,
                    ),
                    target: Some(target),
                    tooltip: None,
                    data: None,
                })
            })
            .collect();
    serde_json::to_value(links).unwrap_or(serde_json::Value::Null)
}

/// `textDocument/foldingRange` — the import block plus every multi-line
/// top-level declaration.
fn folding_ranges_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) = serde_json::from_value::<lsp_types::FoldingRangeParams>(params.clone()) else {
        return serde_json::Value::Null;
    };
    let Some((_module, file, _text)) = state.locate(&params.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let ranges = ipe_lsp_features::folding::folding_ranges(&state.db, file, state.encoding);
    serde_json::to_value(ranges).unwrap_or(serde_json::Value::Null)
}

/// `textDocument/documentSymbol` — the parse tree's hierarchical outline.
fn document_symbols_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) = serde_json::from_value::<lsp_types::DocumentSymbolParams>(params.clone())
    else {
        return serde_json::Value::Null;
    };
    let Some((_module, file, _text)) = state.locate(&params.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let symbols = ipe_lsp_features::symbols::document_symbols(&state.db, file, state.encoding);
    serde_json::to_value(lsp_types::DocumentSymbolResponse::Nested(symbols))
        .unwrap_or(serde_json::Value::Null)
}

/// `textDocument/completion` — in-scope identifiers at the cursor position.
fn completion_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) = serde_json::from_value::<lsp_types::CompletionParams>(params.clone()) else {
        return serde_json::Value::Null;
    };
    let position = params.text_document_position;
    let Some((module, _file, text)) = state.locate(&position.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let Some(root) = state.root else {
        return serde_json::Value::Null;
    };
    let Some(entry_file) = root.files(&state.db).get(&state.entry_module).copied() else {
        return serde_json::Value::Null;
    };
    // Convert the UTF-16 cursor position to a byte offset so completion can read
    // the type the surrounding context expects there (type-directed ranking).
    let byte = offset::position_to_offset(&text, position.position, state.encoding);
    let byte = u32::try_from(byte).unwrap_or(u32::MAX);
    let items =
        ipe_lsp_features::completion::completions(&state.db, root, entry_file, &module, byte);
    serde_json::to_value(items).unwrap_or(serde_json::Value::Null)
}

/// `textDocument/definition` — jump to the defining site of the name under
/// the cursor.
fn definition_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) = serde_json::from_value::<lsp_types::GotoDefinitionParams>(params.clone())
    else {
        return serde_json::Value::Null;
    };
    let position = params.text_document_position_params;
    let Some((module, _file, text)) = state.locate(&position.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let Some(root) = state.root else {
        return serde_json::Value::Null;
    };
    let Some(entry_file) = root.files(&state.db).get(&state.entry_module).copied() else {
        return serde_json::Value::Null;
    };
    let byte = offset::position_to_offset(&text, position.position, state.encoding);
    let byte = u32::try_from(byte).unwrap_or(u32::MAX);
    let Some(def) =
        ipe_lsp_features::navigation::goto_definition(&state.db, root, entry_file, &module, byte)
    else {
        return serde_json::Value::Null;
    };
    let Some(def_uri) = state.uri_for_module(&def.module) else {
        return serde_json::Value::Null;
    };
    // Fetch the target text to convert the byte span to a range.
    let def_text: String = root
        .files(&state.db)
        .get(&def.module)
        .map(|f| f.text(&state.db).clone())
        .unwrap_or_default();
    let range = ipe_lsp_features::offset::span_to_range(&def_text, def.span, state.encoding);
    let location = lsp_types::Location {
        uri: def_uri,
        range,
    };
    serde_json::to_value(location).unwrap_or(serde_json::Value::Null)
}

/// `textDocument/references` — every use site of the name under the cursor.
fn references_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) = serde_json::from_value::<lsp_types::ReferenceParams>(params.clone()) else {
        return serde_json::Value::Null;
    };
    let position = params.text_document_position;
    let Some((module, _file, text)) = state.locate(&position.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let Some(root) = state.root else {
        return serde_json::Value::Null;
    };
    let Some(entry_file) = root.files(&state.db).get(&state.entry_module).copied() else {
        return serde_json::Value::Null;
    };
    let byte = offset::position_to_offset(&text, position.position, state.encoding);
    let byte = u32::try_from(byte).unwrap_or(u32::MAX);
    // Resolve via goto_definition to get the canonical (home, name) pair.
    let Some(def) =
        ipe_lsp_features::navigation::goto_definition(&state.db, root, entry_file, &module, byte)
    else {
        return serde_json::Value::Null;
    };
    let def_text = root
        .files(&state.db)
        .get(&def.module)
        .map(|f| f.text(&state.db).clone())
        .unwrap_or_default();
    let lo = def.span.lo as usize;
    let hi = def.span.hi as usize;
    let Some(def_name) = def_text.get(lo..hi) else {
        return serde_json::Value::Null;
    };
    let refs = ipe_lsp_features::navigation::find_references(
        &state.db,
        root,
        entry_file,
        &def.module,
        def_name,
    );
    let mut locations: Vec<lsp_types::Location> = Vec::new();
    // Include definition if requested.
    if params.context.include_declaration
        && let Some(def_uri) = state.uri_for_module(&def.module)
    {
        let range = ipe_lsp_features::offset::span_to_range(&def_text, def.span, state.encoding);
        locations.push(lsp_types::Location {
            uri: def_uri,
            range,
        });
    }
    for r in refs {
        let Some(ref_uri) = state.uri_for_module(&r.module) else {
            continue;
        };
        let ref_text = root
            .files(&state.db)
            .get(&r.module)
            .map(|f| f.text(&state.db).clone())
            .unwrap_or_default();
        let range = ipe_lsp_features::offset::span_to_range(&ref_text, r.span, state.encoding);
        locations.push(lsp_types::Location {
            uri: ref_uri,
            range,
        });
    }
    serde_json::to_value(locations).unwrap_or(serde_json::Value::Null)
}

/// `textDocument/prepareRename` — validate the position is renameable and
/// return the current identifier and its range.
fn prepare_rename_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) =
        serde_json::from_value::<lsp_types::TextDocumentPositionParams>(params.clone())
    else {
        return serde_json::Value::Null;
    };
    let Some((module, _file, text)) = state.locate(&params.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let Some(root) = state.root else {
        return serde_json::Value::Null;
    };
    let Some(entry_file) = root.files(&state.db).get(&state.entry_module).copied() else {
        return serde_json::Value::Null;
    };
    let byte = offset::position_to_offset(&text, params.position, state.encoding);
    let byte = u32::try_from(byte).unwrap_or(u32::MAX);
    let Some(prep) =
        ipe_lsp_features::rename::prepare_rename(&state.db, root, entry_file, &module, byte)
    else {
        return serde_json::Value::Null;
    };
    let range = ipe_lsp_features::offset::span_to_range(&text, prep.span, state.encoding);
    // Return `{ range, placeholder }` — the standard `PrepareRenameResponse`.
    let response = lsp_types::PrepareRenameResponse::RangeWithPlaceholder {
        range,
        placeholder: prep.name,
    };
    serde_json::to_value(response).unwrap_or(serde_json::Value::Null)
}

/// `textDocument/rename` — apply a rename across all references.
fn rename_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) = serde_json::from_value::<lsp_types::RenameParams>(params.clone()) else {
        return serde_json::Value::Null;
    };
    let position = params.text_document_position;
    let Some((module, _file, text)) = state.locate(&position.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let Some(root) = state.root else {
        return serde_json::Value::Null;
    };
    let Some(entry_file) = root.files(&state.db).get(&state.entry_module).copied() else {
        return serde_json::Value::Null;
    };
    let byte = offset::position_to_offset(&text, position.position, state.encoding);
    let byte = u32::try_from(byte).unwrap_or(u32::MAX);
    let encoding = state.encoding;
    let db = &state.db;
    let uri_of = |m: &[String]| state.uri_for_module(m);
    let text_of =
        |m: &[String]| -> Option<String> { root.files(db).get(m).map(|f| f.text(db).clone()) };
    let req = ipe_lsp_features::rename::RenameRequest {
        byte,
        new_name: &params.new_name,
        encoding,
    };
    let resolver = ipe_lsp_features::rename::ModuleResolver {
        uri_of_module: &uri_of,
        text_of_module: &text_of,
    };
    let Some(ws_edit) =
        ipe_lsp_features::rename::rename(db, root, entry_file, &module, &req, &resolver)
    else {
        return serde_json::Value::Null;
    };
    serde_json::to_value(ws_edit).unwrap_or(serde_json::Value::Null)
}

/// Normalize a path for map keys: canonical when the file exists, verbatim
/// otherwise (fixture loaders use paths that exist nowhere).
fn normalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn handle_notification(
    state: &mut State,
    loader: &dyn ProjectLoader,
    notification: &Notification,
    diag_tx: &Sender<DiagnosticsBatch>,
) {
    match notification.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let Ok(params) = serde_json::from_value::<lsp_types::DidOpenTextDocumentParams>(
                notification.params.clone(),
            ) else {
                return;
            };
            let Ok(path) = params.text_document.uri.to_file_path() else {
                return;
            };
            if path.extension().and_then(|e| e.to_str()) != Some("ipe") {
                return;
            }
            let path = normalize(&path);
            state
                .overlays
                .insert(path.clone(), params.text_document.text);
            ensure_project(state, loader, &path);
            sync_inputs(state);
            recompute(state, diag_tx);
        }
        DidChangeTextDocument::METHOD => {
            let Ok(params) = serde_json::from_value::<lsp_types::DidChangeTextDocumentParams>(
                notification.params.clone(),
            ) else {
                return;
            };
            let Ok(path) = params.text_document.uri.to_file_path() else {
                return;
            };
            let path = normalize(&path);
            let encoding = state.encoding;
            let Some(text) = state.overlays.get_mut(&path) else {
                return;
            };
            for change in &params.content_changes {
                apply_content_change(text, change, encoding);
            }
            if state.fallback {
                // Project resolution failed at open; the edit may have fixed
                // the very defect (e.g. the module header) that blocked it.
                ensure_project_fresh(state, loader, &path);
            }
            sync_inputs(state);
            recompute(state, diag_tx);
        }
        DidSaveTextDocument::METHOD => {
            let Ok(params) = serde_json::from_value::<lsp_types::DidSaveTextDocumentParams>(
                notification.params.clone(),
            ) else {
                return;
            };
            let Ok(path) = params.text_document.uri.to_file_path() else {
                return;
            };
            // Re-resolve the layout from disk (files may have been added,
            // renamed, or removed since the last load).
            ensure_project_fresh(state, loader, &normalize(&path));
            sync_inputs(state);
            recompute(state, diag_tx);
        }
        DidCloseTextDocument::METHOD => {
            let Ok(params) = serde_json::from_value::<lsp_types::DidCloseTextDocumentParams>(
                notification.params.clone(),
            ) else {
                return;
            };
            let Ok(path) = params.text_document.uri.to_file_path() else {
                return;
            };
            let path = normalize(&path);
            state.overlays.remove(&path);
            // The closed buffer reverts to disk truth.
            ensure_project_fresh(state, loader, &path);
            sync_inputs(state);
            recompute(state, diag_tx);
        }
        DidChangeWatchedFiles::METHOD => {
            let anchor = state.module_of_path.keys().next().cloned();
            if let Some(anchor) = anchor {
                ensure_project_fresh(state, loader, &anchor);
                sync_inputs(state);
                recompute(state, diag_tx);
            }
        }
        _ => {}
    }
}

/// Load the project for `path` if the current layout does not know it.
fn ensure_project(state: &mut State, loader: &dyn ProjectLoader, path: &Path) {
    if state.module_of_path.contains_key(path) && !state.fallback {
        return;
    }
    ensure_project_fresh(state, loader, path);
}

/// Unconditionally re-resolve the project layout anchored at `path`.
fn ensure_project_fresh(state: &mut State, loader: &dyn ProjectLoader, path: &Path) {
    let open_text = state.overlays.get(path).map(String::as_str);
    match loader.load(state.workspace_root.as_deref(), path, open_text) {
        Ok(project) => {
            adopt(state, project);
            state.fallback = false;
        }
        Err(err) => {
            eprintln!("[ipe lsp] project load failed: {}", err.detail);
            if state.disk.is_empty() {
                // Never successfully loaded: degrade to a single-file layout
                // so parse diagnostics still flow for the open buffer;
                // retried on the next edit.
                if let Some(text) = state.overlays.get(path).cloned() {
                    let module = vec![module_name_fallback(path)];
                    let mut files = BTreeMap::new();
                    files.insert(
                        module.clone(),
                        LoadedFile {
                            path: path.to_path_buf(),
                            text,
                            origin: ModuleOrigin::User,
                        },
                    );
                    adopt(
                        state,
                        LoadedProject {
                            files,
                            entry_module: module,
                        },
                    );
                    state.fallback = true;
                }
            }
            // A previously-good layout exists: keep it rather than degrading
            // to the single-file fallback, which would drop every other
            // module from the salsa root and clear their real diagnostics
            // on the next publish. `DidSaveTextDocument` and
            // `DidChangeWatchedFiles` retry unconditionally, so the next
            // settled state re-resolves the full project.
        }
    }
}

fn module_name_fallback(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Main")
        .to_owned()
}

fn adopt(state: &mut State, project: LoadedProject) {
    state.entry_module = project.entry_module;
    state.module_of_path = project
        .files
        .iter()
        .filter(|(_, file)| file.origin == ModuleOrigin::User)
        .map(|(module, file)| (normalize(&file.path), module.clone()))
        .collect();
    state.disk = project.files;
}

/// Reconcile the salsa inputs with the current layout, open-buffer overlays
/// shadowing disk text.
fn sync_inputs(state: &mut State) {
    if state.disk.is_empty() {
        return;
    }
    let desired: BTreeMap<Vec<String>, (String, ModuleOrigin)> = state
        .disk
        .iter()
        .map(|(module, file)| {
            let text = state
                .overlays
                .get(&normalize(&file.path))
                .unwrap_or(&file.text)
                .clone();
            (module.clone(), (text, file.origin))
        })
        .collect();
    if let Some(root) = state.root {
        // Blocks until any in-flight worker's cancelled query unwinds and
        // drops its database clone — the cancellation edge.
        ipe_db::sync_source_root(&mut state.db, root, &desired);
    } else {
        let files: BTreeMap<Vec<String>, ipe_db::SourceFile> = desired
            .iter()
            .map(|(module, (text, origin))| {
                (
                    module.clone(),
                    ipe_db::SourceFile::new(&state.db, module.clone(), text.clone(), *origin),
                )
            })
            .collect();
        state.root = Some(ipe_db::SourceRoot::new(&state.db, files));
    }
}

fn apply_content_change(
    text: &mut String,
    change: &TextDocumentContentChangeEvent,
    encoding: PositionEncoding,
) {
    match change.range {
        None => {
            text.clone_from(&change.text);
        }
        Some(range) => {
            let start = offset::position_to_offset(text, range.start, encoding);
            let end = offset::position_to_offset(text, range.end, encoding).max(start);
            text.replace_range(start..end, &change.text);
        }
    }
}

/// Spawn a diagnostics worker against the current inputs. A later edit
/// cancels it (salsa `Cancelled` unwinds out of the query demand); only a
/// batch matching the loop's current generation is published.
fn recompute(state: &mut State, diag_tx: &Sender<DiagnosticsBatch>) {
    let Some(root) = state.root else { return };
    let Some(entry_file) = root.files(&state.db).get(&state.entry_module).copied() else {
        return;
    };
    state.generation = state.generation.wrapping_add(1);
    let generation = state.generation;
    let db = state.db.clone();
    let encoding = state.encoding;
    let entry_module = state.entry_module.clone();
    let mut uri_of: BTreeMap<Vec<String>, Url> = BTreeMap::new();
    for (path, module) in &state.module_of_path {
        if let Ok(uri) = Url::from_file_path(path) {
            uri_of.insert(module.clone(), uri);
        }
    }
    let tx = diag_tx.clone();
    thread::spawn(move || {
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            salsa::Cancelled::catch(AssertUnwindSafe(|| {
                compute_batch(&db, root, entry_file, &uri_of, &entry_module, encoding)
            }))
        }));
        match outcome {
            Ok(Ok(per_uri)) => {
                let _ = tx.send(DiagnosticsBatch {
                    generation,
                    per_uri,
                });
            }
            Ok(Err(_cancelled)) => {} // superseded — the newer worker owns the push
            Err(_panic) => {
                eprintln!("[ipe lsp] internal error: diagnostics worker panicked");
            }
        }
    });
}

/// Pure worker body: collect, attribute, and map diagnostics to URIs.
/// A diagnostic owned by a module with no URI (injected stdlib) is
/// re-attributed to the entry document rather than dropped.
fn compute_batch(
    db: &ipe_db::IpeDatabase,
    root: ipe_db::SourceRoot,
    entry_file: ipe_db::SourceFile,
    uri_of: &BTreeMap<Vec<String>, Url>,
    entry_module: &[String],
    encoding: PositionEncoding,
) -> Vec<(Url, Vec<lsp_types::Diagnostic>)> {
    let collected = diagnostics::collect(db, root, entry_file);
    let files = root.files(db);
    let entry_uri = uri_of.get(entry_module).cloned();
    let mut per_uri: BTreeMap<Url, Vec<lsp_types::Diagnostic>> = uri_of
        .values()
        .map(|uri| (uri.clone(), Vec::new()))
        .collect();
    for module_diags in collected {
        if module_diags.diagnostics.is_empty() {
            continue;
        }
        let direct_uri = uri_of.get(&module_diags.module).cloned();
        let re_attributed = direct_uri.is_none();
        let Some(uri) = direct_uri.or_else(|| entry_uri.clone()) else {
            continue;
        };
        let text: String = files
            .get(&module_diags.module)
            .map(|file| file.text(db).clone())
            .unwrap_or_default();
        for diag in &module_diags.diagnostics {
            let mut lsp = diagnostics::to_lsp(diag, &text, encoding);
            if re_attributed {
                lsp.range = lsp_types::Range::default();
                lsp.message = format!(
                    "in module {}: {}",
                    module_diags.module.join("."),
                    lsp.message
                );
            }
            per_uri.entry(uri.clone()).or_default().push(lsp);
        }
    }
    per_uri.into_iter().collect()
}

/// `textDocument/formatting` — reformat the whole document.
fn formatting_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) = serde_json::from_value::<lsp_types::DocumentFormattingParams>(params.clone())
    else {
        return serde_json::Value::Null;
    };
    let Some((_module, file, _text)) = state.locate(&params.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let edits = ipe_lsp_features::formatting::format_document(&state.db, file, state.encoding);
    serde_json::to_value(edits).unwrap_or(serde_json::Value::Null)
}

/// `textDocument/rangeFormatting` — reformat a selected range.
fn range_formatting_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) =
        serde_json::from_value::<lsp_types::DocumentRangeFormattingParams>(params.clone())
    else {
        return serde_json::Value::Null;
    };
    let Some((_module, file, _text)) = state.locate(&params.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let edits =
        ipe_lsp_features::formatting::format_range(&state.db, file, params.range, state.encoding);
    serde_json::to_value(edits).unwrap_or(serde_json::Value::Null)
}

/// `textDocument/codeAction` — diagnostic-driven quick-fixes.
fn code_action_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) = serde_json::from_value::<lsp_types::CodeActionParams>(params.clone()) else {
        return serde_json::Value::Null;
    };
    let Some((module, _file, text)) = state.locate(&params.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let Some(root) = state.root else {
        return serde_json::Value::Null;
    };
    let Some(entry_file) = root.files(&state.db).get(&state.entry_module).copied() else {
        return serde_json::Value::Null;
    };
    let actions = ipe_lsp_features::code_actions::code_actions(
        ipe_lsp_features::code_actions::DbView {
            db: &state.db,
            root,
            entry: entry_file,
        },
        &module,
        &params.text_document.uri,
        params.range,
        &params.context.diagnostics,
        &text,
        state.encoding,
    );
    serde_json::to_value(actions).unwrap_or(serde_json::Value::Null)
}

/// `textDocument/semanticTokens/full` — full semantic token encoding.
fn semantic_tokens_full_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) = serde_json::from_value::<lsp_types::SemanticTokensParams>(params.clone())
    else {
        return serde_json::Value::Null;
    };
    let Some((_module, file, _text)) = state.locate(&params.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let result =
        ipe_lsp_features::semantic_tokens::semantic_tokens_full(&state.db, file, state.encoding);
    serde_json::to_value(result).unwrap_or(serde_json::Value::Null)
}

/// `textDocument/signatureHelp` — callee signature at the cursor.
fn signature_help_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) = serde_json::from_value::<lsp_types::SignatureHelpParams>(params.clone())
    else {
        return serde_json::Value::Null;
    };
    let position = params.text_document_position_params;
    let Some((module, _file, text)) = state.locate(&position.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let Some(root) = state.root else {
        return serde_json::Value::Null;
    };
    let Some(entry_file) = root.files(&state.db).get(&state.entry_module).copied() else {
        return serde_json::Value::Null;
    };
    let byte = offset::position_to_offset(&text, position.position, state.encoding);
    let byte = u32::try_from(byte).unwrap_or(u32::MAX);
    ipe_lsp_features::signature_help::signature_help(&state.db, root, entry_file, &module, byte)
        .map_or(serde_json::Value::Null, |help| {
            serde_json::to_value(help).unwrap_or(serde_json::Value::Null)
        })
}

/// `textDocument/inlayHint` — type annotation inlay hints.
fn inlay_hints_result(state: &State, params: &serde_json::Value) -> serde_json::Value {
    let Ok(params) = serde_json::from_value::<lsp_types::InlayHintParams>(params.clone()) else {
        return serde_json::Value::Null;
    };
    let Some((module, _file, _text)) = state.locate(&params.text_document.uri) else {
        return serde_json::Value::Null;
    };
    let Some(root) = state.root else {
        return serde_json::Value::Null;
    };
    let Some(entry_file) = root.files(&state.db).get(&state.entry_module).copied() else {
        return serde_json::Value::Null;
    };
    let hints = ipe_lsp_features::inlay_hints::inlay_hints(
        &state.db,
        root,
        entry_file,
        &module,
        params.range,
        state.encoding,
    );
    serde_json::to_value(hints).unwrap_or(serde_json::Value::Null)
}

/// Latest-generation-wins publishing with change suppression: identical
/// payloads are not re-sent, and a URI whose diagnostics healed (or whose
/// module left the project) gets one clearing empty push.
fn publish(state: &mut State, connection: &Connection, batch: DiagnosticsBatch) {
    if batch.generation != state.generation {
        return;
    }
    let mut current: BTreeMap<Url, Vec<lsp_types::Diagnostic>> =
        batch.per_uri.into_iter().collect();
    for uri in state.last_published.keys() {
        current.entry(uri.clone()).or_default();
    }
    for (uri, diags) in &current {
        let previous = state.last_published.get(uri);
        if diags.is_empty() && previous.is_none() {
            continue; // nothing was ever shown here — nothing to clear
        }
        if previous == Some(diags) {
            continue;
        }
        let params = PublishDiagnosticsParams {
            uri: uri.clone(),
            diagnostics: diags.clone(),
            version: None,
        };
        let note = Notification::new(PublishDiagnostics::METHOD.to_owned(), params);
        let _ = connection.sender.send(Message::Notification(note));
    }
    state.last_published = current
        .into_iter()
        .filter(|(_, diags)| !diags.is_empty())
        .collect();
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use crossbeam_channel::RecvTimeoutError;

    use super::{
        Connection, DiagnosticsBatch, LoadedFile, LoadedProject, Message, ModuleOrigin, Path,
        PathBuf, PositionEncoding, ProjectLoader, PublishDiagnostics, PublishDiagnosticsParams,
        State, Url, ensure_project_fresh, normalize, publish, recompute, sync_inputs,
    };
    use crate::loader::LoadError;
    use lsp_types::notification::Notification as _;

    const MAIN_TEXT: &str =
        "module Main exposing (main)\n\nimport Ipe.Io as Io\n\nmain : Task Error ()\nmain =\n    Io.println \"ok\"\n";
    const LIB_TEXT: &str = "module Lib exposing (bad)\n\nbad : Int\nbad = \"nope\"\n";

    /// A two-module loader (`Main` open buffer + a static `Lib` with a real
    /// type error) that fails the NEXT `load` call when armed — the
    /// transient-failure shape `ensure_project_fresh` must survive without
    /// dropping the previously-good layout.
    struct TwoModuleLoader {
        fail_next: Arc<AtomicBool>,
        lib_path: PathBuf,
    }

    impl ProjectLoader for TwoModuleLoader {
        fn load(
            &self,
            _workspace_root: Option<&Path>,
            open_file: &Path,
            open_text: Option<&str>,
        ) -> Result<LoadedProject, LoadError> {
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err(LoadError {
                    detail: "simulated transient failure".to_owned(),
                });
            }
            let mut files = BTreeMap::new();
            files.insert(
                vec!["Main".to_owned()],
                LoadedFile {
                    path: open_file.to_path_buf(),
                    text: open_text.unwrap_or(MAIN_TEXT).to_owned(),
                    origin: ModuleOrigin::User,
                },
            );
            files.insert(
                vec!["Lib".to_owned()],
                LoadedFile {
                    path: self.lib_path.clone(),
                    text: LIB_TEXT.to_owned(),
                    origin: ModuleOrigin::User,
                },
            );
            Ok(LoadedProject {
                files,
                entry_module: vec!["Main".to_owned()],
            })
        }
    }

    /// A load failure on an already-well-formed project (CO-INCR-007) must
    /// not clear `Lib`'s real diagnostics: the prior layout is kept and
    /// retried later, not replaced by the single-file fallback.
    #[test]
    fn transient_load_failure_keeps_prior_layout_diagnostics() {
        let main_path = normalize(Path::new("/lsp-278-test/Main.ipe"));
        let lib_path = normalize(Path::new("/lsp-278-test/Lib.ipe"));
        let lib_uri = Url::from_file_path(&lib_path).expect("lib uri");
        let fail_next = Arc::new(AtomicBool::new(false));
        let loader = TwoModuleLoader {
            fail_next: fail_next.clone(),
            lib_path,
        };

        let mut state = State::new(None, PositionEncoding::Utf16);
        state
            .overlays
            .insert(main_path.clone(), MAIN_TEXT.to_owned());

        // First load succeeds: both modules known, Lib carries a real
        // compiler diagnostic.
        ensure_project_fresh(&mut state, &loader, &main_path);
        assert!(!state.fallback, "first load must succeed cleanly");
        sync_inputs(&mut state);

        let (diag_tx, diag_rx) = crossbeam_channel::unbounded::<DiagnosticsBatch>();
        recompute(&mut state, &diag_tx);
        let batch1 = diag_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("first diagnostics batch");

        let (server_side, client) = Connection::memory();
        publish(&mut state, &server_side, batch1);

        let mut saw_lib_diagnostic = false;
        while let Ok(msg) = client.receiver.recv_timeout(Duration::from_millis(500)) {
            if let Message::Notification(note) = msg
                && note.method == PublishDiagnostics::METHOD
                && let Ok(params) = serde_json::from_value::<PublishDiagnosticsParams>(note.params)
                && params.uri == lib_uri
                && !params.diagnostics.is_empty()
            {
                saw_lib_diagnostic = true;
            }
        }
        assert!(
            saw_lib_diagnostic,
            "Lib's real diagnostic must publish before the transient failure"
        );

        // Arm a transient failure on the NEXT load — the shape of a
        // `didSave`/`DidChangeWatchedFiles` re-resolve racing a momentary
        // I/O error or a mid-rename tree.
        fail_next.store(true, Ordering::SeqCst);
        ensure_project_fresh(&mut state, &loader, &main_path);
        sync_inputs(&mut state);
        recompute(&mut state, &diag_tx);
        let batch2 = diag_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("second diagnostics batch");
        publish(&mut state, &server_side, batch2);

        // The prior layout must survive: no clearing push for Lib's
        // diagnostics reaches the client.
        loop {
            match client.receiver.recv_timeout(Duration::from_millis(500)) {
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
                Ok(Message::Notification(note)) if note.method == PublishDiagnostics::METHOD => {
                    let params: PublishDiagnosticsParams =
                        serde_json::from_value(note.params).expect("valid params");
                    if params.uri == lib_uri {
                        assert!(
                            !params.diagnostics.is_empty(),
                            "a transient load failure must not clear Lib's real diagnostics"
                        );
                    }
                }
                Ok(_) => {}
            }
        }
    }
}

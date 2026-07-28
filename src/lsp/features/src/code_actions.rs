//! Code actions: diagnostic-driven quick-fixes.
//!
//! For each LSP diagnostic in the requested range we produce zero or more
//! `CodeAction`s — workspace edits the client can apply with one click.
//!
//! **Supported quick-fixes (by diagnostic code):**
//!
//! - `IPE-T0001` (type mismatch) / no match arms on a `case`: no automatic fix
//!   (the shape is too varied).
//! - `IPE-N0001` (unused import): "Remove unused import" — delete the import
//!   line.
//! - `IPE-N0003` (missing type annotation): "Add type annotation" — insert the
//!   inferred type annotation above the binding.
//! - `IPE-N0034` (standard-library module used without importing it): "Add
//!   import `Ipe.X`" — insert the named `import Ipe.X` line into the module's
//!   import block, alphabetically among the existing imports.
//!
//! The provider is deliberately conservative: it only acts on codes it can
//! fix with a single-hunk text edit that it can prove correct. Unknown codes
//! produce no actions rather than a guess.

use std::collections::HashMap;

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Diagnostic, NumberOrString, Range, TextEdit,
    Url, WorkspaceEdit,
};

use ipe_db::{Db as _, IpeDatabase, SourceRoot};
use ipe_diagnostics::Span;

use crate::offset::{PositionEncoding, offset_to_position};

/// The salsa database view a quick-fix reads from.
///
/// Bundles the database, the source root, and the build's entry file so the
/// action signatures stay readable — they otherwise thread the same three
/// values through every helper.
#[derive(Clone, Copy)]
pub struct DbView<'a> {
    /// The salsa database snapshot.
    pub db: &'a IpeDatabase,
    /// The compilation's source root.
    pub root: SourceRoot,
    /// The build's entry file (the module `typecheck` is rooted at).
    pub entry: ipe_db::SourceFile,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute quick-fix code actions for the given range and diagnostic list.
///
/// `diagnostics` are the LSP diagnostics currently shown for `uri` — the
/// client forwards them in the request so we do not need to re-collect them.
/// `text` is the current source text of the document.
#[must_use]
pub fn code_actions(
    view: DbView<'_>,
    module: &[String],
    uri: &Url,
    range: Range,
    diagnostics: &[Diagnostic],
    text: &str,
    encoding: PositionEncoding,
) -> Vec<CodeActionOrCommand> {
    let DbView { db, root, .. } = view;
    let files = root.files(db);
    let Some(&_file) = files.get(module) else {
        return Vec::new();
    };

    // Collect actions for each diagnostic that overlaps the requested range.
    let in_range: Vec<&Diagnostic> = diagnostics
        .iter()
        .filter(|d| ranges_overlap(d.range, range))
        .collect();

    if in_range.is_empty() {
        return Vec::new();
    }

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();

    for diag in in_range {
        let Some(NumberOrString::String(code)) = &diag.code else {
            continue;
        };
        match code.as_str() {
            "IPE-N0001" => {
                // Unused import — remove the line containing this diagnostic.
                let action = remove_line_action(uri, diag, text, "Remove unused import", encoding);
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
            "IPE-N0004" => {
                // Missing type annotation — insert the annotation.
                // The diagnostic message carries the inferred type in the form
                // `missing type annotation for `name`: Type`. We extract the
                // type string and synthesise the edit.
                if let Some(action) =
                    add_type_annotation_action(view, module, uri, diag, text, encoding)
                {
                    actions.push(CodeActionOrCommand::CodeAction(action));
                }
            }
            "IPE-N0034" => {
                // Standard-library module used without importing it — insert the
                // named `import Ipe.X` line. The diagnostic names the exact module
                // to add (`add `import Ipe.X` to use it`); we insert it in the
                // module's import block, sorted among the existing imports.
                if let Some(action) = add_import_action(view, module, uri, diag, text, encoding) {
                    actions.push(CodeActionOrCommand::CodeAction(action));
                }
            }
            _ => {}
        }
    }

    actions
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ranges_overlap(a: Range, b: Range) -> bool {
    a.start <= b.end && b.start <= a.end
}

/// Quick-fix that deletes the entire line (including its trailing newline)
/// that contains `diag.range`.
fn remove_line_action(
    uri: &Url,
    diag: &Diagnostic,
    text: &str,
    title: &str,
    encoding: PositionEncoding,
) -> CodeAction {
    let line = diag.range.start.line as usize;
    // Byte span of the full line including its trailing '\n'.
    let (line_start, line_end) = line_byte_range(text, line);
    let start = offset_to_position(text, line_start, encoding);
    let end = offset_to_position(text, line_end, encoding);
    let edit = TextEdit {
        range: Range { start, end },
        new_text: String::new(),
    };
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    CodeAction {
        title: title.to_owned(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }),
        command: None,
        is_preferred: Some(true),
        disabled: None,
        data: None,
    }
}

/// Quick-fix that inserts a type annotation above the binding named in the
/// diagnostic message. Extracts the name and inferred type from the message.
fn add_type_annotation_action(
    view: DbView<'_>,
    module: &[String],
    uri: &Url,
    diag: &Diagnostic,
    text: &str,
    encoding: PositionEncoding,
) -> Option<CodeAction> {
    // Try to extract name + type from the solved type environment.
    // Diagnostic range points at the name token — resolve it via the parse
    // tree to find the line the annotation should precede.
    let DbView { db, root, entry } = view;
    let files = root.files(db);
    let &file = files.get(module)?;
    let parsed = ipe_db::parse(db, file).ok()?;
    let byte = {
        let lo = diag.range.start;
        u32::try_from(crate::offset::position_to_offset(text, lo, encoding)).unwrap_or(u32::MAX)
    };
    // Find which top-level binding spans this byte.
    let interner = db.interner().lock();
    let mut found_name: Option<String> = None;
    let mut annotation_line: Option<u32> = None;
    for value in &parsed.values {
        let name_span = value.value.name.span;
        if name_span.lo <= byte && byte < name_span.hi {
            found_name = interner.resolve(value.value.name.value).map(str::to_owned);
            annotation_line = Some(offset_to_position(text, name_span.lo as usize, encoding).line);
            break;
        }
    }
    drop(interner);
    let name = found_name?;
    let insert_line = annotation_line?;

    // Retrieve inferred type from the solved type environment.
    let solved = ipe_db::typecheck(db, root, entry).ok()?;
    let mut interner = db.interner().lock();
    let home: Vec<ipe_intern::Symbol> = module
        .iter()
        .map(|s| interner.intern(s).ok())
        .collect::<Option<Vec<_>>>()?;
    let name_sym = interner.intern(&name).ok()?;
    drop(interner);

    let ty = solved.env.get(&(home, name_sym))?;
    let interner = db.interner().lock();
    let mut namer = ipe_types::VarNamer::new();
    let doc = ipe_types::ty_to_doc(ty, &interner, &mut namer).ok()?;
    drop(interner);
    let ty_str = ipe_diagnostics::render_ty(&doc);

    // Synthesise `name : Type\n` inserted at the start of `insert_line`.
    let insert_byte = line_byte_range(text, insert_line as usize).0;
    let insert_pos = offset_to_position(text, insert_byte, encoding);
    let annotation = format!("{name} : {ty_str}\n");
    let edit = TextEdit {
        range: Range {
            start: insert_pos,
            end: insert_pos,
        },
        new_text: annotation,
    };
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    Some(CodeAction {
        title: format!("Add type annotation for `{name}`"),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }),
        command: None,
        is_preferred: Some(true),
        disabled: None,
        data: None,
    })
}

/// Quick-fix that inserts the `import Ipe.X` line named by an IPE-N0034
/// diagnostic into the module's import block.
///
/// The module to add is read from the diagnostic message, which the compiler
/// renders as `… add `import Ipe.X` to use it`: the exact `import` clause is
/// backtick-quoted, so we lift the module path out of that clause verbatim
/// rather than reconstructing it. The line is inserted alphabetically among the
/// existing `import` lines (import order is not significant to the compiler, so
/// a sorted position is both valid and predictable); with no existing imports
/// it goes just below the `module … exposing (…)` header, separated by a blank
/// line to match first-party formatting.
fn add_import_action(
    view: DbView<'_>,
    module: &[String],
    uri: &Url,
    diag: &Diagnostic,
    text: &str,
    encoding: PositionEncoding,
) -> Option<CodeAction> {
    let import_module = import_module_from_message(&diag.message)?;

    let DbView { db, root, .. } = view;
    let files = root.files(db);
    let &file = files.get(module)?;
    let parsed = ipe_db::parse(db, file).ok()?;

    // Existing imports as (dotted-path, byte offset of the import declaration).
    // The import keyword starts at the beginning of the line that its module
    // path begins on, so we snap each path span back to its line start.
    let interner = db.interner().lock();
    let mut imports: Vec<(String, usize)> = Vec::with_capacity(parsed.imports.len());
    for imp in &parsed.imports {
        let dotted: Option<Vec<&str>> = imp
            .name
            .value
            .iter()
            .map(|&s| interner.resolve(s))
            .collect();
        let Some(segs) = dotted else { continue };
        let path = segs.join(".");
        let line = offset_to_position(text, imp.name.span.lo as usize, encoding).line as usize;
        let (line_start, _) = line_byte_range(text, line);
        imports.push((path, line_start));
    }
    // The last byte of the header's `exposing (...)` clause: where an
    // import-less module's new import block begins.
    let header_end = parsed.exposing.span.hi as usize;
    drop(interner);

    // Do not offer the action if the import already exists (the diagnostic
    // would be stale) — a duplicate insert never fixes anything.
    if imports.iter().any(|(path, _)| *path == import_module) {
        return None;
    }

    let (insert_byte, new_text) = if imports.is_empty() {
        // No imports yet: open the block just after the header, one blank line
        // down, matching the `module …\n\n\nimport …` first-party convention.
        let after_header_line = offset_to_position(text, header_end, encoding).line as usize;
        let (_, line_end) = line_byte_range(text, after_header_line);
        (line_end, format!("\nimport {import_module}\n"))
    } else {
        // Insert before the first existing import that sorts after the new one;
        // if none, append after the last import line.
        match imports
            .iter()
            .find(|(path, _)| path.as_str() > import_module.as_str())
        {
            Some((_, start)) => (*start, format!("import {import_module}\n")),
            None => {
                let last_line_start = imports.iter().map(|(_, s)| *s).max().unwrap_or(0);
                let last_line = offset_to_position(text, last_line_start, encoding).line as usize;
                let (_, line_end) = line_byte_range(text, last_line);
                (line_end, format!("import {import_module}\n"))
            }
        }
    };

    let insert_pos = offset_to_position(text, insert_byte, encoding);
    let edit = TextEdit {
        range: Range {
            start: insert_pos,
            end: insert_pos,
        },
        new_text,
    };
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    Some(CodeAction {
        title: format!("Add import {import_module}"),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }),
        command: None,
        is_preferred: Some(true),
        disabled: None,
        data: None,
    })
}

/// Lift the `Ipe.X` module path out of an IPE-N0034 message.
///
/// The message ends with `add `import Ipe.X` to use it`. We take the content of
/// the backtick pair whose contents begin with `import `, then strip that
/// keyword — yielding the dotted module path (`Ipe.X`) verbatim. Returns `None`
/// if the message does not carry a backtick-quoted `import` clause, so a
/// reworded or unrelated diagnostic simply produces no action.
fn import_module_from_message(message: &str) -> Option<String> {
    for clause in message.split('`') {
        if let Some(rest) = clause.strip_prefix("import ") {
            let module = rest.trim();
            if !module.is_empty() {
                return Some(module.to_owned());
            }
        }
    }
    None
}

/// Returns `(start_byte, end_byte)` for `line` (0-based), where `end_byte`
/// points just past the trailing `\n` (or to `text.len()` on the last line).
fn line_byte_range(text: &str, line: usize) -> (usize, usize) {
    let mut byte = 0;
    for (i, l) in text.split('\n').enumerate() {
        let next = byte + l.len() + 1; // +1 for the '\n'
        if i == line {
            return (byte, next.min(text.len()));
        }
        byte = next;
    }
    (text.len(), text.len())
}

// Suppress the unused-import warning for `Span` (used only in doc context).
#[allow(dead_code)]
type _SpanAlias = Span;

#[cfg(test)]
mod tests {
    use ipe_db::{IpeDatabase, ModuleOrigin, SourceFile, SourceRoot};
    use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};

    use crate::offset::PositionEncoding;

    use super::{DbView, code_actions};

    fn file(db: &IpeDatabase, path: &[&str], text: &str) -> SourceFile {
        SourceFile::new(
            db,
            path.iter().map(|s| (*s).to_owned()).collect(),
            text.to_owned(),
            ModuleOrigin::User,
        )
    }

    fn root_of(db: &IpeDatabase, files: &[(&[&str], SourceFile)]) -> SourceRoot {
        SourceRoot::new(
            db,
            files
                .iter()
                .map(|(path, f)| (path.iter().map(|s| (*s).to_owned()).collect(), *f))
                .collect(),
        )
    }

    fn diag_at(line: u32, code: &str) -> Diagnostic {
        #[allow(deprecated)]
        Diagnostic {
            range: Range {
                start: Position { line, character: 0 },
                end: Position {
                    line,
                    character: 10,
                },
            },
            severity: Some(DiagnosticSeverity::WARNING),
            code: Some(NumberOrString::String(code.to_owned())),
            code_description: None,
            source: Some("ipe".to_owned()),
            message: format!("test diagnostic {code}"),
            related_information: None,
            tags: None,
            data: None,
        }
    }

    #[test]
    fn unknown_code_produces_no_actions() {
        let db = IpeDatabase::new();
        let src = "module Main exposing (main)\n\nmain : Int\nmain =\n    42\n";
        let entry = file(&db, &["Main"], src);
        let root = root_of(&db, &[(&["Main"], entry)]);
        let uri = Url::from_file_path("/fake/Main.ipe").unwrap();
        let range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 4,
                character: 0,
            },
        };
        let diag = diag_at(2, "IPE-X9999");
        let actions = code_actions(
            DbView {
                db: &db,
                root,
                entry,
            },
            &["Main".to_owned()],
            &uri,
            range,
            &[diag],
            src,
            PositionEncoding::Utf16,
        );
        assert!(actions.is_empty(), "unknown code → no actions");
    }

    #[test]
    fn remove_import_action_deletes_the_import_line() {
        let db = IpeDatabase::new();
        let src = "module Main exposing (main)\n\nimport Unused\n\nmain : Int\nmain =\n    42\n";
        let entry = file(&db, &["Main"], src);
        let root = root_of(&db, &[(&["Main"], entry)]);
        let uri = Url::from_file_path("/fake/Main.ipe").unwrap();
        let range = Range {
            start: Position {
                line: 2,
                character: 0,
            },
            end: Position {
                line: 2,
                character: 13,
            },
        };
        let diag = diag_at(2, "IPE-N0001");
        let actions = code_actions(
            DbView {
                db: &db,
                root,
                entry,
            },
            &["Main".to_owned()],
            &uri,
            range,
            &[diag],
            src,
            PositionEncoding::Utf16,
        );
        assert_eq!(actions.len(), 1, "one action for unused import");
        let action = actions.into_iter().find_map(|a| match a {
            lsp_types::CodeActionOrCommand::CodeAction(action) => Some(action),
            lsp_types::CodeActionOrCommand::Command(_) => None,
        });
        let action = action.expect("the single action is a CodeAction");
        assert_eq!(action.title, "Remove unused import");
        let edit = action
            .edit
            .as_ref()
            .and_then(|e| e.changes.as_ref())
            .and_then(|c| c.values().next())
            .and_then(|v| v.first())
            .expect("edit present");
        assert!(edit.new_text.is_empty(), "replacement is empty (delete)");
    }
}

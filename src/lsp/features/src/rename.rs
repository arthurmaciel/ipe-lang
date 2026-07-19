//! Rename: `textDocument/rename` and `textDocument/prepareRename`.
//!
//! Delegates reference collection to [`crate::navigation`] and definition
//! lookup to [`crate::navigation::goto_definition`]. Builds a
//! [`lsp_types::WorkspaceEdit`] with one `TextEdit` per span (definition +
//! every reference), grouped by document URI.
//!
//! Scope: top-level bindings only — the same scope `navigation` tracks.
//! Rename returns `None` (refuse) for any position that is not on a top-level
//! reference or definition.

use std::collections::BTreeMap;

use ipe_db::{Db as _, IpeDatabase, SourceRoot};
use ipe_diagnostics::Span;
use lsp_types::{TextEdit, Url, WorkspaceEdit};

use crate::navigation::{Definition, NameRef, find_references, goto_definition};
use crate::offset::{PositionEncoding, span_to_range};

/// The identifier and its span at a position, returned by `prepare_rename`.
///
/// The client uses the span to pre-fill the rename input box with the current
/// name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PrepareRename {
    /// The current name of the identifier under the cursor.
    pub name: String,
    /// The byte span of that identifier in the requesting module's source.
    pub span: Span,
}

/// Validate that the position is renameable and return the current name.
///
/// Returns `None` when the position is not on a top-level reference or
/// definition — the client should show no rename UI in that case.
#[must_use]
pub fn prepare_rename(
    db: &IpeDatabase,
    root: SourceRoot,
    _entry: ipe_db::SourceFile,
    module: &[String],
    byte: u32,
) -> Option<PrepareRename> {
    let files = root.files(db);
    let &file = files.get(module)?;

    // Try the position as a reference first (common case).
    let canonical = ipe_db::canonicalize(db, root, file).ok()?;
    if let Some((_, name_sym)) = crate::navigation::find_ref_at_pub(&canonical.module, byte) {
        let name = {
            let interner = db.interner().lock();
            let n = interner.resolve(name_sym).map(str::to_owned)?;
            drop(interner);
            n
        };
        // Re-find the span for the PrepareRename range.
        let span = crate::navigation::ref_span_at(&canonical.module, byte)?;
        return Some(PrepareRename { name, span });
    }

    // Try the position as a definition site.
    let parsed = ipe_db::parse(db, file).ok()?;
    for value in &parsed.values {
        let name_span = value.value.name.span;
        if name_span.lo <= byte && byte < name_span.hi {
            let interner = db.interner().lock();
            let name = interner
                .resolve(value.value.name.value)
                .map(str::to_owned)?;
            drop(interner);
            return Some(PrepareRename {
                name,
                span: name_span,
            });
        }
    }

    None
}

/// Supplies URI and text for a module path, forwarded from the server layer
/// so the features crate never touches the filesystem.
pub struct ModuleResolver<'a> {
    /// Maps a module path to its document URI.
    pub uri_of_module: &'a dyn Fn(&[String]) -> Option<Url>,
    /// Maps a module path to its current source text.
    pub text_of_module: &'a dyn Fn(&[String]) -> Option<String>,
}

/// The cursor position and replacement text for a rename request.
pub struct RenameRequest<'a> {
    /// Cursor byte offset within `module`.
    pub byte: u32,
    /// The replacement name the user typed.
    pub new_name: &'a str,
    /// Position encoding in use for the session.
    pub encoding: PositionEncoding,
}

/// Apply a rename across all references to the top-level identifier at
/// `req.byte` in `module`. Returns the `WorkspaceEdit` the client should
/// apply, or `None` when the position is not renameable.
///
/// `resolver` supplies URI and text callbacks; they are injected by the
/// server because URI construction requires filesystem paths the features
/// crate never touches.
#[must_use]
pub fn rename(
    db: &IpeDatabase,
    root: SourceRoot,
    entry: ipe_db::SourceFile,
    module: &[String],
    req: &RenameRequest<'_>,
    resolver: &ModuleResolver<'_>,
) -> Option<WorkspaceEdit> {
    // Resolve the definition of whatever is at `req.byte`.
    let Definition {
        module: def_module,
        span: def_span,
    } = goto_definition(db, root, entry, module, req.byte)?;

    // Recover the current name from the definition span.
    let def_text = (resolver.text_of_module)(&def_module)?;
    let lo = def_span.lo as usize;
    let hi = def_span.hi as usize;
    let current_name = def_text.get(lo..hi)?;

    // Collect all reference spans.
    let refs: Vec<NameRef> = find_references(db, root, entry, &def_module, current_name);

    // Build the workspace edit: group edits by URI.
    let mut edits_by_uri: BTreeMap<Url, Vec<TextEdit>> = BTreeMap::new();

    // Definition edit.
    if let Some(def_uri) = (resolver.uri_of_module)(&def_module) {
        let range = span_to_range(&def_text, def_span, req.encoding);
        edits_by_uri.entry(def_uri).or_default().push(TextEdit {
            range,
            new_text: req.new_name.to_owned(),
        });
    }

    // Reference edits.
    for r in refs {
        let Some(ref_uri) = (resolver.uri_of_module)(&r.module) else {
            continue;
        };
        let Some(ref_text) = (resolver.text_of_module)(&r.module) else {
            continue;
        };
        let range = span_to_range(&ref_text, r.span, req.encoding);
        edits_by_uri.entry(ref_uri).or_default().push(TextEdit {
            range,
            new_text: req.new_name.to_owned(),
        });
    }

    if edits_by_uri.is_empty() {
        return None;
    }

    Some(WorkspaceEdit {
        changes: Some(edits_by_uri.into_iter().collect()),
        document_changes: None,
        change_annotations: None,
    })
}

#[cfg(test)]
mod tests {
    use ipe_db::{IpeDatabase, ModuleOrigin, SourceFile, SourceRoot};
    use lsp_types::Url;

    use crate::offset::PositionEncoding;

    use super::{prepare_rename, rename};

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

    const HELPER: &str = "module Helper exposing (three)\n\nthree : Int\nthree = 3\n";
    const MAIN: &str = "module Main exposing (main)\n\nimport Helper exposing (three)\n\nmain : Int\nmain = three\n";

    fn ref_byte() -> u32 {
        u32::try_from(MAIN.rfind("three").expect("three in main")).expect("u32")
    }

    fn make_uri(module: &[String]) -> Option<Url> {
        let path = format!("/fake/{}.ipe", module.join("/"));
        Url::from_file_path(path).ok()
    }

    fn make_text(module: &[String]) -> Option<String> {
        match module.first().map(String::as_str) {
            Some("Helper") => Some(HELPER.to_owned()),
            Some("Main") => Some(MAIN.to_owned()),
            _ => None,
        }
    }

    #[test]
    fn prepare_rename_finds_identifier_at_reference_site() {
        let db = IpeDatabase::new();
        let helper = file(&db, &["Helper"], HELPER);
        let entry = file(&db, &["Main"], MAIN);
        let root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);

        let result = prepare_rename(&db, root, entry, &["Main".to_owned()], ref_byte());
        let r = result.expect("prepare_rename returned Some");
        assert_eq!(r.name, "three");
    }

    #[test]
    fn rename_produces_edits_for_definition_and_all_references() {
        let db = IpeDatabase::new();
        let helper = file(&db, &["Helper"], HELPER);
        let entry = file(&db, &["Main"], MAIN);
        let root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);

        let ws_edit = rename(
            &db,
            root,
            entry,
            &["Main".to_owned()],
            &super::RenameRequest {
                byte: ref_byte(),
                new_name: "four",
                encoding: PositionEncoding::Utf16,
            },
            &super::ModuleResolver {
                uri_of_module: &make_uri,
                text_of_module: &make_text,
            },
        )
        .expect("rename returned Some");

        let changes = ws_edit.changes.expect("has changes");
        // Two files should have edits: Helper (definition) and Main (reference).
        assert_eq!(
            changes.len(),
            2,
            "edits in 2 files: {:?}",
            changes.keys().collect::<Vec<_>>()
        );
        for edits in changes.values() {
            for edit in edits {
                assert_eq!(edit.new_text, "four");
            }
        }
    }
}

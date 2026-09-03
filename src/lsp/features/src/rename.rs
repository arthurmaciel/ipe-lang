//! Rename: `textDocument/rename` and `textDocument/prepareRename`.
//!
//! Reference collection and capture-avoidance are delegated to
//! [`ipe_canon::rename`], which operates on the `ReferenceIndex` built from
//! the fully resolved module graph.  This module owns the LSP boundary:
//! converting positions to byte offsets, building the `WorkspaceEdit`, and
//! injecting the URI/text callbacks from the server layer.
//!
//! Scope: top-level bindings only.  Rename returns `None` (refuse) when the
//! position is not on a top-level reference or definition, when the new name
//! is invalid, or when the canon layer detects a capture conflict.
//!
//! The `new_name` string from the LSP client is untrusted: [`ValidatedIdentifier`]
//! is the parse-don't-validate boundary; only the validated form reaches the
//! edit builder.

use std::collections::BTreeMap;

use ipe_canon::ref_index::ReferenceIndex;
use ipe_canon::rename::{EditSet, RenameError, rename as canon_rename};
use ipe_db::{Db as _, IpeDatabase, SourceRoot};
use ipe_diagnostics::Span;
use ipe_intern::Symbol;
use lsp_types::{TextEdit, Url, WorkspaceEdit};

use crate::navigation::goto_definition;
use crate::offset::{PositionEncoding, span_to_range};

// ── Identifier grammar ───────────────────────────────────────────────────────
//
// These predicates mirror `ipe_parse`'s lexer
// (`src/compiler/parse/src/lexer.rs`: `is_ident_start` / `is_ident_continue`).
// If those rules change, update here in lockstep — SSOT is the lexer.

const fn ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

const fn ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Keywords the lexer recognises — a lexically valid identifier that matches
/// one of these is still rejected because renaming to a keyword produces
/// un-parseable source.
const KEYWORDS: &[&str] = &[
    "module", "import", "exposing", "as", "type", "case", "of", "let", "in", "if", "then", "else",
    "do",
];

// ── Case class ───────────────────────────────────────────────────────────────

/// Whether a renamed symbol is a type/constructor (uppercase) or a value
/// (lowercase / `_`). Mirrors the lexer's `Tok::UpperIdent` / `Tok::LowerIdent`
/// split.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SymbolKind {
    /// Type alias, custom type, or constructor — first char must be uppercase.
    Type,
    /// Value binding or function — first char must be lowercase or `_`.
    Value,
}

// ── ValidatedIdentifier ──────────────────────────────────────────────────────

/// A single well-formed Ipê identifier that passed all rename guards: correct
/// lexical shape, not a keyword, and matching case class for the renamed symbol.
///
/// Only constructible via [`ValidatedIdentifier::parse`] — invalid names cannot
/// be represented by this type.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ValidatedIdentifier(String);

impl ValidatedIdentifier {
    /// Parse `raw` into a validated identifier for a rename of `kind`.
    ///
    /// Returns `None` when `raw` is empty, contains spaces or non-ASCII chars,
    /// is not a single `[A-Za-z_][A-Za-z0-9_]*` token, is a keyword, or has
    /// the wrong case class for `kind`.
    #[must_use]
    pub fn parse(raw: &str, kind: SymbolKind) -> Option<Self> {
        let mut chars = raw.chars();
        let first = chars.next()?;
        if !ident_start(first) {
            return None;
        }
        if !chars.all(ident_continue) {
            return None;
        }
        if KEYWORDS.contains(&raw) {
            return None;
        }
        match kind {
            SymbolKind::Type => {
                if !first.is_ascii_uppercase() {
                    return None;
                }
            }
            SymbolKind::Value => {
                if first.is_ascii_uppercase() {
                    return None;
                }
            }
        }
        Some(Self(raw.to_owned()))
    }

    /// The validated identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Infer the [`SymbolKind`] of an identifier from its first character.
fn symbol_kind_of(name: &str) -> SymbolKind {
    match name.chars().next() {
        Some(c) if c.is_ascii_uppercase() => SymbolKind::Type,
        _ => SymbolKind::Value,
    }
}

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
    entry: ipe_db::SourceFile,
    module: &[String],
    byte: u32,
) -> Option<PrepareRename> {
    let files = root.files(db);
    let &file = files.get(module)?;

    // Try the position as a reference first (common case).
    let canonical = crate::db_access::canonicalize_checked(db, root, entry, file)?;
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
///
/// `new_name` is the raw string from the LSP client. [`rename`] parses it
/// into a [`ValidatedIdentifier`] at the boundary — if validation fails,
/// `rename` returns `None` and no edits are emitted.
pub struct RenameRequest<'a> {
    /// Cursor byte offset within `module`.
    pub byte: u32,
    /// The replacement name the user typed (validated inside [`rename`]).
    pub new_name: &'a str,
    /// Position encoding in use for the session.
    pub encoding: PositionEncoding,
}

/// Apply a rename across all references to the top-level identifier at
/// `req.byte` in `module`. Returns the `WorkspaceEdit` the client should
/// apply, or `None` when the position is not renameable.
///
/// Reference collection and capture-avoidance are handled by
/// [`ipe_canon::rename::rename`] operating on a [`ReferenceIndex`] built
/// from the fully resolved module graph.
///
/// `resolver` supplies URI and text callbacks; they are injected by the server
/// because URI construction requires filesystem paths the features crate never
/// touches.
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
    let def = goto_definition(db, root, entry, module, req.byte)?;
    let def_module = &def.module;

    // Recover the current name from the definition span.
    let def_text = (resolver.text_of_module)(def_module)?;
    let lo = def_span_lo_usize(def.span)?;
    let hi = def_span_hi_usize(def.span)?;
    let current_name = def_text.get(lo..hi)?;

    // Pre-validate at the LSP boundary (typed parse-don't-validate gate).
    let kind = symbol_kind_of(current_name);
    ValidatedIdentifier::parse(req.new_name, kind)?;

    // Build the ReferenceIndex from the full resolved module graph.
    // `topo_order` proves the import graph is acyclic; `canonicalize_checked`
    // repeats the guard per file so the raw `ipe_db::canonicalize` path is
    // never taken on the interactive handler path.
    let Ok(order) = ipe_db::topo_order(db, root, entry) else {
        return None;
    };
    let files = root.files(db);

    // Collect canonical modules for every file in topo order.
    // We keep them alive in a Vec so the &Module references remain valid.
    let mut canon_modules: Vec<(ipe_canon::ast::Module, Vec<Symbol>)> = Vec::new();
    for module_path in &*order {
        let Some(&module_file) = files.get(module_path) else {
            continue;
        };
        let Some(canonical) = crate::db_access::canonicalize_checked(db, root, entry, module_file)
        else {
            continue;
        };
        // Convert the String module path to a Symbol path.
        let sym_path: Option<Vec<Symbol>> = {
            let mut interner = db.interner().lock();
            module_path
                .iter()
                .map(|s| interner.intern(s).ok())
                .collect::<Option<Vec<_>>>()
        };
        if let Some(sym_path) = sym_path {
            canon_modules.push((canonical.module.clone(), sym_path));
        }
    }

    // Resolve the defining module path and old_name to symbols.
    let (def_module_syms, old_name_sym) = {
        let mut interner = db.interner().lock();
        let def_syms: Option<Vec<Symbol>> = def_module
            .iter()
            .map(|s| interner.intern(s).ok())
            .collect::<Option<Vec<_>>>();
        let name_sym = interner.intern(current_name).ok();
        drop(interner);
        match (def_syms, name_sym) {
            (Some(d), Some(n)) => (d, n),
            _ => return None,
        }
    };

    // Build the reference index.
    let module_slice: Vec<(&ipe_canon::ast::Module, &[Symbol])> = canon_modules
        .iter()
        .map(|(m, p)| (m, p.as_slice()))
        .collect();
    let index = ReferenceIndex::build(&module_slice);

    // Delegate to the canon rename engine.
    let interner = db.interner().lock();
    let edit_set: EditSet = match canon_rename(
        &interner,
        &index,
        &module_slice,
        &def_module_syms,
        old_name_sym,
        req.new_name,
    ) {
        Ok(es) => es,
        Err(
            RenameError::InvalidIdentifier { .. }
            | RenameError::SymbolNotFound { .. }
            | RenameError::CaptureConflict { .. },
        ) => {
            return None;
        }
    };
    drop(interner);

    edit_set_to_workspace_edit(db, &edit_set, resolver, req.encoding)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Group a canon [`EditSet`] into an LSP [`WorkspaceEdit`] keyed by document URI.
///
/// Each edit's symbol module path is resolved back to a string path for the
/// resolver's URI and text callbacks; an edit whose path, URI, or text cannot be
/// resolved is skipped. Returns `None` when no edit survives resolution.
fn edit_set_to_workspace_edit(
    db: &IpeDatabase,
    edit_set: &EditSet,
    resolver: &ModuleResolver<'_>,
    encoding: PositionEncoding,
) -> Option<WorkspaceEdit> {
    let mut edits_by_uri: BTreeMap<Url, Vec<TextEdit>> = BTreeMap::new();

    for edit in &edit_set.edits {
        let string_path: Option<Vec<String>> = {
            let interner = db.interner().lock();
            edit.file
                .iter()
                .map(|&s| interner.resolve(s).map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        };
        let Some(string_path) = string_path else {
            continue;
        };
        let Some(uri) = (resolver.uri_of_module)(&string_path) else {
            continue;
        };
        let Some(text) = (resolver.text_of_module)(&string_path) else {
            continue;
        };
        let range = span_to_range(&text, edit.span, encoding);
        edits_by_uri.entry(uri).or_default().push(TextEdit {
            range,
            new_text: edit.replacement.clone(),
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

fn def_span_lo_usize(span: Span) -> Option<usize> {
    usize::try_from(span.lo).ok()
}

fn def_span_hi_usize(span: Span) -> Option<usize> {
    usize::try_from(span.hi).ok()
}

#[cfg(test)]
mod tests {
    use ipe_db::{IpeDatabase, ModuleOrigin, SourceFile, SourceRoot};
    use lsp_types::Url;

    use crate::offset::PositionEncoding;

    use super::{SymbolKind, ValidatedIdentifier, prepare_rename, rename};

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

    // ── ValidatedIdentifier boundary tests ───────────────────────────────────

    #[test]
    fn valid_value_identifiers_are_accepted() {
        for name in ["foo", "bar_baz", "_private", "x1", "camelCase"] {
            assert!(
                ValidatedIdentifier::parse(name, SymbolKind::Value).is_some(),
                "{name:?} should be a valid value identifier"
            );
        }
    }

    #[test]
    fn valid_type_identifiers_are_accepted() {
        for name in ["Foo", "MyType", "A1"] {
            assert!(
                ValidatedIdentifier::parse(name, SymbolKind::Type).is_some(),
                "{name:?} should be a valid type identifier"
            );
        }
    }

    #[test]
    fn illegal_names_are_rejected_for_value_rename() {
        // Spaces, leading digit, empty, operator chars, non-ASCII
        for name in ["foo bar", "1x", "", "foo.bar", "foo;drop", "café"] {
            assert!(
                ValidatedIdentifier::parse(name, SymbolKind::Value).is_none(),
                "{name:?} should be rejected for value rename"
            );
        }
    }

    #[test]
    fn keywords_are_rejected() {
        for kw in [
            "let", "if", "then", "else", "type", "case", "of", "in", "module", "import",
        ] {
            assert!(
                ValidatedIdentifier::parse(kw, SymbolKind::Value).is_none(),
                "keyword {kw:?} must be rejected"
            );
        }
    }

    #[test]
    fn wrong_case_class_is_rejected() {
        // Uppercase name for a value rename
        assert!(
            ValidatedIdentifier::parse("Foo", SymbolKind::Value).is_none(),
            "uppercase name must be rejected for value rename"
        );
        // Lowercase name for a type rename
        assert!(
            ValidatedIdentifier::parse("foo", SymbolKind::Type).is_none(),
            "lowercase name must be rejected for type rename"
        );
        // Wrong-case for mixed: "Foo bar" is doubly wrong
        assert!(
            ValidatedIdentifier::parse("Foo bar", SymbolKind::Value).is_none(),
            "Foo bar must be rejected for value rename"
        );
    }

    // ── Integration: rename with illegal names emits no edits ─────────────────

    fn do_rename(new_name: &str) -> Option<lsp_types::WorkspaceEdit> {
        let db = IpeDatabase::new();
        let helper = file(&db, &["Helper"], HELPER);
        let entry = file(&db, &["Main"], MAIN);
        let root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);
        rename(
            &db,
            root,
            entry,
            &["Main".to_owned()],
            &super::RenameRequest {
                byte: ref_byte(),
                new_name,
                encoding: PositionEncoding::Utf16,
            },
            &super::ModuleResolver {
                uri_of_module: &make_uri,
                text_of_module: &make_text,
            },
        )
    }

    #[test]
    fn rename_with_illegal_name_returns_none() {
        for bad in ["foo bar", "1x", "", "Uppercase", "let"] {
            assert!(
                do_rename(bad).is_none(),
                "rename to {bad:?} must return None (no edits)"
            );
        }
    }

    // ── Existing integration tests ─────────────────────────────────────────────

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
        let ws_edit = do_rename("four").expect("rename returned Some");

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

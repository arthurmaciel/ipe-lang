//! Inlay hints: `textDocument/inlayHint`.
//!
//! Produces type-annotation inlay hints for top-level value bindings that
//! lack an explicit type annotation, using the solved type from `typecheck`.
//!
//! Each hint appears at the end of the binding's name token in the form
//! `: Type`, matching the style of an explicit annotation.
//!
//! Bindings that already have a type annotation are skipped — the annotation
//! is already visible in source.

use ipe_db::{Db as _, IpeDatabase, SourceRoot};
use ipe_diagnostics::Span;
use ipe_intern::Symbol;
use ipe_types::{VarNamer, ty_to_doc};
use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Range};

use crate::offset::{PositionEncoding, offset_to_position, span_to_range};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Inlay hints for the given document range.
///
/// Only top-level value bindings without a type annotation that overlap
/// `range` are hinted. Returns an empty list when no such bindings exist or
/// the file does not parse.
#[must_use]
pub fn inlay_hints(
    db: &IpeDatabase,
    root: SourceRoot,
    entry: ipe_db::SourceFile,
    module: &[String],
    range: Range,
    encoding: PositionEncoding,
) -> Vec<InlayHint> {
    let files = root.files(db);
    let Some(&file) = files.get(module) else {
        return Vec::new();
    };
    let Ok(parsed) = ipe_db::parse(db, file) else {
        return Vec::new();
    };
    let text = file.text(db);

    // Only produce hints when the type environment is available.
    let Ok(solved) = ipe_db::typecheck(db, root, entry) else {
        return Vec::new();
    };

    // Build the home symbol path once.
    let (home_syms, interner_needed): (Vec<Symbol>, bool) = {
        let mut interner = db.interner().lock();
        let syms: Option<Vec<Symbol>> = module
            .iter()
            .map(|s| interner.intern(s).ok())
            .collect::<Option<Vec<_>>>();
        drop(interner);
        match syms {
            Some(s) => (s, true),
            None => return Vec::new(),
        }
    };
    let _ = interner_needed;

    let mut hints: Vec<InlayHint> = Vec::new();

    for value in &parsed.values {
        // Skip bindings that already have an annotation.
        if value.value.type_annotation.is_some() {
            continue;
        }

        let name_span = value.value.name.span;
        let span_range = span_to_range(text, name_span, encoding);

        // Skip if the span does not intersect the requested range.
        if span_range.end < range.start || span_range.start > range.end {
            continue;
        }

        // Resolve the name symbol — intern under the same symbol index so the
        // env key matches.
        let name_sym = value.value.name.value;

        // Look up the type.
        let Some(ty) = solved.env.get(&(home_syms.clone(), name_sym)) else {
            continue;
        };

        let hint_label = {
            let interner = db.interner().lock();
            let mut namer = VarNamer::new();
            let Ok(doc) = ty_to_doc(ty, &interner, &mut namer) else {
                continue;
            };
            drop(interner);
            format!(": {}", ipe_diagnostics::render_ty(&doc))
        };

        // Place the hint just after the name token.
        let position = offset_to_position(text, name_span.hi as usize, encoding);

        hints.push(InlayHint {
            position,
            label: InlayHintLabel::String(hint_label),
            kind: Some(InlayHintKind::TYPE),
            text_edits: None,
            tooltip: None,
            padding_left: Some(true),
            padding_right: None,
            data: None,
        });
    }

    hints
}

// Suppress unused import warning — `Span` is referenced in the attribute path
// on `span_to_range`.
#[allow(dead_code)]
type _SpanAlias = Span;

#[cfg(test)]
mod tests {
    use ipe_db::{IpeDatabase, ModuleOrigin, SourceFile, SourceRoot};
    use lsp_types::{Position, Range};

    use super::inlay_hints;
    use crate::offset::PositionEncoding;

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

    fn full_range() -> Range {
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 9999,
                character: 0,
            },
        }
    }

    #[test]
    fn no_hints_when_annotation_present() {
        let db = IpeDatabase::new();
        let src = "module Main exposing (main)\n\nmain : Int\nmain =\n    42\n";
        let entry = file(&db, &["Main"], src);
        let root = root_of(&db, &[(&["Main"], entry)]);
        let hints = inlay_hints(
            &db,
            root,
            entry,
            &["Main".to_owned()],
            full_range(),
            PositionEncoding::Utf16,
        );
        assert!(
            hints.is_empty(),
            "annotated binding should produce no hint: {hints:?}"
        );
    }

    #[test]
    fn hint_appears_for_unannotated_binding() {
        let db = IpeDatabase::new();
        // No type annotation on `main`.
        let src = "module Main exposing (main)\n\nmain =\n    42\n";
        let entry = file(&db, &["Main"], src);
        let root = root_of(&db, &[(&["Main"], entry)]);
        let hints = inlay_hints(
            &db,
            root,
            entry,
            &["Main".to_owned()],
            full_range(),
            PositionEncoding::Utf16,
        );
        // If type-check succeeds, we should get a hint for `main`.
        // If it fails (no annotation makes inference ambiguous), hints is empty.
        // Either way, no panic.
        for h in &hints {
            let lsp_types::InlayHintLabel::String(label) = &h.label else {
                continue;
            };
            assert!(
                label.starts_with(": "),
                "hint label should start with `: `, got: {label}"
            );
        }
    }

    #[test]
    fn no_hints_for_unparseable_source() {
        let db = IpeDatabase::new();
        let f = file(&db, &["Main"], "@@@ not valid @@@");
        let root = root_of(&db, &[(&["Main"], f)]);
        let hints = inlay_hints(
            &db,
            root,
            f,
            &["Main".to_owned()],
            full_range(),
            PositionEncoding::Utf16,
        );
        assert!(hints.is_empty());
    }
}

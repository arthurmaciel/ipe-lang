//! Document symbols: the parse tree's top-level declarations.
//!
//! Hierarchical (unions carry their constructors as children) and pure over
//! `parse` — an unparseable buffer yields an empty outline, never an error.

use ipe_db::{Db as _, IpeDatabase, SourceFile};
use ipe_diagnostics::Span;
use lsp_types::{DocumentSymbol, SymbolKind};

use crate::offset::{PositionEncoding, span_to_range};

/// The hierarchical outline of one module's top-level declarations, in
/// source order.
#[must_use]
pub fn document_symbols(
    db: &IpeDatabase,
    file: SourceFile,
    encoding: PositionEncoding,
) -> Vec<DocumentSymbol> {
    let Ok(module) = ipe_db::parse(db, file) else {
        return Vec::new();
    };
    let text = file.text(db);
    let interner = db.interner().lock();
    let resolve = |sym: ipe_intern::Symbol| interner.resolve(sym).unwrap_or("?").to_owned();

    let mut out: Vec<DocumentSymbol> = Vec::new();
    for value in &module.values {
        // A value's `Located` span is its NAME; the full range runs to the
        // body's end.
        let full = Span::new(
            value.value.name.span.lo,
            value.value.body.span.hi.max(value.span.hi),
        );
        out.push(symbol(
            resolve(value.value.name.value),
            SymbolKind::FUNCTION,
            full,
            value.value.name.span,
            Vec::new(),
            text,
            encoding,
        ));
    }
    for union in &module.unions {
        let children: Vec<DocumentSymbol> = union
            .value
            .ctors
            .iter()
            .map(|ctor| {
                symbol(
                    resolve(ctor.value.name),
                    SymbolKind::ENUM_MEMBER,
                    ctor.span,
                    ctor.span,
                    Vec::new(),
                    text,
                    encoding,
                )
            })
            .collect();
        out.push(symbol(
            resolve(union.value.name.value),
            SymbolKind::ENUM,
            union.span,
            union.value.name.span,
            children,
            text,
            encoding,
        ));
    }
    for alias in &module.aliases {
        out.push(symbol(
            resolve(alias.value.name.value),
            SymbolKind::STRUCT,
            alias.span,
            alias.value.name.span,
            Vec::new(),
            text,
            encoding,
        ));
    }
    out.sort_by_key(|s| (s.range.start.line, s.range.start.character));
    out
}

fn symbol(
    name: String,
    kind: SymbolKind,
    full: Span,
    selection: Span,
    children: Vec<DocumentSymbol>,
    text: &str,
    encoding: PositionEncoding,
) -> DocumentSymbol {
    let range = span_to_range(text, full, encoding);
    let mut selection_range = span_to_range(text, selection, encoding);
    // The protocol requires `selectionRange ⊆ range`; clamp a stray
    // selection (never expected, but totality over parse quirks) inside.
    if selection_range.start < range.start || selection_range.end > range.end {
        selection_range = range;
    }
    #[allow(deprecated)] // `deprecated` is a required-but-deprecated field.
    DocumentSymbol {
        name,
        detail: None,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children: if children.is_empty() {
            None
        } else {
            Some(children)
        },
    }
}

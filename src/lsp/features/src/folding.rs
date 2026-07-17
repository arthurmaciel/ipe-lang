//! Folding ranges: the import block plus every multi-line top-level
//! declaration. Pure over `parse` — an unparseable buffer folds nothing.

use ipe_db::{IpeDatabase, SourceFile};
use lsp_types::{FoldingRange, FoldingRangeKind};

use crate::offset::{PositionEncoding, offset_to_position};

/// The foldable regions of one module, in source order.
#[must_use]
pub fn folding_ranges(
    db: &IpeDatabase,
    file: SourceFile,
    encoding: PositionEncoding,
) -> Vec<FoldingRange> {
    let Ok(module) = ipe_db::parse(db, file) else {
        return Vec::new();
    };
    let text = file.text(db);
    let mut out: Vec<FoldingRange> = Vec::new();

    // The import block folds as one region (first to last import). A bare
    // `import Foo` may carry a synthetic (zero) exposing span — the `max`
    // against the name span keeps the bound on real source text.
    if let (Some(first), Some(last)) = (module.imports.first(), module.imports.last()) {
        let lo = first.name.span.lo;
        let hi = last.name.span.hi.max(last.exposing.span.hi);
        push_range(
            &mut out,
            text,
            lo,
            hi,
            Some(FoldingRangeKind::Imports),
            encoding,
        );
    }

    for value in &module.values {
        // A value's `Located` span is its NAME; the decl runs to the body's
        // end.
        let lo = value.value.name.span.lo;
        let hi = value.value.body.span.hi.max(value.span.hi);
        push_range(&mut out, text, lo, hi, None, encoding);
    }
    for union in &module.unions {
        push_range(&mut out, text, union.span.lo, union.span.hi, None, encoding);
    }
    for alias in &module.aliases {
        push_range(&mut out, text, alias.span.lo, alias.span.hi, None, encoding);
    }
    out.sort_by_key(|range| (range.start_line, range.end_line));
    out
}

/// Append a fold for `[lo, hi)` when it spans more than one line.
fn push_range(
    out: &mut Vec<FoldingRange>,
    text: &str,
    lo: u32,
    hi: u32,
    kind: Option<FoldingRangeKind>,
    encoding: PositionEncoding,
) {
    let start = offset_to_position(text, lo as usize, encoding);
    let end = offset_to_position(text, hi as usize, encoding);
    if end.line > start.line {
        out.push(FoldingRange {
            start_line: start.line,
            start_character: None,
            end_line: end.line,
            end_character: None,
            kind,
            collapsed_text: None,
        });
    }
}

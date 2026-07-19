//! Hover: the solved type of the innermost expression at a position.
//!
//! Reads `typecheck`'s home-keyed region map — the exact types
//! type-directed lowering consumes — so the hover can never disagree with
//! the compiler. On a program that does not type-check (or a position on no
//! expression) the answer is `None`, never a guess.

use ipe_db::{Db as _, IpeDatabase, SourceRoot};
use ipe_diagnostics::Span;

/// A successful hover: the rendered type plus the region it belongs to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HoverInfo {
    /// The type in Ipê surface syntax (e.g. `List (Maybe Int)`).
    pub ty: String,
    /// The source region carrying that type (byte offsets into the module).
    pub span: Span,
}

/// The type of the innermost solved region containing `byte` in `module_file`.
///
/// Reads the per-module `typecheck_module` projection — its `regions` map holds
/// only this module's home-scoped regions, so the lookup needs no home
/// comparison and the handler is unchanged the day the underlying solve becomes
/// genuinely per-module.
#[must_use]
pub fn hover(
    db: &IpeDatabase,
    root: SourceRoot,
    entry: ipe_db::SourceFile,
    module_file: ipe_db::SourceFile,
    byte: u32,
) -> Option<HoverInfo> {
    let types = ipe_db::typecheck_module(db, root, entry, module_file).ok()?;
    // Innermost wins: narrowest containing span, latest start as tiebreaker.
    let mut best: Option<(u32, u32)> = None; // (width, lo)
    for span in types.regions.keys() {
        if span.lo <= byte && byte < span.hi {
            let width = span.hi.saturating_sub(span.lo);
            if best.is_none_or(|(best_width, best_lo)| {
                width < best_width || (width == best_width && span.lo > best_lo)
            }) {
                best = Some((width, span.lo));
            }
        }
    }
    let (width, lo) = best?;
    let span = Span::new(lo, lo.saturating_add(width));
    let interner = db.interner().lock();
    let mut namer = ipe_types::VarNamer::new();
    let doc = types
        .regions
        .get(&span)
        .and_then(|ty| ipe_types::ty_to_doc(ty, &interner, &mut namer).ok());
    drop(interner);
    Some(HoverInfo {
        ty: ipe_diagnostics::render_ty(&doc?),
        span,
    })
}

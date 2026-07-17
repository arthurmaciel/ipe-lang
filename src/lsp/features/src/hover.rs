//! Hover: the solved type of the innermost expression at a position.
//!
//! Reads `typecheck`'s home-keyed region map — the exact types
//! type-directed lowering consumes — so the hover can never disagree with
//! the compiler. On a program that does not type-check (or a position on no
//! expression) the answer is `None`, never a guess.

use ipe_db::{Db as _, IpeDatabase, SourceRoot};
use ipe_diagnostics::Span;
use ipe_intern::Symbol;

/// A successful hover: the rendered type plus the region it belongs to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HoverInfo {
    /// The type in Ipê surface syntax (e.g. `List (Maybe Int)`).
    pub ty: String,
    /// The source region carrying that type (byte offsets into the module).
    pub span: Span,
}

/// The type of the innermost solved region containing `byte` in `module`.
#[must_use]
pub fn hover(
    db: &IpeDatabase,
    root: SourceRoot,
    entry: ipe_db::SourceFile,
    module: &[String],
    byte: u32,
) -> Option<HoverInfo> {
    let solved = ipe_db::typecheck(db, root, entry).ok()?;
    let mut interner = db.interner().lock();
    let home: Vec<Symbol> = module
        .iter()
        .map(|segment| interner.intern(segment))
        .collect::<Result<_, _>>()
        .ok()?;
    // Innermost wins: narrowest containing span, latest start as tiebreaker.
    let mut best: Option<(u32, u32)> = None; // (width, lo)
    for (h, span) in solved.regions.keys() {
        if *h == home && span.lo <= byte && byte < span.hi {
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
    let mut namer = ipe_types::VarNamer::new();
    let doc = solved
        .regions
        .get(&(home, span))
        .and_then(|ty| ipe_types::ty_to_doc(ty, &interner, &mut namer).ok());
    drop(interner);
    Some(HoverInfo {
        ty: ipe_diagnostics::render_ty(&doc?),
        span,
    })
}

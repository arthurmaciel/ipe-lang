//! `expected_type_at`: the type the surrounding context expects at a cursor.
//!
//! Reads `typecheck`'s home-keyed `expected` sidecar — the same additive map
//! constraint generation records at every contextual push-down site (a call
//! argument's declared parameter, a typed body's annotation return, an
//! `if`/`case` branch's shared result, a list/cons element). It is the source
//! of truth for type-directed completion: the candidate whose type unifies
//! with the expected type ranks first, and the expected type's own
//! constructors are surfaced. On a program that does not type-check, or a
//! cursor on no expecting context, the answer is `None` and completion
//! degrades to scope-only ranking — never a guess.

use ipe_db::{Db as _, IpeDatabase, SourceRoot};
use ipe_intern::Symbol;
use ipe_types::Ty;

/// The type expected at `byte` in `module`, if any context pushes one down.
///
/// Innermost wins: the narrowest containing recorded span (latest start as
/// tiebreaker), matching `hover`'s region selection. Returns the solved `Ty`
/// (already zonked by inference) so the caller can compare candidate types
/// against it without touching the solver.
#[must_use]
pub fn expected_type_at(
    db: &IpeDatabase,
    root: SourceRoot,
    entry: ipe_db::SourceFile,
    module: &[String],
    byte: u32,
) -> Option<Ty> {
    let solved = ipe_db::typecheck(db, root, entry).ok()?;
    let home: Vec<Symbol> = {
        let mut interner = db.interner().lock();
        module
            .iter()
            .map(|segment| interner.intern(segment))
            .collect::<Result<_, _>>()
            .ok()?
    };
    let mut best: Option<(u32, u32)> = None; // (width, lo)
    for (h, span) in solved.expected.keys() {
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
    let span = ipe_diagnostics::Span::new(lo, lo.saturating_add(width));
    solved.expected.get(&(home, span)).cloned()
}

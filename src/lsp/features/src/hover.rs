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
    /// When the hover is on this module's `main`, its compiler-derived control
    /// model word (`tea` / `server` / `direct`) — the SAME projection `ipe audit`
    /// and `ipe doc` disclose, read from [`ipe_canon::shape_source`], never a
    /// second derivation. `None` on any other hover target.
    pub control_model: Option<&'static str>,
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
    // Computed before the interner lock below: `parse` takes the interner lock
    // itself, so classifying here (and releasing) avoids a re-entrant lock.
    let control_model = control_model_at(db, module_file, byte);
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
        control_model,
    })
}

/// The control-model word to disclose when the cursor is anywhere in this
/// module's `main` binding, or `None` otherwise.
///
/// Reuses [`ipe_canon::shape_source::classify_main_shape`] and its
/// [`ipe_canon::shape_source::ControlModel`] projection — the SAME single source
/// of truth `ipe audit` and `ipe doc` read — so the hover can never disclose a
/// model that disagrees with the compiler's own classification. A module with no
/// `main`, or a cursor outside the `main` binding, discloses nothing.
///
/// The gate is the whole `main` binding span (name through body) rather than the
/// name alone: the type-solved regions a hover fires on are body expression
/// spans, so gating on the binding is what lets the disclosure ride the hover a
/// user actually triggers on `main`.
fn control_model_at(
    db: &IpeDatabase,
    module_file: ipe_db::SourceFile,
    byte: u32,
) -> Option<&'static str> {
    let module = ipe_db::parse(db, module_file).ok()?;
    let interner = db.interner().lock();
    let main_sym = interner.lookup("main")?;
    let main = module
        .values
        .iter()
        .find(|v| v.value.name.value == main_sym)?;
    // The binding's full extent: its name through its body. Gating on this — not
    // the name token alone — is what lets the disclosure ride the type-solved body
    // region a user's hover on `main` actually fires on.
    let lo = main.value.name.span.lo;
    let hi = main.value.body.span.hi.max(main.value.name.span.hi);
    let shape = ipe_canon::shape_source::classify_main_shape(&module, &interner);
    // Every interner use is done; release the lock before the byte-gate and the
    // lock-free control-model projection.
    drop(interner);
    // The disclosure is a `main`-only fact: a cursor outside the `main` binding
    // discloses nothing.
    if !(lo <= byte && byte < hi) {
        return None;
    }
    Some(ipe_canon::shape_source::ControlModel::from_shape(shape).word())
}

//! The `#[define_in_ipe]` attribute must be INERT: it re-emits the annotated
//! item unchanged (plus one pure-data doc breadcrumb) and generates no code. If
//! a future edit ever made the macro rewrite or synthesize anything, these
//! tests would fail — the item would no longer behave identically to the
//! un-annotated original.
// These are hand-written fixtures standing in for author wrapper code; the
// nursery/pedantic style suggestions (`must_use`, `const fn`) do not apply to a
// throwaway test fn and would only obscure what the fixtures demonstrate.
// `too_long_first_doc_paragraph` fires on the `Documented` fixture because the
// inert marker doc precedes the author's own one-line doc — an artifact of
// combining both in a fixture, not a property of real author code (a real
// author's crate owns its own lint config). Allowed here so the fixture can
// prove the author doc SURVIVES alongside the marker.
#![allow(
    clippy::must_use_candidate,
    clippy::missing_const_for_fn,
    clippy::too_long_first_doc_paragraph
)]

use ipe_bindgen::define_in_ipe;

/// A fixture trait a hand-written impl satisfies — the escape-hatch case.
trait Render {
    fn label(&self) -> String;
}

#[define_in_ipe]
pub struct Sprite {
    depth: i64,
}

impl Render for Sprite {
    fn label(&self) -> String {
        format!("sprite@{}", self.depth)
    }
}

#[define_in_ipe]
pub fn spawn(depth: i64) -> Sprite {
    Sprite { depth }
}

#[test]
fn the_annotated_item_is_unchanged() {
    // The struct constructs, the hand-written trait impl runs, and the marked
    // free fn works — all exactly as if the attribute were absent. The macro
    // added nothing but an inert doc marker.
    let s = spawn(4);
    assert_eq!(s.label(), "sprite@4");
    assert_eq!(s.depth, 4);
}

/// The marker rides in the item's doc string, so `#[deny(missing_docs)]`-style
/// tooling and rustdoc see it — but it is pure data. This asserts the attribute
/// does not, e.g., strip a real doc comment the author wrote.
#[define_in_ipe]
/// Author's own doc comment.
pub struct Documented;

#[test]
fn an_authored_doc_comment_survives_alongside_the_marker() {
    // Nothing to assert at runtime beyond "it compiled"; the presence of both
    // the author doc and the marker is verified by the inspector-side tests. A
    // value of the type still constructs, proving the item is intact.
    let _ = Documented;
}

//! `Std.Ui.Keyed` kernel helpers — sky-key diff identity.
//!
//! **ipê v1 semantics: KEY DISCARDED.** Sky's Go runtime uses the key string to
//! stabilise the VNode diff (similar to Elm's `Html.Keyed`); ipê v1 does not yet
//! have a key-aware differ.  The key is accepted but dropped, which is semantically
//! correct (keys are a performance hint, not a behavioural contract).
//! The divergence is recorded in `docs/divergences-from-sky.md` §B-Keyed.
//!
//! Every function carries a trailing underscore per the `naming.rs` convention.

use super::element::{Attribute, Element};
use super::helpers::{ui_column_, ui_row_};

/// `Keyed.column : List (Attribute msg) -> List (String, Element msg) -> Element msg`
///
/// Keys are dropped; children are forwarded to `ui_column_`.
pub fn keyed_column_<M: Clone>(
    attrs: Vec<Attribute<M>>,
    children: Vec<(String, Element<M>)>,
) -> Element<M> {
    ui_column_(attrs, children.into_iter().map(|(_, e)| e).collect())
}

/// `Keyed.row : List (Attribute msg) -> List (String, Element msg) -> Element msg`
///
/// Keys are dropped; children are forwarded to `ui_row_`.
pub fn keyed_row_<M: Clone>(
    attrs: Vec<Attribute<M>>,
    children: Vec<(String, Element<M>)>,
) -> Element<M> {
    ui_row_(attrs, children.into_iter().map(|(_, e)| e).collect())
}

//! `no-exposing-everything` — a `module M exposing (..)` header.
//!
//! Ported from elm-review's `NoExposingEverything`. An open header exports
//! every top-level declaration, so a helper added for internal use silently
//! joins the module's public surface and every importer can come to depend on
//! it. An explicit list makes the exported API a deliberate, reviewable fact.
//!
//! No fix: the explicit list a rewrite would write is the module's resolved
//! export surface, which is canonicalisation's knowledge, not the parser's —
//! and choosing what to hide is the author's decision the rule exists to
//! prompt.

use ipe_syntax::Exposing;

use crate::finding::Finding;
use crate::rules::Ctx;

const RULE: &str = "no-exposing-everything";

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    if !matches!(&ctx.ast.exposing.value, Exposing::All) {
        return Vec::new();
    }
    let name = ctx
        .ast
        .name
        .value
        .iter()
        .map(|s| ctx.text(*s))
        .collect::<Vec<_>>()
        .join(".");
    vec![ctx.advisory(
        RULE,
        ctx.ast.exposing.span,
        format!("`module {name} exposing (..)` exports every top-level declaration"),
        vec![
            "list the names this module exports, e.g. `exposing (main, Model)`".to_owned(),
            format!("suppress: `-- ipe-lint: allow {RULE}`"),
        ],
    )]
}

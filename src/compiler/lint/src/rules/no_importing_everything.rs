//! `no-importing-everything` — an `import M exposing (..)` declaration.
//!
//! Ported from elm-review's `NoImportingEverything`. An open import brings
//! every name the dependency exports into unqualified scope, so a reader cannot
//! tell where a bare name comes from, and a name the dependency later adds can
//! collide with a local one.
//!
//! The package manifest's `import Ipe.Package exposing (..)` is the mandated
//! form of the manifest DSL and is exempt. Project discovery never lints
//! `package.ipe`, but a single-file LSP session can, so the exemption keys on
//! the import itself rather than on how the file was found.
//!
//! No fix: the explicit list a rewrite would write needs the dependency's
//! resolved export surface, which is canonicalisation's knowledge, not the
//! parser's.

use ipe_syntax::Exposing;

use crate::finding::Finding;
use crate::rules::Ctx;
use crate::rules::rewrite::is_stdlib_root;

const RULE: &str = "no-importing-everything";

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    ctx.ast
        .imports
        .iter()
        .filter(|import| matches!(&import.exposing.value, Exposing::All))
        .filter_map(|import| {
            let path: Vec<&str> = import.name.value.iter().map(|s| ctx.text(*s)).collect();
            if is_manifest_dsl(&path) {
                return None;
            }
            let name = path.join(".");
            Some(ctx.advisory(
                RULE,
                import.exposing.span,
                format!("`import {name} exposing (..)` brings every exported name into scope"),
                vec![
                    "expose only the names you use, e.g. `exposing (map, Model)`, or refer to them qualified".to_owned(),
                    format!("suppress: `-- ipe-lint: allow {RULE}`"),
                ],
            ))
        })
        .collect()
}

/// True for the manifest DSL module `Ipe.Package`, in either root spelling.
fn is_manifest_dsl(path: &[&str]) -> bool {
    matches!(path, [root, "Package"] if is_stdlib_root(root))
}

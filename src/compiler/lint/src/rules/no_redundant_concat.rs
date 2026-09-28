//! `no-redundant-concat` — `List.concat [ xs ]` / `String.concat [ s ]` restate
//! their single element.
//!
//! Ported from elm-review's `NoRedundantConcat`. Flattening a one-element list
//! of lists (or of strings) is exactly that element; the `concat` call adds a
//! traversal without adding meaning. Only `List.concat` / `String.concat`
//! whose qualifier resolves to the stdlib `Ipe.List` / `Ipe.String`, applied
//! to a list literal with exactly one item, are matched.
//!
//! Fix: the matched span is replaced by the element's source, parenthesised
//! unless it is self-delimiting or a tight application standing where the
//! `concat` call stood. No fix is offered when the dropped `concat [ ]` text
//! holds a comment.

use ipe_syntax::Expr_;

use crate::finding::Finding;
use crate::rules::rewrite::{drops_comment, fragment_for, qualifier_is_stdlib, with_fix};
use crate::rules::{Ctx, is_source_call, visit_exprs};

const RULE: &str = "no-redundant-concat";

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    let mut findings = Vec::new();
    visit_exprs(ctx, &mut |expr| {
        let Expr_::Call(callee, args) = &expr.value else {
            return;
        };
        let Expr_::VarQual(module, name) = &callee.value else {
            return;
        };
        if ctx.text(*name) != "concat" || !is_source_call(expr) {
            return;
        }
        let Some(leaf) = ["List", "String"]
            .into_iter()
            .find(|leaf| qualifier_is_stdlib(ctx, *module, leaf))
        else {
            return;
        };
        let [arg] = args.as_slice() else {
            return;
        };
        let Expr_::List(items) = &arg.value else {
            return;
        };
        let [single] = items.as_slice() else {
            return;
        };
        let message = format!("`{leaf}.concat` of a single-element list restates that element");
        let suppress = format!("suppress: `-- ipe-lint: allow {RULE}`");
        let fix = fragment_for(ctx, expr, single)
            .filter(|_| !drops_comment(ctx, expr.span, &[single.span]));
        findings.push(match fix {
            Some(simpler) => with_fix(
                ctx,
                RULE,
                expr.span,
                message,
                vec![format!("write `{simpler}`"), suppress],
                simpler,
            ),
            None => ctx.advisory(
                RULE,
                expr.span,
                message,
                vec!["use the element itself".to_owned(), suppress],
            ),
        });
    });
    findings
}

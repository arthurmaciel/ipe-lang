//! `simplify-cons-append` — `[a] ++ xs` restates `a :: xs`.
//!
//! Ported from elm-review-simplify's `[ a ] ++ xs --> a :: xs` check. `++` on a
//! single-element list literal is exactly a `::`, without the extra list
//! wrapper. Only a single-operator `++` chain whose left operand is a
//! one-element list literal is matched. `++` and `::` share precedence 5,
//! right-associative, so the right operand keeps its meaning verbatim.
//!
//! Fix: the matched span is replaced by `a :: xs`, where `a` is parenthesised
//! unless it binds tighter than every operator (`[ x |> f ] ++ xs` becomes
//! `(x |> f) :: xs`), and the result keeps the match's own grouping parens. No
//! fix is offered when the dropped `[ ] ++` text holds a comment.

use ipe_syntax::Expr_;

use crate::finding::Finding;
use crate::rules::rewrite::{drops_comment, keep_wrapping, operand, with_fix};
use crate::rules::{Ctx, visit_exprs};

const RULE: &str = "simplify-cons-append";

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    let mut findings = Vec::new();
    visit_exprs(ctx, &mut |expr| {
        let Expr_::Binops(pairs, rhs) = &expr.value else {
            return;
        };
        let [(lhs, op)] = pairs.as_slice() else {
            return;
        };
        if ctx.text(op.value) != "++" {
            return;
        }
        let Expr_::List(items) = &lhs.value else {
            return;
        };
        let [single] = items.as_slice() else {
            return;
        };
        let message = "appending a single-element list restates a `::`".to_owned();
        let suppress = format!("suppress: `-- ipe-lint: allow {RULE}`");
        let tail = ctx.slice(rhs.span).trim();
        let fix = operand(ctx, single)
            .filter(|_| {
                !tail.is_empty() && !drops_comment(ctx, expr.span, &[single.span, rhs.span])
            })
            .map(|head| keep_wrapping(ctx, expr, format!("{head} :: {tail}")));
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
                vec!["cons the element with `::`".to_owned(), suppress],
            ),
        });
    });
    findings
}

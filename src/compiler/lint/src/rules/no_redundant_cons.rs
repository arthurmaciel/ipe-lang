//! `no-redundant-cons` — `x :: [ a, b ]` restates `[ x, a, b ]`.
//!
//! Ported from truqu/elm-review-noredundantcons. Consing onto a list literal
//! is exactly that longer list literal, without the extra `::`. Only a
//! single-operator `::` chain whose right operand is a list literal is
//! matched — consing onto a name, a call, or another `::` chain is left
//! alone, since those are not literal-onto-literal.
//!
//! Fix: the matched span is replaced by `[ x, ` followed by the list literal's
//! own source after its `[` (so comments inside the list survive); an empty
//! list yields `[ x ]`. A list element needs no parentheses and a list
//! literal is self-delimiting, so neither side is re-wrapped. No fix is offered
//! when the dropped ` :: ` text holds a comment, or when the list operand is
//! itself parenthesised.

use ipe_syntax::Expr_;

use crate::finding::Finding;
use crate::rules::rewrite::{drops_comment, with_fix};
use crate::rules::{Ctx, visit_exprs};

const RULE: &str = "no-redundant-cons";

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    let mut findings = Vec::new();
    visit_exprs(ctx, &mut |expr| {
        let Expr_::Binops(pairs, rhs) = &expr.value else {
            return;
        };
        let [(lhs, op)] = pairs.as_slice() else {
            return;
        };
        if ctx.text(op.value) != "::" {
            return;
        }
        let Expr_::List(items) = &rhs.value else {
            return;
        };
        let message = "consing onto a list literal restates a longer list literal".to_owned();
        let suppress = format!("suppress: `-- ipe-lint: allow {RULE}`");
        let head = ctx.slice(lhs.span).trim();
        let list = ctx.slice(rhs.span);
        let fix = list
            .strip_prefix('[')
            .filter(|rest| rest.ends_with(']'))
            .filter(|_| !head.is_empty())
            .filter(|_| !drops_comment(ctx, expr.span, &[lhs.span, rhs.span]))
            .map(|rest| {
                let separator = if items.is_empty() { " " } else { ", " };
                format!("[ {head}{separator}{}", rest.trim_start())
            });
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
                vec![
                    "write the element into the list literal".to_owned(),
                    suppress,
                ],
            ),
        });
    });
    findings
}

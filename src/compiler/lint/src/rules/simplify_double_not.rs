//! `simplify-double-not` — `not (not x)` restates `x`.
//!
//! Ported from elm-review-simplify's `not (not x) --> x` check. `not` negates
//! once; negating a negation is the identity, so the inner value is the whole
//! expression's value. Only a direct, fully-applied, source-written `not` call
//! is matched, and only in a module that never rebinds `not` (a top-level
//! `not`, a pattern variable, or an import that could expose one).
//!
//! Fix: the whole `not (not x)` span is replaced by `x`'s source, parenthesised
//! unless it is self-delimiting (see [`crate::rules::rewrite`]). No fix is
//! offered when the dropped `not (not …)` text holds a comment.

use ipe_syntax::{Expr, Expr_};

use crate::finding::Finding;
use crate::rules::rewrite::{binds_name, drops_comment, fragment_for, with_fix};
use crate::rules::{Ctx, is_source_call, visit_exprs};

const RULE: &str = "simplify-double-not";

/// The argument of a source-written one-argument call to unqualified `not`.
fn not_arg<'a>(ctx: &Ctx, expr: &'a Expr) -> Option<&'a Expr> {
    let Expr_::Call(callee, args) = &expr.value else {
        return None;
    };
    let [arg] = args.as_slice() else {
        return None;
    };
    let Expr_::VarLocal(name) = &callee.value else {
        return None;
    };
    (ctx.text(*name) == "not" && is_source_call(expr)).then_some(arg)
}

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    let mut findings = Vec::new();
    if binds_name(ctx, "not") {
        return findings;
    }
    visit_exprs(ctx, &mut |expr| {
        let Some(outer_arg) = not_arg(ctx, expr) else {
            return;
        };
        let Some(inner) = not_arg(ctx, outer_arg) else {
            return;
        };
        let message = "`not (not x)` restates `x`".to_owned();
        let suppress = format!("suppress: `-- ipe-lint: allow {RULE}`");
        let fix = fragment_for(ctx, expr, inner)
            .filter(|_| !drops_comment(ctx, expr.span, &[inner.span]));
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
                vec!["drop both `not`s".to_owned(), suppress],
            ),
        });
    });
    findings
}

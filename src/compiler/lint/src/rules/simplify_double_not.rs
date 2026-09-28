//! `simplify-double-not` — `not (not x)` restates `x`.
//!
//! Ported from elm-review-simplify's `not (not x) --> x` check. `not` negates
//! once; negating a negation is the identity, so the inner value is the whole
//! expression's value. Only a direct, fully-applied `not` call is matched — a
//! `not` reached through an alias or partial application is left alone.

use ipe_syntax::{Expr, Expr_};

use crate::finding::Finding;
use crate::rules::{Ctx, visit_exprs};

/// The argument of `expr`, when `expr` is a one-argument call to unqualified
/// `not`.
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
    (ctx.text(*name) == "not").then_some(arg)
}

/// `text` with every paren pair that encloses all of it removed: an argument's
/// span includes its source parens, which the bare replacement no longer needs.
fn without_enclosing_parens(mut text: &str) -> &str {
    while let Some(body) = text.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
        let mut depth = 0_usize;
        let balanced = body.chars().all(|c| {
            match c {
                '(' => depth += 1,
                ')' => match depth.checked_sub(1) {
                    Some(d) => depth = d,
                    None => return false,
                },
                _ => {}
            }
            true
        });
        if !balanced || depth != 0 {
            break;
        }
        text = body.trim();
    }
    text
}

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    let mut findings = Vec::new();
    visit_exprs(ctx, &mut |expr| {
        let Some(outer_arg) = not_arg(ctx, expr) else {
            return;
        };
        let Some(inner) = not_arg(ctx, outer_arg) else {
            return;
        };
        let simpler = without_enclosing_parens(ctx.slice(inner.span).trim());
        findings.push(ctx.advisory(
            "simplify-double-not",
            expr.span,
            "`not (not x)` restates `x`".to_owned(),
            vec![
                format!("write `{simpler}`"),
                "suppress: `-- ipe-lint: allow simplify-double-not`".to_owned(),
            ],
        ));
    });
    findings
}

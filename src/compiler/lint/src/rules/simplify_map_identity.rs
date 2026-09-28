//! `simplify-map-identity` — `List.map identity xs` restates `xs`.
//!
//! Ported from elm-review-simplify's `List.map identity xs --> xs` check.
//! Mapping the identity function over a list changes nothing; the mapped list
//! is the argument. The `List` qualifier must resolve to the stdlib `Ipe.List`
//! (an `import Foo as List` or a project `Utils.List` refuses), and `identity`
//! must be the ambient `Basics.identity` (a module that rebinds `identity`
//! refuses).
//!
//! Fix: the saturated `List.map identity xs` span is replaced by `xs`'s source
//! (parenthesised unless self-delimiting). The partial `List.map identity` is
//! reported without a fix: rewriting it to `identity` widens its type from
//! `List a -> List a` to `a -> a`, which is not an exact rewrite. No fix is
//! offered when the dropped `List.map identity` text holds a comment.

use ipe_syntax::{Expr, Expr_};

use crate::finding::Finding;
use crate::rules::rewrite::{
    binds_name, drops_comment, fragment_for, qualifier_is_stdlib, with_fix,
};
use crate::rules::{Ctx, is_source_call, visit_exprs};

const RULE: &str = "simplify-map-identity";

/// True when `expr` is a reference to the stdlib `Basics.identity`.
fn is_identity(ctx: &Ctx, expr: &Expr) -> bool {
    match &expr.value {
        Expr_::VarLocal(sym) => ctx.text(*sym) == "identity" && !binds_name(ctx, "identity"),
        Expr_::VarQual(module, sym) => {
            ctx.text(*sym) == "identity" && qualifier_is_stdlib(ctx, *module, "Basics")
        }
        _ => false,
    }
}

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    let mut findings = Vec::new();
    visit_exprs(ctx, &mut |expr| {
        let Expr_::Call(callee, args) = &expr.value else {
            return;
        };
        let Expr_::VarQual(module, name) = &callee.value else {
            return;
        };
        if ctx.text(*name) != "map"
            || !is_source_call(expr)
            || !qualifier_is_stdlib(ctx, *module, "List")
        {
            return;
        }
        let message = "`List.map identity` changes nothing".to_owned();
        let suppress = format!("suppress: `-- ipe-lint: allow {RULE}`");
        match args.as_slice() {
            [f] if is_identity(ctx, f) => findings.push(ctx.advisory(
                RULE,
                expr.span,
                message,
                vec![
                    "drop the `List.map identity` step (use `identity` where a function is required)"
                        .to_owned(),
                    suppress,
                ],
            )),
            [f, xs] if is_identity(ctx, f) => {
                let fix = fragment_for(ctx, expr, xs)
                    .filter(|_| !drops_comment(ctx, expr.span, &[xs.span]));
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
                        vec!["use the list itself".to_owned(), suppress],
                    ),
                });
            }
            _ => {}
        }
    });
    findings
}

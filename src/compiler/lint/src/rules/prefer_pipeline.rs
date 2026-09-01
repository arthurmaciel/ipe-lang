//! `prefer-pipeline` — a nested call chain that reads outside-in, rewritten to a
//! `|>` pipeline that reads left-to-right.
//!
//! `List.map fmt (List.filter live records)` threads `records` through two
//! transforms, but you read it from the outside in. The pipeline
//! `records |> List.filter live |> List.map fmt` reads in evaluation order.
//! Because `x |> f` desugars to exactly `f x`, the rewrite is provably
//! equivalent — so this rule, uniquely among the shipped set, carries a
//! semantics-preserving `--fix`.
//!
//! The rule is deliberately conservative: it fires only on the exact shape
//! `outer a… (inner b… subject)` — the last argument of an outer call is itself
//! a call with at least one argument — and rewrites by slicing the original
//! source spans, so the emitted pipeline reuses the author's own text verbatim.

use ipe_diagnostics::Span;
use ipe_syntax::{Expr, Expr_};

use crate::finding::{Finding, Fix};
use crate::rules::Ctx;

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    let mut findings = Vec::new();
    for value in &ctx.ast.values {
        walk(ctx, &value.value.body, &mut findings);
    }
    findings
}

/// Walk every sub-expression, testing each for the nested-call shape.
fn walk(ctx: &Ctx, expr: &Expr, out: &mut Vec<Finding>) {
    if let Some(finding) = as_pipeline_candidate(ctx, expr) {
        out.push(finding);
        // Do not also report the inner call as its own candidate — one rewrite
        // of the whole nest is enough, and re-running `--fix` catches any
        // remaining nesting on the next pass.
        return;
    }
    match &expr.value {
        Expr_::Call(callee, args) => {
            walk(ctx, callee, out);
            for arg in args {
                walk(ctx, arg, out);
            }
        }
        Expr_::Case(scrut, arms) => {
            walk(ctx, scrut, out);
            for (_pat, body) in arms {
                walk(ctx, body, out);
            }
        }
        Expr_::Lambda(_p, body) => walk(ctx, body, out),
        Expr_::Binops(pairs, last) => {
            for (operand, _op) in pairs {
                walk(ctx, operand, out);
            }
            walk(ctx, last, out);
        }
        Expr_::Let(bindings, body) => {
            for binding in bindings {
                walk(ctx, &binding.body, out);
            }
            walk(ctx, body, out);
        }
        Expr_::If(branches, otherwise) => {
            for (cond, body) in branches {
                walk(ctx, cond, out);
                walk(ctx, body, out);
            }
            walk(ctx, otherwise, out);
        }
        Expr_::Tuple(items) | Expr_::List(items) => {
            for item in items {
                walk(ctx, item, out);
            }
        }
        Expr_::Record(fields) | Expr_::Update(_, fields) => {
            for (_name, value) in fields {
                walk(ctx, value, out);
            }
        }
        Expr_::Access(base, _field) => walk(ctx, base, out),
        _ => {}
    }
}

/// When `expr` is `outer a… (inner b… subject)`, build the finding with its
/// pipeline `--fix`; otherwise `None`.
fn as_pipeline_candidate(ctx: &Ctx, expr: &Expr) -> Option<Finding> {
    let Expr_::Call(_outer_callee, outer_args) = &expr.value else {
        return None;
    };
    let last_outer = outer_args.last()?;
    // The rewrite only makes sense when the outer call has real arguments and
    // its last one is itself a call carrying at least one argument. A
    // parenthesised argument that is not a call, or an empty inner arg list,
    // is not this shape.
    let Expr_::Call(_inner_callee, inner_args) = &last_outer.value else {
        return None;
    };
    let subject = inner_args.last()?;

    // The three verbatim source slices, reused so the rewrite is the author's own
    // text re-threaded, never re-rendered.
    let subject_src = ctx.slice(subject.span).trim();
    let inner_prefix = call_prefix_src(ctx, last_outer)?;
    let outer_prefix = call_prefix_src(ctx, expr)?;
    if subject_src.is_empty() || inner_prefix.is_empty() || outer_prefix.is_empty() {
        return None;
    }

    let replacement = format!("{subject_src} |> {inner_prefix} |> {outer_prefix}");
    Some(Finding {
        rule: "prefer-pipeline",
        module: ctx.module.to_vec(),
        span: expr.span,
        message: "nested calls read outside-in; a `|>` pipeline reads left-to-right".to_owned(),
        help: vec![
            format!("rewrite as `{replacement}`"),
            "the pipeline is exactly equivalent — `x |> f` desugars to `f x`".to_owned(),
            "suppress: `-- ipe-lint: allow prefer-pipeline`".to_owned(),
        ],
        fix: Some(Fix {
            describe: "rewrite the nested call as a `|>` pipeline".to_owned(),
            span: expr.span,
            replacement,
        }),
    })
}

/// The source of a call with its LAST argument dropped — the "prefix" that a
/// pipeline stage applies to the threaded subject. For `List.filter live
/// records` this is `List.filter live`; for a single-argument call `f x` it is
/// the callee `f`.
///
/// Built by slicing from the call's start to the start of its last argument and
/// trimming, so it is the author's verbatim text. Returns `None` if the span
/// arithmetic would be degenerate.
fn call_prefix_src<'a>(ctx: &'a Ctx, call: &Expr) -> Option<&'a str> {
    let Expr_::Call(_callee, args) = &call.value else {
        return None;
    };
    let last = args.last()?;
    if last.span.lo <= call.span.lo {
        return None;
    }
    let prefix = ctx.slice(Span::new(call.span.lo, last.span.lo));
    // A call whose span begins with a grouping paren (`(List.filter live …)`)
    // carries that leading `(` into the slice; drop it and any trailing one.
    let trimmed = prefix
        .trim()
        .trim_start_matches('(')
        .trim_end_matches('(')
        .trim();
    Some(trimmed)
}

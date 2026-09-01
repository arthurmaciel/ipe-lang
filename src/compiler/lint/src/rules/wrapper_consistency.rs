//! `wrapper-consistency` — a shape that several sibling exported APIs wrap as a
//! newtype, but one leaves bare.
//!
//! When a module's exported signatures wrap the parameter named `port` as `Port`
//! in two places but pass it as a bare `Int` in a third, the bare one is almost
//! certainly an oversight — the sibling APIs already established the convention.
//! This is consistency/taste, not soundness, so it is a lint; and because the
//! fix changes a signature and its callers, it is advisory (no `--fix`).
//!
//! The evidence is drawn from the module under lint alone (its own sibling
//! APIs), keeping the rule self-contained and deterministic.

use std::collections::BTreeMap;

use crate::finding::Finding;
use crate::rules::{self, Ctx};

/// How a parameter name is annotated across the module's exported signatures.
#[derive(Default)]
struct Usage<'a> {
    /// Exported bindings where this name is a bare primitive: `(binding, span,
    /// bare-type)`.
    bare: Vec<(&'a str, ipe_diagnostics::Span, &'a str)>,
    /// The distinct newtype names this parameter name is wrapped as elsewhere.
    wrapped_as: std::collections::BTreeSet<&'a str>,
    /// The number of distinct exported bindings that wrap it.
    wrap_sites: usize,
}

/// A bare primitive is a candidate for wrapping; a wrapper is any other bare
/// type-constructor head (a user or stdlib newtype).
fn is_primitive(name: &str) -> bool {
    matches!(name, "Int" | "String" | "Float" | "Bool" | "Char")
}

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    let mut by_param: BTreeMap<&str, Usage> = BTreeMap::new();

    for (value, ann) in rules::annotated_values(ctx) {
        if !rules::is_exported(ctx, value.value.name.value) {
            continue;
        }
        let binding = ctx.text(value.value.name.value);
        let (params, _ret) = rules::flatten_arrow(&ann.value);
        for (idx, param_ty) in params.iter().enumerate() {
            let Some(param_name) = nth_param_name(ctx, value, idx) else {
                continue;
            };
            let Some(head) = rules::con_head_name(ctx, param_ty) else {
                continue;
            };
            let usage = by_param.entry(param_name).or_default();
            if is_primitive(head) {
                usage.bare.push((binding, ann.span, head));
            } else {
                usage.wrapped_as.insert(head);
                usage.wrap_sites += 1;
            }
        }
    }

    let mut findings = Vec::new();
    for (param_name, usage) in &by_param {
        // Fire only when a clear convention exists: at least two sibling
        // bindings wrap this parameter, under a single wrapper name, and at
        // least one binding leaves it bare.
        if usage.wrap_sites < 2 || usage.wrapped_as.len() != 1 || usage.bare.is_empty() {
            continue;
        }
        let wrapper = usage.wrapped_as.iter().next().copied().unwrap_or_default();
        for (binding, span, bare) in &usage.bare {
            findings.push(ctx.advisory(
                "wrapper-consistency",
                *span,
                format!(
                    "exported `{binding}` passes `{param_name}` as a bare `{bare}`, but sibling \
                     APIs wrap it as `{wrapper}`"
                ),
                vec![
                    format!(
                        "the convention is already set by {} sibling signature(s); wrap \
                         `{param_name}` as `{wrapper}` here too",
                        usage.wrap_sites
                    ),
                    "suppress: `-- ipe-lint: allow wrapper-consistency`".to_owned(),
                ],
            ));
        }
    }
    findings
}

fn nth_param_name<'a>(
    ctx: &'a Ctx,
    value: &ipe_diagnostics::Located<ipe_syntax::Value>,
    idx: usize,
) -> Option<&'a str> {
    use ipe_syntax::Pattern_;
    match value.value.patterns.get(idx).map(|p| &p.value) {
        Some(Pattern_::PVar(sym)) => Some(ctx.text(*sym)),
        _ => None,
    }
}

//! `adjacent-bools` — two or more adjacent `Bool` parameters at an exported
//! boundary, which call sites cannot tell apart (`render True False` — which
//! flag is which?).
//!
//! `Bool -> Bool -> Html` is sound and sometimes fine, so this can only be a
//! lint. The remedy — group the two adjacent bool args into a named record —
//! changes the exported signature and every call site. When the first pair of
//! adjacent bools is mechanically identifiable, the finding carries a
//! [`SigFix`] with a `GroupAdjacentBoolsIntoRecord` delta so `ipe lint --fix`
//! can rewrite call sites. Call sites using spread/lambda args are reported as
//! manual-review spans.

use ipe_canon::sig_delta::ShapeDelta;

use crate::finding::{Finding, SigFix};
use crate::rules::{self, Ctx};

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (value, ann) in rules::annotated_values(ctx) {
        if !rules::is_exported(ctx, value.value.name.value) {
            continue;
        }
        let (params, _ret) = rules::flatten_arrow(&ann.value);
        // Find the first run of two or more consecutive `Bool` parameters and
        // record its start index. Two or more in a row is the confusable case;
        // a single `Bool` (or two separated by another type) is fine.
        let mut run_start: Option<usize> = None;
        let mut run = 0usize;
        let mut max_run = 0usize;
        let mut first_pair_start: Option<usize> = None;
        for (i, param) in params.iter().enumerate() {
            if rules::con_head_name(ctx, param) == Some("Bool") {
                if run == 0 {
                    run_start = Some(i);
                }
                run += 1;
                if run >= 2 && first_pair_start.is_none() {
                    first_pair_start =
                        Some(run_start.unwrap_or_else(|| i.saturating_sub(run.saturating_sub(1))));
                }
                max_run = max_run.max(run);
            } else {
                run = 0;
                run_start = None;
            }
        }
        if max_run >= 2 {
            let name = ctx.text(value.value.name.value);
            // Derive parameter names for the first adjacent pair from the
            // binding's patterns, falling back to generic field names when the
            // pattern is not a plain variable (wildcard, destructure).
            let first = first_pair_start.unwrap_or(0);
            let field_a = nth_param_name(ctx, value, first)
                .map_or_else(|| format!("flag{first}"), str::to_owned);
            let field_b = nth_param_name(ctx, value, first + 1)
                .map_or_else(|| format!("flag{}", first + 1), str::to_owned);
            let sig_fix = SigFix {
                symbol_module: ctx.module.to_vec(),
                symbol_name: name.to_owned(),
                param_index: first,
                delta: ShapeDelta::GroupAdjacentBoolsIntoRecord {
                    first_arg_index: first,
                    field_a: field_a.clone(),
                    field_b: field_b.clone(),
                },
            };
            findings.push(ctx.with_sig_fix(
                "adjacent-bools",
                ann.span,
                format!(
                    "exported `{name}` has {max_run} adjacent `Bool` parameters call sites \
                     cannot tell apart"
                ),
                vec![
                    format!(
                        "`ipe lint --fix` groups call-site args into \
                         `{{ {field_a} = …, {field_b} = … }}`"
                    ),
                    "for a sum type per flag (`type Theme = Light | Dark`) suppress and \
                     refactor manually"
                        .to_owned(),
                    "suppress: `-- ipe-lint: allow adjacent-bools`".to_owned(),
                ],
                sig_fix,
            ));
        }
    }
    findings
}

/// The name of the `idx`-th parameter of `value`, when that parameter is a plain
/// variable pattern. A wildcard or destructuring pattern yields `None`.
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

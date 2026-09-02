//! `adjacent-bools` — two or more adjacent `Bool` parameters at an exported
//! boundary, which call sites cannot tell apart (`render True False` — which
//! flag is which?).
//!
//! `Bool -> Bool -> Html` is sound and sometimes fine, so this can only be a
//! lint. The remedy — a named two-case sum per flag (`type Theme = Light |
//! Dark`) or a config record — changes the exported signature and every call
//! site, so the rule is advisory: it reports and teaches, but carries no
//! `--fix`.

use crate::finding::Finding;
use crate::rules::{self, Ctx};

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (value, ann) in rules::annotated_values(ctx) {
        if !rules::is_exported(ctx, value.value.name.value) {
            continue;
        }
        let (params, _ret) = rules::flatten_arrow(&ann.value);
        // The longest run of consecutive `Bool` parameters. Two or more in a row
        // is the confusable case; a single `Bool` (or two separated by another
        // type) is fine.
        let mut run = 0usize;
        let mut max_run = 0usize;
        for param in &params {
            if rules::con_head_name(ctx, param) == Some("Bool") {
                run += 1;
                max_run = max_run.max(run);
            } else {
                run = 0;
            }
        }
        if max_run >= 2 {
            let name = ctx.text(value.value.name.value);
            findings.push(ctx.advisory(
                "adjacent-bools",
                ann.span,
                format!(
                    "exported `{name}` has {max_run} adjacent `Bool` parameters call sites \
                     cannot tell apart"
                ),
                vec![
                    "prefer a named two-case sum per flag (`type Theme = Light | Dark`) or a \
                     config record, so a call site reads `Dark` not `True`"
                        .to_owned(),
                    "suppress: `-- ipe-lint: allow adjacent-bools`".to_owned(),
                ],
            ));
        }
    }
    findings
}

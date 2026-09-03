//! `prim-param` — an exported signature takes a bare primitive at an API edge
//! where a domain newtype fits.
//!
//! `connect : String -> Int -> Task Error Conn` is perfectly sound — the type
//! checker must accept it. Whether *this* `Int` is a port that should be `Port`
//! is contextual and heuristic, which is exactly why it is a lint, not a
//! compiler error. The heuristic is deliberately conservative (false positives
//! erode trust faster than misses): it fires only when a parameter's NAME, at an
//! EXPORTED binding, matches a curated (name → newtype) pair AND the annotated
//! type is the bare primitive that newtype wraps.
//!
//! When the delta is mechanically applicable, the finding carries a [`SigFix`]
//! with a `WrapPrimitive` delta — `ipe lint --fix` threads the parse constructor
//! at every call site. Call sites the engine cannot safely rewrite are reported
//! as manual-review spans.

use ipe_canon::sig_delta::ShapeDelta;

use crate::finding::{Finding, SigFix};
use crate::rules::{self, Ctx};

/// One curated correspondence: a parameter-name substring, the bare primitive it
/// is typically (mis)typed as, the newtype that fits, and the parse to reach it.
struct Domain {
    /// A lower-cased parameter-name substring that signals the domain (`port`).
    name_hint: &'static str,
    /// The bare primitive constructor the parameter is annotated with (`Int`).
    bare: &'static str,
    /// The fully-qualified newtype that fits (`Ipe.Net.Port`).
    newtype: &'static str,
    /// The parse-at-the-edge constructor (`Port.fromInt`).
    parse: &'static str,
}

/// The curated table, seeded from the stdlib newtypes that exist today. Kept
/// small and specific — every entry names a real newtype with a real parser.
const DOMAINS: &[Domain] = &[
    Domain {
        name_hint: "port",
        bare: "Int",
        newtype: "Ipe.Net.Port",
        parse: "Port.fromInt",
    },
    Domain {
        name_hint: "url",
        bare: "String",
        newtype: "Ipe.Url.Url",
        parse: "Url.fromString",
    },
    Domain {
        name_hint: "href",
        bare: "String",
        newtype: "Ipe.Url.Url",
        parse: "Url.fromString",
    },
];

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (value, ann) in rules::annotated_values(ctx) {
        if !rules::is_exported(ctx, value.value.name.value) {
            continue;
        }
        let (params, _ret) = rules::flatten_arrow(&ann.value);
        // Parameter names come from the binding's argument patterns; a param
        // without a plain-variable pattern (a wildcard, a destructure) carries no
        // name to match on and is skipped.
        for (idx, param_ty) in params.iter().enumerate() {
            let Some(param_name) = nth_param_name(ctx, value, idx) else {
                continue;
            };
            let Some(bare) = rules::con_head_name(ctx, param_ty) else {
                continue;
            };
            let lower = param_name.to_ascii_lowercase();
            for domain in DOMAINS {
                if bare == domain.bare && lower.contains(domain.name_hint) {
                    let name = ctx.text(value.value.name.value);
                    // The parse constructor name is the unqualified part after
                    // the dot, e.g. `Port.fromInt` → `Port`. This is the
                    // constructor applied at each call site to wrap the arg.
                    let ctor = domain.parse.split('.').next_back().unwrap_or(domain.parse);
                    let sig_fix = SigFix {
                        symbol_module: ctx.module.to_vec(),
                        symbol_name: name.to_owned(),
                        param_index: idx,
                        delta: ShapeDelta::WrapPrimitive {
                            arg_index: idx,
                            ctor_name: ctor.to_owned(),
                        },
                    };
                    findings.push(ctx.with_sig_fix(
                        "prim-param",
                        ann.span,
                        format!(
                            "exported `{name}` takes a bare `{bare}` for `{param_name}` where \
                             `{}` fits",
                            domain.newtype
                        ),
                        vec![
                            format!(
                                "`{}` makes an invalid `{param_name}` unrepresentable; parse at \
                                 the edge with `{}`",
                                domain.newtype, domain.parse
                            ),
                            format!("`ipe lint --fix` wraps call-site arguments with `{ctor}`"),
                            "suppress: `-- ipe-lint: allow prim-param`".to_owned(),
                        ],
                        sig_fix,
                    ));
                    break;
                }
            }
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

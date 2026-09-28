//! `no-unused-parameters` and `no-unused-patterns` — a pattern variable that
//! its scope never reads.
//!
//! Ported from elm-review's `NoUnused.Parameters` and `NoUnused.Patterns`.
//! `no-unused-parameters` covers the parameters of a top-level function and of
//! a lambda; `no-unused-patterns` covers the variables a `case` arm, a
//! destructuring `let` binder, or a `do`-block `x <- task` binds. A plain
//! `let x = …` binder is `unused-bindings`' job and is not revisited here.
//!
//! A binder is flagged only when its scope — the function or lambda body, the
//! arm body, the `let` bindings and body, the rest of the `do` block — holds no
//! possible read of its name ([`referenced`]). The fixes:
//!
//! - an unused variable `x` becomes `_x`: renaming a binder no read reaches
//!   changes nothing, and the name survives for the rules that read parameter
//!   names ([`crate::rules::param_name`]). When `_x` is already read in the
//!   scope or bound beside `x`, the rename could capture or duplicate, so the
//!   fix is `_` instead: a variable pattern constrains nothing, so the wildcard
//!   types and matches identically.
//! - an unused `inner as name` alias drops ` as name`. The alias is the
//!   loosest wrapper of its pattern and the parser attaches it to the nearest
//!   cons tail, so the remaining `inner` parses to the same sub-tree.
//! - an unused record-pattern field and anything inside an or-pattern are
//!   reported without a fix: removing a field changes the inferred record
//!   type, and every or-pattern alternative must bind the same names.
//!
//! A `_`-prefixed name is intentionally unused and is never flagged. A binder
//! whose span does not slice to its own name is a desugared one the author did
//! not write, and is skipped.

use std::collections::HashSet;

use ipe_diagnostics::Span;
use ipe_syntax::{Expr, Expr_, Pattern, Pattern_};

use crate::finding::{Finding, Fix};
use crate::rules::Ctx;
use crate::rules::rewrite::with_fix;
use crate::rules::uses::referenced;

const PARAMETERS: &str = "no-unused-parameters";
const PATTERNS: &str = "no-unused-patterns";

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    let mut out = Vec::new();
    for value in &ctx.ast.values {
        let value = &value.value;
        let params: Vec<&Pattern> = value.patterns.iter().collect();
        report_group(ctx, PARAMETERS, &params, &params, &[&value.body], &mut out);
        walk(ctx, &value.body, &mut out);
    }
    out
}

/// Report every unused binder of each pattern nested in `expr`.
fn walk(ctx: &Ctx, expr: &Expr, out: &mut Vec<Finding>) {
    match &expr.value {
        Expr_::Lambda(params, body) => {
            // A `do`-block `x <- task` desugars to a zero-width lambda whose
            // parameter is the author's binder, not a lambda parameter.
            let rule = if expr.span.lo == expr.span.hi {
                PATTERNS
            } else {
                PARAMETERS
            };
            let params: Vec<&Pattern> = params.iter().collect();
            report_group(ctx, rule, &params, &params, &[body.as_ref()], out);
            walk(ctx, body, out);
        }
        Expr_::Case(scrut, arms) => {
            walk(ctx, scrut, out);
            for (pat, body) in arms {
                report_group(ctx, PATTERNS, &[pat], &[pat], &[body], out);
                walk(ctx, body, out);
            }
        }
        Expr_::Let(bindings, body) => {
            let scope: Vec<&Expr> = bindings
                .iter()
                .map(|binding| &binding.body)
                .chain(std::iter::once(body.as_ref()))
                .collect();
            let siblings: Vec<&Pattern> = bindings.iter().map(|binding| &binding.pat).collect();
            let targets: Vec<&Pattern> = siblings
                .iter()
                .copied()
                .filter(|pat| !matches!(pat.value, Pattern_::PVar(_)))
                .collect();
            report_group(ctx, PATTERNS, &targets, &siblings, &scope, out);
            for binding in bindings {
                walk(ctx, &binding.body, out);
            }
            walk(ctx, body, out);
        }
        Expr_::Call(callee, args) => {
            walk(ctx, callee, out);
            for arg in args {
                walk(ctx, arg, out);
            }
        }
        Expr_::Access(inner, _) => walk(ctx, inner, out),
        Expr_::Binops(pairs, last) => {
            for (operand, _op) in pairs {
                walk(ctx, operand, out);
            }
            walk(ctx, last, out);
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
        Expr_::VarLocal(_)
        | Expr_::VarQual(_, _)
        | Expr_::Int(_)
        | Expr_::Float(_)
        | Expr_::Str(_)
        | Expr_::MultilineStr { .. }
        | Expr_::Char(_)
        | Expr_::PathLit(_)
        | Expr_::Unit => {}
    }
}

/// The names a rename must not take: every read in the scope and every name
/// the binding group binds.
struct Taken<'a> {
    uses: HashSet<&'a str>,
    bound: HashSet<&'a str>,
}

impl Taken<'_> {
    /// The fix for the unused variable `name`: `_name` when that is free,
    /// otherwise `_`.
    fn replacement(&self, name: &str) -> String {
        let renamed = format!("_{name}");
        if self.uses.contains(renamed.as_str()) || self.bound.contains(renamed.as_str()) {
            "_".to_owned()
        } else {
            renamed
        }
    }
}

/// Report the unused binders of each of `targets`, patterns of one binding
/// group whose names are visible in `scope`.
///
/// `siblings` is the whole group — every parameter of a function, every
/// binder of a `let` — whose names a rename must not duplicate. The scope's
/// reads are collected once, and only when a target binds a name.
fn report_group(
    ctx: &Ctx,
    rule: &'static str,
    targets: &[&Pattern],
    siblings: &[&Pattern],
    scope: &[&Expr],
    out: &mut Vec<Finding>,
) {
    if !targets.iter().any(|pat| binds_name(pat)) {
        return;
    }
    let mut taken = Taken {
        uses: HashSet::new(),
        bound: HashSet::new(),
    };
    for expr in scope {
        referenced(ctx, expr, &mut taken.uses);
    }
    for pat in siblings {
        bound_names(ctx, pat, &mut taken.bound);
    }
    for pat in targets {
        report(ctx, rule, pat, &taken, true, out);
    }
}

/// Every name `pat` binds.
fn bound_names<'a>(ctx: &'a Ctx<'_>, pat: &'a Pattern, out: &mut HashSet<&'a str>) {
    match &pat.value {
        Pattern_::PVar(sym) => {
            out.insert(ctx.text(*sym));
        }
        Pattern_::PAlias(inner, alias) => {
            out.insert(ctx.text(alias.value));
            bound_names(ctx, inner, out);
        }
        Pattern_::PRecord(fields) => {
            out.extend(fields.iter().map(|field| ctx.text(field.value)));
        }
        Pattern_::PCtor(_, _, subs)
        | Pattern_::PTuple(subs)
        | Pattern_::PList(subs)
        | Pattern_::POr(subs) => {
            for sub in subs {
                bound_names(ctx, sub, out);
            }
        }
        Pattern_::PCons(head, tail) => {
            bound_names(ctx, head, out);
            bound_names(ctx, tail, out);
        }
        Pattern_::PAnything
        | Pattern_::PDebugAnything
        | Pattern_::PUnit
        | Pattern_::PInt(_)
        | Pattern_::PBool(_)
        | Pattern_::PChar(_)
        | Pattern_::PStr(_) => {}
    }
}

/// True when `pat` binds at least one name: a variable, an alias, or a record
/// field.
fn binds_name(pat: &Pattern) -> bool {
    match &pat.value {
        Pattern_::PVar(_) | Pattern_::PRecord(_) | Pattern_::PAlias(..) => true,
        Pattern_::PCtor(_, _, subs)
        | Pattern_::PTuple(subs)
        | Pattern_::PList(subs)
        | Pattern_::POr(subs) => subs.iter().any(binds_name),
        Pattern_::PCons(head, tail) => binds_name(head) || binds_name(tail),
        Pattern_::PAnything
        | Pattern_::PDebugAnything
        | Pattern_::PUnit
        | Pattern_::PInt(_)
        | Pattern_::PBool(_)
        | Pattern_::PChar(_)
        | Pattern_::PStr(_) => false,
    }
}

/// True when `name` is a flaggable binder that `uses` never reads.
fn is_unused(name: &str, uses: &HashSet<&str>) -> bool {
    !name.is_empty() && !name.starts_with('_') && !uses.contains(name)
}

/// Report the unused binders of `pat`; `fixable` is false beneath an
/// or-pattern, whose alternatives must keep binding the same names.
fn report(
    ctx: &Ctx,
    rule: &'static str,
    pat: &Pattern,
    taken: &Taken,
    fixable: bool,
    out: &mut Vec<Finding>,
) {
    let uses = &taken.uses;
    match &pat.value {
        Pattern_::PVar(sym) => {
            let name = ctx.text(*sym);
            if is_unused(name, uses) && ctx.slice(pat.span) == name {
                let fix = fixable.then(|| taken.replacement(name));
                out.push(unused_variable(ctx, rule, pat.span, name, fix));
            }
        }
        Pattern_::PAlias(inner, alias) => {
            report(ctx, rule, inner, taken, fixable, out);
            let name = ctx.text(alias.value);
            if is_unused(name, uses) && ctx.slice(alias.span) == name {
                out.push(unused_alias(
                    ctx, rule, inner.span, alias.span, name, fixable,
                ));
            }
        }
        Pattern_::PRecord(fields) => {
            for field in fields {
                let name = ctx.text(field.value);
                if is_unused(name, uses) && ctx.slice(field.span) == name {
                    out.push(ctx.advisory(
                        rule,
                        field.span,
                        format!("record field `{name}` is destructured but never used"),
                        vec![
                            format!("remove `{name}` from the record pattern"),
                            format!("suppress: `-- ipe-lint: allow {rule}`"),
                        ],
                    ));
                }
            }
        }
        Pattern_::POr(alts) => {
            // Every alternative binds the same names, so the first speaks for all.
            if let Some(first) = alts.first() {
                report(ctx, rule, first, taken, false, out);
            }
        }
        Pattern_::PCtor(_, _, subs) | Pattern_::PTuple(subs) | Pattern_::PList(subs) => {
            for sub in subs {
                report(ctx, rule, sub, taken, fixable, out);
            }
        }
        Pattern_::PCons(head, tail) => {
            report(ctx, rule, head, taken, fixable, out);
            report(ctx, rule, tail, taken, fixable, out);
        }
        Pattern_::PAnything
        | Pattern_::PDebugAnything
        | Pattern_::PUnit
        | Pattern_::PInt(_)
        | Pattern_::PBool(_)
        | Pattern_::PChar(_)
        | Pattern_::PStr(_) => {}
    }
}

/// The finding for an unused variable binder, rewritten to `fix` when given.
fn unused_variable(
    ctx: &Ctx,
    rule: &'static str,
    span: Span,
    name: &str,
    fix: Option<String>,
) -> Finding {
    let message = if rule == PARAMETERS {
        format!("parameter `{name}` is never used")
    } else {
        format!("`{name}` is bound by this pattern but never used")
    };
    let help = vec![
        format!("rename it `_{name}` to mark it intentionally unused, or replace it with `_`"),
        format!("suppress: `-- ipe-lint: allow {rule}`"),
    ];
    match fix {
        Some(replacement) => with_fix(ctx, rule, span, message, help, replacement),
        None => ctx.advisory(rule, span, message, help),
    }
}

/// The finding for an unused `inner as name` alias.
///
/// The fix deletes from the end of `inner` through the alias name, offered only
/// when that gap is exactly the `as` keyword and whitespace: no parenthesis, no
/// comment.
fn unused_alias(
    ctx: &Ctx,
    rule: &'static str,
    inner: Span,
    alias: Span,
    name: &str,
    fixable: bool,
) -> Finding {
    let mut finding = ctx.advisory(
        rule,
        alias,
        format!("alias `as {name}` is never used"),
        vec![
            format!("drop `as {name}`, or rename it `_{name}` to keep the name"),
            format!("suppress: `-- ipe-lint: allow {rule}`"),
        ],
    );
    let gap_is_keyword =
        inner.hi <= alias.lo && ctx.slice(Span::new(inner.hi, alias.lo)).trim() == "as";
    if fixable && gap_is_keyword {
        finding.fix = Some(Fix {
            describe: format!("drop `as {name}`"),
            span: Span::new(inner.hi, alias.hi),
            replacement: String::new(),
        });
    }
    finding
}

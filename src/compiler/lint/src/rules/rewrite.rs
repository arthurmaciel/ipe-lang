//! Shared guards that keep a simplifying rewrite semantics-preserving.
//!
//! A simplifying rule re-emits kept sub-expressions in a new shape. Three
//! properties make every such rewrite exact, and a rule offers a [`Fix`] only
//! when all three hold:
//!
//! 1. **Delimited fragments.** A kept fragment is re-emitted verbatim only when
//!    it cannot re-associate with its new neighbours; otherwise it is wrapped in
//!    parentheses ([`fragment_for`], [`operand`]).
//! 2. **No dropped comment.** The source text a rewrite deletes (everything in
//!    the matched span outside the kept fragments) holds no comment
//!    ([`drops_comment`]).
//! 3. **Provable binding.** A trigger name denotes the stdlib binding: the
//!    module never rebinds it ([`binds_name`]) and its qualifier resolves to
//!    the expected stdlib module ([`qualifier_is_stdlib`]).

use ipe_diagnostics::Span;
use ipe_intern::Symbol;
use ipe_syntax::{Exposed, Exposing, Expr, Expr_, Import, Pattern, Pattern_};

use crate::finding::{Finding, Fix};
use crate::rules::{Ctx, is_atom, is_source_call, visit_exprs};

/// True when the source of `expr` begins with a parenthesis the parser
/// unwrapped, so the node's span covers its own grouping parens.
fn is_wrapped(ctx: &Ctx, expr: &Expr) -> bool {
    let starts_paren = ctx.slice(expr.span).starts_with('(');
    match &expr.value {
        Expr_::Call(callee, _) => starts_paren && callee.span.lo > expr.span.lo,
        Expr_::Binops(pairs, _) => {
            starts_paren
                && pairs
                    .first()
                    .is_some_and(|(first, _)| first.span.lo > expr.span.lo)
        }
        Expr_::Lambda(..) | Expr_::If(..) | Expr_::Case(..) | Expr_::Let(..) => starts_paren,
        _ => false,
    }
}

/// True when `expr` reads the same in every expression position: a literal,
/// a name, a bracketed form, or a parenthesised group.
pub fn is_delimited(ctx: &Ctx, expr: &Expr) -> bool {
    is_atom(expr) || matches!(expr.value, Expr_::MultilineStr { .. }) || is_wrapped(ctx, expr)
}

/// True when `expr` is an application whose every argument is delimited, so it
/// binds tighter than any operator.
fn is_tight_call(ctx: &Ctx, expr: &Expr) -> bool {
    let Expr_::Call(callee, args) = &expr.value else {
        return false;
    };
    is_source_call(expr)
        && is_delimited(ctx, callee)
        && args.iter().all(|arg| is_delimited(ctx, arg))
}

/// The trimmed source of `expr`, or `None` when its span is empty.
fn source_of<'a>(ctx: &'a Ctx, expr: &Expr) -> Option<&'a str> {
    let text = ctx.slice(expr.span).trim();
    (!text.is_empty()).then_some(text)
}

/// The source of `kept`, parenthesised as needed to replace `matched`.
///
/// A delimited fragment fits anywhere. A tight application also fits where an
/// unwrapped application stood (an unparenthesised call is never a call
/// argument). Anything else is wrapped.
pub fn fragment_for(ctx: &Ctx, matched: &Expr, kept: &Expr) -> Option<String> {
    let text = source_of(ctx, kept)?;
    let matched_is_bare_call =
        matches!(matched.value, Expr_::Call(..)) && !is_wrapped(ctx, matched);
    if is_delimited(ctx, kept) || (matched_is_bare_call && is_tight_call(ctx, kept)) {
        Some(text.to_owned())
    } else {
        Some(format!("({text})"))
    }
}

/// The source of `kept` as the left operand of an operator, parenthesised
/// unless it binds tighter than every operator.
pub fn operand(ctx: &Ctx, kept: &Expr) -> Option<String> {
    let text = source_of(ctx, kept)?;
    if is_delimited(ctx, kept) || is_tight_call(ctx, kept) {
        Some(text.to_owned())
    } else {
        Some(format!("({text})"))
    }
}

/// `replacement` re-parenthesised when `matched` carried its own grouping
/// parens, for a replacement that is itself an operator chain.
pub fn keep_wrapping(ctx: &Ctx, matched: &Expr, replacement: String) -> String {
    if is_wrapped(ctx, matched) {
        format!("({replacement})")
    } else {
        replacement
    }
}

/// True when rewriting `outer` down to the `kept` spans would delete a comment
/// (or when the spans are not ordered inside `outer`, which refuses the fix).
pub fn drops_comment(ctx: &Ctx, outer: Span, kept: &[Span]) -> bool {
    let mut cursor = outer.lo;
    for span in kept {
        if span.lo < cursor || span.hi < span.lo || span.hi > outer.hi {
            return true;
        }
        if has_comment(ctx.slice(Span::new(cursor, span.lo))) {
            return true;
        }
        cursor = span.hi;
    }
    has_comment(ctx.slice(Span::new(cursor, outer.hi)))
}

/// True when dropped (non-literal) source text holds a line or block comment.
fn has_comment(text: &str) -> bool {
    text.contains("--") || text.contains("{-")
}

/// A finding carrying the machine-applicable rewrite of `span` to `replacement`.
pub fn with_fix(
    ctx: &Ctx,
    rule: &'static str,
    span: Span,
    message: String,
    help: Vec<String>,
    replacement: String,
) -> Finding {
    Finding {
        rule,
        module: ctx.module.to_vec(),
        span,
        message,
        help,
        fix: Some(Fix {
            describe: format!("rewrite as `{replacement}`"),
            span,
            replacement,
        }),
        sig_fix: None,
    }
}

/// True when `qualifier` names the stdlib module `Ipe.<leaf>` in this module.
///
/// An `as` alias wins over a bare leaf, mirroring canon's qualifier
/// resolution. An unimported qualifier is the ambient stdlib module. Any other
/// resolution (a project `Utils.List`, `import Foo as List`) refuses.
pub fn qualifier_is_stdlib(ctx: &Ctx, qualifier: Symbol, leaf: &str) -> bool {
    let qualifier = ctx.text(qualifier);
    let resolved = ctx
        .ast
        .imports
        .iter()
        .find(|i| i.alias.is_some_and(|a| ctx.text(a) == qualifier))
        .or_else(|| {
            ctx.ast.imports.iter().find(|i| {
                i.alias.is_none()
                    && i.name
                        .value
                        .last()
                        .is_some_and(|l| ctx.text(*l) == qualifier)
            })
        });
    resolved.map_or(qualifier == leaf, |import| {
        matches!(module_path(ctx, import).as_slice(), [root, l] if is_stdlib_root(root) && *l == leaf)
    })
}

/// The dotted segments of an import's module name.
fn module_path<'a>(ctx: &'a Ctx<'_>, import: &Import) -> Vec<&'a str> {
    import.name.value.iter().map(|s| ctx.text(*s)).collect()
}

/// True for the stdlib root segment, in either spelling.
pub fn is_stdlib_root(segment: &str) -> bool {
    segment == "Ipe" || segment == "Ipê"
}

/// True when the module could bind `name` to something other than the ambient
/// `Ipe.Basics` value: a top-level declaration, a pattern variable anywhere, an
/// import exposing it, or a wildcard import of any non-`Basics` module.
pub fn binds_name(ctx: &Ctx, name: &str) -> bool {
    let named = |sym: Symbol| ctx.text(sym) == name;
    if ctx.ast.values.iter().any(|v| named(v.value.name.value))
        || ctx.ast.foreigns.iter().any(|f| named(f.value.name.value))
    {
        return true;
    }
    let imported = ctx.ast.imports.iter().any(|import| {
        let is_basics = matches!(
            module_path(ctx, import).as_slice(),
            [root, "Basics"] if is_stdlib_root(root)
        );
        match &import.exposing.value {
            Exposing::All => !is_basics,
            Exposing::List(items) => {
                !is_basics
                    && items
                        .iter()
                        .any(|item| matches!(item.value, Exposed::Value(sym) if named(sym)))
            }
        }
    });
    if imported {
        return true;
    }
    let mut bound = ctx
        .ast
        .values
        .iter()
        .any(|v| v.value.patterns.iter().any(|p| pattern_binds(ctx, p, name)));
    visit_exprs(ctx, &mut |expr| {
        bound = bound
            || match &expr.value {
                Expr_::Lambda(params, _) => params.iter().any(|p| pattern_binds(ctx, p, name)),
                Expr_::Case(_, arms) => arms.iter().any(|(p, _)| pattern_binds(ctx, p, name)),
                Expr_::Let(bindings, _) => {
                    bindings.iter().any(|b| pattern_binds(ctx, &b.pat, name))
                }
                _ => false,
            };
    });
    bound
}

/// True when `pattern` binds a variable spelled `name`.
fn pattern_binds(ctx: &Ctx, pattern: &Pattern, name: &str) -> bool {
    match &pattern.value {
        Pattern_::PVar(sym) => ctx.text(*sym) == name,
        Pattern_::PAlias(inner, alias) => {
            ctx.text(alias.value) == name || pattern_binds(ctx, inner, name)
        }
        Pattern_::PRecord(fields) => fields.iter().any(|f| ctx.text(f.value) == name),
        Pattern_::PCtor(_, _, subs)
        | Pattern_::PTuple(subs)
        | Pattern_::PList(subs)
        | Pattern_::POr(subs) => subs.iter().any(|p| pattern_binds(ctx, p, name)),
        Pattern_::PCons(head, tail) => {
            pattern_binds(ctx, head, name) || pattern_binds(ctx, tail, name)
        }
        Pattern_::PAnything
        | Pattern_::PDebugAnything
        | Pattern_::PUnit
        | Pattern_::PInt(_)
        | Pattern_::PBool(_)
        | Pattern_::PChar(_)
        | Pattern_::PStr(_) => false,
    }
}

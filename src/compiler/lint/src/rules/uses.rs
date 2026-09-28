//! The local names an expression may read, over-approximated by text.
//!
//! A usage-counting rule may flag a binder as unused only when no read of it
//! can exist, so this collector errs toward "used": it records every
//! `VarLocal`, every record-update base (`{ r | … }` reads `r`), and every
//! identifier inside a triple-quoted string's `{{expr}}` interpolation, which
//! the canonicaliser — not the parser — turns into expressions. Names are
//! compared as text, so a shadowed inner use also counts; that can only hide a
//! finding, never invent one.

use std::collections::HashSet;

use ipe_syntax::{Expr, Expr_};

use crate::rules::Ctx;

/// Every local name `expr` may read, by text.
pub fn referenced<'a>(ctx: &'a Ctx<'_>, expr: &'a Expr, out: &mut HashSet<&'a str>) {
    match &expr.value {
        Expr_::VarLocal(sym) => {
            out.insert(ctx.text(*sym));
        }
        Expr_::MultilineStr { raw, .. } => interpolated(raw, out),
        Expr_::Call(callee, args) => {
            referenced(ctx, callee, out);
            for arg in args {
                referenced(ctx, arg, out);
            }
        }
        Expr_::Case(scrut, arms) => {
            referenced(ctx, scrut, out);
            for (_pat, body) in arms {
                referenced(ctx, body, out);
            }
        }
        Expr_::Lambda(_, body) | Expr_::Access(body, _) => referenced(ctx, body, out),
        Expr_::Binops(pairs, last) => {
            for (operand, _op) in pairs {
                referenced(ctx, operand, out);
            }
            referenced(ctx, last, out);
        }
        Expr_::Let(bindings, body) => {
            for binding in bindings {
                referenced(ctx, &binding.body, out);
            }
            referenced(ctx, body, out);
        }
        Expr_::If(branches, otherwise) => {
            for (cond, body) in branches {
                referenced(ctx, cond, out);
                referenced(ctx, body, out);
            }
            referenced(ctx, otherwise, out);
        }
        Expr_::Tuple(items) | Expr_::List(items) => {
            for item in items {
                referenced(ctx, item, out);
            }
        }
        Expr_::Record(fields) => {
            for (_name, value) in fields {
                referenced(ctx, value, out);
            }
        }
        Expr_::Update(base, fields) => {
            out.insert(ctx.text(base.value));
            for (_name, value) in fields {
                referenced(ctx, value, out);
            }
        }
        Expr_::VarQual(_, _)
        | Expr_::Int(_)
        | Expr_::Float(_)
        | Expr_::Str(_)
        | Expr_::Char(_)
        | Expr_::PathLit(_)
        | Expr_::Unit => {}
    }
}

/// Every identifier from the first `{{` of the raw string `raw` onward.
///
/// Canon's splitter decides which `{{ … }}` segments are expressions (escapes,
/// unclosed markers); counting every name after the first marker covers each
/// segment it can pick without reproducing that grammar.
fn interpolated<'a>(raw: &'a str, out: &mut HashSet<&'a str>) {
    let Some(tail) = raw.find("{{").and_then(|open| raw.get(open..)) else {
        return;
    };
    out.extend(
        tail.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '\''))
            .filter(|word| {
                word.chars()
                    .next()
                    .is_some_and(|c| c.is_alphabetic() || c == '_')
            }),
    );
}

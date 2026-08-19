//! AST walkers that produce the raw `AnnotatedToken` stream.
//!
//! Two walkers exist:
//!
//! * [`annotate_full`] — requires both the parse tree and the canonical AST;
//!   produces the richest classification (kernel, constructor, resolved def keys).
//! * [`annotate_syntax`] — parse tree only; produces coarser classification
//!   (`def` is always `None`, no kernel/constructor distinction).
//!
//! Both produce a sorted, deduplicated token stream.

use ipe_diagnostics::Span;
use ipe_intern::{Interner, Symbol};

use crate::{AnnotatedToken, DefKey, TokenClass};

// ---------------------------------------------------------------------------
// Internal raw token (before sorting / dedup)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Raw {
    byte_start: u32,
    byte_len: u32,
    class: TokenClass,
    def: Option<DefKey>,
}

impl Raw {
    fn push(out: &mut Vec<Self>, span: Span, class: TokenClass, def: Option<DefKey>) {
        if span.lo >= span.hi {
            return;
        }
        out.push(Self {
            byte_start: span.lo,
            byte_len: span.hi - span.lo,
            class,
            def,
        });
    }

    fn keyword(out: &mut Vec<Self>, span: Span) {
        Self::push(out, span, TokenClass::Keyword, None);
    }

    fn operator(out: &mut Vec<Self>, span: Span) {
        Self::push(out, span, TokenClass::Operator, None);
    }
}

// ---------------------------------------------------------------------------
// Full annotate (parse + canon)
// ---------------------------------------------------------------------------

pub fn annotate_full(
    syntax: &ipe_syntax::Module,
    canon: &ipe_canon::ast::Module,
    source: &str,
    interner: &Interner,
) -> Vec<AnnotatedToken> {
    let mut raw: Vec<Raw> = Vec::new();

    // Walk the canonical AST for semantically-classified tokens.
    canon_walk(&mut raw, syntax, canon, source, interner);

    finish(raw)
}

// ---------------------------------------------------------------------------
// Syntax-only annotate
// ---------------------------------------------------------------------------

pub fn annotate_syntax(
    syntax: &ipe_syntax::Module,
    source: &str,
    interner: &Interner,
) -> Vec<AnnotatedToken> {
    let mut raw: Vec<Raw> = Vec::new();

    syntax_walk(&mut raw, syntax, source, interner);

    finish(raw)
}

// ---------------------------------------------------------------------------
// Shared finish: sort + dedup
// ---------------------------------------------------------------------------

fn finish(mut raw: Vec<Raw>) -> Vec<AnnotatedToken> {
    raw.sort_by_key(|r| r.byte_start);
    // First token at each byte_start wins; duplicates removed.
    raw.dedup_by_key(|r| r.byte_start);
    raw.into_iter()
        .map(|r| AnnotatedToken {
            byte_start: r.byte_start,
            byte_len: r.byte_len,
            class: r.class,
            def: r.def,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Canonical-AST walk
// ---------------------------------------------------------------------------

fn canon_walk(
    out: &mut Vec<Raw>,
    syntax: &ipe_syntax::Module,
    canon: &ipe_canon::ast::Module,
    source: &str,
    interner: &Interner,
) {
    // Module keyword + name.
    if let Some(kw) = find_keyword(source, 0, "module") {
        Raw::keyword(out, kw);
    }
    Raw::push(out, syntax.name.span, TokenClass::Module, None);

    // Imports — syntactic (canon AST does not retain import list post-resolution).
    for imp in &syntax.imports {
        if let Some(kw) = find_keyword_before(source, imp.name.span.lo, "import") {
            Raw::keyword(out, kw);
        }
        Raw::push(out, imp.name.span, TokenClass::Module, None);
        push_exposing(out, &imp.exposing.value, interner);
    }

    // Union types — syntactic (canon carries unions with resolved ctors).
    for (syn_union, can_union) in syntax.unions.iter().zip(canon.unions.iter()) {
        if let Some(kw) = find_keyword_before(source, syn_union.value.name.span.lo, "type") {
            Raw::keyword(out, kw);
        }
        Raw::push(out, syn_union.value.name.span, TokenClass::Type, None);
        for var in &syn_union.value.vars {
            Raw::push(out, var.span, TokenClass::TypeVar, None);
        }
        for (syn_ctor, can_ctor) in syn_union.value.ctors.iter().zip(can_union.ctors.iter()) {
            let name_len = interner.resolve(can_ctor.name).map_or(0, byte_len_u32);
            let ctor_span = Span::new(syn_ctor.span.lo, syn_ctor.span.lo + name_len);
            let def = resolve_sym(can_union.name, interner)
                .zip(resolve_sym(can_ctor.name, interner))
                .map(|(type_str, name_str)| DefKey::Constructor {
                    module: interner_join(&can_union.home, interner),
                    type_name: type_str,
                    name: name_str,
                });
            Raw::push(out, ctor_span, TokenClass::Constructor, def);
        }
    }

    // Value bindings — walk canonical expr for semantic class.
    for (syn_val, can_def) in syntax.values.iter().zip(canon.defs.iter()) {
        // The binding name itself — top-level Function with DefKey::TopLevel.
        let def = resolve_sym(can_def.name().value, interner).map(|name_str| DefKey::TopLevel {
            module: interner_join(can_def.home(), interner),
            name: name_str,
        });
        Raw::push(out, syn_val.value.name.span, TokenClass::Function, def);
        // Type annotation (syntactic spans not always available).
        if let Some(ann) = &syn_val.value.type_annotation {
            push_type_annotation(out, &ann.value);
        }
        // Parameter patterns.
        for pat in &syn_val.value.patterns {
            push_syn_pattern(out, pat, interner);
        }
        // Body expression — walk the canonical expr for semantic richness.
        let body = match can_def {
            ipe_canon::ast::Def::Untyped { body, .. } | ipe_canon::ast::Def::Typed { body, .. } => {
                body
            }
        };
        canon_expr(out, body, interner);
    }
}

/// Walk a canonical expression, emitting semantically-classified tokens.
// The length comes from the exhaustive match over all Expr_ variants; splitting
// it would obscure the structure without reducing complexity.
#[allow(clippy::too_many_lines)]
fn canon_expr(out: &mut Vec<Raw>, expr: &ipe_canon::ast::Expr, interner: &Interner) {
    use ipe_canon::ast::Expr_;
    match &expr.value {
        Expr_::VarLocal(_) => {
            Raw::push(out, expr.span, TokenClass::Variable, None);
        }
        Expr_::VarTopLevel { module, name } => {
            let def = resolve_sym(*name, interner).map(|name_str| DefKey::TopLevel {
                module: interner_join(module, interner),
                name: name_str,
            });
            Raw::push(out, expr.span, TokenClass::Function, def);
        }
        Expr_::VarKernel {
            id: _,
            module,
            name,
        } => {
            let def = resolve_sym(*module, interner)
                .zip(resolve_sym(*name, interner))
                .map(|(module_str, name_str)| DefKey::Kernel {
                    module: module_str,
                    name: name_str,
                });
            Raw::push(out, expr.span, TokenClass::Kernel, def);
        }
        Expr_::VarCtor {
            home,
            type_name,
            name,
            ..
        } => {
            let def = resolve_sym(*type_name, interner)
                .zip(resolve_sym(*name, interner))
                .map(|(type_str, name_str)| DefKey::Constructor {
                    module: interner_join(home, interner),
                    type_name: type_str,
                    name: name_str,
                });
            Raw::push(out, expr.span, TokenClass::Constructor, def);
        }
        Expr_::Int(_) | Expr_::Float(_) => {
            Raw::push(out, expr.span, TokenClass::Number, None);
        }
        Expr_::Str(_) | Expr_::Char(_) | Expr_::PathLit(_) => {
            Raw::push(out, expr.span, TokenClass::StringLit, None);
        }
        Expr_::Unit => {}
        Expr_::Call(f, args) => {
            canon_expr(out, f, interner);
            for a in args {
                canon_expr(out, a, interner);
            }
        }
        Expr_::ForeignCall { args, .. } => {
            for a in args {
                canon_expr(out, a, interner);
            }
        }
        Expr_::Case(scrutinee, branches) => {
            canon_expr(out, scrutinee, interner);
            for b in branches {
                canon_pattern(out, &b.pat, interner);
                canon_expr(out, &b.body, interner);
            }
        }
        Expr_::Lambda(params, body) => {
            for p in params {
                canon_pattern(out, p, interner);
            }
            canon_expr(out, body, interner);
        }
        Expr_::Binop {
            op: _,
            home,
            func,
            lhs,
            rhs,
        } => {
            // Best-effort operator span: the op symbol lies between lhs and rhs.
            // Span::new normalises lo > hi to a zero-width span, which Raw::push
            // drops; the operator token is simply absent in that degenerate case.
            let op_span = Span::new(lhs.span.hi, rhs.span.lo);
            let def = resolve_sym(*home, interner)
                .zip(resolve_sym(*func, interner))
                .map(|(module_str, name_str)| DefKey::Kernel {
                    module: module_str,
                    name: name_str,
                });
            Raw::push(out, op_span, TokenClass::Operator, def);
            canon_expr(out, lhs, interner);
            canon_expr(out, rhs, interner);
        }
        Expr_::Let(bindings, body) => {
            for b in &bindings[..] {
                canon_pattern(out, &b.pat, interner);
                canon_expr(out, &b.body, interner);
            }
            canon_expr(out, body, interner);
        }
        Expr_::If(branches, else_) => {
            for (cond, then_) in branches {
                canon_expr(out, cond, interner);
                canon_expr(out, then_, interner);
            }
            canon_expr(out, else_, interner);
        }
        Expr_::Tuple(elems) | Expr_::List(elems) => {
            for e in elems {
                canon_expr(out, e, interner);
            }
        }
        Expr_::Cons(h, t) => {
            canon_expr(out, h, interner);
            canon_expr(out, t, interner);
        }
        Expr_::Record(fields) => {
            for (_, v) in fields {
                canon_expr(out, v, interner);
            }
        }
        Expr_::Access(rec, _field) => {
            canon_expr(out, rec, interner);
        }
        Expr_::Update(base, fields) => {
            Raw::push(out, base.span, TokenClass::Variable, None);
            for (_, v) in fields {
                canon_expr(out, v, interner);
            }
        }
    }
}

/// Walk a canonical pattern, emitting semantically-classified tokens.
fn canon_pattern(out: &mut Vec<Raw>, pat: &ipe_canon::ast::Pattern, interner: &Interner) {
    use ipe_canon::ast::Pattern_;
    match &pat.value {
        Pattern_::PAnything => {}
        Pattern_::PVar(_) => {
            Raw::push(out, pat.span, TokenClass::Variable, None);
        }
        Pattern_::PCtor {
            home,
            type_name,
            name,
            args,
            ..
        } => {
            let name_len = interner.resolve(*name).map_or(0, byte_len_u32);
            let ctor_span = Span::new(pat.span.lo, pat.span.lo + name_len);
            let def = resolve_sym(*type_name, interner)
                .zip(resolve_sym(*name, interner))
                .map(|(type_str, name_str)| DefKey::Constructor {
                    module: interner_join(home, interner),
                    type_name: type_str,
                    name: name_str,
                });
            Raw::push(out, ctor_span, TokenClass::Constructor, def);
            for a in args {
                canon_pattern(out, a, interner);
            }
        }
        Pattern_::PTuple(elems) | Pattern_::PList(elems) => {
            for e in elems {
                canon_pattern(out, e, interner);
            }
        }
        Pattern_::PRecord(fields) => {
            for f in fields {
                Raw::push(out, f.span, TokenClass::Variable, None);
            }
        }
        Pattern_::PInt(_) => {
            Raw::push(out, pat.span, TokenClass::Number, None);
        }
        Pattern_::PBool(_) => {
            Raw::push(out, pat.span, TokenClass::Constructor, None);
        }
        Pattern_::PChar(_) | Pattern_::PStr(_) => {
            Raw::push(out, pat.span, TokenClass::StringLit, None);
        }
        Pattern_::PAlias(inner, name) => {
            canon_pattern(out, inner, interner);
            Raw::push(out, name.span, TokenClass::Variable, None);
        }
        Pattern_::PCons(h, t) => {
            canon_pattern(out, h, interner);
            canon_pattern(out, t, interner);
        }
        Pattern_::POr(alts) => {
            for a in alts {
                canon_pattern(out, a, interner);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Syntax-only walk (parse tree, no canon)
// ---------------------------------------------------------------------------

fn syntax_walk(out: &mut Vec<Raw>, syntax: &ipe_syntax::Module, source: &str, interner: &Interner) {
    if let Some(kw) = find_keyword(source, 0, "module") {
        Raw::keyword(out, kw);
    }
    Raw::push(out, syntax.name.span, TokenClass::Module, None);

    for imp in &syntax.imports {
        if let Some(kw) = find_keyword_before(source, imp.name.span.lo, "import") {
            Raw::keyword(out, kw);
        }
        Raw::push(out, imp.name.span, TokenClass::Module, None);
        push_exposing(out, &imp.exposing.value, interner);
    }

    for syn_union in &syntax.unions {
        if let Some(kw) = find_keyword_before(source, syn_union.value.name.span.lo, "type") {
            Raw::keyword(out, kw);
        }
        Raw::push(out, syn_union.value.name.span, TokenClass::Type, None);
        for var in &syn_union.value.vars {
            Raw::push(out, var.span, TokenClass::TypeVar, None);
        }
        for ctor in &syn_union.value.ctors {
            let name_len = interner.resolve(ctor.value.name).map_or(0, byte_len_u32);
            let span = Span::new(ctor.span.lo, ctor.span.lo + name_len);
            Raw::push(out, span, TokenClass::Constructor, None);
        }
    }

    for syn_val in &syntax.values {
        Raw::push(out, syn_val.value.name.span, TokenClass::Function, None);
        if let Some(ann) = &syn_val.value.type_annotation {
            push_type_annotation(out, &ann.value);
        }
        for pat in &syn_val.value.patterns {
            push_syn_pattern(out, pat, interner);
        }
        push_syn_expr(out, &syn_val.value.body, interner);
    }
}

// ---------------------------------------------------------------------------
// Syntax-tree helpers (shared between full and syntax-only walkers)
// ---------------------------------------------------------------------------

fn push_exposing(out: &mut Vec<Raw>, exposing: &ipe_syntax::Exposing, _interner: &Interner) {
    match exposing {
        ipe_syntax::Exposing::All => {}
        ipe_syntax::Exposing::List(items) => {
            for item in items {
                match &item.value {
                    ipe_syntax::Exposed::Value(_) => {
                        Raw::push(out, item.span, TokenClass::Function, None);
                    }
                    ipe_syntax::Exposed::Type(_, _) => {
                        Raw::push(out, item.span, TokenClass::Type, None);
                    }
                }
            }
        }
    }
}

/// Type annotation spans are not fully tracked by the parser yet; this is a
/// named seam that gains richer output when span tracking lands.
const fn push_type_annotation(_out: &mut Vec<Raw>, _ty: &ipe_syntax::TypeAnnotation) {}

fn push_syn_pattern(out: &mut Vec<Raw>, pat: &ipe_syntax::Pattern, interner: &Interner) {
    match &pat.value {
        ipe_syntax::Pattern_::PAnything => {}
        ipe_syntax::Pattern_::PVar(_) => {
            Raw::push(out, pat.span, TokenClass::Variable, None);
        }
        ipe_syntax::Pattern_::PCtor(name, _segs, args) => {
            let name_len = interner.resolve(*name).map_or(0, byte_len_u32);
            let span = Span::new(pat.span.lo, pat.span.lo + name_len);
            Raw::push(out, span, TokenClass::Constructor, None);
            for a in args {
                push_syn_pattern(out, a, interner);
            }
        }
        ipe_syntax::Pattern_::PTuple(elems) | ipe_syntax::Pattern_::PList(elems) => {
            for e in elems {
                push_syn_pattern(out, e, interner);
            }
        }
        ipe_syntax::Pattern_::PRecord(fields) => {
            for f in fields {
                Raw::push(out, f.span, TokenClass::Variable, None);
            }
        }
        ipe_syntax::Pattern_::PInt(_) => {
            Raw::push(out, pat.span, TokenClass::Number, None);
        }
        ipe_syntax::Pattern_::PBool(_) => {
            Raw::push(out, pat.span, TokenClass::Constructor, None);
        }
        ipe_syntax::Pattern_::PChar(_) | ipe_syntax::Pattern_::PStr(_) => {
            Raw::push(out, pat.span, TokenClass::StringLit, None);
        }
        ipe_syntax::Pattern_::PAlias(inner, name) => {
            push_syn_pattern(out, inner, interner);
            Raw::push(out, name.span, TokenClass::Variable, None);
        }
        ipe_syntax::Pattern_::PCons(h, t) => {
            push_syn_pattern(out, h, interner);
            push_syn_pattern(out, t, interner);
        }
        ipe_syntax::Pattern_::POr(alts) => {
            for a in alts {
                push_syn_pattern(out, a, interner);
            }
        }
    }
}

fn push_syn_expr(out: &mut Vec<Raw>, expr: &ipe_syntax::Expr, interner: &Interner) {
    match &expr.value {
        ipe_syntax::Expr_::VarLocal(_) => {
            Raw::push(out, expr.span, TokenClass::Variable, None);
        }
        ipe_syntax::Expr_::VarQual(_, _) => {
            Raw::push(out, expr.span, TokenClass::Function, None);
        }
        ipe_syntax::Expr_::Int(_) | ipe_syntax::Expr_::Float(_) => {
            Raw::push(out, expr.span, TokenClass::Number, None);
        }
        ipe_syntax::Expr_::Str(_)
        | ipe_syntax::Expr_::MultilineStr { .. }
        | ipe_syntax::Expr_::PathLit(_)
        | ipe_syntax::Expr_::Char(_) => {
            Raw::push(out, expr.span, TokenClass::StringLit, None);
        }
        ipe_syntax::Expr_::Unit => {}
        ipe_syntax::Expr_::Call(f, args) => {
            push_syn_expr(out, f, interner);
            for a in args {
                push_syn_expr(out, a, interner);
            }
        }
        ipe_syntax::Expr_::Binops(pairs, last) => {
            for (lhs, op) in pairs {
                push_syn_expr(out, lhs, interner);
                Raw::operator(out, op.span);
            }
            push_syn_expr(out, last, interner);
        }
        ipe_syntax::Expr_::Lambda(params, body) => {
            for p in params {
                push_syn_pattern(out, p, interner);
            }
            push_syn_expr(out, body, interner);
        }
        ipe_syntax::Expr_::Let(bindings, body) => {
            for b in bindings {
                push_syn_pattern(out, &b.pat, interner);
                push_syn_expr(out, &b.body, interner);
            }
            push_syn_expr(out, body, interner);
        }
        ipe_syntax::Expr_::If(branches, else_) => {
            for (cond, then_) in branches {
                push_syn_expr(out, cond, interner);
                push_syn_expr(out, then_, interner);
            }
            push_syn_expr(out, else_, interner);
        }
        ipe_syntax::Expr_::Case(scrutinee, branches) => {
            push_syn_expr(out, scrutinee, interner);
            for (pat, body) in branches {
                push_syn_pattern(out, pat, interner);
                push_syn_expr(out, body, interner);
            }
        }
        ipe_syntax::Expr_::Tuple(elems) | ipe_syntax::Expr_::List(elems) => {
            for e in elems {
                push_syn_expr(out, e, interner);
            }
        }
        ipe_syntax::Expr_::Record(fields) => {
            for (name, val) in fields {
                Raw::push(out, name.span, TokenClass::Variable, None);
                push_syn_expr(out, val, interner);
            }
        }
        ipe_syntax::Expr_::Access(rec, field) => {
            push_syn_expr(out, rec, interner);
            Raw::push(out, field.span, TokenClass::Variable, None);
        }
        ipe_syntax::Expr_::Update(base, fields) => {
            // `base` is a `Located<Symbol>` (bare variable name).
            Raw::push(out, base.span, TokenClass::Variable, None);
            for (name, val) in fields {
                Raw::push(out, name.span, TokenClass::Variable, None);
                push_syn_expr(out, val, interner);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Keyword scanning
// ---------------------------------------------------------------------------

fn find_keyword(source: &str, from: usize, kw: &str) -> Option<Span> {
    let pos = source.get(from..)?.find(kw)?;
    let abs = from + pos;
    let before_ok = abs == 0
        || source
            .as_bytes()
            .get(abs - 1)
            .is_none_or(u8::is_ascii_whitespace);
    let after_ok = source
        .as_bytes()
        .get(abs + kw.len())
        .is_none_or(|b| b.is_ascii_whitespace() || *b == b'(');
    if before_ok && after_ok {
        Some(Span::new(offset_u32(abs), offset_u32(abs + kw.len())))
    } else {
        None
    }
}

/// Scan backwards from `before_byte` up to 64 bytes to find a keyword.
fn find_keyword_before(source: &str, before_byte: u32, kw: &str) -> Option<Span> {
    let window_start = (before_byte as usize).saturating_sub(64);
    let window = source.get(window_start..before_byte as usize)?;
    let rel = window.rfind(kw)?;
    let abs = window_start + rel;
    let before_ok = abs == 0
        || source
            .as_bytes()
            .get(abs - 1)
            .is_none_or(u8::is_ascii_whitespace);
    let after_ok = source
        .as_bytes()
        .get(abs + kw.len())
        .is_none_or(|b| b.is_ascii_whitespace() || *b == b'(');
    if before_ok && after_ok {
        Some(Span::new(offset_u32(abs), offset_u32(abs + kw.len())))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn byte_len_u32(s: &str) -> u32 {
    u32::try_from(s.len()).unwrap_or(u32::MAX)
}

fn offset_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn interner_join(syms: &[Symbol], interner: &Interner) -> String {
    syms.iter()
        .map(|s| interner.resolve(*s).unwrap_or("?"))
        .collect::<Vec<_>>()
        .join(".")
}

/// Resolve a single symbol to an owned `String`, returning `None` when the
/// symbol is not present in `interner`.  A `None` result means the symbol
/// does not belong to this interner — an internal-consistency bug.  Callers
/// treat `None` as "emit no `def`" rather than forging an empty key.
fn resolve_sym(sym: Symbol, interner: &Interner) -> Option<String> {
    interner.resolve(sym).map(str::to_owned)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ipe_intern::Interner;
    use ipe_parse::parse_module;

    fn parse(src: &str) -> (ipe_syntax::Module, Interner) {
        let mut i = Interner::default();
        let m = parse_module(src, &mut i).expect("parse");
        (m, i)
    }

    #[test]
    fn finish_sorts_and_deduplicates() {
        let raw = vec![
            Raw {
                byte_start: 10,
                byte_len: 3,
                class: TokenClass::Keyword,
                def: None,
            },
            Raw {
                byte_start: 0,
                byte_len: 6,
                class: TokenClass::Keyword,
                def: None,
            },
            Raw {
                byte_start: 0,
                byte_len: 6,
                class: TokenClass::Module,
                def: None,
            },
        ];
        let result = finish(raw);
        assert_eq!(result.len(), 2, "dedup removes second token at byte 0");
        let first = result.first().expect("first token");
        let second = result.get(1).expect("second token");
        assert_eq!(first.byte_start, 0);
        assert_eq!(second.byte_start, 10);
        // First occurrence at byte 0 wins — Keyword.
        assert_eq!(first.class, TokenClass::Keyword);
    }

    #[test]
    fn syntax_walk_produces_tokens_for_valid_module() {
        let src = "module Main exposing (main)\n\nmain : Int\nmain =\n    42\n";
        let (syntax, interner) = parse(src);
        let tokens = annotate_syntax(&syntax, src, &interner);
        assert!(!tokens.is_empty(), "syntax walk produces tokens");
    }
}

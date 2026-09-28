//! `unused-imports` — an import declaration whose bound names never appear in
//! the module body.
//!
//! An import introduces names in two ways:
//! 1. A module qualifier — either the `as Alias` alias or, absent that, the
//!    last segment of the dotted module name (`import Ipe.Url` → qualifier
//!    `Url`). This qualifier appears as the first symbol in `VarQual` or as
//!    the non-empty qualifier field in `TType` annotations.
//! 2. Explicitly exposed names: `import Foo exposing (bar, Baz)` — each name
//!    is bound unqualified and may appear as `VarLocal` or a bare type name.
//!
//! `exposing (..)` is a wildcard: the complete exported surface is not known
//! at the parse level, so a wildcard import is conservatively treated as used
//! (never flagged as unused).
//!
//! A name listed in the module's own `exposing` clause is a re-export — it is
//! treated as used so imports that exist solely to re-export are not flagged.
//!
//! This rule is deliberately conservative: when in doubt it does not fire.

use std::collections::HashSet;

use ipe_diagnostics::Span;
use ipe_intern::Symbol;
use ipe_syntax::{Exposing, Expr, Expr_, Import, Pattern, Pattern_, TypeAnnotation};

use crate::finding::{Finding, Fix};
use crate::rules::Ctx;

pub fn check(ctx: &Ctx) -> Vec<Finding> {
    let mut used_qualifiers: HashSet<Symbol> = HashSet::new();
    let mut used_unqualified: HashSet<Symbol> = HashSet::new();

    // Walk all value bodies and their type annotations.
    for value in &ctx.ast.values {
        walk_expr(
            &value.value.body,
            &mut used_qualifiers,
            &mut used_unqualified,
        );
        if let Some(ann) = &value.value.type_annotation {
            walk_type(&ann.value, &mut used_qualifiers, &mut used_unqualified);
        }
    }
    // Union constructor argument types.
    for union in &ctx.ast.unions {
        for ctor in &union.value.ctors {
            for arg in &ctor.value.args {
                walk_type(arg, &mut used_qualifiers, &mut used_unqualified);
            }
        }
    }
    // Type alias bodies.
    for alias in &ctx.ast.aliases {
        walk_type(
            &alias.value.body.value,
            &mut used_qualifiers,
            &mut used_unqualified,
        );
    }
    // Re-exports: names in the module's own `exposing` list that came from
    // imports are treated as used so re-exporting imports are not flagged.
    if let Exposing::List(items) = &ctx.ast.exposing.value {
        use ipe_syntax::Exposed;
        for item in items {
            let sym = match &item.value {
                Exposed::Value(s) | Exposed::Type(s, _) => *s,
            };
            used_unqualified.insert(sym);
        }
    }

    let mut findings = Vec::new();
    'import: for import in &ctx.ast.imports {
        // `imports` is `Vec<Import>` (not `Vec<Located<Import>>`), so no `.value`.

        // Wildcard exposing — conservatively skip (cannot know what it binds).
        if matches!(&import.exposing.value, Exposing::All) {
            continue 'import;
        }

        // The qualifier this import introduces: `as Alias` if present, else the
        // last segment of the dotted module name.
        let qualifier: Option<Symbol> = import.alias.or_else(|| import.name.value.last().copied());
        if qualifier.is_some_and(|q| used_qualifiers.contains(&q)) {
            continue 'import;
        }

        // Check whether any explicitly exposed name is used.
        if let Exposing::List(items) = &import.exposing.value {
            use ipe_syntax::Exposed;
            for item in items {
                let sym = match &item.value {
                    Exposed::Value(s) | Exposed::Type(s, _) => *s,
                };
                if used_unqualified.contains(&sym) {
                    continue 'import;
                }
            }
        }

        // No introduced name was used anywhere in the module body.
        let module_text: String = import
            .name
            .value
            .iter()
            .map(|s| ctx.text(*s))
            .collect::<Vec<_>>()
            .join(".");
        // The finding's own span stays anchored on the `import` keyword — the
        // span consumers other than this fix (diagnostics, hover) key off. The
        // FIX's span is wider: the whole declaration, keyword through the end of
        // its last clause (`as Alias` / `exposing (…)`, which may continue on a
        // following line), rounded out to full lines so deleting it leaves no
        // blank line or stranded continuation behind.
        let clause_end = import_clause_end(ctx.source, import);
        let fix_span = Span::new(
            byte_to_u32(line_start(ctx.source, import.import_kw.lo as usize)),
            byte_to_u32(line_end(ctx.source, clause_end)),
        );
        findings.push(ctx.with_fix(
            "unused-imports",
            import.import_kw,
            format!("`import {module_text}` is never used in this module"),
            vec![
                "remove the import or add an `exposing` clause for the names you need".to_owned(),
                "suppress: `-- ipe-lint: allow unused-imports`".to_owned(),
            ],
            Fix {
                describe: "remove unused import".to_owned(),
                span: fix_span,
                replacement: String::new(),
            },
        ));
    }
    findings
}

fn walk_expr(expr: &Expr, qualifiers: &mut HashSet<Symbol>, unqualified: &mut HashSet<Symbol>) {
    match &expr.value {
        Expr_::VarLocal(sym) => {
            unqualified.insert(*sym);
        }
        Expr_::VarQual(module_sym, _) => {
            qualifiers.insert(*module_sym);
        }
        Expr_::Call(callee, args) => {
            walk_expr(callee, qualifiers, unqualified);
            for arg in args {
                walk_expr(arg, qualifiers, unqualified);
            }
        }
        Expr_::Case(scrut, arms) => {
            walk_expr(scrut, qualifiers, unqualified);
            for (pat, body) in arms {
                walk_pattern(pat, qualifiers, unqualified);
                walk_expr(body, qualifiers, unqualified);
            }
        }
        Expr_::Lambda(pats, body) => {
            for p in pats {
                walk_pattern(p, qualifiers, unqualified);
            }
            walk_expr(body, qualifiers, unqualified);
        }
        Expr_::Binops(pairs, last) => {
            for (e, _op) in pairs {
                walk_expr(e, qualifiers, unqualified);
            }
            walk_expr(last, qualifiers, unqualified);
        }
        Expr_::Let(bindings, body) => {
            for b in bindings {
                walk_expr(&b.body, qualifiers, unqualified);
            }
            walk_expr(body, qualifiers, unqualified);
        }
        Expr_::If(branches, otherwise) => {
            for (cond, then_) in branches {
                walk_expr(cond, qualifiers, unqualified);
                walk_expr(then_, qualifiers, unqualified);
            }
            walk_expr(otherwise, qualifiers, unqualified);
        }
        Expr_::Tuple(elems) | Expr_::List(elems) => {
            for e in elems {
                walk_expr(e, qualifiers, unqualified);
            }
        }
        Expr_::Record(fields) => {
            for (_, v) in fields {
                walk_expr(v, qualifiers, unqualified);
            }
        }
        // `Update` base is a `Located<Symbol>` (a bare variable name) — a use of
        // that name, so an unqualified import of the same name counts as used.
        Expr_::Update(base_sym, fields) => {
            unqualified.insert(base_sym.value);
            for (_, v) in fields {
                walk_expr(v, qualifiers, unqualified);
            }
        }
        Expr_::Access(rec, _) => walk_expr(rec, qualifiers, unqualified),
        // `::` is represented as `Binops` with the `::` operator symbol — no
        // separate `Cons` variant exists at the syntax level.
        Expr_::Int(_)
        | Expr_::Float(_)
        | Expr_::Str(_)
        | Expr_::MultilineStr { .. }
        | Expr_::Char(_)
        | Expr_::PathLit(_)
        | Expr_::Unit => {}
    }
}

fn walk_pattern(
    pat: &Pattern,
    qualifiers: &mut HashSet<Symbol>,
    unqualified: &mut HashSet<Symbol>,
) {
    match &pat.value {
        // `PCtor(name, module_segs, sub_pats)` — module_segs is non-empty for
        // qualified constructors like `Result.Ok`.
        Pattern_::PCtor(_name, module_segs, sub_pats) => {
            // If there are module segments, the first is the qualifier reference.
            if let Some(first_seg) = module_segs.first() {
                qualifiers.insert(*first_seg);
            }
            for sp in sub_pats {
                walk_pattern(sp, qualifiers, unqualified);
            }
        }
        Pattern_::PTuple(ps) | Pattern_::PList(ps) => {
            for p in ps {
                walk_pattern(p, qualifiers, unqualified);
            }
        }
        Pattern_::PCons(h, t) => {
            walk_pattern(h, qualifiers, unqualified);
            walk_pattern(t, qualifiers, unqualified);
        }
        // `PRecord` fields are `Vec<Located<Symbol>>` — just field names bound
        // as variables; no sub-patterns.
        Pattern_::PRecord(_field_names) => {}
        Pattern_::PVar(sym) => {
            unqualified.insert(*sym);
        }
        Pattern_::PAlias(inner, _) => walk_pattern(inner, qualifiers, unqualified),
        Pattern_::POr(alts) => {
            for alt in alts {
                walk_pattern(alt, qualifiers, unqualified);
            }
        }
        Pattern_::PAnything
        | Pattern_::PDebugAnything
        | Pattern_::PUnit
        | Pattern_::PInt(_)
        | Pattern_::PBool(_)
        | Pattern_::PStr(_)
        | Pattern_::PChar(_) => {}
    }
}

fn walk_type(
    ann: &TypeAnnotation,
    qualifiers: &mut HashSet<Symbol>,
    unqualified: &mut HashSet<Symbol>,
) {
    match ann {
        TypeAnnotation::TLambda(a, b) => {
            walk_type(a, qualifiers, unqualified);
            walk_type(b, qualifiers, unqualified);
        }
        TypeAnnotation::TVar(_) | TypeAnnotation::TUnit => {}
        TypeAnnotation::TType(qualifier_sym, segments, args) => {
            // Record the qualifier symbol (may be the empty sentinel — we record
            // it anyway; the import check compares against actual interned
            // qualifier/alias symbols, which are non-empty).
            qualifiers.insert(*qualifier_sym);
            // Also record the first segment as a potential unqualified name
            // (for `exposing`-imported types used bare).
            if let Some(first) = segments.first() {
                unqualified.insert(*first);
            }
            for arg in args {
                walk_type(arg, qualifiers, unqualified);
            }
        }
        TypeAnnotation::TTuple(ts) => {
            for t in ts {
                walk_type(t, qualifiers, unqualified);
            }
        }
        TypeAnnotation::TRecord(fields) | TypeAnnotation::TRecordOpen(_, fields) => {
            for (_, t) in fields {
                walk_type(t, qualifiers, unqualified);
            }
        }
    }
}

/// The byte offset just past the end of an `import` declaration's last clause.
///
/// The parser records spans for the `import` keyword and the dotted module
/// name, but the `as Alias` identifier and the `exposing (…)` clause carry no
/// span that reaches their end — and each may sit on a continuation line below
/// the keyword. This walks the source from just past the module name, following
/// the import grammar tail (`[as Ident] [exposing ( … )]`), so the returned
/// offset covers the whole declaration however it is wrapped across lines. The
/// `exposing` list is consumed through its balanced closing paren, so even a
/// list broken across several lines is covered in full.
///
/// The walk is bounded by the remaining source length and only ever advances,
/// so it terminates. It never indexes: every read goes through `get`, so a
/// malformed tail yields the best offset reached rather than a panic.
pub fn import_clause_end(text: &str, import: &Import) -> usize {
    // Start just past the module name — the grammar tail (`as`, `exposing`)
    // begins there. The keyword span is a floor for a name-less malformed tail.
    let mut pos = import.import_kw.hi.max(import.name.span.hi) as usize;

    // Advance `pos` past `count` UTF-8 characters that satisfy `pred`, stopping
    // at the first that does not (or at end of input). Char-boundary safe.
    let skip_while = |src: &str, from: usize, pred: &dyn Fn(char) -> bool| -> usize {
        let rest = src.get(from..).unwrap_or("");
        let mut consumed = 0usize;
        for ch in rest.chars() {
            if pred(ch) {
                consumed += ch.len_utf8();
            } else {
                break;
            }
        }
        from + consumed
    };
    // True when the source from `at` begins with `kw` followed by a
    // non-identifier boundary (so `as` does not match inside `assets`).
    let starts_kw = |src: &str, at: usize, kw: &str| -> bool {
        let rest = src.get(at..).unwrap_or("");
        rest.strip_prefix(kw).is_some_and(|after| {
            after
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_')
        })
    };
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';

    // Optional `as Alias` (a single, dot-free identifier).
    let after_ws = skip_while(text, pos, &char::is_whitespace);
    if starts_kw(text, after_ws, "as") {
        let alias_start = skip_while(text, after_ws + "as".len(), &char::is_whitespace);
        let alias_end = skip_while(text, alias_start, &is_ident);
        pos = pos.max(alias_end);
    }

    // Optional `exposing ( … )` — consume through the balanced closing paren so
    // a wrapped list (`exposing (\n  a,\n  b\n)`) is covered in full.
    let after_ws = skip_while(text, pos, &char::is_whitespace);
    if starts_kw(text, after_ws, "exposing") {
        let after_kw = skip_while(text, after_ws + "exposing".len(), &char::is_whitespace);
        if text.get(after_kw..).unwrap_or("").starts_with('(') {
            let mut depth = 0i32;
            let mut cursor = after_kw;
            for ch in text.get(after_kw..).unwrap_or("").chars() {
                cursor += ch.len_utf8();
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            pos = pos.max(cursor);
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pos.min(text.len())
}

/// The byte offset of the start of the line containing byte offset `at`.
/// Never panics: `at` is clamped to `text.len()` and the scan is a plain
/// `rfind`, which only ever returns a valid char-boundary offset.
pub fn line_start(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text.get(..at)
        .and_then(|s| s.rfind('\n'))
        .map_or(0, |i| i + 1)
}

/// The byte offset just past the end of the line containing byte offset `at`,
/// through its trailing `\n` (and any `\r` immediately before it) so deleting
/// `text[line_start(at)..line_end(at)]` removes the whole physical line and
/// leaves no blank line behind. Returns `text.len()` on the file's last,
/// unterminated line.
pub fn line_end(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text.get(at..)
        .and_then(|s| s.find('\n'))
        .map_or(text.len(), |i| at + i + 1)
}

/// Lossless `usize -> u32` for a byte offset within a real source file (always
/// far under `u32::MAX`); saturates rather than panics on the unreachable
/// overflow case.
fn byte_to_u32(x: usize) -> u32 {
    u32::try_from(x).unwrap_or(u32::MAX)
}

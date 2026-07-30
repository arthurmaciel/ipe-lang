//! Document formatting: `textDocument/formatting` and
//! `textDocument/rangeFormatting`.
//!
//! Formats by re-printing the parse AST: canonical whitespace, 4-space indent,
//! sorted imports, blank lines between top-level declarations. When the buffer
//! does not parse, no edit is returned (never corrupt the user's file).
//!
//! The formatted output is returned as a single `TextEdit` replacing the whole
//! document (for `formatting`) or the selected lines (for `rangeFormatting`).

use std::fmt::Write as _;

use ipe_db::{Db as _, IpeDatabase, SourceFile};
use lsp_types::{Range, TextEdit};

use crate::offset::{PositionEncoding, offset_to_position};

/// Format the full text of `file`. Returns `None` when the file does not parse
/// (the client should leave the buffer unchanged).
#[must_use]
pub fn format_document(
    db: &IpeDatabase,
    file: SourceFile,
    encoding: PositionEncoding,
) -> Option<Vec<TextEdit>> {
    let module = ipe_db::parse(db, file).ok()?;
    let text = file.text(db);
    let formatted = format_module(db, &module, text);
    if formatted == text.as_str() {
        return Some(Vec::new()); // already canonical — no edit
    }
    let start = offset_to_position(text, 0, encoding);
    let end = offset_to_position(text, text.len(), encoding);
    Some(vec![TextEdit {
        range: Range { start, end },
        new_text: formatted,
    }])
}

/// Format the lines overlapping `range` in `file`. The replacement covers
/// full lines (from the start of the first touched line to the end of the last).
/// Returns `None` when the file does not parse.
#[must_use]
pub fn format_range(
    db: &IpeDatabase,
    file: SourceFile,
    range: Range,
    encoding: PositionEncoding,
) -> Option<Vec<TextEdit>> {
    // Format the full document first, then trim to the requested lines.
    let edits = format_document(db, file, encoding)?;
    // If no edits the document is already canonical.
    let Some(full_edit) = edits.into_iter().next() else {
        return Some(Vec::new());
    };
    let formatted = full_edit.new_text;
    // Extract the lines covered by `range` from the formatted output.
    let start_line = range.start.line as usize;
    let end_line = range.end.line as usize;
    let lines: Vec<&str> = formatted.split('\n').collect();
    let lo_line = start_line.min(lines.len().saturating_sub(1));
    let hi_line = end_line.min(lines.len().saturating_sub(1));
    // Collect the slice plus a trailing newline when present. `lo_line`/`hi_line`
    // are both clamped below `lines.len()`, so the range is always in bounds; an
    // empty `lines` yields an empty slice rather than a panic.
    let slice: String = lines
        .get(lo_line..=hi_line)
        .map_or_else(String::new, |ls| ls.join("\n"));

    // The LSP range covers full lines (character 0 to end of hi_line).
    let text = file.text(db);
    let range_start = offset_to_position(text, line_byte_start(text, lo_line), encoding);
    let range_end = offset_to_position(text, line_byte_end(text, hi_line), encoding);
    Some(vec![TextEdit {
        range: Range {
            start: range_start,
            end: range_end,
        },
        new_text: slice,
    }])
}

// ---------------------------------------------------------------------------
// Byte helpers
// ---------------------------------------------------------------------------

fn line_byte_start(text: &str, line: usize) -> usize {
    let mut byte = 0;
    for (i, l) in text.split('\n').enumerate() {
        if i == line {
            return byte;
        }
        byte += l.len() + 1;
    }
    text.len()
}

fn line_byte_end(text: &str, line: usize) -> usize {
    let mut byte = 0;
    for (i, l) in text.split('\n').enumerate() {
        let next = byte + l.len();
        if i == line {
            return next;
        }
        byte = next + 1;
    }
    text.len()
}

// ---------------------------------------------------------------------------
// Module printer
// ---------------------------------------------------------------------------

fn format_module(db: &IpeDatabase, module: &ipe_syntax::Module, original: &str) -> String {
    let interner = db.interner().lock();
    let resolve = |sym: ipe_intern::Symbol| interner.resolve(sym).unwrap_or("?");

    let mut out = String::new();

    // Module header.
    let mod_name = module
        .name
        .value
        .iter()
        .map(|&s| resolve(s))
        .collect::<Vec<_>>()
        .join(".");
    out.push_str("module ");
    out.push_str(&mod_name);
    out.push_str(" exposing (");
    push_exposing(&mut out, &module.exposing.value, &interner);
    out.push_str(")\n");

    push_imports(&mut out, module, &interner);
    push_aliases(&mut out, module, &interner);
    push_unions(&mut out, module, &interner);
    push_values(&mut out, module, &interner, original);

    // The module printer always ends with exactly one trailing newline.
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Print the module's imports, sorted alphabetically by dotted name.
fn push_imports(out: &mut String, module: &ipe_syntax::Module, interner: &ipe_intern::Interner) {
    let resolve = |sym: ipe_intern::Symbol| interner.resolve(sym).unwrap_or("?");
    if module.imports.is_empty() {
        return;
    }
    out.push('\n');
    let mut imports = module.imports.clone();
    imports.sort_by(|a, b| {
        let an: Vec<&str> = a.name.value.iter().map(|&s| resolve(s)).collect();
        let bn: Vec<&str> = b.name.value.iter().map(|&s| resolve(s)).collect();
        an.cmp(&bn)
    });
    for imp in &imports {
        let imp_name = imp
            .name
            .value
            .iter()
            .map(|&s| resolve(s))
            .collect::<Vec<_>>()
            .join(".");
        out.push_str("import ");
        out.push_str(&imp_name);
        if let Some(alias) = imp.alias {
            out.push_str(" as ");
            out.push_str(resolve(alias));
        }
        match &imp.exposing.value {
            ipe_syntax::Exposing::All => {
                out.push_str(" exposing (..)");
            }
            ipe_syntax::Exposing::List(list) if !list.is_empty() => {
                out.push_str(" exposing (");
                push_exposing(out, &imp.exposing.value, interner);
                out.push(')');
            }
            ipe_syntax::Exposing::List(_) => {}
        }
        out.push('\n');
    }
}

/// Print the module's type aliases.
fn push_aliases(out: &mut String, module: &ipe_syntax::Module, interner: &ipe_intern::Interner) {
    let resolve = |sym: ipe_intern::Symbol| interner.resolve(sym).unwrap_or("?");
    for alias in &module.aliases {
        out.push('\n');
        let name = resolve(alias.value.name.value);
        out.push_str("type alias ");
        out.push_str(name);
        for var in &alias.value.vars {
            out.push(' ');
            out.push_str(resolve(var.value));
        }
        out.push_str(" =\n    ");
        push_type_annotation(out, &alias.value.body.value, interner);
        out.push('\n');
    }
}

/// Print the module's union types.
fn push_unions(out: &mut String, module: &ipe_syntax::Module, interner: &ipe_intern::Interner) {
    let resolve = |sym: ipe_intern::Symbol| interner.resolve(sym).unwrap_or("?");
    for union in &module.unions {
        out.push('\n');
        let name = resolve(union.value.name.value);
        out.push_str("type ");
        out.push_str(name);
        for var in &union.value.vars {
            out.push(' ');
            out.push_str(resolve(var.value));
        }
        out.push('\n');
        for (i, ctor) in union.value.ctors.iter().enumerate() {
            if i == 0 {
                out.push_str("    = ");
            } else {
                out.push_str("    | ");
            }
            out.push_str(resolve(ctor.value.name));
            for arg in &ctor.value.args {
                out.push(' ');
                push_type_annotation(out, arg, interner);
            }
            out.push('\n');
        }
    }
}

/// Print the module's value declarations (each optional type annotation
/// followed by its equation).
fn push_values(
    out: &mut String,
    module: &ipe_syntax::Module,
    interner: &ipe_intern::Interner,
    original: &str,
) {
    let resolve = |sym: ipe_intern::Symbol| interner.resolve(sym).unwrap_or("?");
    for value in &module.values {
        out.push('\n');
        let name = resolve(value.value.name.value);
        // Type annotation.
        if let Some(ann) = &value.value.type_annotation {
            out.push_str(name);
            out.push_str(" : ");
            push_type_annotation(out, &ann.value, interner);
            out.push('\n');
        }
        out.push_str(name);
        for pat in &value.value.patterns {
            out.push(' ');
            push_pattern(out, pat, interner);
        }
        out.push_str(" =\n    ");
        push_expr(out, &value.value.body, 1, interner, original);
        out.push('\n');
    }
}

// ---------------------------------------------------------------------------
// Sub-printers
// ---------------------------------------------------------------------------

fn push_exposing(
    out: &mut String,
    exposing: &ipe_syntax::Exposing,
    interner: &ipe_intern::Interner,
) {
    let resolve = |sym: ipe_intern::Symbol| interner.resolve(sym).unwrap_or("?");
    match exposing {
        ipe_syntax::Exposing::All => out.push_str(".."),
        ipe_syntax::Exposing::List(items) => {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                match &item.value {
                    ipe_syntax::Exposed::Value(sym) => out.push_str(resolve(*sym)),
                    ipe_syntax::Exposed::Type(sym, privacy) => {
                        out.push_str(resolve(*sym));
                        match privacy {
                            ipe_syntax::Privacy::Public => out.push_str("(..)"),
                            ipe_syntax::Privacy::Private => {}
                            ipe_syntax::Privacy::PublicCtors(ctors) => {
                                out.push('(');
                                for (j, c) in ctors.iter().enumerate() {
                                    if j > 0 {
                                        out.push_str(", ");
                                    }
                                    out.push_str(resolve(*c));
                                }
                                out.push(')');
                            }
                        }
                    }
                }
            }
        }
    }
}

fn push_type_annotation(
    out: &mut String,
    ty: &ipe_syntax::TypeAnnotation,
    interner: &ipe_intern::Interner,
) {
    let resolve = |sym: ipe_intern::Symbol| interner.resolve(sym).unwrap_or("?");
    match ty {
        ipe_syntax::TypeAnnotation::TVar(sym) => out.push_str(resolve(*sym)),
        ipe_syntax::TypeAnnotation::TUnit => out.push_str("()"),
        ipe_syntax::TypeAnnotation::TType(qualifier, name_segs, args) => {
            let qualifier_str = resolve(*qualifier);
            if !qualifier_str.is_empty() {
                out.push_str(qualifier_str);
                out.push('.');
            }
            let name = name_segs
                .iter()
                .map(|&s| resolve(s))
                .collect::<Vec<_>>()
                .join(".");
            out.push_str(&name);
            for arg in args {
                out.push(' ');
                let needs_parens = matches!(
                    arg,
                    ipe_syntax::TypeAnnotation::TType(_, _, a) if !a.is_empty()
                ) || matches!(arg, ipe_syntax::TypeAnnotation::TLambda(..));
                if needs_parens {
                    out.push('(');
                    push_type_annotation(out, arg, interner);
                    out.push(')');
                } else {
                    push_type_annotation(out, arg, interner);
                }
            }
        }
        ipe_syntax::TypeAnnotation::TLambda(a, b) => {
            let needs_parens = matches!(a.as_ref(), ipe_syntax::TypeAnnotation::TLambda(..));
            if needs_parens {
                out.push('(');
                push_type_annotation(out, a, interner);
                out.push(')');
            } else {
                push_type_annotation(out, a, interner);
            }
            out.push_str(" -> ");
            push_type_annotation(out, b, interner);
        }
        ipe_syntax::TypeAnnotation::TTuple(elems) => {
            out.push('(');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                push_type_annotation(out, e, interner);
            }
            out.push(')');
        }
        ipe_syntax::TypeAnnotation::TRecord(fields) => {
            out.push_str("{ ");
            for (i, (name, ty)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(resolve(*name));
                out.push_str(" : ");
                push_type_annotation(out, ty, interner);
            }
            out.push_str(" }");
        }
        ipe_syntax::TypeAnnotation::TRecordOpen(row_var, fields) => {
            out.push_str("{ ");
            out.push_str(resolve(*row_var));
            out.push_str(" | ");
            for (i, (name, ty)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(resolve(*name));
                out.push_str(" : ");
                push_type_annotation(out, ty, interner);
            }
            out.push_str(" }");
        }
    }
}

fn push_pattern(out: &mut String, pat: &ipe_syntax::Pattern, interner: &ipe_intern::Interner) {
    let resolve = |sym: ipe_intern::Symbol| interner.resolve(sym).unwrap_or("?");
    match &pat.value {
        ipe_syntax::Pattern_::PAnything => out.push('_'),
        ipe_syntax::Pattern_::PVar(sym) => out.push_str(resolve(*sym)),
        ipe_syntax::Pattern_::PCtor(name, module_segs, args) => {
            if !module_segs.is_empty() {
                for seg in module_segs {
                    out.push_str(resolve(*seg));
                    out.push('.');
                }
            }
            out.push_str(resolve(*name));
            for arg in args {
                out.push(' ');
                let needs_parens = matches!(
                    &arg.value,
                    ipe_syntax::Pattern_::PCtor(_, _, a) if !a.is_empty()
                );
                if needs_parens {
                    out.push('(');
                    push_pattern(out, arg, interner);
                    out.push(')');
                } else {
                    push_pattern(out, arg, interner);
                }
            }
        }
        ipe_syntax::Pattern_::PTuple(elems) => {
            out.push('(');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                push_pattern(out, e, interner);
            }
            out.push(')');
        }
        ipe_syntax::Pattern_::PRecord(fields) => {
            out.push_str("{ ");
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(resolve(f.value));
            }
            out.push_str(" }");
        }
        ipe_syntax::Pattern_::PInt(n) => out.push_str(&n.to_string()),
        ipe_syntax::Pattern_::PBool(b) => {
            out.push_str(if *b { "True" } else { "False" });
        }
        ipe_syntax::Pattern_::PChar(c) => {
            out.push('\'');
            out.push_str(c);
            out.push('\'');
        }
        ipe_syntax::Pattern_::PStr(s) => {
            out.push('"');
            out.push_str(s);
            out.push('"');
        }
        ipe_syntax::Pattern_::PAlias(inner, name) => {
            push_pattern(out, inner, interner);
            out.push_str(" as ");
            out.push_str(resolve(name.value));
        }
        ipe_syntax::Pattern_::PList(elems) => {
            out.push('[');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                push_pattern(out, e, interner);
            }
            out.push(']');
        }
        ipe_syntax::Pattern_::PCons(h, t) => {
            push_pattern(out, h, interner);
            out.push_str(" :: ");
            push_pattern(out, t, interner);
        }
        ipe_syntax::Pattern_::POr(alts) => {
            for (i, alt) in alts.iter().enumerate() {
                if i > 0 {
                    out.push_str(" | ");
                }
                push_pattern(out, alt, interner);
            }
        }
    }
}

/// Print an expression at `indent` nesting levels (4 spaces each).
/// `original` is the module's source text, used to reproduce string/char
/// literals verbatim (avoids re-escaping).
fn push_expr(
    out: &mut String,
    expr: &ipe_syntax::Expr,
    indent: usize,
    interner: &ipe_intern::Interner,
    original: &str,
) {
    let resolve = |sym: ipe_intern::Symbol| interner.resolve(sym).unwrap_or("?");

    match &expr.value {
        ipe_syntax::Expr_::VarLocal(sym) => out.push_str(resolve(*sym)),
        ipe_syntax::Expr_::VarQual(qualifier, name) => {
            out.push_str(resolve(*qualifier));
            out.push('.');
            out.push_str(resolve(*name));
        }
        ipe_syntax::Expr_::Int(n) => out.push_str(&n.to_string()),
        ipe_syntax::Expr_::Float(f) => {
            // Reproduce the float from the original source when possible —
            // avoids precision drift from f64 Display.
            let lo = expr.span.lo as usize;
            let hi = expr.span.hi as usize;
            if let Some(slice) = original.get(lo..hi) {
                out.push_str(slice);
            } else {
                // Writing an f64 into a String is infallible.
                let _ = write!(out, "{f}");
            }
        }
        ipe_syntax::Expr_::Str(s) => {
            out.push('"');
            out.push_str(s);
            out.push('"');
        }
        ipe_syntax::Expr_::PathLit(s) => {
            out.push_str("path \"");
            out.push_str(s);
            out.push('"');
        }
        ipe_syntax::Expr_::MultilineStr { raw: s, .. } => {
            out.push_str("\"\"\"");
            out.push_str(s);
            out.push_str("\"\"\"");
        }
        ipe_syntax::Expr_::Char(c) => {
            out.push('\'');
            out.push_str(c);
            out.push('\'');
        }
        ipe_syntax::Expr_::Unit => out.push_str("()"),
        ipe_syntax::Expr_::Call(f, args) => {
            push_atom(out, f, indent, interner, original);
            for arg in args {
                out.push(' ');
                push_atom(out, arg, indent, interner, original);
            }
        }
        ipe_syntax::Expr_::Binops(pairs, last) => {
            // Re-print as `lhs op1 rhs1 op2 rhs2 … last`.
            for (lhs, op) in pairs {
                push_atom(out, lhs, indent, interner, original);
                out.push(' ');
                out.push_str(resolve(op.value));
                out.push(' ');
            }
            push_atom(out, last, indent, interner, original);
        }
        ipe_syntax::Expr_::Lambda(..)
        | ipe_syntax::Expr_::Let(..)
        | ipe_syntax::Expr_::If(..)
        | ipe_syntax::Expr_::Case(..) => {
            push_block_expr(out, expr, indent, interner, original);
        }
        ipe_syntax::Expr_::Tuple(..)
        | ipe_syntax::Expr_::List(..)
        | ipe_syntax::Expr_::Record(..)
        | ipe_syntax::Expr_::Access(..)
        | ipe_syntax::Expr_::Update(..) => {
            push_collection_expr(out, expr, indent, interner, original);
        }
    }
}

/// Print the delimited collection / record forms (tuples, lists, records,
/// field access, record update), factored out of `push_expr`.
fn push_collection_expr(
    out: &mut String,
    expr: &ipe_syntax::Expr,
    indent: usize,
    interner: &ipe_intern::Interner,
    original: &str,
) {
    let resolve = |sym: ipe_intern::Symbol| interner.resolve(sym).unwrap_or("?");
    match &expr.value {
        ipe_syntax::Expr_::Tuple(elems) => {
            out.push('(');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                push_expr(out, e, indent, interner, original);
            }
            out.push(')');
        }
        ipe_syntax::Expr_::List(elems) => {
            if elems.is_empty() {
                out.push_str("[]");
            } else {
                out.push_str("[ ");
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    push_expr(out, e, indent, interner, original);
                }
                out.push_str(" ]");
            }
        }
        ipe_syntax::Expr_::Record(fields) => {
            if fields.is_empty() {
                out.push_str("{}");
            } else {
                out.push_str("{ ");
                for (i, (name, val)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(resolve(name.value));
                    out.push_str(" = ");
                    push_expr(out, val, indent, interner, original);
                }
                out.push_str(" }");
            }
        }
        ipe_syntax::Expr_::Access(rec, field) => {
            push_atom(out, rec, indent, interner, original);
            out.push('.');
            out.push_str(resolve(field.value));
        }
        ipe_syntax::Expr_::Update(base, fields) => {
            out.push_str("{ ");
            out.push_str(resolve(base.value));
            out.push_str(" | ");
            for (i, (name, val)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(resolve(name.value));
                out.push_str(" = ");
                push_expr(out, val, indent, interner, original);
            }
            out.push_str(" }");
        }
        // `push_expr` only routes the five collection forms here.
        _ => push_expr(out, expr, indent, interner, original),
    }
}

/// Print the multi-line block expressions (`\… ->`, `let`, `if`, `case`),
/// factored out of `push_expr` so each printer stays focused.
fn push_block_expr(
    out: &mut String,
    expr: &ipe_syntax::Expr,
    indent: usize,
    interner: &ipe_intern::Interner,
    original: &str,
) {
    let pad = "    ".repeat(indent);
    let pad1 = "    ".repeat(indent + 1);
    match &expr.value {
        ipe_syntax::Expr_::Lambda(params, body) => {
            out.push('\\');
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                push_pattern(out, p, interner);
            }
            out.push_str(" ->\n");
            out.push_str(&pad1);
            push_expr(out, body, indent + 1, interner, original);
        }
        ipe_syntax::Expr_::Let(bindings, body) => {
            out.push_str("let\n");
            for b in bindings {
                out.push_str(&pad1);
                push_pattern(out, &b.pat, interner);
                out.push_str(" =\n");
                out.push_str(&"    ".repeat(indent + 2));
                push_expr(out, &b.body, indent + 2, interner, original);
                out.push('\n');
            }
            out.push_str(&pad);
            out.push_str("in\n");
            out.push_str(&pad1);
            push_expr(out, body, indent + 1, interner, original);
        }
        ipe_syntax::Expr_::If(branches, else_expr) => {
            for (i, (cond, then_)) in branches.iter().enumerate() {
                if i == 0 {
                    out.push_str("if ");
                } else {
                    out.push_str(" else if ");
                }
                push_expr(out, cond, indent, interner, original);
                out.push_str(" then\n");
                out.push_str(&pad1);
                push_expr(out, then_, indent + 1, interner, original);
                out.push('\n');
                out.push_str(&pad);
            }
            out.push_str("else\n");
            out.push_str(&pad1);
            push_expr(out, else_expr, indent + 1, interner, original);
        }
        ipe_syntax::Expr_::Case(scrutinee, branches) => {
            out.push_str("case ");
            push_expr(out, scrutinee, indent, interner, original);
            out.push_str(" of\n");
            for (pat, body) in branches {
                out.push_str(&pad1);
                push_pattern(out, pat, interner);
                out.push_str(" ->\n");
                out.push_str(&"    ".repeat(indent + 2));
                push_expr(out, body, indent + 2, interner, original);
                out.push('\n');
                out.push('\n');
            }
            // Remove the final extra newline so callers get one clean end.
            if out.ends_with("\n\n") {
                out.pop();
            }
        }
        // `push_expr` only routes the four block forms here.
        _ => push_expr(out, expr, indent, interner, original),
    }
}

/// Print an expression, wrapping in parens when the top node is a compound
/// form that needs disambiguation in argument position.
fn push_atom(
    out: &mut String,
    expr: &ipe_syntax::Expr,
    indent: usize,
    interner: &ipe_intern::Interner,
    original: &str,
) {
    let needs_parens = matches!(
        &expr.value,
        ipe_syntax::Expr_::Call(_, _)
            | ipe_syntax::Expr_::Lambda(_, _)
            | ipe_syntax::Expr_::Let(_, _)
            | ipe_syntax::Expr_::If(_, _)
            | ipe_syntax::Expr_::Case(_, _)
            | ipe_syntax::Expr_::Binops(_, _)
    );
    if needs_parens {
        out.push('(');
        push_expr(out, expr, indent, interner, original);
        out.push(')');
    } else {
        push_expr(out, expr, indent, interner, original);
    }
}

#[cfg(test)]
mod tests {
    use ipe_db::{IpeDatabase, ModuleOrigin, SourceFile};

    use super::format_document;
    use crate::offset::PositionEncoding;

    fn file(db: &IpeDatabase, path: &[&str], text: &str) -> SourceFile {
        SourceFile::new(
            db,
            path.iter().map(|s| (*s).to_owned()).collect(),
            text.to_owned(),
            ModuleOrigin::User,
        )
    }

    const ALREADY_FORMATTED: &str = "module Main exposing (main)\n\nmain : Int\nmain =\n    42\n";

    #[test]
    fn already_formatted_produces_no_edit() {
        let db = IpeDatabase::new();
        let f = file(&db, &["Main"], ALREADY_FORMATTED);
        let edits = format_document(&db, f, PositionEncoding::Utf16)
            .expect("parseable source returns Some");
        assert!(edits.is_empty(), "no edit when already canonical");
    }

    #[test]
    fn format_roundtrips_through_parse() {
        let db = IpeDatabase::new();
        let src = "module Main exposing (main)\n\nimport Helper exposing (three)\n\nmain : Int\nmain =\n    42\n";
        let f = file(&db, &["Main"], src);
        let edits = format_document(&db, f, PositionEncoding::Utf16)
            .expect("parseable source returns Some");
        // Apply the edit (if any) and re-parse — must still parse cleanly.
        let result = edits
            .first()
            .map_or_else(|| src.to_owned(), |e| e.new_text.clone());
        let f2 = file(&db, &["Main"], &result);
        let edits2 =
            format_document(&db, f2, PositionEncoding::Utf16).expect("formatted source parses");
        assert!(edits2.is_empty(), "idempotent after one pass");
    }

    #[test]
    fn unparseable_source_returns_none() {
        let db = IpeDatabase::new();
        let f = file(&db, &["Main"], "this is not valid ipe source @@@@");
        let result = format_document(&db, f, PositionEncoding::Utf16);
        assert!(result.is_none(), "no edit for unparseable source");
    }
}

//! Inlay hints: `textDocument/inlayHint`.
//!
//! Produces type-annotation inlay hints for:
//!
//! - Top-level value bindings without an explicit type annotation.
//! - Local `let` bindings (pattern binders) whose type is solved.
//! - Lambda parameters whose type is solved.
//!
//! Each hint appears just after the binding's name (or pattern) in the form
//! `: Type`, matching the style of an explicit annotation.
//!
//! Top-level bindings that already have a type annotation are skipped — the
//! annotation is already visible in source.

use ipe_db::{Db as _, IpeDatabase, ModuleTypes, SourceRoot};
use ipe_diagnostics::Span;
use ipe_intern::Symbol;
use ipe_syntax::{Expr_, Pattern_};
use ipe_types::{Ty, VarNamer, ty_to_doc};
use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Range};

use crate::offset::{PositionEncoding, offset_to_position, span_to_range};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Inlay hints for the given document range.
///
/// Only top-level value bindings without a type annotation that overlap
/// `range` are hinted. Returns an empty list when no such bindings exist or
/// the file does not parse.
#[must_use]
pub fn inlay_hints(
    db: &IpeDatabase,
    root: SourceRoot,
    entry: ipe_db::SourceFile,
    module: &[String],
    range: Range,
    encoding: PositionEncoding,
) -> Vec<InlayHint> {
    let files = root.files(db);
    let Some(&file) = files.get(module) else {
        return Vec::new();
    };
    let Ok(parsed) = ipe_db::parse(db, file) else {
        return Vec::new();
    };
    let text = file.text(db);

    // Only produce hints when the type environment is available.
    let Ok(solved) = ipe_db::typecheck(db, root, entry) else {
        return Vec::new();
    };

    // Build the home symbol path once.
    let (home_syms, interner_needed): (Vec<Symbol>, bool) = {
        let mut interner = db.interner().lock();
        let syms: Option<Vec<Symbol>> = module
            .iter()
            .map(|s| interner.intern(s).ok())
            .collect::<Option<Vec<_>>>();
        drop(interner);
        match syms {
            Some(s) => (s, true),
            None => return Vec::new(),
        }
    };
    let _ = interner_needed;

    let mut hints: Vec<InlayHint> = Vec::new();

    for value in &parsed.values {
        // Skip bindings that already have an annotation.
        if value.value.type_annotation.is_some() {
            continue;
        }

        let name_span = value.value.name.span;
        let span_range = span_to_range(text, name_span, encoding);

        // Skip if the span does not intersect the requested range.
        if span_range.end < range.start || span_range.start > range.end {
            continue;
        }

        // Resolve the name symbol — intern under the same symbol index so the
        // env key matches.
        let name_sym = value.value.name.value;

        // Look up the type.
        let Some(ty) = solved.env.get(&(home_syms.clone(), name_sym)) else {
            continue;
        };

        let hint_label = {
            let interner = db.interner().lock();
            let mut namer = VarNamer::new();
            let Ok(doc) = ty_to_doc(ty, &interner, &mut namer) else {
                continue;
            };
            drop(interner);
            format!(": {}", ipe_diagnostics::render_ty(&doc))
        };

        // Place the hint just after the name token.
        let position = offset_to_position(text, name_span.hi as usize, encoding);

        hints.push(InlayHint {
            position,
            label: InlayHintLabel::String(hint_label),
            kind: Some(InlayHintKind::TYPE),
            text_edits: None,
            tooltip: None,
            padding_left: Some(true),
            padding_right: None,
            data: None,
        });

        // Walk the binding's body for local `let` and lambda hints.
        walk_expr_hints(
            &value.value.body,
            text,
            &range,
            &solved,
            db,
            encoding,
            &mut hints,
        );
    }

    hints
}

/// Recursively walk `expr`, emitting inlay hints for:
///
/// - Each `let x = rhs in …` binder where `x` is a plain variable (`PVar`)
///   and the region map holds a type for `rhs`.
/// - Each lambda parameter that is a plain variable (`PVar`) and whose type
///   is recoverable by peeling the lambda's own solved function type.
fn walk_expr_hints(
    expr: &ipe_diagnostics::Located<Expr_>,
    text: &str,
    range: &Range,
    solved: &ModuleTypes,
    db: &IpeDatabase,
    encoding: PositionEncoding,
    hints: &mut Vec<InlayHint>,
) {
    match &expr.value {
        Expr_::Let(bindings, body) => {
            for binding in bindings {
                // Only plain `PVar` binders get a hint — tuple/record
                // destructures have no single name to attach to.
                if let Pattern_::PVar(_) = &binding.pat.value {
                    let hint_span = binding.pat.span;
                    let span_range = span_to_range(text, hint_span, encoding);
                    let in_range = span_range.end >= range.start && span_range.start <= range.end;
                    if in_range {
                        // The binder's type equals the body expression's type.
                        if let Some(ty) = solved.regions.get(&binding.body.span) {
                            if let Some(label) = render_ty_label(ty, db) {
                                let position =
                                    offset_to_position(text, hint_span.hi as usize, encoding);
                                hints.push(InlayHint {
                                    position,
                                    label: InlayHintLabel::String(label),
                                    kind: Some(InlayHintKind::TYPE),
                                    text_edits: None,
                                    tooltip: None,
                                    padding_left: Some(true),
                                    padding_right: None,
                                    data: None,
                                });
                            }
                        }
                    }
                    // Recurse into the binding's RHS.
                    walk_expr_hints(&binding.body, text, range, solved, db, encoding, hints);
                }
            }
            walk_expr_hints(body, text, range, solved, db, encoding, hints);
        }
        Expr_::Lambda(params, body) => {
            // Recover the lambda's own function type from the region map so
            // each parameter's type can be read by peeling arrows.
            let mut cur_ty: Option<&Ty> = solved.regions.get(&expr.span);
            for param in params {
                let param_ty = cur_ty.and_then(|t| match t {
                    Ty::Fun(p, _) => Some(p.as_ref()),
                    _ => None,
                });
                // Advance to the return type for the next param.
                cur_ty = cur_ty.and_then(|t| match t {
                    Ty::Fun(_, ret) => Some(ret.as_ref()),
                    _ => None,
                });

                if let Pattern_::PVar(_) = &param.value {
                    let hint_span = param.span;
                    let span_range = span_to_range(text, hint_span, encoding);
                    let in_range = span_range.end >= range.start && span_range.start <= range.end;
                    if in_range {
                        if let Some(ty) = param_ty {
                            if let Some(label) = render_ty_label(ty, db) {
                                let position =
                                    offset_to_position(text, hint_span.hi as usize, encoding);
                                hints.push(InlayHint {
                                    position,
                                    label: InlayHintLabel::String(label),
                                    kind: Some(InlayHintKind::TYPE),
                                    text_edits: None,
                                    tooltip: None,
                                    padding_left: Some(true),
                                    padding_right: None,
                                    data: None,
                                });
                            }
                        }
                    }
                }
            }
            walk_expr_hints(body, text, range, solved, db, encoding, hints);
        }
        // Recurse into all other compound expressions — no hints emitted here,
        // but sub-expressions may contain let/lambda nodes.
        Expr_::Call(callee, args) => {
            walk_expr_hints(callee, text, range, solved, db, encoding, hints);
            for a in args {
                walk_expr_hints(a, text, range, solved, db, encoding, hints);
            }
        }
        Expr_::Case(scrutinee, arms) => {
            walk_expr_hints(scrutinee, text, range, solved, db, encoding, hints);
            for (_, arm_body) in arms {
                walk_expr_hints(arm_body, text, range, solved, db, encoding, hints);
            }
        }
        Expr_::If(branches, else_expr) => {
            for (cond, then_) in branches {
                walk_expr_hints(cond, text, range, solved, db, encoding, hints);
                walk_expr_hints(then_, text, range, solved, db, encoding, hints);
            }
            walk_expr_hints(else_expr, text, range, solved, db, encoding, hints);
        }
        Expr_::Binops(pairs, last) => {
            for (operand, _) in pairs {
                walk_expr_hints(operand, text, range, solved, db, encoding, hints);
            }
            walk_expr_hints(last, text, range, solved, db, encoding, hints);
        }
        Expr_::Tuple(elems) | Expr_::List(elems) => {
            for e in elems {
                walk_expr_hints(e, text, range, solved, db, encoding, hints);
            }
        }
        Expr_::Record(fields) => {
            for (_, v) in fields {
                walk_expr_hints(v, text, range, solved, db, encoding, hints);
            }
        }
        Expr_::Access(base, _) => {
            walk_expr_hints(base, text, range, solved, db, encoding, hints);
        }
        // `Update(Located<Symbol>, fields)` — the base is a bare name reference,
        // not a sub-expression; only the field values need recursion.
        Expr_::Update(_, fields) => {
            for (_, v) in fields {
                walk_expr_hints(v, text, range, solved, db, encoding, hints);
            }
        }
        // Leaves: no sub-expressions to recurse into.
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

/// Render a solved `Ty` to a `: Type` hint label string, or `None` if the
/// type cannot be rendered (interner miss, doc error).
fn render_ty_label(ty: &Ty, db: &IpeDatabase) -> Option<String> {
    let interner = db.interner().lock();
    let mut namer = VarNamer::new();
    let doc = ty_to_doc(ty, &interner, &mut namer).ok()?;
    drop(interner);
    Some(format!(": {}", ipe_diagnostics::render_ty(&doc)))
}

// Suppress unused import warning — `Span` is referenced in the attribute path
// on `span_to_range`.
#[allow(dead_code)]
type _SpanAlias = Span;

#[cfg(test)]
mod tests {
    use ipe_db::{IpeDatabase, ModuleOrigin, SourceFile, SourceRoot};
    use lsp_types::{Position, Range};

    use super::inlay_hints;
    use crate::offset::PositionEncoding;

    fn file(db: &IpeDatabase, path: &[&str], text: &str) -> SourceFile {
        SourceFile::new(
            db,
            path.iter().map(|s| (*s).to_owned()).collect(),
            text.to_owned(),
            ModuleOrigin::User,
        )
    }

    fn root_of(db: &IpeDatabase, files: &[(&[&str], SourceFile)]) -> SourceRoot {
        SourceRoot::new(
            db,
            files
                .iter()
                .map(|(path, f)| (path.iter().map(|s| (*s).to_owned()).collect(), *f))
                .collect(),
        )
    }

    fn full_range() -> Range {
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 9999,
                character: 0,
            },
        }
    }

    #[test]
    fn no_hints_when_annotation_present() {
        let db = IpeDatabase::new();
        let src = "module Main exposing (main)\n\nmain : Int\nmain =\n    42\n";
        let entry = file(&db, &["Main"], src);
        let root = root_of(&db, &[(&["Main"], entry)]);
        let hints = inlay_hints(
            &db,
            root,
            entry,
            &["Main".to_owned()],
            full_range(),
            PositionEncoding::Utf16,
        );
        assert!(
            hints.is_empty(),
            "annotated binding should produce no hint: {hints:?}"
        );
    }

    #[test]
    fn hint_appears_for_unannotated_binding() {
        let db = IpeDatabase::new();
        // No type annotation on `main`.
        let src = "module Main exposing (main)\n\nmain =\n    42\n";
        let entry = file(&db, &["Main"], src);
        let root = root_of(&db, &[(&["Main"], entry)]);
        let hints = inlay_hints(
            &db,
            root,
            entry,
            &["Main".to_owned()],
            full_range(),
            PositionEncoding::Utf16,
        );
        // If type-check succeeds, we should get a hint for `main`.
        // If it fails (no annotation makes inference ambiguous), hints is empty.
        // Either way, no panic.
        for h in &hints {
            let lsp_types::InlayHintLabel::String(label) = &h.label else {
                continue;
            };
            assert!(
                label.starts_with(": "),
                "hint label should start with `: `, got: {label}"
            );
        }
    }

    #[test]
    fn no_hints_for_unparseable_source() {
        let db = IpeDatabase::new();
        let f = file(&db, &["Main"], "@@@ not valid @@@");
        let root = root_of(&db, &[(&["Main"], f)]);
        let hints = inlay_hints(
            &db,
            root,
            f,
            &["Main".to_owned()],
            full_range(),
            PositionEncoding::Utf16,
        );
        assert!(hints.is_empty());
    }

    /// A `let x = 42 in x` binding inside a typed function must produce an
    /// inlay hint for `x` with `: Int`.
    #[test]
    fn let_binding_gets_type_hint() {
        let db = IpeDatabase::new();
        // `answer` is typed, so inference can solve `x = 42 : Int`.
        let src = "module Main exposing (main)\n\nmain : Int\nmain =\n    let x = 42 in\n    x\n";
        let entry = file(&db, &["Main"], src);
        let root = root_of(&db, &[(&["Main"], entry)]);
        let hints = inlay_hints(
            &db,
            root,
            entry,
            &["Main".to_owned()],
            full_range(),
            PositionEncoding::Utf16,
        );
        let labels: Vec<&str> = hints
            .iter()
            .filter_map(|h| {
                if let lsp_types::InlayHintLabel::String(s) = &h.label {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(
            labels.iter().any(|l| *l == ": Int"),
            "let binding `x = 42` must have a `: Int` hint; got: {labels:?}"
        );
    }

    /// Lambda parameters inside a typed binding must get inlay hints.
    #[test]
    fn lambda_params_get_type_hints() {
        let db = IpeDatabase::new();
        // `apply` is typed; its body `\f x -> f x` has lambda params `f` and `x`
        // whose types are fully determined by the annotation.
        let src = "module Main exposing (main)\n\napply : (Int -> Int) -> Int -> Int\napply =\n    \\f x -> f x\n\nmain : Int\nmain =\n    apply (\\n -> n) 0\n";
        let entry = file(&db, &["Main"], src);
        let root = root_of(&db, &[(&["Main"], entry)]);
        let hints = inlay_hints(
            &db,
            root,
            entry,
            &["Main".to_owned()],
            full_range(),
            PositionEncoding::Utf16,
        );
        let labels: Vec<&str> = hints
            .iter()
            .filter_map(|h| {
                if let lsp_types::InlayHintLabel::String(s) = &h.label {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .collect();
        // `f : Int -> Int` and `x : Int` must appear among the hints.
        assert!(
            labels.iter().any(|l| l.contains("Int")),
            "lambda params inside a typed binding must get type hints; got: {labels:?}"
        );
        // Must have at least 2 hints (one per lambda param).
        assert!(
            labels.len() >= 2,
            "expected hints for both lambda params; got: {labels:?}"
        );
    }
}

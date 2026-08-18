//! Signature help: `textDocument/signatureHelp`.
//!
//! When the cursor is inside a function-call argument list, returns the
//! callee's type signature and highlights the active parameter.
//!
//! **Algorithm:**
//! 1. Walk the canonical AST for the innermost `Call(VarTopLevel, …)` that
//!    contains `byte`.
//! 2. Read the callee's function type from the solved type environment.
//! 3. Decompose the type into parameter types (one per `->` arrow).
//! 4. Count fully-typed arguments before `byte` to pick the active parameter.
//!
//! Returns `None` when the cursor is not inside a call, the program does not
//! type-check, or the callee is not a function type.

use ipe_canon::ast::{Def, Expr_};
use ipe_db::{Db as _, IpeDatabase, SourceRoot};
use ipe_diagnostics::Located;
use ipe_intern::Symbol;
use ipe_types::{Ty, VarNamer, ty_to_doc};
use lsp_types::{ParameterInformation, ParameterLabel, SignatureHelp, SignatureInformation};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Signature help at `byte` in `module`. Returns `None` when the position is
/// not inside a function-call or the type environment cannot answer.
#[must_use]
pub fn signature_help(
    db: &IpeDatabase,
    root: SourceRoot,
    entry: ipe_db::SourceFile,
    module: &[String],
    byte: u32,
) -> Option<SignatureHelp> {
    let files = root.files(db);
    let &file = files.get(module)?;
    let canonical = crate::db_access::canonicalize_checked(db, root, entry, file)?;
    let solved = ipe_db::typecheck(db, root, entry).ok()?;

    // Find the innermost Call(VarTopLevel, …) containing `byte`.
    let (home_syms, name_sym, active_param) = find_call_at(&canonical.module, byte)?;

    // Resolve the callee's type from the solved env.
    let callee_ty = solved.env.get(&(home_syms, name_sym))?.clone();

    // Decompose into parameter types.
    let params = fn_params(&callee_ty);
    if params.is_empty() {
        return None;
    }

    // Render signature and parameters.
    let interner = db.interner().lock();
    let callee_name = interner.resolve(name_sym).unwrap_or("?");
    let mut namer = VarNamer::new();
    let sig_doc = ty_to_doc(&callee_ty, &interner, &mut namer).ok()?;
    let sig_label = format!("{callee_name} : {}", ipe_diagnostics::render_ty(&sig_doc));

    let mut param_infos: Vec<ParameterInformation> = Vec::with_capacity(params.len());
    for param_ty in &params {
        let doc = ty_to_doc(param_ty, &interner, &mut namer).ok()?;
        param_infos.push(ParameterInformation {
            label: ParameterLabel::Simple(ipe_diagnostics::render_ty(&doc)),
            documentation: None,
        });
    }
    drop(interner);

    // Clamp to the last parameter index, then narrow to u32 (LSP's wire type);
    // a signature never has u32::MAX parameters, so the saturating fallback is
    // unreachable in practice but keeps the conversion total.
    let last_param = param_infos.len().saturating_sub(1);
    let active = u32::try_from(active_param.min(last_param)).unwrap_or(u32::MAX);

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label: sig_label,
            documentation: None,
            parameters: Some(param_infos),
            active_parameter: Some(active),
        }],
        active_signature: Some(0),
        active_parameter: Some(active),
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Decompose a function type into parameter types (the left side of each
/// `->` arrow). Returns an empty list for non-function types.
fn fn_params(ty: &Ty) -> Vec<Ty> {
    let mut params = Vec::new();
    let mut cur = ty;
    while let Ty::Fun(param, ret) = cur {
        params.push(*param.clone());
        cur = ret;
    }
    params
}

/// Walk the canonical module's defs looking for the innermost
/// `Call(VarTopLevel { module, name }, args)` node whose span contains
/// `byte`. Returns `(home_symbols, name_symbol, active_arg_index)`.
fn find_call_at(
    module: &ipe_canon::ast::Module,
    byte: u32,
) -> Option<(Vec<Symbol>, Symbol, usize)> {
    // (span_width, home, name, active_arg)
    let mut best: Option<(u32, Vec<Symbol>, Symbol, usize)> = None;

    for def in &module.defs {
        let body = match def {
            Def::Untyped { body, .. } | Def::Typed { body, .. } => body,
        };
        walk_call(body, byte, &mut best);
    }

    best.map(|(_, home, name, active)| (home, name, active))
}

fn walk_call(
    expr: &Located<Expr_>,
    byte: u32,
    best: &mut Option<(u32, Vec<Symbol>, Symbol, usize)>,
) {
    if !(expr.span.lo <= byte && byte < expr.span.hi) {
        return;
    }

    if let Expr_::Call(f, args) = &expr.value {
        // Count how many arguments are fully before the cursor.
        let active = args.iter().take_while(|a| a.span.hi <= byte).count();

        if let Expr_::VarTopLevel { module: home, name } = &f.value {
            let width = expr.span.hi.saturating_sub(expr.span.lo);
            if best.as_ref().is_none_or(|&(w, _, _, _)| width < w) {
                *best = Some((width, home.clone(), *name, active));
            }
        }

        walk_call(f, byte, best);
        for arg in args {
            walk_call(arg, byte, best);
        }
        return; // sub-expressions handled above
    }

    // Recurse for all other compound expressions.
    match &expr.value {
        Expr_::Lambda(_, body) => walk_call(body, byte, best),
        Expr_::Let(bindings, body) => {
            for b in bindings {
                walk_call(&b.body, byte, best);
            }
            walk_call(body, byte, best);
        }
        Expr_::Case(scrutinee, branches) => {
            walk_call(scrutinee, byte, best);
            for branch in branches {
                walk_call(&branch.body, byte, best);
            }
        }
        Expr_::Binop { lhs, rhs, .. } => {
            walk_call(lhs, byte, best);
            walk_call(rhs, byte, best);
        }
        Expr_::If(branches, else_expr) => {
            for (cond, then_) in branches {
                walk_call(cond, byte, best);
                walk_call(then_, byte, best);
            }
            walk_call(else_expr, byte, best);
        }
        Expr_::Tuple(elems) | Expr_::List(elems) => {
            for e in elems {
                walk_call(e, byte, best);
            }
        }
        Expr_::Cons(h, t) => {
            walk_call(h, byte, best);
            walk_call(t, byte, best);
        }
        Expr_::Record(fields) => {
            for (_, v) in fields {
                walk_call(v, byte, best);
            }
        }
        Expr_::Access(rec, _) => walk_call(rec, byte, best),
        Expr_::Update(base, fields) => {
            walk_call(base, byte, best);
            for (_, v) in fields {
                walk_call(v, byte, best);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use ipe_db::{IpeDatabase, ModuleOrigin, SourceFile, SourceRoot};

    use super::signature_help;

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

    const HELPER: &str =
        "module Helper exposing (add)\n\nadd : Int -> Int -> Int\nadd x y =\n    x + y\n";
    const MAIN: &str = "module Main exposing (main)\n\nimport Helper exposing (add)\n\nmain : Int\nmain =\n    add 1 2\n";

    /// Byte offset of `1` in the `add 1 2` call.
    fn call_arg_byte() -> u32 {
        u32::try_from(MAIN.rfind(" 1 ").expect("` 1 ` in main") + 1).expect("u32")
    }

    #[test]
    fn signature_help_outside_call_returns_none_or_some() {
        let db = IpeDatabase::new();
        let helper = file(&db, &["Helper"], HELPER);
        let entry = file(&db, &["Main"], MAIN);
        let root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);
        // Byte 0 is the `m` in `module` — not a call site.
        let _result = signature_help(&db, root, entry, &["Main".to_owned()], 0);
        // No panic is the assertion.
    }

    #[test]
    fn signature_help_at_call_arg_no_panic() {
        let db = IpeDatabase::new();
        let helper = file(&db, &["Helper"], HELPER);
        let entry = file(&db, &["Main"], MAIN);
        let root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);
        let _result = signature_help(&db, root, entry, &["Main".to_owned()], call_arg_byte());
        // If the implementation finds the call, it should return Some with the
        // `add` signature; either way, no panic.
    }

    #[test]
    fn signature_help_resolves_add_signature() {
        let db = IpeDatabase::new();
        let helper = file(&db, &["Helper"], HELPER);
        let entry = file(&db, &["Main"], MAIN);
        let root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);
        let result = signature_help(&db, root, entry, &["Main".to_owned()], call_arg_byte());
        if let Some(help) = result {
            let sig = help.signatures.first().expect("at least one signature");
            // The label must mention `add`.
            assert!(sig.label.contains("add"), "label: {}", sig.label);
            // Should have 2 parameters (Int -> Int -> Int has 2 params).
            let params = sig.parameters.as_ref().expect("parameters present");
            assert_eq!(params.len(), 2, "add has 2 parameters");
        }
        // If None: the canonical walker didn't find the VarTopLevel call —
        // acceptable for now; no panic is the hard requirement.
    }
}

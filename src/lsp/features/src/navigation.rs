//! Go-to-definition and find-references over the typed AST.
//!
//! Both features read the canonicalised AST from `ipe_db`, so results are
//! always consistent with what the compiler sees.
//!
//! **Definition** — the declaration site of a top-level name: the name span
//! from the *parse* AST of the defining module, so the selection range is the
//! identifier token rather than the full binding body.
//!
//! **References** — every `VarTopLevel` use site across all in-scope modules
//! whose `(home, name)` matches the target. Does not include the definition
//! span itself — that is the caller's choice to union.
//!
//! Scope: top-level bindings only. Local bindings (`let`, lambda parameters,
//! `case` branches) are not tracked — they do not cross module boundaries and
//! have no persistent canonical identity.

use ipe_canon::ast::{Def, Expr_, Module};
use ipe_db::{Db as _, IpeDatabase, SourceRoot};
use ipe_diagnostics::Span;
use ipe_intern::Symbol;

// ---------------------------------------------------------------------------
// Public API types
// ---------------------------------------------------------------------------

/// A located reference to a top-level name: the module that contains it and
/// the byte span of the identifier token.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NameRef {
    /// The module the reference appears in (its source file's module path).
    pub module: Vec<String>,
    /// The byte span of the identifier in that module's source text.
    pub span: Span,
}

/// The definition site of a top-level name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Definition {
    /// The module that declares the name.
    pub module: Vec<String>,
    /// The byte span of the identifier token at the declaration site.
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Expression walker — shared by definition lookup and reference collection
// ---------------------------------------------------------------------------

/// Walk `expr` recording every `VarTopLevel` node whose span contains `byte`.
/// The narrowest span (smallest width) wins; `best` tracks
/// `(width, home_syms, name_sym)`.
fn walk_for_ref_at(
    expr: &ipe_diagnostics::Located<Expr_>,
    byte: u32,
    best: &mut Option<(u32, Vec<Symbol>, Symbol)>,
) {
    if !(expr.span.lo <= byte && byte < expr.span.hi) {
        return;
    }
    match &expr.value {
        Expr_::VarTopLevel { module, name } => {
            let width = expr.span.hi.saturating_sub(expr.span.lo);
            if best
                .as_ref()
                .is_none_or(|&(prev_width, _, _)| width < prev_width)
            {
                *best = Some((width, module.clone(), *name));
            }
        }
        Expr_::Call(f, args) => {
            walk_for_ref_at(f, byte, best);
            for arg in args {
                walk_for_ref_at(arg, byte, best);
            }
        }
        Expr_::Lambda(_, body) => {
            walk_for_ref_at(body, byte, best);
        }
        Expr_::Let(bindings, body) => {
            for b in bindings {
                walk_for_ref_at(&b.body, byte, best);
            }
            walk_for_ref_at(body, byte, best);
        }
        Expr_::Case(scrutinee, branches) => {
            walk_for_ref_at(scrutinee, byte, best);
            for branch in branches {
                walk_for_ref_at(&branch.body, byte, best);
            }
        }
        Expr_::Binop { lhs, rhs, .. } => {
            walk_for_ref_at(lhs, byte, best);
            walk_for_ref_at(rhs, byte, best);
        }
        Expr_::If(branches, else_expr) => {
            for (cond, then_) in branches {
                walk_for_ref_at(cond, byte, best);
                walk_for_ref_at(then_, byte, best);
            }
            walk_for_ref_at(else_expr, byte, best);
        }
        Expr_::Tuple(elems) | Expr_::List(elems) => {
            for e in elems {
                walk_for_ref_at(e, byte, best);
            }
        }
        Expr_::Cons(h, t) => {
            walk_for_ref_at(h, byte, best);
            walk_for_ref_at(t, byte, best);
        }
        Expr_::Record(fields) => {
            for (_, v) in fields {
                walk_for_ref_at(v, byte, best);
            }
        }
        Expr_::Access(rec, _) => {
            walk_for_ref_at(rec, byte, best);
        }
        Expr_::Update(base, fields) => {
            walk_for_ref_at(base, byte, best);
            for (_, v) in fields {
                walk_for_ref_at(v, byte, best);
            }
        }
        Expr_::ForeignCall { args, .. } => {
            for arg in args {
                walk_for_ref_at(arg, byte, best);
            }
        }
        Expr_::VarLocal(_)
        | Expr_::VarKernel { .. }
        | Expr_::VarCtor { .. }
        | Expr_::Int(_)
        | Expr_::Float(_)
        | Expr_::Str(_)
        | Expr_::PathLit(_)
        | Expr_::CustomElementCtor(_)
        | Expr_::Char(_)
        | Expr_::Unit => {}
    }
}

/// Walk `expr` collecting every `VarTopLevel` span where `home == target_home`
/// and `name == target_name` into `out`.
fn walk_for_refs(
    expr: &ipe_diagnostics::Located<Expr_>,
    target_home: &[Symbol],
    target_name: Symbol,
    out: &mut Vec<Span>,
) {
    match &expr.value {
        Expr_::VarTopLevel { module, name } => {
            if module.as_slice() == target_home && *name == target_name {
                out.push(expr.span);
            }
        }
        Expr_::Call(f, args) => {
            walk_for_refs(f, target_home, target_name, out);
            for arg in args {
                walk_for_refs(arg, target_home, target_name, out);
            }
        }
        Expr_::Lambda(_, body) => {
            walk_for_refs(body, target_home, target_name, out);
        }
        Expr_::Let(bindings, body) => {
            for b in bindings {
                walk_for_refs(&b.body, target_home, target_name, out);
            }
            walk_for_refs(body, target_home, target_name, out);
        }
        Expr_::Case(scrutinee, branches) => {
            walk_for_refs(scrutinee, target_home, target_name, out);
            for branch in branches {
                walk_for_refs(&branch.body, target_home, target_name, out);
            }
        }
        Expr_::Binop { lhs, rhs, .. } => {
            walk_for_refs(lhs, target_home, target_name, out);
            walk_for_refs(rhs, target_home, target_name, out);
        }
        Expr_::If(branches, else_expr) => {
            for (cond, then_) in branches {
                walk_for_refs(cond, target_home, target_name, out);
                walk_for_refs(then_, target_home, target_name, out);
            }
            walk_for_refs(else_expr, target_home, target_name, out);
        }
        Expr_::Tuple(elems) | Expr_::List(elems) => {
            for e in elems {
                walk_for_refs(e, target_home, target_name, out);
            }
        }
        Expr_::Cons(h, t) => {
            walk_for_refs(h, target_home, target_name, out);
            walk_for_refs(t, target_home, target_name, out);
        }
        Expr_::Record(fields) => {
            for (_, v) in fields {
                walk_for_refs(v, target_home, target_name, out);
            }
        }
        Expr_::Access(rec, _) => {
            walk_for_refs(rec, target_home, target_name, out);
        }
        Expr_::Update(base, fields) => {
            walk_for_refs(base, target_home, target_name, out);
            for (_, v) in fields {
                walk_for_refs(v, target_home, target_name, out);
            }
        }
        Expr_::ForeignCall { args, .. } => {
            for arg in args {
                walk_for_refs(arg, target_home, target_name, out);
            }
        }
        Expr_::VarLocal(_)
        | Expr_::VarKernel { .. }
        | Expr_::VarCtor { .. }
        | Expr_::Int(_)
        | Expr_::Float(_)
        | Expr_::Str(_)
        | Expr_::PathLit(_)
        | Expr_::CustomElementCtor(_)
        | Expr_::Char(_)
        | Expr_::Unit => {}
    }
}

// ---------------------------------------------------------------------------
// Internal helpers for the rename provider
// ---------------------------------------------------------------------------

/// Find the `(home_syms, name_sym)` of the innermost `VarTopLevel` containing
/// `byte` in the canonical module `m`.
///
/// Exposed for the rename provider so it can resolve the current name at a
/// reference site without re-running `goto_definition`.
#[must_use]
pub fn find_ref_at_pub(m: &Module, byte: u32) -> Option<(Vec<Symbol>, Symbol)> {
    find_ref_at(m, byte)
}

/// The identifier span of the innermost `VarTopLevel` containing `byte`.
///
/// Exposed for the rename provider's `prepare_rename` range.
#[must_use]
pub fn ref_span_at(m: &Module, byte: u32) -> Option<Span> {
    let mut best: Option<(u32, Span)> = None;
    for def in &m.defs {
        let body = match def {
            Def::Untyped { body, .. } | Def::Typed { body, .. } => body,
        };
        walk_for_span_at(body, byte, &mut best);
    }
    best.map(|(_, span)| span)
}

fn walk_for_span_at(
    expr: &ipe_diagnostics::Located<Expr_>,
    byte: u32,
    best: &mut Option<(u32, Span)>,
) {
    if !(expr.span.lo <= byte && byte < expr.span.hi) {
        return;
    }
    if matches!(expr.value, Expr_::VarTopLevel { .. }) {
        let width = expr.span.hi.saturating_sub(expr.span.lo);
        if best
            .as_ref()
            .is_none_or(|&(prev_width, _)| width < prev_width)
        {
            *best = Some((width, expr.span));
        }
    }
    // Recurse into sub-expressions.
    match &expr.value {
        Expr_::Call(f, args) => {
            walk_for_span_at(f, byte, best);
            for arg in args {
                walk_for_span_at(arg, byte, best);
            }
        }
        Expr_::Lambda(_, body) => walk_for_span_at(body, byte, best),
        Expr_::Let(bindings, body) => {
            for b in bindings {
                walk_for_span_at(&b.body, byte, best);
            }
            walk_for_span_at(body, byte, best);
        }
        Expr_::Case(scrutinee, branches) => {
            walk_for_span_at(scrutinee, byte, best);
            for branch in branches {
                walk_for_span_at(&branch.body, byte, best);
            }
        }
        Expr_::Binop { lhs, rhs, .. } => {
            walk_for_span_at(lhs, byte, best);
            walk_for_span_at(rhs, byte, best);
        }
        Expr_::If(branches, else_expr) => {
            for (cond, then_) in branches {
                walk_for_span_at(cond, byte, best);
                walk_for_span_at(then_, byte, best);
            }
            walk_for_span_at(else_expr, byte, best);
        }
        Expr_::Tuple(elems) | Expr_::List(elems) => {
            for e in elems {
                walk_for_span_at(e, byte, best);
            }
        }
        Expr_::Cons(h, t) => {
            walk_for_span_at(h, byte, best);
            walk_for_span_at(t, byte, best);
        }
        Expr_::Record(fields) => {
            for (_, v) in fields {
                walk_for_span_at(v, byte, best);
            }
        }
        Expr_::Access(rec, _) => walk_for_span_at(rec, byte, best),
        Expr_::Update(base, fields) => {
            walk_for_span_at(base, byte, best);
            for (_, v) in fields {
                walk_for_span_at(v, byte, best);
            }
        }
        Expr_::ForeignCall { args, .. } => {
            for arg in args {
                walk_for_span_at(arg, byte, best);
            }
        }
        Expr_::VarLocal(_)
        | Expr_::VarTopLevel { .. }
        | Expr_::VarKernel { .. }
        | Expr_::VarCtor { .. }
        | Expr_::Int(_)
        | Expr_::Float(_)
        | Expr_::Str(_)
        | Expr_::PathLit(_)
        | Expr_::CustomElementCtor(_)
        | Expr_::Char(_)
        | Expr_::Unit => {}
    }
}

// ---------------------------------------------------------------------------
// Go-to-definition
// ---------------------------------------------------------------------------

/// The definition site of the top-level name under `byte` in `module`, if any.
///
/// Returns `None` when the byte position does not fall on a top-level
/// reference, the module does not type-check, or the defining module is not
/// part of the project (kernel / stdlib).
#[must_use]
pub fn goto_definition(
    db: &IpeDatabase,
    root: SourceRoot,
    entry: ipe_db::SourceFile,
    module: &[String],
    byte: u32,
) -> Option<Definition> {
    let files = root.files(db);
    let &file = files.get(module)?;

    // Find the `VarTopLevel` node whose span contains `byte`.
    let canonical = crate::db_access::canonicalize_checked(db, root, entry, file)?;
    let (def_home_syms, def_name_sym) = find_ref_at(&canonical.module, byte)?;

    // Resolve the home module path to strings.
    let def_module: Vec<String> = {
        let interner = db.interner().lock();
        def_home_syms
            .iter()
            .map(|&sym| interner.resolve(sym).map(str::to_owned))
            .collect::<Option<Vec<_>>>()?
    };

    // Find the name span in the defining module's parse tree.
    let &def_file = files.get(&def_module)?;
    let parsed = ipe_db::parse(db, def_file).ok()?;

    // Resolve `def_name_sym` to a string for comparison against parse-tree names.
    let def_name_str: String = {
        let interner = db.interner().lock();
        interner.resolve(def_name_sym).map(str::to_owned)?
    };

    let span = definition_span_in_parse(&parsed, &def_name_str, db)?;

    Some(Definition {
        module: def_module,
        span,
    })
}

fn find_ref_at(m: &Module, byte: u32) -> Option<(Vec<Symbol>, Symbol)> {
    let mut best: Option<(u32, Vec<Symbol>, Symbol)> = None;
    for def in &m.defs {
        let body = match def {
            Def::Untyped { body, .. } | Def::Typed { body, .. } => body,
        };
        walk_for_ref_at(body, byte, &mut best);
    }
    best.map(|(_, home, name)| (home, name))
}

/// Find the name-token span of a top-level value declaration named `name` in
/// the parse tree of one module.
fn definition_span_in_parse(
    parsed: &ipe_syntax::Module,
    name: &str,
    db: &IpeDatabase,
) -> Option<Span> {
    let interner = db.interner().lock();
    for value in &parsed.values {
        if interner.resolve(value.value.name.value) == Some(name) {
            return Some(value.value.name.span);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Find-references
// ---------------------------------------------------------------------------

/// Every use site of the top-level binding `(home_module, name)` across all
/// modules in the project.
///
/// Does not include the definition span — callers that want
/// "definition + all references" union the two themselves.
#[must_use]
pub fn find_references(
    db: &IpeDatabase,
    root: SourceRoot,
    entry: ipe_db::SourceFile,
    home_module: &[String],
    name: &str,
) -> Vec<NameRef> {
    // Resolve target `(home, name)` to symbols.
    let (home_syms, name_sym) = {
        let mut interner = db.interner().lock();
        let home: Option<Vec<Symbol>> = home_module
            .iter()
            .map(|s| interner.intern(s).ok())
            .collect::<Option<Vec<_>>>();
        let name_res = interner.intern(name).ok();
        drop(interner);
        match (home, name_res) {
            (Some(h), Some(n)) => (h, n),
            _ => return Vec::new(),
        }
    };

    // Dependency-first module order for consistent output. A cycle means no
    // safe canonicalize demand is possible — return empty rather than
    // iterating files and hitting salsa's dependency-cycle panic.
    let Ok(order) = ipe_db::topo_order(db, root, entry) else {
        return Vec::new();
    };

    let files = root.files(db);
    let mut refs: Vec<NameRef> = Vec::new();

    for module_path in &*order {
        let Some(&file) = files.get(module_path) else {
            continue;
        };
        let Ok(canonical) = ipe_db::canonicalize(db, root, file) else {
            continue;
        };
        let mut spans: Vec<Span> = Vec::new();
        for def in &canonical.module.defs {
            let body = match def {
                Def::Untyped { body, .. } | Def::Typed { body, .. } => body,
            };
            walk_for_refs(body, &home_syms, name_sym, &mut spans);
        }
        for span in spans {
            refs.push(NameRef {
                module: module_path.clone(),
                span,
            });
        }
    }
    refs
}

#[cfg(test)]
mod tests {
    use ipe_db::{IpeDatabase, ModuleOrigin, SourceFile, SourceRoot};

    use super::{find_references, goto_definition};

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

    const HELPER: &str = "module Helper exposing (three)\n\nthree : Int\nthree = 3\n";
    const MAIN: &str = "module Main exposing (main)\n\nimport Helper exposing (three)\n\nmain : Int\nmain = three\n";

    fn ref_byte() -> u32 {
        u32::try_from(MAIN.rfind("three").expect("three in main")).expect("fits u32")
    }

    #[test]
    fn goto_definition_resolves_cross_module_reference() {
        let db = IpeDatabase::new();
        let helper = file(&db, &["Helper"], HELPER);
        let entry = file(&db, &["Main"], MAIN);
        let root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);

        let def = goto_definition(&db, root, entry, &["Main".to_owned()], ref_byte())
            .expect("definition found");

        assert_eq!(def.module, vec!["Helper".to_owned()]);
        let lo = def.span.lo as usize;
        let hi = def.span.hi as usize;
        assert_eq!(
            HELPER.get(lo..hi),
            Some("three"),
            "definition span covers identifier"
        );
    }

    #[test]
    fn find_references_returns_all_use_sites() {
        let db = IpeDatabase::new();
        let helper = file(&db, &["Helper"], HELPER);
        let entry = file(&db, &["Main"], MAIN);
        let root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);

        let refs = find_references(&db, root, entry, &["Helper".to_owned()], "three");

        assert_eq!(refs.len(), 1, "expected 1 reference, got: {refs:?}");
        let first = refs.first().expect("one reference");
        assert_eq!(first.module, vec!["Main".to_owned()]);
        let lo = first.span.lo as usize;
        let hi = first.span.hi as usize;
        assert_eq!(MAIN.get(lo..hi), Some("three"), "span covers identifier");
    }
}

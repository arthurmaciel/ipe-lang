//! Go-to-definition and find-references over the typed AST.
//!
//! Both features read the canonicalised AST from `ipe_db`, so results are
//! always consistent with what the compiler sees.
//!
//! **Definition** — the declaration site of a named top-level entity: for a
//! value binding, its name token in the defining module's `values`; for a data
//! constructor, its name token in a `type` union's constructor list. The span
//! comes from the *parse* AST so the selection range is the identifier token
//! rather than the full binding body.
//!
//! **References** — every use site across all in-scope modules whose
//! `(home, name)` matches the target. A name-bearing node participates whether
//! it is a `VarTopLevel` value use, a `VarCtor` constructor value, or a `PCtor`
//! constructor pattern in a `case` arm / lambda parameter / `let` binder. Does
//! not include the definition span itself — that is the caller's choice to
//! union.
//!
//! Scope: top-level bindings and data constructors. Locally-bound names (`let`
//! value binders, lambda value parameters, `case`-bound variables) are not
//! tracked — they do not cross module boundaries and have no persistent
//! canonical identity.

use ipe_canon::ast::{CaseBranch, Def, Expr_, LetBinding, Module, Pattern, Pattern_};
use ipe_db::{Db as _, IpeDatabase, SourceRoot};
use ipe_diagnostics::Span;
use ipe_intern::{Interner, Symbol};

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

/// The byte span of a constructor's *name token* within a `PCtor` pattern.
///
/// A canonical `PCtor` pattern's span covers the whole pattern (`Node l r`),
/// but only the leading name token (`Node`) is the reference. The name token
/// begins at the pattern's low offset — the same convention the semantic-token
/// walker uses — and its width is the constructor name's byte length; an
/// unresolvable symbol yields a zero-width span at the pattern start, which
/// contains no byte and so is silently skipped.
fn ctor_name_span(pat_lo: u32, name: Symbol, interner: &Interner) -> Span {
    let len = interner
        .resolve(name)
        .map_or(0, |s| u32::try_from(s.len()).unwrap_or(0));
    Span::new(pat_lo, pat_lo.saturating_add(len))
}

/// Walk `expr` recording every top-level / constructor use whose span contains
/// `byte`. The narrowest span (smallest width) wins; `best` tracks
/// `(width, home_syms, name_sym)`.
fn walk_for_ref_at(
    expr: &ipe_diagnostics::Located<Expr_>,
    byte: u32,
    interner: &Interner,
    best: &mut Option<(u32, Vec<Symbol>, Symbol)>,
) {
    if !(expr.span.lo <= byte && byte < expr.span.hi) {
        return;
    }
    match &expr.value {
        Expr_::VarTopLevel { module, name } => {
            consider_ref_at(expr.span, module.clone(), *name, best);
        }
        // A constructor used as a value (`Just`, `Red`): its span is the bare
        // name token, so it participates exactly as a top-level use does.
        Expr_::VarCtor { home, name, .. } => {
            consider_ref_at(expr.span, home.clone(), *name, best);
        }
        Expr_::Call(f, args) => {
            walk_for_ref_at(f, byte, interner, best);
            for arg in args {
                walk_for_ref_at(arg, byte, interner, best);
            }
        }
        Expr_::Lambda(params, body) => {
            for p in params {
                walk_pat_for_ref_at(p, byte, interner, best);
            }
            walk_for_ref_at(body, byte, interner, best);
        }
        Expr_::Let(bindings, body) => {
            for LetBinding { pat, body: bval } in bindings {
                walk_pat_for_ref_at(pat, byte, interner, best);
                walk_for_ref_at(bval, byte, interner, best);
            }
            walk_for_ref_at(body, byte, interner, best);
        }
        Expr_::Case(scrutinee, branches) => {
            walk_for_ref_at(scrutinee, byte, interner, best);
            for CaseBranch { pat, body } in branches {
                walk_pat_for_ref_at(pat, byte, interner, best);
                walk_for_ref_at(body, byte, interner, best);
            }
        }
        Expr_::Binop { lhs, rhs, .. } => {
            walk_for_ref_at(lhs, byte, interner, best);
            walk_for_ref_at(rhs, byte, interner, best);
        }
        Expr_::If(branches, else_expr) => {
            for (cond, then_) in branches {
                walk_for_ref_at(cond, byte, interner, best);
                walk_for_ref_at(then_, byte, interner, best);
            }
            walk_for_ref_at(else_expr, byte, interner, best);
        }
        Expr_::Tuple(elems) | Expr_::List(elems) => {
            for e in elems {
                walk_for_ref_at(e, byte, interner, best);
            }
        }
        Expr_::Cons(h, t) => {
            walk_for_ref_at(h, byte, interner, best);
            walk_for_ref_at(t, byte, interner, best);
        }
        Expr_::Record(fields) => {
            for (_, v) in fields {
                walk_for_ref_at(v, byte, interner, best);
            }
        }
        Expr_::Access(rec, _) => {
            walk_for_ref_at(rec, byte, interner, best);
        }
        Expr_::Update(base, fields) => {
            walk_for_ref_at(base, byte, interner, best);
            for (_, v) in fields {
                walk_for_ref_at(v, byte, interner, best);
            }
        }
        Expr_::ForeignCall { args, .. } => {
            for arg in args {
                walk_for_ref_at(arg, byte, interner, best);
            }
        }
        Expr_::VarLocal(_)
        | Expr_::VarKernel { .. }
        | Expr_::Int(_)
        | Expr_::Float(_)
        | Expr_::Str(_)
        | Expr_::PathLit(_)
        | Expr_::CustomElementCtor(_)
        | Expr_::Char(_)
        | Expr_::Unit => {}
    }
}

/// Record a candidate `(home, name)` at `span`, keeping the narrowest.
fn consider_ref_at(
    span: Span,
    home: Vec<Symbol>,
    name: Symbol,
    best: &mut Option<(u32, Vec<Symbol>, Symbol)>,
) {
    let width = span.hi.saturating_sub(span.lo);
    if best
        .as_ref()
        .is_none_or(|&(prev_width, _, _)| width < prev_width)
    {
        *best = Some((width, home, name));
    }
}

/// Walk a pattern recording every `PCtor` name token whose span contains
/// `byte`, descending into every sub-pattern.
fn walk_pat_for_ref_at(
    pat: &Pattern,
    byte: u32,
    interner: &Interner,
    best: &mut Option<(u32, Vec<Symbol>, Symbol)>,
) {
    match &pat.value {
        Pattern_::PCtor {
            home, name, args, ..
        } => {
            let name_span = ctor_name_span(pat.span.lo, *name, interner);
            if name_span.lo <= byte && byte < name_span.hi {
                consider_ref_at(name_span, home.clone(), *name, best);
            }
            for a in args {
                walk_pat_for_ref_at(a, byte, interner, best);
            }
        }
        Pattern_::PTuple(elems) | Pattern_::PList(elems) => {
            for e in elems {
                walk_pat_for_ref_at(e, byte, interner, best);
            }
        }
        Pattern_::PAlias(inner, _) => walk_pat_for_ref_at(inner, byte, interner, best),
        Pattern_::PCons(h, t) => {
            walk_pat_for_ref_at(h, byte, interner, best);
            walk_pat_for_ref_at(t, byte, interner, best);
        }
        Pattern_::POr(alts) => {
            for a in alts {
                walk_pat_for_ref_at(a, byte, interner, best);
            }
        }
        Pattern_::PVar(_)
        | Pattern_::PAnything
        | Pattern_::PDebugAnything
        | Pattern_::PUnit
        | Pattern_::PRecord(_)
        | Pattern_::PInt(_)
        | Pattern_::PBool(_)
        | Pattern_::PChar(_)
        | Pattern_::PStr(_) => {}
    }
}

/// Walk `expr` collecting every use span where `home == target_home` and
/// `name == target_name` into `out` — top-level value uses, constructor value
/// uses, and constructor patterns alike.
fn walk_for_refs(
    expr: &ipe_diagnostics::Located<Expr_>,
    target_home: &[Symbol],
    target_name: Symbol,
    interner: &Interner,
    out: &mut Vec<Span>,
) {
    match &expr.value {
        Expr_::VarTopLevel { module, name } => {
            if module.as_slice() == target_home && *name == target_name {
                out.push(expr.span);
            }
        }
        // A constructor used as a value: its span is the bare name token.
        Expr_::VarCtor { home, name, .. } => {
            if home.as_slice() == target_home && *name == target_name {
                out.push(expr.span);
            }
        }
        Expr_::Call(f, args) => {
            walk_for_refs(f, target_home, target_name, interner, out);
            for arg in args {
                walk_for_refs(arg, target_home, target_name, interner, out);
            }
        }
        Expr_::Lambda(params, body) => {
            for p in params {
                walk_pat_for_refs(p, target_home, target_name, interner, out);
            }
            walk_for_refs(body, target_home, target_name, interner, out);
        }
        Expr_::Let(bindings, body) => {
            for LetBinding { pat, body: bval } in bindings {
                walk_pat_for_refs(pat, target_home, target_name, interner, out);
                walk_for_refs(bval, target_home, target_name, interner, out);
            }
            walk_for_refs(body, target_home, target_name, interner, out);
        }
        Expr_::Case(scrutinee, branches) => {
            walk_for_refs(scrutinee, target_home, target_name, interner, out);
            for CaseBranch { pat, body } in branches {
                walk_pat_for_refs(pat, target_home, target_name, interner, out);
                walk_for_refs(body, target_home, target_name, interner, out);
            }
        }
        Expr_::Binop { lhs, rhs, .. } => {
            walk_for_refs(lhs, target_home, target_name, interner, out);
            walk_for_refs(rhs, target_home, target_name, interner, out);
        }
        Expr_::If(branches, else_expr) => {
            for (cond, then_) in branches {
                walk_for_refs(cond, target_home, target_name, interner, out);
                walk_for_refs(then_, target_home, target_name, interner, out);
            }
            walk_for_refs(else_expr, target_home, target_name, interner, out);
        }
        Expr_::Tuple(elems) | Expr_::List(elems) => {
            for e in elems {
                walk_for_refs(e, target_home, target_name, interner, out);
            }
        }
        Expr_::Cons(h, t) => {
            walk_for_refs(h, target_home, target_name, interner, out);
            walk_for_refs(t, target_home, target_name, interner, out);
        }
        Expr_::Record(fields) => {
            for (_, v) in fields {
                walk_for_refs(v, target_home, target_name, interner, out);
            }
        }
        Expr_::Access(rec, _) => {
            walk_for_refs(rec, target_home, target_name, interner, out);
        }
        Expr_::Update(base, fields) => {
            walk_for_refs(base, target_home, target_name, interner, out);
            for (_, v) in fields {
                walk_for_refs(v, target_home, target_name, interner, out);
            }
        }
        Expr_::ForeignCall { args, .. } => {
            for arg in args {
                walk_for_refs(arg, target_home, target_name, interner, out);
            }
        }
        Expr_::VarLocal(_)
        | Expr_::VarKernel { .. }
        | Expr_::Int(_)
        | Expr_::Float(_)
        | Expr_::Str(_)
        | Expr_::PathLit(_)
        | Expr_::CustomElementCtor(_)
        | Expr_::Char(_)
        | Expr_::Unit => {}
    }
}

/// Walk a pattern collecting every `PCtor` name-token span matching the target,
/// descending into every sub-pattern.
fn walk_pat_for_refs(
    pat: &Pattern,
    target_home: &[Symbol],
    target_name: Symbol,
    interner: &Interner,
    out: &mut Vec<Span>,
) {
    match &pat.value {
        Pattern_::PCtor {
            home, name, args, ..
        } => {
            if home.as_slice() == target_home && *name == target_name {
                out.push(ctor_name_span(pat.span.lo, *name, interner));
            }
            for a in args {
                walk_pat_for_refs(a, target_home, target_name, interner, out);
            }
        }
        Pattern_::PTuple(elems) | Pattern_::PList(elems) => {
            for e in elems {
                walk_pat_for_refs(e, target_home, target_name, interner, out);
            }
        }
        Pattern_::PAlias(inner, _) => {
            walk_pat_for_refs(inner, target_home, target_name, interner, out);
        }
        Pattern_::PCons(h, t) => {
            walk_pat_for_refs(h, target_home, target_name, interner, out);
            walk_pat_for_refs(t, target_home, target_name, interner, out);
        }
        Pattern_::POr(alts) => {
            for a in alts {
                walk_pat_for_refs(a, target_home, target_name, interner, out);
            }
        }
        Pattern_::PVar(_)
        | Pattern_::PAnything
        | Pattern_::PDebugAnything
        | Pattern_::PUnit
        | Pattern_::PRecord(_)
        | Pattern_::PInt(_)
        | Pattern_::PBool(_)
        | Pattern_::PChar(_)
        | Pattern_::PStr(_) => {}
    }
}

// ---------------------------------------------------------------------------
// Internal helpers for the rename provider
// ---------------------------------------------------------------------------

/// Find the `(home_syms, name_sym)` of the innermost top-level / constructor
/// reference containing `byte` in the canonical module `m`.
///
/// Exposed for the rename provider so it can resolve the current name at a
/// reference site without re-running `goto_definition`.
#[must_use]
pub fn find_ref_at_pub(
    m: &Module,
    byte: u32,
    interner: &Interner,
) -> Option<(Vec<Symbol>, Symbol)> {
    find_ref_at(m, byte, interner)
}

/// The identifier span of the innermost top-level / constructor reference
/// containing `byte`.
///
/// Exposed for the rename provider's `prepare_rename` range.
#[must_use]
pub fn ref_span_at(m: &Module, byte: u32, interner: &Interner) -> Option<Span> {
    let mut best: Option<(u32, Span)> = None;
    for def in &m.defs {
        let body = match def {
            Def::Untyped { body, .. } | Def::Typed { body, .. } => body,
        };
        walk_for_span_at(body, byte, interner, &mut best);
    }
    best.map(|(_, span)| span)
}

/// Record a candidate span at `byte`, keeping the narrowest.
fn consider_span_at(span: Span, best: &mut Option<(u32, Span)>) {
    let width = span.hi.saturating_sub(span.lo);
    if best
        .as_ref()
        .is_none_or(|&(prev_width, _)| width < prev_width)
    {
        *best = Some((width, span));
    }
}

fn walk_for_span_at(
    expr: &ipe_diagnostics::Located<Expr_>,
    byte: u32,
    interner: &Interner,
    best: &mut Option<(u32, Span)>,
) {
    if !(expr.span.lo <= byte && byte < expr.span.hi) {
        return;
    }
    match &expr.value {
        // Both a top-level use and a constructor value carry the bare name
        // token as their span.
        Expr_::VarTopLevel { .. } | Expr_::VarCtor { .. } => consider_span_at(expr.span, best),
        _ => {}
    }
    // Recurse into sub-expressions.
    match &expr.value {
        Expr_::Call(f, args) => {
            walk_for_span_at(f, byte, interner, best);
            for arg in args {
                walk_for_span_at(arg, byte, interner, best);
            }
        }
        Expr_::Lambda(params, body) => {
            for p in params {
                walk_pat_for_span_at(p, byte, interner, best);
            }
            walk_for_span_at(body, byte, interner, best);
        }
        Expr_::Let(bindings, body) => {
            for LetBinding { pat, body: bval } in bindings {
                walk_pat_for_span_at(pat, byte, interner, best);
                walk_for_span_at(bval, byte, interner, best);
            }
            walk_for_span_at(body, byte, interner, best);
        }
        Expr_::Case(scrutinee, branches) => {
            walk_for_span_at(scrutinee, byte, interner, best);
            for CaseBranch { pat, body } in branches {
                walk_pat_for_span_at(pat, byte, interner, best);
                walk_for_span_at(body, byte, interner, best);
            }
        }
        Expr_::Binop { lhs, rhs, .. } => {
            walk_for_span_at(lhs, byte, interner, best);
            walk_for_span_at(rhs, byte, interner, best);
        }
        Expr_::If(branches, else_expr) => {
            for (cond, then_) in branches {
                walk_for_span_at(cond, byte, interner, best);
                walk_for_span_at(then_, byte, interner, best);
            }
            walk_for_span_at(else_expr, byte, interner, best);
        }
        Expr_::Tuple(elems) | Expr_::List(elems) => {
            for e in elems {
                walk_for_span_at(e, byte, interner, best);
            }
        }
        Expr_::Cons(h, t) => {
            walk_for_span_at(h, byte, interner, best);
            walk_for_span_at(t, byte, interner, best);
        }
        Expr_::Record(fields) => {
            for (_, v) in fields {
                walk_for_span_at(v, byte, interner, best);
            }
        }
        Expr_::Access(rec, _) => walk_for_span_at(rec, byte, interner, best),
        Expr_::Update(base, fields) => {
            walk_for_span_at(base, byte, interner, best);
            for (_, v) in fields {
                walk_for_span_at(v, byte, interner, best);
            }
        }
        Expr_::ForeignCall { args, .. } => {
            for arg in args {
                walk_for_span_at(arg, byte, interner, best);
            }
        }
        Expr_::VarLocal(_)
        | Expr_::VarTopLevel { .. }
        | Expr_::VarCtor { .. }
        | Expr_::VarKernel { .. }
        | Expr_::Int(_)
        | Expr_::Float(_)
        | Expr_::Str(_)
        | Expr_::PathLit(_)
        | Expr_::CustomElementCtor(_)
        | Expr_::Char(_)
        | Expr_::Unit => {}
    }
}

/// Walk a pattern recording every `PCtor` name-token span containing `byte`.
fn walk_pat_for_span_at(
    pat: &Pattern,
    byte: u32,
    interner: &Interner,
    best: &mut Option<(u32, Span)>,
) {
    match &pat.value {
        Pattern_::PCtor { name, args, .. } => {
            let name_span = ctor_name_span(pat.span.lo, *name, interner);
            if name_span.lo <= byte && byte < name_span.hi {
                consider_span_at(name_span, best);
            }
            for a in args {
                walk_pat_for_span_at(a, byte, interner, best);
            }
        }
        Pattern_::PTuple(elems) | Pattern_::PList(elems) => {
            for e in elems {
                walk_pat_for_span_at(e, byte, interner, best);
            }
        }
        Pattern_::PAlias(inner, _) => walk_pat_for_span_at(inner, byte, interner, best),
        Pattern_::PCons(h, t) => {
            walk_pat_for_span_at(h, byte, interner, best);
            walk_pat_for_span_at(t, byte, interner, best);
        }
        Pattern_::POr(alts) => {
            for a in alts {
                walk_pat_for_span_at(a, byte, interner, best);
            }
        }
        Pattern_::PVar(_)
        | Pattern_::PAnything
        | Pattern_::PDebugAnything
        | Pattern_::PUnit
        | Pattern_::PRecord(_)
        | Pattern_::PInt(_)
        | Pattern_::PBool(_)
        | Pattern_::PChar(_)
        | Pattern_::PStr(_) => {}
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

    // Find the top-level / constructor reference whose span contains `byte`.
    let canonical = crate::db_access::canonicalize_checked(db, root, entry, file)?;
    let (def_home_syms, def_name_sym) = {
        let interner = db.interner().lock();
        find_ref_at(&canonical.module, byte, &interner)?
    };

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

fn find_ref_at(m: &Module, byte: u32, interner: &Interner) -> Option<(Vec<Symbol>, Symbol)> {
    let mut best: Option<(u32, Vec<Symbol>, Symbol)> = None;
    for def in &m.defs {
        let body = match def {
            Def::Untyped { body, .. } | Def::Typed { body, .. } => body,
        };
        walk_for_ref_at(body, byte, interner, &mut best);
    }
    best.map(|(_, home, name)| (home, name))
}

/// Find the name-token span of a declaration named `name` in the parse tree of
/// one module — a top-level value binding, or a data constructor within a
/// `type` union. A constructor's `Located<Ctor>` span begins at its name token;
/// the name width is that identifier's byte length, mirroring the semantic-token
/// walker's convention.
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
    for union in &parsed.unions {
        for ctor in &union.value.ctors {
            let Some(ctor_name) = interner.resolve(ctor.value.name) else {
                continue;
            };
            if ctor_name == name {
                let len = u32::try_from(ctor_name.len()).unwrap_or(0);
                return Some(Span::new(ctor.span.lo, ctor.span.lo.saturating_add(len)));
            }
        }
    }
    drop(interner);
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
        {
            let interner = db.interner().lock();
            for def in &canonical.module.defs {
                let body = match def {
                    Def::Untyped { body, .. } | Def::Typed { body, .. } => body,
                };
                walk_for_refs(body, &home_syms, name_sym, &interner, &mut spans);
            }
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

    // ── Constructor coverage (issue #2653) ───────────────────────────────────
    //
    // `Red` is declared in a `type` union, used as a value in `pick`, and
    // matched as a `case` pattern in `describe`. Both use sites must be visible
    // to find-references, and go-to-definition from the value use must land on
    // the constructor's name token in the union declaration.

    const CTOR_MAIN: &str = "module Main exposing (main)\n\
                             \n\
                             type Color = Red | Green\n\
                             \n\
                             pick : Color\n\
                             pick = Red\n\
                             \n\
                             describe : Color -> Int\n\
                             describe c =\n\
                             \x20   case c of\n\
                             \x20       Red -> 1\n\
                             \x20       Green -> 2\n\
                             \n\
                             main : Int\n\
                             main = describe pick\n";

    fn ctor_value_use_byte() -> u32 {
        // The `Red` in `pick = Red` (the value use), not the declaration.
        let decl = CTOR_MAIN.find("Red | Green").expect("union decl has Red");
        let use_off = CTOR_MAIN[decl + 3..]
            .find("Red")
            .map(|o| decl + 3 + o)
            .expect("value use of Red follows the declaration");
        u32::try_from(use_off).expect("fits u32")
    }

    #[test]
    fn find_references_includes_constructor_value_and_pattern_uses() {
        let db = IpeDatabase::new();
        let entry = file(&db, &["Main"], CTOR_MAIN);
        let root = root_of(&db, &[(&["Main"], entry)]);

        let refs = find_references(&db, root, entry, &["Main".to_owned()], "Red");

        // Two use sites: the value use in `pick`, and the `case` pattern in
        // `describe`. The declaration site itself is not a reference.
        assert_eq!(
            refs.len(),
            2,
            "expected value use + pattern use, got: {refs:?}"
        );
        for r in &refs {
            assert_eq!(r.module, vec!["Main".to_owned()]);
            let lo = r.span.lo as usize;
            let hi = r.span.hi as usize;
            assert_eq!(
                CTOR_MAIN.get(lo..hi),
                Some("Red"),
                "each reference span covers exactly the constructor name"
            );
        }
        // The two spans are distinct positions (value use precedes the pattern).
        let mut los: Vec<u32> = refs.iter().map(|r| r.span.lo).collect();
        los.sort_unstable();
        los.dedup();
        assert_eq!(los.len(), 2, "the two references are at distinct offsets");
    }

    #[test]
    fn goto_definition_resolves_constructor_to_its_union_declaration() {
        let db = IpeDatabase::new();
        let entry = file(&db, &["Main"], CTOR_MAIN);
        let root = root_of(&db, &[(&["Main"], entry)]);

        let def = goto_definition(
            &db,
            root,
            entry,
            &["Main".to_owned()],
            ctor_value_use_byte(),
        )
        .expect("constructor definition found");

        assert_eq!(def.module, vec!["Main".to_owned()]);
        let lo = def.span.lo as usize;
        let hi = def.span.hi as usize;
        assert_eq!(
            CTOR_MAIN.get(lo..hi),
            Some("Red"),
            "definition span covers the constructor name token in the union"
        );
        // The definition is the declaration, not the value use.
        let decl_off = CTOR_MAIN.find("Red | Green").expect("union decl");
        assert_eq!(
            lo, decl_off,
            "definition points at the declaration site, not the use"
        );
    }
}

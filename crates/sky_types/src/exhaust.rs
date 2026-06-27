//! End-of-checking exhaustiveness + redundancy analysis for `case`.
//!
//! Runs after the constraint solver has settled, walking the canonical AST and
//! checking every `case` against the full constructor set of its scrutinee's
//! enum. Two findings are surfaced, both as owned, structured diagnostics:
//!
//! * **SKY-T0010 `NonExhaustiveCase`** — the arms miss at least one
//!   constructor; the missing names are listed in declaration order.
//! * **SKY-T0011 `RedundantCaseBranch`** — a constructor is matched twice.
//!
//! Parse-don't-validate: this is the stage boundary where a loosely-shaped
//! `case` becomes the exhaustive [`sky_ir::Match`]. Catching both defects here
//! is what makes the lowerer's `Match::new` contract (an exhaustive,
//! non-redundant cover) a *genuinely unreachable* compiler-bug case rather than
//! a user-reachable ICE.
//!
//! Patterns that are not nullary constructors (wildcards / variables) are left
//! alone: a wildcard makes the `case` exhaustive, and the M0 lowerer reports the
//! unsupported pattern kind itself (SKY-L0100). This pass only judges the
//! constructor-pattern shape the lowerer can actually build.

use std::collections::{BTreeMap, BTreeSet};

use sky_canon::ast as canon;
use sky_diagnostics::{DResult, Diagnostic, TypeError};
use sky_intern::{Interner, Symbol};

/// `where_` tag for any internal-invariant bug raised while checking.
const STAGE: &str = "intern.resolve";

/// The ordered constructor list of each user enum, keyed by its type name.
type EnumTable = BTreeMap<Symbol, Vec<Symbol>>;

/// Check every `case` in `module` for exhaustiveness + redundancy.
///
/// # Errors
/// * [`TypeError::RedundantCaseBranch`] when a constructor is matched twice.
/// * [`TypeError::NonExhaustiveCase`] when the arms miss a constructor.
/// * [`Diagnostic::CompilerBug`] if a constructor symbol cannot be resolved.
pub fn check(module: &canon::Module, interner: &Interner) -> DResult<()> {
    let enums = build_enum_table(module);
    for def in &module.defs {
        let body = match def {
            canon::Def::Untyped { body, .. } | canon::Def::Typed { body, .. } => body,
        };
        check_expr(body, &enums, interner)?;
    }
    Ok(())
}

/// Index every union's constructors in declaration (`index`) order.
fn build_enum_table(module: &canon::Module) -> EnumTable {
    let mut table = EnumTable::new();
    for union in &module.unions {
        let mut ctors: Vec<&canon::Ctor> = union.ctors.iter().collect();
        ctors.sort_by_key(|c| c.index);
        table.insert(union.name, ctors.into_iter().map(|c| c.name).collect());
    }
    table
}

/// Recursively check a single expression (and its sub-expressions) for `case`
/// defects. The recursion depth is bounded by the parser's nesting cap.
fn check_expr(e: &canon::Expr, enums: &EnumTable, interner: &Interner) -> DResult<()> {
    match &e.value {
        canon::Expr_::Int(_)
        | canon::Expr_::VarLocal(_)
        | canon::Expr_::VarTopLevel { .. }
        | canon::Expr_::VarKernel { .. }
        | canon::Expr_::VarCtor { .. } => Ok(()),
        canon::Expr_::Call(callee, args) => {
            check_expr(callee, enums, interner)?;
            for a in args {
                check_expr(a, enums, interner)?;
            }
            Ok(())
        }
        canon::Expr_::Binop { lhs, rhs, .. } => {
            check_expr(lhs, enums, interner)?;
            check_expr(rhs, enums, interner)
        }
        canon::Expr_::Case(scrut, branches) => {
            check_case(scrut, branches, enums, interner)?;
            check_expr(scrut, enums, interner)?;
            for br in branches {
                check_expr(&br.body, enums, interner)?;
            }
            Ok(())
        }
        canon::Expr_::Let(bindings, body) => {
            for b in bindings {
                check_expr(&b.body, enums, interner)?;
            }
            check_expr(body, enums, interner)
        }
        canon::Expr_::If(branches, else_expr) => {
            for (cond, body) in branches {
                check_expr(cond, enums, interner)?;
                check_expr(body, enums, interner)?;
            }
            check_expr(else_expr, enums, interner)
        }
        canon::Expr_::Tuple(elems) => {
            for elem in elems {
                check_expr(elem, enums, interner)?;
            }
            Ok(())
        }
        canon::Expr_::Record(fields) => {
            for (_, value) in fields {
                check_expr(value, enums, interner)?;
            }
            Ok(())
        }
        canon::Expr_::Access(record, _) => check_expr(record, enums, interner),
        canon::Expr_::Update(base, fields) => {
            check_expr(base, enums, interner)?;
            for (_, value) in fields {
                check_expr(value, enums, interner)?;
            }
            Ok(())
        }
    }
}

/// Check one `case` against its scrutinee enum.
fn check_case(
    scrut: &canon::Expr,
    branches: &[canon::CaseBranch],
    enums: &EnumTable,
    interner: &Interner,
) -> DResult<()> {
    // Determine the scrutinee enum from the first constructor pattern. If any
    // pattern is a wildcard/variable, the `case` is catch-all exhaustive and the
    // unsupported pattern kind is the lowerer's report to make — skip.
    let mut enum_name: Option<Symbol> = None;
    for br in branches {
        match &br.pat.value {
            canon::Pattern_::PCtor { type_name, .. } => {
                if enum_name.is_none() {
                    enum_name = Some(*type_name);
                }
            }
            canon::Pattern_::PAnything | canon::Pattern_::PVar(_) => return Ok(()),
        }
    }
    let Some(enum_name) = enum_name else {
        // No constructor patterns (empty case) — the lowerer reports the empty
        // case; nothing to judge for exhaustiveness here.
        return Ok(());
    };
    let Some(all_ctors) = enums.get(&enum_name) else {
        // Scrutinee type is not a known user enum (cannot happen for a
        // well-typed M0 program); nothing to check.
        return Ok(());
    };

    // Redundancy: first duplicate constructor, in source order.
    let mut seen: BTreeSet<Symbol> = BTreeSet::new();
    for br in branches {
        if let canon::Pattern_::PCtor { name, .. } = &br.pat.value
            && !seen.insert(*name)
        {
            return Err(Diagnostic::Type {
                span: br.pat.span,
                msg: TypeError::RedundantCaseBranch {
                    constructor: resolve(interner, *name)?,
                },
            });
        }
    }

    // Exhaustiveness: every enum constructor must be covered, listed in
    // declaration order.
    let mut missing: Vec<Box<str>> = Vec::new();
    for ctor in all_ctors {
        if !seen.contains(ctor) {
            missing.push(resolve(interner, *ctor)?);
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(Diagnostic::Type {
            span: scrut.span,
            msg: TypeError::NonExhaustiveCase {
                missing: missing.into_boxed_slice(),
            },
        })
    }
}

/// Resolve a constructor symbol to an owned name, or a `CompilerBug` (SKY-I0010)
/// on a forged symbol.
fn resolve(interner: &Interner, sym: Symbol) -> DResult<Box<str>> {
    interner
        .resolve(sym)
        .map(Box::from)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: STAGE,
            detail: format!("no backing string for constructor symbol {}", sym.as_raw()),
        })
}

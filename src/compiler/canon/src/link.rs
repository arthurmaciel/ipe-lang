//! Module linker: merge N canonicalised modules into a single [`ast::Module`].
//!
//! The canonical AST stores every cross-module variable reference as
//! [`ast::Expr_::VarTopLevel { module, name }`], so all references are already
//! fully qualified with their home module's path. The link step therefore needs
//! no renaming — it simply concatenates the unions and top-level definitions from
//! every module (in topological order, dep-first) into one synthetic module.
//!
//! The downstream type-checker, lowerer, and code-generator receive the merged
//! module unchanged; they already emit qualified identifiers
//! (`Module_name`) from the `module` field of every `VarTopLevel` node.

use std::collections::HashSet;

use ipe_diagnostics::{DResult, Diagnostic, NameError, Span};
use ipe_intern::{Interner, Symbol};

use crate::ast;

/// Merge a collection of canonicalised modules into a single [`ast::Module`].
///
/// Modules must be supplied in **dependency-first topological order**: every
/// module's own defs may reference defs from earlier modules in the slice, but
/// not from later ones. Within each module, declarations appear in source order.
///
/// The resulting module's `name` is set to `entry_name` (the path of the
/// user's entry-point module). Downstream stages that key off `module.name`
/// (e.g. package-name selection and `main()` emission)
/// continue to see the entry module's identity unchanged.
///
/// # Correctness invariant
///
/// Every `VarTopLevel { module: m_path, name: n }` node in the merged output
/// must have a corresponding top-level `Def` whose `name.value == n` and whose
/// enclosing module had `name == m_path` before the merge. The caller
/// (the multi-module build driver) is responsible for ensuring this: it must
/// only merge the set of modules discovered and canonicalised by
/// [`crate::canonicalise_module`], which validates every import reference
/// against `ModuleExports` before the caller reaches this function.
///
/// # Errors
///
/// Returns [`NameError::DuplicateType`] (IPE-N0012) when two unions share the
/// SAME nominal identity `(home, name)` — the same type declared twice in one
/// home module. Two DIFFERENT homes declaring the same short name
/// (`Ipe.Palette.Color` and `Main.Color`) are NOT a duplicate: they mangle to
/// distinct Rust enums downstream (`StdPaletteColor` vs `MainColor`), so the
/// gate keys on `(home, name)`, not `name` alone. This makes "two types with
/// the same nominal identity in the linked program" unrepresentable while
/// admitting same-short-name-different-module.
pub fn link(
    entry_name: Vec<Symbol>,
    modules: Vec<ast::Module>,
    interner: &Interner,
) -> DResult<ast::Module> {
    let total_unions: usize = modules.iter().map(|m| m.unions.len()).sum();
    let total_defs: usize = modules.iter().map(|m| m.defs.len()).sum();
    let mut unions = Vec::with_capacity(total_unions);
    let mut defs = Vec::with_capacity(total_defs);
    // The whole-program `unsafe` disclosure is the OR of every linked module's
    // import-derived fact: a program reaches for an escape hatch iff ANY of its
    // modules imported an `Ipe.<M>.Unsafe` submodule.
    let mut imports_unsafe_submodule = false;
    // Nominal-identity gate: reject a genuine duplicate `(home, name)` (the same
    // type declared twice), but ALLOW two distinct homes sharing a short name.
    let mut seen: HashSet<(Vec<Symbol>, Symbol)> = HashSet::new();
    for m in modules {
        imports_unsafe_submodule |= m.imports_unsafe_submodule;
        for u in &m.unions {
            if !seen.insert((u.home.clone(), u.name)) {
                let name = interner
                    .resolve(u.name)
                    .unwrap_or("<?>")
                    .to_owned()
                    .into_boxed_str();
                return Err(Diagnostic::Name {
                    span: Span::DUMMY,
                    msg: NameError::DuplicateType {
                        name,
                        first: Span::DUMMY,
                    },
                });
            }
        }
        unions.extend(m.unions);
        defs.extend(m.defs);
    }
    Ok(ast::Module {
        name: entry_name,
        unions,
        defs,
        imports_unsafe_submodule,
    })
}

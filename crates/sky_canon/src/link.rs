//! Module linker: merge N canonicalised modules into a single [`ast::Module`].
//!
//! The canonical AST stores every cross-module variable reference as
//! [`ast::Expr_::VarTopLevel { module, name }`], so all references are already
//! fully qualified with their home module's path. The link step therefore needs
//! no renaming — it simply concatenates the unions and top-level definitions from
//! every module (in topological order, dep-first) into one synthetic module.
//!
//! The downstream type-checker, lowerer, and code-generator receive the merged
//! module unchanged; they already emit qualified Go identifiers
//! (`Module_name`) from the `module` field of every `VarTopLevel` node.

use sky_intern::Symbol;

use crate::ast;

/// Merge a collection of canonicalised modules into a single [`ast::Module`].
///
/// Modules must be supplied in **dependency-first topological order**: every
/// module's own defs may reference defs from earlier modules in the slice, but
/// not from later ones. Within each module, declarations appear in source order.
///
/// The resulting module's `name` is set to `entry_name` (the path of the
/// user's entry-point module). Downstream stages that key off `module.name`
/// (e.g. the Go-codegen's package-name selection and Go `func main()` emission)
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
#[must_use]
pub fn link(entry_name: Vec<Symbol>, modules: Vec<ast::Module>) -> ast::Module {
    let total_unions: usize = modules.iter().map(|m| m.unions.len()).sum();
    let total_defs: usize = modules.iter().map(|m| m.defs.len()).sum();
    let mut unions = Vec::with_capacity(total_unions);
    let mut defs = Vec::with_capacity(total_defs);
    for m in modules {
        unions.extend(m.unions);
        defs.extend(m.defs);
    }
    ast::Module {
        name: entry_name,
        unions,
        defs,
    }
}

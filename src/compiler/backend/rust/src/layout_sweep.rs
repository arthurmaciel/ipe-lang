//! The layout-budget seam: what the renderer spends laying out each function body.
//!
//! Every body is built and rendered through the same calls the emit path makes
//! for a function body ([`crate::emit_doc::build_doc`] at block indent 1, IR depth
//! 0, then [`crate::render::render_seeded_spend`] at column 4, indent 4), so the
//! spend reported is the spend a real build pays.

use ipe_diagnostics::DResult;
use ipe_intern::{Interner, Symbol};

use crate::EmitCtx;
use crate::emit_types::GenericScope;
use crate::render::{RenderConfig, render_seeded_spend};

pub use crate::render::LAYOUT_FUEL;

/// What laying out one function body spent of [`LAYOUT_FUEL`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyLayoutBudget {
    /// The Rust name of the function whose body was laid out.
    pub func: String,
    /// The fuel the layout search spent, all of [`LAYOUT_FUEL`] when it ran out.
    pub spent: usize,
    /// Whether the search ran out and the body was written in its plain layout.
    pub exhausted: bool,
}

/// The layout spend of every function body in `program`, in module then function order.
///
/// # Errors
/// Propagates any [`ipe_diagnostics::Diagnostic`] from [`EmitCtx::build`] or the
/// body builders.
pub fn body_layout_budgets(
    interner: &Interner,
    program: &ipe_ir::Program,
) -> DResult<Vec<BodyLayoutBudget>> {
    let ctx = EmitCtx::build(
        interner,
        program,
        crate::DbDriver::Sqlite,
        None,
        ipe_ir::Target::Native,
        Vec::new(),
        false,
        None,
        false,
        crate::BuildIntent::Release,
        String::new(),
        false,
        false,
        None,
    )?;
    let mut budgets = Vec::new();
    for module in &program.modules {
        for func in &module.funcs {
            let func_name = ctx.func_name(func.id)?.to_owned();
            let scope_syms: Vec<Symbol> = func.type_params.iter().map(|(s, _)| *s).collect();
            let generics = GenericScope::new(&scope_syms);
            let doc = crate::emit_doc::build_doc(&ctx, &func.body, 1, 0, generics)?;
            let (_, spend) = render_seeded_spend(&doc, RenderConfig::default(), 4, 4);
            budgets.push(BodyLayoutBudget {
                func: func_name,
                spent: spend.spent,
                exhausted: spend.exhausted,
            });
        }
    }
    Ok(budgets)
}

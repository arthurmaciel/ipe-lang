//! Expression and function emission (M0 subset).
//!
//! Ports the M0-relevant arms of `Sky/Generate/Rust/Builder/ExprEmitter.hs` and
//! the function-item shape from `ModuleEmitter.hs`. The byte target is golden
//! `main.rs` lines 129–137 (`main_update` / `sky_main`).

use sky_diagnostics::DResult;
use sky_ir::{BinOp, Callee, Expr, Func, Pat};

use crate::EmitCtx;
use crate::emit_types::render_type;
use crate::naming::kernel_name;

/// One indentation level: four spaces, matching the golden's formatting.
fn indent_of(level: usize) -> String {
    "    ".repeat(level)
}

/// The Rust spelling of an M0 binary operator.
const fn op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
    }
}

/// The Rust name of a call target.
fn callee_name(ctx: &EmitCtx, callee: &Callee) -> DResult<String> {
    match callee {
        Callee::Func(id) => Ok(ctx.func_name(*id)?.to_owned()),
        Callee::Kernel(k) => Ok(kernel_name(*k).to_owned()),
    }
}

/// Emit an expression. `indent` is the indentation level (in 4-space units) of
/// the line the expression *starts* on; it is consumed only by the multi-line
/// `match` form. All other M0 expressions render inline (no leading whitespace,
/// no embedded newlines), so the caller positions them.
///
/// A binary operation is always parenthesised (`(count + 1)`) — matching the
/// golden — so precedence is explicit and never relies on Rust's binding rules.
pub fn emit_expr(ctx: &EmitCtx, expr: &Expr, indent: usize) -> DResult<String> {
    match expr {
        Expr::Int(n) => Ok(n.to_string()),
        Expr::Var(sym) => Ok(ctx.resolve(*sym)?.to_owned()),
        Expr::Ctor { ty, variant } => Ok(format!(
            "{}::{}",
            ctx.enum_name(*ty)?,
            ctx.resolve(*variant)?
        )),
        Expr::BinOp { op, lhs, rhs } => {
            let l = emit_expr(ctx, lhs, indent)?;
            let r = emit_expr(ctx, rhs, indent)?;
            Ok(format!("({} {} {})", l, op_str(*op), r))
        }
        Expr::Call { callee, args } => {
            let name = callee_name(ctx, callee)?;
            let mut parts = Vec::with_capacity(args.len());
            for arg in args {
                parts.push(emit_expr(ctx, arg, indent)?);
            }
            Ok(format!("{name}({})", parts.join(", ")))
        }
        Expr::Match(m) => {
            let scrut = emit_expr(ctx, m.scrutinee(), indent)?;
            let arm_indent = indent_of(indent + 1);
            let close_indent = indent_of(indent);
            let mut arms = Vec::with_capacity(m.arms().len());
            for arm in m.arms() {
                let Pat::Ctor { ty, variant } = &arm.pat;
                let pat = format!("{}::{}", ctx.enum_name(*ty)?, ctx.resolve(*variant)?);
                let body = emit_expr(ctx, &arm.body, indent + 1)?;
                arms.push(format!("{arm_indent}{pat} => {body},"));
            }
            Ok(format!(
                "match {scrut} {{\n{}\n{close_indent}}}",
                arms.join("\n")
            ))
        }
    }
}

/// Emit a whole function item, including its trailing newline.
///
/// Shape: `pub fn <name>(<params>) -> <ret> {\n    <body>\n}\n`. The body is an
/// expression rendered at indentation level 1; the closing brace sits at column
/// 0. Matches golden `main_update` / `sky_main`.
pub fn emit_func(ctx: &EmitCtx, func: &Func) -> DResult<String> {
    let name = ctx.func_name(func.id)?.to_owned();
    let mut params = Vec::with_capacity(func.params.len());
    for (param, ty) in &func.params {
        params.push(format!(
            "{}: {}",
            ctx.resolve(*param)?,
            render_type(ctx, ty)?
        ));
    }
    let ret = render_type(ctx, &func.ret)?;
    let body = emit_expr(ctx, &func.body, 1)?;
    Ok(format!(
        "pub fn {name}({}) -> {ret} {{\n    {body}\n}}\n",
        params.join(", ")
    ))
}

//! Expression and function emission (M0 subset).
//!
//! Ports the M0-relevant arms of `Sky/Generate/Rust/Builder/ExprEmitter.hs` and
//! the function-item shape from `ModuleEmitter.hs`. The byte target is golden
//! `main.rs` lines 129–137 (`main_update` / `sky_main`).

use sky_diagnostics::{DResult, Diagnostic, LowerError, Span};
use sky_ir::{BinOp, Callee, Expr, Func, Pat};

use crate::EmitCtx;
use crate::emit_types::render_type;
use crate::naming::kernel_name;

/// The deepest expression nesting the backend will descend before failing fast.
///
/// `emit_expr` recurses one Rust stack frame per IR-expression level (`BinOp`
/// operands, call arguments, match scrutinee/arm bodies). An adversarially or
/// buggily deep IR spine would otherwise overflow the native stack with no
/// diagnostic. The parser already caps *source* nesting at 256 (SKY-P0003);
/// this matching bound is defence-in-depth against an IR produced past that —
/// well below the native stack ceiling, so the guard fires first.
const MAX_EMIT_DEPTH: u16 = 256;

/// One indentation level: four spaces, matching the golden's formatting.
fn indent_of(level: usize) -> String {
    "    ".repeat(level)
}

/// The Rust spelling of a binary operator. Every Sky M1-core operator maps to
/// the identically-spelled Rust operator except `/=` (Sky inequality), which is
/// Rust's `!=`.
const fn op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Eq => "==",
        BinOp::Neq => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
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
///
/// The bounded entry point: emission starts at depth 0 and fails fast past
/// [`MAX_EMIT_DEPTH`] (SKY-L0200) rather than overflowing the native stack.
pub fn emit_expr(ctx: &EmitCtx, expr: &Expr, indent: usize) -> DResult<String> {
    emit_expr_at(ctx, expr, indent, 0)
}

/// Depth-tracked recursion behind [`emit_expr`]. `depth` is the IR-nesting level
/// of `expr` (0 at the function body); it gates the bounded-emit guard and is
/// independent of `indent` (the textual indentation of `match` arms).
fn emit_expr_at(ctx: &EmitCtx, expr: &Expr, indent: usize, depth: u16) -> DResult<String> {
    if depth > MAX_EMIT_DEPTH {
        return Err(Diagnostic::Lower {
            span: Span::DUMMY,
            msg: LowerError::BackendNestingTooDeep {
                limit: MAX_EMIT_DEPTH,
            },
        });
    }
    let child = depth + 1;
    match expr {
        Expr::Int(n) => Ok(n.to_string()),
        Expr::Var(sym) => ctx.emit_ident(*sym),
        Expr::Ctor { ty, variant } => Ok(format!(
            "{}::{}",
            ctx.enum_name(*ty)?,
            ctx.emit_ident(*variant)?
        )),
        Expr::BinOp { op, lhs, rhs } => {
            let l = emit_expr_at(ctx, lhs, indent, child)?;
            let r = emit_expr_at(ctx, rhs, indent, child)?;
            Ok(format!("({} {} {})", l, op_str(*op), r))
        }
        Expr::Let { name, value, body } => {
            // A `let` expression renders as a parenthesised Rust block so it
            // composes inline anywhere an expression is expected:
            // `({ let <name> = <value>; <body> })`.
            let name = ctx.emit_ident(*name)?;
            let value = emit_expr_at(ctx, value, indent, child)?;
            let body = emit_expr_at(ctx, body, indent, child)?;
            Ok(format!("({{ let {name} = {value}; {body} }})"))
        }
        Expr::If { cond, then_, else_ } => {
            // Parenthesised so the whole `if`/`else` is a single expression
            // value, independent of surrounding precedence.
            let cond = emit_expr_at(ctx, cond, indent, child)?;
            let then_ = emit_expr_at(ctx, then_, indent, child)?;
            let else_ = emit_expr_at(ctx, else_, indent, child)?;
            Ok(format!("(if {cond} {{ {then_} }} else {{ {else_} }})"))
        }
        Expr::Call { callee, args } => {
            let name = callee_name(ctx, callee)?;
            let mut parts = Vec::with_capacity(args.len());
            for arg in args {
                parts.push(emit_expr_at(ctx, arg, indent, child)?);
            }
            Ok(format!("{name}({})", parts.join(", ")))
        }
        Expr::Match(m) => {
            let scrut = emit_expr_at(ctx, m.scrutinee(), indent, child)?;
            let arm_indent = indent_of(indent + 1);
            let close_indent = indent_of(indent);
            let mut arms = Vec::with_capacity(m.arms().len());
            for arm in m.arms() {
                let Pat::Ctor { ty, variant } = &arm.pat;
                let pat = format!("{}::{}", ctx.enum_name(*ty)?, ctx.emit_ident(*variant)?);
                let body = emit_expr_at(ctx, &arm.body, indent + 1, child)?;
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
            ctx.emit_ident(*param)?,
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

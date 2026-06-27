//! Expression and function emission (M0 subset).
//!
//! Ports the M0-relevant arms of `Sky/Generate/Rust/Builder/ExprEmitter.hs` and
//! the function-item shape from `ModuleEmitter.hs`. The byte target is golden
//! `main.rs` lines 129–137 (`main_update` / `sky_main`).

use sky_diagnostics::{DResult, Diagnostic, LowerError, Span};
use sky_intern::Symbol;
use sky_ir::{BinOp, Callee, Expr, Func, IrType, Pat};

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
        Expr::Tuple(elems) => {
            // A tuple constructor renders inline as `(e1, e2, ...)`. The IR
            // invariant guarantees arity ≥ 2, so this is always a genuine Rust
            // tuple; the emission stays total over any vector regardless.
            let mut parts = Vec::with_capacity(elems.len());
            for elem in elems {
                parts.push(emit_expr_at(ctx, elem, indent, child)?);
            }
            Ok(format!("({})", parts.join(", ")))
        }
        // The record arms own several `Vec`/`String` locals; keeping their
        // bodies in dedicated functions (not inlined into this match) holds
        // `emit_expr_at`'s own stack frame small, so the depth guard — not a
        // native overflow — is what bounds a deep `BinOp`/`Call` spine.
        Expr::Record(fields) => emit_record(ctx, fields, indent, depth),
        Expr::Access { record, field } => {
            // Field access `<record>.<field>`. The base is parenthesised so a
            // record literal in record position (`{ ... }.field`) is never
            // misparsed; the field ident is keyword-mangled to match the struct.
            let base = emit_expr_at(ctx, record, indent, child)?;
            let field = ctx.emit_ident(*field)?;
            Ok(format!("({base}).{field}"))
        }
        Expr::Update { record, fields } => emit_update(ctx, record, fields, indent, depth),
        Expr::Lambda { params, ret, body } => emit_lambda(ctx, params, ret, body, indent, depth),
        Expr::Apply { func, args } => emit_apply(ctx, func, args, indent, depth),
        Expr::FuncValue { callee, ty } => emit_func_value(ctx, callee, ty),
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

/// Emit a record literal `{ x = e1, ... }` as a named struct literal
/// `RecXY { x: <e1>, ... }`. `depth` is the literal's own IR-nesting level; its
/// field values are emitted one level deeper. Kept out of the `emit_expr_at`
/// match (`#[inline(never)]`) so its locals don't inflate the recursive frame.
#[inline(never)]
fn emit_record(
    ctx: &EmitCtx,
    fields: &[(Symbol, Expr)],
    indent: usize,
    depth: u16,
) -> DResult<String> {
    let child = depth + 1;
    // The struct is resolved by the literal's field-name set (Rust names
    // struct-literal fields, so write order is free); the field idents are
    // keyword-mangled to match the struct definition.
    let mut key = Vec::with_capacity(fields.len());
    for (sym, _) in fields {
        key.push(ctx.resolve_ident(*sym)?.to_owned());
    }
    let struct_name = ctx.record_name_for_literal(&key)?.to_owned();
    let mut parts = Vec::with_capacity(fields.len());
    for (sym, value) in fields {
        let field_ident = ctx.emit_ident(*sym)?;
        let rendered = emit_expr_at(ctx, value, indent, child)?;
        parts.push(format!("{field_ident}: {rendered}"));
    }
    Ok(format!("{struct_name} {{ {} }}", parts.join(", ")))
}

/// Emit a functional record update `{ record | f = v, ... }` as a clone-and-
/// reassign block: `{ let mut __sky_rec = (<record>).clone(); __sky_rec.f = v;
/// __sky_rec }`. This needs no struct name and leaves the source record
/// untouched; the block scope makes the temporary safe under nesting. Kept out
/// of the match (`#[inline(never)]`) for the same frame-size reason as
/// [`emit_record`].
#[inline(never)]
fn emit_update(
    ctx: &EmitCtx,
    record: &Expr,
    fields: &[(Symbol, Expr)],
    indent: usize,
    depth: u16,
) -> DResult<String> {
    let child = depth + 1;
    let base = emit_expr_at(ctx, record, indent, child)?;
    let mut assigns = Vec::with_capacity(fields.len());
    for (sym, value) in fields {
        let field_ident = ctx.emit_ident(*sym)?;
        let rendered = emit_expr_at(ctx, value, indent, child)?;
        assigns.push(format!(" __sky_rec.{field_ident} = {rendered};"));
    }
    Ok(format!(
        "{{ let mut __sky_rec = ({base}).clone();{} __sky_rec }}",
        assigns.concat()
    ))
}

/// Emit an application of a first-class function value, `(<func>)(<args>)`. The
/// callee is parenthesised so a boxed `dyn Fn` (or any expression value) is
/// applied uniformly — a `Box<dyn Fn(..)>` auto-derefs at the call. `depth` is
/// the application's own IR-nesting level; its callee and arguments are emitted
/// one level deeper. Kept out of the `emit_expr_at` match (`#[inline(never)]`)
/// so its `Vec`/`String` locals don't inflate the recursive frame.
#[inline(never)]
fn emit_apply(
    ctx: &EmitCtx,
    func: &Expr,
    args: &[Expr],
    indent: usize,
    depth: u16,
) -> DResult<String> {
    let child = depth + 1;
    let f = emit_expr_at(ctx, func, indent, child)?;
    let mut parts = Vec::with_capacity(args.len());
    for arg in args {
        parts.push(emit_expr_at(ctx, arg, indent, child)?);
    }
    Ok(format!("({f})({})", parts.join(", ")))
}

/// Emit a top-level function (or kernel) named as a first-class *value* as a
/// type-pinned boxed closure: `{ let __sky_fn: Box<dyn Fn(..) -> R> =
/// Box::new(<name>); __sky_fn }`. The explicit binding type drives the unsized
/// coercion of the named `fn` item (a zero-sized `Fn` implementor) to the boxed
/// trait object, so the value fills a `Box<dyn Fn(..) -> R>` slot uniformly in
/// every position — argument, return, or let-binding — rather than relying on a
/// coercion site that an `if`/`match` branch or a bare `let` would not provide.
/// `ty` is the value's `Fun` IR type; [`render_type`] renders it as the boxed
/// trait object. Kept out of the `emit_expr_at` match (`#[inline(never)]`) for
/// the same frame-size reason as the neighbouring helpers.
#[inline(never)]
fn emit_func_value(ctx: &EmitCtx, callee: &Callee, ty: &IrType) -> DResult<String> {
    let name = callee_name(ctx, callee)?;
    let boxed = render_type(ctx, ty)?;
    Ok(format!(
        "{{ let __sky_fn: {boxed} = Box::new({name}); __sky_fn }}"
    ))
}

/// Emit a lambda `\p0 p1 ... -> body` as a boxed closure
/// `Box::new(move |p0: T0, ...| -> R { <body> })`. The `move` capture takes any
/// free locals by value; the explicit return type pins the closure's signature
/// so it coerces cleanly to the `Box<dyn Fn(..) -> R>` slot it fills. `depth` is
/// the lambda's own IR-nesting level; its body is emitted one level deeper. Kept
/// out of the `emit_expr_at` match (`#[inline(never)]`) for the same frame-size
/// reason as [`emit_record`] / [`emit_update`].
#[inline(never)]
fn emit_lambda(
    ctx: &EmitCtx,
    params: &[(Symbol, IrType)],
    ret: &IrType,
    body: &Expr,
    indent: usize,
    depth: u16,
) -> DResult<String> {
    let child = depth + 1;
    let mut parts = Vec::with_capacity(params.len());
    for (param, ty) in params {
        parts.push(format!(
            "{}: {}",
            ctx.emit_ident(*param)?,
            render_type(ctx, ty)?
        ));
    }
    let ret = render_type(ctx, ret)?;
    let body = emit_expr_at(ctx, body, indent, child)?;
    Ok(format!(
        "Box::new(move |{}| -> {ret} {{ {body} }})",
        parts.join(", ")
    ))
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

//! Expression and function emission (M0 subset).
//!
//! Ports the M0-relevant arms of `Sky/Generate/Rust/Builder/ExprEmitter.hs` and
//! the function-item shape from `ModuleEmitter.hs`. The byte target is golden
//! `main.rs` lines 129–137 (`main_update` / `sky_main`).

use core::fmt::Write as _;

use sky_diagnostics::{DResult, Diagnostic, LowerError, Span};
use sky_intern::Symbol;
use sky_ir::{BinOp, Callee, Expr, Func, IrType, Match, Pat};

use crate::EmitCtx;
use crate::emit_types::{GenericScope, render_type};
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
pub fn emit_expr(
    ctx: &EmitCtx,
    expr: &Expr,
    indent: usize,
    generics: GenericScope,
) -> DResult<String> {
    emit_expr_at(ctx, expr, indent, 0, generics)
}

/// Depth-tracked recursion behind [`emit_expr`]. `depth` is the IR-nesting level
/// of `expr` (0 at the function body); it gates the bounded-emit guard and is
/// independent of `indent` (the textual indentation of `match` arms).
fn emit_expr_at(
    ctx: &EmitCtx,
    expr: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
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
        // The unit value renders as the Rust unit expression `()`.
        Expr::Unit => Ok("()".to_owned()),
        Expr::Var(sym) => ctx.emit_ident(*sym),
        Expr::Ctor { ty, variant, args } => {
            emit_ctor(ctx, *ty, *variant, args, indent, depth, generics)
        }
        Expr::BinOp { op, lhs, rhs } => {
            let l = emit_expr_at(ctx, lhs, indent, child, generics)?;
            let r = emit_expr_at(ctx, rhs, indent, child, generics)?;
            Ok(format!("({} {} {})", l, op_str(*op), r))
        }
        Expr::Let { name, value, body } => {
            // A `let` expression renders as a parenthesised Rust block so it
            // composes inline anywhere an expression is expected:
            // `({ let <name> = <value>; <body> })`.
            let name = ctx.emit_ident(*name)?;
            let value = emit_expr_at(ctx, value, indent, child, generics)?;
            let body = emit_expr_at(ctx, body, indent, child, generics)?;
            Ok(format!("({{ let {name} = {value}; {body} }})"))
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            // An irrefutable destructuring binding renders as a parenthesised
            // Rust block, exactly like `Let`, but with a pattern binder:
            // `({ let <binder> = <value>; <body> })`. The binder is irrefutable
            // (the lowerer guarantees it — a tuple of var / wildcard binders, or
            // a bare var / wildcard), so the `let` is exhaustive Rust.
            let binder = render_pat(ctx, binder)?;
            let value = emit_expr_at(ctx, value, indent, child, generics)?;
            let body = emit_expr_at(ctx, body, indent, child, generics)?;
            Ok(format!("({{ let {binder} = {value}; {body} }})"))
        }
        Expr::If { cond, then_, else_ } => {
            // Parenthesised so the whole `if`/`else` is a single expression
            // value, independent of surrounding precedence.
            let cond = emit_expr_at(ctx, cond, indent, child, generics)?;
            let then_ = emit_expr_at(ctx, then_, indent, child, generics)?;
            let else_ = emit_expr_at(ctx, else_, indent, child, generics)?;
            Ok(format!("(if {cond} {{ {then_} }} else {{ {else_} }})"))
        }
        Expr::Call { callee, args } => {
            let name = callee_name(ctx, callee)?;
            let mut parts = Vec::with_capacity(args.len());
            for arg in args {
                parts.push(emit_expr_at(ctx, arg, indent, child, generics)?);
            }
            Ok(format!("{name}({})", parts.join(", ")))
        }
        Expr::Tuple(elems) => {
            // A tuple constructor renders inline as `(e1, e2, ...)`. The IR
            // invariant guarantees arity ≥ 2, so this is always a genuine Rust
            // tuple; the emission stays total over any vector regardless.
            let mut parts = Vec::with_capacity(elems.len());
            for elem in elems {
                parts.push(emit_expr_at(ctx, elem, indent, child, generics)?);
            }
            Ok(format!("({})", parts.join(", ")))
        }
        // The record arms own several `Vec`/`String` locals; keeping their
        // bodies in dedicated functions (not inlined into this match) holds
        // `emit_expr_at`'s own stack frame small, so the depth guard — not a
        // native overflow — is what bounds a deep `BinOp`/`Call` spine.
        Expr::Record(fields) => emit_record(ctx, fields, indent, depth, generics),
        Expr::Access { record, field } => {
            // Field access `<record>.<field>`. The base is parenthesised so a
            // record literal in record position (`{ ... }.field`) is never
            // misparsed; the field ident is keyword-mangled to match the struct.
            let base = emit_expr_at(ctx, record, indent, child, generics)?;
            let field = ctx.emit_ident(*field)?;
            Ok(format!("({base}).{field}"))
        }
        Expr::Update { record, fields } => {
            emit_update(ctx, record, fields, indent, depth, generics)
        }
        Expr::Lambda { params, ret, body } => {
            emit_lambda(ctx, params, ret, body, indent, depth, generics)
        }
        Expr::Apply { func, args } => emit_apply(ctx, func, args, indent, depth, generics),
        Expr::FuncValue { callee, ty } => emit_func_value(ctx, callee, ty, generics),
        Expr::Match(m) => emit_match(ctx, m, indent, depth, generics),
    }
}

/// Emit a constructor application. A nullary constructor renders as the bare
/// path `EnumName::Variant` (byte-identical to M0); a payload constructor renders
/// `EnumName::Variant(arg0, arg1, …)`. A payload position on a type-size cycle
/// back to its own enum is wrapped in `Box::new(…)` to balance the boxed enum
/// field (see [`crate::EmitCtx::is_cyclic_self_field`]). Kept out of the
/// `emit_expr_at` match (`#[inline(never)]`) so its locals don't inflate the
/// recursive frame.
#[inline(never)]
fn emit_ctor(
    ctx: &EmitCtx,
    ty: Symbol,
    variant: Symbol,
    args: &[Expr],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let path = format!("{}::{}", ctx.enum_name(ty)?, ctx.emit_ident(variant)?);
    if args.is_empty() {
        return Ok(path);
    }
    let fields = ctx.variant_fields(ty, variant)?;
    if fields.len() != args.len() {
        return Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_ctor",
            detail: format!(
                "constructor {} of enum {} applied to {} args but declares {} fields; \
                 a constructor application must be saturated",
                variant.as_raw(),
                ty.as_raw(),
                args.len(),
                fields.len()
            ),
        });
    }
    let mut parts = Vec::with_capacity(args.len());
    for (arg, field_ty) in args.iter().zip(fields.iter()) {
        let rendered = emit_expr_at(ctx, arg, indent, child, generics)?;
        // A cyclic self-edge field is boxed in the enum, so its construction
        // argument is boxed too.
        if ctx.is_cyclic_self_field(field_ty, ty) {
            parts.push(format!("Box::new({rendered})"));
        } else {
            parts.push(rendered);
        }
    }
    Ok(format!("{path}({})", parts.join(", ")))
}

/// Emit a `match`. Each arm's head is a constructor pattern (an exhaustiveness
/// obligation [`sky_ir::Match::new`] guarantees); a payload position binds a
/// variable or is a wildcard. A cyclic self-edge payload field is boxed in
/// the enum, so a variable bound to such a field is unboxed (`let x = *x;`) at the
/// top of the arm body, giving the binder the enum's own (owned) type rather than
/// `Box<…>`. Kept out of the `emit_expr_at` match (`#[inline(never)]`) for the
/// same frame-size reason as the neighbouring helpers.
#[inline(never)]
fn emit_match(
    ctx: &EmitCtx,
    m: &Match,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let scrut = emit_expr_at(ctx, m.scrutinee(), indent, child, generics)?;
    let arm_indent = indent_of(indent + 1);
    let close_indent = indent_of(indent);
    let mut arms = Vec::with_capacity(m.arms().len());
    for arm in m.arms() {
        // The arm head is a constructor pattern (guaranteed by `Match::new`); a
        // non-ctor head is an internal invariant violation, surfaced rather than
        // mis-emitted.
        let Pat::Ctor { ty, variant, args } = &arm.pat else {
            return Err(Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_match",
                detail: "match arm head is not a constructor pattern".to_owned(),
            });
        };
        let path = format!("{}::{}", ctx.enum_name(*ty)?, ctx.emit_ident(*variant)?);
        let (pat, unboxes) = if args.is_empty() {
            (path, String::new())
        } else {
            let fields = ctx.variant_fields(*ty, *variant)?;
            if fields.len() != args.len() {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_match",
                    detail: format!(
                        "constructor pattern {} of enum {} binds {} sub-patterns but the \
                         variant declares {} fields",
                        variant.as_raw(),
                        ty.as_raw(),
                        args.len(),
                        fields.len()
                    ),
                });
            }
            let mut sub_pats = Vec::with_capacity(args.len());
            let mut unbox_lines = String::new();
            for (sub, field_ty) in args.iter().zip(fields.iter()) {
                sub_pats.push(render_pat(ctx, sub)?);
                // A variable bound to a boxed self-edge field is unboxed so the
                // body sees the payload's own type, not `Box<…>`.
                if ctx.is_cyclic_self_field(field_ty, *ty)
                    && let Pat::Var(s) = sub
                {
                    let binder = ctx.emit_ident(*s)?;
                    write!(unbox_lines, "let {binder} = *{binder}; ").map_err(|e| {
                        Diagnostic::CompilerBug {
                            where_: "sky_backend_rust::emit_match",
                            detail: format!("writing unbox binder failed: {e}"),
                        }
                    })?;
                }
            }
            (format!("{path}({})", sub_pats.join(", ")), unbox_lines)
        };
        let body = emit_expr_at(ctx, &arm.body, indent + 1, child, generics)?;
        let arm_body = if unboxes.is_empty() {
            body
        } else {
            format!("{{ {unboxes}{body} }}")
        };
        arms.push(format!("{arm_indent}{pat} => {arm_body},"));
    }
    Ok(format!(
        "match {scrut} {{\n{}\n{close_indent}}}",
        arms.join("\n")
    ))
}

/// Render a pattern to its Rust spelling. Total and recursive over the entire
/// M3a/M3b-1/M3b-2/M3b-3 pattern set:
///
/// * a variable binder (the keyword-mangled name),
/// * a wildcard (`_`),
/// * a literal leaf — int (`0`), bool (`true`), char (`'a'`), string (`"hi"`),
/// * an alias / `as` pattern (`name @ <inner>`),
/// * a tuple pattern (`(sub0, sub1, …)`),
/// * a constructor pattern (`EnumName::Variant` / `EnumName::Variant(sub0, …)`),
/// * a record pattern (`RecXY { x: sub0, y: sub1, .. }`).
///
/// Every nested sub-position recurses through this same function, so an
/// arbitrarily nested shape (`Just (a, b)`, `Node (Node …) x r`,
/// `{ point = (a, b) }`) renders correctly. The renderer stays total: no arm
/// panics, and every fallible lookup is surfaced as a [`Diagnostic`].
fn render_pat(ctx: &EmitCtx, pat: &Pat) -> DResult<String> {
    match pat {
        Pat::Var(sym) => ctx.emit_ident(*sym),
        Pat::Wildcard => Ok("_".to_owned()),
        // Literal leaves render as Rust literals. Int reuses the same spelling as
        // the `Expr::Int` emitter; Bool maps to the Rust keyword constant; Char
        // and Str escape via the `{:?}` Debug form, which produces a valid Rust
        // literal (quotes, backslashes and control chars escaped) and is
        // deterministic.
        Pat::Int(n) => Ok(n.to_string()),
        Pat::Bool(b) => Ok(if *b { "true" } else { "false" }.to_owned()),
        // A well-formed Char pattern carries exactly one character → Rust char
        // literal. A malformed (multi-char / empty) carried string falls back to
        // a string literal rather than emitting invalid Rust, staying total.
        Pat::Char(c) => {
            let mut chars = c.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => Ok(format!("{ch:?}")),
                _ => Ok(format!("{c:?}")),
            }
        }
        Pat::Str(s) => Ok(format!("{s:?}")),
        // `inner as name` → Rust binding-with-subpattern `name @ <inner>`. The
        // inner sub-pattern recurses through this same total renderer.
        Pat::Alias(inner, name) => {
            let name = ctx.emit_ident(*name)?;
            let inner = render_pat(ctx, inner)?;
            Ok(format!("{name} @ {inner}"))
        }
        Pat::Tuple(elems) => {
            // A tuple pattern destructures element-by-element: `(p0, p1, …)`.
            // Stays total over any element vector (no arity assumption).
            let mut subs = Vec::with_capacity(elems.len());
            for sub in elems {
                subs.push(render_pat(ctx, sub)?);
            }
            Ok(format!("({})", subs.join(", ")))
        }
        Pat::Ctor { ty, variant, args } => {
            let path = format!("{}::{}", ctx.enum_name(*ty)?, ctx.emit_ident(*variant)?);
            if args.is_empty() {
                Ok(path)
            } else {
                let mut subs = Vec::with_capacity(args.len());
                for sub in args {
                    subs.push(render_pat(ctx, sub)?);
                }
                Ok(format!("{path}({})", subs.join(", ")))
            }
        }
        Pat::Record(fields) => render_record_pat(ctx, fields),
    }
}

/// Render a record pattern `{ field0 = p0, … }` to a Rust struct pattern
/// `RecXY { field0: p0, …, .. }`.
///
/// The struct is resolved by the pattern's field-name set, exactly as a record
/// LITERAL resolves its struct (Rust names struct-pattern fields, so write order
/// is free). The lowerer surfaces the complete field set, so this exact-set
/// lookup is unambiguous; a miss is an upstream-contract violation surfaced as a
/// [`Diagnostic::CompilerBug`] rather than a silent mis-emit.
///
/// A trailing `..` is always emitted: it both matches the canonical struct-
/// pattern shape and makes the rendering robust to a field the pattern does not
/// bind (zero remaining fields under the complete-set contract — a legal,
/// no-op `..`). A field whose sub-pattern is a variable bound to the field's own
/// name renders in Rust shorthand (`x` rather than the lint-flagged `x: x`).
fn render_record_pat(ctx: &EmitCtx, fields: &[(Symbol, Pat)]) -> DResult<String> {
    // Resolve the struct by the (sorted) set of bound field names.
    let mut key = Vec::with_capacity(fields.len());
    for (sym, _) in fields {
        key.push(ctx.resolve_ident(*sym)?.to_owned());
    }
    let struct_name = ctx.record_name_for_literal(&key)?.to_owned();

    let mut parts = Vec::with_capacity(fields.len());
    for (sym, sub) in fields {
        let field_ident = ctx.emit_ident(*sym)?;
        // Field-pun shorthand: `Rec { x, .. }` instead of `Rec { x: x, .. }`
        // (the latter trips rustc's `non_shorthand_field_patterns` lint). Only
        // when the sub-pattern is a variable whose emitted name equals the
        // field's emitted name.
        if let Pat::Var(var) = sub
            && ctx.emit_ident(*var)? == field_ident
        {
            parts.push(field_ident);
        } else {
            let rendered = render_pat(ctx, sub)?;
            parts.push(format!("{field_ident}: {rendered}"));
        }
    }
    // An empty entry vector is degenerate (the lowerer never produces it), but
    // stay total: render `Rec { .. }` rather than the invalid `Rec { , .. }`.
    if parts.is_empty() {
        Ok(format!("{struct_name} {{ .. }}"))
    } else {
        Ok(format!("{struct_name} {{ {}, .. }}", parts.join(", ")))
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
    generics: GenericScope,
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
        let rendered = emit_expr_at(ctx, value, indent, child, generics)?;
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
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let base = emit_expr_at(ctx, record, indent, child, generics)?;
    let mut assigns = Vec::with_capacity(fields.len());
    for (sym, value) in fields {
        let field_ident = ctx.emit_ident(*sym)?;
        let rendered = emit_expr_at(ctx, value, indent, child, generics)?;
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
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let f = emit_expr_at(ctx, func, indent, child, generics)?;
    let mut parts = Vec::with_capacity(args.len());
    for arg in args {
        parts.push(emit_expr_at(ctx, arg, indent, child, generics)?);
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
fn emit_func_value(
    ctx: &EmitCtx,
    callee: &Callee,
    ty: &IrType,
    generics: GenericScope,
) -> DResult<String> {
    let name = callee_name(ctx, callee)?;
    let boxed = render_type(ctx, ty, generics)?;
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
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let mut parts = Vec::with_capacity(params.len());
    for (param, ty) in params {
        parts.push(format!(
            "{}: {}",
            ctx.emit_ident(*param)?,
            render_type(ctx, ty, generics)?
        ));
    }
    let ret = render_type(ctx, ret, generics)?;
    let body = emit_expr_at(ctx, body, indent, child, generics)?;
    Ok(format!(
        "Box::new(move |{}| -> {ret} {{ {body} }})",
        parts.join(", ")
    ))
}

/// Emit a whole function item, including its trailing newline.
///
/// Shape: `pub fn <name>[<generics>](<params>) -> <ret> {\n    <body>\n}\n`. A
/// monomorphic function (empty `type_params`) emits no generic clause, so its
/// output is byte-identical to the M0 / M1 golden `main_update` / `sky_main`. A
/// fully-parametric function quantifying `[a, b]` emits `pub fn name<T1, T2>(..)`
/// and renders every [`IrType::Generic`] in its signature / body through the
/// matching scope (M2a). The body is an expression rendered at indentation
/// level 1; the closing brace sits at column 0.
pub fn emit_func(ctx: &EmitCtx, func: &Func) -> DResult<String> {
    let name = ctx.func_name(func.id)?.to_owned();
    let generics = GenericScope::new(&func.type_params);

    // The generic clause `<T1, T2, ..>` — one `T{i+1}` per quantified variable
    // in declaration order. Empty for a monomorphic function, so the output
    // matches the pre-M2a golden exactly.
    let generic_clause = if func.type_params.is_empty() {
        String::new()
    } else {
        let names = (1..=func.type_params.len())
            .map(|n| format!("T{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("<{names}>")
    };

    let mut params = Vec::with_capacity(func.params.len());
    for (param, ty) in &func.params {
        params.push(format!(
            "{}: {}",
            ctx.emit_ident(*param)?,
            render_type(ctx, ty, generics)?
        ));
    }
    let ret = render_type(ctx, &func.ret, generics)?;
    let body = emit_expr(ctx, &func.body, 1, generics)?;
    Ok(format!(
        "pub fn {name}{generic_clause}({}) -> {ret} {{\n    {body}\n}}\n",
        params.join(", ")
    ))
}

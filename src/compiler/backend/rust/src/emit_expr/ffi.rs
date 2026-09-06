use super::*;

/// The Rust infix spelling for float binary operators and comparisons.
///
/// `IntAdd`/`IntSub`/`IntMul`/`IntDiv`/`Append` are routed through helpers
/// or `format!` before reaching any infix path and never arrive here.
/// `Add`/`Sub`/`Mul` (polymorphic `Number a`) emit `.ipe_wrapping_add/sub/mul`
/// calls (never infix), so they also never arrive here.
/// All variants are listed so adding a new `BinOp` without wiring it is a
/// compile error rather than a silent gap.
pub const fn op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::FloatAdd => "+",
        BinOp::FloatSub => "-",
        BinOp::FloatMul => "*",
        BinOp::Div => "/",
        BinOp::Eq => "==",
        BinOp::Neq => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        // These route through helpers before reaching here — sentinel strings
        // that are invalid Rust keep the match exhaustive without emitting
        // garbage in case the routing is ever accidentally bypassed.
        BinOp::Add => "ipe_wrapping_add",
        BinOp::Sub => "ipe_wrapping_sub",
        BinOp::Mul => "ipe_wrapping_mul",
        BinOp::IntAdd => "wrapping_add",
        BinOp::IntSub => "wrapping_sub",
        BinOp::IntMul => "wrapping_mul",
        BinOp::IntDiv => "//",
        BinOp::Append => "++",
    }
}

/// Render an `f64` as a Rust literal that is guaranteed to TYPE as `f64`.
///
/// Rust's default `f64` Display drops the decimal point for a whole number
/// (`3.0` prints as `3`), and a bare `3` types as an integer — so a whole-number
/// float literal must keep (or regain) a decimal point. The shortest round-trip
/// Display is used (so the emitted literal parses back to the same bit pattern),
/// and `.0` is appended only when the rendering carries no `.`/`e` exponent
/// marker. A non-finite value (an over-range lexeme reads back as `inf`) can have
/// no decimal literal, so it renders through the `f64` associated constants,
/// keeping the emission total and valid Rust.
pub fn float_literal(f: f64) -> String {
    if f.is_nan() {
        return "f64::NAN".to_owned();
    }
    if f.is_infinite() {
        return if f < 0.0 {
            "f64::NEG_INFINITY"
        } else {
            "f64::INFINITY"
        }
        .to_owned();
    }
    let s = format!("{f}");
    if s.bytes().any(|b| b == b'.' || b == b'e' || b == b'E') {
        s
    } else {
        format!("{s}.0")
    }
}

/// Resolve an FFI wrapper symbol to its fully-qualified `crate::ffi::<name>`
/// path, rejecting any resolved string that is not a legal Rust identifier.
///
/// Both [`callee_name`] (for direct FFI calls) and [`emit_ffi_glued_call`]
/// (for transparent-conversion calls) splice the wrapper name into emitted
/// Rust source via `crate::ffi::{name}`.  A shared validation point ensures
/// neither site can silently emit an illegal identifier regardless of how the
/// symbol was originally interned.
///
/// An illegal name is a compiler invariant failure (the lowerer must have
/// admitted a bad wrapper ident), so this returns [`Diagnostic::CompilerBug`]
/// rather than a user-facing error.
pub fn ffi_path(ctx: &EmitCtx, sym: Symbol) -> DResult<String> {
    let name = ctx.resolve_ident(sym)?;
    let mut chars = name.chars();
    let head_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if head_ok && chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Ok(format!("crate::ffi::{name}"))
    } else {
        Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::ffi_path",
            detail: format!(
                "FFI wrapper ident {name:?} is not a legal Rust identifier; \
                 it must contain only ascii alphanumeric characters and \
                 underscores, starting with a letter or underscore"
            ),
        })
    }
}

/// The Rust name of a call target.
pub fn callee_name(ctx: &EmitCtx, callee: &Callee) -> DResult<String> {
    match callee {
        // Absolute `crate::` path so the call ALWAYS binds to the top-level
        // `fn`, never to a local `let` binder of the same folded name. A local
        // cannot shadow an absolute path, so a local spelled like a top-level
        // fn's Rust name (`let main_update = …` vs `fn main_update`) can no
        // longer intercept the call — closing the E0618 / silent-wrong-call
        // shadow class for every name at once. The `ipe_main` entry point and
        // FFI wrappers are already crate-root, so this is uniform.
        Callee::Func(id) => Ok(format!("crate::{}", ctx.func_name(*id)?)),
        Callee::Kernel(k) => Ok(kernel_name(*k).to_owned()),
        // A foreign wrapper lives in the emitted `src/ffi.rs` module. The
        // shared `ffi_path` helper validates the identifier and constructs the
        // absolute path — an illegal name is a compiler invariant failure.
        Callee::Ffi { ident, .. } => ffi_path(ctx, *ident),
    }
}

/// Does this call target an FFI wrapper with transparent conversion glue?
/// The doc builder keeps such calls as byte-carried leaves so the string
/// emitter's glued rendering is the single source of the emitted text.
pub fn ffi_call_has_glue(ctx: &EmitCtx, callee: &Callee) -> DResult<bool> {
    if let Callee::Ffi { ident, .. } = callee {
        Ok(ctx.ffi_wrapper_glue(*ident)?.is_some())
    } else {
        Ok(false)
    }
}

/// Emit a [`Callee::Ffi`] call through its transparent conversion glue.
///
/// Marked arguments convert Ipê→foreign inline; a glued result converts
/// foreign→Ipê around the call — under the `IpeResult` Ok arm for a fallible
/// wrapper, or over the bare value for an infallible accessor. Unmarked
/// positions render exactly as the generic tail would.
pub fn emit_ffi_glued_call(
    ctx: &EmitCtx,
    wrapper: Symbol,
    glue: &crate::FfiWrapperGlue,
    args: &[Expr],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let name = ffi_path(ctx, wrapper)?;
    let mut parts = Vec::with_capacity(args.len());
    for (i, arg) in args.iter().enumerate() {
        let rendered = emit_expr_at(ctx, arg, indent, depth, generics)?;
        match glue.params.get(i).and_then(Option::as_ref) {
            None => parts.push(rendered),
            Some(t) => parts.push(ffi_to_foreign(ctx, t, &rendered)?),
        }
    }
    let call = format!("{name}({})", parts.join(", "));
    let Some(result) = &glue.result else {
        return Ok(call);
    };
    let conv = ffi_from_foreign(ctx, &result.ty, "__ipe_ffi_v")?;
    if result.in_result {
        Ok(format!(
            "match {call} {{ IpeResult::Ok(__ipe_ffi_v) => IpeResult::Ok({conv}), \
             IpeResult::Err(__ipe_ffi_e) => IpeResult::Err(__ipe_ffi_e) }}"
        ))
    } else {
        Ok(format!("{{ let __ipe_ffi_v = {call}; {conv} }}"))
    }
}

/// Render the Ipê→foreign conversion of `value` (a rendered expression) for
/// one transparent type: a record moves field-for-field into the foreign
/// struct literal; a union matches the app enum into the foreign enum.
pub fn ffi_to_foreign(ctx: &EmitCtx, ty: &crate::FfiGlueType, value: &str) -> DResult<String> {
    match ty {
        crate::FfiGlueType::Record { rust_path, fields } => {
            let moves: Vec<String> = fields
                .iter()
                .map(|f| format!("{f}: __ipe_ffi_r.{f}"))
                .collect();
            Ok(format!(
                "{{ let __ipe_ffi_r = {value}; {rust_path} {{ {} }} }}",
                moves.join(", ")
            ))
        }
        crate::FfiGlueType::Union {
            module,
            name,
            rust_path,
            variants,
        } => {
            let app = ffi_union_app_name(ctx, module, name)?;
            let arms: Vec<String> = variants
                .iter()
                .map(|v| ffi_union_arm(&app, rust_path, v, Direction::ToForeign))
                .collect();
            Ok(format!("match ({value}) {{ {} }}", arms.join(", ")))
        }
    }
}

/// Render the foreign→Ipê conversion of the bound variable `value` for one
/// transparent type: a struct moves field-for-field into the synthesised
/// record struct; an enum matches the foreign enum into the app enum.
pub fn ffi_from_foreign(ctx: &EmitCtx, ty: &crate::FfiGlueType, value: &str) -> DResult<String> {
    match ty {
        crate::FfiGlueType::Record { fields, .. } => {
            // An FFI glue record has a foreign-type-unique field-name set, so
            // field-name resolution is unambiguous — no shape is threaded.
            let rec = ctx.record_name_for_literal(fields, None)?;
            let moves: Vec<String> = fields.iter().map(|f| format!("{f}: {value}.{f}")).collect();
            Ok(format!("{rec} {{ {} }}", moves.join(", ")))
        }
        crate::FfiGlueType::Union {
            module,
            name,
            rust_path,
            variants,
        } => {
            let app = ffi_union_app_name(ctx, module, name)?;
            let arms: Vec<String> = variants
                .iter()
                .map(|v| ffi_union_arm(&app, rust_path, v, Direction::FromForeign))
                .collect();
            Ok(format!("match {value} {{ {} }}", arms.join(", ")))
        }
    }
}

/// Which way a transparent-union match arm converts.
#[derive(Clone, Copy)]
pub enum Direction {
    ToForeign,
    FromForeign,
}

/// One `match` arm converting a transparent enum variant between the app
/// enum (always tuple-shaped — the positional Ipê constructor surface) and
/// the foreign enum (its declared unit/tuple/struct shape).
pub fn ffi_union_arm(
    app: &str,
    rust_path: &str,
    v: &crate::FfiGlueVariant,
    direction: Direction,
) -> String {
    let vn = &v.name;
    let binders: Vec<String> = match &v.payload {
        crate::FfiGluePayload::Unit => Vec::new(),
        crate::FfiGluePayload::Tuple(n) => (0..*n).map(|i| format!("__ipe_ffi_p{i}")).collect(),
        crate::FfiGluePayload::Struct(members) => (0..members.len())
            .map(|i| format!("__ipe_ffi_p{i}"))
            .collect(),
    };
    // The app side is positional; the foreign side re-attaches struct-variant
    // member names.
    let app_side = if binders.is_empty() {
        format!("{app}::{vn}")
    } else {
        format!("{app}::{vn}({})", binders.join(", "))
    };
    let foreign_side = match &v.payload {
        crate::FfiGluePayload::Unit => format!("{rust_path}::{vn}"),
        crate::FfiGluePayload::Tuple(_) => format!("{rust_path}::{vn}({})", binders.join(", ")),
        crate::FfiGluePayload::Struct(members) => {
            let named: Vec<String> = members
                .iter()
                .zip(&binders)
                .map(|(m, b)| format!("{m}: {b}"))
                .collect();
            format!("{rust_path}::{vn} {{ {} }}", named.join(", "))
        }
    };
    match direction {
        Direction::ToForeign => format!("{app_side} => {foreign_side}"),
        Direction::FromForeign => format!("{foreign_side} => {app_side}"),
    }
}

/// The app-side Rust enum name for a transparent union, resolved through the
/// registered `EnumDef` exactly as every other reference to it.
pub fn ffi_union_app_name(ctx: &EmitCtx, module: &[String], name: &str) -> DResult<String> {
    let mut segs = Vec::with_capacity(module.len());
    for m in module {
        segs.push(ctx.lookup_symbol(m)?);
    }
    let name_sym = ctx.lookup_symbol(name)?;
    Ok(ctx.enum_name(&ipe_ir::ModPath(segs), name_sym)?.to_owned())
}

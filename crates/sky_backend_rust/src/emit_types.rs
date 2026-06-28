//! Type emission (M0 subset): user enums and their `SkyStringify` impls, plus
//! IR-type → Rust-type rendering.
//!
//! Ports the M0-relevant arms of `Sky/Generate/Rust/Builder/TypeEmitter.hs`
//! (`unionToRustTypeDef`) and `Emitter.hs` (`typeDefToString` / the enum
//! `skyStringifyEnumImpl`). The byte target is golden `main.rs` lines 31–43.

use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::Symbol;
use sky_ir::{EnumDef, IrType};

use crate::naming::mangle_reserved;
use crate::{EmitCtx, RecordStruct};

/// The generic-type-parameter scope in effect while emitting one function's
/// signature and body (M2a).
///
/// Maps a Sky type-variable [`Symbol`] to its deterministic Rust generic name
/// (`T1`, `T2`, …) by the variable's *position* in the function's quantification
/// order — never by the symbol's spelling — so a function quantifying `[a, b]`
/// renders `a` → `T1` and `b` → `T2` regardless of source naming. Empty for
/// monomorphic functions and for program-level emission (enums, record structs),
/// where no generic is in scope.
///
/// The type is [`Copy`], so it is threaded by value through the emitters.
#[derive(Clone, Copy)]
pub struct GenericScope<'a> {
    params: &'a [Symbol],
}

impl<'a> GenericScope<'a> {
    /// A scope quantifying `params`, in order (`params[i]` → `T{i+1}`).
    #[must_use]
    pub const fn new(params: &'a [Symbol]) -> Self {
        Self { params }
    }

    /// The deterministic Rust generic name for `sym` (`T1`, `T2`, … by position).
    ///
    /// # Errors
    ///
    /// Returns [`Diagnostic::CompilerBug`] when `sym` is not in this scope — the
    /// lowerer is contracted to list every structurally-used type variable in
    /// [`sky_ir::Func::type_params`], so an [`IrType::Generic`] outside the
    /// quantification scope is an internal invariant violation, surfaced rather
    /// than emitted as an undefined Rust identifier.
    fn rust_name(&self, sym: Symbol) -> DResult<String> {
        self.params.iter().position(|p| *p == sym).map_or_else(
            || {
                Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::GenericScope::rust_name",
                    detail: format!(
                        "generic type variable symbol {} is not in the enclosing function's \
                         quantification scope; the lowerer must list every structurally-used \
                         type variable in Func::type_params",
                        sym.as_raw()
                    ),
                })
            },
            |i| Ok(format!("T{}", i.saturating_add(1))),
        )
    }
}

/// Render an IR type to its Rust spelling. `generics` is the enclosing
/// function's generic scope (empty at program level), used to render
/// [`IrType::Generic`] as its deterministic Rust generic name.
pub fn render_type(ctx: &EmitCtx, ty: &IrType, generics: GenericScope) -> DResult<String> {
    Ok(match ty {
        IrType::Int => "i64".to_owned(),
        IrType::Float => "f64".to_owned(),
        IrType::Bool => "bool".to_owned(),
        IrType::Str => "String".to_owned(),
        IrType::Unit => "()".to_owned(),
        IrType::TaskUnit => "SkyTask<()>".to_owned(),
        IrType::Enum(sym) => ctx.enum_name(*sym)?.to_owned(),
        IrType::Tuple(elems) => {
            let mut parts = Vec::with_capacity(elems.len());
            for elem in elems {
                parts.push(render_type(ctx, elem, generics)?);
            }
            format!("({})", parts.join(", "))
        }
        IrType::Record(fields) => ctx.render_record_use(fields, generics)?,
        IrType::Fun(params, ret) => {
            // A first-class function value is a boxed trait object
            // `Box<dyn Fn(T0, ...) -> R>`. A nullary function type renders as
            // `Box<dyn Fn() -> R>`. The boxed-closure optimisation (a concrete,
            // non-boxed generic closure type) is deferred.
            let mut parts = Vec::with_capacity(params.len());
            for param in params {
                parts.push(render_type(ctx, param, generics)?);
            }
            let ret = render_type(ctx, ret, generics)?;
            format!("Box<dyn Fn({}) -> {ret}>", parts.join(", "))
        }
        // A generic type variable renders as the function's corresponding Rust
        // generic (`T1`, `T2`, …), resolved by position in the quantification
        // scope (M2a). No trait bound is emitted — M2a covers only parametric
        // pass-through; constrained variables are rejected upstream.
        IrType::Generic(sym) => generics.rust_name(*sym)?,
    })
}

/// Emit a nullary-variant enum and its derived `SkyStringify` impl, including
/// the trailing newline.
///
/// Shape (matching the golden):
/// ```text
/// #[derive(Clone, Debug, PartialEq)]
/// pub enum MainMsg {
///     Increment,
///     Decrement,
/// }
/// impl SkyStringify for MainMsg {
///     fn sky_show(&self) -> String {
///         match self {
///             MainMsg::Increment => "Increment".to_string(),
///             MainMsg::Decrement => "Decrement".to_string(),
///         }
///     }
/// }
/// ```
pub fn emit_enum(ctx: &EmitCtx, def: &EnumDef) -> DResult<String> {
    let name = ctx.enum_name(def.name)?.to_owned();
    let mut variant_lines = Vec::with_capacity(def.variants.len());
    let mut show_arms = Vec::with_capacity(def.variants.len());
    for variant in &def.variants {
        // The Rust variant ident is keyword-mangled; the `sky_show` string keeps
        // the original Sky name so a variant like `Type` still displays as
        // "Type", not "Type_". For non-keyword variants the two coincide, so the
        // golden stays byte-identical.
        let vn = ctx.emit_ident(*variant)?;
        let display = ctx.resolve_ident(*variant)?;
        variant_lines.push(format!("    {vn},"));
        show_arms.push(format!(
            "            {name}::{vn} => \"{display}\".to_string(),"
        ));
    }
    let variants = variant_lines.join("\n");
    let arms = show_arms.join("\n");
    Ok(format!(
        "#[derive(Clone, Debug, PartialEq)]
pub enum {name} {{
{variants}
}}
impl SkyStringify for {name} {{
    fn sky_show(&self) -> String {{
        match self {{
{arms}
        }}
    }}
}}
"
    ))
}

/// Emit a synthesised record struct and its derived `SkyStringify` impl,
/// including the trailing newline.
///
/// Shape (for `{ x : Int, y : Int }`):
/// ```text
/// #[derive(Clone, Debug, PartialEq)]
/// pub struct RecXY {
///     x: i64,
///     y: i64,
/// }
/// impl SkyStringify for RecXY {
///     fn sky_show(&self) -> String {
///         format!("{{{} {}}}", (&sky_runtime::stringify::Wrap(&self.x)).dispatch(), (&sky_runtime::stringify::Wrap(&self.y)).dispatch())
///     }
/// }
/// ```
///
/// The `sky_show` body mirrors the Go reference's `%v` rendering of a struct
/// (`{f0 f1 ...}`, fields space-separated in declared order, no field names) so
/// stringifying a record reads identically across the two backends. Each field
/// renders through the runtime's total autoref `Wrap(..).dispatch()` shim, which
/// never fails to resolve a method regardless of the field type.
///
/// A GENERIC record shape (M2c — a field typed by a type variable) gains a
/// generic clause on both the struct and its impl. Shape (for `{ value : a }`):
/// ```text
/// #[derive(Clone, Debug, PartialEq)]
/// pub struct RecValue<T1> {
///     value: T1,
/// }
/// impl<T1: SkyStringify + std::fmt::Debug> SkyStringify for RecValue<T1> {
///     ...
/// }
/// ```
/// The impl bounds each parameter `SkyStringify + std::fmt::Debug` so the inline
/// autoref `Wrap(..).dispatch()` resolves at the generic frame (the
/// `SkyStringify` arm is selected with zero autoref, the `Debug` arm is the
/// always-available fallback). `std::fmt::Debug` is spelled in full — the
/// emitted crate's `pub use sky_runtime::*` shadows the `core` crate with the
/// runtime's `core` module, so `core::fmt` would not resolve. A monomorphic
/// record emits an empty clause, so that path is byte-identical to b3.
pub fn emit_record_struct(ctx: &EmitCtx, rec: &RecordStruct) -> DResult<String> {
    let name = &rec.name;
    // The struct's own generic scope: each parameter symbol → `T1`, `T2`, … by
    // position. Empty for a monomorphic record (byte-identical to b3).
    let scope = GenericScope::new(&rec.type_params);
    let mut field_lines = Vec::with_capacity(rec.fields.len());
    let mut show_args = Vec::with_capacity(rec.fields.len());
    for (field_name, field_ty) in &rec.fields {
        let ident = mangle_reserved(field_name.clone());
        let rust_ty = render_type(ctx, field_ty, scope)?;
        field_lines.push(format!("    {ident}: {rust_ty},"));
        show_args.push(format!(
            "(&sky_runtime::stringify::Wrap(&self.{ident})).dispatch()"
        ));
    }
    let fields_block = field_lines.join("\n");

    // Generic clauses: `<T1, T2>` on the struct, `<T1: SkyStringify + Debug, …>`
    // on the impl, `<T1, T2>` on the impl's `for` type. All empty when the record
    // is monomorphic.
    let params: Vec<String> = (1..=rec.type_params.len())
        .map(|i| format!("T{i}"))
        .collect();
    let (decl_clause, impl_bounds, use_clause) = if params.is_empty() {
        (String::new(), String::new(), String::new())
    } else {
        let bounds: Vec<String> = params
            .iter()
            .map(|p| format!("{p}: SkyStringify + std::fmt::Debug"))
            .collect();
        (
            format!("<{}>", params.join(", ")),
            format!("<{}>", bounds.join(", ")),
            format!("<{}>", params.join(", ")),
        )
    };

    // Go `%v` of a struct: `{v0 v1 ...}` — N space-separated `{}` placeholders
    // wrapped in literal braces. With zero fields the rendering is just `{}`.
    let body = if rec.fields.is_empty() {
        "\"{}\".to_string()".to_owned()
    } else {
        let placeholders = vec!["{}"; rec.fields.len()].join(" ");
        let fmt = format!("{{{{{placeholders}}}}}");
        format!("format!(\"{fmt}\", {})", show_args.join(", "))
    };

    Ok(format!(
        "#[derive(Clone, Debug, PartialEq)]
pub struct {name}{decl_clause} {{
{fields_block}
}}
impl{impl_bounds} SkyStringify for {name}{use_clause} {{
    fn sky_show(&self) -> String {{
        {body}
    }}
}}
"
    ))
}

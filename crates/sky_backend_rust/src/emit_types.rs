//! Type emission (M0 subset): user enums and their `SkyStringify` impls, plus
//! IR-type → Rust-type rendering.
//!
//! Ports the M0-relevant arms of `Sky/Generate/Rust/Builder/TypeEmitter.hs`
//! (`unionToRustTypeDef`) and `Emitter.hs` (`typeDefToString` / the enum
//! `skyStringifyEnumImpl`). The byte target is golden `main.rs` lines 31–43.

use sky_diagnostics::DResult;
use sky_ir::{EnumDef, IrType};

use crate::naming::mangle_reserved;
use crate::{EmitCtx, RecordStruct};

/// Render an IR type to its Rust spelling (M0 subset).
pub fn render_type(ctx: &EmitCtx, ty: &IrType) -> DResult<String> {
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
                parts.push(render_type(ctx, elem)?);
            }
            format!("({})", parts.join(", "))
        }
        IrType::Record(fields) => ctx.record_name_for_type(fields)?.to_owned(),
        IrType::Fun(params, ret) => {
            // A first-class function value is a boxed trait object
            // `Box<dyn Fn(T0, ...) -> R>`. A nullary function type renders as
            // `Box<dyn Fn() -> R>`. The boxed-closure optimisation (a concrete,
            // non-boxed generic closure type) is deferred.
            let mut parts = Vec::with_capacity(params.len());
            for param in params {
                parts.push(render_type(ctx, param)?);
            }
            let ret = render_type(ctx, ret)?;
            format!("Box<dyn Fn({}) -> {ret}>", parts.join(", "))
        }
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
pub fn emit_record_struct(ctx: &EmitCtx, rec: &RecordStruct) -> DResult<String> {
    let name = &rec.name;
    let mut field_lines = Vec::with_capacity(rec.fields.len());
    let mut show_args = Vec::with_capacity(rec.fields.len());
    for (field_name, field_ty) in &rec.fields {
        let ident = mangle_reserved(field_name.clone());
        let rust_ty = render_type(ctx, field_ty)?;
        field_lines.push(format!("    {ident}: {rust_ty},"));
        show_args.push(format!(
            "(&sky_runtime::stringify::Wrap(&self.{ident})).dispatch()"
        ));
    }
    let fields_block = field_lines.join("\n");

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
pub struct {name} {{
{fields_block}
}}
impl SkyStringify for {name} {{
    fn sky_show(&self) -> String {{
        {body}
    }}
}}
"
    ))
}

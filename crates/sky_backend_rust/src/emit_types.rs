//! Type emission (M0 subset): user enums and their `SkyStringify` impls, plus
//! IR-type → Rust-type rendering.
//!
//! Ports the M0-relevant arms of `Sky/Generate/Rust/Builder/TypeEmitter.hs`
//! (`unionToRustTypeDef`) and `Emitter.hs` (`typeDefToString` / the enum
//! `skyStringifyEnumImpl`). The byte target is golden `main.rs` lines 31–43.

use sky_diagnostics::DResult;
use sky_ir::{EnumDef, IrType};

use crate::EmitCtx;

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
        let vn = ctx.resolve(*variant);
        variant_lines.push(format!("    {vn},"));
        show_arms.push(format!("            {name}::{vn} => \"{vn}\".to_string(),"));
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

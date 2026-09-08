//! The permissive WIRE layer — the single place the `ipe-ffi-inspector` JSON
//! schema is spelled on the decode side.
//!
//! Every optional key is defaulted and unknown keys are ignored, so the
//! inspector can add fields without breaking an older compiler. Nothing here
//! is trusted: each struct is a byte-mirror of the inspector's output, and the
//! validating conversion in the parent module (`TryFrom<Wire…>`) is the only
//! path a value takes into the typed DOMAIN layer.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct WireParam {
    #[serde(default)]
    pub(super) name: String,
    #[serde(rename = "type")]
    pub(super) ty: String,
    #[serde(default, rename = "ipeType")]
    pub(super) ipe_type: String,
    #[serde(default, rename = "rustType")]
    pub(super) rust_type: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct WireGeneric {
    pub(super) params: Vec<String>,
    #[serde(default)]
    pub(super) bounds: BTreeMap<String, Vec<String>>,
    pub(super) call: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // byte-mirrors the inspector's flag wire shape; the domain layer collapses them into FnShape
pub(super) struct WireFunction {
    pub(super) name: String,
    #[serde(default)]
    pub(super) params: Vec<WireParam>,
    #[serde(default)]
    pub(super) results: Vec<WireParam>,
    #[serde(default)]
    pub(super) variadic: bool,
    pub(super) effect: String,
    #[serde(default, rename = "recvType")]
    pub(super) recv_type: String,
    #[serde(default, rename = "recvRustType")]
    pub(super) recv_rust_type: String,
    #[serde(default, rename = "methodName")]
    pub(super) method_name: String,
    #[serde(default, rename = "isField")]
    pub(super) is_field: bool,
    #[serde(default, rename = "isFieldSet")]
    pub(super) is_field_set: bool,
    #[serde(default, rename = "isPkgVar")]
    pub(super) is_pkg_var: bool,
    #[serde(default, rename = "selfReturning")]
    pub(super) self_returning: bool,
    #[serde(default, rename = "isEnumCtor")]
    pub(super) is_enum_ctor: bool,
    #[serde(default, rename = "isEnumTag")]
    pub(super) is_enum_tag: bool,
    #[serde(default, rename = "isEnumExtract")]
    pub(super) is_enum_extract: bool,
    #[serde(default, rename = "enumVariant")]
    pub(super) enum_variant: String,
    #[serde(default, rename = "enumKind")]
    pub(super) enum_kind: String,
    #[serde(default, rename = "enumStructFields")]
    pub(super) enum_struct_fields: Vec<String>,
    #[serde(default, rename = "enumFieldCount")]
    pub(super) enum_field_count: u64,
    #[serde(default, rename = "enumArms")]
    pub(super) enum_arms: Vec<String>,
    #[serde(default, rename = "enumWildcard")]
    pub(super) enum_wildcard: bool,
    #[serde(default, rename = "isClosureAdapter")]
    pub(super) is_closure_adapter: bool,
    #[serde(default, rename = "closureSig")]
    pub(super) closure_sig: String,
    #[serde(default, rename = "isStructCtor")]
    pub(super) is_struct_ctor: bool,
    #[serde(default, rename = "structName")]
    pub(super) struct_name: String,
    #[serde(default, rename = "structFields")]
    pub(super) struct_ctor_fields: Vec<WireStructField>,
    #[serde(default, rename = "structDerives")]
    pub(super) struct_derives: Vec<String>,
    #[serde(default, rename = "isEnumDef")]
    pub(super) is_enum_def: bool,
    #[serde(default, rename = "enumName")]
    pub(super) enum_def_name: String,
    #[serde(default, rename = "enumVariants")]
    pub(super) enum_def_variants: Vec<WireEnumVariant>,
    #[serde(default, rename = "enumDerives")]
    pub(super) enum_def_derives: Vec<String>,
    #[serde(default)]
    pub(super) generic: Option<WireGeneric>,
    #[serde(default, rename = "callPath")]
    pub(super) call_path: String,
}

/// One inspected public constant: its crate-relative path (`f64::consts::PI`)
/// and its Rust type (`f64`). The consumer validates both at decode.
#[derive(Debug, Deserialize)]
pub(super) struct WireConstant {
    #[serde(default)]
    pub(super) path: String,
    #[serde(default, rename = "type")]
    pub(super) ty: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct WirePkgInfo {
    pub(super) pkg: String,
    pub(super) name: String,
    #[serde(default)]
    pub(super) version: String,
    #[serde(default)]
    pub(super) functions: Vec<WireFunction>,
    #[serde(default)]
    pub(super) constants: Vec<WireConstant>,
    #[serde(default)]
    pub(super) modules: Vec<String>,
    #[serde(default)]
    pub(super) errors: Vec<String>,
    #[serde(default)]
    pub(super) notes: Vec<String>,
    #[serde(default, rename = "transitiveDeps")]
    pub(super) transitive_deps: Vec<WireTransitiveDep>,
    #[serde(default)]
    pub(super) features: Vec<String>,
    #[serde(default, rename = "foreignTypeIds")]
    pub(super) foreign_type_ids: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub(super) types: Vec<crate::transparency::WireForeignType>,
    /// Author-DECLARED opaque handles (`foreign X = { kind = Opaque "Type" }`):
    /// Ipê handle nominal → the resolved absolute Rust path of a reported crate
    /// type. Injected by the CLI's `merge_provides` from the project's `foreign`
    /// declarations, already validated against this crate's reported types; the
    /// decode below re-validates the path shape (the value is spliced into
    /// emitted Rust). Empty for an ordinary inspection with no declarations.
    #[serde(default, rename = "declaredOpaques")]
    pub(super) declared_opaques: std::collections::BTreeMap<String, String>,
    #[serde(default, rename = "wrapperPath")]
    pub(super) wrapper_path: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct WireTransitiveDep {
    pub(super) ident: String,
    pub(super) name: String,
    pub(super) version: String,
}

/// One `[[rust.define.struct]]` field: a name and its carrier spelling. The
/// carrier is validated at decode (`StructDef::parse`), never rendered raw.
#[derive(Debug, Deserialize)]
pub(super) struct WireStructField {
    #[serde(default)]
    pub(super) name: String,
    #[serde(default, rename = "type")]
    pub(super) ty: String,
}

/// One `[[rust.define.enum]]` variant: a name and its positional payload
/// carrier spellings (empty ⇒ a unit variant). Each spelling is validated at
/// decode (`EnumDef::parse`), never rendered raw.
#[derive(Debug, Deserialize)]
pub(super) struct WireEnumVariant {
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) payload: Vec<String>,
}

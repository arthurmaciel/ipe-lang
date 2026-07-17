//! The `PkgInfo` decode boundary — where inspector output enters the typed
//! world.
//!
//! A permissive WIRE layer byte-mirrors the `ipe-ffi-inspector` JSON (every
//! optional key defaulted, unknown keys ignored for forward compatibility).
//! The DOMAIN layer is constructed only through the validating conversion:
//! identifiers become [`RustIdent`]s, the accessor-flag soup collapses into
//! the closed [`FnShape`] sum, the effect string becomes the closed
//! [`Effect`] enum, and each parametric `generic` block's call AST passes the
//! [`Call`] gate. A defective FUNCTION is over-dropped (recorded, package
//! kept); a defective PACKAGE header fails the decode.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::call::Call;
use crate::diag::{Diagnostic, WireDefect};
use crate::naming::{RustIdent, wrapper_ref_name};

// ── wire layer ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct WireParam {
    #[serde(default)]
    name: String,
    #[serde(rename = "type")]
    ty: String,
    #[serde(default, rename = "ipeType")]
    ipe_type: String,
    #[serde(default, rename = "rustType")]
    rust_type: String,
}

#[derive(Debug, Deserialize)]
struct WireGeneric {
    params: Vec<String>,
    #[serde(default)]
    bounds: BTreeMap<String, Vec<String>>,
    call: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // byte-mirrors the inspector's flag wire shape; the domain layer collapses them into FnShape
struct WireFunction {
    name: String,
    #[serde(default)]
    params: Vec<WireParam>,
    #[serde(default)]
    results: Vec<WireParam>,
    #[serde(default)]
    variadic: bool,
    effect: String,
    #[serde(default, rename = "recvType")]
    recv_type: String,
    #[serde(default, rename = "recvRustType")]
    recv_rust_type: String,
    #[serde(default, rename = "methodName")]
    method_name: String,
    #[serde(default, rename = "isField")]
    is_field: bool,
    #[serde(default, rename = "isFieldSet")]
    is_field_set: bool,
    #[serde(default, rename = "isPkgVar")]
    is_pkg_var: bool,
    #[serde(default, rename = "selfReturning")]
    self_returning: bool,
    #[serde(default, rename = "isEnumCtor")]
    is_enum_ctor: bool,
    #[serde(default, rename = "isEnumTag")]
    is_enum_tag: bool,
    #[serde(default, rename = "isEnumExtract")]
    is_enum_extract: bool,
    #[serde(default, rename = "enumVariant")]
    enum_variant: String,
    #[serde(default, rename = "enumKind")]
    enum_kind: String,
    #[serde(default, rename = "enumStructFields")]
    enum_struct_fields: Vec<String>,
    #[serde(default, rename = "enumFieldCount")]
    enum_field_count: u64,
    #[serde(default, rename = "enumArms")]
    enum_arms: Vec<String>,
    #[serde(default, rename = "enumWildcard")]
    enum_wildcard: bool,
    #[serde(default)]
    generic: Option<WireGeneric>,
    #[serde(default, rename = "callPath")]
    call_path: String,
}

#[derive(Debug, Deserialize)]
struct WirePkgInfo {
    pkg: String,
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    functions: Vec<WireFunction>,
    #[serde(default)]
    modules: Vec<String>,
    #[serde(default)]
    errors: Vec<String>,
    #[serde(default)]
    notes: Vec<String>,
    #[serde(default, rename = "transitiveDeps")]
    transitive_deps: Vec<WireTransitiveDep>,
    #[serde(default)]
    features: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct WireTransitiveDep {
    ident: String,
    name: String,
    version: String,
}

// ── domain layer ────────────────────────────────────────────────────────────

/// How a foreign function's effect is classified by the inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// No observable side effect.
    Pure,
    /// Pure but may fail (`Result`-returning).
    Fallible,
    /// Performs I/O or other observable effects.
    Effectful,
}

/// Whether the binding's Ipê-visible type is wrapped in the fallible carrier.
///
/// Decoded ONCE here; both the `.ipei` and `kernel.json` emitters read this
/// same bit, so the two artifacts cannot disagree on getter fallibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallibility {
    /// Projection/match/construct body — never fails; no `Result` wrapper.
    Infallible,
    /// Every other wrapper: the result is `Result Error a` / `Task Error a`.
    TaskError,
}

/// The kind of an enum accessor's target variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumVariantKind {
    /// A payload-free variant.
    Unit,
    /// Positional payload fields.
    Tuple,
    /// Named payload fields.
    Struct,
}

/// One arm of an enum tag accessor's generated `match`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumArm {
    /// The Rust pattern (`A`, `B(..)`, `C{..}`).
    pub pattern: String,
    /// The tag string the arm returns.
    pub tag: String,
}

/// The closed sum the mutually-exclusive accessor flags collapse into.
///
/// Two flags set at once is [`Diagnostic::ShapeContradiction`] — that one
/// binding is dropped, the package survives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FnShape {
    /// An ordinary free function or method.
    Plain,
    /// A synthetic struct-field getter.
    FieldGet,
    /// A synthetic struct-field setter.
    FieldSet,
    /// A synthetic package-level var/const getter.
    PkgVar,
    /// An enum-variant constructor (`E::Variant(args)`).
    EnumCtor {
        /// The Rust variant identifier.
        variant: RustIdent,
        /// The variant's payload kind.
        kind: EnumVariantKind,
        /// Struct-variant field names in declaration order.
        struct_fields: Vec<String>,
    },
    /// An enum tag accessor (exhaustive `match` returning the variant name).
    EnumTag {
        /// The generated match arms.
        arms: Vec<EnumArm>,
        /// Whether the match needs a trailing `_ =>` wildcard arm.
        wildcard: bool,
    },
    /// A single-field payload extractor (`E -> Maybe T`).
    EnumExtract {
        /// The Rust variant identifier.
        variant: RustIdent,
        /// The variant's payload kind.
        kind: EnumVariantKind,
        /// The selected binder: the field NAME (struct variant) or the
        /// positional index as a string (tuple variant).
        selector: String,
        /// The variant's total field arity (tuple extractors bind every
        /// position before returning the selected one).
        field_count: u64,
        /// Whether the match needs a trailing `_ =>` wildcard arm.
        wildcard: bool,
    },
}

/// One foreign parameter or result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    /// The parameter name (may be empty).
    pub name: String,
    /// The foreign Rust type string, verbatim from the inspector.
    pub foreign_ty: String,
    /// The inspector's Ipê-side type override (empty ⇒ derive from
    /// `foreign_ty`).
    pub ipe_type: String,
    /// The inspector's Rust-side type override for wrapper emission.
    pub rust_type: String,
}

/// A parametric generic block: type-param names, per-param trait bounds, and
/// the validated call AST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericFn {
    /// Type-param names in Ipê-source order (positional with call param refs).
    pub params: Vec<String>,
    /// Per-param trait bound names.
    pub bounds: BTreeMap<String, Vec<String>>,
    /// The validated call AST.
    pub call: Call,
}

/// One bindable foreign function, fully validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnInfo {
    name: RustIdent,
    params: Vec<Param>,
    results: Vec<Param>,
    variadic: bool,
    effect: Effect,
    recv_type: String,
    recv_rust_type: String,
    method_name: String,
    shape: FnShape,
    fallibility: Fallibility,
    self_returning: bool,
    generic: Option<GenericFn>,
    call_path: String,
}

impl FnInfo {
    /// The inspector-assigned function name.
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// The disambiguated wrapper-reference name (the tri-artifact key).
    #[must_use]
    pub fn wrapper_ref_name(&self) -> String {
        wrapper_ref_name(self.name.as_str(), &self.recv_type)
    }

    /// The receiver type name (empty for a free function).
    #[must_use]
    pub fn recv_type(&self) -> &str {
        &self.recv_type
    }

    /// The receiver's Rust type override (empty when unknown).
    #[must_use]
    pub fn recv_rust_type(&self) -> &str {
        &self.recv_rust_type
    }

    /// The host method name (empty for a free function).
    #[must_use]
    pub fn method_name(&self) -> &str {
        &self.method_name
    }

    /// The foreign parameters.
    #[must_use]
    pub fn params(&self) -> &[Param] {
        &self.params
    }

    /// The foreign results.
    #[must_use]
    pub fn results(&self) -> &[Param] {
        &self.results
    }

    /// Whether the last foreign param is variadic.
    #[must_use]
    pub const fn variadic(&self) -> bool {
        self.variadic
    }

    /// The inspector's effect classification.
    #[must_use]
    pub const fn effect(&self) -> Effect {
        self.effect
    }

    /// The collapsed accessor shape.
    #[must_use]
    pub const fn shape(&self) -> &FnShape {
        &self.shape
    }

    /// The single stored fallibility bit (both emitters read this).
    #[must_use]
    pub const fn fallibility(&self) -> Fallibility {
        self.fallibility
    }

    /// Whether the method is an owned-threading setter (`&mut self` receiver
    /// whose wrapper moves, mutates, and returns the receiver).
    #[must_use]
    pub const fn self_returning(&self) -> bool {
        self.self_returning
    }

    /// The parametric generic block, when present.
    #[must_use]
    pub const fn generic(&self) -> Option<&GenericFn> {
        self.generic.as_ref()
    }

    /// Crate-relative call path for a submodule free fn (empty otherwise).
    #[must_use]
    pub fn call_path(&self) -> &str {
        &self.call_path
    }
}

/// One resolved crate from the introspection probe's `cargo metadata`.
///
/// Maps the lib identifier to the canonical package name + exact locked
/// version. The manifest emitter reads this so it never guesses `_`→`-` or
/// emits `"*"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitiveDep {
    /// The Rust lib-target identifier (the `::<ident>::…` path segment).
    pub ident: RustIdent,
    /// The canonical package name (the Cargo `[dependencies]` key).
    pub name: String,
    /// The exact resolved version.
    pub version: String,
}

/// A fully-validated package inspection result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkgInfo {
    pkg_path: String,
    name: RustIdent,
    version: String,
    fns: Vec<FnInfo>,
    modules: Vec<String>,
    errors: Vec<String>,
    notes: Vec<String>,
    transitive_deps: Vec<TransitiveDep>,
    features: Vec<String>,
    dropped: Vec<Diagnostic>,
}

impl PkgInfo {
    /// Decode one `PkgInfo` JSON document (the inspector's single-crate
    /// output) through the validating domain conversion.
    ///
    /// # Errors
    ///
    /// A package-level defect (JSON shape, illegal crate name) fails the
    /// decode. A function-level defect drops that one binding into
    /// [`PkgInfo::dropped`] — over-drop, never under-bind.
    pub fn decode_json(text: &str) -> Result<Self, Diagnostic> {
        let wire: WirePkgInfo =
            serde_json::from_str(text).map_err(|e| Diagnostic::WireMalformed {
                context: "package inspection document".to_owned(),
                defect: WireDefect::Json {
                    detail: e.to_string(),
                },
            })?;
        Self::try_from(wire)
    }

    /// The crate path as given to the inspector.
    #[must_use]
    pub fn pkg_path(&self) -> &str {
        &self.pkg_path
    }

    /// The validated crate name.
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// The exact resolved crate version (may be empty on inspector failure).
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The validated bindable functions.
    #[must_use]
    pub fn fns(&self) -> &[FnInfo] {
        &self.fns
    }

    /// Public module paths to glob-import in generated wrappers.
    #[must_use]
    pub fn modules(&self) -> &[String] {
        &self.modules
    }

    /// The inspector's fail-closed error channel. Non-empty means the
    /// inspection is unusable; the driver refuses to emit from it.
    #[must_use]
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// Diagnostic notes for the `ipe add` user.
    #[must_use]
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Every crate resolved by the introspection probe.
    #[must_use]
    pub fn transitive_deps(&self) -> &[TransitiveDep] {
        &self.transitive_deps
    }

    /// The effective feature set the introspection succeeded with.
    #[must_use]
    pub fn features(&self) -> &[String] {
        &self.features
    }

    /// The bindings dropped by the validating conversion, with the reason
    /// each was refused — the over-drop keystone made visible.
    #[must_use]
    pub fn dropped(&self) -> &[Diagnostic] {
        &self.dropped
    }
}

fn decode_effect(function: &str, s: &str) -> Result<Effect, Diagnostic> {
    match s {
        "pure" => Ok(Effect::Pure),
        "fallible" => Ok(Effect::Fallible),
        "effectful" => Ok(Effect::Effectful),
        _ => Err(Diagnostic::WireMalformed {
            context: format!("function `{function}`"),
            defect: WireDefect::UnknownEffect { got: s.to_owned() },
        }),
    }
}

fn decode_variant_kind(function: &str, s: &str) -> Result<EnumVariantKind, Diagnostic> {
    match s {
        "unit" => Ok(EnumVariantKind::Unit),
        "tuple" => Ok(EnumVariantKind::Tuple),
        "struct" => Ok(EnumVariantKind::Struct),
        _ => Err(Diagnostic::WireMalformed {
            context: format!("function `{function}`"),
            defect: WireDefect::Json {
                detail: format!(
                    "unknown enum variant kind {s:?} (expected \"unit\", \"tuple\", or \"struct\")"
                ),
            },
        }),
    }
}

fn decode_arms(function: &str, arms: Vec<String>) -> Result<Vec<EnumArm>, Diagnostic> {
    arms.into_iter()
        .map(|raw| {
            raw.split_once('\t')
                .map(|(pattern, tag)| EnumArm {
                    pattern: pattern.to_owned(),
                    tag: tag.to_owned(),
                })
                .ok_or_else(|| Diagnostic::WireMalformed {
                    context: format!("function `{function}`"),
                    defect: WireDefect::Json {
                        detail: format!("enum tag arm {raw:?} is not \"<pattern>\\t<tag>\"-shaped"),
                    },
                })
        })
        .collect()
}

/// Collapse the six mutually-exclusive accessor flags into the closed shape.
fn decode_shape(w: &WireFunction) -> Result<FnShape, Diagnostic> {
    let flags: [(&'static str, bool); 6] = [
        ("isField", w.is_field),
        ("isFieldSet", w.is_field_set),
        ("isPkgVar", w.is_pkg_var),
        ("isEnumCtor", w.is_enum_ctor),
        ("isEnumTag", w.is_enum_tag),
        ("isEnumExtract", w.is_enum_extract),
    ];
    let set: Vec<&'static str> = flags.iter().filter(|(_, b)| *b).map(|(n, _)| *n).collect();
    if set.len() > 1 {
        return Err(Diagnostic::ShapeContradiction {
            function: w.name.clone(),
            flags: set,
        });
    }
    let ident_field = |s: &str| -> Result<RustIdent, Diagnostic> {
        RustIdent::parse(s).map_err(|defect| Diagnostic::WireMalformed {
            context: format!("function `{}`", w.name),
            defect,
        })
    };
    Ok(match set.first().copied() {
        None => FnShape::Plain,
        Some("isField") => FnShape::FieldGet,
        Some("isFieldSet") => FnShape::FieldSet,
        Some("isPkgVar") => FnShape::PkgVar,
        Some("isEnumCtor") => FnShape::EnumCtor {
            variant: ident_field(&w.enum_variant)?,
            kind: decode_variant_kind(&w.name, &w.enum_kind)?,
            struct_fields: w.enum_struct_fields.clone(),
        },
        Some("isEnumTag") => FnShape::EnumTag {
            arms: decode_arms(&w.name, w.enum_arms.clone())?,
            wildcard: w.enum_wildcard,
        },
        Some(_) => FnShape::EnumExtract {
            variant: ident_field(&w.enum_variant)?,
            kind: decode_variant_kind(&w.name, &w.enum_kind)?,
            selector: w.enum_struct_fields.first().cloned().unwrap_or_default(),
            field_count: w.enum_field_count,
            wildcard: w.enum_wildcard,
        },
    })
}

const fn shape_fallibility(shape: &FnShape) -> Fallibility {
    match shape {
        FnShape::FieldGet
        | FnShape::FieldSet
        | FnShape::EnumCtor { .. }
        | FnShape::EnumTag { .. }
        | FnShape::EnumExtract { .. } => Fallibility::Infallible,
        FnShape::Plain | FnShape::PkgVar => Fallibility::TaskError,
    }
}

fn param_from_wire(w: WireParam) -> Param {
    Param {
        name: w.name,
        foreign_ty: w.ty,
        ipe_type: w.ipe_type,
        rust_type: w.rust_type,
    }
}

impl TryFrom<WireFunction> for FnInfo {
    type Error = Diagnostic;

    fn try_from(w: WireFunction) -> Result<Self, Diagnostic> {
        let context = |defect: WireDefect| Diagnostic::WireMalformed {
            context: format!("function `{}`", w.name),
            defect,
        };
        let name = RustIdent::parse(&w.name).map_err(&context)?;
        if !w.method_name.is_empty() {
            RustIdent::parse(&w.method_name).map_err(&context)?;
        }
        if !w.call_path.is_empty() {
            crate::naming::IdentPath::parse(&w.call_path).map_err(&context)?;
        }
        let effect = decode_effect(&w.name, &w.effect)?;
        let shape = decode_shape(&w)?;
        let fallibility = shape_fallibility(&shape);
        let generic = match &w.generic {
            None => None,
            Some(g) => {
                let refname = wrapper_ref_name(&w.name, &w.recv_type);
                let call = Call::decode(g.params.len(), g.call.clone(), &refname)?;
                Some(GenericFn {
                    params: g.params.clone(),
                    bounds: g.bounds.clone(),
                    call,
                })
            }
        };
        Ok(Self {
            name,
            params: w.params.into_iter().map(param_from_wire).collect(),
            results: w.results.into_iter().map(param_from_wire).collect(),
            variadic: w.variadic,
            effect,
            recv_type: w.recv_type,
            recv_rust_type: w.recv_rust_type,
            method_name: w.method_name,
            shape,
            fallibility,
            self_returning: w.self_returning,
            generic,
            call_path: w.call_path,
        })
    }
}

impl TryFrom<WirePkgInfo> for PkgInfo {
    type Error = Diagnostic;

    fn try_from(w: WirePkgInfo) -> Result<Self, Diagnostic> {
        let name = RustIdent::parse(&w.name).map_err(|defect| Diagnostic::WireMalformed {
            context: format!("crate `{}`", w.name),
            defect,
        })?;
        let mut fns = Vec::with_capacity(w.functions.len());
        let mut dropped = Vec::new();
        for wf in w.functions {
            match FnInfo::try_from(wf) {
                Ok(f) => fns.push(f),
                // Over-drop: the one defective binding is refused and
                // recorded; every other binding in the package survives.
                Err(d) => dropped.push(d),
            }
        }
        let mut transitive_deps = Vec::with_capacity(w.transitive_deps.len());
        for dep in w.transitive_deps {
            let ident =
                RustIdent::parse(&dep.ident).map_err(|defect| Diagnostic::WireMalformed {
                    context: format!("transitive dep `{}`", dep.name),
                    defect,
                })?;
            transitive_deps.push(TransitiveDep {
                ident,
                name: dep.name,
                version: dep.version,
            });
        }
        Ok(Self {
            pkg_path: w.pkg,
            name,
            version: w.version,
            fns,
            modules: w.modules,
            errors: w.errors,
            notes: w.notes,
            transitive_deps,
            features: w.features,
            dropped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::CallDefect;
    use serde_json::json;

    fn decode(v: &serde_json::Value) -> Result<PkgInfo, Diagnostic> {
        PkgInfo::decode_json(&v.to_string())
    }

    fn fn_at(pkg: &PkgInfo, i: usize) -> &FnInfo {
        pkg.fns().get(i).expect("function present")
    }

    fn base_pkg(functions: &serde_json::Value) -> serde_json::Value {
        json!({
            "pkg": "semver",
            "name": "semver",
            "version": "1.0.26",
            "functions": functions,
            "errors": []
        })
    }

    #[test]
    fn decodes_a_minimal_plain_function() {
        let pkg = decode(&base_pkg(&json!([{
            "name": "parse",
            "params": [{"name": "text", "type": "&str", "ipeType": "string", "rustType": "&str"}],
            "results": [{"name": "", "type": "Result<Version, Error>"}],
            "variadic": false,
            "effect": "fallible",
            "exported": true
        }])))
        .expect("decodes");
        assert_eq!(pkg.name(), "semver");
        assert_eq!(pkg.version(), "1.0.26");
        let f = fn_at(&pkg, 0);
        assert_eq!(f.name(), "parse");
        assert_eq!(f.wrapper_ref_name(), "parse");
        assert_eq!(f.effect(), Effect::Fallible);
        assert_eq!(*f.shape(), FnShape::Plain);
        assert_eq!(f.fallibility(), Fallibility::TaskError);
        assert!(pkg.dropped().is_empty());
    }

    #[test]
    fn field_getter_shape_carries_the_infallible_bit() {
        let pkg = decode(&base_pkg(&json!([{
            "name": "major_field",
            "params": [],
            "results": [{"name": "", "type": "u64"}],
            "effect": "pure",
            "recvType": "Version",
            "isField": true
        }])))
        .expect("decodes");
        let f = fn_at(&pkg, 0);
        assert_eq!(*f.shape(), FnShape::FieldGet);
        assert_eq!(f.fallibility(), Fallibility::Infallible);
        assert_eq!(f.wrapper_ref_name(), "major_field_from_version");
    }

    #[test]
    fn enum_shapes_decode_their_payloads() {
        let pkg = decode(&base_pkg(&json!([
            {
                "name": "new_prerelease",
                "effect": "pure",
                "isEnumCtor": true,
                "enumVariant": "Prerelease",
                "enumKind": "tuple"
            },
            {
                "name": "tag_of_op",
                "effect": "pure",
                "isEnumTag": true,
                "enumArms": ["Exact\tExact", "Greater(..)\tGreater"],
                "enumWildcard": true
            },
            {
                "name": "value_as_greater",
                "effect": "pure",
                "isEnumExtract": true,
                "enumVariant": "Greater",
                "enumKind": "tuple",
                "enumStructFields": ["0"],
                "enumFieldCount": 2
            }
        ])))
        .expect("decodes");
        assert_eq!(pkg.fns().len(), 3);
        assert!(matches!(
            fn_at(&pkg, 0).shape(),
            FnShape::EnumCtor {
                kind: EnumVariantKind::Tuple,
                ..
            }
        ));
        let tag = match fn_at(&pkg, 1).shape() {
            FnShape::EnumTag { arms, wildcard } => Some((arms.clone(), *wildcard)),
            _ => None,
        };
        let (arms, wildcard) = tag.expect("decoded as EnumTag");
        assert!(wildcard);
        assert_eq!(
            arms.first().expect("arm present"),
            &EnumArm {
                pattern: "Exact".into(),
                tag: "Exact".into()
            }
        );
        let extract = match fn_at(&pkg, 2).shape() {
            FnShape::EnumExtract {
                selector,
                field_count,
                ..
            } => Some((selector.clone(), *field_count)),
            _ => None,
        };
        let (selector, field_count) = extract.expect("decoded as EnumExtract");
        assert_eq!(selector, "0");
        assert_eq!(field_count, 2);
        // Every accessor shape is infallible — the single stored bit.
        for f in pkg.fns() {
            assert_eq!(f.fallibility(), Fallibility::Infallible);
        }
    }

    #[test]
    fn contradictory_shape_flags_drop_the_one_binding_and_keep_the_package() {
        let pkg = decode(&base_pkg(&json!([
            {
                "name": "good",
                "effect": "pure"
            },
            {
                "name": "confused",
                "effect": "pure",
                "isField": true,
                "isEnumCtor": true,
                "enumVariant": "V",
                "enumKind": "unit"
            }
        ])))
        .expect("package survives");
        assert_eq!(pkg.fns().len(), 1);
        assert_eq!(fn_at(&pkg, 0).name(), "good");
        assert_eq!(pkg.dropped().len(), 1);
        assert!(matches!(
            pkg.dropped().first().expect("dropped diagnostic"),
            Diagnostic::ShapeContradiction { function, flags }
                if function == "confused" && flags == &vec!["isField", "isEnumCtor"]
        ));
    }

    #[test]
    fn unknown_effect_drops_the_binding() {
        let pkg = decode(&base_pkg(&json!([{
            "name": "weird",
            "effect": "spooky"
        }])))
        .expect("package survives");
        assert!(pkg.fns().is_empty());
        assert!(matches!(
            pkg.dropped().first().expect("dropped diagnostic"),
            Diagnostic::WireMalformed {
                defect: WireDefect::UnknownEffect { got },
                ..
            } if got == "spooky"
        ));
    }

    #[test]
    fn an_injection_shaped_function_name_is_refused() {
        let pkg = decode(&base_pkg(&json!([{
            "name": "evil; std::process::exit(1)",
            "effect": "pure"
        }])))
        .expect("package survives");
        assert!(pkg.fns().is_empty());
        assert!(matches!(
            pkg.dropped().first().expect("dropped diagnostic"),
            Diagnostic::WireMalformed {
                defect: WireDefect::InvalidIdent { .. },
                ..
            }
        ));
    }

    #[test]
    fn an_illegal_crate_name_fails_the_whole_package() {
        let v = json!({
            "pkg": "x",
            "name": "bad-crate!",
            "functions": [],
            "errors": []
        });
        assert!(matches!(
            decode(&v),
            Err(Diagnostic::WireMalformed {
                defect: WireDefect::InvalidIdent { .. },
                ..
            })
        ));
    }

    #[test]
    fn a_generic_block_routes_through_the_call_gate() {
        let good = decode(&base_pkg(&json!([{
            "name": "make",
            "effect": "pure",
            "generic": {
                "params": ["a"],
                "bounds": {"a": ["Clone"]},
                "call": {
                    "kind": "function",
                    "path": ["::box1", "Box1"],
                    "typeArgs": [{"param": 0}],
                    "method": "make",
                    "args": [0],
                    "argTypes": [{"param": 0}],
                    "ret": {"ctor": "::box1::Box1", "args": [{"param": 0}]}
                }
            }
        }])))
        .expect("decodes");
        let g = fn_at(&good, 0).generic().expect("generic present");
        assert_eq!(g.params, vec!["a".to_owned()]);
        assert_eq!(
            g.call.render_body(&g.params),
            "::box1::Box1::<A>::make(arg0)"
        );

        // An out-of-range param ref inside the call drops the binding with
        // the F4400 defect attached.
        let bad = decode(&base_pkg(&json!([{
            "name": "make",
            "effect": "pure",
            "generic": {
                "params": ["a"],
                "call": {
                    "kind": "function",
                    "path": ["::box1", "Box1"],
                    "method": "make",
                    "args": [0],
                    "argTypes": [{"param": 0}],
                    "ret": {"param": 7}
                }
            }
        }])))
        .expect("package survives");
        assert!(bad.fns().is_empty());
        assert!(matches!(
            bad.dropped().first().expect("dropped diagnostic"),
            Diagnostic::CallUnrenderable {
                defect: CallDefect::ParamRefOutOfRange {
                    index: 7,
                    n_params: 1
                },
                ..
            }
        ));
    }

    #[test]
    fn inspector_error_channel_and_metadata_survive_the_conversion() {
        let v = json!({
            "pkg": "semver",
            "name": "semver",
            "functions": [],
            "modules": ["semver"],
            "errors": ["rustdoc failed"],
            "notes": ["facade guidance"],
            "transitiveDeps": [
                {"ident": "serde_json", "name": "serde-json", "version": "1.0.145"}
            ],
            "features": ["std"]
        });
        let pkg = decode(&v).expect("decodes");
        assert_eq!(pkg.errors(), ["rustdoc failed".to_owned()]);
        assert_eq!(pkg.notes(), ["facade guidance".to_owned()]);
        assert_eq!(pkg.features(), ["std".to_owned()]);
        assert_eq!(
            pkg.transitive_deps().first().expect("dep").ident.as_str(),
            "serde_json"
        );
        assert_eq!(
            pkg.transitive_deps().first().expect("dep").name,
            "serde-json"
        );
        assert_eq!(pkg.modules(), ["semver".to_owned()]);
    }
}

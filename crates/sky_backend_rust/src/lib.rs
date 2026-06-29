#![forbid(unsafe_code)]
//! The Rust backend for the Sky compiler (Milestone 0).
//!
//! Consumes the backend-agnostic typed [`sky_ir::Program`] and emits a Rust
//! Cargo project. The crate is split into the fixed templates emitted for every
//! program ([`preamble`] / [`epilogue`] / the kernel-wrapper prelude in
//! [`project`]) and the genuinely type-directed emission of the user's types
//! ([`emit_types`]) and functions ([`emit_expr`]). [`naming`] holds the
//! Sky → Rust identifier rules.
//!
//! The single correctness gate is byte-equality against the golden M0 program
//! (`tests/golden/m0/main.rs`).
//!
//! The [`sky_ir`] boundary carries [`sky_intern::Symbol`]s, not strings, so the
//! backend resolves them through the [`sky_intern::Interner`] it is constructed
//! with. The [`sky_backend::Backend`] trait stays string-free.

mod emit_expr;
mod emit_types;
mod naming;
mod preamble;
mod project;

use std::collections::{BTreeMap, BTreeSet};

use sky_backend::{Backend, EmittedProject};
use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::{Interner, Symbol};
use sky_ir::{FuncId, IrType, Program, TypeDef};

pub use preamble::{epilogue, preamble};

/// The Rust code-generation backend.
///
/// Holds a reference to the [`Interner`] used to build the program, so it can
/// resolve the [`Symbol`]s carried by the IR without widening the
/// [`Backend::emit`] signature.
pub struct RustBackend<'a> {
    interner: &'a Interner,
}

impl<'a> RustBackend<'a> {
    /// Construct a backend that resolves IR symbols through `interner`.
    #[must_use]
    pub const fn new(interner: &'a Interner) -> Self {
        Self { interner }
    }
}

impl Backend for RustBackend<'_> {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn emit(&self, program: &Program) -> DResult<EmittedProject> {
        let ctx = EmitCtx::build(self.interner, program)?;
        project::emit_program(&ctx, program)
    }
}

/// A canonical record field list: `(Sky field name, field type)` pairs sorted by
/// field name. The order is the struct's declaration / `SkyStringify` read order.
type RecordFields = Vec<(String, IrType)>;

/// Every DISTINCT field-type shape observed for one field-name set, in
/// first-occurrence order — a generic template and/or its concrete
/// instantiations, reconciled by [`canonicalise_shape`].
type ShapeOccurrences = Vec<RecordFields>;

/// A synthesised struct's reconciled form: its canonical field template and its
/// generic parameter symbols (empty for a monomorphic record).
type CanonicalShape = (RecordFields, Vec<Symbol>);

/// A synthesised Rust struct for one distinct CLOSED record shape.
///
/// `fields` is the field set in canonical (field-name ascending) order — the
/// order the struct is declared in and the order its `SkyStringify` body reads.
pub(crate) struct RecordStruct {
    /// The deduplicated, collision-free Rust struct name (e.g. `RecXY`).
    pub name: String,
    /// The fields as `(Sky field name, field type)`, sorted by field name. The
    /// Rust field identifier is the keyword-mangled field name.
    ///
    /// For a GENERIC record shape (M2c), a field's type may be an
    /// [`IrType::Generic`]; the carried [`Symbol`] is the canonical template's
    /// source type-variable, resolved to its Rust generic name (`T1`, `T2`, …)
    /// through a [`crate::emit_types::GenericScope`] over [`Self::type_params`].
    pub fields: Vec<(String, IrType)>,
    /// The struct's generic type parameters (M2c): the distinct
    /// [`IrType::Generic`] symbols appearing in [`Self::fields`], in
    /// first-occurrence field order. Empty for a monomorphic record — that path
    /// stays byte-identical to b3.
    ///
    /// The order is load-bearing: a parameter's Rust name (`T1`, `T2`, …) is its
    /// *position* here, exactly as for [`sky_ir::Func::type_params`], so struct
    /// declaration, field types, and every use-site instantiation agree.
    pub type_params: Vec<Symbol>,
}

/// Shared emission context: the interner plus the precomputed Sky → Rust name
/// maps so each emit site is a `O(log n)` lookup rather than recomputing the
/// naming rules. Built once per [`RustBackend::emit`].
pub(crate) struct EmitCtx<'a> {
    interner: &'a Interner,
    /// Enum type symbol → Rust type name (e.g. `Msg` → `MainMsg`).
    enum_names: BTreeMap<Symbol, String>,
    /// `(enum type symbol, variant symbol)` → that variant's declared payload
    /// field types, in source (positional) order. Empty vector for a nullary
    /// variant. Used at construction / pattern sites to box (and un-box) a
    /// recursive field so the emitted Rust enum stays finite-sized.
    variant_fields: BTreeMap<(Symbol, Symbol), Vec<IrType>>,
    /// Enum type symbol → the field-type lists of all its variants (one inner
    /// vector per variant, in declaration order). The whole-enum view that
    /// [`Self::is_cyclic_self_field`] walks to decide whether a payload field
    /// sits on a type-size cycle back to its own enum — direct (`Node Tree …`)
    /// or indirect (mutual recursion, or a self-edge routed through a tuple /
    /// record / another generic's type argument).
    enum_variants: BTreeMap<Symbol, Vec<Vec<IrType>>>,
    /// Function id → Rust function name (e.g. `update` → `main_update`).
    func_names: BTreeMap<FuncId, String>,
    /// Every distinct record shape synthesised for the program, in emission
    /// order (sorted by field-name set).
    record_structs: Vec<RecordStruct>,
    /// Sorted field-name set → index into [`Self::record_structs`]. The field
    /// set is the canonical key: every `IrType::Record` and every record
    /// literal resolves to its struct through it.
    record_by_fieldset: BTreeMap<Vec<String>, usize>,
}

impl<'a> EmitCtx<'a> {
    fn build(interner: &'a Interner, program: &Program) -> DResult<Self> {
        let mut enum_names = BTreeMap::new();
        let mut variant_fields: BTreeMap<(Symbol, Symbol), Vec<IrType>> = BTreeMap::new();
        let mut enum_variants: BTreeMap<Symbol, Vec<Vec<IrType>>> = BTreeMap::new();
        let mut func_names = BTreeMap::new();
        for module in &program.modules {
            let segs = module
                .name
                .0
                .iter()
                .map(|s| resolve_sym(interner, *s))
                .collect::<DResult<Vec<&str>>>()?;
            for ty in &module.types {
                let TypeDef::Enum(def) = ty;
                let rust_name = naming::enum_name(&segs, resolve_sym(interner, def.name)?);
                // The IR keys a type by its bare name `Symbol`. Two modules
                // declaring same-named types intern to the *same* `Symbol`, so a
                // plain insert would silently overwrite the first mapping —
                // making both use sites resolve to one module's Rust type. The
                // user-facing duplicate is caught upstream (SKY-N0012); reaching
                // here means the lowerer admitted a cross-module collision the
                // backend cannot disambiguate from a bare `Symbol`, so fail fast
                // (SKY-I0202) rather than emit code referencing the wrong type.
                if let Some(prev) = enum_names.insert(def.name, rust_name.clone()) {
                    return Err(Diagnostic::CompilerBug {
                        where_: "backend.type_name_collision",
                        detail: format!(
                            "type symbol {} maps to two Rust names ({prev} and {rust_name}); \
                             cross-module same-named types are indistinguishable by bare symbol",
                            def.name.as_raw()
                        ),
                    });
                }
                let mut all_fields = Vec::with_capacity(def.variants.len());
                for variant in &def.variants {
                    variant_fields.insert((def.name, variant.name), variant.fields.clone());
                    all_fields.push(variant.fields.clone());
                }
                enum_variants.insert(def.name, all_fields);
            }
            for func in &module.funcs {
                func_names.insert(
                    func.id,
                    naming::module_value(&segs, resolve_sym(interner, func.name)?),
                );
            }
        }

        // Prepass: collect every distinct CLOSED record shape the program uses
        // (recursing into nested records / tuples), so each gets one synthesised
        // struct, declared before any use. Two sources feed it: function
        // signatures (params / return), and the lowerer-surfaced `module.records`
        // — the shapes of record literals that live inside function bodies, where
        // the type appears in no signature. A record literal resolves to its
        // struct through this table by its field-name set; a literal whose set is
        // absent is an internal invariant violation (surfaced as a `CompilerBug`,
        // never a silent mis-emit).
        //
        // Each field-name set maps to the LIST of distinct field-type shapes seen
        // for it. A set may carry both a generic template (`{ value : a }`, from a
        // parametric signature) and concrete instantiations (`{ value : Int }`):
        // [`canonicalise_shape`] reconciles them into a single struct (M2c).
        let mut shapes: BTreeMap<Vec<String>, ShapeOccurrences> = BTreeMap::new();
        for module in &program.modules {
            for func in &module.funcs {
                for (_, ty) in &func.params {
                    collect_record_shapes(interner, ty, &mut shapes)?;
                }
                collect_record_shapes(interner, &func.ret, &mut shapes)?;
            }
            for ty in &module.records {
                collect_record_shapes(interner, ty, &mut shapes)?;
            }
            // An enum variant's payload field type may itself be (or carry) a
            // record shape (`type Boxed a = Box { value : a }`). The variant
            // field types are not in any signature, so collect them here too —
            // otherwise emitting the enum would resolve the record type to a
            // struct that was never synthesised (a `CompilerBug` miss).
            for ty in &module.types {
                let TypeDef::Enum(def) = ty;
                for variant in &def.variants {
                    for field_ty in &variant.fields {
                        collect_record_shapes(interner, field_ty, &mut shapes)?;
                    }
                }
            }
        }
        let mut record_structs = Vec::with_capacity(shapes.len());
        let mut record_by_fieldset = BTreeMap::new();
        let mut used_names: BTreeSet<String> = BTreeSet::new();
        for (key, occurrences) in shapes {
            let (fields, type_params) = canonicalise_shape(&key, &occurrences)?;
            let name = unique_struct_name(naming::record_struct_name(&key), &mut used_names);
            record_by_fieldset.insert(key, record_structs.len());
            record_structs.push(RecordStruct {
                name,
                fields,
                type_params,
            });
        }

        Ok(Self {
            interner,
            enum_names,
            variant_fields,
            enum_variants,
            func_names,
            record_structs,
            record_by_fieldset,
        })
    }

    /// The declared payload field types of constructor `variant` of enum `ty`,
    /// in positional order.
    ///
    /// A miss means a constructor expression / pattern names a variant the
    /// program never declared — an upstream-contract violation (the type checker
    /// pins every constructor to its union), surfaced as a [`Diagnostic::CompilerBug`]
    /// rather than a silent mis-emit.
    fn variant_fields(&self, ty: Symbol, variant: Symbol) -> DResult<&[IrType]> {
        self.variant_fields
            .get(&(ty, variant))
            .map(Vec::as_slice)
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::EmitCtx::variant_fields",
                detail: format!(
                    "no declared field types for variant {} of enum {}",
                    variant.as_raw(),
                    ty.as_raw()
                ),
            })
    }

    /// Is `field` a payload field of enum `enum_sym` that sits on a type-size
    /// cycle back to that enum — so the Rust enum is infinite-sized (E0072)
    /// unless the field is boxed?
    ///
    /// This generalises the old direct-self-edge test (`field` *is* the enum's
    /// own type, `type Tree = … | Node Tree …`) to every cycle the field can
    /// close: mutual recursion between two enums, and a self-edge routed through
    /// a tuple (`Node (Tree, Int)`), a record (`Node { left : Tree }`), or
    /// another generic's type argument (`Node (Maybe Tree)`). The backend wraps
    /// such a field in `Box<…>` at the declaration and balances that with
    /// `Box::new` at construction and a deref at pattern binding — boxing at
    /// least one edge of every cycle, which is what keeps the emitted crate
    /// finite-sized and matches the Go reference's recursive-payload boxing.
    ///
    /// Every *constructible* recursive Sky type routes through an enum (the enum
    /// supplies the nullary base case), so boxing the cyclic enum-payload edge
    /// breaks every reachable cycle; a hypothetical pure record/tuple alias
    /// cycle (no enum on it) is rejected upstream before it can reach the
    /// backend.
    pub(crate) fn is_cyclic_self_field(&self, field: &IrType, enum_sym: Symbol) -> bool {
        let mut visited = BTreeSet::new();
        type_reaches_enum(field, enum_sym, &self.enum_variants, &mut visited)
    }

    /// Every synthesised record struct, in emission order.
    pub(crate) fn record_structs(&self) -> &[RecordStruct] {
        &self.record_structs
    }

    /// Render a record TYPE at a USE SITE to its Rust spelling, keyed by its
    /// field-name set: the bare struct name for a monomorphic shape (`RecXY`,
    /// byte-identical to b3), or the struct instantiated at concrete type
    /// arguments for a generic shape (`RecValue<i64>`, M2c).
    ///
    /// `generics` is the enclosing function's generic scope: a use-site field
    /// type may itself be an [`IrType::Generic`] (a parametric signature passing
    /// the record through, `wrap : a -> { value : a }`), in which case the
    /// argument renders as that function's Rust generic (`RecValue<T1>`).
    ///
    /// The prepass collected every `IrType::Record` reachable from a signature,
    /// so a miss here is an internal invariant violation (SKY-I0204).
    fn render_record_use(
        &self,
        fields: &BTreeMap<Symbol, IrType>,
        generics: emit_types::GenericScope,
    ) -> DResult<String> {
        let mut key = Vec::with_capacity(fields.len());
        for sym in fields.keys() {
            key.push(self.resolve_ident(*sym)?.to_owned());
        }
        key.sort();
        let rec = self.record_struct_by_key(&key)?;
        if rec.type_params.is_empty() {
            // Monomorphic shape: the bare struct name, byte-identical to b3.
            return Ok(rec.name.clone());
        }
        // Generic shape: match the use-site field types against the struct's
        // template to recover one concrete type per generic parameter, then
        // render each (through the ambient scope) as a turbofish-free arg list.
        let mut by_name: BTreeMap<&str, &IrType> = BTreeMap::new();
        for (sym, ty) in fields {
            by_name.insert(self.resolve_ident(*sym)?, ty);
        }
        let mut subst: BTreeMap<Symbol, IrType> = BTreeMap::new();
        for (field_name, template_ty) in &rec.fields {
            let use_ty =
                by_name
                    .get(field_name.as_str())
                    .ok_or_else(|| Diagnostic::CompilerBug {
                        where_: "sky_backend_rust::EmitCtx::render_record_use",
                        detail: format!(
                            "use-site record is missing field `{field_name}` present in the \
                         synthesised struct template"
                        ),
                    })?;
            match_template(template_ty, use_ty, &mut subst)?;
        }
        let mut args = Vec::with_capacity(rec.type_params.len());
        for param in &rec.type_params {
            let arg_ty = subst.get(param).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::EmitCtx::render_record_use",
                detail: format!(
                    "generic record parameter symbol {} was not pinned by any use-site \
                     field; the use site does not instantiate the struct template",
                    param.as_raw()
                ),
            })?;
            args.push(emit_types::render_type(self, arg_ty, generics)?);
        }
        Ok(format!("{}<{}>", rec.name, args.join(", ")))
    }

    /// The Rust struct name for a record LITERAL, keyed by its field names.
    ///
    /// A miss means the literal's shape never appeared in a signature — a
    /// lowerer-contract violation (SKY-I0204), surfaced rather than mis-emitted.
    fn record_name_for_literal(&self, field_names: &[String]) -> DResult<&str> {
        let mut key = field_names.to_vec();
        key.sort();
        self.record_name_by_key(&key)
    }

    /// Resolve a (sorted) field-name set to its synthesised struct name.
    fn record_name_by_key(&self, key: &[String]) -> DResult<&str> {
        Ok(self.record_struct_by_key(key)?.name.as_str())
    }

    /// Resolve a (sorted) field-name set to its synthesised [`RecordStruct`].
    fn record_struct_by_key(&self, key: &[String]) -> DResult<&RecordStruct> {
        self.record_by_fieldset
            .get(key)
            .and_then(|i| self.record_structs.get(*i))
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::EmitCtx::record_name",
                detail: format!(
                    "no synthesised struct for record shape {{{}}}; the lowerer must \
                     surface every record type it constructs in a signature",
                    key.join(", ")
                ),
            })
    }

    /// Resolve a symbol that will be emitted as a Rust identifier, rejecting an
    /// absent *or* empty resolution. The lowerer is contracted never to hand the
    /// backend a dangling or empty-intended value/variant/param symbol, so a
    /// failure here is an internal invariant violation (SKY-I0201) — surfaced as
    /// a [`Diagnostic::CompilerBug`] rather than silently emitting an empty (and
    /// uncompilable) Rust identifier.
    fn resolve_ident(&self, sym: Symbol) -> DResult<&str> {
        match self.interner.resolve(sym) {
            Some(s) if !s.is_empty() => Ok(s),
            _ => Err(Diagnostic::CompilerBug {
                where_: "backend.dangling_symbol",
                detail: format!(
                    "value/variant symbol {} resolved to an empty or absent identifier",
                    sym.as_raw()
                ),
            }),
        }
    }

    /// Resolve a symbol to the Rust identifier to emit for it: checked for
    /// emptiness ([`Self::resolve_ident`]) and then mangled if it collides with
    /// a Rust keyword ([`naming::mangle_reserved`]). Used for every emitted
    /// value/variant/param name.
    fn emit_ident(&self, sym: Symbol) -> DResult<String> {
        Ok(naming::mangle_reserved(self.resolve_ident(sym)?.to_owned()))
    }

    fn enum_name(&self, ty: Symbol) -> DResult<&str> {
        self.enum_names
            .get(&ty)
            .map(String::as_str)
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::EmitCtx::enum_name",
                detail: format!("no Rust name for enum type symbol {}", ty.as_raw()),
            })
    }

    /// The runtime enum path for a built-in constructor's type, or `None` for a
    /// user-declared enum. `Maybe` / `Result` are not user `type` declarations —
    /// their constructors (`Just` / `Nothing` / `Ok` / `Err`) are Prelude
    /// built-ins backed by the runtime's `SkyMaybe` / `SkyResult`, whose variant
    /// names match Sky's verbatim. A `Some` result steers constructor and pattern
    /// emission to the runtime type (no user-enum field-boxing lookup applies, as
    /// neither is self-recursive).
    fn builtin_runtime_enum(&self, ty: Symbol) -> Option<&'static str> {
        // A declared user enum always wins: real Sky cannot name a `type` `Maybe`
        // or `Result` (canonicalisation rejects shadowing a built-in), so a
        // program-level enum carrying that symbol is a distinct, user-owned type
        // and must route to its own emitted enum, not the runtime shortcut.
        if self.enum_names.contains_key(&ty) {
            return None;
        }
        match self.interner.resolve(ty) {
            Some("Maybe") => Some("SkyMaybe"),
            Some("Result") => Some("SkyResult"),
            _ => None,
        }
    }

    fn func_name(&self, id: FuncId) -> DResult<&str> {
        self.func_names
            .get(&id)
            .map(String::as_str)
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::EmitCtx::func_name",
                detail: format!("no Rust name for function id {}", id.as_raw()),
            })
    }
}

/// Walk a type, recording every distinct CLOSED record shape it contains
/// (recursing through tuples and nested records). A shape is keyed by its sorted
/// field-name set; the value accumulates each DISTINCT `(field name, type)` list
/// observed for that set, in first-occurrence order.
///
/// One set can legitimately carry several entries — a generic template
/// (`{ value : a }`) plus concrete instantiations (`{ value : Int }`). The later
/// [`canonicalise_shape`] pass reconciles them into one struct. Storing every
/// distinct occurrence (rather than rejecting the second) is what makes M2c's
/// generic-plus-concrete merge representable.
fn collect_record_shapes(
    interner: &Interner,
    ty: &IrType,
    shapes: &mut BTreeMap<Vec<String>, ShapeOccurrences>,
) -> DResult<()> {
    match ty {
        IrType::Tuple(elems) => {
            for elem in elems {
                collect_record_shapes(interner, elem, shapes)?;
            }
        }
        IrType::Record(map) => {
            for field_ty in map.values() {
                collect_record_shapes(interner, field_ty, shapes)?;
            }
            let mut fields: Vec<(String, IrType)> = Vec::with_capacity(map.len());
            for (sym, field_ty) in map {
                fields.push((resolve_sym(interner, *sym)?.to_owned(), field_ty.clone()));
            }
            fields.sort_by(|a, b| a.0.cmp(&b.0));
            let key: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
            let entry = shapes.entry(key).or_default();
            if !entry.contains(&fields) {
                entry.push(fields);
            }
        }
        IrType::Fun(params, ret) => {
            // A function type contributes no struct of its own, but its
            // parameter and return types may carry record shapes (e.g. a
            // callback over a record).
            for param in params {
                collect_record_shapes(interner, param, shapes)?;
            }
            collect_record_shapes(interner, ret, shapes)?;
        }
        IrType::Enum { args, .. } => {
            // An enum carries no struct of its own, but its type arguments may
            // (e.g. `Maybe { x : Int }`).
            for arg in args {
                collect_record_shapes(interner, arg, shapes)?;
            }
        }
        // `Maybe a` / `Result e a` carry no struct of their own, but their
        // element types may (`Maybe { x : Int }`).
        IrType::Maybe(elem) | IrType::List(elem) => {
            collect_record_shapes(interner, elem, shapes)?;
        }
        IrType::Result(err, ok) => {
            collect_record_shapes(interner, err, shapes)?;
            collect_record_shapes(interner, ok, shapes)?;
        }
        // `Dict k v` / `Set a` carry no struct of their own, but their element
        // types may (`Dict String { x : Int }`).
        IrType::Dict(k, v) => {
            collect_record_shapes(interner, k, shapes)?;
            collect_record_shapes(interner, v, shapes)?;
        }
        IrType::Set(a) => {
            collect_record_shapes(interner, a, shapes)?;
        }
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::TaskUnit
        | IrType::Bytes
        | IrType::Json
        // A generic type variable carries no concrete record shape of its own.
        | IrType::Generic(_) => {}
    }
    Ok(())
}

/// Does `ty` reach the enum type `target` by following type-size edges —
/// tuple elements, record fields, an enum's type arguments, and (memoised by
/// enum name) an enum's own variant payload fields?
///
/// A `Box<…>` and a first-class function value (`Box<dyn Fn …>`) are already a
/// pointer-sized indirection, so traversal does NOT descend through
/// [`IrType::Fun`]; those edges can never make a type infinite-sized.
///
/// `visited` memoises the per-enum *definition* walk (a name-keyed, type-arg-
/// independent set of fields) so a recursive enum is explored once. The
/// use-site type arguments are checked on every visit (NOT memoised), because
/// `Maybe Int` and `Maybe Tree` share the enum name `Maybe` but carry different
/// arguments — memoising under the name would drop the `Tree` argument on the
/// second visit.
fn type_reaches_enum(
    ty: &IrType,
    target: Symbol,
    enums: &BTreeMap<Symbol, Vec<Vec<IrType>>>,
    visited: &mut BTreeSet<Symbol>,
) -> bool {
    match ty {
        IrType::Enum { name, args } => {
            if *name == target {
                return true;
            }
            if args
                .iter()
                .any(|a| type_reaches_enum(a, target, enums, visited))
            {
                return true;
            }
            // Descend into this enum's own variant payload fields once.
            if visited.insert(*name)
                && let Some(variants) = enums.get(name)
            {
                return variants
                    .iter()
                    .flatten()
                    .any(|f| type_reaches_enum(f, target, enums, visited));
            }
            false
        }
        IrType::Tuple(elems) => elems
            .iter()
            .any(|e| type_reaches_enum(e, target, enums, visited)),
        IrType::Record(map) => map
            .values()
            .any(|v| type_reaches_enum(v, target, enums, visited)),
        // `Maybe a` / `Result e a` are the runtime's own (already finite) types;
        // a size cycle can still pass THROUGH their element types, so descend.
        IrType::Maybe(elem) | IrType::List(elem) => type_reaches_enum(elem, target, enums, visited),
        IrType::Result(err, ok) => {
            type_reaches_enum(err, target, enums, visited)
                || type_reaches_enum(ok, target, enums, visited)
        }
        // `Dict k v` / `Set a` are heap-allocated (pointer-sized); they cannot
        // participate in an infinite-size cycle. Recurse into element types for
        // completeness (a Dict/Set whose element type reaches the target enum is
        // still finite because the backing HashMap/BTreeSet is a heap pointer).
        IrType::Dict(k, v) => {
            type_reaches_enum(k, target, enums, visited)
                || type_reaches_enum(v, target, enums, visited)
        }
        IrType::Set(a) => type_reaches_enum(a, target, enums, visited),
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::TaskUnit
        | IrType::Bytes
        | IrType::Json
        | IrType::Fun(_, _)
        | IrType::Generic(_) => false,
    }
}

/// Does this type contain an [`IrType::Generic`] anywhere (a field that is — or
/// structurally carries — a type variable)?
fn contains_generic(ty: &IrType) -> bool {
    match ty {
        IrType::Generic(_) => true,
        IrType::Tuple(elems) => elems.iter().any(contains_generic),
        IrType::Record(map) => map.values().any(contains_generic),
        IrType::Fun(params, ret) => params.iter().any(contains_generic) || contains_generic(ret),
        IrType::Enum { args, .. } => args.iter().any(contains_generic),
        IrType::Maybe(elem) | IrType::List(elem) => contains_generic(elem),
        IrType::Result(err, ok) => contains_generic(err) || contains_generic(ok),
        IrType::Dict(k, v) => contains_generic(k) || contains_generic(v),
        IrType::Set(a) => contains_generic(a),
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::TaskUnit
        | IrType::Bytes
        | IrType::Json => false,
    }
}

/// Collect the distinct [`IrType::Generic`] symbols in `ty`, appending each (in
/// first-occurrence order) to `out` if not already present.
fn collect_generics(ty: &IrType, out: &mut Vec<Symbol>) {
    match ty {
        IrType::Generic(s) => {
            if !out.contains(s) {
                out.push(*s);
            }
        }
        IrType::Tuple(elems) => {
            for e in elems {
                collect_generics(e, out);
            }
        }
        IrType::Record(map) => {
            for v in map.values() {
                collect_generics(v, out);
            }
        }
        IrType::Fun(params, ret) => {
            for p in params {
                collect_generics(p, out);
            }
            collect_generics(ret, out);
        }
        IrType::Enum { args, .. } => {
            for a in args {
                collect_generics(a, out);
            }
        }
        IrType::Maybe(elem) | IrType::List(elem) => collect_generics(elem, out),
        IrType::Result(err, ok) => {
            collect_generics(err, out);
            collect_generics(ok, out);
        }
        IrType::Dict(k, v) => {
            collect_generics(k, out);
            collect_generics(v, out);
        }
        IrType::Set(a) => collect_generics(a, out),
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::TaskUnit
        | IrType::Bytes
        | IrType::Json => {}
    }
}

/// A position-canonical rendering of a field-shape: every [`IrType::Generic`]
/// symbol is replaced by its first-occurrence index, so two alpha-equivalent
/// templates (`{ value : a }` and `{ value : b }`) render the same string and a
/// non-equivalent one (`{ x : a, y : a }` vs `{ x : a, y : b }`) does not. Used
/// only for equality, never emitted.
fn skeleton_key(fields: &[(String, IrType)]) -> String {
    let mut idx: BTreeMap<Symbol, usize> = BTreeMap::new();
    let mut out = String::new();
    for (name, ty) in fields {
        out.push_str(name);
        out.push(':');
        skeleton_ty(ty, &mut idx, &mut out);
        out.push(';');
    }
    out
}

fn skeleton_ty(ty: &IrType, idx: &mut BTreeMap<Symbol, usize>, out: &mut String) {
    match ty {
        IrType::Generic(s) => {
            let next = idx.len();
            let n = *idx.entry(*s).or_insert(next);
            out.push('G');
            out.push_str(&n.to_string());
        }
        IrType::Tuple(elems) => {
            out.push('(');
            for e in elems {
                skeleton_ty(e, idx, out);
                out.push(',');
            }
            out.push(')');
        }
        IrType::Record(map) => {
            out.push('{');
            for (k, v) in map {
                out.push_str(&k.as_raw().to_string());
                out.push(':');
                skeleton_ty(v, idx, out);
                out.push(',');
            }
            out.push('}');
        }
        IrType::Fun(params, ret) => {
            out.push_str("fn(");
            for p in params {
                skeleton_ty(p, idx, out);
                out.push(',');
            }
            out.push_str(")->");
            skeleton_ty(ret, idx, out);
        }
        IrType::Enum { name, args } => {
            // Key by the enum's symbol plus its (possibly generic) type args, so
            // `Maybe a` and `Maybe Int` skeletonise distinctly while `Maybe a`
            // and `Maybe b` (alpha-equivalent) coincide.
            out.push('E');
            out.push_str(&name.as_raw().to_string());
            out.push('<');
            for a in args {
                skeleton_ty(a, idx, out);
                out.push(',');
            }
            out.push('>');
        }
        IrType::Dict(k, v) => {
            out.push_str("Dict<");
            skeleton_ty(k, idx, out);
            out.push(',');
            skeleton_ty(v, idx, out);
            out.push('>');
        }
        IrType::Set(a) => {
            out.push_str("Set<");
            skeleton_ty(a, idx, out);
            out.push('>');
        }
        // Scalar / leaf types (Int / Bool / …): their `Debug` form is a stable,
        // generic-free discriminator — exactly what a skeleton needs.
        other => {
            use core::fmt::Write as _;
            // Writing to a `String` is infallible; the `Result` is discarded.
            let _ = write!(out, "{other:?}");
        }
    }
}

/// Match a struct-template type against a USE-SITE type, recording in `subst`
/// the concrete (or generic-in-the-enclosing-function) type each template
/// [`IrType::Generic`] binds to. A template `Generic` binds any use-site type
/// (consistently — a symbol seen twice must bind the same type); every other
/// node must structurally agree.
///
/// A mismatch means a use site that does not instantiate the struct template —
/// an upstream-contract violation surfaced as a [`Diagnostic::CompilerBug`]
/// (SKY-I0205), never a silent mis-emit.
#[allow(clippy::too_many_lines)]
fn match_template(
    template: &IrType,
    concrete: &IrType,
    subst: &mut BTreeMap<Symbol, IrType>,
) -> DResult<()> {
    let mismatch = || Diagnostic::CompilerBug {
        where_: "sky_backend_rust::match_template",
        detail: format!(
            "use-site record type does not instantiate the synthesised struct template \
             (template {template:?} vs use site {concrete:?})"
        ),
    };
    match template {
        IrType::Generic(s) => match subst.get(s) {
            Some(prev) if prev != concrete => Err(Diagnostic::CompilerBug {
                where_: "sky_backend_rust::match_template",
                detail: format!(
                    "generic parameter symbol {} is bound to two distinct types at one use \
                     site ({prev:?} and {concrete:?})",
                    s.as_raw()
                ),
            }),
            Some(_) => Ok(()),
            None => {
                subst.insert(*s, concrete.clone());
                Ok(())
            }
        },
        IrType::Tuple(ts) => match concrete {
            IrType::Tuple(cs) if cs.len() == ts.len() => {
                for (t, c) in ts.iter().zip(cs.iter()) {
                    match_template(t, c, subst)?;
                }
                Ok(())
            }
            _ => Err(mismatch()),
        },
        IrType::Record(tm) => match concrete {
            IrType::Record(cm) if tm.len() == cm.len() => {
                for ((tk, tv), (ck, cv)) in tm.iter().zip(cm.iter()) {
                    if tk != ck {
                        return Err(mismatch());
                    }
                    match_template(tv, cv, subst)?;
                }
                Ok(())
            }
            _ => Err(mismatch()),
        },
        IrType::Fun(tp, tr) => match concrete {
            IrType::Fun(cp, cr) if tp.len() == cp.len() => {
                for (t, c) in tp.iter().zip(cp.iter()) {
                    match_template(t, c, subst)?;
                }
                match_template(tr, cr, subst)
            }
            _ => Err(mismatch()),
        },
        IrType::Enum { name: tn, args: ta } => match concrete {
            IrType::Enum { name: cn, args: ca } if tn == cn && ta.len() == ca.len() => {
                for (t, c) in ta.iter().zip(ca.iter()) {
                    match_template(t, c, subst)?;
                }
                Ok(())
            }
            _ => Err(mismatch()),
        },
        IrType::Maybe(te) => match concrete {
            IrType::Maybe(ce) => match_template(te, ce, subst),
            _ => Err(mismatch()),
        },
        IrType::List(te) => match concrete {
            IrType::List(ce) => match_template(te, ce, subst),
            _ => Err(mismatch()),
        },
        IrType::Result(terr, tok) => match concrete {
            IrType::Result(cerr, cok) => {
                match_template(terr, cerr, subst)?;
                match_template(tok, cok, subst)
            }
            _ => Err(mismatch()),
        },
        IrType::Dict(tk, tv) => match concrete {
            IrType::Dict(ck, cv) => {
                match_template(tk, ck, subst)?;
                match_template(tv, cv, subst)
            }
            _ => Err(mismatch()),
        },
        IrType::Set(te) => match concrete {
            IrType::Set(ce) => match_template(te, ce, subst),
            _ => Err(mismatch()),
        },
        // A concrete leaf must equal the use-site leaf exactly.
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::TaskUnit
        | IrType::Bytes
        | IrType::Json => {
            if template == concrete {
                Ok(())
            } else {
                Err(mismatch())
            }
        }
    }
}

/// Reconcile every distinct field-type shape observed for one field-name set
/// into a single synthesised struct: its canonical `(field name, type)` template
/// and its generic parameter list (M2c).
///
/// * No occurrence carries a type variable → a MONOMORPHIC struct (empty
///   parameter list). All occurrences must be identical, exactly as b3 required;
///   a second, differing concrete shape is the same "two types for one field
///   set" upstream-contract violation b3 rejected (SKY-I0204).
/// * At least one occurrence is generic → a GENERIC struct. Every generic
///   occurrence must be alpha-equivalent (same [`skeleton_key`]); the first is
///   the canonical template, whose generic symbols name the parameters in
///   first-occurrence field order. Every concrete occurrence must be a valid
///   instantiation of that template (checked via [`match_template`]).
fn canonicalise_shape(key: &[String], occurrences: &[RecordFields]) -> DResult<CanonicalShape> {
    let first = occurrences.first().ok_or_else(|| Diagnostic::CompilerBug {
        where_: "sky_backend_rust::canonicalise_shape",
        detail: format!(
            "record field set {{{}}} has no collected shape",
            key.join(", ")
        ),
    })?;

    let is_generic = |fields: &[(String, IrType)]| fields.iter().any(|(_, t)| contains_generic(t));

    // Pick the canonical generic template (the first generic occurrence), if any.
    let template = occurrences.iter().find(|f| is_generic(f));

    let Some(template) = template else {
        // All-concrete: b3 contract — exactly one shape per field set.
        for other in occurrences.iter().skip(1) {
            if other != first {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::canonicalise_shape",
                    detail: format!(
                        "record field set {{{}}} maps to two distinct field-type shapes; \
                         closed records assume one type per field set",
                        key.join(", ")
                    ),
                });
            }
        }
        return Ok((first.clone(), Vec::new()));
    };

    let template_skeleton = skeleton_key(template);
    let mut type_params: Vec<Symbol> = Vec::new();
    for (_, ty) in template {
        collect_generics(ty, &mut type_params);
    }

    for occ in occurrences {
        if is_generic(occ) {
            // Every generic occurrence must be alpha-equivalent to the template.
            if skeleton_key(occ) != template_skeleton {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::canonicalise_shape",
                    detail: format!(
                        "record field set {{{}}} maps to two non-alpha-equivalent generic \
                         shapes",
                        key.join(", ")
                    ),
                });
            }
        } else {
            // Every concrete occurrence must instantiate the template.
            let mut subst: BTreeMap<Symbol, IrType> = BTreeMap::new();
            for ((_, tv), (_, cv)) in template.iter().zip(occ.iter()) {
                match_template(tv, cv, &mut subst)?;
            }
        }
    }

    Ok((template.clone(), type_params))
}

/// Return `base` if unused, else the first `base_<n>` (n ≥ 2) that is free,
/// recording the chosen name in `used`. Deterministic given a deterministic call
/// order; guarantees a collision-free struct name even when two distinct field
/// sets camel-case to the same base.
fn unique_struct_name(base: String, used: &mut BTreeSet<String>) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    let mut n: u32 = 2;
    loop {
        let candidate = format!("{base}_{n}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n = n.saturating_add(1);
    }
}

/// Resolve a symbol that the IR guarantees came from `interner`. A `None` here
/// means the IR carried a symbol from a different interner — an internal
/// invariant violation, surfaced as a [`Diagnostic::CompilerBug`] rather than a
/// silent empty name.
fn resolve_sym(interner: &Interner, sym: Symbol) -> DResult<&str> {
    interner
        .resolve(sym)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::resolve_sym",
            detail: format!("symbol {} not present in interner", sym.as_raw()),
        })
}

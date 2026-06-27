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

/// A synthesised Rust struct for one distinct CLOSED record shape.
///
/// `fields` is the field set in canonical (field-name ascending) order — the
/// order the struct is declared in and the order its `SkyStringify` body reads.
pub(crate) struct RecordStruct {
    /// The deduplicated, collision-free Rust struct name (e.g. `RecXY`).
    pub name: String,
    /// The fields as `(Sky field name, field type)`, sorted by field name. The
    /// Rust field identifier is the keyword-mangled field name.
    pub fields: Vec<(String, IrType)>,
}

/// Shared emission context: the interner plus the precomputed Sky → Rust name
/// maps so each emit site is a `O(log n)` lookup rather than recomputing the
/// naming rules. Built once per [`RustBackend::emit`].
pub(crate) struct EmitCtx<'a> {
    interner: &'a Interner,
    /// Enum type symbol → Rust type name (e.g. `Msg` → `MainMsg`).
    enum_names: BTreeMap<Symbol, String>,
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
        let mut shapes: BTreeMap<Vec<String>, Vec<(String, IrType)>> = BTreeMap::new();
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
        }
        let mut record_structs = Vec::with_capacity(shapes.len());
        let mut record_by_fieldset = BTreeMap::new();
        let mut used_names: BTreeSet<String> = BTreeSet::new();
        for (key, fields) in shapes {
            let name = unique_struct_name(naming::record_struct_name(&key), &mut used_names);
            record_by_fieldset.insert(key, record_structs.len());
            record_structs.push(RecordStruct { name, fields });
        }

        Ok(Self {
            interner,
            enum_names,
            func_names,
            record_structs,
            record_by_fieldset,
        })
    }

    /// Every synthesised record struct, in emission order.
    pub(crate) fn record_structs(&self) -> &[RecordStruct] {
        &self.record_structs
    }

    /// The Rust struct name for a record TYPE, keyed by its field-name set.
    ///
    /// The prepass collected every `IrType::Record` reachable from a signature,
    /// so a miss here is an internal invariant violation (SKY-I0204).
    fn record_name_for_type(&self, fields: &BTreeMap<Symbol, IrType>) -> DResult<&str> {
        let mut key = Vec::with_capacity(fields.len());
        for sym in fields.keys() {
            key.push(self.resolve_ident(*sym)?.to_owned());
        }
        key.sort();
        self.record_name_by_key(&key)
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
        self.record_by_fieldset
            .get(key)
            .and_then(|i| self.record_structs.get(*i))
            .map(|r| r.name.as_str())
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
/// (recursing through tuples and nested records). A shape is keyed by its
/// sorted field-name set and stored as the canonical `(field name, type)` list.
///
/// Two `IrType::Record`s that share a field-name set but differ in field types
/// cannot both be represented by one struct keyed on that set; this is an
/// upstream-contract violation in M1 (closed records assume one type per field
/// set), surfaced as a [`Diagnostic::CompilerBug`] (SKY-I0204) rather than a
/// silent mis-emit. (The user-facing rejection of such overloaded record types
/// is the lowerer/type-checker's responsibility.)
fn collect_record_shapes(
    interner: &Interner,
    ty: &IrType,
    shapes: &mut BTreeMap<Vec<String>, Vec<(String, IrType)>>,
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
            match shapes.get(&key) {
                Some(existing) if existing != &fields => {
                    return Err(Diagnostic::CompilerBug {
                        where_: "sky_backend_rust::collect_record_shapes",
                        detail: format!(
                            "record field set {{{}}} maps to two distinct field-type shapes; \
                             M1 closed records assume one type per field set",
                            key.join(", ")
                        ),
                    });
                }
                Some(_) => {}
                None => {
                    shapes.insert(key, fields);
                }
            }
        }
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Unit
        | IrType::TaskUnit
        | IrType::Enum(_) => {}
    }
    Ok(())
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

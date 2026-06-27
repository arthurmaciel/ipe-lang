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

use std::collections::BTreeMap;

use sky_backend::{Backend, EmittedProject};
use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::{Interner, Symbol};
use sky_ir::{FuncId, Program, TypeDef};

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

/// Shared emission context: the interner plus the precomputed Sky → Rust name
/// maps so each emit site is a `O(log n)` lookup rather than recomputing the
/// naming rules. Built once per [`RustBackend::emit`].
pub(crate) struct EmitCtx<'a> {
    interner: &'a Interner,
    /// Enum type symbol → Rust type name (e.g. `Msg` → `MainMsg`).
    enum_names: BTreeMap<Symbol, String>,
    /// Function id → Rust function name (e.g. `update` → `main_update`).
    func_names: BTreeMap<FuncId, String>,
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
        Ok(Self {
            interner,
            enum_names,
            func_names,
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

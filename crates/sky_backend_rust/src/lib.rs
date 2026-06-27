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
use sky_diagnostics::{Diagnostic, DResult};
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
        let ctx = EmitCtx::build(self.interner, program);
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
    fn build(interner: &'a Interner, program: &Program) -> Self {
        let mut enum_names = BTreeMap::new();
        let mut func_names = BTreeMap::new();
        for module in &program.modules {
            let segs: Vec<&str> = module.name.0.iter().map(|s| interner.resolve(*s)).collect();
            for ty in &module.types {
                let TypeDef::Enum(def) = ty;
                enum_names.insert(def.name, naming::enum_name(&segs, interner.resolve(def.name)));
            }
            for func in &module.funcs {
                func_names.insert(func.id, naming::module_value(&segs, interner.resolve(func.name)));
            }
        }
        Self { interner, enum_names, func_names }
    }

    fn resolve(&self, sym: Symbol) -> &str {
        self.interner.resolve(sym)
    }

    fn enum_name(&self, ty: Symbol) -> DResult<&str> {
        self.enum_names.get(&ty).map(String::as_str).ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::EmitCtx::enum_name",
            detail: format!("no Rust name for enum type symbol {}", ty.as_raw()),
        })
    }

    fn func_name(&self, id: FuncId) -> DResult<&str> {
        self.func_names.get(&id).map(String::as_str).ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::EmitCtx::func_name",
            detail: format!("no Rust name for function id {}", id.as_raw()),
        })
    }
}

#![forbid(unsafe_code)]
//! Backend-agnostic typed intermediate representation for the Sky compiler
//! (Milestone 0 subset). This is the single boundary every backend consumes:
//! the frontend lowers into [`Program`], backends read it and emit code. No
//! frontend type leaks across this line.
//!
//! Illegal states are unrepresentable for the M0 subset. In particular, a
//! [`Match`] is exhaustive by construction — the only way to build one is
//! [`Match::new`], which verifies the arm set covers exactly the scrutinee's
//! enum variants and returns [`sky_diagnostics::Diagnostic::CompilerBug`]
//! otherwise. A backend that receives a [`Program`] never has to re-check
//! exhaustiveness.

mod ir;
mod pretty;

pub use ir::{
    ir_type_is_derivable, ir_type_is_serde, Arm, BinOp, BoundSet, Callee, EnumDef, Expr, Func,
    FuncId, HtmlEventShape, IrType, KernelFn, Match, ModPath, Module, Pat, Program, TypeDef,
    UiCtor, UiPlain, Variant,
};
pub use pretty::pretty;

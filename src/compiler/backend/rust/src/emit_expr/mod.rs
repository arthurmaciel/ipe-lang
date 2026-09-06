//! Expression and function emission.
//!
//! Ports the relevant arms of `Ipê/Generate/Rust/Builder/ExprEmitter.hs` and
//! the function-item shape from `ModuleEmitter.hs`. The byte target is golden
//! `main.rs` lines 129–137 (`main_update` / `ipe_main`).


pub(crate) use ipe_diagnostics::{DResult, Diagnostic, LowerError, Span};
pub(crate) use ipe_intern::Symbol;
pub(crate) use ipe_ir::{
    Arm, BinOp, BoundSet, Callee, Expr, Func, IrType, KernelFn, MAX_IR_RENDER_DEPTH, Match,
    ModPath, Pat,
};
pub(crate) use crate::EmitCtx;
pub(crate) use crate::doc::Doc;
pub(crate) use crate::emit_types::{GenericScope, render_type};
pub(crate) use crate::emit_ui_plan::{
    ArgPlan, Guard, LitKind, NativeUiEmit, UiDelegate, UiEmitPlan, appearance_literal_args,
    appearance_literal_record_fields, ui_call_shape,
};
pub(crate) use crate::naming::kernel_name;
pub(crate) use crate::render::{RenderConfig, render_seeded};

mod analysis;
mod ffi;
mod kernel_calls;
mod expr;
mod patterns;
mod records;
mod func;
#[cfg(test)]
mod tests;

pub(crate) use analysis::*;
pub(crate) use ffi::*;
pub(crate) use kernel_calls::*;
pub(crate) use expr::*;
pub(crate) use patterns::*;
pub(crate) use records::*;
pub(crate) use func::*;


#![forbid(unsafe_code)]
//! The Rust backend for the Sky compiler (Milestone 0).
//!
//! Consumes the backend-agnostic typed [`sky_ir::Program`] and emits a Rust
//! Cargo project. This crate is split into the fixed templates emitted for
//! every program ([`preamble`] / [`epilogue`]) and — in later tasks — the
//! type/expression emission and project assembly.

mod preamble;

pub use preamble::{epilogue, preamble};

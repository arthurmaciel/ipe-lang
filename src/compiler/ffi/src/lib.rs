//! Shim-free Rust-crate FFI binding generator.
//!
//! Turns the `ipe-ffi-inspector` JSON (`PkgInfo`) into three artifacts: a
//! `.ipei` type-environment seed, a `kernel.json` call registry, and a
//! `<crate>_bindings.rs` wrapper file.
//!
//! There are exactly two typed decode boundaries (`pkginfo`, `call`), so
//! every downstream emitter is a total function over already-validated data.
//!
//! Governing invariant: `ipe build` ⇒ `cargo build`. The only sound error
//! direction is over-drop at introspection (a bindable symbol omitted) plus
//! reject-at-decode with `IPE-F4400` (an unrenderable foreign call refused
//! before emission). Under-bind — emitting a binding cargo then rejects — is
//! forbidden.
//!
//! Module DAG (leaf-first): `num_coerce` → `diag` → `naming` → `pkginfo` /
//! `typeref` → `call` → `emit` → `instance` → `driver`.

pub mod diag;
pub mod num_coerce;

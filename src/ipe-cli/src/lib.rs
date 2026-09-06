#![forbid(unsafe_code)]
//! `ipe` — the command-line driver.
//!
//! Wires the pipeline end to end: read a `.ipe` entry file, run it through
//! [`ipe_parse`] → [`ipe_canon`] → [`ipe_types`] → [`ipe_lower`] → the
//! [`ipe_backend_rust`] emitter, write the emitted Cargo project, and vendor the
//! Ipe runtime module tree into it (a port of the copy step in the Haskell
//! compiler's `Ipe.Generate.Rust.Project`).
//!
//! Generated Rust projects do not depend on the runtime as a Cargo path crate;
//! instead `main.rs` declares `mod ipe_runtime;` and the runtime sources are
//! copied in beside it. The driver therefore must locate
//! `src/runtime/rust/src/` (the in-repo copy) and vendor it under
//! `<out>/src/ipe_runtime/`.
//!
//! Errors are typed ([`CliError`]); no operation panics or unwraps.

pub mod advisory;
pub mod api_surface;
pub mod audit;
pub mod audit_native;
pub mod build_plan;
mod cache;
pub mod clean;
pub mod cli_args;
pub mod contained_path;
pub mod coverage;
pub mod delivery;
pub mod diff;
pub mod doc;
pub mod doc_bundle;
pub mod doc_type_search;
pub mod ffi;
pub mod fmt;
pub mod health;
pub mod help;
pub mod hot_classify;
pub mod index;
pub mod init;
pub mod io_bounded;
pub mod lint;
pub mod lockfile;
pub mod login;
mod lsp;
pub mod migrate;
pub mod native_ffi_consent;
pub mod net;
pub mod pack;
pub mod package_manifest;
pub mod package_name;
pub mod pkg;
pub mod progress;
pub mod project;
pub mod publish;
pub mod registry;
pub mod resolve;
pub mod run_sandbox;
pub mod runtime_embed;
pub mod scratch;
pub mod signing;
pub mod style;
pub mod toolchain;
pub mod unsafe_ack;
pub mod version_check;
pub mod web_consent;
/// The embedded Ipê standard-library source now lives in the dependency-free
/// [`ipe_stdlib`] leaf crate so the WebAssembly frontend can share one copy.
/// Re-exported here so `crate::stdlib::…` call sites resolve unchanged.
pub use ipe_stdlib as stdlib;
pub mod watch;

pub(crate) use std::collections::{BTreeMap, BTreeSet};
pub(crate) use std::fs;
pub(crate) use std::io::Write;
pub(crate) use std::path::{Path, PathBuf};

pub(crate) use ipe_diagnostics::{
    ALL_CODES, Applicability, Diagnostic, HelpLine, Suggestion, explain_page, render, render_json,
    title,
};
pub(crate) use ipe_intern::Interner;

mod driver_error;
mod build_pipeline;
mod commands;
mod commands_pkg;
#[cfg(test)]
mod tests;

pub use driver_error::*;
pub use build_pipeline::*;
pub use commands::*;
pub use commands_pkg::*;


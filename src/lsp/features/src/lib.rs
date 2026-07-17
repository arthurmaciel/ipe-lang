#![forbid(unsafe_code)]
//! `ipe_lsp_features` — pure LSP feature handlers over the `ipe_db` salsa
//! graph.
//!
//! Every handler here is a pure function from (database snapshot, position)
//! to an LSP payload. This crate owns **no** parser, resolver, or solver of
//! its own: diagnostics, types, and navigation targets are read from the same
//! memoized `ipe_db` queries `ipe build` and `ipe watch` run, so an answer
//! that disagrees with the compiler has no code path. No function here
//! touches `std::fs`, `std::env`, or the clock — file text enters through
//! the `SourceFile` inputs the driver (the LSP server crate) sets.

pub mod diagnostics;
pub mod hover;
pub mod offset;
pub mod symbols;

pub use offset::PositionEncoding;

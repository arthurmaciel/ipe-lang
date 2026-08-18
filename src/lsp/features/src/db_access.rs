//! Cycle-safe database accessors for interactive LSP request handlers.
//!
//! Every direct `ipe_db::canonicalize` demand on a cyclic import graph hits
//! salsa's dependency-cycle panic. The diagnostics worker avoids this by
//! gating on `topo_order` first (see `diagnostics.rs`). This module provides
//! the same gate as a single shared accessor so every interactive provider
//! calls it instead of raw `ipe_db::canonicalize` — making the ungated demand
//! structurally unrepresentable on the interactive path.

use std::sync::Arc;

use ipe_db::{CanonicalModule, IpeDatabase, SourceFile, SourceRoot};

/// Cycle-safe canonicalize for interactive request handlers.
///
/// Proves the import graph acyclic via `topo_order` before demanding
/// `canonicalize`, so a cyclic graph yields `None` here instead of
/// salsa's dependency-cycle panic. Mirrors the gate the diagnostics
/// worker uses — the SSOT proof-of-acyclicity for every direct
/// canonicalize demand on the interactive path.
#[must_use]
pub fn canonicalize_checked(
    db: &IpeDatabase,
    root: SourceRoot,
    entry: SourceFile,
    file: SourceFile,
) -> Option<Arc<CanonicalModule>> {
    ipe_db::topo_order(db, root, entry).ok()?; // cycle → None, no panic
    ipe_db::canonicalize(db, root, file).ok()
}

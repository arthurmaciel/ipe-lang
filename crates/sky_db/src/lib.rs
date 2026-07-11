#![forbid(unsafe_code)]
//! `sky_db` — the salsa incremental-compilation database (Phase 1).
//!
//! Authoritative design: `docs/architecture/incremental-compilation-and-watch.md`
//! (locked Q1–Q4). Phase-1 scope + decisions ledger:
//! `docs/architecture/salsa-incremental-compilation-2026-07-11.md`.
//!
//! Phase 1 puts the earliest front-end stages behind memoized salsa queries:
//!
//! - **Inputs** (the parse-don't-validate boundary): [`SourceFile`] (module
//!   path + text, one per in-scope `.sky` module) and [`SourceRoot`] (the
//!   in-scope file set — the design spec's `file_set()`).
//! - **Tracked queries**: [`parse`] (per-file AST, errors as values) and
//!   [`imports`] (per-file import list via the same string-scan the driver's
//!   topological sort has always used).
//!
//! INV-1 (no hidden inputs): no query here touches `std::fs`, `std::env`, or
//! the clock. File reading stays in the driver, which is where inputs are set.
//!
//! The interning story is the plan's Option 3a: the database owns a
//! [`SharedInterner`] (`Arc<Mutex<sky_intern::Interner>>`) shared with the
//! driver. Interning is **append-only** — symbols are never freed or
//! renumbered — so a memoized `Module` from an earlier revision always
//! resolves against the current interner. Symbol *numbering* depends on the
//! query-demand order; the one-shot `skyc` driver demands queries in a fixed
//! topological order against a cold database, so emitted bytes are identical
//! to the pre-salsa pipeline (enforced by the golden-oracle suite). Warm-db
//! reuse stays confined to tests until the clean-vs-incremental parity gate
//! (plan Task 18) exists.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use salsa::Setter as _;
use sky_diagnostics::Diagnostic;
use sky_intern::Interner;
use sky_syntax::Module;

// ---------------------------------------------------------------------------
// The shared interner (plan Task 3, Option 3a)
// ---------------------------------------------------------------------------

/// A database-owned, append-only interner shared between salsa queries and
/// the (non-salsa) driver stages that still take `&mut Interner`.
///
/// Cloning is cheap (an `Arc` bump) and every clone refers to the same
/// underlying table — symbol identity is preserved across the whole build.
#[derive(Clone, Default)]
pub struct SharedInterner(Arc<Mutex<Interner>>);

impl SharedInterner {
    /// A fresh, empty interner.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Lock the interner for use.
    ///
    /// Poison-safe: interning is append-only, so a panic mid-`intern` cannot
    /// leave the table in a logically-invalid state — recovering the guard
    /// from a poisoned mutex is sound (the same pattern the runtime uses for
    /// its poison-safe locks).
    pub fn lock(&self) -> MutexGuard<'_, Interner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

/// The database trait tracked functions run against: salsa plus access to
/// the shared interner.
#[salsa::db]
pub trait Db: salsa::Database {
    /// The build-wide shared interner (see [`SharedInterner`]).
    fn interner(&self) -> &SharedInterner;
}

/// The concrete Sky compiler database.
#[salsa::db]
#[derive(Clone, Default)]
pub struct SkyDatabase {
    storage: salsa::Storage<Self>,
    interner: SharedInterner,
}

impl SkyDatabase {
    /// A fresh (cold) database with a fresh interner.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A fresh database that forwards every salsa runtime event to
    /// `callback` — the observability hook the incrementality tests use to
    /// assert memo hits (`EventKind::WillExecute` absence/presence).
    #[must_use]
    pub fn with_event_callback(
        callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static>,
    ) -> Self {
        Self {
            storage: salsa::Storage::new(Some(callback)),
            interner: SharedInterner::new(),
        }
    }
}

#[salsa::db]
impl salsa::Database for SkyDatabase {}

#[salsa::db]
impl Db for SkyDatabase {
    fn interner(&self) -> &SharedInterner {
        &self.interner
    }
}

// ---------------------------------------------------------------------------
// Inputs (the parse-don't-validate boundary; the driver sets these)
// ---------------------------------------------------------------------------

/// One in-scope `.sky` module: its module path (identity/diagnostic key) and
/// full UTF-8 source text (the real input).
#[salsa::input]
pub struct SourceFile {
    /// Module path segments, e.g. `["Std", "List"]`.
    #[returns(ref)]
    pub module_path: Vec<String>,
    /// Full source text.
    #[returns(ref)]
    pub text: String,
}

/// The in-scope file set — the design spec's `file_set()` input. Makes the
/// "ALL modules" quantifier an explicit input rather than an implicit walk.
#[salsa::input]
pub struct SourceRoot {
    /// Module path → source file, for every module in the build.
    #[returns(ref)]
    pub files: BTreeMap<Vec<String>, SourceFile>,
}

/// Driver-boundary helper: update `file`'s text only when the bytes changed.
///
/// Salsa dirties on every `set_*` regardless of value, so the
/// byte-equal-re-save no-op lives here, at the boundary. Returns `true` when
/// a set happened.
pub fn set_text_if_changed(db: &mut SkyDatabase, file: SourceFile, new_text: &str) -> bool {
    if file.text(db) == new_text {
        return false;
    }
    file.set_text(db).to(new_text.to_owned());
    true
}

// ---------------------------------------------------------------------------
// Tracked queries (Phase 1: parse + imports)
// ---------------------------------------------------------------------------

/// The memoized result of parsing one module. Errors are **values** — a
/// query must be total, so a red parse yields `Err(diagnostic)` for every
/// downstream consumer instead of unwinding.
pub type ParseResult = Result<Arc<Module>, Diagnostic>;

/// Parse one module. Keyed on `file` — depends only on `text(self)` (plus
/// the append-only shared interner), so an edit to module B never re-parses
/// module A.
#[salsa::tracked]
pub fn parse(db: &dyn Db, file: SourceFile) -> ParseResult {
    let text = file.text(db);
    let mut interner = db.interner().lock();
    sky_parse::parse_module(text, &mut interner).map(Arc::new)
}

/// The import list of one module, keyed on `file`'s text.
///
/// Deliberately the same pre-parse string-scan the driver's topological sort
/// has always used (not derived from [`parse`]): the topo sort must work even
/// on files whose parse would fail, and changing that ordering is an
/// observable behavior change that belongs behind the clean-vs-incremental
/// parity gate (plan Task 5/18), not in the byte-identical Phase 1.
#[salsa::tracked]
pub fn imports(db: &dyn Db, file: SourceFile) -> Arc<Vec<Vec<String>>> {
    Arc::new(extract_imports_from_source(file.text(db)))
}

/// Extract `import A.B.C` module paths from raw source, one per line.
///
/// Kernel imports are included verbatim; the caller filters against its
/// known-module set (unknown paths are skipped by the topo sort and later
/// surface as SKY-N0020 in canonicalisation). Returns a `Vec<Vec<String>>`
/// of path segments.
#[must_use]
pub fn extract_imports_from_source(source: &str) -> Vec<Vec<String>> {
    let mut imports: Vec<Vec<String>> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        let Some(after_import) = trimmed.strip_prefix("import ") else {
            continue;
        };
        // Take the token after `import `, stopping at `as`, `exposing`, or
        // whitespace.
        let rest = after_import.trim_start();
        let module_str = rest
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or("");
        // Remove a trailing `as` keyword if it bled in (shouldn't happen but
        // defensive).
        let module_str = module_str
            .strip_suffix(" as")
            .map_or(module_str, str::trim);
        let module_str = module_str.trim_end_matches(" as");
        let parts: Vec<String> = module_str.split('.').map(str::to_owned).collect();
        if parts.first().is_some_and(|s| !s.is_empty()) {
            imports.push(parts);
        }
    }
    imports
}

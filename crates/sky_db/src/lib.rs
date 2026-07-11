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
//! Phase 2 (spec §5 row 2 — plan Tasks 5/6/7/8) adds the canonicalisation
//! tier: [`resolve_imports`] (the closed-enum module-resolution edge, AST-
//! derived), [`canonicalize`] (per-module name resolution), and
//! [`module_interface`] (the export-surface firewall — importers early-cut on
//! dep body-only edits via salsa backdating).
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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use salsa::Setter as _;
use sky_diagnostics::Diagnostic;
use sky_intern::{Interner, Symbol};
/// Re-exported so drivers and tests can name the trust tag and interface
/// type without a direct `sky_canon` dependency.
pub use sky_canon::{ModuleExports, ModuleOrigin};
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
#[salsa::input(debug)]
pub struct SourceFile {
    /// Module path segments, e.g. `["Std", "List"]`.
    #[returns(ref)]
    pub module_path: Vec<String>,
    /// Full source text.
    #[returns(ref)]
    pub text: String,
    /// The driver-vouched trust tag (Phase 2). `EmbeddedStdlib` is only ever
    /// set by the driver for module paths it injected from the compiler's own
    /// embed table — a user file squatting on `Std.Foo` arrives as `User` and
    /// stays SKY-N0025-rejected. Unforgeable from module text by construction:
    /// inputs are set exclusively at the driver boundary.
    pub origin: ModuleOrigin,
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

// ---------------------------------------------------------------------------
// Tracked queries (Phase 2: resolve_imports + module_interface + canonicalize)
// ---------------------------------------------------------------------------

/// Closed-enum result of resolving one `import` path against the file set.
///
/// `Ambiguous` (two files claiming one module path) is deliberately absent:
/// [`SourceRoot`]'s `files` map is a `BTreeMap` keyed by module path — the
/// same invariant the driver's source map enforces (its stdlib injection
/// skips pre-existing keys) — so a double claim is *unrepresentable* at the
/// input boundary, not merely checked here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImportResolution {
    /// The import names an in-scope project module (user source or injected
    /// compiled-source stdlib).
    Resolved(SourceFile),
    /// Not in the file set: either a kernel module (canonicalisation resolves
    /// those against its built-in table) or a genuinely missing module
    /// (canonicalisation emits SKY-N0020).
    Unresolved,
}

/// Per-import resolutions in import-declaration order, or the module's parse
/// diagnostic (the resolution of an unparseable module is *unknown*, not
/// empty — parse-don't-validate).
pub type ImportResolutions = Result<Arc<Vec<(Vec<String>, ImportResolution)>>, Diagnostic>;

/// Resolve every `import` of `file` against the project file set.
///
/// Derived from the parsed AST (the plan's Task-5 shape), NOT the pre-parse
/// string-scan [`imports`] query — this query's consumer is [`canonicalize`],
/// which iterates the AST's import declarations, so the two must agree
/// exactly. The string-scan stays in service of the driver's topological
/// sort only (recorded parity choice, spec §3.4).
///
/// Reads the whole `files` field, so adding/removing/renaming ANY file
/// re-validates every module's resolutions (H6 — the set-vs-contents gate);
/// unchanged results backdate and cut dependents.
#[salsa::tracked]
pub fn resolve_imports(db: &dyn Db, root: SourceRoot, file: SourceFile) -> ImportResolutions {
    let module = parse(db, file)?;
    let files = root.files(db);
    let interner = db.interner().lock();
    let mut resolutions = Vec::with_capacity(module.imports.len());
    for import in &module.imports {
        let path = import
            .name
            .value
            .iter()
            .map(|&segment| {
                interner.resolve(segment).map(str::to_owned).ok_or_else(|| {
                    Diagnostic::CompilerBug {
                        where_: "sky_db.resolve_imports",
                        detail: "parsed import path contains an unresolvable symbol".to_owned(),
                    }
                })
            })
            .collect::<Result<Vec<String>, Diagnostic>>()?;
        let resolution = files
            .get(&path)
            .map_or(ImportResolution::Unresolved, |dep| {
                ImportResolution::Resolved(*dep)
            });
        resolutions.push((path, resolution));
    }
    Ok(Arc::new(resolutions))
}

/// The output of canonicalising one module: the name-resolved AST plus its
/// export surface.
#[derive(Clone, PartialEq, Debug)]
pub struct CanonicalModule {
    /// The name-resolved module.
    pub module: sky_canon::ast::Module,
    /// The module's cross-module-observable export surface.
    pub exports: ModuleExports,
}

/// The memoized result of canonicalising one module.
pub type CanonResult = Result<Arc<CanonicalModule>, Diagnostic>;

/// Canonicalise one module: `parse(self)` + `resolve_imports(self)` +
/// `module_interface(dep)` for each resolved dep.
///
/// Import cycles: the driver's topological sort rejects cycles (SKY-N0021)
/// before any `canonicalize` demand, so the production path never recurses
/// into a cycle. A direct demand on a cyclic import graph hits salsa's
/// cycle panic — fail-loud, never a stale or fixpointed value.
#[salsa::tracked]
pub fn canonicalize(db: &dyn Db, root: SourceRoot, file: SourceFile) -> CanonResult {
    let parsed = parse(db, file)?;
    let resolutions = resolve_imports(db, root, file)?;

    // Demand every dep interface BEFORE taking the interner lock: a cold
    // demand recurses into `canonicalize(dep)`, which locks the
    // (non-reentrant) interner itself.
    let mut dep_interfaces: Vec<(Vec<String>, Arc<ModuleExports>)> = Vec::new();
    for (path, resolution) in resolutions.iter() {
        if let ImportResolution::Resolved(dep) = resolution {
            dep_interfaces.push((path.clone(), module_interface(db, root, *dep)?));
        }
    }

    // Known-module universe for the SKY-N0020 did-you-mean list. Strings
    // only: interning module paths here (before their own canonicalize runs)
    // would perturb the build-wide symbol numbering the byte-identity SEAL
    // pins.
    let known_modules: BTreeSet<Box<str>> = root
        .files(db)
        .keys()
        .map(|path| path.join(".").into_boxed_str())
        .collect();
    let origin = file.origin(db);

    // One lock scope covers expected-path interning + canonicalisation — the
    // exact interning sequence the pre-salsa driver produced (dep paths were
    // interned when each dep's own canonicalize ran, so those interns below
    // are lookups, not appends).
    let mut interner = db.interner().lock();
    let expected_path: Vec<Symbol> = file
        .module_path(db)
        .iter()
        .map(|segment| interner.intern(segment))
        .collect::<Result<_, _>>()?;
    let mut deps: BTreeMap<Vec<Symbol>, &ModuleExports> = BTreeMap::new();
    for (path, interface) in &dep_interfaces {
        let key: Vec<Symbol> = path
            .iter()
            .map(|segment| interner.intern(segment))
            .collect::<Result<_, _>>()?;
        deps.insert(key, interface.as_ref());
    }
    let (module, exports) = sky_canon::canonicalise_module_in_project(
        &parsed,
        &expected_path,
        &deps,
        &known_modules,
        origin,
        &mut interner,
    )?;
    Ok(Arc::new(CanonicalModule { module, exports }))
}

/// The cross-module interface of one module — its export surface, projected
/// out of [`canonicalize`].
///
/// This is the PRIMARY invalidation firewall (plan Task 7): when a body-only
/// edit re-runs `canonicalize(A)` but the exports come out **equal**, salsa
/// backdates this query's memo and every importer's `canonicalize` stays
/// valid without re-executing. Deliberately a projection of `canonicalize`
/// rather than a second parse-only summarizer: one export-computation code
/// path can never drift from what canonicalisation actually injects
/// (correctness > efficiency).
///
/// Sound over-approximation note: [`ModuleExports`] carries exported alias
/// *bodies* (with source spans), so an edit that shifts an exported alias's
/// spans re-canonicalises importers even when nothing semantic changed —
/// over-invalidation, never under-invalidation. Span-erased interfaces are a
/// recorded follow-up.
#[salsa::tracked]
pub fn module_interface(
    db: &dyn Db,
    root: SourceRoot,
    file: SourceFile,
) -> Result<Arc<ModuleExports>, Diagnostic> {
    let canonical = canonicalize(db, root, file)?;
    Ok(Arc::new(canonical.exports.clone()))
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

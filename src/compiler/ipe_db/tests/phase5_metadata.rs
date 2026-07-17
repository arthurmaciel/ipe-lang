#![forbid(unsafe_code)]
//! Phase-5 proofs (spec:
//! `docs/architecture/salsa-incremental-compilation-2026-07-11.md` §5 row 5).
//!
//! [`ipe_db::program_metadata`] is the coarse, LOCKED whole-program
//! DCE-reachability seam over [`ipe_db::lower_program`]'s output: it depends
//! DIRECTLY on the full lowered-IR set (never firewalled behind an interface
//! summary), so it re-executes on the exact same coarse floor
//! `typecheck`/`lower_program` already established in Phase 4. These tests
//! prove that shape, PLUS the actual reachability computation itself
//! (something Phase 4's coarse seams had no equivalent of — `typecheck` and
//! `lower_program` reuse an existing whole-program computation unchanged;
//! `program_metadata` is a genuinely new structural analysis, so it needs its
//! own correctness proof, not just a memoization proof).

use std::sync::{Arc, Mutex, PoisonError};

use ipe_db::{ModuleOrigin, SkyDatabase, SourceFile, SourceRoot};

/// A shared, poison-safe log of executed-query debug keys.
#[derive(Clone, Default)]
struct EventLog(Arc<Mutex<Vec<String>>>);

impl EventLog {
    fn push(&self, entry: String) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(entry);
    }

    /// Number of `WillExecute` events whose debug key mentions `needle`.
    fn executions_of(&self, needle: &str) -> usize {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|e| e.contains(needle))
            .count()
    }

    fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

/// A database whose `WillExecute` events land in the returned log.
fn logged_db() -> (SkyDatabase, EventLog) {
    let log = EventLog::default();
    let sink = log.clone();
    let db = SkyDatabase::with_event_callback(Box::new(move |event: salsa::Event| {
        if let salsa::EventKind::WillExecute { database_key } = event.kind {
            sink.push(format!("{database_key:?}"));
        }
    }));
    (db, log)
}

fn file(db: &SkyDatabase, path: &[&str], text: &str) -> SourceFile {
    SourceFile::new(
        db,
        path.iter().map(|s| (*s).to_owned()).collect(),
        text.to_owned(),
        ModuleOrigin::User,
    )
}

fn root_of(db: &SkyDatabase, files: &[(&[&str], SourceFile)]) -> SourceRoot {
    SourceRoot::new(
        db,
        files
            .iter()
            .map(|(path, f)| (path.iter().map(|s| (*s).to_owned()).collect(), *f))
            .collect(),
    )
}

const DEP_A: &str = "module A exposing (visible)\n\nvisible = 1\n\nhidden = 2\n";
const DEP_A_BODY_EDIT: &str = "module A exposing (visible)\n\nvisible = 1\n\nhidden = 3\n";
const IMPORTER_B: &str = "module B exposing (b)\n\nimport A exposing (visible)\n\nb = visible\n";

// ---------------------------------------------------------------------------
// Memoization shape (same coarse floor as typecheck / lower_program)
// ---------------------------------------------------------------------------

/// Coarse-but-memoized: a repeat demand and a byte-equal re-save execute
/// nothing; a reachable dep's body edit re-executes the whole computation —
/// the same shape the coarse `typecheck`/`lower_program` seams hold, now one
/// layer further down the pipeline.
#[test]
fn program_metadata_memoized_coarse_floor() {
    let (mut db, log) = logged_db();
    let a = file(&db, &["A"], DEP_A);
    let b = file(&db, &["B"], IMPORTER_B);
    let root = root_of(&db, &[(&["A"], a), (&["B"], b)]);

    let meta = ipe_db::program_metadata(&db, root, b).expect("trivial int program must lower");
    assert!(
        !meta.reachable_funcs.is_empty(),
        "a program with at least one def must report at least one reachable func \
         (no-entry fallback: every func is reachable)"
    );
    assert_eq!(log.executions_of("program_metadata("), 1);

    // Repeat demand: memo hit.
    log.clear();
    assert!(ipe_db::program_metadata(&db, root, b).is_ok());
    assert_eq!(
        log.executions_of("program_metadata("),
        0,
        "repeat demand memoized"
    );

    // Byte-equal re-save: boundary no-op, nothing executes anywhere in the
    // chain (parse → canonicalize → linked_program → typecheck →
    // lower_program → program_metadata).
    log.clear();
    assert!(!ipe_db::set_text_if_changed(&mut db, a, DEP_A));
    assert!(ipe_db::program_metadata(&db, root, b).is_ok());
    assert_eq!(log.executions_of("program_metadata("), 0);

    // Dep body edit: the coarse seam re-executes (lower_program's output
    // changed, and program_metadata depends on it directly — never
    // firewalled, per the design spec's own H6 lock).
    log.clear();
    assert!(ipe_db::set_text_if_changed(&mut db, a, DEP_A_BODY_EDIT));
    assert!(ipe_db::program_metadata(&db, root, b).is_ok());
    assert_eq!(
        log.executions_of("program_metadata("),
        1,
        "coarse seam re-computes on a semantic edit anywhere in the program"
    );
}

/// `program_metadata` never reaches its own structural walk on ill-typed
/// input — it short-circuits through `lower_program`'s (and therefore
/// `typecheck`'s) error via `?`.
#[test]
fn program_metadata_short_circuits_on_lower_error() {
    const RED_ENTRY: &str = "module Entry exposing (e)\n\n\
        e : Int\n\
        e = \"not an int\"\n";
    let (db, _log) = logged_db();
    let entry = file(&db, &["Entry"], RED_ENTRY);
    let root = root_of(&db, &[(&["Entry"], entry)]);

    let lower_err =
        ipe_db::lower_program(&db, root, entry).expect_err("ill-typed program must fail to lower");
    let meta_err = ipe_db::program_metadata(&db, root, entry)
        .expect_err("program_metadata must refuse to analyse an unlowered program");
    assert_eq!(
        lower_err, meta_err,
        "program_metadata's short-circuit must surface lower_program's own diagnostic verbatim"
    );
}

// ---------------------------------------------------------------------------
// The reachability computation itself
// ---------------------------------------------------------------------------

/// A genuine dead-code exclusion: `main` calls `live`, which never mentions
/// `dead`. `dead`'s `FuncId` must be absent from `reachable_funcs` even
/// though it is a well-typed, successfully-lowered top-level def.
#[test]
fn program_metadata_excludes_unreached_function() {
    const ENTRY_WITH_DEAD_CODE: &str = "module Entry exposing (main)\n\n\
        live = 1\n\n\
        dead = 2\n\n\
        main = live\n";
    let (db, _log) = logged_db();
    let entry = file(&db, &["Entry"], ENTRY_WITH_DEAD_CODE);
    let root = root_of(&db, &[(&["Entry"], entry)]);

    let program = ipe_db::lower_program(&db, root, entry).expect("must lower");
    let meta = ipe_db::program_metadata(&db, root, entry).expect("must compute metadata");

    let module = program
        .modules
        .first()
        .expect("lower_program must produce at least one module");
    assert!(
        module.entry.is_some(),
        "a module defining `main` must record it as the program entry"
    );
    let main_id = find_func_id(&db, module, "main");
    let live_id = find_func_id(&db, module, "live");
    let dead_id = find_func_id(&db, module, "dead");

    assert!(
        meta.reachable_funcs.contains(&main_id),
        "the entry function itself must be reachable"
    );
    assert!(
        meta.reachable_funcs.contains(&live_id),
        "a function the entry calls must be reachable"
    );
    assert!(
        !meta.reachable_funcs.contains(&dead_id),
        "a function nothing reachable calls must NOT be reported reachable \
         (this is the whole point of a DCE-reachability seam)"
    );
}

/// Transitive reachability: `main` calls `mid`, which calls `leaf` — `leaf`
/// must be reachable even though `main` never names it directly.
#[test]
fn program_metadata_reachability_is_transitive() {
    const CHAIN: &str = "module Entry exposing (main)\n\n\
        leaf = 1\n\n\
        mid = leaf\n\n\
        dead = 99\n\n\
        main = mid\n";
    let (db, _log) = logged_db();
    let entry = file(&db, &["Entry"], CHAIN);
    let root = root_of(&db, &[(&["Entry"], entry)]);

    let program = ipe_db::lower_program(&db, root, entry).expect("must lower");
    let meta = ipe_db::program_metadata(&db, root, entry).expect("must compute metadata");
    let module = program
        .modules
        .first()
        .expect("lower_program must produce at least one module");

    assert!(
        meta.reachable_funcs
            .contains(&find_func_id(&db, module, "main"))
    );
    assert!(
        meta.reachable_funcs
            .contains(&find_func_id(&db, module, "mid"))
    );
    assert!(
        meta.reachable_funcs
            .contains(&find_func_id(&db, module, "leaf")),
        "reachability must transit through an intermediate call, not just direct callees"
    );
    assert!(
        !meta
            .reachable_funcs
            .contains(&find_func_id(&db, module, "dead"))
    );
}

/// Look up a lowered function's [`ipe_ir::FuncId`] by its source name.
/// Test-only helper (`expect`, not `panic!`, per the workspace's clippy
/// bar — bare `panic!` is denied even in test code, `expect` is allowed).
/// `clippy.toml`'s `allow-expect-in-tests` only auto-exempts code lexically
/// inside a `#[test]` fn; a free helper called FROM one needs the explicit
/// allow.
#[allow(clippy::expect_used)]
fn find_func_id(db: &ipe_db::SkyDatabase, module: &ipe_ir::Module, name: &str) -> ipe_ir::FuncId {
    let interner = ipe_db::Db::interner(db).lock();
    let msg = format!("no func named {name:?} in the lowered module");
    module
        .funcs
        .iter()
        .find(|f| interner.resolve(f.name) == Some(name))
        .map(|f| f.id)
        .expect(&msg)
}

/// A program with no `main` binding has no entry to seed a fixpoint from —
/// the conservative fallback treats every function as reachable rather than
/// guessing (never under-reports).
#[test]
fn program_metadata_no_entry_falls_back_to_conservative_reachable_everything() {
    const NO_MAIN: &str = "module Lib exposing (a, b)\n\na = 1\n\nb = 2\n";
    let (db, _log) = logged_db();
    let entry = file(&db, &["Lib"], NO_MAIN);
    let root = root_of(&db, &[(&["Lib"], entry)]);

    let program = ipe_db::lower_program(&db, root, entry).expect("must lower");
    let meta = ipe_db::program_metadata(&db, root, entry).expect("must compute metadata");
    let module = program
        .modules
        .first()
        .expect("lower_program must produce at least one module");
    assert!(module.entry.is_none(), "this fixture defines no `main`");
    assert_eq!(
        meta.reachable_funcs.len(),
        module.funcs.len(),
        "no-entry fallback must report every function reachable, never fewer"
    );
}

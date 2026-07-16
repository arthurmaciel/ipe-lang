#![forbid(unsafe_code)]
//! Phase-4 proofs (spec:
//! `docs/architecture/salsa-incremental-compilation-2026-07-11.md` §5 row 4 —
//! plan Tasks 12 and 13).
//!
//! [`sky_db::typecheck`] and [`sky_db::lower_program`] are the coarse
//! per-program SEAMS over `sky_types::infer_attributed` / `sky_lower::lower`:
//! memoized, but keyed on the whole [`sky_db::linked_program`] merge, not on
//! any individual module. These tests prove exactly that shape — real memo
//! hits on a no-op repeat/re-save, AND a full re-execution on ANY reachable
//! module's body edit, even one unrelated to the edited module's siblings
//! (the documented coarseness, not a bug).

use std::sync::{Arc, Mutex, PoisonError};

use sky_db::{ModuleOrigin, SkyDatabase, SourceFile, SourceRoot};

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

const DEP_C: &str = "module C exposing (c)\n\nc = 2\n";
const DEP_C_BODY_EDIT: &str = "module C exposing (c)\n\nc = 3\n";
const ENTRY_WITH_TWO_DEPS: &str = "module Entry exposing (e)\n\n\
    import A exposing (visible)\n\
    import C exposing (c)\n\n\
    e = visible + c\n";

// ---------------------------------------------------------------------------
// typecheck (Task 12 — the coarse per-program SEAM)
// ---------------------------------------------------------------------------

/// Coarse-but-memoized, the `linked_program` pattern: a repeat demand and a
/// byte-equal re-save execute nothing; a reachable dep's body edit
/// re-executes the whole solve (documented coarseness — true per-module
/// `typecheck(ModuleId)` is the recorded Phase-4 follow-up, not shipped
/// here).
#[test]
fn typecheck_memoized_coarse_floor() {
    let (mut db, log) = logged_db();
    let a = file(&db, &["A"], DEP_A);
    let b = file(&db, &["B"], IMPORTER_B);
    let root = root_of(&db, &[(&["A"], a), (&["B"], b)]);

    let solved = sky_db::typecheck(&db, root, b).expect("trivial int program must type-check");
    assert!(
        !solved.env.is_empty(),
        "solved types must carry at least one binding"
    );
    assert_eq!(log.executions_of("typecheck("), 1);

    // Repeat demand: memo hit.
    log.clear();
    assert!(sky_db::typecheck(&db, root, b).is_ok());
    assert_eq!(log.executions_of("typecheck("), 0, "repeat demand memoized");

    // Byte-equal re-save: boundary no-op, nothing executes anywhere in the
    // chain (parse → canonicalize → linked_program → typecheck).
    log.clear();
    assert!(!sky_db::set_text_if_changed(&mut db, a, DEP_A));
    assert!(sky_db::typecheck(&db, root, b).is_ok());
    assert_eq!(log.executions_of("typecheck("), 0);

    // Dep body edit: the coarse seam re-executes (linked_program's assembled
    // input changed, so typecheck's own dependency changed).
    log.clear();
    assert!(sky_db::set_text_if_changed(&mut db, a, DEP_A_BODY_EDIT));
    assert!(sky_db::typecheck(&db, root, b).is_ok());
    assert_eq!(
        log.executions_of("typecheck("),
        1,
        "coarse seam re-checks on a semantic edit anywhere in the program"
    );
}

/// The seam is genuinely PROGRAM-wide, not per-module: editing module C's
/// body (unrelated to sibling module A — no import edge between them) still
/// forces a full re-execution of `typecheck`, because both are merged into
/// the SAME `linked_program`. A true per-module query would leave an
/// A-only-dependent memo untouched here; this seam does not, and the gap is
/// the documented Phase-4 follow-up.
#[test]
fn typecheck_is_program_wide_not_per_module() {
    let (mut db, log) = logged_db();
    let a = file(&db, &["A"], DEP_A);
    let c = file(&db, &["C"], DEP_C);
    let entry = file(&db, &["Entry"], ENTRY_WITH_TWO_DEPS);
    let root = root_of(&db, &[(&["A"], a), (&["C"], c), (&["Entry"], entry)]);

    assert!(sky_db::typecheck(&db, root, entry).is_ok());
    assert_eq!(log.executions_of("typecheck("), 1);

    // Edit ONLY C — a sibling dep of A under Entry, no edge to A at all.
    log.clear();
    assert!(sky_db::set_text_if_changed(&mut db, c, DEP_C_BODY_EDIT));
    assert!(sky_db::typecheck(&db, root, entry).is_ok());
    assert_eq!(
        log.executions_of("typecheck("),
        1,
        "unrelated sibling's body edit still busts the whole-program seam"
    );
}

// ---------------------------------------------------------------------------
// lower_program (Task 13 — the coarse per-program SEAM, typecheck's sibling)
// ---------------------------------------------------------------------------

/// Same coarse-but-memoized shape as `typecheck`, one layer further down the
/// pipeline: `lower_program` depends on BOTH `linked_program` and
/// `typecheck`, so it re-executes exactly when either upstream seam would
/// have re-run `sky_lower::lower` today — but a repeat demand or a no-op
/// re-save now executes nothing.
#[test]
fn lower_program_memoized_coarse_floor() {
    let (mut db, log) = logged_db();
    let a = file(&db, &["A"], DEP_A);
    let b = file(&db, &["B"], IMPORTER_B);
    let root = root_of(&db, &[(&["A"], a), (&["B"], b)]);

    let program = sky_db::lower_program(&db, root, b).expect("trivial int program must lower");
    assert!(
        !program.modules.is_empty(),
        "lowered IR must carry at least one module"
    );
    assert_eq!(log.executions_of("lower_program("), 1);
    // typecheck is a real dependency, demanded fresh on this first call too.
    assert_eq!(log.executions_of("typecheck("), 1);

    // Repeat demand: both memo-hit.
    log.clear();
    assert!(sky_db::lower_program(&db, root, b).is_ok());
    assert_eq!(
        log.executions_of("lower_program("),
        0,
        "repeat demand memoized"
    );
    assert_eq!(log.executions_of("typecheck("), 0);

    // Byte-equal re-save: still nothing executes.
    log.clear();
    assert!(!sky_db::set_text_if_changed(&mut db, a, DEP_A));
    assert!(sky_db::lower_program(&db, root, b).is_ok());
    assert_eq!(log.executions_of("lower_program("), 0);

    // Dep body edit: both typecheck and lower_program re-execute in lockstep
    // (the documented coarse floor — lower_program has no independent
    // invalidation edge finer than typecheck's own).
    log.clear();
    assert!(sky_db::set_text_if_changed(&mut db, a, DEP_A_BODY_EDIT));
    assert!(sky_db::lower_program(&db, root, b).is_ok());
    assert_eq!(log.executions_of("typecheck("), 1);
    assert_eq!(log.executions_of("lower_program("), 1);
}

/// `lower_program` never reaches `sky_lower::lower` on ill-typed input — it
/// short-circuits through `typecheck`'s error via `?`, so a direct demand on
/// a red program surfaces the SAME diagnostic `typecheck` itself would give
/// (never a `sky_lower`-side panic or a different, confusing error).
#[test]
fn lower_program_short_circuits_on_typecheck_error() {
    const RED_ENTRY: &str = "module Entry exposing (e)\n\n\
        e : Int\n\
        e = \"not an int\"\n";
    let (db, _log) = logged_db();
    let entry = file(&db, &["Entry"], RED_ENTRY);
    let root = root_of(&db, &[(&["Entry"], entry)]);

    let typecheck_err = sky_db::typecheck(&db, root, entry)
        .expect_err("annotated Int binding with a String body must be rejected")
        .0;
    let lower_err = sky_db::lower_program(&db, root, entry)
        .expect_err("lower_program must refuse to lower an ill-typed program")
        .0;
    assert_eq!(
        typecheck_err, lower_err,
        "lower_program's short-circuit must surface typecheck's own diagnostic verbatim"
    );
}

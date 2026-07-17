#![forbid(unsafe_code)]
//! Incrementality proofs (spec:
//! `docs/architecture/salsa-incremental-compilation-2026-07-11.md` §3.6).
//!
//! The load-bearing test is `parse_granularity`: two runs where only one
//! input changed re-execute only the affected query, proven via salsa's
//! event log (`EventKind::WillExecute`).

use std::sync::{Arc, Mutex, PoisonError};

use salsa::Setter as _;
use ipe_db::{IpeDatabase, SourceFile, imports, parse, set_text_if_changed};

const MOD_A: &str = "module A exposing (a)\n\na = 1\n";
const MOD_B: &str = "module B exposing (b)\n\nb = 2\n";
const MOD_B2: &str = "module B exposing (b)\n\nb = 3\n";

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
fn logged_db() -> (IpeDatabase, EventLog) {
    let log = EventLog::default();
    let sink = log.clone();
    let db = IpeDatabase::with_event_callback(Box::new(move |event: salsa::Event| {
        if let salsa::EventKind::WillExecute { database_key } = event.kind {
            sink.push(format!("{database_key:?}"));
        }
    }));
    (db, log)
}

fn file(db: &IpeDatabase, path: &[&str], text: &str) -> SourceFile {
    SourceFile::new(
        db,
        path.iter().map(|s| (*s).to_owned()).collect(),
        text.to_owned(),
        ipe_db::ModuleOrigin::User,
    )
}

/// Edit B only: `parse(A)` is a memo hit (zero re-executions), `parse(B)`
/// re-executes exactly once.
#[test]
fn parse_granularity() {
    let (mut db, log) = logged_db();
    let a = file(&db, &["A"], MOD_A);
    let b = file(&db, &["B"], MOD_B);

    // Cold run: both parse queries execute.
    assert!(parse(&db, a).is_ok(), "module A must parse");
    assert!(parse(&db, b).is_ok(), "module B must parse");
    assert_eq!(log.executions_of("parse"), 2, "cold run executes both");

    // Edit B ONLY, then demand both again.
    log.clear();
    b.set_text(&mut db).to(MOD_B2.to_owned());
    assert!(parse(&db, a).is_ok());
    assert!(parse(&db, b).is_ok());

    let total = log.executions_of("parse");
    assert_eq!(
        total, 1,
        "after editing B only, exactly one parse re-executes (got {total})"
    );
}

/// Byte-equal re-save is a no-op at the driver boundary: no set, no
/// re-execution.
#[test]
fn byte_equal_resave_noop() {
    let (mut db, log) = logged_db();
    let a = file(&db, &["A"], MOD_A);
    assert!(parse(&db, a).is_ok());
    assert_eq!(log.executions_of("parse"), 1);

    log.clear();
    assert!(
        !set_text_if_changed(&mut db, a, MOD_A),
        "identical bytes must not set the input"
    );
    assert!(parse(&db, a).is_ok());
    assert_eq!(
        log.executions_of("parse"),
        0,
        "byte-equal re-save must be a memo hit"
    );

    // And a REAL change does set + re-execute.
    assert!(set_text_if_changed(
        &mut db,
        a,
        "module A exposing (a)\n\na = 9\n"
    ));
    assert!(parse(&db, a).is_ok());
    assert_eq!(log.executions_of("parse"), 1);
}

/// Same granularity shape for the `imports` query.
#[test]
fn imports_granularity() {
    let (mut db, log) = logged_db();
    let a = file(&db, &["A"], "module A exposing (a)\nimport B\n\na = 1\n");
    let b = file(&db, &["B"], MOD_B);

    assert_eq!(imports(&db, a).as_ref(), &vec![vec!["B".to_owned()]]);
    assert!(imports(&db, b).is_empty());
    assert_eq!(log.executions_of("imports"), 2);

    // Edit B: A's import scan is untouched.
    log.clear();
    b.set_text(&mut db).to(MOD_B2.to_owned());
    assert_eq!(imports(&db, a).as_ref(), &vec![vec!["B".to_owned()]]);
    assert_eq!(
        log.executions_of("imports"),
        0,
        "editing B must not re-scan A's imports"
    );
    assert!(imports(&db, b).is_empty());
    assert_eq!(log.executions_of("imports"), 1);
}

/// Inputs are independent: mutating B leaves A's stored value untouched.
#[test]
fn inputs_roundtrip() {
    let (mut db, _log) = logged_db();
    let a = file(&db, &["A"], MOD_A);
    let b = file(&db, &["B"], MOD_B);

    assert_eq!(a.text(&db), MOD_A);
    assert_eq!(b.text(&db), MOD_B);
    assert_eq!(a.module_path(&db), &vec!["A".to_owned()]);

    b.set_text(&mut db).to(MOD_B2.to_owned());
    assert_eq!(a.text(&db), MOD_A, "A's text must survive B's edit");
    assert_eq!(b.text(&db), MOD_B2);
}

/// A parse error is a VALUE of the query (total function), memoized like any
/// other result, and re-evaluated on edit.
#[test]
fn parse_error_is_a_value() {
    let (mut db, log) = logged_db();
    let broken = file(&db, &["A"], "module A exposing (a)\n\na = = 1\n");
    assert!(parse(&db, broken).is_err(), "broken module must yield Err");
    // Memoized: demanding again does not re-execute.
    log.clear();
    assert!(parse(&db, broken).is_err());
    assert_eq!(log.executions_of("parse"), 0);

    // Fixing the text re-executes and yields Ok.
    broken.set_text(&mut db).to(MOD_A.to_owned());
    assert!(parse(&db, broken).is_ok());
}

/// Symbol STRING identity is demand-order independent: two databases
/// demanding `parse` in opposite orders resolve every module name to the
/// same strings. (Numeric symbol ids may differ across orders — the one-shot
/// driver's fixed topo demand order is what pins emit determinism; see spec
/// §3.3.)
#[test]
fn demand_order_determinism() {
    // Resolved module-name strings, or empty when the fixture fails to parse
    // (the final assert_eq against the expected names catches that case).
    let resolve_names = |db: &IpeDatabase, f: SourceFile| -> Vec<String> {
        let Ok(module) = parse(db, f) else {
            return Vec::new();
        };
        let interner = ipe_db::Db::interner(db).lock();
        module
            .name
            .value
            .iter()
            .map(|s| interner.resolve(*s).unwrap_or_default().to_owned())
            .collect()
    };

    // Order 1: A then B.
    let db1 = IpeDatabase::new();
    let a1 = file(&db1, &["A"], MOD_A);
    let b1 = file(&db1, &["B"], MOD_B);
    let first_order_a = resolve_names(&db1, a1);
    let first_order_b = resolve_names(&db1, b1);

    // Order 2: B then A.
    let db2 = IpeDatabase::new();
    let a2 = file(&db2, &["A"], MOD_A);
    let b2 = file(&db2, &["B"], MOD_B);
    let second_order_b = resolve_names(&db2, b2);
    let second_order_a = resolve_names(&db2, a2);

    assert_eq!(first_order_a, second_order_a);
    assert_eq!(first_order_b, second_order_b);
    assert_eq!(first_order_a, vec!["A".to_owned()]);
    assert_eq!(first_order_b, vec!["B".to_owned()]);
}

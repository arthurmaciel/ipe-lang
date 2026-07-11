#![forbid(unsafe_code)]
//! Independent adversarial-review probes for the Phase-2 `module_interface`
//! firewall (salsa backdating). These attack the sharpest under-invalidation
//! vectors NOT covered by `phase2_incrementality.rs`:
//!
//! 1. **Constructor reorder** — same ctor names, same arities, only the
//!    positional `index` changes. If `ModuleExports` were name-only, the
//!    interface would compare `PartialEq`-equal, the importer's memo would
//!    backdate, and its canonical AST would keep the STALE ctor indices —
//!    a silent wrong-code bug. `CtorHome { index, arity }` must punch through.
//!
//! 2. **Transitive alias-body edit** — grand-dep C changes a record alias `P`
//!    that dep A re-surfaces through `scope_aliases` inside its own exported
//!    alias `M`. Importer B (which imports ONLY A) expands `M` and therefore
//!    reads `P`'s body out of A's interface. If `scope_aliases` were missing
//!    from the `PartialEq` projection, B would keep a stale expansion.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use salsa::Setter as _;
use sky_db::{ModuleOrigin, SkyDatabase, SourceFile, SourceRoot, canonicalize, module_interface};

#[derive(Clone, Default)]
struct EventLog(Arc<Mutex<Vec<String>>>);

impl EventLog {
    fn push(&self, entry: String) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(entry);
    }

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
    let map: BTreeMap<Vec<String>, SourceFile> = files
        .iter()
        .map(|(path, f)| (path.iter().map(|s| (*s).to_owned()).collect(), *f))
        .collect();
    SourceRoot::new(db, map)
}

const ADT_DEP: &str = "module A exposing (T(..))\n\ntype T = Red | Blue\n";
/// Same names, same arities (all zero) — ONLY the positional indices flip.
const ADT_DEP_REORDERED: &str = "module A exposing (T(..))\n\ntype T = Blue | Red\n";
const ADT_IMPORTER: &str = "module B exposing (f)\n\nimport A exposing (T(..))\n\nf : T -> Int\nf t =\n    case t of\n        Red ->\n            0\n\n        Blue ->\n            1\n";

/// Reordering a dep's constructors keeps every exported NAME identical; only
/// `CtorHome::index` moves. The interface MUST compare unequal (so the
/// importer re-canonicalises and picks up the new indices) — a backdate here
/// would leave stale ctor indices in the importer's memoized canonical AST,
/// which downstream lowering would turn into wrong pattern dispatch.
#[test]
fn ctor_reorder_punches_through_the_firewall() {
    let (mut db, log) = logged_db();
    let a = file(&db, &["A"], ADT_DEP);
    let b = file(&db, &["B"], ADT_IMPORTER);
    let root = root_of(&db, &[(&["A"], a), (&["B"], b)]);

    let before_iface = module_interface(&db, root, a);
    let before_b = canonicalize(&db, root, b);
    assert!(before_b.is_ok(), "importer must canonicalise, got {before_b:?}");

    log.clear();
    a.set_text(&mut db).to(ADT_DEP_REORDERED.to_owned());

    let after_iface = module_interface(&db, root, a);
    assert_ne!(
        before_iface, after_iface,
        "UNDER-INVALIDATION: ctor reorder (same names, same arities) left the \
         module interface PartialEq-equal — importers would keep stale ctor \
         indices in their memoized canonical ASTs"
    );

    let after_b = canonicalize(&db, root, b);
    assert!(after_b.is_ok(), "importer must re-canonicalise green, got {after_b:?}");
    assert_eq!(
        log.executions_of("canonicalize("),
        2,
        "ctor reorder must re-canonicalise BOTH dep and importer"
    );
    assert_ne!(
        before_b, after_b,
        "UNDER-INVALIDATION: the importer's canonical AST must embed the NEW \
         constructor indices after the dep reorder"
    );
}

const GRAND_DEP_C: &str = "module C exposing (P)\n\ntype alias P = { x : Int }\n";
const GRAND_DEP_C_WIDENED: &str = "module C exposing (P)\n\ntype alias P = { x : Int, y : Int }\n";
const MID_DEP_A: &str = "module A exposing (M)\n\nimport C exposing (P)\n\ntype alias M = { p : P }\n";
const GRAND_IMPORTER_B: &str =
    "module B exposing (f)\n\nimport A exposing (M)\n\nf : M -> Int\nf m = m.p.x\n";

/// Grand-dep alias-body edit: C widens `P`; A's own EXPORTED alias `M`'s raw
/// body (`{ p : P }`) is unchanged, so if the interface projection dropped
/// `scope_aliases` (the channel importers use to expand `P` inside `M`),
/// A's interface would backdate and B would keep a stale expansion of `M`.
#[test]
fn transitive_alias_body_edit_reaches_grand_importer() {
    let (mut db, log) = logged_db();
    let c = file(&db, &["C"], GRAND_DEP_C);
    let a = file(&db, &["A"], MID_DEP_A);
    let b = file(&db, &["B"], GRAND_IMPORTER_B);
    let root = root_of(&db, &[(&["A"], a), (&["B"], b), (&["C"], c)]);

    let before_a_iface = module_interface(&db, root, a);
    let warm = canonicalize(&db, root, b);
    assert!(warm.is_ok(), "grand importer must canonicalise, got {warm:?}");

    log.clear();
    c.set_text(&mut db).to(GRAND_DEP_C_WIDENED.to_owned());

    let after_a_iface = module_interface(&db, root, a);
    assert_ne!(
        before_a_iface, after_a_iface,
        "UNDER-INVALIDATION: widening grand-dep C's alias `P` must change A's \
         interface (A re-surfaces `P` via scope_aliases; importers expand it)"
    );

    assert!(canonicalize(&db, root, b).is_ok());
    assert_eq!(
        log.executions_of("canonicalize("),
        3,
        "a grand-dep alias-body edit must re-canonicalise C, A, AND B — the \
         firewall must not cut the transitive scope channel"
    );
}

/// Control for the transitive probe: a PRIVATE body edit in grand-dep C
/// (exports unchanged) must still firewall at C — A and B stay memoized.
#[test]
fn transitive_private_edit_still_firewalls() {
    let (mut db, log) = logged_db();
    let c = file(
        &db,
        &["C"],
        "module C exposing (P)\n\ntype alias P = { x : Int }\n\nhidden = 1\n",
    );
    let a = file(&db, &["A"], MID_DEP_A);
    let b = file(&db, &["B"], GRAND_IMPORTER_B);
    let root = root_of(&db, &[(&["A"], a), (&["B"], b), (&["C"], c)]);

    let warm = canonicalize(&db, root, b);
    assert!(warm.is_ok());

    log.clear();
    c.set_text(&mut db).to(
        "module C exposing (P)\n\ntype alias P = { x : Int }\n\nhidden = 2\n".to_owned(),
    );
    let after = canonicalize(&db, root, b);
    assert_eq!(
        log.executions_of("canonicalize("),
        1,
        "a private edit in the grand-dep must re-canonicalise ONLY the grand-dep"
    );
    assert_eq!(warm, after, "grand importer's value must be byte-stable");
}

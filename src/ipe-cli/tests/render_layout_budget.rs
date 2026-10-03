//! The layout-cost bound over the golden corpus: every function body of every
//! lowerable `tests/golden/*/Main.ipe` lays out within an eighth of the
//! renderer's fuel, never falling back to its plain layout.
//!
//! Each entry is lowered through the build's front-end seam (compiled-source
//! stdlib injection, the salsa source root, `ipe_db::lower_program`) and each
//! body is measured by the backend's layout-budget seam
//! ([`ipe_backend_rust::body_layout_budgets`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ipe_db::Db;

/// The fixtures whose bodies once ran the fuel out; each must lower and be measured.
const NAMED_FIXTURES: [&str; 2] = ["db_store_rename_column", "analytics_store_gate"];

/// The `ipe-lang` workspace root (two levels up from this crate's manifest).
fn repo_root() -> PathBuf {
    let joined = e2e_support::manifest_dir!().join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Every golden fixture directory that carries a `Main.ipe` entry, sorted.
fn golden_entries() -> Vec<PathBuf> {
    let golden = repo_root().join("tests").join("golden");
    let mut entries = Vec::new();
    let Ok(read) = std::fs::read_dir(&golden) else {
        return entries;
    };
    for dir in read.flatten() {
        let entry = dir.path().join("Main.ipe");
        if entry.is_file() {
            entries.push(entry);
        }
    }
    entries.sort();
    entries
}

/// The fixture directory name of `entry`.
fn fixture_name(entry: &Path) -> String {
    entry
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The layout budget of every body in `entry`, or the reason none was measured.
fn measure(entry: &Path) -> Result<Vec<ipe_backend_rust::BodyLayoutBudget>, String> {
    let main = vec!["Main".to_owned()];
    let src = std::fs::read_to_string(entry).map_err(|e| format!("read: {e}"))?;
    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    sources.insert(main.clone(), (entry.to_path_buf(), src));
    let mut discovered = Vec::new();
    let injected = ipe::project::inject_compiled_std_closure(&mut sources, &mut discovered);
    let db = ipe_db::IpeDatabase::new();
    let root = ipe::create_source_root(&db, &sources, &injected, &BTreeSet::new());
    let entry_file = root
        .files(&db)
        .get(&main)
        .copied()
        .ok_or_else(|| "the entry module is absent from the source root".to_owned())?;
    let program = ipe_db::lower_program(&db, root, entry_file)
        .clone()
        .map_err(|(diag, _)| format!("lower: {diag:?}"))?;
    ipe_backend_rust::body_layout_budgets(&db.interner().lock(), &program)
        .map_err(|e| format!("emit: {e:?}"))
}

#[test]
fn golden_bodies_render_within_fuel_fraction() {
    let entries = golden_entries();
    assert!(
        !entries.is_empty(),
        "no Main.ipe fixtures under tests/golden"
    );
    let ceiling = ipe_backend_rust::LAYOUT_FUEL >> 3;
    let mut measured_bodies = 0usize;
    let mut unmeasured = 0usize;
    let mut named_measured = [0usize; NAMED_FIXTURES.len()];
    let mut over: Vec<String> = Vec::new();
    for entry in &entries {
        let fixture = fixture_name(entry);
        let named = NAMED_FIXTURES.iter().position(|n| *n == fixture);
        let budgets = match measure(entry) {
            Ok(budgets) => budgets,
            Err(reason) => {
                assert!(
                    named.is_none(),
                    "{fixture} must lower and emit to be measured: {reason}"
                );
                // A fixture the pipeline turns away (an expected-error golden, a
                // multi-package shape) has no body a build would lay out.
                unmeasured += 1;
                continue;
            }
        };
        if let Some(slot) = named.and_then(|i| named_measured.get_mut(i)) {
            *slot += budgets.len();
        }
        measured_bodies += budgets.len();
        for b in budgets {
            if b.exhausted || b.spent > ceiling {
                over.push(format!(
                    "{fixture} :: fn {} spent {} (ceiling {ceiling}, exhausted {})",
                    b.func, b.spent, b.exhausted
                ));
            }
        }
    }
    eprintln!(
        "layout budget: {measured_bodies} bodies measured, {unmeasured} fixtures unmeasured, \
         {} over the ceiling",
        over.len()
    );
    assert!(measured_bodies > 0, "no function body was measured");
    for (name, count) in NAMED_FIXTURES.iter().zip(named_measured) {
        assert!(count > 0, "{name}: no function body was measured");
    }
    assert!(
        over.is_empty(),
        "function bodies past an eighth of the layout fuel:\n{}",
        over.join("\n")
    );
}

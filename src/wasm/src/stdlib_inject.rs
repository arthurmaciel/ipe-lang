//! The in-memory embedded-stdlib injection closure.
//!
//! A faithful in-memory port of the native driver's
//! `ipe::project::inject_compiled_std_closure` (the pure part — the native
//! version additionally maintains a `DiscoveredModule` list this crate has no
//! use for). It fixpoints over the compiled-source stdlib import graph, pulling
//! each transitively-imported `Ipe.*` compiled-source module's embedded text
//! into `sources`. No filesystem: the module bodies come from [`ipe_stdlib`]'s
//! `include_str!` table, and imports are found by a token scan
//! ([`ipe_db::extract_imports_from_source`]).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;

/// Inject the transitive compiled-source stdlib closure into `sources`.
///
/// Returns the set of module paths that were injected — the ONLY inputs that
/// earn `ModuleOrigin::EmbeddedStdlib` trust. A module already present in
/// `sources` (a user file, or an already-injected node) is never re-inserted
/// and never tagged: a user file squatting on an `Ipe.*` name stays `User`.
pub fn inject_compiled_std_closure(
    sources: &mut BTreeMap<Vec<String>, (PathBuf, String)>,
) -> BTreeSet<Vec<String>> {
    let mut injected: BTreeSet<Vec<String>> = BTreeSet::new();

    // Seed the worklist from every compiled-source import across current
    // sources. An unused-stdlib program enqueues nothing and returns empty.
    let mut work: VecDeque<Vec<String>> = VecDeque::new();
    for (_, src) in sources.values() {
        for imp in ipe_db::extract_imports_from_source(src) {
            if ipe_stdlib::is_compiled_source_segments(&imp) {
                work.push_back(imp);
            }
        }
    }

    while let Some(path) = work.pop_front() {
        // Already present — a user file OR an already-injected node. Skip; do
        // NOT tag trusted (BTreeMap key = free dedup; user-squat stays User).
        if sources.contains_key(&path) {
            continue;
        }
        let Some(embedded) = ipe_stdlib::compiled_std_source_segments(&path) else {
            // Not a compiled-source module (e.g. a kernel import like
            // `Ipe.String` inside an embedded source): leave it
            // kernel-resolved.
            continue;
        };

        // Synthetic on-disk-looking path, for diagnostics only — never read.
        let synth_path = PathBuf::from("<embedded-stdlib>").join(path.join("."));
        sources.insert(path.clone(), (synth_path, embedded.to_owned()));
        injected.insert(path.clone());

        // Std → Std closure: enqueue the embedded module's OWN compiled-source
        // imports. Fixpoint via the `sources.contains_key` guard above.
        for imp in ipe_db::extract_imports_from_source(embedded) {
            if ipe_stdlib::is_compiled_source_segments(&imp) && !sources.contains_key(&imp) {
                work.push_back(imp);
            }
        }
    }

    injected
}

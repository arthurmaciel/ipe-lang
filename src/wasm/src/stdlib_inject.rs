//! The in-memory embedded-stdlib injection closure.
//!
//! A thin adapter over the shared closure `ipe_stdlib::inject_compiled_std_closure`
//! — the single source of truth for the injection algorithm and its squat-guard,
//! called by both the native CLI driver and this WebAssembly frontend so their
//! trust sets can never drift. The native driver additionally records a
//! `DiscoveredModule` per injected node; this crate has no use for that, so it
//! passes a no-op callback. Imports are found by a token scan
//! ([`ipe_db::extract_imports_from_source`]); the module bodies come from
//! [`ipe_stdlib`]'s `include_str!` table.

use std::collections::{BTreeMap, BTreeSet};
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
    ipe_stdlib::inject_compiled_std_closure(
        sources,
        ipe_db::extract_imports_from_source,
        |_module_path, _synth_path| {},
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wasm wrapper and the shared `ipe_stdlib` core must inject the exact
    /// same trust set for the same sources — the equivalence that keeps the two
    /// frontends from drifting (the whole point of the single-home extraction).
    /// The callback the native driver uses to build its `DiscoveredModule` list
    /// must see exactly the injected paths, no more, no less.
    #[test]
    fn wrapper_matches_shared_core_and_callback_sees_every_injection() {
        let seed = |sources: &mut BTreeMap<Vec<String>, (PathBuf, String)>| {
            sources.insert(
                vec!["Main".to_owned()],
                (
                    PathBuf::from("src/Main.ipe"),
                    "module Main exposing (main)\nimport Ipe.Palette exposing (..)\nmain = 0\n"
                        .to_owned(),
                ),
            );
        };

        let mut via_wrapper: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
        seed(&mut via_wrapper);
        let wrapper_set = inject_compiled_std_closure(&mut via_wrapper);

        let mut via_core: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
        seed(&mut via_core);
        let mut observed: BTreeSet<Vec<String>> = BTreeSet::new();
        let core_set = ipe_stdlib::inject_compiled_std_closure(
            &mut via_core,
            ipe_db::extract_imports_from_source,
            |module_path, _synth| {
                observed.insert(module_path.to_vec());
            },
        );

        assert_eq!(
            wrapper_set, core_set,
            "wrapper and core inject the same set"
        );
        assert_eq!(
            observed, core_set,
            "callback saw exactly the injected paths"
        );
        assert!(
            core_set.contains(&vec!["Ipe".to_owned(), "Palette".to_owned()]),
            "Ipe.Palette must be injected"
        );
        assert_eq!(via_wrapper, via_core, "both paths leave identical sources");
    }

    /// A user file squatting on an `Ipe.*` key is never overwritten and never
    /// tagged trusted — the shared squat-guard, exercised through the wrapper.
    #[test]
    fn user_squat_is_not_injected_or_trusted() {
        let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
        sources.insert(
            vec!["Main".to_owned()],
            (
                PathBuf::from("src/Main.ipe"),
                "module Main exposing (main)\nimport Ipe.Palette exposing (..)\nmain = 0\n"
                    .to_owned(),
            ),
        );
        let palette = vec!["Ipe".to_owned(), "Palette".to_owned()];
        sources.insert(
            palette.clone(),
            (
                PathBuf::from("src/Ipe/Palette.ipe"),
                "module Ipe.Palette exposing (..)\ntoHex = 0\n".to_owned(),
            ),
        );

        let injected = inject_compiled_std_closure(&mut sources);
        assert!(
            !injected.contains(&palette),
            "a user squat must NOT be tagged trusted"
        );
        assert_eq!(
            sources.get(&palette).map(|(p, _)| p.clone()),
            Some(PathBuf::from("src/Ipe/Palette.ipe")),
            "the user's file is left in place, not overwritten by the embed"
        );
    }
}

//! A qualifier naming a known but unimported module asks for its import
//! (IPE-N0034), and the did-you-mean list never offers a qualifier the module
//! could not use.

use std::collections::BTreeMap;

use ipe_canon::{ModuleCatalog, ModuleExports, ModuleOrigin, canonicalise_module_in_project};
use ipe_diagnostics::{Diagnostic, NameError};
use ipe_intern::{Interner, Symbol};

const UTIL: &str = "module Lib.Util exposing (..)\n\nf : Int -> Int\nf n =\n    n\n";

/// Canonicalise `sources` in order against `catalog`, each seeing the exports
/// of every module before it; the last module's error, if any.
fn last_error(sources: &[&str], catalog: &[&str]) -> Option<Diagnostic> {
    let catalog = ModuleCatalog::new(catalog.iter().map(|m| Box::<str>::from(*m)));
    let mut interner = Interner::new();
    let mut deps: BTreeMap<Vec<Symbol>, ModuleExports> = BTreeMap::new();
    let mut last = None;
    for src in sources {
        let result = ipe_parse::parse_module(src, &mut interner).and_then(|parsed| {
            let expected = parsed.name.value.clone();
            let borrowed: BTreeMap<Vec<Symbol>, &ModuleExports> =
                deps.iter().map(|(k, v)| (k.clone(), v)).collect();
            canonicalise_module_in_project(
                &parsed,
                &expected,
                &borrowed,
                &catalog,
                ModuleOrigin::User,
                &mut interner,
            )
        });
        match result {
            Ok((_, exports)) => {
                deps.insert(exports.path.clone(), exports);
                last = None;
            }
            Err(diag) => {
                last = Some(diag);
                break;
            }
        }
    }
    last
}

/// The candidates of an IPE-N0034 diagnostic, or `None` for any other.
fn import_candidates(diag: &Diagnostic) -> Option<Vec<&str>> {
    match diag {
        Diagnostic::Name {
            msg: NameError::ImportRequired { candidates, .. },
            ..
        } => Some(candidates.iter().map(|c| &**c).collect()),
        _ => None,
    }
}

/// The did-you-mean names of an unknown-module diagnostic, or `None` for any other.
fn unknown_module_suggestions(diag: &Diagnostic) -> Option<Vec<&str>> {
    match diag {
        Diagnostic::Name {
            msg: NameError::UnknownModule { suggestions, .. },
            ..
        } => Some(suggestions.names.iter().map(|c| &**c).collect()),
        _ => None,
    }
}

#[test]
fn unimported_compiled_std_qualifier_is_n0034() {
    let src =
        "module Main exposing (main)\n\nmain : List Int\nmain =\n    List.map identity [ 1 ]\n";
    let diag = last_error(&[src], &["Ipe.List"]);
    assert!(diag.is_some(), "expected an import-required diagnostic");
    let Some(diag) = diag else {
        return;
    };
    assert_eq!(diag.code().as_str(), "IPE-N0034", "{diag:?}");
    assert_eq!(import_candidates(&diag), Some(vec!["Ipe.List"]), "{diag:?}");
}

#[test]
fn unimported_compiled_std_type_is_n0034() {
    let src = "module Main exposing (x)\n\nx : Dict.Dict String Int -> Int\nx d =\n    1\n";
    let diag = last_error(&[src], &["Ipe.Dict"]);
    assert!(diag.is_some(), "expected an import-required diagnostic");
    let Some(diag) = diag else {
        return;
    };
    assert_eq!(diag.code().as_str(), "IPE-N0034", "{diag:?}");
    assert!(
        import_candidates(&diag).is_some_and(|c| c.contains(&"Ipe.Dict")),
        "{diag:?}"
    );
}

#[test]
fn unimported_project_module_is_n0034() {
    let src = "module Main exposing (main)\n\nmain : Int\nmain =\n    Util.f 1\n";
    let diag = last_error(&[src], &["Main", "Util"]);
    assert!(diag.is_some(), "expected an import-required diagnostic");
    let Some(diag) = diag else {
        return;
    };
    assert_eq!(diag.code().as_str(), "IPE-N0034", "{diag:?}");
    assert_eq!(import_candidates(&diag), Some(vec!["Util"]), "{diag:?}");
}

#[test]
fn two_catalog_modules_same_last_segment_list_both() {
    let src = "module Main exposing (main)\n\nmain : Int\nmain =\n    Util.f 1\n";
    let diag = last_error(&[src], &["Main", "Lib.Util", "App.Util"]);
    assert!(diag.is_some(), "expected an import-required diagnostic");
    let Some(diag) = diag else {
        return;
    };
    assert_eq!(diag.code().as_str(), "IPE-N0034", "{diag:?}");
    assert_eq!(
        import_candidates(&diag),
        Some(vec!["App.Util", "Lib.Util"]),
        "{diag:?}"
    );
}

/// `Hosts` is one edit from the gated kernel qualifier `Host`, and `Lsit` one
/// transposition from the unimported `List`: neither is offered.
#[test]
fn gated_unimported_qualifier_never_suggested() {
    for (expr, absent) in [("Hosts.name", "Host"), ("Lsit.map identity [ 1 ]", "List")] {
        let src = format!("module Main exposing (main)\n\nmain =\n    {expr}\n");
        let diag = last_error(&[src.as_str()], &["Main", "Ipe.List"]);
        assert!(
            diag.is_some(),
            "expected an unknown-module diagnostic for {expr}"
        );
        let Some(diag) = diag else {
            return;
        };
        let names = unknown_module_suggestions(&diag);
        assert!(
            names.is_some(),
            "expected IPE-N0004 for {expr}, got {diag:?}"
        );
        let Some(names) = names else {
            return;
        };
        assert!(!names.contains(&absent), "{expr}: {names:?}");
        assert!(!names.contains(&"Host"), "{expr}: {names:?}");
    }
}

/// The usable-qualifier filter keeps an imported module: a typo of it is still
/// suggested.
#[test]
fn imported_qualifier_still_suggested() {
    let src =
        "module Main exposing (main)\n\nimport Lib.Util\n\nmain : Int\nmain =\n    Utli.f 1\n";
    let diag = last_error(&[UTIL, src], &["Main", "Lib.Util"]);
    assert!(diag.is_some(), "expected an unknown-module diagnostic");
    let Some(diag) = diag else {
        return;
    };
    let names = unknown_module_suggestions(&diag);
    assert!(names.is_some(), "expected IPE-N0004, got {diag:?}");
    let Some(names) = names else {
        return;
    };
    assert!(names.contains(&"Util"), "{names:?}");
}
